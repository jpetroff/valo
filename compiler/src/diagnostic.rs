//! # Diagnostics (`diagnostic.rs`)
//!
//! **What this file is:** how errors are represented and printed.
//! `Diagnostic` is a compile-time error (code `E####`, message, the source
//! span to underline, optional labels and help lines). `render` prints
//! diagnostics with source snippets using the `ariadne` crate.
//! `render_runtime` prints a VM `RuntimeError` (code `R####`) with its
//! location and stack trace.
//!
//! **Pipeline position:** every compiler stage creates `Diagnostic`s;
//! `main.rs` renders them to stderr.
//!
//! **Textbook chapter:** `docs/08-errors-and-diagnostics.md`.
//!
//! ## Where to change things
//!
//! | I want to…                                   | Look at                                          |
//! |----------------------------------------------|--------------------------------------------------|
//! | create an error in a compiler stage          | `Diagnostic::new(code, message, span)` + `.label()`, `.with()`, `.help()` |
//! | change how compile errors look               | `render` (ariadne `Config`, labels)              |
//! | change how runtime errors look               | `render_runtime`                                  |
//! | change when colours are used                 | `stderr_color`                                    |
//!
//! Compile errors currently use ariadne's own header (`[E0101] Error: …`)
//! while runtime errors use `error[R0101]: …`; unifying them is part of M9.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - builder methods taking and returning `self` (`.label(..).help(..)`) (*Ownership and borrowing*).
//! - an external crate's API (`ariadne::Report::build(...)...finish().write(...)`) (*Cargo, crates and workspaces*).
//! - `use std::fmt::Write as _;` — import a trait only for its methods (*Traits, impl and derive*).

use std::fmt::Write as _;

use ariadne::{Config, IndexType, Label, Report, ReportKind, Source};
use valo_vm::error::RuntimeError;

use crate::span::{SourceMap, Span};

/// A compile-time error with a stable `E####` code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Stable code, e.g. `E0101`.
    pub code: &'static str,
    /// One-line message.
    pub message: String,
    /// Where the problem is.
    pub primary: Span,
    /// Text attached to the primary span (may be empty).
    pub label: String,
    /// Further annotated spans.
    pub secondary: Vec<(Span, String)>,
    /// `help:` lines.
    pub help: Vec<String>,
}

impl Diagnostic {
    /// New diagnostic with a primary span.
    ///
    /// ```
    /// use valo_compiler::diagnostic::Diagnostic;
    /// use valo_compiler::span::Span;
    /// let d = Diagnostic::new("E0101", "unexpected token", Span::new(4, 5))
    ///     .label("here")
    ///     .help("remove it");
    /// assert_eq!(d.code, "E0101");
    /// ```
    pub fn new(code: &'static str, message: impl Into<String>, primary: Span) -> Diagnostic {
        Diagnostic {
            code,
            message: message.into(),
            primary,
            label: String::new(),
            secondary: Vec::new(),
            help: Vec::new(),
        }
    }

    /// Set the primary label.
    pub fn label(mut self, text: impl Into<String>) -> Diagnostic {
        self.label = text.into();
        self
    }

    /// Add a secondary label.
    pub fn with(mut self, span: Span, text: impl Into<String>) -> Diagnostic {
        self.secondary.push((span, text.into()));
        self
    }

    /// Add a help line.
    pub fn help(mut self, text: impl Into<String>) -> Diagnostic {
        self.help.push(text.into());
        self
    }
}

/// Whether diagnostics on stderr should be coloured: stderr is a terminal and
/// `NO_COLOR` is unset (plan D15). Golden tests set `NO_COLOR=1`.
pub fn stderr_color() -> bool {
    use std::io::IsTerminal;
    std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
}

/// Render compile diagnostics, one report each.
pub fn render(sm: &SourceMap, diags: &[Diagnostic], color: bool) -> String {
    let mut out = Vec::new();
    // Byte-based indices, because our spans are byte offsets.
    let config = Config::default()
        .with_color(color)
        .with_index_type(IndexType::Byte);
    let name = sm.name.as_str();
    for d in diags {
        let primary = clamp(sm, d.primary);
        // ariadne draws no marker for a label without text, so always add one.
        let text = if d.label.is_empty() {
            "here"
        } else {
            d.label.as_str()
        };
        let label = Label::new((name, primary.range())).with_message(text);
        let mut report = Report::build(ReportKind::Error, (name, primary.range()))
            .with_config(config)
            .with_code(d.code)
            .with_message(&d.message)
            .with_label(label);
        for (span, text) in &d.secondary {
            report =
                report.with_label(Label::new((name, clamp(sm, *span).range())).with_message(text));
        }
        for h in &d.help {
            report = report.with_help(h);
        }
        let _ = report
            .finish()
            .write((name, Source::from(sm.text.as_str())), &mut out);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Keep spans inside the text so ariadne never panics on EOF positions.
fn clamp(sm: &SourceMap, s: Span) -> Span {
    let len = sm.text.len() as u32;
    Span {
        start: s.start.min(len),
        end: s.end.min(len).max(s.start.min(len)),
    }
}

/// Render a runtime error: header, location and stack trace (plan §5). The
/// source snippet is added in M9.
///
/// ```text
/// error[R0102]: division by zero: 7 // 0
///   --> div_zero.vl:2:1
/// stack trace (most recent call first):
///   0: <main> at div_zero.vl:2:1
/// ```
pub fn render_runtime(sm: &SourceMap, err: &RuntimeError) -> String {
    let mut out = format!("error[{}]: {}\n", err.kind.code(), err.message);
    if let Some(span) = err.primary_span() {
        let (l, c) = sm.line_col(span.start);
        let _ = writeln!(out, "  --> {}:{l}:{c}", sm.name);
    }
    if !err.trace.is_empty() {
        out.push_str("stack trace (most recent call first):\n");
        // Align the "at" column: width of the longest function name.
        let width = err
            .trace
            .iter()
            .map(|t| t.fn_name.chars().count())
            .max()
            .unwrap_or(0);
        for (i, t) in err.trace.iter().enumerate() {
            let (l, c) = sm.line_col(t.span.start);
            let _ = writeln!(out, "  {i}: {:<width$} at {}:{l}:{c}", t.fn_name, sm.name);
        }
    }
    out
}
