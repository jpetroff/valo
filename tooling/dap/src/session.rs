//! # The debug session (`session.rs`)
//!
//! **What this file is:** the adapter's brain. It receives every DAP request
//! from the client, keeps the session state, and drives the program thread
//! (`program.rs`). Everything arrives through one channel (`Input`): client
//! messages from the stdin reader thread (`main.rs`), and "stopped" /
//! "finished" from the program thread, so this loop never waits on two
//! things at once and all protocol state changes happen here, in order.
//!
//! A session, as VS Code drives it:
//!
//! ```text
//!  VS Code                                         valo-dap (this file)
//!   │ ── initialize ──────────────────────────────▶ │  reply: capabilities()
//!   │ ── launch {program, stopOnEntry, noDebug} ──▶ │  read + compile; errors fail the launch
//!   │ ◀────────────────────────────── initialized ─ │  "send me your configuration"
//!   │ ── setBreakpoints {source, breakpoints} ────▶ │  verify: move to the next line with code
//!   │ ── setExceptionBreakpoints ─────────────────▶ │
//!   │ ── configurationDone ───────────────────────▶ │  start the program thread
//!   │ ◀─────────────────────────── output (print) ─ │  host.rs, while it runs
//!   │ ◀──────────── stopped {reason: breakpoint} ── │  Input::Stopped from the hook
//!   │ ── threads, stackTrace ─────────────────────▶ │  Command::StackTrace → program thread
//!   │ ── continue | pause | terminate ────────────▶ │
//!   │ ◀───────────────── exited {exitCode} ──────── │  Input::ProgramEnded
//!   │ ◀───────────────────────────── terminated ─── │
//!   │ ── disconnect ──────────────────────────────▶ │  reply, process exits
//! ```
//!
//! **Textbook chapter:** `docs/13-debugger.md`. Roadmap: `tooling/dap/README.md`.
//!
//! ## Where to change things
//!
//! | I want to…                                  | Look at                                       |
//! |---------------------------------------------|-----------------------------------------------|
//! | announce a new feature to VS Code           | `capabilities()` (e.g. `supportsConditionalBreakpoints`) |
//! | answer a new request                        | `Session::handle_request`, one arm per command |
//! | add a launch option (`args`, `cwd`)          | `Session::launch` + `package.json` `configurationAttributes` |
//! | ask a stopped program something new (D3)     | `program::Command` + `Session::ask`            |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `std::sync::mpsc` channels — a queue between threads; `Sender` can be
//!   cloned, `Receiver` iterates until every sender is gone.
//! - `Arc<T>` — the thread-safe cousin of `Rc<T>`: `Outbox` and `Shared` are
//!   used by both threads.
//! - `thread::spawn(move || …)` — the closure takes ownership of what it uses.
//! - `catch_unwind` — turn a panic in the program thread into an exit code
//!   instead of a session that never ends.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};
use valo_compiler::span::SourceMap;
use valo_compiler::{CompileOptions, compile_source, diagnostic};
use valo_vm::debug::LineTable;

use crate::program::{self, Command, Exception, Launch, Shared, StepKind};
use crate::protocol::Outbox;

/// The only thread valo programs have; DAP still wants an id for it.
pub const THREAD_ID: i64 = 1;

/// A panic in the program thread (a bug in valo, not in the program).
const EXIT_PANIC: i32 = 101;

/// Everything the session loop reacts to.
#[derive(Debug)]
pub enum Input {
    /// A DAP message from the client.
    Message(Value),
    /// The client closed stdin (or sent garbage): end the session.
    ClientClosed,
    /// The program stopped inside the debug hook and waits for commands.
    Stopped {
        /// DAP stop reason: `breakpoint`, `pause`, `entry`, `step`, `exception`.
        reason: &'static str,
        /// Id of the breakpoint that was hit.
        hit: Option<i64>,
        /// The runtime error, for reason `exception` (D4).
        exception: Option<Exception>,
    },
    /// The program thread finished.
    ProgramEnded {
        /// 0 = success, 1 = runtime error, 2 = compile error, 130 = stopped.
        exit_code: i32,
    },
}

