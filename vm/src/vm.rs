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
//! | change float arithmetic or true division `/` | `float_arith`, `arith`                             |
//! | support arithmetic on a new type (str, M7)   | `arith`: add a `(Value::Str(..), Value::Str(..))` arm |
//! | change `==` / `<` rules                      | `values_equal`, `less`                              |
//! | change conditions and jumps                  | `TEST`, `JMP`, `ASSERT` arms in `execute`           |
//! | change the stack trace contents              | `Vm::unwind`                                       |
//! | change `--trace-exec` output                 | `Vm::trace_instr`                                  |
//! | change how calls work                        | `Vm::call` (CALL), `Vm::tail_call` (TAILCALL), `Vm::do_return` (RETURN), `Frame` |
//! | change stack/recursion limits                | `MAX_FRAMES`, `MAX_STACK`                           |
//! | change when the debugger is called           | `Vm::debug_line`, `Vm::debug_error` (hook API in `debug.rs`) |
//! | let natives do I/O                           | `Vm::write_stdout` (natives get `&mut Vm`)          |
//! | change how globals are stored                | `Vm::globals`, `GETGLOBAL`/`SETGLOBAL` arms         |
//!
//! ## Memory layout (plan §2.5)
//!
//! ```text
//!   stack: [ fn slot | R0 | R1 | R2 | ... ]
//!             base-1   ^base
//! ```
//! A `Frame` remembers which closure runs, where its registers start
//! (`base`) and which instruction is next (`pc`). `CALL A B` makes the
//! callee's window start right after the function slot `R[A]`, so its
//! arguments `R[A+1..=A+B]` are already its parameters `R0..`:
//!
//! ```text
//!   caller:  [ f | R0 R1 … R[A]=g  R[A+1]  R[A+2] ]
//!   callee:                  g  | R0'    R1'   R2' …   (base' = base+A+1)
//! ```
//!
//! `RETURN X` stores `R'[X]` into the callee's function slot (the caller's
//! `R[A]`), pops the frame and cuts the stack back to the caller's window, so
//! values of finished calls are dropped at once. Calls between valo
//! functions never recurse in Rust: the loop just switches frames.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `struct Vm<'h>` holding `&'h mut dyn Host` — a borrowed host that must
//!   outlive the VM (*Lifetimes*, *Traits, impl and derive*).
//! - `Result<Value, RuntimeError>` and the `?` operator for early error return (*Option and Result*).
//! - `Rc::clone(&x)` — copy a shared pointer, not the data (*Rc shared ownership*).
//! - `match` with `|` alternatives and guards (`if y == 0`) (*Pattern matching*).
//! - `checked_add` & co. — arithmetic that reports overflow (*Integer types and casts*).
//! - `f64` methods `floor`, `powf`, `copysign`; `as f64` conversion (*Integer types and casts*).

use std::rc::Rc;

use crate::builtins::NATIVES;
use crate::chunk::{Const, GlobalKind, NativeId, Program, Proto, SrcSpan, UpvalSrc};
use crate::debug::{DebugHook, LineTable, Resume, VmView};
use crate::error::{ErrorKind, RuntimeError, Site, TraceEntry};
use crate::host::Host;
use crate::opcode::{Format, Instr, OpCode};
use crate::value::{Closure, Value};

/// Most frames that may be active at once (R0301 beyond).
pub const MAX_FRAMES: usize = 200_000;
/// Most value-stack slots (16 MiB of values; R0301 beyond).
pub const MAX_STACK: usize = 1 << 20;
/// Most built-ins that may be calling back into valo at once (`map` whose
/// callback calls `map` …, plan D7); each level is a Rust recursion of the
/// dispatch loop, so it is capped well below the frame limit (R0301).
pub const MAX_REENTRY: usize = 200;
/// Rust stack a thread running `Vm::run` should have. Each re-entry level
/// (`MAX_REENTRY`) nests the dispatch loop in Rust; unoptimised builds need
/// more than the 2 MiB of a default spawned thread for all 200 levels, so
/// the CLI and the debugger run the VM on a thread of this size.
pub const VM_STACK_BYTES: usize = 64 << 20;

/// Interpreter options.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Print every executed instruction and the register it wrote to stderr
    /// (`valo run --trace-exec`). Debugging aid only.
    pub trace_exec: bool,
}

/// One activation record: the state of one running function.
/// `pub(crate)`: the debugger's `VmView` (`debug.rs`) reads frames.
#[derive(Debug)]
pub(crate) struct Frame {
    /// The function being executed (`<main>` is a closure too).
    pub(crate) closure: Rc<Closure>,
    /// Index of `R0` in the value stack; the function slot is `base - 1`.
    pub(crate) base: usize,
    /// Index of the next instruction ("program counter").
    pub(crate) pc: usize,
    /// Frames this one replaced by tail calls (M5); shown in traces.
    pub(crate) tail_calls: u32,
    /// Entered from a built-in through `Vm::call_value` (M8): its `RETURN`
    /// ends the nested dispatch loop instead of resuming a caller frame.
    pub(crate) reentry: bool,
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
    /// Global slots (top-level bindings, plan D4). `None` = not yet set;
    /// this is an internal state, not a language-level nil.
    globals: Vec<Option<Value>>,
    /// Name of each global slot, for error messages.
    global_names: Vec<Rc<str>>,
    /// Whether each global slot is a built-in (the debugger hides those).
    global_builtin: Vec<bool>,
    /// Where effects (printing) go.
    host: &'h mut dyn Host,
    options: Options,
    /// An attached debugger (`valo-dap`), called at every line start.
    hook: Option<&'h mut dyn DebugHook>,
    /// Which instructions start a line; built by `run` when a hook is set.
    lines: Option<LineTable>,
    /// Built-ins running right now: `(id, frames.len() when called)`, for
    /// `<native map>` lines in stack traces (M8).
    native_marks: Vec<(NativeId, usize)>,
    /// Nested dispatch loops started by `call_value` (≤ `MAX_REENTRY`).
    reentry_depth: usize,
}

