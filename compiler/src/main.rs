//! # `valo` — the command-line driver (`main.rs`)
//!
//! **What this file is:** the program you run. It parses command-line
//! arguments (with the `clap` crate), reads the source file, calls the
//! compiler pipeline (`valo_compiler::compile_source`), runs the result on the
//! VM (`valo_vm::vm::Vm`) and turns the outcome into output and an exit code.
//!
//! **Built as:** `target/debug/valo` (`[[bin]]` in `compiler/Cargo.toml`).
//! Run without installing: `cargo run -q --bin valo -- run file.vl`.
//!
//! **Textbook chapter:** `docs/09-driver-and-cli.md`.
//!
//! ## Commands
//!
//! ```text
//! valo run   <file> [--echo] [--trace-exec] [--dump tokens|ast|resolved|bytecode]
//! valo check <file> [--dump tokens|ast|resolved|bytecode]
//! valo build <file> [-o out]          (not implemented yet)
//! ```
//!
//! Exit codes (plan D16): 0 ok, 1 runtime error, 2 compile error,
//! 64 usage error or unreadable source file.
//!
//! ## Where to change things
//!
//! | I want to…                         | Look at                                              |
//! |------------------------------------|------------------------------------------------------|
//! | add a command-line flag            | the `Command` enum (clap reads the field names and `///` comments) and the matching function (`run`, `check`) |
//! | add a subcommand                   | new variant in `Command` + an arm in `main`          |
//! | add a new `--dump` format          | `enum Dump` + `dump_text`                            |
//! | change exit codes                  | the `EXIT_*` constants                               |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `#[derive(Parser)]`, `#[arg(long)]` — clap generates the argument parser
//!   from the struct definition (*Traits, impl and derive*).
//! - `fn main() -> ExitCode` — the return value is the process exit status.
//! - `Result<SourceMap, ExitCode>` — using the error side to carry "stop with
//!   this exit code" (*Option and Result*).
//! - `eprint!` / `eprintln!` write to stderr, `print!` to stdout (*Macros*).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::error::ErrorKind as ClapErrorKind;
use clap::{Parser, Subcommand, ValueEnum};
use valo_compiler::diagnostic::{self, Diagnostic};
use valo_compiler::span::SourceMap;
use valo_compiler::{
    CompileOptions, ast, compile_source, lexer, parse_source, resolve, resolve_source,
};
use valo_vm::host::StdHost;
use valo_vm::vm::{Options as VmOptions, Vm};

const EXIT_RUNTIME: u8 = 1;
const EXIT_COMPILE: u8 = 2;
const EXIT_USAGE: u8 = 64;

/// The valo language: compile and run `.vl` source files.
// RUST: `#[derive(Parser)]` (from clap) generates `Cli::try_parse()`, which
// reads `std::env::args()`. The `///` comments become `--help` text.
#[derive(Debug, Parser)]
#[command(name = "valo", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Subcommands. EXTEND: a new subcommand is a new variant here.
#[derive(Debug, Subcommand)]
enum Command {
    /// Compile and run a source file.
    Run {
        /// Source file (`.vl`).
        file: PathBuf,
        /// Print the value of every top-level expression.
        #[arg(long)]
        echo: bool,
        /// Trace every executed instruction to stderr (debugging aid).
        #[arg(long)]
        trace_exec: bool,
        /// Print an intermediate representation to stderr before running.
        #[arg(long, value_enum)]
        dump: Option<Dump>,
    },
    /// Compile a source file to bytecode (not implemented yet).
    Build {
        /// Source file (`.vl`).
        file: PathBuf,
        /// Output bytecode file (defaults to the input with a `.vlc` extension).
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Check a source file for errors without running it.
    Check {
        /// Source file (`.vl`).
        file: PathBuf,
        /// Print an intermediate representation to stdout.
        #[arg(long, value_enum)]
        dump: Option<Dump>,
    },
}

/// Intermediate representations that can be printed (`--dump`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Dump {
    /// Token stream, including NEWLINE/INDENT/DEDENT.
    Tokens,
    /// Syntax tree as S-expressions.
    Ast,
    /// Name resolution: global/local slots and what each name refers to.
    Resolved,
    /// Disassembled bytecode.
    Bytecode,
}

