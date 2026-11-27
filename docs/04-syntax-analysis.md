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
pub struct Program { pub items: Vec<Item>, pub next_id: u32 }  // the whole file
pub enum Item {
    Expr(Expr),                                     // an expression line
    Let(Binding),                                   // let name (: type)? = init
    Fn(FnDef),                                      // fn name(params) -> type: body
}
pub struct Binding { pub id: NodeId, pub name: String, pub name_span: Span,
                     pub ann: Option<TypeExpr>, pub init: Expr }

pub struct Expr {
    pub id: NodeId,          // unique number, for side tables in later passes
    pub span: Span,          // source range (chapter 2), includes parentheses
    pub kind: ExprKind,
}
pub enum ExprKind {
    Int(i64), Float(f64), Bool(bool),
    Name(String),                                                     // x
    Unary  { op: UnaryOp, operand: Box<Expr> },                       // -x
    Not(Box<Expr>),                                                   // not x
    Binary { op: BinOp, op_span: Span, lhs: Box<Expr>, rhs: Box<Expr> },
    Logic  { op: LogicOp, op_span: Span, lhs: Box<Expr>, rhs: Box<Expr> }, // and, or
    Compare { first: Box<Expr>, rest: Vec<CmpLink> },                 // a < b <= c
    If { cond: Box<Expr>, then_br: Box<Expr>, else_br: Box<Expr> },
    Block(Vec<Item>),                                                 // indented block
    Assert(Box<Expr>),
    Call { callee: Box<Expr>, args: Vec<Arg>, piped: Option<usize> }, // f(a, k=b)
    Pipe { op_span: Span, lhs: Box<Expr>, rhs: Box<Expr> },           // x |> f
    Lambda(Box<Lambda>),                                              // x => e (M6)
    Str(String), Interp(Vec<StrPart>),                                // "text", "a {x}" (M7)
}
pub struct CmpLink { pub op: CmpOp, pub op_span: Span, pub rhs: Expr }
```

Type annotations have their own small tree, `TypeExpr` (`int`, `list[int]`,
`int | none`, `T ! error`, `(int, int) -> int`, tuples, `()`); it is printed
as S-expressions too (`(list int)`, `(! (| int none) error)`).

- Children are `Box<Expr>`: a heap pointer, needed because `Expr` contains
  `Expr` (chapter 1, *Ownership and borrowing*).
- `NodeId` lets the name resolver (`resolve.rs`, chapter 6) and later type
  checks (M10) attach information to nodes in separate maps instead of
  mutating the tree.
- `op_span` keeps the operator's own position, for errors that should point
  at the operator.
- `elif` has no node of its own: `if a: x elif b: y else: z` is
  `If(a, x, If(b, y, z))`.
- A comparison chain is *one* node, not nested binary nodes: `a < b <= c`
  means `a < b and b <= c` with `b` evaluated once, which nested nodes cannot
  express.
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
program := ( NEWLINE | DEDENT | item )* EOF                    Parser::program
item    := ( let_binding | fn_def | expr ) ( NEWLINE | EOF )   Parser::item
let_binding := 'let' NAME ( ':' type )? '=' expr               Parser::let_binding
fn_def  := 'fn' NAME ( '[' NAME, … ']' )? '(' params ')' ( '->' type )?
           ( ':' branch | '=' expr | '=' NEWLINE INDENT expr DEDENT )   Parser::fn_def
param   := NAME ( ':' type )? ( '=' expr )?                    Parser::params
```

A parameter with a default (`timeout: int = 30`) is keyword-only (syntax.md
S19) and must come after the required ones (E0423).

Every parameter and the return type need an annotation (syntax.md S13). A
missing one is reported as E0410 but parsing goes on, so one run lists all of
them. Trailing commas are allowed in parameter and argument lists.

An `INDENT` that no `:` opened is an error: "E0101 unexpected indentation".

