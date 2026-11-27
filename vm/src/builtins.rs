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
//! **Milestones:** M4 (`print`, `int`, `float`), M7a (`str`, `str_len`,
//! `trim`, `print(end=…)`), M7b (`none`/Result helpers, files, stdin),
//! M8 (`len`, `get`, `range`, `map`, `filter`, `reduce`, `flat_map`, `each`,
//! `contains`, `append`, `reverse`, `lines`, `split`, `join`). See
//! `.memory/plan-mvp.md`.
//!
//! **Callbacks (M8, plan D7):** `map`, `filter`, … call valo functions with
//! `Vm::call_value`, which runs them in a nested dispatch loop. A list
//! argument that nothing else holds is consumed in place (`Rc::try_unwrap`,
//! `Rc::make_mut`), so `append` on a fresh list does not copy it.
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
//! | call a valo function from a built-in | `vm.call_value(f, args)` (see `native_map`)          |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - function pointers `fn(&mut Vm<'_>, Vec<Value>) -> …` stored in a struct (*Closures and iterators*).
//! - a `static` table (`pub static NATIVES: &[NativeDef]`) (*Structs*).
//! - `f64::trunc`, range checks before an `as i64` cast (*Integer types and casts*).

use std::rc::Rc;

use crate::chunk::NativeId;
use crate::error::{ErrorKind, RuntimeError};
use crate::value::{ErrVal, Value, quote};
use crate::vm::{Vm, values_equal};

/// One parameter of a built-in function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamSpec {
    /// Name, for keyword arguments and messages.
    pub name: &'static str,
    /// May be omitted (S19). Receives `Value::Missing` when the caller
    /// omits it (M4b).
    pub has_default: bool,
    /// An omittable parameter that is passed by position, not by name:
    /// only for the positional overloads of a few built-ins (`range(n)` /
    /// `range(a, b)`, Q26). User fns cannot declare these.
    pub positional: bool,
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
        positional: false,
    }
}

/// A keyword-only parameter with a default (S19).
const fn opt(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        has_default: true,
        positional: false,
    }
}

/// A trailing positional parameter that may be omitted (Q26).
const fn pos_opt(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        has_default: true,
        positional: true,
    }
}

/// The id of the built-in called `name` (desugarings refer to `map`,
/// `contains`, … by id, so user code that shadows the name cannot break
/// them, plan D14).
pub fn native_id(name: &str) -> Option<NativeId> {
    NATIVES
        .iter()
        .position(|n| n.name == name)
        .map(|i| NativeId(i as u16))
}

