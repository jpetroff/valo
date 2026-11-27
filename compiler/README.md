# valo-compiler

Compiler for the valo language: a hand-written lexer (emitting
INDENT/DEDENT/NEWLINE tokens for indentation-based syntax), a recursive-descent
and Pratt parser producing a `Box`-tree AST with node ids, name resolution (scopes,
registers, upvalues), code generation into `valo-vm` prototypes, and
diagnostics rendered with source snippets.

The `valo` binary is the main CLI:

- `valo run <file> [--echo] [--trace-exec] [--dump tokens|ast|bytecode]`
- `valo check <file> [--dump tokens|ast|bytecode]`
- `valo build <file> [-o out]` (not implemented yet; bytecode files come after the MVP)

Exit codes: 0 ok, 1 runtime error, 2 compile error, 64 usage error or unreadable file.

Tests: unit + `insta` snapshots in `src/`, golden programs in `tests/golden/`
(`VALO_BLESS=1 cargo test --test golden` rewrites expectations; review the diff),
CLI tests in `tests/cli.rs`.