Type annotations use one function per precedence level, the
recursive-descent version of the Pratt table: `type_expr` (`->`,
right-associative) → `type_result` (`!`) → `type_union` (`|`) →
`type_primary` (a name, `name[args]`, `()`, `(type)`, a tuple). `(A, B) -> C`
is a function type whose parameters are the tuple's elements.

### Expressions: the Pratt loop

`Parser::expr(min_bp)` is exactly the algorithm of 4.1. valo's binding powers
(Python's precedence order, `.memory/syntax.md` S11):

| Operator | `l_bp` | `r_bp` | Effect |
|----------|--------|--------|--------|
| `\|>` | 1 | 2 | left-assoc, weakest: `a + 1 \|> f` is `f(a + 1)` |
| `or` | 3 | 4 | left-assoc |
| `and` | 5 | 6 | left-assoc |
| prefix `not` | — | 7 | `PREFIX_NOT_BP`: `not a == b` is `not (a == b)` |
| `== != < <= > >=` | 8 | 9 | chained (see below) |
| `+` `-` | 10 | 11 | left-assoc |
| `*` `/` `//` `%` | 20 | 21 | left-assoc |
| prefix `-` | — | 30 | `PREFIX_NEG_BP` |
| `**` | 41 | 40 | right-assoc |
| call `f(…)` | 50 | — | postfix, strongest: `-f(x) ** 2` is `-((f(x)) ** 2)` |

**Calls** are a postfix row: when `(` follows an expression, `call_args`
reads the arguments up to `)` — positional expressions, `name=value`
keyword arguments (no positional argument may follow one) and the
placeholder `_` (partial application, accepted from M6). `f(1)(2)` chains.
Inside parentheses the lexer suppresses `NEWLINE`/`INDENT`, so arguments may
span lines.

**Pipes** build an `ExprKind::Pipe`; a line starting with `|>` continues the
previous one (the lexer drops the `NEWLINE`). The pipe is then rewritten by
`desugar.rs`, a small pass between parsing and name resolution:
`x |> f(a)` becomes the call `f(x, a)` and `x |> f` becomes `f(x)`; when
the call has a `_`, the piped value goes there instead (`x |> f(a, _)` is
`f(a, x)`). `--dump ast` shows the tree *before* this rewrite.

**`_` partials** (M6) are rewritten by the same pass: `f(a, _, k=b)` becomes
a block that binds `a` and `b` to temporaries once, ending with a lambda
`$p0 => f($a0, $p0, k=$a1)` marked as a partial of `f` (its display name
is `<partial f@line:col>`). Names starting with `$` cannot be written in
source, so they never clash with yours.

`Parser::prefix` handles what can *start* an expression: a literal (`Int`,
`Float`, `true`, `false`, `()`), a name, prefix `-` (parses its operand with
`expr(30)`), `not` (with `expr(7)`), `(` (parses an expression with `expr(0)`
and expects `)`), `if`, `assert` and lambdas.

**Lambdas** (S3, M6) need a little lookahead. `x =>` is a name followed by
`=>`. For `(`, `paren_lambda_ahead` scans the tokens to the matching `)`
and checks whether `=>` follows: `(a, b: int) => a * b` is a lambda, `(a +
b)` a group — decided before anything is parsed, so no backtracking. The
body is parsed with `expr(0)`, so it reaches as far as possible (`x => x +
1 |> f` is one lambda), and `=>` at the end of a line opens an indented
block, like `:` does. Because the lexer ignores line breaks inside
brackets, a block-bodied lambda cannot be written directly inside a call:
bind it with `let` first. Lambda parameters may have annotations but no
defaults.

**Strings** (M7) arrive from the lexer already split
(`STR_START STR_TEXT INTERP_START … INTERP_END STR_END`, chapter 3).
`Parser::string` collects the parts, parsing each interpolated expression
with `expr(0)`; a string without `{…}` becomes `ExprKind::Str(text)`, one
with interpolation `ExprKind::Interp(parts)`:

```text
"a {x + 1} b {f("in {y}")}"   →   (interp "a " (+ x 1) " b " (call f (interp "in " y)))
```

**Comparison chains.** When the loop meets a comparison operator it calls
`compare_chain`, which keeps consuming `op operand` pairs (operands parsed
with `expr(9)`, so they contain no comparison themselves) for as long as the
next token is a comparison, and builds one `Compare` node. `0 < x <= 10 != y`
gives `(chain 0 < x <= 10 != y)`.

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

### Blocks and `if`: indentation in the grammar

```text
if_expr := 'if' expr 'then' expr 'else' expr                      inline form
         | 'if' expr ':' branch ( 'elif' expr ':' branch )* 'else' ':' branch
branch  := NEWLINE INDENT item+ DEDENT  |  expr                    Parser::branch
```

A branch after `:` is either the rest of the line or an indented block. The
token stream of

```text
if n < 0: -1
else:
    let m = n * 2
    m
```

is `if n < 0 : - 1 NEWLINE else : NEWLINE INDENT let m = n * 2 NEWLINE m
NEWLINE DEDENT`. Two layout rules make this parse:

1. After an inline branch, `NEWLINE` followed by `elif`/`else` is skipped
   (`skip_newline_before_else`): the `else` line continues the `if`.
2. The lexer emits the line's `NEWLINE` *before* the `DEDENT` that closes a
   block, so a line ending with a block has nothing left to end it.
   `end_of_line` therefore also accepts a just-consumed `DEDENT`, and
   `sync_line` (error recovery) skips nothing after one.

A block's value is its last line, which must be an expression (E0104 for a
trailing `let`). Every `if` needs an `else` (E0102): it is an expression,
and its value must exist in both cases (plan D22).