/// Every built-in, indexed by `NativeId`. Append only.
pub static NATIVES: &[NativeDef] = &[
    NativeDef {
        name: "print",
        params: &[req("x"), opt("end")],
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
    // ---- M7a: text ----
    NativeDef {
        name: "str",
        params: &[req("x")],
        effect: false,
        f: native_str,
    },
    NativeDef {
        name: "str_len",
        params: &[req("s")],
        effect: false,
        f: native_str_len,
    },
    NativeDef {
        name: "trim",
        params: &[req("s")],
        effect: false,
        f: native_trim,
    },
    // ---- M7b: none, results, input, files ----
    NativeDef {
        name: "Ok",
        params: &[req("value")],
        effect: false,
        f: native_ok,
    },
    NativeDef {
        name: "Err",
        params: &[req("error")],
        effect: false,
        f: native_err,
    },
    NativeDef {
        name: "context",
        params: &[req("r"), req("msg")],
        effect: false,
        f: native_context,
    },
    NativeDef {
        name: "is_ok",
        params: &[req("r")],
        effect: false,
        f: native_is_ok,
    },
    NativeDef {
        name: "is_err",
        params: &[req("r")],
        effect: false,
        f: native_is_err,
    },
    NativeDef {
        name: "unwrap",
        params: &[req("x")],
        effect: false,
        f: native_unwrap,
    },
    NativeDef {
        name: "or_else",
        params: &[req("x"), req("default")],
        effect: false,
        f: native_or_else,
    },
    NativeDef {
        name: "parse_int",
        params: &[req("s")],
        effect: false,
        f: native_parse_int,
    },
    NativeDef {
        name: "parse_float",
        params: &[req("s")],
        effect: false,
        f: native_parse_float,
    },
    NativeDef {
        name: "read_line",
        params: &[],
        effect: true,
        f: native_read_line,
    },
    NativeDef {
        name: "read_file",
        params: &[req("path")],
        effect: true,
        f: native_read_file,
    },
    NativeDef {
        name: "write_file",
        params: &[req("path"), req("content")],
        effect: true,
        f: native_write_file,
    },
    // ---- M8: lists and higher-order functions (data first, D21) ----
    NativeDef {
        name: "len",
        params: &[req("x")],
        effect: false,
        f: native_len,
    },
    NativeDef {
        name: "get",
        params: &[req("xs"), req("i")],
        effect: false,
        f: native_get,
    },
    NativeDef {
        name: "range",
        params: &[req("a"), pos_opt("b")],
        effect: false,
        f: native_range,
    },
    NativeDef {
        name: "map",
        params: &[req("xs"), req("f")],
        effect: false,
        f: native_map,
    },
    NativeDef {
        name: "filter",
        params: &[req("xs"), req("p")],
        effect: false,
        f: native_filter,
    },
    NativeDef {
        name: "reduce",
        params: &[req("xs"), req("init"), req("f")],
        effect: false,
        f: native_reduce,
    },
    NativeDef {
        name: "flat_map",
        params: &[req("xs"), req("f")],
        effect: false,
        f: native_flat_map,
    },
    NativeDef {
        name: "each",
        params: &[req("xs"), req("f")],
        effect: false,
        f: native_each,
    },
    NativeDef {
        name: "contains",
        params: &[req("xs"), req("x")],
        effect: false,
        f: native_contains,
    },
    NativeDef {
        name: "append",
        params: &[req("xs"), req("x")],
        effect: false,
        f: native_append,
    },
    NativeDef {
        name: "reverse",
        params: &[req("xs")],
        effect: false,
        f: native_reverse,
    },
    NativeDef {
        name: "lines",
        params: &[req("s")],
        effect: false,
        f: native_lines,
    },
    NativeDef {
        name: "split",
        params: &[req("s"), req("sep")],
        effect: false,
        f: native_split,
    },
    NativeDef {
        name: "join",
        params: &[req("xs"), req("sep")],
        effect: false,
        f: native_join,
    },
];

/// R0206 for a native that got an argument of the wrong type.
fn arg_type_error(native: &str, expected: &str, got: &Value) -> RuntimeError {
    RuntimeError::new(
        ErrorKind::NativeArgType,
        format!("{native}() expects {expected}, got {}", got.describe()),
    )
}

/// R0206 unless `v` is a string.
fn str_arg<'v>(native: &str, v: &'v Value) -> Result<&'v str, RuntimeError> {
    match v {
        Value::Str(s) => Ok(s),
        other => Err(arg_type_error(native, "a str", other)),
    }
}

/// `print(x, end="\n")`: write `x` (Display form: strings without quotes)
/// followed by `end`; returns `()`.
fn native_print(vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let end = match &args[1] {
        Value::Missing => "\n",
        other => str_arg("print", other)?,
    };
    let line = format!("{}{end}", args[0]);
    vm.write_stdout(&line)?;
    Ok(Value::Unit)
}

/// `str(x)`: the text `print` would show (a string is returned unchanged).
fn native_str(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    match &args[0] {
        Value::Str(s) => Ok(Value::Str(Rc::clone(s))),
        other => Ok(Value::Str(other.to_string().into())),
    }
}

/// `str_len(s)`: number of Unicode scalar values (not bytes): `str_len("héllo")`
/// is 5.
fn native_str_len(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let s = str_arg("str_len", &args[0])?;
    Ok(Value::Int(s.chars().count() as i64))
}

