//! FerroOS build automation. Works on Windows, Linux and macOS.
//!
//!     cargo xtask kernel-build  build the tailored FerroOS kernel in a QEMU VM
//!     cargo xtask kernel     or fetch a prebuilt generic kernel (quick start)
//!     cargo xtask build      static musl binaries for init, shell and cmd
//!     cargo xtask initramfs  build, then pack build/initramfs.cpio
//!     cargo xtask run [..]   all of the above, then boot QEMU with 64 MB RAM
//!     cargo xtask tor-build  build the Tor service (Arti) in a QEMU VM
//!     cargo xtask debian-build  build the Debian layer (glibc) in a QEMU VM
//!     cargo xtask iso        build/ferroos.iso: live system + installer
//!     cargo xtask heavy-iso  build/ferroos-heavy.iso: FerroOS Heavy, our
//!                            desktop on a hardened Debian base (see heavy/)
//!     cargo xtask wg-test-server  a throwaway WireGuard server VM for testing
//!
//! Files in build/import/ appear as drive D: inside FerroOS (QEMU only).
//!
//! The kernel is build/bzImage: from `cargo xtask kernel`, from
//! tools/build-kernel.sh (Linux/WSL, everything built in), or $FERRO_KERNEL.

mod builder;
mod debian;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

const TARGET: &str = "x86_64-unknown-linux-musl";

/// Drivers a distro kernel ships as modules but FerroOS needs at boot:
/// QEMU's display (bochs DRM + fbdev), the PS/2 mouse, /dev/input/mice,
/// evdev (keyboard), the virtio network card and disks, and the Debian
/// layer's file system. Dependencies are resolved from modules.dep.
const BOOT_MODULES: [&str; 7] = ["bochs", "psmouse", "mousedev", "evdev", "virtio_net", "virtio_blk", "squashfs"];

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("kernel") => fetch_kernel(),
        Some("kernel-build") => builder::kernel_build(),
        Some("tor-build") => builder::tor_build(),
        Some("debian-build") => builder::debian_build(),
        Some("iso") => {
            build();
            initramfs();
            builder::iso();
        }
        Some("heavy-iso") => builder::heavy_iso(),
        Some("wm-test") => builder::wm_test(),
        Some("wg-test-server") => builder::wg_test_server(),
        Some("build") => build(),
        Some("initramfs") => {
            build();
            initramfs();
        }
        Some("run") => {
            build();
            initramfs();
            run(&args[1..]);
        }
        _ => {
            eprintln!(
                "usage: cargo xtask <kernel-build|kernel|build|initramfs|run [extra qemu args]|tor-build|debian-build|iso|heavy-iso|wm-test|wg-test-server>"
            );
            exit(2);
        }
    }
}

pub(crate) fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

pub(crate) fn build_dir() -> PathBuf {
    let d = root().join("build");
    fs::create_dir_all(&d).expect("create build/");
    d
}

fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), PathBuf::from)
}

fn bin_dir() -> PathBuf {
    target_dir().join(TARGET).join("release")
}

/// Downloads that are only build inputs (kept out of build/, which may sync).
pub(crate) fn cache_dir() -> PathBuf {
    target_dir().join("ferro-cache")
}

pub(crate) fn curl(url: &str, dest: &Path) {
    check(Command::new("curl").args(["-fsSL", "-o"]).arg(dest).arg(url));
}

pub(crate) fn qemu_binary() -> String {
    env::var("QEMU").unwrap_or_else(|_| {
        let windows_default = r"C:\Program Files\qemu\qemu-system-x86_64.exe";
        if Path::new(windows_default).exists() {
            windows_default.into()
        } else {
            "qemu-system-x86_64".into()
        }
    })
}

pub(crate) fn check(cmd: &mut Command) {
    let status = cmd.status().unwrap_or_else(|e| {
        eprintln!("xtask: failed to run {:?}: {e}", cmd.get_program());
        exit(1)
    });
    if !status.success() {
        exit(status.code().unwrap_or(1));
    }
}

