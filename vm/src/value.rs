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
//! | change what a function value holds | `struct Closure`                               |
//!
//! ## Design (plan §2.4, D3)
//!
//! valo is functional and values are immutable, so heap-allocated values
//! (strings, lists — later) will be shared with `Rc` rather than mutated in
//! place. There is no nil: optional values use `none` (M7), and registers
//! start out as `Unit`. Variants arrive milestone by milestone:
//! `Closure`, `Native` (M4), `Str`, `None`, `Ok`, `Err` (M7), `List` (M8),
//! `Tuple`, `Dict` (M8b), `Missing` (M4b, internal).
//!
//! ## No reference cycles (invariant)
//!
//! Values are reference counted (`Rc`) and there is no garbage collector, so
//! a cycle of `Rc`s would leak. None can form: values are immutable, and a
//! closure's captures are copied when it is created (plan D5), before the
//! closure itself exists. A function refers to itself only through a global
//! slot (top-level `fn`, M4) or `GETSELF` (local `fn`, M6), never through its
//! own captures.
//!
//! The derived `PartialEq` is for tests only. The language's `==` has its own
//! rules (no int/float mixing, NaN ≠ NaN) and lives in `vm.rs`
//! (`values_equal`).
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - an `enum` whose variants carry data (`Int(i64)`) (*Enums*).
//! - `match` must cover every variant — adding a variant makes the compiler
//!   list every place to update (*Pattern matching*).
//! - `impl fmt::Display` — makes `format!("{value}")` work (*Traits, impl and derive*).

use std::fmt;
use std::rc::Rc;

use crate::builtins::NATIVES;
use crate::chunk::{NativeId, Proto};

/// A runtime value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// The unit value `()`.
    Unit,
    /// `true` / `false`.
    Bool(bool),
    /// 64-bit signed integer; arithmetic is checked (D8).
    Int(i64),
    /// 64-bit IEEE float; arithmetic follows IEEE (overflow gives `inf`),
    /// except that division by zero is an error (Q20).
    Float(f64),
    /// A function defined in valo (M4).
    Closure(Rc<Closure>),
    /// A built-in function: index into `builtins::NATIVES` (M4).
    Native(NativeId),
    /// Internal sentinel, never a language value: an argument the caller
    /// omitted (a defaulted parameter, M4b). `CALL` puts it into the
    /// parameter's register and the callee's prologue (`JMPIFSET`) replaces
    /// it with the default before any other code runs; natives replace it
    /// themselves. It never escapes into user code.
    Missing,
}

/// A function value: compiled code plus the values it captured (plan D5).
#[derive(Debug, Clone, PartialEq)]
pub struct Closure {
    /// The compiled function.
    pub proto: Rc<Proto>,
    /// Captured values (always empty until closures arrive in M6).
    pub captures: Box<[Value]>,
}

impl Closure {
    /// A closure with no captures.
    pub fn new(proto: Rc<Proto>) -> Closure {
        Closure {
            proto,
            captures: Box::new([]),
        }
    }
}

impl Value {
    /// User-visible type name, as used in error messages (lowercase, S4).
    // RUST: `&self` borrows the value read-only; `&'static str` means the
    // returned text is a literal baked into the program.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "()",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Closure(_) | Value::Native(_) => "function",
            Value::Missing => "<missing>",
        }
    }
}

// How a value looks when printed by `--echo` (and later by `print`).
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Unit => f.write_str("()"),
            Value::Bool(b) => write!(f, "{b}"),
            // RUST: `{n}` inside the format string prints the variable `n`.
            Value::Int(n) => write!(f, "{n}"),
            // `{:?}` always shows a decimal point or exponent (`1.0`, `1e100`)
            // and spells the specials `inf`, `-inf`, `NaN` (plan §2.4).
            Value::Float(x) => write!(f, "{x:?}"),
            // `<fn fact/1>`: name and number of parameters.
            Value::Closure(c) => write!(f, "<fn {}/{}>", c.proto.name, c.proto.arity),
            Value::Native(id) => match NATIVES.get(id.0 as usize) {
                Some(n) => write!(f, "<native {}/{}>", n.name, n.params.len()),
                None => f.write_str("<native ?>"),
            },
            // Only visible in `--trace-exec` output.
            Value::Missing => f.write_str("<missing>"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        let shown: Vec<String> = [
            Value::Unit,
            Value::Bool(true),
            Value::Int(-3),
            Value::Float(1.0),
            Value::Float(0.1 + 0.2),
            Value::Float(1e100),
            Value::Float(f64::INFINITY),
            Value::Float(f64::NAN),
            Value::Float(-0.0),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect();
        assert_eq!(
            shown,
            [
                "()",
                "true",
                "-3",
                "1.0",
                "0.30000000000000004",
                "1e100",
                "inf",
                "NaN",
                "-0.0"
            ]
        );
    }
}