/// `trim(s)`: `s` without leading and trailing whitespace.
fn native_trim(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let s = str_arg("trim", &args[0])?;
    Ok(Value::Str(s.trim().into()))
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

// ---- M7b: none, results, input, files -------------------------------------

/// A new `Err(payload)` created at the running code's current position.
fn new_err(vm: &Vm<'_>, payload: Value) -> Value {
    Value::Err(Rc::new(ErrVal {
        payload,
        context: Vec::new(),
        origin: vm.current_site(),
        propagated: Vec::new(),
    }))
}

/// `Ok(v)`.
fn native_ok(_vm: &mut Vm<'_>, mut args: Vec<Value>) -> Result<Value, RuntimeError> {
    Ok(Value::Ok(Rc::new(args.swap_remove(0))))
}

/// `Err(e)`: remembers where it was called (the origin in R0601 output).
fn native_err(vm: &mut Vm<'_>, mut args: Vec<Value>) -> Result<Value, RuntimeError> {
    Ok(new_err(vm, args.swap_remove(0)))
}

/// `context(r, msg)`: an `Err` gets `msg` added to its context chain; an
/// `Ok` passes through unchanged.
fn native_context(_vm: &mut Vm<'_>, mut args: Vec<Value>) -> Result<Value, RuntimeError> {
    let msg: Rc<str> = str_arg("context", &args[1])?.into();
    match args.swap_remove(0) {
        Value::Err(mut e) => {
            Rc::make_mut(&mut e).context.push(msg);
            Ok(Value::Err(e))
        }
        ok @ Value::Ok(_) => Ok(ok),
        other => Err(arg_type_error("context", "a result (Ok or Err)", &other)),
    }
}

/// `is_ok(r)`.
fn native_is_ok(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    match &args[0] {
        Value::Ok(_) => Ok(Value::Bool(true)),
        Value::Err(_) => Ok(Value::Bool(false)),
        other => Err(arg_type_error("is_ok", "a result (Ok or Err)", other)),
    }
}

/// `is_err(r)`.
fn native_is_err(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    match &args[0] {
        Value::Ok(_) => Ok(Value::Bool(false)),
        Value::Err(_) => Ok(Value::Bool(true)),
        other => Err(arg_type_error("is_err", "a result (Ok or Err)", other)),
    }
}

/// `unwrap(x)`: the value inside an `Ok`; R0403 for an `Err`, R0402 for
/// `none`; any other value is returned as it is (an optional that is set).
fn native_unwrap(_vm: &mut Vm<'_>, mut args: Vec<Value>) -> Result<Value, RuntimeError> {
    match args.swap_remove(0) {
        Value::Ok(v) => Ok(Rc::try_unwrap(v).unwrap_or_else(|rc| (*rc).clone())),
        Value::Err(e) => Err(RuntimeError::new(
            ErrorKind::UnwrapErr,
            format!("unwrap of Err({})", e.payload.repr()),
        )
        .note("help: handle the error with `?`, is_err(r) or or_else(r, default)")),
        Value::None => Err(RuntimeError::new(ErrorKind::UnwrapNone, "unwrap of none")
            .note("help: test for none first (`x == none`) or use or_else(x, default)")),
        other => Ok(other),
    }
}

/// `or_else(x, default)` (provisional name): `default` for `none` and for
/// an `Err`, the value inside an `Ok`, else `x` itself.
fn native_or_else(_vm: &mut Vm<'_>, mut args: Vec<Value>) -> Result<Value, RuntimeError> {
    let default = args.pop().expect("two arguments");
    Ok(match args.pop().expect("two arguments") {
        Value::None | Value::Err(_) => default,
        Value::Ok(v) => Rc::try_unwrap(v).unwrap_or_else(|rc| (*rc).clone()),
        other => other,
    })
}

/// `parse_int(s)`: the int written in `s` (surrounding whitespace and a
/// sign allowed), or `none`.
fn native_parse_int(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let s = str_arg("parse_int", &args[0])?;
    Ok(s.trim().parse::<i64>().map_or(Value::None, Value::Int))
}

/// `parse_float(s)`: the float written in `s` (`1.5`, `2e3`, `inf`), or
/// `none`.
fn native_parse_float(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let s = str_arg("parse_float", &args[0])?;
    Ok(s.trim().parse::<f64>().map_or(Value::None, Value::Float))
}

/// `read_line()`: the next line of standard input without its line ending,
/// or `none` at the end of input.
fn native_read_line(vm: &mut Vm<'_>, _args: Vec<Value>) -> Result<Value, RuntimeError> {
    match vm.host().read_line() {
        Ok(Some(line)) => Ok(Value::Str(line.into())),
        Ok(None) => Ok(Value::None),
        Err(e) => Err(RuntimeError::new(
            ErrorKind::HostIo,
            format!("cannot read standard input: {}", io_message(&e)),
        )),
    }
}

/// `read_file(path)`: `Ok(content)` or `Err("cannot read …: reason")`.
fn native_read_file(vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let path = str_arg("read_file", &args[0])?;
    match vm.host().read_file(path) {
        Ok(text) => Ok(Value::Ok(Rc::new(Value::Str(text.into())))),
        Err(e) => {
            let msg = format!("cannot read {}: {}", quote(path), io_message(&e));
            Ok(new_err(vm, Value::Str(msg.into())))
        }
    }
}

/// `write_file(path, content)`: `Ok(())` or `Err("cannot write …: reason")`.
fn native_write_file(vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let path = str_arg("write_file", &args[0])?;
    let content = str_arg("write_file", &args[1])?;
    match vm.host().write_file(path, content) {
        Ok(()) => Ok(Value::Ok(Rc::new(Value::Unit))),
        Err(e) => {
            let msg = format!("cannot write {}: {}", quote(path), io_message(&e));
            Ok(new_err(vm, Value::Str(msg.into())))
        }
    }
}

/// A short, platform-independent reason for an I/O error.
fn io_message(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => "file not found".into(),
        K::PermissionDenied => "permission denied".into(),
        K::IsADirectory => "is a directory".into(),
        K::InvalidData => "not valid UTF-8 text".into(),
        _ => {
            // Drop the OS-specific suffix " (os error 21)".
            let text = e.to_string();
            match text.find(" (os error") {
                Some(i) => text[..i].to_string(),
                None => text,
            }
        }
    }
}

