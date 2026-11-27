# Chapter 7 — The Virtual Machine

## 7.1 Theory: fetch, decode, execute

A bytecode interpreter is a loop that imitates a CPU:

```text
loop:
    instr = code[pc]; pc += 1          fetch
    op, A, B, C = decode(instr)        decode
    switch op: ...                     execute
```

Two pieces of state matter:

- the **program counter** `pc`: index of the next instruction;
- the **registers**: here, slots of one growable array of values, the
  **value stack**. Each function activation (a **frame**) owns a contiguous
  window of it; register `R[i]` of a frame is `stack[base + i]`.

```text
value stack:  [ fn slot | R0 | R1 | R2 | ... ]
                 base-1   ^base
```

Slot `base - 1` holds the function being run (a Lua convention). A callee's
window starts right after its function slot, where the caller put the
arguments, so arguments are passed without copying (section 7.6). `<main>`
is the first frame, with `base = 1`.

## 7.2 Values

File: [`vm/src/value.rs`](../vm/src/value.rs)

```rust
pub enum Value {
    Unit,                  // ()  — also the initial content of every register
    Bool(bool),            // true / false
    Int(i64),              // valo int
    Float(f64),            // valo float
    Closure(Rc<Closure>),  // a valo fn: Closure { proto, captures }
    Native(NativeId),      // a built-in: index into builtins::NATIVES
}
```

Each register holds one `Value`. Rust's `enum` is a tagged union: the tag
says which variant it is, so the VM can check operand types at runtime
(**dynamic typing** at the VM level; valo's annotations add checks on top in
M10). Functions are values: `Closure` (compiled code plus captured values;
captures stay empty until M6) and `Native` (a built-in), both of type
`function`, printed `<fn fact/1>` / `<native print/1>`. Later variants:
strings, `none`, `Ok`/`Err` (M7), lists (M8). Heap values will be shared with
`Rc` — safe because valo values are immutable, and enough for memory
management because immutable values in a strict language cannot form cycles
(plan D3, feasibility report "reference counting, not a tracing GC").

`Value::type_name` gives the lowercase name used in messages (`int`,
`float`, `bool`), and `impl Display` the printed form (`42`, `()`, `true`).
Floats print with Rust's `{:?}`, which always shows that they are floats:
`1.0`, `0.30000000000000004`, `1e100`, `inf`, `NaN`.

The derived `PartialEq` on `Value` is for tests only: the language's `==`
(`values_equal` in `vm.rs`) has its own rules, below.

## 7.3 The interpreter

File: [`vm/src/vm.rs`](../vm/src/vm.rs)

```rust
pub struct Vm<'h> {
    stack: Vec<Value>,          // all registers of all frames
    frames: Vec<Frame>,         // call stack; last = running function
    host: &'h mut dyn Host,     // where effects go (7.6)
    options: Options,           // trace_exec
}

struct Frame { proto: Rc<Proto>, base: usize, pc: usize }
```

### Vm::run

1. Clear state; size the stack to `1 + main.num_regs`, all `Unit`.
2. Push the `<main>` frame with `base = 1`, `pc = 0`.
3. Call `execute()`. On error, `unwind()` attaches the stack trace (7.5).

### Vm::execute — the loop

```rust
loop {
    let frame = self.frames.last_mut()...;          // fetch
    let instr = frame.proto.code[frame.pc]; frame.pc += 1;
    let op = instr.op()?;                           // decode
    let ra = base + instr.a() as usize;             // absolute index of R[A]
    match op {                                      // execute
        OpCode::LOADI => self.stack[ra] = Value::Int(instr.sbx() as i64),
        OpCode::LOADK => self.stack[ra] = /* consts[Bx] as a Value */,
        OpCode::ADD | OpCode::SUB | ... => self.stack[ra] = arith(op, &R[B], &R[C])?,
        OpCode::NEG => ...,
        OpCode::ECHO => self.host.write_stdout(...)?,
        OpCode::RETURN => return Ok(value),
        other => return Err(internal("opcode … not implemented yet")),
    }
    if trace_exec { print one trace line }
}
```

Notes:

- Rust's `match` on a dense `#[repr(u8)]` enum typically compiles to a jump
  table, the same dispatch technique as C's `switch` in `lvm.c`.
- `pc` is incremented *before* executing, so jumps are relative to the next
  instruction, and an error at instruction `i` is found at `pc - 1`.
- Every operation that can fail returns `Result`, and `?` exits the loop with
  the error. The VM never panics on user input; panics (`expect`) are
  reserved for internal invariants.
