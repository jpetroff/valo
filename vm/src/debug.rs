//! # Debugging support (`debug.rs`)
//!
//! **What this file is:** the *mechanism* a debugger needs from the VM, with
//! no debugging *policy* in it. A debugger (`tooling/dap`) installs a
//! [`DebugHook`]; the interpreter loop calls it before the first instruction
//! of every source line it executes, with a read-only [`VmView`] of the call
//! stack. The hook decides whether to stop (a breakpoint, a pause request);
//! stopping is simply *not returning* until the user continues. This is
//! Lua's design (`lua_sethook` with `LUA_MASKLINE`, `ldebug.c`) and Python's
//! (`sys.settrace`).
//!
//! **Pipeline position:** used by `vm.rs` (`Vm::set_hook`, `Vm::debug_line`)
//! when a hook is installed; never used by `valo run`.
//!
//! **Textbook chapter:** `docs/13-debugger.md` (§13.5).
//!
//! ## Where a line starts
//!
//! Every instruction has a source span (`Proto::spans`). The **line start**
//! of line *L* in a function is the *first* instruction (lowest pc) of that
//! function whose span is on line *L*:
//!
//! ```text
//!   0004  [6:10] GETGLOBAL 0 4   ; x      ← line start of 6
//!   0005  [7:5]  LOADI     1 4            ← line start of 7 (inside `(x +\n 4)`)
//!   0006  [6:9]  ADD       0 0 1          ← line 6 again: NOT a line start
//! ```
//!
//! Using only the first instruction means a statement spread over several
//! lines reports each line once, and a breakpoint fires once per execution
//! of its line, however the codegen orders the instructions. Every line with
//! code has exactly one line start per function that has code on it; that
//! set of lines is where breakpoints can go (`LineTable::lines`).
//!
//! A later refinement is a per-instruction "statement start" flag set by
//! codegen (DWARF's `is_stmt`), so stepping never stops mid-expression.
//!
//! ## Where to change things
//!
//! | I want to…                                   | Look at                                   |
//! |----------------------------------------------|-------------------------------------------|
//! | a new hook event (function entry, …)          | `trait DebugHook` + the call site in `vm.rs` (`debug_line`, `debug_error`) |
//! | show more of a paused VM                     | `VmView` (`locals`, `globals`)              |
//! | change where breakpoints may be placed       | `LineTable::new`, `LineTable::resolve`     |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `trait` with a `&mut self` method — the hook keeps its own state (*Traits, impl and derive*).
//! - `HashMap<*const Proto, …>` — a raw pointer used only as an identity key,
//!   never dereferenced (no `unsafe`).
//! - `BTreeSet::range` — the first element `>=` a value, in sorted order.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use crate::chunk::{Program, Proto, SrcSpan};
use crate::error::RuntimeError;
use crate::value::Value;
use crate::vm::Frame;

/// What the program does after the hook returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Go on running.
    Continue,
    /// Stop the program: `Vm::run` returns an `R0801` (interrupted) error.
    Abort,
}

/// Called by the interpreter loop while a debugger is attached.
///
/// The hook runs on the VM's thread, inside `Vm::execute`. Blocking in it
/// pauses the program; the `VmView` stays valid while it blocks.
pub trait DebugHook {
    /// Before the first instruction of a source line (1-based `line`) runs.
    /// `vm.frame(0)` is the frame about to run it.
    fn on_line(&mut self, vm: &VmView<'_>, line: u32) -> Resume;

    /// A runtime error, *before* the call stack is unwound: frames,
    /// registers and globals are as they were when the failing instruction
    /// ran, and `vm.frame(0)` points at that instruction. Blocking here lets
    /// the user look around; when the hook returns, the error ends the run
    /// as usual. Not called for `Interrupted` (the debugger's own abort).
    fn on_error(&mut self, vm: &VmView<'_>, err: &RuntimeError) {
        let _ = (vm, err);
    }
}

/// Read-only window into a paused VM.
pub struct VmView<'a> {
    pub(crate) frames: &'a [Frame],
    pub(crate) stack: &'a [Value],
    pub(crate) globals: &'a [Option<Value>],
    pub(crate) global_names: &'a [Rc<str>],
    pub(crate) global_builtin: &'a [bool],
    /// Called from `on_error`: the innermost frame's `pc` is already past
    /// the failing instruction.
    pub(crate) at_error: bool,
}

