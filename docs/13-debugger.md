# Chapter 13 — The Debugger: the Debug Adapter Protocol

Chapter 12 gave the editor knowledge of what a program **means**. A
debugger shows what a program **does**: it stops the running program at a
line, shows the call stack and the variables, and runs it one step at a
time. This chapter explains how VS Code debuggers are built for any language,
then how valo's debugger, `valo-dap` in [`tooling/dap`](../tooling/dap/README.md),
is built and how it will grow.

## 13.1 The three parts of every debugger

```text
 ┌────────────────────┐   Debug Adapter    ┌────────────────────┐   language-specific   ┌──────────────────┐
 │ debugger UI         │     Protocol       │ debug adapter       │   mechanism           │ debuggee          │
 │ (VS Code: gutter    │ ◀───(JSON over ──▶ │ (valo-dap)          │ ◀────────────────────▶ │ (the valo program │
 │  dots, Call Stack,  │      stdio)        │ knows the language  │   in-process hook,    │  on the VM)       │
 │  Variables, Debug   │                    │ and its runtime     │   ptrace, a socket…   │                   │
 │  Console, toolbar)  │                    │                     │                       │                   │
 └────────────────────┘                    └────────────────────┘                       └──────────────────┘
       generic                                 one per language                            the program
```

1. **The UI** is generic. VS Code knows nothing about Python, Go or valo;
   it draws breakpoints, a Call Stack view, a Variables view, a Debug
   Console and the step/continue toolbar, and fills them with whatever the
   adapter tells it.
