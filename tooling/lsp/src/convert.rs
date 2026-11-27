//! # Compiler diagnostics → LSP diagnostics (`convert.rs`)
//!
//! **What this file is:** the translation from the compiler's error type
//! (`valo_compiler::diagnostic::Diagnostic`: code, message, byte spans,
//! labels, help lines) to the LSP `Diagnostic` the editor draws as a squiggle
//! with a hover message.
//!
//! | valo `Diagnostic`      | LSP `Diagnostic`                                   |
//! |------------------------|----------------------------------------------------|
//! | `primary` (byte span)  | `range` (line + UTF-16 column, via `LineIndex`)    |
//! | `code` (`E0101`)       | `code`, shown next to the message                  |
//! | `message` + `label` + `help` | `message` (one text, several lines)          |
//! | `secondary` spans      | `relatedInformation` (clickable locations)         |
//! | (always an error)      | `severity = ERROR`, `source = "valo"`              |
//!
//! ## Where to change things
//!
//! | I want to…                               | Look at                         |
//! |------------------------------------------|---------------------------------|
//! | warnings / hints (once the compiler has them) | `severity` in `diagnostic`  |
//! | link each code to its documentation      | set `code_description`          |
//! | quick fixes from `help` lines (L4)       | keep `help` structured in `data`, read it in a code-action handler |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - iterator `map` + `collect` into a `Vec` (*Closures and iterators*).
//! - `bool::then_some` to build an `Option` "only if not empty" (*Option and Result*).

use lsp_types::{
    DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Range, Uri,
};
use valo_compiler::diagnostic::Diagnostic;
use valo_compiler::span::Span;

use crate::line_index::LineIndex;

/// LSP range of a byte span.
pub fn range(ix: &LineIndex, text: &str, span: Span) -> Range {
    Range::new(ix.position(text, span.start), ix.position(text, span.end))
}

/// Convert one compiler diagnostic. `uri` names the document, for
/// `relatedInformation` locations (the MVP compiles one file at a time, so
/// every secondary span is in the same document).
pub fn diagnostic(uri: &Uri, ix: &LineIndex, text: &str, d: &Diagnostic) -> lsp_types::Diagnostic {
    let mut message = d.message.clone();
    if !d.label.is_empty() {
        message.push('\n');
        message.push_str(&d.label);
    }
    for help in &d.help {
        message.push_str("\nhelp: ");
        message.push_str(help);
    }

    let related: Vec<DiagnosticRelatedInformation> = d
        .secondary
        .iter()
        .map(|(span, text_)| DiagnosticRelatedInformation {
            location: Location::new(uri.clone(), range(ix, text, *span)),
            message: text_.clone(),
        })
        .collect();

    lsp_types::Diagnostic {
        range: range(ix, text, d.primary),
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(d.code.to_string())),
        source: Some("valo".to_string()),
        message,
        // RUST: `cond.then_some(value)` is `Some(value)` if `cond`, else `None`.
        related_information: (!related.is_empty()).then_some(related),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;
    use pretty_assertions::assert_eq;
    use std::str::FromStr;

    #[test]
    fn converts_all_parts() {
        let text = "let x = 1\nlet x2 = ä + y\n";
        let ix = LineIndex::new(text);
        let uri = Uri::from_str("file:///t.vl").unwrap();
        let y = text.find('y').unwrap();
        let d = Diagnostic::new("E0201", "unknown name `y`", Span::new(y, y + 1))
            .label("not defined")
            .with(Span::new(4, 5), "did you mean this?")
            .help("define `y` before using it");
        let out = diagnostic(&uri, &ix, text, &d);
        // `ä` is 2 bytes but one UTF-16 unit: `y` is at column 13, not 14.
        assert_eq!(
            out.range,
            Range::new(Position::new(1, 13), Position::new(1, 14))
        );
        assert_eq!(out.code, Some(NumberOrString::String("E0201".into())));
        assert_eq!(
            out.message,
            "unknown name `y`\nnot defined\nhelp: define `y` before using it"
        );
        let related = out.related_information.unwrap();
        assert_eq!(
            related[0].location.range,
            Range::new(Position::new(0, 4), Position::new(0, 5))
        );
        assert_eq!(related[0].message, "did you mean this?");
    }
}
