//! What the file manager does to files: copy, move, delete, unpack. Plain
//! std (and `unzip`/`tar` for archives), so it's tested without a screen.
//! Every operation answers with a line for the status bar.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A free name for `name` in `dir`, as Windows picks it: `x` if it's free,
/// else `x - Copy`, `x - Copy (2)`, ... (the extension kept at the end).
pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let (stem, ext) = split_ext(name);
    (1..)
        .map(|n| if n == 1 { format!("{stem} - Copy{ext}") } else { format!("{stem} - Copy ({n}){ext}") })
        .map(|candidate| dir.join(candidate))
        .find(|p| fs::symlink_metadata(p).is_err())
        .expect("some name is free")
}

/// `report.txt` -> (`report`, `.txt`); folders and dotless names keep it all.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// Copies a file, a folder (all of it) or a link to `to`. Permissions are
/// copied where the drive keeps them; a USB stick (FAT, exFAT) can't, and
/// that's no reason to fail.
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(from)?;
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, to);
        #[cfg(not(unix))]
        return Err(io::Error::other(format!("can't copy the link to {}", target.display())));
    }
    if meta.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        let _ = fs::set_permissions(to, meta.permissions());
        return Ok(());
    }
    let mut src = fs::File::open(from)?;
    let mut dst = fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    io::copy(&mut src, &mut dst)?;
    let _ = dst.set_permissions(meta.permissions());
    Ok(())
}

/// Whether `to` is `from` itself or inside it: a folder can't go into itself.
fn into_itself(from: &Path, to_dir: &Path) -> bool {
    let (Ok(a), Ok(b)) = (fs::canonicalize(from), fs::canonicalize(to_dir)) else { return false };
    b.starts_with(a)
}

/// Pastes `from` into `dir`: a copy, or a move when `cut`. A name already
/// there isn't overwritten; the new one gets a free name.
pub fn paste(from: &Path, dir: &Path, cut: bool) -> Result<String, String> {
    let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("Nothing to paste.")?;
    if fs::symlink_metadata(from).is_err() {
        return Err(format!("'{name}' isn't there any more."));
    }
    if into_itself(from, dir) && fs::symlink_metadata(from).is_ok_and(|m| m.is_dir()) {
        return Err(format!("'{name}' can't go inside itself."));
    }
    if cut && from.parent() == Some(dir) {
        return Ok(format!("'{name}' is already here."));
    }
    let to = free_name(dir, &name);
    let shown = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if cut {
        match fs::rename(from, &to) {
            Ok(()) => return Ok(format!("Moved '{name}'.")),
            // Another drive: copy it over, then take the original away.
            Err(e) if e.raw_os_error() == Some(EXDEV) => {}
            Err(e) => return Err(explain("move", &name, &e)),
        }
        if let Err(e) = copy_tree(from, &to) {
            let _ = remove(&to);
            return Err(explain("move", &name, &e));
        }
        return remove(from).map(|()| format!("Moved '{name}'.")).map_err(|e| explain("move", &name, &e));
    }
    match copy_tree(from, &to) {
        Ok(()) if shown == name => Ok(format!("Copied '{name}'.")),
        Ok(()) => Ok(format!("Copied '{name}' as '{shown}'.")),
        Err(e) => {
            // Leave no half-copied file behind.
            let _ = remove(&to);
            Err(explain("copy", &name, &e))
        }
    }
}

/// EXDEV: "not on the same drive", which rename can't do.
const EXDEV: i32 = 18;

#[cfg(unix)]
fn make_tree_writable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Ok(meta) = fs::symlink_metadata(&p) {
                if meta.is_dir() {
                    let mut perms = meta.permissions();
                    let mode = perms.mode();
                    if mode & 0o700 != 0o700 {
                        perms.set_mode(mode | 0o700);
                        let _ = fs::set_permissions(&p, perms);
                    }
                    make_tree_writable(&p);
                } else if !meta.file_type().is_symlink() {
                    let mut perms = meta.permissions();
                    let mode = perms.mode();
                    if mode & 0o200 == 0 {
                        perms.set_mode(mode | 0o600);
                        let _ = fs::set_permissions(&p, perms);
                    }
                }
            }
        }
    }
    if let Ok(meta) = fs::metadata(dir) {
        let mut perms = meta.permissions();
        let mode = perms.mode();
        if mode & 0o700 != 0o700 {
            perms.set_mode(mode | 0o700);
            let _ = fs::set_permissions(dir, perms);
        }
    }
}

