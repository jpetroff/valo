# valo-dap — debugger roadmap

`valo-dap` is valo's **debug adapter**. It speaks the Debug Adapter
Protocol (DAP) to VS Code (or any DAP client: Neovim's nvim-dap, Helix, Zed,
Emacs dap-mode) and runs the program on the valo VM, which is linked into the
same process. How debuggers work and why it is built this way:
[docs/13-debugger.md](../../docs/13-debugger.md). Install:
[tooling/README.md](../README.md#debugger).

```text
tooling/dap/
├── Cargo.toml
├── src/
│   ├── main.rs       stdio wiring: reader thread → channel → session
│   ├── lib.rs        crate map
│   ├── protocol.rs   Content-Length framing, Outbox (responses + events, seq numbers)
│   ├── session.rs    the session state machine, one arm per DAP request
│   ├── program.rs    the program thread: runs the VM; DapHook stops it and answers
│   └── host.rs       DapHost: `print` → `output` events in the Debug Console
└── tests/smoke.rs    starts the real binary and plays VS Code's part
```

The debugger grows in steps, like the language server (`tooling/lsp/README.md`).
Each step adds **capabilities** (flags in `session::capabilities`, which make
VS Code show the matching UI), **requests** (arms in
`Session::handle_request`) and, where needed, a **VM or compiler change**.
Each step ends with a smoke test in `tests/smoke.rs` that plays the client's
part of the new requests.

## Status

| Step | You can… | DAP requests / capabilities | VM / compiler work | Status |
|------|----------|-----------------------------|--------------------|--------|
| **D0** | F5 / ▷ runs the file; `print` output, compile and runtime errors in the Debug Console; exit code | `initialize`, `launch`, `setBreakpoints` (all unverified), `configurationDone`, `threads`, `disconnect`; events `initialized`, `output`, `exited`, `terminated` | none (`Host` is already the door: `DapHost`) | **done** |
| **D1** | stop at line breakpoints, see the call stack, continue, pause, stop, stop on entry; Run Without Debugging ignores breakpoints | `stackTrace` (paged), `continue`, `pause`, `terminate` (`supportsTerminateRequest`), `stopOnEntry` and `noDebug` launch options; event `stopped {reason: breakpoint\|pause\|entry}`; breakpoints verified and moved to the next line with code | **debug hook** in `Vm::execute` (`vm/src/debug.rs`: `DebugHook`, `VmView`, `LineTable`); R0801 for an aborted run | **done** |
| **D2** | step over / into / out | `next`, `stepIn`, `stepOut`; `stopped {reason: step}` | none: `VmView::depth` was enough | **done** |
| **D3** | see variables: parameters, locals, globals; hover a name | `scopes`, `variables`, `evaluate` for bare names (`supportsEvaluateForHovers`) | **compiler**: local-variable debug info (`Proto::locals`: name, register, live pc range); `VmView::locals`, `VmView::globals` | **done** |
| **D4** | stop *at* a runtime error with all frames intact | `setExceptionBreakpoints` filters (`runtime`, `assert`), `exceptionInfo` (`supportsExceptionInfoRequest`); `stopped {reason: exception}` | `DebugHook::on_error`, called in `Vm::run` **before** `unwind` clears the frames | **done** |
| **D5** | conditional breakpoints, hit counts, logpoints, function breakpoints | `supportsConditionalBreakpoints`, `supportsHitConditionalBreakpoints`, `supportsLogPoints`, `setFunctionBreakpoints` | evaluate an expression in a paused frame: compile against the frame's locals; calls need the M8 re-entry mechanism | next |
| **D6** | program in the integrated terminal (stdin works), `args`/`cwd`/`env`; debug `.vlc` files | reverse request `runInTerminal`; `loadedSources`; `Source.checksums` | M7 stdin in `Host`; M11 writes debug info (spans, line starts, locals) into `.vlc` | |
| **D7** | step **back** and reverse-continue | `supportsStepBack`: `stepBack`, `reverseContinue` | the effect handler's `--record`/`--replay` (`host.rs`): re-run from the start with recorded effects to instruction N−1 | stretch |

### D1: how it is built

The adapter owns the debugging *policy* (which lines have breakpoints,
whether a step has finished); the VM only offers a *mechanism*. Lua does the
same (`lua_sethook` with `LUA_MASKLINE`, `ldebug.c`).

```rust
// vm/src/debug.rs
pub trait DebugHook {
    /// Before the first instruction of a source line runs. Blocking here
    /// *is* pausing; `Resume::Abort` ends the run with R0801.
    fn on_line(&mut self, vm: &VmView<'_>, line: u32) -> Resume;
}
pub struct VmView<'a> { /* frames, registers, globals of the paused VM */ }
impl VmView<'_> { pub fn depth(&self) -> usize; pub fn frame(&self, i: usize) -> Option<FrameInfo>; }
pub struct LineTable { /* per Proto: line started at each pc */ }
```

- `Vm::set_hook(&mut hook)`; `Vm::run` then builds a `LineTable`, and
  `Vm::execute` calls `debug_line` before each instruction. Without a hook
  the loop pays one `Option` check per instruction.
- **Line start** = the *first* instruction (lowest pc) of a function whose
  span is on that line. A statement spread over lines (`(x +\n 4)`)
  reports each line once; a breakpoint fires once per execution of its line.
  The hook is called only at line starts, so it is cheap when it does
  nothing.
- **Breakpoint verification** (`Session::set_breakpoints`): the set of lines
  with a line start (`LineTable::lines`), computed at `launch`. A breakpoint
  on a line without code moves to the next line with code; past the last
  one it is unverified. A file other than the program's is unverified.
  `initialized` is sent after `launch` succeeds, so the breakpoints always
  arrive after the program was compiled.
- **Threads** (`program.rs`): the hook runs on the program thread. To stop,
  it posts `Input::Stopped` to the session (which sends the `stopped`
  event) and blocks on a `Command` channel: `StackTrace` (answered with
  JSON built on that thread, because `Rc` values cannot leave it),
  `Continue`, `Abort`. `pause` and `stopOnEntry` set an `AtomicBool` the
  hook checks at the next line start; `terminate` sets an abort flag.
- **Stack frames**: frame *i* (innermost first) has id `2i`; a frame that
  replaced others by tail calls is followed by a label frame `2i + 1`:
  `… 3 tail calls elided …`. `stackTrace` honours `startFrame`/`levels`
  and only builds JSON for the frames asked for (200 000 deep is fine).
- Columns are counted in characters (`SourceMap::line_col`); VS Code wants
  UTF-16 units, which differ only for characters outside the BMP (emoji in
  strings, M7). `linesStartAt1`/`columnsStartAt1` are assumed true (VS Code
  always sends that).

### D2: how stepping is built

No VM change: stepping is the D1 hook with another stop condition
(`program.rs`). The session sends `Command::Step(kind)` to the stopped
program; the hook stores `Step { kind, depth }` with the call depth at that
moment, runs on, and stops (reason `step`) at the next line start where
`DapHook::step_done` holds:

| Request | Key | Stop at the next line start where… |
|---------|-----|------------------------------------|
| `stepIn` | F11 | any frame |
| `next` (step over) | F10 | depth ≤ depth when the step began |
| `stepOut` | Shift+F11 | depth < depth when the step began |

- **Priority**: pause, then a breakpoint, then the step. A breakpoint met
  inside a call being stepped over stops there (reason `breakpoint`), and
  any stop ends the step, as in every debugger.
- **Step out lands mid-line in the caller**: the caller's remaining part
  of the line (e.g. `let a = sq(3)` storing `a`) runs without a stop, and
  the step ends at the caller's *next* line. Same as Lua and Python.
- **Tail calls**: a tail call replaces the frame without changing the
  depth, so `next` over one stops inside the callee (with the label frame
  `… 1 tail call elided …` below it). This is honest: the caller's frame
  is gone (M5). Lua does the same.
- **Built-ins** have no frames and no lines: `stepIn` on `print(x)` steps
  over it. (M8: a lambda passed to `map` *is* a frame, so stepping into
  `map(xs, x => …)` will enter the lambda.)
- **The end of the program**: codegen gives the implicit `LOADUNIT;
  RETURN` at the end of `<main>` a span after the last line of the file.
  The debugger never stops there (`program::last_code_line`), and no
  breakpoint can be placed there, so stepping past the last statement ends
  the program instead of stopping on an empty line.

### D3: how variables are built

Registers have no names at run time, so the compiler records them, like
Lua's `LocVar` (`lfunc.h`) or DWARF's `DW_TAG_variable` with a location
list:

```rust
// vm/src/chunk.rs
pub struct LocalInfo { pub name: Rc<str>, pub reg: u8, pub start_pc: u32, pub end_pc: u32 }
pub struct Proto { /* … */ pub locals: Vec<LocalInfo> }
```

- **Compiler** (`codegen.rs`): `FuncState::declare_local` pushes an entry
  when a parameter or `let` gets its register: parameters from pc 0, a `let`
  from the instruction *after* its initialiser (before that the register
  holds a half-computed value). `close_locals` sets `end_pc` when the
  block ends; parameters stay live to the end of the function.
  `--dump bytecode` prints the table (`locals:`, after `constants:`); the
  codegen snapshots cover it.
- **VM** (`debug.rs`): `VmView::locals(i)` returns the entries of frame
  *i* that are live at its pc (for a caller: the CALL it waits on), read
  from its registers. A shadowing `let x` has its own entry; when both are
  live only the later one is returned. `VmView::globals()` returns the
  top-level bindings that have a value, without built-ins (a `let` not run
  yet is not shown; a name defined twice shows its later slot).
- **Adapter** (`session.rs`, `program.rs`): `scopes` gives each stack frame
  a *Locals* scope (`variablesReference` = `2 + frame`) and the shared
  *Globals* scope (`1`); label frames have none. `variables` asks the
  stopped program thread (`Command::Variables`), which formats values with
  `Display` (as `print` shows them) and their `type_name`. An omitted
  parameter whose default the prologue has not computed yet shows as
  `(default not computed yet)`.
- **Hover and Watch** (`evaluate`): a bare name is looked up in the frame's
  locals, then the globals (`Command::Evaluate`). Anything else is an error
  until D5. VS Code sends the word under the mouse as `expression`.
- No children yet (`variablesReference: 0` for every value): lists and
  dicts (M8) will get references so the UI can expand them; references
  are only valid while stopped, so they can be handed out per stop.

### D4: how stopping at errors is built

- **VM** (`vm.rs`, `debug.rs`): `Vm::run` calls `Vm::debug_error` when
  `execute` returns an error, and only then `unwind`, which clears the
  frames. `DebugHook::on_error` gets a `VmView` with `at_error` set: the
  innermost frame's pc is already past the failing instruction, so the
  view steps back one, and `frame(0)` / `locals(0)` describe the failing
  line. `Interrupted` (R0801, the debugger's own Stop) is not reported.
- **Filters**: `capabilities()` lists two `exceptionBreakpointFilters`,
  *Runtime errors* (`runtime`) and *Failed assertions* (`assert`, R0701),
  both ticked by default in VS Code's Breakpoints view.
  `setExceptionBreakpoints` turns the two `Shared` flags on or off. A
  client that never sends it gets no exception stops.
- **Adapter** (`program.rs`): `DapHook::on_error` checks the flag, builds
  an `Exception` (code + message as `valo run` prints it, with the asserted
  source text and `left:`/`right:` notes) and stops with reason
  `exception`. The `stopped` event carries it as `text`; `exceptionInfo`
  answers the editor's exception widget (`breakMode: "unhandled"`: valo has
  no try/catch).
- **After the stop**: Continue, a step or Stop all let the error go on: the
  program ends with exit code 1 and the error in the Debug Console, as with
  `valo run`. (Continuing *past* an error would need a way to recover; with
  `Result` and `?` in M7, errors that are values never stop here at all.)
- Stack overflow (R0301) stops with 200 000 frames: `stackTrace` paging
  keeps that cheap.

## Language milestones → debugger checklist

Like the grammar and the language server, the debugger follows the language.
Every milestone checks this table.

| Milestone | Debugger work |
|-----------|---------------|
| ~~M6~~ lambdas, closures, `f(a, _)` | **done**: a **Closure** scope listing captured values (`VmView::captures`, names from `Proto::upvals`; only for frames that captured something; hover finds them too); frames are named `<lambda@l:c>` / `<partial f@l:c>`; stepping into a partial enters the partial's frame, then the callee |
| ~~M7~~ strings, `none`, `Ok/Err`, `?`, I/O | **done**: quoted strings in Variables and hover (`Value::repr`); R0601 exception text lists the `Err`'s context and origin; `DapHost` reads/writes files, `read_line` returns `none` (no stdin yet). Still open: stdin → D6 `runInTerminal`; logpoint text `"x = {x}"` reuses interpolation (D5); `?` early return shows as a step out (works through the ordinary depth rule, untested) |
| M8 lists, `map/filter/reduce`, comprehensions | expandable lists (`indexedVariables`, paged); `<native map>` frames between callbacks; stepping into a lambda passed to `map`; evaluate can call functions (re-entry) |
| M8b tuples, dicts | `namedVariables` for dict keys / `.field` |
| M9 diagnostics quality | source snippets in `exceptionInfo.details` |
| M10 runtime type checks | `type` field of each variable from annotations |
| M11 bytecode files | debug info section in `.vlc`; `Source.checksums` to detect stale sources; debug `valo-vm` runs |
| M12 `match` | breakpoints on arms (each arm starts a line) |
| M13 records, unions | record fields as children; variant name as `value` |
| M14 commands, OS layer | child-process output → Debug Console; `--dry-run` as a launch option |
| S16 imports | a file id in `SrcSpan`; one `Source` per frame; breakpoints per file; `loadedSources` |
| S25 test files | "Debug test" code lens above `test "…":` (from valo-lsp, launches valo-dap) |
| effect handler (`--record` / `--replay`) | D7 reverse debugging |

## Seeing the messages

- VS Code: set `"valo.trace.debugger": true`, start a session, open
  Output → **Valo Debug Adapter**: every request, response and event, with
  `→` (to the adapter) and `←` (from it).
- Tests: `cargo test -p valo-dap -- --nocapture` and print `c.log` in a test.
- The spec: <https://microsoft.github.io/debug-adapter-protocol/specification>.
  The reference adapter to read next to it: Microsoft's `vscode-mock-debug`.
