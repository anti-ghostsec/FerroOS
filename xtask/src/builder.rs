//! `cargo xtask kernel-build`: compiles the FerroOS kernel without WSL or a
//! Linux host, inside a throwaway QEMU VM. The VM boots Debian's network
//! installer and installs a minimal Debian into its RAM (tools/vm/init.sh);
//! the source and build live there too, and only the result comes back, as a
//! tar stream on a small raw disk image. Uses WHPX acceleration on Windows
//! when available, otherwise QEMU's software CPU (much slower).

use crate::debian::{self, Installer};
use crate::{build_dir, cache_dir, check, layer_image, programs_dir, qemu_binary, write_package, Cpio};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};

/// Kernel series to build (the release itself is pinned below).
const SERIES: &str = "6.18";
/// Drivers the build VMs need from the installer's kernel: network and disks.
const BUILDER_MODULES: [&str; 2] = ["virtio_net", "virtio_blk"];
/// The build VMs' memory in MB. Debian, its compilers and the build all
/// live in RAM.
const BUILDER_RAM: &str = "4608";

pub fn kernel_build() {
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");
    let hw = std::env::var("FERRO_KERNEL_HW").is_ok_and(|v| v == "1");

    let (version, sha) = kernel_release();
    println!("xtask: building linux-{version}{}", if hw { " with hardware drivers" } else { "" });

    let inst = debian::installer(&cache);

    let root = crate::root();
    let script = fs::read_to_string(root.join("tools/kernel/builder-init.sh"))
        .expect("read builder-init.sh")
        .replace("@VERSION@", &version)
        .replace("@SHA256@", &sha)
        .replace("@HW@", if hw { "1" } else { "0" });
    let extra: Vec<(String, Vec<u8>)> = ["ferro.config", "ferro-hw.config"]
        .iter()
        .map(|name| {
            let text = fs::read_to_string(root.join("tools/kernel").join(name)).expect("read config fragment");
            (format!("builder/{name}"), text.replace('\r', "").into_bytes())
        })
        .collect();
    let initrd = vm_initrd(&inst, "kernel-vm", &script, "95%", &extra);

    let out_img = cache.join("kernel-out.img");
    let f = fs::File::create(&out_img).expect("create result disk");
    f.set_len(256 << 20).expect("size result disk"); // sparse on NTFS/ext4
    drop(f);

    run_builder(&inst, &initrd, &out_img, &[]);
    unpack_result(&out_img);
    let _ = fs::remove_file(&out_img);
    let _ = fs::remove_file(&initrd);
}

/// The kernel release FerroOS is built from, and its SHA-256, both pinned
/// here and reviewed with each bump: a checksum fetched from the same
/// server as the tarball would vouch for nothing. To move to a newer
/// release, take its hash from kernel.org's PGP-signed sha256sums.asc
/// (check the signature) and update both; or, for one build, set
/// FERRO_KERNEL_VERSION and FERRO_KERNEL_SHA256 together.
const KERNEL_VERSION: &str = "6.18.58";
const KERNEL_SHA256: &str = "069b19f9032c4dac8149ae4b6520c35debca0f42a9f6d52ee55ba6ca9b818013";

fn kernel_release() -> (String, String) {
    match (std::env::var("FERRO_KERNEL_VERSION"), std::env::var("FERRO_KERNEL_SHA256")) {
        (Ok(v), Ok(sha)) => (v, sha),
        (Err(_), Err(_)) => {
            debug_assert!(KERNEL_VERSION.starts_with(&format!("{SERIES}.")));
            (KERNEL_VERSION.into(), KERNEL_SHA256.into())
        }
        _ => {
            eprintln!("xtask: set FERRO_KERNEL_VERSION and FERRO_KERNEL_SHA256 together (the hash from kernel.org's signed sha256sums.asc)");
            exit(1)
        }
    }
}

