//! # Compiled code containers (`chunk.rs`)
//!
//! **What this file is:** the data structures that hold compiled code.
//! A `Proto` (function *prototype*) is one compiled function: its
//! instructions, its constant pool and a table mapping each instruction back
//! to the source text. A `Program` is the whole compiled file: the `<main>`
//! prototype plus what is needed to print error locations.
//!
//! **Pipeline position:** output of `compiler/src/codegen.rs`, input of
//! `vm/src/vm.rs` and `vm/src/disasm.rs`. This module must never depend on
//! compiler types, because a later milestone (M11) writes `Program`s to
//! `.vlc` files and loads them without the compiler (plan §2.3).
//!
//! **Textbook chapter:** `docs/05-bytecode-format.md`.
//!
//! ## Where to change things
//!
//! | I want to…                                   | Look at                                      |
//! |----------------------------------------------|----------------------------------------------|
//! | add a new kind of constant                   | `enum Const`, then `vm.rs` (`LOADK`), `disasm.rs` (`const_repr`, `const_entry`), `codegen.rs` |
//! | store more per-function data (params, types)  | `struct Proto` (+ `ProtoBuilder::new`, `FuncState::new` in codegen) |
//! | change what closures capture (M6)             | `struct UpvalDesc`, `enum UpvalSrc` (+ `CLOSURE` in `vm.rs`) |
//! | store more per-program data (types, M10)     | `struct Program` (+ `ProtoBuilder::program`, `codegen::compile`) |
//! | describe global slots (natives, M4)           | `struct GlobalDesc`, `enum GlobalKind`       |
//! | add nested functions                          | `Proto::protos` (indexed by `CLOSURE` Bx)    |
//! | build bytecode by hand in a VM test          | `ProtoBuilder` (see tests in `vm.rs`)       |
//! | record more debug info per local (types, M10) | `struct LocalInfo` (+ `FuncState::declare_local` in codegen) |
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `Rc<Proto>` / `Rc<str>` — shared, reference-counted ownership (*Rc shared ownership*).
//! - `Vec<T>` — growable array (*Structs*).
//! - builder pattern: methods that take `self` and return `Self` so calls chain (*Ownership and borrowing*).
//! - `binary_search` returning `Ok(i)` / `Err(i)` (*Option and Result*).

use std::rc::Rc;

use crate::opcode::Instr;

/// Half-open byte range `[start, end)` into the program's source text.
///
/// The VM-side twin of `compiler::span::Span` (conversions live in
/// `compiler/src/span.rs`). Byte offsets, not line/column: line and column are
/// computed only when an error is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SrcSpan {
    /// First byte.
    pub start: u32,
    /// One past the last byte.
    pub end: u32,
}

impl SrcSpan {
    /// Construct a span.
    pub fn new(start: u32, end: u32) -> SrcSpan {
        // RUST: `SrcSpan { start, end }` is shorthand for
        // `SrcSpan { start: start, end: end }`.
        SrcSpan { start, end }
    }
}

/// A constant-pool entry.
///
/// Small integers are encoded directly in the instruction (`LOADI`); values
/// that do not fit go into the pool and are loaded with `LOADK index`.
#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    /// Integer too large for `LOADI`'s 16-bit immediate.
    Int(i64),
    /// Float literal (there is no float immediate form).
    Float(f64),
    /// String literal, or the text pieces of an interpolated string (M7).
    /// `LOADK` shares the `Rc`, so loading a string never copies it.
    Str(Rc<str>),
}

/// A compiled function.
///
/// RUST: `Rc<str>` is a shared, immutable string. Cloning it copies a pointer
/// and bumps a counter instead of copying the text, which is cheap for names
/// that are copied into every stack-trace entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Proto {
    /// Display name: `<main>`, a fn name, or `<lambda@l:c>`.
    pub name: Rc<str>,
    /// Number of parameters.
    pub arity: u8,
    /// Number of parameters without a default (`required <= arity`; M4b).
    /// A call may pass `required..=arity` arguments; the VM fills the rest
    /// with `Value::Missing`, and the prologue computes their defaults.
    pub required: u8,
    /// Number of registers the frame needs.
    pub num_regs: u8,
    /// Instructions.
    pub code: Vec<Instr>,
    /// Constant pool (`LOADK` operands).
    pub consts: Vec<Const>,
    /// Source span of each instruction, parallel to `code`
    /// (`spans[pc]` belongs to `code[pc]`). Used for error locations.
    pub spans: Vec<SrcSpan>,
    /// Span of the whole definition.
    pub def_span: SrcSpan,
    /// Functions defined inside this one, in definition order. `CLOSURE A Bx`
    /// creates a closure of `protos[Bx]`. For `<main>` these are the
    /// top-level `fn`s (M4).
    pub protos: Vec<Rc<Proto>>,
    /// Debug info: which register holds which named local, and when
    /// (parameters first, then `let`s in the order they are compiled).
    /// Only the debugger reads it (Variables view, D3).
    pub locals: Vec<LocalInfo>,
    /// What a closure of this function captures (M6): `CLOSURE` in the
    /// *enclosing* function copies one value per entry into
    /// `Closure::captures`; `GETUPVAL A B` reads `captures[B]`.
    pub upvals: Vec<UpvalDesc>,
    /// Annotation checks (M10): `CHECKTYPE A Bx` checks `R[A]` against
    /// `checks[Bx]` (parameters in the prologue, annotated `let`s).
    pub checks: Vec<TypeCheck>,
    /// Declared return type (M10); `RETURN` checks the result against it,
    /// also after tail calls (`Frame::ret_checks`).
    pub ret_type: Option<Type>,
}

