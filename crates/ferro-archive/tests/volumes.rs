//! Archives split into several files: RAR volumes under both ways of
//! naming them, and archives cut into numbered pieces. Opened from any
//! part; a missing part is named.
//!
//! `fixtures/rar-volumes` holds sets made by RAR itself (see the README
//! there); the rest are made here with the `rars` writer.

use ferro_archive::{archive_stem, extract, extract_here, open, Error, Format, Limits, Progress, GB};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ferro-archive-test-{}-vol-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn roomy() -> Limits {
    Limits { max_total: 64 * GB, max_ratio: 1000, ratio_after: GB, max_entries: 100_000 }
}

/// Unpacks all of `archive` into a new, empty folder `out` beside it.
fn unpack(archive: &Path, password: Option<&str>, limits: Limits) -> (Result<ferro_archive::Report, Error>, PathBuf) {
    let out = archive.parent().unwrap().join("out");
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    let listing = match open(archive, password) {
        Ok(l) => l,
        Err(e) => return (Err(e), out),
    };
    (extract(archive, &listing, &out, None, password, limits, &Arc::new(Progress::default())), out)
}

/// Every file under `dir`, relative, sorted.
fn tree(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, at: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(at).unwrap().flatten() {
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                walk(base, &p, out);
            } else {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, dir, &mut v);
    v.sort();
    v
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in data {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (!(crc & 1)).wrapping_add(1));
        }
    }
    !crc
}

/// Bytes that don't compress, the same every run.
fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as u8
        })
        .collect()
}

/// A folder holding copies of these fixture files.
fn fixture_set(tag: &str, files: &[&str]) -> PathBuf {
    let d = scratch(tag);
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rar-volumes");
    for f in files {
        fs::copy(from.join(f), d.join(f)).unwrap();
    }
    d
}

struct Real {
    parts: &'static [&'static str],
    password: Option<&'static str>,
    /// Even the list needs the password.
    hidden: bool,
    /// Name, size, CRC-32 (where it's known from elsewhere).
    holds: &'static [(&'static str, u64, Option<u32>)],
}

const BIGTEXT: (&str, u64, Option<u32>) = ("bigtext_64k.bin", 65_536, Some(0xddc9_5682));

const REAL: &[Real] = &[
    // RAR 5, `.partN.rar`.
    Real {
        parts: &["multivol.part1.rar", "multivol.part2.rar", "multivol.part3.rar"],
        password: None,
        hidden: false,
        holds: &[("random_4k.bin", 4096, Some(0xb9c5_4415))],
    },
    Real {
        parts: &["encrypted_multivol.part1.rar", "encrypted_multivol.part2.rar", "encrypted_multivol.part3.rar"],
        password: Some("password"),
        hidden: false,
        holds: &[("random_4k.bin", 4096, Some(0xb9c5_4415))],
    },
    Real {
        parts: &[
            "header_encrypted_stored_multivol.part1.rar",
            "header_encrypted_stored_multivol.part2.rar",
            "header_encrypted_stored_multivol.part3.rar",
        ],
        password: Some("password"),
        hidden: true,
        holds: &[("stored_4k.bin", 4096, Some(0xa087_a9af))],
    },
    // RAR 5, solid, two-digit numbers, two files.
    Real {
        parts: &[
            "solid_multivol.part01.rar",
            "solid_multivol.part02.rar",
            "solid_multivol.part03.rar",
            "solid_multivol.part04.rar",
            "solid_multivol.part05.rar",
            "solid_multivol.part06.rar",
        ],
        password: None,
        hidden: false,
        holds: &[("tiny.txt", 9, Some(0xd27b_5891)), BIGTEXT],
    },
    // RAR 3 (the RAR4 format), `.rar`, `.r00`...
    Real { parts: &["multivol_oldnaming_rar300.rar", "multivol_oldnaming_rar300.r00"], password: None, hidden: false, holds: &[BIGTEXT] },
    Real {
        parts: &[
            "compressed_multivol_prng_rar300.rar",
            "compressed_multivol_prng_rar300.r00",
            "compressed_multivol_prng_rar300.r01",
            "compressed_multivol_prng_rar300.r02",
            "compressed_multivol_prng_rar300.r03",
        ],
        password: None,
        hidden: false,
        holds: &[("cvolume.bin", 4096, Some(0x96de_2bef))],
    },
    Real { parts: &["encrypted_multivol_rar300.rar", "encrypted_multivol_rar300.r00"], password: Some("password"), hidden: false, holds: &[BIGTEXT] },
    // RAR 3, `.partNN.rar`.
    Real {
        parts: &["multivol_newnaming_rar300.part01.rar", "multivol_newnaming_rar300.part02.rar"],
        password: None,
        hidden: false,
        holds: &[BIGTEXT],
    },
    Real {
        parts: &["header_encrypted_newnaming_rar300.part01.rar", "header_encrypted_newnaming_rar300.part02.rar"],
        password: Some("password"),
        hidden: true,
        holds: &[BIGTEXT],
    },
    // RAR 1.x, names in capitals.
    Real {
        parts: &["MULTIVOL.RAR", "MULTIVOL.R00", "MULTIVOL.R01", "MULTIVOL.R02"],
        password: None,
        hidden: false,
        holds: &[("RANDOM.BIN", 65_536, None)],
    },
];

