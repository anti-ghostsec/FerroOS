//! The Debian pieces of the build: the network installer's (netboot) kernel
//! and initrd, which every build VM boots and `cargo xtask kernel` borrows
//! its quick-start kernel from, and the installer packages (udebs) holding
//! the drivers and tools that initrd leaves out.

use crate::builder::curl_text;
use crate::curl;
use flate2::read::GzDecoder;
use lzma_rust2::XzReader;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

pub const MIRROR: &str = "https://deb.debian.org/debian";
/// The Debian release to build with.
pub const SUITE: &str = "stable";

/// Debian's network installer: a kernel and a small busybox system.
pub struct Installer {
    pub linux: PathBuf,
    pub initrd: PathBuf,
    /// The kernel's release, as `uname -r` prints it.
    pub kver: String,
    /// Its drivers by name, still xz-compressed.
    modules: HashMap<String, Vec<u8>>,
}

/// One driver, ready to load.
pub struct Module {
    pub name: String,
    /// The drivers it needs loaded first (built-in ones left out).
    pub deps: Vec<String>,
    pub data: Vec<u8>,
}

/// A file or symlink from an installer package.
pub struct UdebFile {
    pub path: String,
    pub mode: u32,
    pub data: Vec<u8>,
    pub link: Option<String>,
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads `url` to `path` unless it's cached, and checks its SHA-256.
fn fetch_verified(url: &str, path: &Path, sha: &str) -> Vec<u8> {
    if !path.exists() {
        curl(url, path);
    }
    let data = fs::read(path).unwrap_or_default();
    let got = sha256_hex(&data);
    if got != sha {
        let _ = fs::remove_file(path);
        eprintln!("xtask: {url}: checksum mismatch (expected {sha}, got {got})");
        exit(1);
    }
    data
}

fn unxz(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    XzReader::new(data, true).read_to_end(&mut out).expect("unxz");
    out
}

/// Module names treat `-` and `_` alike.
fn module_key(name: &str) -> String {
    name.replace('-', "_")
}

/// The module name of a `.../name.ko.xz` path.
fn module_of(path: &str) -> Option<String> {
    Some(module_key(path.rsplit('/').next()?.strip_suffix(".ko.xz")?))
}

/// The entries of a `newc` cpio archive: name, mode, contents.
fn cpio_entries(data: &[u8]) -> Vec<(String, u32, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while data.len() >= i + 110 && &data[i..i + 6] == b"070701" {
        let field = |n: usize| {
            let s = std::str::from_utf8(&data[i + 6 + n * 8..i + 14 + n * 8]).unwrap_or("0");
            u32::from_str_radix(s, 16).unwrap_or(0) as usize
        };
        let (mode, size, name_len) = (field(1) as u32, field(6), field(11));
        let name_end = i + 110 + name_len;
        let start = name_end.next_multiple_of(4);
        if name_len == 0 || start + size > data.len() {
            break;
        }
        let name = String::from_utf8_lossy(&data[i + 110..name_end - 1]).into_owned();
        if name == "TRAILER!!!" {
            break;
        }
        out.push((name, mode, &data[start..start + size]));
        i = (start + size).next_multiple_of(4);
    }
    out
}

/// The members of an `ar` archive (a .deb or .udeb): name, contents.
fn ar_members(data: &[u8]) -> Vec<(String, &[u8])> {
    let mut out = Vec::new();
    if data.get(..8) != Some(b"!<arch>\n") {
        return out;
    }
    let mut i = 8;
    while data.len() >= i + 60 {
        let name = String::from_utf8_lossy(&data[i..i + 16]).trim().trim_end_matches('/').to_owned();
        let Ok(size) = String::from_utf8_lossy(&data[i + 48..i + 58]).trim().parse::<usize>() else { break };
        let start = i + 60;
        if start + size > data.len() {
            break;
        }
        out.push((name, &data[start..start + size]));
        i = start + size + (size & 1);
    }
    out
}

/// The installer's kernel and initrd, checked against the SHA-256 list
/// published next to them. Cached by checksum, so a Debian point release
/// fetches the new pair.
pub fn installer(cache: &Path) -> Installer {
    let images = format!("{MIRROR}/dists/{SUITE}/main/installer-amd64/current/images");
    let sums = curl_text(&format!("{images}/SHA256SUMS"));
    let get = |name: &str| {
        let rel = format!("netboot/debian-installer/amd64/{name}");
        let sha = sums.lines().find_map(|l| l.strip_suffix(&format!("./{rel}")).map(|h| h.trim().to_owned())).unwrap_or_else(|| {
            eprintln!("xtask: {rel} not in Debian's installer SHA256SUMS");
            exit(1)
        });
        let path = cache.join(format!("debian-installer-{}-{name}", &sha[..16]));
        if !path.exists() {
            println!("xtask: downloading Debian network installer ({name})");
        }
        (fetch_verified(&format!("{images}/{rel}"), &path, &sha), path)
    };
    let (_, linux) = get("linux");
    let (packed, initrd) = get("initrd.gz");

    let mut cpio = Vec::new();
    GzDecoder::new(&packed[..]).read_to_end(&mut cpio).expect("gunzip installer initrd");
    let mut kver = None;
    let mut modules = HashMap::new();
    for (name, _, data) in cpio_entries(&cpio) {
        let Some(rest) = name.strip_prefix("usr/lib/modules/").or_else(|| name.strip_prefix("lib/modules/")) else { continue };
        kver.get_or_insert_with(|| rest.split('/').next().unwrap_or(rest).to_owned());
        if let Some(module) = module_of(&name) {
            modules.insert(module, data.to_vec());
        }
    }
    let kver = kver.unwrap_or_else(|| {
        eprintln!("xtask: no kernel modules in Debian's installer initrd");
        exit(1)
    });
    // A real installation fetches more drivers as it goes. These packages
    // have the virtio disk and, for FerroOS's Debian layer, squashfs.
    for package in ["scsi-modules", "squashfs-modules"] {
        for f in udeb(cache, &format!("{package}-{kver}-di")) {
            if let Some(module) = module_of(&f.path) {
                modules.entry(module).or_insert(f.data);
            }
        }
    }
    Installer { linux, initrd, kver, modules }
}

impl Installer {
    /// The drivers `names` and everything they need, in load order
    /// (dependencies first), decompressed. Ones built into the kernel are
    /// skipped.
    pub fn modules(&self, names: &[&str]) -> Vec<Module> {
        let mut out: Vec<Module> = Vec::new();
        for name in names {
            if !self.modules.contains_key(&module_key(name)) {
                eprintln!("xtask: module {name} not in this kernel (built in?)");
            }
            self.visit(&module_key(name), &mut out);
        }
        out
    }

