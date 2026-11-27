//! # The interpreter (`vm.rs`)
//!
//! **What this file is:** the virtual machine that executes bytecode. It is a
//! loop that fetches the next instruction, decodes its opcode and operands,
//! and performs the operation on *registers* (slots of a value stack). This
//! is the "register-based" design of Lua 5.x (`lvm.c`): instructions name
//! their operands directly (`ADD 0 0 1` = `R0 = R0 + R1`) instead of pushing
//! and popping an operand stack.
//!
//! **Pipeline position:** last stage. Input: `chunk::Program` from the
//! compiler (`compiler/src/codegen.rs`). Output: the program's effects (via
//! `host::Host`) and either a final `Value` or a `RuntimeError`.
//!
//! **Textbook chapter:** `docs/07-virtual-machine.md`.
//!
//! ## Where to change things
//!
//! | I want to…                                   | Look at                                           |
//! |----------------------------------------------|---------------------------------------------------|
//! | make the VM execute a new opcode             | `Vm::execute`, the big `match op { ... }`          |
//! | change what `+ - * // % **` do on ints       | `int_arith` (and `arith` for type dispatch)       |
//! | support arithmetic on a new type (floats, M3)| `arith`: add a `(Value::Float(..), Value::Float(..))` arm |
//! | change the stack trace contents              | `Vm::unwind`                                       |
//! | change `--trace-exec` output                 | `Vm::trace_instr`                                  |
//! | add function calls (M4)                      | `Frame`, `RETURN` arm in `execute`, `Vm::run`      |
//!
//! ## Memory layout (plan §2.5)
//!
//! ```text
//!   stack: [ fn slot | R0 | R1 | R2 | ... ]
//!             base-1   ^base
//! ```
//! A `Frame` remembers which prototype runs, where its registers start
//! (`base`) and which instruction is next (`pc`). M1 has exactly one frame,
//! `<main>`; M4 pushes one frame per call, each with its own window.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `struct Vm<'h>` holding `&'h mut dyn Host` — a borrowed host that must
//!   outlive the VM (*Lifetimes*, *Traits, impl and derive*).
//! - `Result<Value, RuntimeError>` and the `?` operator for early error return (*Option and Result*).
//! - `Rc::clone(&x)` — copy a shared pointer, not the data (*Rc shared ownership*).
//! - `match` with `|` alternatives and guards (`if y == 0`) (*Pattern matching*).
//! - `checked_add` & co. — arithmetic that reports overflow (*Integer types and casts*).

use std::rc::Rc;

use crate::chunk::{Const, Program, Proto};
use crate::error::{ErrorKind, RuntimeError, TraceEntry};
use crate::host::Host;
use crate::opcode::{Format, Instr, OpCode};
use crate::value::Value;

/// Interpreter options.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Print every executed instruction and the register it wrote to stderr
    /// (`valo run --trace-exec`). Debugging aid only.
    pub trace_exec: bool,
}

/// One activation record: the state of one running function.
#[derive(Debug)]
struct Frame {
    /// The code being executed.
    proto: Rc<Proto>,
    /// Index of `R0` in the value stack; the function slot is `base - 1`.
    base: usize,
    /// Index of the next instruction ("program counter").
    pc: usize,
}

/// The virtual machine.
///
/// RUST: `'h` is a *lifetime parameter*. The VM borrows a host
/// (`&'h mut dyn Host`) instead of owning it; `'h` tells the compiler the VM
/// may not outlive that host. `dyn Host` means "any type that implements the
/// `Host` trait", decided at runtime (`StdHost` in the CLI, `TestHost` in tests).
pub struct Vm<'h> {
    /// All registers of all frames, end to end.
    stack: Vec<Value>,
    /// Call stack; the last element is the running function.
    frames: Vec<Frame>,
    /// Where effects (printing) go.
    host: &'h mut dyn Host,
    options: Options,
}

