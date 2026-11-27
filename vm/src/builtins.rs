//! # Native functions (`builtins.rs`) — empty until M4
//!
//! **What this file will be:** the table of built-in functions implemented
//! in Rust and callable from valo (`print`, `len`, `map`, `read_file`, …).
//! Each entry will be a `NativeDef { name, params, effect, f }` (plan D14).
//! The compiler will pre-declare their names as globals, and `CALL` will run
//! them like any other function. Natives that touch the outside world
//! (`effect: true`) go through `host::Host`, which is what makes
//! `--dry-run` / `--replay` possible later.
//!
//! **Milestones:** M4 (`print`, conversions), M7 (strings, files, stdin),
//! M8 (`map`, `filter`, `reduce`, `range`, …). See `.memory/plan-mvp.md`.
//!
//! **Textbook chapter:** `docs/11-roadmap.md`.
