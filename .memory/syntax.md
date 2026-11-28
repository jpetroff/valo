# valo syntax decisions

Source of truth for concrete syntax. Consumed by AI agents; plan-mvp.md stays syntax-agnostic and defers here.
Status: `DECIDED` (user picked) · `PROVISIONAL` (assumed, not yet asked) · `OPEN` (next rounds).
Global style: Python-like, indentation blocks (INDENT/DEDENT/NEWLINE), no `;`, standard operators, expression-oriented (last expression of a block is its value), Pydantic-strict annotations.

## Round 1 — 2026-11-26

### S1 Calls and partial application — DECIDED
- Call: `f(a, b)`. Parens mandatory, comma-separated. No juxtaposition application. No auto-currying.
- Arity mismatch = error. Compile time when callee resolves to a known fn (E-code), else runtime. Hint: `use add(1, _) for a partial`.
- Partial application is explicit with `_` placeholder (Gleam/Scala): `add(1, _)`.
  - Desugar at compile time: `f(a, _, c)` → `(x) => f(a, x, c)` (a and c evaluated once, before the closure is built; bind to temps). Multiple `_` → params in left-to-right order.
  - `_` valid only as a direct call argument. Elsewhere `_` = wildcard pattern / discard.
- Pipe: `x |> f(a)` ≡ `f(x, a)`; `x |> f` ≡ `f(x)`. Value goes into the FIRST argument. If the call contains `_`, the piped value goes there instead (`x |> f(a, _)` ≡ `f(a, x)`).
- Pipe continuation lines: a line starting with `|>` continues the previous expression (lexer: suppress NEWLINE before a line whose first token is `|>`; also inside open brackets).
- Consequence for stdlib: data-FIRST argument order: `map(xs, f)`, `filter(xs, p)`, `reduce(xs, init, f)` with `f(acc, x)`.
- Consequence for VM: no PAP object, no over-application, nullary functions allowed (`read_line()`).

### S2 Named function definition — DECIDED
- Keyword `fn`. Inline Python-style param annotations, `->` return type.
- One-liner: `fn add(x: int, y: int) -> int = x + y` (PROVISIONAL: a single expression may continue on the next indented line after `=`).
- Block: header ends with `:`, indented body, value = last expression.
  ```
  fn fact(n: int) -> int:
      if n <= 1: 1
      else: n * fact(n - 1)
  ```
- Annotations optional or required: OPEN (plan Q8).
- No multi-clause / separate Haskell signature lines. Case analysis via `match`.

### S3 Lambdas — DECIDED (user override: `=>` not `->`)
- `x => x + 1`, `(a, b) => sum(a, b)`, `() => read_line()`.
- Optional param annotations inside parens: `(x: int) => x * 2` (PROVISIONAL).
- Block body: `=>` at end of line, then indented block (PROVISIONAL):
  ```
  handler = req =>
      body = req.body
      json.parse(body)
  ```
- Parsing: at expression start, `IDENT =>` (2-token lookahead) or `(` … matching `)` followed by `=>` (scan tokens to matching paren, no AST backtracking) → lambda; otherwise grouping/tuple.
- `->` is therefore NOT used for lambdas. `->` remaining uses: return type and function types only (match arms use `~>`, S7).

### S4 Type names — DECIDED (Python / Pydantic spelling)
- Scalars: `int`, `float`, `bool`, `str`, `bytes` (later); unit `()` (S9).
- Generics with brackets: `list[int]`, `dict[str, int]`, `tuple[int, str]` (later).
- Optional: `T | none` (spelling per S9). `none` is the empty value, does not inhabit every type; `let x: int = none` is a type error.
- Function types: `str -> int`, `(int, int) -> int`, `() -> str`. `->` is right-associative.
- Result type: `T ! error` (S10).
- Records: `def Name as` (S8).
- Namespace: types are lowercase and live in a separate type namespace (annotation positions only). Same name may also be a value (conversion fn `int("42")`, Python-like) → PROVISIONAL.
- Parser consequence: case does not distinguish types from values. Constructors (`Ok`, `Err`, user records/variants) capitalised by convention → OPEN whether enforced.