impl<'h> Vm<'h> {
    /// A VM that performs effects through `host`.
    pub fn new(host: &'h mut dyn Host, options: Options) -> Vm<'h> {
        Vm {
            stack: Vec::new(),
            frames: Vec::new(),
            globals: Vec::new(),
            global_names: Vec::new(),
            global_builtin: Vec::new(),
            host,
            options,
            hook: None,
            lines: None,
            native_marks: Vec::new(),
            reentry_depth: 0,
        }
    }

    /// Attach a debugger: `hook.on_line` runs before the first instruction
    /// of every source line (`debug.rs`). Without a hook the loop pays one
    /// `Option` check per instruction.
    pub fn set_hook(&mut self, hook: &'h mut dyn DebugHook) {
        self.hook = Some(hook);
    }

    /// Run a program to completion and return `main`'s result.
    pub fn run(&mut self, program: &Program) -> Result<Value, RuntimeError> {
        let main = Rc::new(Closure::new(Rc::clone(&program.main)));
        self.stack.clear();
        self.frames.clear();
        self.native_marks.clear();
        self.reentry_depth = 0;
        // Built-in slots are filled now; user slots wait for SETGLOBAL.
        self.globals = program
            .globals
            .iter()
            .map(|g| match g.kind {
                GlobalKind::Native(id) => Some(Value::Native(id)),
                GlobalKind::User => None,
            })
            .collect();
        self.global_names = program.globals.iter().map(|g| Rc::clone(&g.name)).collect();
        self.global_builtin = program
            .globals
            .iter()
            .map(|g| matches!(g.kind, GlobalKind::Native(_)))
            .collect();
        if self.hook.is_some() {
            self.lines = Some(LineTable::new(program));
        }
        // Slot 0 is main's function slot (Lua convention); registers follow.
        self.stack
            .resize(1 + main.proto.num_regs as usize, Value::Unit);
        self.stack[0] = Value::Closure(Rc::clone(&main));
        self.frames.push(Frame {
            closure: main,
            base: 1,
            pc: 0,
            tail_calls: 0,
            reentry: false,
        });
        // RUST: `map_err` transforms only the error case; here it attaches
        // the stack trace before the error leaves the VM.
        self.execute().map_err(|e| {
            self.debug_error(&e);
            self.unwind(e)
        })
    }

    /// Attach a stack trace (innermost first) to an error.
    ///
    /// For each frame, newest first: function name + the span of the
    /// instruction that was executing (`pc - 1`, because `pc` was already
    /// advanced past it). A built-in that was running a callback when the
    /// error happened appears between the callback's frames and its caller
    /// (`<native map>`, M8). A built-in that failed itself is not listed:
    /// its caller's line (the call) is the error's location.
    fn unwind(&mut self, mut err: RuntimeError) -> RuntimeError {
        let mut trace = Vec::with_capacity(self.frames.len());
        for (k, f) in self.frames.iter().enumerate().rev() {
            // Built-ins called by frame k (mark depth k+1) that have frames
            // above them, innermost (latest) first.
            if k + 1 < self.frames.len() {
                for &(id, _) in self.native_marks.iter().rev().filter(|m| m.1 == k + 1) {
                    trace.push(native_entry(id));
                }
            }
            trace.push(TraceEntry {
                fn_name: Rc::clone(&f.closure.proto.name),
                span: f
                    .closure
                    .proto
                    .spans
                    .get(f.pc.saturating_sub(1))
                    .copied()
                    .unwrap_or_default(),
                tail_calls: f.tail_calls,
                native: false,
            });
        }
        err.trace = trace;
        self.frames.clear();
        self.native_marks.clear();
        self.reentry_depth = 0;
        err
    }

    /// The fetch–decode–execute loop.
    fn execute(&mut self) -> Result<Value, RuntimeError> {
        loop {
            // ---- debugger ----------------------------------------------
            // A hook may stop here (it blocks) or abort the run.
            if self.hook.is_some() {
                self.debug_line()?;
            }

            // ---- fetch -------------------------------------------------
            // RUST: `last_mut()` returns `Option<&mut Frame>`; `expect` unwraps
            // it or panics with the message (a bug, never a user error).
            let frame = self.frames.last_mut().expect("execute without a frame");
            // RUST: `let Some(x) = ... else { ... }` — take the value out of
            // the Option, or run the else block (which must leave the function).
            let Some(&instr) = frame.closure.proto.code.get(frame.pc) else {
                return Err(internal("fell off the end of the code"));
            };
            // Index of this instruction, for `--trace-exec` (jumps change pc).
            let at = frame.pc;
            frame.pc += 1;
            let base = frame.base;
            // Cloning the Rc lets us keep using the prototype while `self`
            // is borrowed mutably below (only a counter is incremented).
            let proto = Rc::clone(&frame.closure.proto);

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
                    if let Some(result) = self.do_return(v) {
                        return Ok(result);
                    }
                    // Not traced: `ra` belongs to the finished frame.
                    continue;
                }
                // `?` (M7, D26): unwrap an Ok, or return the Err from this
                // function, recording this `?` on it.
                OpCode::TRY => {
                    let lenient = instr.c() == 1;
                    let v =
                        std::mem::replace(&mut self.stack[base + instr.b() as usize], Value::Unit);
                    match v {
                        Value::Ok(inner) => {
                            self.stack[ra] =
                                Rc::try_unwrap(inner).unwrap_or_else(|rc| (*rc).clone());
                        }
                        Value::Err(mut e) => {
                            let site = Site {
                                fn_name: Rc::clone(&proto.name),
                                span: proto.spans.get(at).copied().unwrap_or_default(),
                            };
                            Rc::make_mut(&mut e).propagated.push(site);
                            if self.frames.len() == 1 {
                                return Err(RuntimeError::unhandled(&e));
                            }
                            if let Some(result) = self.do_return(Value::Err(e)) {
                                return Ok(result);
                            }
                            // Not traced: the frame is gone.
                            continue;
                        }
                        other if lenient => self.stack[ra] = other,
                        other => return Err(RuntimeError::not_result(&other)),
                    }
                }
                OpCode::LOADNONE => self.stack[ra] = Value::None,
                OpCode::TAILCALL => {
                    if let Some(result) = self.tail_call(ra, instr.b() as usize)? {
                        return Ok(result);
                    }
                    // Not traced: the frame now runs another function.
                    continue;
                }
                OpCode::CLOSURE => {
                    let child = proto
                        .protos
                        .get(instr.bx() as usize)
                        .ok_or_else(|| internal("function index out of range"))?;
                    // Copy every captured value now (plan D5: bindings are
                    // immutable, so a copy is as good as a reference).
                    let current = &self.frames.last().expect("running").closure;
                    let mut captures = Vec::with_capacity(child.upvals.len());
                    for u in &child.upvals {
                        captures.push(match u.src {
                            UpvalSrc::Reg(r) if (r as usize) < proto.num_regs as usize => {
                                self.stack[base + r as usize].clone()
                            }
                            UpvalSrc::Upval(i) => current
                                .captures
                                .get(i as usize)
                                .cloned()
                                .ok_or_else(|| internal("capture index out of range"))?,
                            UpvalSrc::Current => Value::Closure(Rc::clone(current)),
                            UpvalSrc::Reg(_) => return Err(internal("register out of range")),
                        });
                    }
                    self.stack[ra] = Value::Closure(Rc::new(Closure {
                        proto: Rc::clone(child),
                        captures: captures.into_boxed_slice(),
                    }));
                }
                // String interpolation (M7, D25): R[A] = the text of
                // R[B], …, R[B+C-1] joined, in one allocation.
                OpCode::CONCAT => {
                    let first = base + instr.b() as usize;
                    let parts = self
                        .stack
                        .get(first..first + instr.c() as usize)
                        .ok_or_else(|| internal("register out of range"))?;
                    let mut out = String::new();
                    for v in parts {
                        match v {
                            Value::Str(t) => out.push_str(t),
                            // RUST: `write!` into a String (Display) cannot fail.
                            other => {
                                use std::fmt::Write as _;
                                let _ = write!(out, "{other}");
                            }
                        }
                    }
                    self.stack[ra] = Value::Str(out.into());
                }
                // Read a captured value (M6).
                OpCode::GETUPVAL => {
                    let current = &self.frames.last().expect("running").closure;
                    let v = current
                        .captures
                        .get(instr.b() as usize)
                        .cloned()
                        .ok_or_else(|| internal("capture index out of range"))?;
                    self.stack[ra] = v;
                }
                // The running closure itself: a local fn calling itself (M6).
                OpCode::GETSELF => {
                    let current = &self.frames.last().expect("running").closure;
                    self.stack[ra] = Value::Closure(Rc::clone(current));
                }
                OpCode::CALL => self.call(ra, instr.b() as usize)?,
                OpCode::LOADMISSING => self.stack[ra] = Value::Missing,
                // Callee prologue of a defaulted parameter: skip its default
                // expression when the caller passed a value (M4b).
                OpCode::JMPIFSET => {
                    if !matches!(self.stack[ra], Value::Missing) {
                        let frame = self.frame_mut();
                        let target = frame.pc as i64 + i64::from(instr.sbx());
                        if target < 0 || target as usize > proto.code.len() {
                            return Err(internal("jump target out of range"));
                        }
                        frame.pc = target as usize;
                    }
                }
                OpCode::LOADI => self.stack[ra] = Value::Int(instr.sbx() as i64),
                OpCode::LOADK => {
                    let k = proto
                        .consts
                        .get(instr.bx() as usize)
                        .ok_or_else(|| internal("constant index out of range"))?;
                    self.stack[ra] = match k {
                        Const::Int(n) => Value::Int(*n),
                        Const::Float(x) => Value::Float(*x),
                        Const::Str(t) => Value::Str(Rc::clone(t)),
                    };
                }
                // All binary arithmetic shares one arm: R[A] = R[B] op R[C].
                OpCode::ADD
                | OpCode::SUB
                | OpCode::MUL
                | OpCode::DIV
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
                                .ok_or_else(|| RuntimeError::int_overflow(&format!("-({n})")))?,
                        ),
                        Value::Float(x) => Value::Float(-x),
                        // Any other type cannot be negated.
                        // RUST: a bare name in a pattern matches anything and
                        // binds it, here to `other`.
                        other => return Err(RuntimeError::cannot_negate(other)),
                    };
                    self.stack[ra] = v;
                }
                // R[A] = R[B]. Cloning a `Value` copies at most an `Rc`.
                OpCode::MOVE => {
                    let v = self.stack[base + instr.b() as usize].clone();
                    self.stack[ra] = v;
                }
                OpCode::GETGLOBAL => {
                    let g = instr.bx() as usize;
                    let v = match self.globals.get(g) {
                        Some(Some(v)) => v.clone(),
                        // The compiler orders top-level code so this cannot
                        // happen from `<main>`; from M4 a function may run
                        // before the binding it reads (plan D4).
                        Some(None) => {
                            return Err(RuntimeError::global_uninit(&self.global_names[g]));
                        }
                        None => return Err(internal("global index out of range")),
                    };
                    self.stack[ra] = v;
                }
                OpCode::SETGLOBAL => {
                    let v = self.stack[ra].clone();
                    let slot = self
                        .globals
                        .get_mut(instr.bx() as usize)
                        .ok_or_else(|| internal("global index out of range"))?;
                    *slot = Some(v);
                }
                OpCode::LOADBOOL => self.stack[ra] = Value::Bool(instr.b() != 0),
                OpCode::EQ | OpCode::NE => {
                    let b = &self.stack[base + instr.b() as usize];
                    let c = &self.stack[base + instr.c() as usize];
                    let eq = values_equal(b, c)?;
                    self.stack[ra] = Value::Bool(eq == (op == OpCode::EQ));
                }
                // `a > b` / `a >= b` are compiled as LT/LE with swapped operands.
                OpCode::LT | OpCode::LE => {
                    let b = &self.stack[base + instr.b() as usize];
                    let c = &self.stack[base + instr.c() as usize];
                    let v = less(b, c, op == OpCode::LE)?;
                    self.stack[ra] = Value::Bool(v);
                }
                OpCode::NOT => {
                    let v = match &self.stack[base + instr.b() as usize] {
                        Value::Bool(b) => !b,
                        other => return Err(RuntimeError::not_needs_bool(other)),
                    };
                    self.stack[ra] = Value::Bool(v);
                }
                // Lua's `TEST A k`: if R[A] is not the bool B, skip the next
                // instruction (always a JMP). So the JMP runs when R[A] == B.
                OpCode::TEST => {
                    let want = instr.b() != 0;
                    match &self.stack[ra] {
                        Value::Bool(v) => {
                            if *v != want {
                                self.frame_mut().pc += 1;
                            }
                        }
                        other => return Err(RuntimeError::condition_not_bool(None, other)),
                    }
                }
                OpCode::JMP => {
                    let frame = self.frame_mut();
                    // Relative to the next instruction (pc was already advanced).
                    let target = frame.pc as i64 + i64::from(instr.sj_offset());
                    if target < 0 || target as usize > proto.code.len() {
                        return Err(internal("jump target out of range"));
                    }
                    frame.pc = target as usize;
                }
                OpCode::ASSERT => match &self.stack[ra] {
                    Value::Bool(true) => {}
                    Value::Bool(false) => {
                        // The CLI appends the asserted source text to the
                        // message; the VM has no source.
                        let sides =
                            (instr.b() == 1).then(|| (&self.stack[ra + 1], &self.stack[ra + 2]));
                        return Err(RuntimeError::assertion_failed(sides));
                    }
                    other => {
                        return Err(RuntimeError::condition_not_bool(Some("assert"), other));
                    }
                },
                // R[A] = [R[B], …, R[B+C-1]] (M8). The element registers are
                // temporaries, so their values are moved, not cloned.
                OpCode::NEWLIST => {
                    let first = base + instr.b() as usize;
                    let items = self
                        .stack
                        .get_mut(first..first + instr.c() as usize)
                        .ok_or_else(|| internal("register out of range"))?
                        .iter_mut()
                        .map(|v| std::mem::replace(v, Value::Unit))
                        .collect();
                    self.stack[ra] = Value::List(Rc::new(items));
                }
                // R[A] = R[B][R[C]] (M8): a list and an int in 0..len.
                OpCode::INDEX => {
                    let b = &self.stack[base + instr.b() as usize];
                    let c = &self.stack[base + instr.c() as usize];
                    let v = index(b, c)?;
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
                self.trace_instr(&proto, at, instr, ra)?;
            }
        }
    }

    /// Call the debug hook if the next instruction starts a source line.
    fn debug_line(&mut self) -> Result<(), RuntimeError> {
        // RUST: the borrows below are of *different fields* of `self`
        // (`hook` mutably, `frames`/`stack`/`globals`/`lines` read-only),
        // which Rust allows inside one function body.
        let (Some(hook), Some(lines)) = (self.hook.as_mut(), self.lines.as_ref()) else {
            return Ok(());
        };
        let frame = self.frames.last().expect("execute without a frame");
        let Some(line) = lines.line_start(&frame.closure.proto, frame.pc) else {
            return Ok(());
        };
        let view = VmView {
            frames: &self.frames,
            stack: &self.stack,
            globals: &self.globals,
            global_names: &self.global_names,
            global_builtin: &self.global_builtin,
            native_marks: &self.native_marks,
            at_error: false,
        };
        match hook.on_line(&view, line) {
            Resume::Continue => Ok(()),
            Resume::Abort => Err(RuntimeError::interrupted()),
        }
    }

    /// Show a runtime error to an attached debugger while every frame and
    /// register is still intact: `unwind` runs only after the hook returns.
    /// An `Interrupted` error came from the debugger itself and is not shown.
    fn debug_error(&mut self, err: &RuntimeError) {
        if err.kind == ErrorKind::Interrupted {
            return;
        }
        let Some(hook) = self.hook.as_mut() else {
            return;
        };
        let view = VmView {
            frames: &self.frames,
            stack: &self.stack,
            globals: &self.globals,
            global_names: &self.global_names,
            global_builtin: &self.global_builtin,
            native_marks: &self.native_marks,
            at_error: true,
        };
        hook.on_error(&view, err);
    }

    /// The running frame.
    /// Where the running code is: the innermost frame's function and the
    /// span of the instruction it is executing (for a native: the call).
    /// The `Err` built-in records it as the error's origin (M7).
    pub fn current_site(&self) -> Site {
        match self.frames.last() {
            Some(f) => Site {
                fn_name: Rc::clone(&f.closure.proto.name),
                span: f
                    .closure
                    .proto
                    .spans
                    .get(f.pc.saturating_sub(1))
                    .copied()
                    .unwrap_or_default(),
            },
            None => Site {
                fn_name: "<main>".into(),
                span: SrcSpan::default(),
            },
        }
    }

    fn frame_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("execute without a frame")
    }

    /// Finish the running frame with result `v`. Returns `Some(v)` when that
    /// frame was `<main>` (the program is done).
    /// A frame entered through `call_value` also returns `Some(v)`: that
    /// ends the nested dispatch loop, and `call_value` hands `v` to the
    /// built-in that asked for it.
    fn do_return(&mut self, v: Value) -> Option<Value> {
        let done = self.frames.pop().expect("RETURN without a frame");
        if done.reentry {
            // Drop the callee slot and the frame's registers.
            self.stack.truncate(done.base - 1);
            return Some(v);
        }
        let Some(caller) = self.frames.last() else {
            return Some(v);
        };
        // The result replaces the callee in its function slot, which is the
        // caller's `R[A]` of the CALL.
        self.stack[done.base - 1] = v;
        // Drop the callee's registers (and the values they hold).
        let end = caller.base + caller.closure.proto.num_regs as usize;
        self.stack.truncate(end);
        None
    }

    /// `TAILCALL A B` (M5): call `R[A]` and return its result, reusing the
    /// running frame instead of pushing one, so a loop written as tail
    /// recursion runs in constant stack space.
    ///
    /// ```text
    ///   before:  [ f | R0 … R[A]=g  args… ]      frame of f, base
    ///   after:   [ g | args…  (Missing…)  ]      same frame, now running g
    /// ```
    ///
    /// A built-in callee is simply called, then the frame returns its result.
    /// Returns `Some(result)` when `<main>` finished (only via a native).
    fn tail_call(&mut self, ra: usize, nargs: usize) -> Result<Option<Value>, RuntimeError> {
        let c = match &self.stack[ra] {
            Value::Closure(c) => Rc::clone(c),
            Value::Native(_) => {
                self.call(ra, nargs)?;
                let v = std::mem::replace(&mut self.stack[ra], Value::Unit);
                return Ok(self.do_return(v));
            }
            other => return Err(RuntimeError::not_callable(other)),
        };
        let arity = c.proto.arity as usize;
        check_arg_count(&c.proto.name, c.proto.required as usize, arity, nargs)?;
        let base = self.frame_mut().base;
        let top = base + c.proto.num_regs as usize;
        if top > MAX_STACK {
            return Err(stack_overflow());
        }
        // Move callee and arguments down over the function slot and the
        // first registers (destination is always below the source).
        for i in 0..=nargs {
            let v = std::mem::replace(&mut self.stack[ra + i], Value::Unit);
            self.stack[base - 1 + i] = v;
        }
        // Drop the old function's other registers, then size the window.
        self.stack.truncate(base + nargs);
        self.stack.resize(top, Value::Unit);
        for slot in &mut self.stack[base + nargs..base + arity] {
            *slot = Value::Missing;
        }
        let frame = self.frame_mut();
        frame.closure = c;
        frame.pc = 0;
        frame.tail_calls = frame.tail_calls.saturating_add(1);
        Ok(None)
    }

    /// `CALL A B`: call `R[A]` with the `nargs` arguments `R[A+1..=A+nargs]`.
    ///
    /// - valo function: push a frame whose window starts at `A+1`; the loop
    ///   continues in the callee and `RETURN` comes back here.
    /// - built-in: move the arguments off the stack, run it, store the result
    ///   in `R[A]`.
    fn call(&mut self, ra: usize, nargs: usize) -> Result<(), RuntimeError> {
        match &self.stack[ra] {
            Value::Closure(c) => {
                let c = Rc::clone(c);
                let arity = c.proto.arity as usize;
                check_arg_count(&c.proto.name, c.proto.required as usize, arity, nargs)?;
                if self.frames.len() >= MAX_FRAMES {
                    return Err(stack_overflow());
                }
                let base = ra + 1;
                let top = base + c.proto.num_regs as usize;
                if top > MAX_STACK {
                    return Err(stack_overflow());
                }
                if self.stack.len() < top {
                    self.stack.resize(top, Value::Unit);
                }
                // Omitted trailing (defaulted) parameters: the prologue
                // computes their defaults (M4b).
                for slot in &mut self.stack[base + nargs..base + arity] {
                    *slot = Value::Missing;
                }
                self.frames.push(Frame {
                    closure: c,
                    base,
                    pc: 0,
                    tail_calls: 0,
                    reentry: false,
                });
                Ok(())
            }
            &Value::Native(id) => {
                let def = NATIVES
                    .get(id.0 as usize)
                    .ok_or_else(|| internal("native index out of range"))?;
                check_arg_count(def.name, def.required(), def.params.len(), nargs)?;
                // Owned arguments: moved out, `Unit` left behind (plan D12).
                let mut args: Vec<Value> = self.stack[ra + 1..=ra + nargs]
                    .iter_mut()
                    .map(|v| std::mem::replace(v, Value::Unit))
                    .collect();
                // Omitted keyword parameters arrive as `Missing` (M4b).
                args.resize(def.params.len(), Value::Missing);
                // Remember the running built-in for stack traces of errors
                // in its callbacks; on error the mark stays for `unwind`.
                self.native_marks.push((id, self.frames.len()));
                let result = (def.f)(self, args)?;
                self.native_marks.pop();
                self.stack[ra] = result;
                Ok(())
            }
            other => Err(RuntimeError::not_callable(other)),
        }
    }

    /// Call the function value `f` with `args` from inside a built-in
    /// (`map`, `filter`, …, plan D7) and return its result.
    ///
    /// The callee and arguments go on top of the stack like a `CALL`'s
    /// window; a valo function then runs in a *nested* dispatch loop that
    /// ends when that frame returns (`Frame::reentry`). Tail calls inside
    /// the callback work as usual. Errors come back as `Err` with the frames
    /// still in place, so the trace shows the callback, `<native map>` and
    /// the caller. Nesting deeper than `MAX_REENTRY` is R0301.
    pub fn call_value(&mut self, f: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
        let ra = self.stack.len();
        let nargs = args.len();
        if ra + 1 + nargs > MAX_STACK {
            return Err(stack_overflow());
        }
        let is_closure = matches!(f, Value::Closure(_));
        self.stack.push(f);
        self.stack.extend(args);
        if !is_closure {
            // A built-in runs right away; anything else is R0202.
            self.call(ra, nargs)?;
            let v = std::mem::replace(&mut self.stack[ra], Value::Unit);
            self.stack.truncate(ra);
            return Ok(v);
        }
        if self.reentry_depth >= MAX_REENTRY {
            return Err(RuntimeError::reentry_overflow(MAX_REENTRY));
        }
        self.call(ra, nargs)?;
        self.frame_mut().reentry = true;
        self.reentry_depth += 1;
        let result = self.execute();
        self.reentry_depth -= 1;
        result
    }

    /// Write program output (used by natives such as `print`).
    pub fn write_stdout(&mut self, s: &str) -> Result<(), RuntimeError> {
        self.host.write_stdout(s).map_err(host_io)
    }

    /// The host, for natives that perform effects (`read_line`,
    /// `read_file`, …, M7). File errors are values (`Err`), so natives
    /// handle the `io::Result` themselves.
    pub fn host(&mut self) -> &mut dyn Host {
        &mut *self.host
    }

    /// One `--trace-exec` line: `[<main>] 0003 ADD 0 0 1 | R[0]=17`.
    fn trace_instr(
        &mut self,
        proto: &Proto,
        pc: usize,
        instr: Instr,
        ra: usize,
    ) -> Result<(), RuntimeError> {
        let mut line = format!("[{}] {pc:04} {instr:?}", proto.name);
        let writes_a = instr.op().is_ok_and(|op| {
            op.format() != Format::SJ
                && !matches!(
                    op,
                    OpCode::ECHO
                        | OpCode::RETURN
                        | OpCode::SETGLOBAL
                        | OpCode::TEST
                        | OpCode::ASSERT
                        | OpCode::JMPIFSET
                )
        });
        if writes_a {
            if let Some(v) = self.stack.get(ra) {
                line.push_str(&format!(" | R[{}]={}", instr.a(), v.repr()));
            }
        }
        line.push('\n');
        self.host.write_stderr(&line).map_err(host_io)
    }
}

