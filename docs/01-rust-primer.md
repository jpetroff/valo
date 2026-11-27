# Chapter 1 — A Rust Primer for This Codebase

This chapter explains every Rust feature the valo code uses, each with an
example taken from the code. It is not a full Rust course (for that, read
*The Rust Programming Language*, "the Book", chapters 1–10 and 13), but it is
enough to read and change any file in the repository.

Each source file's guide lists the sections of this chapter it relies on,
e.g. "(*Pattern matching*)".

## Cargo, crates and workspaces

- A **crate** is a Rust package: a library, a binary, or both. valo has two:
  `valo-compiler` (folder `compiler/`) and `valo-vm` (folder `vm/`).
- **Cargo** is the build tool and package manager. Each crate has a
  `Cargo.toml` manifest; the top-level `Cargo.toml` defines a **workspace**
  that builds both crates together and shares one `target/` folder and one
  `Cargo.lock` (exact dependency versions).
- External libraries ("dependencies", also crates) come from crates.io. valo
  declares them once under `[workspace.dependencies]` in the root
  `Cargo.toml`, and each crate opts in with `name.workspace = true`.
  Used today: `thiserror` (error types), `ariadne` (error rendering), `clap`
  (command line); for tests `insta`, `proptest`, `pretty_assertions`.
- A crate can contain a library (`src/lib.rs`) and binaries (`src/main.rs`,
  `src/bin/*.rs`). The compiler crate has both: the library `valo_compiler`
  and the binary `valo`. Note: package names use `-`, Rust code uses `_`
  (`valo-vm` is imported as `valo_vm`).

Everyday commands (run in `/home/coder/valo`):

```bash
cargo build --workspace
```
```bash
cargo test --workspace
```
```bash
cargo run -q --bin valo -- run file.vl --echo
```
```bash
cargo clippy --workspace --all-targets -- -D warnings
```
```bash
cargo fmt --all
```
```bash
cargo doc --workspace --no-deps --open
```

`clippy` is a linter (style and bug-pattern warnings), `fmt` formats code
automatically — never format by hand, just run it.

## Modules and use

- Every `.rs` file is a **module**. `lib.rs` declares them:
  `pub mod lexer;` means "the module `lexer` is in `lexer.rs`, and it is
  public (`pub`) — visible to other crates".
- Items (functions, types) are private by default; `pub` exports them.
- Paths: `crate::span::Span` = the `Span` type in this crate's `span` module;
  `super::*` = everything from the parent module (used by test modules);
  `valo_vm::chunk::Program` = an item from another crate.
- `use` imports a path so it can be written short:

  ```rust
  use crate::token::{Keyword, Token, TokenKind};   // compiler/src/lexer.rs
  ```

## Structs

A struct groups named fields:

```rust
pub struct Span {          // compiler/src/span.rs
    pub start: u32,
    pub end: u32,
}
let s = Span { start: 3, end: 5 };
s.start                     // field access
```

Variants you will see:

- **tuple struct** with unnamed fields: `pub struct Instr(pub u32);` — the
  field is `instr.0` (`vm/src/opcode.rs`). A one-field tuple struct is a
  *newtype*: a distinct type wrapping another (`NodeId(u32)` in `ast.rs`).
- **unit struct** with no fields: `pub struct StdHost;` (`vm/src/host.rs`).
- Field init shorthand: `SrcSpan { start, end }` means
  `SrcSpan { start: start, end: end }`.

Methods are defined in an `impl` block:

```rust
impl Span {
    pub fn new(start: usize, end: usize) -> Span { ... }   // associated fn: Span::new(1, 2)
    pub fn to(self, other: Span) -> Span { ... }            // method: a.to(b)
}
```

A function whose first parameter is `self`/`&self`/`&mut self` is a method;
otherwise it is called with `Type::name(...)` (like a static method).

Standard collections used: `Vec<T>` (growable array: `push`, `pop`, `last`,
`get(i)`, indexing `v[i]`), `HashMap<K, V>` (dictionary: `get`, `insert`),
`String` (owned text).

## Enums

Rust enums are *tagged unions*: each variant may carry its own data.