#[test]
fn sets_made_by_rar_open_from_any_part() {
    for (n, set) in REAL.iter().enumerate() {
        let d = fixture_set(&format!("real-{n}"), set.parts);
        let mut first: Option<Vec<Vec<u8>>> = None;
        // (With a password, from the first and the last part only: the key
        // is slow to work out, and it's worked out for every part.)
        let from: Vec<&str> = match set.password {
            Some(_) => vec![set.parts[0], set.parts[set.parts.len() - 1]],
            None => set.parts.to_vec(),
        };
        for part in from {
            let a = d.join(part);
            let first_part = first.is_none();
            if !first_part {
                // (Wrong passwords: tried from the first part only; slow.)
            } else if set.hidden {
                assert_eq!(open(&a, None).unwrap_err(), Error::NeedPassword, "{part}");
                assert_eq!(open(&a, Some("wrong")).unwrap_err(), Error::WrongPassword, "{part}");
            } else if set.password.is_some() {
                assert!(open(&a, None).unwrap().entries[0].encrypted, "{part}");
                let (r, out) = unpack(&a, None, roomy());
                assert_eq!(r, Err(Error::NeedPassword), "{part}");
                assert!(tree(&out).is_empty(), "{part}");
                let (r, out) = unpack(&a, Some("wrong"), roomy());
                assert_eq!(r, Err(Error::WrongPassword), "{part}");
                assert!(tree(&out).is_empty(), "{part}");
            }
            let l = open(&a, set.password).unwrap_or_else(|e| panic!("{part}: {e}"));
            assert_eq!(l.format, Format::Rar);
            // One entry for each file, however many parts it runs across.
            let listed: Vec<(&str, u64)> = l.entries.iter().map(|e| (e.name.as_str(), e.size)).collect();
            let expect: Vec<(&str, u64)> = set.holds.iter().map(|(name, size, _)| (*name, *size)).collect();
            assert_eq!(listed, expect, "{part}");
            let (r, out) = unpack(&a, set.password, roomy());
            assert_eq!(r.unwrap_or_else(|e| panic!("{part}: {e}")).files, set.holds.len(), "{part}");
            let mut got = Vec::new();
            for (name, size, crc) in set.holds {
                let data = fs::read(out.join(name)).unwrap();
                assert_eq!(data.len() as u64, *size, "{part}: {name}");
                if let Some(crc) = crc {
                    assert_eq!(crc32(&data), *crc, "{part}: {name}");
                }
                got.push(data);
            }
            // The same from every part.
            match &first {
                Some(f) => assert_eq!(*f, got, "{part}"),
                None => first = Some(got),
            }
            // Nothing but the files asked for was written.
            assert_eq!(tree(&out).len(), set.holds.len(), "{part}");
        }
        let mut left: Vec<String> = tree(&d).into_iter().filter(|f| !f.starts_with("out/")).collect();
        left.sort();
        let mut parts: Vec<String> = set.parts.iter().map(|p| (*p).to_owned()).collect();
        parts.sort();
        assert_eq!(left, parts, "nothing else beside the archive");
        fs::remove_dir_all(&d).unwrap();
    }
}