## Round 2 — 2026-11-26

### S5 Bindings — DECIDED (user: `let` + Rust-style shadowing)
- `let x = 1`, `let port: int = 8080`. Keyword mandatory for every value binding (top level and blocks).
- Shadowing allowed in the SAME scope and nested scopes: `let text = trim(text)` creates a new binding; RHS sees the previous one. Values stay immutable; closures keep the value they captured.
- No bare `x = 1` assignment form exists. `=` after `let` pattern / in `fn ... = expr` / in named args / field defaults only.
- Plan Q7 resolved → (b).
- PROVISIONAL: `let` with destructuring patterns (`let [a, b] = xs`, `let Disk(host=h) = d`) post-MVP; `let` is not recursive — recursion via `fn`.

### S6 Conditionals — DECIDED
- Block form, Python: `if c:` / `elif c:` / `else:`; each branch either inline after `:` or an indented block. It is an expression.
- Inline form, Haskell/ML: `if c then a else b`. Parser: after the condition, `then` → inline form, `:` → block form.
- PROVISIONAL: inline form has no `elif`; nest instead (`if a then x else if b then y else z`). `else` required whenever the value is used (plan D22).

### S7 Pattern matching — DECIDED (user: `~>` arm arrow)
- ```
  match parse_int(s):
      Ok(n) if n > 0 ~> n
      Ok(_) ~> 0
      Err(e) ~>
          log(e)
          -1
  ```
- `~>` is one lexer token (maximal munch). Guard: `pat if cond ~> expr`. Wildcard `_`. Block body: `~>` at end of line + indented block.
- Arrow summary (no overloading): `->` types only (return type, function types) · `=>` lambdas · `~>` match arms.
- Patterns (PROVISIONAL): literals, `_`, names, constructors `Ok(x)`, list `[a, b, *rest]`, record `Disk(host=h)` / `{host, mount}`. Exhaustiveness checking post-MVP (plan M12).

### S8 Type and record definitions — DECIDED (user: `def ... as`)
- `def` defines TYPES (structure/alias); `fn` defines functions. `as` introduces the body (replaces `:` to avoid colon stacking inside structure definitions).
- Record (Pydantic-style model): `def Name as` + NEWLINE + INDENT + field lines `name: type` or `name: type = default`.
  ```
  def Disk as
      host: str
      mount: str = "/"
      used_pct: int
  ```
