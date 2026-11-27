# Chapter 8 — Errors and Diagnostics

## 8.1 Theory: two kinds of errors

- **Static (compile-time) errors** are found before the program runs: bad
  characters, bad grammar, later undefined names and type mismatches. The
  compiler should report *all* of them in one run, each pointing at the
  source.
- **Dynamic (runtime) errors** happen while the program runs: overflow,
  division by zero, later missing files or bad JSON. They must say *where* the
  program was — including which functions called which — because the input
  that caused them is often gone.

Good diagnostics are a core goal of valo: scripts run on remote machines
where a debugger is rarely available, so the error message *is* the debugger.

## 8.2 Error codes

Every error has a stable code, so tests, documentation and users can refer to
it. Codes never change once a golden test uses them.

| Range | Stage | Codes in use (M3) |
|-------|-------|-------------------|
| E00xx | lexer | E0001 unexpected character · E0002 int literal out of range · E0003 bad dedent · E0006 tab in indentation · E0009 not supported yet · E0010 malformed number |
| E01xx | parser | E0101 unexpected token · E0102 `if` without `else` · E0103 unclosed `(` · E0104 block ends with a `let` · E0105 nested too deeply |
| E02xx | resolve | E0201 undefined name (+ did-you-mean) · E0203 top-level name used before its definition · E0206 `_` used as a value |
| E03xx | codegen | E0301 too many registers / constants |
| R01xx | arithmetic | R0101 integer overflow · R0102 division/modulo by zero · R0104 negative exponent |
| R02xx | types | R0201 operand type mismatch (incl. int/float mixing) · R0203 condition not a bool · R0205 values cannot be ordered |
| R03xx | resources | R0302 global read before it was set (unreachable from `<main>`; from M4 a function can run before a binding it reads) |
| R07xx | assertions | R0701 assertion failed |
| R05xx | host | R0501 output write failed |
| R09xx | internal | R0901 bad bytecode / unimplemented opcode |

The full catalogue, including codes reserved for later milestones, is in
`.memory/plan-mvp.md` §5. To find where an error is produced, search for its
code: `grep -rn '"E0101"' compiler/src`.

## 8.3 Compile-time diagnostics

File: [`compiler/src/diagnostic.rs`](../compiler/src/diagnostic.rs)

```rust
pub struct Diagnostic {
    pub code: &'static str,              // "E0101"
    pub message: String,                 // one line
    pub primary: Span,                   // what to underline
    pub label: String,                   // text under the underline
    pub secondary: Vec<(Span, String)>,  // more underlined places
    pub help: Vec<String>,               // "Help: ..." lines
}
```

Stages build them with a small builder API:

```rust
Diagnostic::new("E0101", format!("expected `)`, found {}", ...), close.span)
    .label("expected `)` here")
    .with(open_paren_span, "to close this `(`")
```

### Collecting instead of stopping

No stage stops at its first error. The lexer and code generator push to a
`Vec<Diagnostic>` and continue; the parser recovers at the next line
(chapter 4). The pipeline (`compile_source` in `lib.rs`) runs stage by stage
and stops after the first stage that produced errors — later stages would
only report consequences of the earlier ones.

### Rendering

`render(sm, diags, color)` turns diagnostics into text with the `ariadne`
crate: the file name and position, the source line, an underline and the
label:

```text
[E0101] Error: expected an expression, found end of line
   ╭─[ parse_error.vl:2:4 ]
   │
 2 │ 3 *
   │    ┬
   │    ╰── expected an expression here
───╯
```

Details:

- `IndexType::Byte` tells ariadne the spans are byte offsets (chapter 2).
- Spans are clamped into the text (`clamp`) so an end-of-file span never
  makes ariadne panic.
- A label without text would draw no marker in ariadne, so `render` uses
  "here" as a default.
- Colours are on only if stderr is a terminal and `NO_COLOR` is unset
  (`stderr_color`), so golden test files contain no colour codes.

## 8.4 Runtime errors

File: [`vm/src/error.rs`](../vm/src/error.rs)

```rust
pub enum ErrorKind { IntOverflow, DivisionByZero, NegativeExponent,
                     TypeMismatch, ConditionNotBool, NotComparable,
                     GlobalUninit, AssertionFailed, HostIo, Internal } // .code() → "R0101", ...

pub struct RuntimeError {
    pub kind: ErrorKind,
    pub message: String,              // "division by zero: 7 // 0"
    pub notes: Vec<String>,           // extra lines: "left: 4", "help: …"
    pub trace: Vec<TraceEntry>,       // innermost frame first
}
pub struct TraceEntry { pub fn_name: Rc<str>, pub span: SrcSpan }
```

The VM creates the error at the failing instruction (`RuntimeError::new`);
`Vm::unwind` adds the trace (chapter 7). Messages include the operand values
(`9223372036854775807 + 1`), because at runtime the values are what the
reader cannot see in the source.

`render_runtime(sm, err)` in `diagnostic.rs` prints:

```text
error[R0101]: integer overflow: 9223372036854775807 + 1
  --> overflow.vl:1:1
stack trace (most recent call first):
  0: <main> at overflow.vl:1:1
```

Notes follow the location line. A failed `assert` also quotes the asserted
expression — the VM has no source text, but the span of the `ASSERT`
instruction is that expression, so `render_runtime` slices it from the file:

```text
error[R0701]: assertion failed: a + a == 5
  --> assert_cmp_fail.vl:2:8
  left: 4
  right: 5
stack trace (most recent call first):
  0: <main> at assert_cmp_fail.vl:2:8
```

M9 adds the source snippet (like compile errors) and unifies both header
styles: compile errors currently use ariadne's `[E0101] Error:` header,
runtime errors `error[R0101]:`.

## 8.5 Exit codes

| Code | Meaning | Where |
|------|---------|-------|
| 0 | success | |
| 1 | runtime error | `main.rs` `run` |
| 2 | compile error | `main.rs` `report` |
| 64 | usage error or unreadable file (BSD `EX_USAGE`) | `main.rs` `load`, `main` |

Distinct codes let shell scripts, cron and systemd tell "the script is
wrong" (2) from "the script failed while running" (1).

## 8.6 Where to change things

| Task | Where |
|------|-------|
| new compile error | `Diagnostic::new("E0xxx", msg, span)` in the stage; add the code to `.memory/plan-mvp.md` §5 and a golden test |
| new runtime error | `ErrorKind` variant + `code()` in `vm/src/error.rs`; raise with `RuntimeError::new` in `vm.rs` |
| reword a message | search for its code |
| compile error layout | `render` |
| runtime error layout | `render_runtime` |

## 8.7 Exercises

1. Make `1 $ 2` report a label "remove this character" instead of "here"
   (`lexer.rs`, the E0001 arm: build the `Diagnostic` with `.label(...)`).
   Which golden file changes? Re-bless it (chapter 10) and review the diff.
2. Why does the lexer stop the pipeline before the parser runs when it found
   errors, instead of letting the parser add its own?

## 8.8 Further reading

- Esteban Küber et al., the rustc diagnostics guide ("rustc-dev-guide",
  chapter *Errors and Lints*) — style rules for compiler messages.
- Elm's compiler messages (Evan Czaplicki, "Compiler Errors for Humans", 2015).
