# Feasibility Report: A Functional Scripting Language in Rust

Nov 24, 2026 · @Eugene

## Executive summary

**Verdict: feasible and a very good compiler-course project, but only if you cut the scope hard and change several assumptions.** A Rust bytecode compiler plus VM for a small, strict, immutable, expression-oriented scripting language with batteries for text, JSON, processes and HTTP is realistic for one student in 2 to 3 semesters of part-time work. A language that combines all of Perl, Haskell, Prolog and PHP is not; that is the main risk.

The five corrections that matter most:

1. **Make it strict by default, not lazy like Haskell.** Laziness hurts predictability and debugging, which are your top goals. Offer laziness only for streams and an explicit `lazy` form.
2. **Use Datalog-style rules, not full Prolog.** Datalog always terminates and is easy to explain; Prolog's backtracking and cut are hard to debug on a remote box.
3. **Errors-as-values and effects-as-data buy robustness, not "functional" by itself.** The research on language choice and bug rates finds only small effects. What fixes bash's failure modes is typed results, no word-splitting, argv-safe commands, and a dry-run mode.
4. **PHP is the wrong model for bytecode files.** PHP's opcache is a per-version in-memory cache. Copy Python's `.pyc` or Lua's precompiled chunks instead: a versioned, cached, optional file.
5. **WebAssembly runs the core, not the OS features.** Spawning processes and raw sockets do not exist in the browser and are limited in WASI. Design OS access as pluggable host modules so the same VM runs in WASM with them switched off.

The idea's strongest, most original angle is the combination you would get for free from an immutable design: **every OS effect can be recorded, mocked, replayed and printed as a plan** (`--dry-run`, `--trace`, `--replay`). That directly serves remote homelab debugging and is something bash and Python cannot offer cheaply. Build the language around that.

What already exists and overlaps (study, do not fear): Nushell, Oils/YSH, Babashka, Nickel, Flix, Koto, Rhai, Starlark and jq/jaq. None combines a functional core, Datalog rules and recordable effects for ops scripting.

## Fact-check: your references and assumptions

Most assumptions hold; four need correcting and two need re-framing. The table gives the verdict, the notes below give the detail.

| Assumption | Verdict | Correction |
| --- | --- | --- |
| Perl was designed around language processing | Mostly true | Perl (1987) was designed for text extraction and reports. Larry Wall's *linguistics* background shaped its feel, not a focus on parsing languages. Copy its text ergonomics, not its context-sensitivity. |
| Haskell's best features: monads, "partially calculated functions", immutability | True, with a term fix | The term is *partial application* (currying). A *partial function* in Haskell means one undefined for some inputs, like `head []`, which you want to avoid. |
| Scala gives rule-based and lazy evaluation | Partly wrong | Scala is not rule-based. It contributes `lazy val`, `LazyList`, pattern matching, case classes and `for`-comprehensions. Rules come from Prolog. |
| Prolog-style rule evaluation fits scripting | Re-frame | Use Datalog (a terminating subset of Prolog). Flix shows Datalog can live inside a functional language as first-class values. |
| Lazy evaluation is desirable | Re-frame | Lazy-by-default causes space leaks and unpredictable timing of side effects. Make the language strict; add lazy streams. |
| PHP is bytecode-compiled but interpreted | True | PHP compiles to Zend opcodes on each request and OPcache caches them in shared memory. Since PHP 8.5, OPcache is always built in. But PHP is a poor model for *shipping* bytecode files. |
| Bytecode can be produced on the fly or as a static file | True | Python `.pyc`, Lua `luac` and Java `.class` all do this. Version the format and never load untrusted bytecode. |
| Rust is a reasonable basis for the compiler and VM | True | Many interpreters prove it. Known pain points: garbage collection with cycles, slow builds, and the borrow checker while learning. |
| Rust crates mean you don't write everything | True | Regex, JSON, Markdown, HTTP, SSH, parsing, diagnostics and REPLs all exist as mature crates. Check maintenance status. |
| A Rust VM runs anywhere including WebAssembly | Half true | The core VM compiles to WASM easily. Process spawning does not exist in WASI or the browser; sockets differ. OS features must be optional host modules. |
| Functional/declarative scripts are less error-prone | Weakly supported | Large studies find only small language effects on bug rates. Robustness comes from specific features: typed errors, no implicit splitting, idempotent operations, dry-run. |

### Notes

