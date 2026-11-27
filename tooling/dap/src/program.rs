//! # The program thread and its debug hook (`program.rs`)
//!
//! **What this file is:** everything that runs on the thread of the valo
//! program. `run_program` compiles and runs it like `valo run`; when
//! debugging, it attaches `DapHook` to the VM. The VM calls the hook at
//! every line start (`valo_vm::debug`); the hook checks for breakpoints and
//! pause requests, and to *stop* it tells the session (`Input::Stopped`) and
//! then blocks, answering `Command`s until one lets the program go on.
//!
//! ```text
//!  session thread                         program thread (VM)
//!   │  setBreakpoints → Shared.breakpoints   │ on_line(view, 3): line 3 has a breakpoint
//!   │ ◀────────── Input::Stopped {breakpoint} │ … blocks on `cmds`
//!   │ ── Command::StackTrace {reply} ───────▶ │ reads VmView, replies with JSON
//!   │ ── Command::Continue ─────────────────▶ │ returns Resume::Continue
//!   │  pause → Shared.pause = true           │ next line start: Input::Stopped {pause}
//!   │ ── Command::Step(Over) ───────────────▶ │ remembers (Over, depth 2), runs on;
//!   │ ◀────────────── Input::Stopped {step} ── │ … stops at the next line start at depth ≤ 2
//!   │ ◀───────── Input::Stopped {exception} ── │ on_error: R0102, frames still intact
//! ```
//!
//! Values in the VM use `Rc`, which cannot cross threads, so the hook turns
//! everything the session asks for into JSON *here*, on the VM's thread.
//!
//! **Textbook chapter:** `docs/13-debugger.md` (§13.5). Roadmap:
//! `tooling/dap/README.md` (D1).
//!
//! ## Where to change things
//!
//! | I want to…                               | Look at                                        |
//! |------------------------------------------|------------------------------------------------|
//! | a new thing to ask a paused program       | a `Command` variant + an arm in `DapHook::wait` |
//! | how values look in the Variables view     | `variable_json`                                 |
//! | a new reason to stop                      | `DapHook::on_line`, `DapHook::on_error`         |
//! | what the exception widget says            | `DapHook::exception`                            |
//! | how stepping decides where to stop        | `Step`, `DapHook::step_done`                    |
//! | how frames look in the Call Stack view    | `DapHook::stack_trace`                          |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `AtomicBool` — a flag two threads may read and write without a lock.
//! - `Mutex<BTreeMap<…>>` — the breakpoint table, replaced by the session
//!   while the program runs.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use valo_compiler::span::{SourceMap, Span};
use valo_compiler::{CompileOptions, compile_source, diagnostic};
use valo_vm::debug::{DebugHook, Resume, Variable, VmView};
use valo_vm::error::{ErrorKind, RuntimeError};
use valo_vm::host::Host;
use valo_vm::value::Value as VmValue;
use valo_vm::vm::{Options as VmOptions, Vm};

use crate::host::DapHost;
use crate::protocol::Outbox;
use crate::session::Input;

/// Exit codes, as the `valo` CLI uses them (`docs/09-driver-and-cli.md`),
/// plus 130 for "stopped by the user" (the shell's code for Ctrl+C).
pub const EXIT_RUNTIME: i32 = 1;
pub const EXIT_COMPILE: i32 = 2;
pub const EXIT_INTERRUPTED: i32 = 130;

/// Requests from the session to a stopped program.
#[derive(Debug)]
pub enum Command {
    /// Run on.
    Continue,
    /// Describe frames `start..start + levels` (`levels == 0`: all) as a
    /// DAP `stackTrace` response body.
    StackTrace {
        start: usize,
        levels: usize,
        reply: Sender<Value>,
    },
    /// Run until the step is done (`next`, `stepIn`, `stepOut`).
    Step(StepKind),
    /// The children of a `variablesReference` (`GLOBALS_REF`,
    /// `locals_ref(frame)` or `captures_ref(frame)`) as a DAP `variables`
    /// response body.
    Variables {
        reference: i64,
        reply: Sender<Value>,
    },
    /// Look a name up in frame `frame` (`None`: globals only), then in the
    /// globals: a DAP `evaluate` response body, or `None` if not found.
    Evaluate {
        frame: Option<usize>,
        name: String,
        reply: Sender<Option<Value>>,
    },
    /// End the program (Stop button).
    Abort,
}

