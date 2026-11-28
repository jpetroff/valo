//! # DAP message framing (`protocol.rs`)
//!
//! **What this file is:** the wire format. DAP uses the same framing as LSP
//! (chapter 12): a header, a blank line, then a JSON body of exactly
//! `Content-Length` bytes.
//!
//! ```text
//! Content-Length: 83\r\n
//! \r\n
//! {"seq":1,"type":"request","command":"initialize","arguments":{"adapterID":"valo"}}
//! ```
//!
//! Unlike LSP it is **not** JSON-RPC. There are three message types, told
//! apart by the `type` field:
//!
//! | `type`     | Direction        | Fields                                                    |
//! |------------|------------------|-----------------------------------------------------------|
//! | `request`  | client → adapter (and rarely adapter → client) | `seq`, `command`, `arguments`  |
//! | `response` | answer to a request | `seq`, `request_seq`, `success`, `command`, `message`, `body` |
//! | `event`    | adapter → client, unasked | `seq`, `event`, `body` (`stopped`, `output`, `exited`, …) |
//!
//! Every message carries its own increasing `seq` number. Responses point back
//! at their request with `request_seq`.
//!
//! **Textbook chapter:** `docs/13-debugger.md` (§13.2).
//!
//! ## Where to change things
//!
//! | I want to…                        | Look at                     |
//! |-----------------------------------|-----------------------------|
//! | send a new kind of event          | `Outbox::event` (callers pick the name and body) |
//! | log every message for debugging   | `Outbox::send` / `read_message` (log to stderr, never stdout) |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `Mutex<T>` — a lock: the session thread and the program thread both
//!   write messages, and a message must never be cut in half by another.
//! - `Box<dyn Write + Send>` — "any writer that may move between threads"
//!   (stdout in the binary, a pipe in tests).
//! - `serde_json::Value` and the `json!` macro — untyped JSON trees.

use std::io::{self, BufRead, Write};
use std::sync::Mutex;

use serde_json::{Value, json};

/// Read one framed message. `Ok(None)` means the client closed the stream.
///
/// Headers end at an empty line; only `Content-Length` matters (the spec
/// allows others, which are ignored).
pub fn read_message(r: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue; // stray blank line between messages
        }
        if let Some(n) = line.strip_prefix("Content-Length:") {
            length = Some(n.trim().parse().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bad Content-Length: {e}"),
                )
            })?);
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    r.read_exact(&mut body)?;
    let msg = serde_json::from_slice(&body)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("bad JSON: {e}")))?;
    Ok(Some(msg))
}

/// Write one framed message.
pub fn write_message(w: &mut impl Write, msg: &Value) -> io::Result<()> {
    let body = msg.to_string();
    write!(w, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    w.flush()
}

/// The adapter's outgoing side: numbers messages and writes them whole.
///
/// Shared (`Arc<Outbox>`) between the session loop and the thread running
/// the valo program, which sends `output` events while it prints.
pub struct Outbox {
    // RUST: the lock guards the writer *and* the sequence counter together,
    // so `seq` numbers reach the client in increasing order.
    inner: Mutex<(Box<dyn Write + Send>, i64)>,
}

impl Outbox {
    /// Wrap a writer (stdout in `main.rs`).
    pub fn new(w: Box<dyn Write + Send>) -> Outbox {
        Outbox {
            inner: Mutex::new((w, 0)),
        }
    }

    /// Stamp `seq` on a message and write it. Write errors mean the client
    /// is gone; there is nobody left to tell, so they are only logged.
    pub fn send(&self, mut msg: Value) {
        // RUST: `lock()` fails only if another thread panicked while holding
        // the lock; the data is still usable, so take it anyway.
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (w, seq) = &mut *guard;
        *seq += 1;
        msg["seq"] = json!(*seq);
        if let Err(e) = write_message(w, &msg) {
            eprintln!("valo-dap: cannot write to client: {e}");
        }
    }

    /// A successful response to `req`, with an optional body.
    pub fn respond(&self, req: &Value, body: Value) {
        self.send(json!({
            "type": "response",
            "request_seq": req["seq"],
            "command": req["command"],
            "success": true,
            "body": body,
        }));
    }

    /// A failed response. VS Code shows `body.error.format` to the user
    /// (for `launch`: as a dialog).
    pub fn fail(&self, req: &Value, message: &str) {
        self.send(json!({
            "type": "response",
            "request_seq": req["seq"],
            "command": req["command"],
            "success": false,
            "message": message,
            "body": { "error": { "id": 1, "format": message, "showUser": true } },
        }));
    }

    /// An event (`initialized`, `output`, `stopped`, `exited`, `terminated`, …).
    pub fn event(&self, name: &str, body: Value) {
        self.send(json!({ "type": "event", "event": name, "body": body }));
    }

    /// Text for the Debug Console. `category`: `stdout` and `stderr` for the
    /// program, `console` for messages from the adapter itself.
    pub fn output(&self, category: &str, text: &str) {
        self.event("output", json!({ "category": category, "output": text }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_round_trips() {
        let msg = json!({ "seq": 1, "type": "request", "command": "threads" });
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let text = String::from_utf8(buf.clone()).unwrap();
        assert!(text.starts_with("Content-Length: "));
        let mut r = io::Cursor::new(buf);
        assert_eq!(read_message(&mut r).unwrap(), Some(msg));
        assert_eq!(read_message(&mut r).unwrap(), None);
    }

    #[test]
    fn length_counts_bytes_not_characters() {
        let msg = json!({ "output": "π ≈ 3.14" });
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut r = io::Cursor::new(buf);
        assert_eq!(read_message(&mut r).unwrap(), Some(msg));
    }
}
