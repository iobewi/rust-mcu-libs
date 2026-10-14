#!/usr/bin/env python3
"""Compare frozen historical boot behavior and OTM2 bytes without copying engines.

Usage: python3 docs/firmware/check_parity.py /path/to/iobewi_old
Requires rustc, cargo and the unmodified historical Git checkout below. Builds only
scratch reference libraries and the current portable update crate; no MCU builds.
"""
import ast
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

BASELINE = '345378ed1ae11a4814c32ee513f20438291cfba7'
ROOT = Path(__file__).resolve().parents[2]
RUSTC = os.environ.get('RUSTC', 'rustc')

BOOT_HARNESS = r'''
extern crate boot;
use boot::*;
fn transition(write: Option<Write>) {
    match write {
        Some(w) => println!("write {} {:?}", w.sector, w.entry.encode()),
        None => println!("none"),
    }
}
fn main() {
    let mut cases = vec![BLANK, [0; ENTRY_SIZE]];
    for seq in [1, 2, 3, 4, u32::MAX - 1, u32::MAX] {
        for state in 0..=4 {
            let entry = Entry::new(seq, state);
            println!("entry {:?} {:?}", entry.body(), entry.encode());
            cases.push(entry.encode());
        }
    }
    for a in &cases { for b in &cases {
        let records = [*a, *b];
        println!("target {:?}", update_target(records, 2));
        transition(confirm(records)); transition(reject(records));
        for target in 0..2 {
            match activate(records, 2, target) {
                Ok(w) => transition(Some(w)),
                Err(ActivateError::NoValidBase) => println!("no-valid-base"),
                Err(ActivateError::SequenceExhausted) => println!("sequence-exhausted"),
            }
        }
        for mask in 0..4 {
            let plan = plan_boot(records, 2, &mut |slot| mask & (1 << slot) != 0);
            match plan.boot {
                Boot::Slot { slot, sector, seq } => println!("boot {slot} {sector} {seq}"),
                Boot::Halt(Halt::NoImage) => println!("no-image"),
                Boot::Halt(Halt::NoUsableEntry) => println!("no-usable-entry"),
            }
            for w in plan.writes() { transition(Some(w)); }
            println!("end-plan");
        }
    }}
}
'''

OTM2_HARNESS = r'''
use codec::{Record, SlotMeta, State};
fn main() {
    for seq in [0, 1, 7, u32::MAX] {
        for state in [State::Empty, State::Valid, State::Staged, State::Activating,
            State::PendingConfirmation, State::RollingBack] {
            let mut record = Record::empty(seq);
            record.state = state;
            record.meta[0] = SlotMeta::new("old-system", "1.2.3", [0x11; 32], 4096, RuntimeApi::new(1, 2)).unwrap();
            record.meta[1] = SlotMeta::new("new-system", "4.5.6", [0x22; 32], 8192, RuntimeApi::new(1, 3)).unwrap();
            match state {
                State::Empty => record.meta = [SlotMeta::EMPTY; 2],
                State::Valid => record.active = Some(Side::A),
                State::Staged | State::Activating => {
                    record.active = Some(Side::A); record.candidate = Some(Side::B);
                }
                State::PendingConfirmation => {
                    record.active = Some(Side::B); record.previous_valid = Some(Side::A);
                }
                State::RollingBack => {
                    record.active = Some(Side::B); record.previous_valid = Some(Side::A);
                }
            }
            let bytes = record.encode();
            println!("{:?}", bytes);
            println!("{:?}", codec::Record::decode(&bytes));
        }
    }
}
'''

OTM1_HARNESS = r'''
extern crate alloc;
extern crate iobewi_firmware_update;
pub use iobewi_firmware_update::Digest;
mod artifact { pub use iobewi_firmware_update::Digest; }
mod error { pub use iobewi_firmware_update::Error; }
pub use state::{Action, BackendOutcome, TransactionState};
pub use storage::TransactionMetadata;
pub use transaction::{ArtifactRecord, TransactionRecord, reconcile};
pub use iobewi_firmware_update::Error;
use codec::{Metadata, Staged, Stage};
fn main() {
    for stage in [Stage::None, Stage::Written, Stage::Activating] {
        for size in [0, 1, 4096, u32::MAX] {
            for deployment in ["deployment-1", "déploiement-2"] {
                let metadata = Metadata {
                    staged: Staged { stage, slot: "ota_1".into(), digest: "sha256:012345".into(),
                        deployment_id: deployment.into(), size },
                    active_digest: "sha256:987654".into(), active_deployment_id: "previous".into(),
                };
                let raw = metadata.encode().unwrap();
                println!("{:?}", raw);
                let read = Metadata::decode(&raw).unwrap();
                println!("{} {} {} {} {} {} {}", read.staged.stage.as_str(), read.staged.slot,
                    read.staged.digest, read.staged.deployment_id, read.staged.size,
                    read.active_digest, read.active_deployment_id);
            }
        }
    }
}
'''

