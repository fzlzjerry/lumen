//! The hand-written parser: recursive descent for statements, precedence-
//! climbing (Pratt-style) for expressions.
//!
//! The parser turns a `Vec<Token>` into a [`Program`], collecting every error it
//! can. On a syntax error it records a diagnostic and **resynchronizes** at the
//! next statement boundary (a `;` or a statement-starting keyword), so one run
//! reports many errors rather than dying on the first. Interpolated strings are
//! handled by sub-parsing each interpolation's pre-tokenized expression.

use crate::ast::*;
use crate::diagnostics::Diagnostic;
use crate::span::Span;
use crate::token::{StrPart, Token, TokenKind};
use TokenKind as K;

/// Marker for "an error was already recorded; unwind to a recovery point".
struct ParseError;
type PResult<T> = Result<T, ParseError>;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    errors: Vec<Diagnostic>,
}

/// The already-parsed head of a comprehension, before its `for` clause.
enum CompHead {
    Array(Expr),
    Map(Expr, Expr),
}

/// Parse `tokens` into a program plus any syntax errors.
pub fn parse(tokens: Vec<Token>) -> (Program, Vec<Diagnostic>) {
    Parser::new(tokens).parse_program()
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0, errors: Vec::new() }
    }

    pub fn parse_program(mut self) -> (Program, Vec<Diagnostic>) {
        let mut items = Vec::new();
        while !self.is_at_end() {
            let start = self.pos;
            match self.declaration() {
                Ok(stmt) => items.push(stmt),
                Err(_) => self.synchronize(),
            }
            // Guarantee forward progress even if recovery consumed nothing.
            if self.pos == start && !self.is_at_end() {
                self.advance();
            }
        }
        (Program { items }, self.errors)
    }

    // ---- token cursor ------------------------------------------------------

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek2(&self) -> &Token {
        self.tokens
            .get(self.pos + 1)
            .unwrap_or_else(|| self.tokens.last().unwrap())
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.pos.saturating_sub(1)]
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek().kind, K::Eof)
    }

    fn advance(&mut self) -> Token {
        if !self.is_at_end() {
            self.pos += 1;
        }
        self.previous().clone()
    }

    /// Discriminant-based comparison (ignores any payload).
    fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(&self.peek().kind) == std::mem::discriminant(kind)
    }

    fn match_kind(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Match a contextual keyword spelled as an identifier (e.g. `as`).
    fn match_contextual(&mut self, word: &str) -> bool {
        if let K::Ident(s) = &self.peek().kind {
            if s == word {
                self.advance();
                return true;
            }
        }
        false
    }

    fn consume(&mut self, kind: &TokenKind, msg: &str) -> PResult<Token> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            let span = self.peek().span;
            let found = self.peek().kind.describe();
            self.error_at(span, format!("{msg}, found {found}"));
            Err(ParseError)
        }
    }

    fn consume_ident(&mut self, msg: &str) -> PResult<(String, Span)> {
        if let K::Ident(name) = &self.peek().kind {
            let name = name.clone();
            let span = self.peek().span;
            self.advance();
            Ok((name, span))
        } else {
            let span = self.peek().span;
            let found = self.peek().kind.describe();
            self.error_at(span, format!("{msg}, found {found}"));
            Err(ParseError)
        }
    }

    fn error_at(&mut self, span: Span, msg: impl Into<String>) {
        self.errors.push(Diagnostic::error("parser", msg, span));
    }

    /// Skip tokens until the start of the next statement looks plausible.
    fn synchronize(&mut self) {
        while !self.is_at_end() {
            if matches!(self.previous().kind, K::Semicolon) {
                return;
            }
            match self.peek().kind {
                K::Let
                | K::Const
                | K::Fn
                | K::Class
                | K::If
                | K::While
                | K::For
                | K::Return
                | K::Break
                | K::Continue
                | K::Try
                | K::Throw
                | K::Import
                | K::Export
                | K::RBrace => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    // ---- declarations ------------------------------------------------------

    fn declaration(&mut self) -> PResult<Stmt> {
        match self.peek().kind {
            K::Let => self.let_decl(),
            K::Const => self.const_decl(),
            K::Fn => self.fn_decl(),
            K::Class => self.class_decl(),
            K::Import => self.import_decl(),
            K::Export => self.export_decl(),
            _ => self.statement(),
        }
    }

    fn let_decl(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::Let, "expected 'let'")?;
        // Destructuring: `let [a, b] = ...` / `let {x, y} = ...`.
        if matches!(self.peek().kind, K::LBracket | K::LBrace) {
            let pattern = self.pattern()?;
            self.consume(&K::Eq, "a destructuring 'let' must be initialized: expected '='")?;
            let init = self.expression()?;
            let semi = self.consume(&K::Semicolon, "expected ';' after the destructuring declaration")?;
            return Ok(Stmt::Destructure { pattern, init, span: kw.span.to(semi.span) });
        }
        let (name, name_span) = self.consume_ident("expected a variable name after 'let'")?;
        let init = if self.match_kind(&K::Eq) {
            Some(self.expression()?)
        } else {
            None
        };
        let semi = self.consume(&K::Semicolon, "expected ';' after variable declaration")?;
        Ok(Stmt::Let { name, name_span, init, span: kw.span.to(semi.span) })
    }

    fn const_decl(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::Const, "expected 'const'")?;
        let (name, name_span) = self.consume_ident("expected a constant name after 'const'")?;
        self.consume(&K::Eq, "a 'const' must be initialized: expected '='")?;
        let init = self.expression()?;
        let semi = self.consume(&K::Semicolon, "expected ';' after constant declaration")?;
        Ok(Stmt::Const { name, name_span, init, span: kw.span.to(semi.span) })
    }

    fn fn_decl(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::Fn, "expected 'fn'")?;
        let (name, name_span) = self.consume_ident("expected a function name after 'fn'")?;
        let func = self.function_rest(Some(name), name_span, kw.span)?;
        Ok(Stmt::Function(func))
    }

    /// Parse `( params ) block` after a function/lambda/method header.
    fn function_rest(
        &mut self,
        name: Option<String>,
        name_span: Span,
        start: Span,
    ) -> PResult<Function> {
        self.consume(&K::LParen, "expected '(' to begin the parameter list")?;
        let params = self.params()?;
        self.consume(&K::RParen, "expected ')' after parameters")?;
        let body = self.block()?;
        let span = start.to(body.span);
        Ok(Function { name, name_span, params, body, span })
    }

    fn params(&mut self) -> PResult<Vec<Param>> {
        let mut params = Vec::new();
        if !self.check(&K::RParen) {
            loop {
                // A rest parameter `..name` collects the remaining arguments and
                // must be last.
                if self.match_kind(&K::DotDot) {
                    let (name, span) = self.consume_ident("expected a name after '..'")?;
                    params.push(Param { name, span, default: None, is_rest: true });
                    self.match_kind(&K::Comma); // optional trailing comma
                    break;
                }
                let (name, span) = self.consume_ident("expected a parameter name")?;
                let default = if self.match_kind(&K::Eq) {
                    Some(self.expression()?)
                } else {
                    None
                };
                params.push(Param { name, span, default, is_rest: false });
                if !self.match_kind(&K::Comma) {
                    break;
                }
                if self.check(&K::RParen) {
                    break; // trailing comma
                }
            }
        }
        Ok(params)
    }

    fn class_decl(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::Class, "expected 'class'")?;
        let (name, name_span) = self.consume_ident("expected a class name after 'class'")?;
        let superclass = if self.match_kind(&K::Lt) {
            let (s, sp) = self.consume_ident("expected a superclass name after '<'")?;
            Some(Spanned::new(s, sp))
        } else {
            None
        };
        self.consume(&K::LBrace, "expected '{' to begin the class body")?;
        let mut methods = Vec::new();
        let mut statics = Vec::new();
        let mut fields = Vec::new();
        while !self.check(&K::RBrace) && !self.is_at_end() {
            let start = self.pos;
            match self.class_member(&mut methods, &mut statics, &mut fields) {
                Ok(()) => {}
                Err(_) => {
                    // Recover to the next member or the closing brace.
                    while !self.check(&K::RBrace) && !self.is_at_end() {
                        if matches!(self.peek().kind, K::Ident(_)) {
                            break;
                        }
                        self.advance();
                    }
                }
            }
            if self.pos == start && !self.check(&K::RBrace) && !self.is_at_end() {
                self.advance();
            }
        }
        let close = self.consume(&K::RBrace, "expected '}' to close the class body")?;
        Ok(Stmt::Class(ClassDecl {
            name,
            name_span,
            superclass,
            methods,
            statics,
            fields,
            span: kw.span.to(close.span),
        }))
    }

    /// Parse one class-body member: a `static` method, an instance method
    /// (`name(...) { ... }`), or a field declaration (`name [= expr];`). `static`
    /// is contextual (only special before a method name).
    fn class_member(
        &mut self,
        methods: &mut Vec<Function>,
        statics: &mut Vec<Function>,
        fields: &mut Vec<Field>,
    ) -> PResult<()> {
        let is_static = matches!(&self.peek().kind, K::Ident(s) if s == "static")
            && matches!(self.peek2().kind, K::Ident(_));
        if is_static {
            self.advance(); // consume the contextual `static`
            statics.push(self.method()?);
            return Ok(());
        }
        if matches!(self.peek().kind, K::Ident(_)) && matches!(self.peek2().kind, K::LParen) {
            methods.push(self.method()?);
            return Ok(());
        }
        // Field declaration: `name [= expr] ;`.
        let (name, name_span) = self.consume_ident("expected a method or field name")?;
        let init = if self.match_kind(&K::Eq) {
            Some(self.expression()?)
        } else {
            None
        };
        let semi = self.consume(&K::Semicolon, "expected ';' after the field declaration")?;
        fields.push(Field { name, name_span, init, span: name_span.to(semi.span) });
        Ok(())
    }

    fn method(&mut self) -> PResult<Function> {
        let (name, name_span) = self.consume_ident("expected a method name")?;
        self.function_rest(Some(name), name_span, name_span)
    }

    fn import_decl(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::Import, "expected 'import'")?;
        let (path, path_span) = self.string_literal("expected a module path string after 'import'")?;
        let kind = if self.match_contextual("as") {
            let (alias, span) = self.consume_ident("expected an alias name after 'as'")?;
            ImportKind::Module { alias: Spanned::new(alias, span) }
        } else if self.match_kind(&K::Dot) {
            self.consume(&K::LBrace, "expected '{' after '.' in a selective import")?;
            let mut names = Vec::new();
            if !self.check(&K::RBrace) {
                loop {
                    let (n, sp) = self.consume_ident("expected an export name")?;
                    names.push(Spanned::new(n, sp));
                    if !self.match_kind(&K::Comma) {
                        break;
                    }
                    if self.check(&K::RBrace) {
                        break;
                    }
                }
            }
            self.consume(&K::RBrace, "expected '}' to close the import list")?;
            ImportKind::Named(names)
        } else {
            let base = module_basename(&path);
            ImportKind::Module { alias: Spanned::new(base, path_span) }
        };
        let semi = self.consume(&K::Semicolon, "expected ';' after import")?;
        Ok(Stmt::Import(Import { path, path_span, kind, span: kw.span.to(semi.span) }))
    }

    fn export_decl(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::Export, "expected 'export'")?;
        let decl = match self.peek().kind {
            K::Let => self.let_decl()?,
            K::Const => self.const_decl()?,
            K::Fn => self.fn_decl()?,
            K::Class => self.class_decl()?,
            _ => {
                let span = self.peek().span;
                self.error_at(span, "expected a declaration (let/const/fn/class) after 'export'");
                return Err(ParseError);
            }
        };
        let span = kw.span.to(decl.span());
        Ok(Stmt::Export { decl: Box::new(decl), span })
    }

    // ---- statements --------------------------------------------------------

    fn statement(&mut self) -> PResult<Stmt> {
        // A leading `[`/`{` whose matching close is followed by `=` is a
        // destructuring assignment (`[a, b] = …`, `{k} = …`), not an array
        // literal / block. See DESIGN D24.
        if matches!(self.peek().kind, K::LBracket | K::LBrace)
            && self.looks_like_destructure_assign()
        {
            return self.destructure_assign_stmt();
        }
        match self.peek().kind {
            K::LBrace => Ok(Stmt::Block(self.block()?)),
            K::If => self.if_stmt(),
            K::While => self.while_stmt(),
            K::For => self.for_stmt(),
            K::Return => self.return_stmt(),
            K::Break => {
                let kw = self.advance();
                let semi = self.consume(&K::Semicolon, "expected ';' after 'break'")?;
                Ok(Stmt::Break { span: kw.span.to(semi.span) })
            }
            K::Continue => {
                let kw = self.advance();
                let semi = self.consume(&K::Semicolon, "expected ';' after 'continue'")?;
                Ok(Stmt::Continue { span: kw.span.to(semi.span) })
            }
            K::Throw => self.throw_stmt(),
            K::Yield => self.yield_stmt(),
            K::Try => self.try_stmt(),
            _ => self.expr_stmt(),
        }
    }

    fn block(&mut self) -> PResult<Block> {
        let open = self.consume(&K::LBrace, "expected '{'")?;
        let mut stmts = Vec::new();
        while !self.check(&K::RBrace) && !self.is_at_end() {
            let start = self.pos;
            match self.declaration() {
                Ok(s) => stmts.push(s),
                Err(_) => self.synchronize(),
            }
            if self.pos == start && !self.check(&K::RBrace) && !self.is_at_end() {
                self.advance();
            }
        }
        let close = self.consume(&K::RBrace, "expected '}' to close this block")?;
        Ok(Block { stmts, span: open.span.to(close.span) })
    }

    fn if_stmt(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::If, "expected 'if'")?;
        let cond = self.expression()?;
        let then_block = self.block()?;
        let mut span = kw.span.to(then_block.span);
        let else_branch = if self.match_kind(&K::Else) {
            let branch = if self.check(&K::If) {
                self.if_stmt()?
            } else {
                Stmt::Block(self.block()?)
            };
            span = kw.span.to(branch.span());
            Some(Box::new(branch))
        } else {
            None
        };
        Ok(Stmt::If { cond, then_block, else_branch, span })
    }

    fn while_stmt(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::While, "expected 'while'")?;
        let cond = self.expression()?;
        let body = self.block()?;
        let span = kw.span.to(body.span);
        Ok(Stmt::While { cond, body, span })
    }

    fn for_stmt(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::For, "expected 'for'")?;
        // for-in is `for IDENT in EXPR { }`.
        if matches!(self.peek().kind, K::Ident(_)) && matches!(self.peek2().kind, K::In) {
            let (var, var_span) = self.consume_ident("expected a loop variable")?;
            self.consume(&K::In, "expected 'in'")?;
            let iter = self.expression()?;
            let body = self.block()?;
            let span = kw.span.to(body.span);
            return Ok(Stmt::ForIn { var, var_span, iter, body, span });
        }
        // C-style `for init; cond; step { }`. The init clause supplies its `;`.
        let init = if self.match_kind(&K::Semicolon) {
            None
        } else if self.check(&K::Let) {
            Some(Box::new(self.let_decl()?))
        } else {
            Some(Box::new(self.expr_stmt()?))
        };
        let cond = if self.check(&K::Semicolon) {
            None
        } else {
            Some(self.expression()?)
        };
        self.consume(&K::Semicolon, "expected ';' after the for-loop condition")?;
        let step = if self.check(&K::LBrace) {
            None
        } else {
            Some(self.expression()?)
        };
        let body = self.block()?;
        let span = kw.span.to(body.span);
        Ok(Stmt::ForC { init, cond, step, body, span })
    }

    fn return_stmt(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::Return, "expected 'return'")?;
        let value = if self.check(&K::Semicolon) {
            None
        } else {
            Some(self.expression()?)
        };
        let semi = self.consume(&K::Semicolon, "expected ';' after return value")?;
        Ok(Stmt::Return { value, span: kw.span.to(semi.span) })
    }

    fn throw_stmt(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::Throw, "expected 'throw'")?;
        let value = self.expression()?;
        let semi = self.consume(&K::Semicolon, "expected ';' after the thrown value")?;
        Ok(Stmt::Throw { value, span: kw.span.to(semi.span) })
    }

    fn yield_stmt(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::Yield, "expected 'yield'")?;
        let value = self.expression()?;
        let semi = self.consume(&K::Semicolon, "expected ';' after the yielded value")?;
        Ok(Stmt::Yield { value, span: kw.span.to(semi.span) })
    }

    fn try_stmt(&mut self) -> PResult<Stmt> {
        let kw = self.consume(&K::Try, "expected 'try'")?;
        let body = self.block()?;
        let mut span = body.span;
        // Zero or more catch clauses, each optionally typed `catch (Kind e)`.
        let mut catches = Vec::new();
        while self.match_kind(&K::Catch) {
            self.consume(&K::LParen, "expected '(' after 'catch'")?;
            // `catch (e)` is a bare catch; `catch (Kind e)` is a typed catch
            // (two identifiers: the error kind then the variable).
            let (first, first_span) = self.consume_ident("expected an exception variable name")?;
            let (kind, name, name_span) = if let K::Ident(_) = self.peek().kind {
                let (var, var_span) = self.consume_ident("expected an exception variable name")?;
                (Some(first), var, var_span)
            } else {
                (None, first, first_span)
            };
            self.consume(&K::RParen, "expected ')' after the catch variable")?;
            let cbody = self.block()?;
            span = kw.span.to(cbody.span);
            catches.push(CatchClause { kind, name, name_span, body: cbody });
        }
        let finally = if self.match_kind(&K::Finally) {
            let b = self.block()?;
            span = kw.span.to(b.span);
            Some(b)
        } else {
            None
        };
        if catches.is_empty() && finally.is_none() {
            self.error_at(
                kw.span,
                "a 'try' must be followed by a 'catch' clause, a 'finally' clause, or both",
            );
            return Err(ParseError);
        }
        Ok(Stmt::Try { body, catches, finally, span: kw.span.to(span) })
    }

    fn expr_stmt(&mut self) -> PResult<Stmt> {
        let expr = self.expression()?;
        let semi = self.consume(&K::Semicolon, "expected ';' after the expression")?;
        let span = expr.span.to(semi.span);
        Ok(Stmt::Expr { expr, span })
    }

    /// Lookahead for a destructuring assignment: starting at the current `[`/`{`,
    /// scan to its matching close bracket/brace (tracking nesting of `()[]{}`) and
    /// report whether the token immediately after is a single `=` (DESIGN D24).
    fn looks_like_destructure_assign(&self) -> bool {
        let mut depth = 0usize;
        let mut i = self.pos;
        while i < self.tokens.len() {
            match self.tokens[i].kind {
                K::LParen | K::LBracket | K::LBrace => depth += 1,
                K::RParen | K::RBracket | K::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.tokens.get(i + 1).map(|t| &t.kind), Some(K::Eq));
                    }
                }
                K::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn destructure_assign_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        let pattern = self.pattern()?;
        self.consume(&K::Eq, "expected '=' in destructuring assignment")?;
        let value = self.expression()?;
        let semi = self.consume(&K::Semicolon, "expected ';' after the assignment")?;
        Ok(Stmt::DestructureAssign { pattern, value, span: start.to(semi.span) })
    }

    // ---- expressions (precedence climbing) --------------------------------

    fn expression(&mut self) -> PResult<Expr> {
        self.assignment()
    }

    fn assignment(&mut self) -> PResult<Expr> {
        let expr = self.ternary()?;
        if self.check(&K::Eq) {
            let eq = self.advance();
            let value = self.assignment()?; // right-associative
            if !is_assignable(&expr.kind) {
                self.error_at(eq.span, "invalid assignment target");
                return Err(ParseError);
            }
            let span = expr.span.to(value.span);
            Ok(Expr::new(ExprKind::Assign { target: Box::new(expr), value: Box::new(value) }, span))
        } else if let Some(op) = compound_op(&self.peek().kind) {
            let tok = self.advance();
            let value = self.assignment()?; // right-associative
            if !is_assignable(&expr.kind) {
                self.error_at(tok.span, "invalid assignment target");
                return Err(ParseError);
            }
            let span = expr.span.to(value.span);
            Ok(Expr::new(
                ExprKind::CompoundAssign { target: Box::new(expr), op, value: Box::new(value) },
                span,
            ))
        } else {
            Ok(expr)
        }
    }

    /// `cond ? then : else` — sits between assignment and `||`. Right-
    /// associative, so `a ? b : c ? d : e` parses as `a ? b : (c ? d : e)`.
    fn ternary(&mut self) -> PResult<Expr> {
        let cond = self.logic_or()?;
        if self.check(&K::Question) {
            self.advance();
            let then_branch = self.assignment()?;
            self.consume(&K::Colon, "expected ':' in conditional expression")?;
            let else_branch = self.assignment()?;
            let span = cond.span.to(else_branch.span);
            Ok(Expr::new(
                ExprKind::Ternary {
                    cond: Box::new(cond),
                    then_branch: Box::new(then_branch),
                    else_branch: Box::new(else_branch),
                },
                span,
            ))
        } else {
            Ok(cond)
        }
    }

    fn logic_or(&mut self) -> PResult<Expr> {
        let mut left = self.logic_and()?;
        while matches!(self.peek().kind, K::PipePipe | K::Or) {
            self.advance();
            let right = self.logic_and()?;
            let span = left.span.to(right.span);
            left = Expr::new(
                ExprKind::Logical {
                    op: LogicalOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    fn logic_and(&mut self) -> PResult<Expr> {
        let mut left = self.equality()?;
        while matches!(self.peek().kind, K::AmpAmp | K::And) {
            self.advance();
            let right = self.equality()?;
            let span = left.span.to(right.span);
            left = Expr::new(
                ExprKind::Logical {
                    op: LogicalOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    fn equality(&mut self) -> PResult<Expr> {
        let mut left = self.comparison()?;
        loop {
            let op = match self.peek().kind {
                K::EqEq => BinaryOp::Eq,
                K::BangEq => BinaryOp::Ne,
                _ => break,
            };
            self.advance();
            let right = self.comparison()?;
            left = self.binary(op, left, right);
        }
        Ok(left)
    }

    fn comparison(&mut self) -> PResult<Expr> {
        let mut left = self.bit_or()?;
        loop {
            let op = match self.peek().kind {
                K::Lt => BinaryOp::Lt,
                K::LtEq => BinaryOp::Le,
                K::Gt => BinaryOp::Gt,
                K::GtEq => BinaryOp::Ge,
                K::Is => BinaryOp::Is,
                _ => break,
            };
            self.advance();
            let right = self.bit_or()?;
            left = self.binary(op, left, right);
        }
        Ok(left)
    }

    // Bitwise operators bind tighter than comparison (Lua/Python convention), in
    // the order `|` < `^` < `&` < shift, with shifts just above `+`/`-`.
    fn bit_or(&mut self) -> PResult<Expr> {
        let mut left = self.bit_xor()?;
        while matches!(self.peek().kind, K::Pipe) {
            self.advance();
            let right = self.bit_xor()?;
            left = self.binary(BinaryOp::BitOr, left, right);
        }
        Ok(left)
    }

    fn bit_xor(&mut self) -> PResult<Expr> {
        let mut left = self.bit_and()?;
        while matches!(self.peek().kind, K::Caret) {
            self.advance();
            let right = self.bit_and()?;
            left = self.binary(BinaryOp::BitXor, left, right);
        }
        Ok(left)
    }

    fn bit_and(&mut self) -> PResult<Expr> {
        let mut left = self.shift()?;
        while matches!(self.peek().kind, K::Amp) {
            self.advance();
            let right = self.shift()?;
            left = self.binary(BinaryOp::BitAnd, left, right);
        }
        Ok(left)
    }

    fn shift(&mut self) -> PResult<Expr> {
        let mut left = self.term()?;
        loop {
            let op = match self.peek().kind {
                K::Shl => BinaryOp::Shl,
                K::Shr => BinaryOp::Shr,
                _ => break,
            };
            self.advance();
            let right = self.term()?;
            left = self.binary(op, left, right);
        }
        Ok(left)
    }

    fn term(&mut self) -> PResult<Expr> {
        let mut left = self.factor()?;
        loop {
            let op = match self.peek().kind {
                K::Plus => BinaryOp::Add,
                K::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.factor()?;
            left = self.binary(op, left, right);
        }
        Ok(left)
    }

    fn factor(&mut self) -> PResult<Expr> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek().kind {
                K::Star => BinaryOp::Mul,
                K::Slash => BinaryOp::Div,
                K::Percent => BinaryOp::Rem,
                _ => break,
            };
            self.advance();
            let right = self.unary()?;
            left = self.binary(op, left, right);
        }
        Ok(left)
    }

    fn binary(&self, op: BinaryOp, left: Expr, right: Expr) -> Expr {
        let span = left.span.to(right.span);
        Expr::new(
            ExprKind::Binary { op, left: Box::new(left), right: Box::new(right) },
            span,
        )
    }

    fn unary(&mut self) -> PResult<Expr> {
        let (op, kw_span) = match self.peek().kind {
            K::Bang | K::Not => (UnaryOp::Not, self.peek().span),
            K::Minus => (UnaryOp::Neg, self.peek().span),
            K::Tilde => (UnaryOp::BitNot, self.peek().span),
            _ => return self.power(),
        };
        self.advance();
        let operand = self.unary()?;
        let span = kw_span.to(operand.span);
        Ok(Expr::new(ExprKind::Unary { op, operand: Box::new(operand) }, span))
    }

    /// `**` exponentiation: binds tighter than unary minus (so `-2 ** 2` is
    /// `-(2 ** 2)`) and is right-associative (`2 ** 3 ** 2` is `2 ** (3 ** 2)`).
    /// The base is a postfix expression; the exponent is a full `unary` so it may
    /// carry a sign (`2 ** -1`).
    fn power(&mut self) -> PResult<Expr> {
        let base = self.postfix()?;
        if matches!(self.peek().kind, K::StarStar) {
            self.advance();
            let exp = self.unary()?;
            Ok(self.binary(BinaryOp::Pow, base, exp))
        } else {
            Ok(base)
        }
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.primary()?;
        loop {
            match self.peek().kind {
                K::LParen => {
                    expr = self.finish_call(expr)?;
                }
                K::LBracket => {
                    self.advance();
                    let index = self.expression()?;
                    let close = self.consume(&K::RBracket, "expected ']' after the index")?;
                    let span = expr.span.to(close.span);
                    expr = Expr::new(
                        ExprKind::Index { object: Box::new(expr), index: Box::new(index) },
                        span,
                    );
                }
                K::Dot => {
                    self.advance();
                    let (name, name_span) =
                        self.consume_ident("expected a property name after '.'")?;
                    let span = expr.span.to(name_span);
                    expr = Expr::new(
                        ExprKind::Get { object: Box::new(expr), name, name_span },
                        span,
                    );
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn finish_call(&mut self, callee: Expr) -> PResult<Expr> {
        self.consume(&K::LParen, "expected '('")?;
        let mut args = Vec::new();
        if !self.check(&K::RParen) {
            loop {
                if self.match_kind(&K::DotDot) {
                    args.push(CallArg::Spread(self.expression()?));
                } else {
                    args.push(CallArg::Item(self.expression()?));
                }
                if !self.match_kind(&K::Comma) {
                    break;
                }
                if self.check(&K::RParen) {
                    break;
                }
            }
        }
        let close = self.consume(&K::RParen, "expected ')' after the arguments")?;
        let span = callee.span.to(close.span);
        Ok(Expr::new(
            ExprKind::Call { callee: Box::new(callee), args, paren_span: close.span },
            span,
        ))
    }

    fn primary(&mut self) -> PResult<Expr> {
        let span = self.peek().span;
        match self.peek().kind.clone() {
            K::Int(n) => {
                self.advance();
                Ok(Expr::new(ExprKind::Int(n), span))
            }
            K::Float(f) => {
                self.advance();
                Ok(Expr::new(ExprKind::Float(f), span))
            }
            K::True => {
                self.advance();
                Ok(Expr::new(ExprKind::Bool(true), span))
            }
            K::False => {
                self.advance();
                Ok(Expr::new(ExprKind::Bool(false), span))
            }
            K::Nil => {
                self.advance();
                Ok(Expr::new(ExprKind::Nil, span))
            }
            K::Str(parts) => {
                self.advance();
                self.build_string(parts, span)
            }
            K::Ident(name) if matches!(self.peek2().kind, K::FatArrow) => {
                // `x => expr` — single-parameter arrow lambda.
                self.advance(); // the identifier
                self.advance(); // '=>'
                let params = vec![Param { name, span, default: None, is_rest: false }];
                self.finish_arrow(params, span)
            }
            K::Ident(name) => {
                self.advance();
                Ok(Expr::new(ExprKind::Var(name), span))
            }
            K::This => {
                self.advance();
                Ok(Expr::new(ExprKind::This, span))
            }
            K::Super => {
                self.advance();
                self.consume(&K::Dot, "expected '.' after 'super'")?;
                let (method, method_span) =
                    self.consume_ident("expected a method name after 'super.'")?;
                Ok(Expr::new(
                    ExprKind::Super { method, method_span },
                    span.to(method_span),
                ))
            }
            K::LParen if self.is_arrow_params() => {
                // `(a, b) => expr` / `() => expr` — arrow lambda with a parameter
                // list (reusing the `fn` parameter parser, so defaults/rest work).
                self.advance(); // '('
                let params = self.params()?;
                self.consume(&K::RParen, "expected ')' after lambda parameters")?;
                self.consume(&K::FatArrow, "expected '=>' after lambda parameters")?;
                self.finish_arrow(params, span)
            }
            K::LParen => {
                self.advance();
                let inner = self.expression()?;
                let close = self.consume(&K::RParen, "expected ')' to close the group")?;
                Ok(Expr::new(inner.kind, span.to(close.span)))
            }
            K::LBracket => self.array_literal(),
            K::LBrace => self.map_literal(),
            K::Fn => self.lambda(),
            K::Match => self.match_expr(),
            _ => {
                let found = self.peek().kind.describe();
                self.error_at(span, format!("expected an expression, found {found}"));
                Err(ParseError)
            }
        }
    }

    fn array_literal(&mut self) -> PResult<Expr> {
        let open = self.consume(&K::LBracket, "expected '['")?;
        let mut elems = Vec::new();
        if !self.check(&K::RBracket) {
            // First element. If it is a plain expression followed by `for`, this is
            // a comprehension (DESIGN D31).
            if self.match_kind(&K::DotDot) {
                elems.push(ArrayElem::Spread(self.expression()?));
            } else {
                let first = self.expression()?;
                if self.check(&K::For) {
                    return self.finish_comprehension(CompHead::Array(first), open.span, &K::RBracket);
                }
                elems.push(ArrayElem::Item(first));
            }
            while self.match_kind(&K::Comma) {
                if self.check(&K::RBracket) {
                    break;
                }
                if self.match_kind(&K::DotDot) {
                    elems.push(ArrayElem::Spread(self.expression()?));
                } else {
                    elems.push(ArrayElem::Item(self.expression()?));
                }
            }
        }
        let close = self.consume(&K::RBracket, "expected ']' to close the array")?;
        Ok(Expr::new(ExprKind::Array(elems), open.span.to(close.span)))
    }

    /// Parse the `for var in iter [if cond]` tail of a comprehension (the
    /// element/entry is already parsed) and the closing bracket.
    fn finish_comprehension(&mut self, head: CompHead, open: Span, close_kind: &TokenKind) -> PResult<Expr> {
        self.consume(&K::For, "expected 'for' in comprehension")?;
        let (var, var_span) = self.consume_ident("expected the comprehension variable")?;
        self.consume(&K::In, "expected 'in' after the comprehension variable")?;
        let iter = self.expression()?;
        let cond = if self.match_kind(&K::If) {
            Some(Box::new(self.expression()?))
        } else {
            None
        };
        let close = self.consume(close_kind, "expected the comprehension to be closed")?;
        let span = open.to(close.span);
        let kind = match head {
            CompHead::Array(element) => ExprKind::ArrayComp {
                element: Box::new(element),
                var,
                var_span,
                iter: Box::new(iter),
                cond,
            },
            CompHead::Map(key, value) => ExprKind::MapComp {
                key: Box::new(key),
                value: Box::new(value),
                var,
                var_span,
                iter: Box::new(iter),
                cond,
            },
        };
        Ok(Expr::new(kind, span))
    }

    fn map_literal(&mut self) -> PResult<Expr> {
        let open = self.consume(&K::LBrace, "expected '{'")?;
        let mut entries = Vec::new();
        if !self.check(&K::RBrace) {
            // First entry. Remember whether the key was a bare identifier so a
            // comprehension can treat it as the loop *variable* rather than a
            // string shorthand (DESIGN D31).
            let key_span = self.peek().span;
            let bare_ident = if let K::Ident(n) = &self.peek().kind { Some(n.clone()) } else { None };
            let key = self.map_key()?;
            self.consume(&K::Colon, "expected ':' after the map key")?;
            let value = self.expression()?;
            if self.check(&K::For) {
                let key_expr = match (key, bare_ident) {
                    (MapKey::Str(_), Some(name)) => Expr::new(ExprKind::Var(name), key_span),
                    (MapKey::Str(s), None) => Expr::new(ExprKind::Str(s), key_span),
                    (MapKey::Computed(e), _) => e,
                };
                return self.finish_comprehension(CompHead::Map(key_expr, value), open.span, &K::RBrace);
            }
            entries.push((key, value));
            while self.match_kind(&K::Comma) {
                if self.check(&K::RBrace) {
                    break;
                }
                let key = self.map_key()?;
                self.consume(&K::Colon, "expected ':' after the map key")?;
                let value = self.expression()?;
                entries.push((key, value));
            }
        }
        let close = self.consume(&K::RBrace, "expected '}' to close the map")?;
        Ok(Expr::new(ExprKind::Map(entries), open.span.to(close.span)))
    }

    fn map_key(&mut self) -> PResult<MapKey> {
        let span = self.peek().span;
        match self.peek().kind.clone() {
            K::Ident(name) => {
                self.advance();
                Ok(MapKey::Str(name))
            }
            K::Str(parts) => {
                self.advance();
                Ok(MapKey::Str(self.plain_string(&parts, span, "map key")?))
            }
            K::LBracket => {
                self.advance();
                let key = self.expression()?;
                self.consume(&K::RBracket, "expected ']' after a computed map key")?;
                Ok(MapKey::Computed(key))
            }
            _ => {
                let found = self.peek().kind.describe();
                self.error_at(
                    span,
                    format!("expected a map key (identifier, string, or [expr]), found {found}"),
                );
                Err(ParseError)
            }
        }
    }

    fn lambda(&mut self) -> PResult<Expr> {
        let kw = self.consume(&K::Fn, "expected 'fn'")?;
        let func = self.function_rest(None, kw.span, kw.span)?;
        let span = func.span;
        Ok(Expr::new(ExprKind::Lambda(func), span))
    }

    /// Finish an arrow lambda once its parameters are parsed: the body is a
    /// single expression (parsed at assignment level, so it extends rightward and
    /// `a => b => c` curries), wrapped as a `Function` that returns it. Reusing
    /// `ExprKind::Lambda` means the resolver and compiler need no changes.
    fn finish_arrow(&mut self, params: Vec<Param>, start: Span) -> PResult<Expr> {
        let body = self.assignment()?;
        let span = start.to(body.span);
        let block = Block { stmts: vec![Stmt::Return { value: Some(body), span }], span };
        let func = Function { name: None, name_span: start, params, body: block, span };
        Ok(Expr::new(ExprKind::Lambda(func), span))
    }

    /// With the cursor on `(`, whether the matching `)` is immediately followed by
    /// `=>` — distinguishing an arrow lambda's parameter list from a grouping.
    fn is_arrow_params(&self) -> bool {
        let mut depth = 0usize;
        let mut i = self.pos;
        while i < self.tokens.len() {
            match &self.tokens[i].kind {
                K::LParen => depth += 1,
                K::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.tokens.get(i + 1).map(|t| &t.kind), Some(K::FatArrow));
                    }
                }
                K::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn match_expr(&mut self) -> PResult<Expr> {
        let kw = self.consume(&K::Match, "expected 'match'")?;
        let subject = self.expression()?;
        self.consume(&K::LBrace, "expected '{' to begin the match arms")?;
        let mut arms = Vec::new();
        if !self.check(&K::RBrace) {
            loop {
                arms.push(self.match_arm()?);
                if !self.match_kind(&K::Comma) {
                    break;
                }
                if self.check(&K::RBrace) {
                    break;
                }
            }
        }
        let close = self.consume(&K::RBrace, "expected '}' to close the match")?;
        Ok(Expr::new(
            ExprKind::Match { subject: Box::new(subject), arms },
            kw.span.to(close.span),
        ))
    }

    fn match_arm(&mut self) -> PResult<MatchArm> {
        let pattern = self.pattern()?;
        let guard = if self.match_kind(&K::If) {
            Some(self.expression()?)
        } else {
            None
        };
        self.consume(&K::FatArrow, "expected '=>' after the pattern")?;
        let body = self.expression()?;
        let span = pattern.span.to(body.span);
        Ok(MatchArm { pattern, guard, body, span })
    }

    /// A pattern, possibly an `a | b | c` alternation (DESIGN D25). The bitwise
    /// `|` token is repurposed here because patterns are a distinct grammar with
    /// no bitwise operators.
    fn pattern(&mut self) -> PResult<Pattern> {
        let first = self.pattern_atom()?;
        if !self.check(&K::Pipe) {
            return Ok(first);
        }
        let start = first.span;
        let mut alts = vec![first];
        while self.match_kind(&K::Pipe) {
            alts.push(self.pattern_atom()?);
        }
        let span = start.to(alts.last().unwrap().span);
        Ok(Pattern { kind: PatternKind::Or(alts), span })
    }

    fn pattern_atom(&mut self) -> PResult<Pattern> {
        let span = self.peek().span;
        match self.peek().kind.clone() {
            K::Ident(name) => {
                self.advance();
                let kind = if name == "_" {
                    PatternKind::Wildcard
                } else {
                    PatternKind::Binding(name)
                };
                Ok(Pattern { kind, span })
            }
            K::Int(n) => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Int(n), span })
            }
            K::Float(f) => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Float(f), span })
            }
            K::Str(parts) => {
                self.advance();
                let s = self.plain_string(&parts, span, "string pattern")?;
                Ok(Pattern { kind: PatternKind::Str(s), span })
            }
            K::True => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Bool(true), span })
            }
            K::False => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Bool(false), span })
            }
            K::Nil => {
                self.advance();
                Ok(Pattern { kind: PatternKind::Nil, span })
            }
            K::Minus => {
                self.advance();
                let neg_span = self.peek().span;
                match self.peek().kind {
                    K::Int(n) => {
                        self.advance();
                        Ok(Pattern { kind: PatternKind::Int(-n), span: span.to(neg_span) })
                    }
                    K::Float(f) => {
                        self.advance();
                        Ok(Pattern { kind: PatternKind::Float(-f), span: span.to(neg_span) })
                    }
                    _ => {
                        self.error_at(neg_span, "expected a number after '-' in a pattern");
                        Err(ParseError)
                    }
                }
            }
            K::LBracket => self.array_pattern(),
            K::LBrace => self.map_pattern(),
            _ => {
                let found = self.peek().kind.describe();
                self.error_at(span, format!("expected a pattern, found {found}"));
                Err(ParseError)
            }
        }
    }

    fn array_pattern(&mut self) -> PResult<Pattern> {
        let open = self.consume(&K::LBracket, "expected '['")?;
        let mut elems = Vec::new();
        let mut rest_count = 0;
        if !self.check(&K::RBracket) {
            loop {
                if self.match_kind(&K::DotDot) {
                    let name = if let K::Ident(n) = &self.peek().kind {
                        let n = n.clone();
                        self.advance();
                        Some(n)
                    } else {
                        None
                    };
                    elems.push(PatElem::Rest(name));
                    rest_count += 1;
                } else {
                    elems.push(PatElem::Pattern(self.pattern()?));
                }
                if !self.match_kind(&K::Comma) {
                    break;
                }
                if self.check(&K::RBracket) {
                    break;
                }
            }
        }
        let close = self.consume(&K::RBracket, "expected ']' to close the array pattern")?;
        let span = open.span.to(close.span);
        if rest_count > 1 {
            self.error_at(span, "an array pattern may have at most one '..' rest element");
            return Err(ParseError);
        }
        Ok(Pattern { kind: PatternKind::Array(elems), span })
    }

    fn map_pattern(&mut self) -> PResult<Pattern> {
        let open = self.consume(&K::LBrace, "expected '{'")?;
        let mut entries = Vec::new();
        if !self.check(&K::RBrace) {
            loop {
                let key_span = self.peek().span;
                let (key, key_is_ident) = match self.peek().kind.clone() {
                    K::Ident(n) => {
                        self.advance();
                        (n, true)
                    }
                    K::Str(parts) => {
                        self.advance();
                        (self.plain_string(&parts, key_span, "map pattern key")?, false)
                    }
                    _ => {
                        let found = self.peek().kind.describe();
                        self.error_at(
                            key_span,
                            format!("expected a map pattern key, found {found}"),
                        );
                        return Err(ParseError);
                    }
                };
                // `{x}` is shorthand for `{x: x}` (binds the key name). A string
                // key requires an explicit `: pattern`.
                let pat = if self.match_kind(&K::Colon) {
                    self.pattern()?
                } else if key_is_ident {
                    Pattern { kind: PatternKind::Binding(key.clone()), span: key_span }
                } else {
                    self.consume(&K::Colon, "a string map-pattern key needs an explicit ': pattern'")?;
                    self.pattern()?
                };
                entries.push((key, pat));
                if !self.match_kind(&K::Comma) {
                    break;
                }
                if self.check(&K::RBrace) {
                    break;
                }
            }
        }
        let close = self.consume(&K::RBrace, "expected '}' to close the map pattern")?;
        Ok(Pattern { kind: PatternKind::Map(entries), span: open.span.to(close.span) })
    }

    // ---- string helpers ----------------------------------------------------

    /// Build an expression from a lexed string's parts: a plain `Str` when there
    /// is no interpolation, otherwise a `StrInterp` whose interpolation segments
    /// are sub-parsed as full expressions.
    fn build_string(&mut self, parts: Vec<StrPart>, span: Span) -> PResult<Expr> {
        if parts.is_empty() {
            return Ok(Expr::new(ExprKind::Str(String::new()), span));
        }
        if parts.len() == 1 {
            if let StrPart::Text(t) = &parts[0] {
                return Ok(Expr::new(ExprKind::Str(t.clone()), span));
            }
        }
        let mut segments = Vec::new();
        for part in parts {
            match part {
                StrPart::Text(t) => segments.push(StrSegment::Text(t)),
                StrPart::Interp(toks) => {
                    let expr = self.sub_parse_expr(toks)?;
                    segments.push(StrSegment::Expr(expr));
                }
            }
        }
        Ok(Expr::new(ExprKind::StrInterp(segments), span))
    }

    /// Parse a pre-tokenized interpolation expression with a sub-parser,
    /// folding its errors back into this parser's list.
    fn sub_parse_expr(&mut self, tokens: Vec<Token>) -> PResult<Expr> {
        let mut sub = Parser::new(tokens);
        let result = sub.expression();
        if result.is_ok() && !sub.is_at_end() {
            let span = sub.peek().span;
            sub.error_at(span, "unexpected tokens after the interpolation expression");
        }
        self.errors.append(&mut sub.errors);
        result
    }

    /// Require a plain (non-interpolated) string literal at the cursor.
    fn string_literal(&mut self, msg: &str) -> PResult<(String, Span)> {
        let span = self.peek().span;
        if let K::Str(parts) = self.peek().kind.clone() {
            self.advance();
            let s = self.plain_string(&parts, span, "string")?;
            Ok((s, span))
        } else {
            let found = self.peek().kind.describe();
            self.error_at(span, format!("{msg}, found {found}"));
            Err(ParseError)
        }
    }

    /// Concatenate a string literal's text parts, rejecting interpolation.
    fn plain_string(&mut self, parts: &[StrPart], span: Span, ctx: &str) -> PResult<String> {
        let mut s = String::new();
        for part in parts {
            match part {
                StrPart::Text(t) => s.push_str(t),
                StrPart::Interp(_) => {
                    self.error_at(span, format!("a {ctx} may not use string interpolation"));
                    return Err(ParseError);
                }
            }
        }
        Ok(s)
    }
}

