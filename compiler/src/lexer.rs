//! # The lexer (`lexer.rs`)
//!
//! **What this file is:** the first compiler stage ("lexical analysis",
//! "scanning", "tokenizing"). It reads the source text character by
//! character and groups characters into tokens: `12`, `+`, `**`, `let`,
//! `name`, … It also throws away what the parser does not need (spaces,
//! comments) and turns *indentation* into explicit tokens.
//!
//! **Pipeline position:** `&str` source in → `Lexed { tokens, diags }` out →
//! `parser.rs`. Entry point: `lex`.
//!
//! **Textbook chapter:** `docs/03-lexical-analysis.md`.
//!
//! ## Layout rules (Python's "offside rule")
//!
//! - an indentation stack produces `INDENT`/`DEDENT` at the start of logical
//!   lines and `NEWLINE` at their end (`Lexer::line_start`);
//! - blank and comment-only lines are ignored;
//! - line breaks inside `()`, `[]`, `{}` are ignored (implicit line joining;
//!   field `depth`);
//! - a line whose first token is `|>` continues the previous line (D24).
//!
//! The `NEWLINE` ending a line is held back (`pending_newline`) until the next
//! non-blank line is seen, so a pipe continuation can cancel it.
//!
//! ## Where to change things
//!
//! | I want to…                                   | Look at                                              |
//! |----------------------------------------------|------------------------------------------------------|
//! | recognise a new operator (e.g. `+=`)         | `Lexer::token`, the big `match c` (longest spelling first), plus `TokenKind` in `token.rs` |
//! | add a keyword                                | only `token.rs` (`Keyword`); `Lexer::ident` looks it up |
//! | change number literal rules                  | `Lexer::number`, `digits`, `bad_suffix`, `check_underscores` |
//! | change string literals (escapes, raw, `{}`)  | `Lexer::string`, `string_body`, `escape`, `raw_body`; the `'}'` arm of `Lexer::token` ends an interpolation |
//! | change indentation rules (tabs, widths)      | `Lexer::line_start`                                  |
//! | change line-continuation rules               | `Lexer::line_start` (pipe-continuation case) and `Lexer::run` (`'\n'` arm) |
//! | change a lexical error message               | search for the code, e.g. `"E0010"`                  |
//!
//! Lexical error codes: E0001 unexpected character, E0002 integer out of
//! range, E0003 bad dedent, E0004 unterminated string, E0005 invalid escape,
//! E0006 tab in indentation, E0007 unterminated interpolation, E0008 empty
//! interpolation `{}`, E0009 syntax not supported yet, E0010 malformed
//! number.
//!
//! ## Strings (M7, syntax.md S12, plan D25)
//!
//! `"a {x + 1} b"` becomes `STR_START STR_TEXT("a ") INTERP_START x + 1
//! INTERP_END STR_TEXT(" b") STR_END`. Between `INTERP_START` and
//! `INTERP_END` the lexer is in normal mode, so any expression — even
//! another string — can appear; a stack of open interpolations
//! (`Lexer::interps`) tells which `}` ends one. Escapes are decoded here;
//! `r"…"` has none and no interpolation.
//!
//! PROVISIONAL-SYNTAX (recorded in `.memory/syntax.md`): indentation uses
//! spaces only (a tab in indentation is E0006); `_` digit separators are
//! allowed between digits; `0x`/`0o`/`0b` integer literals; decimal integers
//! may not have leading zeros (Python rule).
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `struct Lexer<'a>` holding `&'a str` — borrows the source text instead of
//!   copying it (*Lifetimes*).
//! - `&self.src[self.pos..]` — string slicing by byte offset; `.chars()` to
//!   iterate characters (*Strings and characters*).
//! - `while let Some(c) = self.peek()` — loop while an `Option` has a value (*Pattern matching*).
//! - match guards: `'-' if two(self, "->") => ...` (*Pattern matching*).
//! - `Option<Span>` and `.take()` (*Option and Result*).

use crate::diagnostic::Diagnostic;
use crate::span::Span;
use crate::token::{Keyword, Token, TokenKind};

/// Lexer output. Tokens always end with `EOF`, even when there are errors.
#[derive(Debug)]
pub struct Lexed {
    /// The token stream.
    pub tokens: Vec<Token>,
    /// Lexical errors (E00xx).
    pub diags: Vec<Diagnostic>,
}

