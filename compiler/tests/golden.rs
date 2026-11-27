//! # Golden tests (`tests/golden.rs`)
//!
//! **What this file is:** an end-to-end test runner. Every
//! `tests/golden/**/<name>.vl` program is run with the real `valo run` binary
//! and its stdout, stderr and exit code are compared with files next to it:
//!
//! | file              | meaning                                              |
//! |-------------------|------------------------------------------------------|
//! | `<name>.vl`       | the program                                          |
//! | `<name>.stdout`   | expected standard output (required, may be empty)   |
//! | `<name>.stderr`   | expected standard error (absent = must be empty)    |
//! | `<name>.exit`     | expected exit code (absent = 0)                      |
//! | `<name>.args`     | extra `run` flags, e.g. `--echo` (optional)          |
//! | `<name>.stdin`    | standard input to feed (optional)                    |
//!
//! **How to add a test:** create `tests/golden/mN/<name>.vl` and a
//! `<name>.stdout` with the output you *expect* (write it by hand first), then
//! run `cargo test -p valo-compiler --test golden`.
//!
//! - `VALO_BLESS=1` rewrites the expectation files from the actual output.
//!   Review the diff before committing.
//! - `VALO_GOLDEN=<substring>` runs only tests whose path contains it.
//!
//! Each program runs in a fresh directory holding only a copy of the `.vl`
//! file, so paths in messages are stable (`name.vl:1:1`).
//!
//! **Textbook chapter:** `docs/10-testing.md`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "vl") {
            out.push(p);
        }
    }
}

fn read_opt(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok()
}

struct Outcome {
    stdout: String,
    stderr: String,
    exit: i32,
}

fn run_case(vl: &Path, scratch: &Path) -> Outcome {
    let name = vl.file_name().expect("file name");
    let rel = vl.strip_prefix(golden_root()).expect("under golden root");
    let dir = scratch.join(rel.with_extension(""));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    fs::copy(vl, dir.join(name)).expect("copy source");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_valo"));
    cmd.arg("run")
        .arg(name)
        .current_dir(&dir)
        .env("NO_COLOR", "1");
    if let Some(args) = read_opt(&vl.with_extension("args")) {
        cmd.args(args.split_whitespace());
    }
    let stdin = read_opt(&vl.with_extension("stdin"));
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn valo");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    let out = child.wait_with_output().expect("wait for valo");
    Outcome {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        exit: out.status.code().unwrap_or(-1),
    }
}

fn golden_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

#[test]
fn golden() {
    let mut cases = Vec::new();
    collect(&golden_root(), &mut cases);
    cases.sort();
    let filter = std::env::var("VALO_GOLDEN").ok();
    let bless = std::env::var_os("VALO_BLESS").is_some();
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("golden");

    let mut failures = Vec::new();
    let mut ran = 0;
    for vl in &cases {
        let shown = vl
            .strip_prefix(golden_root())
            .unwrap_or(vl)
            .display()
            .to_string();
        if filter.as_deref().is_some_and(|f| !shown.contains(f)) {
            continue;
        }
        ran += 1;
        let got = run_case(vl, &scratch);
        let stdout_path = vl.with_extension("stdout");
        let stderr_path = vl.with_extension("stderr");
        let exit_path = vl.with_extension("exit");

        if bless {
            fs::write(&stdout_path, &got.stdout).expect("write .stdout");
            if got.stderr.is_empty() {
                let _ = fs::remove_file(&stderr_path);
            } else {
                fs::write(&stderr_path, &got.stderr).expect("write .stderr");
            }
            if got.exit == 0 {
                let _ = fs::remove_file(&exit_path);
            } else {
                fs::write(&exit_path, format!("{}\n", got.exit)).expect("write .exit");
            }
            continue;
        }

        let Some(want_stdout) = read_opt(&stdout_path) else {
            failures.push(format!("{shown}: missing {}", stdout_path.display()));
            continue;
        };
        let want_stderr = read_opt(&stderr_path).unwrap_or_default();
        let want_exit: i32 =
            read_opt(&exit_path).map_or(0, |s| s.trim().parse().expect("exit code"));

        let mut problems = Vec::new();
        if got.stdout != want_stdout {
            problems.push(format!(
                "stdout differs:\n{}",
                pretty_assertions::StrComparison::new(&want_stdout, &got.stdout)
            ));
        }
        if got.stderr != want_stderr {
            problems.push(format!(
                "stderr differs:\n{}",
                pretty_assertions::StrComparison::new(&want_stderr, &got.stderr)
            ));
        }
        if got.exit != want_exit {
            problems.push(format!("exit code: want {want_exit}, got {}", got.exit));
        }
        if !problems.is_empty() {
            failures.push(format!("{shown}:\n{}", problems.join("\n")));
        }
    }

    assert!(
        ran > 0 || filter.is_some(),
        "no golden tests found under {}",
        golden_root().display()
    );
    assert!(
        failures.is_empty(),
        "{} of {ran} golden tests failed (VALO_BLESS=1 to accept after review):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
