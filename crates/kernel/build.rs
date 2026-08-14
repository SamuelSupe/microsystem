use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    println!(
        "cargo:rustc-link-arg-bin=microsystem-kernel=-T{}",
        manifest.join("linker.ld").display()
    );
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=src/arch/aarch64/boot.S");
    println!("cargo:rerun-if-changed=src/arch/aarch64/exception.S");
}
