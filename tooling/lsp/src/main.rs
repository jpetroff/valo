//! # `valo-lsp` — the language server binary (`main.rs`)
//!
//! **What this file is:** the program an editor starts. It connects
//! stdin/stdout to the LSP transport, answers the `initialize` handshake with
//! the server's capabilities (`server::capabilities`), then hands control to
//! the main loop (`server::run`) until the editor sends `shutdown` + `exit`.
//!
//! **Built as:** `target/debug/valo-lsp`. Install with
//! `cargo install --path tooling/lsp`. The editor starts it; running it by
//! hand just waits for JSON-RPC on stdin.
//!
//! stdout belongs to the protocol: printing anything else there corrupts the
//! message stream. Log to stderr instead (VS Code shows it in the
//! "Valo Language Server" output channel).
//!
//! **Textbook chapter:** `docs/12-editor-tooling.md`.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `Box<dyn Error + Send + Sync>` — "any error type", so `?` works on
//!   errors from different crates (*Traits, impl and derive*).

use std::error::Error;

use lsp_server::Connection;

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    eprintln!("valo-lsp {} starting", env!("CARGO_PKG_VERSION"));

    // RUST: `Connection::stdio()` starts two threads, one reading framed
    // messages from stdin and one writing them to stdout; `connection` gives
    // us channels to and from them.
    let (connection, io_threads) = Connection::stdio();

    // Handshake: wait for `initialize`, reply with our capabilities.
    let init = serde_json::json!({
        "capabilities": valo_lsp::server::capabilities(),
        "serverInfo": { "name": "valo-lsp", "version": env!("CARGO_PKG_VERSION") },
    });
    let (id, _params) = connection.initialize_start()?;
    connection.initialize_finish(id, init)?;

    valo_lsp::server::run(&connection)?;

    // RUST: dropping `connection` closes the channels so the I/O threads can
    // finish; `join` waits for them.
    drop(connection);
    io_threads.join()?;
    eprintln!("valo-lsp stopped");
    Ok(())
}
