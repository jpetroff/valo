//! # The server loop (`server.rs`)
//!
//! **What this file is:** what the server does after the `initialize`
//! handshake. It receives every message from the editor, keeps the text of
//! each open `.vl` file in memory, re-checks a file with the real compiler
//! whenever it changes, and sends the errors back as
//! `textDocument/publishDiagnostics`.
//!
//! LSP has three message kinds, all handled in `run`:
//!
//! - **requests** (have an id, need exactly one response): `shutdown` now;
//!   `textDocument/definition`, `hover`, … later;
//! - **notifications** (no id, no response): `didOpen`, `didChange`,
//!   `didClose`;
//! - **responses** to requests the *server* sent to the editor (none yet).
//!
//! The editor owns the truth about open files: after `didOpen` the server
//! must use the text it was sent, not the file on disk (which is stale until
//! the user saves).
//!
//! **Textbook chapter:** `docs/12-editor-tooling.md`.
//!
//! ## Where to change things
//!
//! | I want to…                                   | Look at                                     |
//! |----------------------------------------------|---------------------------------------------|
//! | advertise a new feature to the editor        | `capabilities` (e.g. `definition_provider`) |
//! | answer a new request (hover, definition, …)  | `handle_request`                            |
//! | react to a new notification (didSave, …)     | `handle_notification`                       |
//! | run more/fewer compiler stages               | `check`                                     |
//! | send changes as edits, not whole text        | `TextDocumentSyncKind::INCREMENTAL` + `didChange` |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `HashMap<Uri, Document>` — the open-document store (*Collections*).
//! - generic functions with a trait bound (`fn notification<N: Notification>`)
//!   and associated constants (`N::METHOD`) (*Traits, impl and derive*).
//! - `std::panic::catch_unwind` — a compiler bug must not kill the editor's
//!   server (*Errors and panics*).

use std::collections::HashMap;
use std::error::Error;
use std::panic::{AssertUnwindSafe, catch_unwind};

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    Notification as LspNotification, PublishDiagnostics,
};
use lsp_types::{
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    Uri,
};
use valo_compiler::span::SourceMap;
use valo_compiler::{CompileOptions, compile_source};

use crate::convert;
use crate::line_index::LineIndex;

/// Errors that end the server (broken connection, protocol violations).
pub type ServerError = Box<dyn Error + Send + Sync>;

/// What the server can do, sent in the `initialize` response. The editor
/// only sends requests for features listed here.
///
/// EXTEND: each roadmap step (tooling/lsp/README.md) turns on one more field,
/// e.g. `definition_provider: Some(OneOf::Left(true))` for go-to-definition.
pub fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        // FULL: every change sends the whole text. Simple and fast enough for
        // script-sized files; INCREMENTAL (only the edited range) is an
        // optimisation for later.
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        ..Default::default()
    }
}

/// One open file, as the editor last sent it.
#[derive(Debug)]
struct Document {
    text: String,
    /// Editor's version number, echoed in diagnostics so the editor can drop
    /// results that belong to an older text.
    version: i32,
}

/// Server state: the open documents.
#[derive(Debug, Default)]
struct Server {
    docs: HashMap<Uri, Document>,
}

/// Main loop: handle messages until the editor asks to shut down.
pub fn run(connection: &Connection) -> Result<(), ServerError> {
    let mut server = Server::default();
    // RUST: iterating a channel receiver blocks until the next message
    // arrives and ends when the editor closes the connection.
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                server.handle_request(connection, req)?;
            }
            Message::Notification(not) => server.handle_notification(connection, not)?,
            Message::Response(_) => {}
        }
    }
    Ok(())
}

