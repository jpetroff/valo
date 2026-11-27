//! # Error-code coverage (`tests/codes.rs`)
//!
//! Every error code in the catalogues — `diagnostic::E_CODES` (compile
//! time) and `ErrorKind::ALL` (run time) — must appear in the expected
//! stderr of at least one golden test (plan M9 exit criterion). Exceptions:
//! retired codes, R0801 (only the debugger stops a run), R0901 (internal
//! VM error, unreachable from valid bytecode) and R0501 (output failure,
//! tested in `cli.rs` by writing to `/dev/full`).
//!
//! **Textbook chapter:** `docs/10-testing.md`.

use std::fs;
use std::path::{Path, PathBuf};

use valo_compiler::diagnostic::{E_CODES, E_RETIRED};
use valo_vm::error::ErrorKind;

/// Codes tested elsewhere or not reachable from a program.
const NOT_IN_GOLDENS: &[&str] = &["R0501", "R0801", "R0901"];

/// Codes whose goldens arrive with the milestone that produces them.
// M10: E0401 literal annotation mismatch, R0204 annotation mismatch at run time.
const PENDING: &[&str] = &["E0401", "R0204"];

fn stderr_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read golden dir").flatten() {
        let p = entry.path();
        if p.is_dir() {
            stderr_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "stderr") {
            out.push(p);
        }
    }
}

#[test]
fn every_error_code_has_a_golden() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut files = Vec::new();
    stderr_files(&root, &mut files);
    let all: String = files
        .iter()
        .map(|f| fs::read_to_string(f).expect("read stderr"))
        .collect();
    let codes = E_CODES
        .iter()
        .map(|(c, _)| *c)
        .chain(ErrorKind::ALL.iter().map(|k| k.code()));
    let missing: Vec<&str> = codes
        .filter(|c| !E_RETIRED.contains(c) && !NOT_IN_GOLDENS.contains(c) && !PENDING.contains(c))
        .filter(|c| !all.contains(&format!("error[{c}]")))
        .collect();
    assert!(
        missing.is_empty(),
        "codes without a golden test: {missing:?}"
    );
}

#[test]
fn codes_are_unique_and_sorted() {
    let e: Vec<&str> = E_CODES.iter().map(|(c, _)| *c).collect();
    let r: Vec<&str> = ErrorKind::ALL.iter().map(|k| k.code()).collect();
    for list in [e, r] {
        let mut sorted = list.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(list, sorted);
    }
}
