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
    /// R0104 negative integer exponent.
    NegativeExponent,
    /// R0201 operand type mismatch.
    TypeMismatch,
    /// R0302 global read before its `SETGLOBAL` ran.
    GlobalUninit,
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
            ErrorKind::NegativeExponent => "R0104",
            ErrorKind::TypeMismatch => "R0201",
            ErrorKind::GlobalUninit => "R0302",
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
    /// Span of the instruction executing in this frame.
    pub span: SrcSpan,
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
    /// Stack trace, innermost frame first.
    pub trace: Vec<TraceEntry>,
}

impl RuntimeError {
    /// New error without a trace (the VM fills it in while unwinding).
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> RuntimeError {
        RuntimeError {
            kind,
            message: message.into(),
            trace: Vec::new(),
        }
    }

    /// Span of the innermost frame, if any.
    pub fn primary_span(&self) -> Option<SrcSpan> {
        // RUST: `first()` returns `Option<&TraceEntry>`; `map` transforms the
        // inside if present (`Some`) and leaves `None` alone.
        self.trace.first().map(|t| t.span)
    }
}
