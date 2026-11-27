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
//! | add a new runtime error              | add a variant to `ErrorKind` + its code in `ErrorKind::code`, its snippet label in `ErrorKind::label` and `ErrorKind::ALL`; write a constructor below (`RuntimeError::division_by_zero`, …); add it to the catalogue in `.memory/plan-mvp.md` §5 |
//! | change the wording of an error       | its constructor in the `impl RuntimeError` block "Constructors" (all messages live there, M9) |
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
use crate::value::{ErrVal, Value};

/// Category of a runtime error; each maps to a stable code (plan §5).
/// Codes never change once a golden test uses them. This enum is the
/// catalogue of `R####` codes; every one except R0801 (debugger only) and
/// R0901 (internal) has a golden test (`compiler/tests/codes.rs`).
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
    /// R0401 list index out of range (M8); later also a missing dict key.
    IndexOutOfRange,
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
    /// Every kind, in code order (for the catalogue test).
    pub const ALL: &'static [ErrorKind] = &[
        ErrorKind::IntOverflow,
        ErrorKind::DivisionByZero,
        ErrorKind::FloatToInt,
        ErrorKind::NegativeExponent,
        ErrorKind::TypeMismatch,
        ErrorKind::NotCallable,
        ErrorKind::ConditionNotBool,
        ErrorKind::NotComparable,
        ErrorKind::NativeArgType,
        ErrorKind::WrongArgCount,
        ErrorKind::NotResult,
        ErrorKind::StackOverflow,
        ErrorKind::GlobalUninit,
        ErrorKind::IndexOutOfRange,
        ErrorKind::UnwrapNone,
        ErrorKind::UnwrapErr,
        ErrorKind::HostIo,
        ErrorKind::UnhandledErr,
        ErrorKind::AssertionFailed,
        ErrorKind::Interrupted,
        ErrorKind::Internal,
    ];

    /// Short text under the source snippet of the failing instruction.
    pub fn label(self) -> &'static str {
        match self {
            ErrorKind::IntOverflow => "result does not fit in an int",
            ErrorKind::DivisionByZero => "divisor is zero",
            ErrorKind::FloatToInt => "no int has this value",
            ErrorKind::NegativeExponent => "negative exponent",
            ErrorKind::TypeMismatch => "wrong operand type",
            ErrorKind::NotCallable => "not a function",
            ErrorKind::ConditionNotBool => "not a bool",
            ErrorKind::NotComparable => "cannot be compared",
            ErrorKind::NativeArgType => "wrong argument type",
            ErrorKind::WrongArgCount => "wrong number of arguments",
            ErrorKind::NotResult => "not a result",
            ErrorKind::StackOverflow => "calls nested too deeply",
            ErrorKind::GlobalUninit => "read before its `let` ran",
            ErrorKind::IndexOutOfRange => "out of range",
            ErrorKind::UnwrapNone => "value is none",
            ErrorKind::UnwrapErr => "value is an Err",
            ErrorKind::HostIo => "input/output failed",
            ErrorKind::UnhandledErr => "Err not handled",
            ErrorKind::AssertionFailed => "this is false",
            ErrorKind::Interrupted => "stopped here",
            ErrorKind::Internal => "internal error",
        }
    }

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
            ErrorKind::IndexOutOfRange => "R0401",
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
    /// A built-in that was running a callback (`<native map>`, M8): it has
    /// no source position (`span` is empty).
    pub native: bool,
}

/// What an index error (R0401) tried to read, so a renderer that has the
/// source can name the access path: `rows[1][3]: index out of range (len 2)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    /// The index, as written in messages (`repr`).
    pub index: String,
    /// Length of the indexed value.
    pub len: usize,
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
    /// For R0401: the index and length (boxed for the same reason).
    pub access: Option<Box<Access>>,
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
            access: None,
        }
    }

    /// Add a note line.
    pub fn note(mut self, line: impl Into<String>) -> RuntimeError {
        self.notes.push(line.into());
        self
    }

    /// Add a `help: …` note.
    pub fn help(self, text: &str) -> RuntimeError {
        self.note(format!("help: {text}"))
    }

    /// Span of the innermost frame, if any.
    pub fn primary_span(&self) -> Option<SrcSpan> {
        // RUST: `first()` returns `Option<&TraceEntry>`; `map` transforms the
        // inside if present (`Some`) and leaves `None` alone.
        self.trace.first().map(|t| t.span)
    }
}

/// Constructors: every runtime error message is worded here (M9), so the
/// VM and the built-ins only say *what* failed. Values in messages use
/// `Value::describe` (`int (3)`, cut to 40 characters).
impl RuntimeError {
    /// R0101 `x op y` does not fit in an int.
    pub fn int_overflow(expr: &str) -> RuntimeError {
        RuntimeError::new(ErrorKind::IntOverflow, format!("integer overflow: {expr}"))
    }