/// A type annotation as the VM checks it (M10, plan D13).
///
/// Strict, like Pydantic's strict mode: no coercion (an `int` is not a
/// `float`). Type variables and `any` accept everything; a function type
/// checks only that the value can be called with that many arguments.
/// Deviation from plan §2.3 (`Program.types` + `TypeId`): each check carries
/// its own small tree, because the VM keeps no `Program` while running.
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// `any`.
    Any,
    /// A type variable of a generic fn (`T` in `fn first[T](…)`), or the
    /// unchecked error type `error` of `T ! error`: accepts every value.
    Var(Rc<str>),
    /// `()`.
    Unit,
    /// `int`.
    Int,
    /// `float`.
    Float,
    /// `bool`.
    Bool,
    /// `str`.
    Str,
    /// `none` (on its own, or as a member of `T | none`).
    None,
    /// `list[T]`: every element is checked (plan Q9).
    List(Box<Type>),
    /// `A | B | …` (flattened; `T | none` is the optional type, S9).
    Union(Vec<Type>),
    /// `T ! E`: an `Err` is accepted as it is, `Ok(v)` needs `v: T`.
    Result(Box<Type>, Box<Type>),
    /// `(A, B) -> C`: a function that accepts that many arguments.
    Fn(Vec<Type>, Box<Type>),
}

impl Type {
    /// Whether the check accepts every value (so `list[T]` need not visit
    /// its elements).
    pub fn accepts_all(&self) -> bool {
        matches!(self, Type::Any | Type::Var(_))
    }

    /// Binding strength for printing: `->` < `!` < `|` < everything else.
    fn level(&self) -> u8 {
        match self {
            Type::Fn(..) => 0,
            Type::Result(..) => 1,
            Type::Union(_) => 2,
            _ => 3,
        }
    }

    fn fmt_at(&self, f: &mut std::fmt::Formatter<'_>, min: u8) -> std::fmt::Result {
        if self.level() < min {
            write!(f, "(")?;
            self.fmt_at(f, 0)?;
            return write!(f, ")");
        }
        match self {
            Type::Any => write!(f, "any"),
            Type::Var(n) => write!(f, "{n}"),
            Type::Unit => write!(f, "()"),
            Type::Int => write!(f, "int"),
            Type::Float => write!(f, "float"),
            Type::Bool => write!(f, "bool"),
            Type::Str => write!(f, "str"),
            Type::None => write!(f, "none"),
            Type::List(t) => {
                write!(f, "list[")?;
                t.fmt_at(f, 0)?;
                write!(f, "]")
            }
            Type::Union(ts) => {
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    t.fmt_at(f, 3)?;
                }
                Ok(())
            }
            Type::Result(t, e) => {
                t.fmt_at(f, 2)?;
                write!(f, " ! ")?;
                e.fmt_at(f, 2)
            }
            Type::Fn(params, ret) => {
                match params.as_slice() {
                    [p] if p.level() > 0 => p.fmt_at(f, 1)?,
                    _ => {
                        write!(f, "(")?;
                        for (i, p) in params.iter().enumerate() {
                            if i > 0 {
                                write!(f, ", ")?;
                            }
                            p.fmt_at(f, 0)?;
                        }
                        write!(f, ")")?;
                    }
                }
                write!(f, " -> ")?;
                ret.fmt_at(f, 0)
            }
        }
    }
}

/// User spelling (syntax.md S4): `list[int]`, `int | none`, `int ! error`,
/// `(int, int) -> int`.
impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_at(f, 0)
    }
}

