//! The platform's primary console is one build-time setting, `IOBEWI_CONSOLE_PRIMARY`, read by
//! every image of the platform (bootloader and application alike). It becomes a constant and a
//! greppable stamp; `.github/scripts/esp_console.py` refuses images stamped differently.

use std::{env, fs, path::PathBuf};

#[path = "src/backend.rs"]
mod backend;

const VARIABLE: &str = "IOBEWI_CONSOLE_PRIMARY";

fn main() {
    println!("cargo:rerun-if-env-changed={VARIABLE}");
    println!("cargo:rerun-if-changed=src/backend.rs");
    let backend = match env::var(VARIABLE) {
        Ok(text) => backend::ConsoleBackend::from_name(text.trim()).unwrap_or_else(|| {
            panic!(
                "{VARIABLE}: `{text}` is not a console; use `none`, `usb-serial-jtag` or `uart<N>` (default `uart0`)"
            )
        }),
        Err(_) => backend::ConsoleBackend::Uart(0),
    };
    let stamp = format!("IOBEWI-CONSOLE primary={}\0", name(backend));
    let source = format!(
        "/// The platform's primary console (`{VARIABLE}`, default `uart0`).\n\
         pub const PRIMARY: ConsoleBackend = ConsoleBackend::{backend:?};\n\
         /// Greppable in the binaries: tools compare the stamps of the images of one platform.\n\
         pub static STAMP: [u8; {}] = *b{stamp:?};\n",
        stamp.len()
    );
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("console_config.rs"),
        source,
    )
    .unwrap();
}

/// The spelling `from_name` accepts, as written in the stamp.
fn name(backend: backend::ConsoleBackend) -> String {
    match backend {
        backend::ConsoleBackend::None => "none".into(),
        backend::ConsoleBackend::UsbSerialJtag => "usb-serial-jtag".into(),
        backend::ConsoleBackend::Uart(n) => format!("uart{n}"),
    }
}
