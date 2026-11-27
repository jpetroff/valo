# Chapter 0 — Overview

## 0.1 What valo is

valo is a small functional scripting language for automating a home lab:
processing text and JSON, running commands, talking to the network. Its
design borrows from Python (indentation, operators, type annotations),
Haskell (immutability, `->` types, pattern matching) and Rust (`Result` and
`?`). Its *implementation* follows **Lua 5.x**: a compiler that turns source
code into compact bytecode, and a register-based virtual machine that runs it.

The full design lives in `.memory/syntax.md` (syntax) and `.memory/plan-mvp.md`
(implementation plan). This textbook covers what has been **built** so far.

## 0.2 What works today (milestones M0 and M1)

A valo program is currently a sequence of integer expressions, one per line:

```text
# precedence and Python-style arithmetic
(2 + 3) * 4 - 10 // 3
-7 // 2
2 ** 10
```

```bash
cargo run -q --bin valo -- run demo.vl --echo
```

prints `17`, `-4`, `1024`. Supported:

- integer literals: `42`, `1_000`, `0xff`, `0o755`, `0b1010`;
- operators `+ - * // % **` and unary `-`, with Python's precedence and
  Python's rounding for `//` and `%`;
- parentheses, `#` comments, blank lines, line joining inside brackets;
- precise compile errors (with source snippets) and runtime errors (overflow,
  division by zero, negative exponent) with a stack trace;
- debugging views of every stage: `--dump tokens`, `--dump ast`,
  `--dump bytecode`, `--trace-exec`.

Everything else in the language (names, functions, strings, lists, …) is
recognised by the lexer but rejected with error `E0009 … is not supported
yet`. Chapter 11 lists what comes next.

## 0.3 The pipeline

A compiler is a chain of translations. Each stage takes one representation of
the program and produces a more machine-friendly one:

```text
                ┌─────────────────── compiler crate (valo-compiler) ───────────────────┐
 "(1+2)*3\n" ──▶│ lexer ──tokens──▶ parser ──AST──▶ resolve ──▶ codegen ──Program──────│──▶ VM ──▶ output
                │ ch. 3            ch. 4             (M2+)       ch. 6                  │    ch. 7
                └──────────── errors at any stage: Diagnostic (ch. 8) ─────────────────┘
```

The same example through every stage (real output of the tools):

**Source** — `ex.vl`:

```text
(1 + 2) * 3
-2 ** 2
```

**Tokens** — `valo check ex.vl --dump tokens` (byte range, then token):

```text
   0..1    (
   1..2    1
   3..4    +
   5..6    2
   6..7    )
   8..9    *
  10..11   3
  11..12   NEWLINE
  12..13   -
  13..14   2
  15..17   **
  18..19   2
  19..20   NEWLINE
  20..20   EOF
```

**Syntax tree** — `--dump ast` (S-expressions: operator first):

```text
(expr (* (+ 1 2) 3))
(expr (neg (** 2 2)))
```

Note how the tree makes precedence explicit: `-2 ** 2` is `-(2 ** 2)`.

**Bytecode** — `--dump bytecode`:

```text
function <main> arity=0 regs=2 consts=0   ; ex.vl:1:1
  0000  [1:2]   LOADI     0 1
  0001  [1:6]   LOADI     1 2
  0002  [1:1]   ADD       0 0 1
  0003  [1:11]  LOADI     1 3
  0004  [1:1]   MUL       0 0 1
  0005  [2:2]   LOADI     0 2
  0006  [2:7]   LOADI     1 2
  0007  [2:2]   POW       0 0 1
  0008  [2:1]   NEG       0 0
  0009  [3:1]   LOADUNIT  0
  0010  [3:1]   RETURN    0
constants: (none)
```

**Execution** — `valo run ex.vl --echo --trace-exec` (trace on stderr):

```text
[<main>] 0000 LOADI 0 1 | R[0]=1
[<main>] 0001 LOADI 1 2 | R[1]=2
[<main>] 0002 ADD 0 0 1 | R[0]=3
[<main>] 0003 LOADI 1 3 | R[1]=3
[<main>] 0004 MUL 0 0 1 | R[0]=9
9
...
```

