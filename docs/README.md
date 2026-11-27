# valo — Building a Compiler and VM, One Milestone at a Time

This folder is a small textbook about how the valo compiler and virtual
machine are built. It describes **what exists in the code today** (milestones
M0–M1: integer arithmetic runs end to end), explains the compiler-construction
ideas behind each part, and points to the exact files, types and functions
that implement them.

It is written for a reader who knows programming and some compiler theory
from a course, but **not Rust**. Rust features are explained where they
appear, and collected in [chapter 1](01-rust-primer.md).

## Reading order

| #  | Chapter | What you learn | Main code |
|----|---------|----------------|-----------|
| 0  | [Overview](00-overview.md) | the big picture: pipeline, crates, what M0–M1 can run | `compiler/src/lib.rs`, `vm/src/lib.rs` |
| 1  | [A Rust primer for this codebase](01-rust-primer.md) | every Rust feature used, with examples from the code | — |
| 2  | [Source text and spans](02-source-text-and-spans.md) | how positions are tracked for error messages | `compiler/src/span.rs` |
| 3  | [Lexical analysis](03-lexical-analysis.md) | characters → tokens, indentation → INDENT/DEDENT | `compiler/src/lexer.rs`, `token.rs` |
| 4  | [Syntax analysis](04-syntax-analysis.md) | tokens → syntax tree, Pratt parsing, error recovery | `compiler/src/parser.rs`, `ast.rs` |
| 5  | [The bytecode format](05-bytecode-format.md) | instructions, opcodes, prototypes, disassembly | `vm/src/opcode.rs`, `chunk.rs`, `disasm.rs` |
| 6  | [Code generation](06-code-generation.md) | syntax tree → register bytecode | `compiler/src/codegen.rs` |
| 7  | [The virtual machine](07-virtual-machine.md) | fetch–decode–execute, values, arithmetic semantics, the host | `vm/src/vm.rs`, `value.rs`, `host.rs` |
| 8  | [Errors and diagnostics](08-errors-and-diagnostics.md) | compile errors, runtime errors, stack traces, error codes | `compiler/src/diagnostic.rs`, `vm/src/error.rs` |
| 9  | [The driver and the CLI](09-driver-and-cli.md) | the `valo` command, flags, exit codes | `compiler/src/main.rs` |
| 10 | [Testing](10-testing.md) | unit, snapshot, property and golden tests | `compiler/tests/`, `#[cfg(test)]` modules |
| 11 | [Roadmap: what comes next](11-roadmap.md) | how M2–M10 extend each part | `.memory/plan-mvp.md` |
| 12 | [Editor tooling](12-editor-tooling.md) | TextMate highlighting, the Language Server Protocol, VS Code extension | `tooling/lsp/`, `tooling/vscode/` |
| 13 | [The debugger](13-debugger.md) | how VS Code debuggers work, the Debug Adapter Protocol, what a VM needs to be debuggable | `tooling/dap/`, `vm/src/debug.rs`, `tooling/vscode/extension.js` |

Chapters 2–9 follow the path of a program through the system. If you only
want to change something quickly, use the table below.

## "I want to change…" — where to look

| Task | File → item |
|------|-------------|
| add an operator token | `compiler/src/token.rs` → `TokenKind`; `compiler/src/lexer.rs` → `Lexer::token` |
| add a keyword | `compiler/src/token.rs` → `Keyword`, `Keyword::lookup`, `Keyword::as_str` |
| change operator precedence | `compiler/src/parser.rs` → `Parser::expr` (binding-power table) |
| add a new expression form | `compiler/src/ast.rs` → `ExprKind`; `parser.rs` → `Parser::prefix`/`expr`; `codegen.rs` → `FuncState::expr` |
| add a VM instruction | `vm/src/opcode.rs` → `opcodes! { … }`; `vm/src/vm.rs` → `Vm::execute`; `compiler/src/codegen.rs` |
| change arithmetic rules | `vm/src/vm.rs` → `int_arith`, `arith` |
| add a runtime type | `vm/src/value.rs` → `Value` |
| add or reword an error | compile: search the code string, e.g. `"E0101"`; runtime: `vm/src/error.rs` → `ErrorKind` |
| change how errors look | `compiler/src/diagnostic.rs` → `render`, `render_runtime` |
| add a CLI flag | `compiler/src/main.rs` → `enum Command` |
| add a test program | `compiler/tests/golden/mN/<name>.vl` + `<name>.stdout` |
| highlight new syntax in VS Code | `tooling/vscode/syntaxes/valo.tmLanguage.json` |
| add an editor feature (hover, …) | `tooling/lsp/src/server.rs` → `capabilities`, `handle_request` |
| add a debugger feature (stepping, …) | `tooling/dap/src/session.rs` → `capabilities`, `Session::handle_request`; plan in `tooling/dap/README.md` |

## Conventions in the source code

- The `//!` comment block at the top of every `.rs` file is a **file guide**:
  what the file does, its place in the pipeline, a *where to change things*
  table, and the Rust features it uses.
- `// RUST:` comments explain a Rust language feature where it is used.
- `// EXTEND:` comments mark the exact spot to edit when adding a feature.
  List them all with:

  ```bash
  grep -rn "EXTEND" /home/coder/valo/compiler/src /home/coder/valo/vm/src
  ```

- `cargo doc --workspace --no-deps --open` renders all `///` and `//!`
  comments as browsable HTML.

## Related documents

- [`lang/`](lang/README.md) — the language itself: a [syntax guide](lang/syntax.md)
  with runnable examples and the [formal grammar](lang/grammar.md).
- `.memory/plan-mvp.md` — the implementation plan (milestones M0–M10, design
  decisions D1–D29, error-code catalogue). Written for AI agents: dense.
- `.memory/syntax.md` — every syntax decision made so far.
- `notes/Feasibility Report …md` — the original research and motivation.
