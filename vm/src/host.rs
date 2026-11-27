//! # The host interface (`host.rs`)
//!
//! **What this file is:** the single door between a running valo program and
//! the outside world. The VM never prints, reads files or runs processes
//! itself; it asks a `Host` to do it. Swapping the host changes what effects
//! *mean*: the real process streams (`StdHost`), an in-memory recorder for
//! tests (`TestHost`) and, later, `--dry-run` / `--record` / `--replay`
//! handlers (the feasibility report's "effects through one door").
//!
//! **Pipeline position:** passed to `Vm::new` by the CLI
//! (`compiler/src/main.rs`) or by tests.
//!
//! **Textbook chapter:** `docs/07-virtual-machine.md` (section *The host*).
//!
//! ## Where to change things
//!
//! | I want to…                              | Look at                                          |
//! |-----------------------------------------|--------------------------------------------------|
//! | add a new effect (read stdin, files)    | add a method to `trait Host`, implement it for `StdHost` and `TestHost` (M7) |
//! | capture output differently in tests     | `TestHost`                                       |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `trait` — an interface; `impl Host for StdHost` implements it (*Traits, impl and derive*).
//! - `&mut dyn Host` in `vm.rs` — "some value implementing Host", chosen at runtime.
//! - `io::Result<()>` — success with no value, or an I/O error (*Option and Result*).

use std::io::{self, Write};

/// Side effects the VM may perform.
pub trait Host {
    /// Write text to standard output.
    fn write_stdout(&mut self, s: &str) -> io::Result<()>;
    /// Write text to standard error (diagnostic output such as `--trace-exec`).
    fn write_stderr(&mut self, s: &str) -> io::Result<()>;
}

/// The real process streams.
// RUST: a "unit struct" — a type with no fields; it exists only to carry the
// `Host` implementation.
#[derive(Debug, Default)]
pub struct StdHost;

impl Host for StdHost {
    fn write_stdout(&mut self, s: &str) -> io::Result<()> {
        // `lock()` gives exclusive access to stdout for this write;
        // `write_all` returns `Err` if the pipe is closed, which the VM
        // reports as R0501.
        io::stdout().lock().write_all(s.as_bytes())
    }

    fn write_stderr(&mut self, s: &str) -> io::Result<()> {
        io::stderr().lock().write_all(s.as_bytes())
    }
}

/// Captures output in memory; used by tests.
#[derive(Debug, Default)]
pub struct TestHost {
    /// Everything written to stdout.
    pub stdout: String,
    /// Everything written to stderr.
    pub stderr: String,
}

impl Host for TestHost {
    fn write_stdout(&mut self, s: &str) -> io::Result<()> {
        self.stdout.push_str(s);
        Ok(())
    }

    fn write_stderr(&mut self, s: &str) -> io::Result<()> {
        self.stderr.push_str(s);
        Ok(())
    }
}
