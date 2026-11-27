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
pub fn compile(program: &ast::Program, resolved: &Resolved, sm: &SourceMap, opts: Options)
    -> Result<Program, Vec<Diagnostic>>
```

1. Create the `<main>` `FuncState`.
2. For each top-level expression: allocate `R0`, compile into it, emit `ECHO`
   if `--echo` was given, free `R0` again. For each top-level `let`: compile
   the initialiser into a temporary and store it with `SETGLOBAL`.
3. Emit `LOADUNIT r; RETURN r` (every function returns a value; a script's
   result is `()`).
4. `finish()` and wrap in a `Program` with the globals table, the file name
   and the line index.

## 6.4 Names: the resolver, globals and locals

File: [`compiler/src/resolve.rs`](../compiler/src/resolve.rs)

Before code generation, the **resolver** decides what every name refers to.
It walks the tree with a stack of scopes (the top level, then one per
indented block) and records, keyed by `NodeId`:

- for every `let`: the slot it defines — a **global** (`Res::Global(GlobalId)`)
  at top level, a **local** (`Res::Local(BindingId)`) inside a block;
- for every name: the slot it reads.

Shadowing (syntax.md S5) falls out of the order of work: the initialiser of
`let x = x + 1` is resolved *before* the new `x` is pushed, so it reads the
previous one; the new binding gets a new slot. Lookup searches the innermost
scope first and, inside a scope, the newest binding first. Errors: E0201
undefined name (with a "did you mean" from an edit-distance search over the
visible names), E0203 a top-level name used above its `let`, E0206 `_` used
as a value. `valo check f.vl --dump resolved` prints the result.

Code generation then only looks names up:

- a **global** is an index into the VM's globals array: `SETGLOBAL r g`
  after the `let`, `GETGLOBAL dst g` at each use. Top-level bindings are
  globals (plan D4) so that functions (M4) can reach them by index;
- a **local** lives in a register: the `let` allocates the next free register
  and compiles the initialiser straight into it (`FuncState::locals` maps the
  binding to it); a use is `MOVE dst r`, or nothing if it is already in `dst`.

## 6.5 Blocks: registers live as long as their scope

`FuncState::block(items, dst)` remembers `free_reg`, gives each `let` the next
register, compiles a middle expression line into a temporary, the last line
into `dst`, and finally releases everything above the remembered mark. The
next block reuses the same registers — the test `block_registers_are_reused`
checks `regs=4` for two sequential blocks against a hand count. Because
bindings are immutable, a local's register is written exactly once.

## 6.6 Jumps: `if`, `and`/`or`, comparison chains

A conditional jump is Lua's pair `TEST A B; JMP sJ`: `TEST` skips the `JMP`
unless `R[A] == B`. The jump distance is unknown when the `JMP` is emitted, so
`emit_jmp` writes a placeholder and returns its index, and `patch_here`
fills in the offset once the target is reached (**backpatching**; offsets
beyond ±8M instructions are E0301).

```text
if c then a else b            a and b                  a < b <= c
  c → dst                       a → dst                  a → dst;  b → t1
  TEST dst 0                    TEST dst 0               LT dst dst t1
  JMP else                      JMP end                  TEST dst 0
  a → dst                       b → dst                  JMP end
  JMP end                       [TEST dst 0; JMP +0]     c → t2
else: b → dst                 end:                       LE dst t1 t2
end:                                                   end:
```

- The condition uses `dst` itself: both branches overwrite it.
- `or` is the same as `and` with `TEST dst 1` (jump when true).
- `and`/`or` operands must be bools. `TEST` checks the left one; the right
  one is checked by `TEST dst 0; JMP +0` (both outcomes continue at the same
  place) unless it is a bool by construction (a comparison, `not`, a bool
  literal, another `and`/`or`).
- In a chain each operand is evaluated once, left to right, and a false link
  skips the rest — so `5 < 1 < 1 // 0` is `false`, not an error.
- `a > b` is emitted as `LT dst b a` (one opcode pair for four operators).
- `assert e` evaluates `e` into a register `t` and emits `ASSERT t 0`. If `e`
  is a single comparison, its two sides go to `t+1` and `t+2` and the
  instruction is `ASSERT t 1`, so a failure can print both values.

## 6.7 Small optimisations

- **Constant pool deduplication**: `100000 + 100000` stores `100000` once
  (test `constants_are_deduplicated`). The key is `ConstKey`: floats by bit
  pattern, so `0.0` and `-0.0` are distinct constants.
- **Immediate operands**: integers −32768…32767 go directly into the
  instruction (`LOADI`), avoiding a memory access to the pool.
- **Negative literal folding** (ints and floats): `-3` becomes `LOADI 1 -3` rather than
  `LOADI 1 3; NEG 1 1`. The tree is `Unary(Neg, Int(3))`; the fold is safe
  because the lexer only produces non-negative literals ≤ `i64::MAX`, whose
  negation always fits.

These are *peephole-style* decisions made locally while emitting. Anything
more (constant folding of whole expressions, dead code removal) would be the
job of a separate pass, not planned for the MVP.

## 6.8 Functions and calls

**One `FuncState` per function.** `compile_fn` compiles a `fn` into its own
`Proto`: each parameter gets the next register (`R0…R(arity-1)`, recorded in
`FuncState::locals` like a `let`), the body goes into the next register, and
`RETURN` returns it. The finished prototype becomes a child of `<main>`
(`Proto::protos`); `CLOSURE r k` turns child `k` into a function value at
runtime.

