//! # `valo_vm` — the valo runtime and bytecode format
//!
//! This crate owns everything needed to *execute* valo programs. It
//! deliberately does not depend on the compiler, so a VM-only build (for
//! example for WASM, or for running precompiled `.vlc` files) stays small, in
//! the same way Lua can run `luac` output without the parser.
//!
//! ## Map of the crate
//!
//! | File            | Contents                                                        | Chapter in `docs/` |
//! |-----------------|-----------------------------------------------------------------|--------------------|
//! | [`opcode`]      | instruction set (`OpCode`) and 32-bit encoding (`Instr`)         | 05-bytecode-format |
//! | [`chunk`]       | compiled functions (`Proto`), whole programs (`Program`), constants | 05-bytecode-format |
//! | [`disasm`]      | bytecode → readable text (`--dump bytecode`)                     | 05-bytecode-format |
//! | [`value`]       | runtime values (`Value`)                                         | 07-virtual-machine |
//! | [`vm`]          | the interpreter loop (`Vm`), arithmetic semantics                | 07-virtual-machine |
//! | [`host`]        | the door to the outside world (`Host`, `StdHost`, `TestHost`)    | 07-virtual-machine |
//! | [`error`]       | runtime errors (`RuntimeError`, `ErrorKind`, `R####` codes)      | 08-errors-and-diagnostics |
//! | [`builtins`]    | native functions (`print`, `int`, `float`)                       | 07-virtual-machine |
//! | `bin/valo-vm.rs`| standalone bytecode runner (placeholder until M11)               | 09-driver-and-cli |
//!
//! ## Comment conventions used in every source file
//!
//! - The `//!` block at the top of a file is its **guide**: what it does,
//!   where it sits in the pipeline, a "where to change things" table, and the
//!   Rust features it uses (explained in `docs/01-rust-primer.md`).
//! - `/// ...` documents the item below it (shown by `cargo doc --open`).
//! - `// RUST: ...` explains a Rust language feature at the point it is used.
//! - `// EXTEND: ...` marks the place to edit when adding a feature.
//!   Search for it: `grep -rn "EXTEND" vm compiler`.

// RUST: `pub mod x;` declares a module whose code lives in `src/x.rs` and
// makes it visible to other crates (the compiler uses `valo_vm::chunk`, ...).
pub mod builtins;
pub mod chunk;
pub mod disasm;
pub mod error;
pub mod host;
pub mod opcode;
pub mod value;
pub mod vm;

/// Version of this crate, as declared in `Cargo.toml`.
// RUST: `env!` reads an environment variable at *compile* time; Cargo sets
// `CARGO_PKG_VERSION` from the `version` field of Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