/// Deletes a file, a link, or a folder with all that's in it.
pub fn remove(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        if let Err(e) = fs::remove_dir_all(path) {
            #[cfg(unix)]
            {
                make_tree_writable(path);
                if fs::remove_dir_all(path).is_ok() {
                    return Ok(());
                }
                // Fallback to rm -rf for stubborn external filesystems/drives
                let status = std::process::Command::new("rm").arg("-rf").arg("--").arg(path).status();
                if let Ok(st) = status {
                    if st.success() && !path.exists() {
                        return Ok(());
                    }
                }
            }
            return Err(e);
        }
        Ok(())
    } else if let Err(e) = fs::remove_file(path) {
        #[cfg(unix)]
        {
            if let Ok(meta) = fs::symlink_metadata(path) {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = meta.permissions();
                perms.set_mode(perms.mode() | 0o600);
                let _ = fs::set_permissions(path, perms);
                if fs::remove_file(path).is_ok() {
                    return Ok(());
                }
            }
        }
        Err(e)
    } else {
        Ok(())
    }
}

pub fn delete(path: &Path) -> Result<String, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    remove(path).map(|()| format!("Deleted '{name}'.")).map_err(|e| explain("delete", &name, &e))
}

/// Renames `path` to `new_name` in the same folder.
pub fn rename(path: &Path, new_name: &str) -> Result<String, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let new_name = new_name.trim();
    if new_name.is_empty() || new_name == "." || new_name == ".." || new_name.contains('/') || new_name.contains('\0') {
        return Err("A name can't be empty or have a / in it.".into());
    }
    if new_name == name {
        return Ok(String::new());
    }
    let to = path.with_file_name(new_name);
    if fs::symlink_metadata(&to).is_ok() {
        return Err(format!("There's already something called '{new_name}' here."));
    }
    fs::rename(path, &to).map(|()| format!("Renamed to '{new_name}'.")).map_err(|e| explain("rename", &name, &e))
}

/// The kinds of archive Extract Here unpacks.
pub fn is_archive(name: &str) -> bool {
    let n = name.to_lowercase();
    [".zip", ".tar", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst"].iter().any(|e| n.ends_with(e))
}

/// The archive's name without its extension: `photos.tar.gz` -> `photos`.
fn archive_stem(name: &str) -> &str {
    let n = name.to_lowercase();
    for e in [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tgz", ".txz", ".tbz2", ".zip", ".tar"] {
        if n.ends_with(e) {
            return &name[..name.len() - e.len()];
        }
    }
    name
}

/// Unpacks an archive next to itself, the way people expect: one folder
/// inside (as most archives have) comes out as that folder, not a folder
/// inside a folder of the same name; loose files come out together in a
/// folder named after the archive. Nothing already there is overwritten.
pub fn extract_here(archive: &Path) -> Result<(String, PathBuf), String> {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = archive.parent().ok_or("No folder to unpack into.")?;
    // Unpacked first into a hidden folder beside it, on the same drive, so
    // what comes out is then only renamed into place.
    let work = (0..).map(|n| dir.join(format!(".ferro-extract-{}-{n}", std::process::id()))).find(|p| !p.exists()).expect("a free name");
    fs::create_dir(&work).map_err(|e| explain("unpack", &name, &e))?;
    let result = unpack(archive, &work).and_then(|()| {
        let inside: Vec<_> = fs::read_dir(&work).map_err(|e| e.to_string())?.flatten().collect();
        let (from, want) = match inside.as_slice() {
            [] => return Err(format!("'{name}' is empty.")),
            [one] if one.path().is_dir() => (one.path(), one.file_name().to_string_lossy().into_owned()),
            _ => (work.clone(), archive_stem(&name).to_owned()),
        };
        let to = free_name(dir, &want);
        fs::rename(&from, &to).map_err(|e| explain("unpack", &name, &e))?;
        let shown = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok((format!("Unpacked '{name}' into '{shown}'."), to))
    });
    let _ = fs::remove_dir_all(&work);
    result
}

fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    let name = archive.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut cmd = if name.ends_with(".zip") {
        let mut c = Command::new("unzip");
        // -qq: quiet; -n: never overwrite (nothing's there anyway).
        c.arg("-qq").arg("-n").arg(archive).arg("-d").arg(into);
        c
    } else {
        let mut c = Command::new("tar");
        // No owners or device files from someone else's archive.
        c.arg("-xf").arg(archive).arg("-C").arg(into).arg("--no-same-owner").arg("--no-same-permissions");
        c
    };
    let out = cmd.output().map_err(|e| format!("Couldn't unpack: {e}"))?;
    // unzip says 1 for warnings (such as odd names) with the files out.
    if out.status.success() || (name.ends_with(".zip") && out.status.code() == Some(1)) {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&out.stderr);
    let why = why.lines().find(|l| !l.trim().is_empty()).unwrap_or("it may be damaged");
    Err(format!("Couldn't unpack it: {}", why.trim()))
}

