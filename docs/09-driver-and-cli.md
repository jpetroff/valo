# Chapter 9 — The Driver and the CLI

## 9.1 Theory: the compiler driver

The **driver** is the program the user runs. It does not compile anything
itself; it reads arguments and files, calls the stages in order, chooses what
to print, and turns the outcome into an exit status. (`gcc` and `rustc` are
drivers around many internal stages.)

## 9.2 valo's commands

File: [`compiler/src/main.rs`](../compiler/src/main.rs), binary `valo`.

```text
valo run   <file> [--echo] [--trace-exec] [--dump tokens|ast|resolved|bytecode]
valo check <file> [--dump tokens|ast|resolved|bytecode]
valo build <file> [-o out]           not implemented yet (bytecode files, M11)
valo --help | --version
every command: [--color auto|always|never]
```

| Flag | Effect |
|------|--------|
| `--echo` | print the value of every top-level expression line, not `let` or `fn` lines (debugging aid; programs use `print`) |
| `--trace-exec` | trace every executed instruction to stderr (chapter 7) |
| `--dump tokens` | token list (chapter 3) |
| `--dump ast` | syntax tree as S-expressions (chapter 4) |
| `--dump resolved` | name resolution: global and local slots, and the slot each name use refers to (chapter 6) |
| `--dump bytecode` | disassembly (chapter 5) |
| `--color auto\|always\|never` | colour error messages; `auto` (default) colours when stderr is a terminal and `NO_COLOR` is unset (chapter 8) |

With `check`, dumps go to stdout and nothing runs. With `run`, dumps go to
**stderr** before running, so stdout contains only the program's own output.

Without installing anything:

```bash
cargo run -q --bin valo -- run demo.vl --echo
```

or build once and call the binary directly: `target/debug/valo run demo.vl`.

## 9.3 How it works

Arguments are declared as Rust types; the `clap` crate generates the parser,
`--help` text (from the `///` comments) and error messages:

```rust
#[derive(Parser)]
struct Cli { #[command(subcommand)] command: Command }

#[derive(Subcommand)]
enum Command {
    Run { file: PathBuf, #[arg(long)] echo: bool, #[arg(long)] trace_exec: bool,
          #[arg(long, value_enum)] dump: Option<Dump> },
    Build { ... },
    Check { file: PathBuf, #[arg(long, value_enum)] dump: Option<Dump> },
}
```

`trace_exec` becomes `--trace-exec` automatically.

Control flow of `valo run`:

```text
main
 ├─ Cli::try_parse()          bad arguments → print usage, exit 64 (help/version → 0)
 └─ run(path, echo, trace_exec, dump)
     ├─ load(path)            unreadable → "error: cannot read …", exit 64
     ├─ dump_text(...)        optional, to stderr
     ├─ compile_source(...)   Err(diags) → report(): render to stderr, exit 2
     └─ Vm::new(&mut StdHost, ..).run(&program)
                              Err(e) → render_runtime to stderr, exit 1
                              Ok     → exit 0
```

`main` uses `Cli::try_parse()` instead of `Cli::parse()` because clap's default
exit code for usage errors is 2, which valo reserves for compile errors.

## 9.4 The library/binary split

`main.rs` is thin on purpose. The real work is in the library
(`valo_compiler::compile_source`, `parse_source`, `lexer::lex`, …), which
tests and future tools (a REPL, a language server, `valo-vm`) can call
directly.

```rust
// compiler/src/lib.rs
pub fn parse_source(sm: &SourceMap) -> Result<ast::Program, Vec<Diagnostic>>
pub fn compile_source(sm: &SourceMap, opts: CompileOptions) -> Result<Program, Vec<Diagnostic>>
```

## 9.5 valo-vm

`vm/src/bin/valo-vm.rs` is a placeholder for a runner of precompiled `.vlc`
files, which needs bytecode serialisation (M11). It prints a message and
exits with 2.

## 9.6 Where to change things

| Task | Where |
|------|-------|
| new flag | add a field to the `Command` variant (with a `///` doc line), pass it on in `main` |
| new subcommand (e.g. `fmt`, `test`) | new `Command` variant + `match` arm in `main` |
| new dump format | `enum Dump` + arm in `dump_text` |
| exit codes | `EXIT_RUNTIME`, `EXIT_COMPILE`, `EXIT_USAGE` |

## 9.7 Exercises

1. Add `--dump all` that prints tokens, AST and bytecode in sequence.
2. What happens with `valo run demo.vl > out.txt --echo --trace-exec`? Which
   file receives the trace, and why is that the right design?
