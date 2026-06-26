//! The Lumen virtual machine instruction set.
//!
//! Lumen compiles to a flat byte stream for a **stack machine**: each opcode is
//! one byte, optionally followed by operand bytes. Operand widths are fixed per
//! opcode (documented in `OPCODES.md` and in the doc comment on each variant):
//! `u8` operands are one byte; `u16` operands are two bytes, big-endian. The only
//! variable-length instruction is [`OpCode::Closure`], whose upvalue descriptors
//! trail the operand (their count comes from the referenced function prototype).
//!
//! Keeping the numeric encoding in one place (the `#[repr(u8)]` enum plus
//! [`OpCode::from_u8`]) means the compiler, disassembler, and VM can never
//! disagree about what byte means what.

/// A single VM instruction. The `#[repr(u8)]` discriminant *is* the byte written
/// to the code stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    // ---- literals & stack ----
    /// Push `nil`.
    Nil = 0,
    /// Push `true`.
    True,
    /// Push `false`.
    False,
    /// `Const u16` — push `constants[idx]`.
    Const,
    /// Pop and discard the top value.
    Pop,
    /// `PopN u8` — pop and discard the top `n` values (scope cleanup).
    PopN,

    // ---- variables ----
    /// `DefineGlobal u16` — pop a value and bind it to the global named by
    /// `constants[idx]`.
    DefineGlobal,
    /// `GetGlobal u16` — push the value of the named global (throws `NameError`
    /// if undefined).
    GetGlobal,
    /// `SetGlobal u16` — assign the named global to the top value (left in
    /// place); throws `NameError` if the global was never defined.
    SetGlobal,
    /// `GetLocal u8` — push the value in the current frame's slot.
    GetLocal,
    /// `SetLocal u8` — store the top value (left in place) into the slot.
    SetLocal,
    /// `GetUpvalue u8` — push the value of the current closure's upvalue.
    GetUpvalue,
    /// `SetUpvalue u8` — store the top value (left in place) into the upvalue.
    SetUpvalue,
    /// Close the upvalue that captured the top-of-stack local (hoist it to the
    /// heap) and pop it.
    CloseUpvalue,

    // ---- arithmetic / logic (operate on the stack top) ----
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// Unary numeric negation.
    Neg,
    /// Logical negation (result is always a bool).
    Not,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    // ---- control flow ----
    /// `Jump u16` — add the operand to `ip` (forward).
    Jump,
    /// `JumpIfFalse u16` — if the top value (left in place) is falsy, jump
    /// forward by the operand.
    JumpIfFalse,
    /// `Loop u16` — subtract the operand from `ip` (backward jump).
    Loop,

    // ---- functions ----
    /// `Call u8` — call the value `argc` slots below the top, with `argc` args.
    Call,
    /// `Closure u16 (u8 u8)*` — build a closure from the function prototype
    /// `constants[idx]`; the trailing `upvalue_count` pairs are
    /// `(is_local, index)` capture descriptors.
    Closure,
    /// Return the top value from the current function.
    Return,

    // ---- collections ----
    /// Push a new empty array.
    NewArray,
    /// Pop a value and append it to the array now on top (array stays).
    ArrayPush,
    /// Pop an iterable and append all its elements to the array now on top.
    ArrayExtend,
    /// Push a new empty map.
    NewMap,
    /// Pop a value then a key, inserting `key -> value` into the map now on top.
    MapInsert,
    /// Pop an index then an object; push `object[index]`.
    IndexGet,
    /// Pop a value, an index, and an object; set `object[index] = value` and
    /// push the value.
    IndexSet,

    // ---- objects & classes ----
    /// `GetProp u16` — pop an object; push its `constants[idx]` property (field
    /// or bound method).
    GetProp,
    /// `SetProp u16` — pop a value then an object; set the named field and push
    /// the value.
    SetProp,
    /// `Class u16` — push a new class named `constants[idx]`.
    Class,
    /// Pop a superclass and set it on the class now on top (which stays).
    Inherit,
    /// `Method u16` — pop a closure and add it as the named method to the class
    /// now on top.
    Method,
    /// `GetSuper u16` — pop a receiver; push the named method resolved from the
    /// current method's home class's superclass, bound to the receiver.
    GetSuper,

    // ---- exceptions ----
    /// `PushHandler u16` — register a `catch` handler at `ip + operand`.
    PushHandler,
    /// Pop the most recently registered handler (try block finished normally).
    PopHandler,
    /// Pop a value and throw it.
    Throw,

    // ---- misc ----
    /// `Interpolate u8` — pop `n` values, render each to a string, concatenate
    /// them in order, and push the result. Implements `${...}` strings.
    Interpolate,
    /// Push a copy of the top value.
    Dup,
    /// `Import u16` — load (compile + run once, cached) the module named by the
    /// string constant, and push the resulting module value.
    Import,
    /// `IterNext u8 u8 u16` — for-in step. Operands: the iterable slot, the index
    /// slot, and a forward jump. If the index is past the end, jump by the
    /// offset; otherwise push the current element and increment the index slot.
    IterNext,

    // ---- pattern matching ----
    /// `MatchArray u8 u8` — pop a value; push `true` iff it is an array whose
    /// length is exactly (flag=1) or at least (flag=0) the first operand.
    MatchArray,
    /// `MatchMapHas u16` — pop a value; push `true` iff it is a map containing
    /// the key named by the string constant.
    MatchMapHas,
    /// `ArrayRest u8 u8` — pop an array; push the sub-array from index `front`
    /// up to `length - back` (used to bind an array pattern's `..rest`).
    ArrayRest,

    /// `CloseUpvalueSlot u8` — close any open upvalue capturing the frame slot,
    /// **without** popping the stack. Emitted at the bottom of each C-style
    /// `for` iteration so closures capture that iteration's loop variable
    /// (per-iteration binding, like JS `let`).
    CloseUpvalueSlot,

    /// `Invoke u16 u8` — call method `constants[name]` on the receiver `argc`
    /// slots below the top, with `argc` args. A fused `GET_PROP` + `CALL` that
    /// skips allocating a bound method for the common instance-method case.
    Invoke,

    /// `DefaultArg u8 u16` — at function entry, if parameter `index` (0-based)
    /// was supplied by the caller, jump forward by the offset (skipping the
    /// default-value expression); otherwise fall through to evaluate it.
    DefaultArg,
}

