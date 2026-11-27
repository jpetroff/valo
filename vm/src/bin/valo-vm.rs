//! # `valo-vm` — standalone bytecode runner (placeholder)
//!
//! **What this file will be:** a small executable that loads a precompiled
//! `.vlc` file and runs it with the VM, without shipping the compiler (like
//! Lua's ability to run `luac` output). It needs bytecode serialisation,
//! planned for M11 (after the MVP). Until then it prints a message and exits
//! with code 2.
//!
//! **Built as:** `target/debug/valo-vm` (declared under `[[bin]]` in
//! `vm/Cargo.toml`).
//!
//! **Textbook chapter:** `docs/09-driver-and-cli.md`.

use std::process::ExitCode;

// RUST: `main` may return `ExitCode`; its value becomes the process exit status.
fn main() -> ExitCode {
    eprintln!(
        "valo-vm {}: not implemented: runs precompiled .vlc files",
        valo_vm::VERSION
    );
    ExitCode::from(2)
}
