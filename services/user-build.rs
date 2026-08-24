use std::env;
use std::path::PathBuf;

fn main() {
    let package = env::var("CARGO_PKG_NAME").unwrap();
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let linker = manifest.parent().unwrap().join("user.ld");
    println!("cargo:rustc-link-arg-bin={package}=-T{}", linker.display());
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64") {
        println!("cargo:rustc-link-arg-bin={package}=-no-pie");
    }
    println!("cargo:rerun-if-changed={}", linker.display());
}
