# valo-vm

Bytecode format and runtime for the valo language: instruction encoding
(Lua-style 32-bit iABC/iABx/iAsBx words), function prototypes, immutable
`Rc`-based values, the register-based interpreter, runtime errors with stack
traces, and the native function table.

This crate does not depend on the compiler. The `valo-vm` binary runs
precompiled `.vlc` files (not implemented yet).