fn build() {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    check(Command::new("rustup").args(["target", "add", TARGET]));
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root()).args(["build", "--release", "--target", TARGET, "-p", "ferro"]);
    // ferro-net's TLS crypto (ring) has some C. Compile it with clang for the
    // musl target; tools/c-shim provides the few libc headers it includes.
    // Clock floor for ferro-net: the build day (whole days, so this doesn't
    // force a rebuild every second).
    let today = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) / 86_400 * 86_400;
    cmd.env("FERRO_BUILD_TIME", today.to_string());
    // Optional programs and the Debian layer are verified against hashes
    // compiled into FerroOS.
    for (path, var) in [(programs_dir().join("arti"), "FERRO_ARTI_SHA256"), (layer_image(), "FERRO_DEBIAN_SHA256")] {
        match fs::read(path) {
            Ok(data) => {
                use sha2::{Digest, Sha256};
                let hex: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
                cmd.env(var, hex);
            }
            Err(_) => {
                cmd.env_remove(var);
            }
        }
    }
    // What checks the Debian layer block by block (dm-verity's root hash).
    match fs::read_to_string(programs_dir().join("debian.verity")) {
        Ok(line) if layer_image().exists() => {
            cmd.env("FERRO_DEBIAN_VERITY", line.trim());
        }
        _ => {
            cmd.env_remove("FERRO_DEBIAN_VERITY");
        }
    }
    let shim = root().join("tools").join("c-shim").join("include");
    if env::var_os("CC_x86_64_unknown_linux_musl").is_none() {
        cmd.env("CC_x86_64_unknown_linux_musl", "clang")
            .env("AR_x86_64_unknown_linux_musl", "llvm-ar")
            .env("CFLAGS_x86_64_unknown_linux_musl", format!("--target={TARGET} -ffreestanding -I{}", shim.display()));
    }
    check(&mut cmd);
}

/// Builds one of the pure-Rust programs as a static binary that runs on any
/// Linux (FerroOS Heavy's desktop programs), and returns where it is.
pub(crate) fn build_static(package: &str) -> PathBuf {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    check(Command::new("rustup").args(["target", "add", TARGET]));
    check(Command::new(cargo).current_dir(root()).args(["build", "--release", "--target", TARGET, "-p", package]));
    bin_dir().join(package)
}

fn initramfs() {
    let read = |name: &str| {
        let p = bin_dir().join(name);
        fs::read(&p).unwrap_or_else(|e| {
            eprintln!("xtask: {}: {e}", p.display());
            exit(1)
        })
    };
    let mut cpio = Cpio::default();
    for d in ["bin", "dev", "etc", "home", "lib", "mnt", "proc", "ProgramData", "root", "run", "sys", "tmp", "usr", "var", "var/log"] {
        cpio.dir(d);
    }
    // Where programs built for ordinary Linux look for their loader, their
    // libraries and a shell: all in the Debian layer, once it's on /usr.
    cpio.symlink("lib64", "usr/lib64");
    cpio.symlink("lib/x86_64-linux-gnu", "/usr/lib/x86_64-linux-gnu");
    // And where the kernel looks for Wi-Fi cards' firmware.
    cpio.symlink("lib/firmware", "/usr/lib/firmware");
    cpio.symlink("bin/sh", "/usr/bin/sh");
    cpio.symlink("bin/bash", "/usr/bin/bash");
    // The kernel opens /dev/console for PID 1 before devtmpfs exists.
    cpio.chardev("dev/console", 5, 1, 0o600);
    cpio.chardev("dev/null", 1, 3, 0o666);
    // Every program is one multicall binary, so the standard library and
    // shared crates sit in RAM once instead of once per program.
    cpio.file("bin/ferro", &read("ferro"), 0o755);
    cpio.symlink("init", "bin/ferro");
    for name in ["ferro-shell", "ferro-cmd", "ferro-run", "ferro-system", "ferro-net", "nslookup", "fetch"] {
        cpio.symlink(&format!("bin/{name}"), "ferro");
    }
    // One interactive user; the desktop and every app run as it, not root.
    // One interactive user, plus the Tor service's own account.
    cpio.file(
        "etc/passwd",
        b"root:x:0:0:root:/root:/bin/ferro-cmd\nuser:x:1000:1000:FerroOS User:/home/user:/bin/ferro-cmd\ntor:x:900:900:Tor:/run/tor:/bin/false\n",
        0o644,
    );
    cpio.file("etc/group", b"root:x:0:\nuser:x:1000:\ntor:x:900:\n", 0o644);
    // QEMU test convenience: build/import/ becomes drive D:.
    let import = build_dir().join("import");
    if let Ok(entries) = fs::read_dir(&import) {
        cpio.dir_mode("mnt/d", 0o755);
        for e in entries.flatten().filter(|e| e.path().is_file()) {
            let name = e.file_name().to_string_lossy().into_owned();
            cpio.file(&format!("mnt/d/{name}"), &fs::read(e.path()).unwrap(), 0o644);
            println!("xtask: D:\\{name} (from build/import)");
        }
    }
    if let Some(n) = custom_module_tree(&mut cpio) {
        println!("xtask: {n} kernel modules (zstd); init loads only those matching present hardware");
    } else if let Some(modules) = boot_modules() {
        cpio.dir("lib/modules");
        let mut list = String::new();
        for (name, data) in &modules {
            let path = format!("lib/modules/{name}.ko");
            cpio.file(&path, data, 0o644);
            list.push_str(&format!("/{path}\n"));
        }
        cpio.file("etc/modules", list.as_bytes(), 0o644);
        println!("xtask: {} kernel modules for {}", modules.len(), BOOT_MODULES.join(", "));
    }
    cpio.file("etc/hostname", b"ferro\n", 0o644);
    let release = format!("NAME=FerroOS\nID=ferroos\nVERSION={}\n", env!("CARGO_PKG_VERSION"));
    cpio.file("etc/os-release", release.as_bytes(), 0o644);

    let out = build_dir().join("initramfs.cpio");
    let bytes = cpio.finish();
    fs::write(&out, &bytes).expect("write initramfs");
    println!("xtask: {} ({} KB)", out.display(), bytes.len() / 1024);
    programs_image();
}

