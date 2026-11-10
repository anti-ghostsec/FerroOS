//! The jail's Landlock rules, on a shell started inside them. (Nothing is
//! tested on a kernel without Landlock.)
#![cfg(target_os = "linux")]

use ferro_sandbox::linux::{self, Jail};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn folders(name: &str, names: &[&str]) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("rules-{name}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    for d in names {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    root.canonicalize().unwrap()
}

/// What `script` printed, run by a shell confined by `jail` (which may
/// start the usual tools).
fn inside(mut jail: Jail, dir: &Path, script: &str) -> String {
    jail.exec.extend(["/bin".to_owned(), "/usr/bin".to_owned()]);
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", script]).current_dir(dir);
    // SAFETY: the child only makes the syscalls of lock_privileges and jail.
    unsafe {
        cmd.pre_exec(move || linux::lock_privileges().and_then(|()| linux::jail(&jail)).map(drop));
    }
    let out = cmd.output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn text(p: PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn files_are_read_but_not_changed() {
    if linux::landlock_abi() < 1 {
        return;
    }
    let root = folders("read", &["docs"]);
    fs::write(root.join("docs/a"), "as it was\n").unwrap();
    let said = inside(
        Jail::default(),
        &root,
        "cat docs/a; echo x >>docs/a; echo x >docs/new; rm docs/a; mv docs/a docs/b; mkdir docs/d; ln -s a docs/l; ln docs/a docs/h; echo done",
    );
    assert_eq!(said, "as it was\ndone\n");
    assert_eq!(fs::read_dir(root.join("docs")).unwrap().count(), 1, "nothing made, nothing removed");
    assert_eq!(fs::read_to_string(root.join("docs/a")).unwrap(), "as it was\n");
}

#[test]
fn a_changed_folder_takes_plain_files_only() {
    if linux::landlock_abi() < 1 {
        return;
    }
    let root = folders("change", &["work", "other"]);
    fs::write(root.join("work/a"), "one\n").unwrap();
    fs::write(root.join("single"), "one\n").unwrap();
    let jail = Jail { change: vec![text(root.join("work")), text(root.join("single"))], ..Jail::default() };
    inside(
        jail,
        &root,
        "echo two >>work/a; echo new >work/b; mv work/b work/c; mkdir work/d; ln -s /bin/sh work/l; echo two >>single; echo x >other/x",
    );
    assert_eq!(fs::read_to_string(root.join("work/a")).unwrap(), "one\ntwo\n");
    assert_eq!(fs::read_to_string(root.join("work/c")).unwrap(), "new\n");
    assert!(!root.join("work/d").exists() && root.join("work/l").symlink_metadata().is_err(), "no folders, no links");
    assert_eq!(fs::read_to_string(root.join("single")).unwrap(), "one\ntwo\n", "a file named is changed as it is");
    assert!(!root.join("other/x").exists());
}

#[test]
fn files_leave_a_folder_for_the_bin_unchanged() {
    // Moving between folders needs Landlock ABI 2 (Linux 5.19).
    if linux::landlock_abi() < 2 {
        return;
    }
    let root = folders("bin", &["videos", "bin"]);
    for f in ["a", "b"] {
        fs::write(root.join("videos").join(f), "film\n").unwrap();
    }
    let jail = Jail { take_from: vec![text(root.join("videos"))], move_into: vec![text(root.join("bin"))], ..Jail::default() };
    inside(jail, &root, "mv videos/a bin/a; echo x >>bin/a; echo x >>videos/b; echo x >videos/new; mv videos/b videos/c; cp bin/a videos/back");
    assert_eq!(fs::read_to_string(root.join("bin/a")).unwrap(), "film\n", "moved, and not changed there");
    assert_eq!(fs::read_to_string(root.join("videos/b")).unwrap(), "film\n", "what stays isn't changed or renamed");
    assert_eq!(fs::read_dir(root.join("videos")).unwrap().count(), 1, "nothing new in the folder");
}

#[test]
fn nothing_outside_is_signalled() {
    // Signals stay inside from Landlock ABI 6 (Linux 6.12).
    if linux::landlock_abi() < 6 {
        return;
    }
    let root = folders("signal", &[]);
    let me = std::process::id();
    let said = inside(Jail::default(), &root, &format!("kill -0 {me} 2>/dev/null && echo reached; kill -0 $$ && echo itself"));
    assert_eq!(said, "itself\n");
}

#[test]
fn only_the_programs_named_start() {
    if linux::landlock_abi() < 1 {
        return;
    }
    let root = folders("exec", &[]);
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "/bin/true && echo started; echo done"]).current_dir(&root);
    let jail = Jail { exec: vec!["/bin/sh".to_owned()], ..Jail::default() };
    // SAFETY: as in `inside`.
    unsafe {
        cmd.pre_exec(move || linux::lock_privileges().and_then(|()| linux::jail(&jail)).map(drop));
    }
    assert_eq!(String::from_utf8_lossy(&cmd.output().unwrap().stdout), "done\n");
}
