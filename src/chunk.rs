//! Bytecode chunks, the constant pool, and compiled function prototypes.
//!
//! A [`Chunk`] is the output of the compiler for one function body: the byte
//! stream, a parallel line table (for stack traces and the disassembler), and a
//! pool of [`Constant`]s. Constants are deliberately *not* heap/GC values — they
//! are plain immutable data (`Int`/`Float`/`Str`) plus `Rc`-shared function
//! prototypes ([`FnProto`]). This keeps the whole front end (lex→…→compile)
//! independent of the runtime heap, which only appears in Phase 5. The VM
//! materializes a constant into a runtime value when it executes the instruction
//! that loads it (e.g. interning a `Str` into a heap string).

use crate::opcode::OpCode;
use std::rc::Rc;

/// What kind of callable a prototype describes (affects `this`/return rules and
/// disassembly headers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnKind {
    Script,
    Function,
    Method,
    Initializer,
}

/// A compiled function: its code, metadata, and how many upvalues its closures
/// capture. Shared via `Rc` because the same prototype backs every closure made
/// from it, and prototypes are immutable "code", not GC-managed "data".
#[derive(Debug, PartialEq)]
pub struct FnProto {
    pub name: Option<String>,
    /// Total number of declared parameters (the rest parameter, if any, counts
    /// as one). This is the maximum fixed arity.
    pub arity: usize,
    /// Number of required parameters (those before the first default or rest).
    pub required_arity: usize,
    /// Whether the last parameter is a rest parameter (`..name`).
    pub has_rest: bool,
    pub upvalue_count: usize,
    /// Whether this function's body contains `yield` — a generator function whose
    /// call returns a `Generator` rather than running its body (DESIGN D29).
    pub is_generator: bool,
    pub chunk: Chunk,
    pub kind: FnKind,
    /// For a module/script prototype: the names declared with `export`, which
    /// become the module's public surface. Empty for ordinary functions.
    pub exports: Vec<String>,
    /// Debug info: the name of the local declared at each slot (last wins when a
    /// slot is reused). Empty string for the reserved/compiler-internal slots.
    /// Used by the debugger to show locals by name.
    pub local_names: Vec<String>,
}

impl FnProto {
    /// A human label for traces/disassembly, e.g. `fn fib` or `<script>`.
    pub fn display_name(&self) -> String {
        match self.kind {
            FnKind::Script => "<script>".to_string(),
            FnKind::Method | FnKind::Initializer => {
                format!("method {}", self.name.as_deref().unwrap_or("?"))
            }
            FnKind::Function => format!("fn {}", self.name.as_deref().unwrap_or("<anonymous>")),
        }
    }
}

/// A value baked into a chunk at compile time.
#[derive(Clone, Debug)]
pub enum Constant {
    Int(i64),
    Float(f64),
    Str(String),
    Fn(Rc<FnProto>),
}

impl Constant {
    /// Equality used for constant-pool deduplication. Floats compare by bit
    /// pattern (so `NaN`s and `-0.0`/`0.0` are distinguished, which is what we
    /// want for dedup); functions are never considered equal (each is unique).
    fn dedup_eq(&self, other: &Constant) -> bool {
        match (self, other) {
            (Constant::Int(a), Constant::Int(b)) => a == b,
            (Constant::Float(a), Constant::Float(b)) => a.to_bits() == b.to_bits(),
            (Constant::Str(a), Constant::Str(b)) => a == b,
            _ => false,
        }
    }
}

/// A compiled byte stream plus its constants and per-byte source lines.
#[derive(Debug, PartialEq, Default)]
pub struct Chunk {
    pub code: Vec<u8>,
    pub constants: Vec<Constant>,
    /// `lines[i]` is the 1-based source line of the byte at `code[i]`.
    pub lines: Vec<u32>,
}

// Hand-written because `Constant` (with f64/Rc) has no derivable `PartialEq` we
// want; chunk equality is only used in tests.
impl PartialEq for Constant {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Constant::Int(a), Constant::Int(b)) => a == b,
            (Constant::Float(a), Constant::Float(b)) => a.to_bits() == b.to_bits(),
            (Constant::Str(a), Constant::Str(b)) => a == b,
            (Constant::Fn(a), Constant::Fn(b)) => Rc::ptr_eq(a, b) || a == b,
            _ => false,
        }
    }
}

impl Chunk {
    pub fn new() -> Self {
        Chunk::default()
    }

    /// Append one opcode byte tagged with its source line.
    pub fn write_op(&mut self, op: OpCode, line: u32) {
        self.code.push(op as u8);
        self.lines.push(line);
    }

    /// Append a raw operand byte.
    pub fn write_u8(&mut self, byte: u8, line: u32) {
        self.code.push(byte);
        self.lines.push(line);
    }

    /// Append a big-endian `u16` operand.
    pub fn write_u16(&mut self, value: u16, line: u32) {
        self.code.push((value >> 8) as u8);
        self.code.push((value & 0xff) as u8);
        self.lines.push(line);
        self.lines.push(line);
    }

    /// Read a big-endian `u16` at byte offset `at`.
    pub fn read_u16(&self, at: usize) -> u16 {
        ((self.code[at] as u16) << 8) | (self.code[at + 1] as u16)
    }

    /// Patch a previously-written `u16` (used for jump backpatching).
    pub fn patch_u16(&mut self, at: usize, value: u16) {
        self.code[at] = (value >> 8) as u8;
        self.code[at + 1] = (value & 0xff) as u8;
    }

    /// Add a constant, deduplicating simple values, and return its index.
    pub fn add_constant(&mut self, c: Constant) -> usize {
        if let Some(i) = self.constants.iter().position(|x| x.dedup_eq(&c)) {
            return i;
        }
        self.constants.push(c);
        self.constants.len() - 1
    }

    pub fn len(&self) -> usize {
        self.code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.code.is_empty()
    }

    /// The source line of the byte at `ip` (saturating for safety).
    pub fn line_at(&self, ip: usize) -> u32 {
        self.lines.get(ip).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u16_roundtrip_and_patch() {
        let mut c = Chunk::new();
        c.write_op(OpCode::Jump, 1);
        c.write_u16(0xBEEF, 1);
        assert_eq!(c.read_u16(1), 0xBEEF);
        c.patch_u16(1, 0x1234);
        assert_eq!(c.read_u16(1), 0x1234);
    }

    #[test]
    fn constant_dedup() {
        let mut c = Chunk::new();
        let a = c.add_constant(Constant::Int(5));
        let b = c.add_constant(Constant::Int(5));
        let d = c.add_constant(Constant::Str("hi".into()));
        let e = c.add_constant(Constant::Str("hi".into()));
        assert_eq!(a, b);
        assert_eq!(d, e);
        assert_eq!(c.constants.len(), 2);
    }

    #[test]
    fn distinct_floats_not_merged_with_ints() {
        let mut c = Chunk::new();
        c.add_constant(Constant::Int(1));
        c.add_constant(Constant::Float(1.0));
        assert_eq!(c.constants.len(), 2);
    }
}
