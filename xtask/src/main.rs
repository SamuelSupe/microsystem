use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use base64::Engine;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
enum Architecture {
    Aarch64,
    Riscv64,
}

impl Architecture {
    fn from_env() -> Result<Self, String> {
        match env::var("ARCH")
            .unwrap_or_else(|_| "aarch64".into())
            .as_str()
        {
            "aarch64" => Ok(Self::Aarch64),
            "riscv64" => Ok(Self::Riscv64),
            arch => Err(format!("ARCH must be aarch64 or riscv64 (got {arch})")),
        }
    }

    fn target(self) -> &'static str {
        match self {
            Self::Aarch64 => "aarch64-unknown-none-softfloat",
            Self::Riscv64 => "riscv64gc-unknown-none-elf",
        }
    }

    fn qemu_binary(self) -> &'static str {
        match self {
            Self::Aarch64 => "qemu-system-aarch64",
            Self::Riscv64 => "qemu-system-riscv64",
        }
    }

    fn virtio_pci_options(self) -> &'static str {
        match self {
            Self::Aarch64 => "disable-legacy=on,iommu_platform=on,romfile=",
            Self::Riscv64 => "disable-legacy=on,iommu_platform=on,romfile=",
        }
    }
}

fn main() -> ExitCode {
    let command = env::args().nth(1).unwrap_or_else(|| "help".into());
    let result = match command.as_str() {
        "build" => Architecture::from_env().and_then(build),
        "run" => Architecture::from_env().and_then(|arch| build(arch).and_then(|_| run_qemu(arch))),
        "gui" => Architecture::from_env().and_then(run_qemu_gui),
        "qemu" => Architecture::from_env().and_then(run_qemu),
        "test" => test(),
        "fsck" => fsck(),
        _ => {
            eprintln!(
                "usage: ARCH=aarch64|riscv64 cargo run -p xtask -- <build|run|qemu|gui|test|fsck>"
            );
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build(arch: Architecture) -> Result<(), String> {
    fs::create_dir_all("build").map_err(|e| e.to_string())?;
    ensure_ssh_material()?;
    let ca_bundle_hash = build_ca_bundle()?;
    run(Command::new("cargo").args([
        "build",
        "--release",
        "--target",
        arch.target(),
        "-p",
        "microsystem-init",
        "--features",
        "user-bin",
    ]))?;
    for package in [
        "microsystem-console",
        "microsystem-devmgr",
        "microsystem-block",
        "microsystem-badptr",
        "microsystem-fs",
        "microsystem-privprobe",
        "microsystem-resourceprobe",
        "microsystem-resourcefault",
        "microsystem-resourcekill",
        "microsystem-shell",
        "microsystem-counter",
        "microsystem-crossptr",
        "microsystem-spinner",
        "microsystem-idle",
        "microsystem-pageprobe",
        "microsystem-windowd",
        "microsystem-terminal",
        "microsystem-files",
        "microsystem-monitor",
        "microsystem-netd",
        "microsystem-mica-service",
        "microsystem-sshd",
    ] {
        let mut command = Command::new("cargo");
        command.args([
            "build",
            "--release",
            "--target",
            arch.target(),
            "-p",
            package,
            "--features",
            "user-bin",
        ]);
        if package == "microsystem-mica-service" {
            command.env("MICROSYSTEM_CA_BUNDLE_SHA256", &ca_bundle_hash);
        }
        run(&mut command)?;
    }
    create_bootfs(arch)?;
    run(Command::new("cargo").args([
        "build",
        "--release",
        "--target",
        arch.target(),
        "-p",
        "microsystem-kernel",
        "--features",
        "baremetal",
    ]))?;
    run(Command::new("cargo").args(["build", "--release", "-p", "mfsctl"]))?;
    let disk = disk_path();
    if !disk.exists() {
        run(Command::new(host_binary("mfsctl")).args([
            "mkfs",
            disk.to_str().ok_or("invalid disk path")?,
            "64",
        ]))?;
    }
    build_unifont()?;
    for directory in [
        "/.system",
        "/.system/fonts",
        "/.system/mica",
        "/.system/examples",
        "/.system/examples/mica",
        "/.system/certs",
        "/.system/ssh",
    ] {
        run(Command::new(host_binary("mfsctl")).args([
            "mkdir",
            disk.to_str().ok_or("invalid disk path")?,
            directory,
        ]))?;
    }
    run(Command::new(host_binary("mfsctl")).args([
        "put",
        disk.to_str().ok_or("invalid disk path")?,
        "build/unifont-17.0.05.ufb",
        "/.system/fonts/unifont-17.0.05.ufb",
    ]))?;
    run(Command::new(host_binary("mfsctl")).args([
        "put",
        disk.to_str().ok_or("invalid disk path")?,
        "build/ca-bundle.derpack",
        "/.system/certs/ca-bundle.derpack",
    ]))?;
    for example in [
        "hello",
        "fs-status",
        "http-status",
        "gui-counter",
        "browser",
        "editor",
    ] {
        let source = format!("assets/mica/{example}.mica");
        let destination = format!("/.system/examples/mica/{example}.mica");
        run(Command::new(host_binary("mfsctl")).args([
            "put",
            disk.to_str().ok_or("invalid disk path")?,
            &source,
            &destination,
        ]))?;
    }
    run(Command::new(host_binary("mfsctl")).args([
        "put",
        disk.to_str().ok_or("invalid disk path")?,
        "assets/ssh/mica-policy",
        "/.system/ssh/mica-policy",
    ]))?;
    Ok(())
}

fn build_unifont() -> Result<(), String> {
    const GLYPHS: usize = 65_536;
    const HEADER_BYTES: usize = 16;
    const RECORD_BYTES: usize = 36;
    let compressed =
        fs::File::open("assets/fonts/unifont-17.0.05.hex.gz").map_err(|error| error.to_string())?;
    let decoder = flate2::read::GzDecoder::new(compressed);
    let reader = BufReader::new(decoder);
    let mut records = Vec::<(u16, u8, [u8; 32])>::new();
    for line in reader.lines() {
        let line = line.map_err(|error| error.to_string())?;
        let Some((code, bitmap)) = line.split_once(':') else {
            continue;
        };
        let code = u32::from_str_radix(code, 16).map_err(|_| "invalid Unifont codepoint")?;
        if code >= GLYPHS as u32 || !matches!(bitmap.len(), 32 | 64) {
            continue;
        }
        let mut bytes = [0u8; 32];
        for (index, pair) in bitmap.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = u8::from_str_radix(
                core::str::from_utf8(pair).map_err(|_| "invalid Unifont hex")?,
                16,
            )
            .map_err(|_| "invalid Unifont bitmap")?;
        }
        records.push((code as u16, if bitmap.len() == 32 { 8 } else { 16 }, bytes));
    }
    let mut output = vec![0u8; HEADER_BYTES + GLYPHS * 4];
    output[..4].copy_from_slice(b"UFB1");
    output[4..8].copy_from_slice(&0x0011_0005u32.to_le_bytes());
    output[8..12].copy_from_slice(&(records.len() as u32).to_le_bytes());
    output[12..16].copy_from_slice(&(RECORD_BYTES as u32).to_le_bytes());
    for (code, width, bitmap) in records {
        let offset = output.len() as u32;
        let index = HEADER_BYTES + code as usize * 4;
        output[index..index + 4].copy_from_slice(&offset.to_le_bytes());
        output.push(width);
        output.extend_from_slice(&[16, 0, 0]);
        output.extend_from_slice(&bitmap);
    }
    fs::write("build/unifont-17.0.05.ufb", &output).map_err(|error| error.to_string())?;
    eprintln!(
        "Unifont 17.0.05: {} glyphs, {} bytes",
        u32::from_le_bytes(output[8..12].try_into().unwrap()),
        output.len()
    );
    Ok(())
}

fn build_ca_bundle() -> Result<String, String> {
    const VERSION: &str = "1.0.9";
    const SOURCE_SHA256: &str = "581232b6fb8d5b8df315d34c798099ee759cb4930ffed77c331e7b38fed82b15";
    const BUNDLE_SHA256: &str = "328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6";
    const CERTIFICATES: usize = 121;

    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .ok_or("CARGO_HOME and HOME are unavailable")?;
    let registry_sources = cargo_home.join("registry/src");
    let mut source = None;
    for registry in fs::read_dir(&registry_sources).map_err(|error| error.to_string())? {
        let candidate = registry
            .map_err(|error| error.to_string())?
            .path()
            .join(format!("webpki-roots-{VERSION}/src/lib.rs"));
        if candidate.is_file() {
            source = Some(candidate);
            break;
        }
    }
    let source = source.ok_or("pinned webpki-roots source is unavailable")?;
    let text = fs::read(&source).map_err(|error| error.to_string())?;
    if hex_sha256(&text) != SOURCE_SHA256 {
        return Err("webpki-roots source hash does not match the pinned snapshot".into());
    }
    let text = core::str::from_utf8(&text).map_err(|error| error.to_string())?;
    let mut certificates = parse_pem_certificates(text)?;
    if certificates.len() != CERTIFICATES {
        return Err("pinned Mozilla snapshot contains an unexpected certificate count".into());
    }
    let pinned_bundle = encode_ca_bundle(&certificates)?;
    if hex_sha256(&pinned_bundle) != BUNDLE_SHA256 {
        return Err("generated CA bundle hash does not match the pinned snapshot".into());
    }

    let mut local_certificates = 0usize;
    if let Some(path) = env::var_os("SSL_CERT_FILE") {
        let pem = fs::read(&path).map_err(|error| {
            format!(
                "failed to read host CA bundle {}: {error}",
                PathBuf::from(path).display()
            )
        })?;
        let text = core::str::from_utf8(&pem).map_err(|error| error.to_string())?;
        for certificate in parse_pem_certificates(text)? {
            if certificate.len() <= 16 * 1024
                && !certificates.iter().any(|existing| existing == &certificate)
            {
                certificates.push(certificate);
                local_certificates += 1;
            }
        }
    }

    let bundle = encode_ca_bundle(&certificates)?;
    if bundle.len() > 256 * 1024 {
        return Err("combined CA bundle exceeds the Mica 256 KiB read limit".into());
    }
    eprintln!(
        "CA bundle: pinned={} host-extra={} bytes={}",
        CERTIFICATES,
        local_certificates,
        bundle.len()
    );
    let bundle_hash = hex_sha256(&bundle);
    fs::write("build/ca-bundle.derpack", bundle).map_err(|error| error.to_string())?;
    Ok(bundle_hash)
}

fn parse_pem_certificates(text: &str) -> Result<Vec<Vec<u8>>, String> {
    let mut certificates = Vec::new();
    let mut encoded = String::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim().strip_prefix('*').unwrap_or(line.trim()).trim();
        match line {
            "-----BEGIN CERTIFICATE-----" => {
                if inside {
                    return Err("nested PEM certificate boundary".into());
                }
                inside = true;
                encoded.clear();
            }
            "-----END CERTIFICATE-----" if inside => {
                certificates.push(
                    base64::engine::general_purpose::STANDARD
                        .decode(&encoded)
                        .map_err(|error| error.to_string())?,
                );
                inside = false;
            }
            _ if inside => encoded.push_str(line),
            _ => {}
        }
    }
    if inside {
        return Err("unterminated PEM certificate".into());
    }
    Ok(certificates)
}

fn encode_ca_bundle(certificates: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let payload_bytes = certificates
        .iter()
        .try_fold(0usize, |bytes, certificate| {
            bytes.checked_add(4 + certificate.len())
        })
        .ok_or("CA bundle size overflow")?;
    let mut bundle = Vec::with_capacity(16 + payload_bytes);
    bundle.extend_from_slice(b"MCAB");
    bundle.extend_from_slice(&1u32.to_le_bytes());
    bundle.extend_from_slice(&(certificates.len() as u32).to_le_bytes());
    bundle.extend_from_slice(&(payload_bytes as u32).to_le_bytes());
    for certificate in certificates {
        bundle.extend_from_slice(&(certificate.len() as u32).to_le_bytes());
        bundle.extend_from_slice(certificate);
    }
    Ok(bundle)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn ensure_ssh_material() -> Result<(), String> {
    use base64::Engine as _;
    use ed25519_dalek::SigningKey;

    let directory = Path::new("build/ssh");
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let host = Path::new("build/ssh-host-key.bin");
    let authorized = Path::new("build/ssh-authorized-key.bin");
    let private = directory.join("id_ed25519");
    if host.exists() && authorized.exists() && private.exists() {
        return Ok(());
    }
    let mut entropy = fs::File::open("/dev/urandom").map_err(|error| error.to_string())?;
    let mut host_seed = [0u8; 32];
    let mut client_seed = [0u8; 32];
    let mut check = [0u8; 4];
    entropy
        .read_exact(&mut host_seed)
        .map_err(|error| error.to_string())?;
    entropy
        .read_exact(&mut client_seed)
        .map_err(|error| error.to_string())?;
    entropy
        .read_exact(&mut check)
        .map_err(|error| error.to_string())?;
    let client = SigningKey::from_bytes(&client_seed);
    let public = client.verifying_key().to_bytes();
    fs::write(host, host_seed).map_err(|error| error.to_string())?;
    fs::write(authorized, public).map_err(|error| error.to_string())?;

    let mut public_blob = Vec::new();
    ssh_string(&mut public_blob, b"ssh-ed25519");
    ssh_string(&mut public_blob, &public);
    let mut private_blob = Vec::new();
    private_blob.extend_from_slice(&check);
    private_blob.extend_from_slice(&check);
    ssh_string(&mut private_blob, b"ssh-ed25519");
    ssh_string(&mut private_blob, &public);
    let mut secret = [0u8; 64];
    secret[..32].copy_from_slice(&client_seed);
    secret[32..].copy_from_slice(&public);
    ssh_string(&mut private_blob, &secret);
    ssh_string(&mut private_blob, b"microsystem-dev");
    let mut padding = 1u8;
    while private_blob.len() % 8 != 0 {
        private_blob.push(padding);
        padding = padding.wrapping_add(1);
    }
    let mut openssh = b"openssh-key-v1\0".to_vec();
    ssh_string(&mut openssh, b"none");
    ssh_string(&mut openssh, b"none");
    ssh_string(&mut openssh, b"");
    openssh.extend_from_slice(&1u32.to_be_bytes());
    ssh_string(&mut openssh, &public_blob);
    ssh_string(&mut openssh, &private_blob);
    let encoded = base64::engine::general_purpose::STANDARD.encode(openssh);
    let mut pem = String::from("-----BEGIN OPENSSH PRIVATE KEY-----\n");
    for chunk in encoded.as_bytes().chunks(70) {
        pem.push_str(core::str::from_utf8(chunk).map_err(|_| "invalid key encoding")?);
        pem.push('\n');
    }
    pem.push_str("-----END OPENSSH PRIVATE KEY-----\n");
    fs::write(&private, pem).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    let public_line = format!(
        "ssh-ed25519 {} microsystem-dev\n",
        base64::engine::general_purpose::STANDARD.encode(public_blob)
    );
    fs::write(directory.join("id_ed25519.pub"), public_line).map_err(|error| error.to_string())?;
    Ok(())
}

fn ssh_string(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u32).to_be_bytes());
    output.extend_from_slice(value);
}

