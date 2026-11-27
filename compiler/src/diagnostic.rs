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
//! | change how runtime errors look               | `render_runtime` (trace truncation: `TRACE_HEAD`, `TRACE_TAIL`) |
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
use valo_vm::chunk::SrcSpan;
use valo_vm::error::{ErrorKind, RuntimeError};

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

/// The source text of the indexed value of the index expression at `span`
/// (`rows[1]` for `rows[1][i + 1]`): the text without its final `[…]`,
/// found by scanning back to the matching `[`. A base written over several
/// lines is joined with single spaces. `None` if the text does not have that shape.
fn access_base(sm: &SourceMap, span: SrcSpan) -> Option<String> {
    let text = sm.text.get(span.start as usize..span.end as usize)?;
    let bytes = text.as_bytes();
    if bytes.last() != Some(&b']') {
        return None;
    }
    let mut depth = 0usize;
    let mut open = None;
    for (i, &b) in bytes.iter().enumerate().rev() {
        match b {
            b']' => depth += 1,
            b'[' => {
                depth -= 1;
                if depth == 0 {
                    open = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let base = text[..open?].trim_end();
    if base.is_empty() {
        return None;
    }
    if base.contains('\n') {
        return Some(base.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    Some(base.to_string())
}

/// Innermost stack-trace entries shown for a long trace.
const TRACE_HEAD: usize = 10;
/// Outermost stack-trace entries shown for a long trace.
const TRACE_TAIL: usize = 5;

/// Render a runtime error: header, location, notes and stack trace
/// (plan §5). The source snippet is added in M9. A trace longer than 15
/// entries shows the innermost 10 and the outermost 5; a frame that made
/// tail calls (M5) is followed by `(... N tail calls elided ...)`.
///
/// ```text
/// error[R0701]: assertion failed: a == b
///   --> t.vl:3:8
///   left: 4
///   right: 5
/// stack trace (most recent call first):
///   0: <main> at t.vl:3:8
/// ```
///
/// A failed `assert` quotes its source text: the VM has no source, but the
/// span of the `ASSERT` instruction is the asserted expression.
pub fn render_runtime(sm: &SourceMap, err: &RuntimeError) -> String {
    let message = match (&err.access, err.primary_span()) {
        // R0401 (M8): name the access path, `rows[1][3]: …`.
        (Some(a), Some(span)) => match access_base(sm, span) {
            Some(base) => format!("{base}[{}]: index out of range (len {})", a.index, a.len),
            None => err.message.clone(),
        },
        _ => err.message.clone(),
    };
    let mut out = format!("error[{}]: {message}", err.kind.code());
    if let (ErrorKind::AssertionFailed, Some(span)) = (err.kind, err.primary_span()) {
        let s = clamp(sm, Span::new(span.start as usize, span.end as usize));
        let _ = write!(out, ": {}", &sm.text[s.range()]);
    }
    out.push('\n');
    if let Some(span) = err.primary_span() {
        let (l, c) = sm.line_col(span.start);
        let _ = writeln!(out, "  --> {}:{l}:{c}", sm.name);
    }
    // R0601: where the Err came from and how it got here (D26, Q21).
    if let Some(trail) = &err.trail {
        let at = |site: &valo_vm::error::Site| {
            let (l, c) = sm.line_col(site.span.start);
            format!("{} at {}:{l}:{c}", site.fn_name, sm.name)
        };
        if !trail.context.is_empty() {
            out.push_str("  context (outermost first):\n");
            for msg in trail.context.iter().rev() {
                let _ = writeln!(out, "    {msg}");
            }
        }
        let _ = writeln!(out, "  origin: {}", at(&trail.origin));
        if !trail.propagated.is_empty() {
            out.push_str("  propagated by `?` in:\n");
            for site in &trail.propagated {
                let _ = writeln!(out, "    {}", at(site));
            }
        }
    }
    for note in &err.notes {
        let _ = writeln!(out, "  {note}");
    }
    if !err.trace.is_empty() {
        out.push_str("stack trace (most recent call first):\n");
        // Long traces (deep recursion) keep the innermost `TRACE_HEAD` and
        // the outermost `TRACE_TAIL` entries.
        let n = err.trace.len();
        let shown: Vec<usize> = if n > TRACE_HEAD + TRACE_TAIL {
            (0..TRACE_HEAD).chain(n - TRACE_TAIL..n).collect()
        } else {
            (0..n).collect()
        };
        // Align the "at" column: width of the longest function name shown,
        // and of the largest index.
        let width = shown
            .iter()
            .map(|&i| err.trace[i].fn_name.chars().count())
            .max()
            .unwrap_or(0);
        let iw = (n - 1).to_string().len();
        for (k, &i) in shown.iter().enumerate() {
            if k > 0 && shown[k - 1] + 1 != i {
                let omitted = i - shown[k - 1] - 1;
                let _ = writeln!(out, "  {:iw$}  ... {omitted} frames omitted ...", "");
            }
            let t = &err.trace[i];
            // A built-in running a callback (`<native map>`, M8) has no
            // source position.
            if t.native {
                let _ = writeln!(out, "  {i:>iw$}: {}", t.fn_name);
                continue;
            }
            let (l, c) = sm.line_col(t.span.start);
            let _ = writeln!(
                out,
                "  {i:>iw$}: {:<width$} at {}:{l}:{c}",
                t.fn_name, sm.name
            );
            if t.tail_calls > 0 {
                let s = if t.tail_calls == 1 { "" } else { "s" };
                let _ = writeln!(
                    out,
                    "  {:iw$}  (... {} tail call{s} elided ...)",
                    "", t.tail_calls
                );
            }
        }
    }
    out
}
