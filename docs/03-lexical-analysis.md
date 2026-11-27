# Chapter 3 — Lexical Analysis

## 3.1 Theory: what a lexer does

The **lexer** (scanner, tokenizer) turns a stream of characters into a stream
of **tokens** — the "words" of the language. In `(1 + 2) * 3`:

```text
characters:  (  1  ␣  +  ␣  2  )  ␣  *  ␣  3  \n
tokens:      (  1     +     2  )     *     3  NEWLINE
```

Doing this in a separate stage keeps the parser simple: the parser never sees
spaces or comments, and never has to decide whether `**` is one operator or
two. Classic textbooks describe tokens with regular expressions and build a
finite automaton; production compilers (rustc, Go, Clang, CPython) usually
hand-write the lexer instead, because it is short, fast and gives better
error messages. valo does the same.

Two classic principles:

- **Maximal munch**: always take the longest token possible. `**` is
  `StarStar`, not two `Star`s; `->` is `Arrow`, not `Minus`, `Gt`.
- **Lookahead**: decide by peeking at the next characters without consuming
  them. valo's lexer needs at most two characters of lookahead (`1.5` vs
  `1.x`, `2e3` vs `2e+3`).

### Layout: the offside rule

valo is indentation-based like Python. Indentation is turned into explicit
tokens so that the parser can treat blocks like brackets:

```text
fn f(x: int) -> int:          fn f ( x : int ) -> int : NEWLINE
    let y = x                 INDENT let y = x NEWLINE
    y                         y NEWLINE
f(3)                          DEDENT f ( 3 ) NEWLINE EOF
```

