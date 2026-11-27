//! # Source positions (`span.rs`)
//!
//! **What this file is:** how the compiler remembers *where* in the source
//! text something came from. A `Span` is a half-open byte range
//! `[start, end)`; every token, AST node and emitted instruction carries one,
//! so any error — at compile time or at runtime — can point at the exact
//! source characters. A `SourceMap` owns the file's text and converts byte
//! offsets to human `line:column` positions.
//!
//! **Pipeline position:** used by every stage (lexer, parser, codegen,
//! diagnostics). `Span` converts to the VM's `SrcSpan` (in
//! `vm/src/chunk.rs`) when it is stored in bytecode.
//!
//! **Textbook chapter:** `docs/02-source-text-and-spans.md`.
//!
//! ## Where to change things
//!
//! | I want to…                              | Look at                              |
//! |-----------------------------------------|--------------------------------------|
//! | change how columns are counted (tabs?)  | `SourceMap::line_col`                |
//! | support several files (modules, later)  | add a file id to `Span` and a list of files to a new map type |
//!
//! Spans store byte offsets because slicing Rust strings is by byte; line and
//! column are computed only when an error is printed. The MVP compiles one
//! file at a time, so spans carry no file id yet.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `impl From<Span> for SrcSpan` — conversion usable as `span.into()` (*Traits, impl and derive*).
//! - `impl Into<String>` parameters (*Strings and characters*).
//! - `u32` vs `usize` and `as` casts (*Integer types and casts*).

use std::ops::Range;

use valo_vm::chunk::SrcSpan;

/// Half-open byte range into the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    /// First byte.
    pub start: u32,
    /// One past the last byte.
    pub end: u32,
}

impl Span {
    /// Construct a span.
    pub fn new(start: usize, end: usize) -> Span {
        Span {
            start: start as u32,
            end: end as u32,
        }
    }

    /// Smallest span covering both.
    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// As a `usize` range.
    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

impl From<Span> for SrcSpan {
    fn from(s: Span) -> SrcSpan {
        SrcSpan::new(s.start, s.end)
    }
}

impl From<SrcSpan> for Span {
    fn from(s: SrcSpan) -> Span {
        Span {
            start: s.start,
            end: s.end,
        }
    }
}

/// A source file and its line index.
#[derive(Debug, Clone)]
pub struct SourceMap {
    /// Display name (path as given on the command line).
    pub name: String,
    /// Full text.
    pub text: String,
    /// Byte offset at which each line starts.
    pub line_starts: Vec<u32>,
}

impl SourceMap {
    /// Index a source text.
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> SourceMap {
        let text = text.into();
        let line_starts = valo_vm::chunk::line_starts(&text);
        SourceMap {
            name: name.into(),
            text,
            line_starts,
        }
    }

    /// 1-based line and 1-based column (in characters) of a byte offset.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        // Clamp so an end-of-file position never indexes past the text.
        let offset = offset.min(self.text.len() as u32);
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.line_starts[line] as usize;
        // RUST: `&self.text[a..b]` borrows a slice of the string by *byte*
        // range; `.chars().count()` counts Unicode characters in it, so a
        // 2-byte `ä` counts as one column.
        let col = self.text[start..offset as usize].chars().count();
        (line as u32 + 1, col as u32 + 1)
    }

    /// Text covered by a span.
    pub fn slice(&self, span: Span) -> &str {
        &self.text[span.range()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_counts_chars() {
        let sm = SourceMap::new("t.vl", "ab\nä+c\n");
        assert_eq!(sm.line_col(0), (1, 1));
        assert_eq!(sm.line_col(3), (2, 1));
        assert_eq!(sm.line_col(5), (2, 2)); // after 2-byte 'ä'
        assert_eq!(sm.line_col(8), (3, 1));
    }
}