impl Server {
    fn handle_request(&mut self, connection: &Connection, req: Request) -> Result<(), ServerError> {
        // EXTEND: match on `req.method` here (`lsp_types::request::HoverRequest::METHOD`, …)
        // and answer with `Response::new_ok(req.id, result)`.
        let resp = Response::new_err(
            req.id,
            ErrorCode::MethodNotFound as i32,
            format!("valo-lsp does not support {} yet", req.method),
        );
        connection.sender.send(resp.into())?;
        Ok(())
    }

    fn handle_notification(
        &mut self,
        connection: &Connection,
        not: Notification,
    ) -> Result<(), ServerError> {
        match not.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let p = params::<DidOpenTextDocument>(not)?;
                let doc = Document {
                    text: p.text_document.text,
                    version: p.text_document.version,
                };
                self.docs.insert(p.text_document.uri.clone(), doc);
                self.publish(connection, &p.text_document.uri)?;
            }
            DidChangeTextDocument::METHOD => {
                let p = params::<DidChangeTextDocument>(not)?;
                // With FULL sync there is exactly one change: the whole text.
                if let Some(change) = p.content_changes.into_iter().last() {
                    let doc = Document {
                        text: change.text,
                        version: p.text_document.version,
                    };
                    self.docs.insert(p.text_document.uri.clone(), doc);
                    self.publish(connection, &p.text_document.uri)?;
                }
            }
            DidCloseTextDocument::METHOD => {
                let p = params::<DidCloseTextDocument>(not)?;
                self.docs.remove(&p.text_document.uri);
                // Clear the squiggles of a closed file.
                send_diagnostics(connection, p.text_document.uri, Vec::new(), None)?;
            }
            // Notifications we do not handle are ignored (the protocol allows that).
            _ => {}
        }
        Ok(())
    }

    /// Check one open document and send its diagnostics.
    fn publish(&self, connection: &Connection, uri: &Uri) -> Result<(), ServerError> {
        let Some(doc) = self.docs.get(uri) else {
            return Ok(());
        };
        let diagnostics = check(uri, &doc.text);
        send_diagnostics(connection, uri.clone(), diagnostics, Some(doc.version))
    }
}

/// Run the compiler on a text and convert its errors. An empty list means
/// "no errors" and clears earlier squiggles.
///
/// EXTEND: when the compiler gains passes that report errors without
/// stopping (M9 multi-error recovery, M10 types), they show up here for free,
/// because this calls the same `compile_source` as `valo check`.
pub fn check(uri: &Uri, text: &str) -> Vec<lsp_types::Diagnostic> {
    let sm = SourceMap::new(uri.as_str(), text);
    // RUST: `catch_unwind` turns a panic inside the closure into an `Err`
    // instead of ending the process; `AssertUnwindSafe` tells the compiler we
    // accept that `sm` may be half-used after a panic (we drop it anyway).
    let result = catch_unwind(AssertUnwindSafe(|| {
        compile_source(&sm, CompileOptions::default())
    }));
    match result {
        Ok(Ok(_program)) => Vec::new(),
        Ok(Err(diags)) => {
            let ix = LineIndex::new(text);
            diags
                .iter()
                .map(|d| convert::diagnostic(uri, &ix, text, d))
                .collect()
        }
        Err(_) => {
            eprintln!("valo-lsp: the compiler panicked on {}", uri.as_str());
            Vec::new()
        }
    }
}

/// Decode the parameters of a notification of type `N`.
fn params<N: LspNotification>(not: Notification) -> Result<N::Params, ServerError> {
    Ok(serde_json::from_value(not.params)?)
}

/// Send `textDocument/publishDiagnostics` for one file.
fn send_diagnostics(
    connection: &Connection,
    uri: Uri,
    diagnostics: Vec<lsp_types::Diagnostic>,
    version: Option<i32>,
) -> Result<(), ServerError> {
    let params = PublishDiagnosticsParams {
        uri,
        diagnostics,
        version,
    };
    let not = Notification::new(PublishDiagnostics::METHOD.to_string(), params);
    connection.sender.send(not.into())?;
    Ok(())
}
