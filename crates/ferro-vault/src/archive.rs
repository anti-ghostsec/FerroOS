//! Packs the persisted folders into one blob for the vault, and back.
//! Format: `FVA1` then entries `[kind u8][mode u32][path_len u16][path]
//! [data_len u32][data]`, kind 1 = directory, 2 = file.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"FVA1";

fn mode_of(md: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        md.mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        if md.is_dir() {
            0o755
        } else {
            0o644
        }
    }
}

/// Walks `base/<root>` for each root, skipping relative paths `skip` rejects.
fn walk(base: &Path, roots: &[&str], skip: &dyn Fn(&str) -> bool, f: &mut dyn FnMut(&str, &Path, &fs::Metadata) -> io::Result<()>) -> io::Result<()> {
    let mut stack: Vec<String> = roots.iter().map(|r| r.trim_matches('/').to_owned()).collect();
    while let Some(rel) = stack.pop() {
        let path = base.join(&rel);
        let Ok(md) = fs::symlink_metadata(&path) else { continue };
        if skip(&rel) || md.file_type().is_symlink() {
            continue;
        }
        f(&rel, &path, &md)?;
        if md.is_dir() {
            let mut names: Vec<String> = fs::read_dir(&path)?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            stack.extend(names.into_iter().rev().map(|n| format!("{rel}/{n}")));
        }
    }
    Ok(())
}

pub fn pack(base: &Path, roots: &[&str], skip: &dyn Fn(&str) -> bool) -> io::Result<Vec<u8>> {
    let mut out = MAGIC.to_vec();
    walk(base, roots, skip, &mut |rel, path, md| {
        let (kind, data) = if md.is_dir() { (1u8, Vec::new()) } else { (2u8, fs::read(path)?) };
        out.push(kind);
        out.extend_from_slice(&mode_of(md).to_le_bytes());
        out.extend_from_slice(&(rel.len() as u16).to_le_bytes());
        out.extend_from_slice(rel.as_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        Ok(())
    })?;
    Ok(out)
}

/// A cheap fingerprint (names, sizes, modification times) to notice changes
/// without reading or encrypting anything.
pub fn fingerprint(base: &Path, roots: &[&str], skip: &dyn Fn(&str) -> bool) -> u64 {
    let mut h = DefaultHasher::new();
    let _ = walk(base, roots, skip, &mut |rel, _, md| {
        rel.hash(&mut h);
        md.len().hash(&mut h);
        md.modified().ok().hash(&mut h);
        Ok(())
    });
    h.finish()
}

/// Recreates the packed folders under `base`. Rejects absolute paths and
/// `..` so a tampered archive can't write outside them. Returns every path
/// created (so the caller can hand ownership to the desktop user).
pub fn unpack(data: &[u8], base: &Path) -> io::Result<Vec<PathBuf>> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "damaged vault archive");
    if data.get(..4) != Some(MAGIC) {
        return Err(bad());
    }
    let mut created = Vec::new();
    let mut i = 4;
    while i < data.len() {
        let kind = data[i];
        let mode = u32::from_le_bytes(data.get(i + 1..i + 5).ok_or_else(bad)?.try_into().unwrap());
        let plen = u16::from_le_bytes(data.get(i + 5..i + 7).ok_or_else(bad)?.try_into().unwrap()) as usize;
        let rel = std::str::from_utf8(data.get(i + 7..i + 7 + plen).ok_or_else(bad)?).map_err(|_| bad())?;
        i += 7 + plen;
        let dlen = u32::from_le_bytes(data.get(i..i + 4).ok_or_else(bad)?.try_into().unwrap()) as usize;
        let body = data.get(i + 4..i + 4 + dlen).ok_or_else(bad)?;
        i += 4 + dlen;
        if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|c| c == ".." || c.is_empty()) {
            return Err(bad());
        }
        let path = base.join(rel);
        match kind {
            1 => fs::create_dir_all(&path)?,
            2 => {
                if let Some(p) = path.parent() {
                    fs::create_dir_all(p)?;
                }
                fs::write(&path, body)?;
            }
            _ => return Err(bad()),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode));
        }
        #[cfg(not(unix))]
        let _ = mode;
        created.push(path);
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_round_trip() {
        let src = std::env::temp_dir().join(format!("fva-src-{}", std::process::id()));
        let dst = std::env::temp_dir().join(format!("fva-dst-{}", std::process::id()));
        fs::create_dir_all(src.join("ProgramData/ferro")).unwrap();
        fs::create_dir_all(src.join("ProgramData/app/tmp")).unwrap();
        fs::write(src.join("ProgramData/ferro/choices.conf"), "app x network=allow\n").unwrap();
        fs::write(src.join("ProgramData/app/tmp/junk"), "temp").unwrap();
        let skip = |rel: &str| rel.ends_with("/tmp");
        let before = fingerprint(&src, &["ProgramData"], &skip);
        let blob = pack(&src, &["ProgramData", "missing"], &skip).unwrap();
        unpack(&blob, &dst).unwrap();
        assert_eq!(fs::read_to_string(dst.join("ProgramData/ferro/choices.conf")).unwrap(), "app x network=allow\n");
        assert!(!dst.join("ProgramData/app/tmp").exists(), "temp folders aren't persisted");
        fs::write(src.join("ProgramData/ferro/choices.conf"), "changed\n").unwrap();
        assert_ne!(before, fingerprint(&src, &["ProgramData"], &skip));
        fs::remove_dir_all(&src).unwrap();
        fs::remove_dir_all(&dst).unwrap();
    }

    #[test]
    fn rejects_escaping_paths() {
        for evil in ["../etc/passwd", "/etc/passwd", "a//b"] {
            let mut blob = MAGIC.to_vec();
            blob.push(2);
            blob.extend_from_slice(&0o644u32.to_le_bytes());
            blob.extend_from_slice(&(evil.len() as u16).to_le_bytes());
            blob.extend_from_slice(evil.as_bytes());
            blob.extend_from_slice(&0u32.to_le_bytes());
            assert!(unpack(&blob, &std::env::temp_dir()).is_err(), "{evil}");
        }
    }
}
