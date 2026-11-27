//! # Smoke test (`tests/smoke.rs`)
//!
//! The crate links, as does its `valo-vm` dependency. Files in `tests/` are
//! *integration tests*: each is compiled as a separate program that can only
//! use the crate's public API (see `docs/10-testing.md`).

#[test]
fn version_matches_manifest() {
    assert_eq!(valo_compiler::VERSION, env!("CARGO_PKG_VERSION"));
    assert!(!valo_vm::VERSION.is_empty());
}