    /// R0102 `x op 0` for `/`, `//` (division) or `%` (modulo).
    pub fn division_by_zero(sym: &str, x: &Value, y: &Value) -> RuntimeError {
        let what = if sym == "%" { "modulo" } else { "division" };
        RuntimeError::new(
            ErrorKind::DivisionByZero,
            format!("{what} by zero: {x} {sym} {y}"),
        )
    }

    /// R0103 `int(x)` of NaN, an infinity or a float outside the int range.
    pub fn float_to_int(x: f64) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::FloatToInt,
            format!("cannot convert {x:?} to int"),
        )
    }

    /// R0104 `x ** y` with an int `y < 0`.
    pub fn negative_exponent(x: i64, y: i64) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::NegativeExponent,
            format!("negative exponent: {x} ** {y}"),
        )
        .help("int powers need an exponent >= 0; for a float result use float(x) ** float(y)")
    }

    /// R0201 binary operator on operands it does not accept, with a hint
    /// for the common causes (int/float mixing, D10; text building).
    pub fn operand_types(sym: &str, b: &Value, c: &Value) -> RuntimeError {
        mismatch_hint(
            RuntimeError::new(
                ErrorKind::TypeMismatch,
                format!(
                    "unsupported operand types for `{sym}`: {} and {}",
                    b.describe(),
                    c.describe()
                ),
            ),
            b,
            c,
        )
    }

    /// R0201 `==`/`<` between values of different types.
    pub fn compare_types(b: &Value, c: &Value) -> RuntimeError {
        mismatch_hint(
            RuntimeError::new(
                ErrorKind::TypeMismatch,
                format!("cannot compare {} and {}", b.describe(), c.describe()),
            ),
            b,
            c,
        )
    }

    /// R0201 unary minus on a non-number.
    pub fn cannot_negate(v: &Value) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::TypeMismatch,
            format!("cannot negate {}", v.describe()),
        )
    }

    /// R0201 `not` on a non-bool.
    pub fn not_needs_bool(v: &Value) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::TypeMismatch,
            format!("`not` needs a bool, found {}", v.describe()),
        )
    }

    /// R0201 `xs[i]` with a non-int index.
    pub fn index_type(i: &Value) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::TypeMismatch,
            format!("list index must be an int, not {}", i.describe()),
        )
    }

    /// R0201 `v[i]` on a value that cannot be indexed.
    pub fn not_indexable(v: &Value) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::TypeMismatch,
            format!("cannot index {}", v.describe()),
        )
    }

    /// R0202 calling a value that is not a function.
    pub fn not_callable(v: &Value) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::NotCallable,
            format!("cannot call {}: it is not a function", v.describe()),
        )
    }

    /// R0203 `if`/`and`/`or` condition (`what` = None) or `assert`
    /// (`what` = Some("assert")) that is not a bool.
    pub fn condition_not_bool(what: Option<&str>, v: &Value) -> RuntimeError {
        let message = match what {
            Some(w) => format!("`{w}` needs a bool, found {}", v.describe()),
            None => format!("expected a bool, found {}", v.describe()),
        };
        RuntimeError::new(ErrorKind::ConditionNotBool, message)
    }

    /// R0205 `==` between two functions.
    pub fn functions_not_comparable() -> RuntimeError {
        RuntimeError::new(ErrorKind::NotComparable, "functions cannot be compared")
    }

    /// R0205 `<` on a type without an order (bool, `()`, list, …).
    pub fn not_ordered(v: &Value) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::NotComparable,
            format!("values of type {} cannot be ordered", v.type_name()),
        )
        .help("only int, float and str have an order")
    }

    /// R0206 a built-in got an argument of the wrong type.
    pub fn native_arg(native: &str, expected: &str, got: &Value) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::NativeArgType,
            format!("{native}() expects {expected}, got {}", got.describe()),
        )
    }

    /// R0206 element `i` of a list argument has the wrong type.
    pub fn native_elem(native: &str, expected: &str, i: usize, got: &Value) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::NativeArgType,
            format!(
                "{native}() expects {expected}, but element {i} is {}",
                got.describe()
            ),
        )
    }

    /// R0207 unless `required <= nargs <= arity` (checked by the caller).
    pub fn wrong_arg_count(
        name: &str,
        required: usize,
        arity: usize,
        nargs: usize,
    ) -> RuntimeError {
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let takes = if required == arity {
            format!("{arity} argument{}", plural(arity))
        } else {
            format!("{required} to {arity} arguments")
        };
        let given = if nargs == 1 { "was" } else { "were" };
        RuntimeError::new(
            ErrorKind::WrongArgCount,
            format!("`{name}` takes {takes} but {nargs} {given} given"),
        )
    }

    /// R0208 `?` on a value that is not `Ok`/`Err`.
    pub fn not_result(v: &Value) -> RuntimeError {
        let err = RuntimeError::new(
            ErrorKind::NotResult,
            format!("`?` needs a result (Ok or Err), got {}", v.describe()),
        );
        if matches!(v, Value::None) {
            err.help("`?` passes on errors of results; for an optional value use or_else(x, default) or compare with none")
        } else {
            err
        }
    }

    /// R0301 more than `max` frames.
    pub fn stack_overflow(max: usize) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::StackOverflow,
            format!("stack overflow: more than {max} nested calls"),
        )
        .help("make the recursive call the last thing the function does (a tail call), e.g. by passing an accumulator")
    }

    /// R0301 more than `max` nested callbacks through built-ins (D7).
    pub fn reentry_overflow(max: usize) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::StackOverflow,
            format!("stack overflow: more than {max} nested calls through built-ins such as map"),
        )
        .help("a callback that calls map/filter/reduce on deeper and deeper data recurses through the built-in; use a recursive fn instead")
    }

    /// R0302 a global read before its `SETGLOBAL` ran (D4).
    pub fn global_uninit(name: &str) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::GlobalUninit,
            format!("global `{name}` used before initialisation"),
        )
    }

    /// R0401 `xs[i]` outside `0..len` (the CLI names the access path from
    /// `access`).
    pub fn index_out_of_range(i: i64, len: usize) -> RuntimeError {
        let mut err = RuntimeError::new(
            ErrorKind::IndexOutOfRange,
            format!("index {i} out of range (len {len})"),
        );
        if i < 0 {
            err = err.help("negative indices are not supported; count from the start (len(xs) - 1 is the last element)");
        }
        err.access = Some(Box::new(Access {
            index: i.to_string(),
            len,
        }));
        err
    }

    /// R0402 `unwrap(none)`.
    pub fn unwrap_none() -> RuntimeError {
        RuntimeError::new(ErrorKind::UnwrapNone, "unwrap of none")
            .help("test for none first (`x == none`) or use or_else(x, default)")
    }

    /// R0403 `unwrap(Err(e))`.
    pub fn unwrap_err(payload: &Value) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::UnwrapErr,
            format!("unwrap of Err({})", crate::value::preview(&payload.repr())),
        )
        .help("handle the error with `?`, is_err(r) or or_else(r, default)")
    }

    /// R0501 writing program output failed.
    pub fn output_failed(e: &std::io::Error) -> RuntimeError {
        RuntimeError::new(ErrorKind::HostIo, format!("cannot write output: {e}"))
    }

    /// R0501 reading standard input failed.
    pub fn input_failed(reason: &str) -> RuntimeError {
        RuntimeError::new(
            ErrorKind::HostIo,
            format!("cannot read standard input: {reason}"),
        )
    }

    /// R0601 an `Err` reached the top level: the payload in the message,
    /// its history in `trail` (rendered by the CLI).
    pub fn unhandled(e: &ErrVal) -> RuntimeError {
        let mut err = RuntimeError::new(
            ErrorKind::UnhandledErr,
            format!("unhandled Err: {}", e.payload.repr()),
        );
        err.trail = Some(Box::new(ErrTrail {
            context: e.context.clone(),
            origin: e.origin.clone(),
            propagated: e.propagated.clone(),
        }));
        err
    }

    /// R0701 `assert` was false; `sides` = both sides of a top-level
    /// comparison. The CLI appends the asserted source text.
    pub fn assertion_failed(sides: Option<(&Value, &Value)>) -> RuntimeError {
        let err = RuntimeError::new(ErrorKind::AssertionFailed, "assertion failed");
        match sides {
            Some((l, r)) => err
                .note(format!("left: {}", l.repr()))
                .note(format!("right: {}", r.repr())),
            None => err,
        }
    }

    /// R0801 the debugger stopped the run.
    pub fn interrupted() -> RuntimeError {
        RuntimeError::new(ErrorKind::Interrupted, "program stopped by the debugger")
    }

    /// R0901 malformed bytecode.
    pub fn internal(msg: impl Into<String>) -> RuntimeError {
        RuntimeError::new(ErrorKind::Internal, msg)
    }
}

/// R0201 hints (D10): int/float never mix; text is built by interpolation.
fn mismatch_hint(err: RuntimeError, b: &Value, c: &Value) -> RuntimeError {
    match (b, c) {
        (Value::Int(_), Value::Float(_)) | (Value::Float(_), Value::Int(_)) => err.help(
            "int and float never mix implicitly (only `/` converts); make both operands the same type",
        ),
        (Value::Str(_), _) | (_, Value::Str(_)) => err.help(
            "to build text from other values, use interpolation: \"{a}{b}\" (or convert with str())",
        ),
        _ => err,
    }
}
