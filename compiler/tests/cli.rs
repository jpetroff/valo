//! # CLI tests (`tests/cli.rs`)
//!
//! Command-line behaviour that golden files cannot express (plan M0): exit
//! code 64 for unreadable files and bad arguments, `--help` / `--version`,
//! and `check --dump bytecode`. Each test starts the real `valo` binary;
//! `env!("CARGO_BIN_EXE_valo")` is its path, provided by Cargo.
//!
//! **Textbook chapter:** `docs/10-testing.md`.

use std::process::Command;

fn valo() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_valo"));
    c.env("NO_COLOR", "1");
    c
}

#[test]
fn unreadable_file_is_a_usage_error() {
    let out = valo()
        .args(["run", "definitely-missing.vl"])
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("error: cannot read"));
}

#[test]
fn bad_arguments_exit_64_help_exits_0() {
    assert_eq!(
        valo().arg("run").output().expect("spawn").status.code(),
        Some(64)
    );
    assert_eq!(
        valo().arg("--help").output().expect("spawn").status.code(),
        Some(0)
    );
    let v = valo().arg("--version").output().expect("spawn");
    assert_eq!(v.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&v.stdout).contains(valo_compiler::VERSION));
}

#[test]
fn check_dumps_bytecode() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("d.vl"), "1 + 2\n").expect("write");
    let out = valo()
        .args(["check", "d.vl", "--dump", "bytecode"])
        .current_dir(&dir)
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("function <main> arity=0"), "{text}");
    assert!(text.contains("ADD       0 0 1"), "{text}");
}

/// R0501: program output that cannot be written (stdout is `/dev/full`).
#[cfg(target_os = "linux")]
#[test]
fn write_failure_is_r0501() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("p.vl"), "print(\"hello\")\n").expect("write");
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full");
    let out = valo()
        .args(["run", "p.vl"])
        .current_dir(&dir)
        .stdout(full)
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.starts_with("error[R0501]: cannot write output"),
        "{err}"
    );
}

/// `--color always` colours the header even when stderr is not a terminal.
#[test]
fn color_flag() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("c.vl"), "print(nope)\n").expect("write");
    for (flag, colored) in [("always", true), ("never", false), ("auto", false)] {
        let out = valo()
            .args(["check", "c.vl", "--color", flag])
            .current_dir(&dir)
            .output()
            .expect("spawn");
        assert_eq!(out.status.code(), Some(2));
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(err.starts_with("\x1b["), colored, "--color {flag}: {err:?}");
    }
}