// Short names for the error constructors used most in `execute`.

fn internal(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::internal(msg)
}

/// R0301.
fn stack_overflow() -> RuntimeError {
    RuntimeError::stack_overflow(MAX_FRAMES)
}

/// R0207 unless `required <= nargs <= arity`.
fn check_arg_count(
    name: &str,
    required: usize,
    arity: usize,
    nargs: usize,
) -> Result<(), RuntimeError> {
    if (required..=arity).contains(&nargs) {
        return Ok(());
    }
    Err(RuntimeError::wrong_arg_count(name, required, arity, nargs))
}

fn host_io(e: std::io::Error) -> RuntimeError {
    RuntimeError::output_failed(&e)
}

/// Binary arithmetic (D8–D10). Type-directed: int×int and float×float.
/// Mixed int/float is an error (R0201), with one exception: true division
/// `/` converts int operands to float and always returns a float (D9).
///
pub fn arith(op: OpCode, b: &Value, c: &Value) -> Result<Value, RuntimeError> {
    // RUST: matching on a tuple of two references checks both types at once.
    match (b, c) {
        (Value::Int(x), Value::Int(y)) if op == OpCode::DIV => {
            float_arith(op, *x as f64, *y as f64, b, c).map(Value::Float)
        }
        (Value::Int(x), Value::Int(y)) => int_arith(op, *x, *y).map(Value::Int),
        (Value::Float(x), Value::Float(y)) => float_arith(op, *x, *y, b, c).map(Value::Float),
        // `+` joins two strings (D11).
        (Value::Str(x), Value::Str(y)) if op == OpCode::ADD => {
            let mut out = String::with_capacity(x.len() + y.len());
            out.push_str(x);
            out.push_str(y);
            Ok(Value::Str(out.into()))
        }
        // `+` joins two lists into a new one (D11). The operands are
        // registers that may still be read, so neither is reused in place.
        (Value::List(x), Value::List(y)) if op == OpCode::ADD => {
            let mut out = Vec::with_capacity(x.len() + y.len());
            out.extend(x.iter().cloned());
            out.extend(y.iter().cloned());
            Ok(Value::List(Rc::new(out)))
        }
        (Value::Int(x), Value::Float(y)) if op == OpCode::DIV => {
            float_arith(op, *x as f64, *y, b, c).map(Value::Float)
        }
        (Value::Float(x), Value::Int(y)) if op == OpCode::DIV => {
            float_arith(op, *x, *y as f64, b, c).map(Value::Float)
        }
        _ => Err(RuntimeError::operand_types(symbol(op), b, c)),
    }
}