fn main() -> ExitCode {
    // Parse arguments ourselves (instead of `Cli::parse()`) so that usage
    // errors exit with 64 rather than clap's default 2, which we reserve for
    // compile errors.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return match e.kind() {
                ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion => ExitCode::SUCCESS,
                _ => ExitCode::from(EXIT_USAGE),
            };
        }
    };
    match cli.command {
        Command::Run {
            file,
            echo,
            trace_exec,
            dump,
        } => run(&file, echo, trace_exec, dump),
        Command::Check { file, dump } => check(&file, dump),
        Command::Build { .. } => {
            eprintln!(
                "error: `valo build` is not implemented yet (bytecode files are planned after the MVP)"
            );
            ExitCode::from(EXIT_USAGE)
        }
    }
}

/// Read the source file; on failure print an error and return exit code 64.
fn load(path: &Path) -> Result<SourceMap, ExitCode> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(SourceMap::new(path.display().to_string(), text)),
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", path.display());
            Err(ExitCode::from(EXIT_USAGE))
        }
    }
}

/// Print compile errors and return exit code 2.
fn report(sm: &SourceMap, diags: &[Diagnostic]) -> ExitCode {
    eprint!(
        "{}",
        diagnostic::render(sm, diags, diagnostic::stderr_color())
    );
    ExitCode::from(EXIT_COMPILE)
}

/// Render a dump; `Err` carries the exit code for compile errors.
fn dump_text(sm: &SourceMap, dump: Dump, echo: bool) -> Result<String, ExitCode> {
    match dump {
        Dump::Tokens => {
            let lexed = lexer::lex(&sm.text);
            if !lexed.diags.is_empty() {
                return Err(report(sm, &lexed.diags));
            }
            Ok(lexer::dump(&lexed.tokens))
        }
        Dump::Ast => parse_source(sm)
            .map(|p| ast::dump(&p))
            .map_err(|d| report(sm, &d)),
        Dump::Resolved => resolve_source(sm)
            .map(|(_, r)| resolve::dump(&r, sm))
            .map_err(|d| report(sm, &d)),
        Dump::Bytecode => compile_source(sm, CompileOptions { echo })
            .map(|p| valo_vm::disasm::disassemble(&p))
            .map_err(|d| report(sm, &d)),
    }
}

/// `valo check`: compile only (or print a dump to stdout).
fn check(path: &Path, dump: Option<Dump>) -> ExitCode {
    let sm = match load(path) {
        Ok(sm) => sm,
        Err(code) => return code,
    };
    if let Some(d) = dump {
        return match dump_text(&sm, d, false) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(code) => code,
        };
    }
    match compile_source(&sm, CompileOptions::default()) {
        Ok(_) => ExitCode::SUCCESS,
        Err(diags) => report(&sm, &diags),
    }
}

/// `valo run`: compile, then execute on the VM. Dumps go to stderr so that
/// stdout contains only the program's own output.
fn run(path: &Path, echo: bool, trace_exec: bool, dump: Option<Dump>) -> ExitCode {
    let sm = match load(path) {
        Ok(sm) => sm,
        Err(code) => return code,
    };
    if let Some(d) = dump {
        match dump_text(&sm, d, echo) {
            Ok(text) => eprint!("{text}"),
            Err(code) => return code,
        }
    }
    let program = match compile_source(&sm, CompileOptions { echo }) {
        Ok(p) => p,
        Err(diags) => return report(&sm, &diags),
    };
    // The real host: the program's output goes to this process's stdout.
    let mut host = StdHost;
    let mut vm = Vm::new(&mut host, VmOptions { trace_exec });
    match vm.run(&program) {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            eprint!("{}", diagnostic::render_runtime(&sm, &err));
            ExitCode::from(EXIT_RUNTIME)
        }
    }
}