pub(crate) fn curl_text(url: &str) -> String {
    let out = Command::new("curl").args(["-fsSL", url]).output().expect("run curl");
    if !out.status.success() {
        eprintln!("xtask: download failed: {url}");
        exit(1);
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn run_builder(inst: &Installer, initrd: &Path, out_img: &Path, extra_disks: &[&Path]) {
    let cpus = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
    let accel = if cfg!(windows) { "whpx" } else { "kvm" };
    println!("xtask: starting builder VM ({cpus} CPUs, {BUILDER_RAM} MB RAM, accel {accel}, fallback tcg)");
    let mut cmd = Command::new(qemu_binary());
    cmd.args(["-accel", accel, "-accel", "tcg", "-smp", &cpus.to_string(), "-m", BUILDER_RAM])
        .args(["-display", "none", "-no-reboot", "-serial", "stdio"])
        .arg("-kernel")
        .arg(&inst.linux)
        .arg("-initrd")
        .arg(initrd)
        .args(["-append", "console=ttyS0 quiet no_timer_check"])
        .args(["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"])
        .arg("-drive")
        .arg(format!("file={},format=raw,if=virtio", out_img.display()));
    for d in extra_disks {
        cmd.arg("-drive").arg(format!("file={},format=raw,if=virtio", d.display()));
    }
    check(&mut cmd);
}

/// A build VM's initramfs: the installer's own initrd, followed by a second
/// archive the kernel unpacks over it. That one replaces /init with
/// tools/vm/init.sh, and adds `script` as the VM's job (/builder/run.sh), the
/// drivers and debootstrap, and `extra` files. `tmpfs` sizes the RAM disk
/// Debian is installed into.
fn vm_initrd(inst: &Installer, name: &str, script: &str, tmpfs: &str, extra: &[(String, Vec<u8>)]) -> PathBuf {
    let cache = cache_dir().join("builder");
    let init = fs::read_to_string(crate::root().join("tools/vm/init.sh"))
        .expect("read tools/vm/init.sh")
        .replace('\r', "")
        .replace("@SUITE@", debian::SUITE)
        .replace("@TMPFS@", tmpfs);
    let mut cpio = Cpio::default();
    let mut dirs = std::collections::BTreeSet::new();
    let mut parents = |cpio: &mut Cpio, path: &str| {
        let mut acc = String::new();
        for part in path.split('/').collect::<Vec<_>>().split_last().map_or(&[][..], |(_, d)| d) {
            acc = if acc.is_empty() { (*part).to_owned() } else { format!("{acc}/{part}") };
            if dirs.insert(acc.clone()) {
                cpio.dir(&acc);
            }
        }
    };
    parents(&mut cpio, "dev/console");
    cpio.chardev("dev/console", 5, 1, 0o600);
    cpio.file("init", init.as_bytes(), 0o755);
    parents(&mut cpio, "builder/modules/order");
    cpio.file("builder/run.sh", script.replace('\r', "").as_bytes(), 0o755);
    let mut order = String::new();
    for m in inst.modules(&BUILDER_MODULES) {
        cpio.file(&format!("builder/modules/{}.ko", m.name), &m.data, 0o644);
        order.push_str(&format!("{}\n", m.name));
    }
    cpio.file("builder/modules/order", order.as_bytes(), 0o644);
    // debootstrap installs Debian; bootstrap-base has the helper it uses
    // where there's no perl.
    for package in ["debootstrap-udeb", "bootstrap-base"] {
        for f in debian::udeb(&cache, package) {
            if !f.path.contains("debootstrap") {
                continue;
            }
            parents(&mut cpio, &f.path);
            match &f.link {
                Some(target) => cpio.symlink(&f.path, target),
                None => cpio.file(&f.path, &f.data, f.mode),
            }
        }
    }
    for (path, data) in extra {
        parents(&mut cpio, path);
        cpio.file(path, data, 0o644);
    }
    let mut image = fs::read(&inst.initrd).expect("read installer initrd");
    // The kernel only looks for the next archive at a multiple of 4 bytes.
    image.resize(image.len().next_multiple_of(4), 0);
    image.extend_from_slice(&cpio.finish());
    let initrd = cache.join(format!("{name}.cpio"));
    fs::write(&initrd, image).expect("write VM initramfs");
    initrd
}

/// `cargo xtask tor-build`: builds Arti (Tor in Rust) as one static binary
/// in a throwaway VM, into build/programs/arti. It goes on the programs disk,
/// not into the RAM-backed root, so Tor costs no RAM while it's off.
pub fn tor_build() {
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");
    let inst = debian::installer(&cache);
    let script = fs::read_to_string(crate::root().join("tools/tor/builder-tor.sh")).expect("read builder-tor.sh");
    // Bigger than the VM's RAM: the scratch disk backs it as swap.
    let initrd = vm_initrd(&inst, "tor-vm", &script, "14G", &[]);
    let out_img = cache.join("tor-out.img");
    let scratch = cache.join("scratch.img");
    for (img, size) in [(&out_img, 128u64 << 20), (&scratch, 12u64 << 30)] {
        let f = fs::File::create(img).expect("create VM disk");
        f.set_len(size).expect("size VM disk"); // sparse
    }
    run_builder(&inst, &initrd, &out_img, &[&scratch]);
    let _ = fs::remove_file(&scratch);
    let _ = fs::remove_file(&initrd);

    let dest = programs_dir();
    fs::create_dir_all(&dest).unwrap();
    let mut archive = tar::Archive::new(fs::File::open(&out_img).unwrap());
    let mut found = false;
    for entry in archive.entries().expect("read result").flatten() {
        let mut entry = entry;
        let name = entry.path().map(|p| p.to_string_lossy().trim_start_matches("./").to_owned()).unwrap_or_default();
        if name == "arti" || name == "arti.version" {
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            fs::write(dest.join(&name), &data).unwrap();
            found |= name == "arti";
        }
    }
    let _ = fs::remove_file(&out_img);
    if !found {
        eprintln!("xtask: the Tor build VM produced no binary; see its log above");
        exit(1);
    }
    let version = fs::read_to_string(dest.join("arti.version")).unwrap_or_default();
    println!(
        "xtask: Tor service ready: build/programs/arti ({} KB, {}). Rebuild with `cargo xtask initramfs` to include it.",
        fs::metadata(dest.join("arti")).unwrap().len() / 1024,
        version.trim()
    );
}

/// `cargo xtask debian-build`: builds the Debian layer in a throwaway VM,
/// into build/programs/debian.img: a compressed, read-only image of a minimal
/// Debian's /usr. FerroOS mounts it so programs built for ordinary Linux
/// (glibc) run; it rides on the ISO, so installing needs no network.
pub fn debian_build() {
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");
    let inst = debian::installer(&cache);
    let script =
        fs::read_to_string(crate::root().join("tools/debian/builder-debian.sh")).expect("read builder-debian.sh").replace("@SUITE@", debian::SUITE);
    let initrd = vm_initrd(&inst, "debian-vm", &script, "95%", &[]);
    let out_img = cache.join("debian-out.img");
    let f = fs::File::create(&out_img).expect("create result disk");
    f.set_len(512 << 20).expect("size result disk"); // sparse
    drop(f);
    run_builder(&inst, &initrd, &out_img, &[]);
    let _ = fs::remove_file(&initrd);

    let dest = programs_dir();
    fs::create_dir_all(&dest).unwrap();
    let mut archive = tar::Archive::new(fs::File::open(&out_img).unwrap());
    let mut found = false;
    for entry in archive.entries().expect("read result").flatten() {
        let mut entry = entry;
        let name = entry.path().map(|p| p.to_string_lossy().trim_start_matches("./").to_owned()).unwrap_or_default();
        if name == "debian.img" || name == "debian.version" || name == "debian.verity" {
            io::copy(&mut entry, &mut fs::File::create(dest.join(&name)).unwrap()).expect("write Debian layer");
            found |= name == "debian.img";
        }
    }
    let _ = fs::remove_file(&out_img);
    if !found {
        eprintln!("xtask: the Debian build VM produced no image; see its log above");
        exit(1);
    }
    let version = fs::read_to_string(dest.join("debian.version")).unwrap_or_default();
    println!(
        "xtask: Debian layer ready: build/programs/debian.img ({} MB, Debian {}). Rebuild with `cargo xtask initramfs` to include it.",
        fs::metadata(layer_image()).unwrap().len() >> 20,
        version.trim()
    );
}

/// What every FerroOS kernel command line has. The *hash_entries keep the
/// kernel's lookup tables at desktop sizes instead of growing them with RAM.
const CMDLINE: &str =
    "quiet loglevel=3 console=ttyS0 init_on_alloc=1 init_on_free=1 dhash_entries=32768 ihash_entries=16384 thash_entries=4096 uhash_entries=512";

/// The files a machine starts FerroOS from, Secure Boot on or off.
struct BootFiles {
    /// Debian's shim, signed by Microsoft: what firmware starts.
    shim: Vec<u8>,
    /// shim's key manager, for approving this build's key.
    key_manager: Vec<u8>,
    /// Kernel + FerroOS + command line in one signed file, for installed disks.
    installed: Vec<u8>,
    /// The same for the installation medium (a live session).
    live: Vec<u8>,
    /// This build's key, as the key manager reads it.
    certificate: Vec<u8>,
}

/// Signs `kernel` and `initramfs` in a throwaway VM (tools/boot). The key
/// is made on the first run and kept in build/secureboot: whoever has it
/// can sign things your machine will start, so it stays on this computer.
fn boot_files(inst: &Installer, kernel: &[u8], initramfs: &[u8]) -> BootFiles {
    let cache = cache_dir().join("builder");
    let keys = build_dir().join("secureboot");
    fs::create_dir_all(&keys).expect("create build/secureboot");
    // A root prompt on the serial port, for test images only.
    let rescue = if std::env::var("FERRO_RESCUE").is_ok_and(|v| v == "1") { " ferro.rescue" } else { "" };
    let mut extra = vec![
        ("in/bzImage".to_owned(), kernel.to_vec()),
        ("in/initramfs.cpio".to_owned(), initramfs.to_vec()),
        ("in/cmdline-installed".to_owned(), format!("ferro.vault=PARTLABEL=ferro-vault {CMDLINE}{rescue}").into_bytes()),
        ("in/cmdline-live".to_owned(), format!("ferro.live {CMDLINE}{rescue}").into_bytes()),
    ];
    for name in ["ferro-mok.key", "ferro-mok.crt"] {
        if let Ok(data) = fs::read(keys.join(name)) {
            extra.push((format!("in/{name}"), data));
        }
    }
    let script = fs::read_to_string(crate::root().join("tools/boot/builder-boot.sh")).expect("read builder-boot.sh");
    let initrd = vm_initrd(inst, "boot-vm", &script, "95%", &extra);
    let out_img = cache.join("boot-out.img");
    let f = fs::File::create(&out_img).expect("create result disk");
    f.set_len(256 << 20).expect("size result disk"); // sparse
    drop(f);
    run_builder(inst, &initrd, &out_img, &[]);
    let _ = fs::remove_file(&initrd);

    let mut found = std::collections::HashMap::new();
    let mut archive = tar::Archive::new(fs::File::open(&out_img).unwrap());
    for entry in archive.entries().expect("read result").flatten() {
        let mut entry = entry;
        let name = entry.path().map(|p| p.to_string_lossy().trim_start_matches("./").to_owned()).unwrap_or_default();
        let mut data = Vec::new();
        if entry.header().entry_type().is_file() && entry.read_to_end(&mut data).is_ok() {
            found.insert(name, data);
        }
    }
    let _ = fs::remove_file(&out_img);
    let mut take = |name: &str| {
        found.remove(name).unwrap_or_else(|| {
            eprintln!("xtask: the boot-file VM produced no {name}; see its log above");
            exit(1)
        })
    };
    let files = BootFiles {
        shim: take("shimx64.efi"),
        key_manager: take("mmx64.efi"),
        installed: take("ferro-installed.efi"),
        live: take("ferro-live.efi"),
        certificate: take("ferro-mok.der"),
    };
    for name in ["ferro-mok.key", "ferro-mok.crt", "ferro-mok.der"] {
        if !keys.join(name).exists() {
            let data = if name == "ferro-mok.der" { files.certificate.clone() } else { take(name) };
            fs::write(keys.join(name), data).expect("save signing key");
            println!("xtask: saved build/secureboot/{name}");
        }
    }
    files
}

/// `cargo xtask iso`: build/ferroos.iso, a hybrid (BIOS + UEFI, DVD or USB
/// stick) image with a GRUB menu: try FerroOS live, or install it. The
/// installation payload rides along as an extra partition, and the Debian
/// layer as another. On UEFI it starts through shim, so it boots (and
/// installs systems that boot) with Secure Boot on, once this build's key is
/// approved.
pub fn iso() {
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");
    let kernel = fs::read(build_dir().join("bzImage")).expect("build/bzImage (run `cargo xtask kernel-build`)");
    let initramfs = fs::read(build_dir().join("initramfs.cpio")).expect("build/initramfs.cpio");
    let inst = debian::installer(&cache);
    let boot = boot_files(&inst, &kernel, &initramfs);
    let mut files = vec![
        ("bootx64.efi".to_owned(), boot.shim.clone()),
        ("grubx64.efi".to_owned(), boot.installed),
        ("mmx64.efi".to_owned(), boot.key_manager.clone()),
        ("mok.der".to_owned(), boot.certificate.clone()),
    ];
    match fs::read(programs_dir().join("arti")) {
        Ok(arti) => files.push(("arti".into(), arti)),
        Err(_) => println!("xtask: no Tor service (run `cargo xtask tor-build` to include it)"),
    }
    let payload = cache.join("payload.img");
    write_package(&payload, &files);

    let script = fs::read_to_string(crate::root().join("tools/iso/builder-iso.sh")).expect("read builder-iso.sh");
    let mut extra = vec![
        ("in/bzImage".to_owned(), kernel),
        ("in/initramfs.cpio".to_owned(), initramfs),
        ("in/payload.img".to_owned(), fs::read(&payload).unwrap()),
        ("in/boot/shimx64.efi".to_owned(), boot.shim),
        ("in/boot/grubx64.efi".to_owned(), boot.live),
        ("in/boot/mmx64.efi".to_owned(), boot.key_manager),
        ("in/boot/ferro-mok.der".to_owned(), boot.certificate),
    ];
    match fs::read(layer_image()) {
        Ok(layer) => extra.push(("in/debian.img".to_owned(), layer)),
        Err(_) => println!("xtask: no Debian layer (run `cargo xtask debian-build` to include it)"),
    }
    let initrd = vm_initrd(&inst, "iso-vm", &script, "95%", &extra);
    let out_img = cache.join("iso-out.img");
    let f = fs::File::create(&out_img).expect("create result disk");
    f.set_len(1 << 30).expect("size result disk"); // sparse
    drop(f);
    run_builder(&inst, &initrd, &out_img, &[]);
    let _ = fs::remove_file(&initrd);
    let _ = fs::remove_file(&payload);

    let dest = build_dir().join("ferroos.iso");
    let mut archive = tar::Archive::new(fs::File::open(&out_img).unwrap());
    let mut found = false;
    for entry in archive.entries().expect("read result").flatten() {
        let mut entry = entry;
        if entry.path().is_ok_and(|p| p.to_string_lossy().trim_start_matches("./") == "ferroos.iso") {
            io::copy(&mut entry, &mut fs::File::create(&dest).unwrap()).expect("write ferroos.iso");
            found = true;
        }
    }
    let _ = fs::remove_file(&out_img);
    if !found {
        eprintln!("xtask: the ISO build VM produced no image; see its log above");
        exit(1);
    }
    println!(
        "xtask: {} ({} MB). Write it to a USB stick (e.g. with Rufus or Balena Etcher), or boot it in a VM.",
        dest.display(),
        fs::metadata(&dest).unwrap().len() >> 20
    );
}

/// `cargo xtask heavy-iso`: build/ferroos-heavy.iso, FerroOS Heavy: a small
/// Debian base with our own desktop (ferro-wm) and the hardening in heavy/.
/// It runs straight from the medium and installs to a disk with no network
/// (`ferro-install`): LUKS2, Debian's signed boot chain for Secure Boot.
/// Drivers, Steam and the like are `apt install`ed afterwards.
///
/// `FERRO_HEAVY_TEST=1` makes a test image that also logs on over the
/// serial line, so a VM can be driven without its screen.
pub fn heavy_iso() {
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");
    let root = crate::root();
    let heavy = root.join("heavy");
    let read = |rel: &str| fs::read_to_string(heavy.join(rel)).unwrap_or_else(|e| panic!("heavy/{rel}: {e}")).replace('\r', "");
    let packages: Vec<String> =
        read("packages.txt").lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_owned).collect();
    let test = std::env::var("FERRO_HEAVY_TEST").is_ok_and(|v| v == "1");
    if test {
        println!("xtask: TEST image: logs on by itself over the serial line");
    }
    let programs: Vec<(String, Vec<u8>)> = [
        "ferro-wm",
        "ferro-files",
        "ferro-properties",
        "ferro-rename",
        "ferro-cmd",
        "ferro-logon",
        "ferro-setup",
        "ferro-taskmgr",
        "ferro-appearance",
        "ferro-network",
        "ferro-volume",
        "ferro-datetime",
        "ferro-associations",
        "ferro-screensaver",
        "ferro-viewer",
        "ferro-jail",
        "ferro-program-props",
        "ferro-snip",
        "ferro-info",
    ]
    .iter()
    .map(|name| (format!("in/bin/{name}"), fs::read(crate::build_static(name)).unwrap_or_else(|e| panic!("{name}: {e}"))))
    .collect();
    let script = fs::read_to_string(root.join("tools/heavy/builder-base.sh"))
        .expect("read builder-base.sh")
        .replace("@SUITE@", debian::SUITE)
        .replace("@PACKAGES@", &packages.join(" "))
        .replace("@BOOT_OPTIONS@", if test { "console=tty0 console=ttyS0,115200 ferro.test" } else { "" });
    let mut extra = vec![
        ("in/heavy/harden.sh".to_owned(), read("harden.sh").replace("@SUITE@", debian::SUITE).into_bytes()),
        ("in/heavy/ferro-install".to_owned(), read("ferro-install").into_bytes()),
        ("in/heavy/ferro-heavy".to_owned(), read("ferro-heavy").into_bytes()),
        ("in/heavy/ferro-terminal".to_owned(), read("ferro-terminal").into_bytes()),
        ("in/heavy/ferro-tor".to_owned(), read("ferro-tor").into_bytes()),
        ("in/heavy/ferro-switch-hardware".to_owned(), read("ferro-switch-hardware").into_bytes()),
        ("in/heavy/ferro-vpn".to_owned(), read("ferro-vpn").into_bytes()),
        ("in/heavy/ferro-vm-vpn-killswitch".to_owned(), read("ferro-vm-vpn-killswitch").into_bytes()),
        ("in/heavy/help.txt".to_owned(), read("help.txt").into_bytes()),
        ("in/heavy/ferro-torbrowser".to_owned(), read("ferro-torbrowser").into_bytes()),
        ("in/heavy/tor-bridges.conf".to_owned(), read("tor-bridges.conf").into_bytes()),
        ("in/heavy/ferro-inhibit".to_owned(), read("ferro-inhibit").into_bytes()),
        ("in/heavy/ferro-vm".to_owned(), read("ferro-vm").into_bytes()),
        ("in/heavy/ferro-uninstall".to_owned(), read("ferro-uninstall").into_bytes()),
        ("in/heavy/ferro-installer".to_owned(), read("ferro-installer").into_bytes()),
        ("in/heavy/ferro-video".to_owned(), read("ferro-video").into_bytes()),
        ("in/heavy/ferro-update".to_owned(), read("ferro-update").into_bytes()),
    ];
    // The Video Player's mpv settings and scripts.
    extra.extend(
        ["mpv.conf", "input.conf", "osc.lua", "ferro-thumbs.lua", "ferro-delete.lua", "ferro-menu.lua"]
            .map(|name| (format!("in/heavy/mpv/{name}"), read(&format!("mpv/{name}")).into_bytes())),
    );
    // The logon screen's scripts.
    extra.extend(
        ["logon", "logon-session", "check-password", "unlock-disk", "initramfs-duress"]
            .map(|name| (format!("in/heavy/{name}"), read(name).into_bytes())),
    );
    let wp_dir = heavy.join("wallpapers");
    if let Ok(entries) = fs::read_dir(&wp_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Ok(bytes) = fs::read(&p) {
                    let file_name = p.file_name().unwrap().to_string_lossy();
                    extra.push((format!("in/heavy/wallpapers/{file_name}"), bytes));
                }
            }
        }
    }
    extra.extend(programs);
    let inst = debian::installer(&cache);
    // The system is unpacked, packed and copied into the ISO, all in the
    // VM's RAM disk: bigger than the VM's RAM (the full desktop, LibreWolf
    // and Firefox, firmware, and the updates being installed), so the
    // scratch disk backs it as swap, as for the Tor build.
    let initrd = vm_initrd(&inst, "heavy-vm", &script, "14G", &extra);
    let out_img = cache.join("heavy-out.img");
    let scratch = cache.join("heavy-scratch.img");
    for (img, size) in [(&out_img, 2u64 << 30), (&scratch, 12u64 << 30)] {
        let f = fs::File::create(img).expect("create VM disk");
        f.set_len(size).expect("size VM disk"); // sparse
    }
    run_builder(&inst, &initrd, &out_img, &[&scratch]);
    let _ = fs::remove_file(&scratch);
    let _ = fs::remove_file(&initrd);

    let dest = build_dir().join("ferroos-heavy.iso");
    let mut archive = tar::Archive::new(fs::File::open(&out_img).unwrap());
    let mut found = false;
    for entry in archive.entries().expect("read result").flatten() {
        let mut entry = entry;
        let name = entry.path().map(|p| p.to_string_lossy().trim_start_matches("./").to_owned()).unwrap_or_default();
        if name == "ferroos-heavy.iso" {
            io::copy(&mut entry, &mut fs::File::create(&dest).unwrap()).expect("write ferroos-heavy.iso");
            found = true;
        } else if name == "ferroos-heavy-update.tar.gz" {
            // FerroOS's own programs from this image, for ferro-update.
            io::copy(&mut entry, &mut fs::File::create(build_dir().join(&name)).unwrap()).expect("write the update bundle");
        }
    }
    let _ = fs::remove_file(&out_img);
    if !found {
        eprintln!("xtask: the Heavy build VM produced no image; see its log above");
        exit(1);
    }
    println!("xtask: {} ({} MB)", dest.display(), fs::metadata(&dest).unwrap().len() >> 20);
}

