//! The bytecode disassembler.
//!
//! [`disassemble`] renders a function prototype (and, recursively, every nested
//! function constant) into human-readable assembly: byte offset, source line,
//! mnemonic, and decoded operands (constant values for constant-referencing
//! ops, absolute targets for jumps, capture descriptors for `CLOSURE`). It is
//! the primary debugging tool for the compiler and VM and underpins the
//! disassembly tests and the `lumen disasm` / debugger commands.

use crate::chunk::{Chunk, Constant, FnProto};
use crate::opcode::OpCode;
use crate::util::format_float;

/// Disassemble a whole prototype tree into a string.
pub fn disassemble(proto: &FnProto) -> String {
    let mut out = String::new();
    disassemble_proto(proto, &mut out);
    out
}

fn disassemble_proto(proto: &FnProto, out: &mut String) {
    out.push_str(&format!(
        "== {} (arity {}, {} upvalues) ==\n",
        proto.display_name(),
        proto.arity,
        proto.upvalue_count
    ));
    let chunk = &proto.chunk;
    let mut offset = 0;
    while offset < chunk.code.len() {
        offset = disassemble_instruction(chunk, offset, out);
    }
    // Recurse into nested function constants.
    for c in &chunk.constants {
        if let Constant::Fn(f) = c {
            out.push('\n');
            disassemble_proto(f, out);
        }
    }
}

/// Disassemble a single instruction at `offset`, appending a line to `out`, and
/// return the offset of the next instruction.
pub fn disassemble_instruction(chunk: &Chunk, offset: usize, out: &mut String) -> usize {
    // Offset and line gutter (`|` when the line is unchanged from the previous).
    let line = chunk.line_at(offset);
    let line_str = if offset > 0 && chunk.line_at(offset - 1) == line {
        "   |".to_string()
    } else {
        format!("{line:>4}")
    };
    let prefix = format!("{offset:04} {line_str} ");

    let byte = chunk.code[offset];
    let op = match OpCode::from_u8(byte) {
        Some(op) => op,
        None => {
            out.push_str(&format!("{prefix}<bad opcode {byte}>\n"));
            return offset + 1;
        }
    };

    let name = op.name();
    match op {
        // Constant-referencing (u16) ops: show the constant's value.
        OpCode::Const
        | OpCode::DefineGlobal
        | OpCode::GetGlobal
        | OpCode::SetGlobal
        | OpCode::GetProp
        | OpCode::SetProp
        | OpCode::Class
        | OpCode::Method
        | OpCode::GetSuper
        | OpCode::Import
        | OpCode::MatchMapHas => {
            let idx = chunk.read_u16(offset + 1);
            out.push_str(&format!(
                "{prefix}{name:<14} {idx:>4} {}\n",
                const_repr(chunk, idx)
            ));
            offset + 3
        }
        // Plain u8 slot/count operands.
        OpCode::PopN
        | OpCode::GetLocal
        | OpCode::SetLocal
        | OpCode::GetUpvalue
        | OpCode::SetUpvalue
        | OpCode::Call
        | OpCode::Interpolate => {
            let n = chunk.code[offset + 1];
            out.push_str(&format!("{prefix}{name:<14} {n:>4}\n"));
            offset + 2
        }
        // Forward jumps: show the absolute target.
        OpCode::Jump | OpCode::JumpIfFalse | OpCode::PushHandler => {
            let dist = chunk.read_u16(offset + 1) as usize;
            let target = offset + 3 + dist;
            out.push_str(&format!("{prefix}{name:<14} {:>4} -> {target:04}\n", dist));
            offset + 3
        }
        OpCode::Loop => {
            let dist = chunk.read_u16(offset + 1) as usize;
            let target = offset + 3 - dist;
            out.push_str(&format!("{prefix}{name:<14} {:>4} -> {target:04}\n", dist));
            offset + 3
        }
        // CLOSURE: u16 proto index, then upvalue_count (is_local, index) pairs.
        OpCode::Closure => {
            let idx = chunk.read_u16(offset + 1);
            out.push_str(&format!(
                "{prefix}{name:<14} {idx:>4} {}\n",
                const_repr(chunk, idx)
            ));
            let mut o = offset + 3;
            if let Some(Constant::Fn(proto)) = chunk.constants.get(idx as usize) {
                for _ in 0..proto.upvalue_count {
                    let is_local = chunk.code[o];
                    let index = chunk.code[o + 1];
                    let kind = if is_local == 1 { "local" } else { "upvalue" };
                    out.push_str(&format!(
                        "{:04}    |     |                 {kind} {index}\n",
                        o
                    ));
                    o += 2;
                }
            }
            o
        }
        // for-in: iter slot, index slot, exit jump.
        OpCode::IterNext => {
            let iter = chunk.code[offset + 1];
            let idx = chunk.code[offset + 2];
            let dist = chunk.read_u16(offset + 3) as usize;
            let target = offset + 5 + dist;
            out.push_str(&format!(
                "{prefix}{name:<14} iter={iter} idx={idx} -> {target:04}\n"
            ));
            offset + 5
        }
        // DEFAULT_ARG: u8 param index + u16 forward skip.
        OpCode::DefaultArg => {
            let param = chunk.code[offset + 1];
            let dist = chunk.read_u16(offset + 2) as usize;
            let target = offset + 4 + dist;
            out.push_str(&format!("{prefix}{name:<14} param={param} -> {target:04}\n"));
            offset + 4
        }
        // INVOKE / SUPER_INVOKE: u16 method-name constant + u8 argc.
        OpCode::Invoke | OpCode::SuperInvoke => {
            let idx = chunk.read_u16(offset + 1);
            let argc = chunk.code[offset + 3];
            out.push_str(&format!(
                "{prefix}{name:<14} {idx:>4} {} ({argc} args)\n",
                const_repr(chunk, idx)
            ));
            offset + 4
        }
        // Two u8 operands.
        OpCode::MatchArray | OpCode::ArrayRest => {
            let a = chunk.code[offset + 1];
            let b = chunk.code[offset + 2];
            out.push_str(&format!("{prefix}{name:<14} {a:>4} {b}\n"));
            offset + 3
        }
        // No operands.
        _ => {
            out.push_str(&format!("{prefix}{name}\n"));
            offset + 1
        }
    }
}

