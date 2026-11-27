//! # Runtime values (`value.rs`)
//!
//! **What this file is:** the `Value` type — everything a valo program can
//! compute at runtime. Every VM register holds one `Value`.
//!
//! **Pipeline position:** used by `vm/src/vm.rs` (registers, arithmetic) and
//! printed by `ECHO` / `print`. The compiler never creates `Value`s; it creates
//! constants (`chunk::Const`) that the VM turns into values.
//!
//! **Textbook chapter:** `docs/07-virtual-machine.md` (section *Values*).
//!
//! ## Where to change things
//!
//! | I want to…                         | Look at                                        |
//! |------------------------------------|------------------------------------------------|
//! | add a new runtime type             | add a variant to `enum Value`, then `type_name` and `Display` below (the compiler will point at every other `match` that must handle it) |
//! | change how a value is printed      | `impl fmt::Display for Value`                  |
//! | change type names in error messages| `Value::type_name`                             |
//!
//! ## Design (plan §2.4, D3)
//!
//! valo is functional and values are immutable, so heap-allocated values
//! (strings, lists — later) will be shared with `Rc` rather than mutated in
//! place. There is no nil: optional values use `none` (M7), and registers
//! start out as `Unit`. Variants arrive milestone by milestone:
//! `Bool`, `Float` (M3), `Closure`, `Native` (M4), `Str`, `None`, `Ok`, `Err`
//! (M7), `List` (M8), `Tuple`, `Dict` (M8b), `Missing` (M4b, internal).
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - an `enum` whose variants carry data (`Int(i64)`) (*Enums*).
//! - `match` must cover every variant — adding a variant makes the compiler
//!   list every place to update (*Pattern matching*).
//! - `impl fmt::Display` — makes `format!("{value}")` work (*Traits, impl and derive*).

use std::fmt;

/// A runtime value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// The unit value `()`.
    Unit,
    /// 64-bit signed integer; arithmetic is checked (D8).
    Int(i64),
}

impl Value {
    /// User-visible type name, as used in error messages (lowercase, S4).
    // RUST: `&self` borrows the value read-only; `&'static str` means the
    // returned text is a literal baked into the program.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "()",
            Value::Int(_) => "int",
        }
    }
}

// How a value looks when printed by `--echo` (and later by `print`).
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Unit => f.write_str("()"),
            // RUST: `{n}` inside the format string prints the variable `n`.
            Value::Int(n) => write!(f, "{n}"),
        }
    }
}