/// `variablesReference` of the Globals scope.
pub const GLOBALS_REF: i64 = 1;

/// `variablesReference` of the Locals scope of frame `i` (innermost = 0).
/// References only need to stay valid while the program is stopped.
pub fn locals_ref(frame: usize) -> i64 {
    2 + 2 * frame as i64
}

/// `variablesReference` of the Closure scope (captured values, M6) of
/// frame `i`.
pub fn captures_ref(frame: usize) -> i64 {
    3 + 2 * frame as i64
}

/// The three DAP steps. All of them stop at a *line start*
/// (`valo_vm::debug`); they differ in which call depths count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// `stepIn`: the next line start anywhere, entering calls.
    In,
    /// `next` (step over): the next line start in this frame or a caller.
    Over,
    /// `stepOut`: the next line start in a caller.
    Out,
}

/// A step in progress: its kind and the call depth when it began.
#[derive(Debug, Clone, Copy)]
struct Step {
    kind: StepKind,
    depth: usize,
}

/// The last line of `text` that is not blank (1-based; 0 for an empty
/// text). Code generation gives the implicit end of `<main>` (`LOADUNIT;
/// RETURN`) a span at the very end of the file, which is a line after
/// this one; the debugger never stops there or places breakpoints there.
pub fn last_code_line(text: &str) -> u32 {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .last()
        .map_or(0, |(i, _)| i as u32 + 1)
}

/// State the session changes while the program runs.
#[derive(Debug, Default)]
pub struct Shared {
    /// Breakpoints of the program's file: line → breakpoint id.
    pub breakpoints: Mutex<BTreeMap<u32, i64>>,
    /// Stop at the next line start (`pause`, `stopOnEntry`).
    pub pause: AtomicBool,
    /// End the program at the next line start (`terminate`, `disconnect`).
    pub abort: AtomicBool,
    /// Exception filter `runtime`: stop at runtime errors (except failed
    /// asserts). Off until `setExceptionBreakpoints` turns it on.
    pub break_on_error: AtomicBool,
    /// Exception filter `assert`: stop at failed `assert`s (R0701).
    pub break_on_assert: AtomicBool,
}

/// A runtime error the program stopped at, for the `stopped` event and the
/// `exceptionInfo` request (plain strings: they cross to the session thread).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exception {
    /// The error code, `R0102`.
    pub id: String,
    /// The message as `valo run` prints it, with notes (`left: …`).
    pub description: String,
}

/// What to run, and how.
pub struct Launch {
    /// Path as given in `launch` (used in error messages).
    pub name: String,
    /// The DAP `Source` object for stack frames: `{name, path}`.
    pub source: Value,
    /// Source text.
    pub text: String,
    /// `false` for Run Without Debugging: no hook, full speed.
    pub debug: bool,
    /// Pause before the first line (`stopOnEntry`).
    pub stop_on_entry: bool,
}

/// Compile and run the program, as `valo run` does, with output going to
/// the Debug Console. Returns the exit code.
///
/// It compiles again (`launch` compiled once to report errors early): a
/// compiled `Program` holds `Rc` pointers, so it must be built on the thread
/// that runs it.
pub fn run_program(
    launch: Launch,
    out: &Arc<Outbox>,
    shared: &Shared,
    tx: Sender<Input>,
    cmds: Receiver<Command>,
) -> i32 {
    let sm = SourceMap::new(launch.name, launch.text);
    let program = match compile_source(&sm, CompileOptions::default()) {
        Ok(p) => p,
        Err(diags) => {
            out.output("stderr", &diagnostic::render(&sm, &diags, false));
            return EXIT_COMPILE;
        }
    };
    shared.pause.store(launch.stop_on_entry, Ordering::SeqCst);
    let mut hook = DapHook {
        sm: &sm,
        source: launch.source,
        last_line: last_code_line(&sm.text),
        shared,
        tx,
        cmds,
        first_line: true,
        step: None,
    };
    let mut host = DapHost::new(Arc::clone(out));
    let mut vm = Vm::new(&mut host, VmOptions::default());
    if launch.debug {
        vm.set_hook(&mut hook);
    }
    let result = vm.run(&program);
    drop(vm);
    match result {
        Ok(_) => 0,
        Err(err) if err.kind == ErrorKind::Interrupted => EXIT_INTERRUPTED,
        Err(err) => {
            let _ = host.write_stderr(&diagnostic::render_runtime(&sm, &err));
            EXIT_RUNTIME
        }
    }
}

