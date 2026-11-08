//! Each format both ways round: archives made here, then listed and
//! unpacked; and the archives meant to do harm, which mustn't.

use ferro_archive::{extract, extract_here, open, Error, Format, Limits, Progress, GB};
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ferro-archive-test-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn roomy() -> Limits {
    Limits { max_total: 64 * GB, max_ratio: 1000, ratio_after: GB, max_entries: 100_000 }
}

/// Unpacks all of `archive` into a new folder `out` beside it.
fn unpack(archive: &Path, password: Option<&str>, limits: Limits) -> (Result<ferro_archive::Report, Error>, PathBuf) {
    let out = archive.parent().unwrap().join("out");
    fs::create_dir_all(&out).unwrap();
    let listing = match open(archive, password) {
        Ok(l) => l,
        Err(e) => return (Err(e), out),
    };
    (extract(archive, &listing, &out, None, password, limits, &Arc::new(Progress::default())), out)
}

/// Every path under `dir`, relative, sorted.
fn tree(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, at: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(at).unwrap().flatten() {
            let p = e.path();
            out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
            if e.file_type().unwrap().is_dir() {
                walk(base, &p, out);
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, dir, &mut v);
    v.sort();
    v
}

// ---------------------------------------------------------------- ZIP

fn zip_with(path: &Path, build: impl FnOnce(&mut zip::ZipWriter<fs::File>)) {
    let mut z = zip::ZipWriter::new(fs::File::create(path).unwrap());
    build(&mut z);
    z.finish().unwrap();
}

fn opts() -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

#[test]
fn zip_lists_and_extracts() {
    let d = scratch("zip");
    let a = d.join("photos.zip");
    zip_with(&a, |z| {
        z.add_directory("Trip/", opts()).unwrap();
        z.start_file("Trip/a.txt", opts()).unwrap();
        z.write_all(b"hello").unwrap();
        z.start_file("Trip/sub/b.txt", opts().compression_method(zip::CompressionMethod::Stored)).unwrap();
        z.write_all(&vec![7u8; 100_000]).unwrap();
    });
    let l = open(&a, None).unwrap();
    assert_eq!(l.format, Format::Zip);
    let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Trip/", "Trip/a.txt", "Trip/sub/b.txt"]);
    assert!(l.entries[0].is_dir && !l.entries[1].is_dir);
    assert_eq!(l.entries[2].size, 100_000);
    assert!(l.entries[1].modified.is_some());

    let (r, out) = unpack(&a, None, roomy());
    let r = r.unwrap();
    assert_eq!(r.files, 2);
    assert_eq!(fs::read(out.join("Trip/a.txt")).unwrap(), b"hello");
    assert_eq!(fs::read(out.join("Trip/sub/b.txt")).unwrap().len(), 100_000);
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn zip_names_leaving_the_folder_are_skipped() {
    let d = scratch("zip-traversal");
    let a = d.join("evil.zip");
    zip_with(&a, |z| {
        for name in ["../escaped.txt", "/tmp/absolute.txt", "ok/../../escaped2.txt", "..\\escaped3.txt", "C:\\escaped4.txt", "fine.txt"] {
            z.start_file(name, opts()).unwrap();
            z.write_all(b"x").unwrap();
        }
    });
    let (r, out) = unpack(&a, None, roomy());
    let r = r.unwrap();
    assert_eq!(r.files, 1);
    assert_eq!(r.unsafe_names.len(), 5, "{:?}", r.unsafe_names);
    assert_eq!(tree(&out), ["fine.txt"]);
    // Nothing beside the folder either.
    assert!(!d.join("escaped.txt").exists() && !d.join("escaped2.txt").exists());
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn zip_links_never_lead_outside() {
    let d = scratch("zip-links");
    let a = d.join("links.zip");
    zip_with(&a, |z| {
        // A link out, then a file meant to land through it.
        z.add_symlink("up", "..", opts()).unwrap();
        z.start_file("up/through.txt", opts()).unwrap();
        z.write_all(b"x").unwrap();
        z.add_symlink("abs", "/etc", opts()).unwrap();
        z.add_symlink("sneaky", "a/../../..", opts()).unwrap();
        // A link inside, which is fine.
        z.start_file("docs/readme.txt", opts()).unwrap();
        z.write_all(b"read me").unwrap();
        z.add_symlink("docs/latest", "readme.txt", opts()).unwrap();
        z.add_symlink("also", "docs/readme.txt", opts()).unwrap();
    });
    let (r, out) = unpack(&a, None, roomy());
    let r = r.unwrap();
    assert_eq!(r.links, 2);
    assert_eq!(r.skipped_links, 3);
    assert!(!d.join("through.txt").exists());
    // "up" is a real folder (made for the file meant to go through it).
    assert!(fs::symlink_metadata(out.join("up")).unwrap().is_dir() && fs::symlink_metadata(out.join("abs")).is_err());
    assert_eq!(fs::read(out.join("docs/latest")).unwrap(), b"read me");
    assert_eq!(fs::read_link(out.join("also")).unwrap(), Path::new("docs/readme.txt"));
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_link_already_there_is_not_followed() {
    let d = scratch("zip-planted");
    let a = d.join("a.zip");
    zip_with(&a, |z| {
        z.start_file("note.txt", opts()).unwrap();
        z.write_all(b"new").unwrap();
        z.start_file("dir/inner.txt", opts()).unwrap();
        z.write_all(b"new").unwrap();
        z.start_file("kept.txt", opts()).unwrap();
        z.write_all(b"new").unwrap();
    });
    let out = d.join("out");
    let outside = d.join("outside");
    fs::create_dir_all(&out).unwrap();
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(outside.join("note.txt"), out.join("note.txt")).unwrap();
    std::os::unix::fs::symlink(&outside, out.join("dir")).unwrap();
    fs::write(out.join("kept.txt"), "mine").unwrap();
    let l = open(&a, None).unwrap();
    let r = extract(&a, &l, &out, None, None, roomy(), &Arc::new(Progress::default())).unwrap();
    assert!(!outside.join("note.txt").exists() && !outside.join("inner.txt").exists());
    assert_eq!(fs::read_to_string(out.join("kept.txt")).unwrap(), "mine");
    assert_eq!(r.existing, 2);
    assert_eq!(r.unsafe_names, ["dir/inner.txt"]);
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn zip_bombs_stop_at_the_limit() {
    let d = scratch("zip-bomb");
    let a = d.join("bomb.zip");
    zip_with(&a, |z| {
        for i in 0..4 {
            z.start_file(format!("zeros{i}.bin"), opts()).unwrap();
            z.write_all(&vec![0u8; 8 << 20]).unwrap();
        }
    });
    let size = fs::metadata(&a).unwrap().len();
    assert!(size < 200_000, "{size}");
    // What it says it holds is over the room: refused before writing.
    let tight = Limits { max_total: 10 << 20, ..roomy() };
    let (r, out) = unpack(&a, None, tight);
    assert!(matches!(r, Err(Error::TooBig(_))), "{r:?}");
    assert!(tree(&out).is_empty());
    // By ratio: stopped part way, the file being written removed.
    let ratio = Limits { max_ratio: 50, ratio_after: 1 << 20, ..roomy() };
    let (r, out) = unpack(&a, None, ratio);
    assert!(matches!(&r, Err(Error::TooBig(why)) if why.contains("zip bomb")), "{r:?}");
    for f in tree(&out) {
        assert_eq!(fs::metadata(out.join(&f)).unwrap().len(), 8 << 20, "{f} is whole or gone");
    }
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn an_entry_giving_more_than_it_says_is_stopped() {
    let d = scratch("zip-liar");
    let a = d.join("liar.zip");
    zip_with(&a, |z| {
        z.start_file("big.bin", opts().compression_method(zip::CompressionMethod::Stored)).unwrap();
        z.write_all(&[1u8; 1000]).unwrap();
    });
    // Make both headers say 10 bytes, where 1000 are stored.
    let mut bytes = fs::read(&a).unwrap();
    let mut patched = 0;
    for sig in [&b"PK\x03\x04"[..], &b"PK\x01\x02"[..]] {
        let at = bytes.windows(4).position(|w| w == sig).unwrap();
        let off = if sig[2] == 3 { at + 22 } else { at + 24 };
        assert_eq!(&bytes[off..off + 4], &1000u32.to_le_bytes());
        bytes[off..off + 4].copy_from_slice(&10u32.to_le_bytes());
        patched += 1;
    }
    assert_eq!(patched, 2);
    fs::write(&a, bytes).unwrap();
    let (r, out) = unpack(&a, None, roomy());
    // The zip reader sees the mismatch, or we do: either way nothing is left.
    assert!(r.is_err(), "{r:?}");
    assert!(fs::metadata(out.join("big.bin")).map_or(true, |m| m.len() <= 10));
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn zip_passwords() {
    let d = scratch("zip-pw");
    let a = d.join("secret.zip");
    zip_with(&a, |z| {
        z.start_file("secret.txt", opts().with_aes_encryption(zip::AesMode::Aes256, "hunter2")).unwrap();
        z.write_all(b"the plans").unwrap();
    });
    let l = open(&a, None).unwrap();
    assert!(l.entries[0].encrypted);
    let (r, out) = unpack(&a, None, roomy());
    assert_eq!(r, Err(Error::NeedPassword));
    assert!(tree(&out).is_empty());
    let (r, out) = unpack(&a, Some("wrong"), roomy());
    assert_eq!(r, Err(Error::WrongPassword));
    assert!(tree(&out).is_empty());
    let (r, out) = unpack(&a, Some("hunter2"), roomy());
    assert_eq!(r.unwrap().files, 1);
    assert_eq!(fs::read(out.join("secret.txt")).unwrap(), b"the plans");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn only_the_selection_comes_out() {
    let d = scratch("zip-sel");
    let a = d.join("s.zip");
    zip_with(&a, |z| {
        z.start_file("one.txt", opts()).unwrap();
        z.write_all(b"1").unwrap();
        z.add_directory("pics/", opts()).unwrap();
        z.start_file("pics/two.png", opts()).unwrap();
        z.write_all(b"2").unwrap();
        z.start_file("three.txt", opts()).unwrap();
        z.write_all(b"3").unwrap();
    });
    let l = open(&a, None).unwrap();
    // The folder chosen brings what's in it.
    let which = l.with_contents(&[1]);
    assert_eq!(which, [1, 2]);
    let out = d.join("out");
    fs::create_dir(&out).unwrap();
    extract(&a, &l, &out, Some(&which), None, roomy(), &Arc::new(Progress::default())).unwrap();
    assert_eq!(tree(&out), ["pics", "pics/two.png"]);
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn extract_here_does_not_nest() {
    let d = scratch("here");
    let one = d.join("Game.zip");
    zip_with(&one, |z| {
        z.start_file("Game/run.sh", opts()).unwrap();
        z.write_all(b"#!/bin/sh").unwrap();
    });
    let loose = d.join("loose.zip");
    zip_with(&loose, |z| {
        z.start_file("a.txt", opts()).unwrap();
        z.start_file("b.txt", opts()).unwrap();
    });
    let p = Arc::new(Progress::default());
    let (_, out) = extract_here(&one, None, &p).unwrap();
    assert_eq!(out, d.join("Game"));
    assert!(out.join("run.sh").is_file());
    let (_, out) = extract_here(&one, None, &p).unwrap();
    assert_eq!(out, d.join("Game (2)"));
    let (said, out) = extract_here(&loose, None, &p).unwrap();
    assert_eq!(out, d.join("loose"));
    assert!(said.contains("'loose'"), "{said}");
    assert!(out.join("a.txt").is_file() && out.join("b.txt").is_file());
    assert!(fs::read_dir(&d).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".ferro-extract")));
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn cancelling_stops_and_cleans_up() {
    let d = scratch("cancel");
    let a = d.join("c.zip");
    zip_with(&a, |z| {
        z.start_file("a.bin", opts()).unwrap();
        z.write_all(&vec![1u8; 1 << 20]).unwrap();
    });
    let l = open(&a, None).unwrap();
    let out = d.join("out");
    fs::create_dir(&out).unwrap();
    let p = Arc::new(Progress::default());
    p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(extract(&a, &l, &out, None, None, roomy(), &p), Err(Error::Cancelled));
    assert!(tree(&out).is_empty());
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn not_an_archive() {
    let d = scratch("junk");
    let a = d.join("junk.zip");
    fs::write(&a, b"PK\x03\x04 but not really a zip at all").unwrap();
    assert!(matches!(open(&a, None), Err(Error::Damaged(_) | Error::Unsupported(_))));
    let b = d.join("notes.txt");
    fs::write(&b, b"hello").unwrap();
    assert!(matches!(open(&b, None), Err(Error::Unsupported(_))));
    fs::remove_dir_all(&d).unwrap();
}

// ---------------------------------------------------------------- 7z

fn sevenz_with(path: &Path, password: Option<&str>, entries: &[(&str, Option<&[u8]>)]) {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, EncoderMethod};
    let mut w = ArchiveWriter::create(path).unwrap();
    if let Some(pw) = password {
        w.set_content_methods(vec![sevenz_rust2::encoder_options::AesEncoderOptions::new(pw.into()).into(), EncoderMethod::LZMA2.into()]);
        w.set_encrypt_header(true);
    }
    for (name, data) in entries {
        match data {
            Some(bytes) => {
                w.push_archive_entry(ArchiveEntry::new_file(name), Some(Cursor::new(bytes.to_vec()))).unwrap();
            }
            None => {
                w.push_archive_entry::<&[u8]>(ArchiveEntry::new_directory(name), None).unwrap();
            }
        }
    }
    w.finish().unwrap();
}

#[test]
fn sevenz_lists_and_extracts() {
    let d = scratch("7z");
    let a = d.join("docs.7z");
    let big = vec![b'z'; 300_000];
    sevenz_with(&a, None, &[("Docs", None), ("Docs/a.txt", Some(b"alpha")), ("Docs/deep/b.bin", Some(&big)), ("c.txt", Some(b"gamma"))]);
    let l = open(&a, None).unwrap();
    assert_eq!(l.format, Format::SevenZ);
    assert_eq!(l.entries.len(), 4);
    assert!(l.entries.iter().any(|e| e.name == "Docs" && e.is_dir));
    let (r, out) = unpack(&a, None, roomy());
    r.unwrap();
    assert_eq!(fs::read(out.join("Docs/a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(out.join("Docs/deep/b.bin")).unwrap(), big);
    assert_eq!(fs::read(out.join("c.txt")).unwrap(), b"gamma");

    // Some, from a solid block: what's before them is decoded, not written.
    let l = open(&a, None).unwrap();
    let c = l.entries.iter().position(|e| e.name == "c.txt").unwrap();
    let out2 = d.join("out2");
    fs::create_dir(&out2).unwrap();
    extract(&a, &l, &out2, Some(&[c]), None, roomy(), &Arc::new(Progress::default())).unwrap();
    assert_eq!(tree(&out2), ["c.txt"]);
    assert_eq!(fs::read(out2.join("c.txt")).unwrap(), b"gamma");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn sevenz_names_leaving_the_folder_are_skipped() {
    let d = scratch("7z-traversal");
    let a = d.join("evil.7z");
    sevenz_with(
        &a,
        None,
        &[("../escaped.txt", Some(b"x")), ("/abs.txt", Some(b"x")), ("a/../../escaped2.txt", Some(b"x")), ("ok.txt", Some(b"fine"))],
    );
    let (r, out) = unpack(&a, None, roomy());
    let r = r.unwrap();
    assert_eq!(r.unsafe_names.len(), 3, "{:?}", r.unsafe_names);
    assert_eq!(tree(&out), ["ok.txt"]);
    assert!(!d.join("escaped.txt").exists() && !d.join("escaped2.txt").exists());
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn sevenz_bomb_stops() {
    let d = scratch("7z-bomb");
    let a = d.join("bomb.7z");
    let zeros = vec![0u8; 16 << 20];
    sevenz_with(&a, None, &[("z1", Some(&zeros)), ("z2", Some(&zeros))]);
    let (r, _) = unpack(&a, None, Limits { max_total: 8 << 20, ..roomy() });
    assert!(matches!(r, Err(Error::TooBig(_))), "{r:?}");
    let (r, out) = unpack(&a, None, Limits { max_ratio: 100, ratio_after: 1 << 20, ..roomy() });
    assert!(matches!(r, Err(Error::TooBig(_))), "{r:?}");
    let left: Vec<_> = tree(&out).iter().map(|f| (f.clone(), fs::metadata(out.join(f)).unwrap().len())).collect();
    assert!(left.iter().all(|(_, n)| *n == 16 << 20), "{left:?}");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn sevenz_passwords() {
    let d = scratch("7z-pw");
    let a = d.join("secret.7z");
    sevenz_with(&a, Some("hunter2"), &[("secret.txt", Some(b"the plans"))]);
    // Its names are encrypted too: listing it needs the password.
    assert_eq!(open(&a, None).unwrap_err(), Error::NeedPassword);
    assert_eq!(open(&a, Some("wrong")).unwrap_err(), Error::WrongPassword);
    let (r, out) = unpack(&a, Some("hunter2"), roomy());
    r.unwrap();
    assert_eq!(fs::read(out.join("secret.txt")).unwrap(), b"the plans");
    fs::remove_dir_all(&d).unwrap();
}

// ---------------------------------------------------------------- RAR

fn rar_with(path: &Path, version: rars::ArchiveVersion, solid: bool, password: Option<&str>, entries: &[(&str, &[u8])]) -> rars::Result<()> {
    let mut b = rars::Builder::new(version).solid(solid).password(password.map(|p| p.as_bytes().to_vec()));
    for (name, data) in entries {
        b.add_bytes(name.as_bytes().to_vec(), data.to_vec(), Some(1_700_000_000), None)?;
    }
    b.write_to_path(path, None)
}

#[test]
fn rar5_lists_and_extracts() {
    let d = scratch("rar5");
    let a = d.join("music.rar");
    let big: Vec<u8> = (0..400_000u32).map(|i| (i % 251) as u8).collect();
    rar_with(&a, rars::ArchiveVersion::Rar50, false, None, &[("Album/01.txt", b"track one"), ("Album/big.bin", &big), ("cover.txt", b"cover")])
        .unwrap();
    let l = open(&a, None).unwrap();
    assert_eq!(l.format, Format::Rar);
    let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Album/01.txt", "Album/big.bin", "cover.txt"]);
    assert_eq!(l.entries[1].size, 400_000);
    assert!(l.entries[0].modified.is_some());
    let (r, out) = unpack(&a, None, roomy());
    assert_eq!(r.unwrap().files, 3);
    assert_eq!(fs::read(out.join("Album/01.txt")).unwrap(), b"track one");
    assert_eq!(fs::read(out.join("Album/big.bin")).unwrap(), big);
    assert_eq!(
        fs::metadata(out.join("cover.txt")).unwrap().modified().unwrap(),
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)
    );
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn rar4_and_solid_rar() {
    for (tag, version, solid) in [
        ("rar4", rars::ArchiveVersion::Rar29, false),
        ("rar4-solid", rars::ArchiveVersion::Rar29, true),
        ("rar5-solid", rars::ArchiveVersion::Rar50, true),
    ] {
        let d = scratch(tag);
        let a = d.join("old.rar");
        rar_with(&a, version, solid, None, &[("a.txt", b"first file, first file"), ("b.txt", b"second file, first file"), ("c.txt", b"third")])
            .unwrap();
        let l = open(&a, None).unwrap();
        assert_eq!(l.entries.len(), 3, "{tag}");
        let (r, out) = unpack(&a, None, roomy());
        assert_eq!(r.unwrap().files, 3, "{tag}");
        assert_eq!(fs::read(out.join("b.txt")).unwrap(), b"second file, first file", "{tag}");
        // Only the last: in a solid archive the others are decoded, not written.
        let out2 = d.join("out2");
        fs::create_dir(&out2).unwrap();
        extract(&a, &l, &out2, Some(&[2]), None, roomy(), &Arc::new(Progress::default())).unwrap();
        assert_eq!(tree(&out2), ["c.txt"], "{tag}");
        assert_eq!(fs::read(out2.join("c.txt")).unwrap(), b"third", "{tag}");
        fs::remove_dir_all(&d).unwrap();
    }
}

#[test]
fn rar_passwords() {
    let d = scratch("rar-pw");
    let a = d.join("secret.rar");
    rar_with(&a, rars::ArchiveVersion::Rar50, false, Some("hunter2"), &[("secret.txt", b"the plans")]).unwrap();
    let l = open(&a, None).unwrap();
    assert!(l.entries[0].encrypted);
    let (r, out) = unpack(&a, None, roomy());
    assert_eq!(r, Err(Error::NeedPassword));
    assert!(tree(&out).is_empty());
    let (r, out) = unpack(&a, Some("wrong"), roomy());
    assert_eq!(r, Err(Error::WrongPassword));
    assert!(tree(&out).is_empty());
    let (r, out) = unpack(&a, Some("hunter2"), roomy());
    r.unwrap();
    assert_eq!(fs::read(out.join("secret.txt")).unwrap(), b"the plans");

    // With its names encrypted too, even the list needs it.
    let b = d.join("hidden.rar");
    let mut builder = rars::Builder::new(rars::ArchiveVersion::Rar50).password(Some(b"hunter2".to_vec())).header_encryption(true);
    builder.add_bytes(b"x.txt".to_vec(), b"x".to_vec(), None, None).unwrap();
    builder.write_to_path(&b, None).unwrap();
    assert_eq!(open(&b, None).unwrap_err(), Error::NeedPassword);
    assert_eq!(open(&b, Some("wrong")).unwrap_err(), Error::WrongPassword);
    assert_eq!(open(&b, Some("hunter2")).unwrap().entries[0].name, "x.txt");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn rar_bomb_stops() {
    let d = scratch("rar-bomb");
    let a = d.join("bomb.rar");
    let zeros = vec![0u8; 16 << 20];
    rar_with(&a, rars::ArchiveVersion::Rar50, false, None, &[("z1", &zeros), ("z2", &zeros)]).unwrap();
    assert!(fs::metadata(&a).unwrap().len() < 1 << 20);
    let (r, _) = unpack(&a, None, Limits { max_total: 8 << 20, ..roomy() });
    assert!(matches!(r, Err(Error::TooBig(_))), "{r:?}");
    let (r, out) = unpack(&a, None, Limits { max_ratio: 100, ratio_after: 1 << 20, ..roomy() });
    assert!(matches!(r, Err(Error::TooBig(_))), "{r:?}");
    assert!(tree(&out).iter().all(|f| fs::metadata(out.join(f)).unwrap().len() == 16 << 20));
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn rar_links_and_unsafe_names_are_not_made() {
    let d = scratch("rar-evil");
    let a = d.join("evil.rar");
    let mut b = rars::Builder::new(rars::ArchiveVersion::Rar50);
    b.add_bytes(b"ok.txt".to_vec(), b"fine".to_vec(), None, None).unwrap();
    b.add_unix_symlink(b"out".to_vec(), b"../..".to_vec(), true, None, None).unwrap();
    // The RAR writer refuses names like "../x" itself; if it ever takes one,
    // it must still not land outside.
    let traversal = b.add_bytes(b"../escaped.txt".to_vec(), b"x".to_vec(), None, None).is_ok();
    b.write_to_path(&a, None).unwrap();
    let (r, out) = unpack(&a, None, roomy());
    let r = r.unwrap();
    assert_eq!(r.files, 1);
    assert_eq!(r.skipped_links, 1);
    assert_eq!(r.unsafe_names.len(), usize::from(traversal));
    assert_eq!(tree(&out), ["ok.txt"]);
    assert!(!d.join("escaped.txt").exists());
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn extract_here_works_sandboxed() {
    // As the program runs: Landlock on (it applies to this thread only),
    // which refuses to move anything from one folder to another.
    let d = scratch("here-sandboxed");
    let one = d.join("Game.zip");
    zip_with(&one, |z| {
        z.add_directory("Game/", opts()).unwrap();
        z.start_file("Game/data/level1.txt", opts()).unwrap();
        z.write_all(b"level").unwrap();
    });
    let loose = d.join("loose.zip");
    zip_with(&loose, |z| {
        z.start_file("a.txt", opts()).unwrap();
        z.start_file("b.txt", opts()).unwrap();
    });
    let (one2, loose2) = (one.clone(), loose.clone());
    let got = std::thread::spawn(move || {
        if ferro_sandbox::linux::landlock_abi() < 1 {
            eprintln!("no Landlock here: checked without it");
        } else {
            ferro_sandbox::linux::lock_privileges().unwrap();
            ferro_sandbox::linux::restrict_exec(&[]).unwrap();
        }
        let p = Arc::new(Progress::default());
        (extract_here(&one2, None, &p), extract_here(&one2, None, &p), extract_here(&loose2, None, &p))
    })
    .join()
    .unwrap();
    assert_eq!(got.0.unwrap().1, d.join("Game"));
    assert_eq!(fs::read(d.join("Game/data/level1.txt")).unwrap(), b"level");
    assert_eq!(got.1.unwrap().1, d.join("Game (2)"));
    assert!(d.join("Game (2)/data/level1.txt").is_file());
    assert_eq!(got.2.unwrap().1, d.join("loose"));
    assert!(d.join("loose/a.txt").is_file());
    assert!(fs::read_dir(&d).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".ferro-extract")));
    fs::remove_dir_all(&d).unwrap();
}
