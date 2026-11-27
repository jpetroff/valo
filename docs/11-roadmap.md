# Chapter 11 — Roadmap: How the Next Milestones Extend Each Part

M0–M1 built the complete pipeline for the smallest useful language: integer
arithmetic. Every later milestone widens the same pipeline; no stage is
replaced. This chapter maps each planned milestone onto the files you have
now read. The authoritative plan is `.memory/plan-mvp.md`; syntax is in
`.memory/syntax.md`.

| Milestone | Language feature | Lexer | Parser / AST | Resolve | Codegen | VM |
|-----------|------------------|-------|--------------|---------|---------|----|
| **M2** | `let` bindings, names, indented blocks | — | `Item::Let`, `ExprKind::Name`, blocks from `INDENT…DEDENT` | scopes, locals → registers, globals table | `MOVE`, `GETGLOBAL`, `SETGLOBAL` | globals array |
| **M3** | `bool`, `float`, `== < …`, `and/or/not`, `if/elif/else`, `assert` | — | comparisons and logic in the Pratt table, `if` expressions | — | `TEST` + `JMP` with backpatching, `DIV` | `Value::Bool/Float`, comparisons, jumps |
| **M4** | `fn` definitions, calls, `print`, pipes `\|>` | — | `Item::Fn`, call postfix, type annotations parsed | function scopes, arity checks, pipe rewriting | a stack of `FuncState`s, `CLOSURE`, `CALL` | frames, `RETURN` to caller, native functions in `builtins.rs` |
| **M4b** | keyword and default arguments | — | `name=value` arguments | normalise to positional | `LOADMISSING`, `JMPIFSET` | missing-argument sentinel |
| **M5** | recursion without stack growth | — | — | tail position | `TAILCALL` | frame reuse, stack limits |
| **M6** | lambdas `x => …`, closures, `f(a, _)` | — | lambda syntax | captures (upvalues), `_` rewriting | `GETUPVAL`, `GETSELF` | closures capture by value |
| **M7** | strings `"…{x}…"`, file/stdin I/O, `none`, `Ok/Err`, `?` | string scanner with interpolation | string parts | — | `CONCAT`, `LOADNONE`, `TRY` | `Value::Str`, `Host` gains I/O |
| **M8** | lists, indexing, `map/filter/reduce`, comprehensions | — | list literals, `[i]`, comprehensions | comprehension rewriting | `NEWLIST`, `INDEX` | `Value::List`, natives calling back into the VM |
| **M8b** | tuples, dicts, `.key` | — | dict/tuple literals | — | `NEWTUPLE`, `NEWDICT`, `GETFIELD` | `Value::Dict` |
| **M9** | diagnostics quality | — | multi-error recovery, did-you-mean | — | — | source snippets in runtime errors |
| **M10** | runtime type checks for annotations | — | — | — | `CHECKTYPE` | type checks with precise paths |

After the MVP: bytecode files (`valo build`, `valo-vm`, M11), `match` (M12),
records and unions (M13), commands and the OS layer (M14), regex (M15), test
files (M16), and the effect handler (`--dry-run`, `--record`, `--replay`) built
on `host.rs`.

## Patterns you will reuse

- **Add a token** → `token.rs` + `Lexer::token` (chapter 3).
- **Add syntax** → `ExprKind`/`Item` variant + `Parser::prefix`/`expr`/`item`
  (chapter 4). The Rust compiler then lists every `match` to update.
- **Add an instruction** → `opcodes!` (already declared for the MVP) +
  `Vm::execute` + `FuncState::expr` (chapters 5–7).
- **Add a value type** → `Value` variant + `type_name` + `Display` + `arith`
  (chapter 7).
- **Add an error** → a code, a message, a golden test (chapters 8, 10).
- **Every feature** → a golden test written *before* the implementation,
  with hand-written expected output.

## The open questions that block upcoming work

`.memory/plan-mvp.md` §7 lists open design questions with default answers.
The ones relevant to the next milestones: Q1 (int overflow: checked — already
implemented), Q6 (program entry: script-style top level), Q17 (pipe
evaluation order), Q19–Q23 (details of `?` and errors). Defaults are used
unless you decide otherwise.