/// Tokenize a whole source text. This is the only public entry point.
pub fn lex(src: &str) -> Lexed {
    let mut lx = Lexer {
        src,
        pos: 0,
        tokens: Vec::new(),
        diags: Vec::new(),
        indents: vec![0],
        depth: 0,
        at_line_start: true,
        line_has_tokens: false,
        pending_newline: None,
        interps: Vec::new(),
    };
    lx.run();
    Lexed {
        tokens: lx.tokens,
        diags: lx.diags,
    }
}

/// Lexer state while scanning one source text.
// RUST: `<'a>` is a lifetime parameter: `src` is borrowed from the caller and
// the `Lexer` may not outlive it. No copy of the source is made.
struct Lexer<'a> {
    /// The whole source text.
    src: &'a str,
    /// Byte offset of the next unread character.
    pos: usize,
    /// Output so far.
    tokens: Vec<Token>,
    /// Errors so far (the lexer keeps going after an error).
    diags: Vec<Diagnostic>,
    /// Indentation widths of the open blocks; always starts with 0.
    indents: Vec<u32>,
    /// Open bracket depth; layout is suspended while > 0.
    depth: u32,
    /// True right after a newline (outside brackets): indentation must be measured.
    at_line_start: bool,
    /// Whether the current physical line produced any token (blank lines don't).
    line_has_tokens: bool,
    /// A NEWLINE token waiting to be emitted (see the module guide).
    pending_newline: Option<Span>,
    /// Open `{…}` interpolations inside string literals, innermost last
    /// (the *mode stack*, plan D25): while one is open the lexer produces
    /// ordinary tokens, and the `}` that closes it returns to string text.
    interps: Vec<Interp>,
}

/// An open interpolation `"…{expr}…"`.
#[derive(Debug, Clone, Copy)]
struct Interp {
    /// Bracket depth outside the interpolation (`depth` is one more while
    /// it is open, which also suppresses line layout inside it).
    depth: u32,
    /// Byte offset of the string's opening quote.
    string_start: usize,
    /// Byte offset of the `{`.
    brace: usize,
}