- Unimplemented opcodes produce `R0901` (internal error). The compiler never
  emits them, so this only happens with corrupt bytecode.

### --trace-exec

With `valo run file.vl --trace-exec`, `trace_instr` prints every executed
instruction and the register it wrote to stderr:

```text
[<main>] 0002 ADD 0 0 1 | R[0]=3
```

It is the VM's built-in debugger: compare it with `--dump bytecode` to see
exactly what happened.

## 7.4 Arithmetic semantics

The meaning of every operator is defined in one place: `arith` (type
dispatch) and `int_arith` (integer rules). Design decisions D8–D9:

| Operator | Rule | Example |
|----------|------|---------|
| `+ - *` | checked: overflow is error R0101, never wraps | `9223372036854775807 + 1` → R0101 |
| `//` | **floor** division (rounds toward −∞), like Python | `-7 // 2 = -4` (C/Rust: `-3`) |
| `%` | **floor** modulo: result has the divisor's sign | `-7 % 3 = 2`, `7 % -3 = -2` |
| `// %` by 0 | error R0102 | `7 // 0` |
| `**` | checked power; negative exponent is R0104 (an int result is impossible) | `2 ** -1` → R0104 |
| `-x` | checked: `-(-9223372036854775807 - 1)` overflows | R0101 |

Why floor semantics? They keep the identity `x == (x // y) * y + x % y` *and*
make `%` useful for cyclic things (`-1 % 7 == 6`, a day-of-week). Rust's
built-in `/` and `%` truncate toward zero, so `int_arith` adjusts:

```rust
OpCode::IDIV => {
    let q = x.checked_div(y)?;                      // truncating, e.g. -7/2 = -3
    if x % y != 0 && ((x < 0) != (y < 0)) { q - 1 } else { q }   // floor: -4
}
```

Edge cases handled explicitly: `i64::MIN // -1` overflows (R0101);
`i64::MIN % -1` is 0 although Rust's `%` would overflow; `(-1) ** huge` and
`1 ** huge` work for exponents that do not fit `u32`.

The property test `floor_div_mod_identity` checks, for random `x` and `y`,
that `(x // y) * y + x % y == x` (computed in 128-bit to avoid overflow) and
that the remainder has the divisor's sign and is smaller in magnitude.

### Floats, true division and mixing (plan D9, D10)

`arith` dispatches on both operand types: int×int goes to `int_arith`,
float×float to `float_arith`. Float arithmetic is IEEE (overflow gives
`inf`, `inf - inf` gives `NaN`) with two Python rules on top: `//` and `%`
round toward −∞ like their int versions, and a zero divisor is R0102 rather
than `inf`. `/` (`DIV`) is the one operator that accepts ints: it converts
them and always returns a float (`7 / 2 == 3.5`, `6 / 3 == 2.0`). Every
other int/float mix is R0201 with a hint — valo never converts silently.

### Comparisons, conditions and globals

- `EQ`/`NE` (`values_equal`): both sides must have the same type, else
  R0201; floats follow IEEE, so `NaN == NaN` is false.
- `LT`/`LE` (`less`): ints and floats (strings in M7). Bools and `()` have no
  order (R0205).
- `TEST`, `ASSERT` and `NOT` require a bool (R0203, R0203, R0201): valo has
  no "truthy" values.
- `JMP` sets `pc` relative to the next instruction.
- `ASSERT` fails with R0701; the CLI adds the asserted source text and, for
  `ASSERT A 1`, the notes `left: …` and `right: …`.
- Globals are `Vec<Option<Value>>`, one slot per top-level `let`, sized from
  `Program.globals`. `None` means "not set yet" (R0302 on read) — an internal
  state, not a language-level nil.

## 7.5 Runtime errors and stack traces

When `execute` returns `Err(e)`, `Vm::unwind` fills `e.trace` with one
`TraceEntry { fn_name, span }` per active frame, innermost first; the span is
`proto.spans[pc - 1]`, the instruction that was executing. The CLI renders it
(chapter 8):

```text
error[R0102]: division by zero: 7 // 0
  --> div_zero.vl:2:1
stack trace (most recent call first):
  0: <main> at div_zero.vl:2:1
```

The trace lists every active call, innermost first — the main tool for
debugging scripts on remote machines.

## 7.6 Calls and frames

```rust
struct Frame { closure: Rc<Closure>, base: usize, pc: usize, tail_calls: u32 }
```

`Vm::frames` is the call stack; the last frame runs. Calls between valo
functions never recurse in Rust: `CALL` pushes a frame and the same loop
continues in the callee, `RETURN` pops it and continues in the caller.

