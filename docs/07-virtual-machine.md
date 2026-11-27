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

Slot `base - 1` holds the function being run (a Lua convention; it will hold
the closure from M4 on). When calls arrive (M4), a callee's window starts just
above the caller's arguments, so arguments are passed without copying. M1 has
only one frame, `<main>`, with `base = 1`.

## 7.2 Values

File: [`vm/src/value.rs`](../vm/src/value.rs)

```rust
pub enum Value {
    Unit,        // ()  — also the initial content of every register
    Int(i64),    // valo int
}
```

Each register holds one `Value`. Rust's `enum` is a tagged union: the tag
says which variant it is, so the VM can check operand types at runtime
(**dynamic typing** at the VM level; valo's annotations add checks on top in
M10). Later variants: `Bool`, `Float` (M3), closures and natives (M4),
strings, `none`, `Ok`/`Err` (M7), lists (M8). Heap values will be shared with
`Rc` — safe because valo values are immutable, and enough for memory
management because immutable values in a strict language cannot form cycles
(plan D3, feasibility report "reference counting, not a tracing GC").

`Value::type_name` gives the lowercase name used in messages (`int`), and
`impl Display` the printed form (`42`, `()`).

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
- `pc` is incremented *before* executing, so jumps (M3) will be relative to
  the next instruction, and an error at instruction `i` is found at `pc - 1`.
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

Type errors (operands that are not both `int`) give R0201 from `arith`. In M1
no program can produce one; with booleans and floats (M3) they become
reachable — and `arith` gets new arms for floats.

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

With function calls (M4) the trace will list every caller — the main tool for
debugging scripts on remote machines.

## 7.6 The host: one door to the outside world

File: [`vm/src/host.rs`](../vm/src/host.rs)

```rust
pub trait Host {
    fn write_stdout(&mut self, s: &str) -> io::Result<()>;
    fn write_stderr(&mut self, s: &str) -> io::Result<()>;
}
```

The VM never touches `std::io` directly. Every effect goes through the
`Host` it was given:

- `StdHost` — the real process streams (used by `valo run`);
- `TestHost` — collects output in two `String`s (used by unit tests).

This is the seed of a key valo feature from the feasibility report: because
*all* effects pass through one interface, a different host can log them
(`--trace`), pretend to perform them (`--dry-run`), record them to a file
(`--record`) or answer them from a recording (`--replay`). M7 adds stdin and
file access to the trait.

A write failure (e.g. stdout is a closed pipe) becomes R0501 instead of a
crash.

## 7.7 Where to change things

| Task | Where |
|------|-------|
| execute a new opcode | `Vm::execute`, new `match` arm |
| floats (M3) | `Value::Float`, `arith` arm `(Float, Float)`, `DIV` opcode, `Const::Float` |
| function calls (M4) | push a `Frame` in `CALL`; in `RETURN`, pop and store into the caller's `R[A]` instead of returning |
| a new effect | method on `Host` + both implementations |
| change number formatting | `impl Display for Value` |

## 7.8 Exercises

1. Run `valo run f.vl --echo --trace-exec` on `2 ** 3 ** 2` and match each
   trace line to the disassembly.
2. Write a VM unit test (in `vm.rs`, using `ProtoBuilder`) that executes
   `NEG` on `i64::MIN` loaded via `LOADK`, and asserts `ErrorKind::IntOverflow`.
3. Why does `Vm::execute` clone the `Rc<Proto>` (`let proto =
   Rc::clone(&frame.proto)`) instead of keeping a reference to the frame?
   (Hint: chapter 1, *Ownership and borrowing*, the "cannot borrow as mutable"
   error.)

## 7.9 Further reading

- *The Implementation of Lua 5.0*, §7.
- *Crafting Interpreters*, ch. 15 "A Virtual Machine".
- Python Language Reference, "Binary arithmetic operations" — the floor
  division definition.
- Lua 5.4 source: `lvm.c` (`luaV_execute`), `luaV_idiv`, `luaV_mod`.