/// One `CHECKTYPE` site: the type and what is checked, for the message
/// (`parameter n of fact`, `binding x`).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeCheck {
    /// Required type.
    pub ty: Type,
    /// Subject of the check, as named in R0204 messages.
    pub what: Rc<str>,
}

/// One captured value of a closure (Lua's `Upvaldesc` in `lobject.h`).
///
/// Captures are copied when the closure is created (plan D5): valo
/// bindings are immutable, so there are no open upvalues and no `CLOSE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpvalDesc {
    /// Where the enclosing function finds the value.
    pub src: UpvalSrc,
    /// Source name of the captured binding (traces, debugger, disasm).
    pub name: Rc<str>,
}

/// Where `CLOSURE` takes a captured value from, relative to the function
/// that executes the `CLOSURE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpvalSrc {
    /// Its register `R[i]`.
    Reg(u8),
    /// Its own capture `captures[i]`.
    Upval(u8),
    /// Its own closure: a lambda inside a local fn that calls that fn.
    /// (Deviation from Lua's `instack`/`idx` pair; it keeps a closure from
    /// ever capturing itself, so closures form no `Rc` cycles.)
    Current,
}

/// One named local variable of a function: lives in register `reg` while
/// `start_pc <= pc < end_pc` (Lua's `LocVar` in `lfunc.h`; DWARF location
/// lists play the same role in native code).
///
/// Registers have no names at run time; without this table a debugger
/// could only show `R0`, `R1`, … A shadowing `let x` gets its own entry;
/// both may be live at once, and the later one is the visible one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalInfo {
    /// Source name.
    pub name: Rc<str>,
    /// Register (`R[reg]` of the function's frame).
    pub reg: u8,
    /// First instruction at which the variable holds its value: `0` for a
    /// parameter, the instruction after the initialiser for a `let`.
    pub start_pc: u32,
    /// First instruction after its scope (the end of its block).
    pub end_pc: u32,
}

impl LocalInfo {
    /// Whether the variable holds its value when instruction `pc` runs.
    pub fn live_at(&self, pc: usize) -> bool {
        (self.start_pc as usize..self.end_pc as usize).contains(&pc)
    }
}

/// Index of a native function in `builtins::NATIVES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NativeId(pub u16);

/// What a global slot holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalKind {
    /// A top-level `let` or `fn`, set by `SETGLOBAL`.
    User,
    /// A built-in function; the VM fills the slot before running `<main>`.
    Native(NativeId),
}

/// Description of one global slot (plan D4). Built-in functions, top-level
/// `fn`s and top-level `let` bindings live in globals;
/// `GETGLOBAL`/`SETGLOBAL` address them by index (`Bx`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalDesc {
    /// Source name (shadowed top-level names get one slot each, so names may
    /// repeat). Used in error messages and the disassembly.
    pub name: Rc<str>,
    /// User slot or built-in.
    pub kind: GlobalKind,
}

/// A complete compiled program.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    /// Top-level code.
    pub main: Rc<Proto>,
    /// Global slots, indexed by `GETGLOBAL`/`SETGLOBAL` operand `Bx`.
    pub globals: Vec<GlobalDesc>,
    /// Source file name, for messages.
    pub source_name: Rc<str>,
    /// Byte offset at which each source line starts (line 1 at index 0).
    pub line_starts: Vec<u32>,
}

impl Program {
    /// 1-based line and byte-based column of a byte offset. Use the compiler's
    /// source map instead when the source text is available (it counts
    /// columns in characters).
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        // `binary_search` finds `offset` in the sorted line-start table:
        // `Ok(i)`  = offset is exactly the start of line i;
        // `Err(i)` = offset would be inserted at i, so it lies on line i - 1.
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (line as u32 + 1, offset - self.line_starts[line] + 1)
    }
}

/// Byte offsets of line starts in `text` (always starts with 0).
pub fn line_starts(text: &str) -> Vec<u32> {
    // RUST: an iterator chain. `once(0)` yields 0; `match_indices('\n')` yields
    // the position of every newline; each line starts one byte after a
    // newline; `collect()` gathers everything into a `Vec<u32>`.
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i as u32 + 1))
        .collect()
}

/// Incremental builder for hand-assembled prototypes, used by VM unit tests
/// to exercise opcodes before the parser supports them.
///
/// ```
/// use valo_vm::chunk::ProtoBuilder;
/// use valo_vm::opcode::{Instr, OpCode};
/// let program = ProtoBuilder::new("<main>", 1)
///     .op(Instr::asbx(OpCode::LOADI, 0, 42))
///     .op(Instr::abc(OpCode::RETURN, 0, 0, 0))
///     .program();
/// assert_eq!(program.main.code.len(), 2);
/// ```
#[derive(Debug)]
pub struct ProtoBuilder {
    proto: Proto,
    globals: Vec<GlobalDesc>,
}