def run(*args, **kwargs):
    return subprocess.check_output(args, **kwargs)

def main():
    old = Path(sys.argv[1]).resolve()
    if run('git', '-C', str(old), 'rev-parse', 'HEAD', text=True).strip() != BASELINE:
        sys.exit('Historical checkout must be at ' + BASELINE)
    subprocess.run(['git', '-C', str(old), 'diff', '--quiet', 'HEAD'], check=True)
    with tempfile.TemporaryDirectory(prefix='firmware-parity-') as directory:
        tmp = Path(directory)
        outputs = {}
        build = run('cargo', 'build', '-p', 'iobewi-firmware-update', '--no-default-features',
                    '--message-format=json', cwd=ROOT, text=True)
        update_lib = None
        image_lib = None
        for line in build.splitlines():
            event = json.loads(line)
            if event.get('reason') == 'compiler-artifact' and event.get('target', {}).get('name') == 'iobewi_firmware_update':
                update_lib = next((Path(f) for f in event['filenames'] if f.endswith('.rlib')), None)
            if event.get('reason') == 'compiler-artifact' and event.get('target', {}).get('name') == 'iobewi_firmware_image':
                image_lib = next((Path(f) for f in event['filenames'] if f.endswith('.rlib')), None)
        if update_lib is None or image_lib is None:
            sys.exit('Portable update rlib not found')
        model = tmp / 'libiobewi_update_model.rlib'
        subprocess.run([RUSTC, '--edition=2024', '--crate-name', 'iobewi_update_model', '--crate-type=lib',
                        str(old / 'firmware/model/src/lib.rs'), '-o', str(model)], check=True)
        for name, root, boot_path, codec_path in [
            ('old', old, 'firmware/boot/src/lib.rs', 'workload/update/src/otm2.rs'),
            ('new', ROOT, 'common/firmware/boot/src/lib.rs', 'common/firmware/update/src/compat/otm2.rs'),
        ]:
            lib = tmp / ('libboot_' + name + '.rlib')
            subprocess.run([RUSTC, '--edition=2024', '--crate-name', 'boot', '--crate-type=lib',
                            str(root / boot_path), '-o', str(lib)], check=True)
            for kind, source, extern in [
                ('boot', BOOT_HARNESS, ['--extern', 'boot=' + str(lib)]),
                ('otm2', '#[path = "' + str(root / codec_path) + '"] mod codec;\n' +
                 ('use iobewi_update_model::{RuntimeApi, Side};\n' if name == 'old' else 'use codec::{RuntimeApi, Side};\n') +
                 OTM2_HARNESS, ['--extern', 'iobewi_update_model=' + str(model)]),
                ('otm1', ''.join('#[path = "' + str(root / (("firmware/update/src/" if name == "old" else "common/firmware/update/src/") + module + '.rs')) + '"] mod ' + module + ';\n' for module in ['state', 'storage', 'transaction']) + '#[path = "' + str(root / ('firmware/update/src/metadata.rs' if name == 'old' else
                    'common/firmware/update/src/compat/otm1.rs')) + '"] mod codec;\n' + OTM1_HARNESS,
                 ['--extern', 'iobewi_firmware_update=' + str(update_lib), '--extern', 'iobewi_firmware_image=' + str(image_lib), '-L', 'dependency=' + str(update_lib.parent / 'deps')]),
            ]:
                harness = tmp / (kind + '_' + name + '.rs')
                binary = tmp / (kind + '_' + name)
                harness.write_text(source)
                subprocess.run([RUSTC, '--edition=2024', '-A', 'dead_code', str(harness), *extern,
                                '-o', str(binary)], check=True)
                outputs[kind, name] = run(str(binary))
        for kind in ['boot', 'otm1', 'otm2']:
            before, after = outputs[kind, 'old'], outputs[kind, 'new']
            if before != after:
                sys.exit(kind + ': historical/current mismatch')
            print(kind + ': identical; sha256=' + hashlib.sha256(after).hexdigest())
        if '--write-fixtures' in sys.argv[2:]:
            fixtures = ROOT / 'common/firmware/update/tests/fixtures'
            fixtures.mkdir(parents=True, exist_ok=True)
            for kind in ['otm1', 'otm2']:
                records = [bytes(ast.literal_eval(line)) for line in outputs[kind, 'old'].decode().splitlines() if line.startswith('[')]
                framed = b''.join(len(record).to_bytes(2, 'little') + record for record in records)
                (fixtures / (kind + '.bin')).write_bytes(framed)
                print(kind + ': wrote ' + str(len(records)) + ' frozen historical fixtures')
        print('boot: 30 record vectors, 1024 metadata pairs, 4096 image-validation scenarios')
        print('otm1: 24 record vectors across 3 stages, size boundaries and UTF-8 identities')
        print('otm2: 24 record vectors across all 6 states and sequence boundaries')

if __name__ == '__main__':
    main()
