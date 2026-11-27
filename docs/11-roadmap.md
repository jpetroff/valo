# Chapter 11 — Roadmap: How the Next Milestones Extend Each Part

M0–M1 built the complete pipeline for the smallest useful language: integer
arithmetic. M2–M3 added `let` bindings and blocks (the resolver, globals,
locals in registers), bools, floats, comparisons, `and`/`or`/`not`, `if` and
`assert` (jumps with backpatching). M4 added functions: `fn` definitions,
calls with frames, built-ins and the pipe (a desugaring pass); M4b keyword
arguments and defaults; M5 proper tail calls; M6 lambdas, local fns,
closures that capture by value and `_` partials. Every later
milestone widens the same pipeline; no stage is replaced. This chapter maps each planned milestone onto the files you have
now read. The authoritative plan is `.memory/plan-mvp.md`; syntax is in
`.memory/syntax.md`.

| Milestone | Language feature | Lexer | Parser / AST | Resolve | Codegen | VM |
|-----------|------------------|-------|--------------|---------|---------|----|
| ~~M2~~ (done) | `let` bindings, names, indented blocks | — | `Item::Let`, `ExprKind::Name`, blocks from `INDENT…DEDENT` | scopes, locals → registers, globals table | `MOVE`, `GETGLOBAL`, `SETGLOBAL` | globals array |
| ~~M3~~ (done) | `bool`, `float`, `== < …`, `and/or/not`, `if/elif/else`, `assert` | — | comparisons and logic in the Pratt table, `if` expressions | — | `TEST` + `JMP` with backpatching, `DIV` | `Value::Bool/Float`, comparisons, jumps |
| ~~M4~~ (done) | `fn` definitions, calls, `print`, pipes `\|>` | — | `Item::Fn`, call postfix, type annotations parsed | function scopes, arity checks, pipe rewriting (`desugar.rs`) | a stack of `FuncState`s, `CLOSURE`, `CALL` | frames, `RETURN` to caller, native functions in `builtins.rs` |
| ~~M4b~~ (done) | keyword and default arguments | — | `name=value` arguments | normalise to positional | `LOADMISSING`, `JMPIFSET` | missing-argument sentinel |
| ~~M5~~ (done) | recursion without stack growth | — | — | tail position | `TAILCALL` | frame reuse, stack limits |
| ~~M6~~ (done) | lambdas `x => …`, closures, local `fn`s, `f(a, _)` | — | `ExprKind::Lambda`, `paren_lambda_ahead` | function stack, captures (`Res::Upvalue`), `_` rewriting in `desugar.rs` | `compile_function`, `FuncState::closure`, `GETUPVAL`, `GETSELF` | `CLOSURE` copies captures (`Proto::upvals`) |
| **M7** | strings `"…{x}…"`, file/stdin I/O, `none`, `Ok/Err`, `?` | string scanner with interpolation | string parts | — | `CONCAT`, `LOADNONE`, `TRY` | `Value::Str`, `Host` gains I/O |
| **M8** | lists, indexing, `map/filter/reduce`, comprehensions | — | list literals, `[i]`, comprehensions | comprehension rewriting | `NEWLIST`, `INDEX` | `Value::List`, natives calling back into the VM |
| **M8b** | tuples, dicts, `.key` | — | dict/tuple literals | — | `NEWTUPLE`, `NEWDICT`, `GETFIELD` | `Value::Dict` |
| **M9** | diagnostics quality | — | multi-error recovery, did-you-mean | — | — | source snippets in runtime errors |
| **M10** | runtime type checks for annotations | — | — | — | `CHECKTYPE` | type checks with precise paths |

After the MVP: bytecode files (`valo build`, `valo-vm`, M11), `match` (M12),
records and unions (M13), commands and the OS layer (M14), regex (M15), test
files (M16), and the effect handler (`--dry-run`, `--record`, `--replay`) built
on `host.rs`.

Editor tooling grows alongside: each milestone also updates the TextMate
grammar and, where it adds names or scopes, the language server. The plan per
milestone is in `tooling/lsp/README.md`; how it works is chapter 12. The
debugger (`tooling/dap`, chapter 13) follows too: new values need a way to be
shown in the Variables view, new scopes (closures, M6) a scope of their own;
its checklist is in `tooling/dap/README.md`.

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
- **New syntax** → a rule in `tooling/vscode/syntaxes/valo.tmLanguage.json`
  and a line in `tooling/vscode/samples/showcase.vl` (chapter 12).

## The open questions that block upcoming work

`.memory/plan-mvp.md` §7 lists open design questions with default answers.
The ones relevant to the next milestones: Q1 (int overflow: checked — already
implemented), Q6 (program entry: script-style top level), Q17 (pipe
evaluation order), Q19–Q23 (details of `?` and errors). Defaults are used
unless you decide otherwise.
