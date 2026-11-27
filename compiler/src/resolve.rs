//! # Name resolution (`resolve.rs`) — empty until M2
//!
//! **What this file will be:** the pass between parsing and code generation
//! that answers "which declaration does this name refer to?". It will:
//!
//! - rewrite convenience syntax into core forms ("desugaring"): pipes
//!   `x |> f(a)` → `f(x, a)`, partials `f(a, _)` → a lambda, comprehensions
//!   → `map`/`filter`;
//! - build lexical scopes (`let` bindings, function parameters, blocks);
//! - give every global a fixed index and every local a register;
//! - find which variables a closure captures (Lua "upvalues");
//! - check call arity when the callee is known.
//!
//! **Milestones:** M2 (bindings, scopes), M4 (functions, pipes, arity),
//! M6 (closures, `_` partials), M8 (comprehensions). See
//! `.memory/plan-mvp.md`.
//!
//! **Pipeline position:** `ast::Program` in, side tables keyed by
//! `ast::NodeId` out (consumed by `codegen.rs`). M1 has no names, so the pass
//! only exists to keep the pipeline shape stable.
//!
//! **Textbook chapter:** `docs/11-roadmap.md`.

use crate::ast::Program;
use crate::diagnostic::Diagnostic;

/// Resolve names in a program. M1: nothing to resolve.
// RUST: the leading `_` in `_program` tells the compiler the parameter is
// intentionally unused (no warning).
pub fn resolve(_program: &Program) -> Result<(), Vec<Diagnostic>> {
    Ok(())
}