    fn visit(&self, name: &str, out: &mut Vec<Module>) {
        if out.iter().any(|m| m.name == name) {
            return;
        }
        let Some(packed) = self.modules.get(name) else { return };
        let data = unxz(packed);
        let deps: Vec<String> = depends(&data).into_iter().filter(|d| self.modules.contains_key(d)).collect();
        for d in &deps {
            self.visit(d, out);
        }
        out.push(Module { name: name.to_owned(), deps, data });
    }
}

/// A module's `depends=` list, from its .modinfo strings.
fn depends(ko: &[u8]) -> Vec<String> {
    let key = b"\0depends=";
    let Some(at) = ko.windows(key.len()).position(|w| w == key) else { return Vec::new() };
    let value = &ko[at + key.len()..];
    let value = &value[..value.iter().position(|&b| b == 0).unwrap_or(value.len())];
    String::from_utf8_lossy(value).split(',').filter(|s| !s.is_empty()).map(module_key).collect()
}

/// The files of installer package `name`, checked against the SHA-256 in
/// the installer's package index.
pub fn udeb(cache: &Path, name: &str) -> Vec<UdebFile> {
    let mut index = String::new();
    let packed = Command::new("curl")
        .args(["-fsSL", &format!("{MIRROR}/dists/{SUITE}/main/debian-installer/binary-amd64/Packages.gz")])
        .output()
        .expect("run curl");
    GzDecoder::new(&packed.stdout[..]).read_to_string(&mut index).unwrap_or_default();
    let wanted = format!("Package: {name}");
    let record = index.split("\n\n").find(|r| r.lines().any(|l| l == wanted)).unwrap_or_else(|| {
        eprintln!("xtask: {name} not in Debian's installer package index");
        exit(1)
    });
    let field = |k: &str| record.lines().find_map(|l| l.strip_prefix(k)).expect("package index field").trim().to_owned();
    let (file, sha) = (field("Filename:"), field("SHA256:"));
    let path = cache.join(file.rsplit('/').next().unwrap());
    let deb = fetch_verified(&format!("{MIRROR}/{file}"), &path, &sha);

    let members = ar_members(&deb);
    let (_, packed) = members.iter().find(|(n, _)| n == "data.tar.xz").unwrap_or_else(|| {
        eprintln!("xtask: {name}: no data.tar.xz");
        exit(1)
    });
    let mut out = Vec::new();
    let mut archive = tar::Archive::new(XzReader::new(*packed, true));
    for entry in archive.entries().expect("read udeb") {
        let mut entry = entry.expect("udeb entry");
        let path = entry.path().unwrap().to_string_lossy().trim_start_matches("./").trim_end_matches('/').to_owned();
        let mode = entry.header().mode().unwrap_or(0o644) & 0o7777;
        match entry.header().entry_type() {
            tar::EntryType::Regular => {
                let mut data = Vec::new();
                entry.read_to_end(&mut data).expect("read udeb file");
                out.push(UdebFile { path, mode, data, link: None });
            }
            tar::EntryType::Symlink => {
                let link = entry.link_name().unwrap().unwrap().to_string_lossy().into_owned();
                out.push(UdebFile { path, mode, data: Vec::new(), link: Some(link) });
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cpio_entries() {
        let mut cpio = crate::Cpio::default();
        cpio.dir("usr");
        cpio.file("usr/a.ko.xz", b"abcde", 0o644);
        cpio.symlink("init", "usr/a");
        let bytes = cpio.finish();
        let entries = cpio_entries(&bytes);
        let names: Vec<&str> = entries.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(names, ["usr", "usr/a.ko.xz", "init"]);
        assert_eq!(entries[1].2, b"abcde");
        assert_eq!(entries[1].1, 0o100644);
    }

    #[test]
    fn reads_ar_members() {
        let mut ar = b"!<arch>\n".to_vec();
        for (name, body) in [("debian-binary", &b"2.0\n"[..]), ("data.tar.xz", &b"xyz"[..])] {
            ar.extend_from_slice(format!("{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", 0, 0, 0, 100644, body.len()).as_bytes());
            ar.extend_from_slice(body);
            if body.len() % 2 == 1 {
                ar.push(b'\n');
            }
        }
        let members = ar_members(&ar);
        assert_eq!(members.len(), 2);
        assert_eq!(members[0], ("debian-binary".to_owned(), &b"2.0\n"[..]));
        assert_eq!(members[1], ("data.tar.xz".to_owned(), &b"xyz"[..]));
    }

    #[test]
    fn reads_module_dependencies() {
        assert_eq!(depends(b"\x7fELF..\0license=GPL\0depends=net-failover,failover\0retpoline=Y\0"), ["net_failover", "failover"]);
        assert!(depends(b"\0depends=\0name=evdev\0").is_empty());
        assert_eq!(module_of("usr/lib/modules/6.12/kernel/drivers/char/hw_random/virtio-rng.ko.xz").as_deref(), Some("virtio_rng"));
    }
}
