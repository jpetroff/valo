//! # Smoke test (`vm/tests/smoke.rs`)
//!
//! The crate links and exposes its version. Real VM tests live next to the
//! code they test (`#[cfg(test)] mod tests` in `vm/src/*.rs`); see
//! `docs/10-testing.md`.

#[test]
fn version_matches_manifest() {
    assert_eq!(valo_vm::VERSION, env!("CARGO_PKG_VERSION"));
}
