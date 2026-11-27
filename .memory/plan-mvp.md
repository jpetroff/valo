# valo MVP implementation plan

Entry point for agents implementing the minimal working valo. Dense by design.

- Inputs: `notes/Feasibility Report A Functional Scripting Language in Rust.md` (background), `.memory/syntax.md` (concrete syntax, owned by user; this plan never fixes syntax).
- Guiding model: Lua 5.x VM/runtime (refs: `lvm.c` luaV_execute, `lparser.c`/`lcode.c` FuncState+expdesc+freereg, `ldo.c` luaD_precall/pretailcall, `lfunc.c` upvalues, `ldebug.c` funcname/line info). Deviate only for functional specifics; each deviation is listed in §3.
- Workspace (exists): `compiler/` = crate `valo-compiler`, bin `valo` (clap: `run`, `build`, `check`), modules `span token lexer parser ast resolve codegen diagnostic`; `vm/` = crate `valo-vm`, bin `valo-vm`, modules `opcode chunk value vm error builtins`. Deps already declared: `thiserror`, `ariadne`, `clap`, dev `insta`, `proptest`, `pretty_assertions`. `unsafe_code = forbid`.
- New modules this plan adds (allowed, small): `vm/src/host.rs` (host-function trait, M0), optionally `vm/src/disasm.rs` if `chunk.rs` grows past ~600 lines.
- Naming: plan prose uses capitalised semantic type names (Int, Float, Bool, Str, Unit, List, Option, Result). User-visible spellings (annotations, error messages, disasm type table) follow syntax.md S4: lowercase `int float bool str`, `list[T]`, `T | none` for optional (no wrapper), `T ! error` for Result, literals `true false none`, unit `()` (syntax.md S9–S12). Types live in a separate namespace (annotation positions only); a type name may also be a value (e.g. a conversion fn).
- Syntax decisions already fixed by syntax.md S1–S4 that change semantics here: calls are arity-exact (no auto-currying), explicit `_`-placeholder partial application, pipe feeds the FIRST argument, nullary fns allowed, lambdas are a distinct form (not `->`).
- Syntax rule: when a milestone needs a construct not yet in `syntax.md`, VM/codegen work proceeds; parser + golden `.vl` sources for that construct wait for `syntax.md`. Never invent concrete syntax in code or tests; if forced to stub, mark `// PROVISIONAL-SYNTAX` and record it in `.memory/syntax.md` open items.

## 1. Architecture overview

### 1.1 Pipeline

```
 file.vl ──► span::SourceMap { name, text, line_starts }
   │
   ▼ lexer.rs    Vec<Token{kind, span}>  NEWLINE/INDENT/DEDENT synthesized     [valo check --dump tokens]
   ▼ parser.rs   ast::Program  Box-tree, every node: NodeId(u32) + Span          [--dump ast]
   ▼ resolve.rs  desugar pre-pass (pipe → call, `_`-partial → lambda), then
                 Resolved { uses: NodeId→Res, fns: FnId→FnInfo, globals, known_arity }  [--dump resolved]
   ▼ codegen.rs  valo_vm::chunk::Program { main, protos, globals, types, debug } [--dump bytecode]
   ▼ vm::Vm::run(&Program, &mut dyn Host) -> Result<Value, RuntimeError>
   ▼ main.rs     stdout via Host | render Diagnostic / RuntimeError to stderr | exit code
 every compiler stage: Result<T, Vec<Diagnostic>>; stop after the first stage that reports errors
```

- Dump flag is `--dump <tokens|ast|resolved|bytecode>` on `check` (and `run`); exact flag naming is CLI, not language syntax.
- `valo build` / `valo-vm` (bytecode files) are post-MVP (M11, Q11). Keep `chunk::Program` free of compiler types so it serializes later.

### 1.2 Crate responsibilities

| crate/module | owns |
|---|---|
| vm/opcode | `OpCode` (`#[repr(u8)]`), `Instr(u32)` encode/decode, operand-format table |
| vm/chunk | `Proto`, `Program`, `Const`, `UpvalDesc`, `TypeDesc`, `DebugInfo` (spans, line_starts, names), disassembler |
| vm/value | `Value`, `Closure`, `ErrVal`, `Display`, equality/ordering |
| vm/vm | `Vm`, `Frame`, dispatch loop, call protocol, `call_value` re-entry |
| vm/error | `RuntimeError{kind, message, trace}`, `ErrorKind`, codes `R####` |
| vm/builtins | `NativeDef` registry (`NATIVES: &[NativeDef]`), native impls |
| vm/host (new) | `trait Host` (print/read_line/read_file/write_file), `StdHost`, `TestHost` |
| compiler/span | `FileId`, `Span{start,end}` (u32 byte offsets), `SourceMap::line_col` (1-based line, col in chars) |
| compiler/token, lexer | `TokenKind`, hand-written lexer, indent stack |
| compiler/ast | `Program`, `Item`, `Expr`, `TypeExpr`, `NodeId`; S-expr printer for snapshots |
| compiler/parser | recursive descent + Pratt; error recovery to NEWLINE/DEDENT |
| compiler/resolve | desugar (pipe, placeholder partial; may split into `desugar.rs`), scopes, binding ids, capture analysis, global table, builtin seeding, static arity check |
| compiler/codegen | `FuncState` (register allocator, jump patching, const dedup), tail-position flag |
| compiler/diagnostic | `Diagnostic{code, severity, message, primary: Span, labels, notes}`, ariadne renderer, runtime-error renderer |
| compiler/main.rs | CLI; drives pipeline; exit codes (D16) |

## 2. Core data structures (target shape at end of MVP; introduced incrementally)

### 2.1 Instruction encoding (D2)

```
 bit 31            24 23            16 15             8 7              0
     +---------------+---------------+---------------+---------------+
iABC |       C       |       B       |       A       |    opcode     |
iABx |           Bx (u16)            |       A       |    opcode     |
iAsBx|           sBx (i16, 2's c.)   |       A       |    opcode     |
isJ  |                 sJ (i24, 2's complement)      |    opcode     |
     +---------------+---------------+---------------+---------------+
```

- `#[repr(transparent)] struct Instr(u32)`; ctors `abc/abx/asbx/sj`; getters `op() a() b() c() bx() sbx() sj()`; `OpCode::try_from(u8)`.
- Deviation from Lua 5.4 (7-bit op, k-bit, 17-bit Bx): byte-aligned fields, simpler decode, no RK/k operands in MVP. Limits: 255 registers/proto (Lua: 255), 65535 consts/proto, jumps ±8M instrs. Overflow → compile error E0301.
- Jumps relative to next instr: `pc += sJ`.

### 2.2 Opcode table (full MVP set)