/// A named value, as the Variables view shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    /// Source name.
    pub name: Rc<str>,
    /// Current value.
    pub value: Value,
}

/// One active call, as a debugger shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameInfo {
    /// Function display name (`<main>`, `fact`).
    pub name: Rc<str>,
    /// Span of the instruction this frame is at: for the innermost frame the
    /// one about to run, for the others the call they are waiting on.
    pub span: SrcSpan,
    /// Calls this frame replaced through tail calls (M5).
    pub tail_calls: u32,
}

impl VmView<'_> {
    /// Number of active frames (`<main>` counts).
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// Frame `i` (innermost first) and the pc it is at: for the innermost
    /// frame the instruction about to run (`pc`) or, at an error, the one
    /// that failed; for a caller the CALL it waits on (its `pc` is already
    /// past it: `pc - 1`).
    fn frame_at(&self, i: usize) -> Option<(&Frame, usize)> {
        let f = self
            .frames
            .len()
            .checked_sub(i + 1)
            .map(|k| &self.frames[k])?;
        // At an error the innermost frame has also moved past its
        // (failing) instruction.
        let pc = if i == 0 && !self.at_error {
            f.pc
        } else {
            f.pc.saturating_sub(1)
        };
        Some((f, pc))
    }

    /// Frame `i`, innermost first (`0` = running now).
    pub fn frame(&self, i: usize) -> Option<FrameInfo> {
        let (f, pc) = self.frame_at(i)?;
        Some(FrameInfo {
            name: Rc::clone(&f.closure.proto.name),
            span: f.closure.proto.spans.get(pc).copied().unwrap_or_default(),
            tail_calls: f.tail_calls,
        })
    }

    /// The named locals of frame `i` that hold a value at its current pc
    /// (`Proto::locals`), in declaration order (parameters first). Of
    /// several live locals with one name, only the latest (the one that
    /// shadows the others) is returned.
    pub fn locals(&self, i: usize) -> Vec<Variable> {
        let Some((f, pc)) = self.frame_at(i) else {
            return Vec::new();
        };
        let live: Vec<_> = f
            .closure
            .proto
            .locals
            .iter()
            .filter(|l| l.live_at(pc))
            .collect();
        live.iter()
            .enumerate()
            .filter(|(k, l)| !live[k + 1..].iter().any(|later| later.name == l.name))
            .filter_map(|(_, l)| {
                let value = self.stack.get(f.base + l.reg as usize)?.clone();
                Some(Variable {
                    name: Rc::clone(&l.name),
                    value,
                })
            })
            .collect()
    }

    /// Top-level bindings (`let`, `fn`) that have a value, in slot order;
    /// built-ins are left out. A top-level name defined twice has two slots;
    /// only the later one with a value is returned.
    pub fn globals(&self) -> Vec<Variable> {
        let set: Vec<(&Rc<str>, &Value)> = self
            .globals
            .iter()
            .zip(self.global_names)
            .zip(self.global_builtin)
            .filter(|(_, builtin)| !**builtin)
            .filter_map(|((value, name), _)| Some((name, value.as_ref()?)))
            .collect();
        set.iter()
            .enumerate()
            .filter(|(k, (name, _))| !set[k + 1..].iter().any(|(later, _)| later == name))
            .map(|(_, (name, value))| Variable {
                name: Rc::clone(name),
                value: (*value).clone(),
            })
            .collect()
    }
}