/// `cargo xtask wm-test`: runs ferro-wm (FerroOS Heavy's window manager) on
/// a virtual screen in a throwaway VM, with ordinary programs and a scripted
/// mouse, and saves a screenshot of each step in build/wm-test.
pub fn wm_test() {
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");
    let extra: Vec<(String, Vec<u8>)> = [
        "ferro-wm",
        "ferro-files",
        "ferro-properties",
        "ferro-taskmgr",
        "ferro-appearance",
        "ferro-network",
        "ferro-volume",
        "ferro-datetime",
        "ferro-associations",
        "ferro-screensaver",
        "ferro-viewer",
    ]
    .iter()
    .map(|name| (format!("in/{name}"), fs::read(crate::build_static(name)).unwrap_or_else(|e| panic!("{name}: {e}"))))
    .collect();
    let inst = debian::installer(&cache);
    let script = fs::read_to_string(crate::root().join("tools/heavy/wm-test.sh")).expect("read wm-test.sh");
    let initrd = vm_initrd(&inst, "wm-vm", &script, "95%", &extra);
    let out_img = cache.join("wm-out.img");
    let f = fs::File::create(&out_img).expect("create result disk");
    f.set_len(64 << 20).expect("size result disk"); // sparse
    drop(f);
    run_builder(&inst, &initrd, &out_img, &[]);
    let _ = fs::remove_file(&initrd);

    let dest = build_dir().join("wm-test");
    let _ = fs::remove_dir_all(&dest);
    fs::create_dir_all(&dest).unwrap();
    let mut archive = tar::Archive::new(fs::File::open(&out_img).unwrap());
    let mut count = 0;
    for entry in archive.entries().expect("read result").flatten() {
        let mut entry = entry;
        let name = entry.path().map(|p| p.to_string_lossy().trim_start_matches("./").to_owned()).unwrap_or_default();
        if entry.header().entry_type().is_file() && !name.contains('/') {
            io::copy(&mut entry, &mut fs::File::create(dest.join(&name)).unwrap()).expect("write result");
            count += usize::from(name.ends_with(".png"));
        }
    }
    let _ = fs::remove_file(&out_img);
    println!("xtask: {count} screenshots in {}", dest.display());
}