**Perl.** Wall studied linguistics at Berkeley and wrote an essay, *Natural Language Principles in Perl*: learn once, use many times; ambiguity is fine if quickly resolved; punctuation carries meaning. The features worth borrowing are concrete: regex literals as operators (`=~`, `s///`), string interpolation, here-docs, named captures, and the one-liner flags `-n`, `-p`, `-i`, `-e` that turn a script into a stream filter. The features to avoid are scalar-vs-list context, implicit `$_` globals and "there's more than one way to do it"; they contradict your error-proof goal. For real grammar-level language processing, look at Perl's successor Raku, whose `grammar` blocks make parsers first-class.

**Haskell.** Keep: immutability, algebraic data types, pattern matching, currying and partial application, pure functions, and a `do`-like notation for sequencing fallible work. Reconsider: laziness by default. Simon Peyton Jones's 2003 retrospective *Wearing the hair shirt* concluded purity matters more than laziness and is independent of it. Strict-but-pure designs (PureScript, Idris, Koka, Roc, Elm, Unison) are the modern consensus. Monads themselves need type classes to be ergonomic; in a first version, offer `Result`/`Option` chaining syntax and defer general monads.

**Scala.** What it really offers your design is the *syntax* of functional programming in a practical language: `for`-comprehensions as monad sugar, exhaustive `match`, case classes as records, `LazyList`. Its scripting story is also instructive: Li Haoyi's `os-lib` and the Ammonite/scala-cli tooling were built to replace bash scripts with typed Scala.

**Prolog vs Datalog.** Prolog gives unification, backtracking and arbitrary recursion; programs can loop forever and `cut` makes control flow hard to reason about. Datalog drops function symbols and evaluates bottom-up, so every query terminates and the answer is a set of facts you can print. That is exactly what you want for questions like "which services must restart given these failed units and dependencies". See Flix for a language that embeds it.

**PHP.** The Zend engine compiles each script to opcode arrays at request time; OPcache stores them in shared memory to skip recompilation. The PHP 8.5 RFC made OPcache non-optional (statically compiled in; still switchable by `opcache.enable`). The file cache exists but is tied to the exact PHP build. Better models for "compile ahead, ship a file": Python `.pyc` (magic number + source hash, auto-recompiled if stale), Lua precompiled chunks, Erlang `.beam`, Java `.class`. Lesson from Lua: after exploits, Lua 5.2 removed its bytecode verifier, so loading untrusted bytecode can escape the sandbox. Treat bytecode as a cache, sign it, or verify it.

**Rust.** A sound choice: Gleam, Nushell, Nickel, Starlark-rust, Rhai, Rune, Koto, Boa (JavaScript) and RustPython are all written in it. One counter-example to know: the Roc team rewrote its 300k-line Rust compiler in Zig, citing slow builds and little benefit from the borrow checker for a compiler's simple lifetimes. That does not apply to a student-scale project, but it shows that arenas and indices, not references, are the idiomatic way to write compilers in Rust.

**WebAssembly.** WASI 0.3 (June 2026) added native async to the component model, and WASI 1.0 is targeted for late 2026 or early 2027. Files, clocks, randomness and HTTP are covered; spawning processes is not, and Rust's `std::process` returns errors on WASI targets. So: core language and text/JSON/Markdown libraries run everywhere; `proc`, `ssh` and raw sockets are native-only. A browser playground is still very valuable for teaching and for testing pure logic.

**"Functional = fewer bugs."** The well-known GitHub study (Ray et al., FSE 2014) found small associations between language and defects; the TOPLAS 2019 reproduction by Berger et al. found most of those disappear after correcting the statistics. Argue for your language on specific mechanisms instead. Bash scripts fail through unquoted variables and word splitting, ignored exit codes, `set -e` gaps, partial failure halfway through, and non-idempotent steps. Each of those has a concrete language-level fix (see the design section).

## Prior art to study

No existing tool combines a pure functional core, Datalog rules and recordable effects for ops scripting, so the niche is real; but each piece exists somewhere. Read these before designing syntax, and steal freely.