/// For every function of a program: which instructions start a source line.
#[derive(Debug, Default)]
pub struct LineTable {
    /// Per prototype (by address): line number at each pc, `0` if that pc
    /// does not start a line.
    starts: HashMap<*const Proto, Vec<u32>>,
    /// Every line that has a line start somewhere.
    lines: BTreeSet<u32>,
}

impl LineTable {
    /// Walk `main` and every nested function.
    pub fn new(program: &Program) -> LineTable {
        let mut table = LineTable::default();
        let mut todo: Vec<&Rc<Proto>> = vec![&program.main];
        while let Some(proto) = todo.pop() {
            let mut seen = BTreeSet::new();
            let starts = proto
                .spans
                .iter()
                .map(|span| {
                    let (line, _) = program.line_col(span.start);
                    if seen.insert(line) {
                        table.lines.insert(line);
                        line
                    } else {
                        0
                    }
                })
                .collect();
            table.starts.insert(Rc::as_ptr(proto), starts);
            todo.extend(proto.protos.iter());
        }
        table
    }

    /// The line that instruction `pc` of `proto` starts, if any.
    pub fn line_start(&self, proto: &Proto, pc: usize) -> Option<u32> {
        let line = *self.starts.get(&(proto as *const Proto))?.get(pc)?;
        (line != 0).then_some(line)
    }

    /// All lines with code, in order.
    pub fn lines(&self) -> &BTreeSet<u32> {
        &self.lines
    }

    /// Where a breakpoint requested on `line` goes: that line if it has
    /// code, else the next line that has (`None` past the last one).
    pub fn resolve(&self, line: u32) -> Option<u32> {
        self.lines.range(line..).next().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{ProtoBuilder, line_starts};
    use crate::opcode::{Instr, OpCode};

    /// A `<main>` whose instructions sit on the given lines of a 9-line text.
    fn program_on_lines(lines: &[u32]) -> Program {
        let text = "a\nb\nc\nd\ne\nf\ng\nh\ni\n";
        let ls = line_starts(text);
        let mut b = ProtoBuilder::new("<main>", 1);
        for i in 0..lines.len() {
            let op = if i + 1 == lines.len() {
                OpCode::RETURN
            } else {
                OpCode::LOADUNIT
            };
            b = b.op(Instr::abc(op, 0, 0, 0));
        }
        let mut p = b.program();
        let main = Rc::get_mut(&mut p.main).unwrap();
        main.spans = lines
            .iter()
            .map(|l| SrcSpan::new(ls[*l as usize - 1], ls[*l as usize - 1] + 1))
            .collect();
        p.line_starts = ls;
        p
    }

    #[test]
    fn first_instruction_of_each_line_starts_it() {
        let p = program_on_lines(&[1, 1, 3, 4, 3, 5]);
        let t = LineTable::new(&p);
        let starts: Vec<Option<u32>> = (0..6).map(|pc| t.line_start(&p.main, pc)).collect();
        assert_eq!(starts, [Some(1), None, Some(3), Some(4), None, Some(5)]);
        assert_eq!(t.lines().iter().copied().collect::<Vec<_>>(), [1, 3, 4, 5]);
    }

    /// Records every line it is called on; aborts on `abort_at`.
    struct Recorder {
        lines: Vec<(u32, usize, Rc<str>)>,
        abort_at: Option<u32>,
    }

    impl DebugHook for Recorder {
        fn on_line(&mut self, vm: &VmView<'_>, line: u32) -> Resume {
            let top = vm.frame(0).expect("a frame");
            self.lines.push((line, vm.depth(), top.name));
            if self.abort_at == Some(line) {
                Resume::Abort
            } else {
                Resume::Continue
            }
        }
    }

    fn run_with(p: &Program, hook: &mut Recorder) -> Result<Value, crate::error::RuntimeError> {
        let mut host = crate::host::TestHost::default();
        let mut vm = crate::vm::Vm::new(&mut host, crate::vm::Options::default());
        vm.set_hook(hook);
        vm.run(p)
    }

    #[test]
    fn hook_runs_once_per_line_start() {
        let p = program_on_lines(&[1, 1, 3, 4, 3, 5]);
        let mut rec = Recorder {
            lines: Vec::new(),
            abort_at: None,
        };
        assert_eq!(run_with(&p, &mut rec), Ok(Value::Unit));
        let lines: Vec<u32> = rec.lines.iter().map(|l| l.0).collect();
        assert_eq!(lines, [1, 3, 4, 5]);
        assert!(rec.lines.iter().all(|l| l.1 == 1 && &*l.2 == "<main>"));
    }

    #[test]
    fn abort_stops_the_run_with_r0801() {
        let p = program_on_lines(&[1, 2, 3]);
        let mut rec = Recorder {
            lines: Vec::new(),
            abort_at: Some(2),
        };
        let err = run_with(&p, &mut rec).unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Interrupted);
        assert_eq!(err.kind.code(), "R0801");
        assert_eq!(rec.lines.len(), 2);
    }

