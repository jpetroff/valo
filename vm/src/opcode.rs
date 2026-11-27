//! # Instruction encoding (`opcode.rs`)
//!
//! **What this file is:** the definition of the VM's machine language. Every
//! instruction the compiler emits and the VM executes is one 32-bit number
//! (`Instr`) whose lowest byte says *what to do* (`OpCode`) and whose other
//! bytes say *with which registers/constants* (the operands `A`, `B`, `C`,
//! `Bx`, `sBx`, `sJ`).
//!
//! **Pipeline position:** shared vocabulary between
//! `compiler/src/codegen.rs` (writes instructions) and `vm/src/vm.rs`
//! (reads and executes them). `vm/src/disasm.rs` prints them.
//!
//! **Textbook chapter:** `docs/05-bytecode-format.md`.
//!
//! ## Where to change things
//!
//! | I want to…                               | Look at                                                     |
//! |------------------------------------------|-------------------------------------------------------------|
//! | add a new instruction                    | the `opcodes! { ... }` list below (append; keep numbers dense), then `vm.rs` `execute`, then `codegen.rs` |
//! | change how operands are packed into bits | `Instr::abc` / `abx` / `asbx` / `sj` and the getters `a()`, `b()`… |
//! | change what the disassembler prints      | `vm/src/disasm.rs` (uses `OpCode::name` and `OpCode::format`) |
//!
//! ## Layout (Lua 5.x style, byte-aligned fields; plan §2.1, D2)
//!
//! ```text
//!  bit 31            24 23            16 15             8 7              0
//!      +---------------+---------------+---------------+---------------+
//! iABC |       C       |       B       |       A       |    opcode     |
//! iABx |           Bx (u16)            |       A       |    opcode     |
//! iAsBx|           sBx (i16)           |       A       |    opcode     |
//! isJ  |                 sJ (i24, two's complement)    |    opcode     |
//!      +---------------+---------------+---------------+---------------+
//! ```
//!
//! Jumps are relative to the next instruction: `pc += sJ`.
//!
//! ## Rust features used here (see `docs/01-rust-primer.md`)
//!
//! - `macro_rules!` — a macro that generates the `OpCode` enum and three
//!   lookup functions from one list, so they can never disagree (*Macros*).
//! - `#[repr(u8)]` enum — each variant is stored as one byte (*Enums*).
//! - `impl TryFrom<u8>` — fallible conversion byte → opcode (*Traits, impl and derive*).
//! - bit operators `<<`, `>>`, `|`, `&` and `as` casts (*Integer types and casts*).
//! - `proptest!` — property-based tests at the bottom (*Tests*).

use std::fmt;

/// Operand layout of an opcode, used by the disassembler and `--trace-exec`.
///
/// RUST: a plain `enum` with no data in its variants is like a C enum.
/// `#[derive(...)]` asks the compiler to write common trait impls for us:
/// `Debug` (printable with `{:?}`), `Clone`/`Copy` (cheap to duplicate),
/// `PartialEq`/`Eq` (comparable with `==`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `A` only.
    A,
    /// `A B`.
    AB,
    /// `A B C`.
    ABC,
    /// `A Bx` (unsigned 16-bit).
    ABx,
    /// `A sBx` (signed 16-bit).
    AsBx,
    /// `sJ` (signed 24-bit jump).
    SJ,
}

