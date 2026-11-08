//! Damaged RAR archives: bytes changed, cut short. Each must end in a clear
//! error (or, if the damage missed, a good extraction), never a panic and
//! never a hang.

use ferro_archive::{extract, open, Limits, Progress, GB};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Duration;

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ferro-archive-test-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// A few small archives of each kind the RAR reader handles differently.
fn originals() -> Vec<(&'static str, Vec<u8>)> {
    let text: Vec<u8> = b"some text that repeats, some text that repeats. ".repeat(200);
    let mixed: Vec<u8> = (0..6000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8).collect();
    let kinds: [(&str, rars::ArchiveVersion, bool, Option<&[u8]>); 5] = [
        ("rar5", rars::ArchiveVersion::Rar50, false, None),
        ("rar5-solid", rars::ArchiveVersion::Rar50, true, None),
        ("rar5-password", rars::ArchiveVersion::Rar50, false, Some(b"pw")),
        ("rar4", rars::ArchiveVersion::Rar29, false, None),
        ("rar4-solid", rars::ArchiveVersion::Rar29, true, None),
    ];
    kinds
        .iter()
        .map(|(tag, v, solid, pw)| {
            let mut b = rars::Builder::new(*v).solid(*solid).password(pw.map(<[u8]>::to_vec));
            b.add_bytes(b"dir/a.txt".to_vec(), text.clone(), Some(1_700_000_000), None).unwrap();
            b.add_bytes(b"b.bin".to_vec(), mixed.clone(), Some(1_700_000_000), None).unwrap();
            b.add_bytes(b"c.txt".to_vec(), b"short".to_vec(), Some(1_700_000_000), None).unwrap();
            let path = std::env::temp_dir().join(format!("ferro-archive-test-{}-original-{tag}.rar", std::process::id()));
            b.write_to_path(&path, None).unwrap();
            let bytes = fs::read(&path).unwrap();
            fs::remove_file(&path).unwrap();
            (*tag, bytes)
        })
        .collect()
}

/// A small, fixed sequence of numbers (the same every run).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Lists and unpacks `file` on a thread of its own: whether it panicked,
/// and whether it was still going after `limit`.
fn try_one(file: &Path, out: &Path, limit: Duration) -> Result<(), String> {
    let (tx, rx) = channel();
    let (file, out) = (file.to_path_buf(), out.to_path_buf());
    let progress = Arc::new(Progress::default());
    let p = progress.clone();
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(|| {
            let limits = Limits { max_total: 64 << 20, max_ratio: 1000, ratio_after: GB, max_entries: 10_000 };
            for pw in [None, Some("pw")] {
                if let Ok(l) = open(&file, pw) {
                    let _ = fs::remove_dir_all(&out);
                    fs::create_dir_all(&out).unwrap();
                    let _ = extract(&file, &l, &out, None, pw, limits, &p);
                }
            }
        });
        let _ = tx.send(r.is_ok());
    });
    match rx.recv_timeout(limit) {
        Ok(true) => Ok(()),
        Ok(false) => Err("panicked".into()),
        Err(_) => {
            // Cancel must still end it.
            progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(_) => Err("slow, but Cancel ended it".into()),
                Err(_) => Err("hung, even after Cancel".into()),
            }
        }
    }
}

#[test]
fn damaged_rars_fail_cleanly() {
    let d = scratch("rar-damaged");
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut bad = Vec::new();
    let mut tried = 0;
    for (tag, bytes) in originals() {
        for round in 0..60 {
            let mut b = bytes.clone();
            match round % 3 {
                // Cut short.
                0 => b.truncate((rng.next() as usize) % b.len()),
                // A few bytes changed.
                1 => {
                    for _ in 0..1 + rng.next() % 4 {
                        let at = (rng.next() as usize) % b.len();
                        b[at] ^= (rng.next() as u8) | 1;
                    }
                }
                // A run of bytes overwritten (a header, a length).
                _ => {
                    let at = (rng.next() as usize) % b.len();
                    for x in b.iter_mut().skip(at).take(8) {
                        *x = rng.next() as u8;
                    }
                }
            }
            let file = d.join(format!("{tag}-{round}.rar"));
            fs::write(&file, &b).unwrap();
            tried += 1;
            if let Err(why) = try_one(&file, &d.join("out"), Duration::from_secs(30)) {
                bad.push(format!("{tag} round {round}: {why}"));
            } else {
                let _ = fs::remove_file(&file);
            }
        }
    }
    assert!(bad.is_empty(), "{} of {tried} went wrong (left in {}): {bad:#?}", bad.len(), d.display());
    fs::remove_dir_all(&d).unwrap();
}