/// What the adapter can do, sent in reply to `initialize`. VS Code hides UI
/// for anything not listed (no "conditional breakpoint" menu item without
/// `supportsConditionalBreakpoints`, …), so each roadmap step adds flags here.
pub fn capabilities() -> Value {
    json!({
        // VS Code sends `configurationDone` after the breakpoints; we start
        // the program only then, so no breakpoint is set too late.
        "supportsConfigurationDoneRequest": true,
        // The Stop button sends `terminate` (end the program, keep the
        // session to show its last output) before `disconnect`.
        "supportsTerminateRequest": true,
        // Hovering a name while stopped shows its value (`evaluate` with
        // context "hover").
        "supportsEvaluateForHovers": true,
        // Checkboxes in the Breakpoints view; VS Code sends the ticked ones
        // in `setExceptionBreakpoints` (D4).
        "exceptionBreakpointFilters": [
            { "filter": "runtime", "label": "Runtime errors", "default": true,
              "description": "Stop where a runtime error happens (overflow, division by zero, wrong type, …), before the call stack is unwound." },
            { "filter": "assert", "label": "Failed assertions", "default": true,
              "description": "Stop where an `assert` fails (R0701)." },
        ],
        // The exception widget in the editor (`exceptionInfo`).
        "supportsExceptionInfoRequest": true,
        // EXTEND (D5): "supportsConditionalBreakpoints", "supportsHitConditionalBreakpoints",
        // "supportsLogPoints", "supportsFunctionBreakpoints", …
    })
}

/// The program file, accepted by `launch`.
struct Program {
    /// `launch` arguments, until `configurationDone` starts the program.
    pending: Option<Launch>,
    /// Canonical path, to match `setBreakpoints` sources against.
    path: String,
    /// Lines where a breakpoint can stop (`LineTable::lines`).
    lines: BTreeSet<u32>,
}

/// Session state.
struct Session {
    out: Arc<Outbox>,
    /// Handed to the program thread so it can post `Stopped`/`ProgramEnded`.
    tx: Sender<Input>,
    /// Breakpoints and flags shared with the program thread.
    shared: Arc<Shared>,
    program: Option<Program>,
    /// Commands to the program thread; `Some` once it started.
    cmds: Option<Sender<Command>>,
    /// The program waits in its hook for commands.
    stopped: bool,
    /// The error of the current stop, if it is one (`exceptionInfo`).
    exception: Option<Exception>,
    /// Breakpoint ids are unique for the whole session.
    next_breakpoint_id: i64,
}

/// Whether the loop goes on after a request.
#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Stop,
}

/// Run a session until the client disconnects or closes the stream.
///
/// `tx` is a sender for the same channel `rx` reads, so the program thread
/// can post into it.
pub fn run(out: Arc<Outbox>, tx: Sender<Input>, rx: Receiver<Input>) {
    let mut session = Session {
        out,
        tx,
        shared: Arc::new(Shared::default()),
        program: None,
        cmds: None,
        stopped: false,
        exception: None,
        next_breakpoint_id: 1,
    };
    for input in rx {
        match input {
            Input::Message(msg) => {
                // The client may also send responses (to reverse requests
                // such as `runInTerminal`, D6); none are sent yet.
                if msg["type"] == "request" && session.handle_request(&msg) == Flow::Stop {
                    break;
                }
            }
            Input::ClientClosed => break,
            Input::Stopped {
                reason,
                hit,
                exception,
            } => {
                session.stopped = true;
                let mut body = json!({
                    "reason": reason,
                    "threadId": THREAD_ID,
                    "allThreadsStopped": true,
                });
                if let Some(id) = hit {
                    body["hitBreakpointIds"] = json!([id]);
                }
                if let Some(e) = &exception {
                    // `description` labels the paused thread in the Call
                    // Stack view; `text` is shown next to it.
                    body["description"] = json!(format!("Paused on error {}", e.id));
                    body["text"] = json!(e.description);
                }
                session.exception = exception;
                session.out.event("stopped", body);
            }
            Input::ProgramEnded { exit_code } => {
                session.stopped = false;
                session.cmds = None;
                session
                    .out
                    .event("exited", json!({ "exitCode": exit_code }));
                session.out.event("terminated", json!({}));
            }
        }
    }
    session.abort();
}

