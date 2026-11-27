//! # Native functions (`builtins.rs`)
//!
//! **What this file is:** the table of built-in functions implemented in
//! Rust and callable from valo (`print`, `int`, `float`; later `len`, `map`,
//! `read_file`, …). Each entry is a [`NativeDef`] (plan D14).
//!
//! **How they are wired in:** the compiler seeds one global slot per entry
//! (slots `0..NATIVES.len()`, in table order) and resolves the names like any
//! other global, so user code may shadow them. Before `<main>` runs, the VM
//! fills those slots with `Value::Native(id)`. `CALL` on such a value checks
//! the argument count and calls `f` with the arguments moved off the stack.
//! Natives that touch the outside world (`effect: true`) go through
//! `host::Host`, which is what makes `--dry-run` / `--replay` possible later.
//!
//! **Milestones:** M4 (`print`, `int`, `float`), M7 (strings, files, stdin),
//! M8 (`map`, `filter`, `reduce`, `range`, …). See `.memory/plan-mvp.md`.
//!
//! **Textbook chapter:** `docs/07-virtual-machine.md` (section *Calls*).
//!
//! ## Where to change things
//!
//! | I want to…                    | Look at                                                     |
//! |-------------------------------|-------------------------------------------------------------|
//! | add a built-in                | write `fn native_x(vm, args)`, append a `NativeDef` to `NATIVES` (append only: ids are table positions) |
//! | give a built-in a keyword parameter with a default | `ParamSpec { has_default: true }`; the function then receives `Value::Missing` when it is omitted (M4b) |
//! | report a wrong argument type  | `arg_type_error` (R0206)                                    |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - function pointers `fn(&mut Vm<'_>, Vec<Value>) -> …` stored in a struct (*Closures and iterators*).
//! - a `static` table (`pub static NATIVES: &[NativeDef]`) (*Structs*).
//! - `f64::trunc`, range checks before an `as i64` cast (*Integer types and casts*).

use crate::error::{ErrorKind, RuntimeError};
use crate::value::Value;
use crate::vm::Vm;

/// One parameter of a built-in function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamSpec {
    /// Name, for keyword arguments and messages.
    pub name: &'static str,
    /// Keyword-only parameter with a default (S19). Receives
    /// `Value::Missing` when the caller omits it (M4b).
    pub has_default: bool,
}

/// Signature of a native implementation: the VM (for the host) and the
/// arguments, owned, in parameter order.
pub type NativeFn = fn(&mut Vm<'_>, Vec<Value>) -> Result<Value, RuntimeError>;

/// A built-in function.
#[derive(Debug, Clone, Copy)]
pub struct NativeDef {
    /// Name in the prelude.
    pub name: &'static str,
    /// Parameters (required ones first).
    pub params: &'static [ParamSpec],
    /// Performs I/O through the host (S22 effect seed; not checked yet).
    pub effect: bool,
    /// Implementation.
    pub f: NativeFn,
}

impl NativeDef {
    /// Number of parameters without a default.
    pub fn required(&self) -> usize {
        self.params.iter().filter(|p| !p.has_default).count()
    }
}

const fn req(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        has_default: false,
    }
}

/// Every built-in, indexed by `NativeId`. Append only.
pub static NATIVES: &[NativeDef] = &[
    NativeDef {
        name: "print",
        params: &[req("x")],
        effect: true,
        f: native_print,
    },
    NativeDef {
        name: "int",
        params: &[req("x")],
        effect: false,
        f: native_int,
    },
    NativeDef {
        name: "float",
        params: &[req("x")],
        effect: false,
        f: native_float,
    },
];

/// R0206 for a native that got an argument of the wrong type.
fn arg_type_error(native: &str, expected: &str, got: &Value) -> RuntimeError {
    RuntimeError::new(
        ErrorKind::NativeArgType,
        format!(
            "{native}() expects {expected}, got {} ({got})",
            got.type_name()
        ),
    )
}

/// `print(x)`: write `x` (Display form) and a newline; returns `()`.
fn native_print(vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let line = format!("{}\n", args[0]);
    vm.write_stdout(&line)?;
    Ok(Value::Unit)
}

/// `int(x)`: float → int, rounding toward zero (R0103 for NaN, infinities and
/// values outside the int range); an int is returned unchanged.
fn native_int(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(x) => {
            let t = x.trunc();
            // 2^63 is exactly representable; every float in [-2^63, 2^63)
            // fits in i64. NaN fails both comparisons.
            const LIMIT: f64 = 9_223_372_036_854_775_808.0;
            if (-LIMIT..LIMIT).contains(&t) {
                Ok(Value::Int(t as i64))
            } else {
                Err(RuntimeError::new(
                    ErrorKind::FloatToInt,
                    format!("cannot convert {x:?} to int"),
                ))
            }
        }
        other => Err(arg_type_error("int", "a float", other)),
    }
}

/// `float(x)`: int → float (may round above 2^53); a float is returned
/// unchanged.
fn native_float(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    match &args[0] {
        Value::Int(n) => Ok(Value::Float(*n as f64)),
        Value::Float(x) => Ok(Value::Float(*x)),
        other => Err(arg_type_error("float", "an int", other)),
    }
}