- Alias / union on one line: `def MaybeInt as int | none`. Parser: after `as`, NEWLINE+INDENT → record body, else a type expression.
- Construction / update / validation: see S17. Access `d.used_pct`. Records immutable, no methods, no inheritance. Type names capitalised by convention (user's examples: `Disk`, `MaybeInt`) while builtin types stay lowercase.
- Tagged unions: see S18.
- Empty value spelling resolved in S9: `none` (user's `null` here is superseded).

## Round 3 — 2026-11-26

### S9 Literals and the empty value — DECIDED (user: lowercase `none`)
- `true`, `false`, `none`. All keywords lowercase, matching lowercase builtin types.
- Supersedes `None` (S4) and `null` (S8): optional type is `T | none`; `def MaybeInt as int | none`.
- Option has NO visible wrapper (Python/TypeScript style): a value of `int | none` is either an int or `none`. `none` does not inhabit `int`. No `Some(x)` constructor. `T | none | none` = `T | none`.
- PROVISIONAL: unit value and type `()` — what `print(...)` returns, displayed `()`.

### S10 Errors — DECIDED (Rust-style)
- Result constructors `Ok(v)`, `Err(e)`. Result type `T ! error` (`!` binds looser than `|`: `int | none ! error` = result of optional int). PROVISIONAL: `error` is a builtin record (message, context chain, span); user error types later (`T ! MyErr`).
- Postfix `expr?`: Ok(v) → v; Err(e) → return Err(e) from the enclosing `fn`. At top level, an unhandled Err exits 1 printing the context chain + trace.
- `context(r, "msg")` (pipe form `r |> context("msg")`) adds a message to an Err chain; Ok passes through.
- PROVISIONAL: `?` binds to the whole pipe stage it ends: `x |> f(a)?` ≡ `f(x, a)?` (the pipe is rewritten to a call before `?` applies); outside pipes `?` is the tightest postfix like `.field` and calls.
- PROVISIONAL: `?` on `T | none` not allowed in MVP (use `match` or an `or_else`-style helper); revisit.

### S11 Operators — DECIDED (Python exactly)
- Logic: `and`, `or`, `not` (short-circuit, bool operands only).
- Comparison: `== != < <= > >=`, chaining `0 < x <= 100` (middle evaluated once).
- Arithmetic: `+ - *`, `/` true division ALWAYS float (`7 / 2 == 3.5`, the one implicit int→float conversion), `//` floor division (int,int → int), `%` floor modulo, `**` power, unary `-`.
- Other int/float mixing stays an error (`1 + 2.0` → error with conversion hint); plan D10.
- `+` also concatenates str+str and list+list (same types only).
- Pipe `|>` (S1) has the LOWEST binary precedence. Full precedence table (PROVISIONAL, Python order): `|>` < `or` < `and` < `not` < comparisons < `+ -` < `* / // %` < unary `-` < `**` (right-assoc) < postfix (call, `.field`, `[index]`, `?`).

### S12 Strings — DECIDED (`"{x}"` always on)
- Every normal string literal interpolates: `"Disk {d.mount} on {d.host}"`. Any expression inside `{}`.
- Literal braces escaped: `\{` `\}`. Usual escapes `\n \t \" \\ \u{1F600}`.
- PROVISIONAL: raw strings `r"..."` — no escapes, no interpolation (use them for JSON templates and regexes to avoid escaping braces).
- PROVISIONAL: triple-quoted `"""..."""` multi-line, auto-dedented to the closing quote's indentation, interpolating; `r"""..."""` raw variant.
- PROVISIONAL: future command literals use the same `{}` interpolation, each `{}` = exactly one argv element.
- Lexer: string token with embedded expression segments; track brace depth; nested strings inside `{}` allowed.

## Round 4 — 2026-11-26

### S13 Required annotations — DECIDED (plan Q8 → b)
- Every `fn` parameter AND return type must be annotated. Missing → E0410 `parameter p of fn grade needs a type annotation, e.g. p: int` (return: `... needs a return type, e.g. -> int`).
- `let` annotations optional; lambda param annotations optional.
- Annotated values checked at runtime at fn entry/return (plan D13/M10).
- Generics: S20. PROVISIONAL: escape-hatch type `any`.

### S14 Collections — DECIDED (Python literals + `.key` on dicts)
- List `[1, 2, 3]`, dict `{"host": "nas", "port": 22}` (keys are expressions), empty dict `{}`, tuple `(a, b)` / `(a,)`.
- Index: `xs[0]`, `d["k"]` — missing key / out-of-range index = runtime error naming the path (`d.items[3]: index out of range (len 2)`). Safe lookup `get(d, "k")` / `get(xs, i)` → `T | none`.
- `.key` sugar: on a dict, `d.key` ≡ `d["key"]` (identifier-shaped keys only; others use `d["content-type"]`). On a `def` record, `.field` is the field. Modules are records, so `json.parse` is the same mechanism. No methods exist, so `.name` is never ambiguous.
- Parsed JSON: objects → `dict[str, json]`, arrays → `list[json]`; `json` = builtin union type of the JSON values.
- PROVISIONAL: `range(a, b)` end-exclusive, `range(n)` = `range(0, n)`; `x in xs` membership operator (Python); no set literal in v1.
- Lambda vs tuple: `(a, b) =>` is a lambda, `(a, b)` without `=>` is a tuple (S3 lookahead).

### S15 Comprehensions — DECIDED (Python, alongside pipelines)
- List: `[expr for x in xs if cond]`, nested `for` clauses allowed: `[(h, p) for h in hosts for p in ports]`.
- Dict: `{k: v for x in xs if cond}`.
- Pure sugar, desugared in the compiler: single `for` → `map`(+`filter`), multiple `for` → `flat_map`. No VM support. Loop variables are fresh immutable bindings per element.
- `for` appears ONLY inside comprehensions; there are no `for`/`while` statements (iteration = recursion, pipelines, comprehensions).
- PROVISIONAL: destructuring in the loop target `for (h, p) in pairs`.

### S16 Modules and comments — DECIDED (Python import)
- `import json`, `import std.proc as p`, `from text import lines, trim`, local file `import ./lib/disks` (path relative to the importing file, extension omitted; binds the last segment, `as` to rename).
- Calls on imports are qualified (`json.parse`) unless imported by name with `from`.
- A module evaluates to a record of its top-level `fn`/`def`/`let` bindings (Lua-style module = value). PROVISIONAL: everything top level is exported; `_name` = private.
- Prelude (no import needed, flat names): `print`, `len`, `map`, `filter`, `reduce`, `flat_map`, `range`, `get`, `int`, `float`, `str`, `Ok`, `Err`, `context`… (final list with stdlib design). Everything else via `import`. Plan Q14 → hybrid.
- Comments: `#` to end of line. First line `#!...` shebang allowed. PROVISIONAL: `##` doc comments attach to the next `fn`/`def`.
- MVP: only the prelude; `import` is post-MVP except as reserved keywords.

## Round 5 — 2026-11-26

### S17 Records: construct, copy-update, validate — DECIDED (Python `**` spread)
- Construct with keyword args: `Disk(host="nas", used_pct=91)`; fields with defaults may be omitted. PROVISIONAL: positional construction `Disk("nas", 91)` → E0420 (keyword-only, matches S19 spirit).
- Copy-update by spread: `Disk(**d, used_pct=95)`. DEVIATION from Python (which raises on duplicate kwargs): explicit keywords AFTER a spread override the spread's fields; explicit duplicate keywords (`a=1, a=2`) → error.
- Build from a dict: `Disk(**raw)` — strict checks, runtime error on mismatch (missing field, wrong type, PROVISIONAL: unknown extra key = error, Pydantic `extra="forbid"`).
- Non-crashing validation: `Disk.validate(raw)` → `Disk ! error` with field path in the message (`used_pct: expected int, got str ("91%")`). PROVISIONAL: a type name in value position is its constructor; `.validate` is a builtin pseudo-field on every `def` type.
- PROVISIONAL: `**` spread also in fn calls for keyword params: `run(cmd, **opts)`. Spreading a record into a dict: `dict(**d)`.
- No `with` keyword (rejected option).

### S18 Tagged unions — DECIDED (leading `|`)
- Block form: `def Name as` + INDENT + lines starting with `|`:
  ```
  def Event as
      | Up(host: str)
      | Down(host: str, reason: str)
      | Maintenance
  ```
- One-line form: `def Shape as Circle(r: float) | Rect(w: float, h: float)`.
- Parser: after `def Name as` NEWLINE INDENT, first token `|` → union body, IDENT `:` → record body. On one line, a `|`-separated list where any member is `Name(fields)` or an undefined capitalised name → union of new variants; otherwise a type alias (`int | none`). PROVISIONAL rule: one-line form defines variants only if ALL members are new constructor names; mixing aliases and new variants → error.
- Variant fields use record field syntax (`name: type = default`). Fieldless variants are values (`Maintenance`).
- Construction: same rules as records (S17): `Down(host="nas", reason="ping")`. PROVISIONAL: patterns may be positional in declared field order (`Down(h, r)`) or keyword (`Down(host=h)`).
- Variant names live in the module namespace (Haskell style). PROVISIONAL: also reachable qualified as `Event.Up`.
- Closed set → `match` exhaustiveness checks (post-MVP).
- Generic unions with S20: `def Tree[T] as | Leaf | Node(left: Tree[T], value: T, right: Tree[T])`.

### S19 Arguments — DECIDED (defaults are keyword-only)
- Parameters without default = required, passed positionally. Parameters with default MUST be passed by name: `run("df -h", timeout=5)`; `run("df -h", 5)` → E0421 `timeout has a default and must be passed by name: timeout=5`.
- Required params after a defaulted one → E0423 (declaration order: required first).
- PROVISIONAL: required params may also be passed by name (`run(cmd="df")`), Python-like.
- Pipe and `_` placeholder act on positional (required) params only: `cmd |> run(retries=2)` ≡ `run(cmd, retries=2)`.
- Default expressions evaluated at each call (callee side), may reference earlier params. No Python shared-mutable-default footgun (values are immutable).
- PROVISIONAL (implementation constraint, visible to user): keyword args require a statically known callee (top-level/local named fn, native, record/variant constructor, immutable binding of those). Keyword args to an unknown function value → E0422. Lambdas have no defaults.

### S20 Generics — DECIDED (PEP 695)
- `fn first[T](xs: list[T]) -> T | none`, `fn swap[A, B](p: (A, B)) -> (B, A)`.
- Generic types: `def Page[T] as` + fields; use `Page[int]`.
- Type vars must be declared; undeclared names in types → E0402. Tuple types `(A, B)`.
- MVP: type vars accepted, checked only for outer shape (S13); full checking with HM inference later.

### S21 Nested functions — PROVISIONAL (assumed, not asked)
- `fn` allowed inside `fn` bodies; may be self-recursive; closures capture by value (plan D5). Local mutual recursion unsupported in MVP (plan Q16).

## Round 6 — 2026-11-26

### S22 Effects — DECIDED (inferred, no marker)
- No `io`/`pure` keyword. The compiler infers an effect bit per fn: io if it (transitively) calls an io native (files, processes, network, clock, env, randomness, stdin/stdout). Fixpoint over the call graph; calls through unknown fn values count as "maybe io".
- Not a restriction: pure code may call io code. Guarantees for `--dry-run`, `--trace`, `--record`, `--replay` come from the RUNTIME: every io native goes through the host effect handler, regardless of inference.
- Tooling: `valo check --effects` lists each fn as `pure` / `io (via run, read_file)`. Diagnostics may mention it (e.g. "this test calls io fn disk; mock it").
- PROVISIONAL: inference results stored in bytecode debug info for traces; possible later opt-in `pure fn` assertion if wanted (not planned).

### S23 Command literals — DECIDED (backticks)
- `` `ssh {host} df -P {mount}` `` builds a `cmd` VALUE; nothing runs until `run(c)`.
- Split into argv at COMPILE time on unquoted whitespace. Each `{expr}` is exactly one argv element (no word-splitting, no globbing). `{*xs}` spreads a `list[str]` into separate elements. Text glued to an interpolation stays one element: `--name={n}` → one arg.
- PROVISIONAL: inside backticks, `"..."` groups words into one literal arg (`` `grep "two words" f` ``); no other shell metacharacters (`> < & ; * $`) have meaning — they are literal text. Escapes `\`` `\{` `\}`.
- PROVISIONAL: interpolated non-str values converted with `str()` (int, float, bool); lists only via `{*xs}`; other types → error.
- `run(c, timeout: int = 30, allow_fail: bool = false, stdin: str | none = none, env: dict[str, str] = {}, cwd: str | none = none) -> out ! error` (defaults keyword-only, S19). `out` record: `status`, `stdout`, `stderr`, `duration`.
- Non-zero exit → `Err` (message has command, status, stderr tail) unless `allow_fail=true`.
- Process pipe: `c1 | c2` combines cmd values into a pipeline cmd; `|` in EXPRESSIONS is defined only for `cmd` (type syntax `|` unaffected). Pipeline fails if any stage fails (pipefail by default).
- Lexer: backtick literal token with argv segments + interpolation segments (reuses S12 interpolation machinery).

### S24 Regex — DECIDED (`re"..."`)
- `re"..."`: raw (no escape processing, like `r""`), validated and compiled at compile time; invalid regex = compile error with span. Flags inline: `re"(?i)error"`. PROVISIONAL engine: `regex` crate syntax (no backreferences/lookaround; `fancy-regex` later if needed).
- `s =~ r` → bool (unanchored search, Perl semantics; anchor with `^ $`). PROVISIONAL `!~` negation.
- In `match`, a regex is a pattern: matches if found; named groups bind as variables of type `str` (optional groups → `str | none`). Arm order matters as usual.
- Functions (prelude or `text` module, provisional names): `find(s, r) -> match_ | none`, `find_all(s, r) -> list[str]`, `captures(s, r) -> dict[str, str] | none`, `replace(s, r, repl)`, `split(s, r)`.
- PROVISIONAL: no interpolation inside `re"..."` in v1 (avoid regex injection); build dynamic regexes with `regex(str) -> regex ! error`.

### S25 Tests — DECIDED (separate test files)
- Tests live only in `*_test.vl` files, which import the module under test. `valo test [path]` discovers them.
- Inside test files: `test "name":` + indented block; `assert expr` (on failure prints both sides of a top-level comparison + span); PROVISIONAL `mock name = fn_value` inside a test block replaces an io native or io fn for that test only (implemented via the effect handler / global rebinding in the test runner).
- `test` blocks in non-test files → compile error. Test files are never run by `valo run`.
- Depends on imports (S16) → post-MVP. PROVISIONAL: `assert` is also usable in normal code (runtime error on failure) — candidate for the MVP since golden tests benefit.

## Implementation notes from M1 (PROVISIONAL, chosen while building the lexer; not yet confirmed by the user)
- Indentation: spaces only; a tab inside indentation = E0006. Any consistent width; dedent must return to an earlier level (E0003).
- Integer literals: decimal, `0x`/`0o`/`0b` (Python); `_` separators only between digits (`1_000`, not `1__0`/`_1`/`1_`); decimal leading zeros rejected (`007` = E0010, hint `0o`), `0`/`00` allowed (Python).
- Float literals (lexed now, used in M3): `1.5`, `2e3`, `1e-2`; `1.` followed by a non-digit is int `1` then `.`.
- No unary `+`. `-2 ** 2` = `-(2 ** 2)`, `2 ** -1` allowed (Python).
- `int` range is i64; the literal `9223372036854775808` is out of range (E0002) even after `-`, so `i64::MIN` is written `-9223372036854775807 - 1`.

## Implementation notes from M2–M3 (PROVISIONAL, chosen while building; not yet confirmed by the user)
- Block form of `if` (S6): after an inline branch, `elif`/`else` on the next line at the same indentation continue the `if` (`if c: 1` NEWLINE `else: 2`). Branches can mix inline and indented forms.
- A block's last line must be an expression (E0104) — user-confirmed for M2; the top level of a file may end with `let`.
- `let _ = expr` evaluates and discards; `_` used as a value is an error (E0206).
- `assert expr` is an expression of type `()` usable anywhere an expression is (plan D29).

## Implementation notes from M4–M5 (PROVISIONAL unless marked user-confirmed)
- USER-CONFIRMED: a top-level `fn` is visible in the whole file (hoisted); a later top-level `let` of the same name shadows it from that line on; two top-level `fn`s with the same name are an error (E0207).
- USER-CONFIRMED: a fn body sees the top-level binding in effect where the `fn` is written; if there is none yet, the first later `let` of that name (forward reference; reading it before it runs is a runtime error).
- USER-CONFIRMED (for now): required params cannot be passed by name (S19 provisional item deferred); a keyword naming a required param is an error with the hint "pass `x` by position".
- Trailing commas allowed in parameter lists, argument lists and type-variable lists (Python).
- `()` is the unit expression as well as the unit type.
- One-liner `fn f(…) -> T =` may put its single expression on the next, indented line (S2 provisional, as in the composite example's `sum_sq_even`).
- `int(x)` / `float(x)` return a value of their own type unchanged; `int` truncates toward zero.
- A positional argument after a keyword argument is a syntax error (Python).
- `x |> (y |> f)` calls the computed function `(y |> f)` with `x`; only a plain call on the right of `|>` receives the value as its first argument.

## Composite example (rounds 1–6; builtin names provisional)

```
#!/usr/bin/env valo
import json

def Disk as
    host: str
    mount: str = "/"
    used_pct: int

fn grade(p: int) -> str:
    if p > 90: "crit"
    elif p > 80: "warn"
    else: "ok"

fn read_pct(path: str) -> int ! error:
    let text = read_file(path) |> context("reading {path}")?
    match parse_int(trim(text)):
        none ~> Err(error("not a number: {text}"))
        p ~> Ok(p)

fn sum_sq_even(xs: list[int]) -> int =
    xs |> filter(x => x % 2 == 0) |> map(x => x ** 2) |> reduce(0, (acc, x) => acc + x)

def Event as
    | Up(host: str)
    | Full(disk: Disk)

fn parse_line(host: str, line: str) -> Disk | none:
    match line:
        re"(?<pct>\d+)% (?<mnt>\S+)$" ~> Disk(host=host, mount=mnt, used_pct=int(pct))
        _ ~> none

fn df(host: str, mount: str = "/") -> list[Disk] ! error:   # io inferred (S22)
    let out = run(`ssh {host} df -P {mount}`, timeout=10)?   # argv-safe (S23)
    let rows = out.stdout |> lines |> drop(1)
    rows |> filter_map(parse_line(host, _)) |> Ok          # filter_map provisional (Q-S26)
let disks = [Disk(host="nas", used_pct=91), Disk(host="pi", used_pct=40)]
let bumped = Disk(**disks[0], used_pct=95)             # copy-update via spread
let full = disks |> filter(d => d.used_pct > 85)
let status = if len(full) > 0 then "alert" else "ok"
let ratio = len(full) / len(disks)          # 0.5, float
full |> each(d => print("Disk {d.mount} on {d.host}: {grade(d.used_pct)}"))
let usage = {d.host: d.used_pct for d in disks}        # dict comprehension
let cfg = json.parse(read_file("cfg.json")?)?          # needs `import json` (post-MVP)
let port = cfg.server.port                             # .key sugar on dicts
let limit = read_pct("/etc/valo/limit")?    # unhandled Err → exit 1 with chain
```

## Open questions for next rounds
- Q-S21 `match` pattern details: list rest `[a, *rest]`, dict patterns, literal/range patterns, `as` bindings, or-patterns (`A | B ~>`; note `|` meaning in patterns).
- Q-S22 slicing `s[1:3]`, `xs[-1]` (Python negative indices?), string methods vs functions.
- Q-S23 streams / lazy sequences for big files and logs (`lines(file)` lazy?), `each` vs comprehension for side effects.
- Q-S24 concurrency syntax for many hosts (`par_map(xs, f, limit=4)` only, or more).
- Q-S26 filtering `T | none` results: `filter_map(xs, f)` vs walrus in comprehensions vs `[d for d in map(...) if d != none]`; and whether a `T ! error` fn body auto-wraps its final value in `Ok` (currently: explicit `Ok(...)` required, Rust-style).
- Q-S25 one-liner mode flags (`valo -n 'expr'`, `-p`, `-j`) and implicit `line` binding.