#[test]
fn a_missing_part_is_named() {
    let solid: &[&str] = REAL[3].parts;
    // (the set, the one taken away, the one opened)
    let cases: &[(&[&str], &str, &str)] = &[
        (solid, "solid_multivol.part03.rar", "solid_multivol.part01.rar"),
        (solid, "solid_multivol.part03.rar", "solid_multivol.part05.rar"),
        (solid, "solid_multivol.part01.rar", "solid_multivol.part02.rar"),
        (solid, "solid_multivol.part06.rar", "solid_multivol.part01.rar"),
        (REAL[0].parts, "multivol.part3.rar", "multivol.part2.rar"),
        (REAL[4].parts, "multivol_oldnaming_rar300.r00", "multivol_oldnaming_rar300.rar"),
        (REAL[4].parts, "multivol_oldnaming_rar300.rar", "multivol_oldnaming_rar300.r00"),
        (REAL[5].parts, "compressed_multivol_prng_rar300.r01", "compressed_multivol_prng_rar300.r03"),
        (REAL[5].parts, "compressed_multivol_prng_rar300.r03", "compressed_multivol_prng_rar300.rar"),
        (REAL[9].parts, "MULTIVOL.R02", "MULTIVOL.RAR"),
    ];
    for (n, (parts, gone, opened)) in cases.iter().enumerate() {
        let d = fixture_set(&format!("missing-{n}"), parts);
        fs::remove_file(d.join(gone)).unwrap();
        let e = open(&d.join(opened), None).unwrap_err();
        // Names are matched whatever their case; the one asked for is spelt
        // as the format does.
        assert!(e.to_string().to_lowercase().contains(&format!("'{}' is missing", gone.to_lowercase())), "{opened} without {gone}: {e}");
        let (r, out) = unpack(&d.join(opened), None, roomy());
        assert!(r.is_err());
        assert!(tree(&out).is_empty());
        fs::remove_dir_all(&d).unwrap();
    }
}

/// A RAR5 set made here: `volume` bytes a part, named `{stem}.partN.rar`.
fn rar5_set(dir: &Path, stem: &str, volume: usize, password: Option<&str>, hide: bool, entries: &[(&str, &[u8])]) -> Vec<PathBuf> {
    let mut b = rars::Builder::new(rars::ArchiveVersion::Rar50)
        .volume_size(Some(volume))
        .password(password.map(|p| p.as_bytes().to_vec()))
        .header_encryption(hide);
    for (name, data) in entries {
        b.add_bytes(name.as_bytes().to_vec(), data.to_vec(), Some(1_700_000_000), None).unwrap();
    }
    let volumes = b.build_volumes(None).unwrap();
    assert!(volumes.len() > 1, "one volume only");
    volumes
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let p = dir.join(format!("{stem}.part{}.rar", i + 1));
            fs::write(&p, v).unwrap();
            p
        })
        .collect()
}

