//! `cargo xtask kernel-build`: compiles the FerroOS kernel without WSL or a
//! Linux host, inside a throwaway QEMU VM (a minimal Linux userland and
//! kernel). The source and build live in the VM's RAM; only the result comes
//! back, as a tar stream on a small raw disk image. Uses WHPX acceleration on
//! Windows when available, otherwise QEMU's software CPU (much slower).

use crate::{build_boot_loader, build_dir, cache_dir, check, curl, programs_dir, qemu_binary, write_package, Cpio};
use flate2::read::{GzDecoder, MultiGzDecoder};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};

/// Source of the build VM's userland and kernel (Alpine Linux). Used only at
/// build time; nothing from it ships in FerroOS.
const BUILD_VM_MIRROR: &str = "https://dl-cdn.alpinelinux.org/alpine/latest-stable";
/// Kernel series to build; the newest release in it is picked.
const SERIES: &str = "6.18";
const BUILDER_MODULES: [&str; 4] = ["failover", "net_failover", "virtio_net", "virtio_blk"];

pub fn kernel_build() {
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");
    let hw = std::env::var("FERRO_KERNEL_HW").is_ok_and(|v| v == "1");

    let (version, sha) = kernel_release();
    println!("xtask: building linux-{version}{}", if hw { " with hardware drivers" } else { "" });

    let (vmlinuz, modules) = builder_kernel(&cache);
    let rootfs = minirootfs(&cache);

    let root = crate::root();
    let script = fs::read_to_string(root.join("tools/kernel/builder-init.sh"))
        .expect("read builder-init.sh")
        .replace('\r', "")
        .replace("@VERSION@", &version)
        .replace("@SHA256@", &sha)
        .replace("@HW@", if hw { "1" } else { "0" });

    let mut cpio = Cpio::default();
    tar_into_cpio(&rootfs, &mut cpio);
    cpio.chardev("dev/console", 5, 1, 0o600);
    cpio.file("init", script.as_bytes(), 0o755);
    cpio.dir("builder");
    cpio.dir("builder/modules");
    for name in ["ferro.config", "ferro-hw.config"] {
        let text = fs::read_to_string(root.join("tools/kernel").join(name)).expect("read config fragment");
        cpio.file(&format!("builder/{name}"), text.replace('\r', "").as_bytes(), 0o644);
    }
    for (name, data) in &modules {
        cpio.file(&format!("builder/modules/{name}.ko"), data, 0o644);
    }
    let initrd = cache.join("builder.cpio");
    fs::write(&initrd, cpio.finish()).expect("write builder initramfs");

    let out_img = cache.join("out.img");
    let f = fs::File::create(&out_img).expect("create out.img");
    f.set_len(256 << 20).expect("size out.img"); // sparse on NTFS/ext4
    drop(f);

    run_builder(&vmlinuz, &initrd, &out_img, &[]);
    unpack_result(&out_img);
    let _ = fs::remove_file(&out_img);
    let _ = fs::remove_file(&initrd);
}

/// Newest `SERIES` release from kernel.org and its published SHA-256.
fn kernel_release() -> (String, String) {
    let version = match std::env::var("FERRO_KERNEL_VERSION") {
        Ok(v) => v,
        Err(_) => {
            let json = curl_text("https://www.kernel.org/releases.json");
            let prefix = format!("\"version\": \"{SERIES}.");
            json.match_indices(&prefix)
                .filter_map(|(i, _)| {
                    let rest = &json[i + prefix.len()..];
                    rest[..rest.find('"')?].parse::<u32>().ok()
                })
                .max()
                .map(|patch| format!("{SERIES}.{patch}"))
                .unwrap_or_else(|| {
                    eprintln!("xtask: no {SERIES}.x release in releases.json; set FERRO_KERNEL_VERSION");
                    exit(1)
                })
        }
    };
    let major = version.split('.').next().unwrap();
    let sums = curl_text(&format!("https://cdn.kernel.org/pub/linux/kernel/v{major}.x/sha256sums.asc"));
    let file = format!("linux-{version}.tar.xz");
    let sha = sums.lines().find_map(|l| l.strip_suffix(&file).map(|h| h.trim().to_owned())).unwrap_or_else(|| {
        eprintln!("xtask: {file} not in kernel.org sha256sums");
        exit(1)
    });
    (version, sha)
}