```rust
pub enum ExprKind {                        // compiler/src/ast.rs
    Int(i64),                              // tuple-like variant
    Unary { op: UnaryOp, operand: Box<Expr> },   // struct-like variant
    Binary { op: BinOp, op_span: Span, lhs: Box<Expr>, rhs: Box<Expr> },
}
```

Simple enums without data work like C enums (`enum BinOp { Add, Sub, … }`).
`#[repr(u8)]` with `LOADI = 2` fixes each variant's numeric value
(`vm/src/opcode.rs`), so `op as u8` gives the opcode byte.

Enums are the backbone of a compiler: one enum per kind of tree node or
token, matched exhaustively (next section).

## Pattern matching

`match` compares a value against patterns, top to bottom, and **must cover
every case** — the compiler refuses to build otherwise. This is the single
most useful safety net when extending the language: add a variant to
`ExprKind` and the compiler lists every `match` that needs a new arm.

```rust
match &e.kind {                                     // compiler/src/codegen.rs
    ExprKind::Int(n) => self.load_int(*n, dst, e.span),
    ExprKind::Unary { op: UnaryOp::Neg, operand } => { ... }
    ExprKind::Binary { op, op_span, lhs, rhs } => { ... }
}
```

Pattern features used in the code:

| Pattern | Meaning | Example |
|---------|---------|---------|
| `A \| B` | either | `OpCode::IDIV \| OpCode::MOD` |
| `x` (a name) | anything, bound to `x` | `other => …` |
| `_` | anything, ignored | `_ => break` |
| `Some('x' \| 'X')` | nested patterns | `lexer.rs` `number` |
| `pat if cond` | **guard**: arm applies only if `cond` is true | `'-' if two(self, "->") => …` |
| `(a, b)` | match two values at once | `match (b, c)` in `vm.rs` `arith` |

Shorter forms:

- `if let Some(x) = opt { … }` — run the block only if the pattern matches.
- `while let Some(c) = self.peek() { … }` — loop while it matches.
- `let Some(c) = self.peek() else { break };` — **let-else**: bind or leave
  (the `else` block must `return`, `break` or `continue`).
- `matches!(kind, TokenKind::LParen | TokenKind::LBracket)` — a `bool`.

## Option and Result

Rust has no `null` and no exceptions. Instead:

- `Option<T>` is `Some(value)` or `None` — "maybe a value".
  `self.src[self.pos..].chars().next()` returns `Option<char>`: `None` at the end.
- `Result<T, E>` is `Ok(value)` or `Err(error)` — "success or failure".
  `compile_source` returns `Result<Program, Vec<Diagnostic>>`.

The **`?` operator** propagates errors: `let p = parse_source(sm)?;` means
"if this is `Err(e)`, return `Err(e)` from the current function right now;
otherwise unwrap the `Ok` value". This replaces exception throwing.

Helpers you will meet:

| Method | Does |
|--------|------|
| `opt.ok_or_else(\|\| err)` | `Option` → `Result`: `None` becomes `Err(err)` |
| `res.map_err(f)` | transform only the error |
| `res.map(f)` / `opt.map(f)` | transform only the success value |
| `x.expect("msg")` | unwrap or **panic** (crash) with `msg` — used only where failure would be a bug |
| `x.unwrap_or_default()` | unwrap or use the type's default value |
| `opt.is_some_and(\|c\| …)` | `Some` and the condition holds |
| `opt.take()` | move the value out, leave `None` |

In the parser, `type PResult<T> = Result<T, Failed>` is a *type alias*: the
error side carries no data because the diagnostic was already stored.

## Ownership and borrowing

Rust's central rule: every value has exactly one **owner**; when the owner
goes out of scope, the value is freed. There is no garbage collector.

- **Move**: `let a = b;` or passing `b` to a function moves it; `b` cannot be
  used afterwards (for most types). `fn finish(mut self)` in `codegen.rs`
  consumes the `FuncState` — after `finish` it is gone, by design.
- **Borrow**: `&x` lends read-only access, `&mut x` lends exclusive write
  access. Many readers *or* one writer, never both at once. Functions take
  `&SourceMap` because they only need to read it.
- **Copy** types (integers, `bool`, `char`, small structs marked
  `#[derive(Copy)]` like `Span`) are duplicated instead of moved.
- **`.clone()`** makes an explicit copy. The parser clones the current token
  (`self.peek().clone()`) so it can keep it while advancing the cursor.