2. **The debug adapter** is the only language-specific part. It translates
   UI requests ("set a breakpoint at line 7", "step over", "what is in frame
   2?") into the language's own way of controlling a program, and translates
   what happens back into events ("stopped at a breakpoint", "printed this").
3. **The debuggee** is the running program and its runtime. How the adapter
   controls it depends on the runtime:

   | Runtime | How the adapter controls it |
   |---------|-----------------------------|
   | native code (C, Rust) | the OS debugging API (`ptrace` on Linux), writes `int3` instructions as breakpoints, reads DWARF debug info to map addresses ↔ lines and variables (gdb, lldb; adapters: CodeLLDB, cpptools) |
   | a VM with a debug wire protocol (Java, .NET, Node) | the VM opens a socket and speaks its own protocol (JDWP, Chrome DevTools Protocol); the adapter translates |
   | an interpreter with a hook API (Python, Lua, Ruby) | the adapter (or a library loaded into the program) installs a **trace hook**: the interpreter calls it on every line, and the hook blocks while the program is "paused" (debugpy uses `sys.settrace`; Lua has `lua_sethook`) |

   valo is the third kind, and simpler still: the adapter **links the VM**
   and runs the program on its own thread (§13.4).

This is the same *M × N → M + N* idea as the language server (§12.2): one
generic UI per editor, one adapter per language.

## 13.2 The Debug Adapter Protocol

Microsoft published the **Debug Adapter Protocol (DAP)** in 2016, next to
LSP. VS Code, Visual Studio, Neovim (nvim-dap), Helix, Zed, Emacs (dap-mode)
and JetBrains IDEs (partly) speak it, so one adapter serves all of them.

**Framing** is identical to LSP: `Content-Length: N\r\n\r\n` + N bytes of
JSON (`tooling/dap/src/protocol.rs` → `read_message`, `write_message`).
**Messages** are *not* JSON-RPC; each has a `type`:

```jsonc
// request (client → adapter): a command with arguments
{"seq": 5, "type": "request", "command": "setBreakpoints",
 "arguments": {"source": {"path": "/w/demo.vl"}, "breakpoints": [{"line": 3}]}}
// response: points back with request_seq, says success or not
{"seq": 9, "type": "response", "request_seq": 5, "command": "setBreakpoints", "success": true,
 "body": {"breakpoints": [{"verified": true, "line": 3}]}}
// event (adapter → client, unasked)
{"seq": 12, "type": "event", "event": "stopped",
 "body": {"reason": "breakpoint", "threadId": 1, "hitBreakpointIds": [1]}}
```

Two ideas carry the whole protocol:

- **Capabilities.** The `initialize` response lists what the adapter can do
  (`supportsConditionalBreakpoints`, `supportsStepBack`, …). VS Code hides
  UI for anything missing. An adapter can therefore start nearly empty and
  grow, the way `valo-lsp` did.
- **Stop, then ask.** When the program stops, the adapter sends only a
  `stopped` event. The UI then *pulls* what it shows, lazily:

  ```text
  ◀── stopped {reason: "breakpoint", threadId: 1}
  ──▶ threads                       ◀── [{id: 1, name: "main"}]
  ──▶ stackTrace {threadId: 1}      ◀── frames [{id: 0, name: "fact", line: 3, source}, …]
  ──▶ scopes {frameId: 0}           ◀── [{name: "Locals", variablesReference: 1000}, …]
  ──▶ variables {variablesReference: 1000}   ◀── [{name: "n", value: "4"}]
  ```

  `variablesReference` is a number the adapter invents; a value with
  children (a list, a record) gets its own, and the UI asks for its
  `variables` only when the user expands it. These numbers, and frame ids,
  only need to stay valid **until the program runs again**.

### A whole session

```text
 VS Code                                              valo-dap
   │ ── initialize {adapterID: "valo", linesStartAt1} ─▶ │  reply: capabilities
   │ ── launch {program: "/w/demo.vl", stopOnEntry} ───▶ │  read + compile; fail = dialog
   │ ◀─────────────────────────────────── initialized ── │  "send configuration now"
   │ ── setBreakpoints {source, breakpoints} (per file) ▶ │  reply: verified / moved lines
   │ ── setExceptionBreakpoints {filters} ─────────────▶ │
   │ ── configurationDone ─────────────────────────────▶ │  start the program
   │ ◀──────────────────────────── output {stdout} ──── │  program prints
   │ ◀─────────────── stopped {reason: breakpoint} ──── │  (D1; reason exception: D4)
   │ ── threads / stackTrace ──────────────────────────▶ │  (D1)
   │ ── scopes / variables / evaluate (hover) ─────────▶ │  (D3)
   │ ── continue | pause | terminate ──────────────────▶ │  (D1)
   │ ── next | stepIn | stepOut ───────────────────────▶ │  (D2) → stopped {reason: step}
   │ ◀────────────────────────── exited {exitCode} ──── │
   │ ◀────────────────────────────────── terminated ─── │
   │ ── disconnect ────────────────────────────────────▶ │  process exits
```

`launch` starts a new program; the other way in is **`attach`**, which
connects to one already running (a server, a remote machine). valo only
launches for now.

The spec lets the adapter choose when to send `initialized`. valo sends it
after a successful `launch`, so the breakpoints always arrive after the
program was compiled and can be checked against its line table (§13.5).

## 13.3 The VS Code side: the extension

A debugger reaches VS Code through an extension's `package.json`, like a
language does (§12.1). valo's lives in the same extension as the grammar and
the LSP client, [`tooling/vscode`](../tooling/vscode/package.json):

```jsonc
"contributes": {
  "breakpoints": [{ "language": "valo" }],        // allow clicking the gutter in .vl files
  "debuggers": [{
    "type": "valo",                                // the "type" in launch.json
    "label": "Valo",
    "languages": ["valo"],                         // offered for .vl files
    "configurationAttributes": {                   // JSON schema: completion + checking of launch.json
      "launch": { "required": ["program"],
                  "properties": { "program": {…}, "stopOnEntry": {…} } }
    },
    "initialConfigurations": [ … ],                // written by "create a launch.json file"
    "configurationSnippets": [ … ]                 // "Add Configuration…" menu
  }]
}
```

`.vscode/launch.json` holds the user's **debug configurations**; every field
except `type`, `request` and `name` is passed unchanged to the adapter as
`launch` arguments. VS Code substitutes variables such as `${file}` and
`${workspaceFolder}` first:

```jsonc
{ "version": "0.2.0",
  "configurations": [
    { "type": "valo", "request": "launch", "name": "Debug demo", "program": "${workspaceFolder}/demo.vl" }
  ] }
```

The extension code ([`extension.js`](../tooling/vscode/extension.js))
registers three things with `vscode.debug`:

| Registration | Job in valo |
|--------------|-------------|
| `registerDebugAdapterDescriptorFactory('valo', …)` | says **how to start the adapter**. valo returns a `DebugAdapterExecutable` (a child process, DAP over stdio), found like `valo-lsp`: setting `valo.debugger.path`, else `target/debug/valo-dap` in the open checkout, else `PATH`. The alternatives are `DebugAdapterServer` (connect to a TCP port; useful to debug the adapter itself) and `DebugAdapterInlineImplementation` (an adapter written in JavaScript inside the extension). |
| `registerDebugConfigurationProvider('valo', …)` | **fixes up a configuration** before launch. With no launch.json, F5 sends an empty one; `resolveDebugConfiguration` turns it into "debug the active `.vl` file". |
| `registerDebugAdapterTrackerFactory('valo', …)` | **observes** every message. valo logs them to the *Valo Debug Adapter* output channel when `valo.trace.debugger` is on. |

Two commands, `valo.runFile` and `valo.debugFile`, put ▷ buttons in the
editor title. *Run* (Ctrl+F5) sends `noDebug: true` in the launch arguments:
the same adapter runs the program, breakpoints are ignored.

## 13.4 valo-dap: the adapter

`valo-dap` is a Rust binary that depends on `valo-compiler` and `valo-vm`,
as `valo-lsp` depends on the compiler. The program does **not** run in a
separate process: the adapter compiles it and runs it on a thread of its own.

```text
  stdin ──reader thread──▶ mpsc channel ──▶ session::run ──▶ Outbox ──▶ stdout
                               ▲                                ▲
                               └── ProgramEnded ── program thread: Vm::run(DapHost)
                                                                └── output events
```

| File | Job |
|------|-----|
| `src/main.rs` | reader thread: framed messages from stdin into a channel; runs the session; `process::exit` at the end stops a program still running |
| `src/protocol.rs` | framing; `Outbox` stamps `seq` numbers and writes whole messages under a `Mutex` (two threads write) |
| `src/session.rs` | the state machine: one `match` arm per request; `launch` compiles once to report errors early and learn which lines have code; `configurationDone` starts the program thread |
| `src/program.rs` | the program thread: runs the VM with `DapHook` attached; the hook stops at breakpoints and pause requests and answers the session while stopped |
| `src/host.rs` | `DapHost`: the program's `print` becomes an `output` event |

Why in-process instead of starting `valo run` as a child process with a
debug socket?

- **No second protocol.** A separate process would need its own wire format
  for "set breakpoint", "read register" — a second DAP. In-process, the
  adapter calls the VM directly.
- **The VM was built for it.** All effects go through `Host`
  (`vm/src/host.rs`, chapter 7), so redirecting output is one small struct;
  the VM did not change for step D0.
- **Same code as `valo run`.** `run_program` in `session.rs` is the CLI's
  `run` with another host, so the debugger runs exactly what the CLI runs.

The price: `Rc` values cannot cross threads, so the program is compiled on
the thread that runs it, and while paused the session asks the program
thread to describe frames and values as plain JSON:

```text
  session thread                              program thread (inside Vm::execute)
   │                                            │ DapHook::on_line(view, 2): breakpoint!
   │ ◀──────── Input::Stopped {breakpoint} ──── │ … blocks on its command channel
   │ ── stopped event to VS Code                │
   │ ◀── stackTrace request                     │
   │ ── Command::StackTrace {reply} ──────────▶ │ reads VmView, builds JSON
   │ ◀──────────────────────────── JSON ─────── │
   │ ── response to VS Code                     │
   │ ◀── continue request                       │
   │ ── Command::Continue ────────────────────▶ │ returns Resume::Continue: program runs on
```

The session thread sends every event and response itself, so the protocol
state (stopped or running) changes in one place, in message order. `pause`
and the Stop button only set flags (`AtomicBool`) that the hook checks at
the next line start.

Valo programs have one thread, but DAP is built for many: `threads`
answers with a single thread `{id: 1, name: "main"}`, and every `stopped`
event names it.

## 13.5 What a VM needs to be debuggable

Stopping a program needs four things from the VM and compiler. Line
numbers and frames existed for error messages; the hook was added for D1
(`vm/src/debug.rs`); variable names for D3.

| Need | What it is | valo today |
|------|------------|------------|
| **line table** | instruction → source position, to know which line runs and to place breakpoints | **have it**: `Proto::spans` (one span per instruction) + `Program::line_starts` (chapter 5) — the same data that prints stack traces |
| **frame inspection** | list the active calls with their current position | `VmView::depth`, `VmView::frame(i)`: a read-only window on `Vm::frames` (D1) |
| **a hook** | a call from the interpreter loop to the debugger at each new line, where the debugger may block (= pause) | `DebugHook::on_line`, called from `Vm::execute` via `Vm::debug_line` (D1) |
| **variable names** | register → name, valid for a range of instructions | `Proto::locals`, recorded by the compiler (D3); read through `VmView::locals` |

**What is a "new line"?** Instructions of one line are not always
consecutive: `let y = (x +\n    4)` compiles to *load x* (line 6), *load 4*
(line 7), *add* (line 6 again). valo calls the first instruction of each
line in a function its **line start** (`LineTable`, `debug.rs`) and calls
the hook only there, so each line is reported once per execution however
the code generator orders the instructions. The set of lines that have a
line start is where breakpoints can go: a breakpoint on a blank line or a
comment moves to the next one (the editor shows the dot jump). Native
debuggers solve the same problem with DWARF's `is_stmt` flag, which the
compiler sets on the instructions where a statement begins; valo can add
such a flag later if stepping stops mid-expression too often.

**Breakpoints in an interpreter** need no patched instructions (unlike
`int3` in native code). The hook runs at every line start and looks the
line up in the breakpoint table; that table is all the breakpoint state
there is. Without a debugger, the loop pays one `Option` check per
instruction, too little to measure (a 30-million-call benchmark runs as
fast as before).

**Abort.** The Stop button must end a program that may be anywhere. The
hook returns `Resume::Abort`, and `Vm::run` returns a runtime error
R0801 (*interrupted*), which unwinds like any other error; the adapter
recognises it and prints nothing.

**Variable names** are the one thing only the compiler knows. At run time
`let y = x * x` is just "register 3"; to show `y = 9` in the Variables view,
codegen writes a small table into each function (`Proto::locals`, the same
idea as Lua's `LocVar` and DWARF location lists):

```text
locals:                               (valo check f.vl --dump bytecode)
  R0   a          pc 0000..0023       parameter: live in the whole function
  R3   x          pc 0005..0022       let x = a + b       (from after its initialiser)
  R4   x          pc 0008..0022       let x = x * 2       (shadows the first x)
  R6   t          pc 0016..0017       a let inside an `if` block
```

A variable is shown when its range contains the frame's pc. A `let` starts
*after* its initialiser, because before that its register holds a
half-computed value. When two variables with one name are live (shadowing),
only the later one is shown. The Variables view asks for it in three steps:
`scopes` for a frame (*Locals*, *Closure*, *Globals*), then `variables` for
a scope's `variablesReference`, answered by the program thread from
`VmView::locals`, `VmView::captures` and `VmView::globals`. Hovering a name
sends `evaluate`, which looks it up the same way (locals, then captures,
then globals).

Values are shown as they would appear inside other values: strings quoted
(`"a\tb"`, `Value::repr`), results as `Ok(1)` / `Err("x")`. When a run
stops at an unhandled `Err` (R0601), the exception text adds the `Err`'s
context messages and origin.

*Closure* (M6) lists what a lambda or local fn captured: the values in
`Closure::captures`, named by `Proto::upvals` (the compiler stores each
captured binding's name next to where `CLOSURE` takes it from). The scope
is offered only for frames whose closure captured something. Temporaries of
`_` partials (`$a0`) are hidden.

**Stepping** needs no new VM support: it is the same hook with a different
stop condition. When the user steps, the hook remembers the step's kind and
the call depth at that moment (`VmView::depth`), lets the program run, and
stops at the first line start that satisfies (`DapHook::step_done` in
`tooling/dap/src/program.rs`):

| Step | Key | Stop at the next line start where the depth is… |
|------|-----|--------------------------------------------------|
| *step in* | F11 | anything: the first line of a called function counts |
| *step over* | F10 | no deeper than at the start: calls run through |
| *step out* | Shift+F11 | shallower than at the start: the rest of this function runs |

Consequences worth knowing:

- A breakpoint inside a function being stepped over still stops there:
  breakpoints are checked before the step condition, and any stop ends the
  step.
- *Step out* of `sq` in `let a = sq(3)` does not stop after the call on
  the same line (that would not be a line start); it stops at the caller's
  next line, with `a` already set.
- Tail calls (M5) replace the frame without changing the depth, so stepping
  over a tail call lands in the callee: the caller really is gone, and the
  Call Stack shows `… 1 tail call elided …` in its place.
- Codegen puts the implicit end of `<main>` on the line after the last one.
  The debugger skips that line (`program::last_code_line`), so stepping past
  the last statement ends the program instead of stopping on an empty line.

**Errors.** Without a debugger, a runtime error leaves `execute` and
`Vm::unwind` copies the frames into a stack trace, then clears them. A
debugger wants to stop *before* that, with frames and registers intact, so
the user can look at the values that caused the error. That is one more
hook call, `DebugHook::on_error`, in `Vm::run` between `execute` returning
`Err` and `unwind` (D4). One detail: the failing frame's `pc` has already
moved past the failing instruction (the loop increments it while
fetching), so the view handed to `on_error` steps back one.

In VS Code this is configured as **exception breakpoints**: checkboxes in
the Breakpoints view that the adapter declares in its capabilities
(`exceptionBreakpointFilters`). valo has two, *Runtime errors* and *Failed
assertions*. When one fires, the editor shows the exception widget under
the failing line with what `exceptionInfo` returns (`R0102 division by
zero: 7 // 0`), the Call Stack shows every frame, and Variables shows the
values. Continue lets the error end the program as `valo run` would; there
is no try/catch to resume into.

The full step plan with design notes is in
[`tooling/dap/README.md`](../tooling/dap/README.md): D0 run (done) → D1
breakpoints, stack, pause (done) → D2 stepping (done) → D3 variables (done) → D4 stop on errors (done) →
D5 conditional breakpoints, logpoints, evaluate → D6 terminal I/O and `.vlc`
files → D7 step back.

## 13.6 Step back for free (later)

Reverse debugging usually needs a heavy machine recorder (`rr`, WinDbg TTD).
valo has a shortcut: the language is pure except through `Host`, so a run
whose effects were recorded (`--record`, planned in `host.rs`) can be
replayed exactly. "Step back" = re-run from the start with the recorded
effects and stop one line earlier (count instructions). DAP supports this
with `supportsStepBack`, `stepBack` and `reverseContinue`: roadmap step D7.

## 13.7 Install and try

Build or install the adapter, then (re)install the extension, as for
`valo-lsp`:

```bash
cargo build -p valo-dap                      # inside the checkout: target/debug/valo-dap is found automatically
cargo install --path tooling/dap             # or: put valo-dap on PATH for any folder
cd tooling/vscode && npm install && npm run package
code --install-extension tooling/vscode/valo-0.1.0.vsix
```

Open a `.vl` file, click left of a line number to set a breakpoint and
press **F5** (or the ▷ debug button in the editor title). The program stops
there; the **Call Stack** view shows the active calls, and output appears in
the **Debug Console**. Details and troubleshooting:
[tooling/README.md](../tooling/README.md#debugger).

## 13.8 Where to change things

| Task | File → item |
|------|-------------|
| announce a capability | `tooling/dap/src/session.rs` → `capabilities` |
| answer a new DAP request | `session.rs` → `Session::handle_request` |
| add a launch option | `session.rs` → `Session::launch` + `tooling/vscode/package.json` → `debuggers[0].configurationAttributes` |
| change how the adapter is found | `tooling/vscode/extension.js` → `findBinary`, `ValoDebugAdapterFactory` |
| change F5-without-launch.json | `extension.js` → `ValoConfigurationProvider` |
| send program output elsewhere | `tooling/dap/src/host.rs` → `DapHost` |
| a new reason to stop (step, error) | `tooling/dap/src/program.rs` → `DapHook::on_line`; VM side: `vm/src/debug.rs` → `DebugHook` |
| change where breakpoints can go | `vm/src/debug.rs` → `LineTable` |
| change which variables are shown | `vm/src/debug.rs` → `VmView::locals`, `globals`; compiler side: `codegen.rs` → `declare_local`, `close_locals` |
| change how a value looks in Variables | `tooling/dap/src/program.rs` → `variable_json` |
| add an exception filter | `tooling/dap/src/session.rs` → `capabilities` (`exceptionBreakpointFilters`), `setExceptionBreakpoints`; `program.rs` → `DapHook::on_error` |
| test a session | `tooling/dap/tests/smoke.rs` |

## 13.9 Exercises

1. Turn on `"valo.trace.debugger": true`, press F5 on a file with one
   breakpoint, and read the *Valo Debug Adapter* output. In what order do
   `launch`, `setBreakpoints` and `configurationDone` arrive, and why must
   the program not start before the last one?
2. Make the Run button (`noDebug: true`) and the Debug button behave
   differently: print `valo-dap: running without debugging` to the
   `console` category when `noDebug` is set. Add a smoke test.
3. Set a breakpoint inside `fact` and look at the Call Stack after a few
   *Continue*s. Then write `fact` with an accumulator so the recursive call
   is a tail call (chapter 7, M5). How does the Call Stack change, and where
   does the label frame `… N tail calls elided …` come from
   (`DapHook::stack_trace`)?
4. Step through `let y = (x +\n    4)` (an expression over two lines) with
   F10. Where does it stop, and why does line 6 not show up a second time
   when the `ADD` on line 6 runs (§13.5, *What is a "new line"?*)?
5. Read about `stepBack` and `reverseContinue` in the DAP specification.
   Why can a deterministic replay of recorded effects (§13.6) step
   back without saving every intermediate state?
6. Stop inside a block (`let y = if c:` with a `let t` inside) and watch
   `t` appear and disappear in the Variables view as you step. Compare with
   the `locals:` table from `valo check --dump bytecode`: which instruction
   ends `t`'s range, and why does the range end *before* the block's value
   is moved into `y`?
7. Write `fn f(n: int) -> int = 1 + f(n - 1)` (no base case) and debug
   `f(0)` with *Runtime errors* ticked. Where does it stop, how many frames
   does the Call Stack say there are, and why does VS Code not hang while
   showing them (`startFrame`/`levels` in `DapHook::stack_trace`)?