/// The debugger inside the VM.
struct DapHook<'a> {
    /// For character-based line/column of spans.
    sm: &'a SourceMap,
    source: Value,
    /// Lines after this one hold no code of the user (`last_code_line`).
    last_line: u32,
    shared: &'a Shared,
    /// To the session: `Input::Stopped`.
    tx: Sender<Input>,
    /// From the session, while stopped.
    cmds: Receiver<Command>,
    /// No line has run yet: a pause now is `stopOnEntry`.
    first_line: bool,
    /// The step in progress, if any.
    step: Option<Step>,
}

impl DebugHook for DapHook<'_> {
    fn on_line(&mut self, vm: &VmView<'_>, line: u32) -> Resume {
        if self.shared.abort.load(Ordering::SeqCst) {
            return Resume::Abort;
        }
        if line > self.last_line {
            return Resume::Continue; // the implicit end of `<main>`
        }
        let entry = std::mem::replace(&mut self.first_line, false);
        // Reasons in priority order. A breakpoint met while stepping over a
        // call wins, and any stop ends the step (as in every debugger).
        let breakpoint = self
            .shared
            .breakpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&line)
            .copied();
        let (reason, hit) = if self.shared.pause.swap(false, Ordering::SeqCst) {
            (if entry { "entry" } else { "pause" }, None)
        } else if let Some(id) = breakpoint {
            ("breakpoint", Some(id))
        } else if self.step_done(vm) {
            ("step", None)
        } else {
            return Resume::Continue;
        };
        self.step = None;
        let stopped = Input::Stopped {
            reason,
            hit,
            exception: None,
        };
        if self.tx.send(stopped).is_err() {
            return Resume::Abort; // session gone
        }
        self.wait(vm)
    }

    /// A runtime error, before unwinding (D4): stop if its exception filter
    /// is on. Whatever the user does next (continue, step, stop), the error
    /// then ends the program as usual.
    fn on_error(&mut self, vm: &VmView<'_>, err: &RuntimeError) {
        let filter = if err.kind == ErrorKind::AssertionFailed {
            &self.shared.break_on_assert
        } else {
            &self.shared.break_on_error
        };
        if self.shared.abort.load(Ordering::SeqCst) || !filter.load(Ordering::SeqCst) {
            return;
        }
        self.step = None;
        let stopped = Input::Stopped {
            reason: "exception",
            hit: None,
            exception: Some(self.exception(vm, err)),
        };
        if self.tx.send(stopped).is_ok() {
            self.wait(vm);
        }
    }
}