fn create_bootfs(arch: Architecture) -> Result<(), String> {
    let mut archive = Vec::new();
    let services = [
        ("init", "microsystem-init"),
        ("console", "microsystem-console"),
        ("devmgr", "microsystem-devmgr"),
        ("block", "microsystem-block"),
        ("mfs", "microsystem-fs"),
        ("badptr", "microsystem-badptr"),
        ("privprobe", "microsystem-privprobe"),
        ("resourceprobe", "microsystem-resourceprobe"),
        ("resourcefault", "microsystem-resourcefault"),
        ("resourcekill", "microsystem-resourcekill"),
        ("pageprobe", "microsystem-pageprobe"),
        ("shell", "microsystem-shell"),
        ("counter", "microsystem-counter"),
        ("crossptr", "microsystem-crossptr"),
        ("spinner", "microsystem-spinner"),
        ("idle", "microsystem-idle"),
        ("windowd", "microsystem-windowd"),
        ("terminal", "microsystem-terminal"),
        ("files", "microsystem-files"),
        ("monitor", "microsystem-monitor"),
        ("netd", "microsystem-netd"),
        ("mica", "microsystem-mica-service"),
        ("sshd", "microsystem-sshd"),
    ];
    for (index, (service, binary)) in services.iter().enumerate() {
        let image = fs::read(
            PathBuf::from("target")
                .join(arch.target())
                .join("release")
                .join(binary),
        )
        .map_err(|error| error.to_string())?;
        append_newc(
            &mut archive,
            (index + 1) as u32,
            &format!("bin/{service}"),
            0o100755,
            &image,
        );
    }
    append_newc(
        &mut archive,
        100,
        "etc/services",
        0o100644,
        b"init\ndevmgr\nconsole\nblock\nmfs\nshell\nnetd\nsshd\nwindowd\nterminal\nfiles\nmonitor\n",
    );
    append_newc(&mut archive, 0, "TRAILER!!!", 0, &[]);
    fs::write("build/bootfs.cpio", archive).map_err(|error| error.to_string())
}