fn const_repr(chunk: &Chunk, idx: u16) -> String {
    match chunk.constants.get(idx as usize) {
        Some(Constant::Int(n)) => format!("; {n}"),
        Some(Constant::Float(f)) => format!("; {}", format_float(*f)),
        Some(Constant::Str(s)) => format!("; \"{s}\""),
        Some(Constant::Fn(f)) => format!("; <{}>", f.display_name()),
        None => "; <bad const>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::compile;

    fn disasm_src(src: &str) -> String {
        let (program, errs) = crate::parse_source(src);
        assert!(errs.is_empty(), "front-end errors: {errs:?}");
        let proto = compile(&program).expect("compile ok");
        disassemble(&proto)
    }

    #[test]
    fn disassembles_arithmetic() {
        let d = disasm_src("let x = 1 + 2 * 3;");
        assert!(d.contains("CONST"));
        assert!(d.contains("ADD"));
        assert!(d.contains("MUL"));
        assert!(d.contains("DEF_GLOBAL"));
    }

    #[test]
    fn disassembles_function_and_recurses() {
        let d = disasm_src("fn add(a, b) { return a + b; } add(1, 2);");
        assert!(d.contains("== fn add"));
        assert!(d.contains("CLOSURE"));
        assert!(d.contains("CALL"));
        assert!(d.contains("RETURN"));
    }

    #[test]
    fn jump_targets_are_absolute() {
        let d = disasm_src("if true { print(1); }");
        assert!(d.contains("JUMP_IF_FALSE"));
        assert!(d.contains("->"));
    }

    #[test]
    fn closure_shows_upvalues() {
        let d = disasm_src("fn outer() { let n = 1; return fn() { return n; }; }");
        assert!(d.contains("CLOSURE"));
        assert!(d.contains("local")); // captures `n` as a local upvalue
    }
}
