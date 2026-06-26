//! The AST printer: turn a [`Program`] back into normalized Lumen source.
//!
//! This serves two roles. As a *test oracle* it gives the round-trip property
//! `print(parse(print(parse(src)))) == print(parse(src))` — i.e. printing is
//! idempotent, which is a strong check that the parser preserves structure. As
//! the basis of `lumen fmt` (Phase 8) it produces a single canonical layout:
//! four-space indentation, symbolic logical operators, and minimal parentheses
//! reconstructed from precedence.

use crate::ast::*;
use crate::util::{escape_string, format_float, is_identifier};

/// Print a whole program to canonical source (ends with a trailing newline).
pub fn print_program(program: &Program) -> String {
    let mut p = Printer { out: String::new(), indent: 0 };
    for (i, item) in program.items.iter().enumerate() {
        if i > 0 && needs_blank_line(&program.items[i - 1], item) {
            p.out.push('\n');
        }
        p.print_stmt(item);
    }
    p.out
}

/// Print a single expression (used by the REPL / hover tooltips).
pub fn print_expr(expr: &Expr) -> String {
    let mut p = Printer { out: String::new(), indent: 0 };
    p.expr(expr, 0)
}

/// Separate top-level functions and classes with a blank line for readability.
fn needs_blank_line(prev: &Stmt, cur: &Stmt) -> bool {
    fn is_def(s: &Stmt) -> bool {
        match s {
            Stmt::Function(_) | Stmt::Class(_) => true,
            Stmt::Export { decl, .. } => is_def(decl),
            _ => false,
        }
    }
    is_def(prev) || is_def(cur)
}

struct Printer {
    out: String,
    indent: usize,
}

impl Printer {
    fn pad(&self) -> String {
        "    ".repeat(self.indent)
    }

    /// Render a sub-tree to a string by temporarily redirecting the buffer.
    fn render<F: FnOnce(&mut Self)>(&mut self, f: F) -> String {
        let saved = std::mem::take(&mut self.out);
        f(self);
        std::mem::replace(&mut self.out, saved)
    }

    // ---- statements --------------------------------------------------------

    fn print_stmt(&mut self, s: &Stmt) {
        let pad = self.pad();
        self.out.push_str(&pad);
        self.append_stmt(s);
        self.out.push('\n');
    }