### Nesting limit

`expr` counts its recursion depth; beyond `MAX_DEPTH` (200, about 100 nested
parentheses) it reports E0105 instead of letting this or a later recursive
pass overflow the stack.

### Constructs of later milestones

The parser recognises tokens that are valid valo but not implemented yet —
`[`, `.`, `?`, `in`, `match`, `none` — and reports
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
| E0101 | unexpected token / expected X | `1 +` (missing operand), `1 2`, `(1 2)`, `let = 1` |
| E0102 | `if` without `else` | `if x > 0: 1` alone |
| E0103 | unclosed `(` (label on the opening parenthesis) | `(4 + 5` at end of file |
| E0104 | block ends with a `let` or `fn` | an indented branch whose last line is `let t = 1` |
| E0105 | expression nested too deeply | 150 nested parentheses |
| E0410 | missing parameter or return annotation | `fn f(x) -> int = x` |
| E0423 | required parameter after a defaulted one | `fn f(a: int = 1, b: int) -> int = b` |
| E0009 | valid syntax, not supported yet | `[1, 2]`, `x.y` |

## 4.5 Where to change things

| Task | Where |
|------|-------|
| new binary operator | 1. `BinOp` + `as_str` (`ast.rs`); 2. a row `(BinOp::X, l_bp, r_bp)` in `Parser::expr`; 3. an opcode mapping in `codegen.rs` |
| change precedence | the numbers in `Parser::expr` / `PREFIX_NEG_BP` |
| new expression start (lambdas, strings) | new `ExprKind` variant + arm in `Parser::prefix` |
| postfix forms (`.field`, `?`) | a row in the `Parser::expr` table with a high `l_bp`, like calls (`Infix::Call`, `call_args`) |
| new statements (`def`) | `Parser::item`: dispatch on the first token, as for `let` and `fn` |
| type annotation syntax | `type_expr` → `type_result` → `type_union` → `type_primary` (one function per precedence level) |
| a construct with an indented body | after its `:`, call `branch` (or `block` directly), as `fn_def` does |
| a new rewrite of syntax into simpler syntax | `desugar.rs` |

## 4.6 Exercises

1. With the binding powers above, parse `not a == b or c` and `-x ** 2 < y`
   on paper, then check with `valo check f.vl --dump ast`.
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