- **`Box<T>`** owns a value on the heap. A recursive type needs it:
  `Expr` contains `Box<Expr>` because a struct cannot contain itself directly
  (its size would be infinite).
- **Builder pattern**: methods that take `self` and return `Self` can be
  chained: `Diagnostic::new(..).label("..").help("..")`,
  `ProtoBuilder::new(..).op(..).op(..).program()`.

## Lifetimes

A **lifetime** (`'a`) names how long a borrow is valid. You meet them where a
struct *holds* a reference:

```rust
struct Lexer<'a> { src: &'a str, ... }        // compiler/src/lexer.rs
pub struct Vm<'h> { host: &'h mut dyn Host, ... }   // vm/src/vm.rs
```

Read `Lexer<'a>` as "a lexer that borrows a string for some period `'a`, and
may not outlive it". The compiler checks this; you rarely write lifetimes
anywhere else. `impl Lexer<'_>` means "for whatever lifetime". `&'static str`
is a reference valid for the whole program — string literals baked into the
binary, like error codes `"E0101"`.

## Rc shared ownership

Sometimes several parts need to own the same value. `Rc<T>` ("reference
counted") allows that: `Rc::clone(&x)` creates another owner by incrementing
a counter — the data is **not** copied; the value is freed when the last
owner disappears.

- `Program.main: Rc<Proto>` — the VM frame and the program share the prototype.
- `Proto.name: Rc<str>` — function names are copied into every stack-trace
  entry cheaply.

Writing `Rc::clone(&x)` rather than `x.clone()` is a convention to make clear
that only a pointer is copied. Later milestones use `Rc` for strings and lists
(valo values are immutable, so sharing is safe).

## Traits, impl and derive

A **trait** is an interface: a set of methods a type promises to provide.

```rust
pub trait Host {                                   // vm/src/host.rs
    fn write_stdout(&mut self, s: &str) -> io::Result<()>;
    fn write_stderr(&mut self, s: &str) -> io::Result<()>;
}
impl Host for StdHost { ... }   // real stdout
impl Host for TestHost { ... }  // captures text in memory
```

`&mut dyn Host` (in `Vm`) is "a mutable reference to *some* type implementing
`Host`" — the concrete type is chosen at runtime (dynamic dispatch).

Standard traits implemented in the code:

| Trait | Gives you | Where |
|-------|-----------|-------|
| `fmt::Display` | `format!("{x}")`, `.to_string()` | `Value`, `OpCode`, `TokenKind`, `Expr` |
| `fmt::Debug` | `format!("{x:?}")` | `Instr` (custom), most types (derived) |
| `From<A> for B` | `B::from(a)` and `a.into()` | `Span` ↔ `SrcSpan` in `span.rs` |
| `TryFrom<u8>` | `OpCode::try_from(byte)` → `Result` | `opcode.rs` |
| `Default` | `T::default()` | `Options`, `TestHost`, `SrcSpan` |

**`#[derive(...)]`** asks the compiler to write standard trait
implementations automatically: `Debug`, `Clone`, `Copy`, `PartialEq` (`==`),
`Eq`, `Hash` (usable as a `HashMap` key), `Default`. Libraries add their own
derives: `#[derive(Error)]` (thiserror) in `vm/src/error.rs`,
`#[derive(Parser)]` (clap) in `compiler/src/main.rs`.

`impl Into<String>` as a parameter type means "anything convertible into a
`String`" — so `Diagnostic::new("E0101", "text", span)` and
`Diagnostic::new("E0101", format!(...), span)` both work.

## Closures and iterators

A **closure** is an anonymous function: `|x| x + 1`, `|t| t.span`. It can use
variables from its surroundings:

```rust
let ovf = || overflow(format!("integer overflow: {x} {sym} {y}"));   // vm.rs
x.checked_add(y).ok_or_else(ovf)
```

**Iterators** process sequences lazily with chained adapters, ending in a
consumer:

```rust
err.trace = self.frames.iter().rev()          // vm/src/vm.rs, Vm::unwind
    .map(|f| TraceEntry { ... })
    .collect();                                // build a Vec
```

Adapters used: `iter`, `rev`, `map`, `filter`, `enumerate` (adds an index),
`zip` (pairs two sequences), `chain`, `chars` (characters of a string).
Consumers: `collect`, `count`, `all`, `any`, `max`, `find`.

## Strings and characters

- `String` is owned, growable UTF-8 text; `&str` is a borrowed view of text
  (a "string slice"). Functions usually take `&str`.
- Slicing is by **byte** offset: `&self.src[start..self.pos]`. That is why
  spans store byte offsets (chapter 2). Slicing in the middle of a multi-byte
  character panics, so the lexer always advances by `c.len_utf8()`.
- `char` is one Unicode scalar value (1–4 bytes in UTF-8). `.chars()` iterates
  characters; `.chars().count()` counts them (used for column numbers).
- `format!("{x} and {}", y)` builds a `String`; `{x:<7}` pads to width 7 left
  aligned, `{pc:04}` pads with zeros, `{x:?}` uses `Debug`.

## Integer types and casts

| Type | Range | Used for |
|------|-------|----------|
| `i64` | ±9.2·10¹⁸ | valo `int` values |
| `u8` | 0–255 | opcode, register operands |
| `u16`, `i16` | 0–65535, ±32767 | `Bx`, `sBx` operands, constant indices |
| `u32` | 0–4·10⁹ | instructions, byte offsets in spans |
| `usize` | pointer-sized | indices into `Vec`s and strings |

- `x as u32` casts between integer types (may truncate silently — used only
  where the range is known).
- `u16::try_from(n)` converts and returns `Err` if it does not fit (used when
  the range is *not* known, e.g. `codegen.rs` `load_int`).
- In debug builds, ordinary `+` panics on overflow. The VM must never panic
  on user input, so it uses `checked_add`, `checked_mul`, … which return
  `None` on overflow (chapter 7).
- `saturating_sub(1)` subtracts but stops at 0.

## Macros

Names ending in `!` are macros — code that generates code at compile time:

- `println!`, `eprintln!` (stderr), `format!` (to a `String`), `write!`/
  `writeln!` (into a buffer), `assert_eq!`, `vec![…]`, `matches!`, `env!`
  (compile-time environment variable), `debug_assert!`.
- valo defines one macro itself: `opcodes! { … }` in `vm/src/opcode.rs`,
  written with `macro_rules!`. From one list of `NAME = number, FORMAT;` lines
  it generates the `OpCode` enum, `OpCode::name`, `OpCode::format` and
  `TryFrom<u8>`, so they cannot get out of sync. You only edit the list.

## Tests

- `#[cfg(test)] mod tests { … }` at the bottom of a file is compiled only by
  `cargo test`. Functions marked `#[test]` are tests; `assert_eq!(a, b)`
  fails the test if `a != b`.
- Files in `compiler/tests/` and `vm/tests/` are *integration tests*: separate
  programs that use only the public API (or run the `valo` binary).
- `insta::assert_snapshot!(text)` compares `text` with a stored file in
  `src/snapshots/`.
- `proptest! { … }` runs a test with hundreds of random inputs.

Chapter 10 explains the testing strategy in detail.

## Doc comments and attributes

- `//` ordinary comment; `///` documents the item below; `//!` documents the
  enclosing file/module. `cargo doc` renders `///` and `//!` as HTML.
  Code blocks inside doc comments are compiled and run as tests ("doctests")
  unless marked ` ```text `.
- `#[...]` is an **attribute** on the item below: `#[derive(..)]`,
  `#[test]`, `#[cfg(test)]`, `#[repr(u8)]`, `#[arg(long)]` (clap).
- `#![...]` (with `!`) applies to the enclosing item.

## Reading compiler errors

Rust's error messages are long but precise. Read the first error first; later
ones are often consequences. The most common ones while editing valo:

| Message | Usually means |
|---------|---------------|
| `non-exhaustive patterns: X not covered` | you added an enum variant; add an arm to that `match` |
| `borrow of moved value` | a value was moved earlier; borrow (`&x`) or `clone()` instead |
| `cannot borrow *self as mutable because it is also borrowed as immutable` | you hold a reference into `self` while calling a `&mut self` method; copy the needed data out first (see `let base = frame.base;` in `vm.rs`) |
| `mismatched types: expected u8, found usize` | add a cast `as u8` or use `try_from` |
| `unused variable` (warning) | remove it or prefix with `_` |