/// Optional programs built by `tor-build` (kept out of the RAM-backed root).
pub(crate) fn programs_dir() -> PathBuf {
    build_dir().join("programs")
}

/// build/programs/debian.img: the Debian layer, built by `debian-build` (see
/// ferro-system's `layer`).
pub(crate) fn layer_image() -> PathBuf {
    programs_dir().join("debian.img")
}

/// build/programs.img: the programs disk (see ferro-system's `programs`).
fn programs_image() -> Option<PathBuf> {
    let files: Vec<(String, Vec<u8>)> = ["arti"].iter().filter_map(|n| Some((n.to_string(), fs::read(programs_dir().join(n)).ok()?))).collect();
    if files.is_empty() {
        return None;
    }
    let path = build_dir().join("programs.img");
    write_package(&path, &files);
    Some(path)
}

/// A programs-disk image holding `files` (name, contents).
pub(crate) fn write_package(path: &Path, files: &[(String, Vec<u8>)]) {
    use ferro_system::programs::{build_header, Entry};
    let mut entries = Vec::new();
    let mut offset = 4096u64;
    for (name, data) in files {
        entries.push(Entry { name: name.clone(), offset, size: data.len() as u64 });
        offset += (data.len() as u64).next_multiple_of(4096);
    }
    let mut img = build_header(&entries);
    for (_, data) in files {
        img.extend_from_slice(data);
        img.resize(img.len().next_multiple_of(4096), 0);
    }
    fs::write(path, &img).expect("write package image");
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    println!("xtask: {} ({} KB: {})", path.display(), img.len() / 1024, names.join(", "));
}

fn run(extra: &[String]) {
    let kernel = env::var_os("FERRO_KERNEL").map_or_else(|| build_dir().join("bzImage"), PathBuf::from);
    if !kernel.exists() {
        eprintln!("xtask: no kernel at {}. Run `cargo xtask kernel`, tools/build-kernel.sh (Linux/WSL), or set FERRO_KERNEL.", kernel.display());
        exit(1);
    }
    let qemu = qemu_binary();
    check(
        Command::new(qemu)
            .args(["-m", "64M", "-vga", "std", "-serial", "mon:stdio", "-no-reboot"])
            .args(["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"])
            // The encrypted vault disk: settings and files survive reboots.
            .arg("-drive")
            .arg(format!("file={},format=raw,if=virtio", vault_image().display()))
            // Optional programs (Tor) and the Debian layer, read-only and
            // verified before use.
            .args(programs_drive())
            .arg("-kernel")
            .arg(&kernel)
            .arg("-initrd")
            .arg(build_dir().join("initramfs.cpio"))
            // no_timer_check: QEMU's software CPU (TCG) can deliver timer IRQs too
            // slowly for the kernel's boot-time IO-APIC check, which then panics.
            // fbdev_emulation=0: ferro-shell draws via DRM, so the kernel's
            // fbdev copy of the screen is pure overhead.
            // ferro.rescue: the root prompt on this serial console.
            // lockdown=integrity: no raw kernel memory access, signed modules only.
            // FerroOS's own kernel has both compiled in; these cover prebuilt ones.
            .args(["-append", "console=ttyS0 ferro.rescue loglevel=4 no_timer_check drm_kms_helper.fbdev_emulation=0 lockdown=integrity ferro.vault=/dev/vda dhash_entries=32768 ihash_entries=16384 thash_entries=4096 uhash_entries=512"])
            .args(extra),
    );
}