fn curl_text(url: &str) -> String {
    let out = Command::new("curl").args(["-fsSL", url]).output().expect("run curl");
    if !out.status.success() {
        eprintln!("xtask: download failed: {url}");
        exit(1);
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The build VM's kernel package: it boots the VM, and the virtio network and
/// block modules come from the same package.
fn builder_kernel(cache: &Path) -> (PathBuf, Vec<(String, Vec<u8>)>) {
    let index = cache.join("APKINDEX.tar.gz");
    curl(&format!("{BUILD_VM_MIRROR}/main/x86_64/APKINDEX.tar.gz"), &index);
    let mut text = String::new();
    // Signed index: like an .apk, several gzip members forming one tar.
    let mut archive = tar::Archive::new(MultiGzDecoder::new(fs::File::open(&index).unwrap()));
    archive.set_ignore_zeros(true);
    for entry in archive.entries().expect("read APKINDEX") {
        let mut entry = entry.expect("APKINDEX entry");
        if entry.path().is_ok_and(|p| p.as_os_str() == "APKINDEX") {
            entry.read_to_string(&mut text).expect("read APKINDEX");
        }
    }
    let version = text
        .split("\n\n")
        .find(|rec| rec.lines().any(|l| l == "P:linux-virt"))
        .and_then(|rec| rec.lines().find_map(|l| l.strip_prefix("V:")))
        .expect("linux-virt in APKINDEX")
        .to_owned();

    let apk = cache.join(format!("linux-virt-{version}.apk"));
    if !apk.exists() {
        println!("xtask: downloading build VM kernel ({version})");
        curl(&format!("{BUILD_VM_MIRROR}/main/x86_64/linux-virt-{version}.apk"), &apk);
    }
    let vmlinuz = cache.join("builder-vmlinuz");
    let mut found: HashMap<String, Vec<u8>> = HashMap::new();
    // An .apk is three concatenated gzip members forming one tar stream.
    let mut archive = tar::Archive::new(MultiGzDecoder::new(fs::File::open(&apk).unwrap()));
    archive.set_ignore_zeros(true);
    for entry in archive.entries().expect("read apk") {
        let mut entry = entry.expect("apk entry");
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        if path == "boot/vmlinuz-virt" {
            io::copy(&mut entry, &mut fs::File::create(&vmlinuz).unwrap()).expect("extract vmlinuz");
            continue;
        }
        let name = path.rsplit('/').next().unwrap_or("").trim_end_matches(".gz").trim_end_matches(".ko");
        if path.ends_with(".ko.gz") && BUILDER_MODULES.contains(&name) {
            let mut data = Vec::new();
            GzDecoder::new(entry).read_to_end(&mut data).expect("gunzip module");
            found.insert(name.to_owned(), data);
        }
    }
    let modules =
        BUILDER_MODULES.iter().map(|m| (m.to_string(), found.remove(*m).unwrap_or_else(|| panic!("module {m} missing from apk")))).collect();
    (vmlinuz, modules)
}

/// The build VM's minimal root filesystem, verified against the SHA-256 in
/// the publisher's release list.
fn minirootfs(cache: &Path) -> PathBuf {
    let yaml = curl_text(&format!("{BUILD_VM_MIRROR}/releases/x86_64/latest-releases.yaml"));
    // Entries are YAML maps; find the one whose `file:` is the minirootfs.
    let block = yaml.split("\n-").find(|b| b.contains("file: alpine-minirootfs-")).expect("minirootfs in latest-releases.yaml");
    let field = |k: &str| block.lines().find_map(|l| l.trim().strip_prefix(k).map(|v| v.trim().to_owned())).expect("yaml field");
    let (file, sha) = (field("file:"), field("sha256:"));
    let path = cache.join(&file);
    if !path.exists() {
        println!("xtask: downloading build VM root filesystem");
        curl(&format!("{BUILD_VM_MIRROR}/releases/x86_64/{file}"), &path);
    }
    let digest = Sha256::digest(fs::read(&path).unwrap());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if hex != sha {
        let _ = fs::remove_file(&path);
        eprintln!("xtask: {file}: checksum mismatch (expected {sha}, got {hex})");
        exit(1);
    }
    path
}

/// Repacks a .tar.gz as initramfs entries, keeping modes and symlinks
/// (busybox's applets are symlinks, which unpacking on Windows would lose).
fn tar_into_cpio(tgz: &Path, cpio: &mut Cpio) {
    let mut archive = tar::Archive::new(GzDecoder::new(fs::File::open(tgz).unwrap()));
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    for entry in archive.entries().expect("read minirootfs") {
        let mut entry = entry.expect("minirootfs entry");
        let path = entry.path().unwrap().to_string_lossy().trim_start_matches("./").trim_end_matches('/').to_owned();
        if path.is_empty() {
            continue;
        }
        let mode = entry.header().mode().unwrap_or(0o644) & 0o7777;
        match entry.header().entry_type() {
            tar::EntryType::Directory => cpio.dir_mode(&path, mode),
            tar::EntryType::Symlink => {
                let target = entry.link_name().unwrap().unwrap().to_string_lossy().into_owned();
                cpio.symlink(&path, &target);
            }
            tar::EntryType::Link => {
                let target = entry.link_name().unwrap().unwrap().to_string_lossy().trim_start_matches("./").to_owned();
                let data = files.get(&target).cloned().unwrap_or_default();
                cpio.file(&path, &data, mode);
            }
            tar::EntryType::Regular => {
                let mut data = Vec::new();
                entry.read_to_end(&mut data).unwrap();
                cpio.file(&path, &data, mode);
                files.insert(path, data);
            }
            _ => {}
        }
    }
}

fn run_builder(vmlinuz: &Path, initrd: &Path, out_img: &Path, extra_disks: &[&Path]) {
    let cpus = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
    let accel = if cfg!(windows) { "whpx" } else { "kvm" };
    println!("xtask: starting builder VM ({cpus} CPUs, 3.5 GB RAM, accel {accel}, fallback tcg)");
    let mut cmd = Command::new(qemu_binary());
    cmd.args(["-accel", accel, "-accel", "tcg", "-smp", &cpus.to_string(), "-m", "3584"])
        .args(["-display", "none", "-no-reboot", "-serial", "stdio"])
        .arg("-kernel")
        .arg(vmlinuz)
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

/// The build VM's initramfs: its userland, `script` as /init, and the
/// virtio drivers it needs to reach the network and disks.
fn vm_initrd(cache: &Path, name: &str, script: &str, modules: &[(String, Vec<u8>)], extra: &[(String, Vec<u8>)]) -> PathBuf {
    let rootfs = minirootfs(cache);
    let mut cpio = Cpio::default();
    tar_into_cpio(&rootfs, &mut cpio);
    cpio.chardev("dev/console", 5, 1, 0o600);
    cpio.file("init", script.replace('\r', "").as_bytes(), 0o755);
    cpio.dir("builder");
    cpio.dir("builder/modules");
    for (name, data) in modules {
        cpio.file(&format!("builder/modules/{name}.ko"), data, 0o644);
    }
    let mut dirs = std::collections::BTreeSet::new();
    for (path, data) in extra {
        let mut acc = String::new();
        for part in path.split('/').collect::<Vec<_>>().split_last().map_or(&[][..], |(_, d)| d) {
            acc = if acc.is_empty() { (*part).to_owned() } else { format!("{acc}/{part}") };
            if dirs.insert(acc.clone()) {
                cpio.dir(&acc);
            }
        }
        cpio.file(path, data, 0o644);
    }
    let initrd = cache.join(format!("{name}.cpio"));
    fs::write(&initrd, cpio.finish()).expect("write VM initramfs");
    initrd
}

/// `cargo xtask tor-build`: builds Arti (Tor in Rust) as one static binary
/// in a throwaway VM, into build/programs/arti. It goes on the programs disk,
/// not into the RAM-backed root, so Tor costs no RAM while it's off.
pub fn tor_build() {
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");
    let (vmlinuz, modules) = builder_kernel(&cache);
    let script = fs::read_to_string(crate::root().join("tools/tor/builder-tor.sh")).expect("read builder-tor.sh");
    let initrd = vm_initrd(&cache, "tor-vm", &script, &modules, &[]);
    let out_img = cache.join("out.img");
    let scratch = cache.join("scratch.img");
    for (img, size) in [(&out_img, 128u64 << 20), (&scratch, 12u64 << 30)] {
        let f = fs::File::create(img).expect("create VM disk");
        f.set_len(size).expect("size VM disk"); // sparse
    }
    run_builder(&vmlinuz, &initrd, &out_img, &[&scratch]);
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

/// `cargo xtask iso`: build/ferroos.iso, a hybrid (BIOS + UEFI, DVD or USB
/// stick) image with a GRUB menu: try FerroOS live, or install it. The
/// installation payload rides along as an extra partition.
pub fn iso() {
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");
    let kernel = fs::read(build_dir().join("bzImage")).expect("build/bzImage (run `cargo xtask kernel-build`)");
    let initramfs = fs::read(build_dir().join("initramfs.cpio")).expect("build/initramfs.cpio");
    let loader = fs::read(build_boot_loader()).expect("ferro-boot.efi");
    let mut files = vec![("bzImage".to_owned(), kernel.clone()), ("initramfs".to_owned(), initramfs.clone()), ("bootx64.efi".to_owned(), loader)];
    match fs::read(programs_dir().join("arti")) {
        Ok(arti) => files.push(("arti".into(), arti)),
        Err(_) => println!("xtask: no Tor service (run `cargo xtask tor-build` to include it)"),
    }
    let payload = cache.join("payload.img");
    write_package(&payload, &files);

    let (vmlinuz, modules) = builder_kernel(&cache);
    let script = fs::read_to_string(crate::root().join("tools/iso/builder-iso.sh")).expect("read builder-iso.sh");
    let extra = vec![
        ("in/bzImage".to_owned(), kernel),
        ("in/initramfs.cpio".to_owned(), initramfs),
        ("in/payload.img".to_owned(), fs::read(&payload).unwrap()),
    ];
    let initrd = vm_initrd(&cache, "iso-vm", &script, &modules, &extra);
    let out_img = cache.join("out.img");
    let f = fs::File::create(&out_img).expect("create out.img");
    f.set_len(256 << 20).expect("size out.img");
    drop(f);
    run_builder(&vmlinuz, &initrd, &out_img, &[]);
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

/// `cargo xtask wg-test-server`: a throwaway WireGuard VPN server in a VM,
/// for testing FerroOS's VPN without a provider. It forwards its clients'
/// traffic to the internet, and writes a client profile to
/// build/import/wg-test.conf (drive D: in FerroOS). Stop it with Ctrl+C.
pub fn wg_test_server() {
    let cache = cache_dir().join("builder");
    fs::create_dir_all(&cache).expect("create builder cache");
    let (vmlinuz, modules) = builder_kernel(&cache);
    let tree = module_tree(&cache);
    let script = fs::read_to_string(crate::root().join("tools/wg-test/server.sh")).expect("read server.sh");
    let initrd = vm_initrd(&cache, "wg-vm", &script, &modules, &tree);
    let accel = if cfg!(windows) { "whpx" } else { "kvm" };
    println!("xtask: starting WireGuard test server (UDP 51820 on this machine)");
    let mut child = Command::new(qemu_binary())
        .args(["-accel", accel, "-accel", "tcg", "-smp", "1", "-m", "512"])
        .args(["-display", "none", "-no-reboot", "-serial", "stdio"])
        .arg("-kernel")
        .arg(&vmlinuz)
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

/// Every driver of the build VM's kernel, for VMs that need more than the
/// virtio basics (the WireGuard test server needs WireGuard and NAT).
fn module_tree(cache: &Path) -> Vec<(String, Vec<u8>)> {
    let apk = fs::read_dir(cache)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("linux-virt-") && n.to_string_lossy().ends_with(".apk")))
        .expect("build VM kernel package");
    let mut out = Vec::new();
    let mut archive = tar::Archive::new(MultiGzDecoder::new(fs::File::open(&apk).unwrap()));
    archive.set_ignore_zeros(true);
    for entry in archive.entries().expect("read apk") {
        let mut entry = entry.expect("apk entry");
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        if path.starts_with("lib/modules/") {
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            out.push((path, data));
        }
    }
    out
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