#[test]
fn a_set_made_here_with_many_files() {
    let d = scratch("made");
    let (big, other) = (noise(300_000, 7), noise(90_000, 99));
    let entries: &[(&str, &[u8])] = &[
        ("Album/01.txt", b"track one"),
        ("Album/big.bin", &big),
        ("Album/sub/other.bin", &other),
        ("cover.txt", b"cover"),
        ("last.txt", b"the end"),
    ];
    let parts = rar5_set(&d, "music", 64 << 10, None, false, entries);
    assert!(parts.len() >= 6, "{} parts", parts.len());
    for part in [&parts[0], &parts[2], parts.last().unwrap()] {
        let l = open(part, None).unwrap();
        let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Album/01.txt", "Album/big.bin", "Album/sub/other.bin", "cover.txt", "last.txt"]);
        assert_eq!(l.entries[1].size, 300_000);
        let (r, out) = unpack(part, None, roomy());
        assert_eq!(r.unwrap().files, 5);
        for (name, data) in entries {
            assert_eq!(fs::read(out.join(name)).unwrap(), *data, "{name}");
        }
        assert_eq!(
            fs::metadata(out.join("cover.txt")).unwrap().modified().unwrap(),
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)
        );
        // Again into the same folder: nothing is replaced.
        let again = extract(part, &l, &out, None, None, roomy(), &Arc::new(Progress::default())).unwrap();
        assert_eq!((again.files, again.existing), (0, 5));
    }
    // Only some: one that runs across parts, one early (the rest is then
    // not gone through), and the last.
    let l = open(&parts[1], None).unwrap();
    for (which, expect) in
        [(vec![2usize], vec!["Album/sub/other.bin"]), (vec![0], vec!["Album/01.txt"]), (vec![1, 4], vec!["Album/big.bin", "last.txt"])]
    {
        let out = d.join("some");
        let _ = fs::remove_dir_all(&out);
        fs::create_dir(&out).unwrap();
        let r = extract(&parts[1], &l, &out, Some(&which), None, roomy(), &Arc::new(Progress::default())).unwrap();
        assert_eq!(r.files, expect.len(), "{which:?}");
        assert_eq!(tree(&out), expect, "{which:?}");
        for i in which {
            assert_eq!(fs::read(out.join(entries[i].0)).unwrap(), entries[i].1);
        }
    }
    // Extract Here, from a part in the middle: named for the set.
    let (said, to) = extract_here(&parts[3], None, &Arc::new(Progress::default())).unwrap();
    assert_eq!(to, d.join("music"), "{said}");
    assert_eq!(fs::read(to.join("Album/big.bin")).unwrap(), big);
    assert_eq!(archive_stem("music.part4.rar"), "music");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn sets_made_here_with_passwords() {
    let d = scratch("made-pw");
    let big = noise(200_000, 3);
    let parts = rar5_set(&d, "secret", 50_000, Some("hunter2"), false, &[("a.txt", b"the plans"), ("big.bin", &big)]);
    let part = &parts[parts.len() - 1];
    assert!(open(part, None).unwrap().entries.iter().all(|e| e.encrypted));
    assert_eq!(unpack(part, None, roomy()).0, Err(Error::NeedPassword));
    let (r, out) = unpack(part, Some("wrong"), roomy());
    assert_eq!(r, Err(Error::WrongPassword));
    assert!(tree(&out).is_empty());
    let (r, out) = unpack(part, Some("hunter2"), roomy());
    assert_eq!(r.unwrap().files, 2);
    assert_eq!(fs::read(out.join("big.bin")).unwrap(), big);

    // Names encrypted too.
    let parts = rar5_set(&d, "hidden", 50_000, Some("hunter2"), true, &[("a.txt", b"the plans"), ("big.bin", &big)]);
    for part in &parts {
        assert_eq!(open(part, None).unwrap_err(), Error::NeedPassword);
        assert_eq!(open(part, Some("wrong")).unwrap_err(), Error::WrongPassword);
        let (r, out) = unpack(part, Some("hunter2"), roomy());
        assert_eq!(r.unwrap().files, 2);
        assert_eq!(fs::read(out.join("big.bin")).unwrap(), big);
    }
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn an_old_style_set_made_here() {
    // The RAR4 writer splits one file only.
    let d = scratch("made-old");
    let big = noise(150_000, 11);
    let mut b = rars::Builder::new(rars::ArchiveVersion::Rar29).volume_size(Some(40_000));
    b.add_bytes(b"film.bin".to_vec(), big.clone(), Some(1_700_000_000), None).unwrap();
    let volumes = b.build_volumes(None).unwrap();
    assert!(volumes.len() >= 4);
    let names: Vec<String> = (0..volumes.len()).map(|i| if i == 0 { "film.rar".to_owned() } else { format!("film.r{:02}", i - 1) }).collect();
    for (name, v) in names.iter().zip(&volumes) {
        fs::write(d.join(name), v).unwrap();
    }
    // Beside it, another archive that isn't of the set.
    fs::write(d.join("film2.r00"), b"not a rar").unwrap();
    for name in &names {
        let l = open(&d.join(name), None).unwrap();
        assert_eq!(l.entries.len(), 1, "{name}");
        assert_eq!(l.entries[0].size, 150_000, "{name}");
        let (r, out) = unpack(&d.join(name), None, roomy());
        assert_eq!(r.unwrap().files, 1, "{name}");
        assert_eq!(fs::read(out.join("film.bin")).unwrap(), big, "{name}");
    }
    fs::remove_file(d.join("film.r01")).unwrap();
    let e = open(&d.join("film.rar"), None).unwrap_err();
    assert!(e.to_string().contains("'film.r01' is missing"), "{e}");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn an_archive_in_one_file_is_not_taken_for_a_set() {
    let d = scratch("alone");
    let mut b = rars::Builder::new(rars::ArchiveVersion::Rar50);
    b.add_bytes(b"x.txt".to_vec(), b"alone".to_vec(), None, None).unwrap();
    // Named like parts, and with such files beside them: still one file each.
    for name in ["notes.rar", "trip.part1.rar"] {
        b.write_to_path(&d.join(name), None).unwrap();
    }
    fs::write(d.join("notes.r00"), b"something else").unwrap();
    fs::write(d.join("trip.part2.rar"), b"something else").unwrap();
    for name in ["notes.rar", "trip.part1.rar"] {
        let (r, out) = unpack(&d.join(name), None, roomy());
        assert_eq!(r.unwrap().files, 1, "{name}");
        assert_eq!(fs::read(out.join("x.txt")).unwrap(), b"alone");
    }
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_part_that_is_something_else_is_refused() {
    let d = scratch("mixed");
    let big = noise(100_000, 5);
    let parts = rar5_set(&d, "a", 30_000, None, false, &[("big.bin", &big)]);
    // The second part swapped for an archive of its own.
    let mut b = rars::Builder::new(rars::ArchiveVersion::Rar50);
    b.add_bytes(b"x.txt".to_vec(), b"alone".to_vec(), None, None).unwrap();
    b.write_to_path(&parts[1], None).unwrap();
    let e = open(&parts[0], None).unwrap_err();
    assert!(matches!(e, Error::Damaged(_)), "{e}");
    // And for a part of another set: the decoder notices.
    let other = scratch("mixed-other");
    let others = rar5_set(&other, "a", 30_000, None, false, &[("big.bin", &noise(100_000, 6))]);
    fs::copy(&others[1], &parts[1]).unwrap();
    let (r, out) = unpack(&parts[0], None, roomy());
    assert!(r.is_err(), "{r:?}");
    assert!(tree(&out).is_empty());
    fs::remove_dir_all(&d).unwrap();
    fs::remove_dir_all(&other).unwrap();
}

#[test]
fn a_bomb_in_parts_stops() {
    let d = scratch("bomb");
    let zeros = vec![0u8; 16 << 20];
    let parts = rar5_set(&d, "bomb", 2000, None, false, &[("z1", &zeros), ("z2", &zeros), ("z3", &zeros)]);
    let size: u64 = parts.iter().map(|p| fs::metadata(p).unwrap().len()).sum();
    assert!(size < 1 << 20, "{size}");
    let (r, out) = unpack(&parts[0], None, Limits { max_total: 8 << 20, ..roomy() });
    assert!(matches!(r, Err(Error::TooBig(_))), "{r:?}");
    assert!(tree(&out).is_empty());
    // By how much it grows, counted against all the parts together.
    let (r, out) = unpack(&parts[1], None, Limits { max_ratio: 100, ratio_after: 1 << 20, ..roomy() });
    assert!(matches!(r, Err(Error::TooBig(_))), "{r:?}");
    assert!(tree(&out).iter().all(|f| fs::metadata(out.join(f)).unwrap().len() == 16 << 20));
    assert!(tree(&out).len() < 3);
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn nothing_from_a_set_is_written_through_a_link() {
    let d = scratch("evil");
    let big = noise(60_000, 21);
    let mut b = rars::Builder::new(rars::ArchiveVersion::Rar50).volume_size(Some(20_000));
    b.add_bytes(b"ok.txt".to_vec(), b"fine".to_vec(), None, None).unwrap();
    b.add_bytes(b"dir/big.bin".to_vec(), big.clone(), None, None).unwrap();
    b.add_bytes(b"end.txt".to_vec(), b"end".to_vec(), None, None).unwrap();
    let volumes = b.build_volumes(None).unwrap();
    assert!(volumes.len() > 1);
    for (i, v) in volumes.iter().enumerate() {
        fs::write(d.join(format!("evil.part{}.rar", i + 1)), v).unwrap();
    }
    let a = d.join("evil.part2.rar");
    let l = open(&a, None).unwrap();
    // (The writer here puts no links into a set, so those aren't tried.)
    // `dir` is already there as a link leading out: not followed.
    let out = d.join("out");
    let outside = d.join("outside");
    fs::create_dir_all(&out).unwrap();
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, out.join("dir")).unwrap();
    let r = extract(&a, &l, &out, None, None, roomy(), &Arc::new(Progress::default())).unwrap();
    assert_eq!(r.files, 2, "{r:?}");
    assert_eq!(r.unsafe_names, ["dir/big.bin"]);
    assert!(tree(&outside).is_empty());
    assert_eq!(fs::read(out.join("end.txt")).unwrap(), b"end");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn cancelling_a_set_stops_and_cleans_up() {
    let d = scratch("cancel");
    let big = noise(400_000, 31);
    let parts = rar5_set(&d, "c", 50_000, None, false, &[("big.bin", &big)]);
    let l = open(&parts[0], None).unwrap();
    let out = d.join("out");
    fs::create_dir(&out).unwrap();
    let p = Arc::new(Progress::default());
    p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(extract(&parts[0], &l, &out, None, None, roomy(), &p), Err(Error::Cancelled));
    assert!(fs::read_dir(&out).unwrap().next().is_none());
    fs::remove_dir_all(&d).unwrap();
}

// ------------------------------------------------- numbered pieces

/// `whole` cut into `name.001`, `name.002`... of `piece` bytes.
fn cut(dir: &Path, name: &str, whole: &[u8], piece: usize) -> Vec<PathBuf> {
    whole
        .chunks(piece)
        .enumerate()
        .map(|(i, c)| {
            let p = dir.join(format!("{name}.{:03}", i + 1));
            fs::write(&p, c).unwrap();
            p
        })
        .collect()
}

#[test]
fn archives_cut_into_numbered_pieces() {
    let d = scratch("pieces");
    let (big, small) = (noise(120_000, 41), b"small file".to_vec());

    // 7z
    let whole = d.join("whole.7z");
    {
        let mut w = sevenz_rust2::ArchiveWriter::create(&whole).unwrap();
        for (name, data) in [("docs/big.bin", &big), ("small.txt", &small)] {
            w.push_archive_entry(sevenz_rust2::ArchiveEntry::new_file(name), Some(std::io::Cursor::new(data.clone()))).unwrap();
        }
        w.finish().unwrap();
    }
    let sevenz = cut(&d, "set.7z", &fs::read(&whole).unwrap(), 25_000);
    fs::remove_file(&whole).unwrap();

    // zip
    let whole = d.join("whole.zip");
    {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(fs::File::create(&whole).unwrap());
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("docs/big.bin", o).unwrap();
        z.write_all(&big).unwrap();
        z.start_file("small.txt", o).unwrap();
        z.write_all(&small).unwrap();
        z.finish().unwrap();
    }
    let zipped = cut(&d, "set.zip", &fs::read(&whole).unwrap(), 25_000);
    fs::remove_file(&whole).unwrap();

    // rar
    let whole = d.join("whole.rar");
    {
        let mut b = rars::Builder::new(rars::ArchiveVersion::Rar50);
        b.add_bytes(b"docs/big.bin".to_vec(), big.clone(), None, None).unwrap();
        b.add_bytes(b"small.txt".to_vec(), small.clone(), None, None).unwrap();
        b.write_to_path(&whole, None).unwrap();
    }
    let rared = cut(&d, "set.rar", &fs::read(&whole).unwrap(), 25_000);
    fs::remove_file(&whole).unwrap();

    for (set, format) in [(&sevenz, Format::SevenZ), (&zipped, Format::Zip), (&rared, Format::Rar)] {
        assert!(set.len() >= 4, "{format:?}: {} pieces", set.len());
        for piece in set.iter() {
            let l = open(piece, None).unwrap_or_else(|e| panic!("{}: {e}", piece.display()));
            assert_eq!(l.format, format);
            let (r, out) = unpack(piece, None, roomy());
            assert_eq!(r.unwrap().files, 2, "{}", piece.display());
            assert_eq!(fs::read(out.join("docs/big.bin")).unwrap(), big);
            assert_eq!(fs::read(out.join("small.txt")).unwrap(), small);
        }
        let name = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
        // A piece gone from the middle, opened before it and after it.
        fs::rename(&set[1], d.join("kept")).unwrap();
        for piece in [&set[0], &set[3]] {
            let e = open(piece, None).unwrap_err();
            assert!(e.to_string().contains(&format!("'{}' is missing", name(&set[1]))), "{e}");
        }
        fs::rename(d.join("kept"), &set[1]).unwrap();
        // The first gone.
        fs::remove_file(&set[0]).unwrap();
        let e = open(&set[2], None).unwrap_err();
        assert!(e.to_string().contains(&format!("'{}' is missing", name(&set[0]))), "{e}");
    }
    assert_eq!(archive_stem("set.7z.002"), "set");
    fs::remove_dir_all(&d).unwrap();
}
