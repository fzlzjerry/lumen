//! The bytecode compiler: AST → [`FnProto`] (chunks of bytecode).
//!
//! This is a single-pass code generator in the lineage of *Crafting
//! Interpreters*' clox, but extended for Lumen's richer feature set (arrays,
//! maps, string interpolation, modules, exceptions with `finally`, and pattern
//! matching). It assumes the resolver (Phase 3) already proved the program
//! statically valid, so it is concerned purely with *layout and emission*:
//! allocating local slots, building upvalue capture tables, backpatching jumps,
//! and lowering each construct to instructions.
//!
//! A `Compiler` holds a stack of [`FnState`]s, one per function currently being
//! compiled (the innermost on top). Nested functions push a new state; finishing
//! one pops it, freezes its chunk into an `Rc<FnProto>`, adds it as a constant in
//! the enclosing chunk, and emits `CLOSURE` with the capture descriptors.

use crate::ast::*;
use crate::chunk::{Chunk, Constant, FnKind, FnProto};
use crate::diagnostics::Diagnostic;
use crate::opcode::OpCode;
use crate::span::Span;
use std::rc::Rc;

/// Compile a whole program into the top-level (script) function prototype.
pub fn compile(program: &Program) -> Result<Rc<FnProto>, Vec<Diagnostic>> {
    Compiler::new().compile_script(program, false).map(|(p, _)| p)
}

/// Compile for the REPL: if the final top-level statement is a bare expression,
/// its value is returned from the script (instead of discarded) so the REPL can
/// print it. The returned bool says whether that happened.
pub fn compile_repl(program: &Program) -> Result<(Rc<FnProto>, bool), Vec<Diagnostic>> {
    Compiler::new().compile_script(program, true)
}

struct CLocal {
    name: String,
    depth: usize,
    is_captured: bool,
}

#[derive(Clone, Copy)]
struct CUpvalue {
    is_local: bool,
    index: u8,
}

/// Where a value being pattern-matched lives, so we can re-load it (and its
/// sub-parts) without leaving extra junk on the stack.
enum Access {
    Local(u8),
    /// `Index(base, i)` — `base[i]`; negative `i` counts from the end.
    Index(Box<Access>, i64),
    /// `Key(base, k)` — `base["k"]`.
    Key(Box<Access>, String),
}

/// The body of a comprehension being compiled: an array element, or a map
/// `key`/`value` pair.
enum Comp<'a> {
    Array(&'a Expr),
    Map(&'a Expr, &'a Expr),
}

/// Control-flow frames threaded for `break`/`continue`/`return` so they emit the
/// right scope/handler/finally cleanup when jumping out of nested constructs.
enum Cf {
    Loop {
        continue_target: usize,
        break_jumps: Vec<usize>,
        local_base: usize,
    },
    Try {
        finally: Option<Block>,
        n_handlers: u8,
    },
}

struct FnState {
    chunk: Chunk,
    name: Option<String>,
    arity: usize,
    kind: FnKind,
    locals: Vec<CLocal>,
    upvalues: Vec<CUpvalue>,
    scope_depth: usize,
    cf: Vec<Cf>,
    exports: Vec<String>,
    /// Slot -> declared local name (debug info), highest-watermark.
    local_names: Vec<String>,
}

impl FnState {
    fn new(kind: FnKind, name: Option<String>, slot0_name: &str) -> Self {
        FnState {
            chunk: Chunk::new(),
            name,
            arity: 0,
            kind,
            // Slot 0 is reserved: the closure itself for functions, the receiver
            // (`this`) for methods.
            locals: vec![CLocal { name: slot0_name.to_string(), depth: 0, is_captured: false }],
            upvalues: Vec::new(),
            scope_depth: 0,
            cf: Vec::new(),
            exports: Vec::new(),
            local_names: vec![slot0_name.to_string()],
        }
    }
}

struct Compiler {
    funcs: Vec<FnState>,
    errors: Vec<Diagnostic>,
    /// Set just before compiling a statement's *value* expression (where the
    /// operand stack is clean). A `match` reads and clears it: at a clean position
    /// it compiles in place; otherwise it wraps itself in an IIFE so its temp slots
    /// are correct regardless of the surrounding operand stack (DESIGN D34).
    stmt_value_pos: bool,
}

impl Compiler {
    fn new() -> Self {
        Compiler { funcs: Vec::new(), errors: Vec::new(), stmt_value_pos: false }
    }

    /// Compile an expression in statement-value position (a clean operand stack).
    fn compile_value_expr(&mut self, e: &Expr) {
        self.stmt_value_pos = true;
        self.compile_expr(e);
    }

    fn compile_script(
        mut self,
        program: &Program,
        repl: bool,
    ) -> Result<(Rc<FnProto>, bool), Vec<Diagnostic>> {
        self.funcs.push(FnState::new(FnKind::Script, None, ""));
        // In REPL mode, a trailing bare expression's value is returned (printed)
        // instead of discarded.
        let last = program.items.len().wrapping_sub(1);
        let mut returns_expr = false;
        for (i, item) in program.items.iter().enumerate() {
            if repl && i == last {
                if let Stmt::Expr { expr, .. } = item {
                    self.compile_value_expr(expr); // leave the value on the stack
                    let line = expr.span.line;
                    self.emit_op(OpCode::Return, line); // and return it
                    returns_expr = true;
                    break;
                }
            }
            self.compile_stmt(item);
        }
        if !returns_expr {
            let line = program.items.last().map(|s| s.span().line).unwrap_or(1);
            self.emit_op(OpCode::Nil, line);
            self.emit_op(OpCode::Return, line);
        }
        let state = self.funcs.pop().unwrap();
        if !self.errors.is_empty() {
            return Err(self.errors);
        }
        Ok((
            Rc::new(FnProto {
                name: None,
                arity: 0,
                required_arity: 0,
                has_rest: false,
                upvalue_count: 0,
                is_generator: false,
                chunk: state.chunk,
                kind: FnKind::Script,
                exports: state.exports,
                local_names: state.local_names,
            }),
            returns_expr,
        ))
    }

    // ---- low-level emission ------------------------------------------------

    fn cur(&mut self) -> &mut FnState {
        self.funcs.last_mut().unwrap()
    }
    fn cur_ref(&self) -> &FnState {
        self.funcs.last().unwrap()
    }
    fn chunk(&mut self) -> &mut Chunk {
        &mut self.funcs.last_mut().unwrap().chunk
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.errors.push(Diagnostic::error("compiler", msg, span));
    }

    fn emit_op(&mut self, op: OpCode, line: u32) {
        self.chunk().write_op(op, line);
    }
    fn emit_byte(&mut self, b: u8, line: u32) {
        self.chunk().write_u8(b, line);
    }
    fn emit_op_u16(&mut self, op: OpCode, operand: u16, line: u32) {
        self.chunk().write_op(op, line);
        self.chunk().write_u16(operand, line);
    }
    fn emit_op_u8(&mut self, op: OpCode, operand: u8, line: u32) {
        self.chunk().write_op(op, line);
        self.chunk().write_u8(operand, line);
    }

    fn make_const(&mut self, c: Constant, span: Span) -> u16 {
        let idx = self.chunk().add_constant(c);
        if idx > u16::MAX as usize {
            self.error(span, "too many constants in one function (max 65536)");
            return 0;
        }
        idx as u16
    }

    fn string_const(&mut self, s: &str, span: Span) -> u16 {
        self.make_const(Constant::Str(s.to_string()), span)
    }

    fn emit_load_const(&mut self, c: Constant, span: Span) {
        let idx = self.make_const(c, span);
        self.emit_op_u16(OpCode::Const, idx, span.line);
    }

    /// Emit a jump with a placeholder operand; returns the operand's byte offset
    /// for later [`patch_jump`](Self::patch_jump).
    fn emit_jump(&mut self, op: OpCode, line: u32) -> usize {
        self.emit_op(op, line);
        let at = self.chunk().len();
        self.chunk().write_u16(0xFFFF, line);
        at
    }

    fn patch_jump(&mut self, at: usize, span: Span) {
        let target = self.chunk().len();
        let dist = target - (at + 2);
        if dist > u16::MAX as usize {
            self.error(span, "branch too large (over 65535 bytes)");
            return;
        }
        self.chunk().patch_u16(at, dist as u16);
    }

    fn emit_loop(&mut self, loop_start: usize, span: Span) {
        self.emit_op(OpCode::Loop, span.line);
        let dist = self.chunk().len() + 2 - loop_start;
        if dist > u16::MAX as usize {
            self.error(span, "loop body too large (over 65535 bytes)");
        }
        self.chunk().write_u16(dist as u16, span.line);
    }

    // ---- scopes & locals ---------------------------------------------------

    fn begin_scope(&mut self) {
        self.cur().scope_depth += 1;
    }

    fn end_scope(&mut self, line: u32) {
        self.cur().scope_depth -= 1;
        let depth = self.cur_ref().scope_depth;
        while let Some(last) = self.cur_ref().locals.last() {
            if last.depth <= depth {
                break;
            }
            let captured = last.is_captured;
            self.emit_op(if captured { OpCode::CloseUpvalue } else { OpCode::Pop }, line);
            self.cur().locals.pop();
        }
    }

    fn add_local(&mut self, name: &str, span: Span) {
        if self.cur_ref().locals.len() >= 256 {
            self.error(span, "too many local variables in one function (max 256)");
            return;
        }
        let slot = self.cur_ref().locals.len();
        let depth = self.cur_ref().scope_depth;
        self.cur().locals.push(CLocal { name: name.to_string(), depth, is_captured: false });
        let names = &mut self.cur().local_names;
        if slot >= names.len() {
            names.resize(slot + 1, String::new());
        }
        names[slot] = name.to_string();
    }

    fn is_global_scope(&self) -> bool {
        matches!(self.cur_ref().kind, FnKind::Script) && self.cur_ref().scope_depth == 0
    }

