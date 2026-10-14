fn main() {
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"supervised\"))");
    // Reuse esp-hal's normal application linker, not the second-stage script.
    println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
}
