//! Host-only fixture for an explicit initial flash of the reference scenario.
//! Uses the real boot codec; never runs in the firmware or erases configuration.
fn main() {
    use iobewi_firmware_boot::{Entry, state};
    let mut args = std::env::args().skip(1);
    let output = args.next().expect("output file required");
    let slot: u8 = args
        .next()
        .expect("initial slot 0 or 1 required")
        .parse()
        .expect("slot number");
    assert!(slot <= 1 && args.next().is_none(), "choose slot 0 or 1");
    let mut bytes = [0xff; 0x2000];
    let entry = Entry::new(u32::from(slot) + 1, state::VALID).encode();
    bytes[..entry.len()].copy_from_slice(&entry);
    std::fs::write(output, bytes).expect("write initial confirmed boot metadata");
}