fn append_newc(out: &mut Vec<u8>, inode: u32, name: &str, mode: u32, data: &[u8]) {
    out.extend_from_slice(b"070701");
    for value in [
        inode,
        mode,
        0,
        0,
        1,
        0,
        data.len() as u32,
        0,
        0,
        0,
        0,
        name.len() as u32 + 1,
        0,
    ] {
        out.extend_from_slice(format!("{value:08x}").as_bytes());
    }
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn qemu_command(arch: Architecture, gui: bool) -> Command {
    let mut command = Command::new(arch.qemu_binary());
    match arch {
        Architecture::Aarch64 => {
            command.args([
                "-machine",
                "virt-7.2,virtualization=on,gic-version=3,iommu=smmuv3",
                "-cpu",
                "cortex-a72",
                "-accel",
                "tcg,thread=multi",
                "-smp",
                "2",
                "-m",
                "256M",
            ]);
            if gui {
                command.args([
                    "-display",
                    "none",
                    "-vnc",
                    "0.0.0.0:0",
                    "-serial",
                    "stdio",
                    "-monitor",
                    "none",
                    "-qmp",
                    "unix:target/gui-qmp.sock,server=on,wait=off",
                ]);
            } else {
                command.arg("-nographic");
            }
            command.args([
                "-L",
                "/usr/lib/ipxe/qemu",
                "-no-reboot",
                "-semihosting-config",
                "enable=on,target=native",
            ]);
        }
        Architecture::Riscv64 => {
            command.args([
                "-machine",
                "virt,iommu-sys=on",
                "-bios",
                "default",
                "-cpu",
                "rv64",
                "-accel",
                "tcg,thread=multi",
                "-smp",
                "2",
                "-m",
                "256M",
            ]);
            if gui {
                command.args([
                    "-display",
                    "none",
                    "-vnc",
                    "0.0.0.0:0",
                    "-serial",
                    "stdio",
                    "-monitor",
                    "none",
                    "-qmp",
                    "unix:target/gui-qmp.sock,server=on,wait=off",
                ]);
            } else {
                command.arg("-nographic");
            }
            command.arg("-no-reboot");
        }
    }
    command
}

fn run_qemu(arch: Architecture) -> Result<(), String> {
    let netdev = network_backend();
    let kernel = kernel_path(arch);
    let kernel = kernel.to_str().ok_or("invalid kernel path")?;
    let drive = format!(
        "if=none,file={},format=raw,cache=writeback,id=disk0",
        disk_path().display()
    );
    let block_device = format!(
        "virtio-blk-pci,drive=disk0,{},addr=2",
        arch.virtio_pci_options()
    );
    let net_device = format!(
        "virtio-net-pci,netdev=net0,{},addr=6,mac=52:54:00:12:34:56",
        arch.virtio_pci_options()
    );
    let rng_device = format!(
        "virtio-rng-pci,rng=rng0,{},addr=7",
        arch.virtio_pci_options()
    );
    let mut command = qemu_command(arch, false);
    command
        .arg("-kernel")
        .arg(kernel)
        .arg("-drive")
        .arg(drive)
        .arg("-device")
        .arg(block_device)
        .arg("-netdev")
        .arg(netdev)
        .arg("-device")
        .arg(net_device)
        .args([
            "-object",
            "rng-random,filename=/dev/urandom,id=rng0",
            "-device",
        ])
        .arg(rng_device);
    run(&mut command)
}

fn run_qemu_gui(arch: Architecture) -> Result<(), String> {
    let port = env::var("GUI_PORT").unwrap_or_else(|_| "5900".into());
    let ssh_port = env::var("SSH_PORT").unwrap_or_else(|_| "2222".into());
    eprintln!("MicroSystem GUI: vnc://127.0.0.1:{port}");
    eprintln!(
        "MicroSystem SSH: ssh -F /dev/null -T -p {ssh_port} -i build/ssh/id_ed25519 micro@127.0.0.1"
    );
    let netdev = network_backend();
    let qmp_socket = Path::new("target/gui-qmp.sock");
    if qmp_socket.exists() {
        fs::remove_file(qmp_socket).map_err(|error| error.to_string())?;
    }
    let kernel = kernel_path(arch);
    let kernel = kernel.to_str().ok_or("invalid kernel path")?;
    let drive = format!(
        "if=none,file={},format=raw,cache=writeback,id=disk0",
        disk_path().display()
    );
    let block_device = format!(
        "virtio-blk-pci,drive=disk0,{},addr=2",
        arch.virtio_pci_options()
    );
    let gpu_device = format!("virtio-gpu-pci,{},addr=3", arch.virtio_pci_options());
    let keyboard_device = format!("virtio-keyboard-pci,{},addr=4", arch.virtio_pci_options());
    let tablet_device = format!("virtio-tablet-pci,{},addr=5", arch.virtio_pci_options());
    let net_device = format!(
        "virtio-net-pci,netdev=net0,{},addr=6,mac=52:54:00:12:34:56",
        arch.virtio_pci_options()
    );
    let rng_device = format!(
        "virtio-rng-pci,rng=rng0,{},addr=7",
        arch.virtio_pci_options()
    );
    let mut command = qemu_command(arch, true);
    command
        .arg("-kernel")
        .arg(kernel)
        .arg("-drive")
        .arg(drive)
        .arg("-device")
        .arg(block_device)
        .arg("-device")
        .arg(gpu_device)
        .arg("-device")
        .arg(keyboard_device)
        .arg("-device")
        .arg(tablet_device)
        .arg("-netdev")
        .arg(netdev)
        .arg("-device")
        .arg(net_device)
        .args([
            "-object",
            "rng-random,filename=/dev/urandom,id=rng0",
            "-device",
        ])
        .arg(rng_device);
    run(&mut command)
}

fn network_backend() -> String {
    let bind = if env::var("MICROSYSTEM_CONTAINER").as_deref() == Ok("1") {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };
    format!("user,id=net0,restrict=off,hostfwd=tcp:{bind}:2222-10.0.2.15:22")
}

fn test() -> Result<(), String> {
    Architecture::from_env()?;
    run(Command::new("cargo").args([
        "test",
        "-p",
        "microsystem-abi",
        "-p",
        "mfs1",
        "-p",
        "microsystem-kernel",
        "-p",
        "microsystem-gui",
        "-p",
        "microsystem-mica",
    ]))?;
    let smoke = Path::new("scripts/smoke-qemu.sh");
    if smoke.exists() {
        run(&mut Command::new(smoke))?;
    }
    let powercut = Path::new("scripts/powercut-qemu.sh");
    if powercut.exists() {
        run(&mut Command::new(powercut))?;
    }
    let fault_injection = Path::new("scripts/fault-injection-qemu.sh");
    if fault_injection.exists() {
        run(&mut Command::new(fault_injection))?;
    }
    let gui = Path::new("scripts/gui-qemu.sh");
    if gui.exists() {
        run(&mut Command::new(gui))?;
    }
    let ssh = Path::new("scripts/ssh-qemu.sh");
    if ssh.exists() {
        run(&mut Command::new(ssh))?;
    }
    let mica = Path::new("scripts/mica-qemu.sh");
    if mica.exists() {
        run(Command::new(mica).env("MICROSYSTEM_MICA_TLS_FIXTURE", "1"))?;
    }
    Ok(())
}

fn fsck() -> Result<(), String> {
    let arch = Architecture::from_env()?;
    if !disk_path().exists() {
        build(arch)?;
    }
    run(Command::new("cargo").args([
        "run",
        "--release",
        "-p",
        "mfsctl",
        "--",
        "fsck",
        disk_path().to_str().ok_or("invalid disk path")?,
    ]))
}

fn kernel_path(arch: Architecture) -> PathBuf {
    PathBuf::from("target")
        .join(arch.target())
        .join("release")
        .join("microsystem-kernel")
}

fn host_binary(name: &str) -> PathBuf {
    PathBuf::from("target").join("release").join(name)
}

fn disk_path() -> PathBuf {
    PathBuf::from("build/microsystem.img")
}

fn run(command: &mut Command) -> Result<(), String> {
    eprintln!("+ {command:?}");
    let status = command.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed with {status}"))
    }
}