impl DapHook<'_> {
    /// The error as `valo run` prints its first lines: a failed `assert`
    /// quotes its source text (the span of the failing instruction), and
    /// notes such as `left: 4` follow on their own lines.
    fn exception(&self, vm: &VmView<'_>, err: &RuntimeError) -> Exception {
        let mut description = err.message.clone();
        if err.kind == ErrorKind::AssertionFailed {
            if let Some(f) = vm.frame(0) {
                let span = Span::new(f.span.start as usize, f.span.end as usize);
                if span.end as usize <= self.sm.text.len() {
                    description.push_str(": ");
                    description.push_str(self.sm.slice(span));
                }
            }
        }
        for note in &err.notes {
            description.push('\n');
            description.push_str(note);
        }
        Exception {
            id: err.kind.code().to_string(),
            description,
        }
    }

    /// Does this line start finish the step in progress?
    ///
    /// | step  | stops where the call depth is…  |
    /// |-------|---------------------------------|
    /// | In    | anything                        |
    /// | Over  | ≤ the depth when it began       |
    /// | Out   | < the depth when it began       |
    ///
    /// A tail call (M5) replaces its frame without changing the depth, so
    /// stepping over one stops in the callee: the caller is really gone.
    fn step_done(&self, vm: &VmView<'_>) -> bool {
        match self.step {
            None => false,
            Some(Step { kind, depth }) => match kind {
                StepKind::In => true,
                StepKind::Over => vm.depth() <= depth,
                StepKind::Out => vm.depth() < depth,
            },
        }
    }

    /// Stopped: serve the session until it lets the program go on.
    fn wait(&mut self, vm: &VmView<'_>) -> Resume {
        loop {
            match self.cmds.recv() {
                Ok(Command::Continue) => return Resume::Continue,
                Ok(Command::Step(kind)) => {
                    self.step = Some(Step {
                        kind,
                        depth: vm.depth(),
                    });
                    return Resume::Continue;
                }
                Ok(Command::Variables { reference, reply }) => {
                    let vars = match usize::try_from(reference - locals_ref(0)) {
                        _ if reference == GLOBALS_REF => vm.globals(),
                        Ok(k) if k % 2 == 0 => vm.locals(k / 2),
                        Ok(k) => vm.captures(k / 2),
                        Err(_) => Vec::new(),
                    };
                    let vars: Vec<Value> = vars.iter().map(variable_json).collect();
                    let _ = reply.send(json!({ "variables": vars }));
                }
                Ok(Command::Evaluate { frame, name, reply }) => {
                    let locals = frame.map(|i| vm.locals(i)).unwrap_or_default();
                    let captures = frame.map(|i| vm.captures(i)).unwrap_or_default();
                    let found = locals
                        .into_iter()
                        .chain(captures)
                        .chain(vm.globals())
                        .find(|v| *v.name == *name)
                        .map(|v| {
                            let j = variable_json(&v);
                            json!({ "result": j["value"], "type": j["type"], "variablesReference": 0 })
                        });
                    let _ = reply.send(found);
                }
                Ok(Command::StackTrace {
                    start,
                    levels,
                    reply,
                }) => {
                    let _ = reply.send(self.stack_trace(vm, start, levels));
                }
                // `Err`: the session hung up.
                Ok(Command::Abort) | Err(_) => return Resume::Abort,
            }
        }
    }

    /// A `stackTrace` response body. Frame `i` (innermost first) gets id
    /// `2i`; a frame that replaced others through tail calls is followed by
    /// a grey label frame (id `2i + 1`), as `valo run` prints
    /// "(... N tail calls elided ...)".
    fn stack_trace(&self, vm: &VmView<'_>, start: usize, levels: usize) -> Value {
        let want = if levels == 0 {
            usize::MAX
        } else {
            start.saturating_add(levels)
        };
        let mut shown = Vec::new();
        let mut total = 0;
        for i in 0..vm.depth() {
            let Some(f) = vm.frame(i) else { break };
            total += 1 + usize::from(f.tail_calls > 0);
            if shown.len() >= want {
                continue; // only counting now (deep recursion: 200 000 frames)
            }
            let (line, column) = self.sm.line_col(f.span.start);
            shown.push(json!({
                "id": 2 * i,
                "name": &*f.name,
                "source": self.source,
                "line": line,
                "column": column,
            }));
            if f.tail_calls > 0 {
                let s = if f.tail_calls == 1 { "" } else { "s" };
                shown.push(json!({
                    "id": 2 * i + 1,
                    "name": format!("… {} tail call{s} elided …", f.tail_calls),
                    "line": 0,
                    "column": 0,
                    "presentationHint": "label",
                }));
            }
        }
        let frames: Vec<Value> = shown.into_iter().skip(start).take(want - start).collect();
        json!({ "stackFrames": frames, "totalFrames": total })
    }
}

/// A DAP `Variable`: name, value as `print` shows it, type name. Values
/// have no children yet; lists and dicts (M8) will get a
/// `variablesReference` so the UI can expand them.
fn variable_json(v: &Variable) -> Value {
    let value = match &v.value {
        // An omitted parameter whose default the prologue has not computed.
        VmValue::Missing => "(default not computed yet)".to_string(),
        other => other.to_string(),
    };
    json!({
        "name": &*v.name,
        "value": value,
        "type": v.value.type_name(),
        "variablesReference": 0,
    })
}

#[cfg(test)]
mod tests {
    use super::last_code_line;

    #[test]
    fn last_code_line_skips_trailing_blank_lines() {
        assert_eq!(last_code_line(""), 0);
        assert_eq!(last_code_line("a\n"), 1);
        assert_eq!(last_code_line("a\nb"), 2);
        assert_eq!(last_code_line("a\n\n  \n"), 1);
    }
}
