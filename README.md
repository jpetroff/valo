# valo

valo is a functional, indentation-based scripting language. Source is compiled
to bytecode and executed by a register-based virtual machine; Lua 5.x is the
guiding model for the VM and runtime.

Status: early MVP. Bindings, ints, floats, bools, conditionals, functions, lambdas and closures, calls, keyword arguments, `_` partial application, strings with `{…}` interpolation, `none`, `Ok`/`Err` with `?`, stdin and file I/O run, with proper tail calls (milestones M0–M7).

File extensions (tentative): source files use `.vl`, compiled bytecode uses `.vlc`.

## Workspace layout

```text
valo/                      Cargo workspace
├── compiler/  valo-compiler   lexer, parser, resolver, codegen
│   └── bin: valo              CLI: run / build / check
│        │
│        │ depends on (bytecode types)
│        ▼
├── vm/        valo-vm         bytecode format, values, interpreter
│   └── bin: valo-vm           runs precompiled .vlc files
└── tooling/                   developer tools (see tooling/README.md)
    ├── lsp/   valo-lsp        language server: errors as you type in any LSP editor
    ├── dap/   valo-dap        debug adapter: run/debug .vl files from VS Code (DAP)
    └── vscode/                VS Code extension: highlighting, LSP client, debugger
```

The dependency only goes one way: `valo-vm` never depends on the compiler, so
the VM can be built on its own (for WASM, or to run precompiled `.vlc` files,
like Lua running `luac` output).

## Status

Milestones M0–M7 of `.memory/plan-mvp.md` are done: `let` bindings with shadowing,
indented blocks, ints, floats, bools, comparisons (chained), `and`/`or`/`not`,
`if`/`elif`/`else`, `assert`, `fn` definitions (hoisted at top level, local `fn`s inside
blocks, annotated), lambdas (`x => x + 1`, `(a, b) => …`) and closures that capture by
value, `_` partial application (`add(1, _)`, `x |> sub(10, _)`), calls with
compile-time argument checks, keyword-only parameters with defaults (`f(x, retries=2)`),
pipes `|>`, proper tail calls (loops as tail recursion run in constant stack space; deep
non-tail recursion stops with a clean R0301), strings (`"Hello, {name}!"`, escapes, raw
`r"…"`, `+`, comparisons), `none`, results with `?` and `context` (an unhandled `Err` prints
its context chain and where it came from), and the built-ins `print` (with `end=`), `int`,
`float`, `str`, `str_len`, `trim`, `parse_int`, `parse_float`, `Ok`, `Err`, `context`,
`is_ok`, `is_err`, `unwrap`, `or_else`, `read_line`, `read_file`, `write_file`
run end to end (indentation-aware lexer → Pratt parser → desugaring → name resolver →
register bytecode → VM with call frames and stack traces).

```sh
printf 'fn fact(n: int) -> int:\n    if n <= 1: 1\n    else: n * fact(n - 1)\n\n20 |> fact |> print\n' > demo.vl
cargo run -q --bin valo -- run demo.vl                 # 2432902008176640000
cargo run -q --bin valo -- check demo.vl --dump bytecode
```

## Documentation

`docs/` is a textbook on how this compiler and VM are built, chapter by chapter, with
references to the code — start at [docs/README.md](docs/README.md). Every source file
opens with a guide comment (what it does, where to change things, Rust features used).

## Building and testing

```sh
cargo build --workspace                # debug build
cargo build --workspace --release      # small, stripped binaries in target/release/
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --check
```

## Crates

- `valo-compiler` (`compiler/`): turns `.vl` source into bytecode, and provides the `valo` CLI.
- `valo-vm` (`vm/`): owns the bytecode format and runtime, and provides the `valo-vm` runner.
- `valo-lsp` (`tooling/lsp/`): language server built on `valo-compiler`
  (uses `lsp-server`, `lsp-types`, `serde_json`).
- `valo-dap` (`tooling/dap/`): debug adapter built on `valo-compiler` and
  `valo-vm` (uses `serde_json`; the protocol framing is hand-written).

## Editor support

`tooling/` holds a VS Code extension (syntax highlighting, indentation), the
`valo-lsp` language server (compile errors as you type) and the `valo-dap`
debug adapter (F5 runs a `.vl` file under the debugger, with breakpoints
the call stack, stepping, variables, and stopping at runtime errors). Install steps: [tooling/README.md](tooling/README.md); how
they work: [docs/12-editor-tooling.md](docs/12-editor-tooling.md) and
[docs/13-debugger.md](docs/13-debugger.md).

## Planned crates

None of these are dependencies yet. Each is added when the milestone that
needs it starts:

- **postcard + serde**: reading and writing `.vlc` bytecode files. Not bincode
  (RUSTSEC-2025-0141).
- **imbl**: persistent (immutable, structurally shared) collections for runtime
  lists and maps.
- **regex**: a regular-expression standard library module.
- **serde_json**: a JSON standard library module (already used by `valo-lsp`).
- **comrak**: Markdown rendering for a standard library module or doc tooling.
- **ureq**: a blocking HTTP client standard library module.
- **rustyline / reedline**: line editing for an interactive REPL.
- **tracing**: structured logging/instrumentation for the compiler and VM.

Deliberately not used: **logos, chumsky, pest, lalrpop**. The lexer and parser
are hand-written, to handle indentation (INDENT/DEDENT), to control diagnostics
and error recovery, and as a learning exercise.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