    /// Sees errors: the code, the failing line, the depth, the frame's locals.
    #[derive(Default)]
    struct ErrorWatcher {
        seen: Vec<(&'static str, u32, usize, Vec<Variable>)>,
    }

    impl DebugHook for ErrorWatcher {
        fn on_line(&mut self, _vm: &VmView<'_>, _line: u32) -> Resume {
            Resume::Continue
        }
        fn on_error(&mut self, vm: &VmView<'_>, err: &crate::error::RuntimeError) {
            let span = vm.frame(0).expect("frames are intact").span;
            self.seen
                .push((err.kind.code(), span.start, vm.depth(), vm.locals(0)));
        }
    }

    #[test]
    fn on_error_sees_the_failing_instruction_before_unwinding() {
        // R0 = 7; R1 = 0; R0 = R0 // R1   (local `n` = R0 from pc 1)
        let mut p = ProtoBuilder::new("<main>", 2)
            .op(Instr::asbx(OpCode::LOADI, 0, 7))
            .op(Instr::asbx(OpCode::LOADI, 1, 0))
            .op(Instr::abc(OpCode::IDIV, 0, 0, 1))
            .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
            .local("n", 0, 1, 4)
            .program();
        Rc::get_mut(&mut p.main).unwrap().spans =
            (0..4).map(|i| SrcSpan::new(i * 10, i * 10 + 1)).collect();
        let mut w = ErrorWatcher::default();
        let mut host = crate::host::TestHost::default();
        let mut vm = crate::vm::Vm::new(&mut host, crate::vm::Options::default());
        vm.set_hook(&mut w);
        let err = vm.run(&p).unwrap_err();
        drop(vm);
        assert_eq!(err.kind.code(), "R0102");
        // Unwinding still happened afterwards: the trace is there.
        assert_eq!(err.trace.len(), 1);
        let (code, at, depth, locals) = &w.seen[0];
        assert_eq!((*code, *at, *depth), ("R0102", 20, 1));
        assert_eq!(locals[0].name.as_ref(), "n");
        assert_eq!(locals[0].value, Value::Int(7));
    }

    #[test]
    fn abort_is_not_reported_as_an_error() {
        let p = program_on_lines(&[1, 2]);
        let mut rec = Recorder {
            lines: Vec::new(),
            abort_at: Some(1),
        };
        // `Recorder` has the default `on_error`; the point is that the run
        // ends with R0801 and nothing else is called.
        assert_eq!(run_with(&p, &mut rec).unwrap_err().kind.code(), "R0801");
    }

    #[test]
    fn breakpoints_move_to_the_next_line_with_code() {
        let t = LineTable::new(&program_on_lines(&[1, 3, 7]));
        assert_eq!(t.resolve(1), Some(1));
        assert_eq!(t.resolve(2), Some(3));
        assert_eq!(t.resolve(4), Some(7));
        assert_eq!(t.resolve(8), None);
    }
}
