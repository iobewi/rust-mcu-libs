use std::{env, fs, path::PathBuf};

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    // The memory map is a SoC fact: the linker scripts live with the arch code
    // (`arch/esp32/firmware/boot/linker`), the bootloader only selects one.
    let linker = if std::env::var_os("CARGO_FEATURE_ESP32C3").is_some() {
        "iobewi-esp-boot-esp32c3.x"
    } else if std::env::var_os("CARGO_FEATURE_ESP32S3").is_some() {
        "iobewi-esp-boot-esp32s3.x"
    } else {
        panic!("no supported ESP boot target feature selected");
    };

    let search = format!("{dir}/../../../arch/esp32/firmware/boot/linker");
    println!("cargo:rustc-link-search={search}");
    println!("cargo:rustc-link-arg=-T{linker}");
    println!("cargo:rerun-if-changed={search}/{linker}");

    verified_boot_config();
}

/// What a verified bootloader trusts is fixed when it is built: the SHA-256 of each pinned key
/// encoding (1 to 3), the product tag, the allowed hardware variants and a minimum security
/// version. Nothing has a default: a verified bootloader that trusts nothing, or that was
/// configured by accident, must not build.
fn verified_boot_config() {
    for name in [
        "IOBEWI_BOOT_PINNED_KEYS",
        "IOBEWI_BOOT_PRODUCT",
        "IOBEWI_BOOT_VARIANTS",
        "IOBEWI_BOOT_MIN_SECURITY_VERSION",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    if env::var_os("CARGO_FEATURE_VERIFIED_BOOT").is_none() {
        return;
    }
    let need = |name: &str| {
        env::var(name).unwrap_or_else(|_| panic!("`verified-boot` needs {name} at build time"))
    };
    let pins: Vec<[u8; 32]> = need("IOBEWI_BOOT_PINNED_KEYS")
        .split(',')
        .map(|hex| {
            let hex = hex.trim();
            assert!(
                hex.len() == 64,
                "a pin is 64 hex digits (SHA-256 of the key encoding)"
            );
            let mut pin = [0u8; 32];
            for (i, byte) in pin.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("pin is hexadecimal");
            }
            assert!(
                pin != [0u8; 32],
                "an all-zero pin is a placeholder, not a key"
            );
            pin
        })
        .collect();
    assert!((1..=3).contains(&pins.len()), "pin 1 to 3 keys");
    let product = need("IOBEWI_BOOT_PRODUCT");
    assert!(
        (1..=16).contains(&product.len()) && product.bytes().all(|b| (0x21..=0x7e).contains(&b)),
        "product tag: 1 to 16 printable ASCII characters"
    );
    let variants: Vec<u16> = need("IOBEWI_BOOT_VARIANTS")
        .split(',')
        .map(|v| v.trim().parse().expect("variants are decimal u16 values"))
        .collect();
    assert!(
        (1..=4).contains(&variants.len()),
        "allow 1 to 4 hardware variants"
    );
    let min_security: u32 = env::var("IOBEWI_BOOT_MIN_SECURITY_VERSION")
        .map(|v| {
            v.trim()
                .parse()
                .expect("minimum security version is a decimal u32")
        })
        .unwrap_or(0);

    let pins_src: String = pins
        .iter()
        .map(|p| format!("{:?},", p))
        .collect::<Vec<_>>()
        .join("");
    let variants_src: String = variants.iter().map(|v| format!("{v},")).collect();
    let source = format!(
        "pub static PINS: [[u8; 32]; {}] = [{}];\n\
         pub const PRODUCT: &str = {:?};\n\
         pub static VARIANTS: [u16; {}] = [{}];\n\
         pub const MIN_SECURITY_VERSION: u32 = {};\n",
        pins.len(),
        pins_src,
        product,
        variants.len(),
        variants_src,
        min_security
    );
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("verified_config.rs");
    fs::write(out, source).unwrap();
}
