# The valo language

User-facing documentation of the valo language as implemented today
(milestones M0–M8). The chapters one level up (`docs/00-…` to `docs/13-…`)
explain how the compiler and VM are *built*; this folder describes the
*language* itself.

| Document | What it is for |
|----------|----------------|
| [Syntax guide](syntax.md) | Every syntax pattern with runnable examples: literals, strings, `let`, operators, `if`, blocks, `fn`, calls, lambdas, `_` partials, `|>`, results and `?`, `assert`, type annotations, built-ins, lists and comprehensions. Start here. |
| [Formal grammar](grammar.md) | The normative specification: lexical grammar, layout (INDENT/DEDENT) algorithm, EBNF syntactic grammar, operator binding powers, desugaring rules and static constraints with their error codes. |

## At a glance

```valo
fn grade(pct: int) -> str:
    if pct > 90: "crit"
    elif pct > 80: "warn"
    else: "ok"

fn parse_pct(s: str) -> int ! error:
    let n = parse_int(s)
    if n == none then Err("not a number: {s}") else Ok(n)

let pct = parse_pct("93")?
print("{pct}% → {grade(pct)}")              # → 93% → crit
```

- Indentation-based blocks, no `;`, expression-oriented (no `return`).
- `let` bindings are immutable; rebinding shadows.
- Required type annotations on `fn` parameters and return types.
- Lambdas `x => …`, explicit partials `f(a, _)`, pipes `x |> f(a)`.
- Errors as values: `Ok`/`Err`, propagated with `?`.
- Lists `[1, 2]`, `xs[i]`, `x in xs`, comprehensions `[x * 2 for x in xs if x > 0]`.

Syntax that is decided but not implemented yet (dicts, tuples, `match`,
`def` records, imports, command literals, regexes) is listed in
[syntax guide §19](syntax.md#19-reserved-syntax-not-implemented-yet); the
full record of design decisions is `.memory/syntax.md`.