/// `cargo xtask wg-test-server`: a throwaway WireGuard VPN server in a VM,
/// for testing FerroOS's VPN without a provider. It forwards its clients'
/// traffic to the internet, and writes a client profile to
/// build/import/wg-test.conf (drive D: in FerroOS). Stop it with Ctrl+C.
pub fn wg_test_server() {
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");
    let inst = debian::installer(&cache);
    let script = fs::read_to_string(crate::root().join("tools/wg-test/server.sh")).expect("read server.sh");
    let initrd = vm_initrd(&inst, "wg-vm", &script, "95%", &[]);
    let accel = if cfg!(windows) { "whpx" } else { "kvm" };
    println!("xtask: starting WireGuard test server (UDP 51820 on this machine)");
    let mut child = Command::new(qemu_binary())
        .args(["-accel", accel, "-accel", "tcg", "-smp", "2", "-m", "2048"])
        .args(["-display", "none", "-no-reboot", "-serial", "stdio"])
        .arg("-kernel")
        .arg(&inst.linux)
        .arg("-initrd")
        .arg(&initrd)
        .args(["-append", "console=ttyS0 quiet no_timer_check"])
        .args(["-netdev", "user,id=n0,hostfwd=udp:127.0.0.1:51820-:51820", "-device", "virtio-net-pci,netdev=n0"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("start QEMU");
    let mut conf: Option<String> = None;
    for line in BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok) {
        let line = line.trim_end_matches('\r').to_owned();
        match (&mut conf, line.as_str()) {
            (_, "FERRO-WG-CONFIG-BEGIN") => conf = Some(String::new()),
            (Some(c), "FERRO-WG-CONFIG-END") => {
                let dir = build_dir().join("import");
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("wg-test.conf"), c.as_bytes()).unwrap();
                println!("xtask: client profile written to build/import/wg-test.conf (D:\\wg-test.conf in FerroOS)");
                conf = None;
            }
            (Some(c), l) => {
                c.push_str(l);
                c.push('\n');
            }
            (None, l) => {
                println!("{l}");
                let _ = io::stdout().flush();
            }
        }
    }
    let _ = child.wait();
}

fn unpack_result(out_img: &Path) {
    let dest = build_dir().join("kernel");
    let _ = fs::remove_dir_all(&dest);
    fs::create_dir_all(&dest).unwrap();
    let mut archive = tar::Archive::new(fs::File::open(out_img).unwrap());
    // Module trees contain symlinks (build/source) we don't need.
    let mut count = 0;
    for entry in archive.entries().expect("read result") {
        let mut entry = entry.expect("result entry");
        if entry.header().entry_type().is_symlink() {
            continue;
        }
        entry.unpack_in(&dest).expect("unpack result");
        count += 1;
    }
    let bz = dest.join("bzImage");
    if count == 0 || !bz.exists() {
        eprintln!("xtask: the builder VM produced no kernel; see its log above");
        exit(1);
    }
    fs::copy(&bz, build_dir().join("bzImage")).unwrap();
    fs::write(build_dir().join("kernel-source"), "custom").unwrap();
    let version = fs::read_to_string(dest.join("VERSION")).unwrap_or_default();
    println!(
        "xtask: custom kernel {} ready: build/bzImage ({} KB), modules in build/kernel/lib/modules",
        version.trim(),
        fs::metadata(&bz).unwrap().len() / 1024
    );
}