/// Float arithmetic: IEEE results (overflow gives `inf`, `inf - inf` gives
/// `NaN`), Python floor semantics for `//` and `%`, and R0102 for a zero
/// divisor (Q20). `b` and `c` are the original operands, for messages.
pub fn float_arith(op: OpCode, x: f64, y: f64, b: &Value, c: &Value) -> Result<f64, RuntimeError> {
    if matches!(op, OpCode::DIV | OpCode::IDIV | OpCode::MOD) && y == 0.0 {
        return Err(RuntimeError::division_by_zero(symbol(op), b, c));
    }
    Ok(match op {
        OpCode::ADD => x + y,
        OpCode::SUB => x - y,
        OpCode::MUL => x * y,
        OpCode::DIV => x / y,
        OpCode::IDIV => (x / y).floor(),
        OpCode::MOD => {
            // Rust's `%` keeps the dividend's sign; Python's the divisor's.
            let r = x % y;
            if r == 0.0 {
                0.0_f64.copysign(y)
            } else if (r < 0.0) != (y < 0.0) {
                r + y
            } else {
                r
            }
        }
        OpCode::POW => x.powf(y),
        _ => return Err(internal(format!("{op} is not a float operation"))),
    })
}

/// `==` (D10): same-type comparison only. Floats compare by IEEE rules, so
/// `NaN == NaN` is false. Different types → R0201.
/// EXTEND (M7): `none` may be compared with any type; strings, lists, Ok/Err.
pub fn values_equal(b: &Value, c: &Value) -> Result<bool, RuntimeError> {
    Ok(match (b, c) {
        (Value::Unit, Value::Unit) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        // Lists: same length and equal elements, in order (M8). Comparing
        // elements of different types is an error, as it is outside lists.
        (Value::List(x), Value::List(y)) => {
            if x.len() != y.len() {
                return Ok(false);
            }
            for (a, b) in x.iter().zip(y.iter()) {
                if !values_equal(a, b)? {
                    return Ok(false);
                }
            }
            true
        }
        // Any value may be compared with `none`: that is how an optional
        // is tested (plan §2.4).
        (Value::None, other) | (other, Value::None) => matches!(other, Value::None),
        (Value::Ok(x), Value::Ok(y)) => return values_equal(x, y),
        (Value::Err(x), Value::Err(y)) => return values_equal(&x.payload, &y.payload),
        (Value::Ok(_), Value::Err(_)) | (Value::Err(_), Value::Ok(_)) => false,
        _ if b.type_name() == "function" && c.type_name() == "function" => {
            return Err(RuntimeError::functions_not_comparable());
        }
        _ => return Err(RuntimeError::compare_types(b, c)),
    })
}

/// `<` (or `<=` when `or_equal`): ints, floats, strings (by code point,
/// M7). Bools and
/// `()` have no order (R0205); different types → R0201. Any comparison with
/// `NaN` is false.
pub fn less(b: &Value, c: &Value, or_equal: bool) -> Result<bool, RuntimeError> {
    match (b, c) {
        (Value::Int(x), Value::Int(y)) => Ok(if or_equal { x <= y } else { x < y }),
        (Value::Float(x), Value::Float(y)) => Ok(if or_equal { x <= y } else { x < y }),
        (Value::Str(x), Value::Str(y)) => Ok(if or_equal { x <= y } else { x < y }),
        _ if b.type_name() == c.type_name() => Err(RuntimeError::not_ordered(b)),
        _ => Err(RuntimeError::compare_types(b, c)),
    }
}

/// `xs[i]` (M8): a list and an int index in `0..len` (no negative
/// indexing in the MVP). Out of range → R0401 with the index and length
/// (`RuntimeError::access`, so the CLI can name the access path).
pub fn index(b: &Value, c: &Value) -> Result<Value, RuntimeError> {
    match (b, c) {
        (Value::List(xs), Value::Int(i)) => usize::try_from(*i)
            .ok()
            .and_then(|i| xs.get(i))
            .cloned()
            .ok_or_else(|| RuntimeError::index_out_of_range(*i, xs.len())),
        (Value::List(_), other) => Err(RuntimeError::index_type(other)),
        (other, _) => Err(RuntimeError::not_indexable(other)),
    }
}