fn programs_drive() -> Vec<String> {
    [build_dir().join("programs.img"), layer_image()]
        .iter()
        .filter(|img| img.exists())
        .flat_map(|img| ["-drive".into(), format!("file={},format=raw,if=virtio,readonly=on", img.display())])
        .collect()
}

/// A 16 MB disk image for the vault, created on first use. It only ever
/// holds ciphertext, so it's safe to keep alongside the project.
fn vault_image() -> PathBuf {
    let img = build_dir().join("vault.img");
    if !img.exists() {
        let f = fs::File::create(&img).expect("create vault.img");
        f.set_len(16 << 20).expect("size vault.img");
        println!("xtask: created {} (empty: you'll choose a password at first boot)", img.display());
    }
    img
}

fn prebuilt_dir() -> PathBuf {
    build_dir().join("prebuilt")
}

/// Downloads the prebuilt generic kernel: the one Debian's network installer
/// boots, with the few drivers FerroOS needs from it (~2 MB of modules
/// instead of the whole tree, since build/ may live in a synced folder).
fn fetch_kernel() {
    let dir = prebuilt_dir();
    let _ = fs::remove_dir_all(&dir);
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");

    let inst = debian::installer(&cache);
    println!("xtask: prebuilt kernel {}", inst.kver);
    let modules = dir.join("pkg").join("lib").join("modules").join(&inst.kver);
    fs::create_dir_all(&modules).expect("create build/prebuilt");
    let mut dep = String::new();
    for m in inst.modules(&BOOT_MODULES) {
        fs::write(modules.join(format!("{}.ko", m.name)), &m.data).expect("write module");
        let deps: String = m.deps.iter().map(|d| format!(" {d}.ko")).collect();
        dep.push_str(&format!("{}.ko:{deps}\n", m.name));
    }
    fs::write(modules.join("modules.dep"), dep).expect("write modules.dep");

    fs::copy(&inst.linux, build_dir().join("bzImage")).unwrap_or_else(|e| {
        eprintln!("xtask: {}: {e}", inst.linux.display());
        exit(1)
    });
    fs::write(build_dir().join("kernel-source"), "prebuilt").expect("write kernel-source");
    fs::write(dir.join("VERSION"), &inst.kver).expect("write VERSION");
    println!("xtask: kernel ready: {}", build_dir().join("bzImage").display());
}

/// The module directory and the boot modules' relative paths in load order
/// (dependencies first). `None` without a kernel module tree.
fn resolve_modules() -> Option<(PathBuf, Vec<String>)> {
    // Modules must match the kernel in build/bzImage.
    let custom = fs::read_to_string(build_dir().join("kernel-source")).is_ok_and(|s| s.trim() == "custom");
    let base = if custom { build_dir().join("kernel") } else { prebuilt_dir().join("pkg") };
    let modules = fs::read_dir(base.join("lib").join("modules")).ok()?.flatten().next()?.path();
    let dep_text = fs::read_to_string(modules.join("modules.dep")).ok()?;
    let deps: HashMap<&str, Vec<&str>> =
        dep_text.lines().filter_map(|l| l.split_once(':')).map(|(m, d)| (m, d.split_whitespace().collect())).collect();

    fn visit<'a>(m: &'a str, deps: &HashMap<&'a str, Vec<&'a str>>, order: &mut Vec<&'a str>) {
        if order.contains(&m) {
            return;
        }
        for d in deps.get(m).into_iter().flatten() {
            visit(d, deps, order);
        }
        order.push(m);
    }
    let mut order = Vec::new();
    for want in BOOT_MODULES {
        match deps.keys().find(|k| module_name(k) == want) {
            Some(rel) => visit(rel, &deps, &mut order),
            None => eprintln!("xtask: module {want} not in this kernel (built in?)"),
        }
    }
    let order = order.into_iter().map(str::to_owned).collect();
    Some((modules, order))
}

fn module_name(rel: &str) -> String {
    rel.rsplit('/').next().unwrap_or(rel).trim_end_matches(".gz").trim_end_matches(".ko").to_owned()
}