// ---- M8: lists and higher-order functions ---------------------------------

/// R0206 unless `v` is a list.
fn list_arg(native: &str, v: Value) -> Result<Rc<Vec<Value>>, RuntimeError> {
    match v {
        Value::List(xs) => Ok(xs),
        other => Err(arg_type_error(native, "a list", &other)),
    }
}

/// R0206 unless `v` is an int.
fn int_arg(native: &str, v: &Value) -> Result<i64, RuntimeError> {
    match v {
        Value::Int(n) => Ok(*n),
        other => Err(arg_type_error(native, "an int", other)),
    }
}

/// The elements of a list argument, owned: moved out when nothing else
/// holds the list, cloned otherwise.
fn owned_items(xs: Rc<Vec<Value>>) -> Vec<Value> {
    Rc::try_unwrap(xs).unwrap_or_else(|rc| (*rc).clone())
}

/// The two-argument split of `args` (no allocation).
fn two(mut args: Vec<Value>) -> (Value, Value) {
    let b = args.pop().expect("two arguments");
    let a = args.pop().expect("two arguments");
    (a, b)
}

/// `len(x)`: number of elements of a list, or of Unicode scalar values of
/// a str (like `str_len`).
fn native_len(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    match &args[0] {
        Value::List(xs) => Ok(Value::Int(xs.len() as i64)),
        Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
        other => Err(arg_type_error("len", "a list or a str", other)),
    }
}

/// `get(xs, i)`: `xs[i]`, or `none` when `i` is out of range (S14).
fn native_get(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let (xs, i) = two(args);
    let i = int_arg("get", &i)?;
    let xs = list_arg("get", xs)?;
    Ok(usize::try_from(i)
        .ok()
        .and_then(|i| xs.get(i))
        .cloned()
        .unwrap_or(Value::None))
}

/// `range(n)` = `0, 1, …, n-1`; `range(a, b)` = `a, …, b-1` (end-exclusive,
/// S14); empty when the end is not above the start.
fn native_range(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let (lo, hi) = match &args[1] {
        Value::Missing => (0, int_arg("range", &args[0])?),
        b => (int_arg("range", &args[0])?, int_arg("range", b)?),
    };
    Ok(Value::List(Rc::new(
        (lo..hi.max(lo)).map(Value::Int).collect(),
    )))
}

/// `map(xs, f)`: `[f(x) for each x]`.
fn native_map(vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let (xs, f) = two(args);
    let xs = owned_items(list_arg("map", xs)?);
    let mut out = Vec::with_capacity(xs.len());
    for x in xs {
        out.push(vm.call_value(f.clone(), vec![x])?);
    }
    Ok(Value::List(Rc::new(out)))
}

/// `filter(xs, p)`: the elements for which `p` returns `true` (`p` must
/// return a bool, R0206).
fn native_filter(vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let (xs, p) = two(args);
    let xs = owned_items(list_arg("filter", xs)?);
    let mut out = Vec::new();
    for x in xs {
        match vm.call_value(p.clone(), vec![x.clone()])? {
            Value::Bool(true) => out.push(x),
            Value::Bool(false) => {}
            other => {
                return Err(arg_type_error(
                    "filter",
                    "a predicate that returns a bool",
                    &other,
                ));
            }
        }
    }
    Ok(Value::List(Rc::new(out)))
}