| op | fmt | semantics | M |
|---|---|---|---|
| LOADUNIT A | ABC | R[A] = Unit | M0 |
| RETURN A | ABC | return R[A] (single value, D19) | M0 |
| LOADI A sBx | AsBx | R[A] = Int(sBx) | M1 |
| LOADK A Bx | ABx | R[A] = K[Bx] | M1 |
| ADD/SUB/MUL/IDIV/MOD/POW A B C | ABC | R[A] = R[B] op R[C]; type-directed (D8–D10); ADD also Str+Str, List+List (D11) | M1 (Float: M3, Str: M7, List: M8) |
| DIV A B C | ABC | true division, always Float; Int operands converted (the only implicit Int→Float, D9) | M3 |
| NEG A B | ABC | R[A] = -R[B] | M1 |
| ECHO A | ABC | debug: host.print(display R[A]); emitted only under `run --echo` | M1 |
| MOVE A B | ABC | R[A] = R[B] (Rc clone) | M2 |
| GETGLOBAL A Bx | ABx | R[A] = G[Bx]; R0302 if uninitialized | M2 |
| SETGLOBAL A Bx | ABx | G[Bx] = R[A] (define-once; compiler guarantees) | M2 |
| LOADBOOL A B | ABC | R[A] = Bool(B != 0) | M3 |
| EQ/NE/LT/LE A B C | ABC | R[A] = Bool(R[B] cmp R[C]); GT/GE = swapped LT/LE (D18) | M3 |
| NOT A B | ABC | R[A] = !R[B] (Bool only) | M3 |
| TEST A B | ABC | Bool required (R0203); if R[A] != Bool(B) then pc++ (next instr is the JMP taken when R[A]==B; Lua TEST A k) | M3 |
| JMP sJ | sJ | pc += sJ | M3 |
| CALL A B | AB | R[A] = R[A](R[A+1..=A+B]); B = nargs ≥ 0; must satisfy `required ≤ B ≤ arity` else R0207; VM pads missing trailing params with Missing (M4b) | M4 |
| TAILCALL A B | AB | as CALL, reusing current frame | M5 |
| CLOSURE A Bx | ABx | R[A] = closure(protos[Bx]) capturing per `upvals` (D5) | M4 (captures: M6) |
| GETUPVAL A B | ABC | R[A] = captures[B] | M6 |
| GETSELF A | ABC | R[A] = current closure (stack[base-1]) — local self-recursion | M6 |
| CONCAT A B C | ABC | R[A] = Str(to_str(R[B]) ++ … ++ to_str(R[B+C-1])); N-ary, Str parts unchanged; string interpolation only (D25) | M7 |
| LOADNONE A | ABC | R[A] = none | M7 |
| TRY A B C | ABC | postfix `?`: Ok(v) → R[A] = v; Err → append propagation site, return R[B] from current fn (top level: R0601); other → R0208 (Q19). C=1 lenient: non-Result passes through (top-level discarded expressions, Q23) | M7 |
| NEWLIST A B C | ABC | R[A] = List(R[B..B+C]) | M8 |
| INDEX A B C | ABC | R[A] = R[B][R[C]]: List×Int (negative or ≥ len → R0401; no negative indexing in MVP), Dict×key (missing → R0401), Tuple×Int; message carries index/key + len | M8 (Dict/Tuple: M8b) |
| NEWTUPLE A B C | ABC | R[A] = Tuple(R[B..B+C]) | M8b |
| NEWDICT A B C | ABC | R[A] = Dict from C key/value pairs at R[B..B+2C] (later duplicate key wins) | M8b |
| GETFIELD A B C | ABC | R[A] = R[B].K[C] (K[C] = Str field name); type-directed: Dict → key lookup (missing → R0401); later Record → field, Module → export (shared op, S14) | M8b |
| ASSERT A B | ABC | R[A] must be Bool (R0203); if false → R0701; B=1: R[A+1], R[A+2] hold lhs/rhs of a top-level comparison for the message | M3 |
| LOADMISSING A | ABC | R[A] = Missing (internal sentinel: omitted defaulted arg) | M4b |
| JMPIFSET A sBx | AsBx | if R[A] != Missing then pc += sBx (skips the param's default-expr code in the callee prologue) | M4b |
| CHECKTYPE A Bx | ABx | assert R[A] : types[Bx] else R0204 | M10 |

No CALLHOST opcode: natives are `Value::Native` in globals, invoked by CALL (D14). No CLOSE (D5). No fused compare-jump, no ADDK/ADDI (post-MVP perf).

### 2.3 Prototype / program

```rust
struct Proto {
    name: Rc<str>,               // "fact", "<lambda@3:9>", "<main>"
    arity: u8,                   // total params ≥ 0 (D20); <main> has 0
    required: u8,                // params without default (M4b; == arity before)
    param_names: Vec<Rc<str>>,   // keyword normalisation diagnostics, later CALLKW/spread (M13)
    num_regs: u8,                // frame size; VM ensures stack capacity at call
    code: Vec<Instr>,
    consts: Vec<Const>,          // Const::{Int(i64), Float(f64), Str(Rc<str>)}
    protos: Vec<Rc<Proto>>,      // nested functions (CLOSURE Bx indexes here)
    upvals: Vec<UpvalDesc>,      // UpvalDesc { from_parent_reg: bool, index: u8 }
    ret_type: Option<TypeId>,    // M10; param checks are CHECKTYPE in the prologue
    spans: Vec<SrcSpan>,         // parallel to code; SrcSpan{start:u32,end:u32}
    def_span: SrcSpan,
}
struct Program {
    main: Rc<Proto>,
    globals: Vec<GlobalDesc>,    // { name: Rc<str>, kind: Native(NativeId) | User }
    types: Vec<TypeDesc>,        // M10
    source_name: Rc<str>,
    line_starts: Vec<u32>,       // byte offset of each line; VM computes line/col without compiler
}
```

- `SrcSpan` lives in `vm::chunk`; `compiler::span::Span` converts into it (FileId dropped: single file in MVP).

### 2.4 Values (D3)

```rust
enum Value {
    Unit, Bool(bool), Int(i64), Float(f64),
    Str(Rc<str>),                    // M7
    List(Rc<Vec<Value>>),            // M8
    Tuple(Rc<[Value]>),              // M8b
    Dict(Rc<IndexMap<Key, Value>>),  // M8b; Key = Str | Int | Bool | Tuple-of-keys; insertion order (Q24)
    Closure(Rc<Closure>),            // Closure { proto: Rc<Proto>, captures: Box<[Value]> }
    Native(NativeId),                // u16 into builtins::NATIVES
    None,                            // the `none` literal; optional = plain T or None, no wrapper (S9), M7
    Ok(Rc<Value>),                   // M7
    Missing,                         // internal only: omitted defaulted arg; never escapes the callee prologue (debug_assert in Display/EQ), M4b
    Err(Rc<ErrVal>),                 // ErrVal { payload: Value, context: Vec<Rc<str>>, origin: SiteRef, propagated: Vec<SiteRef> }, M7
}
```

- No nil. Registers are initialised to `Unit` (unobservable filler; compiler never reads an unwritten register).
- `Display` (print) vs `Debug-repr` (inside lists / show of Str quoted). Int decimal; Float via `{:?}` (`1.0`, `0.1`, `1e100`, `inf`, `NaN`); Bool `true/false`; Unit per syntax.md (default `()`); List `[1, 2]`; Str inside containers quoted+escaped; `none`, `Ok(1)`, `Err("x")`; `<fn fact/1>`, `<native print/1>`, `<fn <partial add@4:9>/1>`. Type names in messages lowercase (`int`, `list`, `function`).
- Equality: structural over Unit/Bool/Int/Float(IEEE)/Str/List/None/Ok/Err; functions → R0205. EQ/NE with `none` on either side is allowed for any type (that is how optionals are tested); otherwise no cross-type comparison (D10). Ordering LT/LE: Int×Int, Float×Float, Str×Str (codepoint order); else R0205.
- `T | none | none` = `T | none` (no nesting; known limitation: `get` on a list of optionals cannot distinguish "missing" from "present none", as in TypeScript).

### 2.5 Value stack and call frames

```
value stack: Vec<Value> (one per Vm)

   caller frame: func slot at base-1, window R0..R(num_regs-1)
   ┌────────┬─────┬─────┬─────┬─────┬────────┬────────┬────────┐
   │ f      │ R0  │ R1  │ ... │     │ R[A]=g │ R[A+1] │ R[A+2] │  ← CALL A 2
   └────────┴─────┴─────┴─────┴─────┴────────┴────────┴────────┘
     base-1   ▲base                     │        callee window:
                                        ▼
                                   ┌────────┬─────┬─────┬─────┬─────┐
                                   │ g      │ R0' │ R1' │ R2' │ ... │  R0',R1' = params
                                   └────────┴─────┴─────┴─────┴─────┘
                                     base'-1  ▲base' = base+A+1
   RETURN X  (in g): stack[base'-1] = R'[X]  → caller sees result in R[A]; pop frame
   TAILCALL A B (in g): move R'[A..=A+B] to stack[base'-1..]; pc=0; same Frame record
```

```rust
struct Frame {
    closure: Rc<Closure>,            // natives get no Frame; see NativeMark
    base: usize, pc: usize,
    ret_checks: Vec<TypeId>,            // deduped return-type checks, usually len ≤ 1 (M10)
    tail_calls: u32,                    // frames elided by TAILCALL, for traces (M5)
    reentry: bool,                      // entered via Vm::call_value; run-loop exits on its RETURN (M8)
}
struct Vm { stack: Vec<Value>, frames: Vec<Frame>, globals: Vec<Option<Value>>,
            native_marks: Vec<(NativeId, usize /*frames.len() at entry*/)>, host: &mut dyn Host }
```

- Dispatch loop is non-recursive for bytecode→bytecode calls; Rust recursion only via `call_value` (M8), capped.
- Limits: `MAX_STACK = 1 << 20` slots, `MAX_FRAMES = 200_000`, `MAX_REENTRY = 200` → R0301.
- Internal `Option<Value>` in globals is not a language-level nil.

### 2.6 Error-reporting data flow

```
 lexer/parser/resolve/codegen ── Diagnostic{code E####, primary Span, labels, notes} ──► ariadne render (stderr), exit 2
                                   │
 codegen: for every emitted Instr push span of the responsible AST node
          (BinOp→operator expr, CALL→call expr, CHECKTYPE→annotation+binding, TEST→condition)
                                   ▼
            Proto.spans[pc]   Program.line_starts   Proto.name
                                   │
 VM fault at pc ──► RuntimeError{kind, message}
     unwind: for each Frame (innermost first) + interleaved NativeMark:
        TraceEntry{ fn_name, span: proto.spans[frame.pc-1], tail_calls }   (natives: "<native map>")
                                   ▼
 valo CLI: header "error[R####]: message" + "--> file:line:col"
          + ariadne snippet for innermost bytecode span (needs source text)
          + "stack trace (most recent call first)" list, truncated (top 10 / bottom 5)
 valo-vm (post-MVP): same without snippet (line/col from line_starts)
```

## 3. Decisions (tradeoffs, terse)

| id | decision | rejected alt + why |
|---|---|---|
| D1 | Codegen directly from resolved AST (Lua `lcode.c` style: `compile_expr(e, dst: Reg)`, `FuncState.free_reg` stack discipline). Parser stays separate (snapshots, recovery). | ANF IR: cleaner for optimisation/inference, +1 pass and +1 data model; zero benefit before an optimiser. Revisit with HM inference. |
| D2 | 32-bit instr, 8/8/8/8 fields (§2.1). | Lua 5.4 bit-packing: denser but fiddly; Rust enum `Instr`: 8–16 bytes/instr, not Lua-like, harder to serialize. |
| D3 | `enum Value`, Rc payloads, 16 bytes. | NaN-boxing: perf only, needs unsafe. |
| D4 | Top-level bindings = globals table indexed at compile time (GETGLOBAL Bx). Top-level function defs hoisted: main starts with CLOSURE+SETGLOBAL for every top-level fn, so all top-level fns are mutually recursive (letrec by index, no Rc cycles). Top-level value bindings evaluated in order; use-before-def from top-level code = E0203 at compile time; from fn bodies = runtime R0302. | Lua `_ENV` table: dynamic, hash lookups, no compile-time name errors. |
| D5 | Closures capture by value at CLOSURE time (flat closures), keeping Lua shapes (UpvalDesc, CLOSURE, GETUPVAL) but no open upvalues, no CLOSE, no RefCell. Sound because bindings are immutable and initialised before capture (strict + ordered). Self-recursion of local fns via GETSELF (no self-capture → no cycle). Q4. | Lua open/closed upvalues: needed only for mutation; costs RefCell + open list + CLOSE on every scope exit. |
| D6 | No auto-currying (syntax.md S1). Calls are arity-exact: CALL checks `callee.arity == nargs`, else R0207. Resolver checks statically when the callee is known (top-level fn, native, local named fn, immutable binding whose init is a lambda/partial) → E0205 with hint "use f(a, _) for a partial". Partial application = explicit `_` placeholder, desugared before resolution: `f(a, _, c)` → block { t1 = a; t2 = c; lambda(x) → f(t1, x, t2) } (non-placeholder args evaluated once, before the closure is built; callee also bound to a temp unless it is a plain name, so static arity check + display name survive; multiple `_` → lambda params left-to-right; inner call arity checked like any call). Runtime cost: one ordinary closure per partial; zero VM support (no PAP value, no pending args, no over-application). | Auto-currying with PAP object + over-application: rejected by user; needed PAP splicing, pending_args continuation, and made arity errors undetectable. |
| D7 | map/filter/reduce = native builtins re-entering the VM via `Vm::call_value(f, args) -> Result<Value, RuntimeError>`: pushes callee with `reentry=true`, runs dispatch loop until that frame returns. Errors propagate as Rust `Err`; trace includes `<native map>` mark; depth capped (MAX_REENTRY → R0301). Tail calls inside the callback work (inside the nested loop); tail-calling *through* a native boundary does not (acceptable). O(n) with owned args + `Rc::make_mut`. | Prelude in valo: no re-entry, but list building is O(n²) without move/last-use analysis (accumulator stays referenced by a local). Revisit when lists become `imbl` (O(log n) push) — then prelude in valo is preferred. Continuation-style natives (Lua 5.2 `lua_callk`): needed once effects/yield arrive; overkill now. |
| D8 | Int = i64, checked arithmetic, overflow → R0101. Q1. | wrapping (silent bugs); bigint (dep + slower all paths). |
| D9 | Python operators (S11, Q2=c): `/` = DIV, always Float (Int operands converted — the only implicit Int→Float); `//` = IDIV floor division (Int×Int → Int; Float×Float → floored Float); `%` = MOD floor modulo (sign of divisor); `**` = POW (Int×Int with exp ≥ 0, checked, negative exp → R0104 instead of Python's float; Float×Float `powf`). Division/modulo by zero → R0102 for Int and Float (Python; Q20). Mixed Int/Float for `//`, `%`, `**` → R0201. | one type-directed div op: user picked Python split. |
| D10 | No implicit Int/Float mixing (arith, compare, equality): R0201 with hint "convert with float()/int()" — single documented exception: `/` (D9). Explicit `int_to_float`, `float_to_int` (trunc; NaN/out-of-range → R0103; final names may be `float()`/`int()` per S4 value/type namespace split). Q3. | Lua-style promotion: convenient, conflicts with strict (Pydantic-strict) annotations. |
| D11 | Str = `Rc<str>`, UTF-8, immutable; `str_len` counts Unicode scalars. `+` on Str×Str and List×List reuses ADD (type-directed; mixed types → R0201); CONCAT opcode is reserved for N-ary interpolation. | `Rc<String>`+make_mut: faster repeated append, more API surface; rope: overkill. Separate `++`-style opcode: unnecessary, `+` is the user-facing op. |
| D12 | List = `Rc<Vec<Value>>`; natives take owned args (drained from stack temps) so `Rc::make_mut` mutates in place when unique. | `imbl::Vector`: O(log n) persistent ops, extra dep; switch post-MVP (README already lists imbl). |
| D13 | Annotations checked at runtime by CHECKTYPE (bindings, params in callee prologue so indirect calls are covered) and by `frame.ret_checks` at RETURN (survives TAILCALL, deduped → O(1) space for self-recursion). Plus a compile-time check only when the initialiser is a literal (E0401). Strict: no coercion (Int ∉ Float). Q8, Q9. fn param + return annotations REQUIRED (S13, Q8=b: E0410 from M4); `let`/lambda annotations optional. Type vars (`fn f[T](…)`) and `any`: parsed, accepted, only the outer shape checked (`list[T]` → is a list). | Static checking: needs inference; Phase 2 of feasibility report. |
| D14 | Natives = `NativeDef{name, arity, effect: bool, f: fn(&mut Vm, Vec<Value>) -> Result<Value, RuntimeError>}` in `builtins::NATIVES`; resolver seeds global scope with their names at indices 0..N; effectful natives call `vm.host`. `effect` flag is the runtime hook for the future effect handler and the seed set for post-MVP effect inference (S22: effects inferred, no `io`/`pure` marker, no purity enforcement — nothing in the MVP checks purity). Natives declare `params: &[ParamSpec{name, has_default}]` like protos (keyword-only defaults resolved inside the native when it receives Missing, M4b); positional overloads via arity range (`range(n)`/`range(a, b)`) remain a native-only exception (Q26). Prelude names are flat (S16, Q14 hybrid); desugarings (comprehensions) reference natives by `NativeId`, never by name, so user shadowing of `map` cannot break them. | CALLHOST opcode: separate call path, duplicates arity logic. |
| D15 | Diagnostics rendered with `ariadne` (already a dep), color off when stderr is not a TTY or `NO_COLOR` set → deterministic goldens. Q12. | hand-rolled renderer: full control, ~200 lines. |
| D16 | Exit codes: 0 ok; 1 runtime error; 2 compile error; 64 CLI usage / unreadable source file. Q13. | |
| D17 | Per-instruction span table (8 bytes/instr) + per-program `line_starts`. | Lua 5.4 delta-compressed lineinfo: smaller, only lines; do post-MVP. |
| D18 | Comparisons produce Bool into a register; branches use TEST+JMP. | Lua fused `EQ A B k`+JMP: 1 fewer instr/branch, more complex codegen (expdesc jump lists). Optimise later. |
| D19 | Exactly one return value; CALL has no C/nresults. | Lua multiple results: no use in valo (tuples later). |
| D20 | Nullary functions allowed (arity ≥ 0); call with empty argument list (`read_line()`-style per S1). A nullary fn referenced without a call is just a function value. | Unit-argument workaround: only needed under auto-currying (dropped). |
| D21 | Argument order data-FIRST: `map(xs, f)`, `filter(xs, p)`, `reduce(xs, init, f)` with `f(acc, x)`, `get(xs, i)`, `split(s, sep)`, `join(xs, sep)` — because pipe feeds the first argument (S1). Builtin names provisional; final list → syntax.md/stdlib section. | data-last: suits auto-currying, which was dropped. |
| D24 | Pipe is pure desugar (resolve pre-pass, spans kept): `x |> f(args)` → `f(x, args)` (inserted as first positional, before any keyword args: `cmd |> run(retries=2)` → `run(cmd, retries=2)`); `x |> f` (non-call RHS) → `f(x)`; if the RHS call contains `_`, `x` fills that slot instead and no lambda is created (`x |> f(a, _)` → `f(a, x)`). LHS evaluated first, into a temp, before callee/other args (Q17). Lexer: a line starting with the pipe token continues the previous expression (suppress NEWLINE/INDENT). Zero VM cost. | pipe opcode/runtime: no benefit. |
| D22 | Conditional: both branches required. If syntax.md adds an else-less form, its missing branch yields Unit (not checkable without static types). | else-less = compile error: needs types to know the value is used. |
| D25 | String interpolation (S12): every normal string literal interpolates `{expr}`; `\{` `\}` escape braces; raw `r"…"` = no escapes, no interpolation. Lexer emits `StrStart, StrText, InterpStart, <expr tokens>, InterpEnd, …, StrEnd` with a mode stack (brace depth per interpolation; nested strings allowed; NEWLINE/INDENT suppressed inside). Codegen: parts into consecutive regs, one `CONCAT A B n` (one allocation, Display per part). | desugar to `+`/`to_str` chain: n-1 intermediate allocations, longer bytecode. |
| D26 | Result (S10): `Ok(v)`/`Err(e)` built-in constructors; `?` = TRY opcode (one fused op instead of TESTTAG+JMP+RETURN, because it also records the propagation site). `?` returns from the innermost function (fn or lambda, Rust closure semantics; Q22). Err values carry a context chain (`context(r, msg)` native) plus origin site (where `Err` was constructed) and `?` propagation sites (Zig-style error return trace; Q21). Unhandled Err at top level (`?` in main) → R0601, exit 1, prints payload + chain + sites. MVP Err payload = any value (io errors: Str message); builtin `error` record post-MVP (needs records). `?` binds to the whole pipe stage: `x |> f(a)?` → `f(x, a)?` (pipe desugar runs under `?`). | exceptions: rejected by design; full stack snapshot per Err: O(depth) per construction. |
| D27 | Collections (S14/S15): MVP core = List + indexing + `range` + `get` + list comprehensions (M8). Tuples, dicts, `.key` (GETFIELD), dict comprehensions = M8b, a stretch milestone inside MVP that may slip post-MVP without blocking M9/M10. Comprehensions are pure desugar in the resolve pre-pass: `[e for x in xs]` → `map(xs, x => e)`; `if c` → `filter(…, x => c)` first; each extra `for` clause → `flat_map` around the inner chain; loop vars = lambda params (fresh immutable binding per element); generated lambdas named `<comprehension@l:c>`. Index errors name the access path using the source text of the indexed expression (renderer slices the span): `d.items[3]: index out of range (len 2)`. | dicts in core M8: user's MVP scope lists only List; dict needs a new dep (`indexmap`) + hashing rules. Comprehension opcodes/loops: no VM loops exist; desugar reuses tested natives. |
| D28 | Keyword args + defaults (S19), milestone M4b. Required params positional, defaulted params keyword-only (E0421), required-after-default E0423, keywords only to statically known callees (E0422), unknown keyword E0424 (+did-you-mean), duplicate keyword E0425, missing required arg → E0205. Resolver normalises every keyword call into a full positional arg list in declaration order; omitted defaulted params get LOADMISSING. Defaults are compiled CALLEE-side: prologue per defaulted param `JMPIFSET p skip; <default expr → p>; skip:`, before the CHECKTYPE prologue (so defaults are type-checked too). Dynamic calls (unknown callee) pass positional required args only; CALL/TAILCALL accept `required ≤ nargs ≤ arity` and pad with Missing. Pipe/`_` fill positional params only; keyword args in a `_` partial are evaluated once like other fixed args. | call-site inlining of default exprs: no sentinel, but wrong scope (defaults may reference callee-private names/captures/earlier params), duplicates code per call site, impossible for dynamic calls. |
| D29 | `assert expr` (S25 candidate, provisional for non-test code) enters the MVP in M3: expression of type `()`; never compiled out. If `expr` is a single top-level comparison, codegen keeps lhs/rhs in R[t+1], R[t+2] and emits `ASSERT t 1`; else `ASSERT t 0`. Failure R0701: `assertion failed: <source text of expr>` + `left: …` / `right: …` (Display-repr) + trace. Goldens from M3 on may use `assert` for self-checking programs (empty stdout, exit 0). | wait for `test` blocks (post-MVP, needs imports): goldens would rely on stdout diffs only. |
| D23 | Function display names: named defs → name; lambda bound directly to a binding → binding name; placeholder partial → `<partial f@line:col>`; else `<lambda@line:col>`; top-level → `<main>`. | |

## 4. Testing strategy

- Golden tests: `compiler/tests/golden/mN/<name>.vl` + `<name>.stdout` (required) + optional `<name>.stderr` (absent ⇒ must be empty), `<name>.exit` (absent ⇒ 0), `<name>.stdin`, `<name>.args` (extra `run` flags, e.g. `--echo`), `<name>.files/` (fixtures).
- Runner `compiler/tests/golden.rs`: one `#[test]` iterating all files, spawns `env!("CARGO_BIN_EXE_valo") run <name>.vl` with cwd = fresh temp dir containing the `.vl` + fixtures (relative path ⇒ stable paths in stderr), env `NO_COLOR=1`; collects all failures, prints `pretty_assertions` diffs. `VALO_BLESS=1` rewrites expectations; `VALO_GOLDEN=<substr>` filters. Bless only after manual review of the diff.
- From M3, goldens may self-check with `assert` (D29) instead of printing every intermediate value; failing asserts give R0701 with both sides.
- Golden programs are written once `syntax.md` covers their constructs; below, each exit test describes the program abstractly + exact expected output.
- insta snapshots (unit tests inside each module): tokens (`lexer`), AST S-expr (`parser`), resolved dump (`resolve`), disassembly (`codegen` end-to-end from source), rendered diagnostics (`diagnostic`), rendered runtime errors (`compiler` integration). Snapshot input = small inline source strings.
- VM-only unit tests (`vm/`): hand-assembled protos via a tiny builder (`ProtoBuilder`), run with `TestHost` capturing stdout; independent of syntax — use these to land VM features before parser support exists.
- proptest: `Instr` encode/decode roundtrip (M0); Int arithmetic ops vs `i64::checked_*`/floor reference (M1); lexer never panics on arbitrary input (M1); display/equality laws (M8).
- Disassembler from M1 (`--dump bytecode`); `run --trace-exec` (stderr: `pc op operands | R[A]=value`) from M1 — debugging aid, never in goldens.
- Each milestone done = `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` all green; previous goldens untouched unless the milestone intentionally changes output (note it).

Disassembly format (snapshot-stable):

```
function <main> arity=0 regs=3 consts=1 upvals=0 protos=1   ; file.vl:1:1
  0000  [1:1]   CLOSURE   0 0        ; <fn fact/1>
  0001  [1:1]   SETGLOBAL 0 3        ; fact
  0002  [5:1]   GETGLOBAL 1 3        ; fact
  0003  [5:6]   LOADI     2 10
  0004  [5:1]   CALL      1 1
  0005  [5:1]   RETURN    1
constants: (none)
  function fact arity=1 regs=4 ...   ; nested protos indented
```

## 5. Error code catalogue (grows per milestone; codes stable once in a golden)

| range | stage | MVP codes |
|---|---|---|
| E00xx | lexer | E0001 unexpected character, E0002 Int literal out of range, E0003 inconsistent dedent, E0004 unterminated string, E0005 invalid escape, E0006 tab/space mix (per syntax.md), E0007 unterminated interpolation (missing `}`), E0008 empty interpolation `{}`, E0009 reserved syntax not supported yet (backtick cmd, `re"…"`, `=~`; also used by parser/codegen for constructs of later milestones), E0010 malformed number literal (leading zeros, bad `_`, bad suffix) |
| E01xx | parser | E0101 unexpected token / expected X, E0102 `if` without `else`, E0103 unclosed delimiter, E0104 block ends with a `let` binding (or `fn` definition), E0105 expression nested too deeply |
| E02xx | resolve | E0201 undefined name (+did-you-mean), E0202 (retired: same-scope rebinding is allowed per Q7=b; keep code unused), E0203 top-level use before definition, E0204 local mutual recursion unsupported (Q16), E0205 wrong number of arguments to known fn (+hint `f(a, _)` partial), E0206 `_` placeholder outside a direct call argument (M2–M4: `_` read as a value), E0207 defined twice (top-level fn, parameter, type variable; M4), E0208 `?` directly inside a comprehension (M8) |
| E03xx | codegen | E0301 limit exceeded (registers/consts/jump/list literal >255 elems) |
| E04xx | types | E0401 literal does not match annotation, E0402 unknown type name, E0403 malformed type, E0410 missing fn param/return annotation (`parameter p of fn grade needs a type annotation, e.g. p: int`) |
| E042x | arguments | E0421 defaulted param passed positionally (`timeout has a default and must be passed by name: timeout=5`), E0422 keyword args to a non-static callee, E0423 required param after defaulted param, E0424 unknown keyword (+did-you-mean), E0425 duplicate keyword; post-MVP: E0420 positional record construction |
| R01xx | arithmetic | R0101 Int overflow, R0102 division or modulo by zero (Int and Float, Q20), R0103 float→int out of range/NaN, R0104 negative Int exponent |
| R02xx | type/value | R0201 operand type mismatch, R0202 not callable, R0203 condition not Bool, R0204 annotation mismatch, R0205 not comparable, R0206 wrong arg type to native, R0207 wrong number of arguments (dynamic callee), R0208 `?` on a non-Result value (incl. `none`, Q19) |
| R03xx | resources | R0301 stack overflow (stack/frames/re-entry), R0302 global used before initialisation |
| R04xx | data | R0401 index out of range / missing dict key (message: access path + index/key + len), R0402 unwrap on `none`, R0403 unwrap on Err |
| R06xx | results | R0601 unhandled Err at top level (payload + context chain + origin/propagation sites) |
| R07xx | assertions | R0701 assertion failed (expr source text, left/right values for a top-level comparison) |
| R08xx | interruption | R0801 run stopped from outside (debugger Stop/terminate; later Ctrl+C) — never printed by valo-dap |
| R05xx | host | R0501 stdout/stderr write failure (file errors are `Err` values, not faults) |

Runtime error output (stderr), shape since M9 (compile errors share header + box):

```
error[R0102]: division by zero: 10 // 0
   ╭─[ div.vl:3:5 ]
   │
 3 │     <source line 3>
   │     ──┬──
   │       ╰──── divisor is zero
───╯
  help: … / left: … / right: …   (notes)
stack trace (most recent call first):
  0: safe_div   at div.vl:3:5
  1: compute    at div.vl:7:3
     (... 4 tail calls elided ...)
  2: <main>     at div.vl:10:1
```

## 6. Milestones

Format per milestone: Goal · Blocks · Tasks (by module) · Opcodes · Data · Tests · Exit · Out of scope.

### M0 — Skeleton, pipeline stub, harness — DONE 2026-11-26

- Goal: `valo run empty.vl` goes through every stage and exits 0; harness + snapshot tooling in place.
- Blocks: empty program.
- Tasks:
  - vm/opcode: `OpCode` enum (all MVP names declared, only LOADUNIT/RETURN implemented), `Instr` + 4 formats.
  - vm/chunk: `Proto`, `Program`, `SrcSpan`, `Const`; `ProtoBuilder` (test helper, `#[cfg(test)]`-or-pub-for-tests).
  - vm/value: `Value::Unit` (+ enum declared with later variants commented/TODO).
  - vm/vm: `Vm::run(&Program, &mut dyn Host) -> Result<Value, RuntimeError>`; loop executes LOADUNIT/RETURN.
  - vm/error: `RuntimeError`, `ErrorKind`, `code()` → `"R####"`.
  - vm/host (new): `trait Host { fn write_stdout(&mut self, s: &str) -> io::Result<()>; }` (extended in M7), `StdHost`, `TestHost` (captures output; used by VM unit tests).
  - compiler/span: `Span`, `SourceMap{name,text,line_starts}`, `line_col(offset)`.
  - compiler/diagnostic: `Diagnostic`, `render(&SourceMap, &[Diagnostic], color: bool) -> String`.
  - compiler/lexer: returns `[EOF]`; parser → empty `ast::Program`; resolve → no-op; codegen → `LOADUNIT 0; RETURN 0`.
  - compiler/main.rs: `run`/`check` read file → pipeline → exit code (D16). `build` stays "not implemented".
  - tests: `golden.rs` runner, `insta` wired.
- Opcodes: LOADUNIT, RETURN.
- Tests: proptest `Instr` roundtrip for all formats incl. sBx = -32768/32767, sJ = ±(2^23-1); golden `m0/empty` (stdout empty, exit 0); integration test: nonexistent file → exit 64, stderr starts `error: cannot read`.
- Exit: `cargo test` green; `valo run` on empty file exits 0.
- Out of scope: any token besides EOF.

> Q12: diagnostic renderer — (a) ariadne, color off when not a TTY/`NO_COLOR` (b) hand-rolled (c) codespan-reporting. Default (a).
> Q13: exit codes — (a) 0/1 runtime/2 compile/64 usage+unreadable file (b) 0/1 only. Default (a).

### M1 — Integer arithmetic end-to-end — DONE 2026-11-26

- Implementation notes: constructs beyond the milestone are lexed and rejected with E0009 (names, floats, `/`, comparisons, calls, strings…); new E0010 malformed number literal; `-<int literal>` folded into LOADI/LOADK; compile diagnostics render with ariadne's own header (`[E0101] Error: …`), runtime errors with `error[R####]: …` — unify in M9; R0901 = internal VM error (bad bytecode / unimplemented opcode); `vm/src/disasm.rs` added; golden runner `compiler/tests/golden.rs`, CLI tests `compiler/tests/cli.rs`.

- Goal: an Int expression is lexed, parsed, compiled, executed; result observable via `run --echo`.
- Blocks: Int literal, unary minus, binary + − × floor-div (`//`) mod (`%`) pow (`**`), grouping, sequence of top-level expressions.
- Tasks:
  - lexer: Int literals (digit-separator rule per syntax.md), operator tokens, parens, `#` comments to end of line (shebang `#!` on line 1 is just a comment; `##` doc comments lexed as comments for now), reserved keywords table incl. MVP-unused `import from as for in def match test mock` (S7, S14–S16, S25; `as` also used by `def … as`; `assert` becomes live in M3), NEWLINE; full INDENT/DEDENT engine now (indent stack; blank/comment-only lines ignored; NEWLINE/INDENT suppressed inside open brackets; DEDENTs flushed at EOF; E0003); E0001, E0002.
  - parser: Pratt expression parser; precedence/associativity table from syntax.md (pow right-assoc, unary minus vs pow per syntax.md); top-level = NEWLINE-separated items; recovery: skip to NEWLINE; E0101/E0103.
  - ast: `Expr::{Lit, Unary, Binary}`, `NodeId`, S-expr printer.
  - codegen: `FuncState{proto, free_reg, max_reg}`; `compile_expr(e, dst)`; LOADI when value fits i16 else LOADK with const dedup (`HashMap<ConstKey, u16>`); one span per instr; `--echo` → ECHO after each top-level expression.
  - vm: dispatch for LOADI/LOADK/arith/NEG/ECHO; checked ops; IDIV/MOD floor semantics; errors with single-frame location.
  - chunk: disassembler; `--dump bytecode`, `--dump tokens`, `--dump ast`; `run --trace-exec`.
- Opcodes: LOADI, LOADK, ADD, SUB, MUL, IDIV, MOD, POW, NEG, ECHO (true division DIV needs Float → M3).
- Data: `Const::Int`; `Value::Int`.
- Tests:
  - insta: tokens of an operator-heavy line; tokens of a nested-indentation snippet (INDENT/DEDENT correctness even though parser ignores blocks yet); AST of add-then-mul precedence, of unary minus applied to a power (binding per syntax.md), right-assoc pow; disasm of a 3-op expression.
  - golden `m1/precedence` (`--echo`): ((2 plus 3) times 4) minus (10 floor-div 3), written with minimal parentheses → `17`; `m1/floor` (`--echo`, 4 exprs): −7 // 2, −7 % 3, 7 % −3, 2 ** 10 → `-4\n2\n-2\n1024`; `m1/overflow`: 9223372036854775807 plus 1 → stderr `error[R0101]: integer overflow …` + location line, exit 1; `m1/div_zero`: Int `//` 0 → R0102 exit 1; `m1/neg_pow`: 2 ** −1 → R0104; `m1/lex_error`: stray character → E0001 snippet, exit 2; `m1/parse_error`: dangling operator → E0101, exit 2; `m1/big_literal`: 2^63 literal → E0002.
- Exit: all goldens pass; `valo check --dump bytecode` prints disassembly.
- Out of scope: Float, names, blocks as expressions, print.

> Q1: Int overflow — (a) checked → R0101 (b) wrapping (c) bigint. Default (a).
> Q2: RESOLVED (c) Python split — `/` always Float, `//` floor div, `%` floor mod, `**` power (syntax.md S11). See D9.

### M2 — Bindings, names, blocks — DONE 2026-11-27

- Implementation notes: shipped in two commits on branch `m2-m3` — top-level `let`/names/globals/resolver first, then blocks together with M3 (user decision: no block syntax of its own; blocks are opened by `if` branches, later `fn` bodies and lambdas). A block ending in `let` is E0104 (user decision; top level may end with `let`). `let _ = e` evaluates and discards; `_` as a value is E0206. `let` lines are not echoed by `--echo`. Did-you-mean uses edit distance with adjacent transpositions, threshold `max(1, len/3)` (`resolve::suggest`, reuse for E0424). `--dump resolved` prints globals/locals tables and every use. The full type-annotation grammar (S4/S20) is parsed here already; M4 only adds E0402/E0410. The "closure captured earlier still sees the old value" part of `m2/rebind` moved to M6 (`m6/rebind_capture`). Parser nesting limit E0105 (`parser::MAX_DEPTH` = 200 recursion levels ≈ 100 nested parentheses) added because deep nesting overflowed test-thread stacks.

- Goal: immutable bindings at top level and inside indented block expressions.
- Blocks: binding (with optional type annotation — parsed into `TypeExpr`, ignored until M10), name reference, block expression (bindings + final expr, value = last expr, nested scopes).
- Tasks:
  - parser: binding item/expr, annotation `TypeExpr` (names + list/fn type forms per syntax.md), block via INDENT…DEDENT.
  - ast: `Expr::{Name, Block}`, `Binding{pattern: Name, ann: Option<TypeExpr>, init}`.
  - resolve: scope stack; `BindingId`; `Res::{Local(BindingId), Global(GlobalId)}`; global table; E0201, E0203; same-scope rebinding (Q7=b) allocates a NEW BindingId/register (or new global slot at top level); RHS resolves against the PREVIOUS binding; `--dump resolved`.
  - codegen: locals live in registers allocated at binding time; block end resets `free_reg` (result already in `dst`); top-level bindings → SETGLOBAL; refs → MOVE/GETGLOBAL (avoid MOVE when compiling a local into its own reg).
  - vm: MOVE, GETGLOBAL (R0302 check), SETGLOBAL; `globals: Vec<Option<Value>>` sized from `Program.globals`.
- Opcodes: MOVE, GETGLOBAL, SETGLOBAL.
- Tests: insta resolved dump with shadowing in nested blocks; disasm showing register reuse after a block ends; golden `m2/bindings` (`--echo`): three top-level bindings combined → `42`; `m2/block_value`: block computing intermediate locals → value; `m2/shadow`: inner shadow doesn't affect outer → `1\n2\n1`; `m2/undefined` → E0201 exit 2; `m2/rebind`: same-scope `let` rebinding using old value in RHS → new value, closure captured earlier still sees old value; `m2/use_before_def` → E0203.
- Exit: goldens pass; max register count in disasm equals hand-computed value for the block test.
- Out of scope: functions, annotation checking, destructuring patterns.

> Q6: program entry — (a) script-style top level, expression results discarded (b) required `main` fn. Default (a).
> Q7: RESOLVED (b) — user chose Rust-style shadowing incl. same scope, bindings introduced by a keyword (syntax.md S5).
> Q8: RESOLVED (b) (syntax.md S13) — fn param + return annotations required (E0410 from M4); `let`/lambda optional; type vars and `any` accepted, outer shape only. Runtime checks stay in M10 (see Reordering notes).

### M3 — Bool, Float, comparisons, logic, conditionals — DONE 2026-11-27

- Implementation notes: `if` grammar = inline `if c then a else b` (no elif) or block form with `:` branches, each branch inline or an indented block; `NEWLINE` + `elif`/`else` after an inline branch continues the `if`; `end_of_line` accepts a just-consumed DEDENT. Missing else → E0102 at the `if`. Comparison chains are one `Compare` AST node. `and`/`or` right operands get a `TEST r 0; JMP +0` bool check unless syntactically bool. Error choices: int/float mixing and cross-type `==` → R0201 with a help note; `<` on bool/unit → R0205; non-bool in TEST/ASSERT → R0203 ("expected a bool, found int"); `not` on non-bool → R0201. Float `//`/`%` floor like Python, zero divisor R0102 for all of `/ // %`; `**` on floats is IEEE `powf` (no Python ZeroDivisionError for `0.0 ** -1.0`). `RuntimeError.notes` carries `left:`/`right:` and help lines; the CLI quotes the asserted source text. `m1/true_div` (expected E0009) was removed; `m3/true_div` replaces it. `--trace-exec` now prints the executed instruction's index even after a jump.

- Goal: branching programs over Int/Float/Bool.
- Blocks: Bool/Float literals, true division, comparison ops incl. chaining (`a < b <= c`, middle evaluated once), logical conjunction/disjunction (short-circuit), negation, conditional expression (block and inline forms per syntax.md S6).
- Tasks:
  - lexer: Float literals (forms per syntax.md), keywords for Bool/logic/conditional.
  - parser: conditional with both branches (D22), logic/compare precedence.
  - codegen: comparisons → EQ/NE/LT/LE (GT/GE by operand swap); conditional: `cond→t; TEST t 0; JMP else; then→dst; JMP end; else: else→dst; end:`; and/or: `a→dst; TEST dst 0|1; JMP end; b→dst; end:`; comparison chain `a op1 b op2 c`: a→r1, b→r2 (evaluated once), `op1 r1 r2 → dst; TEST dst 0; JMP end; c→r3; op2 r2 r3 → dst; end:` (generalises to n links); jump patching helpers `emit_jmp() -> Patch`, `patch_here(Patch)`; E0301 on jump overflow.
  - vm: Float in arith (same-type only, R0201 "cannot add int and float" + conversion hint), DIV (D9: Int operands → Float; zero divisor → R0102), Float IDIV/MOD, EQ/NE/LT/LE, NOT, TEST (R0203), JMP, LOADBOOL; `Value::{Bool, Float}`, `Const::Float`.
- Assert (D29): parser `assert expr`; codegen ASSERT; vm R0701 (message rendered by CLI with source text of the asserted span).
- Opcodes: DIV, LOADBOOL, EQ, NE, LT, LE, NOT, TEST, JMP, ASSERT.
- Tests: disasm snapshot of nested conditional (check jump offsets); golden `m3/compare` (`--echo`) → `true\nfalse\n…`; `m3/short_circuit`: `false` AND (a comparison containing an Int division by zero) → `false`, no error; `m3/float` → `0.30000000000000004\n1.0\ninf\nNaN` (inf/NaN via overflow and inf−inf, not division); `m3/true_div`: 7 / 2, 6 / 3, 7.5 // 2.0, −7.5 % 2.0 → `3.5\n2.0\n3.0\n0.5`; `m3/fdiv_zero`: 1.0 / 0.0 → R0102; `m3/chain` (`--echo`): two 3-link chains, one true, one false → `true\nfalse` (once-only evaluation of the middle operand is tested in M4 `m4/chain_once`); `m3/chain_short`: chain whose first link is false skips evaluating a later operand that would divide by zero → `false`; `m3/mix_error`: Int plus Float → R0201 exit 1; `m3/cond_not_bool`: conditional on Int → R0203; `m3/nested_if`: classify sign of three numbers → `-1\n0\n1`; `m3/assert_ok`: 5 passing asserts → empty stdout, exit 0; `m3/assert_cmp_fail`: failing `a == b` with computed sides → R0701 with `left: 4` / `right: 5`, exit 1; `m3/assert_plain_fail`: failing non-comparison Bool expr → R0701 without left/right.
- Exit: goldens pass.
- Out of scope: match, Int↔Float conversion natives (M4).

> Q3: Int/Float mixing — (a) never implicit (R0201), explicit conversion natives (b) Lua-style promotion. Default (a), with the single exception of `/` (D9, Q2 resolved).
> Q30: `assert` in non-test code — (a) yes from M3, R0701 (b) only in `test` blocks (post-MVP). Default (a).
> Q20: division/modulo by zero — (a) R0102 for Int and Float (Python) (b) R0102 for Int, IEEE inf/NaN for Float. Default (a).

### M4 — Functions, calls, frames, natives, print — DONE 2026-11-27

- Implementation notes (branch `m4-m5`):
  - User decisions: top-level fn/let names follow the Rust item rule (fn hoisted and visible in the whole file; a later `let` shadows it from its line on; two top-level fns with one name → new E0207, also used for duplicate params/type vars). A fn body sees the top-level binding in effect where the fn is written, else the first later `let` of that name (forward reference → R0302 at runtime if read too early). All global slots are numbered in a resolver pre-pass: natives `0..N`, then top-level fns and lets in source order.
  - Pipe desugaring lives in a new `compiler/src/desugar.rs` (between parse and resolve); `--dump ast` shows the pre-desugar tree (`|>` nodes), desugared calls print as `pcall`. `x |> (y |> f)` calls the computed function with `x`.
  - Codegen call protocol: the whole window `[base, base+n]` is reserved before evaluation, so a piped argument is evaluated before the callee (Q17) and M4b keyword args can go straight into their slots. `base = dst` when dst is the top register.
  - VM: `Frame.closure: Rc<Closure>` (main too); RETURN truncates the stack to the caller's window (frees callee values at once); MAX_FRAMES/MAX_STACK → R0301 already checked in CALL (golden in M5). Natives: `NativeDef { name, params: &[ParamSpec], effect, f }`, args moved off the stack; `int`/`float` accept their own type unchanged, other types R0206. Functions compared with `==` → R0205. `StdHost` buffers stdout (`BufWriter`), flushed before stderr writes and by the CLI after `run`.
  - E0205 help names the params: "use a partial: `add(a, _)`". E0402 suggests a type-var declaration only for capitalised names inside a fn, else lists the known types. Type namespace: `int float bool str none any error`, `list[1]`, `dict[2]`, `tuple[1+]`; E0403 for wrong type-arg counts or a generic used bare. `let` annotations are checked too.
  - Keyword args and `_` arguments are parsed (`Arg::{Keyword, Placeholder}`) and rejected with E0009 by the resolver until M4b/M6; nested `fn` → E0009 until M6. Trailing commas allowed in params/args; `()` is a unit expression; `fn … =` body may continue on one indented line.
  - Disasm header gains `protos=N`; nested protos print indented after their parent's constants; resolved dump prints the natives on one line and marks fn globals. Existing snapshots changed only by these and by global indices shifting by 3 (natives).
  - Test change vs. the plan: `m4/fact_overflow` overflows in the *outermost* multiplication, so its trace has 2 entries, not 21; the deep trace is covered by the new `m4/deep_trace` (error at the bottom of a 20-deep recursion → 22 entries; re-blessed in M5 when truncation lands). Extra goldens: `global_uninit`, `dup_fn`, `let_shadows_fn`, `conversions`.

- Goal: top-level named functions (arity ≥ 0), arity-exact calls (static + dynamic check), native registry, real output via `print`, pipe into calls; multi-frame runtime traces.
- Blocks: function definition (top-level, named, 0..n params, param + return annotations REQUIRED per S13, optional type params `[T, …]` (PEP 695, S20; declared before use, undeclared type name → E0402; outer-shape checking only, M10)), call (incl. nullary), pipe (first-argument form only; placeholder form in M6), Unit literal.
- Tasks:
  - lexer: pipe token; NEWLINE suppression before a line starting with the pipe token.
  - parser: function definition incl. type params, `TypeExpr` grammar (full S4/S20 form: names, `list[T]`, `T | none`, `T ! E`, fn types, tuple types `(A, B)`, `any`), call, pipe (`Expr::Pipe`), Unit literal; missing param/return annotation → E0410; `_` in argument position parsed as `Arg::Placeholder` (rejected with "not yet supported" until M6).
  - resolve: desugar pre-pass for pipe (D24); type namespace + type-name resolution (E0402/E0403; type vars scoped to their fn) — annotations stored, NOT checked at runtime until M10; `FnId`, params as locals; record `known_arity` per binding → E0205; seed builtins into global scope (indices 0..N from `vm::builtins::NATIVES`); top-level fns registered before body resolution (D4 hoisting) → all top-level fns visible everywhere.
  - codegen: one `FuncState` per function (stack of FuncStates); params = R0..R(arity-1); main prologue: CLOSURE+SETGLOBAL for each top-level fn; call: callee → R[base], args → R[base+1..], `CALL base n`, MOVE dst if needed.
  - vm: `Frame`; CALL on Closure (arity ≠ nargs → R0207) and Native (owned args `Vec<Value>` drained from temps); RETURN pops frame, writes to func slot; R0202; stack growth `ensure(base+num_regs)`; trace built from frames.
  - vm/host: `StdHost` buffered stdout, flushed at exit, on error exit and before stdin reads.
  - vm/builtins: `print` (any value, Display, newline; returns Unit), `float(i)` Int→Float, `int(f)` Float→Int (trunc, R0103) (value/type namespaces are separate, S4).
  - diagnostic: runtime error renderer (header + location + trace; no snippet yet).
- Opcodes: CALL, CLOSURE (0 captures).
- Data: `Closure`, `NativeDef`, `Frame`, `GlobalDesc`.
- Tests: disasm snapshot of a 2-function program (nested proto listing); golden `m4/hello_int`: print of computed Int → `7`; `m4/calls`: chain of 3 functions → output; `m4/print_returns_unit` → `()` (display per syntax.md); `m4/fact`: non-tail recursive factorial 20 → `2432902008176640000`; `m4/fact_overflow`: fact 21 → R0101 with trace listing 21 `fact` frames + `<main>` (golden re-blessed in M5 when truncation lands); `m4/trace3`: error 3 calls deep → trace with 4 entries, exact line:col; `m4/not_callable`: calling an Int → R0202; `m4/nullary`: nullary fn called twice → output twice; `m4/arity_static`: 2-param top-level fn called with 1 arg → E0205 with partial hint, exit 2; `m4/arity_dynamic`: fn value passed as argument and called with wrong count → R0207 trace, exit 1; `m4/pipe`: chain of 3 pipes into 1- and 2-arg fns incl. continuation lines → value; `m4/pipe_order`: pipe LHS with print side effect evaluated before other args (Q17) → ordered output; `m4/forward_ref`: fn defined after its call site at top level works; `m4/missing_ann`: fn param without annotation → E0410 with example hint, exit 2; `m4/missing_ret`: fn without return type → E0410; `m4/unknown_type`: param typed with an undefined type name → E0402; `m4/chain_once`: comparison chain whose middle operand is a printing fn call → printed once.
- Exit: goldens pass; from now on goldens use `print`, not `--echo` (old M1–M3 goldens keep `--echo`).
- Out of scope: closures, lambdas, `_` partials, tail calls, Str.

> Q17: pipe evaluation order — (a) LHS first, then callee and remaining args left-to-right (b) plain textual desugar order (`f(a, x)` evaluates `a` before `x`). Default (a).
> Q18: RESOLVED (a) by syntax.md S19 (provisional) — static checks (arity, keywords) for top-level/local named fns, natives, constructors, immutable bindings of those.

> Q10: print — (a) any value, Display + newline (b) Str only (c) `print` + `println`. Default (a).
> Q14: RESOLVED hybrid (S16) — prelude flat (`print len map filter reduce flat_map range get int float str Ok Err context …`), everything else module-qualified once imports exist (post-MVP). See Q25 for non-prelude builtins in MVP.
> Q15: RESOLVED (a) — unit displays `()` (syntax.md S9); functions `<fn f/n>`.

### M4b — Keyword arguments and defaults — DONE 2026-11-27

- Implementation notes (branch `m4-m5`):
  - Resolver `plan_call` produces `Resolved::calls: NodeId → CallPlan { slots: Vec<Slot::{Arg(i), Missing}> }` for every call to a known signature (natives and fns share `Sig { params: Vec<(name, has_default)> }`). Trailing omitted slots are dropped: CALL passes fewer args and the VM pads `Missing`; gaps get `LOADMISSING`. Codegen evaluates args in source order straight into their declaration-order slots (window reserved first), so `f(c=show(3), b=show(2))` prints 3 then 2.
  - User decision: a required param passed by name → E0424 "pass `x` by position" (S19 provisional item deferred); it counts as supplied so E0205 is not reported twice. E0421 per positional arg landing on a defaulted param; E0205 only for missing required / too many positional; E0425 shows both occurrences; E0422 on every keyword arg of a dynamic call.
  - Defaults are resolved param by param (a default sees only earlier params) and compiled callee-side: `JMPIFSET i +n; <default → Ri>` per defaulted param (sBx offset, E0301 beyond 32767 instrs). `Proto.required`; disasm adds `required=K` only when K ≠ arity; JMPIFSET comment `; to N if passed`.
  - `Value::Missing` displays `<missing>` (only visible via `--trace-exec`; no debug_assert so tracing a prologue cannot panic). Natives get `args` padded with `Missing` to `params.len()`.
  - Native keyword default (`print(x, end="\n")`) deferred to M7 (needs Str); the native path is the same `Sig` path (unit test: `print(1, end=2)` → E0424).
  - Extra goldens: `kw_order` (source-order evaluation), `kw_required` (E0424).

- Goal: S19 call protocol (D28) for user fns and natives; may merge into M4 if small.
- Blocks: parameter default, keyword argument at call site.
- Tasks:
  - parser: `param: type = expr` in fn headers; `name=expr` call args (after positional args); E0423.
  - resolve: normalise keyword calls for statically known callees (same set as the E0205 check, Q18) → positional list + Missing slots; E0421/E0422/E0424/E0425; pipe inserts as first positional.
  - codegen: LOADMISSING for omitted params; callee prologue JMPIFSET + default expr per defaulted param (default may reference earlier params).
  - vm: `Proto.required`; CALL/TAILCALL range check + Missing padding; `Value::Missing`; natives receive Missing for omitted keyword params.
  - vm/builtins: first keyword default on a native for testing (e.g. `print(x, end="\n")`, provisional).
- Opcodes: LOADMISSING, JMPIFSET.
- Tests: disasm snapshot of a prologue with two defaults; golden `m4b/defaults`: fn with 2 defaulted params called with none, one, both, in swapped keyword order → 4 lines; `m4b/default_uses_param`: default referencing an earlier param → value; `m4b/default_each_call`: default expr calling a printing fn evaluated once per call that omits it; `m4b/positional_default` → E0421 with hint, exit 2; `m4b/required_after_default` → E0423; `m4b/unknown_kw` → E0424 did-you-mean; `m4b/dynamic_omit`: fn with defaults passed as a value and called positionally with required args only → uses defaults; `m4b/dynamic_kw` → E0422; `m4b/pipe_kw`: pipe into a call with keyword args → value.
- Exit: goldens pass.
- Out of scope: `**` spread in calls (M13), required params passed by name (provisional in S19; add when confirmed), lambda defaults (none per S19).

> Q26: positional overloads on natives (`range(n)` / `range(a, b)`) — (a) keep arity ranges as a native-only exception (b) `range(stop, start=0)`-style keyword default (breaks Python reading) (c) two natives (`range`, `range_from`). Default (a).
> Q27: dynamic calls omitting defaulted params — (a) allowed, VM pads Missing (b) dynamic calls must supply every param positionally. Default (a).

### M5 — Recursion depth and proper tail calls — DONE 2026-11-27

- Implementation notes (branch `m4-m5`):
  - Codegen: `FuncState::expr_at(e, dst, tail)`; tail positions = fn body, both `if` branches in tail position, last line of a block in tail position. TAILCALL emits no trailing MOVE (dead code); the function's final `RETURN dst` stays for non-call branches.
  - VM: `tail_call` moves callee+args to `base-1..`, truncates the stack to `base+nargs`, resizes to the new window, pads `Missing` (M4b defaults work through tail calls), replaces `frame.closure`, `tail_calls += 1`. Native in tail position: CALL path then shared `do_return`. MAX_FRAMES/MAX_STACK checks were already in CALL (M4); R0301 carries a `help:` note about accumulators.
  - Trace rendering (in `diagnostic.rs`, VM keeps the full trace): `(... N tail calls elided ...)` under the frame; > 15 entries → first 10, `... N frames omitted ...`, last 5; trace indices are right-aligned to the widest index (existing short traces unchanged).
  - Goldens: no strings yet, so `m5/countdown` prints `0` instead of `done`; extra `m5/tail_native`. `m4/deep_trace` re-blessed (truncated). All m5 goldens together take ~1.5 s in a debug build.
  - Memory check (release, max RSS via getrusage): countdown 10^3 → 9.9 MB, 10^7 → 9.6 MB (0.54 s); non-tail depth 150 000 → 14.3 MB.

- Goal: loops via tail recursion in O(1) stack; clean overflow error for deep non-tail recursion.
- Blocks: tail position (no new syntax).
- Tasks:
  - codegen: `tail: bool` flag threaded through `compile_expr`; tail positions = fn body result, both branches of conditional in tail pos, last expr of block in tail pos (not and/or operands in MVP); emit TAILCALL instead of CALL+RETURN.
  - vm: TAILCALL = move callee+args down to `base-1`, reset pc, replace `frame.closure`, `tail_calls += 1`; tail call to a native = call native then RETURN its result. Limits MAX_STACK/MAX_FRAMES → R0301; trace truncation (top 10 + `… N frames omitted …` + bottom 5) and `(... N tail calls elided ...)` lines.
- Opcodes: TAILCALL.
- Tests: disasm snapshot: tail call in both branches; golden `m5/countdown`: tail-recursive count from 1_000_000 → `done`; `m5/sum_acc`: sum 1..=1_000_000 with accumulator → `500000500000` (goldens run debug builds; keep sizes ≤ 10^6); `m5/even_odd`: mutual top-level recursion on 1_000_001 → `false`; `m5/deep_nontail`: non-tail recursion depth 10^6 → R0301 with truncated trace, exit 1, no Rust panic/abort; `m5/tail_trace`: error after 5 tail calls shows elision note.
- Exit: goldens pass; memory stays flat (`/usr/bin/time` max RSS for countdown 10^7 within 2× of 10^3; manual check, note in PR).
- Out of scope: tail calls across native boundaries.

### M6 — Closures, lambdas, partial application — DONE 2026-11-27

- Implementation notes (branch `m6-m7`):
  - Parser: `ExprKind::Lambda(Lambda { params, body, partial_of })`; `IDENT =>` or `( … ) =>` (`paren_lambda_ahead` token scan); body = `branch()` (inline `expr(0)`, or `=>` NEWLINE INDENT block). Lambda params: optional annotations, `=` default → E0101. A block-bodied lambda cannot sit inside brackets (the lexer drops line breaks there): bind it with `let`. `ast::Program.next_id` lets desugar mint ids; `Call.piped` is now `Option<usize>` (index of the piped argument, evaluated first).
  - Desugar (`Desugar` struct): `_` partial → `Block[let $c = callee (non-name callee only); let $aN = fixed args incl. keyword values (piped one first, then callee, then source order); Lambda($p0.. => call)]` (no block when nothing is fixed). Pipe RHS call with `_` → the FIRST `_` gets the value, more `_` → partial with the piped value fixed (`x |> f(_, _)` ≡ `f(x, _)`).
  - Resolver: `funcs: Vec<FnScope { scopes: Vec<Scope { names, later_fns }>, captures, self_name }>` replaces the `mem::take` save/restore; `Res::{Upvalue { binding, index }, SelfRef(binding)}`; `lookup_in` recursion = Lua `singlevar`; `Resolved::captures: NodeId → FnCaptures { name, span, list: Vec<Capture { binding, from: CaptureFrom::{Local, Upvalue, SelfFn} }> }` (lambda keyed by its Expr id, local fn by its FnDef id). Local fns are bound before their body (self → `SelfRef`, GETSELF). Lambdas and nested fns get a `FnCtx` too (forward refs to later top-level lets allowed, like fn bodies; nested fns inherit type vars). E0204 = a later sibling local fn used from inside another function (else E0201 + "defined later in this block"). Signatures keyed by `SigKey::{Global, Binding}`; `let g = <lambda | partial block>` gets the lambda's signature (named after `g`), so `inc(1, 2)` is E0205. Lambda param `_` binds nothing. `$` names are kept out of `uses` and did-you-mean. >255 captures → E0301.
  - Codegen: `compile_function` shared by top-level fns, local fns, lambdas; the parent fills `Proto::upvals` (`FuncState::closure`). D5 deviation: `UpvalDesc { src: UpvalSrc::{Reg, Upval, Current}, name }` — `Current` lets a lambda inside a local fn capture that fn (no cycle: it points to the fn, never back). Upvalue names are debug info (disasm, DAP Closure scope). `$` temporaries get no `LocalInfo`. Disasm: `upvals=N` in the header only when non-zero, `upvals:` table, GETUPVAL/GETSELF comments.
  - Tooling: DAP Closure scope (`VmView::captures`, `captures_ref(frame) = 3 + 2*frame`, `locals_ref = 2 + 2*frame`; offered only when non-empty; hover searches captures); fixed a race in the `stop_on_entry_then_pause` smoke test. LSP: nothing to wire until L2.
  - Error test note: R0207 for a partial called with the wrong count is raised at the *caller* (the CALL), so the trace's innermost frame is the caller; the message names `<partial f@l:c>`. E0206 help now also mentions call placeholders (`m2/discard_name` re-blessed).
  - Extra goldens: `lambda_block`, `local_fn_defaults`, `self_capture`. Memory (release, max RSS): `partial_loop` 10^3 → 10.2 MB, 10^7 → 10.0 MB (0.78 s).

- Goal: first-class functions: nested named fns, lambdas, captures, explicit `_` partials, pipe into placeholder slot.
- Blocks: lambda (one-line and block body, per syntax.md S3; no defaults), nested `fn` inside `fn` bodies (S21; annotations required like any `fn`; self-recursive via GETSELF), placeholder partial application, pipe into placeholder slot.
- Tasks:
  - parser: lambda (lookahead rule per S3), local function definition inside blocks; accept `Arg::Placeholder`; E0206 when `_` used as an expression elsewhere.
  - resolve (desugar pre-pass): placeholder call → block of temps + lambda (D6); pipe into call with `_` → fill slot (D24); spans of generated nodes = original call span.
  - resolve: per-function free-variable analysis; capture chain through intermediate functions (Lua `singlevar` recursion: each intermediate fn gets an upvalue); `Res::{Upvalue(idx), SelfRef}`; local fn referencing itself → SelfRef; local mutual recursion → E0204 (Q16).
  - codegen: CLOSURE with `UpvalDesc{from_parent_reg, index}` list; GETUPVAL; GETSELF; lambda naming (D23).
  - vm: CLOSURE copies captures (D5). Call protocol unchanged from M4 (arity-exact).
- Opcodes: GETUPVAL, GETSELF (+ CLOSURE captures).
- Diagram (D5, Q4):

```
 Lua 5.x (mutable locals)                      valo MVP (immutable ⇒ closed at creation)
 CLOSURE ─► UpVal::Open(stack_idx) ◄─ shared   CLOSURE ─► captures[i] = clone(
    │        (open list, sorted by idx)             │      if from_parent_reg { R[index] }
    │  scope exit / frame return: CLOSE A           │      else { parent.captures[index] })
    ▼                                               ▼
 UpVal::Closed(value)  (moved off stack)       GETUPVAL A B = clone(captures[B])
 GETUPVAL reads via Rc<RefCell<UpVal>>         no open list, no CLOSE, no RefCell
```

- Tests: golden `m6/rebind_capture` (moved from M2): a closure created before a same-scope rebinding still sees the old value; insta resolved dump showing 3-level capture chain; disasm of CLOSURE with mixed reg/upvalue descriptors; golden `m6/adder`: function returning lambda capturing param → `8`; `m6/capture3`: capture across 3 nesting levels → value; `m6/partial`: 3-arg fn with `_` in each position and with two `_` → same result each time; `m6/partial_eval_once`: placeholder partial whose fixed arg prints a side effect, partial then called 3× → side effect printed once, before the first call; `m6/pipe_placeholder`: `x` piped into a call with `_` in 2nd position → value, no closure in disasm; `m6/compose`: compose two partials → value; `m6/partial_arity_static`: partial bound to a name, called with 2 args → E0205 exit 2; `m6/partial_arity_dynamic`: partial passed to a fn that calls it with 2 args → R0207 trace shows `<partial f@…>`; `m6/placeholder_misuse`: `_` outside a call argument → E0206 exit 2; `m6/local_rec`: local named recursive fn (GETSELF) computing fib 20 → `6765`; `m6/partial_loop`: tail-recursive loop invoked through a partial (tail call from lambda body) 10^6 iterations → flat stack; `m6/local_mutual` → E0204 exit 2.
- Exit: goldens pass; no `Rc` cycle possible by construction (closures never capture themselves; document invariant in `vm/value.rs`).
- Out of scope: local mutual recursion, operator sections (`_` only valid as a direct call argument), named/default args, auto-currying.

> Q4: closure capture — (a) by value at creation, no open upvalues (b) Lua open/closed upvalues. Default (a).
> Q16: local mutual recursion — (a) unsupported, E0204 (b) letrec groups via shared env record. Default (a).

### M7 — Str, interpolation, text I/O host functions, optional (`none`) and Result with `?` — DONE 2026-11-27

- Implementation notes, M7a (strings; branch `m6-m7`, committed separately from M7b):
  - Lexer tokens `StrStart StrText(decoded) InterpStart … InterpEnd StrEnd`; mode stack `Lexer::interps: Vec<Interp { depth, string_start, brace }>` — the `}` whose bracket depth matches the top entry ends the interpolation; `depth` is +1 while one is open. Escapes `\n \t \r \0 \" \\ \{ \} \u{1-6 hex}`, else E0005. A string reaching end of line/file = E0004, but only E0007 when that string sits inside an unclosed `{…}` (`"a {x"`); a line break inside `{…}` = E0007; `{}` / `{ }` = E0008. A lone `}` in text is literal. Raw `r"…"`: no escapes/interpolation, ends at the first `"`. Backtick, `re"`, `=~`, `!~` stay E0009.
  - AST `ExprKind::Str(String)` (no interpolation) / `ExprKind::Interp(Vec<StrPart::{Text, Expr}>)`; codegen `Const::Str` (deduplicated, `ConstKey::Str(Rc<str>)`), `CONCAT dst base n` over consecutive registers (> 255 parts → E0301).
  - VM `Value::Str(Rc<str>)` (`type_name` "str"); `Value::repr()` / `value::quote` quote+escape strings for messages, assert `left:`/`right:`, R0206 messages and the debugger; Display is the plain text. `+` on str×str; str + other → R0201 with an interpolation hint; `==`, `<` by code point.
  - Host: `read_line` (flushes stdout; strips `\n`/`\r\n`), `read_file`, `write_file` on `StdHost`, `TestHost` (scripted `stdin: VecDeque`, in-memory `files`), `DapHost` (files via std::fs, stdin = end of input until D6). The natives that use them land in M7b with `none`/Result.
  - Natives appended: `str`, `str_len`, `trim`; `print` gained `end` (keyword, default `"\n"`, deferred from M4b) — so `print` displays as `<native print/2>` and a dynamic 2-argument call passes `end` positionally (Q27). Every global index shifted by 3 (snapshots re-accepted, only numbering changed).
  - Golden runner: `<name>.files/` fixtures copied next to the program (and skipped when collecting tests). DAP Variables/hover show strings quoted. Grammar: `str_len`, `trim` added to the built-in list.
  - Extra goldens: `print_end`, `str_ops`, `assert_str`.
- Implementation notes, M7b (`none`, results, `?`):
  - Parser: `none` → `ExprKind::None`; postfix `?` = Pratt row `Infix::Try` at `CALL_BP` → `ExprKind::Try`. Desugar: `Pipe(x, Try(rhs))` → `Try(pipe(x, rhs))` recursively (`x |> f? |> g(1)??` works). The compile-time error for a literal `none?` (Q19) was skipped: it is R0208 at runtime like any non-result.
  - Codegen: `LOADNONE`; `e?` → `TRY dst dst 0`; EVERY top-level expression line ends with lenient `TRY dst dst 1` (Q23: static types would be needed to emit it only for results). All `<main>` disassembly snapshots changed by that.
  - VM: `Value::{None, Ok(Rc<Value>), Err(Rc<ErrVal { payload, context, origin: Site, propagated: Vec<Site> }>)}`; `Site { fn_name, span }` lives in `error.rs`; `type_name` of Ok/Err = "result"; Display `none`, `Ok(1)`, `Err("x")` (payload via repr); `Value::describe()` for messages (`int (3)`, `none`). `none == x` allowed for any x; Ok/Err compare by content. TRY: Err → push the `?` site, `do_return` it; in `<main>` (frames.len() == 1) → R0601 with `RuntimeError::trail: Option<Box<ErrTrail>>` (boxed: clippy large_err). `Vm::current_site()` = innermost frame + span of its current instruction (the `Err` built-in's origin); `Vm::host()` gives natives the host.
  - Natives appended (12): `Ok Err context is_ok is_err unwrap or_else parse_int parse_float read_line read_file write_file` — global indices shifted again (+12; snapshots: numbering only). `or_else(x, d)`: `none`/`Err` → d, `Ok(v)` → v, else x. `parse_int`/`parse_float` trim whitespace, accept a sign. File errors are values: `Err("cannot read \"p\": file not found")` (platform-neutral reasons via `io_message`); a failing stdin read is R0501.
  - R0601 rendering (`diagnostic.rs`): `context (outermost first):`, `origin: f at file:l:c`, `propagated by \`?\` in:` list, then the stack trace. DAP exception text adds context + origin; DAP `read_line` returns `none`.
  - New codes: R0208, R0402, R0403, R0601 (catalogue §5 unchanged; chapter 8 table updated).
  - Extra goldens: `toplevel_err_discarded`, `try_in_lambda`.

- Goal: text programs: hello world, interpolated output, stdin line processing, file read/write with `?` propagation and context chains.
- Blocks: string literal (escapes, interpolation `{expr}`, raw strings), string concatenation via `+`, string comparison; `none` literal; Result constructors `Ok`/`Err` (builtins), postfix `?`.
- Tasks:
  - lexer: string mode stack for interpolation (D25), raw strings, escapes incl. `\{ \}`, `none` keyword, `?` postfix token; E0004/E0005/E0007/E0008. Reserve (lex, then reject with E0009 "not supported yet"): backtick command literal start (S23; post-MVP lexing reuses this interpolation mode stack), `re"` regex prefix (S24), `=~` / `!~` tokens. `|` in expression position (cmd pipelines, S23) → E0101 in MVP; `|` stays a type-level token.
  - parser: `Expr::Interp(Vec<Part>)`, `Expr::Try` (postfix, tightest with calls); `none` literal.
  - resolve (desugar): `Pipe(x, Try(call))` → `Try(Call(…x…))` (D26).
  - codegen: `Const::Str` dedup; interpolation → parts in consecutive regs + CONCAT; LOADNONE; `?` → TRY (dst, src).
  - vm: `Value::{Str, None, Ok, Err}`; ADD on Str×Str (else R0201 + hint "use interpolation"); CONCAT (N-ary Display); LOADNONE; TRY (D26; Err origin captured by the `Err` native from the calling frame's current span; propagation sites appended via `Rc::make_mut`); R0601 render path in CLI; EQ/LT on Str; EQ with none.
  - vm/host: extend trait: `read_line() -> io::Result<Option<String>>` (strips `\n`/`\r\n`), `read_file(&str) -> io::Result<String>`, `write_file(&str, &str) -> io::Result<()>`, `write_stderr`. `TestHost` scripted stdin + in-memory FS.
  - vm/builtins (all `effect=true` for I/O): `read_line() -> str | none` (nullary), `read_file(path) -> str ! error`, `write_file(path, content) -> () ! error` (MVP Err payload = io error message Str), `str(x) -> str` (Display; formerly `show`), `str_len(s)`, `trim(s)`, `parse_int(s) -> int | none`, `parse_float(s)`, `Ok(v)`, `Err(e)`, `context(r, msg)` (Ok passes through; Err gets msg pushed on its chain), `is_ok(r)`, `is_err(r)`, `unwrap(x)` (Ok(v) → v, Err → R0403, none → R0402, plain value → itself), `or_else(x, default)` (none → default; else x; provisional name). R0206 for wrong arg types.
- Opcodes: CONCAT (N-ary), LOADNONE, TRY.
- Tests: golden `m7/hello` → `Hello, world!`; `m7/escapes` → tab/newline/quote/unicode escape output; `m7/interp`: two ints and their sum interpolated, plus a nested string literal inside `{}` → `2 + 3 = 5` line etc.; `m7/braces`: escaped braces print literally; `m7/raw`: raw string containing `{x}` and a backslash-n prints verbatim; `m7/interp_unterminated` → E0007 exit 2; `m7/unicode_len`: length of a 5-scalar non-ASCII string → `5`; `m7/cat_stdin`: recursion over nullary `read_line` until it returns `none`, prefixing line numbers (`.stdin` 3 lines) → `1: a\n2: b\n3: c`; `m7/read_fixture`: read `.files/input.txt`, print trimmed → contents; `m7/write_read`: write then read back → contents; `m7/read_missing`: `is_ok` of missing file → `false`; `m7/unwrap_none` → R0402 trace exit 1; `m7/parse`: parse_int on "42" and "x" → `42\nnone`; `m7/optional_eq`: compare optional results with `none` → `false\ntrue`; `m7/concat_mismatch`: str + int → R0201 with interpolation hint; `m7/try_ok`: two `?` on Ok results inside a fn → value; `m7/context_chain`: `inner` returns `Err("bad port")`, `middle` does `inner()?` after `context(…, "parsing port")`, `outer` adds `context(…, "loading config")` and `?`, top level calls `outer()?` → stderr: `error[R0601]: unhandled Err: "bad port"`, context lines `loading config` / `parsing port`, origin `inner at …:line:col`, propagation sites `middle`, `outer`, `<main>`; exit 1; `m7/try_returns_err`: fn using `?` returns the Err to a caller that prints it → `Err("x")`, exit 0; `m7/try_none`: `?` on `none` → R0208 exit 1 (Q19).
- Exit: goldens pass; `StdHost` flushes stdout before reading stdin and on error exit (stdout/stderr ordering in goldens is per-stream, so no interleaving assertions).
- Out of scope: triple-quoted strings (provisional in S12), format specs inside `{}`, indexing/slicing, regex, match (M12), builtin `error` record (needs records).

> Q5: RESOLVED — optional = plain value or `none` (no wrapper, S9); Result = `Ok`/`Err` + postfix `?` + `context` (S10); no match in MVP.
> Q19: `?` on `none` — (a) not allowed: R0208 at runtime (compile-time E-code when operand is literally `none`) (b) allowed: `none` propagates as the function's result (TS-like) (c) converts to `Err("none")`. Default (a). (b) is cheap in TRY but mixes optional/Result typing.
> Q21: Err trace data — (a) origin site + `?` propagation sites (Zig-style) (b) full stack snapshot at `Err` construction (c) context chain only. Default (a).
> Q22: `?` inside a lambda — (a) returns from the lambda (Rust closures) (b) compile error in lambdas. Default (a).
> Q25: non-prelude builtins (`read_file`, `trim`, …) before imports exist — (a) flat globals in MVP, moved to modules later (b) builtin module records via GETFIELD (needs M8b) (c) minimal builtin `import` in MVP. Default (a).
> Q29: auto-wrap of a `T ! error` fn body's final value in `Ok` (syntax Q-S26) — (a) no, explicit `Ok(...)`; R0204 + hint in M10 (b) auto-wrap. Default (a).
> Q23: top-level expression statement whose value is an Err (discarded) — (a) treat as unhandled → R0601 exit 1 (codegen emits TRY on top-level expression results only when Result, via a lenient TRY flag) (b) silently discarded (Q6). Default (a).

### M8 — Lists and higher-order functions — DONE 2026-11-27

- Implementation notes (branch `m6-m7`, two commits: "M8 part 1/2" VM + built-ins, "M8 part 2/2" front end):
  - VM: `Value::List(Rc<Vec<Value>>)`; NEWLIST moves its element registers; INDEX = List × Int in `0..len`, else R0401 (`RuntimeError::access: Option<Box<Access { index, len }>>`) / R0201 (non-int index, non-list). ADD List×List always builds a new list (operands are borrowed registers; no in-place reuse — deviation from "make_mut when left unique"). `values_equal` recurses into lists; lists have no order (R0205).
  - Re-entry (D7): `Frame::reentry`; `Vm::call_value` pushes callee+args at stack top, runs a nested `execute`; `do_return` of a re-entry frame truncates to its slot and returns. `native_marks: Vec<(NativeId, frames.len())>` pushed/popped around every native call (left on error); `unwind` emits `<native map>` (`TraceEntry::native`, no span) only for marks that have frames above them — a built-in that fails itself is not listed (its caller's CALL stays the location, existing goldens unchanged). `MAX_REENTRY = 200` → R0301 "more than 200 nested calls through built-ins". Debug builds need > 2 MiB Rust stack for 200 levels, so the CLI (`main.rs`: whole command on a thread) and valo-dap run the VM on a `VM_STACK_BYTES` = 64 MiB thread.
  - Built-ins appended (14): `len` (list, str scalars), `get` (`none` out of range/negative), `range(a, b?)`, `map`, `filter` (predicate must return bool → R0206), `reduce`, `flat_map` (callback must return a list → R0206), `each`, `contains` (list via `==` rules, or str substring), `append` (`Rc::make_mut`), `reverse`, `lines`, `split` (empty sep → R0206), `join` (str elements only, R0206 names the element). Callbacks iterate shared lists lazily (`for_each_item`, no up-front copy). `big` golden: 0.13 s, 47 MB RSS (release). `builtins::native_id(name)`.
  - Q26 = (a): `ParamSpec::positional` marks an omittable positional param (`range`'s `b`); resolver `Sig::positional` = params passable by position (no E0421 for them); E0205 says "takes 1 to 2 arguments".
  - Parser: `[` prefix → `Parser::list` (literal or comprehension, decided by `for` after the first element); postfix `[` at `CALL_BP`; `in` / `not in` rows at `CMP_BP` that do not chain (E0101 "add parentheses"; also after a comparison chain). Comprehension target = NAME only (`(`→ E0009 destructuring). AST: `List`, `Index`, `In`, `Comprehension { elem, clauses: Vec<Clause::{For, If}> }`, `Builtin(u16)`; `Lambda.partial_of` → `origin: LambdaOrigin::{Source, Partial(f), Comprehension}`.
  - Desugar: `x in xs` → `Call(Builtin(contains), [xs, x], piped: Some(1))` (x first); `not in` → `Not(…)`; comprehension built innermost-out (`map` for the last `for`, `flat_map` around earlier ones, `filter` per `if`); lambdas `<comprehension@l:c>` (all at the `[`). Codegen `Builtin(id)` = `GETGLOBAL id` (native slots never written).
  - E0208 (resolver, `FnScope::comprehension`): `?` directly inside a comprehension lambda (element, `if`, or a later `for`'s list); the first list and user lambdas inside are fine.
  - CLI: R0401 header `<base source>[<index>]: index out of range (len N)` (base = INDEX span text minus its last balanced `[…]`; multi-line base joined by spaces); native trace lines print without location.
  - DAP: `<native map>` label frames (ids from `NATIVE_FRAME_ID_BASE`, odd → no scopes); non-empty lists expand (`LIST_REF_BASE + k`, children `[i]`, registry cleared on resume); `evaluate` returns the reference too.
  - Extras (user-approved): `not in`, `in` on str, `len` on str. Extra goldens: `not_in`, `str_in_len`, `comp_try`, `list_add_eq`, `join_split_lines`, `join_bad`.

- Goal: list literals, indexing, ranges, comprehensions, map/filter/reduce/flat_map with closures and partials.
- Blocks: list literal (empty and non-empty), list concatenation, index expression, list comprehension (one or more `for` clauses, optional `if` filters), membership `x in xs` (provisional; desugar to native `contains(xs, x)`).
- Tasks:
  - parser: list literal (multi-line per syntax.md bracket rules), postfix index, list comprehension inside brackets, `in` operator (comparison precedence).
  - resolve (desugar): comprehensions → map/filter/flat_map (D27); `in` → `contains`.
  - codegen: elements into consecutive regs, NEWLIST; >255 elements → E0301; INDEX.
  - vm: `Value::List`; NEWLIST; INDEX on List (non-Int index → R0201; out of range → R0401); ADD on List×List (new list; make_mut when left is unique); structural equality; display.
  - vm/vm: `Vm::call_value(&mut self, f: Value, args: Vec<Value>) -> Result<Value, RuntimeError>` (pushes callee + args at stack top, frame `reentry=true`, nested run loop returns at that frame's RETURN; handles Closure/Native, arity check R0207; MAX_REENTRY); `native_marks` push/pop around every native call for traces.
  - vm/builtins: data-first (D21): `len(xs)`, `get(xs, i) -> T | none`, `range(n)` / `range(lo, hi)` (half-open, arity range D14), `flat_map(xs, f)`, `contains(xs, x)`, `append(xs, x)` (make_mut), `reverse(xs)`, `map(xs, f)`, `filter(xs, p)`, `reduce(xs, init, f)` with `f(acc, x)`, `each(xs, f)`, `lines(s)`, `split(s, sep)`, `join(xs, sep)`.
- Opcodes: NEWLIST, INDEX (+ ADD for List).
- Tests: proptest: `reduce` sum == Rust sum on random Int vecs (via TestHost-run hand-built protos or compiled source); golden `m8/literal` → `[1, 2, 3]\n[]\n[[1], [2, 3]]\n["a", "b"]`; `m8/pipeline`: sum of squares of evens in range 1..=10 as one pipe chain range → filter → map → reduce (multi-line continuation) → `220`; `m8/closure_map`: map with lambda capturing local → list; `m8/partial_map`: map with a `_`-partial of a 2-arg fn as f, and a partial of `map` itself (`_` in the list slot) applied later → list; `m8/each_print`: print each element → lines; `m8/words`: read fixture, `lines` + `split` + count words → number; `m8/big`: map over range of 10^6 then reduce → value (also proves O(n) `append`/`map`: debug run < 5 s); `m8/err_in_callback`: error inside lambda passed to map → trace `<lambda@…>`, `<native map>`, caller, `<main>`; `m8/reentry_overflow`: recursion through map nesting > MAX_REENTRY → R0301 clean; `m8/index`: `xs[0]`, `xs[2]` → values; `m8/index_oob`: index 3 of a 2-element list reached via a nested expression → R0401 message `<path>[3]: index out of range (len 2)`; `m8/range1`: `range(4)` → `[0, 1, 2, 3]`; `m8/comp`: `[x ** 2 for x in range(10) if x % 2 == 0]` → `[0, 4, 16, 36, 64]`; `m8/comp_nested`: two `for` clauses producing pair sums → flat list; `m8/comp_shadow_map`: user `let map = …` shadowing does not break a comprehension → list; `m8/in`: membership true/false → `true\nfalse`.
- Exit: goldens pass.
- Out of scope: persistent lists (imbl), list patterns, slicing, negative indices, lazy streams, tuples/dicts (M8b), records.

### M8b — Tuples, dicts, `.key` access (stretch; may slip post-MVP)

- Goal: Python-style tuple and dict values for JSON-shaped data; one field-access opcode shared with future records/modules.
- Blocks: tuple literal (incl. 1-tuple form per S14; lambda/tuple disambiguation by S3 lookahead), dict literal (expression keys, empty dict), index on dict/tuple, `.key` on dicts (identifier-shaped keys), dict comprehension.
- Tasks:
  - parser: tuple vs grouping vs lambda; dict literal and dict comprehension inside braces; postfix `.name` (`Expr::Field`).
  - resolve (desugar): dict comprehension → `dict(map(filter(xs, …), x => (k, v)))` (native `dict(pairs)`).
  - codegen: NEWTUPLE, NEWDICT, GETFIELD (field name as `Const::Str`).
  - vm: `Value::{Tuple, Dict}` (dep `indexmap`); hashable keys = Str/Int/Bool/Tuple-of-keys, else R0206; INDEX on Dict/Tuple; GETFIELD type-directed (Dict now; Record/Module later; other → R0201); equality structural (dict order-insensitive), display `(1, "a")`, `{"k": 1}`.
  - vm/builtins: `dict(pairs)`, `keys(d)`, `values(d)`, `get(d, k) -> V | none` (extend `get`), `len` on dict/tuple.
  - M10 follow-up: CHECKTYPE for `tuple[...]`, `dict[K, V]` (deep, path in message).
- Opcodes: NEWTUPLE, NEWDICT, GETFIELD (+ INDEX on Dict/Tuple).
- Tests: golden `m8b/literals` → tuple/dict display; `m8b/dotkey`: nested dict `cfg.server.port` → `22`; `m8b/missing_key`: `cfg.server.prot` → R0401 `cfg.server.prot: missing key "prot"` + did-you-mean `port`; `m8b/dict_comp`: `{h: len(h) for h in hosts}` → dict; `m8b/unhashable`: float key → R0206.
- Exit: goldens pass; GETFIELD has a single dispatch point ready for Record/Module variants.
- Out of scope: sets, dict update/merge syntax, records (`def … as`), JSON parsing (needs imports/modules).

> Q24: dict semantics — (a) insertion-ordered (Python 3.7+, `indexmap`), keys Str/Int/Bool/Tuple (b) sorted `BTreeMap`, no new dep. Default (a).

### M9 — Diagnostics quality pass — DONE 2026-11-27

- Implementation notes (branch `m9-m10`, three commits M9a/M9b/M9c):
  - User decision: one rustc-style layout for compile AND runtime errors: our own header `error[CODE]: message`, then an ariadne box (ariadne's own `Error:` header line is rendered with an empty message and dropped by `diagnostic::snippet`; a unit test guards the ariadne layout). The box's `╭─[ file:l:c ]` replaces the old `  --> file:l:c` line. Runtime errors draw the box for the innermost frame's instruction; its label is `ErrorKind::label()` (`divisor is zero`, `not a function`, …); notes, R0601 trail and trace follow below the box. Every `.stderr` golden re-blessed (formatting only in M9a).
  - `--color auto|always|never` (global clap flag) → `diagnostic::ColorChoice`; `stderr_color()` removed. DAP renders without colour.
  - User decision: `ErrorKind` stays a plain `Copy` code enum (DAP matches on it); all wording moved into named constructors in `vm/src/error.rs` (`RuntimeError::operand_types`, `::not_callable`, `::division_by_zero`, … ~30) + `RuntimeError::help`. `Value::describe` cuts the value to 40 chars (`value::preview`, `PREVIEW_CHARS`). Message changes: R0201 `unsupported operand types for `+`: int (1) and float (2.0)` (Python wording kept instead of the planned `cannot add …`), R0202 `cannot call int (3): it is not a function`, R0203 shows the value, R0104's explanation became a help note.
  - Recovery was already per line at top level and inside blocks (`Parser::sync_line`, `block()`); the cap is in the renderer (`MAX_SHOWN = 20`, then `error: N more errors not shown`) so LSP still gets every diagnostic.
  - Catalogue: `diagnostic::E_CODES` + `E_RETIRED` (E0202), `ErrorKind::ALL`. `compiler/tests/codes.rs` requires an `error[CODE]` in some golden `.stderr` for every code (exempt: retired, R0501 — `cli.rs` test with stdout = `/dev/full`, R0801 debugger-only, R0901 internal; `PENDING` = E0401 until M10). Reuses existing goldens instead of a `m9/codes/` copy per code.
  - New goldens `m9/`: `dedent` E0003, `unterminated_str` E0004, `bad_escape` E0005, `tab_indent` E0006, `empty_interp` E0008, `not_supported` E0009, `bad_number` E0010, `too_deep` E0105, `too_many_elems` E0301, `bad_type` E0403, `dup_keyword` E0425, `not_ordered` R0205, `unwrap_err` R0403, `recovery` (3 errors), `did_you_mean` (local + builtin). insta: 4 multi-error snapshots in `diagnostic.rs` + a 23-error cap test.
  - `debug_assert!` in `Diagnostic::new` against inverted spans (a `Span` always exists, so "no diagnostic lacks a span" holds by type). W0201 not done (post-MVP lint).

- Goal: errors are the product: snippets everywhere, recovery, hints, stable codes.
- Tasks:
  - parser: multi-error recovery (sync on NEWLINE at current indent / DEDENT; cap 20 diagnostics); E0103 with label at opening delimiter.
  - resolve: did-you-mean (Levenshtein ≤ 2 over visible names, incl. builtins); (E0202 retired.) Optional warning W0201 'shadowed binding never used' (post-MVP lint).
  - diagnostic: ariadne snippet for runtime errors (innermost bytecode span, label = message); notes (e.g. R0201 "note: use int_to_float to convert"); messages include value types and short value preview (≤ 40 chars) — `cannot add int (3) and str ("x")`.
  - vm/error: all kinds carry structured data (`ErrorKind::TypeMismatch{op, lhs: TypeName, rhs: TypeName}` etc.), message formatting in one place.
  - main.rs: `--color=auto|always|never`; error output flushed after stdout.
  - catalogue lives in doc comments of `vm/src/error.rs` (R codes) and `compiler/src/diagnostic.rs` (E codes); keep §5 in sync.
- Tests: one golden per error code in §5 (dir `m9/codes/`); insta snapshots of 5 multi-error files; golden `m9/recovery`: file with 3 independent syntax errors → 3 diagnostics, exit 2; `m9/did_you_mean` → hint naming the close binding.
- Exit: every code in §5 has a golden; no diagnostic lacks a span (assert in `Diagnostic::new` debug build).
- Out of scope: `--error-format json`, LSP, warnings/lints (unused binding etc.).
- Note: M9 is cross-cutting; prefer landing each piece in the milestone that introduces the error and keep M9 as the sweep.

### M10 — Type annotations with runtime checks

- Goal: Pydantic-strict annotations on bindings, params, returns enforced at runtime.
- Blocks: runtime checking of the annotations parsed since M4 (fn params/returns required, `let`/lambda optional); type expressions per S4 (+ `any`, type vars): `int float bool str`, unit `()`, `list[T]`, `T | none` (optional, no wrapper), `T ! error` (Result; `!` binds looser than `|`), function types `A -> B`, `(A, B) -> C`, `() -> A` (right-assoc).
- Tasks:
  - resolve: separate type namespace (annotation positions only; a type name may coincide with a value name); resolve type names → `TypeDesc`; `T | none` → `Optional(T)` (flattened: `T | none | none` = `T | none`); `T ! E` → `Result(T, E)`; E0402/E0403.
  - codegen: intern `TypeDesc` → `TypeId` in `Program.types`; CHECKTYPE after binding init; callee prologue CHECKTYPE per annotated param; `Proto.ret_type`; compile-time literal mismatch E0401.
  - vm: CHECKTYPE: scalars by tag; list[T] deep check (Q9) with element path (`at element 3`); `T | none`: accept `none`, else check T; `T ! error`: accept Err (payload unchecked in MVP), Ok(v) → check v against T; `any` and bare type vars: no check; `list[T]` with T a type var: shape only; function type = callable with matching arity (Closure/Native; param/return types unchecked — documented limitation); frame creation pushes `proto.ret_type` into `ret_checks`, TAILCALL merges (dedupe), RETURN validates all → R0204 with message `return value of fact: expected int, got float (1.0)`.
  - chunk: disasm prints type table.
- Opcodes: CHECKTYPE.
- Tests: disasm snapshot with CHECKTYPE + type table; golden `m10/ok`: fully annotated program → normal output; `m10/binding_mismatch`: binding annotated float bound to an int result of a call → R0204 exit 1 (`expected float, got int (3)`, no coercion); `m10/literal_mismatch`: int-annotated binding initialised with literal `none` → E0401 exit 2; `m10/optional`: `int | none` binding accepts `5` and `none`, rejects a str → R0204; `m10/result`: `int ! error` return annotation accepts Ok(1) and Err("x"), rejects Ok("s") → R0204; `m10/result_no_autowrap`: `int ! error` fn whose body ends in a plain `3` → R0204 `expected int ! error, got int (3)` + note "wrap the value in Ok(...)" (no auto-wrap, Q29); `m10/param_mismatch` via a `_` partial → R0204 trace points into callee prologue with call-site frame below; `m10/list_deep`: `list[int]`-annotated binding with a str at index 2 → message contains `at element 2`; `m10/ret_tail`: annotated tail-recursive fn returning wrong type after 10^5 tail calls → R0204, flat stack (unknown type names are caught in M4, `m4/unknown_type`).
- Exit: goldens pass; all M4–M9 goldens still green now that their (required) fn annotations are enforced — any mislabelled golden is a bug to fix in the golden.
- Out of scope: inference, generic instantiation checks (type vars accepted, only outer shape checked; S13), records and unions (`def … as`, post-MVP), user error types (`T ! MyErr`).

> Q9: list[T] depth — (a) deep, path in message (b) shallow (c) first k elements. Default (a).

### Reordering / merging notes

| change | ok? | why |
|---|---|---|
| merge M2+M3 | yes | both tiny, no new runtime structures; keep separate if a reviewer wants small diffs |
| merge M4b into M4 | yes | M4b is resolver normalisation + 2 small opcodes; separate only to keep the M4 diff reviewable |
| merge M4+M5 | yes | TAILCALL is ~40 lines once frames exist; separate keeps call-protocol bugs isolated |
| move M10 (runtime checks) right after M5 | decided NO, but parse/resolve/E0410 moved to M4 | Q8=b makes annotations required, so M4 already parses + stores them and rejects missing ones (E0410/E0402). Runtime CHECKTYPE stays in M10: it needs list/optional/Result/dict types to be complete in one pass, and correct programs behave identically without it. M10 then validates every earlier golden's annotations. An agent may pull a scalar-only CHECKTYPE (int/float/bool/str/unit/any) forward to right after M5 if desired; it does not change any golden of a correct program. |
| M8b (tuples/dicts) after M10 or post-MVP | yes | stretch scope (D27); nothing in M9/M10 depends on it except their dict/tuple follow-ups |
| swap M7 ↔ M8 | yes | independent; M8 first loses `lines/split/join` (move them to M7) |
| split M6 into M6a closures/lambdas, M6b `_` partials + pipe placeholder slot | recommended if M6 PR > ~1500 lines | M6b is desugar-only, no VM change |
| move pipe (D24) from M4 to M6 | yes | pure desugar; M4 keeps it only because calls exist there |
| M9 earlier/incremental | yes (preferred) | codes added with features; M9 = sweep |
| M1 before syntax.md is final | partial | VM/codegen via `ProtoBuilder` tests can land; parser waits |

### Post-MVP (not planned here; listed to keep MVP boundaries sharp)

- M11 bytecode files: `postcard` serialization of `Program` with magic+version+source hash, `valo build`, `valo-vm file.vlc`, load-time verifier (jump targets, register < num_regs, const/proto indices) (Q11).

> Q11: bytecode files — (a) post-MVP M11 (b) inside MVP after M8. Default (a).

- M12 pattern matching (match expression with its own arm-arrow token per S7, guards, `_` wildcard — E0206 then applies only to expression positions; TESTTAG/GETFIELD-style opcodes; exhaustiveness).
- M13 records & tagged unions (S8, S17, S18, S20), after M12 (M12 covers literal/`none`/Ok/Err/list/tuple patterns; record/variant patterns + union exhaustiveness land here):
  - Program gets a shape table: `shapes: Vec<Shape{name, type_params, fields: Vec<Field{name, type: TypeId, has_default}>, union: Option<UnionId>}>`, `unions: Vec<Union{name, variants: Vec<ShapeId>}>`; TypeDesc gains `Shape(ShapeId)`, `Union(UnionId)`.
  - Value: `Record(Rc<Record{shape: ShapeId, fields: Box<[Value]>}>)`; fieldless variants = interned 0-field records (global constants).
  - Constructor = compiler-generated proto per shape: keyword-only params = fields (D28 normalisation, defaults via JMPIFSET prologue, CHECKTYPE per field), body `NEWRECORD A Bx` (Bx = ShapeId, fields in R[A+1..]). Positional construction → E0420.
  - Spread `Disk(**d, used_pct=95)` / `Disk(**raw)`: `CALLKW A B` = call R[A] with keyword dict R[B] (spread source record→dict, explicit keywords override, duplicate explicit keywords → compile error); VM maps names via `param_names`, extra key → runtime error (extra="forbid"), missing → Missing → defaults. Also serves `run(cmd, **opts)`.
  - `Type.validate(v)` → `T ! error`: same routine as CALLKW but field failures become `Err` with field path; nested dict → nested record coercion per field type (Q28).
  - GETFIELD (shared with M8b dicts, later modules): Record → field index by name in shape (linear scan; inline cache later). M12/M13 match opcodes: `TESTTAG A Bx` (skip next if R[A] is a record of shape Bx), `GETFIELDI A B C` (positional field C, for positional patterns).
  - CHECKTYPE: `Shape` → O(1) shape-id compare (fields already checked at construction, values immutable); `Union` → shape ∈ variants; generic `Page[int]` → shape only (S20).
  - Display `Disk(host="nas", mount="/", used_pct=91)`, `Maintenance`; equality structural (same shape + fields). Builtin `error` record replaces MVP Str Err payloads.

> Q28: `validate` nested coercion — (a) dicts coerced into nested record types per field annotation (Pydantic) (b) nested values must already be records. Default (a).

- Effect handler over `Host` (dry-run/record/replay), continuation-style natives. All io natives (`effect=true`) route through it regardless of inference (S22).
- Effect inference pass + `valo check --effects` (S22): per-fn io bit = fixpoint over the call graph seeded by `NativeDef.effect`; calls through unknown fn values = maybe-io; output `pure` / `io (via run, read_file)`; result stored in debug info for traces. No enforcement.
- M14 OS layer (S23): backtick `cmd` values (argv split at compile time, each `{expr}` one element, `{*xs}` spread; lexer reuses the M7 interpolation mode stack), `Value::Cmd`, native `run(c, timeout=30, allow_fail=false, stdin=none, env={}, cwd=none) -> out ! error` (keyword defaults via M4b, `out` record needs M13), `c1 | c2` pipelines (pipefail), all through the host effect handler; `std::process` behind a cargo feature (no WASM).
- M15 regex (S24): `re"…"` validated + compiled at compile time with the `regex` crate (invalid → compile error with span; regex stored as a constant), `=~`/`!~` (unanchored search), natives `find`, `find_all`, `captures`, `replace`, `split`, `regex(str) -> regex ! error`; regex patterns in `match` with named groups binding `str` / `str | none` (needs M12).
- M16 tests (S25): `*_test.vl` discovery, `valo test`, `test "name":` blocks, `mock name = fn_value` via effect handler / global rebinding in the runner (needs imports); `test` blocks outside test files → compile error. `assert` already exists (D29).
- Modules/imports (S16): `import`, `from … import`, `import ./path`, module = record value (GETFIELD), `_private`; move non-prelude builtins into modules (Q25). Keywords already reserved in M1.
- `imbl` lists, prelude in valo (revisit D7), sets, triple-quoted strings, `##` doc comments, slicing.
- Static type inference (HM), delta-compressed line info, fused compare-jump, RK/immediate operands.

## 7. Open questions

Each is also marked inline (`> Q<n>`) at the milestone it affects. Work proceeds on the default; a different answer changes only the listed milestone(s).

syntax.md S1–S4 (round 1) settle no existing Q directly; they removed auto-currying (D6), Unit-arg nullary workaround (D20) and data-last order (D21). Rounds 2–3 (S5–S12) resolved Q2 (c), Q5, Q7 (b), Q15 (a); Q3 stays (a) with the `/` exception. Round 4 (S13–S16) resolved Q8 (b) and Q14 (hybrid); new Q24, Q25. Round 5 (S17–S21) resolved Q18 (a); new Q26–Q28. Round 6 (S22–S25) resolved no existing Q; new Q29 (mirrors syntax Q-S26 auto-wrap), Q30. New: Q17, Q18 (round 1), Q19–Q23 (rounds 2–3).

| id | question | options | default | blocks |
|---|---|---|---|---|
| Q1 | Int overflow | (a) checked → R0101 / (b) wrapping / (c) bigint | (a) | M1 |
| Q2 | Int div & mod | RESOLVED (c): Python split `/` `//` `%` `**` (S11, D9) | (c) | M1, M3 |
| Q3 | Int↔Float mixing | (a) never implicit except `/` (D9), explicit conversions / (b) Lua-style promotion | (a) | M3 |
| Q4 | upvalue model | (a) capture-by-value at CLOSURE, Lua shapes kept / (b) full Lua open/closed upvalues | (a) | M6 |
| Q5 | Option/Result in MVP | RESOLVED: bare `none` optional + `Ok`/`Err`, `?`, `context`; no match (S9, S10, D26) | — | M7 |
| Q6 | program entry | (a) script: top-level items run in order, expression results discarded / (b) required `main` fn (Unit → Unit) | (a) | M2, M4 |
| Q7 | shadowing | RESOLVED: (b) same-scope rebinding (syntax.md S5) | (b) | M2 |
| Q8 | annotations optional? | RESOLVED (b): fn params + return required (E0410, M4); `let`/lambda optional; type vars/`any` shape-only (S13) | (b) | M4 (parse/E0410), M10 (checks) |
| Q9 | list[T] check depth | (a) deep O(n) with element path / (b) shallow / (c) first k elements | (a) | M10 |
| Q10 | print semantics | (a) any value, Display + newline / (b) Str only / (c) `print` + `println` | (a) | M4 |
| Q11 | bytecode files in MVP | (a) post-MVP M11, keep `Program` serializable / (b) after M8 | (a) | none |
| Q12 | diagnostic renderer | (a) ariadne (dep exists), no color off-TTY / (b) hand-rolled / (c) codespan-reporting | (a) | M0 |
| Q13 | exit codes | (a) 0 ok / 1 runtime / 2 compile / 64 usage+unreadable file / (b) 0 / 1 | (a) | M0 |
| Q14 | builtin namespacing | RESOLVED hybrid: prelude flat, rest module-qualified (S16) | hybrid | M4 |
| Q15 | Unit & function display | RESOLVED (a): `()` (S9), `<fn f/n>` | (a) | M4 |
| Q16 | local mutual recursion | (a) unsupported (E0204), use top-level fns / (b) letrec groups via shared env record | (a) | M6 |
| Q17 | pipe evaluation order | (a) LHS first / (b) textual desugar order | (a) | M4 (M6 for `_` slot) |
| Q18 | scope of static arity/keyword check | RESOLVED (a) by S19 (provisional): top-level + local named fns, natives, constructors, immutable bindings of those | (a) | M4, M4b, M6 |
| Q19 | `?` on `none` | (a) disallowed, R0208 (E-code if literal) / (b) propagate `none` / (c) convert to Err | (a) | M7 |
| Q20 | division/modulo by zero | (a) R0102 for Int and Float (Python) / (b) Float → IEEE inf/NaN | (a) | M1, M3 |
| Q21 | Err trace data | (a) origin + `?` propagation sites / (b) full stack snapshot / (c) chain only | (a) | M7 |
| Q22 | `?` inside lambda | (a) returns from lambda / (b) compile error | (a) | M7 |
| Q23 | discarded top-level Err value | (a) unhandled → R0601 exit 1 / (b) silently discarded | (a) | M7 |
| Q24 | dict ordering & key types | (a) insertion order (`indexmap`), keys Str/Int/Bool/Tuple / (b) `BTreeMap` sorted, no dep | (a) | M8b |
| Q25 | non-prelude builtins in MVP (`read_file`, `write_file`, `read_line`, `trim`, `split`, `lines`, `join`, `str_len`…) without imports | (a) flat globals in MVP, moved into modules when imports land (goldens updated then) / (b) builtin module records (`io`, `text`) reachable without import via GETFIELD (needs M8b) / (c) minimal `import` of builtin modules inside MVP | (a) | M7, M8 |
| Q26 | positional overloads on natives (`range`) | RESOLVED (a): native-only arity range (`ParamSpec::positional`, M8) | (a) | M4b, M8 |
| Q27 | dynamic calls omitting defaulted params | (a) allowed, Missing padding / (b) must pass all positionally | (a) | M4b |
| Q28 | `validate` nested coercion | (a) dict → nested record per field type (Pydantic) / (b) nested values must already be records | (a) | M13 |
| Q29 | `T ! error` fn body auto-wraps final value in `Ok` (syntax Q-S26) | (a) no: explicit `Ok(...)` required, R0204 + hint / (b) auto-wrap non-Result values | (a) | M7, M10 |
| Q30 | `assert` in normal (non-test) code | (a) yes, from M3, R0701 (D29) / (b) only inside `test` blocks (post-MVP) | (a) | M3 |
