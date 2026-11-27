# Chapter 12 — Editor Tooling: Highlighting and a Language Server

Chapters 2–9 built a program that reads a whole file and prints errors once.
An editor needs the same knowledge **continuously**: colours while you type,
errors before you save, later "go to definition" and completion. This
chapter explains how editors get that knowledge, using VS Code as the
example, and how valo provides it in [`tooling/`](../tooling/README.md).

There are two layers, and they are built completely differently:

| Layer | Knows | Built from | Runs | valo files |
|-------|-------|------------|------|------------|
| **Syntactic** (highlighting, brackets, comments, indentation, folding) | what the text *looks like* | declarative JSON + regular expressions | inside the editor, no code | `tooling/vscode/syntaxes/`, `language-configuration.json` |
| **Semantic** (errors, definitions, hover, completion, rename) | what the program *means* | the real compiler | a separate process, the **language server** | `tooling/lsp/` (crate `valo-lsp`) |

A VS Code **extension** (`tooling/vscode/`) packages both: it declares the
syntactic layer and starts the language server.

## 12.1 How VS Code learns about a language

An extension is a folder with a `package.json` *manifest*. The manifest's
`contributes` section declares what it adds, without any code:

```jsonc
// tooling/vscode/package.json (abridged)
"contributes": {
  "languages": [{
    "id": "valo",                          // the language id everything else refers to
    "extensions": [".vl"],                 // files that get this language
    "firstLine": "^#!.*\\bvalo\\b",        // …or a shebang line
    "configuration": "./language-configuration.json"
  }],
  "grammars": [{
    "language": "valo",
    "scopeName": "source.valo",
    "path": "./syntaxes/valo.tmLanguage.json"
  }]
}
```

### Language configuration

`language-configuration.json` describes editing behaviour that every
language needs in some form:

