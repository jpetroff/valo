# Chapter 4 — Syntax Analysis

## 4.1 Theory: from tokens to a tree

The **parser** checks that tokens form valid sentences of the language and
builds an **abstract syntax tree** (AST): a tree whose shape encodes the
structure the programmer meant. "Abstract" because it drops everything that
only served the text: parentheses, commas, comments, layout tokens.

```text
tokens:  (  1  +  2  )  *  3          AST:        *
                                                 / \
                                                +   3
                                               / \
                                              1   2
```

Two families of parsing techniques:

- **bottom-up** (LR, LALR — yacc, bison, lalrpop): table-driven, generated
  from a grammar; powerful, but error messages are hard to make good;
- **top-down** (LL, recursive descent): one function per grammar rule, calling
  each other recursively; easy to write by hand, easy to give good errors.
  Used by rustc, Go, Clang, V8, CPython (since 3.9, PEG-based).

valo uses hand-written recursive descent for statements and **Pratt parsing**
for expressions.

### Precedence and associativity

Grammar rules alone make `1 + 2 * 3` ambiguous. Two properties disambiguate:

- **precedence**: `*` binds tighter than `+`, so `1 + (2 * 3)`;
- **associativity**: `-` is left-associative, `10 - 4 - 3 = (10 - 4) - 3`;
  `**` is right-associative, `2 ** 3 ** 2 = 2 ** (3 ** 2) = 512`.

A textbook LL grammar encodes this with one rule per precedence level
(`expr → term (('+'|'-') term)*`, `term → factor …`) — one function per
level, tedious with many levels.

### Pratt parsing

Vaughan Pratt's method (1973) replaces the ladder of rules with one loop and
a table of **binding powers**. Each infix operator has a left binding power
(how strongly it pulls the expression to its left) and a right binding power
(what it requires of the expression to its right):

```text
parse_expr(min_bp):
    lhs = parse_prefix()                       # literal, -x, (...)
    loop:
        op = next token; if not an infix operator: stop
        (l_bp, r_bp) = binding_power(op)
        if l_bp < min_bp: stop                 # op binds too weakly to take lhs
        consume op
        rhs = parse_expr(r_bp)
        lhs = Binary(op, lhs, rhs)
    return lhs
```

Choosing `r_bp = l_bp + 1` makes an operator left-associative; `r_bp = l_bp - 1`
right-associative. Adding an operator means adding one table row.

## 4.2 valo's syntax tree

File: [`compiler/src/ast.rs`](../compiler/src/ast.rs)

```rust
pub struct Program { pub items: Vec<Item> }        // the whole file
pub enum Item { Expr(Expr) }                        // M1: one expression per line

pub struct Expr {
    pub id: NodeId,          // unique number, for side tables in later passes
    pub span: Span,          // source range (chapter 2), includes parentheses
    pub kind: ExprKind,
}
pub enum ExprKind {
    Int(i64),
    Unary  { op: UnaryOp, operand: Box<Expr> },                       // -x
    Binary { op: BinOp, op_span: Span, lhs: Box<Expr>, rhs: Box<Expr> },
}
pub enum UnaryOp { Neg }
pub enum BinOp { Add, Sub, Mul, Div, IDiv, Mod, Pow }
```

- Children are `Box<Expr>`: a heap pointer, needed because `Expr` contains
  `Expr` (chapter 1, *Ownership and borrowing*).
- `NodeId` will let the name resolver (M2) and type checks (M10) attach
  information to nodes in separate maps instead of mutating the tree.
- `op_span` keeps the operator's own position, for errors that should point
  at the operator (e.g. "`/` is not supported yet").
- Parentheses produce no node: `(1 + 2)` is just the `Binary` node with a
  span widened to include the parentheses.

`impl Display for Expr` prints S-expressions (operator first, fully
parenthesised), which makes the tree shape visible in one line:
`valo check f.vl --dump ast`.

## 4.3 The parser

File: [`compiler/src/parser.rs`](../compiler/src/parser.rs). Entry point:

```rust
pub fn parse(tokens: &[Token]) -> (Program, Vec<Diagnostic>)
```

State: the token slice, a cursor `pos`, the collected `diags`, and the next
`NodeId`. Cursor helpers: `peek()` (current token; returns EOF forever at the
end, so the parser can never run off the slice) and `bump()` (consume and
return it).

### Statements: recursive descent

```text
program := ( NEWLINE | DEDENT | item )* EOF      Parser::program
item    := expr ( NEWLINE | EOF )                Parser::item
```

An `INDENT` at top level is an error in M1 (no construct opens a block yet):
"E0101 unexpected indentation".

### Expressions: the Pratt loop