// RUST: `macro_rules!` defines a macro: code that writes code at compile time.
// The pattern `$( ... )*` matches "zero or more repetitions" of
// `<doc comments> NAME = NUMBER, FORMAT;` in the `opcodes! { ... }` list below.
// For every repetition it emits one enum variant plus one arm in each `match`.
// Result: the opcode list is written exactly once.
macro_rules! opcodes {
    ($( $(#[$doc:meta])* $name:ident = $val:literal, $fmt:ident ;)*) => {
        /// VM operation codes. All MVP opcodes are declared up front so the
        /// numbering stays stable; the interpreter implements them milestone
        /// by milestone (see `.memory/plan-mvp.md` §2.2).
        // RUST: `#[repr(u8)]` stores each variant as the byte given after `=`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum OpCode {
            $( $(#[$doc])* $name = $val, )*
        }

        impl OpCode {
            /// Every opcode, in numeric order.
            // RUST: `&'static [OpCode]` = a slice that lives for the whole
            // program (it is baked into the binary).
            pub const ALL: &'static [OpCode] = &[$(OpCode::$name),*];

            /// Upper-case mnemonic used by the disassembler.
            pub fn name(self) -> &'static str {
                // RUST: `stringify!(LOADK)` turns the identifier into "LOADK".
                match self { $(OpCode::$name => stringify!($name),)* }
            }

            /// Operand layout.
            pub fn format(self) -> Format {
                match self { $(OpCode::$name => Format::$fmt,)* }
            }
        }

        // RUST: implementing the standard `TryFrom` trait lets callers write
        // `OpCode::try_from(byte)`, which returns `Ok(op)` or `Err(byte)`.
        impl TryFrom<u8> for OpCode {
            type Error = u8;
            fn try_from(b: u8) -> Result<Self, u8> {
                match b {
                    $($val => Ok(OpCode::$name),)*
                    other => Err(other),
                }
            }
        }
    };
}

// The instruction set. Comments after `M#` say in which milestone the VM
// starts executing the opcode (`.memory/plan-mvp.md`).
// EXTEND: to add an opcode, append a line here with the next free number,
// give it a doc comment, then handle it in `vm/src/vm.rs` (`execute`) and emit
// it from `compiler/src/codegen.rs`. The test `opcode_numbering_is_dense`
// fails if numbers have gaps or are out of order.
opcodes! {
    /// `R[A] = ()`.
    LOADUNIT = 0, A;
    /// Return `R[A]` from the current function.
    RETURN = 1, A;
    /// `R[A] = Int(sBx)`.
    LOADI = 2, AsBx;
    /// `R[A] = K[Bx]`.
    LOADK = 3, ABx;
    /// `R[A] = R[B] + R[C]`.
    ADD = 4, ABC;
    /// `R[A] = R[B] - R[C]`.
    SUB = 5, ABC;
    /// `R[A] = R[B] * R[C]`.
    MUL = 6, ABC;
    /// `R[A] = R[B] / R[C]` (true division, always float; M3).
    DIV = 7, ABC;
    /// `R[A] = R[B] // R[C]` (floor division).
    IDIV = 8, ABC;
    /// `R[A] = R[B] % R[C]` (floor modulo, sign of divisor).
    MOD = 9, ABC;
    /// `R[A] = R[B] ** R[C]`.
    POW = 10, ABC;
    /// `R[A] = -R[B]`.
    NEG = 11, AB;
    /// Debug aid for `valo run --echo`: print `R[A]`.
    ECHO = 12, A;
    /// `R[A] = R[B]` (M2).
    MOVE = 13, AB;
    /// `R[A] = G[Bx]` (M2).
    GETGLOBAL = 14, ABx;
    /// `G[Bx] = R[A]` (M2).
    SETGLOBAL = 15, ABx;
    /// `R[A] = Bool(B != 0)` (M3).
    LOADBOOL = 16, AB;
    /// `R[A] = R[B] == R[C]` (M3).
    EQ = 17, ABC;
    /// `R[A] = R[B] != R[C]` (M3).
    NE = 18, ABC;
    /// `R[A] = R[B] < R[C]` (M3).
    LT = 19, ABC;
    /// `R[A] = R[B] <= R[C]` (M3).
    LE = 20, ABC;
    /// `R[A] = not R[B]` (M3).
    NOT = 21, AB;
    /// If `R[A] != Bool(B)` skip the next instruction (M3).
    TEST = 22, AB;
    /// `pc += sJ` (M3).
    JMP = 23, SJ;
    /// `R[A] = R[A](R[A+1..=A+B])` (M4).
    CALL = 24, AB;
    /// Tail call reusing the current frame (M5).
    TAILCALL = 25, AB;
    /// `R[A] = closure(protos[Bx])` (M4).
    CLOSURE = 26, ABx;
    /// `R[A] = captures[B]` (M6).
    GETUPVAL = 27, AB;
    /// `R[A] = current closure` (M6).
    GETSELF = 28, A;
    /// N-ary string interpolation (M7).
    CONCAT = 29, ABC;
    /// `R[A] = none` (M7).
    LOADNONE = 30, A;
    /// Postfix `?` (M7).
    TRY = 31, ABC;
    /// `R[A] = [R[B..B+C]]` (M8).
    NEWLIST = 32, ABC;
    /// `R[A] = R[B][R[C]]` (M8).
    INDEX = 33, ABC;
    /// `R[A] = (R[B..B+C])` (M8b).
    NEWTUPLE = 34, ABC;
    /// Dict from C key/value pairs (M8b).
    NEWDICT = 35, ABC;
    /// `R[A] = R[B].K[C]` (M8b).
    GETFIELD = 36, ABC;
    /// `assert` (M3).
    ASSERT = 37, AB;
    /// `R[A] = Missing` (M4b).
    LOADMISSING = 38, A;
    /// `if R[A] != Missing { pc += sBx }` (M4b).
    JMPIFSET = 39, AsBx;
    /// Runtime type check (M10).
    CHECKTYPE = 40, ABx;
}

// RUST: implementing `fmt::Display` makes `OpCode` usable in `format!("{op}")`.
// `f.pad(...)` (instead of `write!`) respects width flags like `{:<9}`.
impl fmt::Display for OpCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.name())
    }
}

