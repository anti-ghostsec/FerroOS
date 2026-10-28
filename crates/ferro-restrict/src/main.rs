//! ferro-restrict: the root half of Restricted folders (see the library).
//!
//!     sudo ferro-restrict lock PATH     the folder becomes root:root 0700
//!     sudo ferro-restrict unlock PATH   back to the owner, group and mode it had
//!
//! Run only through sudo, with the password asked (there's no NOPASSWD rule
//! for it): the account is sudo's SUDO_UID, and only a folder of that
//! account's, in its home or on its drives, is touched. Everything is done
//! on the folder once opened (no following links, and the opened folder
//! checked to be the one named), so swapping in a link to /etc halfway
//! through gets nothing.

use ferro_restrict::{check_place, keeps_owners, record_path, Record, RECORDS};
use std::ffi::CStr;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{fchown, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::exit;

fn main() {
    match run() {
        Ok(done) => println!("{done}"),
        Err(why) => {
            eprintln!("ferro-restrict: {why}");
            exit(1);
        }
    }
}

fn run() -> Result<String, String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [verb, path] = args.as_slice() else {
        return Err("usage: sudo ferro-restrict lock|unlock PATH".into());
    };
    let lock = match verb.to_str() {
        Some("lock") => true,
        Some("unlock") => false,
        _ => return Err("usage: sudo ferro-restrict lock|unlock PATH".into()),
    };
    if unsafe { libc::geteuid() } != 0 {
        return Err("this needs your password: run it with sudo.".into());
    }
    // Set by sudo itself, after it clears what the caller passed in.
    let uid: u32 = std::env::var("SUDO_UID").ok().and_then(|u| u.parse().ok()).ok_or("this needs to be run with sudo.")?;
    if uid == 0 {
        return Err("run it with sudo from the account whose folder it is.".into());
    }
    let (user, home) = account(uid)?;
    let path = Path::new(path);

    // The folder named, not what a link there points to.
    let named = fs::symlink_metadata(path).map_err(|e| format!("Can't find '{}': {e}.", path.display()))?;
    if named.file_type().is_symlink() {
        return Err(format!("'{}' is a shortcut (a link), not a folder.", path.display()));
    }
    if !named.is_dir() {
        return Err(format!("'{}' isn't a folder.", path.display()));
    }
    let real = fs::canonicalize(path).map_err(|e| format!("Can't find '{}': {e}.", path.display()))?;
    check_place(&real, &home, &user)?;

    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(&real)
        .map_err(|e| format!("Can't open '{}': {e}.", real.display()))?;
    // A folder on the way swapped for a link since it was checked: the
    // folder opened is somewhere else.
    let opened = fs::read_link(format!("/proc/self/fd/{}", dir.as_raw_fd())).map_err(|e| format!("Can't check the folder: {e}."))?;
    if opened != real {
        return Err("The folder changed while it was being checked. Nothing was changed.".into());
    }
    keeps_owners(fs_type(&dir)?)?;
    let meta = dir.metadata().map_err(|e| e.to_string())?;
    let record = record_path(meta.dev(), meta.ino());
    if lock {
        restrict(&dir, &real, uid, &meta, &record)
    } else {
        free(&dir, &real, uid, &record)
    }
}

fn restrict(dir: &File, real: &Path, uid: u32, meta: &fs::Metadata, record: &Path) -> Result<String, String> {
    if meta.uid() == 0 && record.exists() {
        return Ok(format!("'{}' is already restricted.", real.display()));
    }
    if meta.uid() != uid {
        return Err(format!("'{}' isn't yours, so it can't be restricted.", real.display()));
    }
    let was = Record { uid: meta.uid(), gid: meta.gid(), mode: meta.mode() & 0o7777 };
    // Written before the folder changes hands: stopped halfway, unlock still
    // knows what to give back.
    write_record(record, &was.to_text(real))?;
    let given = fchown(dir, Some(0), Some(0)).and_then(|()| dir.set_permissions(Permissions::from_mode(0o700)));
    let now = dir.metadata().map_err(|e| e.to_string())?;
    if given.is_err() || now.uid() != 0 || now.mode() & 0o7777 != 0o700 {
        // A drive that says yes and keeps nothing: back as it was.
        let _ = fchown(dir, Some(was.uid), Some(was.gid));
        let _ = dir.set_permissions(Permissions::from_mode(was.mode));
        let _ = fs::remove_file(record);
        return Err("This drive didn't keep the change of owner, so the folder can't be restricted here.".into());
    }
    Ok(format!("'{}' is restricted. Untick Restricted in its Properties to open it again.", real.display()))
}

fn free(dir: &File, real: &Path, uid: u32, record: &Path) -> Result<String, String> {
    let text = fs::read_to_string(record).map_err(|_| format!("'{}' isn't restricted.", real.display()))?;
    let was = Record::parse(&text).ok_or("The folder's record is damaged; an administrator can give it back with chown.")?;
    if was.uid != uid {
        return Err("This folder was restricted by another account.".into());
    }
    fchown(dir, Some(was.uid), Some(was.gid)).map_err(|e| format!("Can't give the folder back: {e}."))?;
    dir.set_permissions(Permissions::from_mode(was.mode)).map_err(|e| format!("Can't give the folder back: {e}."))?;
    let _ = fs::remove_file(record);
    Ok(format!("'{}' is open again.", real.display()))
}

/// Written whole or not at all, readable by all (Properties and File
/// Explorer look the record up), changeable only by root.
fn write_record(record: &Path, text: &str) -> Result<(), String> {
    fs::create_dir_all(RECORDS).map_err(|e| format!("Can't keep the record: {e}."))?;
    fs::set_permissions(RECORDS, Permissions::from_mode(0o711)).map_err(|e| format!("Can't keep the record: {e}."))?;
    let tmp = record.with_extension("new");
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)
        .map_err(|e| format!("Can't keep the record: {e}."))?;
    f.write_all(text.as_bytes()).and_then(|()| f.sync_all()).map_err(|e| format!("Can't keep the record: {e}."))?;
    fs::rename(&tmp, record).map_err(|e| format!("Can't keep the record: {e}."))
}

/// The filesystem the open folder is on, as statfs names it.
#[allow(clippy::unnecessary_cast)]
fn fs_type(dir: &File) -> Result<u32, String> {
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatfs(dir.as_raw_fd(), &mut st) } != 0 {
        return Err(format!("Can't tell what drive the folder is on: {}.", std::io::Error::last_os_error()));
    }
    Ok(st.f_type as u64 as u32)
}

/// The account's name and home, from the system's list of accounts (not
/// from anything the caller could set).
fn account(uid: u32) -> Result<(String, PathBuf), String> {
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr(), buf.len(), &mut found) };
    if rc != 0 || found.is_null() || pw.pw_name.is_null() || pw.pw_dir.is_null() {
        return Err(format!("There's no account {uid}."));
    }
    let name = unsafe { CStr::from_ptr(pw.pw_name) }.to_string_lossy().into_owned();
    let home = unsafe { CStr::from_ptr(pw.pw_dir) }.to_string_lossy().into_owned();
    // The home as it really is (it may be reached through a link).
    let home = fs::canonicalize(&home).unwrap_or_else(|_| PathBuf::from(home));
    Ok((name, home))
}
