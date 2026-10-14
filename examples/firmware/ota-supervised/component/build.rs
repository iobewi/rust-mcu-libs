fn main() {
    if std::env::var_os("CARGO_FEATURE_ESP32S3").is_none() {
        return;
    }
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../common/firmware/component/sdk/ld/esp32s3-v1.ld");
    println!("cargo:rustc-link-arg-bins=-T{}", file.display());
    for flag in [
        "-nostartfiles",
        "-nodefaultlibs",
        "-Wl,--gc-sections",
        "-Wl,--build-id=none",
    ] {
        println!("cargo:rustc-link-arg-bins={flag}");
    }
    println!("cargo:rerun-if-changed={}", file.display());
}