/// An error, said so people know what to do about it.
pub fn explain(verb: &str, name: &str, e: &io::Error) -> String {
    match e.raw_os_error() {
        // EROFS
        Some(30) => format!(
            "Can't {verb} '{name}': the drive is read-only. A Windows drive is when Windows wasn't fully shut down: turn off Fast Startup there."
        ),
        // EACCES, EPERM
        Some(13 | 1) => format!("Can't {verb} '{name}': you don't have permission here."),
        // ENOSPC
        Some(28) => format!("Can't {verb} '{name}': the drive is full."),
        // ENOTEMPTY
        Some(39) => format!("Can't {verb} '{name}': folder is in use by another program or drive is busy."),
        _ => format!("Can't {verb} '{name}': {e}."),
    }
}

/// Size in the units people read: 512 bytes, 1.4 KB, 3.0 MB...
#[allow(dead_code)]
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ferro-files-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn free_names_follow_windows() {
        let d = scratch("names");
        assert_eq!(free_name(&d, "a.txt"), d.join("a.txt"));
        fs::write(d.join("a.txt"), "").unwrap();
        assert_eq!(free_name(&d, "a.txt"), d.join("a - Copy.txt"));
        fs::write(d.join("a - Copy.txt"), "").unwrap();
        assert_eq!(free_name(&d, "a.txt"), d.join("a - Copy (2).txt"));
        fs::create_dir(d.join("Pics")).unwrap();
        assert_eq!(free_name(&d, "Pics"), d.join("Pics - Copy"));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn copy_move_rename_delete() {
        let d = scratch("ops");
        let (src, dst) = (d.join("src"), d.join("dst"));
        fs::create_dir_all(src.join("folder/inner")).unwrap();
        fs::write(src.join("folder/inner/f.txt"), "hello").unwrap();
        fs::create_dir(&dst).unwrap();

        assert!(paste(&src.join("folder"), &dst, false).is_ok());
        assert_eq!(fs::read_to_string(dst.join("folder/inner/f.txt")).unwrap(), "hello");
        // Again: a second copy beside it, nothing overwritten.
        assert!(paste(&src.join("folder"), &dst, false).unwrap().contains("folder - Copy"));
        // Into itself: refused.
        assert!(paste(&src.join("folder"), &src.join("folder/inner"), false).is_err());

        assert!(paste(&src.join("folder"), &dst, true).is_ok());
        assert!(!src.join("folder").exists());
        assert!(dst.join("folder - Copy (2)/inner/f.txt").exists());

        assert!(rename(&dst.join("folder"), "renamed").is_ok());
        assert!(dst.join("renamed").is_dir());
        assert!(rename(&dst.join("renamed"), "folder - Copy").is_err(), "taken");
        assert!(rename(&dst.join("renamed"), "a/b").is_err());

        assert!(delete(&dst.join("renamed")).is_ok());
        assert!(!dst.join("renamed").exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn extract_does_not_nest() {
        if Command::new("zip").arg("-v").output().is_err() || Command::new("unzip").arg("-v").output().is_err() {
            return;
        }
        let d = scratch("zip");
        // One folder inside: it comes out as that folder.
        fs::create_dir_all(d.join("build/Game/data")).unwrap();
        fs::write(d.join("build/Game/data/x"), "1").unwrap();
        fs::write(d.join("build/Game/readme"), "2").unwrap();
        // Loose files: together, in a folder named after the archive.
        fs::write(d.join("build/a.txt"), "3").unwrap();
        let zip = |out: &str, what: &[&str]| {
            assert!(Command::new("zip").arg("-qr").arg(d.join(out)).args(what).current_dir(d.join("build")).status().unwrap().success());
        };
        zip("Game.zip", &["Game"]);
        zip("loose.zip", &["a.txt", "Game/readme"]);

        let (_, out) = extract_here(&d.join("Game.zip")).unwrap();
        assert_eq!(out, d.join("Game"));
        assert!(d.join("Game/data/x").exists() && !d.join("Game/Game").exists());
        // Again: beside it, not on top of it.
        let (_, out) = extract_here(&d.join("Game.zip")).unwrap();
        assert_eq!(out, d.join("Game - Copy"));

        let (_, out) = extract_here(&d.join("loose.zip")).unwrap();
        assert_eq!(out, d.join("loose"));
        assert!(d.join("loose/a.txt").exists() && d.join("loose/Game/readme").exists());
        // No working folder left behind.
        assert!(fs::read_dir(&d).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".ferro-extract")));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn archive_names() {
        assert!(is_archive("A.ZIP") && is_archive("x.tar.gz") && !is_archive("x.gz"));
        assert_eq!(archive_stem("photos.tar.gz"), "photos");
        assert_eq!(archive_stem("Game.zip"), "Game");
        assert_eq!(human_size(512), "512 bytes");
        assert_eq!(human_size(1536), "1.5 KB");
    }
}
