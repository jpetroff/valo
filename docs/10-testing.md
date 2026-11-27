# Chapter 10 — Testing

A compiler is a large function from text to behaviour, with an enormous
input space. valo uses four kinds of tests, each catching a different class
of bug. All run with one command:

```bash
cargo test --workspace
```

Current count: 36 tests plus 2 doctests (the code examples in the doc
comments of `ProtoBuilder` and `Diagnostic::new` are compiled and run too),
all green, together with
`cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`.
A milestone is done only when all three pass.

## 10.1 Unit tests

Small tests next to the code, in a `#[cfg(test)] mod tests` block at the
bottom of each file (chapter 1, *Tests*). Examples:

| File | Tests |
|------|-------|
| `vm/src/opcode.rs` | opcode numbering is dense; extreme `sBx`/`sJ` values |
| `vm/src/vm.rs` | hand-assembled programs run (`ProtoBuilder`); floor division/modulo table; error trace present |
| `vm/src/chunk.rs` | line index and `line_col` |
| `compiler/src/span.rs` | columns count characters, not bytes |
| `compiler/src/lexer.rs` | layout cases, numbers, keywords, every lexical error code |
| `compiler/src/parser.rs` | error recovery reports several errors per run |
| `compiler/src/codegen.rs` | constant deduplication; register limit E0301 |

VM tests use `ProtoBuilder` + `TestHost`, so they test the VM alone, without
the compiler — useful to land a VM feature before the parser supports it.

Run a subset by name: `cargo test -p valo-compiler lexer`.

## 10.2 Snapshot tests (insta)

For outputs that are long but must not change by accident — token dumps, ASTs,
disassembly — writing the expected text inside the test is unwieldy. Snapshot
tests store it in a file instead:

```rust
insta::assert_snapshot!(parse_ok("1 + 2 * 3\n..."));
```

The first run writes `src/snapshots/<crate>__<module>__tests__<name>.snap`;
later runs compare against it. Current snapshots:

```text
compiler/src/snapshots/valo_compiler__lexer__tests__operators_maximal_munch.snap
compiler/src/snapshots/valo_compiler__lexer__tests__indentation_blocks.snap
compiler/src/snapshots/valo_compiler__parser__tests__precedence_and_associativity.snap
compiler/src/snapshots/valo_compiler__codegen__tests__three_op_expression.snap
```

When a change is intended (e.g. a new disassembly column):

```bash
INSTA_UPDATE=always cargo test --workspace
```

then **read the diff** of the `.snap` files (`git diff`) before committing.
A snapshot is only as good as the review of its first version.

## 10.3 Property tests (proptest)

Instead of hand-picked inputs, a property test states a law and lets
`proptest` try hundreds of random inputs (and shrink any failure to a minimal
example):

| Property | File |
|----------|------|
| encode → decode returns the same fields, for every format | `vm/src/opcode.rs` |
| `+ - *` agree with Rust's `checked_*` | `vm/src/vm.rs` |
| `x == (x // y) * y + x % y`, remainder has divisor's sign | `vm/src/vm.rs` |
| the lexer never panics and always ends with EOF, on random Unicode | `compiler/src/lexer.rs` |
| DEDENTs never exceed INDENTs (plus recovered errors) | `compiler/src/lexer.rs` |

The lexer property catches a whole bug class at once: slicing a string inside
a multi-byte character, which would crash the compiler on unusual input.

## 10.4 Golden tests (end to end)

File: [`compiler/tests/golden.rs`](../compiler/tests/golden.rs); programs in
`compiler/tests/golden/`.

Each `.vl` program is run through the real `valo run` binary, and stdout,
stderr and the exit code are compared with files next to it:

```text
compiler/tests/golden/m1/
├── floor.vl          -7 // 2   -7 % 3   7 % -3   2 ** 10
├── floor.args        --echo
├── floor.stdout      -4 2 -2 1024 (one per line)
├── overflow.vl       9223372036854775807 + 1
├── overflow.stdout   (empty)
├── overflow.stderr   error[R0101]: integer overflow: … + location + trace
└── overflow.exit     1
```

| File | Meaning |
|------|---------|
| `.vl` | program |
| `.stdout` | expected stdout (required) |
| `.stderr` | expected stderr (absent = must be empty) |
| `.exit` | expected exit code (absent = 0) |
| `.args` | extra flags for `valo run` |
| `.stdin` | input to feed |
| `.files/` | a directory whose contents are copied next to the program (fixtures for `read_file`, M7) |

Golden tests test what the user sees, including error message wording. They
are the regression suite for every later milestone: M1's goldens must keep
passing unchanged in M2…M10 unless a change is intended.

Present goldens: `m0/empty`, `m0/no_echo`, and in `m1/`: `precedence`,
`floor`, `pow`, `layout`, `overflow`, `div_zero`, `neg_pow`, `lex_error`,
`parse_error`, `big_literal`, `bad_indent`, `true_div`.

### Adding a golden test

1. Write `compiler/tests/golden/m1/<name>.vl`.
2. Write `<name>.stdout` **by hand** with the output you expect (and
   `<name>.args` if you need `--echo`).
3. Run `cargo test -p valo-compiler --test golden`.
4. For error tests, generate stderr/exit with
   `VALO_BLESS=1 cargo test -p valo-compiler --test golden`, then read the
   generated `.stderr` and check it says what you expect.

`VALO_GOLDEN=floor cargo test -p valo-compiler --test golden` runs only
matching tests. Each test runs in a fresh scratch directory under `target/`
with a copy of the `.vl` (plus the fixtures from `<name>.files/`), so file
names in messages are stable and files a program writes stay there.

## 10.5 CLI tests

`compiler/tests/cli.rs` checks what golden files cannot: exit 64 for a missing
file and bad arguments, exit 0 for `--help`/`--version`, that
`check --dump bytecode` prints a disassembly, `--color`, and R0501 when
stdout is `/dev/full` (Linux).

## 10.6 Error-code coverage

`compiler/tests/codes.rs` reads every expected `.stderr` file and checks
that each code of the two catalogues — `E_CODES` in `diagnostic.rs` and
`ErrorKind::ALL` in `vm/src/error.rs` — appears in at least one of them
(retired codes, the debugger-only R0801, internal R0901 and R0501 are
exempt). Adding a code without a golden test fails this test. Goldens for
codes no earlier milestone needed live in `compiler/tests/golden/m9/`.

## 10.7 Exercises

1. Add a golden test `m1/mod_neg.vl` for `-1 % 7` (expect `6`).
2. Break `int_arith`'s floor adjustment on purpose (remove `- 1`) and see
   which tests catch it: unit, property and golden.
3. Write a property test: for random `x`, the source text `format!("{x}")`
   lexes to exactly `[Int(x), NEWLINE, EOF]`.

## 10.8 Further reading

- insta documentation (insta.rs); proptest book (altsysrq.github.io/proptest-book).
- The rustc test suite's "UI tests" — the industrial form of golden tests.
