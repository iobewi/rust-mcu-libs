//! The partition table offset is a build-time property of the whole firmware: the second stage,
//! the application and the generated table must agree on it. The one source is the variable
//! `esp-bootloader-esp-idf` already uses for the application's own table lookup
//! (default 0x8000), so there is no second setting to keep in step with it.

use std::{env, fs, path::PathBuf};

const VARIABLE: &str = "ESP_BOOTLOADER_ESP_IDF_CONFIG_PARTITION_TABLE_OFFSET";
const HISTORICAL: u32 = 0x8000;
/// `ota_0` starts at 0x20000 in every reference layout; the table sector, `otadata` (8 KiB)
/// and `ota_meta` (8 KiB) must fit in front of it.
const MAX_OFFSET: u32 = 0x20000 - 0x5000;

fn parse(text: &str) -> Option<u32> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed={VARIABLE}");
    let offset = match env::var(VARIABLE) {
        Ok(text) => {
            parse(&text).unwrap_or_else(|| panic!("{VARIABLE}: `{text}` is not an integer"))
        }
        Err(_) => HISTORICAL,
    };
    assert!(
        offset % 0x1000 == 0 && (HISTORICAL..=MAX_OFFSET).contains(&offset),
        "{VARIABLE}={offset:#x}: the partition table sits on a 4 KiB boundary between {HISTORICAL:#x} and {MAX_OFFSET:#x}"
    );
    let stamp = format!("IOBEWI-GEOMETRY table={offset:#010x}\0");
    let source = format!(
        "pub const TABLE_OFFSET: u32 = {offset:#x};\n\
         /// Greppable in the binaries: tools compare it with the table they generate.\n\
         pub static STAMP: [u8; {}] = *b{stamp:?};\n",
        stamp.len()
    );
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("geometry.rs"),
        source,
    )
    .unwrap();
}