    /// After a value has been left on the stack, bind it to `name` — as a global
    /// (top level) or a fresh local.
    fn define_variable(&mut self, name: &str, span: Span) {
        if self.is_global_scope() {
            let idx = self.string_const(name, span);
            self.emit_op_u16(OpCode::DefineGlobal, idx, span.line);
        } else {
            self.add_local(name, span);
        }
    }

    fn resolve_local(&mut self, fn_idx: usize, name: &str, span: Span) -> Option<u8> {
        let locals = &self.funcs[fn_idx].locals;
        for i in (0..locals.len()).rev() {
            if locals[i].name == name {
                return Some(i as u8);
            }
        }
        let _ = span;
        None
    }

    fn resolve_upvalue(&mut self, fn_idx: usize, name: &str, span: Span) -> Option<u8> {
        if fn_idx == 0 {
            return None;
        }
        if let Some(local) = self.resolve_local(fn_idx - 1, name, span) {
            self.funcs[fn_idx - 1].locals[local as usize].is_captured = true;
            return self.add_upvalue(fn_idx, true, local, span);
        }
        if let Some(up) = self.resolve_upvalue(fn_idx - 1, name, span) {
            return self.add_upvalue(fn_idx, false, up, span);
        }
        None
    }

    fn add_upvalue(&mut self, fn_idx: usize, is_local: bool, index: u8, span: Span) -> Option<u8> {
        if let Some(i) = self.funcs[fn_idx]
            .upvalues
            .iter()
            .position(|u| u.is_local == is_local && u.index == index)
        {
            return Some(i as u8);
        }
        if self.funcs[fn_idx].upvalues.len() >= 256 {
            self.error(span, "too many captured variables (upvalues) in one function (max 256)");
            return Some(0);
        }
        self.funcs[fn_idx].upvalues.push(CUpvalue { is_local, index });
        Some((self.funcs[fn_idx].upvalues.len() - 1) as u8)
    }

    fn named_variable_get(&mut self, name: &str, span: Span) {
        let cur = self.funcs.len() - 1;
        if let Some(slot) = self.resolve_local(cur, name, span) {
            self.emit_op_u8(OpCode::GetLocal, slot, span.line);
        } else if let Some(up) = self.resolve_upvalue(cur, name, span) {
            self.emit_op_u8(OpCode::GetUpvalue, up, span.line);
        } else {
            let idx = self.string_const(name, span);
            self.emit_op_u16(OpCode::GetGlobal, idx, span.line);
        }
    }

    fn named_variable_set(&mut self, name: &str, span: Span) {
        let cur = self.funcs.len() - 1;
        if let Some(slot) = self.resolve_local(cur, name, span) {
            self.emit_op_u8(OpCode::SetLocal, slot, span.line);
        } else if let Some(up) = self.resolve_upvalue(cur, name, span) {
            self.emit_op_u8(OpCode::SetUpvalue, up, span.line);
        } else {
            let idx = self.string_const(name, span);
            self.emit_op_u16(OpCode::SetGlobal, idx, span.line);
        }
    }

    // ---- statements --------------------------------------------------------

