//! # The debug console as a host (`host.rs`)
//!
//! **What this file is:** a `valo_vm::host::Host` whose output goes to the
//! Debug Console. Under the debugger the adapter's stdout belongs to the
//! protocol, so the program cannot print there; each `print` becomes a DAP
//! `output` event instead. This is the payoff of routing every effect through
//! one door (`vm/src/host.rs`): the VM needs no change to be run by the
//! debugger.
//!
//! **Textbook chapter:** `docs/13-debugger.md` (§13.5).
//!
//! ## Where to change things
//!
//! | I want to…                                   | Look at                                    |
//! |----------------------------------------------|--------------------------------------------|
//! | support a new effect                         | a new `Host` method → implement it here too |
//! | give the program real stdin                  | `read_line` (now: end of input; D6)         |
//! | send output to the integrated terminal       | `runInTerminal` reverse request (README, D6) |

use std::io;
use std::sync::Arc;

use valo_vm::host::Host;

use crate::protocol::Outbox;

/// Sends the program's output to the client as `output` events.
pub struct DapHost {
    out: Arc<Outbox>,
}

impl DapHost {
    /// A host writing through `out`.
    pub fn new(out: Arc<Outbox>) -> DapHost {
        DapHost { out }
    }
}

impl Host for DapHost {
    fn write_stdout(&mut self, s: &str) -> io::Result<()> {
        self.out.output("stdout", s);
        Ok(())
    }

    fn write_stderr(&mut self, s: &str) -> io::Result<()> {
        self.out.output("stderr", s);
        Ok(())
    }

    /// The adapter's stdin carries the protocol, so there is no input for
    /// the program yet: `read_line` reports end of input. D6 runs the
    /// program in the integrated terminal instead (`runInTerminal`).
    fn read_line(&mut self) -> io::Result<Option<String>> {
        Ok(None)
    }

    fn read_file(&mut self, path: &str) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn write_file(&mut self, path: &str, content: &str) -> io::Result<()> {
        std::fs::write(path, content)
    }
}