impl<'h> Vm<'h> {
    /// A VM that performs effects through `host`.
    pub fn new(host: &'h mut dyn Host, options: Options) -> Vm<'h> {
        Vm {
            stack: Vec::new(),
            frames: Vec::new(),
            host,
            options,
        }
    }

    /// Run a program to completion and return `main`'s result.
    pub fn run(&mut self, program: &Program) -> Result<Value, RuntimeError> {
        let main = Rc::clone(&program.main);
        self.stack.clear();
        self.frames.clear();
        // Slot 0 is main's function slot (Lua convention); registers follow.
        self.stack.resize(1 + main.num_regs as usize, Value::Unit);
        self.frames.push(Frame {
            proto: main,
            base: 1,
            pc: 0,
        });
        // RUST: `map_err` transforms only the error case; here it attaches
        // the stack trace before the error leaves the VM.
        self.execute().map_err(|e| self.unwind(e))
    }

    /// Attach a stack trace (innermost first) to an error.
    fn unwind(&mut self, mut err: RuntimeError) -> RuntimeError {
        // For each frame, newest first: function name + the span of the
        // instruction that was executing (`pc - 1`, because `pc` was already
        // advanced past it).
        err.trace = self
            .frames
            .iter()
            .rev()
            .map(|f| TraceEntry {
                fn_name: Rc::clone(&f.proto.name),
                span: f
                    .proto
                    .spans
                    .get(f.pc.saturating_sub(1))
                    .copied()
                    .unwrap_or_default(),
            })
            .collect();
        self.frames.clear();
        err
    }

    /// The fetch–decode–execute loop.
    fn execute(&mut self) -> Result<Value, RuntimeError> {
        loop {
            // ---- fetch -------------------------------------------------
            // RUST: `last_mut()` returns `Option<&mut Frame>`; `expect` unwraps
            // it or panics with the message (a bug, never a user error).
            let frame = self.frames.last_mut().expect("execute without a frame");
            // RUST: `let Some(x) = ... else { ... }` — take the value out of
            // the Option, or run the else block (which must leave the function).
            let Some(&instr) = frame.proto.code.get(frame.pc) else {
                return Err(internal("fell off the end of the code"));
            };
            frame.pc += 1;
            let base = frame.base;
            // Cloning the Rc lets us keep using the prototype while `self`
            // is borrowed mutably below (only a counter is incremented).
            let proto = Rc::clone(&frame.proto);

            // ---- decode ------------------------------------------------
            // RUST: `?` = "if this is an Err, return it from `execute` now".
            let op = instr
                .op()
                .map_err(|b| internal(format!("invalid opcode byte {b}")))?;
            // Absolute stack index of register A.
            let ra = base + instr.a() as usize;

            // ---- execute -----------------------------------------------
            // EXTEND: every new opcode gets an arm here.
            match op {
                OpCode::LOADUNIT => self.stack[ra] = Value::Unit,
                OpCode::RETURN => {
                    // `mem::replace` moves the value out and leaves `Unit` behind.
                    let v = std::mem::replace(&mut self.stack[ra], Value::Unit);
                    self.frames.pop();
                    // M4: if a caller frame remains, store into its slot and continue.
                    return Ok(v);
                }
                OpCode::LOADI => self.stack[ra] = Value::Int(instr.sbx() as i64),
                OpCode::LOADK => {
                    let k = proto
                        .consts
                        .get(instr.bx() as usize)
                        .ok_or_else(|| internal("constant index out of range"))?;
                    self.stack[ra] = match k {
                        Const::Int(n) => Value::Int(*n),
                    };
                }
                // All binary arithmetic shares one arm: R[A] = R[B] op R[C].
                OpCode::ADD
                | OpCode::SUB
                | OpCode::MUL
                | OpCode::IDIV
                | OpCode::MOD
                | OpCode::POW => {
                    let b = &self.stack[base + instr.b() as usize];
                    let c = &self.stack[base + instr.c() as usize];
                    let v = arith(op, b, c)?;
                    self.stack[ra] = v;
                }
                OpCode::NEG => {
                    let v = match &self.stack[base + instr.b() as usize] {
                        Value::Int(n) => Value::Int(
                            // -i64::MIN does not fit in i64: checked_neg
                            // returns None, which becomes R0101.
                            n.checked_neg()
                                .ok_or_else(|| overflow(format!("integer overflow: -({n})")))?,
                        ),
                        // Any other type (only `()` in M1) cannot be negated.
                        // RUST: a bare name in a pattern matches anything and
                        // binds it, here to `other`.
                        other => {
                            return Err(RuntimeError::new(
                                ErrorKind::TypeMismatch,
                                format!("cannot negate {}", other.type_name()),
                            ));
                        }
                    };
                    self.stack[ra] = v;
                }
                OpCode::ECHO => {
                    let s = format!("{}\n", self.stack[ra]);
                    self.host.write_stdout(&s).map_err(host_io)?;
                }
                // Declared in opcode.rs but not implemented yet: the compiler
                // never emits them, so reaching this means corrupt bytecode.
                other => {
                    return Err(internal(format!("opcode {other} is not implemented yet")));
                }
            }

            if self.options.trace_exec {
                self.trace_instr(&proto, instr, ra)?;
            }
        }
    }

    /// One `--trace-exec` line: `[<main>] 0003 ADD 0 0 1 | R[0]=17`.
    fn trace_instr(&mut self, proto: &Proto, instr: Instr, ra: usize) -> Result<(), RuntimeError> {
        let pc = self.frames.last().map_or(0, |f| f.pc.saturating_sub(1));
        let mut line = format!("[{}] {pc:04} {instr:?}", proto.name);
        let writes_a = instr.op().is_ok_and(|op| {
            op.format() != Format::SJ && !matches!(op, OpCode::ECHO | OpCode::RETURN)
        });
        if writes_a {
            if let Some(v) = self.stack.get(ra) {
                line.push_str(&format!(" | R[{}]={v}", instr.a()));
            }
        }
        line.push('\n');
        self.host.write_stderr(&line).map_err(host_io)
    }
}

// Small constructors for common errors, to keep `execute` readable.

fn internal(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::new(ErrorKind::Internal, msg)
}

fn overflow(msg: String) -> RuntimeError {
    RuntimeError::new(ErrorKind::IntOverflow, msg)
}

fn host_io(e: std::io::Error) -> RuntimeError {
    RuntimeError::new(ErrorKind::HostIo, format!("cannot write output: {e}"))
}

/// Binary arithmetic (D8, D9). Type-directed; M1 handles `int` only.
///
/// EXTEND (M3): add `(Value::Float(x), Value::Float(y))` here. Mixed
/// int/float operands stay an error (R0201) except for `/` (plan D9, D10).
pub fn arith(op: OpCode, b: &Value, c: &Value) -> Result<Value, RuntimeError> {
    // RUST: matching on a tuple of two references checks both types at once.
    match (b, c) {
        (Value::Int(x), Value::Int(y)) => int_arith(op, *x, *y).map(Value::Int),
        _ => Err(RuntimeError::new(
            ErrorKind::TypeMismatch,
            format!(
                "unsupported operand types for {}: {} and {}",
                symbol(op),
                b.type_name(),
                c.type_name()
            ),
        )),
    }
}

/// Source-level symbol of an arithmetic opcode (for error messages).
pub fn symbol(op: OpCode) -> &'static str {
    match op {
        OpCode::ADD => "+",
        OpCode::SUB => "-",
        OpCode::MUL => "*",
        OpCode::DIV => "/",
        OpCode::IDIV => "//",
        OpCode::MOD => "%",
        OpCode::POW => "**",
        _ => "?",
    }
}