**Hoisting.** Before any top-level line, `<main>` creates every top-level fn
and stores it in its global slot, so a call may appear above the definition
and fns may call each other in any order:

```text
0000  CLOSURE   0 0    ; <fn sq/1>
0001  SETGLOBAL 0 3    ; sq          (slots 0..2 are the built-ins)
```

**The call window.** For `f(a, b)` into `dst`, `FuncState::call` first
reserves consecutive registers `base, base+1, base+2` (callee and arguments;
`base = dst` when `dst` is the topmost register, which saves a `MOVE`), then
evaluates into them, then emits `CALL base 2`. Reserving first means the
pieces can be evaluated in any order without moving values afterwards: a
call made from a pipe (`x |> f(a)`, rewritten by `desugar.rs` to `f(x, a)`
with `piped = true`) evaluates `x` *before* `f` (Q17); keyword arguments
(M4b) will be evaluated in source order straight into the slots of their
parameters.

**Keyword arguments and defaults** (M4b, plan D28). Parameters with a
default are passed by name. For a call to a known fn the resolver records a
`CallPlan`: which argument fills which parameter. `call` then makes window
slot `k` parameter `k`, evaluates the arguments *in source order* into their
slots, and puts `LOADMISSING` into a slot nobody filled; trailing omitted
parameters are not passed at all (the VM pads them). The callee computes a
missing value itself, in a prologue before its body:

```text
fn f(a: int, b: int = 1, c: int = a * 2)      f(1, c=3)
  JMPIFSET  1 +1   ; b passed? skip              GETGLOBAL 0 f
  LOADI     1 1                                  LOADI     1 1      a
  JMPIFSET  2 +3   ; c passed? skip              LOADI     3 3      c (slot 3)
  MOVE 2 0; LOADI 3 2; MUL 2 2 3                 LOADMISSING 2      b omitted
  … body …                                       CALL      0 3
```

Defaults are compiled callee-side (not copied to every call site) because
they may use earlier parameters, must work for calls through a function
value, and are evaluated anew at each call.

```text
print(sq(3))             5 |> add(1)  into R0
  GETGLOBAL 0 print        LOADI     1 5      piped value first
  GETGLOBAL 1 sq           GETGLOBAL 0 add    then the callee
  LOADI     2 3            LOADI     2 1      then the other arguments
  CALL      1 1            CALL      0 2
  CALL      0 1
```

**Tail calls** (M5). `FuncState::expr_at(e, dst, tail)` carries one more
fact: whether `e`'s value is the function's result. That holds for the fn
body, for both branches of an `if` in tail position and for the last line
of a block in tail position — not for `and`/`or` operands, comparisons or
anything in `<main>`. A call in tail position is emitted as `TAILCALL`,
which makes the callee *replace* the current frame:

```text
fn loop(n: int, acc: int) -> int =                 GETGLOBAL 2 loop
    if n == 0 then acc else loop(n - 1, acc + n)   … arguments …
                                                   TAILCALL  2 2
```

So a loop written as recursion runs in one frame; `1 + loop(…)` is not a
tail call (the addition still needs the result) and stays `CALL`.

## 6.9 What codegen does *not* do

- It does not check types: `1 + 2.0` compiles, and the VM reports R0201
  (`arith`, chapter 7). Type errors are found at runtime until static checks
  arrive (post-MVP).
- It does not decide *meaning* of operators: `IDIV` is emitted for `//`; what
  `//` does with negative numbers is defined once, in the VM.

## 6.10 Where to change things

| Task | Where |
|------|-------|
| compile a new expression kind | `FuncState::expr`: new `match` arm |
| map a new operator to an opcode | the `let opcode = match op { … }` in the `Binary` arm |
| how names are loaded | `FuncState::name` (`GETGLOBAL` / `MOVE`); what they mean: `resolve.rs` |
| a new branching construct | `emit_jmp` + `patch_here`; see `if_expr`, `logic`, `compare` |
| register lifetime of locals | `FuncState::block` |
| how a fn is compiled | `compile_fn` (parameters, body, `RETURN`) |
| how calls are compiled | `FuncState::call` (the argument window) |
| point runtime errors at the operator | pass `*op_span` instead of `e.span` to the arithmetic `emit` |

## 6.11 Exercises

1. Compile `1 + (2 + (3 + 4))` and `((1 + 2) + 3) + 4` with
   `valo check f.vl --dump bytecode`. Compare `regs=`. Why do they differ?
2. Implement constant folding for `Binary` with two `Int` children (compute at
   compile time with `valo_vm::vm::int_arith`). Which runtime errors must you
   preserve, and how? (Hint: `1 // 0` must still fail at runtime, or become a
   compile error — a language design decision.)
3. `a + b` with two locals emits `MOVE 0 1; MOVE 3 2; ADD 0 0 3`. Change the
   `Binary` arm so that an operand which is a local is used from its own
   register (`ADD 0 1 2`). Which snapshot changes?

## 6.12 Further reading

- *The Implementation of Lua 5.0*, §6 (register allocation in the compiler).
- Lua 5.4 source: `lcode.c` — `luaK_exp2nextreg`, `freereg`, `luaK_code`.
- *Crafting Interpreters*, ch. 17 (code generation for a stack machine — a
  useful contrast).
- Sethi & Ullman, "The Generation of Optimal Code for Arithmetic
  Expressions", JACM 1970.
