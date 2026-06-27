//! The abstract syntax tree.
//!
//! Every node carries a [`Span`] so later stages (resolver, compiler) can attach
//! diagnostics and line numbers. The tree is produced by the parser and consumed
//! by the AST printer, the resolver, and the compiler. It is intentionally a
//! plain data structure with no behavior beyond construction.

use crate::span::Span;

/// A whole parsed program: a sequence of top-level declarations/statements.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub items: Vec<Stmt>,
}

/// A braced sequence of declarations, used for function/loop/`if` bodies and as
/// a standalone block statement.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

/// A function or method definition. `name` is `None` for anonymous lambdas.
#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: Option<String>,
    pub name_span: Span,
    pub params: Vec<Param>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub span: Span,
    /// A default value (`fn f(x = 10)`), used when the argument is omitted.
    pub default: Option<Expr>,
    /// A rest parameter (`fn f(..args)`) collecting trailing arguments into an
    /// array. At most one, and it must be last.
    pub is_rest: bool,
}

/// A class declaration with an optional single superclass and its methods.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassDecl {
    pub name: String,
    pub name_span: Span,
    pub superclass: Option<Spanned<String>>,
    pub methods: Vec<Function>,
    pub span: Span,
}

/// How an `import` binds names into the current scope.
#[derive(Clone, Debug, PartialEq)]
pub enum ImportKind {
    /// `import "m";` (alias = basename of the path) or `import "m" as alias;`.
    Module { alias: Spanned<String> },
    /// `import "m".{a, b};` — bind the named exports directly.
    Named(Vec<Spanned<String>>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub path: String,
    pub path_span: Span,
    pub kind: ImportKind,
    pub span: Span,
}

/// A value paired with the span it came from (handy for names/keys).
#[derive(Clone, Debug, PartialEq)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Span) -> Self {
        Spanned { value, span }
    }
}

/// A statement or declaration.
#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Let {
        name: String,
        name_span: Span,
        init: Option<Expr>,
        span: Span,
    },
    Const {
        name: String,
        name_span: Span,
        init: Expr,
        span: Span,
    },
    /// `let <pattern> = init;` — bind the variables in an array/map pattern from
    /// the destructured value.
    Destructure {
        pattern: Pattern,
        init: Expr,
        span: Span,
    },
    /// `<pattern> = value;` — assign the variables of an array/map pattern from
    /// the destructured value (the targets must already exist and be mutable).
    DestructureAssign {
        pattern: Pattern,
        value: Expr,
        span: Span,
    },
    Function(Function),
    Class(ClassDecl),
    Import(Import),
    /// `export <decl>` — wraps a `Let`/`Const`/`Function`/`Class`.
    Export {
        decl: Box<Stmt>,
        span: Span,
    },
    Expr {
        expr: Expr,
        span: Span,
    },
    Block(Block),
    If {
        cond: Expr,
        then_block: Block,
        /// `else` branch: another `If` (for `else if`) or a `Block`, if present.
        else_branch: Option<Box<Stmt>>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Block,
        span: Span,
    },
    ForIn {
        var: String,
        var_span: Span,
        iter: Expr,
        body: Block,
        span: Span,
    },
    ForC {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Block,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    Break {
        span: Span,
    },
    Continue {
        span: Span,
    },
    Throw {
        value: Expr,
        span: Span,
    },
    Try {
        body: Block,
        /// The `catch (e) { ... }` clause, if present. At least one of `catch`
        /// or `finally` is always present (the parser enforces this).
        catch: Option<CatchClause>,
        finally: Option<Block>,
        span: Span,
    },
}

/// A `catch (name) { body }` clause.
#[derive(Clone, Debug, PartialEq)]
pub struct CatchClause {
    pub name: String,
    pub name_span: Span,
    pub body: Block,
}

impl Stmt {
    /// The source span of this statement.
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let { span, .. }
            | Stmt::Const { span, .. }
            | Stmt::Destructure { span, .. }
            | Stmt::DestructureAssign { span, .. }
            | Stmt::Export { span, .. }
            | Stmt::Expr { span, .. }
            | Stmt::If { span, .. }
            | Stmt::While { span, .. }
            | Stmt::ForIn { span, .. }
            | Stmt::ForC { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::Break { span }
            | Stmt::Continue { span }
            | Stmt::Throw { span, .. }
            | Stmt::Try { span, .. } => *span,
            Stmt::Function(f) => f.span,
            Stmt::Class(c) => c.span,
            Stmt::Import(i) => i.span,
            Stmt::Block(b) => b.span,
        }
    }
}

/// An expression node.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Expr { kind, span }
    }
}

/// One segment of an interpolated string: literal text or an embedded expression.
#[derive(Clone, Debug, PartialEq)]
pub enum StrSegment {
    Text(String),
    Expr(Expr),
}

/// An element of an array literal: a plain value or an `..spread`.
#[derive(Clone, Debug, PartialEq)]
pub enum ArrayElem {
    Item(Expr),
    Spread(Expr),
}

/// An argument in a call: a plain value or an `..spread` that unpacks an
/// iterable into the argument list (mirror of [`ArrayElem`]).
#[derive(Clone, Debug, PartialEq)]
pub enum CallArg {
    Item(Expr),
    Spread(Expr),
}

/// A key in a map literal.
#[derive(Clone, Debug, PartialEq)]
pub enum MapKey {
    /// A string key — produced by both `ident:` and `"string":` forms.
    Str(String),
    /// A computed `[expr]:` key.
    Computed(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    /// `~x` — bitwise complement (integer only).
    BitNot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `x is Class` — instance-of test (true iff `x` is an instance whose class
    /// is `Class` or a subclass of it).
    Is,
    /// Integer-only bitwise / shift operators.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicalOp {
    And,
    Or,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    /// A string with no interpolation.
    Str(String),
    /// A string with `${...}` interpolation: a list of text/expr segments.
    StrInterp(Vec<StrSegment>),
    Bool(bool),
    Nil,
    Array(Vec<ArrayElem>),
    Map(Vec<(MapKey, Expr)>),
    /// An identifier reference.
    Var(String),
    This,
    Super {
        method: String,
        method_span: Span,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// `target op= value` (e.g. `x += 1`). The target is evaluated once.
    CompoundAssign {
        target: Box<Expr>,
        op: BinaryOp,
        value: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Logical {
        op: LogicalOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// `cond ? then_branch : else_branch` — only the taken branch is evaluated.
    Ternary {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<CallArg>,
        /// Span of the closing paren, for "wrong number of args" diagnostics.
        paren_span: Span,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    Get {
        object: Box<Expr>,
        name: String,
        name_span: Span,
    },
    Lambda(Function),
    Match {
        subject: Box<Expr>,
        arms: Vec<MatchArm>,
    },
}

/// One arm of a `match` expression.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

/// A pattern in a `match` arm.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PatternKind {
    /// `_` — matches anything, binds nothing.
    Wildcard,
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Nil,
    /// A bare identifier — matches anything and binds it.
    Binding(String),
    /// `[p, q, ..rest]` — fixed elements with at most one rest.
    Array(Vec<PatElem>),
    /// `{key: pat, ...}` — match by key, recursively.
    Map(Vec<(String, Pattern)>),
    /// `p1 | p2 | ...` — matches if any alternative matches. Alternatives may not
    /// bind variables (DESIGN D25).
    Or(Vec<Pattern>),
}

/// An element of an array pattern.
#[derive(Clone, Debug, PartialEq)]
pub enum PatElem {
    Pattern(Pattern),
    /// `..` or `..name` — zero-or-more, optionally bound.
    Rest(Option<String>),
}
