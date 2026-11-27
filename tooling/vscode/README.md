# Valo for VS Code

Language support for [valo](https://github.com/jpetroff/valo), a functional,
indentation-based scripting language (`.vl` files).

- Syntax highlighting, including string interpolation, regex and command literals
- Comment toggling (`#`), bracket matching and auto-closing
- Indentation after `:`, `=>`, `~>`, `as`; folding by indentation
- Errors as you type, from the real compiler, via the `valo-lsp` language server
- Run and debug `.vl` files (F5, or ▷ in the editor title) via the `valo-dap`
  debug adapter: breakpoints, call stack, stepping, variables, hover,
  stopping at runtime errors, pause, stop; output in the Debug Console (`tooling/dap/README.md`).

The language server and debug adapter must be installed separately:

```bash
cargo install --path tooling/lsp
cargo install --path tooling/dap
```

Settings: `valo.server.path` (path to `valo-lsp`), `valo.trace.server`
(log LSP traffic), `valo.debugger.path` (path to `valo-dap`),
`valo.trace.debugger` (log DAP traffic). Commands: **Valo: Restart Language
Server**, **Valo: Run File**, **Valo: Debug File**.

Build and install instructions: `tooling/README.md` in the valo repository.
