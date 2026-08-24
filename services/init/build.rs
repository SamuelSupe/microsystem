use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    println!(
        "cargo:rustc-link-arg-bin=microsystem-init=-T{}",
        manifest.join("user.ld").display()
    );
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64") {
        println!("cargo:rustc-link-arg-bin=microsystem-init=-no-pie");
    }
    println!("cargo:rerun-if-changed=user.ld");
}