/// One encoded 32-bit instruction.
///
/// RUST: a "tuple struct" with one unnamed field (`self.0`).
/// `#[repr(transparent)]` guarantees it has exactly the memory layout of a
/// `u32`, which will matter when bytecode is written to files (M11).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Instr(pub u32);

/// Largest positive `sJ` offset.
pub const MAX_SJ: i32 = (1 << 23) - 1;
/// Smallest `sJ` offset.
pub const MIN_SJ: i32 = -(1 << 23);

// Constructors ("encoders") and getters ("decoders"). Every field is a fixed
// slice of the 32 bits: shift it into place with `<<` when encoding, shift it
// back with `>>` and truncate with `as u8` / `as u16` when decoding.
impl Instr {
    /// `iABC` instruction.
    pub fn abc(op: OpCode, a: u8, b: u8, c: u8) -> Instr {
        // RUST: `x as u32` converts (casts) between integer types.
        Instr(op as u32 | (a as u32) << 8 | (b as u32) << 16 | (c as u32) << 24)
    }

    /// `iABx` instruction.
    pub fn abx(op: OpCode, a: u8, bx: u16) -> Instr {
        Instr(op as u32 | (a as u32) << 8 | (bx as u32) << 16)
    }

    /// `iAsBx` instruction.
    pub fn asbx(op: OpCode, a: u8, sbx: i16) -> Instr {
        // `sbx as u16` keeps the same 16 bits (two's complement); `sbx()`
        // reverses it.
        Instr::abx(op, a, sbx as u16)
    }

    /// `isJ` instruction. `sj` must be within [`MIN_SJ`, `MAX_SJ`].
    pub fn sj(op: OpCode, sj: i32) -> Instr {
        // RUST: `debug_assert!` checks only in debug builds (tests, `cargo run`).
        debug_assert!((MIN_SJ..=MAX_SJ).contains(&sj), "jump offset out of range");
        Instr(op as u32 | ((sj as u32) & 0x00FF_FFFF) << 8)
    }

    /// Raw opcode byte.
    pub fn op_byte(self) -> u8 {
        self.0 as u8
    }

    /// Decoded opcode, or the raw byte if it is not a valid opcode.
    pub fn op(self) -> Result<OpCode, u8> {
        OpCode::try_from(self.op_byte())
    }

