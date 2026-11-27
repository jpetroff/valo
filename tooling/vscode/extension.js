// @ts-check
// # Valo for VS Code — the language client (`extension.js`)
//
// What this file is: the only code in the extension. Highlighting, comment
// toggling, bracket matching and indentation are declared in package.json,
// language-configuration.json and syntaxes/valo.tmLanguage.json — VS Code
// applies them without running any code.
//
// This file adds the *semantic* part: when a `.vl` file is opened
// (`activationEvents: onLanguage:valo` in package.json), VS Code calls
// `activate`, which starts the `valo-lsp` binary (tooling/lsp) as a child
// process and connects to it with `vscode-languageclient`. From then on the
// library forwards editor events (file opened, text changed) to the server as
// LSP messages and turns the server's replies (diagnostics, later hovers,
// definitions, …) into squiggles, tooltips and jumps. Adding a feature to
// the server usually needs no change here: the client discovers it from the
// server's capabilities.
//
// It also connects VS Code's debugger UI to `valo-dap` (tooling/dap): the
// `debuggers` contribution in package.json declares the debug type `valo`;
// `ValoDebugAdapterFactory` tells VS Code which program speaks the Debug
// Adapter Protocol for it, and `ValoConfigurationProvider` fills in a launch
// configuration when there is no launch.json (F5 on an open `.vl` file).
//
// Textbook chapters: docs/12-editor-tooling.md (LSP), docs/13-debugger.md (DAP).
//
// Where to change things:
//   which binaries are started   → `findBinary` (`valo-lsp`, `valo-dap`)
//   which files the server sees  → `documentSelector` in `startClient`
//   launch.json defaults         → `ValoConfigurationProvider`, package.json `debuggers`
//   new commands                 → package.json `contributes.commands` + `registerCommand` here

const fs = require('fs');
const path = require('path');
const vscode = require('vscode');
const { LanguageClient } = require('vscode-languageclient/node');

/** @type {LanguageClient | undefined} */
let client;

/**
 * Pick the executable of a valo tool (`valo-lsp`, `valo-dap`):
 * 1. the setting `valo.<settingKey>`, if set;
 * 2. `target/debug/<name>` inside an open workspace folder (developing valo itself:
 *    `cargo build` and restart the server / debug session to try a change);
 * 3. `<name>` from PATH (`cargo install --path tooling/lsp` or `tooling/dap`).
 * @param {string} settingKey
 * @param {string} name
 * @returns {string}
 */
function findBinary(settingKey, name) {
  const configured = vscode.workspace.getConfiguration('valo').get(settingKey, '');
  if (configured) {
    return configured;
  }
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    const candidate = path.join(folder.uri.fsPath, 'target', 'debug', name);
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return name;
}

/** @returns {string} */
function serverCommand() {
  return findBinary('server.path', 'valo-lsp');
}

/**
 * Starts one `valo-dap` process per debug session. VS Code then talks DAP to
 * it over stdin/stdout, exactly like the language client talks LSP.
 * @implements {vscode.DebugAdapterDescriptorFactory}
 */
class ValoDebugAdapterFactory {
  createDebugAdapterDescriptor() {
    return new vscode.DebugAdapterExecutable(findBinary('debugger.path', 'valo-dap'), []);
  }
}

/**
 * Called before every valo debug session starts. With no launch.json, F5
 * passes an empty configuration: debug the `.vl` file in the active editor.
 * @implements {vscode.DebugConfigurationProvider}
 */
class ValoConfigurationProvider {
  /**
   * @param {vscode.WorkspaceFolder | undefined} _folder
   * @param {vscode.DebugConfiguration} config
   */
  resolveDebugConfiguration(_folder, config) {
    if (!config.type && !config.request && !config.name) {
      const editor = vscode.window.activeTextEditor;
      if (editor && editor.document.languageId === 'valo') {
        config.type = 'valo';
        config.request = 'launch';
        config.name = 'Debug current Valo file';
        config.program = '${file}';
      }
    }
    if (!config.program) {
      vscode.window.showInformationMessage('Valo: open a .vl file, or set `program` in launch.json.');
      return undefined; // cancels the session
    }
    return config;
  }
}

