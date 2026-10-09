//! The programs disk: optional, larger programs (the Tor service) kept on
//! disk instead of in the RAM-backed root file system, and copied into RAM
//! only while in use. On the installation USB stick it's a partition that
//! also carries what Setup copies to the disk (kernel, initramfs, boot
//! loader).
//!
//! The disk is untrusted: each program's SHA-256 is compiled into FerroOS,
//! and a copy that doesn't match is refused.
//!
//! Layout (written by `cargo xtask programs`): a 4 KiB header, then each
//! program at a 4 KiB-aligned offset.
//!
//! ```text
//! 0    "FERROPKG"  u32 version (1)  u32 count
//! 16   count x { name: 48 bytes, NUL-padded | offset: u64 | size: u64 }
//! ```

pub const MAGIC: &[u8; 8] = b"FERROPKG";

/// Expected SHA-256 of each program, fixed when FerroOS was built.
pub fn expected_sha256(name: &str) -> Option<&'static str> {
    match name {
        "arti" => option_env!("FERRO_ARTI_SHA256"),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

/// Parses the header block.
pub fn parse_header(h: &[u8]) -> Option<Vec<Entry>> {
    if h.get(..8)? != MAGIC || u32::from_le_bytes(h.get(8..12)?.try_into().ok()?) != 1 {
        return None;
    }
    let count = u32::from_le_bytes(h.get(12..16)?.try_into().ok()?) as usize;
    (0..count.min(64))
        .map(|i| {
            let e = h.get(16 + i * 64..16 + (i + 1) * 64)?;
            let name = String::from_utf8(e[..48].iter().copied().take_while(|&b| b != 0).collect()).ok()?;
            Some(Entry { name, offset: u64::from_le_bytes(e[48..56].try_into().ok()?), size: u64::from_le_bytes(e[56..64].try_into().ok()?) })
        })
        .collect()
}

/// Builds a header block for `entries` (used by the build tool and tests).
pub fn build_header(entries: &[Entry]) -> Vec<u8> {
    let mut h = vec![0u8; 4096];
    h[..8].copy_from_slice(MAGIC);
    h[8..12].copy_from_slice(&1u32.to_le_bytes());
    h[12..16].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    for (i, e) in entries.iter().enumerate() {
        let b = &mut h[16 + i * 64..16 + (i + 1) * 64];
        let n = e.name.len().min(47);
        b[..n].copy_from_slice(&e.name.as_bytes()[..n]);
        b[48..56].copy_from_slice(&e.offset.to_le_bytes());
        b[56..64].copy_from_slice(&e.size.to_le_bytes());
    }
    h
}

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs::{self, File};
    use std::io::{self, Read, Seek, SeekFrom, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// True if `dev` is a programs disk (so it must never become a vault).
    pub fn is_programs_disk(dev: &Path) -> bool {
        let mut magic = [0u8; 8];
        File::open(dev).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == MAGIC
    }

    /// The programs disk or partition, and what's on it.
    pub fn find_disk() -> Option<(PathBuf, Vec<Entry>)> {
        // Whole disks and partitions alike.
        for e in fs::read_dir("/sys/class/block").ok()?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("vd") || name.starts_with("sd") || name.starts_with("nvme")) {
                continue;
            }
            let dev = PathBuf::from("/dev").join(&name);
            let mut h = vec![0u8; 4096];
            if File::open(&dev).and_then(|mut f| f.read_exact(&mut h)).is_err() {
                continue;
            }
            if let Some(entries) = parse_header(&h) {
                return Some((dev, entries));
            }
        }
        None
    }

    /// Reads entry `name` whole (Setup's kernel, initramfs and boot loader).
    pub fn read(name: &str) -> io::Result<Vec<u8>> {
        let (dev, entries) = find_disk().ok_or_else(|| io::Error::other("The installation files weren't found."))?;
        let e = entries.iter().find(|e| e.name == name).ok_or_else(|| io::Error::other(format!("{name} isn't on the installation medium")))?;
        let mut f = File::open(dev)?;
        f.seek(SeekFrom::Start(e.offset))?;
        let mut data = vec![0u8; e.size as usize];
        f.read_exact(&mut data)?;
        Ok(data)
    }

    /// Copies program `name` from the programs disk to `dest` (mode 0755),
    /// verifying it against the hash built into FerroOS.
    pub fn extract(name: &str, dest: &Path) -> io::Result<()> {
        let want = expected_sha256(name).ok_or_else(|| io::Error::other(format!("{name} isn't part of this FerroOS build")))?;
        let (dev, entries) = find_disk().ok_or_else(|| io::Error::other("the programs disk isn't attached"))?;
        let e = entries.iter().find(|e| e.name == name).ok_or_else(|| io::Error::other(format!("{name} isn't on the programs disk")))?;
        let mut src = File::open(dev)?;
        src.seek(SeekFrom::Start(e.offset))?;
        let tmp = dest.with_extension("part");
        let mut hash = Sha256::new();
        let copied = (|| -> io::Result<()> {
            let mut out = File::create(&tmp)?;
            let mut left = e.size;
            let mut buf = vec![0u8; 256 * 1024];
            while left > 0 {
                let n = buf.len().min(left as usize);
                src.read_exact(&mut buf[..n])?;
                hash.update(&buf[..n]);
                out.write_all(&buf[..n])?;
                left -= n as u64;
            }
            Ok(())
        })();
        // The copy in RAM is what runs; don't keep a second one cached.
        // SAFETY: fadvise on our own open descriptor.
        unsafe {
            use std::os::fd::AsRawFd;
            libc::posix_fadvise(src.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
        }
        if let Err(e) = copied {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        let got: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got != want {
            let _ = fs::remove_file(&tmp);
            return Err(io::Error::other(format!("{name} on the programs disk has been modified; refusing to run it")));
        }
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
        fs::rename(&tmp, dest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let entries = vec![Entry { name: "arti".into(), offset: 4096, size: 123 }];
        let h = build_header(&entries);
        assert_eq!(parse_header(&h), Some(entries));
        let mut bad = h.clone();
        bad[0] = b'X';
        assert_eq!(parse_header(&bad), None);
    }
}
