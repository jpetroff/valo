# valo syntax guide

This guide shows every syntax pattern the current valo implementation
accepts (milestones M0–M8), with small programs you can run. The precise
rules are in the [formal grammar](grammar.md).

Every block marked `valo` below is a complete program:

```bash
valo run example.vl
```

Lines starting with `# →` show what the program prints.

## Contents

1. [A first program](#1-a-first-program)
2. [Source text, lines and indentation](#2-source-text-lines-and-indentation)
3. [Literals](#3-literals)
4. [Strings](#4-strings)
5. [Bindings: `let`](#5-bindings-let)
6. [Operators](#6-operators)
7. [Conditionals: `if`](#7-conditionals-if)
8. [Blocks are expressions](#8-blocks-are-expressions)
9. [Functions: `fn`](#9-functions-fn)
10. [Calls and keyword arguments](#10-calls-and-keyword-arguments)
11. [Lambdas and closures](#11-lambdas-and-closures)
12. [Partial application with `_`](#12-partial-application-with-_)
13. [The pipe `|>`](#13-the-pipe-)
14. [none, results and `?`](#14-none-results-and-)
15. [`assert`](#15-assert)
16. [Type annotations](#16-type-annotations)
17. [Built-in functions](#17-built-in-functions)
18. [Lists and comprehensions](#18-lists-and-comprehensions)
19. [Reserved syntax (not implemented yet)](#19-reserved-syntax-not-implemented-yet)

---

## 1. A first program

```valo
#!/usr/bin/env valo
# Classify a few disk usage percentages.

fn grade(pct: int) -> str:
    if pct > 90: "crit"
    elif pct > 80: "warn"
    else: "ok"

let limit = 85
let usages = 40
print("{usages}% is {grade(usages)}")       # → 40% is ok
print("{97}% is {grade(97)}")               # → 97% is crit
print(grade(limit) |> str_len)              # → 4
```

What this shows:

- A program is a sequence of **lines**. Each line is a `let` binding, a `fn`
  definition or an expression. Lines run top to bottom.
- **Indentation** marks blocks, as in Python. There are no braces and no `;`.
- **Everything is an expression.** `if` produces a value; a function's body
  produces its result. There is no `return`.
- Strings **interpolate** `{expr}` automatically.

---

## 2. Source text, lines and indentation

### Comments and shebang

`#` starts a comment that runs to the end of the line. A first line such as
`#!/usr/bin/env valo` is therefore an ordinary comment, and makes a valo file
executable on Unix.

```valo
#!/usr/bin/env valo
# a whole-line comment
let x = 1   # a trailing comment
print(x)    # → 1
```

### Logical lines

A line ends at a line break. Blank lines and comment-only lines are ignored
everywhere, including inside indented blocks.

A line may continue on the next physical line in exactly two cases:

1. **Inside brackets.** Between `(` and its `)` (or `[` and `]`) line
   breaks and indentation are ignored (Python's implicit line joining):

   ```valo
   fn add3(a: int, b: int, c: int) -> int = a + b + c
   print(add3(
       1,
       2,
       3,
   ))                                  # → 6
   ```

2. **Before a `|>`.** A line whose first token is `|>` continues the previous
   line, whatever its indentation:

   ```valo
   fn double(x: int) -> int = x * 2
   let n = 5
       |> double
       |> double
   print(n)                            # → 20
   ```

There is no `\` line continuation.

### Indentation

- Indent with **spaces**. A tab in indentation is an error (E0006).
- Any width works, as long as it is consistent: a line that is indented more
  than the previous one opens a block; a line that is indented less must
  return to the column of an enclosing block (otherwise E0003).
- Indentation is only allowed where a block is expected: after a `:` that
  ends a line, after `=>` that ends a line, and after `=` that ends a
  `fn` header (see [§9](#9-functions-fn)). An unexpected indented line is an
  error (E0101).

```valo
fn classify(n: int) -> str:
    if n < 0:
        "negative"
    else:
        if n == 0: "zero"
        else: "positive"

print(classify(-3))                     # → negative
```

### Names

A name starts with a letter or `_` and continues with letters, digits and
`_`. Letters may be any Unicode letters. Names are case-sensitive.

```valo
let user_count = 3
let Δt = 2
let _tmp = 1
print(user_count * Δt + _tmp)           # → 7
```

The following words are **reserved** and cannot be used as names:

```text
and     as      assert  def     elif    else    false   fn
for     from    if      import  in      let     match   mock
none    not     or      test    then    true
```

(`def`, `match`, `import`, `from`, `as`, `test` and `mock` are reserved for
features that are not implemented yet; see [§19](#19-reserved-syntax-not-implemented-yet).
`for` and `in` are used by lists, [§18](#18-lists-and-comprehensions).)

The single underscore `_` is special: it discards a value (`let _ = …`),
names an unused lambda parameter, or marks a missing argument
(`f(a, _)`). It can never be read (E0206).

---

## 3. Literals

### Integers

`int` is a 64-bit signed integer.

```valo
print(42)               # → 42
print(1_000_000)        # → 1000000   (underscores between digits)
print(0xff)             # → 255       (hexadecimal)
print(0o755)            # → 493       (octal)
print(0b1010)           # → 10        (binary)
```

Rules:

- `_` may only sit **between two digits**: `1_000` is fine; `_1`, `1_` and
  `1__0` are errors (E0010).
- Decimal integers cannot have **leading zeros**: `007` is an error (write
  `0o7` for octal). `0` and `00` are allowed.
- A literal outside the 64-bit range is an error (E0002). The smallest
  `int` is written `-9223372036854775807 - 1`.
- A number directly followed by letters (`12abc`) is an error.

### Floats

`float` is a 64-bit IEEE 754 number. A float literal has a fraction, an
exponent, or both:

```valo
print(1.5)              # → 1.5
print(2e3)              # → 2000.0
print(1.25e-2)          # → 0.0125
print(3.0)              # → 3.0
```

A fraction needs a digit after the dot: `1.` followed by a non-digit is the
integer `1` followed by `.`. There is no leading-dot form (`.5`).

### Booleans, none and unit

```valo
print(true)             # → true
print(false)            # → false
print(none)             # → none
print(())               # → ()
```

- `none` is the "no value" value (see [§14](#14-none-results-and-)).
- `()` is the **unit** value: the result of an expression that has nothing
  useful to return, such as `print(…)` or `assert …`. It is also the unit
  type in annotations: `-> ()`.

---

## 4. Strings

### Interpolation

Every ordinary string literal interpolates: `{` … `}` holds any expression,
whose value is converted to text.

```valo
let a = 2
let b = 3
print("{a} + {b} = {a + b}")            # → 2 + 3 = 5
fn greet(name: str) -> str = "Hello, {name}!"
print(greet("valo"))                    # → Hello, valo!
print("nested: {"inner {a * 10}"}")     # → nested: inner 20
print("{1.5} {true} {()}")              # → 1.5 true ()
```

An interpolation may contain any expression, including another string
literal. `{}` with nothing inside is an error (E0008).

### Escapes

| Escape      | Meaning                          |
|-------------|----------------------------------|
| `\n`        | line feed                        |
| `\t`        | tab                              |
| `\r`        | carriage return                  |
| `\0`        | NUL character                    |
| `\"`        | `"`                              |
| `\\`        | `\`                              |
| `\{`        | literal `{`                      |
| `\}`        | literal `}`                      |
| `\u{1F600}` | Unicode scalar, 1–6 hex digits   |

Any other `\x` is an error (E0005). A lone `}` in string text is a literal
`}`; only `{` must be escaped.

```valo
print("tab:\tend")                      # → tab:	end
print("quote: \" backslash: \\")        # → quote: " backslash: \
print("braces: \{x} and }")             # → braces: {x} and }
print("smile: \u{1F600}")               # → smile: 😀
```

### Raw strings

`r"…"` is a raw string: no escapes and no interpolation. A raw string cannot
contain `"`.

```valo
print(r"C:\temp\{name}")                # → C:\temp\{name}
```

### One line only

A string literal — and every interpolation inside it — must end on the line
where it starts (E0004, E0007). Use `\n` for line breaks inside a string.

### Concatenation

`+` joins two strings. Mixing a string with another type is an error; use
interpolation or `str(x)`:

```valo
let n = 3
print("n = " + str(n))                  # → n = 3
print("n = {n}")                        # → n = 3
```

---

## 5. Bindings: `let`

```valo
let port = 8080
let host: str = "nas"                  # optional type annotation
print("{host}:{port}")                 # → nas:8080
```

- `let` is **mandatory** for every binding; there is no bare `x = 1`.
- Values are **immutable**. To "change" a name, bind it again: a new `let`
  of the same name **shadows** the old one, in the same scope or a nested
  one. The right-hand side still sees the previous binding.

```valo
let text = "  hello  "
let text = trim(text)                  # shadows; RHS reads the old `text`
print("[{text}]")                      # → [hello]
```

- `let _ = expr` evaluates `expr` and discards the value:

```valo
let _ = print("side effect")           # → side effect
```

- A `let` is not recursive: its right-hand side cannot refer to the name
  being defined (use `fn` for recursion).

---

## 6. Operators

### Arithmetic

| Operator | Meaning                         | Example          | Result |
|----------|---------------------------------|------------------|--------|
| `+`      | addition / string concatenation | `7 + 2`          | `9`    |
| `-`      | subtraction, unary negation     | `7 - 2`, `-x`    | `5`    |
| `*`      | multiplication                  | `7 * 2`          | `14`   |
| `/`      | **true division, always float** | `7 / 2`          | `3.5`  |
| `//`     | floor division                  | `-7 // 2`        | `-4`   |
| `%`      | floor modulo (sign of divisor)  | `-7 % 3`         | `2`    |
| `**`     | power, right-associative        | `2 ** 3 ** 2`    | `512`  |

```valo
print(7 / 2)                # → 3.5
print(-7 // 2)              # → -4
print(-7 % 3)               # → 2
print(2 ** 3 ** 2)          # → 512
print(-2 ** 2)              # → -4      (that is -(2 ** 2))
print(2.0 ** -1.0)          # → 0.5
```

`int` and `float` never mix implicitly: `1 + 2.0` is a runtime error. The
one exception is `/`, which always produces a float. Convert explicitly with
`float(x)` or `int(x)` (which truncates toward zero); text becomes a number
with `parse_int` / `parse_float`. Integer overflow, an
integer power with a negative exponent, and division by zero are runtime
errors. There is no unary `+`.

### Comparison

`==  !=  <  <=  >  >=` compare values of the same type and produce a `bool`.
Comparisons **chain** as in Python: `a < b <= c` means
`a < b and b <= c`, with `b` evaluated once.

```valo
let x = 50
print(0 < x <= 100)         # → true
print(1 < 2 > 3)            # → false
print(none == 0)            # → false   (anything can be compared to none with ==/!=)
```

Comparing an `int` with a `float` or a `str`, or ordering values that have no
order (`true < false`), is a runtime error.

### Logic

`and`, `or`, `not` take `bool` operands only (a non-bool operand is a runtime
error). `and` and `or` short-circuit.

```valo
let x = 5
print(x > 0 and x < 10)     # → true
print(not x == 5)           # → false   (not binds looser than ==)
print(false or true)        # → true
```

### Precedence

From **lowest** to **highest** binding:

| Level | Operators                          | Associativity                          |
|-------|------------------------------------|----------------------------------------|
| 1     | `x => …`, `if … then … else …`, `assert …` | prefix forms that extend as far right as possible |
| 2     | `|>`                               | left                                   |
| 3     | `or`                               | left                                   |
| 4     | `and`                              | left                                   |
| 5     | `not`                              | prefix                                 |
| 6     | `==  !=  <  <=  >  >=`             | chained                                |
| 6     | `in`, `not in`                     | not chained                            |
| 7     | `+  -`                             | left                                   |
| 8     | `*  /  //  %`                      | left                                   |
| 9     | unary `-`                          | prefix                                 |
| 10    | `**`                               | right (right operand may be negated: `2 ** -1`) |
| 11    | call `f(…)`, index `xs[i]`, postfix `?` | postfix                           |

Parentheses group: `(1 + 2) * 3`.

---

## 7. Conditionals: `if`

`if` is an expression, and **both branches are required** (E0102 for a
missing `else`): its value must exist in every case.

### Inline form: `then` / `else`

```valo
let n = 7
let parity = if n % 2 == 0 then "even" else "odd"
print(parity)                                  # → odd
print(if n < 0 then "neg" else if n == 0 then "zero" else "pos")   # → pos
```

The inline form has no `elif`; chain with `else if`. Like any expression it
must fit on one logical line — wrap it in parentheses to spread it over
several lines, or use the block form.

### Block form: `:` / `elif` / `else`

```valo
fn grade(p: int) -> str:
    if p > 90: "crit"
    elif p > 80: "warn"
    else: "ok"

print(grade(85))                               # → warn
```

Each branch is either an expression on the same line after `:` or an
indented block on the following lines. The two styles can be mixed:

```valo
fn describe(n: int) -> str:
    if n > 100:
        let excess = n - 100
        "too big by {excess}"
    elif n < 0: "negative"
    else: "fine"

print(describe(130))                           # → too big by 30
print(describe(5))                             # → fine
```

`elif` and `else` start their own lines at the same indentation as the line
holding the `if`. The condition must be a `bool` (runtime error otherwise).

---

## 8. Blocks are expressions

An indented block is a sequence of lines; its value is the value of its
**last line**, which must therefore be an expression — a block cannot end
with a `let` or a `fn` (E0104). Names bound inside a block are visible only
until the end of that block.

```valo
let area = if true:
    let w = 3
    let h = 4
    w * h
else: 0
print(area)                                    # → 12
```

At the top level of a file, the last line may be anything. The values of
top-level expression lines are discarded — except an `Err`, which stops the
program (see [§14](#14-none-results-and-)).

---

## 9. Functions: `fn`

### One-line and block bodies

```valo
# `=` body: a single expression
fn add(x: int, y: int) -> int = x + y

# `:` body: an indented block; the value is its last line
fn hyp2(a: int, b: int) -> int:
    let aa = a * a
    let bb = b * b
    aa + bb

# a `=` body may sit alone on the next, indented line
fn sum_sq(a: int, b: int) -> int =
    a * a + b * b

print(add(1, 2))                               # → 3
print(hyp2(3, 4))                              # → 25
print(sum_sq(1, 2))                            # → 5
```

A `:` body may also be a single expression on the same line:
`fn one() -> int: 1`.

### Annotations are required

Every parameter and the return type of a `fn` **must** be annotated (E0410).
A function that returns nothing useful declares `-> ()`:

```valo
fn log(msg: str) -> ():
    print("[log] {msg}")

log("started")                                 # → [log] started
```

(Annotations are checked for well-formedness today; checking values against
them at runtime is planned for a later milestone.)

### Type parameters

A generic function lists its type variables in brackets after the name:

```valo
fn first_or[T](x: T | none, default: T) -> T =
    if x == none then default else x

print(first_or(none, 7))                       # → 7
print(first_or("a", "b"))                      # → a
```

### Default parameters are keyword-only

A parameter may have a default value. Such parameters:

- must come **after** all required parameters (E0423);
- can only be passed **by name** (E0421 if passed by position);
- have their default evaluated **at each call** that omits them, and the
  default may use earlier parameters.

```valo
fn area(w: int, h: int = 2, scale: int = 1) -> int = w * h * scale
fn span(start: int, stop: int = start + 10) -> int = stop - start

print(area(3))                                 # → 6
print(area(3, h=5))                            # → 15
print(area(3, scale=10, h=5))                  # → 150
print(span(5))                                 # → 10
```

Required parameters are always passed by position (E0424 if passed by name).

### Recursion, hoisting and tail calls

A top-level `fn` is visible in the **whole file**, so functions can call each
other in any order, and a program may use a function before the line that
defines it. Two top-level `fn`s with the same name are an error (E0207); a
later top-level `let` of the same name shadows the `fn` from that line on.

Calls in tail position are **proper tail calls**: they do not grow the stack,
so recursion can replace loops.

```valo
print(is_even(10))                             # → true

fn is_even(n: int) -> bool = if n == 0 then true else is_odd(n - 1)
fn is_odd(n: int) -> bool = if n == 0 then false else is_even(n - 1)

fn sum_to(n: int, acc: int = 0) -> int =
    if n == 0 then acc else sum_to(n - 1, acc=acc + n)

print(sum_to(100000))                          # → 5000050000
```

### Local functions

A `fn` may be defined inside a block. It is visible from its own line to the
end of the block (not hoisted), may call itself, and captures the values of
enclosing locals (a closure). A local `fn` may not call a *later* local `fn`
of the same block (E0204); put mutually recursive functions at the top level.

```valo
fn make_adder(n: int) -> (int) -> int:
    fn add(x: int) -> int = x + n
    add

let add2 = make_adder(2)
print(add2(40))                                # → 42
```

---

## 10. Calls and keyword arguments

```text
f()                    no arguments
f(a, b)                positional arguments
f(a, key=v)            keyword arguments come after positional ones
f(a, b,)               a trailing comma is allowed
f(a)(b)                calling the result of a call
```

- Parentheses are mandatory; there is no juxtaposition (`f x`) and no
  automatic currying. Calling with the wrong number of arguments is an error
  (at compile time when the callee is known, E0205; otherwise at runtime).
- A positional argument after a keyword argument is a syntax error.
- Keyword arguments need a callee known at compile time — a `fn`, a
  built-in, or a `let` bound to one of those (E0422 when calling a computed
  function value with keywords).
- Arguments are evaluated left to right, in the order written.

```valo
fn scale(x: int, by: int = 2, offset: int = 0) -> int = x * by + offset
print(scale(5))                                # → 10
print(scale(5, offset=1, by=10))               # → 51
print("no newline", end="")
print("!")                                     # → no newline!
```

---

## 11. Lambdas and closures

A lambda is an anonymous function written with `=>`:

```text
x => x + 1                one parameter, no parentheses
(a, b) => a * b           several parameters
(x: int) => x * 2         optional annotations
() => 42                  no parameters
_ => 0                    an ignored parameter
```

```valo
let inc = x => x + 1
let mul = (a: int, b: int) => a * b
let answer = () => 42
print(inc(1))                                  # → 2
print(mul(6, 7))                               # → 42
print(answer())                                # → 42
print((x => x * x)(9))                         # → 81
```

### Block-bodied lambdas

`=>` at the end of a line opens an indented block:

```valo
let describe = (n: int) =>
    let doubled = n * 2
    let squared = n * n
    doubled + squared
print(describe(3))                             # → 15
```

Inside brackets line breaks are ignored, so a block-bodied lambda cannot be
written directly as a call argument; bind it with `let` first.

### How far a lambda extends

A lambda body extends **as far to the right as possible**:
`x => x + 1 |> f` is `x => ((x + 1) |> f)`. Parenthesise the lambda to end it
earlier: `(x => x + 1) |> f`.

### Rules

- Lambda parameters cannot have defaults (use a named `fn`), and annotations
  are optional.
- Lambdas and local `fn`s are **closures**: they capture the *values* of the
  enclosing locals they use, at the moment they are created. Since values are
  immutable, a later `let` of the same name does not affect a closure.

```valo
fn compose(f: (int) -> int, g: (int) -> int) -> (int) -> int =
    x => g(f(x))

let x = 10
let add_x = y => x + y
let x = 1000                                   # shadows; add_x keeps 10
print(add_x(1))                                # → 11
print(compose(y => y + 1, y => y * 2)(5))      # → 12
```

---

## 12. Partial application with `_`

`_` in place of an argument makes a call **partial**: instead of calling, it
builds a function that waits for the missing arguments, in left-to-right
order.

```valo
fn sub(a: int, b: int) -> int = a - b
let ten_minus = sub(10, _)                     # x => sub(10, x)
let minus_ten = sub(_, 10)                     # x => sub(x, 10)
let swap_sub = sub(_, _)                       # (x, y) => sub(x, y)
print(ten_minus(3))                            # → 7
print(minus_ten(3))                            # → -7
print(swap_sub(5, 2))                          # → 3
```

The other arguments (and the callee, if it is an expression) are evaluated
**once**, when the partial is built, not at every call. `_` is only valid as
a whole positional argument; `f(_ + 1)` is an error (E0206).

---

## 13. The pipe `|>`

`x |> f(a, b)` is `f(x, a, b)`: the value on the left becomes the **first**
argument of the call on the right. If the right side is not a call, it is
called with the value: `x |> f` is `f(x)`.

```valo
fn double(x: int) -> int = x * 2
fn sub(a: int, b: int) -> int = a - b

print(5 |> double)                             # → 10
print(5 |> sub(1))                             # → 4    sub(5, 1)
print(1 |> sub(10, _))                         # → 9    the value fills the first `_`
5 |> double |> print                           # → 10
```

- If the call on the right contains `_`, the piped value fills the **first**
  `_` instead of becoming the first argument. Remaining `_`s keep it a
  partial: `x |> f(_, _)` is `f(x, _)`.
- Keyword arguments stay where they are: `x |> scale(by=10)` is
  `scale(x, by=10)`.
- `|>` has the **lowest** operator precedence: `1 + 2 |> f` is `f(1 + 2)`.
- A line starting with `|>` continues the previous line, so long pipelines
  read top to bottom:

```valo
fn scale(x: int, by: int = 2, offset: int = 0) -> int = x * by + offset

let n = 5
    |> scale(offset=1)
    |> scale(by=10)
print(n)                                       # → 110
```

- The piped value is evaluated before the callee and the other arguments.

---

## 14. none, results and `?`

### none

`none` stands for "no value". A value annotated `T | none` is either a `T` or
`none` — there is no wrapper. Test it with `==` / `!=`, or use the helpers
`or_else` and `unwrap`:

```valo
let a = parse_int(" 42 ")
let b = parse_int("forty-two")
print(a)                                       # → 42
print(b)                                       # → none
print(b == none)                               # → true
print(or_else(b, 0))                           # → 0
```

### Results: `Ok` and `Err`

Operations that can fail return a **result**: `Ok(value)` or `Err(error)`.
Its type is written `T ! E`, e.g. `int ! error`.

```valo
fn parse_port(s: str) -> int ! error:
    let n = parse_int(s)
    if n == none: Err("not a number: {s}")
    elif 0 < n <= 65535: Ok(n)
    else: Err("out of range: {n}")

print(parse_port("8080"))                      # → Ok(8080)
print(parse_port("x"))                         # → Err("not a number: x")
print(is_ok(parse_port("99999")))              # → false
print(or_else(parse_port("x"), 80))            # → 80
```

### Propagating errors with `?`

Postfix `?` unwraps a result: on `Ok(v)` the expression's value is `v`; on
`Err(e)` the enclosing function (the nearest `fn` or lambda) returns
`Err(e)` immediately. `?` on a value that is not a result is a runtime error.

`context(result, "message")` adds a message to an error chain and passes
`Ok` values through. It reads well in a pipe: in `x |> f(a)?`, the `?`
applies to the whole stage `f(x, a)`.

```valo
fn parse_port(s: str) -> int ! error:
    let n = parse_int(s)
    if n == none then Err("not a number: {s}") else Ok(n)

fn load(raw: str) -> int ! error:
    let port = parse_port(raw) |> context("reading the port")?
    Ok(port + 1)

print(load("8080"))                            # → Ok(8081)
print(is_err(load("http")))                    # → true
```

`?` binds as tightly as a call: `1 + f(x)?` is `1 + (f(x)?)`.

### Unhandled errors at the top level

A top-level line whose value is an `Err` stops the program with exit code 1,
printing the error and its context chain, as if the line ended with `?`. The
same happens for `?` at the top level.

```text
fn inner() -> int ! error = Err("bad port")
fn outer() -> int ! error:
    let v = inner() |> context("loading config")?
    Ok(v)
outer()?            # stops here with exit code 1
print("never")
```

```text
error[R0601]: unhandled Err: "bad port"
   ╭─[ example.vl:5:1 ]   (source box with line 5 shortened here)
  context (outermost first):
    loading config
  origin: inner at example.vl:1:29
  propagated by `?` in:
    outer at example.vl:3:13
    <main> at example.vl:5:1
```

---

## 15. `assert`

`assert expr` checks that a `bool` expression is `true` and otherwise stops
the program with a runtime error that shows the expression (and, for a
comparison, both sides). It is an expression of type `()`, usable anywhere.

```valo
fn fact(n: int) -> int = if n <= 1 then 1 else n * fact(n - 1)
assert fact(5) == 120
assert 0 < fact(3) <= 6
print("ok")                                    # → ok
```

`assert` extends as far right as possible: `assert a == b` asserts the whole
comparison.

---

## 16. Type annotations

Annotations appear after `:` on `let` bindings and parameters and after `->`
on `fn` return types. Types live in their own namespace, separate from
values.

| Syntax               | Meaning                                     |
|----------------------|---------------------------------------------|
| `int`, `float`, `bool`, `str` | scalar types                       |
| `none`               | the type of `none`                          |
| `()`                 | the unit type                               |
| `any`                | any value                                   |
| `error`              | the error payload type                      |
| `list[int]`          | list whose elements are all `int`           |
| `dict[str, int]`, `tuple[int, str]`, `(int, str)` | reserved for dicts and tuples (not supported yet, E0009) |
| `T | none`           | union / optional                            |
| `T ! error`          | result: `Ok(T)` or `Err(error)`             |
| `int -> int`         | function type                               |
| `(int, int) -> int`  | function of two parameters                  |
| `() -> str`          | function of no parameters                   |
| `T`                  | a type variable declared in `fn f[T](…)`    |

Precedence in types, from loosest to tightest: `->` (right-associative) <
`!` < `|`. So `int | none ! error` is a result of an optional `int`, and
`int -> int -> int` is `int -> (int -> int)`. Parentheses group:
`(int | none) -> str`.

```valo
let r: int | none ! error = Ok(none)
let f: (int, int) -> int = (a, b) => a + b
let g: int -> int -> int = a => b => a * b
print(f(2, 3) + g(4)(5))                       # → 25
```

Unknown type names (E0402) and the wrong number of type arguments
(`list[int, int]`, E0403) are compile errors.

**Annotations are checked when the program runs** (strict, like Pydantic's
strict mode): a value that does not match stops the program with R0204.
Nothing is converted — an `int` is not a `float`.

- `let x: T = e` checks the value of `e`; parameters are checked when the
  function starts (also defaults, and also calls through a function value);
  the return type is checked when the function returns, even after tail
  calls.
- `list[T]` checks every element and names the first bad one:
  `binding rows: expected list[list[int]], but element [2][2] is str ("5")`.
- `T | none` accepts `none` or a `T`. `T ! error` accepts any `Err` and an
  `Ok(v)` whose `v` is a `T` — a plain `T` is *not* wrapped in `Ok` for you
  (the message says `wrap the value in Ok(...)`).
- A function type checks only that the function accepts that many
  arguments. Type variables (`T`) and `any` accept everything.
- A literal that cannot match is found at compile time (E0401):

```valo
let ratio: float = 1          # error[E0401]: binding ratio: expected float, got int (1)
fn mean(xs: list[float]) -> float = reduce(xs, 0.0, (a, x) => a + x) / float(len(xs))
print(mean([1.0, 2.0]))       # → 1.5
print(mean([1, 2]))           # error[R0204]: parameter xs of mean: expected list[float], but element [0] is int (1)
```

---

## 17. Built-in functions

These are always available (the *prelude*). Like any global name, they can
be shadowed by a `let` or `fn` of the same name.

| Function                        | Result                                                    |
|---------------------------------|-----------------------------------------------------------|
| `print(x, end="\n")`            | writes `x` then `end`; strings print without quotes; `()` |
| `int(x)`                        | `float` → `int`, truncating toward 0; an `int` unchanged  |
| `float(x)`                      | `int` → `float`; a `float` unchanged                      |
| `str(x)`                        | text form of any value                                    |
| `str_len(s)`                    | number of characters in `s`                               |
| `trim(s)`                       | `s` without leading and trailing whitespace               |
| `parse_int(s)`, `parse_float(s)`| the number in `s` (whitespace ignored), or `none`         |
| `Ok(v)`, `Err(e)`               | build a result                                            |
| `is_ok(r)`, `is_err(r)`         | test a result                                             |
| `context(r, msg)`               | add `msg` to an `Err`'s chain; `Ok` passes through        |
| `unwrap(x)`                     | value of `Ok`; runtime error for `Err` or `none`; else `x`|
| `or_else(x, default)`           | `default` for `none` or `Err`; value of `Ok`; else `x`    |
| `read_line()`                   | next line of standard input, or `none` at the end         |
| `read_file(path)`               | `Ok(text)` or `Err("cannot read …")`                      |
| `write_file(path, content)`     | `Ok(())` or `Err("cannot write …")`                       |
| `len(x)`                        | number of elements of a list, or characters of a str      |
| `get(xs, i)`                    | `xs[i]`, or `none` when `i` is out of range               |
| `range(n)`, `range(a, b)`       | `[0, …, n-1]`, `[a, …, b-1]`                              |
| `map(xs, f)`                    | `[f(x) for x in xs]`                                      |
| `filter(xs, p)`                 | the elements for which `p` returns `true`                 |
| `reduce(xs, init, f)`           | `f(…f(f(init, x0), x1)…)`                                 |
| `flat_map(xs, f)`               | the lists `f(x)` joined                                   |
| `each(xs, f)`                   | calls `f(x)` for every element; `()`                      |
| `contains(xs, x)`               | `x in xs`                                                 |
| `append(xs, x)`, `reverse(xs)`  | a new list with `x` at the end / in reverse order         |
| `lines(s)`                      | the lines of `s`, without `
` / `
`                   |
| `split(s, sep)`, `join(xs, sep)`| like Python's `s.split(sep)` and `sep.join(xs)`           |

```valo
# Number the lines of standard input.
fn number_lines(n: int) -> ():
    let line = read_line()
    if line == none: ()
    else:
        print("{n}: {line}")
        number_lines(n + 1)

number_lines(1)
```

---

## 18. Lists and comprehensions

### List literals and indexing

A list is written in brackets; it may be empty, nest, and span several
lines (line breaks inside brackets are ignored). Lists are immutable values.

```valo
let xs = [10, 20, 30]
let grid = [
    [1, 2],
    [3, 4],
]
print(xs)                           # → [10, 20, 30]
print([])                           # → []
print(["a", "b"])                   # → ["a", "b"]
print(xs[0] + grid[1][0])           # → 13
print(xs + [40])                    # → [10, 20, 30, 40]
print(xs == [10, 20, 30])           # → true
```

Strings inside a list print with quotes. `xs[i]` needs `0 <= i < len(xs)`;
anything else is a runtime error (R0401) that names the access path:

```text
error[R0401]: grid[1][5]: index out of range (len 2)
```

Negative indices are not supported; use `get(xs, i)`, which returns `none`
instead of failing. `+` builds a new list; `==` compares element by
element; lists cannot be ordered with `<`.

### Membership: `in`, `not in`

```valo
let hosts = ["nas", "pi"]
print("pi" in hosts)                # → true
print("mac" not in hosts)           # → true
print("disk" in "disk full")        # → true
```

On a string, `in` tests for a substring. `in` binds like a comparison
(`x + 1 in xs` is `(x + 1) in xs`) but does not chain: `a < b in c` is an
error (E0101) — add parentheses.

### Comprehensions

`[expr for x in list]` builds a list from another one; `if` clauses keep
only some elements; further `for` clauses nest, and may use the earlier
loop variables:

```valo
print([x ** 2 for x in range(10) if x % 2 == 0])   # → [0, 4, 16, 36, 64]
print([x + y for x in [10, 20] for y in [1, 2]])   # → [11, 12, 21, 22]
print([[x, y] for x in range(3) for y in range(x)])
# → [[1, 0], [2, 0], [2, 1]]
```

A comprehension is shorthand for `map`, `filter` and `flat_map` with
lambdas (they appear as `<comprehension@line:col>` in stack traces), and it
keeps working even if your program rebinds those names. The loop variable is
a single name (or `_`).

`?` cannot be used inside a comprehension (E0208): it would only leave the
comprehension's hidden function, not your `fn`. Build the list of results
first and check it, or loop with a recursive `fn`.

### Higher-order built-ins

The list built-ins take the list **first**, so they read well in a pipe:

```valo
let total = range(1, 11)
    |> filter(x => x % 2 == 0)
    |> map(x => x ** 2)
    |> reduce(0, (acc, x) => acc + x)
print(total)                                       # → 220

let words = "the quick brown fox" |> split(" ")
print(len(words))                                  # → 4
print(words |> map(str_len) |> map(str) |> join("+"))   # → 3+5+5+3
each(["a", "b"], s => print("item {s}"))
# → item a
# → item b
```

An error inside the function you pass shows the built-in in the stack
trace, between your function and its caller:

```text
stack trace (most recent call first):
  0: <lambda@4:13> at err.vl:4:18
  1: <native map>
  2: halve_all     at err.vl:4:5
  3: <main>        at err.vl:6:7
```

---

## 19. Reserved syntax (not implemented yet)

The lexer already recognises these forms so that existing programs keep
working when they arrive; today they are reported as "not supported yet"
(E0009):

| Syntax                                 | Planned meaning                   |
|----------------------------------------|-----------------------------------|
| `{"k": v}`                             | dict literals                     |
| `d.key`, `record.field`                | field access                      |
| `[f(a, b) for (a, b) in pairs]`        | destructuring loop variables      |
| `match v:` with `pat ~> expr` arms     | pattern matching                  |
| `def Name as …`                        | records, tagged unions, aliases   |
| `import m`, `from m import f`          | modules                           |
| `` `cmd {arg}` ``                      | command literals                  |
| `re"…"`, `=~`, `!~`                    | regular expressions               |
| `test "name":`, `mock f = …`           | tests                             |

The tokens `~>` (match arms) and `|` (unions, process pipes) exist already;
`|` is used today only in types.