    /// Append a statement's text with no leading pad and no trailing newline.
    /// Multi-line constructs (blocks) self-indent their interior lines.
    fn append_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { name, init, .. } => {
                self.out.push_str("let ");
                self.out.push_str(name);
                if let Some(e) = init {
                    let v = self.expr(e, 0);
                    self.out.push_str(" = ");
                    self.out.push_str(&v);
                }
                self.out.push(';');
            }
            Stmt::Const { name, init, .. } => {
                let v = self.expr(init, 0);
                self.out.push_str("const ");
                self.out.push_str(name);
                self.out.push_str(" = ");
                self.out.push_str(&v);
                self.out.push(';');
            }
            Stmt::Destructure { pattern, init, .. } => {
                let pat = self.pattern_str(pattern);
                let v = self.expr(init, 0);
                self.out.push_str("let ");
                self.out.push_str(&pat);
                self.out.push_str(" = ");
                self.out.push_str(&v);
                self.out.push(';');
            }
            Stmt::Function(f) => self.append_function("fn ", f),
            Stmt::Class(c) => self.append_class(c),
            Stmt::Import(im) => self.append_import(im),
            Stmt::Export { decl, .. } => {
                self.out.push_str("export ");
                self.append_stmt(decl);
            }
            Stmt::Expr { expr, .. } => {
                let e = self.expr(expr, 0);
                self.out.push_str(&e);
                self.out.push(';');
            }
            Stmt::Block(b) => self.append_block(b),
            Stmt::If { cond, then_block, else_branch, .. } => {
                let c = self.expr(cond, 0);
                self.out.push_str("if ");
                self.out.push_str(&c);
                self.out.push(' ');
                self.append_block(then_block);
                if let Some(else_) = else_branch {
                    self.out.push_str(" else ");
                    match else_.as_ref() {
                        Stmt::If { .. } => self.append_stmt(else_),
                        Stmt::Block(b) => self.append_block(b),
                        other => self.append_stmt(other),
                    }
                }
            }
            Stmt::While { cond, body, .. } => {
                let c = self.expr(cond, 0);
                self.out.push_str("while ");
                self.out.push_str(&c);
                self.out.push(' ');
                self.append_block(body);
            }
            Stmt::ForIn { var, iter, body, .. } => {
                let it = self.expr(iter, 0);
                self.out.push_str("for ");
                self.out.push_str(var);
                self.out.push_str(" in ");
                self.out.push_str(&it);
                self.out.push(' ');
                self.append_block(body);
            }
            Stmt::ForC { init, cond, step, body, .. } => {
                self.out.push_str("for ");
                match init {
                    Some(s) => self.append_stmt(s), // includes its ';'
                    None => self.out.push(';'),
                }
                if let Some(c) = cond {
                    let cs = self.expr(c, 0);
                    self.out.push(' ');
                    self.out.push_str(&cs);
                }
                self.out.push(';');
                if let Some(st) = step {
                    let ss = self.expr(st, 0);
                    self.out.push(' ');
                    self.out.push_str(&ss);
                }
                self.out.push(' ');
                self.append_block(body);
            }
            Stmt::Return { value, .. } => {
                self.out.push_str("return");
                if let Some(e) = value {
                    let v = self.expr(e, 0);
                    self.out.push(' ');
                    self.out.push_str(&v);
                }
                self.out.push(';');
            }
            Stmt::Break { .. } => self.out.push_str("break;"),
            Stmt::Continue { .. } => self.out.push_str("continue;"),
            Stmt::Throw { value, .. } => {
                let v = self.expr(value, 0);
                self.out.push_str("throw ");
                self.out.push_str(&v);
                self.out.push(';');
            }
            Stmt::Try { body, catch, finally, .. } => {
                self.out.push_str("try ");
                self.append_block(body);
                if let Some(c) = catch {
                    self.out.push_str(" catch (");
                    self.out.push_str(&c.name);
                    self.out.push_str(") ");
                    self.append_block(&c.body);
                }
                if let Some(f) = finally {
                    self.out.push_str(" finally ");
                    self.append_block(f);
                }
            }
        }
    }

    fn append_function(&mut self, keyword: &str, f: &Function) {
        self.out.push_str(keyword);
        if let Some(name) = &f.name {
            self.out.push_str(name);
        }
        self.out.push('(');
        let params = self.params_str(&f.params);
        self.out.push_str(&params);
        self.out.push_str(") ");
        self.append_block(&f.body);
    }

    /// Render a parameter list, including defaults (`x = expr`) and rest
    /// (`..name`).
    fn params_str(&mut self, params: &[Param]) -> String {
        let mut parts = Vec::with_capacity(params.len());
        for p in params {
            if p.is_rest {
                parts.push(format!("..{}", p.name));
            } else if let Some(d) = &p.default {
                let ds = self.expr(d, 0);
                parts.push(format!("{} = {}", p.name, ds));
            } else {
                parts.push(p.name.clone());
            }
        }
        parts.join(", ")
    }

    fn append_class(&mut self, c: &ClassDecl) {
        self.out.push_str("class ");
        self.out.push_str(&c.name);
        if let Some(sc) = &c.superclass {
            self.out.push_str(" < ");
            self.out.push_str(&sc.value);
        }
        self.out.push(' ');
        if c.methods.is_empty() {
            self.out.push_str("{}");
            return;
        }
        self.out.push_str("{\n");
        self.indent += 1;
        for (i, m) in c.methods.iter().enumerate() {
            if i > 0 {
                self.out.push('\n');
            }
            let pad = self.pad();
            self.out.push_str(&pad);
            self.append_function("", m); // methods have no `fn` keyword
            self.out.push('\n');
        }
        self.indent -= 1;
        let pad = self.pad();
        self.out.push_str(&pad);
        self.out.push('}');
    }

    fn append_import(&mut self, im: &Import) {
        self.out.push_str("import \"");
        self.out.push_str(&escape_string(&im.path));
        self.out.push('"');
        match &im.kind {
            ImportKind::Module { alias } => {
                // Re-emit `as alias` unless the alias is just the basename.
                let base = im
                    .path
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&im.path)
                    .strip_suffix(".lum")
                    .unwrap_or_else(|| im.path.rsplit(['/', '\\']).next().unwrap_or(&im.path));
                if alias.value != base {
                    self.out.push_str(" as ");
                    self.out.push_str(&alias.value);
                }
            }
            ImportKind::Named(names) => {
                self.out.push_str(".{");
                let joined: Vec<&str> = names.iter().map(|n| n.value.as_str()).collect();
                self.out.push_str(&joined.join(", "));
                self.out.push('}');
            }
        }
        self.out.push(';');
    }

    fn append_block(&mut self, b: &Block) {
        if b.stmts.is_empty() {
            self.out.push_str("{}");
            return;
        }
        self.out.push_str("{\n");
        self.indent += 1;
        for s in &b.stmts {
            self.print_stmt(s);
        }
        self.indent -= 1;
        let pad = self.pad();
        self.out.push_str(&pad);
        self.out.push('}');
    }

    // ---- expressions -------------------------------------------------------

    /// Render `e` for a context that requires precedence at least `ctx`,
    /// adding parentheses when `e`'s own precedence is lower.
    fn expr(&mut self, e: &Expr, ctx: u8) -> String {
        let p = expr_prec(e);
        let s = self.expr_inner(e);
        if p < ctx {
            format!("({s})")
        } else {
            s
        }
    }

    fn expr_inner(&mut self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Int(n) => n.to_string(),
            ExprKind::Float(f) => format_float(*f),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Nil => "nil".to_string(),
            ExprKind::Str(s) => format!("\"{}\"", escape_string(s)),
            ExprKind::StrInterp(segments) => {
                let mut out = String::from("\"");
                for seg in segments {
                    match seg {
                        StrSegment::Text(t) => out.push_str(&escape_string(t)),
                        StrSegment::Expr(e) => {
                            let inner = self.expr(e, 0);
                            out.push_str("${");
                            out.push_str(&inner);
                            out.push('}');
                        }
                    }
                }
                out.push('"');
                out
            }
            ExprKind::Var(name) => name.clone(),
            ExprKind::This => "this".to_string(),
            ExprKind::Super { method, .. } => format!("super.{method}"),
            ExprKind::Array(elems) => {
                let parts: Vec<String> = elems
                    .iter()
                    .map(|el| match el {
                        ArrayElem::Item(e) => self.expr(e, 0),
                        ArrayElem::Spread(e) => format!("..{}", self.expr(e, 0)),
                    })
                    .collect();
                format!("[{}]", parts.join(", "))
            }
            ExprKind::Map(entries) => {
                if entries.is_empty() {
                    return "{}".to_string();
                }
                let parts: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| {
                        let key = match k {
                            MapKey::Str(s) if is_identifier(s) => s.clone(),
                            MapKey::Str(s) => format!("\"{}\"", escape_string(s)),
                            MapKey::Computed(e) => format!("[{}]", self.expr(e, 0)),
                        };
                        format!("{}: {}", key, self.expr(v, 0))
                    })
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            ExprKind::Assign { target, value } => {
                format!("{} = {}", self.expr(target, 9), self.expr(value, 1))
            }
            ExprKind::Unary { op, operand } => {
                let sym = match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Not => "!",
                };
                format!("{}{}", sym, self.expr(operand, 8))
            }
            ExprKind::Binary { op, left, right } => {
                let p = expr_prec(e);
                format!(
                    "{} {} {}",
                    self.expr(left, p),
                    binary_sym(*op),
                    self.expr(right, p + 1)
                )
            }
            ExprKind::Logical { op, left, right } => {
                let p = expr_prec(e);
                let sym = match op {
                    LogicalOp::And => "&&",
                    LogicalOp::Or => "||",
                };
                format!("{} {} {}", self.expr(left, p), sym, self.expr(right, p + 1))
            }
            ExprKind::Call { callee, args, .. } => {
                let a: Vec<String> = args.iter().map(|x| self.expr(x, 0)).collect();
                format!("{}({})", self.expr(callee, 9), a.join(", "))
            }
            ExprKind::Index { object, index } => {
                format!("{}[{}]", self.expr(object, 9), self.expr(index, 0))
            }
            ExprKind::Get { object, name, .. } => {
                format!("{}.{}", self.expr(object, 9), name)
            }
            ExprKind::Lambda(f) => {
                let params = self.params_str(&f.params);
                let body = self.render(|p| p.append_block(&f.body));
                format!("fn({params}) {body}")
            }
            ExprKind::Match { subject, arms } => {
                let subj = self.expr(subject, 0);
                let arms_str = self.render(|p| {
                    p.out.push_str("{\n");
                    p.indent += 1;
                    for arm in arms {
                        let pad = p.pad();
                        p.out.push_str(&pad);
                        let pat = p.pattern_str(&arm.pattern);
                        p.out.push_str(&pat);
                        if let Some(g) = &arm.guard {
                            let gs = p.expr(g, 0);
                            p.out.push_str(" if ");
                            p.out.push_str(&gs);
                        }
                        let body = p.expr(&arm.body, 0);
                        p.out.push_str(" => ");
                        p.out.push_str(&body);
                        p.out.push_str(",\n");
                    }
                    p.indent -= 1;
                    let pad = p.pad();
                    p.out.push_str(&pad);
                    p.out.push('}');
                });
                format!("match {subj} {arms_str}")
            }
        }
    }

    fn pattern_str(&mut self, pat: &Pattern) -> String {
        match &pat.kind {
            PatternKind::Wildcard => "_".to_string(),
            PatternKind::Int(n) => n.to_string(),
            PatternKind::Float(f) => format_float(*f),
            PatternKind::Str(s) => format!("\"{}\"", escape_string(s)),
            PatternKind::Bool(b) => b.to_string(),
            PatternKind::Nil => "nil".to_string(),
            PatternKind::Binding(name) => name.clone(),
            PatternKind::Array(elems) => {
                let parts: Vec<String> = elems
                    .iter()
                    .map(|el| match el {
                        PatElem::Pattern(p) => self.pattern_str(p),
                        PatElem::Rest(None) => "..".to_string(),
                        PatElem::Rest(Some(n)) => format!("..{n}"),
                    })
                    .collect();
                format!("[{}]", parts.join(", "))
            }
            PatternKind::Map(entries) => {
                let parts: Vec<String> = entries
                    .iter()
                    .map(|(k, p)| {
                        let key = if is_identifier(k) {
                            k.clone()
                        } else {
                            format!("\"{}\"", escape_string(k))
                        };
                        format!("{}: {}", key, self.pattern_str(p))
                    })
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
        }
    }
}