/// A `<native map>` line of a stack trace.
fn native_entry(id: NativeId) -> TraceEntry {
    let name = NATIVES.get(id.0 as usize).map_or("?", |n| n.name);
    TraceEntry {
        fn_name: format!("<native {name}>").into(),
        span: SrcSpan::default(),
        tail_calls: 0,
        native: true,
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
    let ovf = || RuntimeError::int_overflow(&format!("{x} {sym} {y}"));
    match op {
        OpCode::ADD => x.checked_add(y).ok_or_else(ovf),
        OpCode::SUB => x.checked_sub(y).ok_or_else(ovf),
        OpCode::MUL => x.checked_mul(y).ok_or_else(ovf),
        // RUST: `if y == 0` after the pattern is a *guard*: this arm applies
        // only when the condition holds; otherwise matching continues below.
        OpCode::IDIV | OpCode::MOD if y == 0 => Err(RuntimeError::division_by_zero(
            sym,
            &Value::Int(x),
            &Value::Int(0),
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
                return Err(RuntimeError::negative_exponent(x, y));
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
    use crate::chunk::{Const, ProtoBuilder, UpvalSrc};
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
    fn move_and_globals_round_trip() {
        // let g = 7; R1 = g; R2 = R1
        let p = ProtoBuilder::new("<main>", 3)
            .global("g")
            .op(Instr::asbx(OpCode::LOADI, 0, 7))
            .op(Instr::abx(OpCode::SETGLOBAL, 0, 0))
            .op(Instr::abx(OpCode::GETGLOBAL, 1, 0))
            .op(Instr::abc(OpCode::MOVE, 2, 1, 0))
            .op(Instr::abc(OpCode::RETURN, 2, 0, 0))
            .program();
        assert_eq!(run(&p).0, Ok(Value::Int(7)));
    }

    #[test]
    fn unset_global_is_r0302() {
        let p = ProtoBuilder::new("<main>", 1)
            .global("late")
            .op(Instr::abx(OpCode::GETGLOBAL, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        let err = run(&p).0.unwrap_err();
        assert_eq!(err.kind.code(), "R0302");
        assert!(err.message.contains("`late`"), "{}", err.message);
    }

    /// M6: `CLOSURE` copies captures from a register, from the creator's own
    /// captures and the creator itself; `GETUPVAL`/`GETSELF` read them.
    #[test]
    fn closures_capture_by_value() {
        // inner: R0 = captures[0] + captures[1]; returns it.
        let inner = ProtoBuilder::new("inner", 2)
            .upval("x", UpvalSrc::Reg(0))
            .upval("y", UpvalSrc::Upval(0))
            .op(Instr::abc(OpCode::GETUPVAL, 0, 0, 0))
            .op(Instr::abc(OpCode::GETUPVAL, 1, 1, 0))
            .op(Instr::abc(OpCode::ADD, 0, 0, 1))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .build();
        // mid (captures y = 30 from main's R0): R0 = 12; R1 = closure(inner);
        // R0 = 99 (the copy keeps 12); return R1().
        let mid = ProtoBuilder::new("mid", 3)
            .upval("y", UpvalSrc::Reg(0))
            .proto(inner)
            .op(Instr::asbx(OpCode::LOADI, 0, 12))
            .op(Instr::abx(OpCode::CLOSURE, 1, 0))
            .op(Instr::asbx(OpCode::LOADI, 0, 99))
            .op(Instr::abc(OpCode::CALL, 1, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 1, 0, 0))
            .build();
        let p = ProtoBuilder::new("<main>", 2)
            .proto(mid)
            .op(Instr::asbx(OpCode::LOADI, 0, 30))
            .op(Instr::abx(OpCode::CLOSURE, 1, 0))
            .op(Instr::abc(OpCode::CALL, 1, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 1, 0, 0))
            .program();
        assert_eq!(run(&p).0, Ok(Value::Int(42)));
    }

    #[test]
    fn getself_and_current_capture_are_the_running_closure() {
        // g returns its capture, which is f's closure (UpvalSrc::Current).
        let g = ProtoBuilder::new("g", 1)
            .upval("f", UpvalSrc::Current)
            .op(Instr::abc(OpCode::GETUPVAL, 0, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .build();
        // f: R0 = closure(g); return R0(), i.e. f itself.
        let f = ProtoBuilder::new("f", 1)
            .proto(g)
            .op(Instr::abx(OpCode::CLOSURE, 0, 0))
            .op(Instr::abc(OpCode::CALL, 0, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .build();
        // h: return GETSELF.
        let h = ProtoBuilder::new("h", 1)
            .op(Instr::abc(OpCode::GETSELF, 0, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .build();
        for (child, name) in [(f, "f"), (h, "h")] {
            let p = ProtoBuilder::new("<main>", 1)
                .proto(child)
                .op(Instr::abx(OpCode::CLOSURE, 0, 0))
                .op(Instr::abc(OpCode::CALL, 0, 0, 0))
                .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
                .program();
            match run(&p).0 {
                Ok(Value::Closure(c)) => assert_eq!(&*c.proto.name, name),
                other => panic!("{other:?}"),
            }
        }
    }

    /// M7: CONCAT joins the Display form of its parts; `+`, `==` and `<` on
    /// strings.
    #[test]
    fn strings_concat_compare_and_add() {
        let p = ProtoBuilder::new("<main>", 4)
            .konst(Const::Str("n=".into()))
            .konst(Const::Str("é".into()))
            .op(Instr::abx(OpCode::LOADK, 1, 0))
            .op(Instr::asbx(OpCode::LOADI, 2, 7))
            .op(Instr::abx(OpCode::LOADK, 3, 1))
            .op(Instr::abc(OpCode::CONCAT, 0, 1, 3))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        assert_eq!(run(&p).0, Ok(Value::Str("n=7é".into())));
        let (a, b) = (Value::Str("ab".into()), Value::Str("b".into()));
        assert_eq!(arith(OpCode::ADD, &a, &b), Ok(Value::Str("abb".into())));
        assert!(less(&a, &b, false).unwrap());
        assert!(!values_equal(&a, &b).unwrap());
        let e = arith(OpCode::ADD, &a, &Value::Int(1)).unwrap_err();
        assert_eq!(e.kind.code(), "R0201");
        assert!(e.notes[0].contains("interpolation"), "{:?}", e.notes);
        assert_eq!(Value::Str("a\"{".into()).repr(), "\"a\\\"\\{\"");
    }

    /// Index of a built-in by name.
    fn native(name: &str) -> u16 {
        crate::builtins::native_id(name).expect("known native").0
    }

    /// M7: `fail()` returns `Err("bad")` (made by the `Err` built-in);
    /// `mid()` does `fail()?`; `<main>` calls `mid()` and applies TRY with
    /// flag `c`. The Err records its origin and every `?` on its way.
    fn try_program(c: u8) -> Program {
        let fail = ProtoBuilder::new("fail", 2)
            .global("Err")
            .konst(Const::Str("bad".into()))
            .op(Instr::abx(OpCode::GETGLOBAL, 0, 0))
            .op(Instr::abx(OpCode::LOADK, 1, 0))
            .op(Instr::abc(OpCode::CALL, 0, 1, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .build();
        let mid = ProtoBuilder::new("mid", 1)
            .op(Instr::abx(OpCode::GETGLOBAL, 0, 1))
            .op(Instr::abc(OpCode::CALL, 0, 0, 0))
            .op(Instr::abc(OpCode::TRY, 0, 0, 0))
            .op(Instr::asbx(OpCode::LOADI, 0, 99)) // never reached
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .build();
        ProtoBuilder::new("<main>", 1)
            .native_global(native("Err"))
            .global("fail")
            .global("mid")
            .proto(fail)
            .proto(mid)
            .op(Instr::abx(OpCode::CLOSURE, 0, 0))
            .op(Instr::abx(OpCode::SETGLOBAL, 0, 1))
            .op(Instr::abx(OpCode::CLOSURE, 0, 1))
            .op(Instr::abx(OpCode::SETGLOBAL, 0, 2))
            .op(Instr::abx(OpCode::GETGLOBAL, 0, 2))
            .op(Instr::abc(OpCode::CALL, 0, 0, 0))
            .op(Instr::abc(OpCode::TRY, 0, 0, c))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program()
    }

    #[test]
    fn try_propagates_err_and_reports_it_at_the_top() {
        for c in [0, 1] {
            let e = run(&try_program(c)).0.unwrap_err();
            assert_eq!(e.kind.code(), "R0601");
            assert_eq!(e.message, "unhandled Err: \"bad\"");
            let trail = e.trail.expect("trail");
            assert_eq!(&*trail.origin.fn_name, "fail");
            let sites: Vec<&str> = trail.propagated.iter().map(|s| &*s.fn_name).collect();
            assert_eq!(sites, ["mid", "<main>"]);
        }
    }

    #[test]
    fn try_unwraps_ok_and_checks_its_operand() {
        let prog = |load: Instr, c: u8| {
            ProtoBuilder::new("<main>", 2)
                .native_global(native("Ok"))
                .op(Instr::abx(OpCode::GETGLOBAL, 0, 0))
                .op(load)
                .op(Instr::abc(OpCode::CALL, 0, 1, 0))
                .op(Instr::abc(OpCode::TRY, 0, 0, 0))
                .op(Instr::abc(OpCode::MOVE, 1, 0, 0))
                .op(Instr::abc(OpCode::TRY, 1, 1, c))
                .op(Instr::abc(OpCode::RETURN, 1, 0, 0))
                .program()
        };
        // Ok(7)? = 7; then 7? passes only when lenient.
        let seven = Instr::asbx(OpCode::LOADI, 1, 7);
        assert_eq!(run(&prog(seven, 1)).0, Ok(Value::Int(7)));
        let e = run(&prog(seven, 0)).0.unwrap_err();
        assert_eq!(e.kind.code(), "R0208");
        assert_eq!(e.message, "`?` needs a result (Ok or Err), got int (7)");
        // Ok(none)? = none; none? is R0208 with a hint.
        let none = Instr::abc(OpCode::LOADNONE, 1, 0, 0);
        let e = run(&prog(none, 0)).0.unwrap_err();
        assert_eq!(e.message, "`?` needs a result (Ok or Err), got none");
        assert!(e.notes[0].contains("or_else"), "{:?}", e.notes);
    }

    #[test]
    fn none_compares_with_anything_and_results_structurally() {
        let ok = |v| Value::Ok(Rc::new(v));
        assert!(values_equal(&Value::None, &Value::None).unwrap());
        assert!(!values_equal(&Value::None, &Value::Int(0)).unwrap());
        assert!(!values_equal(&Value::Str("".into()), &Value::None).unwrap());
        assert!(values_equal(&ok(Value::Int(1)), &ok(Value::Int(1))).unwrap());
        assert!(!values_equal(&ok(Value::Int(1)), &ok(Value::Int(2))).unwrap());
        assert_eq!(ok(Value::Str("a".into())).to_string(), "Ok(\"a\")");
        assert_eq!(Value::None.to_string(), "none");
    }

    /// `fn add(a, b) = a + b` as a prototype.
    fn add_proto() -> Proto {
        ProtoBuilder::new("add", 3)
            .arity(2)
            .op(Instr::abc(OpCode::ADD, 2, 0, 1))
            .op(Instr::abc(OpCode::RETURN, 2, 0, 0))
            .build()
    }

    /// `<main>` calling `add` with `nargs` arguments (2, 3, 4, …).
    fn call_add(nargs: u8) -> Program {
        let mut b = ProtoBuilder::new("<main>", 6)
            .proto(add_proto())
            .op(Instr::abx(OpCode::CLOSURE, 0, 0));
        for i in 0..nargs {
            b = b.op(Instr::asbx(OpCode::LOADI, i + 1, i16::from(i) + 2));
        }
        b.op(Instr::abc(OpCode::CALL, 0, nargs, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program()
    }

    #[test]
    fn call_and_return() {
        let p = call_add(2);
        let mut host = TestHost::default();
        let mut vm = Vm::new(&mut host, Options::default());
        assert_eq!(vm.run(&p), Ok(Value::Int(5)));
        // The callee's registers were dropped on return.
        assert_eq!(vm.stack.len(), 1 + 6);
    }

    #[test]
    fn wrong_arg_count_and_not_callable() {
        let e = run(&call_add(1)).0.unwrap_err();
        assert_eq!(e.kind.code(), "R0207");
        assert_eq!(e.message, "`add` takes 2 arguments but 1 was given");
        let p = ProtoBuilder::new("<main>", 2)
            .op(Instr::asbx(OpCode::LOADI, 0, 1))
            .op(Instr::abc(OpCode::CALL, 0, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        let e = run(&p).0.unwrap_err();
        assert_eq!(e.kind.code(), "R0202");
        assert_eq!(e.message, "cannot call int (1): it is not a function");
    }

    #[test]
    fn native_print_and_its_arg_count() {
        let print = |nargs: u8| {
            ProtoBuilder::new("<main>", 4)
                .native_global(0)
                .op(Instr::abx(OpCode::GETGLOBAL, 0, 0))
                .op(Instr::asbx(OpCode::LOADI, 1, 7))
                .op(Instr::asbx(OpCode::LOADI, 2, 8))
                .op(Instr::asbx(OpCode::LOADI, 3, 9))
                .op(Instr::abc(OpCode::CALL, 0, nargs, 0))
                .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
                .program()
        };
        let (r, host) = run(&print(1));
        assert_eq!(r, Ok(Value::Unit));
        assert_eq!(host.stdout, "7\n");
        // `end` (M7) must be a str.
        let e = run(&print(2)).0.unwrap_err();
        assert_eq!(e.message, "print() expects a str, got int (8)");
        let e = run(&print(3)).0.unwrap_err();
        assert_eq!(e.kind.code(), "R0207");
    }

    #[test]
    fn trace_lists_every_frame() {
        // <main> → outer → inner, which divides by zero.
        let inner = ProtoBuilder::new("inner", 2)
            .op(Instr::asbx(OpCode::LOADI, 0, 1))
            .op(Instr::asbx(OpCode::LOADI, 1, 0))
            .op(Instr::abc(OpCode::IDIV, 0, 0, 1))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .build();
        let outer = ProtoBuilder::new("outer", 1)
            .global("inner")
            .op(Instr::abx(OpCode::GETGLOBAL, 0, 0))
            .op(Instr::abc(OpCode::CALL, 0, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .build();
        let p = ProtoBuilder::new("<main>", 1)
            .global("inner")
            .global("outer")
            .proto(inner)
            .proto(outer)
            .op(Instr::abx(OpCode::CLOSURE, 0, 0))
            .op(Instr::abx(OpCode::SETGLOBAL, 0, 0))
            .op(Instr::abx(OpCode::CLOSURE, 0, 1))
            .op(Instr::abx(OpCode::SETGLOBAL, 0, 1))
            .op(Instr::abc(OpCode::CALL, 0, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        let e = run(&p).0.unwrap_err();
        assert_eq!(e.kind.code(), "R0102");
        let names: Vec<&str> = e.trace.iter().map(|t| &*t.fn_name).collect();
        assert_eq!(names, ["inner", "outer", "<main>"]);
    }

    #[test]
    fn functions_display_and_do_not_compare() {
        let f = Value::Closure(Rc::new(Closure::new(Rc::new(add_proto()))));
        assert_eq!(f.to_string(), "<fn add/2>");
        assert_eq!(
            Value::Native(crate::chunk::NativeId(0)).to_string(),
            "<native print/2>"
        );
        assert_eq!(values_equal(&f, &f).unwrap_err().kind.code(), "R0205");
    }

    #[test]
    fn omitted_defaults_are_missing_until_the_prologue() {
        // fn f(a, b = 10) = a + b   — prologue: JMPIFSET 1 +1; LOADI 1 10
        let f = ProtoBuilder::new("f", 3)
            .arity(2)
            .required(1)
            .op(Instr::asbx(OpCode::JMPIFSET, 1, 1))
            .op(Instr::asbx(OpCode::LOADI, 1, 10))
            .op(Instr::abc(OpCode::ADD, 2, 0, 1))
            .op(Instr::abc(OpCode::RETURN, 2, 0, 0))
            .build();
        let call = |nargs: u8| {
            ProtoBuilder::new("<main>", 4)
                .proto(f.clone())
                .op(Instr::abx(OpCode::CLOSURE, 0, 0))
                .op(Instr::asbx(OpCode::LOADI, 1, 1))
                .op(Instr::asbx(OpCode::LOADI, 2, 2))
                .op(Instr::abc(OpCode::CALL, 0, nargs, 0))
                .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
                .program()
        };
        assert_eq!(run(&call(1)).0, Ok(Value::Int(11)));
        assert_eq!(run(&call(2)).0, Ok(Value::Int(3)));
        let e = run(&call(0)).0.unwrap_err();
        assert_eq!(e.message, "`f` takes 1 to 2 arguments but 0 were given");
        // LOADMISSING in the caller has the same effect as omitting.
        let p = ProtoBuilder::new("<main>", 4)
            .proto(f)
            .op(Instr::abx(OpCode::CLOSURE, 0, 0))
            .op(Instr::asbx(OpCode::LOADI, 1, 5))
            .op(Instr::abc(OpCode::LOADMISSING, 2, 0, 0))
            .op(Instr::abc(OpCode::CALL, 0, 2, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        assert_eq!(run(&p).0, Ok(Value::Int(15)));
    }

    #[test]
    fn tail_calls_run_in_constant_stack() {
        // fn count(n) = if n == 0 then 7 else count(n - 1)   (tail call)
        let count = ProtoBuilder::new("count", 4)
            .arity(1)
            .global("count")
            .op(Instr::asbx(OpCode::LOADI, 1, 0))
            .op(Instr::abc(OpCode::EQ, 1, 0, 1))
            .op(Instr::abc(OpCode::TEST, 1, 0, 0))
            .op(Instr::sj(OpCode::JMP, 2))
            .op(Instr::asbx(OpCode::LOADI, 1, 7))
            .op(Instr::abc(OpCode::RETURN, 1, 0, 0))
            .op(Instr::abx(OpCode::GETGLOBAL, 1, 0))
            .op(Instr::asbx(OpCode::LOADI, 3, 1))
            .op(Instr::abc(OpCode::SUB, 2, 0, 3))
            .op(Instr::abc(OpCode::TAILCALL, 1, 1, 0))
            .build();
        let p = ProtoBuilder::new("<main>", 2)
            .global("count")
            .proto(count)
            .op(Instr::abx(OpCode::CLOSURE, 0, 0))
            .op(Instr::abx(OpCode::SETGLOBAL, 0, 0))
            .op(Instr::asbx(OpCode::LOADI, 1, 1000))
            .op(Instr::abc(OpCode::CALL, 0, 1, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        let mut host = TestHost::default();
        let mut vm = Vm::new(&mut host, Options::default());
        assert_eq!(vm.run(&p), Ok(Value::Int(7)));
        // 1000 tail calls reused one frame: only main's window is left, and
        // the frame count never grew (a CALL would have pushed 1001 frames).
        assert_eq!(vm.stack.len(), 1 + 2, "callee window dropped on return");
    }

    #[test]
    fn tail_call_keeps_one_frame_and_counts_elisions() {
        // f(n) = if n == 0 then 1 // 0 else f(n - 1), called with 5: the
        // error trace has one `f` frame that made 5 tail calls.
        let f = ProtoBuilder::new("f", 4)
            .arity(1)
            .global("f")
            .op(Instr::asbx(OpCode::LOADI, 1, 0))
            .op(Instr::abc(OpCode::EQ, 1, 0, 1))
            .op(Instr::abc(OpCode::TEST, 1, 0, 0))
            .op(Instr::sj(OpCode::JMP, 3))
            .op(Instr::asbx(OpCode::LOADI, 1, 1))
            .op(Instr::abc(OpCode::IDIV, 1, 1, 0))
            .op(Instr::abc(OpCode::RETURN, 1, 0, 0))
            .op(Instr::abx(OpCode::GETGLOBAL, 1, 0))
            .op(Instr::asbx(OpCode::LOADI, 3, 1))
            .op(Instr::abc(OpCode::SUB, 2, 0, 3))
            .op(Instr::abc(OpCode::TAILCALL, 1, 1, 0))
            .build();
        let p = ProtoBuilder::new("<main>", 2)
            .global("f")
            .proto(f)
            .op(Instr::abx(OpCode::CLOSURE, 0, 0))
            .op(Instr::abx(OpCode::SETGLOBAL, 0, 0))
            .op(Instr::asbx(OpCode::LOADI, 1, 5))
            .op(Instr::abc(OpCode::CALL, 0, 1, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        let e = run(&p).0.unwrap_err();
        assert_eq!(e.kind.code(), "R0102");
        let frames: Vec<(&str, u32)> = e
            .trace
            .iter()
            .map(|t| (&*t.fn_name, t.tail_calls))
            .collect();
        assert_eq!(frames, [("f", 5), ("<main>", 0)]);
    }

    #[test]
    fn tail_call_to_a_native_returns_its_result() {
        // fn show(x) = print(x)   (tail call to a native), called from main.
        let show = ProtoBuilder::new("show", 3)
            .arity(1)
            .op(Instr::abx(OpCode::GETGLOBAL, 1, 0))
            .op(Instr::abc(OpCode::MOVE, 2, 0, 0))
            .op(Instr::abc(OpCode::TAILCALL, 1, 1, 0))
            .build();
        let p = ProtoBuilder::new("<main>", 2)
            .native_global(0)
            .proto(show)
            .op(Instr::abx(OpCode::CLOSURE, 0, 0))
            .op(Instr::asbx(OpCode::LOADI, 1, 9))
            .op(Instr::abc(OpCode::CALL, 0, 1, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        let (r, host) = run(&p);
        assert_eq!(r, Ok(Value::Unit));
        assert_eq!(host.stdout, "9\n");
    }

    fn float(x: f64) -> Value {
        Value::Float(x)
    }

    #[test]
    fn float_arithmetic_matches_python() {
        let f = |op, x: f64, y: f64| arith(op, &float(x), &float(y)).unwrap();
        assert_eq!(f(OpCode::ADD, 0.1, 0.2), float(0.30000000000000004));
        assert_eq!(f(OpCode::IDIV, 7.5, 2.0), float(3.0));
        assert_eq!(f(OpCode::IDIV, -7.5, 2.0), float(-4.0));
        assert_eq!(f(OpCode::MOD, -7.5, 2.0), float(0.5));
        assert_eq!(f(OpCode::MOD, 7.5, -2.0), float(-0.5));
        assert_eq!(f(OpCode::POW, 2.0, 0.5), float(2f64.sqrt()));
        assert_eq!(f(OpCode::MUL, 1e308, 10.0), float(f64::INFINITY));
        // True division converts ints (D9).
        let div = |b: Value, c: Value| arith(OpCode::DIV, &b, &c).unwrap();
        assert_eq!(div(Value::Int(7), Value::Int(2)), float(3.5));
        assert_eq!(div(Value::Int(6), Value::Int(3)), float(2.0));
        assert_eq!(div(Value::Int(1), float(4.0)), float(0.25));
        assert_eq!(div(float(1.0), Value::Int(4)), float(0.25));
    }

    #[test]
    fn float_zero_divisor_and_mixing_are_errors() {
        for op in [OpCode::DIV, OpCode::IDIV, OpCode::MOD] {
            let e = arith(op, &float(1.0), &float(0.0)).unwrap_err();
            assert_eq!(e.kind, ErrorKind::DivisionByZero, "{op}");
        }
        let e = arith(OpCode::DIV, &Value::Int(1), &Value::Int(0)).unwrap_err();
        assert_eq!(e.message, "division by zero: 1 / 0");
        let e = arith(OpCode::ADD, &Value::Int(1), &float(2.0)).unwrap_err();
        assert_eq!(e.kind, ErrorKind::TypeMismatch);
        assert_eq!(e.notes.len(), 1, "int/float mix carries a hint");
        let e = arith(OpCode::ADD, &Value::Bool(true), &Value::Int(1)).unwrap_err();
        assert!(e.notes.is_empty());
    }

    #[test]
    fn comparisons() {
        use Value::{Bool, Int, Unit};
        assert_eq!(values_equal(&Int(1), &Int(1)), Ok(true));
        assert_eq!(values_equal(&Bool(true), &Bool(false)), Ok(false));
        assert_eq!(values_equal(&Unit, &Unit), Ok(true));
        assert_eq!(values_equal(&float(f64::NAN), &float(f64::NAN)), Ok(false));
        assert_eq!(less(&Int(1), &Int(2), false), Ok(true));
        assert_eq!(less(&Int(2), &Int(2), false), Ok(false));
        assert_eq!(less(&Int(2), &Int(2), true), Ok(true));
        assert_eq!(less(&float(f64::NAN), &float(1.0), true), Ok(false));
        let kind = |r: Result<bool, RuntimeError>| r.unwrap_err().kind;
        assert_eq!(
            kind(values_equal(&Int(1), &float(1.0))),
            ErrorKind::TypeMismatch
        );
        assert_eq!(
            kind(values_equal(&Int(1), &Bool(true))),
            ErrorKind::TypeMismatch
        );
        assert_eq!(
            kind(less(&Bool(true), &Bool(false), false)),
            ErrorKind::NotComparable
        );
        assert_eq!(kind(less(&Unit, &Unit, true)), ErrorKind::NotComparable);
        assert_eq!(
            kind(less(&Int(1), &float(2.0), false)),
            ErrorKind::TypeMismatch
        );
    }

    #[test]
    fn test_and_jmp_branch() {
        // R0 = 3 < 5; if R0 { R1 = 10 } else { R1 = 20 }; return R1
        let program = |a: i16, b: i16| {
            ProtoBuilder::new("<main>", 3)
                .op(Instr::asbx(OpCode::LOADI, 0, a))
                .op(Instr::asbx(OpCode::LOADI, 1, b))
                .op(Instr::abc(OpCode::LT, 0, 0, 1))
                .op(Instr::abc(OpCode::TEST, 0, 0, 0))
                .op(Instr::sj(OpCode::JMP, 2)) // to the else branch
                .op(Instr::asbx(OpCode::LOADI, 1, 10))
                .op(Instr::sj(OpCode::JMP, 1)) // to the end
                .op(Instr::asbx(OpCode::LOADI, 1, 20))
                .op(Instr::abc(OpCode::RETURN, 1, 0, 0))
                .program()
        };
        assert_eq!(run(&program(3, 5)).0, Ok(Value::Int(10)));
        assert_eq!(run(&program(5, 3)).0, Ok(Value::Int(20)));
    }

    #[test]
    fn test_requires_a_bool() {
        let p = ProtoBuilder::new("<main>", 1)
            .op(Instr::asbx(OpCode::LOADI, 0, 1))
            .op(Instr::abc(OpCode::TEST, 0, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        assert_eq!(run(&p).0.unwrap_err().kind.code(), "R0203");
    }

    #[test]
    fn assert_passes_and_fails() {
        let assert_prog = |a: i16, b: i16, with_sides: u8| {
            ProtoBuilder::new("<main>", 3)
                .op(Instr::asbx(OpCode::LOADI, 1, a))
                .op(Instr::asbx(OpCode::LOADI, 2, b))
                .op(Instr::abc(OpCode::EQ, 0, 1, 2))
                .op(Instr::abc(OpCode::ASSERT, 0, with_sides, 0))
                .op(Instr::abc(OpCode::LOADUNIT, 0, 0, 0))
                .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
                .program()
        };
        assert_eq!(run(&assert_prog(4, 4, 1)).0, Ok(Value::Unit));
        let e = run(&assert_prog(4, 5, 1)).0.unwrap_err();
        assert_eq!(e.kind.code(), "R0701");
        assert_eq!(e.notes, ["left: 4", "right: 5"]);
        assert!(run(&assert_prog(4, 5, 0)).0.unwrap_err().notes.is_empty());
        let p = ProtoBuilder::new("<main>", 1)
            .op(Instr::asbx(OpCode::LOADI, 0, 1))
            .op(Instr::abc(OpCode::ASSERT, 0, 0, 0))
            .program();
        assert_eq!(run(&p).0.unwrap_err().kind.code(), "R0203");
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

    // ---- M8: lists, re-entry from built-ins ---------------------------------

    fn ints(xs: &[i64]) -> Value {
        Value::List(Rc::new(xs.iter().map(|&n| Value::Int(n)).collect()))
    }

    /// `[10, 20, 30][i]` for an index loaded by `load` into R4.
    fn index_program(load: Instr) -> Program {
        ProtoBuilder::new("<main>", 5)
            .op(Instr::asbx(OpCode::LOADI, 1, 10))
            .op(Instr::asbx(OpCode::LOADI, 2, 20))
            .op(Instr::asbx(OpCode::LOADI, 3, 30))
            .op(Instr::abc(OpCode::NEWLIST, 0, 1, 3))
            .op(load)
            .op(Instr::abc(OpCode::INDEX, 0, 0, 4))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program()
    }

    #[test]
    fn newlist_and_index() {
        assert_eq!(
            run(&index_program(Instr::asbx(OpCode::LOADI, 4, 2))).0,
            Ok(Value::Int(30))
        );
        for (i, msg) in [
            (3, "index 3 out of range (len 3)"),
            (-1, "index -1 out of range (len 3)"),
        ] {
            let e = run(&index_program(Instr::asbx(OpCode::LOADI, 4, i)))
                .0
                .unwrap_err();
            assert_eq!(e.kind.code(), "R0401");
            assert_eq!(e.message, msg);
            let a = e.access.expect("access");
            assert_eq!((a.index.as_str(), a.len), (i.to_string().as_str(), 3));
        }
        let e = run(&index_program(Instr::abc(OpCode::LOADNONE, 4, 0, 0)))
            .0
            .unwrap_err();
        assert_eq!(e.kind.code(), "R0201");
        assert_eq!(e.message, "list index must be an int, not none");
        // An empty NEWLIST.
        let p = ProtoBuilder::new("<main>", 1)
            .op(Instr::abc(OpCode::NEWLIST, 0, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        assert_eq!(run(&p).0, Ok(ints(&[])));
    }

    #[test]
    fn lists_add_compare_and_display() {
        let (a, b) = (ints(&[1, 2]), ints(&[3]));
        assert_eq!(arith(OpCode::ADD, &a, &b), Ok(ints(&[1, 2, 3])));
        assert_eq!(arith(OpCode::SUB, &a, &b).unwrap_err().kind.code(), "R0201");
        assert!(values_equal(&a, &ints(&[1, 2])).unwrap());
        assert!(!values_equal(&a, &b).unwrap());
        let mixed = Value::List(Rc::new(vec![Value::Float(1.0)]));
        assert_eq!(values_equal(&b, &mixed).unwrap_err().kind.code(), "R0201");
        assert_eq!(less(&a, &b, false).unwrap_err().kind.code(), "R0205");
        let nested = Value::List(Rc::new(vec![a, Value::Str("x".into()), ints(&[])]));
        assert_eq!(nested.to_string(), "[[1, 2], \"x\", []]");
        assert_eq!(nested.type_name(), "list");
    }

    /// `<main>`: `map([1, 2, 3], f)` with `f` = the prototype `child`.
    fn map_program(child: Proto) -> Program {
        ProtoBuilder::new("<main>", 6)
            .native_global(native("map"))
            .proto(child)
            .op(Instr::abx(OpCode::GETGLOBAL, 0, 0))
            .op(Instr::asbx(OpCode::LOADI, 3, 1))
            .op(Instr::asbx(OpCode::LOADI, 4, 2))
            .op(Instr::asbx(OpCode::LOADI, 5, 3))
            .op(Instr::abc(OpCode::NEWLIST, 1, 3, 3))
            .op(Instr::abx(OpCode::CLOSURE, 2, 0))
            .op(Instr::abc(OpCode::CALL, 0, 2, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program()
    }

    #[test]
    fn map_calls_back_into_bytecode() {
        let double = ProtoBuilder::new("double", 1)
            .arity(1)
            .op(Instr::abc(OpCode::ADD, 0, 0, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .build();
        let (r, host) = run(&map_program(double));
        assert_eq!(r, Ok(ints(&[2, 4, 6])));
        assert_eq!(host.stdout, "");
    }

    #[test]
    fn tail_calls_inside_a_callback_return_to_the_builtin() {
        // f(x) = str(x)   — a tail call to a built-in, from a callback;
        // g(x) = add(x, x) — a tail call to a closure (global slot 1).
        let f = ProtoBuilder::new("f", 3)
            .arity(1)
            .op(Instr::abx(OpCode::GETGLOBAL, 1, 1))
            .op(Instr::abc(OpCode::MOVE, 2, 0, 0))
            .op(Instr::abc(OpCode::TAILCALL, 1, 1, 0))
            .build();
        let mut p = map_program(f);
        p.globals.push(crate::chunk::GlobalDesc {
            name: "str".into(),
            kind: GlobalKind::Native(NativeId(native("str"))),
        });
        let strs = |xs: &[&str]| {
            Value::List(Rc::new(
                xs.iter().map(|s| Value::Str((*s).into())).collect(),
            ))
        };
        assert_eq!(run(&p).0, Ok(strs(&["1", "2", "3"])));
        let g = ProtoBuilder::new("g", 4)
            .arity(1)
            .proto(add_proto())
            .op(Instr::abx(OpCode::CLOSURE, 1, 0))
            .op(Instr::abc(OpCode::MOVE, 2, 0, 0))
            .op(Instr::abc(OpCode::MOVE, 3, 0, 0))
            .op(Instr::abc(OpCode::TAILCALL, 1, 2, 0))
            .build();
        assert_eq!(run(&map_program(g)).0, Ok(ints(&[2, 4, 6])));
    }

    #[test]
    fn errors_in_callbacks_show_the_builtin_in_the_trace() {
        // f(x) = x // 0
        let f = ProtoBuilder::new("f", 2)
            .arity(1)
            .op(Instr::asbx(OpCode::LOADI, 1, 0))
            .op(Instr::abc(OpCode::IDIV, 0, 0, 1))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .build();
        let e = run(&map_program(f)).0.unwrap_err();
        assert_eq!(e.kind.code(), "R0102");
        let names: Vec<_> = e.trace.iter().map(|t| (&*t.fn_name, t.native)).collect();
        assert_eq!(
            names,
            [("f", false), ("<native map>", true), ("<main>", false)]
        );
        // A built-in that fails itself is not listed: the call is the place.
        let p = ProtoBuilder::new("<main>", 3)
            .native_global(native("map"))
            .op(Instr::abx(OpCode::GETGLOBAL, 0, 0))
            .op(Instr::asbx(OpCode::LOADI, 1, 1))
            .op(Instr::asbx(OpCode::LOADI, 2, 1))
            .op(Instr::abc(OpCode::CALL, 0, 2, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        let e = run(&p).0.unwrap_err();
        assert_eq!(e.message, "map() expects a list, got int (1)");
        assert_eq!(e.trace.len(), 1);
    }

    /// `f(x) = map([x], f)` called with 1: recurses through `map` forever.
    fn endless_map_recursion() -> Program {
        let f = ProtoBuilder::new("f", 4)
            .arity(1)
            .op(Instr::abx(OpCode::GETGLOBAL, 1, 0))
            .op(Instr::abc(OpCode::MOVE, 3, 0, 0))
            .op(Instr::abc(OpCode::NEWLIST, 2, 3, 1))
            .op(Instr::abc(OpCode::GETSELF, 3, 0, 0))
            .op(Instr::abc(OpCode::CALL, 1, 2, 0))
            .op(Instr::abc(OpCode::RETURN, 1, 0, 0))
            .build();
        ProtoBuilder::new("<main>", 2)
            .native_global(native("map"))
            .proto(f)
            .op(Instr::abx(OpCode::CLOSURE, 0, 0))
            .op(Instr::asbx(OpCode::LOADI, 1, 1))
            .op(Instr::abc(OpCode::CALL, 0, 1, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program()
    }

    #[test]
    fn recursion_through_builtins_is_capped() {
        // On a thread with the stack the CLI gives the VM (test threads get
        // 2 MiB). `Rc` values cannot cross threads, so only plain data comes
        // back.
        let (code, message, depth) = std::thread::Builder::new()
            .stack_size(VM_STACK_BYTES)
            .spawn(|| {
                let e = run(&endless_map_recursion()).0.unwrap_err();
                (e.kind.code(), e.message, e.trace.len())
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(code, "R0301");
        assert!(message.contains("built-ins"), "{message}");
        // <main>, the first f, 200 re-entered f frames and their 200 marks.
        assert_eq!(depth, 2 * MAX_REENTRY + 2);
    }

    /// `reduce(xs, 0, add)` on a list built by NEWLIST.
    fn reduce_sum(xs: &[i64]) -> Result<Value, RuntimeError> {
        let mut b = ProtoBuilder::new("<main>", 4 + xs.len() as u8)
            .native_global(native("reduce"))
            .proto(add_proto());
        for (i, &x) in xs.iter().enumerate() {
            b = b
                .konst(Const::Int(x))
                .op(Instr::abx(OpCode::LOADK, 4 + i as u8, i as u16));
        }
        let p = b
            .op(Instr::abx(OpCode::GETGLOBAL, 0, 0))
            .op(Instr::abc(OpCode::NEWLIST, 1, 4, xs.len() as u8))
            .op(Instr::asbx(OpCode::LOADI, 2, 0))
            .op(Instr::abx(OpCode::CLOSURE, 3, 0))
            .op(Instr::abc(OpCode::CALL, 0, 3, 0))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .program();
        run(&p).0
    }

    proptest! {
        #[test]
        fn reduce_add_is_the_sum(xs in prop::collection::vec(-1_000_000i64..1_000_000, 0..60)) {
            prop_assert_eq!(reduce_sum(&xs), Ok(Value::Int(xs.iter().sum())));
        }
    }
}