## 0.4 Repository layout

```text
valo/
├── Cargo.toml              workspace: lists the two crates and shared dependency versions
├── compiler/               crate valo-compiler (library valo_compiler + binary `valo`)
│   ├── src/
│   │   ├── lib.rs          pipeline functions parse_source, compile_source
│   │   ├── main.rs         the `valo` command-line tool
│   │   ├── span.rs         Span, SourceMap                        (ch. 2)
│   │   ├── token.rs        TokenKind, Keyword                     (ch. 3)
│   │   ├── lexer.rs        lex()                                  (ch. 3)
│   │   ├── ast.rs          Program, Item, Expr, ExprKind          (ch. 4)
│   │   ├── parser.rs       parse()                                (ch. 4)
│   │   ├── resolve.rs      name resolution (empty until M2)
│   │   ├── codegen.rs      compile()                              (ch. 6)
│   │   ├── diagnostic.rs   Diagnostic, render, render_runtime     (ch. 8)
│   │   └── snapshots/      expected outputs of snapshot tests     (ch. 10)
│   └── tests/              golden.rs + golden/ programs, cli.rs   (ch. 10)
├── vm/                     crate valo-vm (library valo_vm + binary `valo-vm`)
│   └── src/
│       ├── lib.rs
│       ├── opcode.rs       OpCode, Instr                          (ch. 5)
│       ├── chunk.rs        Proto, Program, Const, SrcSpan         (ch. 5)
│       ├── disasm.rs       disassemble()                          (ch. 5)
│       ├── value.rs        Value                                  (ch. 7)
│       ├── vm.rs           Vm, int_arith                          (ch. 7)
│       ├── host.rs         Host, StdHost, TestHost                (ch. 7)
│       ├── error.rs        RuntimeError, ErrorKind                (ch. 8)
│       ├── builtins.rs     native functions (empty until M4)
│       └── bin/valo-vm.rs  standalone runner (placeholder)
├── docs/                   this textbook
├── .memory/                plan-mvp.md, syntax.md (design documents)
└── notes/                  feasibility report
```

## 0.5 Why two crates?

A *crate* is a Rust package (chapter 1). The VM crate does **not** depend on
the compiler crate; the dependency only goes the other way
(`compiler/Cargo.toml` lists `valo-vm`). This mirrors Lua, where the
interpreter can run precompiled bytecode without containing the parser. It
keeps a future VM-only build small — for WebAssembly, or for copying a single
runner binary to a server together with precompiled `.vlc` files (milestone
M11).

The *bytecode format* therefore lives in the VM crate (`opcode.rs`,
`chunk.rs`): the compiler imports it to produce bytecode, the VM uses it to
execute bytecode.

## 0.6 Design choices at a glance

| Choice | Alternative | Why (plan decision) |
|--------|-------------|---------------------|
| hand-written lexer | generator (`logos`) | indentation tokens need custom logic; better errors; learning value |
| hand-written Pratt parser | parser generator (`lalrpop`, `pest`) | best error messages, easy precedence tables |
| AST → bytecode directly | intermediate representation (ANF/SSA) | simpler; no optimiser yet (D1) |
| register VM (Lua 5) | stack VM (Lua 4, Python, JVM) | fewer instructions, fewer copies |
| 32-bit fixed-width instructions | variable-length bytes | simple decoding, Lua-like (D2) |
| checked integer arithmetic | wrapping (C, release Rust) | silent overflow is a bug source (D8) |
| Python `//` and `%` semantics | C/Rust truncation | familiar to the target users (D9) |

## 0.7 Further reading

- Robert Nystrom, *Crafting Interpreters* — chapters 14–17 (bytecode, scanning,
  compiling expressions) map directly onto chapters 3–7 here.
- R. Ierusalimschy, L. H. de Figueiredo, W. Celes, *The Implementation of Lua
  5.0* (2005) — register VM design.
- Aho, Lam, Sethi, Ullman, *Compilers: Principles, Techniques, and Tools*
  ("the Dragon Book") — chapters 1–3 for the classical theory.