impl OpCode {
    /// Decode a byte to an opcode, or `None` if it is not a valid opcode.
    pub fn from_u8(b: u8) -> Option<OpCode> {
        // Safe because the range check guarantees `b` is a valid discriminant of
        // this contiguous `#[repr(u8)]` enum.
        if b <= OpCode::DefaultArg as u8 {
            Some(unsafe { std::mem::transmute::<u8, OpCode>(b) })
        } else {
            None
        }
    }

    /// Mnemonic used by the disassembler.
    pub fn name(self) -> &'static str {
        match self {
            OpCode::Nil => "NIL",
            OpCode::True => "TRUE",
            OpCode::False => "FALSE",
            OpCode::Const => "CONST",
            OpCode::Pop => "POP",
            OpCode::PopN => "POP_N",
            OpCode::DefineGlobal => "DEF_GLOBAL",
            OpCode::GetGlobal => "GET_GLOBAL",
            OpCode::SetGlobal => "SET_GLOBAL",
            OpCode::GetLocal => "GET_LOCAL",
            OpCode::SetLocal => "SET_LOCAL",
            OpCode::GetUpvalue => "GET_UPVALUE",
            OpCode::SetUpvalue => "SET_UPVALUE",
            OpCode::CloseUpvalue => "CLOSE_UPVALUE",
            OpCode::Add => "ADD",
            OpCode::Sub => "SUB",
            OpCode::Mul => "MUL",
            OpCode::Div => "DIV",
            OpCode::Rem => "REM",
            OpCode::Neg => "NEG",
            OpCode::Not => "NOT",
            OpCode::Eq => "EQ",
            OpCode::Ne => "NE",
            OpCode::Lt => "LT",
            OpCode::Le => "LE",
            OpCode::Gt => "GT",
            OpCode::Ge => "GE",
            OpCode::Jump => "JUMP",
            OpCode::JumpIfFalse => "JUMP_IF_FALSE",
            OpCode::Loop => "LOOP",
            OpCode::Call => "CALL",
            OpCode::Closure => "CLOSURE",
            OpCode::Return => "RETURN",
            OpCode::NewArray => "NEW_ARRAY",
            OpCode::ArrayPush => "ARRAY_PUSH",
            OpCode::ArrayExtend => "ARRAY_EXTEND",
            OpCode::NewMap => "NEW_MAP",
            OpCode::MapInsert => "MAP_INSERT",
            OpCode::IndexGet => "INDEX_GET",
            OpCode::IndexSet => "INDEX_SET",
            OpCode::GetProp => "GET_PROP",
            OpCode::SetProp => "SET_PROP",
            OpCode::Class => "CLASS",
            OpCode::Inherit => "INHERIT",
            OpCode::Method => "METHOD",
            OpCode::GetSuper => "GET_SUPER",
            OpCode::PushHandler => "PUSH_HANDLER",
            OpCode::PopHandler => "POP_HANDLER",
            OpCode::Throw => "THROW",
            OpCode::Interpolate => "INTERPOLATE",
            OpCode::Dup => "DUP",
            OpCode::Import => "IMPORT",
            OpCode::IterNext => "ITER_NEXT",
            OpCode::MatchArray => "MATCH_ARRAY",
            OpCode::MatchMapHas => "MATCH_MAP_HAS",
            OpCode::ArrayRest => "ARRAY_REST",
            OpCode::CloseUpvalueSlot => "CLOSE_UPVALUE_SLOT",
            OpCode::Invoke => "INVOKE",
            OpCode::DefaultArg => "DEFAULT_ARG",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_opcodes() {
        // Every discriminant from 0..=IterNext must decode back to itself.
        for b in 0..=(OpCode::DefaultArg as u8) {
            let op = OpCode::from_u8(b).expect("valid opcode");
            assert_eq!(op as u8, b);
        }
    }

    #[test]
    fn invalid_byte_is_none() {
        assert!(OpCode::from_u8(OpCode::DefaultArg as u8 + 1).is_none());
        assert!(OpCode::from_u8(255).is_none());
    }
}