fn binary_sym(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
    }
}

/// Precedence of an expression, matching the parser's levels (higher binds
/// tighter). Atoms are 10; assignment is 1.
fn expr_prec(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Assign { .. } => 1,
        ExprKind::Logical { op: LogicalOp::Or, .. } => 2,
        ExprKind::Logical { op: LogicalOp::And, .. } => 3,
        ExprKind::Binary { op, .. } => match op {
            BinaryOp::Eq | BinaryOp::Ne => 4,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => 5,
            BinaryOp::Add | BinaryOp::Sub => 6,
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => 7,
        },
        ExprKind::Unary { .. } => 8,
        ExprKind::Call { .. } | ExprKind::Index { .. } | ExprKind::Get { .. } => 9,
        _ => 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn print_src(src: &str) -> String {
        let (tokens, lex_errs) = crate::lexer::lex(src);
        assert!(lex_errs.is_empty(), "lex: {lex_errs:?}");
        let (program, errs) = parse(tokens);
        assert!(errs.is_empty(), "parse: {errs:?}");
        print_program(&program)
    }

    /// The key property: printing is idempotent on a re-parse.
    fn assert_roundtrip(src: &str) {
        let once = print_src(src);
        let twice = print_src(&once);
        assert_eq!(once, twice, "\n--- once ---\n{once}\n--- twice ---\n{twice}");
    }

    #[test]
    fn roundtrip_examples() {
        assert_roundtrip("let x=1+2*3;");
        assert_roundtrip("let x = (1 + 2) * 3;");
        assert_roundtrip("fn f(a,b){return a+b;}");
        assert_roundtrip("if a{b();}else if c{d();}else{e();}");
        assert_roundtrip("while x>0{x=x-1;}");
        assert_roundtrip("for let i=0;i<10;i=i+1{print(i);}");
        assert_roundtrip("for x in xs{use(x);}");
        assert_roundtrip("class Dog<Animal{init(n){this.name=n;}speak(){return super.speak();}}");
        assert_roundtrip(r#"let m={x:1,"y-z":2,[k]:3};"#);
        assert_roundtrip("let a=[1,2,..rest,3];");
        assert_roundtrip(r#"let s="hi ${name}, ${1+2}!";"#);
        assert_roundtrip(r#"let r=match v{0=>"z",[a,..rest]=>"arr",{k:x}=>"map",n if n>0=>"pos",_=>"other"};"#);
        assert_roundtrip("try{throw \"x\";}catch(e){print(e);}finally{done();}");
        assert_roundtrip("import \"math\" as m; import \"x\".{a,b}; export fn p(){return 1;}");
        assert_roundtrip("let f = fn(x){return x*2;};");
    }

    #[test]
    fn precedence_parens_minimal() {
        // 1 + 2 * 3 needs no parens; (1 + 2) * 3 does.
        assert_eq!(print_src("let x = 1 + 2 * 3;"), "let x = 1 + 2 * 3;\n");
        assert_eq!(print_src("let x = (1 + 2) * 3;"), "let x = (1 + 2) * 3;\n");
    }

    #[test]
    fn left_assoc_subtraction_keeps_shape() {
        // a - b - c is (a - b) - c; printing must not add parens but must keep
        // a - (b - c) parenthesized.
        assert_eq!(print_src("let x = a - b - c;"), "let x = a - b - c;\n");
        assert_eq!(print_src("let x = a - (b - c);"), "let x = a - (b - c);\n");
    }

    #[test]
    fn keyword_operators_normalize_to_symbols() {
        assert_eq!(print_src("let x = a and b or c;"), "let x = a && b || c;\n");
        assert_eq!(print_src("let x = not a;"), "let x = !a;\n");
    }

    #[test]
    fn float_prints_with_decimal() {
        assert_eq!(print_src("let x = 5.0;"), "let x = 5.0;\n");
    }
}
