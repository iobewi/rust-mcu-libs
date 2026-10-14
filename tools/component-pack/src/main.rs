//! `iobewi-workload-pack <workload.elf> -o <out.iwni> [--api 1.0] [test overrides...]`
use iobewi_firmware_update::compat::otm2::RuntimeApi;
use iobewi_workload_image::{HEADER_LEN, ImageHeader, esp32s3_layout};
use iobewi_workload_pack::{Overrides, PackOptions, pack};
use sha2::{Digest, Sha256};

fn parse_hex_or_dec(s: &str) -> u32 {
    s.strip_prefix("0x").map_or_else(
        || s.parse().unwrap(),
        |h| u32::from_str_radix(h, 16).unwrap(),
    )
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None;
    let mut output = None;
    let mut api = RuntimeApi::new(1, 0);
    let mut o = Overrides::default();
    let mut inspect = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" => output = args.next(),
            "--api" => {
                let v = args.next().expect("--api MAJOR.MINOR");
                let (ma, mi) = v.split_once('.').expect("MAJOR.MINOR");
                api = RuntimeApi::new(ma.parse().unwrap(), mi.parse().unwrap());
            }
            "--inspect" => inspect = true,
            // Test aids: hostile headers. Never used by a normal build.
            "--force-target" => o.target = Some(parse_hex_or_dec(&args.next().unwrap()) as u16),
            "--force-abi" => o.abi_version = Some(parse_hex_or_dec(&args.next().unwrap()) as u16),
            "--force-entry" => o.entry = Some(parse_hex_or_dec(&args.next().unwrap())),
            "--force-format" => {
                o.format_version = Some(parse_hex_or_dec(&args.next().unwrap()) as u16)
            }
            "--force-magic" => {
                let m = args.next().unwrap();
                o.magic = Some(m.as_bytes()[..4].try_into().unwrap());
            }
            "--force-code-addr" => o.code_addr = Some(parse_hex_or_dec(&args.next().unwrap())),
            other => input = Some(other.to_string()),
        }
    }
    let input = input.expect("usage: iobewi-workload-pack <elf> -o <out> [--api M.m]");
    let elf = std::fs::read(&input).expect("read elf");
    let layout = esp32s3_layout(api, iobewi_workload_abi::ABI_VERSION);
    let packed = pack(
        &elf,
        &PackOptions {
            layout,
            requires: api,
            overrides: o,
        },
    )
    .unwrap_or_else(|e| {
        eprintln!("pack failed: {e:?}");
        std::process::exit(1);
    });
    let h: &ImageHeader = &packed.header;
    let digest = Sha256::digest(&packed.bytes);
    println!(
        "header={HEADER_LEN} code={} rodata+data={} bss={} image={} entry={:#x} requires={}.{} sha256:{digest:x}",
        h.code_size,
        h.data_size,
        h.bss_size,
        packed.bytes.len(),
        h.entry,
        h.requires.major,
        h.requires.minor
    );
    if !inspect {
        let out = output.expect("-o <out>");
        std::fs::write(&out, &packed.bytes).expect("write image");
    }
}