The algorithm (from Python's tokenizer) keeps a **stack of indentation
widths**, starting with `[0]`. At the start of each non-blank line, measure
the indentation `w`:

- `w > top`: push `w`, emit `INDENT`;
- `w == top`: nothing;
- `w < top`: pop and emit one `DEDENT` per popped level until `top == w`; if
  no level equals `w`, the dedent is inconsistent → error `E0003`.

At end of file, emit a `DEDENT` for every level still open.

## 3.2 valo's tokens

File: [`compiler/src/token.rs`](../compiler/src/token.rs)

```rust
pub struct Token { pub kind: TokenKind, pub span: Span }

pub enum TokenKind {
    Int(i64), Float(f64), Ident(String), Kw(Keyword),
    Plus, Minus, Star, StarStar, Slash, SlashSlash, Percent,      // + - * ** / // %
    LParen, RParen, LBracket, RBracket, LBrace, RBrace,           // ( ) [ ] { }
    Comma, Dot, Colon, Eq, EqEq, NotEq, Lt, Le, Gt, Ge,           // , . : = == != < <= > >=
    Arrow, FatArrow, SquigArrow, PipeGt, Bar, Bang, Question,     // -> => ~> |> | ! ?
    Newline, Indent, Dedent, Eof,                                 // layout
}

pub enum Keyword { Let, Fn, Def, As, If, Elif, Else, Then, Match, And, Or, Not,
                   True, False, None, Import, From, For, In, Assert, Test, Mock }
```

Design notes:

- The lexer already knows **every** token of the designed language
  (`.memory/syntax.md`), even though the parser accepts only integers and
  arithmetic in M1. Later milestones then change only the parser.
- Literal values are converted by the lexer: `Int(255)` for `0xff`. The
  parser never re-reads digits.
- Keywords are recognised after reading an identifier: `Keyword::lookup(text)`
  returns `Some(Keyword::Let)` for `"let"`, `None` for ordinary names.
  Constructor names like `Ok` stay `Ident`.
- `TokenKind::describe` gives the wording for messages ("expected an
  expression, found end of line").

## 3.3 The lexer

File: [`compiler/src/lexer.rs`](../compiler/src/lexer.rs). Entry point:

```rust
pub fn lex(src: &str) -> Lexed           // Lexed { tokens: Vec<Token>, diags: Vec<Diagnostic> }
```

The token list always ends with `Eof`, even when there were errors, so the
parser can always run (the pipeline stops before the parser if there were
lexical errors, but tools like `--dump tokens` still work).

### State

```rust
struct Lexer<'a> {
    src: &'a str,                   // the source text (borrowed, not copied)
    pos: usize,                     // byte offset of the next unread character
    tokens: Vec<Token>,             // output
    diags: Vec<Diagnostic>,         // errors; scanning continues after an error
    indents: Vec<u32>,              // the indentation stack, starts as [0]
    depth: u32,                     // open (, [, { — layout is off while > 0
    at_line_start: bool,            // must measure indentation before the next token
    line_has_tokens: bool,          // did this physical line produce a token?
    pending_newline: Option<Span>,  // a NEWLINE not yet emitted (see 3.4)
}
```

Helper methods: `peek()` (next character or `None`), `peek_at(n)`,
`starts_with("->")`, `push(kind, start)` (emit a token spanning
`start..pos`), `error(code, msg, start, end)`.

### The main loop — `Lexer::run`

```text
loop:
    if at_line_start and depth == 0:  line_start()      # indentation → INDENT/DEDENT
    skip spaces, tabs, \r
    match next char:
        none  → stop
        '#'   → skip to end of line                     # comment
        '\n'  → if depth == 0: remember a pending NEWLINE (if the line had tokens),
                               set at_line_start
        else  → token(c)
finish()                                                # final NEWLINE, DEDENTs, EOF
```

### One token — `Lexer::token`

- digit → `number()`; letter or `_` → `ident()`;
- otherwise one big `match` on the character, with guards implementing
  maximal munch:

  ```rust
  '*' if two(self, "**") => (TokenKind::StarStar, 2),
  '*' => (TokenKind::Star, 1),
  ```

- brackets update `depth`;
- `"` and `r"` start a string (`string()`, below); `` ` `` (commands) and
  `re"…"` (regexes) are reserved for later milestones: `quoted()` skips to
  the closing quote and reports `E0009 … not supported yet`; `=~` and `!~`
  likewise via `unsupported()`.
- anything else → `E0001 unexpected character`.

### Numbers — `Lexer::number`

Python's literal rules (chosen in M1, recorded in `.memory/syntax.md`):

| Input | Token | Rule |
|-------|-------|------|
| `1_000` | `Int(1000)` | `_` allowed **between** digits (`check_underscores`) |
| `0xff`, `0o755`, `0b1010` | `Int(255)`, `Int(493)`, `Int(10)` | radix prefixes |
| `1.5`, `2e3`, `1e-2` | `Float(..)` | float literals |
| `1.x` | `Int(1)`, `Dot`, `Ident(x)` | a fraction needs a digit after the dot |
| `007` | E0010 | leading zeros forbidden (avoids C's octal trap); `00` is fine |
| `12ab` | E0010 | letters glued to a number (`bad_suffix`) |
| `9223372036854775808` | E0002 | does not fit in `i64` |

The lexer has no negative literals: `-5` is the token `Minus` followed by
`Int(5)`. Consequently the smallest `i64` cannot be written as a single
literal (its positive counterpart overflows); see chapter 6 for how `-5`
still becomes a single instruction.

### Strings — a lexer with modes (M7)

A string is not one token. `"a {x + 1} b"` (syntax.md S12: every normal
string interpolates) becomes

```text
STR_START  STR_TEXT("a ")  INTERP_START  x + 1  INTERP_END  STR_TEXT(" b")  STR_END
```

so the parser can parse `x + 1` with its ordinary expression rules. The
lexer therefore switches between two *modes*: in string mode
(`string_body`) it collects text, decoding escapes (`\n \t \r \0 \" \\
\{ \} \u{1F600}`, else E0005), until the closing `"` or a `{`; at a `{` it
emits `INTERP_START`, pushes an entry on the mode stack `interps` and goes
back to normal tokens. The `}` that closes the interpolation is recognised
by the bracket depth recorded on that entry — so `{ f({1: 2}) }` works —
and switches back to string mode (the `'}'` arm of `token`). Because normal
mode is fully active inside `{…}`, a string may contain another string with
its own interpolations: `"{"in{y}"}"`. This is the plan's D25; Python 3.12
(PEP 701) lexes f-strings the same way.

While an interpolation is open, `depth` is one higher, so line layout is
suspended as inside brackets; still, a line break inside `{…}` is E0007
(interpolations end on their line), and so is the end of the file. Other
string errors: a string reaching the end of its line is E0004 (inside an
unclosed `{…}` only the E0007 is reported: `"a {x"` means the `"` was meant
to close the string), `{}` with nothing inside is E0008. A raw string
`r"…"` (`raw_body`) has no escapes and no interpolation; it ends at the
first `"`.

## 3.4 The pending NEWLINE and pipe continuation

A logical line normally ends with `NEWLINE`. But valo lets a pipeline continue
on the next line if that line *starts* with `|>`:

```text
xs
    |> f
    |> g
```

should lex as `xs |> f |> g NEWLINE` — no NEWLINE and no INDENT in between.
The lexer cannot know this when it sees the `\n` after `xs`. So it does not
emit the NEWLINE immediately: it stores it in `pending_newline`. In
`line_start()`, at the first real character of the next line:

- if the line starts with `|>`: drop the pending NEWLINE and skip
  indentation processing (continuation);
- otherwise: emit the pending NEWLINE, then process indentation.

Blank and comment-only lines are skipped without touching either, which is
why comments between lines never produce empty NEWLINEs.

**Implicit line joining**: while `depth > 0` (inside `()`, `[]`, `{}`),
newlines and indentation are ignored completely:

```text
(1 +
    2)          →   ( 1 + 2 ) NEWLINE
```

## 3.5 Lexical errors

| Code | Message | Where in `lexer.rs` |
|------|---------|---------------------|
| E0001 | unexpected character `$` | `token`, last arm |
| E0002 | integer literal is out of range | `out_of_range` |
| E0003 | unindent does not match any outer level | `line_start` |
| E0004 | unterminated string | `unterminated` |
| E0005 | invalid escape sequence | `escape`, `bad_escape` |
| E0006 | tab character in indentation | `line_start` |
| E0007 | unterminated interpolation (`{` without `}` on the line) | `unclosed_interps` |
| E0008 | empty interpolation `{}` | `string_body` |
| E0009 | … not supported yet | `quoted`, `unsupported` |
| E0010 | malformed number literal | `number`, `bad_suffix`, `check_underscores` |

The lexer never stops at the first error: it records a `Diagnostic`, skips
the bad characters and continues, so one run shows all lexical errors.

## 3.6 Seeing it work

```bash
cargo run -q --bin valo -- check demo.vl --dump tokens
```

Output format: `start..end TOKEN` per line (from `lexer::dump`). The snapshot
tests in `lexer.rs` store two such dumps:
`compiler/src/snapshots/valo_compiler__lexer__tests__*.snap`.

## 3.7 Where to change things

| Task | Steps |
|------|-------|
| new operator, e.g. `+=` | 1. add `PlusEq` to `TokenKind` and its spelling in `Display` (`token.rs`); 2. in `Lexer::token`, add `'+' if two(self, "+=") => (TokenKind::PlusEq, 2),` **above** `'+' => …`; 3. add it to the `operators_maximal_munch` snapshot test |
| new keyword | `Keyword` + `lookup` + `as_str` in `token.rs` (nothing in the lexer) |
| new escape sequence | `Lexer::escape` (and `value::quote` in the VM, which writes strings back) |
| triple-quoted strings (S12, later) | `Lexer::string` (detect `"""`), `string_body` (allow newlines) |
| allow tabs | `Lexer::line_start` |

## 3.8 Exercises

1. Run `--dump tokens` on a file containing `a:\n  b:\n    c` and explain the
   two `DEDENT` tokens at the end.
2. Why does `quoted()` set `line_has_tokens = true` even though it emits no
   token? (Hint: what would happen to NEWLINE and indentation otherwise?)
3. The property test `never_panics_and_ends_with_eof` feeds random Unicode
   text to `lex`. Which bug class does it protect against? (Slicing a string
   in the middle of a multi-byte character.)

## 3.9 Further reading

- *Crafting Interpreters*, ch. 4 "Scanning" and ch. 16 "Scanning on Demand".
- Python Language Reference, §2.1.8 "Indentation" — the original offside
  algorithm valo follows.
- Dragon Book, ch. 3 — the automata theory behind lexers.
