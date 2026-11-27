//! # Positions for the editor (`line_index.rs`)
//!
//! **What this file is:** conversion between the two ways of saying *where*
//! something is in a file:
//!
//! - the compiler uses **byte offsets** into the UTF-8 text (`Span`, see
//!   `compiler/src/span.rs`), because Rust slices strings by byte;
//! - LSP uses **line + character**, both 0-based, where "character" counts
//!   **UTF-16 code units** (the string encoding of JavaScript, in which VS
//!   Code is written).
//!
//! For ASCII text all three units (bytes, chars, UTF-16 units) agree. They
//! differ for other text: `ä` is 2 bytes / 1 UTF-16 unit, `😀` is 4 bytes /
//! 2 UTF-16 units. Getting this wrong puts squiggles in the wrong column on
//! any line that contains such characters.
//!
//! `SourceMap::line_col` in the compiler is *not* reused: it produces 1-based
//! columns in characters, for human-readable messages.
//!
//! ## Where to change things
//!
//! | I want to…                                  | Look at                       |
//! |---------------------------------------------|-------------------------------|
//! | support `\r` line breaks (old Mac files)    | `LineIndex::new`              |
//! | use UTF-8 columns (LSP 3.17 `positionEncoding`) | `position` / `offset`     |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `char::len_utf16`, `char::len_utf8` — size of one character in each
//!   encoding (*Strings and characters*).
//! - `binary_search` on a sorted `Vec` (*Collections*).

use lsp_types::Position;

/// Start offsets of every line of one text, for fast offset → line lookup.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset where each line starts; `line_starts[0] == 0`.
    line_starts: Vec<u32>,
}

impl LineIndex {
    /// Index a text. Lines end at `\n`; a `\r` before it (Windows `\r\n`)
    /// stays part of the line, which only matters for a position *after* it.
    pub fn new(text: &str) -> LineIndex {
        let mut line_starts = vec![0];
        // RUST: `match_indices` yields `(byte_offset, matched_str)` pairs.
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i as u32 + 1));
        LineIndex { line_starts }
    }

    /// LSP position of a byte offset. Offsets past the end, or inside a
    /// multi-byte character, are clamped to the nearest valid place before.
    pub fn position(&self, text: &str, offset: u32) -> Position {
        let offset = (offset as usize).min(text.len());
        let line = match self.line_starts.binary_search(&(offset as u32)) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.line_starts[line] as usize;
        let mut character = 0u32;
        // RUST: `char_indices` yields each char with its byte offset.
        for (i, c) in text[start..].char_indices() {
            if start + i + c.len_utf8() > offset {
                break;
            }
            character += c.len_utf16() as u32;
        }
        Position::new(line as u32, character)
    }

    /// Byte offset of an LSP position (the inverse of [`LineIndex::position`]).
    /// A character past the end of its line maps to the line end; a line past
    /// the end of the text maps to the end of the text.
    ///
    /// EXTEND: needed from L2 on (go-to-definition, hover: the editor sends
    /// a cursor position and we look up the AST node at that offset).
    pub fn offset(&self, text: &str, pos: Position) -> u32 {
        let Some(&start) = self.line_starts.get(pos.line as usize) else {
            return text.len() as u32;
        };
        let start = start as usize;
        let mut units = 0u32;
        for (i, c) in text[start..].char_indices() {
            if c == '\n' || units >= pos.character {
                return (start + i) as u32;
            }
            units += c.len_utf16() as u32;
        }
        text.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position::new(line, character)
    }

    #[test]
    fn ascii_lines() {
        let text = "let a = 1\nlet b = 2\n";
        let ix = LineIndex::new(text);
        assert_eq!(ix.position(text, 0), pos(0, 0));
        assert_eq!(ix.position(text, 4), pos(0, 4));
        assert_eq!(ix.position(text, 10), pos(1, 0));
        assert_eq!(ix.position(text, 14), pos(1, 4));
        // End of file: after the final newline, on the (empty) last line.
        assert_eq!(ix.position(text, 20), pos(2, 0));
        assert_eq!(ix.position(text, 999), pos(2, 0));
    }

    #[test]
    fn utf16_columns() {
        // `ä` = 2 bytes, 1 UTF-16 unit; `😀` = 4 bytes, 2 UTF-16 units.
        let text = "# ä😀 x\n";
        let ix = LineIndex::new(text);
        let x = text.find('x').unwrap() as u32; // byte 9
        assert_eq!(x, 9);
        assert_eq!(ix.position(text, x), pos(0, 6));
        assert_eq!(ix.offset(text, pos(0, 6)), x);
        // An offset inside `😀` clamps back to its start.
        assert_eq!(ix.position(text, 6), pos(0, 3));
    }

    #[test]
    fn offset_round_trips_and_clamps() {
        let text = "ab\nä+c\n";
        let ix = LineIndex::new(text);
        for offset in [0, 1, 2, 3, 5, 6, 7, 8] {
            assert_eq!(ix.offset(text, ix.position(text, offset)), offset);
        }
        assert_eq!(ix.offset(text, pos(0, 99)), 2); // clamped to line end
        assert_eq!(ix.offset(text, pos(9, 0)), text.len() as u32);
    }
}