`Parser::expr(min_bp)` is exactly the algorithm of 4.1. valo's binding powers
(Python's precedence order, `.memory/syntax.md` S11):

| Operator | `l_bp` | `r_bp` | Effect |
|----------|--------|--------|--------|
| `+` `-` | 10 | 11 | left-assoc, weakest |
| `*` `/` `//` `%` | 20 | 21 | left-assoc |
| prefix `-` | — | 30 | `PREFIX_NEG_BP` |
| `**` | 41 | 40 | right-assoc, strongest |

`Parser::prefix` handles what can *start* an expression: an `Int` literal,
prefix `-` (parses its operand with `expr(30)`), or `(` (parses an
expression with `expr(0)` and expects `)`).

Worked example — `-2 ** 2`, which Python (and valo) read as `-(2 ** 2)`:

```text
expr(0)
  prefix: '-'  → operand = expr(30)
                   prefix: 2
                   '**' l_bp 41 ≥ 30 → consume, rhs = expr(40) → 2
                   → Binary(**, 2, 2)
               → Unary(neg, Binary(**, 2, 2))
  no more operators → done
```

The prefix `-` asked for `min_bp = 30`; `**` binds at 41 > 30, so it is taken
*inside* the negation. A `*` (l_bp 20 < 30) would not be: `-2 * 3` is
`(-2) * 3`.

And `2 ** -1`: the right side of `**` is parsed with `expr(40)`, which starts
with `prefix()` — and prefix accepts `-`. So `2 ** -1` parses (and fails at
runtime with R0104, chapter 7), exactly like Python.

The snapshot test `precedence_and_associativity` in `parser.rs` checks nine
such cases; its expected output is in
`compiler/src/snapshots/valo_compiler__parser__tests__precedence_and_associativity.snap`:

```text
(expr (+ 1 (* 2 3)))
(expr (* (+ 1 2) 3))
(expr (- (- 10 4) 3))
(expr (** 2 (** 3 2)))
(expr (neg (** 2 2)))
(expr (** 2 (neg 1)))
(expr (% (// (neg 7) 2) 3))
(expr (neg (neg 1)))
(expr (* (** 2 (neg 1)) 3))
```

### Constructs of later milestones

The parser recognises tokens that are valid valo but not implemented yet —
names, `let`, `if`, `==`, calls `f(`, `[`, `.`, `?`, floats — and reports
`E0009 … is not supported yet` instead of a confusing "unexpected token".
These checks are in `Parser::prefix` (things that start an expression) and in
the operator table of `Parser::expr` (things that follow one).

## 4.4 Error recovery

A parser that stops at the first error forces an edit–compile cycle per
mistake. valo's parser uses **panic-mode recovery** at line granularity:

1. A parsing function that finds an error records a `Diagnostic` and returns
   `Err(Failed)`. `Failed` carries no data — the diagnostic is already stored.
2. `?` propagates `Err(Failed)` up to `Parser::program`.
3. `program` calls `sync_line()`: skip tokens up to and including the next
   `NEWLINE` (and whole indented blocks via `skip_block()`), then continue
   with the next line.

So `1 +\n2 2\n3\n(1` reports three errors (E0101, E0101, E0103) in one run —
see the test `errors_recover_per_line`.

A subtle point in `prefix()`: when the token that is *not* an expression is a
`NEWLINE`/`INDENT`/`DEDENT`, the parser steps back one token before
reporting, so that `sync_line` still sees the line end and does not swallow
the next line.

### Syntax error codes

| Code | Meaning | Example |
|------|---------|---------|
| E0101 | unexpected token / expected X | `1 +` (missing operand), `1 2`, `(1 2)` |
| E0103 | unclosed `(` (label on the opening parenthesis) | `(4 + 5` at end of file |
| E0009 | valid syntax, not supported yet | `f(1)`, `[1, 2]` |

## 4.5 Where to change things

| Task | Where |
|------|-------|
| new binary operator | 1. `BinOp` + `as_str` (`ast.rs`); 2. a row `(BinOp::X, l_bp, r_bp)` in `Parser::expr`; 3. an opcode mapping in `codegen.rs` |
| change precedence | the numbers in `Parser::expr` / `PREFIX_NEG_BP` |
| new expression start (names, `true`, `if`) | new `ExprKind` variant + arm in `Parser::prefix` |
| postfix forms (calls, `.field`, `?`) | in the `Parser::expr` loop, before the infix table (they bind tightest) |
| new statements (`fn`, `def`) | `Parser::item`: dispatch on the first token, as for `let` (`let_binding`) |
| type annotation syntax | `type_expr` → `type_result` → `type_union` → `type_primary` (one function per precedence level) |
| blocks (M2) | parse `NEWLINE INDENT item* DEDENT` where a `:` ends a header |

## 4.6 Exercises

1. Add comparison operators (`==`, `<`) with binding power 5/6 on paper. Where
   do they go relative to `not` (Python: `not a == b` is `not (a == b)`)?
2. Why is `**`'s left binding power (41) higher than prefix `-`'s (30), but its
   right side parsed at 40, not at 30? What would `2 ** -1 * 3` mean if the
   right side were parsed with `expr(0)`?
3. Draw the call tree of `Parser::expr` for `10 - 4 - 3` and show where left
   associativity comes from.

## 4.7 Further reading

- Bob Nystrom, "Pratt Parsers: Expression Parsing Made Easy" (blog, 2011);
  *Crafting Interpreters*, ch. 17 "Compiling Expressions".
- Alex Kladov (matklad), "Simple but Powerful Pratt Parsing" (2020) — the
  `(l_bp, r_bp)` formulation used here.
- Dragon Book, ch. 4 — LL/LR theory and error recovery strategies.