/// Whether an expression is a valid assignment target (`=` or `op=` left side).
fn is_assignable(kind: &ExprKind) -> bool {
    matches!(kind, ExprKind::Var(_) | ExprKind::Index { .. } | ExprKind::Get { .. })
}

/// Map a compound-assignment token (`+=`, …) to its arithmetic operator.
fn compound_op(kind: &K) -> Option<BinaryOp> {
    Some(match kind {
        K::PlusEq => BinaryOp::Add,
        K::MinusEq => BinaryOp::Sub,
        K::StarEq => BinaryOp::Mul,
        K::SlashEq => BinaryOp::Div,
        K::PercentEq => BinaryOp::Rem,
        _ => return None,
    })
}

/// Derive the binding name for a bare `import "path";` — the basename without a
/// directory prefix or `.lum` extension.
fn module_basename(path: &str) -> String {
    let after_slash = path.rsplit(['/', '\\']).next().unwrap_or(path);
    after_slash.strip_suffix(".lum").unwrap_or(after_slash).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn parse_ok(src: &str) -> Program {
        let (tokens, lex_errs) = lex(src);
        assert!(lex_errs.is_empty(), "lex errors: {lex_errs:?}");
        let (program, errs) = parse(tokens);
        assert!(errs.is_empty(), "parse errors: {errs:?}");
        program
    }

    fn parse_errs(src: &str) -> Vec<Diagnostic> {
        let (tokens, _) = lex(src);
        let (_program, errs) = parse(tokens);
        errs
    }

    #[test]
    fn let_and_const() {
        let p = parse_ok("let x = 1; const Y = 2; let z;");
        assert_eq!(p.items.len(), 3);
        assert!(matches!(p.items[0], Stmt::Let { .. }));
        assert!(matches!(p.items[1], Stmt::Const { .. }));
        assert!(matches!(&p.items[2], Stmt::Let { init: None, .. }));
    }

    #[test]
    fn precedence_is_correct() {
        // 1 + 2 * 3 should be 1 + (2 * 3).
        let p = parse_ok("1 + 2 * 3;");
        if let Stmt::Expr { expr, .. } = &p.items[0] {
            if let ExprKind::Binary { op: BinaryOp::Add, right, .. } = &expr.kind {
                assert!(matches!(right.kind, ExprKind::Binary { op: BinaryOp::Mul, .. }));
            } else {
                panic!("expected top-level Add, got {:?}", expr.kind);
            }
        } else {
            panic!("expected expr stmt");
        }
    }

    #[test]
    fn comparison_below_arithmetic() {
        // a + b < c * d  =>  (a + b) < (c * d)
        let p = parse_ok("a + b < c * d;");
        if let Stmt::Expr { expr, .. } = &p.items[0] {
            assert!(matches!(expr.kind, ExprKind::Binary { op: BinaryOp::Lt, .. }));
        } else {
            panic!();
        }
    }

    #[test]
    fn right_assoc_assignment() {
        let p = parse_ok("a = b = 3;");
        if let Stmt::Expr { expr, .. } = &p.items[0] {
            if let ExprKind::Assign { value, .. } = &expr.kind {
                assert!(matches!(value.kind, ExprKind::Assign { .. }));
            } else {
                panic!("expected assign");
            }
        } else {
            panic!();
        }
    }

    #[test]
    fn unary_and_logical() {
        parse_ok("let x = !a && -b || not c;");
    }

    #[test]
    fn calls_indexing_members_chain() {
        let p = parse_ok("a.b(c)[d].e;");
        // Outermost is a Get (.e).
        if let Stmt::Expr { expr, .. } = &p.items[0] {
            assert!(matches!(expr.kind, ExprKind::Get { .. }));
        } else {
            panic!();
        }
    }

    #[test]
    fn control_flow_forms() {
        parse_ok(
            "if a { b(); } else if c { d(); } else { e(); }
             while x { x = x - 1; }
             for let i = 0; i < 10; i = i + 1 { print(i); }
             for item in items { use(item); }",
        );
    }

    #[test]
    fn functions_classes_modules() {
        parse_ok(
            "fn add(a, b) { return a + b; }
             class Dog < Animal { init(n) { this.name = n; } speak() { return super.speak(); } }
             import \"math\" as m;
             import \"x\".{a, b};
             export fn pub() { return 1; }",
        );
    }

    #[test]
    fn collections_and_interpolation() {
        parse_ok(r#"let a = [1, 2, ..rest, 3]; let m = {x: 1, "y": 2, [k]: 3}; let s = "hi ${name}!";"#);
    }

    #[test]
    fn try_throw_match() {
        parse_ok(
            r#"try { throw "x"; } catch (e) { print(e); } finally { cleanup(); }
               let r = match v { 0 => "z", [a, ..rest] => "arr", {k: x} => "map", n if n > 0 => "pos", _ => "other" };"#,
        );
    }

    #[test]
    fn string_with_no_interp_is_plain() {
        let p = parse_ok(r#"let s = "plain";"#);
        if let Stmt::Let { init: Some(e), .. } = &p.items[0] {
            assert!(matches!(e.kind, ExprKind::Str(_)));
        } else {
            panic!();
        }
    }

    #[test]
    fn statement_brace_is_block_not_map() {
        // `{ ... }` at statement position is a block.
        let p = parse_ok("{ let x = 1; }");
        assert!(matches!(p.items[0], Stmt::Block(_)));
    }

    #[test]
    fn condition_starting_with_map_literal() {
        // No parens needed: `{a: 1}` is the condition (a map), then the block.
        parse_ok(r#"if {"a": 1} { print("yes"); }"#);
    }

    #[test]
    fn error_missing_semicolon_reports_and_recovers() {
        let errs = parse_errs("let x = 1 let y = 2;");
        assert!(!errs.is_empty());
        assert!(errs[0].message.contains("expected ';'"));
    }

    #[test]
    fn error_reports_multiple() {
        // Two separate broken statements should both be reported (recovery).
        let errs = parse_errs("let = 1; let y 2;");
        assert!(errs.len() >= 2, "expected >=2 errors, got {}", errs.len());
    }

    #[test]
    fn error_invalid_assignment_target() {
        let errs = parse_errs("1 + 2 = 3;");
        assert!(errs.iter().any(|e| e.message.contains("invalid assignment target")));
    }

    #[test]
    fn module_basename_strips_path_and_ext() {
        assert_eq!(module_basename("math"), "math");
        assert_eq!(module_basename("lib/geometry.lum"), "geometry");
        assert_eq!(module_basename("./a/b/c.lum"), "c");
    }
}
