//! # Smoke test: the server as an editor sees it (`tests/smoke.rs`)
//!
//! Plays the editor's part of the protocol against the real server loop
//! (`valo_lsp::server::run`), without processes or stdio:
//! `Connection::memory()` gives two connected ends, one for the "editor"
//! (this test) and one for the server, which runs on its own thread.
//!
//! Each test sends JSON-RPC messages exactly as VS Code would and checks the
//! `textDocument/publishDiagnostics` notifications that come back.
//!
//! **Textbook chapter:** `docs/12-editor-tooling.md`.

use std::thread;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Exit, Initialized,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{HoverRequest, Initialize, Request as _, Shutdown};
use lsp_types::{NumberOrString, Position, PublishDiagnosticsParams, Range};
use serde_json::json;

const URI: &str = "file:///tmp/demo.vl";

/// The editor's end of a running server.
struct Editor {
    conn: Connection,
    server: Option<thread::JoinHandle<()>>,
}

impl Editor {
    /// Start the server thread and do the `initialize` handshake, as
    /// `main.rs` does over stdio.
    fn start() -> Editor {
        let (editor, server) = Connection::memory();
        let handle = thread::spawn(move || {
            let (id, _params) = server.initialize_start().expect("initialize");
            let init = json!({ "capabilities": valo_lsp::server::capabilities() });
            server.initialize_finish(id, init).expect("initialized");
            valo_lsp::server::run(&server).expect("server loop");
        });
        let ed = Editor {
            conn: editor,
            server: Some(handle),
        };
        ed.request(1, Initialize::METHOD, json!({ "capabilities": {} }));
        let resp = ed.recv();
        let Message::Response(resp) = resp else {
            panic!("expected initialize response, got {resp:?}")
        };
        let caps = &resp.response_result.expect("result")["capabilities"];
        assert_eq!(caps["textDocumentSync"], json!(1)); // 1 = FULL
        ed.notify(Initialized::METHOD, json!({}));
        ed
    }

    fn request(&self, id: i32, method: &str, params: serde_json::Value) {
        let req = Request::new(RequestId::from(id), method.to_string(), params);
        self.conn.sender.send(req.into()).unwrap();
    }

    fn notify(&self, method: &str, params: serde_json::Value) {
        let not = Notification::new(method.to_string(), params);
        self.conn.sender.send(not.into()).unwrap();
    }

    fn recv(&self) -> Message {
        self.conn
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("server answered in time")
    }

    /// Wait for the next `publishDiagnostics`.
    fn diagnostics(&self) -> PublishDiagnosticsParams {
        match self.recv() {
            Message::Notification(n) if n.method == PublishDiagnostics::METHOD => {
                serde_json::from_value(n.params).unwrap()
            }
            other => panic!("expected publishDiagnostics, got {other:?}"),
        }
    }

    fn open(&self, text: &str) {
        let doc = json!({ "uri": URI, "languageId": "valo", "version": 1, "text": text });
        self.notify(DidOpenTextDocument::METHOD, json!({ "textDocument": doc }));
    }

    fn change(&self, version: i32, text: &str) {
        self.notify(
            DidChangeTextDocument::METHOD,
            json!({
                "textDocument": { "uri": URI, "version": version },
                "contentChanges": [{ "text": text }],
            }),
        );
    }

    /// `shutdown` request + `exit` notification; the server thread ends.
    fn stop(mut self) {
        self.request(99, Shutdown::METHOD, json!(null));
        let Message::Response(resp) = self.recv() else {
            panic!("expected shutdown response")
        };
        assert_eq!(resp.id, RequestId::from(99));
        self.notify(Exit::METHOD, json!(null));
        self.server.take().unwrap().join().unwrap();
    }
}

fn range(l1: u32, c1: u32, l2: u32, c2: u32) -> Range {
    Range::new(Position::new(l1, c1), Position::new(l2, c2))
}

#[test]
fn open_change_close_publishes_diagnostics() {
    let ed = Editor::start();

    // A parse error: `3 *` has no right operand (the cursor is after `*`).
    ed.open("1 + 2\n3 *\n");
    let d = ed.diagnostics();
    assert_eq!(d.uri.as_str(), URI);
    assert_eq!(d.version, Some(1));
    assert_eq!(d.diagnostics.len(), 1);
    let e = &d.diagnostics[0];
    assert_eq!(e.code, Some(NumberOrString::String("E0101".into())));
    assert_eq!(e.source.as_deref(), Some("valo"));
    assert_eq!(e.range.start, Position::new(1, 3));
    assert!(
        e.message.starts_with("expected an expression"),
        "{}",
        e.message
    );

    // A resolver error, with a help line.
    ed.change(2, "let _ = 1 + 2\n_ * 2\n");
    let d = ed.diagnostics();
    assert_eq!(d.version, Some(2));
    let e = &d.diagnostics[0];
    assert_eq!(e.code, Some(NumberOrString::String("E0206".into())));
    assert_eq!(e.range, range(1, 0, 1, 1));
    assert!(e.message.contains("\nhelp: "), "{}", e.message);

    // Fixed: an empty list clears the squiggles.
    ed.change(
        3,
        "fn double(x: int) -> int = x * 2\n\n21 |> double |> print\n",
    );
    let d = ed.diagnostics();
    assert_eq!(d.version, Some(3));
    assert_eq!(d.diagnostics, Vec::new());

    // Closing a broken file also clears its diagnostics.
    ed.change(4, "3 *\n");
    assert_eq!(ed.diagnostics().diagnostics.len(), 1);
    ed.notify(
        DidCloseTextDocument::METHOD,
        json!({ "textDocument": { "uri": URI } }),
    );
    let d = ed.diagnostics();
    assert_eq!(d.diagnostics, Vec::new());
    assert_eq!(d.version, None);

    ed.stop();
}

#[test]
fn unsupported_requests_get_an_error_not_silence() {
    let ed = Editor::start();
    ed.request(
        7,
        HoverRequest::METHOD,
        json!({ "textDocument": { "uri": URI }, "position": { "line": 0, "character": 0 } }),
    );
    let Message::Response(resp) = ed.recv() else {
        panic!("expected a response")
    };
    assert_eq!(resp.id, RequestId::from(7));
    assert!(resp.response_result.is_err());
    ed.stop();
}
