//! Static semantic analysis (the resolver).
//!
//! The resolver walks the AST once and reports every *static* error it can,
//! before any bytecode is produced:
//!
//! - undefined variable reads and writes (a name that is neither a local, an
//!   enclosing local captured as an upvalue, a known global, nor a built-in);
//! - reading a local variable inside its own initializer (`let a = a;`);
//! - re-declaring a name already bound in the same scope (including duplicate
//!   function parameters and duplicate pattern bindings);
//! - assigning to a `const`;
//! - `this` outside a method, `super` outside a subclass method;
//! - `break`/`continue` outside a loop, `return` outside a function, and
//!   returning a value from an `init` method;
//! - a class inheriting from itself, and `export` outside the top level.
//!
//! It deliberately does **not** assign stack slots or build upvalue tables —
//! that is the compiler's job (Phase 4), which re-derives the same lexical facts
//! while emitting code. The resolver is the validator; the compiler is the
//! authority on layout. Both obey the scope rules in SPEC §5, so they agree on
//! every name's classification.

use crate::ast::*;
use crate::builtins::is_builtin;
use crate::diagnostics::Diagnostic;
use crate::span::Span;
use std::collections::HashMap;

/// Resolve a program, returning all static errors (empty == valid).
pub fn resolve(program: &Program) -> Vec<Diagnostic> {
    resolve_with(program, &[])
}

/// Like [`resolve`], but treating `predefined` names as already-defined globals
/// (used by the REPL, where each input sees the globals of prior inputs).
pub fn resolve_with(program: &Program, predefined: &[String]) -> Vec<Diagnostic> {
    let mut r = Resolver::new();
    for name in predefined {
        r.predefined.insert(name.clone());
    }
    r.collect_globals(program);
    r.funcs.push(FuncCtx::new(FuncKind::Script, false, false));
    for item in &program.items {
        r.resolve_stmt(item);
    }
    r.funcs.pop();
    r.errors
}

#[derive(Clone, Copy, PartialEq)]
enum FuncKind {
    Script,
    Function,
    Method,
    Initializer,
}

struct Local {
    name: String,
    depth: usize,
    initialized: bool,
    is_const: bool,
}

struct FuncCtx {
    kind: FuncKind,
    locals: Vec<Local>,
    scope_depth: usize,
    loop_depth: usize,
    allows_this: bool,
    allows_super: bool,
}

impl FuncCtx {
    fn new(kind: FuncKind, allows_this: bool, allows_super: bool) -> Self {
        FuncCtx {
            kind,
            locals: Vec::new(),
            scope_depth: 0,
            loop_depth: 0,
            allows_this,
            allows_super,
        }
    }
}

/// How an assignment target name resolves, for the const/undefined checks.
enum Assignability {
    Mutable,
    Constant,
    Undefined,
}

struct Resolver {
    errors: Vec<Diagnostic>,
    /// Top-level names -> is_const. Collected up front so forward references and
    /// mutual recursion between globals are legal.
    globals: HashMap<String, bool>,
    /// Names defined by earlier REPL inputs; treated as known mutable globals.
    predefined: std::collections::HashSet<String>,
    funcs: Vec<FuncCtx>,
}

impl Resolver {
    fn new() -> Self {
        Resolver {
            errors: Vec::new(),
            globals: HashMap::new(),
            predefined: std::collections::HashSet::new(),
            funcs: Vec::new(),
        }
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.errors.push(Diagnostic::error("resolver", msg, span));
    }

    // ---- global collection -------------------------------------------------

    fn collect_globals(&mut self, program: &Program) {
        for item in &program.items {
            self.collect_global_decl(item);
        }
    }

