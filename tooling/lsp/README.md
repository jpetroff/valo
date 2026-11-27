# valo-lsp — language server roadmap

`valo-lsp` is the semantic half of valo's editor support (the syntactic half
is the TextMate grammar in `tooling/vscode`). How it works:
[docs/12-editor-tooling.md](../../docs/12-editor-tooling.md). Build and install:
[tooling/README.md](../README.md).

The server grows in steps. Each step turns on one LSP capability in
`src/server.rs` → `capabilities`, answers its requests in
`Server::handle_request`, and builds on a compiler part that already exists.

## Status

| Step | Feature | LSP | Built on | Status |
|------|---------|-----|----------|--------|
| **L0** | highlighting, comment toggle, brackets, Enter-indent, folding | — (declarative, `tooling/vscode`) | `.memory/syntax.md` | done |
| **L1** | errors as you type | `textDocument/publishDiagnostics`, full text sync | `compile_source`, `Diagnostic` | done |
| **L2** | navigation | `definition`, `references`, `documentHighlight`, `rename` + `prepareRename`, `hover`, `documentSymbol` | `resolve::Resolved` (`uses`, `locals`, `globals` spans), `ast::Item::Fn` for signatures | next |
| **L3** | writing help | `completion` (keywords, globals, in-scope locals, keyword args), `signatureHelp` (keyword-only defaults, S19), `semanticTokens` (param / local / global fn / built-in) | resolver must expose the scope at an offset | |
| **L4** | fixes | `codeAction` quick fixes from `help` lines (did-you-mean E0201, "pass by name" E0421), `inlayHint` (param names), `formatting` once `valo fmt` exists | `help` kept structured in `Diagnostic` | |
| **L5** | grows with the language | see below | | |
| **L6** | debugger | Debug Adapter Protocol: a separate binary, `valo-dap` | see [`tooling/dap/README.md`](../dap/README.md) (steps D0–D7) | D0–D4 done |

### L2 notes

- A request carries `(uri, position)`. `LineIndex::offset` (already written
  and tested) converts it to a byte offset; the target is the `Use` in
  `Resolved::uses` whose span contains it, or the definition span in
  `locals`/`globals` when the cursor is on a definition.
- Keep the last successful `(ast::Program, Resolved)` per document: while the
  user types, the current text is often broken, and stale navigation data
  is more useful than none.
- Built-ins have an empty definition span: hover says "built-in", no jump.

## Language milestones → tooling checklist

Every language milestone adds a tooling item:

1. new syntax → grammar rule + an example line in
   `tooling/vscode/samples/showcase.vl`;
2. new error codes → nothing to do (they flow through L1), but add an LSP
   smoke test if the error has secondary spans or help lines;
3. new names/scopes → check L2/L3 still find them.

| Milestone | Tooling work |
|-----------|--------------|
| M6 lambdas, closures, `f(a, _)` | lambda params in hover/rename (resolver ready: lambda params are ordinary locals, a captured use is `Res::Upvalue`/`SelfRef` and `Res::binding()` names its definition; `$` names from `_` partials are never in `Resolved::uses`); `_` placeholder scope — with L2 |
| M7 strings, `none`, `Ok/Err`, `?` | grammar already covers interpolation; semantic tokens inside `{…}`; exercise 2 of chapter 12 |
| M8/M8b lists, dicts, comprehensions | comprehension loop variables in scope; completion after `.` for records/dicts (later) |
| M9 diagnostics quality | multi-error recovery shows more errors at once in L1; did-you-mean → L4 quick fixes |
| M10 runtime type checks | types in hover and signature help |
| M12 `match` | exhaustiveness diagnostics; constructor completion |
| M13 records, unions | outline entries for `def`; field completion; go to field definition |
| S16 imports | multi-file: file id in `Span`, workspace index, cross-file definition; incremental recomputation (`salsa`) |
| S22 effects | inlay hints `io` / `pure` on `fn` headers |
| S25 tests | code lens "run test" above `test "…":` blocks |
| M11 bytecode debug info | debug `.vlc` files (D6 in `tooling/dap/README.md`, which has the debugger's own checklist) |
