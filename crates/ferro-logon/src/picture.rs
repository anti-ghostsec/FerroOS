//! The logon screen's picture, decoded out of harm's way.
//!
//! The logon screen runs as root, and its picture is chosen by whoever is
//! an administrator (Appearance > Use on the logon screen too writes it into
//! /etc/ferro/logon, which the sudo group may write without a password).
//! A picture made to break the decoder mustn't become root for them, so:
//!
//!   - only a plain file directly in /etc/ferro/logon is taken (no link,
//!     no other folder), opened here and handed over already open;
//!   - it's decoded by this same program in a process of its own (`ferro-logon
//!     --picture`), as nobody, with no groups, a time limit and a size limit,
//!     which gives back only the width, the height and the pixels.

use ferro_gfx::DecodedImage;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const FOLDER: &str = "/etc/ferro/logon";
/// Larger than any wallpaper; what's bigger isn't read.
const MOST_BYTES: u64 = 32 << 20;
const MOST_SIDE: usize = 8192;
const NOBODY: u32 = 65534;
const TIME: Duration = Duration::from_secs(10);

/// Whether `path` names a file directly in the logon folder (its name
/// alone: no `..`, no folder inside it).
pub fn allowed(path: &str) -> bool {
    let p = Path::new(path);
    p.parent() == Some(Path::new(FOLDER)) && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| !n.starts_with('.') && !n.is_empty())
}

/// The picture at `path`, if it's one the logon screen may show. As root,
/// only one in the logon folder; run by the person (the lock screen), any
/// they can read themselves.
pub fn load(path: &str) -> Option<DecodedImage> {
    // SAFETY: geteuid has no preconditions.
    let root = unsafe { libc::geteuid() } == 0;
    if root && !allowed(path) {
        return None;
    }
    // O_NOFOLLOW: a link put there in its place isn't followed; O_NONBLOCK:
    // a FIFO there can't keep the logon screen waiting.
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path).ok()?;
    let meta = file.metadata().ok()?;
    // One name only: not a second name for a file of root's elsewhere.
    if !meta.is_file() || meta.size() == 0 || meta.size() > MOST_BYTES || (root && meta.nlink() != 1) {
        return None;
    }
    let me = std::env::current_exe().ok()?;
    let mut cmd = Command::new(me);
    cmd.arg("--picture").env_clear().stdin(Stdio::from(file)).stdout(Stdio::piped()).stderr(Stdio::null());
    if root {
        // (Setting the user as root also drops the supplementary groups.)
        cmd.uid(NOBODY).gid(NOBODY);
    }
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            let mem = libc::rlimit { rlim_cur: 1 << 30, rlim_max: 1 << 30 };
            let cpu = libc::rlimit { rlim_cur: 10, rlim_max: 10 };
            libc::setrlimit(libc::RLIMIT_AS, &mem);
            libc::setrlimit(libc::RLIMIT_CPU, &cpu);
            libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
            Ok(())
        });
    }
    let mut child = cmd.spawn().ok()?;
    let mut out = child.stdout.take()?;
    // Read on a thread, so a decoder that hangs is ended at the time limit.
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = out.by_ref().take(16 + (MOST_SIDE * MOST_SIDE * 4) as u64).read_to_end(&mut bytes);
        bytes
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < TIME => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    parse(&reader.join().ok()?)
}

/// The decoder's answer: the width and height (u32, little-endian), then
/// that many pixels (u32 each).
fn parse(bytes: &[u8]) -> Option<DecodedImage> {
    let w = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) as usize;
    let h = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    if w == 0 || h == 0 || w > MOST_SIDE || h > MOST_SIDE || bytes.len() != 8 + w * h * 4 {
        return None;
    }
    let pixels = bytes[8..].as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
    Some(DecodedImage::new(w, h, pixels))
}

fn encode(img: &DecodedImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + img.pixels.len() * 4);
    out.extend_from_slice(&(img.width as u32).to_le_bytes());
    out.extend_from_slice(&(img.height as u32).to_le_bytes());
    for p in &img.pixels {
        out.extend_from_slice(&p.to_le_bytes());
    }
    out
}

/// `ferro-logon --picture`: the picture on stdin, decoded, on stdout.
pub fn decode_stdin() -> ! {
    let mut bytes = Vec::new();
    let ok = std::io::stdin().take(MOST_BYTES).read_to_end(&mut bytes).is_ok();
    let img = ok.then(|| DecodedImage::decode(&bytes)).flatten().filter(|i| i.width <= MOST_SIDE && i.height <= MOST_SIDE);
    match img {
        Some(img) => {
            let _ = std::io::stdout().write_all(&encode(&img));
            std::process::exit(0)
        }
        None => std::process::exit(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_file_right_in_the_logon_folder() {
        assert!(allowed("/etc/ferro/logon/wallpaper.png"));
        assert!(!allowed("/etc/shadow"));
        assert!(!allowed("/home/you/Pictures/a.png"));
        assert!(!allowed("/etc/ferro/logon/../../shadow"));
        assert!(!allowed("/etc/ferro/logon/sub/a.png"));
        assert!(!allowed("/etc/ferro/logon/.wallpaper.123.tmp"));
        assert!(!allowed("/etc/ferro/logon/"));
        assert!(!allowed("etc/ferro/logon/a.png"));
    }

    #[test]
    fn the_answer_round_trips_and_lies_are_refused() {
        let img = DecodedImage::new(2, 1, vec![0x00ff0000, 0x000000ff]);
        let back = parse(&encode(&img)).unwrap();
        assert_eq!((back.width, back.height, back.pixels), (2, 1, vec![0x00ff0000, 0x000000ff]));
        let mut short = encode(&img);
        short.pop();
        assert!(parse(&short).is_none());
        let mut huge = encode(&img);
        huge[0..4].copy_from_slice(&100_000u32.to_le_bytes());
        assert!(parse(&huge).is_none());
        assert!(parse(&[]).is_none());
    }
}
