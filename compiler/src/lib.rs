//! # `valo_compiler` — from valo source (`.vl`) to bytecode
//!
//! This crate turns source text into a `valo_vm::chunk::Program` that the VM
//! crate executes. It also contains the `valo` command-line tool
//! (`src/main.rs`).
//!
//! ## The pipeline (plan §1.1)
//!
//! ```text
//!  source text ──lexer──▶ tokens ──parser──▶ AST ──desugar──▶ AST ──resolve──▶ (names) ──codegen──▶ Program
//!   span.rs      lexer.rs  token.rs  parser.rs ast.rs desugar.rs     resolve.rs           codegen.rs   (valo_vm)
//!                   └──────────── errors: diagnostic.rs (Diagnostic, E#### codes) ───────────┘
//! ```
//!
//! Each stage returns *all* the errors it found; the pipeline stops after the
//! first stage that reported any (see `parse_source` and `compile_source`
//! below), because later stages would only produce confusing follow-up errors.
//!
//! ## Map of the crate
//!
//! | File              | Contents                                                     | Chapter in `docs/` |
//! |-------------------|--------------------------------------------------------------|--------------------|
//! | [`span`]          | byte ranges (`Span`), source text + line index (`SourceMap`)  | 02-source-text-and-spans |
//! | [`token`]         | token kinds (`TokenKind`, `Keyword`)                          | 03-lexical-analysis |
//! | [`lexer`]         | text → tokens, indentation → INDENT/DEDENT                    | 03-lexical-analysis |
//! | [`ast`]           | syntax tree (`Program`, `Item`, `Expr`, `BinOp`)              | 04-syntax-analysis |
//! | [`parser`]        | tokens → AST (recursive descent + Pratt)                     | 04-syntax-analysis |
//! | [`desugar`]       | rewrites pipes into calls                                    | 04-syntax-analysis |
//! | [`resolve`]       | names and scopes: which binding a name refers to             | 11-roadmap |
//! | [`codegen`]       | AST → register bytecode                                      | 06-code-generation |
//! | [`diagnostic`]    | error type and pretty printing                               | 08-errors-and-diagnostics |
//! | `main.rs`         | the `valo` CLI: `run`, `check`, `build`                      | 09-driver-and-cli |
//! | `tests/golden.rs` | runs every program in `tests/golden/` and compares output    | 10-testing |
//!
//! Comment conventions (`// RUST:`, `// EXTEND:`) are described in the
//! `valo_vm` crate docs (`vm/src/lib.rs`).

pub mod ast;
pub mod codegen;
pub mod desugar;
pub mod diagnostic;
pub mod lexer;
pub mod parser;
pub mod resolve;
pub mod span;
pub mod token;

// RUST: `use` brings names into scope so we can write `Diagnostic` instead of
// `diagnostic::Diagnostic`.
use diagnostic::Diagnostic;
use span::SourceMap;
use valo_vm::chunk::Program;

/// Version of this crate, as declared in `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Options for [`compile_source`].
/// EXTEND: compiler flags (e.g. optimisation switches) go here.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompileOptions {
    /// Print the value of every top-level expression (`valo run --echo`).
    pub echo: bool,
}

/// Lex and parse; stops at the first stage with errors.
///
/// RUST: the return type `Result<A, B>` means "either `Ok(A)` or `Err(B)`".
/// Here: the syntax tree, or the list of errors.
pub fn parse_source(sm: &SourceMap) -> Result<ast::Program, Vec<Diagnostic>> {
    let lexed = lexer::lex(&sm.text);
    if !lexed.diags.is_empty() {
        return Err(lexed.diags);
    }
    let (program, diags) = parser::parse(&lexed.tokens);
    if !diags.is_empty() {
        return Err(diags);
    }
    Ok(program)
}

/// Lex, parse, desugar and resolve names (used by `--dump resolved`).
pub fn resolve_source(
    sm: &SourceMap,
) -> Result<(ast::Program, resolve::Resolved), Vec<Diagnostic>> {
    let program = desugar::desugar(parse_source(sm)?);
    let resolved = resolve::resolve(&program)?;
    Ok((program, resolved))
}

/// Full pipeline: source to bytecode.
///
/// EXTEND: a new compiler pass (e.g. type checking, M10) is one more line
/// between `resolve` and `codegen`.
pub fn compile_source(sm: &SourceMap, opts: CompileOptions) -> Result<Program, Vec<Diagnostic>> {
    // RUST: `?` returns early with the `Err` if a stage failed.
    let (program, resolved) = resolve_source(sm)?;
    codegen::compile(
        &program,
        &resolved,
        sm,
        codegen::Options { echo: opts.echo },
    )
}