/// `reduce(xs, init, f)`: `f(…f(f(init, x0), x1)…, xn)`.
fn native_reduce(vm: &mut Vm<'_>, mut args: Vec<Value>) -> Result<Value, RuntimeError> {
    let f = args.pop().expect("three arguments");
    let mut acc = args.pop().expect("three arguments");
    let xs = owned_items(list_arg("reduce", args.pop().expect("three arguments"))?);
    for x in xs {
        acc = vm.call_value(f.clone(), vec![acc, x])?;
    }
    Ok(acc)
}

/// `flat_map(xs, f)`: the lists `f(x)` joined (`f` must return a list).
fn native_flat_map(vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let (xs, f) = two(args);
    let xs = owned_items(list_arg("flat_map", xs)?);
    let mut out = Vec::new();
    for x in xs {
        match vm.call_value(f.clone(), vec![x])? {
            Value::List(ys) => out.extend(owned_items(ys)),
            other => {
                return Err(arg_type_error(
                    "flat_map",
                    "a function that returns a list",
                    &other,
                ));
            }
        }
    }
    Ok(Value::List(Rc::new(out)))
}

/// `each(xs, f)`: call `f(x)` for every element (for its effects); `()`.
fn native_each(vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let (xs, f) = two(args);
    let xs = owned_items(list_arg("each", xs)?);
    for x in xs {
        vm.call_value(f.clone(), vec![x])?;
    }
    Ok(Value::Unit)
}

/// `contains(xs, x)` (`x in xs`): whether a list has an element equal to
/// `x` (`==` rules: comparing different types is an error), or whether a
/// str contains the str `x`.
fn native_contains(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    match (&args[0], &args[1]) {
        (Value::List(xs), x) => {
            for y in xs.iter() {
                if values_equal(y, x)? {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }
        (Value::Str(s), Value::Str(t)) => Ok(Value::Bool(s.contains(&**t))),
        (Value::Str(_), other) => Err(arg_type_error(
            "contains",
            "a str to search for in a str",
            other,
        )),
        (other, _) => Err(arg_type_error("contains", "a list or a str", other)),
    }
}

/// `append(xs, x)`: a list with `x` added at the end. O(1) when nothing
/// else holds `xs` (it is reused), else a copy.
fn native_append(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let (xs, x) = two(args);
    let mut xs = list_arg("append", xs)?;
    Rc::make_mut(&mut xs).push(x);
    Ok(Value::List(xs))
}

/// `reverse(xs)`: the elements in reverse order.
fn native_reverse(_vm: &mut Vm<'_>, mut args: Vec<Value>) -> Result<Value, RuntimeError> {
    let mut xs = list_arg("reverse", args.swap_remove(0))?;
    Rc::make_mut(&mut xs).reverse();
    Ok(Value::List(xs))
}

/// `lines(s)`: the lines of `s` without their line endings (`\n` or
/// `\r\n`); a final line ending does not start another line.
fn native_lines(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let s = str_arg("lines", &args[0])?;
    Ok(Value::List(Rc::new(
        s.lines().map(|l| Value::Str(l.into())).collect(),
    )))
}

/// `split(s, sep)`: the pieces of `s` between occurrences of `sep`
/// (Python's `s.split(sep)`; an empty `sep` is an error).
fn native_split(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let s = str_arg("split", &args[0])?;
    let sep = str_arg("split", &args[1])?;
    if sep.is_empty() {
        return Err(arg_type_error("split", "a non-empty separator", &args[1]));
    }
    Ok(Value::List(Rc::new(
        s.split(sep).map(|p| Value::Str(p.into())).collect(),
    )))
}

/// `join(xs, sep)`: the strs of `xs` with `sep` between them.
fn native_join(_vm: &mut Vm<'_>, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let sep = str_arg("join", &args[1])?;
    let Value::List(xs) = &args[0] else {
        return Err(arg_type_error("join", "a list of str", &args[0]));
    };
    let mut out = String::new();
    for (i, x) in xs.iter().enumerate() {
        let Value::Str(t) = x else {
            return Err(RuntimeError::new(
                ErrorKind::NativeArgType,
                format!(
                    "join() expects a list of str, but element {i} is {}",
                    x.describe()
                ),
            )
            .note("help: convert the elements first, e.g. map(xs, str)"));
        };
        if i > 0 {
            out.push_str(sep);
        }
        out.push_str(t);
    }
    Ok(Value::Str(out.into()))
}
