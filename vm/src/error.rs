//! # Runtime errors (`error.rs`)
//!
//! **What this file is:** the error type the VM returns when a program fails
//! while running (overflow, division by zero, …). A `RuntimeError` has a kind
//! (with a stable `R####` code), a message and a call-stack trace: function
//! name and source span for each active frame, innermost first.
//!
//! **Pipeline position:** created in `vm/src/vm.rs`; the trace is attached in
//! `Vm::unwind`; printed by `compiler/src/diagnostic.rs` (`render_runtime`)
//! against the source text. `valo-vm` (post-MVP) will print it from the
//! program's line table alone.
//!
//! **Textbook chapter:** `docs/08-errors-and-diagnostics.md`.
//!
//! ## Where to change things
//!
//! | I want to…                           | Look at                                                  |
//! |--------------------------------------|----------------------------------------------------------|
//! | add a new runtime error              | add a variant to `ErrorKind` + its code in `ErrorKind::code`; create it with `RuntimeError::new(kind, msg)` in `vm.rs`; add it to the catalogue in `.memory/plan-mvp.md` §5 |
//! | change how runtime errors are printed| `compiler/src/diagnostic.rs` → `render_runtime`         |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `#[derive(Error)]` from the `thiserror` crate plus `#[error("...")]` —
//!   generates the standard `Display` and `std::error::Error` impls (*Traits, impl and derive*).
//! - `impl Into<String>` parameter — accepts `&str` or `String` (*Strings and characters*).
//! - `Option<SrcSpan>` (*Option and Result*).

use std::rc::Rc;

use thiserror::Error;

use crate::chunk::SrcSpan;

/// Category of a runtime error; each maps to a stable code (plan §5).
/// Codes never change once a golden test uses them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// R0101 integer overflow.
    IntOverflow,
    /// R0102 division or modulo by zero.
    DivisionByZero,
    /// R0103 float → int conversion of NaN, an infinity or an out-of-range value.
    FloatToInt,
    /// R0104 negative integer exponent.
    NegativeExponent,
    /// R0201 operand type mismatch.
    TypeMismatch,
    /// R0202 calling a value that is not a function.
    NotCallable,
    /// R0203 a condition (`if`, `and`/`or` operand, `assert`) is not a bool.
    ConditionNotBool,
    /// R0205 values of this type have no order (`<` on bools), or cannot be
    /// compared at all (functions).
    NotComparable,
    /// R0206 a built-in function got an argument of the wrong type.
    NativeArgType,
    /// R0207 wrong number of arguments in a call the compiler could not check.
    WrongArgCount,
    /// R0208 `?` on a value that is not a result (`Ok`/`Err`), incl. `none`.
    NotResult,
    /// R0402 `unwrap` of `none`.
    UnwrapNone,
    /// R0403 `unwrap` of an `Err`.
    UnwrapErr,
    /// R0601 an `Err` reached the top level unhandled (`?` in `<main>`, or
    /// a top-level expression whose value is an `Err`, Q23).
    UnhandledErr,
    /// R0301 too many nested calls.
    StackOverflow,
    /// R0302 global read before its `SETGLOBAL` ran.
    GlobalUninit,
    /// R0701 `assert` failed.
    AssertionFailed,
    /// R0801 the run was stopped from outside (the debugger's Stop button).
    Interrupted,
    /// R0501 writing to stdout/stderr failed.
    HostIo,
    /// R0901 malformed bytecode or an opcode this build does not implement.
    Internal,
}

impl ErrorKind {
    /// Stable error code.
    pub fn code(self) -> &'static str {
        match self {
            ErrorKind::IntOverflow => "R0101",
            ErrorKind::DivisionByZero => "R0102",
            ErrorKind::FloatToInt => "R0103",
            ErrorKind::NegativeExponent => "R0104",
            ErrorKind::TypeMismatch => "R0201",
            ErrorKind::NotCallable => "R0202",
            ErrorKind::ConditionNotBool => "R0203",
            ErrorKind::NotComparable => "R0205",
            ErrorKind::NativeArgType => "R0206",
            ErrorKind::WrongArgCount => "R0207",
            ErrorKind::NotResult => "R0208",
            ErrorKind::UnwrapNone => "R0402",
            ErrorKind::UnwrapErr => "R0403",
            ErrorKind::UnhandledErr => "R0601",
            ErrorKind::StackOverflow => "R0301",
            ErrorKind::GlobalUninit => "R0302",
            ErrorKind::AssertionFailed => "R0701",
            ErrorKind::Interrupted => "R0801",
            ErrorKind::HostIo => "R0501",
            ErrorKind::Internal => "R0901",
        }
    }
}

/// One frame of a stack trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEntry {
    /// Function display name.
    pub fn_name: Rc<str>,
    /// Span of the instruction executing in this frame (for a caller: the
    /// call it is waiting on).
    pub span: SrcSpan,
    /// Frames this one replaced through tail calls (M5); printed as
    /// "(... N tail calls elided ...)".
    pub tail_calls: u32,
}

/// A place in the program: function name and the span of an instruction
/// (where an `Err` was made, or a `?` passed it on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// Function display name.
    pub fn_name: Rc<str>,
    /// Source span.
    pub span: SrcSpan,
}

/// The history of an unhandled `Err` (R0601, plan D26/Q21): its context
/// messages and where it was created and propagated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrTrail {
    /// `context(r, msg)` messages, innermost (first added) first.
    pub context: Vec<Rc<str>>,
    /// Where `Err(…)` (or a failing built-in) created it.
    pub origin: Site,
    /// Every `?` that passed it on, in order (innermost first).
    pub propagated: Vec<Site>,
}

/// A runtime fault.
// RUST: `thiserror` turns the `#[error(...)]` line into a `Display` impl, so
// `err.to_string()` gives e.g. "error[R0102]: division by zero: 7 // 0".
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("error[{}]: {message}", kind.code())]
pub struct RuntimeError {
    /// Category.
    pub kind: ErrorKind,
    /// Human-readable message, without the code.
    pub message: String,
    /// Extra lines printed under the location: the two sides of a failed
    /// `assert a == b` (`left: …`, `right: …`), or a `help: …` hint.
    pub notes: Vec<String>,
    /// Stack trace, innermost frame first.
    pub trace: Vec<TraceEntry>,
    /// For R0601: the `Err`'s context chain and sites (boxed: rare, and it
    /// would make every `Result<_, RuntimeError>` larger).
    pub trail: Option<Box<ErrTrail>>,
}

impl RuntimeError {
    /// New error without a trace (the VM fills it in while unwinding).
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> RuntimeError {
        RuntimeError {
            kind,
            message: message.into(),
            notes: Vec::new(),
            trace: Vec::new(),
            trail: None,
        }
    }

    /// Add a note line.
    pub fn note(mut self, line: impl Into<String>) -> RuntimeError {
        self.notes.push(line.into());
        self
    }

    /// Span of the innermost frame, if any.
    pub fn primary_span(&self) -> Option<SrcSpan> {
        // RUST: `first()` returns `Option<&TraceEntry>`; `map` transforms the
        // inside if present (`Some`) and leaves `None` alone.
        self.trace.first().map(|t| t.span)
    }
}
