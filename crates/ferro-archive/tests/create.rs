//! Archives made here, then read back: 7z and zip, each compression level,
//! with and without a password; and nothing left anywhere but the output.

use ferro_archive::create::{create, free_archive_name, suggested_stem, Level, NewFormat};
use ferro_archive::{extract, open, Error, Format, Limits, Progress, GB};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ferro-archive-test-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn roomy() -> Limits {
    Limits { max_total: 64 * GB, max_ratio: 1000, ratio_after: GB, max_entries: 100_000 }
}

/// A small tree: folders, an empty file and an empty folder, a program,
/// some text that compresses, some that doesn't, and a link (left out).
fn sample(d: &Path) -> PathBuf {
    let src = d.join("src/Trip");
    fs::create_dir_all(src.join("Photos")).unwrap();
    fs::create_dir_all(src.join("Empty")).unwrap();
    fs::write(src.join("notes.txt"), "a line of notes\n".repeat(5000)).unwrap();
    let noise: Vec<u8> = (0..200_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    fs::write(src.join("Photos/a.jpg"), &noise).unwrap();
    fs::write(src.join("Photos/empty.txt"), "").unwrap();
    fs::write(src.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", src.join("link")).unwrap();
    let t = fs::File::options().write(true).open(src.join("notes.txt")).unwrap();
    t.set_modified(UNIX_EPOCH + Duration::from_secs(1_600_000_000)).unwrap();
    src
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

/// Makes, lists and unpacks; checks what comes out is what went in.
fn round_trip(tag: &str, format: NewFormat, level: Level, password: Option<&str>) -> PathBuf {
    let d = scratch(tag);
    let src = sample(&d);
    let a = d.join(format!("Trip.{}", format.ext()));
    let made = create(std::slice::from_ref(&src), &a, format, level, password, &Arc::new(Progress::default())).unwrap();
    assert_eq!((made.files, made.folders, made.skipped), (4, 3, 1), "{tag}");

    let l = open(&a, password).unwrap();
    assert_eq!(l.format, if format == NewFormat::Zip { Format::Zip } else { Format::SevenZ });
    let mut names: Vec<_> = l.entries.iter().map(|e| e.name.trim_end_matches('/').to_owned()).collect();
    names.sort();
    assert_eq!(names, ["Trip", "Trip/Empty", "Trip/Photos", "Trip/Photos/a.jpg", "Trip/Photos/empty.txt", "Trip/notes.txt", "Trip/run.sh"], "{tag}");
    assert_eq!(l.entries.iter().any(|e| e.encrypted), password.is_some(), "{tag}");

    let out = d.join("out");
    fs::create_dir(&out).unwrap();
    extract(&a, &l, &out, None, password, roomy(), &Arc::new(Progress::default())).unwrap();
    for f in ["notes.txt", "Photos/a.jpg", "Photos/empty.txt", "run.sh"] {
        assert_eq!(fs::read(out.join("Trip").join(f)).unwrap(), fs::read(src.join(f)).unwrap(), "{tag} {f}");
    }
    assert!(out.join("Trip/Empty").is_dir(), "{tag}");
    assert!(!out.join("Trip/link").exists(), "{tag}");
    let mtime = fs::metadata(out.join("Trip/notes.txt")).unwrap().modified().unwrap();
    let off = mtime.duration_since(UNIX_EPOCH).unwrap().as_secs().abs_diff(1_600_000_000);
    assert!(off <= 2, "{tag}: time off by {off}s");
    assert_eq!(fs::metadata(out.join("Trip/run.sh")).unwrap().permissions().mode() & 0o100, 0o100, "{tag}: still runs");
    a
}

#[test]
fn sevenz_round_trips() {
    for (tag, level) in [("7z-store", Level::Store), ("7z-normal", Level::Normal), ("7z-max", Level::Maximum)] {
        let a = round_trip(tag, NewFormat::SevenZ, level, None);
        // Only the time it was changed: no created or opened times.
        let r = sevenz_rust2::ArchiveReader::open(&a, sevenz_rust2::Password::empty()).unwrap();
        assert!(r.archive().files.iter().all(|f| !f.has_creation_date && !f.has_access_date), "{tag}");
        fs::remove_dir_all(a.parent().unwrap()).unwrap();
    }
}

#[test]
fn zip_round_trips() {
    let mut sizes = Vec::new();
    for (tag, level) in [("zip-store", Level::Store), ("zip-normal", Level::Normal), ("zip-max", Level::Maximum)] {
        let a = round_trip(tag, NewFormat::Zip, level, None);
        let bytes = fs::read(&a).unwrap();
        // No owner (Info-ZIP's "ux"/"Ux") nor extra times ("UT").
        for id in [[0x75u8, 0x78], [0x55, 0x78], [0x55, 0x54]] {
            assert!(!bytes.windows(2).any(|w| w == id), "{tag}: extra field {id:?}");
        }
        sizes.push(bytes.len());
        fs::remove_dir_all(a.parent().unwrap()).unwrap();
    }
    assert!(sizes[1] < sizes[0] && sizes[2] <= sizes[1], "{sizes:?}");
}

#[test]
fn sevenz_with_a_password_hides_the_names_too() {
    let a = round_trip("7z-pw", NewFormat::SevenZ, Level::Normal, Some("correct horse"));
    assert_eq!(open(&a, None).unwrap_err(), Error::NeedPassword);
    assert_eq!(open(&a, Some("wrong")).unwrap_err(), Error::WrongPassword);
    let bytes = fs::read(&a).unwrap();
    assert!(
        !bytes.windows(9).any(|w| w == b"notes.txt")
            && !bytes.windows(18).any(|w| w == "notes.txt".encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>())
    );
    fs::remove_dir_all(a.parent().unwrap()).unwrap();
}

#[test]
fn zip_with_a_password_is_aes() {
    let a = round_trip("zip-pw", NewFormat::Zip, Level::Normal, Some("correct horse"));
    let bytes = fs::read(&a).unwrap();
    // AES (method 99, its 0x9901 field), never ZipCrypto.
    assert!(bytes.windows(2).any(|w| w == [0x01, 0x99]));
    let mut z = zip::ZipArchive::new(fs::File::open(&a).unwrap()).unwrap();
    for i in 0..z.len() {
        let f = z.by_index_raw(i).unwrap();
        if !f.is_dir() {
            assert!(f.encrypted());
            assert!(f.extra_data_fields().any(|x| matches!(x, zip::extra_fields::ExtraField::AeXEncryption(_))), "{}", f.name().unwrap());
        }
    }
    let l = open(&a, None).unwrap();
    let out = a.parent().unwrap().join("out2");
    fs::create_dir(&out).unwrap();
    assert_eq!(extract(&a, &l, &out, None, None, roomy(), &Arc::new(Progress::default())), Err(Error::NeedPassword));
    assert_eq!(extract(&a, &l, &out, None, Some("wrong"), roomy(), &Arc::new(Progress::default())), Err(Error::WrongPassword));
    assert!(tree(&out).iter().all(|p| !p.contains(".ferro-part")));
    fs::remove_dir_all(a.parent().unwrap()).unwrap();
}

#[test]
fn nothing_is_left_behind() {
    // What's in the temp folder (other tests' own folders aside), and the
    // folders around the work, before and after.
    let ours = |n: &str| n.starts_with(&format!("ferro-archive-test-{}-", std::process::id()));
    let tmp = || -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !ours(n))
            .collect();
        v.sort();
        v
    };
    let d = scratch("leftovers");
    let src = sample(&d);
    let before = tmp();
    let a = d.join("Trip.7z");
    create(std::slice::from_ref(&src), &a, NewFormat::SevenZ, Level::Normal, Some("pw"), &Arc::new(Progress::default())).unwrap();
    let out = d.join("out");
    fs::create_dir(&out).unwrap();
    let l = open(&a, Some("pw")).unwrap();
    extract(&a, &l, &out, None, Some("pw"), roomy(), &Arc::new(Progress::default())).unwrap();
    let mut top: Vec<_> = fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    top.sort();
    assert_eq!(top, ["Trip.7z", "out", "src"]);
    assert!(tree(&out).iter().all(|p| p == "Trip" || p.starts_with("Trip/")));
    assert!(tree(&out).iter().all(|p| !p.contains(".ferro-part")));
    let after = tmp();
    let new: Vec<_> = after.iter().filter(|n| !before.contains(n) && n.contains("ferro")).collect();
    assert!(new.is_empty(), "left in the temp folder: {new:?}");

    // Cancelled part way: no archive, no hidden part.
    let p = Arc::new(Progress::default());
    p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let b = d.join("Cancelled.zip");
    assert_eq!(create(std::slice::from_ref(&src), &b, NewFormat::Zip, Level::Normal, None, &p), Err(Error::Cancelled));
    assert!(!b.exists());
    assert!(fs::read_dir(&d).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".ferro-part")));
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn never_over_an_archive_already_there() {
    let d = scratch("taken");
    let src = sample(&d);
    let a = d.join("Trip.7z");
    fs::write(&a, "mine").unwrap();
    assert!(create(&[src], &a, NewFormat::SevenZ, Level::Store, None, &Arc::new(Progress::default())).is_err());
    assert_eq!(fs::read(&a).unwrap(), b"mine");
    assert_eq!(free_archive_name(&d, "Trip", NewFormat::SevenZ), d.join("Trip (2).7z"));
    assert_eq!(free_archive_name(&d, "Trip", NewFormat::Zip), d.join("Trip.zip"));
    assert_eq!(suggested_stem(&[d.join("src/Trip")]), "Trip");
    assert_eq!(suggested_stem(&[d.join("src/Trip/notes.txt")]), "notes");
    assert_eq!(suggested_stem(&[d.join("src/Trip/notes.txt"), d.join("src/Trip/run.sh")]), "Trip");
    fs::remove_dir_all(&d).unwrap();
}
