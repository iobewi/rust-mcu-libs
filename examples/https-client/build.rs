fn main() {
    // Reuse esp-hal's normal application linker.
    println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
}