// RUST: `impl Lexer<'_>` — methods for any lifetime; `'_` means "some
// lifetime we don't need to name here".
impl Lexer<'_> {
    // ---- small helpers -------------------------------------------------

    /// The next character, without consuming it (`None` at end of input).
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    /// The character `n` positions ahead (0 = same as `peek`).
    fn peek_at(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    /// Does the unread input start with `s`?
    fn starts_with(&self, s: &str) -> bool {
        self.src[self.pos..].starts_with(s)
    }

    /// Emit a token spanning `start..pos`.
    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start, self.pos),
        });
        self.line_has_tokens = true;
    }

    /// Record a lexical error spanning `start..end`.
    // RUST: `msg: impl Into<String>` accepts both `"literal"` and a `String`
    // built with `format!`.
    fn error(&mut self, code: &'static str, msg: impl Into<String>, start: usize, end: usize) {
        self.diags
            .push(Diagnostic::new(code, msg, Span::new(start, end)));
    }

    // ---- main loop -----------------------------------------------------

    /// Scan the whole input.
    fn run(&mut self) {
        loop {
            if self.at_line_start && self.depth == 0 {
                self.at_line_start = false;
                self.line_start();
            }
            // Skip intra-line whitespace.
            while let Some(c) = self.peek() {
                if c == ' ' || c == '\t' || c == '\r' {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            // RUST: let-else — stop the loop at end of input.
            let Some(c) = self.peek() else { break };
            match c {
                '#' => self.skip_to_eol(),
                '\n' if !self.interps.is_empty() => self.unclosed_interps(),
                '\n' => {
                    let start = self.pos;
                    self.pos += 1;
                    // Inside brackets a newline is just whitespace.
                    if self.depth == 0 {
                        if self.line_has_tokens {
                            // Don't emit NEWLINE yet: the next line may be a
                            // `|>` continuation (see `line_start`).
                            self.pending_newline = Some(Span::new(start, start + 1));
                            self.line_has_tokens = false;
                        }
                        self.at_line_start = true;
                    }
                }
                _ => self.token(c),
            }
        }
        self.finish();
    }

    /// Handle indentation at the start of a physical line (outside brackets).
    ///
    /// Compares this line's indentation width with the top of `indents`:
    /// deeper → push + `INDENT`; shallower → pop + one `DEDENT` per closed
    /// level; a width matching no open level → E0003.
    fn line_start(&mut self) {
        let start = self.pos;
        let mut width: u32 = 0;
        let mut tab_reported = false;
        while let Some(c) = self.peek() {
            match c {
                ' ' => width += 1,
                '\t' => {
                    if !tab_reported {
                        self.error(
                            "E0006",
                            "tab character in indentation; indent with spaces",
                            self.pos,
                            self.pos + 1,
                        );
                        tab_reported = true;
                    }
                    width += 1;
                }
                '\r' => {}
                _ => break,
            }
            self.pos += 1;
        }
        match self.peek() {
            // Blank line, comment-only line or EOF: layout unaffected.
            None | Some('\n') | Some('#') => return,
            _ => {}
        }
        if self.starts_with("|>") {
            // Pipe continuation: the previous logical line goes on.
            self.pending_newline = None;
            return;
        }
        // A real new line: release the held-back NEWLINE first.
        // RUST: `Option::take()` returns the value and leaves `None` behind.
        if let Some(nl) = self.pending_newline.take() {
            self.tokens.push(Token {
                kind: TokenKind::Newline,
                span: nl,
            });
        }
        // RUST: `*` dereferences: `last()` gives `Option<&u32>`, `expect`
        // unwraps to `&u32`, `*` copies out the `u32`.
        let top = *self.indents.last().expect("indent stack never empty");
        if width > top {
            self.indents.push(width);
            self.tokens.push(Token {
                kind: TokenKind::Indent,
                span: Span::new(start, self.pos),
            });
        } else if width < top {
            while width < *self.indents.last().expect("indent stack never empty") {
                self.indents.pop();
                self.tokens.push(Token {
                    kind: TokenKind::Dedent,
                    span: Span::new(self.pos, self.pos),
                });
            }
            if width != *self.indents.last().expect("indent stack never empty") {
                self.error(
                    "E0003",
                    "unindent does not match any outer indentation level",
                    start,
                    self.pos,
                );
                // Recover by treating this width as a new level of the outer block.
                self.indents.push(width);
            }
        }
    }

    /// End of input: final NEWLINE, close all open blocks, then EOF.
    fn finish(&mut self) {
        self.unclosed_interps();
        let end = self.src.len();
        if let Some(nl) = self.pending_newline.take() {
            self.tokens.push(Token {
                kind: TokenKind::Newline,
                span: nl,
            });
        } else if self.line_has_tokens {
            // Last line had no trailing '\n'.
            self.tokens.push(Token {
                kind: TokenKind::Newline,
                span: Span::new(end, end),
            });
        }
        while self.indents.len() > 1 {
            self.indents.pop();
            self.tokens.push(Token {
                kind: TokenKind::Dedent,
                span: Span::new(end, end),
            });
        }
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(end, end),
        });
    }

    /// Skip a `#` comment (up to, not including, the newline).
    fn skip_to_eol(&mut self) {
        match self.src[self.pos..].find('\n') {
            Some(i) => self.pos += i,
            None => self.pos = self.src.len(),
        }
    }

    // ---- individual tokens ---------------------------------------------

    /// Lex one token starting with character `c`.
    fn token(&mut self, c: char) {
        let start = self.pos;
        if c.is_ascii_digit() {
            return self.number();
        }
        if c.is_alphabetic() || c == '_' {
            return self.ident();
        }
        // RUST: a closure; `two(self, "->")` checks for a 2-character operator.
        let two = |s: &Self, t: &str| s.starts_with(t);
        // "Maximal munch": always try the longest spelling first, so `**` is
        // one token, not two `*`. The guard (`if ...`) picks the arm.
        // EXTEND: new operators go here (longer spellings above shorter ones).
        let (kind, len) = match c {
            '+' => (TokenKind::Plus, 1),
            '-' if two(self, "->") => (TokenKind::Arrow, 2),
            '-' => (TokenKind::Minus, 1),
            '*' if two(self, "**") => (TokenKind::StarStar, 2),
            '*' => (TokenKind::Star, 1),
            '/' if two(self, "//") => (TokenKind::SlashSlash, 2),
            '/' => (TokenKind::Slash, 1),
            '%' => (TokenKind::Percent, 1),
            '(' => (TokenKind::LParen, 1),
            ')' => (TokenKind::RParen, 1),
            '[' => (TokenKind::LBracket, 1),
            ']' => (TokenKind::RBracket, 1),
            '{' => (TokenKind::LBrace, 1),
            // The `}` that closes an interpolation goes back to string text.
            '}' if self
                .interps
                .last()
                .is_some_and(|i| i.depth + 1 == self.depth) =>
            {
                let i = self.interps.pop().expect("checked above");
                self.depth = i.depth;
                self.pos += 1;
                self.push(TokenKind::InterpEnd, start);
                return self.string_body(i.string_start);
            }
            '}' => (TokenKind::RBrace, 1),
            ',' => (TokenKind::Comma, 1),
            '.' => (TokenKind::Dot, 1),
            ':' => (TokenKind::Colon, 1),
            '=' if two(self, "==") => (TokenKind::EqEq, 2),
            '=' if two(self, "=>") => (TokenKind::FatArrow, 2),
            '=' if two(self, "=~") => return self.unsupported(2, "the regex match operator `=~`"),
            '=' => (TokenKind::Eq, 1),
            '!' if two(self, "!=") => (TokenKind::NotEq, 2),
            '!' if two(self, "!~") => return self.unsupported(2, "the regex operator `!~`"),
            '!' => (TokenKind::Bang, 1),
            '<' if two(self, "<=") => (TokenKind::Le, 2),
            '<' => (TokenKind::Lt, 1),
            '>' if two(self, ">=") => (TokenKind::Ge, 2),
            '>' => (TokenKind::Gt, 1),
            '~' if two(self, "~>") => (TokenKind::SquigArrow, 2),
            '|' if two(self, "|>") => (TokenKind::PipeGt, 2),
            '|' => (TokenKind::Bar, 1),
            '?' => (TokenKind::Question, 1),
            '"' => return self.string(1, false),
            '`' => return self.quoted(0, '`', "command literals"),
            other => {
                // RUST: `len_utf8()` — a char may take 1–4 bytes in UTF-8.
                self.pos += other.len_utf8();
                self.error(
                    "E0001",
                    format!("unexpected character `{other}`"),
                    start,
                    self.pos,
                );
                return;
            }
        };
        self.pos += len;
        // Track bracket depth for implicit line joining.
        if matches!(
            kind,
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace
        ) {
            self.depth += 1;
        } else if matches!(
            kind,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
        ) {
            self.depth = self.depth.saturating_sub(1);
        }
        self.push(kind, start);
    }

    /// Reserved syntax from a later milestone: report E0009 and skip it.
    fn unsupported(&mut self, len: usize, what: &str) {
        let start = self.pos;
        self.pos += len;
        self.error(
            "E0009",
            format!("{what} is not supported yet"),
            start,
            self.pos,
        );
    }

    /// A quoted literal not supported yet (strings: M7; commands, regexes:
    /// post-MVP). Skip to the closing quote on the same line to avoid cascades.
    fn quoted(&mut self, prefix: usize, quote: char, what: &str) {
        let start = self.pos;
        self.pos += prefix + 1;
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.pos += c.len_utf8();
            if c == '\\' {
                // Skip the escaped character so `\"` does not end the literal.
                if let Some(n) = self.peek().filter(|n| *n != '\n') {
                    self.pos += n.len_utf8();
                }
            } else if c == quote {
                break;
            }
        }
        self.error(
            "E0009",
            format!("{what} are not supported yet"),
            start,
            self.pos,
        );
        // Keep the line non-empty so layout stays consistent for the parser.
        self.line_has_tokens = true;
    }

    // ---- strings (M7, syntax.md S12, plan D25) ---------------------------

    /// A string literal whose opening (`"` or `r"`) is `open_len` bytes at
    /// `pos`. A raw string has no escapes and no interpolation.
    fn string(&mut self, open_len: usize, raw: bool) {
        let start = self.pos;
        self.pos += open_len;
        self.push(TokenKind::StrStart, start);
        if raw {
            self.raw_body(start)
        } else {
            self.string_body(start)
        }
    }

    /// Text of a normal string, from `pos` up to the closing `"` (emits
    /// `StrText`, `StrEnd`) or to a `{` (emits `InterpStart` and returns:
    /// the expression is lexed as ordinary tokens, see `interps`).
    fn string_body(&mut self, string_start: usize) {
        let mut text = String::new();
        let mut text_start = self.pos;
        loop {
            let Some(c) = self.peek() else {
                return self.unterminated(string_start, text, text_start);
            };
            match c {
                '"' => {
                    self.flush_text(&mut text, text_start);
                    let q = self.pos;
                    self.pos += 1;
                    self.push(TokenKind::StrEnd, q);
                    return;
                }
                '\n' => return self.unterminated(string_start, text, text_start),
                '\\' => self.escape(&mut text),
                '{' => {
                    self.flush_text(&mut text, text_start);
                    let brace = self.pos;
                    // `{}` or `{  }`: nothing to interpolate (E0008).
                    let inner = &self.src[brace + 1..];
                    let blank = inner.len() - inner.trim_start_matches([' ', '\t']).len();
                    if inner[blank..].starts_with('}') {
                        self.pos = brace + 1 + blank + 1;
                        self.diags.push(
                            Diagnostic::new(
                                "E0008",
                                "empty interpolation `{}` in a string",
                                Span::new(brace, self.pos),
                            )
                            .label("expected an expression between the braces")
                            .help("write `\\{` and `\\}` for literal braces"),
                        );
                        text_start = self.pos;
                        continue;
                    }
                    self.pos += 1;
                    self.push(TokenKind::InterpStart, brace);
                    self.interps.push(Interp {
                        depth: self.depth,
                        string_start,
                        brace,
                    });
                    self.depth += 1;
                    return;
                }
                _ => {
                    text.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    /// Text of a raw string `r"…"` up to the closing `"`: no escapes, no
    /// interpolation (so it cannot contain `"`).
    fn raw_body(&mut self, string_start: usize) {
        let text_start = self.pos;
        let rest = &self.src[self.pos..];
        let end = rest.find(['"', '\n']).unwrap_or(rest.len());
        let mut text = rest[..end].to_string();
        self.pos += end;
        if self.peek() != Some('"') {
            return self.unterminated(string_start, std::mem::take(&mut text), text_start);
        }
        self.flush_text(&mut text, text_start);
        let q = self.pos;
        self.pos += 1;
        self.push(TokenKind::StrEnd, q);
    }

    /// Emit the text collected so far (if any) as one `StrText`.
    fn flush_text(&mut self, text: &mut String, text_start: usize) {
        if !text.is_empty() {
            self.push(TokenKind::StrText(std::mem::take(text)), text_start);
        }
    }

    /// E0004: the string reaches the end of the line or file. The token
    /// stream is closed with `StrEnd` so the line still looks complete.
    fn unterminated(&mut self, string_start: usize, mut text: String, text_start: usize) {
        self.flush_text(&mut text, text_start);
        self.push(TokenKind::StrEnd, self.pos);
        // Inside an unclosed `{…}`, the `"` that started this string was
        // most likely meant to close the outer one (`"a {x"`): only the
        // interpolation is reported (E0007, at the end of the line).
        if !self.interps.is_empty() {
            return;
        }
        self.diags.push(
            Diagnostic::new(
                "E0004",
                "unterminated string",
                Span::new(string_start, self.pos),
            )
            .label("this string has no closing `\"` on its line")
            .help("strings end on the line where they start; write `\\n` for a line break"),
        );
    }

    /// One escape sequence after `\\` (at `pos`), decoded into `text`:
    /// `\\n \\t \\r \\0 \\" \\\\ \\{ \\} \\u{1F600}`; anything else is E0005.
    fn escape(&mut self, text: &mut String) {
        let start = self.pos;
        self.pos += 1;
        let Some(c) = self.peek().filter(|c| *c != '\n') else {
            return self.bad_escape(start, "a `\\` at the end of the line");
        };
        self.pos += c.len_utf8();
        let decoded = match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0' => '\0',
            '"' => '"',
            '\\' => '\\',
            '{' => '{',
            '}' => '}',
            'u' => match self.unicode_escape() {
                Some(ch) => ch,
                None => {
                    return self.bad_escape(
                        start,
                        "a malformed unicode escape; write it as `\\u{1F600}` (1–6 hex digits)",
                    );
                }
            },
            other => {
                return self.bad_escape(start, &format!("an unknown escape `\\{other}`"));
            }
        };
        text.push(decoded);
    }

    /// The `{hex}` part of `\\u{…}` (after the `u`): a Unicode scalar value.
    fn unicode_escape(&mut self) -> Option<char> {
        let rest = &self.src[self.pos..];
        let body = rest.strip_prefix('{')?;
        let close = body.find(['}', '"', '\n'])?;
        if !body[close..].starts_with('}') {
            return None;
        }
        let hex = &body[..close];
        self.pos += 1 + close + 1;
        if hex.is_empty() || hex.len() > 6 {
            return None;
        }
        char::from_u32(u32::from_str_radix(hex, 16).ok()?)
    }

    /// E0005 for the escape starting at `start` (up to `pos`).
    fn bad_escape(&mut self, start: usize, what: &str) {
        self.diags.push(
            Diagnostic::new(
                "E0005",
                "invalid escape sequence in a string",
                Span::new(start, self.pos),
            )
            .label(what.to_string())
            .help("valid escapes: \\n \\t \\r \\0 \\\" \\\\ \\{ \\} \\u{…}; use a raw string r\"…\" for backslashes"),
        );
    }

    /// E0007 for every interpolation still open at the end of a line or of
    /// the file: an interpolated expression must end on its line.
    fn unclosed_interps(&mut self) {
        while let Some(i) = self.interps.pop() {
            self.depth = i.depth;
            self.diags.push(
                Diagnostic::new(
                    "E0007",
                    "unterminated interpolation in a string",
                    Span::new(i.brace, i.brace + 1),
                )
                .label("this `{` is never closed")
                .with(
                    Span::new(i.string_start, i.string_start + 1),
                    "in the string starting here",
                )
                .help("close it with `}` on the same line, or write `\\{` for a literal brace"),
            );
        }
    }

    /// Identifier or keyword: a letter or `_`, then letters, digits, `_`.
    fn ident(&mut self) {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        let text = &self.src[start..self.pos];
        // `r"..."` and `re"..."` are string-like literals, not names.
        if self.peek() == Some('"') && text == "r" {
            self.pos = start;
            return self.string(2, true);
        }
        if self.peek() == Some('"') && text == "re" {
            self.pos = start;
            return self.quoted(text.len(), '"', "regex literals");
        }
        let kind = match Keyword::lookup(text) {
            Some(k) => TokenKind::Kw(k),
            None => TokenKind::Ident(text.to_string()),
        };
        self.push(kind, start);
    }

    /// Number literal: `123`, `1_000`, `0xff`, `0o755`, `0b1010`, `1.5`, `2e3`.
    fn number(&mut self) {
        let start = self.pos;
        // RUST: matching a tuple of two Options; `'x' | 'X'` = either char.
        let radix = match (self.peek(), self.peek_at(1)) {
            (Some('0'), Some('x' | 'X')) => 16,
            (Some('0'), Some('o' | 'O')) => 8,
            (Some('0'), Some('b' | 'B')) => 2,
            _ => 10,
        };
        if radix != 10 {
            // ---- 0x / 0o / 0b integers ----
            self.pos += 2;
            let digits_start = self.pos;
            self.digits(radix);
            let digits = self.src[digits_start..self.pos].replace('_', "");
            if self.bad_suffix(start) {
                return;
            }
            if digits.is_empty() {
                self.error(
                    "E0010",
                    "missing digits after the radix prefix",
                    start,
                    self.pos,
                );
                return;
            }
            if !self.check_underscores(start, digits_start) {
                return;
            }
            return match i64::from_str_radix(&digits, radix) {
                Ok(n) => self.push(TokenKind::Int(n), start),
                Err(_) => self.out_of_range(start),
            };
        }

        // ---- decimal integers and floats ----
        self.digits(10);
        let mut is_float = false;
        // A fraction needs a digit after the dot: `1.x` is `1`, `.`, `x`.
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.pos += 1;
            self.digits(10);
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let sign = usize::from(matches!(self.peek_at(1), Some('+' | '-')));
            if self.peek_at(1 + sign).is_some_and(|c| c.is_ascii_digit()) {
                is_float = true;
                self.pos += 1 + sign;
                self.digits(10);
            }
        }
        if self.bad_suffix(start) || !self.check_underscores(start, start) {
            return;
        }
        let text = self.src[start..self.pos].replace('_', "");
        if is_float {
            // RUST: `parse::<f64>()` — the `::<T>` ("turbofish") names the
            // target type of a generic function.
            match text.parse::<f64>() {
                Ok(x) => self.push(TokenKind::Float(x), start),
                Err(_) => self.error("E0010", "malformed float literal", start, self.pos),
            }
            return;
        }
        if text.len() > 1 && text.starts_with('0') && text.bytes().any(|b| b != b'0') {
            self.error(
                "E0010",
                "leading zeros are not allowed in decimal integer literals",
                start,
                self.pos,
            );
            self.diags
                .last_mut()
                .expect("just pushed")
                .help
                .push("use 0o for octal".into());
            return;
        }
        // i64 overflow (e.g. 9223372036854775808) → E0002.
        match text.parse::<i64>() {
            Ok(n) => self.push(TokenKind::Int(n), start),
            Err(_) => self.out_of_range(start),
        }
    }

    /// Consume digits of the given radix and `_` separators.
    fn digits(&mut self, radix: u32) {
        while let Some(c) = self.peek() {
            if c == '_' || c.is_digit(radix) {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// `12abc`, `0x1g`: report the whole run as one malformed literal.
    fn bad_suffix(&mut self, start: usize) -> bool {
        if !self.peek().is_some_and(|c| c.is_alphanumeric()) {
            return false;
        }
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        let text = &self.src[start..self.pos];
        self.error(
            "E0010",
            format!("invalid number literal `{text}`"),
            start,
            self.pos,
        );
        true
    }

    /// `_` separators must sit between two digits.
    fn check_underscores(&mut self, start: usize, digits_start: usize) -> bool {
        let text = &self.src[digits_start..self.pos];
        let b = text.as_bytes();
        // RUST: `.all(|(i, &c)| ...)` is true if the closure is true for every
        // (index, byte) pair.
        let ok = b.iter().enumerate().all(|(i, &c)| {
            c != b'_'
                || (i > 0
                    && i + 1 < b.len()
                    && b[i - 1].is_ascii_alphanumeric()
                    && b[i + 1].is_ascii_alphanumeric())
        });
        if !ok {
            self.error(
                "E0010",
                "`_` in a number must sit between two digits",
                start,
                self.pos,
            );
        }
        ok
    }

    fn out_of_range(&mut self, start: usize) {
        self.error(
            "E0002",
            "integer literal is out of range for int (64-bit)",
            start,
            self.pos,
        );
        self.diags
            .last_mut()
            .expect("just pushed")
            .help
            .push(format!("the largest int is {}", i64::MAX));
    }
}

/// One token per line, for `--dump tokens` and snapshots.
/// Format: `start..end KIND` (byte offsets).
pub fn dump(tokens: &[Token]) -> String {
    tokens
        .iter()
        .map(|t| format!("{:>4}..{:<4} {}\n", t.span.start, t.span.end, t.kind))
        .collect()
}

// Tests: `cargo test -p valo-compiler lexer` runs only these.
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Token kinds joined by spaces, e.g. "1 + 2 NEWLINE EOF".
    fn kinds(src: &str) -> String {
        let l = lex(src);
        assert!(l.diags.is_empty(), "unexpected diagnostics: {:?}", l.diags);
        l.tokens
            .iter()
            .map(|t| t.kind.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Error codes produced for a source.
    fn codes(src: &str) -> Vec<&'static str> {
        lex(src).diags.iter().map(|d| d.code).collect()
    }

    // Snapshot tests: the expected output lives in
    // `src/snapshots/valo_compiler__lexer__tests__*.snap`.
    #[test]
    fn operators_maximal_munch() {
        insta::assert_snapshot!(lex_dump(
            "-7 // 2 ** 3 % (1 + 2) * 4 / 5 -> => ~> |> | == != <= >= ! ?"
        ));
    }

    #[test]
    fn indentation_blocks() {
        insta::assert_snapshot!(lex_dump(
            "fn f(x: int) -> int:\n    let y = x\n\n    # comment\n    if y:\n        1\n    else:\n        2\nf(3)\n"
        ));
    }

    fn lex_dump(src: &str) -> String {
        let l = lex(src);
        assert!(l.diags.is_empty(), "{:?}", l.diags);
        dump(&l.tokens)
    }

    #[test]
    fn brackets_join_lines() {
        assert_eq!(kinds("(1 +\n   2)\n3"), "( 1 + 2 ) NEWLINE 3 NEWLINE EOF");
    }

    #[test]
    fn pipe_continues_line() {
        assert_eq!(
            kinds("xs\n    |> f\n    |> g\n1"),
            "xs |> f |> g NEWLINE 1 NEWLINE EOF"
        );
    }

    #[test]
    fn dedent_flushed_at_eof() {
        assert_eq!(
            kinds("a:\n  b:\n    c"),
            "a : NEWLINE INDENT b : NEWLINE INDENT c NEWLINE DEDENT DEDENT EOF"
        );
    }

    #[test]
    fn comments_and_shebang() {
        assert_eq!(
            kinds("#!/usr/bin/env valo\n1 # one\n## doc\n2"),
            "1 NEWLINE 2 NEWLINE EOF"
        );
    }

    #[test]
    fn numbers() {
        assert_eq!(
            kinds("1_000 0xff 0o755 0b1010 0 00 1.5 2e3 1e-2 1.x"),
            "1000 255 493 10 0 0 1.5 2000.0 0.01 1 . x NEWLINE EOF"
        );
    }

    #[test]
    fn keywords_are_reserved() {
        assert_eq!(
            kinds("let fn def as _ Ok"),
            "let fn def as _ Ok NEWLINE EOF"
        );
        let l = lex("none");
        assert_eq!(l.tokens[0].kind, TokenKind::Kw(Keyword::None));
    }

    #[test]
    fn lexical_errors() {
        assert_eq!(codes("1 $ 2"), ["E0001"]);
        assert_eq!(codes("9223372036854775808"), ["E0002"]);
        assert_eq!(codes("9223372036854775807"), Vec::<&str>::new());
        assert_eq!(codes("a:\n    b\n  c"), ["E0003"]);
        assert_eq!(codes("a:\n\tb"), ["E0006"]);
        assert_eq!(codes("`ls` re\"y\" a =~ b !~ c"), ["E0009"; 4]);
        assert!(codes("\"hi {x}\" r\"\\d{2}\"").is_empty());
        assert_eq!(codes("\"abc"), ["E0004"]);
        assert_eq!(codes("r\"abc\nx"), ["E0004"]);
        assert_eq!(codes("\"a\\qb\" \"\\u{110000}\" \"\\u{}\""), ["E0005"; 3]);
        assert_eq!(codes("\"a {x\nb"), ["E0007"]);
        assert_eq!(codes("\"a {x"), ["E0007"]);
        assert_eq!(codes("\"a {} b { } c\""), ["E0008"; 2]);
        assert_eq!(codes("007 1__0 12ab 0x"), ["E0010"; 4]);
    }

    // Snapshot: src/snapshots/valo_compiler__lexer__tests__strings.snap
    #[test]
    fn strings() {
        // Escapes, a raw string, an interpolation holding a nested string
        // with its own interpolation, and a dict-like `{…}` inside one.
        insta::assert_snapshot!(lex_dump(
            "\"a\\t\\\"b\\u{e9} \\{x\\}\"\nr\"\\d{2}\"\n\"{n + 1} and {\"in{m}\"}!\"\n\"{ f({1: 2}) }\""
        ));
    }

    // Property tests: random inputs must never crash the lexer.
    proptest! {
        #[test]
        fn never_panics_and_ends_with_eof(src in "\\PC{0,64}") {
            let l = lex(&src);
            prop_assert_eq!(&l.tokens.last().unwrap().kind, &TokenKind::Eof);
        }

        #[test]
        fn indents_balance(src in "[ a:\n#\t(]{0,64}") {
            let l = lex(&src);
            let ind = l.tokens.iter().filter(|t| t.kind == TokenKind::Indent).count();
            let ded = l.tokens.iter().filter(|t| t.kind == TokenKind::Dedent).count();
            prop_assert!(ded <= ind + l.diags.len());
        }
    }
}
