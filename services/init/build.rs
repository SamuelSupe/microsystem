use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    println!(
        "cargo:rustc-link-arg-bin=microsystem-init=-T{}",
        manifest.join("user.ld").display()
    );
    println!("cargo:rerun-if-changed=user.ld");
}