    fn compile_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, init, span, .. } => {
                match init {
                    Some(e) => self.compile_value_expr(e),
                    None => self.emit_op(OpCode::Nil, span.line),
                }
                self.define_variable(name, *span);
            }
            Stmt::Const { name, init, span, .. } => {
                self.compile_value_expr(init);
                self.define_variable(name, *span);
            }
            Stmt::Destructure { pattern, init, span } => {
                self.compile_destructure(pattern, init, *span);
            }
            Stmt::DestructureAssign { pattern, value, span } => {
                self.compile_destructure_assign(pattern, value, *span);
            }
            Stmt::Function(f) => {
                let line = f.span.line;
                let name = f.name.clone().unwrap_or_default();
                if self.is_global_scope() {
                    self.compile_function(f, FnKind::Function, line);
                    let idx = self.string_const(&name, f.name_span);
                    self.emit_op_u16(OpCode::DefineGlobal, idx, line);
                } else {
                    // Declare before the body so the function can recurse.
                    self.add_local(&name, f.name_span);
                    self.compile_function(f, FnKind::Function, line);
                }
            }
            Stmt::Class(c) => self.compile_class(c),
            Stmt::Import(im) => self.compile_import(im),
            Stmt::Export { decl, .. } => {
                self.record_exports(decl);
                self.compile_stmt(decl);
            }
            Stmt::Expr { expr, span } => {
                self.compile_value_expr(expr);
                self.emit_op(OpCode::Pop, span.line);
            }
            Stmt::Block(b) => {
                self.begin_scope();
                for s in &b.stmts {
                    self.compile_stmt(s);
                }
                self.end_scope(b.span.line);
            }
            Stmt::If { cond, then_block, else_branch, span } => {
                self.compile_if(cond, then_block, else_branch.as_deref(), *span);
            }
            Stmt::While { cond, body, span } => self.compile_while(cond, body, *span),
            Stmt::ForIn { var, iter, body, span, .. } => {
                self.compile_for_in(var, iter, body, *span)
            }
            Stmt::ForC { init, cond, step, body, span } => {
                self.compile_for_c(init.as_deref(), cond.as_ref(), step.as_ref(), body, *span)
            }
            Stmt::Return { value, span } => self.compile_return(value.as_ref(), *span),
            Stmt::Break { span } => self.compile_break(true, *span),
            Stmt::Continue { span } => self.compile_break(false, *span),
            Stmt::Throw { value, span } => {
                self.compile_value_expr(value);
                self.emit_op(OpCode::Throw, span.line);
            }
            Stmt::Yield { value, span } => {
                self.compile_value_expr(value);
                self.emit_op(OpCode::Yield, span.line);
            }
            Stmt::Try { body, catches, finally, span } => {
                self.compile_try(body, catches, finally.as_ref(), *span)
            }
        }
    }

    fn record_exports(&mut self, decl: &Stmt) {
        let name = match decl {
            Stmt::Let { name, .. } | Stmt::Const { name, .. } => Some(name.clone()),
            Stmt::Function(f) => f.name.clone(),
            Stmt::Class(c) => Some(c.name.clone()),
            _ => None,
        };
        if let Some(n) = name {
            self.cur().exports.push(n);
        }
    }

    fn compile_if(
        &mut self,
        cond: &Expr,
        then_block: &Block,
        else_branch: Option<&Stmt>,
        span: Span,
    ) {
        self.compile_expr(cond);
        let else_jump = self.emit_jump(OpCode::JumpIfFalse, span.line);
        self.emit_op(OpCode::Pop, span.line); // pop condition (true branch)
        self.compile_block(then_block);
        let end_jump = self.emit_jump(OpCode::Jump, span.line);
        self.patch_jump(else_jump, span);
        self.emit_op(OpCode::Pop, span.line); // pop condition (false branch)
        if let Some(else_) = else_branch {
            self.compile_stmt(else_);
        }
        self.patch_jump(end_jump, span);
    }

    fn compile_block(&mut self, block: &Block) {
        self.begin_scope();
        for s in &block.stmts {
            self.compile_stmt(s);
        }
        self.end_scope(block.span.line);
    }

    fn compile_while(&mut self, cond: &Expr, body: &Block, span: Span) {
        let loop_start = self.chunk().len();
        self.compile_expr(cond);
        let exit_jump = self.emit_jump(OpCode::JumpIfFalse, span.line);
        self.emit_op(OpCode::Pop, span.line); // pop cond (enter body)
        let local_base = self.cur_ref().locals.len();
        self.cur().cf.push(Cf::Loop { continue_target: loop_start, break_jumps: Vec::new(), local_base });
        self.compile_block(body);
        let cf = self.cur().cf.pop().unwrap();
        self.emit_loop(loop_start, span);
        self.patch_jump(exit_jump, span);
        self.emit_op(OpCode::Pop, span.line); // pop cond (exit)
        self.patch_breaks(cf, span);
    }

    fn compile_for_c(
        &mut self,
        init: Option<&Stmt>,
        cond: Option<&Expr>,
        step: Option<&Expr>,
        body: &Block,
        span: Span,
    ) {
        let line = span.line;
        self.begin_scope();
        if let Some(i) = init {
            self.compile_stmt(i);
        }
        // If the init declared a loop variable, remember its slot so each
        // iteration can close its captures (per-iteration binding, DESIGN D11).
        let loop_var_slot = match init {
            Some(Stmt::Let { .. }) => Some((self.cur_ref().locals.len() - 1) as u8),
            _ => None,
        };
        let local_base = self.cur_ref().locals.len();

        let loop_start = self.chunk().len();
        let mut exit_jump = None;
        if let Some(c) = cond {
            self.compile_expr(c);
            exit_jump = Some(self.emit_jump(OpCode::JumpIfFalse, line));
            self.emit_op(OpCode::Pop, line);
        }
        // Skip the post-body block (close + step) on the first pass.
        let body_jump = self.emit_jump(OpCode::Jump, line);
        // `post_body` is both the normal end-of-iteration path and the `continue`
        // target: close per-iteration captures, run the step, loop to the cond.
        let post_body = self.chunk().len();
        if let Some(slot) = loop_var_slot {
            self.emit_op_u8(OpCode::CloseUpvalueSlot, slot, line);
        }
        if let Some(s) = step {
            self.compile_expr(s);
            self.emit_op(OpCode::Pop, line); // discard step value
        }
        self.emit_loop(loop_start, span);
        self.patch_jump(body_jump, span);

        self.cur().cf.push(Cf::Loop { continue_target: post_body, break_jumps: Vec::new(), local_base });
        self.compile_block(body);
        let cf = self.cur().cf.pop().unwrap();
        self.emit_loop(post_body, span); // after the body, close + step

        if let Some(ej) = exit_jump {
            self.patch_jump(ej, span);
            self.emit_op(OpCode::Pop, line);
        }
        self.patch_breaks(cf, span);
        self.end_scope(line);
    }

    fn compile_for_in(&mut self, var: &str, iter: &Expr, body: &Block, span: Span) {
        self.begin_scope();
        // Hidden locals: the iterable and the running index.
        self.compile_expr(iter);
        self.add_local("@iter", span);
        let iter_slot = (self.cur_ref().locals.len() - 1) as u8;
        self.emit_load_const(Constant::Int(0), span);
        self.add_local("@idx", span);
        let idx_slot = (self.cur_ref().locals.len() - 1) as u8;

        let loop_start = self.chunk().len();
        self.emit_op(OpCode::IterNext, span.line);
        self.emit_byte(iter_slot, span.line);
        self.emit_byte(idx_slot, span.line);
        let exit_at = self.chunk().len();
        self.chunk().write_u16(0xFFFF, span.line); // patched to the loop exit

        // The element pushed by IterNext becomes the loop variable.
        let local_base = self.cur_ref().locals.len();
        self.cur().cf.push(Cf::Loop { continue_target: loop_start, break_jumps: Vec::new(), local_base });
        self.begin_scope();
        self.add_local(var, span);
        for s in &body.stmts {
            self.compile_stmt(s);
        }
        self.end_scope(span.line); // pop loop var (closing it if captured)
        let cf = self.cur().cf.pop().unwrap();
        self.emit_loop(loop_start, span);
        self.patch_jump(exit_at, span); // IterNext's exit lands here
        self.patch_breaks(cf, span);
        self.end_scope(span.line); // pop @iter and @idx
    }

    /// Compile a comprehension (`[e for x in it if c]` / `{k: v for ...}`) as an
    /// immediately-invoked zero-visible-arg function whose body is the build loop
    /// (DESIGN D31). Running it in its own frame keeps the loop's slots clean
    /// regardless of the surrounding operand stack (so a comprehension works as a
    /// call argument, operand, etc.). The iterable is evaluated in the enclosing
    /// scope and passed as the function's single argument; everything else
    /// (`element`/`cond`/`key`/`value`) is compiled inside and captures outer
    /// variables as upvalues.
    fn compile_comprehension(
        &mut self,
        comp: Comp,
        var: &str,
        var_span: Span,
        iter: &Expr,
        cond: Option<&Expr>,
        span: Span,
    ) {
        let line = span.line;
        self.funcs.push(FnState::new(FnKind::Function, None, ""));
        self.cur().arity = 1;
        self.begin_scope();
        // Slot 1: the iterable (the function's one parameter).
        self.add_local("@it", span);
        let it_slot = (self.cur_ref().locals.len() - 1) as u8;
        // Accumulator and running index.
        self.emit_op(if matches!(comp, Comp::Array(_)) { OpCode::NewArray } else { OpCode::NewMap }, line);
        self.add_local("@acc", span);
        let acc_slot = (self.cur_ref().locals.len() - 1) as u8;
        self.emit_load_const(Constant::Int(0), span);
        self.add_local("@idx", span);
        let idx_slot = (self.cur_ref().locals.len() - 1) as u8;

        let loop_start = self.chunk().len();
        self.emit_op(OpCode::IterNext, line);
        self.emit_byte(it_slot, line);
        self.emit_byte(idx_slot, line);
        let exit_at = self.chunk().len();
        self.chunk().write_u16(0xFFFF, line);

        // The element pushed by IterNext becomes the loop variable.
        self.begin_scope();
        self.add_local(var, var_span);
        // Optional `if cond`: skip the push when false (popping the test bool on
        // both paths).
        let skip = cond.map(|c| {
            self.compile_expr(c);
            let j = self.emit_jump(OpCode::JumpIfFalse, line);
            self.emit_op(OpCode::Pop, line); // matched: drop the test bool
            j
        });
        match comp {
            Comp::Array(element) => {
                self.emit_op_u8(OpCode::GetLocal, acc_slot, line);
                self.compile_expr(element);
                self.emit_op(OpCode::ArrayPush, line);
                self.emit_op(OpCode::Pop, line); // drop the acc copy ARRAY_PUSH left
            }
            Comp::Map(key, value) => {
                self.emit_op_u8(OpCode::GetLocal, acc_slot, line);
                self.compile_expr(key);
                self.compile_expr(value);
                self.emit_op(OpCode::MapInsert, line);
                self.emit_op(OpCode::Pop, line); // drop the acc copy MAP_INSERT left
            }
        }
        if let Some(j) = skip {
            let done = self.emit_jump(OpCode::Jump, line);
            self.patch_jump(j, span); // cond-false lands here with the bool still on top
            self.emit_op(OpCode::Pop, line);
            self.patch_jump(done, span);
        }
        self.end_scope(line); // pop the loop variable (closing it if captured)
        self.emit_loop(loop_start, span);
        self.patch_jump(exit_at, span); // IterNext's exit lands here
        // Return the accumulator.
        self.emit_op_u8(OpCode::GetLocal, acc_slot, line);
        self.emit_op(OpCode::Return, line);

        // Freeze the function and emit it as a closure, then invoke it with the
        // iterable (evaluated in the enclosing scope) as the sole argument.
        let state = self.funcs.pop().unwrap();
        let upvalues = state.upvalues;
        let proto = Rc::new(FnProto {
            name: None,
            arity: 1,
            required_arity: 1,
            has_rest: false,
            upvalue_count: upvalues.len(),
            is_generator: false,
            chunk: state.chunk,
            kind: FnKind::Function,
            exports: Vec::new(),
            local_names: state.local_names,
        });
        let idx = self.make_const(Constant::Fn(proto), span);
        self.emit_op_u16(OpCode::Closure, idx, line);
        for up in &upvalues {
            self.emit_byte(u8::from(up.is_local), line);
            self.emit_byte(up.index, line);
        }
        self.compile_expr(iter); // the argument, in the enclosing scope
        self.emit_op_u8(OpCode::Call, 1, line);
    }

    /// Patch a finished loop's `break` jumps to the current position.
    fn patch_breaks(&mut self, cf: Cf, span: Span) {
        if let Cf::Loop { break_jumps, .. } = cf {
            for j in break_jumps {
                self.patch_jump(j, span);
            }
        }
    }

    fn compile_break(&mut self, is_break: bool, span: Span) {
        // Walk control-flow frames to the nearest loop, running each crossed
        // try's finally and popping its handlers, then discarding loop-body
        // locals before jumping.
        let cf_len = self.cur_ref().cf.len();
        let mut actions: Vec<(u8, Option<Block>)> = Vec::new();
        let mut target: Option<(usize, usize, usize)> = None; // (index, local_base, continue_target)
        for i in (0..cf_len).rev() {
            match &self.cur_ref().cf[i] {
                Cf::Try { finally, n_handlers } => actions.push((*n_handlers, finally.clone())),
                Cf::Loop { local_base, continue_target, .. } => {
                    target = Some((i, *local_base, *continue_target));
                    break;
                }
            }
        }
        let (loop_idx, local_base, continue_target) = match target {
            Some(t) => t,
            None => {
                self.error(span, "internal: break/continue with no enclosing loop");
                return;
            }
        };
        for (n, fin) in actions {
            for _ in 0..n {
                self.emit_op(OpCode::PopHandler, span.line);
            }
            if let Some(f) = fin {
                self.compile_block(&f);
            }
        }
        self.discard_locals_above(local_base, span.line);
        if is_break {
            let j = self.emit_jump(OpCode::Jump, span.line);
            if let Cf::Loop { break_jumps, .. } = &mut self.cur().cf[loop_idx] {
                break_jumps.push(j);
            }
        } else {
            self.emit_loop(continue_target, span);
        }
    }

    /// Emit cleanup (`POP`/`CLOSE_UPVALUE`) for locals above `base` *without*
    /// removing them from tracking (compilation of the body continues).
    fn discard_locals_above(&mut self, base: usize, line: u32) {
        let locals = &self.cur_ref().locals;
        let ops: Vec<bool> = (base..locals.len()).rev().map(|i| locals[i].is_captured).collect();
        for captured in ops {
            self.emit_op(if captured { OpCode::CloseUpvalue } else { OpCode::Pop }, line);
        }
    }

    fn compile_return(&mut self, value: Option<&Expr>, span: Span) {
        let has_finally = self
            .cur_ref()
            .cf
            .iter()
            .any(|c| matches!(c, Cf::Try { finally: Some(_), .. }));
        // Tail-call optimization: `return f(args);` reuses the current frame when
        // no `finally` must run first (DESIGN D30). Spread calls keep the normal
        // path (the argument count is dynamic).
        if !has_finally {
            if let Some(Expr { kind: ExprKind::Call { callee, args, paren_span }, span: cspan }) = value {
                if !args.iter().any(|a| matches!(a, CallArg::Spread(_))) {
                    self.compile_tail_call(callee, args, *paren_span, cspan.line, *cspan);
                    return;
                }
            }
        }
        match value {
            Some(e) => self.compile_value_expr(e),
            None => {
                if matches!(self.cur_ref().kind, FnKind::Initializer) {
                    self.emit_op_u8(OpCode::GetLocal, 0, span.line); // return `this`
                } else {
                    self.emit_op(OpCode::Nil, span.line);
                }
            }
        }
        if has_finally {
            // Park the return value in a temp so the finally blocks' locals do
            // not collide with it, run them, then reload and return.
            self.begin_scope();
            self.add_local("@ret", span);
            let ret_slot = (self.cur_ref().locals.len() - 1) as u8;
            let finallys: Vec<Block> = self
                .cur_ref()
                .cf
                .iter()
                .rev()
                .filter_map(|c| match c {
                    Cf::Try { finally: Some(f), .. } => Some(f.clone()),
                    _ => None,
                })
                .collect();
            for f in finallys {
                self.compile_block(&f);
            }
            self.emit_op_u8(OpCode::GetLocal, ret_slot, span.line);
            self.emit_op(OpCode::Return, span.line);
            self.discard_scope();
        } else {
            self.emit_op(OpCode::Return, span.line);
        }
    }

    /// Drop the current scope from compiler tracking with no emitted cleanup
    /// (used after an unconditional `Return`, where cleanup would be dead code).
    fn discard_scope(&mut self) {
        let depth = self.cur_ref().scope_depth;
        self.cur().locals.retain(|l| l.depth < depth);
        self.cur().scope_depth -= 1;
    }

    fn compile_try(
        &mut self,
        body: &Block,
        catches: &[CatchClause],
        finally: Option<&Block>,
        span: Span,
    ) {
        let line = span.line;
        match (catches.is_empty(), finally) {
            // try / catch (no finally)
            (false, None) => {
                let catch_jump = self.emit_jump(OpCode::PushHandler, line);
                self.cur().cf.push(Cf::Try { finally: None, n_handlers: 1 });
                self.compile_block(body);
                self.cur().cf.pop();
                self.emit_op(OpCode::PopHandler, line);
                let end_jump = self.emit_jump(OpCode::Jump, line);
                self.patch_jump(catch_jump, span);
                self.compile_catch_dispatch(catches, span);
                self.patch_jump(end_jump, span);
            }
            // try / catch / finally
            (false, Some(f)) => {
                let fin_handler = self.emit_jump(OpCode::PushHandler, line);
                let catch_jump = self.emit_jump(OpCode::PushHandler, line);
                self.cur().cf.push(Cf::Try { finally: Some(f.clone()), n_handlers: 2 });
                self.compile_block(body);
                self.cur().cf.pop();
                self.emit_op(OpCode::PopHandler, line); // pop catch handler
                let after_body = self.emit_jump(OpCode::Jump, line);
                self.patch_jump(catch_jump, span);
                self.compile_catch_dispatch(catches, span);
                self.patch_jump(after_body, span); // normal-B path joins here
                self.emit_op(OpCode::PopHandler, line); // pop finally handler
                self.compile_block(f); // normal-path finally
                let end_jump = self.emit_jump(OpCode::Jump, line);
                // finally-handler landing pad: an exception escaped try/catch.
                self.patch_jump(fin_handler, span);
                self.begin_scope();
                self.add_local("@exc", span);
                let exc_slot = (self.cur_ref().locals.len() - 1) as u8;
                self.compile_block(f);
                self.emit_op_u8(OpCode::GetLocal, exc_slot, line);
                self.emit_op(OpCode::Throw, line); // rethrow
                self.discard_scope();
                self.patch_jump(end_jump, span);
            }
            // try / finally (no catch): exceptions run finally, then propagate.
            (true, Some(f)) => {
                let fin_handler = self.emit_jump(OpCode::PushHandler, line);
                self.cur().cf.push(Cf::Try { finally: Some(f.clone()), n_handlers: 1 });
                self.compile_block(body);
                self.cur().cf.pop();
                self.emit_op(OpCode::PopHandler, line);
                self.compile_block(f); // normal-path finally
                let end_jump = self.emit_jump(OpCode::Jump, line);
                self.patch_jump(fin_handler, span); // landing pad
                self.begin_scope();
                self.add_local("@exc", span);
                let exc_slot = (self.cur_ref().locals.len() - 1) as u8;
                self.compile_block(f);
                self.emit_op_u8(OpCode::GetLocal, exc_slot, line);
                self.emit_op(OpCode::Throw, line); // rethrow
                self.discard_scope();
                self.patch_jump(end_jump, span);
            }
            (true, None) => unreachable!("parser guarantees catch or finally"),
        }
    }

    /// Compile the catch clauses as a dispatch chain (DESIGN D28). On entry the
    /// thrown value is on top of the stack. Each typed clause tests `MATCH_ERROR`
    /// and runs its body on a match; a bare clause runs unconditionally; with no
    /// bare clause the chain re-throws. The thrown value is consumed on every
    /// path, and control falls through to the code after this (the catch exit).
    fn compile_catch_dispatch(&mut self, catches: &[CatchClause], span: Span) {
        let line = span.line;
        let mut end_jumps = Vec::new();
        let mut has_bare = false;
        for c in catches {
            match &c.kind {
                Some(kind) => {
                    self.emit_op(OpCode::Dup, line); // [exc, exc]
                    let kidx = self.string_const(kind, span);
                    self.emit_op_u16(OpCode::MatchError, kidx, line); // [exc, bool]
                    let skip = self.emit_jump(OpCode::JumpIfFalse, line);
                    self.emit_op(OpCode::Pop, line); // matched: drop bool -> [exc]
                    self.compile_catch_body(c, span); // binds exc, runs body, pops exc
                    end_jumps.push(self.emit_jump(OpCode::Jump, line));
                    self.patch_jump(skip, span); // unmatched: [exc, bool]
                    self.emit_op(OpCode::Pop, line); // drop bool -> [exc]
                }
                None => {
                    has_bare = true;
                    self.compile_catch_body(c, span); // binds exc, runs body, pops exc
                    end_jumps.push(self.emit_jump(OpCode::Jump, line));
                    break; // a bare catch catches everything; rest is unreachable
                }
            }
        }
        if !has_bare {
            self.emit_op(OpCode::Throw, line); // no clause matched: re-raise
        }
        for j in end_jumps {
            self.patch_jump(j, span);
        }
    }

    /// Bind the on-stack thrown value to a clause's variable and run its body
    /// (the binding is cleaned up when the scope ends).
    fn compile_catch_body(&mut self, c: &CatchClause, span: Span) {
        self.begin_scope();
        self.add_local(&c.name, span);
        for s in &c.body.stmts {
            self.compile_stmt(s);
        }
        self.end_scope(span.line);
    }

    fn compile_function(&mut self, f: &Function, kind: FnKind, line: u32) {
        let slot0 = if matches!(kind, FnKind::Method | FnKind::Initializer) { "this" } else { "" };
        self.funcs.push(FnState::new(kind, f.name.clone(), slot0));
        self.begin_scope();

        let has_rest = f.params.last().map(|p| p.is_rest).unwrap_or(false);
        let fixed = if has_rest { f.params.len() - 1 } else { f.params.len() };
        let required = f
            .params
            .iter()
            .take_while(|p| p.default.is_none() && !p.is_rest)
            .count();
        self.cur().arity = fixed;

        for p in &f.params {
            self.add_local(&p.name, p.span);
        }
        // Prologue: fill any omitted optional parameters with their defaults.
        for (i, p) in f.params.iter().enumerate() {
            if let Some(d) = &p.default {
                self.emit_op(OpCode::DefaultArg, line);
                self.emit_byte(i as u8, line);
                let skip_at = self.chunk().len();
                self.chunk().write_u16(0xFFFF, line);
                self.compile_expr(d);
                self.emit_op_u8(OpCode::SetLocal, (i + 1) as u8, line);
                self.emit_op(OpCode::Pop, line);
                self.patch_jump(skip_at, f.span);
            }
        }

        for s in &f.body.stmts {
            self.compile_stmt(s);
        }
        // Implicit return.
        if matches!(kind, FnKind::Initializer) {
            self.emit_op_u8(OpCode::GetLocal, 0, line);
        } else {
            self.emit_op(OpCode::Nil, line);
        }
        self.emit_op(OpCode::Return, line);

        let state = self.funcs.pop().unwrap();
        let upvalues = state.upvalues;
        let proto = Rc::new(FnProto {
            name: state.name,
            arity: state.arity,
            required_arity: required,
            has_rest,
            upvalue_count: upvalues.len(),
            is_generator: stmts_contain_yield(&f.body.stmts),
            chunk: state.chunk,
            kind: state.kind,
            exports: Vec::new(),
            local_names: state.local_names,
        });
        let idx = self.make_const(Constant::Fn(proto), f.span);
        self.emit_op_u16(OpCode::Closure, idx, line);
        for up in &upvalues {
            self.emit_byte(u8::from(up.is_local), line);
            self.emit_byte(up.index, line);
        }
    }

    /// Compile `let <pattern> = init;` — evaluate `init` into a hidden temp,
    /// then extract and bind each variable (as globals at top level, locals
    /// otherwise). Patterns are validated flat by the resolver.
    fn compile_destructure(&mut self, pattern: &Pattern, init: &Expr, span: Span) {
        self.compile_expr(init);
        self.define_variable("@destr", span); // the source value
        match &pattern.kind {
            PatternKind::Array(elems) => {
                let rest_pos = elems.iter().position(|e| matches!(e, PatElem::Rest(_)));
                let len = elems.len() as i64;
                for (i, el) in elems.iter().enumerate() {
                    match el {
                        PatElem::Pattern(p) => {
                            if let PatternKind::Binding(name) = &p.kind {
                                let idx = if rest_pos.map(|rp| i < rp).unwrap_or(true) {
                                    i as i64
                                } else {
                                    (i as i64) - len
                                };
                                self.named_variable_get("@destr", span);
                                self.emit_load_const(Constant::Int(idx), span);
                                self.emit_op(OpCode::IndexGet, span.line);
                                self.define_variable(name, span);
                            }
                        }
                        PatElem::Rest(Some(name)) => {
                            let front = rest_pos.unwrap();
                            let back = elems.len() - front - 1;
                            self.named_variable_get("@destr", span);
                            self.emit_op(OpCode::ArrayRest, span.line);
                            self.emit_byte(front as u8, span.line);
                            self.emit_byte(back as u8, span.line);
                            self.define_variable(name, span);
                        }
                        PatElem::Rest(None) => {}
                    }
                }
            }
            PatternKind::Map(entries) => {
                for (key, p) in entries {
                    if let PatternKind::Binding(name) = &p.kind {
                        self.named_variable_get("@destr", span);
                        self.emit_load_const(Constant::Str(key.clone()), span);
                        self.emit_op(OpCode::IndexGet, span.line);
                        self.define_variable(name, span);
                    }
                }
            }
            _ => {} // rejected by the resolver
        }
    }

    /// Compile `<pattern> = value;`. The RHS is evaluated once and kept on the
    /// stack as an anonymous temp; each target is read out with DUP + INDEX_GET
    /// (or ARRAY_REST) and stored into its existing variable, then the temp is
    /// dropped. Using DUP avoids a named temp local that would otherwise leak.
    fn compile_destructure_assign(&mut self, pattern: &Pattern, value: &Expr, span: Span) {
        self.compile_expr(value); // [..., src]
        match &pattern.kind {
            PatternKind::Array(elems) => {
                let rest_pos = elems.iter().position(|e| matches!(e, PatElem::Rest(_)));
                let len = elems.len() as i64;
                for (i, el) in elems.iter().enumerate() {
                    match el {
                        PatElem::Pattern(p) => {
                            if let PatternKind::Binding(name) = &p.kind {
                                let idx = if rest_pos.map(|rp| i < rp).unwrap_or(true) {
                                    i as i64
                                } else {
                                    (i as i64) - len
                                };
                                self.emit_op(OpCode::Dup, span.line);
                                self.emit_load_const(Constant::Int(idx), span);
                                self.emit_op(OpCode::IndexGet, span.line);
                                self.named_variable_set(name, span);
                                self.emit_op(OpCode::Pop, span.line);
                            }
                        }
                        PatElem::Rest(Some(name)) => {
                            let front = rest_pos.unwrap();
                            let back = elems.len() - front - 1;
                            self.emit_op(OpCode::Dup, span.line);
                            self.emit_op(OpCode::ArrayRest, span.line);
                            self.emit_byte(front as u8, span.line);
                            self.emit_byte(back as u8, span.line);
                            self.named_variable_set(name, span);
                            self.emit_op(OpCode::Pop, span.line);
                        }
                        PatElem::Rest(None) => {}
                    }
                }
            }
            PatternKind::Map(entries) => {
                for (key, p) in entries {
                    if let PatternKind::Binding(name) = &p.kind {
                        self.emit_op(OpCode::Dup, span.line);
                        self.emit_load_const(Constant::Str(key.clone()), span);
                        self.emit_op(OpCode::IndexGet, span.line);
                        self.named_variable_set(name, span);
                        self.emit_op(OpCode::Pop, span.line);
                    }
                }
            }
            _ => {} // rejected by the resolver
        }
        self.emit_op(OpCode::Pop, span.line); // drop the source temp
    }

    fn compile_class(&mut self, c: &ClassDecl) {
        let line = c.span.line;
        let name_idx = self.string_const(&c.name, c.name_span);
        // 1. Create and bind the (empty) class so methods can refer to it.
        if self.is_global_scope() {
            self.emit_op_u16(OpCode::Class, name_idx, line);
            self.emit_op_u16(OpCode::DefineGlobal, name_idx, line);
        } else {
            self.add_local(&c.name, c.name_span);
            self.emit_op_u16(OpCode::Class, name_idx, line);
        }
        // 2. Scope holding "super" (if any) and a stack-top reference to the
        //    class for method definition.
        self.begin_scope();
        if let Some(sc) = &c.superclass {
            self.named_variable_get(&sc.value, sc.span);
            self.add_local("super", sc.span);
        }
        self.named_variable_get(&c.name, c.name_span); // push the class
        if c.superclass.is_some() {
            self.emit_op(OpCode::Inherit, line); // copy-down inheritance
        }
        // 3. Instance methods other than `init` (each a closure that may capture
        //    "super"). `init` is compiled from `effective_init` so field
        //    initializers run at the top of the constructor (DESIGN D27).
        for m in &c.methods {
            if m.name.as_deref() == Some("init") {
                continue;
            }
            self.compile_function(m, FnKind::Method, m.span.line);
            let m_idx = self.string_const(m.name.as_deref().unwrap_or(""), m.name_span);
            self.emit_op_u16(OpCode::Method, m_idx, m.span.line);
        }
        if let Some(init) = c.effective_init() {
            self.compile_function(&init, FnKind::Initializer, init.span.line);
            let m_idx = self.string_const("init", init.name_span);
            self.emit_op_u16(OpCode::Method, m_idx, init.span.line);
        }
        // 4. Static methods: ordinary functions stored in the class's static table.
        for s in &c.statics {
            self.compile_function(s, FnKind::Function, s.span.line);
            let s_idx = self.string_const(s.name.as_deref().unwrap_or(""), s.name_span);
            self.emit_op_u16(OpCode::StaticMethod, s_idx, s.span.line);
        }
        // 5. Pop the stack-top class reference; end the scope (closes "super").
        self.emit_op(OpCode::Pop, line);
        self.end_scope(line);
    }

    fn compile_import(&mut self, im: &Import) {
        let line = im.span.line;
        let path_idx = self.string_const(&im.path, im.path_span);
        self.emit_op_u16(OpCode::Import, path_idx, line);
        match &im.kind {
            ImportKind::Module { alias } => {
                self.define_variable(&alias.value, alias.span);
            }
            ImportKind::Named(names) => {
                if self.is_global_scope() {
                    for n in names {
                        self.emit_op(OpCode::Dup, line); // copy module
                        let nidx = self.string_const(&n.value, n.span);
                        self.emit_op_u16(OpCode::GetProp, nidx, line);
                        let didx = self.string_const(&n.value, n.span);
                        self.emit_op_u16(OpCode::DefineGlobal, didx, line);
                    }
                    self.emit_op(OpCode::Pop, line); // drop the module
                } else {
                    self.add_local("@mod", im.path_span);
                    let mod_slot = (self.cur_ref().locals.len() - 1) as u8;
                    for n in names {
                        self.emit_op_u8(OpCode::GetLocal, mod_slot, line);
                        let nidx = self.string_const(&n.value, n.span);
                        self.emit_op_u16(OpCode::GetProp, nidx, line);
                        self.add_local(&n.value, n.span);
                    }
                }
            }
        }
    }

    // ---- expressions -------------------------------------------------------

    fn compile_expr(&mut self, expr: &Expr) {
        let span = expr.span;
        let line = span.line;
        // Whether this is the value of a statement (clean operand stack). Read and
        // cleared here so nested sub-expressions see `false`; only `match` uses it.
        let clean = std::mem::take(&mut self.stmt_value_pos);
        match &expr.kind {
            ExprKind::Int(n) => self.emit_load_const(Constant::Int(*n), span),
            ExprKind::Float(f) => self.emit_load_const(Constant::Float(*f), span),
            ExprKind::Str(s) => self.emit_load_const(Constant::Str(s.clone()), span),
            ExprKind::Bool(true) => self.emit_op(OpCode::True, line),
            ExprKind::Bool(false) => self.emit_op(OpCode::False, line),
            ExprKind::Nil => self.emit_op(OpCode::Nil, line),
            ExprKind::StrInterp(segments) => {
                let n = segments.len();
                for seg in segments {
                    match seg {
                        StrSegment::Text(t) => self.emit_load_const(Constant::Str(t.clone()), span),
                        StrSegment::Expr(e) => self.compile_expr(e),
                    }
                }
                if n > 255 {
                    self.error(span, "too many interpolation segments (max 255)");
                }
                self.emit_op_u8(OpCode::Interpolate, n as u8, line);
            }
            ExprKind::Array(elems) => {
                self.emit_op(OpCode::NewArray, line);
                for el in elems {
                    match el {
                        ArrayElem::Item(e) => {
                            self.compile_expr(e);
                            self.emit_op(OpCode::ArrayPush, line);
                        }
                        ArrayElem::Spread(e) => {
                            self.compile_expr(e);
                            self.emit_op(OpCode::ArrayExtend, line);
                        }
                    }
                }
            }
            ExprKind::Map(entries) => {
                self.emit_op(OpCode::NewMap, line);
                for (k, v) in entries {
                    match k {
                        MapKey::Str(s) => self.emit_load_const(Constant::Str(s.clone()), span),
                        MapKey::Computed(e) => self.compile_expr(e),
                    }
                    self.compile_expr(v);
                    self.emit_op(OpCode::MapInsert, line);
                }
            }
            ExprKind::Var(name) => self.named_variable_get(name, span),
            ExprKind::This => self.named_variable_get("this", span),
            ExprKind::Super { method, .. } => {
                self.named_variable_get("this", span); // receiver
                self.named_variable_get("super", span); // superclass
                let idx = self.string_const(method, span);
                self.emit_op_u16(OpCode::GetSuper, idx, line);
            }
            ExprKind::Assign { target, value } => self.compile_assign(target, value, span),
            ExprKind::CompoundAssign { target, op, value } => {
                self.compile_compound_assign(target, *op, value, span)
            }
            ExprKind::Unary { op, operand } => {
                self.compile_expr(operand);
                self.emit_op(
                    match op {
                        UnaryOp::Neg => OpCode::Neg,
                        UnaryOp::Not => OpCode::Not,
                        UnaryOp::BitNot => OpCode::BitNot,
                    },
                    line,
                );
            }
            ExprKind::Binary { op, left, right } => {
                self.compile_expr(left);
                self.compile_expr(right);
                self.emit_op(binary_op(*op), line);
            }
            ExprKind::Logical { op, left, right } => self.compile_logical(*op, left, right, span),
            ExprKind::Ternary { cond, then_branch, else_branch } => {
                self.compile_ternary(cond, then_branch, else_branch, span)
            }
            ExprKind::Call { callee, args, paren_span } => {
                self.compile_call(callee, args, *paren_span, line, span)
            }
            ExprKind::Index { object, index } => {
                self.compile_expr(object);
                self.compile_expr(index);
                self.emit_op(OpCode::IndexGet, line);
            }
            ExprKind::Get { object, name, .. } => {
                self.compile_expr(object);
                let idx = self.string_const(name, span);
                self.emit_op_u16(OpCode::GetProp, idx, line);
            }
            ExprKind::Lambda(f) => self.compile_function(f, FnKind::Function, line),
            ExprKind::Match { subject, arms } => self.compile_match(subject, arms, span, clean),
            ExprKind::ArrayComp { element, var, var_span, iter, cond } => {
                self.compile_comprehension(Comp::Array(element), var, *var_span, iter, cond.as_deref(), span)
            }
            ExprKind::MapComp { key, value, var, var_span, iter, cond } => {
                self.compile_comprehension(Comp::Map(key, value), var, *var_span, iter, cond.as_deref(), span)
            }
        }
    }

    fn compile_call(
        &mut self,
        callee: &Expr,
        args: &[CallArg],
        paren_span: Span,
        line: u32,
        span: Span,
    ) {
        let has_spread = args.iter().any(|a| matches!(a, CallArg::Spread(_)));
        if has_spread {
            // Build the argument list as an array (reusing the array-literal
            // spread machinery), then apply it with CALL_SPREAD. This path also
            // handles method/super calls without the INVOKE fusion: the callee is
            // first materialized as a (possibly bound) value on the stack.
            if let ExprKind::Get { object, name, name_span } = &callee.kind {
                self.compile_expr(object);
                let idx = self.string_const(name, *name_span);
                self.emit_op_u16(OpCode::GetProp, idx, line);
            } else if let ExprKind::Super { method, method_span } = &callee.kind {
                self.named_variable_get("this", span);
                self.named_variable_get("super", span);
                let idx = self.string_const(method, *method_span);
                self.emit_op_u16(OpCode::GetSuper, idx, line);
            } else {
                self.compile_expr(callee);
            }
            self.emit_op(OpCode::NewArray, line);
            for a in args {
                match a {
                    CallArg::Item(e) => {
                        self.compile_expr(e);
                        self.emit_op(OpCode::ArrayPush, line);
                    }
                    CallArg::Spread(e) => {
                        self.compile_expr(e);
                        self.emit_op(OpCode::ArrayExtend, line);
                    }
                }
            }
            self.emit_op(OpCode::CallSpread, line);
            return;
        }

        if args.len() > 255 {
            self.error(paren_span, "too many call arguments (max 255)");
        }
        // `obj.method(args)` fuses to a single INVOKE and `super.m(args)`
        // to SUPER_INVOKE — both skip the bound-method allocation of the
        // generic GET_PROP/GET_SUPER + CALL path.
        if let ExprKind::Get { object, name, name_span } = &callee.kind {
            self.compile_expr(object);
            for a in args {
                self.compile_call_arg(a);
            }
            let idx = self.string_const(name, *name_span);
            self.emit_op_u16(OpCode::Invoke, idx, line);
            self.emit_byte(args.len() as u8, line);
        } else if let ExprKind::Super { method, method_span } = &callee.kind {
            // Layout: [this, args…, superclass]. SUPER_INVOKE pops the
            // superclass and calls with `this` already in the receiver slot.
            self.named_variable_get("this", span);
            for a in args {
                self.compile_call_arg(a);
            }
            self.named_variable_get("super", span);
            let idx = self.string_const(method, *method_span);
            self.emit_op_u16(OpCode::SuperInvoke, idx, line);
            self.emit_byte(args.len() as u8, line);
        } else {
            self.compile_expr(callee);
            for a in args {
                self.compile_call_arg(a);
            }
            self.emit_op_u8(OpCode::Call, args.len() as u8, line);
        }
    }

    /// Compile a non-spread call argument (the spread path is handled inline in
    /// `compile_call`).
    fn compile_call_arg(&mut self, arg: &CallArg) {
        match arg {
            CallArg::Item(e) => self.compile_expr(e),
            CallArg::Spread(_) => unreachable!("spread args use the CALL_SPREAD path"),
        }
    }

    /// Compile a tail-position call (`return f(args);`) as `TAIL_CALL` + `RETURN`
    /// (DESIGN D30). Method/super callees first materialize a bound method (no
    /// INVOKE fusion) so the single `TAIL_CALL` path handles them too.
    fn compile_tail_call(
        &mut self,
        callee: &Expr,
        args: &[CallArg],
        paren_span: Span,
        line: u32,
        span: Span,
    ) {
        if args.len() > 255 {
            self.error(paren_span, "too many call arguments (max 255)");
        }
        if let ExprKind::Get { object, name, name_span } = &callee.kind {
            self.compile_expr(object);
            let idx = self.string_const(name, *name_span);
            self.emit_op_u16(OpCode::GetProp, idx, line);
        } else if let ExprKind::Super { method, method_span } = &callee.kind {
            self.named_variable_get("this", span);
            self.named_variable_get("super", span);
            let idx = self.string_const(method, *method_span);
            self.emit_op_u16(OpCode::GetSuper, idx, line);
        } else {
            self.compile_expr(callee);
        }
        for a in args {
            self.compile_call_arg(a);
        }
        self.emit_op_u8(OpCode::TailCall, args.len() as u8, line);
        // For an optimized (closure) callee this is dead code; otherwise it
        // returns the call's result from the current frame.
        self.emit_op(OpCode::Return, line);
    }

    fn compile_assign(&mut self, target: &Expr, value: &Expr, span: Span) {
        match &target.kind {
            ExprKind::Var(name) => {
                self.compile_expr(value);
                self.named_variable_set(name, span);
            }
            ExprKind::Index { object, index } => {
                self.compile_expr(object);
                self.compile_expr(index);
                self.compile_expr(value);
                self.emit_op(OpCode::IndexSet, span.line);
            }
            ExprKind::Get { object, name, .. } => {
                self.compile_expr(object);
                self.compile_expr(value);
                let idx = self.string_const(name, span);
                self.emit_op_u16(OpCode::SetProp, idx, span.line);
            }
            _ => self.error(span, "invalid assignment target"),
        }
    }

    /// `target op= value`, evaluating the target (and any object/index
    /// sub-expressions) exactly once.
    fn compile_compound_assign(&mut self, target: &Expr, op: BinaryOp, value: &Expr, span: Span) {
        let line = span.line;
        match &target.kind {
            ExprKind::Var(name) => {
                self.named_variable_get(name, span);
                self.compile_expr(value);
                self.emit_op(binary_op(op), line);
                self.named_variable_set(name, span);
            }
            ExprKind::Get { object, name, .. } => {
                // `obj` is evaluated once and duplicated for the read; `SetProp`
                // consumes the original. Top-relative ops, so safe when nested.
                self.compile_expr(object); // [obj]
                self.emit_op(OpCode::Dup, line); // [obj, obj]
                let idx = self.string_const(name, span);
                self.emit_op_u16(OpCode::GetProp, idx, line); // [obj, cur]
                self.compile_expr(value); // [obj, cur, rhs]
                self.emit_op(binary_op(op), line); // [obj, result]
                self.emit_op_u16(OpCode::SetProp, idx, line); // [result]
            }
            ExprKind::Index { object, index } => {
                // `obj` and `index` are each evaluated once, then `Dup2` copies
                // the pair for the read while `INDEX_SET` consumes the originals.
                // All ops are top-relative, so this is correct even when the
                // compound assignment is nested in a larger expression.
                self.compile_expr(object); // [obj]
                self.compile_expr(index); // [obj, idx]
                self.emit_op(OpCode::Dup2, line); // [obj, idx, obj, idx]
                self.emit_op(OpCode::IndexGet, line); // [obj, idx, cur]
                self.compile_expr(value); // [obj, idx, cur, rhs]
                self.emit_op(binary_op(op), line); // [obj, idx, result]
                self.emit_op(OpCode::IndexSet, line); // [result]
            }
            _ => self.error(span, "invalid assignment target"),
        }
    }

    fn compile_logical(&mut self, op: LogicalOp, left: &Expr, right: &Expr, span: Span) {
        let line = span.line;
        match op {
            LogicalOp::And => {
                self.compile_expr(left);
                let end = self.emit_jump(OpCode::JumpIfFalse, line); // short-circuit, keep left
                self.emit_op(OpCode::Pop, line);
                self.compile_expr(right);
                self.patch_jump(end, span);
            }
            LogicalOp::Or => {
                self.compile_expr(left);
                let else_jump = self.emit_jump(OpCode::JumpIfFalse, line);
                let end = self.emit_jump(OpCode::Jump, line); // left truthy: keep it
                self.patch_jump(else_jump, span);
                self.emit_op(OpCode::Pop, line);
                self.compile_expr(right);
                self.patch_jump(end, span);
            }
        }
    }

    /// `cond ? then : else` — like the `||`/`&&` lowering, `JumpIfFalse` leaves
    /// the condition on the stack, so each branch pops it before evaluating.
    fn compile_ternary(&mut self, cond: &Expr, then_branch: &Expr, else_branch: &Expr, span: Span) {
        let line = span.line;
        self.compile_expr(cond);
        let else_jump = self.emit_jump(OpCode::JumpIfFalse, line);
        self.emit_op(OpCode::Pop, line); // discard the (truthy) condition
        self.compile_expr(then_branch);
        let end = self.emit_jump(OpCode::Jump, line);
        self.patch_jump(else_jump, span);
        self.emit_op(OpCode::Pop, line); // discard the (falsy) condition
        self.compile_expr(else_branch);
        self.patch_jump(end, span);
    }

    // ---- pattern matching --------------------------------------------------

    /// Compile a `match` expression. At a statement-value position (a clean
    /// operand stack) it compiles in place; otherwise it wraps itself in an
    /// immediately-invoked function so its `@subj` and binding slots are correct
    /// regardless of any operand temporaries already on the stack (DESIGN D34).
    fn compile_match(&mut self, subject: &Expr, arms: &[MatchArm], span: Span, clean: bool) {
        if clean {
            self.compile_match_inplace(subject, arms, span);
        } else {
            self.compile_match_iife(subject, arms, span);
        }
    }

    fn compile_match_inplace(&mut self, subject: &Expr, arms: &[MatchArm], span: Span) {
        self.begin_scope();
        self.compile_expr(subject);
        self.add_local("@subj", span);
        let subj_slot = (self.cur_ref().locals.len() - 1) as u8;
        self.compile_match_dispatch(arms, subj_slot, span);
        // The match value sits in @subj's slot; detach the temp without emitting
        // a pop so the value remains as this expression's result.
        self.cur().locals.pop();
        self.cur().scope_depth -= 1;
    }

    /// Compile the match as an IIFE: the subject is evaluated in the enclosing
    /// scope and passed as the single argument (slot 1), so the dispatch's slots
    /// are clean; the result is returned (DESIGN D34).
    fn compile_match_iife(&mut self, subject: &Expr, arms: &[MatchArm], span: Span) {
        let line = span.line;
        self.funcs.push(FnState::new(FnKind::Function, None, ""));
        self.cur().arity = 1;
        self.begin_scope();
        self.add_local("@subj", span); // the subject parameter
        let subj_slot = (self.cur_ref().locals.len() - 1) as u8;
        self.compile_match_dispatch(arms, subj_slot, span);
        self.emit_op_u8(OpCode::GetLocal, subj_slot, line); // the result, left in @subj
        self.emit_op(OpCode::Return, line);
        let state = self.funcs.pop().unwrap();
        let upvalues = state.upvalues;
        let proto = Rc::new(FnProto {
            name: None,
            arity: 1,
            required_arity: 1,
            has_rest: false,
            upvalue_count: upvalues.len(),
            is_generator: false,
            chunk: state.chunk,
            kind: FnKind::Function,
            exports: Vec::new(),
            local_names: state.local_names,
        });
        let idx = self.make_const(Constant::Fn(proto), span);
        self.emit_op_u16(OpCode::Closure, idx, line);
        for up in &upvalues {
            self.emit_byte(u8::from(up.is_local), line);
            self.emit_byte(up.index, line);
        }
        self.compile_expr(subject); // the argument, in the enclosing scope
        self.emit_op_u8(OpCode::Call, 1, line);
    }

    /// The arm dispatch shared by both match paths: the subject is in slot
    /// `subj_slot`, and the matching arm's value is left in that same slot.
    fn compile_match_dispatch(&mut self, arms: &[MatchArm], subj_slot: u8, span: Span) {
        let line = span.line;
        let mut end_jumps = Vec::new();
        for arm in arms {
            // Phase 1: a side-effect-free test leaving exactly one bool.
            self.compile_pattern_test(&arm.pattern, &Access::Local(subj_slot), span);
            let next_arm = self.emit_jump(OpCode::JumpIfFalse, line);
            self.emit_op(OpCode::Pop, line); // matched: drop the test bool

            self.begin_scope();
            let bind_base = self.cur_ref().locals.len();
            // Phase 2: bind pattern variables (structure already verified).
            self.compile_pattern_bind(&arm.pattern, &Access::Local(subj_slot), span);

            let mut guard_fail = None;
            if let Some(g) = &arm.guard {
                self.compile_expr(g);
                guard_fail = Some(self.emit_jump(OpCode::JumpIfFalse, line));
                self.emit_op(OpCode::Pop, line); // guard true: drop its bool
            }

            // Body -> result, then move it into @subj and clean bindings.
            self.compile_expr(&arm.body);
            self.emit_op_u8(OpCode::SetLocal, subj_slot, line); // @subj = result
            self.emit_op(OpCode::Pop, line); // drop the result copy left by SetLocal
            self.clean_arm_bindings(bind_base, line);
            end_jumps.push(self.emit_jump(OpCode::Jump, line));

            // Guard-failed landing: drop guard bool + bindings, fall to next arm.
            if let Some(gf) = guard_fail {
                self.patch_jump(gf, span);
                self.emit_op(OpCode::Pop, line); // drop guard bool
                self.clean_arm_bindings(bind_base, line);
                // fall through to end_scope below, then next_arm
            }
            self.discard_scope(); // drop the arm's binding scope from tracking

            // Pattern-failed landing: drop the test bool, fall to next arm.
            self.patch_jump(next_arm, span);
            self.emit_op(OpCode::Pop, line);
        }
        // No arm matched.
        self.emit_load_const(Constant::Str("no matching pattern in match expression".into()), span);
        self.emit_op(OpCode::Throw, line);

        for j in end_jumps {
            self.patch_jump(j, span);
        }
        // The matching arm's value is left in @subj's slot; the caller decides
        // how to surface it (detach in place, or `RETURN` from the IIFE).
    }

    /// Emit cleanup for an arm's binding locals (between `base` and the top),
    /// keeping nothing else, and remove them from tracking.
    fn clean_arm_bindings(&mut self, base: usize, line: u32) {
        let locals = &self.cur_ref().locals;
        let ops: Vec<bool> = (base..locals.len()).rev().map(|i| locals[i].is_captured).collect();
        for captured in &ops {
            self.emit_op(if *captured { OpCode::CloseUpvalue } else { OpCode::Pop }, line);
        }
        self.cur().locals.truncate(base);
    }

    /// Emit code that pushes the value described by `acc`.
    fn emit_access(&mut self, acc: &Access, span: Span) {
        match acc {
            Access::Local(slot) => self.emit_op_u8(OpCode::GetLocal, *slot, span.line),
            Access::Index(base, i) => {
                self.emit_access(base, span);
                self.emit_load_const(Constant::Int(*i), span);
                self.emit_op(OpCode::IndexGet, span.line);
            }
            Access::Key(base, k) => {
                self.emit_access(base, span);
                self.emit_load_const(Constant::Str(k.clone()), span);
                self.emit_op(OpCode::IndexGet, span.line);
            }
        }
    }

    /// Phase 1: push a single bool — does `pat` match the value at `acc`?
    fn compile_pattern_test(&mut self, pat: &Pattern, acc: &Access, span: Span) {
        let line = span.line;
        match &pat.kind {
            PatternKind::Wildcard | PatternKind::Binding(_) => {
                self.emit_op(OpCode::True, line); // always matches
            }
            PatternKind::Int(n) => self.test_literal(acc, Constant::Int(*n), span),
            PatternKind::Float(f) => self.test_literal(acc, Constant::Float(*f), span),
            PatternKind::Str(s) => self.test_literal(acc, Constant::Str(s.clone()), span),
            PatternKind::Bool(b) => {
                self.emit_access(acc, span);
                self.emit_op(if *b { OpCode::True } else { OpCode::False }, line);
                self.emit_op(OpCode::Eq, line);
            }
            PatternKind::Nil => {
                self.emit_access(acc, span);
                self.emit_op(OpCode::Nil, line);
                self.emit_op(OpCode::Eq, line);
            }
            PatternKind::Array(elems) => self.test_array(elems, acc, span),
            PatternKind::Map(entries) => self.test_map(entries, acc, span),
            PatternKind::Or(alts) => self.test_or(alts, acc, span),
        }
    }

    /// `p1 | p2 | ...`: push `true` iff any alternative matches. Alternatives bind
    /// nothing (DESIGN D25), so this is a pure short-circuiting OR of their tests.
    fn test_or(&mut self, alts: &[Pattern], acc: &Access, span: Span) {
        let line = span.line;
        let mut successes = Vec::new();
        for alt in alts {
            self.compile_pattern_test(alt, acc, span); // [b]
            let is_false = self.emit_jump(OpCode::JumpIfFalse, line); // b false: keep, skip
            self.emit_op(OpCode::Pop, line); // b true: drop it
            successes.push(self.emit_jump(OpCode::Jump, line)); // -> success
            self.patch_jump(is_false, span);
            self.emit_op(OpCode::Pop, line); // drop the false bool, try next alt
        }
        self.emit_op(OpCode::False, line); // no alternative matched
        let done = self.emit_jump(OpCode::Jump, line);
        for s in successes {
            self.patch_jump(s, span);
        }
        self.emit_op(OpCode::True, line);
        self.patch_jump(done, span);
    }

    fn test_literal(&mut self, acc: &Access, c: Constant, span: Span) {
        self.emit_access(acc, span);
        self.emit_load_const(c, span);
        self.emit_op(OpCode::Eq, span.line);
    }

    fn test_array(&mut self, elems: &[PatElem], acc: &Access, span: Span) {
        let line = span.line;
        let rest_pos = elems.iter().position(|e| matches!(e, PatElem::Rest(_)));
        let front = rest_pos.unwrap_or(elems.len());
        let back = rest_pos.map(|p| elems.len() - p - 1).unwrap_or(0);
        let minlen = (front + back) as u8;
        let exact = u8::from(rest_pos.is_none());

        // is-array & length check
        self.emit_access(acc, span);
        self.emit_op(OpCode::MatchArray, line);
        self.emit_byte(minlen, line);
        self.emit_byte(exact, line);
        let mut fails = vec![self.emit_jump(OpCode::JumpIfFalse, line)];
        self.emit_op(OpCode::Pop, line);

        // test each fixed (non-rest) element
        for (i, el) in elems.iter().enumerate() {
            if let PatElem::Pattern(p) = el {
                let idx = if rest_pos.map(|rp| i < rp).unwrap_or(true) {
                    i as i64 // before the rest: index from the front
                } else {
                    (i as i64) - (elems.len() as i64) // after the rest: from the end
                };
                let sub = Access::Index(Box::new(clone_access(acc)), idx);
                self.compile_pattern_test(p, &sub, span);
                fails.push(self.emit_jump(OpCode::JumpIfFalse, line));
                self.emit_op(OpCode::Pop, line);
            }
        }
        self.emit_op(OpCode::True, line);
        let done = self.emit_jump(OpCode::Jump, line);
        for f in fails {
            self.patch_jump(f, span); // each leaves its false bool on the stack
        }
        self.patch_jump(done, span);
    }

    fn test_map(&mut self, entries: &[(String, Pattern)], acc: &Access, span: Span) {
        let line = span.line;
        let mut fails = Vec::new();
        for (k, p) in entries {
            // is-map & has-key
            self.emit_access(acc, span);
            let kidx = self.string_const(k, span);
            self.emit_op_u16(OpCode::MatchMapHas, kidx, line);
            fails.push(self.emit_jump(OpCode::JumpIfFalse, line));
            self.emit_op(OpCode::Pop, line);
            // value matches sub-pattern
            let sub = Access::Key(Box::new(clone_access(acc)), k.clone());
            self.compile_pattern_test(p, &sub, span);
            fails.push(self.emit_jump(OpCode::JumpIfFalse, line));
            self.emit_op(OpCode::Pop, line);
        }
        self.emit_op(OpCode::True, line);
        let done = self.emit_jump(OpCode::Jump, line);
        for f in fails {
            self.patch_jump(f, span);
        }
        self.patch_jump(done, span);
    }

    /// Phase 2: bind a pattern's variables as locals (structure already verified
    /// by phase 1, so all accesses are safe).
    fn compile_pattern_bind(&mut self, pat: &Pattern, acc: &Access, span: Span) {
        match &pat.kind {
            PatternKind::Binding(name) => {
                self.emit_access(acc, span);
                self.add_local(name, span);
            }
            PatternKind::Array(elems) => {
                let rest_pos = elems.iter().position(|e| matches!(e, PatElem::Rest(_)));
                for (i, el) in elems.iter().enumerate() {
                    match el {
                        PatElem::Pattern(p) => {
                            let idx = if rest_pos.map(|rp| i < rp).unwrap_or(true) {
                                i as i64
                            } else {
                                (i as i64) - (elems.len() as i64)
                            };
                            let sub = Access::Index(Box::new(clone_access(acc)), idx);
                            self.compile_pattern_bind(p, &sub, span);
                        }
                        PatElem::Rest(Some(name)) => {
                            let front = rest_pos.unwrap();
                            let back = elems.len() - front - 1;
                            self.emit_access(acc, span);
                            self.emit_op(OpCode::ArrayRest, span.line);
                            self.emit_byte(front as u8, span.line);
                            self.emit_byte(back as u8, span.line);
                            self.add_local(name, span);
                        }
                        PatElem::Rest(None) => {}
                    }
                }
            }
            PatternKind::Map(entries) => {
                for (k, p) in entries {
                    let sub = Access::Key(Box::new(clone_access(acc)), k.clone());
                    self.compile_pattern_bind(p, &sub, span);
                }
            }
            _ => {} // wildcard and literals bind nothing
        }
    }
}

