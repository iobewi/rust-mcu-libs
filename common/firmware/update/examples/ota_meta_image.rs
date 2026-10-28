//! Writes the factory image of an `ota_meta` partition: copy 0 holds an OTSF
//! frame carrying the empty OTM1 record produced by the real codec, copy 1
//! stays erased. Usage: `ota_meta_image <out.bin> [erase_size] [region_size]`.
use iobewi_firmware_update::frame_store::provisioned_image;
use iobewi_firmware_update::metadata::Metadata;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .expect("usage: ota_meta_image <out.bin> [erase_size] [region_size]");
    let erase: usize = args.next().map_or(4096, |v| v.parse().expect("erase_size"));
    let region: usize = args
        .next()
        .map_or(2 * erase, |v| v.parse().expect("region_size"));
    assert_eq!(region, 2 * erase, "ota_meta is exactly two erase units");
    let payload = Metadata::default().encode().expect("empty OTM1");
    let image = provisioned_image(erase, &payload).expect("frame fits an erase unit");
    std::fs::write(&out, image).expect("write image");
}
