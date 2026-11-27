# Chapter 2 — Source Text and Spans

## 2.1 The problem

Every message a compiler prints — "unexpected character", "division by
zero" — must say *where*. The location has to survive the whole pipeline:
characters become tokens, tokens become tree nodes, nodes become
instructions, and at runtime an instruction fails. If any stage forgets
where its input came from, the error message can only say "something went
wrong somewhere".

The standard solution is a **span**: a pair of positions attached to every
token, every tree node and every instruction.

## 2.2 Byte offsets, not line:column

A span could store line and column numbers directly, but valo (like rustc,
Roslyn and most modern compilers) stores **byte offsets** into the source
text, and computes line and column only when an error is actually printed:

- an offset is one integer, cheap to copy and compare;
- merging spans is trivial (`min` of starts, `max` of ends);
- slicing the source text by span is direct (`&text[start..end]`);
- the line index is computed once per file, and only used for errors.

Spans are **half-open**: `[start, end)` includes `start` and excludes `end`.
The token `**` at offsets 15 and 16 has span `15..17`; its length is
`end - start`. An empty span (`20..20`) marks a position, e.g. end of file.

## 2.3 In valo

File: [`compiler/src/span.rs`](../compiler/src/span.rs)

```rust
pub struct Span { pub start: u32, pub end: u32 }

impl Span {
    pub fn new(start: usize, end: usize) -> Span
    pub fn to(self, other: Span) -> Span      // smallest span covering both
    pub fn range(self) -> Range<usize>        // for slicing: &text[span.range()]
}

pub struct SourceMap {
    pub name: String,            // file name for messages, e.g. "demo.vl"
    pub text: String,            // the whole source
    pub line_starts: Vec<u32>,   // byte offset where each line begins
}

impl SourceMap {
    pub fn new(name, text) -> SourceMap
    pub fn line_col(&self, offset: u32) -> (u32, u32)   // 1-based line and column
    pub fn slice(&self, span: Span) -> &str
}
```

`u32` offsets limit a source file to 4 GiB, and halve the memory of `usize`
on 64-bit machines.

### The line index

`line_starts` is computed by `valo_vm::chunk::line_starts` (it lives in the VM
crate because bytecode also stores it, see below). For the text
`"ab\ncd\n\nx"` it is `[0, 3, 6, 7]`: line 1 starts at byte 0, line 2 at 3
(after the first `\n`), and so on.

To convert an offset to a line, `line_col` uses **binary search** on this
sorted table — O(log n) per lookup:

- `binary_search(&offset)` returns `Ok(i)` if `offset` is exactly a line
  start (line `i + 1`), or `Err(i)` if it would be inserted at `i`, so it lies
  on line `i` (index `i - 1`).
- The column counts **characters**, not bytes, from the line start
  (`.chars().count()`), so `ä` (2 bytes in UTF-8) is one column.

### Spans in bytecode: SrcSpan

The VM crate must not depend on the compiler, so it has its own identical
type, `SrcSpan` ([`vm/src/chunk.rs`](../vm/src/chunk.rs)). `span.rs`
implements `From<Span> for SrcSpan` and back, so code generation can write
`SrcSpan::from(span)` or `span.into()`.

Every compiled function (`Proto`) keeps a `spans: Vec<SrcSpan>` parallel to
its `code`: `spans[pc]` is the source of instruction `code[pc]`. The compiled
`Program` also carries `line_starts` and the file name. That is how a runtime
error knows its line — and how `valo-vm` will report positions in the future
even without the source text (then with byte columns, `Program::line_col`).

## 2.4 How spans flow through the pipeline

```text
lexer      Token { kind, span }                 span of the characters
parser     Expr  { id, span, kind }             span covering all its tokens (lhs.span.to(rhs.span))
codegen    emit(instr, e.span) → Proto.spans    span of the node that produced the instruction
vm         error at pc → Proto.spans[pc]        → TraceEntry.span
cli        SourceMap::line_col(span.start)      → "demo.vl:2:1"
```

Example: in `7 // (2 - 2)` the `IDIV` instruction gets the span of the whole
expression, so the error is reported at its first character:

```text
error[R0102]: division by zero: 7 // 0
   ╭─[ div_zero.vl:2:1 ]
   │
 2 │ 7 // (2 - 2)
   │ ──────┬─────
   │       ╰─────── divisor is zero
───╯
```

## 2.5 Where to change things

| Task | Where |
|------|-------|
| columns count tabs as 4 | `SourceMap::line_col` |
| point runtime errors at the operator instead of the expression | `codegen.rs` `FuncState::expr`: pass `*op_span` instead of `e.span` to `emit` for the arithmetic instruction (the AST already stores `op_span`) |
| several files (imports, post-MVP) | add a file id to `Span` and a list of `SourceMap`s |

## 2.6 Exercises

1. What is `line_col` for offset 6 in `"ab\ncd\n\nx"`? (Answer: `(3, 1)`: the
   empty third line.) Check with the unit test in `span.rs`.
2. Why does `line_col` clamp `offset` to `text.len()` first? (Hint: the EOF
   token has span `len..len`, and an empty file has `len = 0`.)

## 2.7 Further reading

- *Crafting Interpreters*, ch. 14.6 "Line information" — the run-length
  alternative to a per-instruction table.
- rustc's `rustc_span` crate — the industrial version of the same idea.