```text
CALL A B   (callee in R[A], arguments in R[A+1..=A+B])

 caller window:  [ R0 … R[A]=g  R[A+1]  R[A+2] … ]
 callee window:              g | R0'    R1'    R2' …     base' = base + A + 1

RETURN X (in g):  stack[base'-1] = R'[X]   → the caller finds the result in R[A]
                  pop frame; cut the stack back to the caller's window
```

- **valo function** (`Value::Closure`): the argument count must be between
  `proto.required` and `proto.arity`, else R0207. Omitted trailing
  parameters are filled with the internal sentinel `Value::Missing`; the
  callee's prologue (`JMPIFSET p skip` + the default expression) replaces
  it before the body runs, so `Missing` never reaches user code (the compiler already rejects most such calls with
  E0205; R0207 catches calls through a function *value*). The stack grows to
  `base' + num_regs` and a frame is pushed.
- **built-in** (`Value::Native`): its `NativeDef` (`builtins.rs`) gives the
  name, parameters and a Rust function `fn(&mut Vm, Vec<Value>) -> Result<…>`.
  The arguments are *moved* off the stack into the `Vec` (plan D12), padded
  with `Missing` for omitted keyword parameters, the
  function runs, and its result goes to `R[A]`. No frame is pushed: an error
  inside a built-in is reported at the call.
- anything else: R0202 "cannot call a value of type int".
- **Limits**: more than `MAX_FRAMES` (200 000) frames or `MAX_STACK` (2^20)
  slots is R0301, a clean error instead of exhausting memory.
- **`RETURN` cuts the stack** back to the caller's window, so values of
  finished calls are freed at once (with `Rc`, a value lives as long as a
  stack slot still points to it).

Globals start empty except the built-in slots: `run` fills them with
`Value::Native(id)` from `GlobalDesc::kind` before `<main>` starts.
`<main>`'s first instructions create the top-level fns (`CLOSURE` +
`SETGLOBAL`), so every fn exists before any top-level line runs.

## 7.7 The host: one door to the outside world

File: [`vm/src/host.rs`](../vm/src/host.rs)

```rust
pub trait Host {
    fn write_stdout(&mut self, s: &str) -> io::Result<()>;
    fn write_stderr(&mut self, s: &str) -> io::Result<()>;
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
```

The VM never touches `std::io` directly. Every effect goes through the
`Host` it was given:

- `StdHost` — the real process streams (used by `valo run`). Stdout is
  buffered (`BufWriter`): flushed before anything is written to stderr and
  by the CLI when the program ends, normally or with an error;
- `TestHost` — collects output in two `String`s (used by unit tests).

This is the seed of a key valo feature from the feasibility report: because
*all* effects pass through one interface, a different host can log them
(`--trace`), pretend to perform them (`--dry-run`), record them to a file
(`--record`) or answer them from a recording (`--replay`). M7 adds stdin and
file access to the trait.

A write failure (e.g. stdout is a closed pipe) becomes R0501 instead of a
crash.

## 7.8 Where to change things

| Task | Where |
|------|-------|
| execute a new opcode | `Vm::execute`, new `match` arm |
| float arithmetic | `float_arith`; type dispatch in `arith` |
| what `==` / `<` accept | `values_equal`, `less` |
| how calls work | `Vm::call` (CALL), the `RETURN` arm, `Frame` |
| add a built-in | `builtins.rs`: a function + an entry appended to `NATIVES` |
| recursion limits | `MAX_FRAMES`, `MAX_STACK` |
| a new effect | method on `Host` + both implementations |
| change number formatting | `impl Display for Value` |

## 7.9 Exercises

1. Run `valo run f.vl --echo --trace-exec` on `2 ** 3 ** 2` and match each
   trace line to the disassembly.
2. Write a VM unit test (in `vm.rs`, using `ProtoBuilder`) that executes
   `NEG` on `i64::MIN` loaded via `LOADK`, and asserts `ErrorKind::IntOverflow`.
3. Why does `Vm::execute` clone the `Rc<Proto>` (`let proto =
   Rc::clone(&frame.closure.proto)`) instead of keeping a reference to the frame?
   (Hint: chapter 1, *Ownership and borrowing*, the "cannot borrow as mutable"
   error.)

## 7.10 Further reading

- *The Implementation of Lua 5.0*, §7.
- *Crafting Interpreters*, ch. 15 "A Virtual Machine".
- Python Language Reference, "Binary arithmetic operations" — the floor
  division definition.
- Lua 5.4 source: `lvm.c` (`luaV_execute`), `luaV_idiv`, `luaV_mod`.
