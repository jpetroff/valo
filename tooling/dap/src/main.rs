//! # `valo-dap` — the debug adapter binary (`main.rs`)
//!
//! **What this file is:** the program VS Code starts when a valo debug
//! session begins (`tooling/vscode/extension.js` →
//! `DebugAdapterExecutable`). One process = one debug session. It connects
//! stdin/stdout to the session loop:
//!
//! ```text
//!  stdin ──reader thread──▶ channel ──▶ session::run ──▶ Outbox ──▶ stdout
//!                             ▲                            ▲
//!                             └── ProgramEnded ── program thread (VM) ── output events
//! ```
//!
//! stdout belongs to the protocol: printing anything else there corrupts the
//! message stream. Log to stderr.
//!
//! **Built as:** `target/debug/valo-dap`. Install with
//! `cargo install --path tooling/dap`. Running it by hand just waits for DAP
//! messages on stdin.
//!
//! **Textbook chapter:** `docs/13-debugger.md`.

use std::io;
use std::sync::{Arc, mpsc};
use std::thread;

use valo_dap::protocol::{Outbox, read_message};
use valo_dap::session::{self, Input};

fn main() {
    let out = Arc::new(Outbox::new(Box::new(io::stdout())));
    let (tx, rx) = mpsc::channel();

    // Reader thread: framed messages from stdin into the channel. It ends
    // when the client closes the stream.
    let reader_tx = tx.clone();
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        loop {
            match read_message(&mut stdin) {
                Ok(Some(msg)) => {
                    if reader_tx.send(Input::Message(msg)).is_err() {
                        break; // session over
                    }
                }
                Ok(None) => {
                    let _ = reader_tx.send(Input::ClientClosed);
                    break;
                }
                Err(e) => {
                    eprintln!("valo-dap: {e}");
                    let _ = reader_tx.send(Input::ClientClosed);
                    break;
                }
            }
        }
    });

    session::run(out, tx, rx);

    // RUST: `process::exit` ends every thread, including a valo program
    // that is still running when the user pressed Stop.
    std::process::exit(0);
}