fn clone_access(acc: &Access) -> Access {
    match acc {
        Access::Local(s) => Access::Local(*s),
        Access::Index(b, i) => Access::Index(Box::new(clone_access(b)), *i),
        Access::Key(b, k) => Access::Key(Box::new(clone_access(b)), k.clone()),
    }
}

/// Whether a statement list contains a `yield` belonging to *this* function —
/// i.e. not descending into nested function/lambda bodies (their `yield`s make
/// them generators). Used to mark a function as a generator (DESIGN D29).
fn stmts_contain_yield(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_contains_yield)
}

fn stmt_contains_yield(s: &Stmt) -> bool {
    match s {
        Stmt::Yield { .. } => true,
        Stmt::Block(b) => stmts_contain_yield(&b.stmts),
        Stmt::If { then_block, else_branch, .. } => {
            stmts_contain_yield(&then_block.stmts)
                || else_branch.as_deref().is_some_and(stmt_contains_yield)
        }
        Stmt::While { body, .. } => stmts_contain_yield(&body.stmts),
        Stmt::ForIn { body, .. } => stmts_contain_yield(&body.stmts),
        Stmt::ForC { init, body, .. } => {
            init.as_deref().is_some_and(stmt_contains_yield) || stmts_contain_yield(&body.stmts)
        }
        Stmt::Try { body, catches, finally, .. } => {
            stmts_contain_yield(&body.stmts)
                || catches.iter().any(|c| stmts_contain_yield(&c.body.stmts))
                || finally.as_ref().is_some_and(|f| stmts_contain_yield(&f.stmts))
        }
        // Functions/classes (and lambdas inside expressions) start a new function
        // scope, so their `yield`s belong to them, not here.
        _ => false,
    }
}

