# valo formal grammar

This document specifies the concrete syntax of valo as implemented by the
current compiler (milestones M0–M8). It is normative: where it and the
[syntax guide](syntax.md) differ, this document wins; where this document and
the compiler differ, it is a bug in one of them.

The specification has four layers:

1. [Lexical grammar](#2-lexical-grammar) — characters → tokens.
2. [Layout](#3-layout) — indentation → `NEWLINE`, `INDENT`, `DEDENT` tokens.
3. [Syntactic grammar](#4-syntactic-grammar) — tokens → syntax tree, with
   [operator precedence](#5-expression-structure-precedence-and-associativity)
   fixing the shape of expressions.
4. [Static constraints](#7-static-constraints) — rules the grammar does not
   express, checked after parsing — and the
   [desugaring](#6-desugaring) that defines derived forms.

Implementation: `compiler/src/lexer.rs` (layers 1–2), `compiler/src/parser.rs`
(layer 3), `compiler/src/desugar.rs` and `compiler/src/resolve.rs` (layer 4).
`valo check --dump tokens <file>` prints the token stream (with layout
tokens), `--dump ast` the syntax tree before desugaring, and
`--dump resolved` what each name refers to.

---

## 1. Notation

The grammar uses EBNF:

| Notation        | Meaning                                         |
|-----------------|-------------------------------------------------|
| `rule = … ;`    | definition of a nonterminal                     |
| `"text"`        | terminal: the exact characters / token spelling |
| `UPPER`         | a token class (`IDENT`, `INT`, `NEWLINE`, …)    |
| `a b`           | sequence                                        |
| `a \| b`        | alternative                                     |
| `[ a ]`         | optional (zero or one)                          |
| `{ a }`         | repetition (zero or more)                       |
| `( a )`         | grouping                                        |
| `a - b`         | `a`, but not matching `b`                       |
| `(* … *)`       | comment / side condition in prose               |

In the lexical grammar, terminals are characters. In the syntactic grammar,
terminals are tokens; whitespace and comments have already been removed.

---

## 2. Lexical grammar

### 2.1 Source text

A source file is UTF-8 text. Line breaks are `"\n"`; a `"\r"` is treated as
whitespace, so CRLF files work.

```ebnf
line_break   = "\n" ;
whitespace   = " " | "\t" | "\r" ;          (* between tokens; see §3 for indentation *)
comment      = "#" { char - line_break } ;
```

Comments and whitespace separate tokens and are otherwise discarded. A
shebang line `#!…` is a comment.

The lexer is **maximal munch**: at each position it produces the longest
token that matches (`**` is one token, not two `*`; `->` is one token, not
`-` `>`).

### 2.2 Identifiers and keywords

```ebnf
IDENT        = ident_start { ident_char } - keyword ;
ident_start  = letter | "_" ;
ident_char   = letter | number | "_" ;
letter       = (* any character with the Unicode Alphabetic property *) ;
number       = (* any character with the Unicode Numeric property *) ;
```

Exception: the identifiers `r` and `re` immediately followed by `"` are not
identifiers but the start of a raw string (§2.4) and of a regex literal
(reserved, §8).

```ebnf
keyword      = "and" | "as" | "assert" | "def" | "elif" | "else" | "false"
             | "fn" | "for" | "from" | "if" | "import" | "in" | "let"
             | "match" | "mock" | "none" | "not" | "or" | "test" | "then"
             | "true" ;
```

`_` is lexed as an ordinary `IDENT`; its special roles are syntactic (§4.6)
and static (§7).

### 2.3 Numbers

```ebnf
INT          = dec_int | hex_int | oct_int | bin_int ;
dec_int      = "0" { [ "_" ] "0" }
             | nonzero_digit { [ "_" ] digit } ;
hex_int      = "0" ( "x" | "X" ) hex_digit { [ "_" ] hex_digit } ;
oct_int      = "0" ( "o" | "O" ) oct_digit { [ "_" ] oct_digit } ;
bin_int      = "0" ( "b" | "B" ) bin_digit { [ "_" ] bin_digit } ;

FLOAT        = digits "." digits [ exponent ]
             | digits exponent ;
digits       = digit { [ "_" ] digit } ;
exponent     = ( "e" | "E" ) [ "+" | "-" ] digits ;

digit        = "0" … "9" ;
nonzero_digit= "1" … "9" ;
hex_digit    = digit | "a" … "f" | "A" … "F" ;
oct_digit    = "0" … "7" ;
bin_digit    = "0" | "1" ;
```

Side conditions:

- A number token may not be immediately followed by a letter, digit or `_`
  (`12abc`, `0x1g` are errors, E0010).
- `1.` not followed by a digit is the `INT` `1` followed by the token `.`.
- The value of an `INT` must lie in `0 … 2^63 − 1` (E0002). Negative numbers
  are formed with the prefix operator `-`.
- A decimal `INT` with a leading zero other than all-zeros (`007`) is an
  error (E0010). The rule does not apply to floats (`01.5` is `1.5`).

### 2.4 Strings

```ebnf
string        = '"' { str_char | escape | interpolation } '"'
              | 'r"' { raw_char } '"' ;
str_char      = char - ( '"' | "\" | "{" | line_break ) ;
raw_char      = char - ( '"' | line_break ) ;
escape        = "\" ( "n" | "t" | "r" | "0" | '"' | "\" | "{" | "}"
                    | "u{" hex_digit [hex_digit] [hex_digit]
                           [hex_digit] [hex_digit] [hex_digit] "}" ) ;
interpolation = "{" expr_tokens "}" ;
```

- `\u{…}` must denote a Unicode scalar value (not a surrogate, ≤ `10FFFF`).
  Any other escape is an error (E0005).
- `expr_tokens` is lexed as ordinary tokens (§2.5), so an interpolation may
  contain any expression, including nested strings. The `}` that ends the
  interpolation is the first `}` at the interpolation's own bracket depth.
  `{` followed only by blanks and `}` is an error (E0008).
- A string, including every interpolation inside it, must end on the line
  where it starts (E0004, E0007) — even inside brackets.
- A raw string has no escapes and no interpolation; it cannot contain `"`.

The lexer emits a string as a token sequence, so the parser sees the
interpolated expressions:

```ebnf
string_tokens = STR_START { STR_TEXT | INTERP_START expr INTERP_END } STR_END ;
```

`STR_TEXT` carries the decoded text (escapes already applied). For example,
`"a {x + 1}!"` becomes
`STR_START STR_TEXT("a ") INTERP_START IDENT(x) "+" INT(1) INTERP_END STR_TEXT("!") STR_END`.

### 2.5 Operators and punctuation

```text
+    -    *    **   /    //   %
==   !=   <    <=   >    >=
(    )    [    ]    {    }    ,    .    :    =
->   =>   ~>   |>   |    !    ?
```

Any other character outside a string or comment is an error (E0001).
`` ` `` (command literal), `re"…"`, `=~` and `!~` are recognised but
rejected as not yet supported (§8).

---

## 3. Layout

Layout turns the line structure of the source into three tokens:

| Token     | Meaning                                       |
|-----------|-----------------------------------------------|
| `NEWLINE` | end of a logical line                         |
| `INDENT`  | the next line is indented more: a block opens |
| `DEDENT`  | one block closes (one token per closed level) |

The lexer keeps a stack of indentation widths, initially `[0]`, a bracket
depth `d` (the number of open `(`, `[`, `{` and string interpolations), and
a *pending* `NEWLINE`.

1. **Line breaks.** A line break with `d > 0` is whitespace (implicit line
   joining). A line break inside a string interpolation is an error (E0007).
   A line break with `d = 0` that ends a line which produced tokens makes a
   `NEWLINE` pending; it is emitted only when the next significant line is
   known (rule 2).
2. **Start of a physical line** (only when `d = 0`). Count the leading
   spaces: that is the line's *width*. A tab in indentation is an error
   (E0006) and counts as one column. Then:
   - **blank, comment-only, or end of input:** ignore the line entirely;
   - **first token is `|>`:** a *continuation line* — discard the pending
     `NEWLINE` and do not compare widths; the line continues the previous
     logical line;
   - **otherwise:** emit the pending `NEWLINE`, then compare the width `w`
     with the top `t` of the stack:
     - `w > t`: push `w`, emit `INDENT`;
     - `w < t`: pop and emit one `DEDENT` while `w <` top; if `w` is then not
       equal to the top, it is an error (E0003);
     - `w = t`: nothing.
3. **End of input.** Emit the pending `NEWLINE` (or a `NEWLINE` if the last
   line had tokens but no final line break), one `DEDENT` per stack entry
   above `0`, and `EOF`.

Consequences used by the syntactic grammar:

- Every non-empty logical line ends with exactly one `NEWLINE`.
- A `NEWLINE` always comes **before** the `INDENT`/`DEDENT`s of the next line.
  So an indented block is `NEWLINE INDENT … NEWLINE DEDENT`, and a line that
  ends with a block has consumed its own `NEWLINE` inside the block.
- `INDENT`, `DEDENT` and `NEWLINE` never occur inside brackets.

Example:

```text
fn f(x: int) -> int:        fn IDENT ( IDENT : IDENT ) -> IDENT :
    let y = x * 2           NEWLINE INDENT let IDENT = IDENT * INT
    y                       NEWLINE IDENT
print(f(1))                 NEWLINE DEDENT IDENT ( IDENT ( INT ) )
                            NEWLINE EOF
```

---

## 4. Syntactic grammar

### 4.1 Programs and items

```ebnf
program      = { item | NEWLINE } EOF ;

item         = ( binding | fn_def | expr ) item_end ;
item_end     = NEWLINE | EOF
             | (* nothing, if the item's last token is a DEDENT *) ;

block        = NEWLINE INDENT item { item } DEDENT ;
body         = block | expr ;
```

`body` is what follows `:` in `fn` and `if`, and `=>` in a lambda: either an
indented block or an expression on the same line. When a `NEWLINE` follows
the `:` or `=>` it must be followed by `INDENT`.

The last item of a `block` must be an `expr` (E0104, §7). A `block`'s value
is the value of its last item. Items at the top level (`program`) are not
restricted.

### 4.2 Bindings

```ebnf
binding      = "let" IDENT [ ":" type ] "=" expr ;
```

### 4.3 Function definitions

```ebnf
fn_def       = "fn" IDENT [ type_params ] "(" [ param_list ] ")"
               "->" type fn_body ;

type_params  = "[" IDENT { "," IDENT } [ "," ] "]" ;
param_list   = param { "," param } [ "," ] ;
param        = IDENT ":" type [ "=" expr ] ;

fn_body      = ":" body
             | "=" expr
             | "=" NEWLINE INDENT expr NEWLINE DEDENT ;
```

The third form of `fn_body` lets a single-expression body start on the next,
indented line. A `"="` body is always exactly one expression.

The parser accepts a parameter without `: type` and a header without
`-> type` to report them precisely, but both are errors (E0410).

### 4.4 Expressions

```ebnf
expr         = { operand binary_op } last_operand ;

operand      = { prefix_op } postfix_expr ;
last_operand = { prefix_op } ( postfix_expr | open_form ) ;

prefix_op    = "-" | "not" ;
binary_op    = "|>" | "or" | "and"
             | "==" | "!=" | "<" | "<=" | ">" | ">="
             | "in" | "not" "in"
             | "+" | "-" | "*" | "/" | "//" | "%" | "**" ;

postfix_expr = primary { call_suffix | index_suffix | "?" } ;
call_suffix  = "(" [ arg_list ] ")" ;
index_suffix = "[" expr "]" ;

primary      = INT | FLOAT | "true" | "false" | "none"
             | string_tokens
             | IDENT
             | "(" ")"                         (* unit *)
             | "(" expr ")"                    (* grouping *)
             | list ;

list         = "[" [ expr { "," expr } [ "," ] ] "]"          (* list literal *)
             | "[" expr comp_for { comp_for | comp_if } "]" ;  (* comprehension *)
comp_for     = "for" IDENT "in" expr ;
comp_if      = "if" expr ;

open_form    = lambda | if_expr | assert_expr ;
```

Line breaks inside `[ … ]` are ignored, like inside `( … )`. The loop
variable of a `comp_for` is a single name (or `_`).

This grammar describes *which token sequences are expressions*. The tree
they denote — which operator applies to which operands — is fixed by the
precedence rules of §5. An `open_form` ends with an `expr` that extends as
far to the right as possible, so it can only be the last operand.

### 4.5 Conditionals

```ebnf
if_expr      = "if" expr "then" expr "else" expr        (* inline form *)
             | "if" expr ":" body if_rest ;             (* block form  *)

if_rest      = [ NEWLINE ] "elif" expr ":" body if_rest
             | [ NEWLINE ] "else" ":" body ;
```

- The optional `NEWLINE` in `if_rest` is present exactly when the preceding
  `body` was an inline `expr`: `elif`/`else` then start the next line.
  After a `block` body the `NEWLINE` was consumed inside the block.
  Either way `elif`/`else` stand at the indentation of the line holding the
  `if`.
- The `else` branch is required in both forms (E0102).
- The condition and the `then` branch are each a full `expr`; they end at
  `then`, `:`, or `else`, which are not operators.

### 4.6 Calls and arguments

```ebnf
arg_list     = arg { "," arg } [ "," ] ;
arg          = IDENT "=" expr                (* keyword argument   *)
             | "_"                           (* placeholder        *)
             | expr ;                        (* positional argument *)
```

- `IDENT "="` is recognised by two-token lookahead: an identifier directly
  followed by `=` (not `==`) starts a keyword argument.
- `"_"` is a placeholder only when it is the whole argument, i.e. followed
  by `,` or `)`. Elsewhere `_` is an `IDENT` (and reading it is E0206).
- A positional `expr` argument may not follow a keyword argument (E0101).
  Placeholders may appear anywhere.

### 4.7 Lambdas

```ebnf
lambda       = IDENT "=>" body
             | "(" [ lparam { "," lparam } [ "," ] ] ")" "=>" body ;
lparam       = IDENT [ ":" type ] ;
```

Disambiguation, without backtracking:

- an `IDENT` directly followed by `=>` starts a lambda;
- a `(` starts a lambda if the matching `)` is directly followed by `=>`
  (found by scanning tokens while counting `( [ {` and `) ] }`); otherwise
  it starts a unit `()` or a grouping.

Lambda parameters cannot have defaults (E0101). Because brackets suppress
layout, a lambda with a `block` body cannot appear inside brackets.

### 4.8 Assertions

```ebnf
assert_expr  = "assert" expr ;
```

### 4.9 Types

```ebnf
type         = result_type [ "->" type ] ;              (* right-associative *)
result_type  = union_type [ "!" union_type ] ;
union_type   = type_atom { "|" type_atom } ;

type_atom    = IDENT [ "[" type_list "]" ]              (* int, list[int] *)
             | "none"
             | "(" ")"                                  (* unit *)
             | "(" type ")"                             (* grouping *)
             | "(" type "," [ type_list ] ")" ;         (* tuple *)

type_list    = type { "," type } [ "," ] ;
```

In a function type `A -> R`, the left side lists the parameters: `()` means
no parameters, a tuple `(A, B)` means two parameters `A` and `B`, and any
other type is a single parameter.

Type precedence, loosest first: `->` < `!` < `|`. So
`int | none ! error -> str` is `((int | none) ! error) -> str`.

Valid type names (§7): `int`, `float`, `bool`, `str`, `none`, `any`, `error`,
the generic names `list[T]`, `dict[K, V]`, `tuple[T, …]`, and the type
variables declared by the enclosing `fn`.

---

## 5. Expression structure: precedence and associativity

The tree shape of an `expr` is defined by **binding powers**. Each binary
operator has a left binding power (lbp) and a right binding power (rbp);
each prefix operator has a single binding power.

| Operator                     | lbp | rbp | Kind / associativity                    |
|------------------------------|-----|-----|-----------------------------------------|
| `|>`                         | 1   | 2   | left                                    |
| `or`                         | 3   | 4   | left                                    |
| `and`                        | 5   | 6   | left                                    |
| prefix `not`                 | —   | 7   | prefix                                  |
| `== != < <= > >=`            | 8   | 9   | chained (one node, §5.2)                |
| `in`, `not in`               | 8   | 9   | does not chain (see below)              |
| `+ -`                        | 10  | 11  | left                                    |
| `* / // %`                   | 20  | 21  | left                                    |
| prefix `-`                   | —   | 30  | prefix                                  |
| `**`                         | 41  | 40  | right                                   |
| call `(`, index `[`, postfix `?` | 50 | —  | postfix                                 |
| lambda, `if`, `assert`       | —   | 0   | open forms: trailing `expr` extends maximally |

### 5.1 Parsing algorithm (normative)

`parse_expr(min)` parses an expression whose infix operators all have
`lbp ≥ min`. A whole `expr` is `parse_expr(0)`.

```text
parse_expr(min):
    lhs := parse_prefix()
    loop:
        op := next token
        if op is not an infix or postfix operator, or lbp(op) < min: return lhs
        if op is "(":            lhs := Call(lhs, parse_args())
        elif op is "[":          lhs := Index(lhs, parse_expr(0)); expect "]"
        elif op is "?":          lhs := Try(lhs)
        elif op is "in" / "not" "in":
                                 lhs := In(lhs, parse_expr(9))
                                 if the next token is a comparison or "in": E0101
        elif op is a comparison: lhs := parse_chain(lhs)            -- §5.2
        else:                    consume op; lhs := Binary(op, lhs, parse_expr(rbp(op)))

parse_prefix():
    "-"       → Neg(parse_expr(30))
    "not"     → Not(parse_expr(7))
    "assert"  → Assert(parse_expr(0))
    "if"      → if_expr (§4.5); its trailing expressions use parse_expr(0)
    lambda    → Lambda(params, body)   (§4.7); an inline body is parse_expr(0)
    "(" expr ")" → the inner expression (parse_expr(0))
    "["       → a list literal or comprehension (§4.4); elements parse_expr(0)
    otherwise → a primary (§4.4)
```

Consequences:

- `a - b - c` is `(a - b) - c`; `2 ** 3 ** 2` is `2 ** (3 ** 2)`.
- `-2 ** 2` is `-(2 ** 2)`; `2 ** -1` is `2 ** (-1)`; `-x * y` is `(-x) * y`.
- `not a == b` is `not (a == b)`; `not a and b` is `(not a) and b`.
- `1 + 2 |> f` is `(1 + 2) |> f`.
- `x => x + 1 |> f` is `x => ((x + 1) |> f)`.
- `if c then a else b + 1` is `if c then a else (b + 1)`.
- `f(x)?` is `Try(Call(f, x))`; `1 + f(x)?` is `1 + Try(Call(f, x))`;
  `f()?(1)?` is `Try(Call(Try(Call(f)), 1))`.
- `x |> f(a)?` parses as `x |> Try(Call(f, a))`; desugaring moves the `?`
  outside the pipe (§6.2).
- `x + 1 in xs` is `(x + 1) in xs`; `not x in xs` is `not (x in xs)`;
  `f(x)[0]?` is `Try(Index(Call(f, x), 0))`.
- `a < b in c`, `a in b == c` and `a in b in c` are errors (E0101): unlike
  Python, `in` does not chain; add parentheses.

### 5.2 Comparison chains

A run of comparisons `e0 op1 e1 op2 e2 … opn en` (each `ei` parsed with
`parse_expr(9)`) forms **one** node, not a nested tree. It means

```text
e0 op1 e1  and  e1 op2 e2  and  …  and  e(n-1) opn en
```

with every `ei` evaluated at most once, left to right, stopping at the first
false link.

### 5.3 Nesting limit

Expressions and types may nest at most 200 levels deep (each parenthesised
level costs about two); deeper input is E0105.

---

## 6. Desugaring

Some forms are defined by translation into other forms before name
resolution. `$`-names below are compiler-generated and cannot clash with
source names.

### 6.1 `elif`

```text
if c1: b1 elif c2: b2 else: b3   ≡   if c1: b1 else: (if c2: b2 else: b3)
```

### 6.2 Pipe

Let `v` be the left operand of `|>`.

| Source                          | Meaning                                 |
|---------------------------------|-----------------------------------------|
| `v \|> f(a, b, k=c)`            | `f(v, a, b, k=c)` — `v` becomes the first positional argument |
| `v \|> f(a, _, b)`              | `f(a, v, b)` — `v` fills the first `_`  |
| `v \|> f(_, _)`                 | `f(v, _)` — still a partial (§6.3)      |
| `v \|> e` (`e` not a call)      | `e(v)`                                  |
| `v \|> f(a)?`                   | `f(v, a)?` — the `?` applies to the whole stage |

`v` is evaluated before the callee and the other arguments. Only a call
written directly on the right of `|>` receives `v` as an argument:
`v |> (w |> f)` calls the value of `(w |> f)` with `v`.

### 6.3 Partial application

A call whose arguments contain placeholders becomes a lambda with one
parameter per placeholder, in left-to-right order. Every other argument (and
the callee, unless it is a plain name) is evaluated once, in source order,
when the partial is built:

```text
f(a, _, k=b)   ≡   { let $a0 = a
                     let $a1 = b
                     ($p0) => f($a0, $p0, k=$a1) }
```

### 6.4 `?`

```text
e?   ≡   match e:  Ok(v) ~> v
                   Err(x) ~> return Err(x) from the innermost fn or lambda
```

(`match` and `return` are not source syntax; this is the meaning.) A value
that is neither `Ok` nor `Err` is a runtime error. At the top level, the
`Err` stops the program (exit code 1). A top-level expression item whose
value is an `Err` behaves the same way.

### 6.5 Membership

```text
x in xs        ≡   contains(xs, x)          (x evaluated first)
x not in xs    ≡   not contains(xs, x)
```

`contains` is the built-in, even where the program rebinds that name.

### 6.6 List comprehensions

The clauses are translated from the last `for` outwards. With `map`,
`filter` and `flat_map` the built-ins (whatever those names are bound to):

```text
[e for x in xs]                   ≡   map(xs, x => e)
[e for x in xs if c1 if c2]       ≡   map(filter(filter(xs, x => c1), x => c2), x => e)
[e for x in xs <clauses>]         ≡   flat_map(xs, x => [e <clauses>])    (when <clauses> has a `for`)
```

So a later clause may use earlier loop variables, and the list of a later
`for` is evaluated once per element of the earlier one. The generated
lambdas are named `<comprehension@line:col>` in stack traces. A `?` inside
`e`, an `if`, or the list of a later `for` would only return from such a
lambda, so it is an error (E0208); the first `for`'s list is evaluated
outside and may use `?`.

### 6.7 String interpolation

`"t0{e1}t1…{en}tn"` evaluates `e1 … en` left to right, converts each value to
text as `str(ei)` does, and concatenates the pieces.

---

## 7. Static constraints

Rules checked after parsing. The parser recovers line by line, so one run
reports several errors.

### 7.1 Structure

| Code  | Rule |
|-------|------|
| E0101 | Unexpected token (any violation of §4), including an indented line where no block is open, a positional argument after a keyword argument, and a lambda parameter with a default. |
| E0102 | `if` without `else`. |
| E0103 | `(` or `[` never closed (line breaks inside brackets are joined, so this is reported at the end of input). |
| E0104 | A block ends with a `let` or `fn` item. |
| E0105 | Expression or type nested too deeply (§5.3). |
| E0410 | A `fn` parameter without a type, or a `fn` without a return type. |
| E0423 | A required parameter after a parameter with a default. |

### 7.2 Names and scopes

- **Global scope.** The built-in functions (the prelude) come first, then
  every top-level `fn` and `let`, in source order. Built-ins can be shadowed.
- **Top-level `fn`s are hoisted**: visible in the whole file. Two top-level
  `fn`s with the same name are E0207. A top-level `let` of the same name
  shadows the `fn` from its own line on.
- **`let`** introduces a new binding, visible from the next item to the end
  of the enclosing block (or file). Its initialiser is resolved *before* the
  name is bound, so `let x = x + 1` reads the previous `x`. Rebinding a name
  in the same scope is allowed (shadowing).
- **Parameters** are bound in the function body. A default expression sees
  only the parameters before it.
- **Local `fn`s** (a `fn` item inside a block) are visible from their own
  line to the end of the block and inside their own body. A local `fn` may
  not refer to a later local `fn` of the same block (E0204).
- **Inside a `fn` body**, a top-level name resolves to the binding in effect
  where the `fn` is written; failing that, to the first later top-level
  `let` of that name (reading it before it is set is runtime error R0302).
- **Lookup order:** locals (innermost block first, newest first), then the
  locals of enclosing functions (captured by value), then top-level `let`s,
  then top-level `fn`s, then the prelude.

| Code  | Rule |
|-------|------|
| E0201 | Undefined name. |
| E0203 | A top-level name used before its `let` at the top level. |
| E0204 | A local `fn` uses a later local `fn` of its block. |
| E0206 | `_` used as a value (`_` may only be a `let` name, a lambda parameter, or a placeholder argument). |
| E0207 | A name defined twice by top-level `fn`s. |
| E0301 | A function captures more than 255 variables. |

### 7.3 Calls

A callee has a **known signature** when it is a top-level or local `fn`, a
built-in, or a name bound by `let` to one of those, to a lambda, or to a
partial. Calls to known signatures are checked statically; others at runtime.

| Code  | Rule |
|-------|------|
| E0205 | Wrong number of positional arguments. |
| E0421 | A parameter with a default passed by position (defaults are keyword-only). |
| E0422 | Keyword arguments to a callee without a known signature. |
| E0424 | Unknown keyword, or a required parameter passed by name. |
| E0425 | The same keyword given twice. |
| E0208 | `?` directly inside a comprehension (§6.6). |

### 7.4 Types

| Code  | Rule |
|-------|------|
| E0402 | Unknown type name (not built-in and not a declared type variable). |
| E0403 | Wrong number of type arguments (`list[int, int]`). |

Values are not yet checked against annotations.

### 7.5 Lexical errors

| Code  | Rule |
|-------|------|
| E0001 | Unexpected character. |
| E0002 | Integer literal out of range. |
| E0003 | Dedent to a width that matches no enclosing block. |
| E0004 | Unterminated string. |
| E0005 | Invalid escape sequence. |
| E0006 | Tab in indentation. |
| E0007 | Unterminated interpolation. |
| E0008 | Empty interpolation `{}`. |
| E0009 | Reserved syntax that is not supported yet (§8). |
| E0010 | Malformed number literal. |

---

## 8. Reserved syntax

These forms are lexed (and reserved) but rejected with E0009 by the current
implementation. Their planned meaning is recorded in `.memory/syntax.md`.

| Form | Where it is rejected |
|------|----------------------|
| keywords `def`, `match`, `import`, `from`, `test`, `mock` at the start of an expression | parser |
| `{` at the start of an expression (dict literals) | parser |
| `.` (field access) after an expression | parser |
| `for (a, b) in …` (destructuring in a comprehension) | parser |
| `` `…` `` command literals, `re"…"` regex literals | lexer |
| `=~`, `!~` | lexer |

The tokens `~>` (match arms), `{`, `}`, `.` and `|` (in expressions) are
reserved for these features; `|` is used today only in types.

---

## Appendix A. Complete syntactic grammar

```ebnf
program      = { item | NEWLINE } EOF ;
item         = ( binding | fn_def | expr ) item_end ;
item_end     = NEWLINE | EOF | (* nothing after a DEDENT *) ;
block        = NEWLINE INDENT item { item } DEDENT ;
body         = block | expr ;

binding      = "let" IDENT [ ":" type ] "=" expr ;

fn_def       = "fn" IDENT [ type_params ] "(" [ param_list ] ")"
               "->" type fn_body ;
type_params  = "[" IDENT { "," IDENT } [ "," ] "]" ;
param_list   = param { "," param } [ "," ] ;
param        = IDENT ":" type [ "=" expr ] ;
fn_body      = ":" body
             | "=" expr
             | "=" NEWLINE INDENT expr NEWLINE DEDENT ;

expr         = { operand binary_op } last_operand ;
operand      = { prefix_op } postfix_expr ;
last_operand = { prefix_op } ( postfix_expr | open_form ) ;
prefix_op    = "-" | "not" ;
binary_op    = "|>" | "or" | "and"
             | "==" | "!=" | "<" | "<=" | ">" | ">="
             | "in" | "not" "in"
             | "+" | "-" | "*" | "/" | "//" | "%" | "**" ;
postfix_expr = primary { call_suffix | index_suffix | "?" } ;
call_suffix  = "(" [ arg_list ] ")" ;
index_suffix = "[" expr "]" ;
primary      = INT | FLOAT | "true" | "false" | "none"
             | string_tokens | IDENT
             | "(" ")" | "(" expr ")" | list ;
list         = "[" [ expr { "," expr } [ "," ] ] "]"
             | "[" expr comp_for { comp_for | comp_if } "]" ;
comp_for     = "for" IDENT "in" expr ;
comp_if      = "if" expr ;
string_tokens= STR_START { STR_TEXT | INTERP_START expr INTERP_END } STR_END ;
open_form    = lambda | if_expr | assert_expr ;

if_expr      = "if" expr "then" expr "else" expr
             | "if" expr ":" body if_rest ;
if_rest      = [ NEWLINE ] "elif" expr ":" body if_rest
             | [ NEWLINE ] "else" ":" body ;

arg_list     = arg { "," arg } [ "," ] ;
arg          = IDENT "=" expr | "_" | expr ;

lambda       = IDENT "=>" body
             | "(" [ lparam { "," lparam } [ "," ] ] ")" "=>" body ;
lparam       = IDENT [ ":" type ] ;

assert_expr  = "assert" expr ;

type         = result_type [ "->" type ] ;
result_type  = union_type [ "!" union_type ] ;
union_type   = type_atom { "|" type_atom } ;
type_atom    = IDENT [ "[" type_list "]" ] | "none"
             | "(" ")" | "(" type ")" | "(" type "," [ type_list ] ")" ;
type_list    = type { "," type } [ "," ] ;
```