/**
 * With `"valo.trace.debugger": true`, log every DAP message between VS Code
 * and valo-dap to the **Valo Debug Adapter** output channel (for learning).
 * @param {vscode.OutputChannel} output
 * @implements {vscode.DebugAdapterTrackerFactory}
 */
function traceFactory(output) {
  return {
    createDebugAdapterTracker() {
      if (!vscode.workspace.getConfiguration('valo').get('trace.debugger', false)) {
        return undefined;
      }
      return {
        onWillReceiveMessage: (/** @type {unknown} */ m) => output.appendLine(`→ ${JSON.stringify(m)}`),
        onDidSendMessage: (/** @type {unknown} */ m) => output.appendLine(`← ${JSON.stringify(m)}`),
        onError: (/** @type {Error} */ e) => output.appendLine(`error: ${e.message}`),
        onExit: (/** @type {number | undefined} */ code) => output.appendLine(`valo-dap exited (${code})`),
      };
    },
  };
}

/**
 * Run or debug the file shown in the editor (the ▷ buttons in the editor title).
 * @param {vscode.Uri | undefined} uri
 * @param {boolean} noDebug
 */
function startFile(uri, noDebug) {
  const target = uri ?? vscode.window.activeTextEditor?.document.uri;
  if (!target) {
    return;
  }
  vscode.debug.startDebugging(
    vscode.workspace.getWorkspaceFolder(target),
    {
      type: 'valo',
      request: 'launch',
      name: noDebug ? 'Run Valo file' : 'Debug Valo file',
      program: target.fsPath,
    },
    { noDebug },
  );
}

/** @param {vscode.OutputChannel} output */
async function startClient(output) {
  const command = serverCommand();
  output.appendLine(`starting ${command}`);
  client = new LanguageClient(
    'valo', // id: also the prefix of the `valo.trace.server` setting
    'Valo Language Server',
    // Server: a child process speaking LSP over its stdin/stdout.
    { command, args: [] },
    {
      // Client: send the server every document whose language id is `valo`
      // (package.json maps `.vl` files to it), on disk or unsaved.
      documentSelector: [{ language: 'valo' }],
      outputChannel: output,
    },
  );
  try {
    await client.start();
  } catch (err) {
    client = undefined;
    vscode.window.showErrorMessage(
      `Valo: could not start the language server (${command}). ` +
        'Install it with `cargo install --path tooling/lsp` or set `valo.server.path`. ' +
        `${err}`,
    );
  }
}

/** @param {vscode.ExtensionContext} context */
async function activate(context) {
  const output = vscode.window.createOutputChannel('Valo Language Server');
  const dapOutput = vscode.window.createOutputChannel('Valo Debug Adapter');
  context.subscriptions.push(output, dapOutput);
  context.subscriptions.push(
    vscode.commands.registerCommand('valo.restartServer', async () => {
      await client?.stop();
      await startClient(output);
    }),
    // Debugger (docs/13-debugger.md): adapter process + launch defaults.
    vscode.debug.registerDebugAdapterDescriptorFactory('valo', new ValoDebugAdapterFactory()),
    vscode.debug.registerDebugConfigurationProvider('valo', new ValoConfigurationProvider()),
    vscode.debug.registerDebugAdapterTrackerFactory('valo', traceFactory(dapOutput)),
    vscode.commands.registerCommand('valo.runFile', (uri) => startFile(uri, true)),
    vscode.commands.registerCommand('valo.debugFile', (uri) => startFile(uri, false)),
  );
  await startClient(output);
}

async function deactivate() {
  await client?.stop();
}

module.exports = { activate, deactivate };
