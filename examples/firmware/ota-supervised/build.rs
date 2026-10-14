fn main() {
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"esp32c3\"))");
    if std::env::var_os("CARGO_FEATURE_FIRMWARE").is_none() {
        return;
    }
    println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
    println!(
        "cargo:rustc-link-search={}",
        std::env::var("CARGO_MANIFEST_DIR").unwrap()
    );
    println!("cargo:rustc-link-arg-bins=-Tnative-region.x");
    println!("cargo:rerun-if-changed=native-region.x");
}
