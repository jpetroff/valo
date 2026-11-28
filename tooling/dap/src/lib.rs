//! # `valo_dap` — the valo debug adapter
//!
//! A *debug adapter* sits between an editor's generic debugger UI and one
//! language's runtime. VS Code (or any other client) sends it requests in the
//! **Debug Adapter Protocol** (DAP): "launch this file", "set breakpoints on
//! lines 3 and 7", "continue", "what are the stack frames?". The adapter
//! answers them using the language's own machinery — here, `valo-compiler` to
//! compile the file and `valo-vm` to run it, both linked into this process.
//!
//! ```text
//!  VS Code debugger UI ──DAP over stdio──▶ valo-dap ──links──▶ valo-compiler + valo-vm
//!   (breakpoints, call stack,              session.rs           (the debuggee runs on
//!    variables, debug console)                                    a thread in here)
//! ```
//!
//! **Status: step D4** of `tooling/dap/README.md`: launch, line
//! breakpoints, call stack, continue, pause, stop on entry, step over / in /
//! out, variables and hover, stopping at runtime errors and failed asserts
//! (exception filters), terminate; program output and errors in the Debug
//! Console. Conditional breakpoints and logpoints are next (D5).
//!
//! **Textbook chapter:** `docs/13-debugger.md`.
//!
//! ## Map of the crate
//!
//! | File            | Contents                                                     |
//! |-----------------|--------------------------------------------------------------|
//! | [`protocol`]    | message framing (`Content-Length`), `Outbox` for responses and events |
//! | [`session`]     | the session state machine: one arm per DAP request             |
//! | [`program`]     | the program thread: runs the VM, its debug hook stops and answers |
//! | [`host`]        | `DapHost`: the program's `print` output becomes `output` events |
//! | `main.rs`       | stdio wiring: reader thread → channel → `session::run`         |
//! | `tests/smoke.rs`| starts the real binary and plays VS Code's part of a session    |

pub mod host;
pub mod program;
pub mod protocol;
pub mod session;