fn binary_op(op: BinaryOp) -> OpCode {
    match op {
        BinaryOp::Add => OpCode::Add,
        BinaryOp::Sub => OpCode::Sub,
        BinaryOp::Mul => OpCode::Mul,
        BinaryOp::Div => OpCode::Div,
        BinaryOp::Rem => OpCode::Rem,
        BinaryOp::Pow => OpCode::Pow,
        BinaryOp::Eq => OpCode::Eq,
        BinaryOp::Ne => OpCode::Ne,
        BinaryOp::Lt => OpCode::Lt,
        BinaryOp::Le => OpCode::Le,
        BinaryOp::Gt => OpCode::Gt,
        BinaryOp::Ge => OpCode::Ge,
        BinaryOp::Is => OpCode::Is,
        BinaryOp::BitAnd => OpCode::BitAnd,
        BinaryOp::BitOr => OpCode::BitOr,
        BinaryOp::BitXor => OpCode::BitXor,
        BinaryOp::Shl => OpCode::Shl,
        BinaryOp::Shr => OpCode::Shr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disassembler::disassemble;

    /// Compile `src` (front end must be clean) and return its disassembly.
    fn dis(src: &str) -> String {
        let (program, errs) = crate::parse_source(src);
        assert!(errs.is_empty(), "front-end errors: {errs:?}");
        let proto = compile(&program).expect("compile ok");
        disassemble(&proto)
    }

    #[test]
    fn locals_use_slots_not_globals() {
        // Inside a function, `a`/`b` are local slots, not globals. (The script
        // still binds the function name `f` with DEF_GLOBAL.)
        let d = dis("fn f() { let a = 1; let b = 2; return a + b; }");
        assert!(d.contains("== fn f"));
        assert!(d.contains("GET_LOCAL"));
        assert!(!d.contains("; \"a\"") && !d.contains("; \"b\"")); // a, b never named globals
    }

    #[test]
    fn closure_captures_upvalue() {
        let d = dis("fn outer() { let n = 1; return fn() { return n; }; }");
        assert!(d.contains("CLOSURE"));
        assert!(d.contains("GET_UPVALUE"));
        assert!(d.contains("local 1")); // capture enclosing local slot 1
    }

    #[test]
    fn while_loop_uses_loop_and_jump_if_false() {
        let d = dis("while true { break; }");
        assert!(d.contains("JUMP_IF_FALSE"));
        assert!(d.contains("LOOP"));
    }

    #[test]
    fn for_in_uses_iter_next() {
        let d = dis("for x in [1,2,3] { print(x); }");
        assert!(d.contains("ITER_NEXT"));
        assert!(d.contains("NEW_ARRAY"));
    }

    #[test]
    fn class_emits_class_method_inherit_super() {
        let d = dis(
            "class A { m() { return 1; } }
             class B < A { m() { let r = super.m(); return r; } }",
        );
        assert!(d.contains("CLASS"));
        assert!(d.contains("METHOD"));
        assert!(d.contains("INHERIT"));
        // A non-tail super *call* fuses to SUPER_INVOKE (no separate GET_SUPER + CALL).
        assert!(d.contains("SUPER_INVOKE"));
    }

    #[test]
    fn tail_call_emits_tail_call_opcode() {
        // `return f(...)` in tail position compiles to TAIL_CALL (DESIGN D30).
        let d = dis("fn f(n) { if n == 0 { return 0; } return f(n - 1); }");
        assert!(d.contains("TAIL_CALL"));
        // A tail super call materializes a bound method then TAIL_CALLs it.
        let d2 = dis(
            "class A { m() { return 1; } }
             class B < A { m() { return super.m(); } }",
        );
        assert!(d2.contains("GET_SUPER"));
        assert!(d2.contains("TAIL_CALL"));
    }

    #[test]
    fn super_reference_without_call_emits_get_super() {
        // Referencing a super method without immediately calling it still uses
        // GET_SUPER (only the fused call form becomes SUPER_INVOKE).
        let d = dis(
            "class A { m() { return 1; } }
             class B < A { m() { let f = super.m; return f(); } }",
        );
        assert!(d.contains("GET_SUPER"));
        assert!(!d.contains("SUPER_INVOKE"));
    }

    #[test]
    fn interpolation_emits_interpolate() {
        let d = dis(r#"let s = "a ${1} b ${2}";"#);
        assert!(d.contains("INTERPOLATE"));
    }

    #[test]
    fn try_emits_handlers() {
        let d = dis("try { throw 1; } catch (e) { print(e); }");
        assert!(d.contains("PUSH_HANDLER"));
        assert!(d.contains("POP_HANDLER"));
        assert!(d.contains("THROW"));
    }

    #[test]
    fn try_finally_has_two_handlers_and_rethrow() {
        let d = dis("try { throw 1; } catch (e) {} finally { print(1); }");
        // Two PUSH_HANDLER for the finally-protected try/catch.
        assert_eq!(d.matches("PUSH_HANDLER").count(), 2);
    }

    #[test]
    fn match_emits_match_ops() {
        let d = dis(r#"let r = match v { [a, b] => a, {k: x} => x, _ => 0 }; let v = 1;"#);
        assert!(d.contains("MATCH_ARRAY"));
        assert!(d.contains("MATCH_MAP_HAS"));
    }

    #[test]
    fn maps_and_arrays() {
        let d = dis("let a = [1, ..b, 3]; let b = [2];");
        assert!(d.contains("NEW_ARRAY"));
        assert!(d.contains("ARRAY_PUSH"));
        assert!(d.contains("ARRAY_EXTEND"));
    }

    #[test]
    fn import_emits_import_op() {
        let d = dis(r#"import "math" as m;"#);
        assert!(d.contains("IMPORT"));
    }

    #[test]
    fn property_get_and_set() {
        let d = dis("fn f(o) { o.x = 1; return o.y; }");
        assert!(d.contains("SET_PROP"));
        assert!(d.contains("GET_PROP"));
    }
}