impl Session {
    /// One arm per DAP command. Unknown commands get an error response, as
    /// the spec requires: the client must never wait for an answer forever.
    fn handle_request(&mut self, req: &Value) -> Flow {
        let args = &req["arguments"];
        // RUST: `as_str()` gives `Option<&str>`; `unwrap_or("")` turns a
        // missing command into one that matches no arm.
        match req["command"].as_str().unwrap_or("") {
            "initialize" => self.out.respond(req, capabilities()),
            "launch" => self.launch(req, args),
            "setBreakpoints" => {
                let body = self.set_breakpoints(args);
                self.out.respond(req, body);
            }
            "setExceptionBreakpoints" => {
                let filters: Vec<&str> = args["filters"]
                    .as_array()
                    .map(|f| f.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let on = |name: &str| filters.contains(&name);
                self.shared
                    .break_on_error
                    .store(on("runtime"), Ordering::SeqCst);
                self.shared
                    .break_on_assert
                    .store(on("assert"), Ordering::SeqCst);
                self.out.respond(req, json!({}));
            }
            // The exception widget: what the program stopped on.
            "exceptionInfo" => match &self.exception {
                Some(e) if self.stopped => self.out.respond(
                    req,
                    json!({
                        "exceptionId": e.id,
                        "description": e.description,
                        // valo has no try/catch: every runtime error is unhandled.
                        "breakMode": "unhandled",
                        "details": { "message": e.description, "typeName": "runtime error" },
                    }),
                ),
                _ => self.out.fail(req, "the program is not stopped at an error"),
            },
            "configurationDone" => {
                self.out.respond(req, json!({}));
                self.start();
            }
            // VS Code asks for threads whenever the program stops.
            "threads" => self.out.respond(
                req,
                json!({ "threads": [{ "id": THREAD_ID, "name": "main" }] }),
            ),
            "stackTrace" => {
                let start = args["startFrame"].as_u64().unwrap_or(0) as usize;
                let levels = args["levels"].as_u64().unwrap_or(0) as usize;
                match self.ask(|reply| Command::StackTrace {
                    start,
                    levels,
                    reply,
                }) {
                    Some(body) => self.out.respond(req, body),
                    None => self.out.fail(req, "the program is not stopped"),
                }
            }
            "continue" => {
                // Reply first: events from the running program come after.
                self.out
                    .respond(req, json!({ "allThreadsContinued": true }));
                self.resume(Command::Continue);
            }
            // F10 / F11 / Shift+F11. The hook decides where the step ends
            // (`DapHook::step_done`) and reports `stopped {reason: step}`.
            "next" | "stepIn" | "stepOut" => {
                let kind = match req["command"].as_str() {
                    Some("stepIn") => StepKind::In,
                    Some("stepOut") => StepKind::Out,
                    _ => StepKind::Over,
                };
                self.out.respond(req, json!({}));
                self.resume(Command::Step(kind));
            }
            "pause" => {
                // Takes effect at the next line start (`DapHook::on_line`).
                self.shared.pause.store(true, Ordering::SeqCst);
                self.out.respond(req, json!({}));
            }
            "terminate" => {
                self.out.respond(req, json!({}));
                self.abort();
            }
            // End of session. A program still running is stopped here, or
            // when `main.rs` exits the process.
            "disconnect" => {
                self.out.respond(req, json!({}));
                return Flow::Stop;
            }
            // Scopes of a frame: its locals, its captures (if any) and the
            // globals. Label frames
            // (odd ids: tail-call notes) have none.
            "scopes" => {
                let id = args["frameId"].as_i64().unwrap_or(-1);
                if !self.stopped {
                    self.out.fail(req, "the program is not stopped");
                } else if id < 0 || id % 2 == 1 {
                    self.out.respond(req, json!({ "scopes": [] }));
                } else {
                    let frame = (id / 2) as usize;
                    let mut scopes = vec![json!(
                        { "name": "Locals", "presentationHint": "locals",
                          "variablesReference": program::locals_ref(frame), "expensive": false }
                    )];
                    // Closure: the values a lambda or local fn captured
                    // (M6); only shown when there are any.
                    let captures = program::captures_ref(frame);
                    let has_captures = self
                        .ask(|reply| Command::Variables {
                            reference: captures,
                            reply,
                        })
                        .is_some_and(|b| b["variables"].as_array().is_some_and(|v| !v.is_empty()));
                    if has_captures {
                        scopes.push(json!(
                            { "name": "Closure", "variablesReference": captures, "expensive": false }
                        ));
                    }
                    scopes.push(json!(
                        { "name": "Globals",
                          "variablesReference": program::GLOBALS_REF, "expensive": false }
                    ));
                    self.out.respond(req, json!({ "scopes": scopes }));
                }
            }
            "variables" => {
                let reference = args["variablesReference"].as_i64().unwrap_or(0);
                match self.ask(|reply| Command::Variables { reference, reply }) {
                    Some(body) => self.out.respond(req, body),
                    None => self.out.fail(req, "the program is not stopped"),
                }
            }
            // Hover, Watch and the Debug Console. D3 evaluates names only;
            // expressions need the compiler in a paused frame (D5).
            "evaluate" => self.evaluate(req, args),
            other => self.out.fail(
                req,
                &format!("valo-dap does not support `{other}` yet (see tooling/dap/README.md)"),
            ),
        }
        Flow::Continue
    }

    /// `launch`: check the arguments and compile once, so a broken program
    /// fails here, with its errors in the Debug Console and a dialog. On
    /// success, send `initialized`: VS Code now sends the breakpoints, which
    /// can only be verified against a compiled program.
    fn launch(&mut self, req: &Value, args: &Value) {
        let Some(name) = args["program"].as_str().filter(|p| !p.is_empty()) else {
            self.out
                .fail(req, "launch: `program` (path to a .vl file) is required");
            return;
        };
        let text = match std::fs::read_to_string(name) {
            Ok(text) => text,
            Err(e) => {
                self.out.fail(req, &format!("cannot read {name}: {e}"));
                return;
            }
        };
        let sm = SourceMap::new(name, text);
        let lines = match compile_source(&sm, CompileOptions::default()) {
            // Lines with code, minus the implicit end of `<main>` after
            // the last line (`program::last_code_line`).
            Ok(compiled) => {
                let last = program::last_code_line(&sm.text);
                let mut lines = LineTable::new(&compiled).lines().clone();
                lines.retain(|&l| l <= last);
                lines
            }
            Err(diags) => {
                self.out
                    .output("stderr", &diagnostic::render(&sm, &diags, false));
                self.out.fail(
                    req,
                    &format!("{name} has compile errors (shown in the Debug Console)"),
                );
                return;
            }
        };
        let path = canonical(name);
        let file_name = Path::new(&path)
            .file_name()
            .map_or_else(|| path.clone(), |f| f.to_string_lossy().into_owned());
        self.program = Some(Program {
            pending: Some(Launch {
                name: name.to_string(),
                source: json!({ "name": file_name, "path": path }),
                text: sm.text,
                debug: args["noDebug"] != true,
                stop_on_entry: args["stopOnEntry"] == true,
            }),
            path,
            lines,
        });
        self.out.respond(req, json!({}));
        self.out.event("initialized", json!({}));
    }

    /// `setBreakpoints`: the client sends *all* breakpoints of one file each
    /// time; they replace the previous ones. Each is verified: moved to the
    /// next line with code, or rejected past the last one.
    fn set_breakpoints(&mut self, args: &Value) -> Value {
        let requested: Vec<u64> = args["breakpoints"]
            .as_array()
            .map(|bps| bps.iter().filter_map(|bp| bp["line"].as_u64()).collect())
            .unwrap_or_default();
        let ours = self
            .program
            .as_ref()
            .filter(|p| args["source"]["path"].as_str().map(canonical).as_deref() == Some(&p.path));
        let Some(program) = ours else {
            // Another file: valo programs are one file until imports (S16).
            let breakpoints: Vec<Value> = requested
                .iter()
                .map(|line| {
                    json!({ "verified": false, "line": line,
                                    "message": "this file is not part of the running program" })
                })
                .collect();
            return json!({ "breakpoints": breakpoints });
        };
        let mut table = BTreeMap::new();
        let mut breakpoints = Vec::new();
        for &line in &requested {
            let actual = u32::try_from(line)
                .ok()
                .and_then(|l| program.lines.range(l..).next().copied());
            let Some(actual) = actual else {
                breakpoints.push(json!({ "verified": false, "line": line,
                                         "message": "no code on or after this line" }));
                continue;
            };
            // Two requests that land on one line share a breakpoint.
            let id = *table.entry(actual).or_insert_with(|| {
                self.next_breakpoint_id += 1;
                self.next_breakpoint_id - 1
            });
            breakpoints.push(json!({ "id": id, "verified": true, "line": actual }));
        }
        *lock(&self.shared.breakpoints) = table;
        json!({ "breakpoints": breakpoints })
    }

    /// `evaluate`: a bare name, looked up in the frame's locals, then in the
    /// globals.
    fn evaluate(&mut self, req: &Value, args: &Value) {
        let expr = args["expression"].as_str().unwrap_or("").trim();
        let is_name = expr
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
            && expr.chars().all(|c| c.is_alphanumeric() || c == '_');
        if !is_name {
            self.out.fail(
                req,
                "only variable names can be evaluated yet (expressions: step D5)",
            );
            return;
        }
        // Frame ids are `2i` (`DapHook::stack_trace`); odd ids are labels.
        let frame = args["frameId"]
            .as_i64()
            .filter(|id| *id >= 0 && id % 2 == 0)
            .map(|id| (id / 2) as usize);
        let name = expr.to_string();
        match self.ask(|reply| Command::Evaluate { frame, name, reply }) {
            None => self.out.fail(req, "the program is not stopped"),
            Some(None) => self.out.fail(req, &format!("`{expr}` is not defined here")),
            Some(Some(body)) => self.out.respond(req, body),
        }
    }

    /// Start the launched program on its own thread.
    fn start(&mut self) {
        // `take()` empties the Option: a second `configurationDone` cannot
        // start the program twice.
        let Some(launch) = self.program.as_mut().and_then(|p| p.pending.take()) else {
            return;
        };
        let (cmd_tx, cmd_rx) = mpsc::channel();
        self.cmds = Some(cmd_tx);
        let out = Arc::clone(&self.out);
        let shared = Arc::clone(&self.shared);
        let tx = self.tx.clone();
        thread::spawn(move || {
            let ended = tx.clone();
            let exit_code = catch_unwind(AssertUnwindSafe(|| {
                program::run_program(launch, &out, &shared, tx, cmd_rx)
            }))
            .unwrap_or(EXIT_PANIC);
            let _ = ended.send(Input::ProgramEnded { exit_code });
        });
    }

    /// Ask the stopped program something and wait for the answer. `None`
    /// when it is not stopped (or gone).
    fn ask<T>(&self, command: impl FnOnce(Sender<T>) -> Command) -> Option<T> {
        if !self.stopped {
            return None;
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        self.cmds.as_ref()?.send(command(reply_tx)).ok()?;
        reply_rx.recv().ok()
    }

    /// Let a stopped program go on with `command`.
    fn resume(&mut self, command: Command) {
        self.exception = None;
        if std::mem::replace(&mut self.stopped, false) {
            if let Some(cmds) = &self.cmds {
                let _ = cmds.send(command);
            }
        }
    }

    /// End the program: at its next line start if running, now if stopped.
    fn abort(&mut self) {
        self.shared.abort.store(true, Ordering::SeqCst);
        self.resume(Command::Abort);
    }
}

/// A path in the form used to compare files (symlinks and `..` resolved).
fn canonical(path: &str) -> String {
    std::fs::canonicalize(path).map_or_else(|_| path.to_string(), |p| p.display().to_string())
}

/// Lock a mutex even if another thread panicked while holding it.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