    /// Field `A`.
    pub fn a(self) -> u8 {
        (self.0 >> 8) as u8
    }

    /// Field `B`.
    pub fn b(self) -> u8 {
        (self.0 >> 16) as u8
    }

    /// Field `C`.
    pub fn c(self) -> u8 {
        (self.0 >> 24) as u8
    }

    /// Field `Bx`.
    pub fn bx(self) -> u16 {
        (self.0 >> 16) as u16
    }

    /// Field `sBx`.
    pub fn sbx(self) -> i16 {
        self.bx() as i16
    }

    /// Field `sJ` (sign-extended from 24 bits).
    pub fn sj_offset(self) -> i32 {
        // Casting to i32 first makes `>>` an *arithmetic* shift, which copies
        // the sign bit down: that is the sign extension from 24 to 32 bits.
        (self.0 as i32) >> 8
    }
}

// `{:?}` formatting of an instruction, e.g. `ADD 0 0 1`; used by `--trace-exec`.
impl fmt::Debug for Instr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.op() {
            Ok(op) => match op.format() {
                Format::A => write!(f, "{op} {}", self.a()),
                Format::AB => write!(f, "{op} {} {}", self.a(), self.b()),
                Format::ABC => write!(f, "{op} {} {} {}", self.a(), self.b(), self.c()),
                Format::ABx => write!(f, "{op} {} {}", self.a(), self.bx()),
                Format::AsBx => write!(f, "{op} {} {}", self.a(), self.sbx()),
                Format::SJ => write!(f, "{op} {}", self.sj_offset()),
            },
            Err(b) => write!(f, "<bad opcode {b}> {:#010x}", self.0),
        }
    }
}

// RUST: `#[cfg(test)]` compiles this module only for `cargo test`.
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn opcode_numbering_is_dense() {
        for (i, op) in OpCode::ALL.iter().enumerate() {
            assert_eq!(*op as usize, i, "{op} out of order");
            assert_eq!(OpCode::try_from(i as u8), Ok(*op));
        }
        assert!(OpCode::try_from(OpCode::ALL.len() as u8).is_err());
    }

    #[test]
    fn sbx_and_sj_extremes() {
        for v in [i16::MIN, -1, 0, 1, i16::MAX] {
            assert_eq!(Instr::asbx(OpCode::LOADI, 7, v).sbx(), v);
        }
        for v in [MIN_SJ, -1, 0, 1, MAX_SJ] {
            let i = Instr::sj(OpCode::JMP, v);
            assert_eq!(i.sj_offset(), v);
            assert_eq!(i.op(), Ok(OpCode::JMP));
        }
    }

    // Property tests: proptest generates hundreds of random inputs and checks
    // that encode-then-decode gives back exactly what went in.
    proptest! {
        #[test]
        fn abc_roundtrip(op in 0..OpCode::ALL.len(), a: u8, b: u8, c: u8) {
            let op = OpCode::ALL[op];
            let i = Instr::abc(op, a, b, c);
            prop_assert_eq!((i.op(), i.a(), i.b(), i.c()), (Ok(op), a, b, c));
        }

        #[test]
        fn abx_roundtrip(a: u8, bx: u16) {
            let i = Instr::abx(OpCode::LOADK, a, bx);
            prop_assert_eq!((i.op(), i.a(), i.bx()), (Ok(OpCode::LOADK), a, bx));
        }

        #[test]
        fn asbx_roundtrip(a: u8, sbx: i16) {
            let i = Instr::asbx(OpCode::LOADI, a, sbx);
            prop_assert_eq!((i.op(), i.a(), i.sbx()), (Ok(OpCode::LOADI), a, sbx));
        }

        #[test]
        fn sj_roundtrip(sj in MIN_SJ..=MAX_SJ) {
            let i = Instr::sj(OpCode::JMP, sj);
            prop_assert_eq!((i.op(), i.sj_offset()), (Ok(OpCode::JMP), sj));
        }
    }
}