/// Checked `int` arithmetic with Python floor semantics for `//` and `%`.
///
/// - overflow anywhere → R0101 (never wraps silently; D8);
/// - `x // 0`, `x % 0` → R0102;
/// - `//` rounds toward negative infinity: `-7 // 2 == -4`;
/// - `%` takes the sign of the divisor: `-7 % 3 == 2`, `7 % -3 == -2`;
/// - `x ** negative` → R0104 (would need a float result; plan D9).
pub fn int_arith(op: OpCode, x: i64, y: i64) -> Result<i64, RuntimeError> {
    let sym = symbol(op);
    // RUST: a closure (anonymous function) that builds the overflow error;
    // `ok_or_else(ovf)` calls it only when a checked operation returned None.
    let ovf = || overflow(format!("integer overflow: {x} {sym} {y}"));
    match op {
        OpCode::ADD => x.checked_add(y).ok_or_else(ovf),
        OpCode::SUB => x.checked_sub(y).ok_or_else(ovf),
        OpCode::MUL => x.checked_mul(y).ok_or_else(ovf),
        // RUST: `if y == 0` after the pattern is a *guard*: this arm applies
        // only when the condition holds; otherwise matching continues below.
        OpCode::IDIV | OpCode::MOD if y == 0 => Err(RuntimeError::new(
            ErrorKind::DivisionByZero,
            format!(
                "{} by zero: {x} {sym} 0",
                if op == OpCode::MOD {
                    "modulo"
                } else {
                    "division"
                }
            ),
        )),
        OpCode::IDIV => {
            // Rust's `/` truncates toward zero; Python's `//` floors. They
            // differ when the signs differ and there is a remainder.
            let q = x.checked_div(y).ok_or_else(ovf)?;
            Ok(if x % y != 0 && ((x < 0) != (y < 0)) {
                q - 1
            } else {
                q
            })
        }
        OpCode::MOD => {
            // i64::MIN % -1 overflows in Rust although the answer is 0.
            if y == -1 {
                return Ok(0);
            }
            // Rust's `%` takes the sign of the dividend; shift into the
            // divisor's sign to get Python's result.
            let r = x % y;
            Ok(if r != 0 && ((r < 0) != (y < 0)) {
                r + y
            } else {
                r
            })
        }
        OpCode::POW => {
            if y < 0 {
                return Err(RuntimeError::new(
                    ErrorKind::NegativeExponent,
                    format!("negative exponent: {x} ** {y} (int powers need an exponent >= 0)"),
                ));
            }
            // `checked_pow` takes a u32 exponent; huge exponents only work
            // for bases 0, 1 and -1.
            match u32::try_from(y) {
                Ok(e) => x.checked_pow(e).ok_or_else(ovf),
                Err(_) => match x {
                    0 | 1 => Ok(x),
                    -1 => Ok(if y % 2 == 0 { 1 } else { -1 }),
                    _ => Err(ovf()),
                },
            }
        }
        _ => Err(internal(format!("{op} is not an int operation"))),
    }
}

