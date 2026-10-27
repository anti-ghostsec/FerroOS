//! Runs the real ferro-jail: what's inside may run itself, not others.

use std::process::Command;

fn jail() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ferro-jail"))
}

#[test]
fn only_the_jailed_program_runs() {
    // The shell itself runs; a program it tries to start doesn't, unless
    // allowed. (Skipped on a kernel without Landlock.)
    let out = jail().args(["/bin/sh", "-c", "/bin/true && echo started"]).output().unwrap();
    let said = String::from_utf8_lossy(&out.stderr);
    if said.contains("Landlock is not enabled") {
        return;
    }
    assert!(!String::from_utf8_lossy(&out.stdout).contains("started"), "another program ran: {said}");
    let out = jail().args(["--exec", "/bin/true", "/bin/sh", "-c", "/bin/true && echo started"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("started"));
}

#[test]
fn missing_program_is_reported() {
    let out = jail().arg("no-such-program-anywhere").output().unwrap();
    assert_eq!(out.status.code(), Some(127));
}
