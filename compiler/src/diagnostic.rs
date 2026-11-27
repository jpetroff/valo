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
//! | change when colours are used                 | `ColorChoice` (`--color`)                         |
//!
//! Compile and runtime errors share one layout (M9): our own rustc-style
//! header `error[CODE]: message`, then an ariadne box with the source line
//! (its first line `╭─[ file:line:col ]` is the location). ariadne always
//! starts a report with its own `Error: …` header line; `snippet` drops it.
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
        debug_assert!(primary.start <= primary.end, "inverted span in {code}");
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

/// When to colour diagnostics (`--color`, plan D15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorChoice {
    /// Colour when stderr is a terminal and `NO_COLOR` is unset.
    #[default]
    Auto,
    /// Always colour.
    Always,
    /// Never colour (golden tests set `NO_COLOR=1` and use `Auto`).
    Never,
}

impl ColorChoice {
    /// Whether output to stderr should be coloured.
    pub fn stderr(self) -> bool {
        use std::io::IsTerminal;
        match self {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
            }
        }
    }
}

/// The header line shared by compile and runtime errors:
/// `error[E0101]: message` (bold red `error[…]` when coloured).
fn header(out: &mut String, code: &str, message: &str, color: bool) {
    if color {
        let _ = writeln!(
            out,
            "\x1b[1;31merror[{code}]\x1b[0m\x1b[1m: {message}\x1b[0m"
        );
    } else {
        let _ = writeln!(out, "error[{code}]: {message}");
    }
}

/// An ariadne source box for `primary` (+ `secondary` labels and `help:`
/// lines), without ariadne's own header line.
fn snippet(
    sm: &SourceMap,
    primary: Span,
    label: &str,
    secondary: &[(Span, String)],
    help: &[String],
    color: bool,
) -> String {
    // Byte-based indices, because our spans are byte offsets.
    let config = Config::default()
        .with_color(color)
        .with_index_type(IndexType::Byte);
    let name = sm.name.as_str();
    let primary = clamp(sm, primary);
    // ariadne draws no marker for a label without text, so always add one.
    let text = if label.is_empty() { "here" } else { label };
    // An empty message keeps ariadne's header to exactly one line.
    let mut report = Report::build(ReportKind::Error, (name, primary.range()))
        .with_config(config)
        .with_label(Label::new((name, primary.range())).with_message(text));
    for (span, text) in secondary {
        report = report.with_label(Label::new((name, clamp(sm, *span).range())).with_message(text));
    }
    for h in help {
        report = report.with_help(h);
    }
    let mut out = Vec::new();
    let _ = report
        .finish()
        .write((name, Source::from(sm.text.as_str())), &mut out);
    let text = String::from_utf8_lossy(&out);
    match text.split_once('\n') {
        Some((_, rest)) => rest.to_string(),
        None => String::new(),
    }
}

/// Render compile diagnostics, one report each:
///
/// ```text
/// error[E0201]: undefined name `lenght`
///    ╭─[ t.vl:1:7 ]
///    │
///  1 │ print(lenght(xs))
///    │       ───┬──
///    │          ╰──── not found in this scope
///    │
///    │ Help: did you mean `len`?
/// ───╯
/// ```
pub fn render(sm: &SourceMap, diags: &[Diagnostic], color: bool) -> String {
    let mut out = String::new();
    for d in diags {
        header(&mut out, d.code, &d.message, color);
        out.push_str(&snippet(
            sm,
            d.primary,
            &d.label,
            &d.secondary,
            &d.help,
            color,
        ));
    }
    out
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

/// Render a runtime error: header, source box of the innermost frame's
/// instruction (M9), notes and stack trace (plan §5). A trace longer than 15
/// entries shows the innermost 10 and the outermost 5; a frame that made
/// tail calls (M5) is followed by `(... N tail calls elided ...)`.
///
/// ```text
/// error[R0701]: assertion failed: a == b
///    ╭─[ t.vl:3:8 ]
///    │
///  3 │ assert a == b
///    │        ───┬──
///    │           ╰──── here
/// ───╯
///   left: 4
///   right: 5
/// stack trace (most recent call first):
///   0: <main> at t.vl:3:8
/// ```
///
/// A failed `assert` quotes its source text: the VM has no source, but the
/// span of the `ASSERT` instruction is the asserted expression.
pub fn render_runtime(sm: &SourceMap, err: &RuntimeError, color: bool) -> String {
    let message = match (&err.access, err.primary_span()) {
        // R0401 (M8): name the access path, `rows[1][3]: …`.
        (Some(a), Some(span)) => match access_base(sm, span) {
            Some(base) => format!("{base}[{}]: index out of range (len {})", a.index, a.len),
            None => err.message.clone(),
        },
        _ => err.message.clone(),
    };
    let message = match (err.kind, err.primary_span()) {
        (ErrorKind::AssertionFailed, Some(span)) => {
            let s = clamp(sm, Span::new(span.start as usize, span.end as usize));
            format!("{message}: {}", &sm.text[s.range()])
        }
        _ => message,
    };
    let mut out = String::new();
    header(&mut out, err.kind.code(), &message, color);
    if let Some(span) = err.primary_span() {
        let span = Span::new(span.start as usize, span.end as usize);
        out.push_str(&snippet(sm, span, "", &[], &[], color));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `snippet` drops ariadne's own header line; if an ariadne upgrade
    /// changes its layout, this fails before every golden does.
    #[test]
    fn header_is_ours_and_box_follows() {
        let sm = SourceMap::new("t.vl", "1 $ 2\n");
        let d = Diagnostic::new("E0001", "unexpected character `$`", Span::new(2, 3));
        let out = render(&sm, &[d], false);
        let mut lines = out.lines();
        assert_eq!(lines.next(), Some("error[E0001]: unexpected character `$`"));
        assert_eq!(lines.next(), Some("   ╭─[ t.vl:1:3 ]"));
        assert!(!out.contains("Error:"), "{out}");
    }

    #[test]
    fn color_always_marks_the_header() {
        let sm = SourceMap::new("t.vl", "x\n");
        let d = Diagnostic::new("E0201", "undefined name `x`", Span::new(0, 1));
        let out = render(&sm, &[d], true);
        assert!(out.starts_with("\x1b[1;31merror[E0201]"), "{out:?}");
    }
}
