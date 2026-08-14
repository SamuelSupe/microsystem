use std::env;
use std::path::PathBuf;

fn main() {
    let package = env::var("CARGO_PKG_NAME").unwrap();
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let linker = manifest.parent().unwrap().join("user.ld");
    println!("cargo:rustc-link-arg-bin={package}=-T{}", linker.display());
    println!("cargo:rerun-if-changed={}", linker.display());
}
