# valo

valo is a functional, indentation-based scripting language. Source is compiled
to bytecode and executed by a register-based virtual machine; Lua 5.x is the
guiding model for the VM and runtime.

Status: early MVP. Bindings, ints, floats, bools, conditionals, functions, calls and `print` run (milestones M0–M4).

File extensions (tentative): source files use `.vl`, compiled bytecode uses `.vlc`.

## Workspace layout

```text
valo/                      Cargo workspace
├── compiler/  valo-compiler   lexer, parser, resolver, codegen
│   └── bin: valo              CLI: run / build / check
│        │
│        │ depends on (bytecode types)
│        ▼
└── vm/        valo-vm         bytecode format, values, interpreter
    └── bin: valo-vm           runs precompiled .vlc files
```

The dependency only goes one way: `valo-vm` never depends on the compiler, so
the VM can be built on its own (for WASM, or to run precompiled `.vlc` files,
like Lua running `luac` output).

## Status

Milestones M0–M4 of `.memory/plan-mvp.md` are done: `let` bindings with shadowing,
indented blocks, ints, floats, bools, comparisons (chained), `and`/`or`/`not`,
`if`/`elif`/`else`, `assert`, top-level `fn` definitions (hoisted, annotated), calls with
compile-time argument-count checks, pipes `|>`, and the built-ins `print`, `int`, `float`
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

## Planned crates

None of these are dependencies yet. Each is added when the milestone that
needs it starts:

- **postcard + serde**: reading and writing `.vlc` bytecode files. Not bincode
  (RUSTSEC-2025-0141).
- **imbl**: persistent (immutable, structurally shared) collections for runtime
  lists and maps.
- **regex**: a regular-expression standard library module.
- **serde_json**: a JSON standard library module.
- **comrak**: Markdown rendering for a standard library module or doc tooling.
- **ureq**: a blocking HTTP client standard library module.
- **rustyline / reedline**: line editing for an interactive REPL.
- **tracing**: structured logging/instrumentation for the compiler and VM.

Deliberately not used: **logos, chumsky, pest, lalrpop**. The lexer and parser
are hand-written, to handle indentation (INDENT/DEDENT), to control diagnostics
and error recovery, and as a learning exercise.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