    fn collect_global_decl(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, name_span, .. } => self.add_global(name, *name_span, false),
            Stmt::Const { name, name_span, .. } => self.add_global(name, *name_span, true),
            Stmt::Destructure { pattern, .. } => self.collect_pattern_globals(pattern),
            Stmt::Function(f) => self.add_global(f.name.as_deref().unwrap_or(""), f.name_span, false),
            Stmt::Class(c) => self.add_global(&c.name, c.name_span, false),
            Stmt::Import(im) => match &im.kind {
                ImportKind::Module { alias } => self.add_global(&alias.value, alias.span, false),
                ImportKind::Named(names) => {
                    for n in names {
                        self.add_global(&n.value, n.span, false);
                    }
                }
            },
            Stmt::Export { decl, .. } => self.collect_global_decl(decl),
            _ => {}
        }
    }

    fn add_global(&mut self, name: &str, span: Span, is_const: bool) {
        if self.globals.contains_key(name) {
            self.error(span, format!("'{name}' is already declared at the top level"));
        } else {
            self.globals.insert(name.to_string(), is_const);
        }
    }

    // ---- scope machinery ---------------------------------------------------

    fn current(&mut self) -> &mut FuncCtx {
        self.funcs.last_mut().unwrap()
    }

    fn current_ref(&self) -> &FuncCtx {
        self.funcs.last().unwrap()
    }

    fn is_global_scope(&self) -> bool {
        let f = self.current_ref();
        f.kind == FuncKind::Script && f.scope_depth == 0
    }

    fn begin_scope(&mut self) {
        self.current().scope_depth += 1;
    }

    fn end_scope(&mut self) {
        let depth = self.current().scope_depth;
        self.current().locals.retain(|l| l.depth < depth);
        self.current().scope_depth -= 1;
    }

    fn declare_local(&mut self, name: &str, span: Span, is_const: bool) {
        let depth = self.current_ref().scope_depth;
        let dup = self
            .current_ref()
            .locals
            .iter()
            .rev()
            .take_while(|l| l.depth == depth)
            .any(|l| l.name == name);
        if dup {
            self.error(span, format!("'{name}' is already declared in this scope"));
        }
        self.current().locals.push(Local {
            name: name.to_string(),
            depth,
            initialized: false,
            is_const,
        });
    }

    fn define_last(&mut self) {
        if let Some(l) = self.current().locals.last_mut() {
            l.initialized = true;
        }
    }

    /// Declare and immediately initialize a binding (params, loop vars, catch
    /// vars, pattern bindings, recursive function names).
    fn declare_defined(&mut self, name: &str, span: Span, is_const: bool) {
        self.declare_local(name, span, is_const);
        self.define_last();
    }

    /// Bind a declaration name: as a global at top level, else as a local.
    /// `recursive` marks it initialized immediately (so a local `fn` can recurse).
    fn bind_decl(&mut self, name: &str, span: Span, is_const: bool, recursive: bool) {
        if self.is_global_scope() {
            return; // already in the globals table from collect_globals
        }
        self.declare_local(name, span, is_const);
        if recursive {
            self.define_last();
        }
    }

    // ---- name lookup -------------------------------------------------------

    /// `(initialized, is_const)` for the nearest local of this name in the
    /// current function, if any.
    fn find_local_current(&self, name: &str) -> Option<(bool, bool)> {
        self.current_ref()
            .locals
            .iter()
            .rev()
            .find(|l| l.name == name)
            .map(|l| (l.initialized, l.is_const))
    }

    /// `is_const` if the name is a local of some *enclosing* function (i.e. an
    /// upvalue capture).
    fn find_upvalue(&self, name: &str) -> Option<bool> {
        let n = self.funcs.len();
        if n < 2 {
            return None;
        }
        for f in self.funcs[..n - 1].iter().rev() {
            if let Some(l) = f.locals.iter().rev().find(|l| l.name == name) {
                return Some(l.is_const);
            }
        }
        None
    }

    /// Resolve a variable *read*, emitting an error if it is undefined or read
    /// in its own initializer.
    fn resolve_read(&mut self, name: &str, span: Span) {
        if let Some((initialized, _)) = self.find_local_current(name) {
            if !initialized {
                self.error(span, format!("cannot read '{name}' in its own initializer"));
            }
            return;
        }
        if self.find_upvalue(name).is_some() {
            return;
        }
        if self.globals.contains_key(name) || self.predefined.contains(name) || is_builtin(name) {
            return;
        }
        self.error(span, format!("undefined variable '{name}'"));
    }

    fn assignability(&self, name: &str) -> Assignability {
        if let Some((_, is_const)) = self.find_local_current(name) {
            return if is_const { Assignability::Constant } else { Assignability::Mutable };
        }
        if let Some(is_const) = self.find_upvalue(name) {
            return if is_const { Assignability::Constant } else { Assignability::Mutable };
        }
        if let Some(&is_const) = self.globals.get(name) {
            return if is_const { Assignability::Constant } else { Assignability::Mutable };
        }
        if self.predefined.contains(name) || is_builtin(name) {
            return Assignability::Mutable;
        }
        Assignability::Undefined
    }

    // ---- statements --------------------------------------------------------

    fn resolve_block(&mut self, block: &Block) {
        self.begin_scope();
        for stmt in &block.stmts {
            self.resolve_stmt(stmt);
        }
        self.end_scope();
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, name_span, init, .. } => {
                if self.is_global_scope() {
                    if let Some(e) = init {
                        self.resolve_expr(e);
                    }
                } else {
                    self.declare_local(name, *name_span, false);
                    if let Some(e) = init {
                        self.resolve_expr(e);
                    }
                    self.define_last();
                }
            }
            Stmt::Const { name, name_span, init, .. } => {
                if self.is_global_scope() {
                    self.resolve_expr(init);
                } else {
                    self.declare_local(name, *name_span, true);
                    self.resolve_expr(init);
                    self.define_last();
                }
            }
            Stmt::Destructure { pattern, init, .. } => {
                self.validate_destructure(pattern);
                self.resolve_expr(init);
                if !self.is_global_scope() {
                    self.declare_pattern(pattern);
                }
            }
            Stmt::Function(f) => {
                let name = f.name.as_deref().unwrap_or("");
                self.bind_decl(name, f.name_span, false, true);
                self.resolve_function(f, FuncKind::Function, false, false);
            }
            Stmt::Class(c) => self.resolve_class(c),
            Stmt::Import(im) => {
                if !self.is_global_scope() {
                    match &im.kind {
                        ImportKind::Module { alias } => {
                            self.declare_defined(&alias.value, alias.span, false)
                        }
                        ImportKind::Named(names) => {
                            for n in names {
                                self.declare_defined(&n.value, n.span, false);
                            }
                        }
                    }
                }
            }
            Stmt::Export { decl, span } => {
                if !self.is_global_scope() {
                    self.error(*span, "'export' is only allowed at the top level");
                }
                self.resolve_stmt(decl);
            }
            Stmt::Expr { expr, .. } => self.resolve_expr(expr),
            Stmt::Block(b) => self.resolve_block(b),
            Stmt::If { cond, then_block, else_branch, .. } => {
                self.resolve_expr(cond);
                self.resolve_block(then_block);
                if let Some(else_) = else_branch {
                    self.resolve_stmt(else_);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.resolve_expr(cond);
                self.current().loop_depth += 1;
                self.resolve_block(body);
                self.current().loop_depth -= 1;
            }
            Stmt::ForIn { var, var_span, iter, body, .. } => {
                self.resolve_expr(iter);
                self.begin_scope();
                self.declare_defined(var, *var_span, false);
                self.current().loop_depth += 1;
                for s in &body.stmts {
                    self.resolve_stmt(s);
                }
                self.current().loop_depth -= 1;
                self.end_scope();
            }
            Stmt::ForC { init, cond, step, body, .. } => {
                self.begin_scope();
                if let Some(i) = init {
                    self.resolve_stmt(i);
                }
                if let Some(c) = cond {
                    self.resolve_expr(c);
                }
                if let Some(s) = step {
                    self.resolve_expr(s);
                }
                self.current().loop_depth += 1;
                for s in &body.stmts {
                    self.resolve_stmt(s);
                }
                self.current().loop_depth -= 1;
                self.end_scope();
            }
            Stmt::Return { value, span } => {
                match self.current_ref().kind {
                    FuncKind::Script => {
                        self.error(*span, "'return' outside of a function");
                    }
                    FuncKind::Initializer if value.is_some() => {
                        self.error(*span, "cannot return a value from an 'init' method");
                    }
                    _ => {}
                }
                if let Some(e) = value {
                    self.resolve_expr(e);
                }
            }
            Stmt::Break { span } => {
                if self.current_ref().loop_depth == 0 {
                    self.error(*span, "'break' outside of a loop");
                }
            }
            Stmt::Continue { span } => {
                if self.current_ref().loop_depth == 0 {
                    self.error(*span, "'continue' outside of a loop");
                }
            }
            Stmt::Throw { value, .. } => self.resolve_expr(value),
            Stmt::Try { body, catch, finally, .. } => {
                self.resolve_block(body);
                if let Some(c) = catch {
                    self.begin_scope();
                    self.declare_defined(&c.name, c.name_span, false);
                    for s in &c.body.stmts {
                        self.resolve_stmt(s);
                    }
                    self.end_scope();
                }
                if let Some(f) = finally {
                    self.resolve_block(f);
                }
            }
        }
    }

    fn resolve_class(&mut self, c: &ClassDecl) {
        self.bind_decl(&c.name, c.name_span, false, true);
        let has_super = c.superclass.is_some();
        if let Some(sc) = &c.superclass {
            if sc.value == c.name {
                self.error(sc.span, format!("class '{}' cannot inherit from itself", c.name));
            } else {
                self.resolve_read(&sc.value, sc.span);
            }
        }
        for m in &c.methods {
            let kind = if m.name.as_deref() == Some("init") {
                FuncKind::Initializer
            } else {
                FuncKind::Method
            };
            self.resolve_function(m, kind, true, has_super);
        }
    }

    fn resolve_function(
        &mut self,
        f: &Function,
        kind: FuncKind,
        allows_this: bool,
        allows_super: bool,
    ) {
        self.funcs.push(FuncCtx::new(kind, allows_this, allows_super));
        self.begin_scope();
        let mut seen_default = false;
        for p in &f.params {
            if p.is_rest {
                self.declare_defined(&p.name, p.span, false);
                continue;
            }
            if let Some(d) = &p.default {
                seen_default = true;
                // The default is evaluated at call time in the function scope, so
                // it may reference earlier params (already declared) but not this
                // one (declared just after).
                self.resolve_expr(d);
            } else if seen_default {
                self.error(
                    p.span,
                    format!("required parameter '{}' cannot follow a parameter with a default", p.name),
                );
            }
            self.declare_defined(&p.name, p.span, false);
        }
        for s in &f.body.stmts {
            self.resolve_stmt(s);
        }
        self.end_scope();
        self.funcs.pop();
    }

    // ---- expressions -------------------------------------------------------

    fn resolve_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Nil => {}
            ExprKind::StrInterp(segments) => {
                for seg in segments {
                    if let StrSegment::Expr(e) = seg {
                        self.resolve_expr(e);
                    }
                }
            }
            ExprKind::Array(elems) => {
                for el in elems {
                    match el {
                        ArrayElem::Item(e) | ArrayElem::Spread(e) => self.resolve_expr(e),
                    }
                }
            }
            ExprKind::Map(entries) => {
                for (k, v) in entries {
                    if let MapKey::Computed(e) = k {
                        self.resolve_expr(e);
                    }
                    self.resolve_expr(v);
                }
            }
            ExprKind::Var(name) => self.resolve_read(name, expr.span),
            ExprKind::This => {
                if !self.current_ref().allows_this {
                    self.error(expr.span, "'this' can only be used inside a method");
                }
            }
            ExprKind::Super { method, .. } => {
                if !self.current_ref().allows_super {
                    self.error(
                        expr.span,
                        "'super' can only be used inside a method of a class with a superclass",
                    );
                }
                let _ = method;
            }
            ExprKind::Assign { target, value } => {
                self.resolve_expr(value);
                match &target.kind {
                    ExprKind::Var(name) => match self.assignability(name) {
                        Assignability::Constant => {
                            self.error(target.span, format!("cannot assign to constant '{name}'"));
                        }
                        Assignability::Undefined => {
                            self.error(
                                target.span,
                                format!("assignment to undefined variable '{name}'"),
                            );
                        }
                        Assignability::Mutable => {}
                    },
                    _ => self.resolve_expr(target),
                }
            }
            ExprKind::Unary { operand, .. } => self.resolve_expr(operand),
            ExprKind::Binary { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            ExprKind::Logical { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            ExprKind::Call { callee, args, .. } => {
                self.resolve_expr(callee);
                for a in args {
                    self.resolve_expr(a);
                }
            }
            ExprKind::Index { object, index } => {
                self.resolve_expr(object);
                self.resolve_expr(index);
            }
            ExprKind::Get { object, .. } => self.resolve_expr(object),
            ExprKind::Lambda(f) => {
                let this = self.current_ref().allows_this;
                let supr = self.current_ref().allows_super;
                self.resolve_function(f, FuncKind::Function, this, supr);
            }
            ExprKind::Match { subject, arms } => {
                self.resolve_expr(subject);
                for arm in arms {
                    self.begin_scope();
                    self.declare_pattern(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.resolve_expr(g);
                    }
                    self.resolve_expr(&arm.body);
                    self.end_scope();
                }
            }
        }
    }

    /// Add a destructuring pattern's binding names as globals (top-level use).
    fn collect_pattern_globals(&mut self, pat: &Pattern) {
        match &pat.kind {
            PatternKind::Binding(name) => self.add_global(name, pat.span, false),
            PatternKind::Array(elems) => {
                for el in elems {
                    match el {
                        PatElem::Pattern(p) => self.collect_pattern_globals(p),
                        PatElem::Rest(Some(name)) => self.add_global(name, pat.span, false),
                        PatElem::Rest(None) => {}
                    }
                }
            }
            PatternKind::Map(entries) => {
                for (_, p) in entries {
                    self.collect_pattern_globals(p);
                }
            }
            _ => {}
        }
    }

    /// A destructuring `let` pattern must be a flat array or map of variable
    /// names / wildcards / rest (nested patterns and literals are match-only).
    fn validate_destructure(&mut self, pat: &Pattern) {
        match &pat.kind {
            PatternKind::Array(elems) => {
                for el in elems {
                    if let PatElem::Pattern(p) = el {
                        if !matches!(p.kind, PatternKind::Binding(_) | PatternKind::Wildcard) {
                            self.error(
                                p.span,
                                "destructuring elements must be variable names, '_', or '..rest'",
                            );
                        }
                    }
                }
            }
            PatternKind::Map(entries) => {
                for (_, p) in entries {
                    if !matches!(p.kind, PatternKind::Binding(_) | PatternKind::Wildcard) {
                        self.error(p.span, "destructuring map values must be variable names or '_'");
                    }
                }
            }
            _ => self.error(
                pat.span,
                "destructuring requires an array '[...]' or map '{...}' pattern",
            ),
        }
    }

    /// Declare every binding a pattern introduces. Duplicate bindings in one
    /// pattern are caught by `declare_local`'s same-scope duplicate check.
    fn declare_pattern(&mut self, pat: &Pattern) {
        match &pat.kind {
            PatternKind::Binding(name) => self.declare_defined(name, pat.span, false),
            PatternKind::Array(elems) => {
                for el in elems {
                    match el {
                        PatElem::Pattern(p) => self.declare_pattern(p),
                        PatElem::Rest(Some(name)) => self.declare_defined(name, pat.span, false),
                        PatElem::Rest(None) => {}
                    }
                }
            }
            PatternKind::Map(entries) => {
                for (_, p) in entries {
                    self.declare_pattern(p);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn errs(src: &str) -> Vec<Diagnostic> {
        let (tokens, lex_errs) = crate::lexer::lex(src);
        assert!(lex_errs.is_empty(), "lex: {lex_errs:?}");
        let (program, perrs) = parse(tokens);
        assert!(perrs.is_empty(), "parse: {perrs:?}");
        resolve(&program)
    }

    fn ok(src: &str) {
        let e = errs(src);
        assert!(e.is_empty(), "unexpected resolver errors: {e:?}");
    }

    fn has_error(src: &str, needle: &str) {
        let e = errs(src);
        assert!(
            e.iter().any(|d| d.message.contains(needle)),
            "expected an error containing '{needle}', got {e:?}"
        );
    }

    #[test]
    fn valid_programs() {
        ok("let x = 1; print(x);");
        ok("fn a() { return b(); } fn b() { return 1; }"); // mutual recursion (globals)
        ok("fn outer() { let n = 0; return fn() { n = n + 1; return n; }; }"); // closure
        ok("let f = fn() { return f; };"); // recursive closure via upvalue
        ok("for let i = 0; i < 10; i = i + 1 { print(i); }");
        ok("for x in [1,2,3] { print(x); }");
        ok("class A { m() { return this.x; } } class B < A { m() { return super.m(); } }");
        ok(r#"let r = match v { [a, b] => a + b, {k: x} => x, n if n > 0 => n, _ => 0 }; let v = 1;"#);
        ok("try { throw 1; } catch (e) { print(e); }");
    }

    #[test]
    fn undefined_variable() {
        has_error("print(nope);", "undefined variable 'nope'");
        has_error("fn f() { return missing; }", "undefined variable 'missing'");
    }

    #[test]
    fn use_before_declaration_is_undefined() {
        has_error("fn f() { print(y); let y = 1; }", "undefined variable 'y'");
    }

    #[test]
    fn read_in_own_initializer() {
        has_error("fn f() { let a = a + 1; }", "in its own initializer");
    }

    #[test]
    fn duplicate_in_same_scope() {
        has_error("fn f() { let x = 1; let x = 2; }", "already declared in this scope");
        has_error("fn f(a, a) { return a; }", "already declared in this scope");
        has_error("let r = match v { [a, a] => a, _ => 0 }; let v = 1;", "already declared");
    }

    #[test]
    fn duplicate_global() {
        has_error("let x = 1; let x = 2;", "already declared at the top level");
    }

    #[test]
    fn const_reassignment() {
        has_error("const PI = 3; PI = 4;", "cannot assign to constant 'PI'");
        has_error("fn f() { const c = 1; c = 2; }", "cannot assign to constant 'c'");
    }

    #[test]
    fn assignment_to_undefined() {
        has_error("nope = 1;", "assignment to undefined variable 'nope'");
    }

    #[test]
    fn this_outside_method() {
        has_error("fn f() { return this; }", "'this' can only be used inside a method");
        has_error("print(this);", "'this'");
    }

    #[test]
    fn super_rules() {
        has_error("class A { m() { return super.m(); } }", "'super' can only be used");
        ok("class A { m() { return 1; } } class B < A { m() { return super.m(); } }");
    }

    #[test]
    fn break_continue_context() {
        has_error("break;", "'break' outside of a loop");
        has_error("fn f() { continue; }", "'continue' outside of a loop");
        // A function inside a loop does not inherit the loop context.
        has_error("while true { fn f() { break; } }", "'break' outside of a loop");
    }

    #[test]
    fn return_context() {
        has_error("return 1;", "'return' outside of a function");
        has_error("class A { init() { return 5; } }", "cannot return a value from an 'init'");
        ok("class A { init() { return; } }"); // bare return in init is fine
    }

    #[test]
    fn self_inheritance() {
        has_error("class A < A { }", "cannot inherit from itself");
    }

    #[test]
    fn export_only_top_level() {
        has_error("fn f() { export let x = 1; }", "'export' is only allowed at the top level");
        ok("export fn f() { return 1; }");
    }

    #[test]
    fn lambda_inherits_this() {
        ok("class A { m() { let f = fn() { return this.x; }; return f(); } }");
    }
}
