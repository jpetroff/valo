//! # `valo_lsp` — the valo language server
//!
//! A **language server** is a separate program that an editor starts and
//! talks to over stdin/stdout using the Language Server Protocol (LSP):
//! JSON-RPC messages such as "this file was opened, here is its text" (editor
//! → server) and "here are the errors in that file" (server → editor). The
//! server reuses the real compiler (`valo_compiler`), so the red squiggles in
//! the editor and the output of `valo check` always agree.
//!
//! The same binary works in any LSP editor (VS Code through
//! `tooling/vscode`, Neovim, Helix, Zed, …).
//!
//! **Textbook chapter:** `docs/12-editor-tooling.md`. Feature roadmap:
//! `tooling/lsp/README.md`.
//!
//! ## Message flow
//!
//! ```text
//!  editor                                   valo-lsp
//!    │ ── initialize (client capabilities) ──▶ │
//!    │ ◀── result (server capabilities) ────── │   main.rs
//!    │ ── initialized ───────────────────────▶ │
//!    │ ── textDocument/didOpen (text) ───────▶ │   server.rs: compile
//!    │ ◀── textDocument/publishDiagnostics ─── │   convert.rs: Span → Range
//!    │ ── textDocument/didChange (new text) ─▶ │   (again on every edit)
//!    │ ── shutdown / exit ───────────────────▶ │
//! ```
//!
//! ## Map of the crate
//!
//! | File              | Contents                                                      |
//! |-------------------|---------------------------------------------------------------|
//! | `main.rs`         | the `valo-lsp` binary: stdio transport, initialize handshake  |
//! | [`server`]        | main loop, open-document store, request/notification dispatch |
//! | [`line_index`]    | byte offsets (valo spans) ↔ LSP line/UTF-16 positions         |
//! | [`convert`]       | valo `Diagnostic` → LSP `Diagnostic`                          |
//! | `tests/smoke.rs`  | drives the server in memory, as an editor would               |
//!
//! Comment conventions (`// RUST:`, `// EXTEND:`) are described in the
//! `valo_vm` crate docs (`vm/src/lib.rs`).

pub mod convert;
pub mod line_index;
pub mod server;
