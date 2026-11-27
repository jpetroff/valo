# Chapter 6 — Code Generation

## 6.1 Theory: from tree to instructions

**Code generation** walks the syntax tree and emits instructions for the
target machine. For a register machine, the central question is **register
allocation**: which register holds each intermediate value?

Optimising compilers build an intermediate representation, compute value
lifetimes and colour an interference graph. Simple compilers for register VMs
— Lua's among them — use a much simpler scheme that works because expression
temporaries have **nested lifetimes**: the temporary for `2 * 3` in
`1 + 2 * 3` is created after, and dies before, the one holding `1`. So
registers can be allocated like a **stack**:

- `free_reg` is the first unused register;
- *allocate* returns `free_reg` and increments it;
- *free* resets `free_reg` back down.

valo generates code **directly from the AST** in one pass (no intermediate
representation; plan D1), like Lua's `lcode.c`.

## 6.2 The core idea: compile into a destination

The central function has the signature

```rust
fn expr(&mut self, e: &Expr, dst: u8)      // compiler/src/codegen.rs, FuncState::expr
```

meaning "emit code that leaves the value of `e` in register `dst`". The
*caller* chooses the destination. Each expression kind is one case:

| Expression | Emitted code |
|------------|--------------|
| `Int(n)`, small | `LOADI dst n` |
| `Int(n)`, large | `LOADK dst k` with `consts[k] = n` |
| `-e` | code for `e` into `dst`; `NEG dst dst` |
| `-<literal>` | folded: `LOADI dst -n` (no `NEG`) |
| `a op b` | code for `a` into `dst`; `t = alloc()`; code for `b` into `t`; `OP dst dst t`; `free_to(t)` |

The binary rule is the whole register-allocation strategy: the left operand
reuses the destination, the right operand gets one fresh temporary, which is
released as soon as the operator has consumed it.

### Worked example

`(1 + 70000) * -3 - 2 ** 2` (the snapshot test `three_op_expression`):

```text
expr(Sub, dst=0)
├─ expr(Mul, dst=0)
│  ├─ expr(Add, dst=0)
│  │  ├─ expr(1, dst=0)           LOADI 0 1
│  │  ├─ t=1: expr(70000, 1)      LOADK 1 0        ; 70000  (constant pool)
│  │  └─                          ADD   0 0 1      free R1
│  ├─ t=1: expr(-3, 1)            LOADI 1 -3       (folded negation)
│  └─                             MUL   0 0 1      free R1
├─ t=1: expr(Pow, dst=1)
│  ├─ expr(2, 1)                  LOADI 1 2
│  ├─ t=2: expr(2, 2)             LOADI 2 2
│  └─                             POW   1 1 2      free R2
└─                                SUB   0 0 1      free R1
                                  LOADUNIT 0 ; RETURN 0
```

Three registers in total (`regs=3` in the disassembly): `R2` was needed only
because the right operand of `-` was itself a binary expression. In general,
this scheme needs one register plus one per level of the deepest chain of
nested *right* operands. Sethi and Ullman showed that evaluating the
"heavier" subtree first can need fewer registers; valo keeps strict
left-to-right evaluation because the order of side effects (M4+) must match
the source.

## 6.3 FuncState

File: [`compiler/src/codegen.rs`](../compiler/src/codegen.rs)

```rust
struct FuncState {                  // Lua: FuncState in lparser.h
    proto: Proto,                   // the prototype being filled (chapter 5)
    free_reg: u16,                  // first free register
    max_reg: u16,                   // high-water mark → proto.num_regs
    int_consts: HashMap<i64, u16>,  // value → constant index (deduplication)
    diags: Vec<Diagnostic>,         // code generation errors
}
```

