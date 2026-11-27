//! # Tokens (`token.rs`)
//!
//! **What this file is:** the vocabulary of the language — the list of
//! "words" the lexer can produce: literals (`Int`, `Float`), names (`Ident`),
//! reserved words (`Kw(Keyword)`), punctuation (`Plus`, `Arrow`, …) and the
//! layout tokens `Newline`, `Indent`, `Dedent` that make indentation
//! meaningful (valo is indentation-based like Python), plus `Eof`.
//!
//! **Pipeline position:** produced by `lexer.rs`, consumed by `parser.rs`.
//!
//! **Textbook chapter:** `docs/03-lexical-analysis.md`.
//!
//! ## Where to change things
//!
//! | I want to…                         | Look at                                                         |
//! |------------------------------------|-----------------------------------------------------------------|
//! | add a keyword                      | `enum Keyword`, `Keyword::lookup`, `Keyword::as_str`            |
//! | add an operator / punctuation      | `enum TokenKind` + its spelling in `impl fmt::Display`; then recognise it in `lexer.rs` (`Lexer::token`) |
//! | change "expected X, found Y" texts | `TokenKind::describe`                                           |
//!
//! The full punctuation and keyword set of `.memory/syntax.md` is lexed from
//! M1 on, even where the parser does not accept it yet, so later milestones
//! only touch the parser.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `enum TokenKind` with data-carrying variants (`Int(i64)`, `Kw(Keyword)`) (*Enums*).
//! - `Option<Keyword>` return from `lookup` (*Option and Result*).
//! - `impl fmt::Display` (*Traits, impl and derive*).
//! - `PartialEq` without `Eq` on `TokenKind`: floats (`f64`) can be NaN,
//!   which is not equal to itself, so full `Eq` is impossible.

use std::fmt;

use crate::span::Span;

/// Reserved words (syntax.md S2–S25).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyword {
    Let,
    Fn,
    Def,
    As,
    If,
    Elif,
    Else,
    Then,
    Match,
    And,
    Or,
    Not,
    True,
    False,
    None,
    Import,
    From,
    For,
    In,
    Assert,
    Test,
    Mock,
}

impl Keyword {
    /// Keyword for an identifier spelling, if reserved.
    pub fn lookup(s: &str) -> Option<Keyword> {
        Some(match s {
            "let" => Keyword::Let,
            "fn" => Keyword::Fn,
            "def" => Keyword::Def,
            "as" => Keyword::As,
            "if" => Keyword::If,
            "elif" => Keyword::Elif,
            "else" => Keyword::Else,
            "then" => Keyword::Then,
            "match" => Keyword::Match,
            "and" => Keyword::And,
            "or" => Keyword::Or,
            "not" => Keyword::Not,
            "true" => Keyword::True,
            "false" => Keyword::False,
            "none" => Keyword::None,
            "import" => Keyword::Import,
            "from" => Keyword::From,
            "for" => Keyword::For,
            "in" => Keyword::In,
            "assert" => Keyword::Assert,
            "test" => Keyword::Test,
            "mock" => Keyword::Mock,
            _ => return None,
        })
    }

    /// Source spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Keyword::Let => "let",
            Keyword::Fn => "fn",
            Keyword::Def => "def",
            Keyword::As => "as",
            Keyword::If => "if",
            Keyword::Elif => "elif",
            Keyword::Else => "else",
            Keyword::Then => "then",
            Keyword::Match => "match",
            Keyword::And => "and",
            Keyword::Or => "or",
            Keyword::Not => "not",
            Keyword::True => "true",
            Keyword::False => "false",
            Keyword::None => "none",
            Keyword::Import => "import",
            Keyword::From => "from",
            Keyword::For => "for",
            Keyword::In => "in",
            Keyword::Assert => "assert",
            Keyword::Test => "test",
            Keyword::Mock => "mock",
        }
    }
}

/// Token kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// Integer literal (already range-checked).
    Int(i64),
    /// Float literal (parsed from M3 on).
    Float(f64),
    /// Identifier, including `_` and capitalised constructor names.
    Ident(String),
    /// Reserved word.
    Kw(Keyword),
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `**`
    StarStar,
    /// `/`
    Slash,
    /// `//`
    SlashSlash,
    /// `%`
    Percent,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// `:`
    Colon,
    /// `=`
    Eq,
    /// `==`
    EqEq,
    /// `!=`
    NotEq,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `->` (types)
    Arrow,
    /// `=>` (lambdas)
    FatArrow,
    /// `~>` (match arms)
    SquigArrow,
    /// `|>` (pipe)
    PipeGt,
    /// `|` (unions, process pipes)
    Bar,
    /// `!` (result types)
    Bang,
    /// `?` (error propagation)
    Question,
    /// End of a logical line.
    Newline,
    /// Indentation increased.
    Indent,
    /// Indentation decreased (one per level).
    Dedent,
    /// End of input.
    Eof,
}

impl TokenKind {
    /// Short description for "expected X, found Y" messages.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Int(n) => format!("integer `{n}`"),
            TokenKind::Float(x) => format!("float `{x:?}`"),
            TokenKind::Ident(s) => format!("name `{s}`"),
            TokenKind::Kw(k) => format!("keyword `{}`", k.as_str()),
            TokenKind::Newline => "end of line".into(),
            TokenKind::Indent => "indentation".into(),
            TokenKind::Dedent => "end of indented block".into(),
            TokenKind::Eof => "end of file".into(),
            other => format!("`{other}`"),
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TokenKind::Int(n) => return write!(f, "{n}"),
            TokenKind::Float(x) => return write!(f, "{x:?}"),
            TokenKind::Ident(s) => return f.write_str(s),
            TokenKind::Kw(k) => k.as_str(),
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::StarStar => "**",
            TokenKind::Slash => "/",
            TokenKind::SlashSlash => "//",
            TokenKind::Percent => "%",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::Comma => ",",
            TokenKind::Dot => ".",
            TokenKind::Colon => ":",
            TokenKind::Eq => "=",
            TokenKind::EqEq => "==",
            TokenKind::NotEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::Le => "<=",
            TokenKind::Gt => ">",
            TokenKind::Ge => ">=",
            TokenKind::Arrow => "->",
            TokenKind::FatArrow => "=>",
            TokenKind::SquigArrow => "~>",
            TokenKind::PipeGt => "|>",
            TokenKind::Bar => "|",
            TokenKind::Bang => "!",
            TokenKind::Question => "?",
            TokenKind::Newline => "NEWLINE",
            TokenKind::Indent => "INDENT",
            TokenKind::Dedent => "DEDENT",
            TokenKind::Eof => "EOF",
        };
        f.write_str(s)
    }
}

/// A token and its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// What it is.
    pub kind: TokenKind,
    /// Where it is.
    pub span: Span,
}