| Project | Written in | What it is | What to steal |
| --- | --- | --- | --- |
| [Nushell](https://www.nushell.sh/) | Rust | Shell where pipelines carry tables and records, not text; v0.115 (Aug 2026), parses Markdown with `from md` | Structured pipelines, `from json`/`to json`, error messages with source spans, its crate list |
| [Oils / YSH](https://oils.pub/) | Python→C++ | Upgrade path from bash: OSH runs bash, YSH is a cleaner language | Catalogue of bash pitfalls and how YSH fixes each (errexit, word splitting) |
| [Babashka](https://babashka.org/) | Clojure (GraalVM) | Functional, immutable scripting runtime, about 25 ms startup | Proof that a functional bash replacement works; `babashka.process`, `babashka.fs` APIs |
| [Nickel](https://github.com/nickel-lang/nickel) | Rust | Lazy functional configuration language with contracts, 1.0 in 2023 | Contracts as runtime-checked schemas for JSON; a real Rust codebase for a functional language |
| [Flix](https://flix.dev/) | Scala (JVM) | Functional + logic language with first-class Datalog and an effect system | How to embed Datalog in a typed functional language; purity tracking |
| [Koka](https://koka-lang.github.io/) | Haskell→C | Research language with algebraic effects and Perceus reference counting | Effect handlers; reference counting without a tracing GC |
| [Starlark](https://github.com/facebook/starlark-rust) | Rust (Meta) | Deterministic, hermetic Python dialect for build config | Determinism and sandboxing as language guarantees |
| [jaq](https://github.com/01mf02/jaq) / jq | Rust / C | JSON query languages | Path expressions and filters over unknown JSON |
| [Koto](https://github.com/koto-lang/koto), [Rune](https://github.com/rune-rs/rune), [Rhai](https://rhai.rs/) | Rust | Embeddable scripting languages with bytecode VMs | Readable VM and compiler code at your target scale |
| [Steel](https://github.com/mattwparas/steel) | Rust | Embeddable Scheme | Functional VM, tail calls, contracts in Rust |
| [Raku](https://raku.org/) | NQP/MoarVM | Perl's successor | First-class grammars for language processing |
| Scala `os-lib` / scala-cli | Scala | Libraries for typed shell-style scripting | API shape for processes and paths |
| Dhall, CUE, Pkl | Haskell, Go, Java | Typed configuration languages | Totality (Dhall), unification of data and schema (CUE) |

Also worth a glance: Elvish and Murex (structured shells), Amber (a language that compiles to bash), Google's `zx` (JavaScript scripting), Unison (content-addressed code), and Gleam (a friendly typed functional language whose compiler is in Rust).

Honest alternative: if the only goal were a better homelab tool, Nushell or Babashka today would cover most of your day-to-day needs. The case for building your own is the course, the learning, and the effects-as-data angle none of them offer.

## Pros and cons

The pros are strongest as a learning project and as a personal tool; the cons concentrate on scope, adoption and the cost of learning Rust and compilers at once.

|  | Pros | Cons |
| --- | --- | --- |
| Learning | Covers every compiler-course topic end to end: lexing, parsing, scoping, type inference, code generation, a VM, GC | Learning Rust, language design and compiler construction simultaneously triples the early difficulty |
| Fit to purpose | Immutable data and pure functions make scripts testable and replayable; Result types stop silent failures | Shell one-liners are terse because they are untyped and imperative; a functional language can feel heavier for 3-line jobs |
| Text and AI work | Regex literals, PEG grammars, JSON and Markdown as built-in values are a genuine improvement over bash + jq + sed | Python already has excellent libraries for AI tasks (tokenizers, SDKs); you will be competing on ergonomics, not capability |
| Robustness | Argv-safe commands, typed errors, dry-run and replay directly attack bash's real failure modes | The safety comes from your standard library design as much as from the language; a sloppy `proc` API undoes it |
| Implementation | Rust crates cover regex, JSON, Markdown, HTTP, SSH, line editing, diagnostics; a static binary deploys by `scp` | Tracing GC in Rust is hard; slow Rust builds; you own every bug in the VM |
| Portability | One VM runs on Linux, macOS, Windows and (core only) WASM | OS features differ per platform; WASM cannot spawn processes |
| Bytecode | Precompiled files skip parsing, enable caching and hide source if wanted | You must version the format, handle stale caches and never trust foreign bytecode |
| Longevity | You will use it daily, which drives good design decisions | A language with one user has no ecosystem, no Stack Overflow, and no help when you are stuck at 2 a.m. on a broken server |

The last con matters most for the homelab goal: until the language is mature, keep a bash fallback for anything that repairs the machine your language runs on.

## Recommended language design

Design a strict, pure-by-default, expression-oriented language whose only way to touch the world is through effect calls the runtime can log, mock or refuse. Every other choice below follows from that.

### Core semantics

- **Strict evaluation**, with lazy `Stream` values (like Rust iterators or Scala `LazyList`) for files, logs and pipelines, and an explicit `lazy expr` for the rare case.
- **Immutable values only**: numbers, strings, bytes, lists, maps, records, tagged unions (`Ok`, `Err`, `Some`, `None`, user-defined). Use persistent data structures so updates are cheap.
- **Everything is an expression**: `if`, `match`, blocks. No statements, no `null`.
- **Functions are curried** and support partial application and the pipe operator `|>`. Use `.field` as a shorthand lambda for field access.
- **Proper tail calls** in the VM. Functional code loops by recursion; without tail calls, recursion over a 100k-line log overflows.
- **Exhaustive pattern matching** with destructuring of records, lists and regex captures.

### Types

Start dynamically typed with runtime *shapes*, add inference later. That order keeps the first two milestones achievable.

1. **Phase 1, dynamic + shapes.** Values carry runtime types. Add Nickel-style contracts: `let cfg: {host: Str, port: Int} = json.parse(text)` checks the shape at runtime with a precise error path (`cfg.port: expected Int, got "80"`).
2. **Phase 2, Hindley-Milner inference** with row polymorphism for records, so `fn (r) = r.name` works on any record with a `name` field. Keep a `Json` type for unknown data that you narrow with `match`.
3. **Skip type classes** at first. They are what makes Haskell monads pleasant, and they roughly double the type checker. Use built-in sugar for the common monads instead (below).

### Errors and effects

This is where your robustness goal is won or lost.

- **Errors are values**: every fallible operation returns `Result`. Provide a `?` postfix (from Rust) to propagate, and `try { ... }` blocks for sequencing, which is Haskell's `do` restricted to Result and Option.
- **No exceptions, no silent failure**: an unhandled `Err` at top level exits non-zero and prints the full context chain (`disk check on nas → ssh exited 255 → Connection refused`). Add `.context("...")` like Rust's `anyhow`.
- **Effects through one door**: processes, files, network, clock, environment and randomness are calls to host functions. The VM routes every such call through an *effect handler* it can swap: real (default), `--dry-run` (log and return planned values), `--record` (log inputs and outputs to a file) and `--replay` (answer from that file). This is the practical payoff of algebraic effects (Koka, Unison, OCaml 5) without a full effect type system.
- **Purity annotation**: mark functions `pure` (default) or `io`. The compiler forbids calling `io` from `pure`. Flix and Koka show how far this can go; a single bit is enough to start.

### Text and language processing (the Perl part)

- Regex literals with named captures: `/(?<user>\w+)@(?<host>[\w.]+)/`, usable as patterns in `match`.
- String interpolation `"${x}"`, raw strings, here-docs, and `dedent`.
- **PEG grammars as values**, Raku-style: `grammar Kv { line = key "=" value; key = /\w+/; value = /.*/ }` compiles to a parser that returns records. This is the feature that gives you Perl-level text power with structure.
- Built-in parsers and printers for JSON, YAML, TOML, CSV and Markdown (to an AST you can walk, and back). For AI work add front-matter, code-fence extraction and token-ish splitting.
- A one-liner mode borrowed from Perl: `yl -n 'line |> ...'` runs per line, `-p` prints the result, `-j` reads JSON from stdin.

### Processes (the bash part)

- **Command literals that are argv, not strings**: `` cmd`ssh ${host} df -h /` `` splits at compile time, and each `${}` is exactly one argument. Shell injection and word-splitting bugs become impossible.
- Results are records: `{status, stdout, stderr, duration}`. Non-zero exit is `Err` unless you opt out with `.allow_fail()`.
- Pipes between processes, timeouts, retries with backoff, and `par_map` for running across many hosts with a concurrency limit.
- Idempotent helpers in the spirit of Ansible: `ensure.file(path, content)`, `ensure.line_in(path, line)`, `ensure.service(name, :running)`. Each reports *changed* or *unchanged*, so dry-run output is a real plan.

### Network

HTTP client (`http.get(url) |> json`), DNS lookup, TCP connect checks, SSH (exec and file copy), and a tiny HTTP server for webhooks. All go through the effect door, so they can be recorded and replayed.

### Rules (the Prolog part, as Datalog)

Facts and rules declared in a `rules` block, queried as sets. Example: derive which services need a restart from facts about failed units, dependencies and maintenance windows. Restrict to stratified Datalog with negation; this always terminates and every answer can be explained by printing the derivation. Implement it as a naive then semi-naive bottom-up evaluator in Rust; it is a few hundred lines.

### What a script could look like

```
use std/{proc, json, md, http}

type Disk = {host: Str, mount: Str, used_pct: Int}

let hosts = ["nas", "pihole", "media"]

fn disk(host) io =
  cmd`ssh ${host} df -P /`
  |> proc.run?
  |> .stdout |> lines |> drop(1)
  |> map(fn l -> match split_ws(l) {
       [_, _, _, _, pct, mount] -> {host, mount, used_pct: int(trim_end(pct, "%"))?}
     })
  |> Ok
  |> context("disk check on ${host}")

rules {
  alert(h, m) :- disk(h, m, p), p > 85, not silenced(h).
}

let report = hosts |> par_map(disk, limit: 4) |> collect_results?

report |> query(alert) |> each(fn a -> http.post(env.NTFY_URL, "Disk ${a.m} on ${a.h} is full"))
```

Run it as `yl check.yl --dry-run` to see the SSH calls and the POST it *would* make, or `--replay last.rec` to debug yesterday's failure on your laptop.

## Implementation architecture

Build a conventional multi-stage compiler that emits compact bytecode for a register VM, with the OS behind a swappable host interface. The diagram shows how the parts connect.

&#91;embedded content: architecture · compiler, bytecode, VM, host modules\]

The compiler can run in memory or write a `.ylc` file; either way the VM reaches the OS only through the effect handler, which is what makes dry-run, replay and WASM builds cheap.

### Compiler stages

1. **Lexer** → tokens with byte spans. Use the `logos` crate, or hand-write it (a good first Rust exercise). Regex literals and command literals need a lexer *mode*, so hand-writing may be simpler.
2. **Parser** → AST. Hand-written recursive descent with Pratt parsing for operators gives the best error messages and teaches the most. Alternatives: `chumsky` (combinators with error recovery; 0.10+ is the rewrite, 1.0 still alpha), `lalrpop` (LR(1) generator), `pest` (PEG). If you want an IDE later, look at `rowan` (lossless syntax trees, used by rust-analyzer).
3. **Name resolution and desugaring** → HIR. Resolve variables to slots, turn pipelines, `?`, `try` blocks and string interpolation into core calls. Keep source spans on every node.
4. **Checks** → purity check, exhaustiveness of `match`, and later type inference.
5. **Lowering** → a small IR (A-normal form is the easiest for functional languages: every intermediate value gets a name).
6. **Code generation** → bytecode chunks per function: instructions, constant pool, line table, and upvalue descriptors for closures.
7. **Serialization** → a `.ylc` file with a magic number, format version, source hash, and the chunks. Load it directly if the hash matches the source; otherwise recompile, as Python does with `.pyc`.

Write these as separate Rust modules in one Cargo workspace: `syntax`, `compiler`, `vm`, `stdlib`, `cli`, `wasm`. Store AST and IR nodes in vectors and refer to them by index (`la-arena`, `id-arena` or `slotmap`); this avoids almost all lifetime fights.

### The VM

- **Register-based**, like Lua 5 (the Lua 5.0 implementation paper explains why it beats a stack VM). A stack VM is simpler and fine for version one; Crafting Interpreters' clox is stack-based and has many Rust ports.
- **Value representation**: start with a plain Rust `enum Value { Int(i64), Float(f64), Str(Rc<str>), List(Rc<...>), ... }`. NaN-boxing is an optimisation for later, if ever.
- **Closures with upvalues**, proper tail calls (`TAILCALL` reuses the frame), and a `CALL_HOST` instruction that goes through the effect handler.
- **Error values carry a trace**: each frame adds function name and source span, so a remote failure prints a real stack.

### Memory: use reference counting, not a tracing GC

This is the single biggest simplification your design buys. Strict evaluation plus immutable data means values cannot form reference cycles, so Rust's `Rc` (or `Arc` for parallel work) frees everything with no garbage collector. Koka (Perceus) and Lean 4 use reference counting in production for the same reason, and get in-place updates when a count is 1.

The two places cycles can appear are recursive closures and lazy thunks. Handle recursion with top-level function tables and a special `letrec` that captures by index, not by reference. If you later need a real collector, study `gc-arena` (used by the Piccolo Lua VM and Ruffle) or `dumpster`.

For persistent lists and maps use `imbl` (the maintained fork of `im`) or `rpds`.

### Bytecode as cache and as artifact

- On the fly: `yl script.yl` compiles in memory, optionally writes `.ylc` next to it.
- Static: `yl build script.yl -o script.ylc` for deployment; `yl run script.ylc` on the server.
- Self-contained: append bytecode to a copy of the VM binary to produce one executable, the trick Deno and Bun use for `compile`.
- Safety: refuse bytecode with a different format version; verify jump targets and register indices when loading, or only load bytecode you compiled. Lua 5.2 removed its verifier and untrusted bytecode became a sandbox escape.
- Serialization: `postcard` or `rkyv`. Avoid `bincode`; it was declared unmaintained in December 2025 (RUSTSEC-2025-0141).

### WebAssembly target

Compile the `vm` and `stdlib` pure parts to `wasm32-unknown-unknown` with `wasm-bindgen` for a browser playground, and to `wasm32-wasip2` for running under Wasmtime. Put OS modules behind Cargo features (`features = ["proc", "ssh", "net"]`) so the WASM build simply omits them and scripts get a clear `Err(Unsupported)`. WASI 0.3's async support may later let HTTP work there too.

### Rust crates to lean on

| Need | Crate(s) | Note |
| --- | --- | --- |
| Lexing / parsing | `logos`, `chumsky`, `lalrpop`, `pest`, `winnow`, `rowan` | Hand-written is fine too |
| Error reports | `ariadne`, `miette`, `codespan-reporting` | Coloured, source-annotated diagnostics |
| Incremental compilation | `salsa` | Only once you build an LSP |
| REPL | `reedline` (Nushell's), `rustyline` | History, completion, highlighting |
| CLI | `clap` | Subcommands `run`, `build`, `fmt`, `test` |
| Persistent collections | `imbl`, `rpds` | `im` is unmaintained |
| Regex | `regex`, `fancy-regex` | `fancy-regex` adds backreferences and lookaround |
| JSON / TOML / CSV | `serde_json`, `toml`, `csv` | Mature |
| YAML | `saphyr`/`serde-saphyr`, `yaml-rust2` | `serde_yaml` is deprecated; `serde_yml` was flagged unsound |
| Markdown | `comrak` (CommonMark + GFM AST), `pulldown-cmark`, `markdown` | `comrak` gives an editable AST |
| JSON queries | `jaq-core` | jq semantics as a library |
| Processes | `std::process`, `duct`, `nix` | `nix` for signals and Unix APIs |
| System info | `sysinfo` | CPU, memory, disks, processes |
| HTTP | `ureq` (blocking, small), `reqwest` (async) | Start with `ureq` |
| SSH | `russh` (pure Rust), `ssh2` (libssh2) | Or shell out to `ssh` first |
| File watching | `notify` | For daemons and triggers |
| Bytecode files | `postcard`, `rkyv` | Not `bincode` |
| Logging and traces | `tracing` | Structured events for `--trace` |
| LSP | `tower-lsp-server` | Community fork; original `tower-lsp` stalled |
| Tests | `insta`, `proptest`, `criterion` | Snapshot tests for parser and errors |
| WASM | `wasm-bindgen`, `wasmtime` | Playground and WASI runs |
| JIT (much later) | `cranelift` | Optional, probably never needed |

## Debuggability and robustness on remote machines

On a headless server you rarely get a debugger, so make the runtime itself the debugger: every run should be explainable after the fact from one file. Because all effects pass through one handler, each feature below is a small addition.

| Feature | What it does | Why it matters remotely |
| --- | --- | --- |
| `--dry-run` | Executes pure code, logs each effect it *would* perform, returns planned values | Review a change to 20 hosts before it touches them |
| `--trace` | Writes JSON-lines events: function entry, effect call, arguments, result, duration, source span | Read with `jq` or your own language over SSH; no debugger needed |
| `--record run.rec` / `--replay run.rec` | Records every effect's inputs and outputs; replays them deterministically | Copy a failed run to your laptop and step through it exactly |
| Context chains | Each `Err` carries a stack of `.context()` messages plus source spans | The error tells you *which* host and *which* step |
| Exit codes and JSON errors | Non-zero exit on any unhandled `Err`; `--error-format json` | Works with systemd, cron and monitoring |
| Deterministic mode | Clock, randomness and environment come from the handler | Tests and replays give identical results |
| `assert` and contracts | Runtime shape checks with a path to the bad field | Catches malformed JSON from APIs at the boundary |
| Built-in `test` blocks | `test "parses df" { ... }` with effect mocks | Scripts get unit tests without a framework |
| Single static binary | `x86_64-unknown-linux-musl` and `aarch64` builds | `scp` one file to a Raspberry Pi and run |
| Secrets redaction | Values of type `Secret` print as `***` in traces and errors | Traces are safe to share |
| Timeouts and retries | Every process and network call has a default timeout | No script hangs forever at 3 a.m. |

Two more habits from the operations world are worth building in:

- **Plan and apply**, as Terraform does: `yl plan deploy.yl` prints the changes from `ensure.*` calls, `yl apply` performs them. Idempotent operations make reruns safe after a partial failure.
- **Lint for footguns** in the compiler: unused `Result`, unchecked `allow_fail`, a command literal built from an unvalidated string. ShellCheck's catalogue of bash bugs is a good list of what to prevent by design.

## Learning Rust alongside the project

Learn Rust on a throwaway interpreter first, then start your real language; trying to learn the borrow checker and design a language in the same file is the most common way this kind of project stalls.

1. **Weeks 1–3: the basics.** The Rust Book chapters 1–10 and 13 (closures and iterators), plus Rustlings in parallel. Goal: comfortable with `enum`, `match`, `Result`, `?`, `Vec`, `HashMap`, `String` vs `&str`, modules and `cargo test`.
2. **Weeks 4–7: build Lox.** Port Crafting Interpreters' tree-walking interpreter (part II) to Rust. Then read a Rust clox port such as `loxido` (it has a safe and an unsafe branch and passes the book's test suite) and write your own bytecode VM. This teaches you compilers and Rust idioms on a design that someone else already debugged.
3. **Week 8 onward: your language**, reusing what worked.

Rust habits that make compilers easier:

- **Own data, refer by index.** AST and IR nodes live in `Vec`s or arenas; nodes point to each other with `NodeId(u32)`. No `&'a` lifetimes in your data structures at first.
- **`Rc` for runtime values, not for compiler data.** Shared ownership belongs in the VM, where values really are shared.
- **Clone freely in early versions.** Optimise after you have tests.
- **One big `enum` per tree level** (`Expr`, `Pattern`, `Instr`), matched exhaustively. Rust's exhaustiveness checking is your friend when you add a new node kind.
- **Errors as `Result<T, Diagnostic>`** throughout the compiler; collect several instead of stopping at the first.
- **Snapshot tests** (`insta`) for every parser and error message. They make refactoring safe.
- **Avoid `unsafe`, `async` and macros** until the language works. Read *Learn Rust With Entirely Too Many Linked Lists* to understand why linked structures are awkward in Rust, which is directly relevant when you implement functional lists.

Later reading, once the basics click: *Rust for Rustaceans* (Gjengset), and the rust-analyzer and Nushell source code for how large Rust language tools are organised.

## Implementation roadmap

Plan on roughly a year at about 10 hours a week, in seven phases that each end in something you can run. Phases 1 and 2 together are a complete compiler-course deliverable on their own; everything after is the part that makes the language yours.

&#91;embedded content: roadmap · 7 phases with exit gates\]

Phase 4 is highlighted because recordable effects are the feature no existing tool offers; do not let phase 5's type system delay it. Keep a running `examples/` folder of your real homelab scripts from phase 1 on and use them as the regression suite for every later phase.

## Risks, scope cuts and what to skip

The biggest risk is scope: five reference languages each took teams years. Decide now what version 1 will *not* do, and write it in the README.

| Risk | Likelihood | Mitigation |
| --- | --- | --- |
| Scope creep from five paradigms | High | Ship a minimal core first; every feature must serve a script you actually run |
| Learning Rust stalls progress | High | Lox-in-Rust warm-up; index-based data; clone freely |
| Type system takes over the project | Medium | Dynamic + contracts first; HM inference as a separate milestone |
| GC and memory bugs | Medium | Reference counting on immutable data; no `unsafe` |
| Syntax bikeshedding | Medium | Copy existing syntax (Gleam, F#, Elixir pipes) and change only what hurts |
| Slow startup kills scripting use | Low–Medium | Measure from day one; target under 20 ms for hello-world; cache bytecode |
| Your language breaks the server it should fix | Medium | Keep bash fallbacks for recovery; run new scripts with `--dry-run` first |
| Course deadline vs personal ambition | High | Agree with your instructor which milestone is the graded deliverable |

**Skip in version 1**, and say so explicitly:

- Laziness by default, and Haskell-style type classes.
- Full Prolog (unification, backtracking, cut). Datalog only.
- Macros and user-defined syntax.
- Your own regex engine, async runtime, HTTP stack or YAML parser. Use crates.
- JIT compilation and NaN-boxing.
- Threads inside the VM. Run parallel host calls on a thread pool; keep the VM single-threaded.
- Windows-specific process semantics; target Linux and macOS first.
- A package manager. Use file imports and a standard library.

**Keep**, because they are the reason the language exists: Result-based errors, argv-safe commands, the effect handler with dry-run and replay, text and JSON built-ins, and good error messages.

## Where to look further

Start with the first three items in each group; the rest are for when a specific milestone needs them.

**Books (implementation)**

- *Crafting Interpreters*, Robert Nystrom, free at craftinginterpreters.com. Tree-walker then bytecode VM; the best single starting point.
- *Writing an Interpreter in Go* and *Writing a Compiler in Go*, Thorsten Ball. Same journey, different style; easy to translate to Rust.
- *Essentials of Compilation*, Jeremy Siek (free draft). Incremental compiler construction, with a Python and a Racket edition.
- *Engineering a Compiler*, Cooper and Torczon. IR, optimisation, register allocation.
- *Modern Compiler Implementation in ML*, Andrew Appel. Functional-language compilation.
- *The Implementation of Functional Programming Languages*, Simon Peyton Jones (1987, free PDF), and *Implementing Functional Languages: a tutorial*, Peyton Jones and Lester. Only if you ever add laziness.

**Books and papers (types and semantics)**

- *Types and Programming Languages*, Benjamin Pierce. The standard text.
- *Write You a Haskell*, Stephen Diehl (online). Hindley-Milner inference step by step.
- "Complete and Easy Bidirectional Typechecking", Dunfield and Krishnaswami. A practical alternative to pure HM.
- "Perceus: Garbage Free Reference Counting with Reuse", Reinking, Xie, de Moura and Leijen, PLDI 2021.
- "Counting Immutable Beans", Ullrich and de Moura, 2019. Reference counting in Lean 4.
- "The Implementation of Lua 5.0", Ierusalimschy, de Figueiredo and Celes. Register VMs and closures.
- Flix papers on first-class Datalog (OOPSLA 2020, OOPSLA 2025), Madsen and Lhoták.
- *Foundations of Databases*, Abiteboul, Hull and Vianu, chapters on Datalog (free online). Semi-naive evaluation and stratified negation.
- "Algebraic Effects for Functional Programming", Leijen (Koka), and the Effekt language documentation.
- "Wearing the hair shirt: a retrospective on Haskell", Peyton Jones, POPL 2003.
- "Natural Language Principles in Perl", Larry Wall.

**Codebases to read (all Rust unless noted)**

- `loxido` and other clox ports: small, complete bytecode VMs.
- Koto, Rune, Rhai: production-ish scripting VMs at your target scale.
- Nickel: a functional language with contracts.
- Nushell: the stdlib you want (processes, structured data, Markdown), plus `reedline`.
- Gleam compiler: friendly errors and a functional language's type checker.
- Piccolo and `gc-arena`: if you ever need a tracing GC.
- jaq: JSON filters.
- Starlark-rust: determinism and sandboxing.

**Communities**

r/ProgrammingLanguages and its Discord, the Rust users forum, the Crafting Interpreters community, and the Koka, Gleam and Nushell Discords.

### Sources

- [PHP RFC: Make OPcache a non-optional part of PHP](https://wiki.php.net/rfc/make_opcache_required) and [PHP 8.5 UPGRADING](https://raw.githubusercontent.com/php/php-src/PHP-8.5/UPGRADING)
- [WASI 0.3 announcement, Bytecode Alliance](https://bytecodealliance.org/articles/WASI-0.3) and [WASI roadmap](https://wasi.dev/roadmap)
- [Rust platform support: wasm32-wasip1](https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip1.html)
- [Roc rewrite notes, Richard Feldman](https://gist.github.com/rtfeldman/77fb430ee57b42f5f2ca973a3992532f)
- [RUSTSEC-2025-0141: bincode is unmaintained](https://rustsec.org/advisories/RUSTSEC-2025-0141)
- [serde\_yml documentation (unsound advisory, alternatives)](https://docs.rs/serde_yml/)
- [Flix: first-class Datalog, OOPSLA 2025 paper](https://plg.uwaterloo.ca/~olhotak/pubs/oopsla25b.pdf)
- [Lua mailing list on the 5.2 bytecode verifier](https://lua-users.org/lists/lua-l/2010-08/msg00488.html)
- [Berger et al., reproduction study on languages and code quality](https://arxiv.org/abs/1901.10220)
- [Wearing the hair shirt, Peyton Jones](https://www.microsoft.com/en-us/research/?p=252881)
- [Natural Language Principles in Perl, discussed in Dr. Dobb's](https://www.drdobbs.com/web-development/on-perl-as-a-natural-language/184416103)
- [Nushell 0.115.0 release notes](https://www.nushell.sh/blog/2026-08-15-nushell_v0_115_0.html)
- [Nickel 1.0 release, Tweag](https://www.tweag.io/blog/2023-05-17-nickel-1.0-release)
- [Babashka](https://babashka.org/)
- [Oils blog](https://oilshell.org/blog/2025/01/release-0.24.0.html)
- [Perceus, Microsoft Research](https://www.microsoft.com/en-us/research/publication/perceus-garbage-free-reference-counting-with-reuse-2/)
- [chumsky on docs.rs](https://docs.rs/crate/chumsky/latest)
- [imbl on docs.rs](https://docs.rs/crate/imbl/latest)
- [tower-lsp-server on docs.rs](https://docs.rs/crate/tower-lsp-server/latest)
- [Piccolo Lua VM](https://github.com/kyren/piccolo)
- [loxido, a Rust clox port](https://github.com/ceronman/loxido)
- [Rust learning resources 2026, corrode.dev](https://corrode.dev/blog/rust-learning-resources-2026/)