| Method | Job |
|--------|-----|
| `new(name, def_span)` | empty prototype |
| `emit(instr, span)` | append to `proto.code` **and** `proto.spans` — the only way instructions are added, so the span table is always complete |
| `alloc(span) -> u8` | next register; E0301 if more than 255 are needed |
| `free_to(reg)` | release `reg` and everything above |
| `int_const(n, span) -> u16` | index of `n` in the pool, adding it once |
| `load_int(n, dst, span)` | `LOADI` if `n` fits in `i16`, else `LOADK` |
| `expr(e, dst)` | the recursive compiler (6.2) |
| `finish(self)` | set `num_regs`, return the `Proto` or the errors |

The register counters are `u16` although registers are `u8`: that way
`free_reg` can reach 255 and be detected as "too many" without overflowing.
`alloc` keeps returning a dummy register after reporting E0301 so that code
generation finishes and reports all errors at once.

### Compiling the program — `compile`

```rust
pub fn compile(program: &ast::Program, sm: &SourceMap, opts: Options)
    -> Result<Program, Vec<Diagnostic>>
```

1. Create the `<main>` `FuncState`.
2. For each top-level expression: allocate `R0`, compile into it, emit `ECHO`
   if `--echo` was given, free `R0` again.
3. Emit `LOADUNIT r; RETURN r` (every function returns a value; a script's
   result is `()`).
4. `finish()` and wrap in a `Program` with the file name and line index.

## 6.4 Small optimisations

- **Constant pool deduplication**: `100000 + 100000` stores `100000` once
  (test `constants_are_deduplicated`).
- **Immediate operands**: integers −32768…32767 go directly into the
  instruction (`LOADI`), avoiding a memory access to the pool.
- **Negative literal folding**: `-3` becomes `LOADI 1 -3` rather than
  `LOADI 1 3; NEG 1 1`. The tree is `Unary(Neg, Int(3))`; the fold is safe
  because the lexer only produces non-negative literals ≤ `i64::MAX`, whose
  negation always fits.

These are *peephole-style* decisions made locally while emitting. Anything
more (constant folding of whole expressions, dead code removal) would be the
job of a separate pass, not planned for the MVP.

## 6.5 What codegen does *not* do

- It does not check types: `1 + ()` cannot be written yet, but in general
  type errors are found at runtime by the VM (`arith`, chapter 7) until static
  checks arrive (post-MVP).
- It does not decide *meaning* of operators: `IDIV` is emitted for `//`; what
  `//` does with negative numbers is defined once, in the VM.
- True division `/` is parsed but rejected here with E0009, because it
  always produces a float and floats arrive in M3.

## 6.6 Where to change things

| Task | Where |
|------|-------|
| compile a new expression kind | `FuncState::expr`: new `match` arm |
| map a new operator to an opcode | the `let opcode = match op { … }` in the `Binary` arm |
| top-level `let` (M2) | `compile`: new `Item` arm; locals get registers via `alloc` and stay allocated until their block ends |
| conditionals (M3) | emit `TEST` + `JMP` with placeholder offsets, then patch them once the target is known ("backpatching") |
| functions (M4) | a stack of `FuncState`s; `CLOSURE` in the parent |
| point runtime errors at the operator | pass `*op_span` instead of `e.span` to the arithmetic `emit` |

## 6.7 Exercises

1. Compile `1 + (2 + (3 + 4))` and `((1 + 2) + 3) + 4` with
   `valo check f.vl --dump bytecode`. Compare `regs=`. Why do they differ?
2. Implement constant folding for `Binary` with two `Int` children (compute at
   compile time with `valo_vm::vm::int_arith`). Which runtime errors must you
   preserve, and how? (Hint: `1 // 0` must still fail at runtime, or become a
   compile error — a language design decision.)

## 6.8 Further reading

- *The Implementation of Lua 5.0*, §6 (register allocation in the compiler).
- Lua 5.4 source: `lcode.c` — `luaK_exp2nextreg`, `freereg`, `luaK_code`.
- *Crafting Interpreters*, ch. 17 (code generation for a stack machine — a
  useful contrast).
- Sethi & Ullman, "The Generation of Optimal Code for Arithmetic
  Expressions", JACM 1970.
