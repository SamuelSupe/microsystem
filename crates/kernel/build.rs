use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH")
        .or_else(|_| env::var("TARGET_ARCH"))
        .unwrap_or_else(|_| "aarch64".to_owned());
    println!("cargo:rerun-if-env-changed=TARGET_ARCH");
    let (linker, sources) = match target_arch.as_str() {
        "riscv64" => (
            "linker-riscv64.ld",
            &[
                "src/arch/riscv64",
                "src/arch/riscv64/boot.S",
                "src/arch/riscv64/exception.S",
            ][..],
        ),
        "x86_64" => (
            "linker-x86_64.ld",
            &[
                "src/arch/x86_64",
                "src/arch/x86_64/boot.S",
                "src/arch/x86_64/exception.S",
            ][..],
        ),
        _ => (
            "linker.ld",
            &["src/arch/aarch64/boot.S", "src/arch/aarch64/exception.S"][..],
        ),
    };
    println!(
        "cargo:rustc-link-arg-bin=microsystem-kernel=-T{}",
        manifest.join(linker).display()
    );
    if target_arch == "x86_64" {
        println!("cargo:rustc-link-arg-bin=microsystem-kernel=-no-pie");
    }
    println!("cargo:rerun-if-changed={linker}");
    for source in sources {
        println!("cargo:rerun-if-changed={source}");
    }
}