// Unit tests: run hand-assembled bytecode (no compiler involved) and check
// arithmetic against Python's rules. `cargo test -p valo-vm` runs them.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Const, ProtoBuilder};
    use crate::host::TestHost;
    use crate::opcode::Instr;
    use proptest::prelude::*;

    fn run(p: &Program) -> (Result<Value, RuntimeError>, TestHost) {
        let mut host = TestHost::default();
        let r = Vm::new(&mut host, Options::default()).run(p);
        (r, host)
    }

    #[test]
    fn empty_program_returns_unit() {
        let p = ProtoBuilder::new("<main>", 1)
            .op(Instr::abc(OpCode::LOADUNIT, 0, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        assert_eq!(run(&p).0, Ok(Value::Unit));
    }

    #[test]
    fn arithmetic_and_echo() {
        // (2 + 70000) * -3, echoed
        let p = ProtoBuilder::new("<main>", 2)
            .konst(Const::Int(70000))
            .op(Instr::asbx(OpCode::LOADI, 0, 2))
            .op(Instr::abx(OpCode::LOADK, 1, 0))
            .op(Instr::abc(OpCode::ADD, 0, 0, 1))
            .op(Instr::asbx(OpCode::LOADI, 1, 3))
            .op(Instr::abc(OpCode::NEG, 1, 1, 0))
            .op(Instr::abc(OpCode::MUL, 0, 0, 1))
            .op(Instr::abc(OpCode::ECHO, 0, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        let (r, host) = run(&p);
        assert_eq!(r, Ok(Value::Int(-210006)));
        assert_eq!(host.stdout, "-210006\n");
    }

    #[test]
    fn errors_carry_a_trace() {
        let p = ProtoBuilder::new("<main>", 2)
            .op(Instr::asbx(OpCode::LOADI, 0, 1))
            .op(Instr::asbx(OpCode::LOADI, 1, 0))
            .op(Instr::abc(OpCode::IDIV, 0, 0, 1))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        let err = run(&p).0.unwrap_err();
        assert_eq!(err.kind, ErrorKind::DivisionByZero);
        assert_eq!(err.kind.code(), "R0102");
        assert_eq!(err.trace.len(), 1);
        assert_eq!(&*err.trace[0].fn_name, "<main>");
    }

    #[test]
    fn floor_semantics_match_python() {
        let cases = [
            (OpCode::IDIV, -7, 2, -4),
            (OpCode::IDIV, 7, -2, -4),
            (OpCode::IDIV, -7, -2, 3),
            (OpCode::MOD, -7, 3, 2),
            (OpCode::MOD, 7, -3, -2),
            (OpCode::MOD, -7, -3, -1),
            (OpCode::MOD, i64::MIN, -1, 0),
            (OpCode::POW, 2, 10, 1024),
            (OpCode::POW, -1, 1 << 40, 1),
            (OpCode::POW, 0, 0, 1),
        ];
        for (op, x, y, want) in cases {
            assert_eq!(int_arith(op, x, y).unwrap(), want, "{x} {} {y}", symbol(op));
        }
        assert_eq!(
            int_arith(OpCode::IDIV, i64::MIN, -1).unwrap_err().kind,
            ErrorKind::IntOverflow
        );
        assert_eq!(
            int_arith(OpCode::POW, 2, -1).unwrap_err().kind,
            ErrorKind::NegativeExponent
        );
        assert_eq!(
            int_arith(OpCode::POW, 2, 64).unwrap_err().kind,
            ErrorKind::IntOverflow
        );
    }

    proptest! {
        #[test]
        fn add_sub_mul_match_checked(x: i64, y: i64) {
            prop_assert_eq!(int_arith(OpCode::ADD, x, y).ok(), x.checked_add(y));
            prop_assert_eq!(int_arith(OpCode::SUB, x, y).ok(), x.checked_sub(y));
            prop_assert_eq!(int_arith(OpCode::MUL, x, y).ok(), x.checked_mul(y));
        }

        #[test]
        fn floor_div_mod_identity(x: i64, y in prop::num::i64::ANY.prop_filter("nonzero", |y| *y != 0)) {
            // Python invariant: x == (x // y) * y + x % y, and x % y has the sign of y.
            if let (Ok(q), Ok(r)) = (int_arith(OpCode::IDIV, x, y), int_arith(OpCode::MOD, x, y)) {
                prop_assert_eq!(i128::from(q) * i128::from(y) + i128::from(r), i128::from(x));
                prop_assert!(r == 0 || (r < 0) == (y < 0));
                prop_assert!(r.unsigned_abs() < y.unsigned_abs());
            }
        }
    }
}
