# valo tooling

Developer tools around the language: editor support and a debugger. How they
work is explained in [docs/12-editor-tooling.md](../docs/12-editor-tooling.md)
(highlighting, language server) and [docs/13-debugger.md](../docs/13-debugger.md)
(debugger).

```text
tooling/
├── lsp/      valo-lsp: language server (Rust, workspace member). Editor-neutral:
│             VS Code, Neovim, Helix, Zed… Roadmap: lsp/README.md
├── dap/      valo-dap: debug adapter (Rust, workspace member). Debug Adapter
│             Protocol, editor-neutral. Roadmap D0–D7: dap/README.md
└── vscode/   VS Code extension: syntax highlighting, editing rules, the
              client that starts valo-lsp, and the debugger that starts valo-dap
```

## Requirements

- Rust toolchain (as for the rest of the workspace).
- Node.js with npm, only for the VS Code extension (`brew install node`).

## Build and test the language server and debug adapter

```bash
cargo build -p valo-lsp -p valo-dap   # → target/debug/valo-lsp, target/debug/valo-dap
cargo test -p valo-lsp -p valo-dap
```

## Install in VS Code

### 1. Put the server and the debug adapter on PATH

```bash
cargo install --path tooling/lsp
```

```bash
cargo install --path tooling/dap
```

This builds release binaries into `~/.cargo/bin/valo-lsp` and
`~/.cargo/bin/valo-dap`. Repeat after changing the compiler, VM or tools.
(Inside a valo checkout the extension prefers `target/debug/valo-lsp` and
`target/debug/valo-dap` when they exist, so during development a plain
`cargo build` plus **Valo: Restart Language Server** is enough; the debug
adapter is started fresh for every debug session.)

### 2. Install the extension

Package it as a `.vsix` file and install that:

```bash
cd tooling/vscode && npm install && npm run package
```

```bash
code --install-extension tooling/vscode/valo-0.1.0.vsix
```

(or Extensions view → `…` menu → **Install from VSIX…**). With VS Code
Remote-SSH, install it in the remote window: extensions with a language
server run on the remote machine, next to `valo-lsp`.

Then **Developer: Reload Window** and open a `.vl` file.

### Development loop (no packaging)

- Open `tooling/vscode` in VS Code and press **F5**: a second window, the
  *Extension Development Host*, opens the valo repository with the extension
  loaded from source. Edit the grammar or `extension.js`, then reload that
  window (Ctrl+R).
- Or link the folder into the extensions directory once and reload:

  ```bash
  ln -s "$PWD/tooling/vscode" ~/.vscode-server/extensions/valo-lang.valo-0.1.0
  ```

  (`~/.vscode/extensions/` for a local, non-remote VS Code.)

### Checking that it works

- Bottom-right status bar shows **Valo** as the language of a `.vl` file.
- **Developer: Inspect Editor Tokens and Scopes** shows `source.valo` scopes.
- Type `3 *` on a line: a red squiggle with `E0101` appears. Fix it and it
  disappears.
- Output panel → **Valo Language Server** shows the server log. Set
  `"valo.trace.server": "verbose"` to see every LSP message.

## Debugger

`valo-dap` runs a `.vl` file under VS Code's debugger. Steps **D0–D4** are
done: `print` output, compile errors and runtime errors in the Debug
Console; line breakpoints, the call stack, continue, pause, stop, stop on
entry, step over / into / out, and the Variables view (locals, parameters,
globals; hover a name to see its value), and stopping at runtime errors
and failed asserts. Conditional breakpoints and logpoints come in step D5
([dap/README.md](dap/README.md)).

- **Quick start:** open a `.vl` file and press **F5**, or use the ▷ buttons
  in the editor title (*Valo: Run File* = Ctrl+F5 without debugging,
  *Valo: Debug File*). No `launch.json` is needed.
- **launch.json** (Run and Debug view → *create a launch.json file* →
  *Valo*), for a fixed entry file:

  ```jsonc
  {
    "version": "0.2.0",
    "configurations": [
      { "type": "valo", "request": "launch", "name": "Debug main", "program": "${workspaceFolder}/main.vl" }
    ]
  }
  ```

  | Attribute | Meaning |
  |-----------|---------|
  | `program` | the `.vl` file to run (required; `${file}` = the active editor) |
  | `stopOnEntry` | pause before the first line runs |

- **Checking that it works:** click the gutter left of a line number to set
  a breakpoint (a red dot), press F5: the line is highlighted and **Call
  Stack** lists the active calls, innermost first. Continue (F5), Pause
  (F6) and Stop (Shift+F5) work. A breakpoint on a blank line jumps to the
  next line with code; one after the last line stays grey (hover it for the
  reason). Step Over (F10), Step Into (F11) and Step Out (Shift+F11) move
  one line at a time; stepping over a tail call enters the callee, because
  the tail call replaced the caller's frame. While stopped, the
  **Variables** view lists the frame's parameters and `let`s (*Locals*) and
  the top-level bindings (*Globals*); click another frame in the Call Stack
  to see its variables, or hover a name in the editor. With *Runtime
  errors* or *Failed assertions* ticked under **Breakpoints**, the program
  stops on the line that fails, with the error shown under it and all
  frames and values still there; Continue then ends it with the error. A program with a
  compile error fails to launch with a dialog, and
  the error text is in the Debug Console. *Run Without Debugging* (Ctrl+F5)
  ignores breakpoints.
- **Troubleshooting:** *Couldn't spawn debug adapter* → `valo-dap` was not
  found: install it (step 1) or set `valo.debugger.path`. To see the
  protocol, set `"valo.trace.debugger": true` and open Output →
  **Valo Debug Adapter**.
- **Other editors:** any DAP client can start `valo-dap` (stdio, no
  arguments) with launch arguments `{ "program": "/path/file.vl" }`. For
  example, Neovim with nvim-dap:

  ```lua
  local dap = require('dap')
  dap.adapters.valo = { type = 'executable', command = 'valo-dap' }
  dap.configurations.valo = {
    { type = 'valo', request = 'launch', name = 'Debug file', program = '${file}' },
  }
  ```

## Settings

| Setting | Default | Meaning |
|---------|---------|---------|
| `valo.server.path` | `""` | path to `valo-lsp`; empty = `target/debug/valo-lsp` of an open checkout, else `PATH` |
| `valo.trace.server` | `"off"` | log LSP traffic (`messages`, `verbose`) |
| `valo.debugger.path` | `""` | path to `valo-dap`; empty = `target/debug/valo-dap` of an open checkout, else `PATH` |
| `valo.trace.debugger` | `false` | log DAP traffic to the **Valo Debug Adapter** output channel |

## Other editors

`valo-lsp` speaks plain LSP over stdio. For example, Helix
(`~/.config/helix/languages.toml`):

```toml
[language-server.valo-lsp]
command = "valo-lsp"

[[language]]
name = "valo"
scope = "source.valo"
file-types = ["vl"]
comment-token = "#"
indent = { tab-width = 4, unit = "    " }
language-servers = ["valo-lsp"]
```
