//! Host utility: run the actual boot image validator against a generated app.
//! Never copies image parsing or chip memory-map constants.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
fn main() {
    use iobewi_firmware_image::esp::{self, Read, Verify};
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("image.bin required");
    let base = u32::from_str_radix(
        args.next()
            .as_deref()
            .unwrap_or("20000")
            .trim_start_matches("0x"),
        16,
    )
    .expect("hex slot offset");
    let capacity = u32::from_str_radix(
        args.next()
            .as_deref()
            .unwrap_or("180000")
            .trim_start_matches("0x"),
        16,
    )
    .expect("hex slot size");
    struct Image {
        base: u32,
        bytes: std::vec::Vec<u8>,
    }
    impl Read for Image {
        fn read(&mut self, offset: u32, out: &mut [u8]) -> Result<(), ()> {
            let at = offset.checked_sub(self.base).ok_or(())? as usize;
            let end = at.checked_add(out.len()).ok_or(())?;
            out.copy_from_slice(self.bytes.get(at..end).ok_or(())?);
            Ok(())
        }
    }
    let mut flash = Image {
        base,
        bytes: std::fs::read(path).expect("read image"),
    };
    assert!(flash.bytes.len() <= capacity as usize, "image exceeds slot");
    for verify in [Verify::Structure, Verify::Full] {
        let image = esp::validate(
            &mut flash,
            base,
            capacity,
            &iobewi_esp_boot::MEMORY_MAP,
            verify,
        )
        .expect("image refused by boot validator");
        println!(
            "Image accepted: {} segments, entry 0x{:08x}",
            image.count, image.entry
        );
    }
}
#[cfg(not(any(feature = "esp32c3", feature = "esp32s3")))]
fn main() {
    eprintln!("run on host with --features esp32c3 or esp32s3");
}