impl ProtoBuilder {
    /// Start a prototype with the given name and register count.
    pub fn new(name: &str, num_regs: u8) -> ProtoBuilder {
        ProtoBuilder {
            proto: Proto {
                // RUST: `.into()` converts `&str` into `Rc<str>` (via the
                // standard `From` trait); the target type is inferred.
                name: name.into(),
                arity: 0,
                required: 0,
                num_regs,
                code: Vec::new(),
                consts: Vec::new(),
                spans: Vec::new(),
                def_span: SrcSpan::default(),
                protos: Vec::new(),
                locals: Vec::new(),
                upvals: Vec::new(),
                checks: Vec::new(),
                ret_type: None,
            },
            globals: Vec::new(),
        }
    }

    /// Append an instruction with an empty span.
    // RUST: `mut self` takes the builder by value (ownership moves in), changes
    // it, and hands it back, so calls can be chained: `.op(..).op(..)`.
    pub fn op(mut self, instr: Instr) -> ProtoBuilder {
        self.proto.code.push(instr);
        self.proto.spans.push(SrcSpan::default());
        self
    }

    /// Append a constant.
    pub fn konst(mut self, k: Const) -> ProtoBuilder {
        self.proto.consts.push(k);
        self
    }

    /// Set the number of parameters (all required).
    pub fn arity(mut self, n: u8) -> ProtoBuilder {
        self.proto.arity = n;
        self.proto.required = n;
        self
    }

    /// Set the number of parameters without a default (after `arity`).
    pub fn required(mut self, n: u8) -> ProtoBuilder {
        self.proto.required = n;
        self
    }

    /// Record a named local in register `reg`, live for `start..end`.
    pub fn local(mut self, name: &str, reg: u8, start: u32, end: u32) -> ProtoBuilder {
        self.proto.locals.push(LocalInfo {
            name: name.into(),
            reg,
            start_pc: start,
            end_pc: end,
        });
        self
    }

    /// Declare the next captured value (`captures[i]` of closures of this
    /// prototype).
    pub fn upval(mut self, name: &str, src: UpvalSrc) -> ProtoBuilder {
        self.proto.upvals.push(UpvalDesc {
            src,
            name: name.into(),
        });
        self
    }

    /// Add an annotation check (the next `CHECKTYPE` index, M10).
    pub fn check(mut self, what: &str, ty: Type) -> ProtoBuilder {
        self.proto.checks.push(TypeCheck {
            ty,
            what: what.into(),
        });
        self
    }

    /// Declare the return type (M10).
    pub fn ret(mut self, ty: Type) -> ProtoBuilder {
        self.proto.ret_type = Some(ty);
        self
    }

    /// Add a nested prototype (the next `CLOSURE` index).
    pub fn proto(mut self, child: Proto) -> ProtoBuilder {
        self.proto.protos.push(Rc::new(child));
        self
    }

    /// Declare the next global slot (used by `program`).
    pub fn global(mut self, name: &str) -> ProtoBuilder {
        self.globals.push(GlobalDesc {
            name: name.into(),
            kind: GlobalKind::User,
        });
        self
    }

    /// Declare the next global slot as the built-in `NATIVES[id]`.
    pub fn native_global(mut self, id: u16) -> ProtoBuilder {
        let name = crate::builtins::NATIVES
            .get(id as usize)
            .map_or("<native>", |n| n.name);
        self.globals.push(GlobalDesc {
            name: name.into(),
            kind: GlobalKind::Native(NativeId(id)),
        });
        self
    }

    /// Finish the prototype.
    pub fn build(self) -> Proto {
        self.proto
    }

    /// Wrap the prototype as the `main` of a program.
    pub fn program(self) -> Program {
        Program {
            main: Rc::new(self.proto),
            globals: self.globals,
            source_name: "<test>".into(),
            line_starts: vec![0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_and_line_col() {
        let ls = line_starts("ab\ncd\n\nx");
        assert_eq!(ls, vec![0, 3, 6, 7]);
        let p = Program {
            main: Rc::new(ProtoBuilder::new("<main>", 1).build()),
            globals: Vec::new(),
            source_name: "t".into(),
            line_starts: ls,
        };
        assert_eq!(p.line_col(0), (1, 1));
        assert_eq!(p.line_col(4), (2, 2));
        assert_eq!(p.line_col(6), (3, 1));
        assert_eq!(p.line_col(7), (4, 1));
    }
}