- `comments.lineComment: "#"` — what **Ctrl+/** inserts;
- `brackets`, `autoClosingPairs`, `surroundingPairs` — bracket matching,
  typing `(` inserts `)`, selecting text and typing `"` quotes it;
- `onEnterRules` — after a line ending in `:` (or `=>`, `~>`, `as`), Enter
  indents the next line. valo is indentation-based (chapter 3), so this
  matters more than in a brace language;
- `folding.offSide: true` — fold by indentation, Python-style ("off-side
  rule" is the textbook name for indentation-defined blocks).

### TextMate grammars: how highlighting works

Highlighting uses **TextMate grammars**, a format from the TextMate editor
(2004) that VS Code, GitHub, Sublime Text and many others adopted. A grammar
is a list of regular-expression rules; each match gives a piece of text a
**scope name**:

```jsonc
{ "match": "\\b(if|elif|else|then|match)\\b", "name": "keyword.control.valo" }
{ "begin": "\"", "end": "\"", "name": "string.quoted.double.valo",
  "patterns": [ { "begin": "\\{", "end": "\\}", "name": "meta.interpolation.valo",
                  "patterns": [ { "include": "source.valo" } ] } ] }
```

- `match` rules colour one regex match.
- `begin`/`end` rules open a region (a string, a comment) that can contain
  its own `patterns` — this is how `"{x + 1}"` gets valo code highlighted
  *inside* a string: the interpolation region includes the whole grammar
  again.
- **The theme, not the grammar, picks colours.** Scope names are
  hierarchical and conventional (`keyword.control`, `string.quoted`,
  `entity.name.function`, `support.type`, `comment.line`, `constant.numeric`);
  a colour theme maps scope prefixes to colours. Using the conventional names
  makes valo look right in every existing theme.

Command Palette → **Developer: Inspect Editor Tokens and Scopes** shows the
scopes under the cursor; it is the main debugging tool for a grammar.

The limits of this approach follow from the theory of chapter 3: a TextMate
grammar is a set of regular expressions with a stack of open regions, run one
line at a time. It cannot know that `x` in one place is a parameter and in
another a global — that needs name resolution (`compiler/src/resolve.rs`). It
is a *lexer-level* approximation, and that is exactly why it is fast enough
to run on every keystroke. The grammar may also run **ahead of the
compiler**: it already colours syntax that is decided in `.memory/syntax.md`
but not implemented yet (strings, `match`, `def`), so the editor is ready
when the compiler catches up.

(Alternatives: Tree-sitter grammars, used by Neovim, Helix and Zed, are real
incremental parsers; VS Code does not use them for highlighting. Semantic
tokens, §12.6, are VS Code's way to add compiler-accurate colours on top.)

## 12.2 The Language Server Protocol

Before 2016 every editor needed its own plugin for every language, written in
the editor's language: *M editors × N languages* plugins. Microsoft's
**Language Server Protocol (LSP)** cut this to *M + N*: the language side is
one program, the **server**, which any editor's generic **client** talks to.

- **Transport:** the editor starts the server as a child process and talks
  over its stdin/stdout. Each message is a header plus a JSON body:

  ```text
  Content-Length: 85\r\n
  \r\n
  {"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{…}}}
  ```

- **JSON-RPC 2.0** gives three message kinds: *requests* (with an `id`,
  exactly one *response* comes back: "where is the definition of the name at
  line 3, column 7?"), and *notifications* (no `id`, no reply: "file opened",
  "here are the errors in this file").
- **Capabilities:** the first request, `initialize`, exchanges what each side
  supports. The editor only asks for features the server listed, so a server
  can start with almost nothing and grow.

The life of a valo file in the editor:

```text
 VS Code (client)                                   valo-lsp (server)
   │ ── initialize {capabilities: …} ─────────────────▶ │  main.rs
   │ ◀── {capabilities: {textDocumentSync: 1}} ──────── │  server::capabilities
   │ ── initialized ──────────────────────────────────▶ │
   │ ── textDocument/didOpen {uri, version, text} ────▶ │  server::Server::handle_notification
   │                                                    │  → compile_source (chapters 3–6)
   │ ◀── textDocument/publishDiagnostics {uri, […]} ─── │  convert::diagnostic
   │ ── textDocument/didChange {version: 2, text} ────▶ │  (on every edit, again)
   │ ◀── textDocument/publishDiagnostics ───────────── │
   │ ── shutdown ─▶ ◀── null ── exit ─▶                 │  process ends
```

Notice the editor sends the **text**, not just the file name. While the user
types, the file on disk is stale; the server must check what is in the
editor buffer.

## 12.3 valo-lsp: the server

Crate [`tooling/lsp`](../tooling/lsp/src/lib.rs), binary `valo-lsp`. It
depends on `valo-compiler` as a library — the reason the compiler was split
into `lib.rs` + `main.rs` in chapter 9.

| File | Job |
|------|-----|
| `src/main.rs` | stdio transport (`lsp_server::Connection::stdio`), `initialize` handshake |
| `src/server.rs` | main loop: dispatch messages, keep open documents in a `HashMap<Uri, Document>`, re-check on change |
| `src/convert.rs` | `valo_compiler::diagnostic::Diagnostic` → LSP `Diagnostic` |
| `src/line_index.rs` | byte offsets ↔ LSP positions |
| `tests/smoke.rs` | plays the editor against the real server loop, in memory |

Libraries: [`lsp-server`](https://docs.rs/lsp-server) (the transport and
handshake helpers from rust-analyzer) and
[`lsp-types`](https://docs.rs/lsp-types) (a Rust struct for every LSP
message). `lsp-server` is synchronous: one loop reads one message at a time.
The more popular `tower-lsp-server` is asynchronous (tokio); valo's compiler
is synchronous and fast, so the simple loop is enough and keeps the
mechanics visible.

**Diagnostics are the compiler's diagnostics.** `server::check` builds a
`SourceMap` (chapter 2) from the buffer text and calls the same
`compile_source` that `valo check` uses (chapter 9). Every `E####` error, its
label, its `help:` lines and secondary spans appear in the editor with no
extra code. Improving compiler errors (M9) improves the editor for free.
`check` wraps the compiler call in `catch_unwind`, so a compiler bug on
half-typed code costs one round of diagnostics, not the whole server.

## 12.4 Positions: bytes, characters and UTF-16

Chapter 2 chose **byte offsets** for spans. LSP positions are `(line,
character)`, both 0-based, where *character* counts **UTF-16 code units** —
the string encoding of JavaScript, inherited from VS Code. The units differ
as soon as a line contains non-ASCII text:

| Text | UTF-8 bytes | chars | UTF-16 units |
|------|-------------|-------|--------------|
| `a`  | 1 | 1 | 1 |
| `ä`  | 2 | 1 | 1 |
| `😀` | 4 | 1 | 2 |

`LineIndex` (`tooling/lsp/src/line_index.rs`) stores each line's start
offset, finds a line by binary search, then walks the characters of that line
adding `char::len_utf16`. It is a separate type from `SourceMap::line_col`
because that one counts characters, 1-based, for humans. (LSP 3.17 lets
client and server negotiate UTF-8 positions instead, but VS Code only
speaks UTF-16.)

## 12.5 The client: `extension.js`

[`tooling/vscode/extension.js`](../tooling/vscode/extension.js) is the only
code in the extension. VS Code loads it the first time a valo file opens
(`activationEvents: ["onLanguage:valo"]`) and calls `activate`, which creates
a `LanguageClient` from the npm package `vscode-languageclient`:

- **server options** — the command to start: the `valo.server.path`
  setting, else `target/debug/valo-lsp` of an open valo checkout, else
  `valo-lsp` from `PATH`;
- **client options** — which documents to send (`{ language: "valo" }`).

The library does all protocol work. When the server gains a feature (say
`definitionProvider`), VS Code's "Go to Definition" starts working without
any change to the client.

Setting `"valo.trace.server": "verbose"` logs every message to the
**Valo Language Server** output channel — the best way to watch §12.2
happen.

## 12.6 What comes next

The roadmap in [`tooling/lsp/README.md`](../tooling/lsp/README.md) adds one
capability at a time, each built on a part of the compiler that already
exists:

- **Navigation** (go to definition, find references, rename, hover, outline)
  reads `resolve::Resolved`: `uses` lists every name with its span and its
  target (`Res::Local`/`Res::Global`), `locals` and `globals` hold the
  definition spans. A request carries a cursor position; `LineIndex::offset`
  turns it into a byte offset, and the answer is the span of the `Use` that
  contains it.
- **Completion** needs to know which names are in scope *at the cursor*, in a
  file that is half-typed and usually broken — the reason parser error
  recovery (chapter 4) matters so much for tooling.
- **Semantic tokens** let the server colour tokens with resolver knowledge
  (parameter vs global vs built-in), layered over the TextMate colours.
- **Multiple files** (imports) bring incremental recomputation: re-checking
  only what an edit affects. Libraries such as `salsa` (used by
  rust-analyzer) do this by memoising compiler queries.
- **Debugging** uses a sibling protocol, the **Debug Adapter Protocol**
  (DAP): same JSON framing, same client/server split, with requests like
  `setBreakpoints`, `stackTrace`, `next`. A valo debug adapter
  (`tooling/dap`) would drive the VM and needs the bytecode's span
  information (chapter 5), which already maps every instruction to source.

## 12.7 Where to change things

| Task | Where |
|------|-------|
| colour a new keyword or literal | `tooling/vscode/syntaxes/valo.tmLanguage.json` + an example in `samples/showcase.vl` |
| Enter / indentation / bracket behaviour | `tooling/vscode/language-configuration.json` |
| advertise a new LSP feature | `tooling/lsp/src/server.rs` → `capabilities` |
| answer a new request | `server.rs` → `Server::handle_request` |
| change how errors look in the editor | `tooling/lsp/src/convert.rs` |
| how the server binary is found | `tooling/vscode/extension.js` → `serverCommand` |
| build, install, package | [`tooling/README.md`](../tooling/README.md) |

## 12.8 Exercises

1. Turn on `"valo.trace.server": "verbose"`, open a file, type one character
   and find the `didChange` and `publishDiagnostics` messages in the output.
   How big is `didChange` for a 500-line file, and what would
   `TextDocumentSyncKind::INCREMENTAL` change?
2. Once strings exist (M7), write `let s = "😀" + x` (with `x` undefined) and
   check that the squiggle is under `x`, not one column off. Which line of
   `line_index.rs` makes that true? (Until then, the unit test
   `utf16_columns` checks the same thing.)
3. Implement `textDocument/documentSymbol` (the Outline view): advertise
   `document_symbol_provider`, then in `handle_request` parse the document
   (`parse_source`) and return one symbol per top-level `Item::Fn` and
   `Item::Let`.