/// The boot modules plus dependencies, dependencies first, decompressed
/// (the prebuilt kernel can't load gzip modules itself). `None` when the
/// kernel has no module tree.
/// Our own kernel: ship its whole (compressed) module tree with
/// modules.dep and modules.alias; init matches drivers to the hardware it
/// finds and frees the rest. Returns the module count.
fn custom_module_tree(cpio: &mut Cpio) -> Option<usize> {
    if !fs::read_to_string(build_dir().join("kernel-source")).is_ok_and(|s| s.trim() == "custom") {
        return None;
    }
    let base = build_dir().join("kernel");
    let tree = fs::read_dir(base.join("lib").join("modules")).ok()?.flatten().next()?.path();
    let version = tree.file_name()?.to_string_lossy().into_owned();
    let mut count = 0;
    let mut dirs = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    fn walk(dir: &Path, rel: &str, files: &mut Vec<(String, PathBuf)>, dirs: &mut std::collections::BTreeSet<String>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                dirs.insert(r.clone());
                walk(&e.path(), &r, files, dirs);
            } else if ft.is_file() && (name.contains(".ko") || name == "modules.dep" || name == "modules.alias") {
                files.push((r, e.path()));
            }
        }
    }
    walk(&tree, "", &mut files, &mut dirs);
    cpio.dir("lib/modules");
    cpio.dir(&format!("lib/modules/{version}"));
    for d in &dirs {
        cpio.dir(&format!("lib/modules/{version}/{d}"));
    }
    for (rel, path) in files {
        let data = fs::read(&path).ok()?;
        count += usize::from(rel.contains(".ko"));
        cpio.file(&format!("lib/modules/{version}/{rel}"), &data, 0o644);
    }
    Some(count)
}

fn boot_modules() -> Option<Vec<(String, Vec<u8>)>> {
    let (modules, order) = resolve_modules()?;
    let out = order
        .into_iter()
        .map(|rel| {
            let raw = fs::read(modules.join(&rel)).unwrap_or_else(|e| {
                eprintln!("xtask: {rel}: {e}");
                exit(1)
            });
            let data = if rel.ends_with(".gz") {
                let mut v = Vec::new();
                flate2::read::GzDecoder::new(&raw[..]).read_to_end(&mut v).expect("gunzip module");
                v
            } else {
                raw
            };
            (module_name(&rel), data)
        })
        .collect();
    Some(out)
}

/// Minimal writer for the kernel's `newc` cpio initramfs format, so building
/// an image needs neither root nor a `cpio` binary.
#[derive(Default)]
pub(crate) struct Cpio {
    out: Vec<u8>,
    ino: u32,
}

impl Cpio {
    fn entry(&mut self, name: &str, mode: u32, rdev: (u32, u32), data: &[u8]) {
        self.ino += 1;
        let nlink = if mode & 0o170000 == 0o040000 { 2 } else { 1 };
        let fields = [self.ino, mode, 0, 0, nlink, 0, data.len() as u32, 0, 0, rdev.0, rdev.1, name.len() as u32 + 1, 0];
        self.out.extend_from_slice(b"070701");
        for f in fields {
            self.out.extend_from_slice(format!("{f:08X}").as_bytes());
        }
        self.out.extend_from_slice(name.as_bytes());
        self.out.push(0);
        self.pad();
        self.out.extend_from_slice(data);
        self.pad();
    }

    fn pad(&mut self) {
        while !self.out.len().is_multiple_of(4) {
            self.out.push(0);
        }
    }

    pub(crate) fn dir(&mut self, name: &str) {
        self.dir_mode(name, 0o755);
    }

    pub(crate) fn dir_mode(&mut self, name: &str, perm: u32) {
        self.entry(name, 0o040000 | perm, (0, 0), &[]);
    }

    pub(crate) fn symlink(&mut self, name: &str, target: &str) {
        self.entry(name, 0o120777, (0, 0), target.as_bytes());
    }

    pub(crate) fn file(&mut self, name: &str, data: &[u8], perm: u32) {
        self.entry(name, 0o100000 | perm, (0, 0), data);
    }

    pub(crate) fn chardev(&mut self, name: &str, major: u32, minor: u32, perm: u32) {
        self.entry(name, 0o020000 | perm, (major, minor), &[]);
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
        self.entry("TRAILER!!!", 0, (0, 0), &[]);
        self.out
    }
}
