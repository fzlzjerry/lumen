//! Tokens — the output of the lexer and the input to the parser.
//!
//! A [`Token`] is a [`TokenKind`] plus the [`Span`] it occupies in the source.
//! String literals are special: because Lumen supports `${expr}` interpolation,
//! a string is lexed into a sequence of [`StrPart`]s — literal text runs and
//! already-tokenized interpolation expressions — so the parser can rebuild the
//! string as a concatenation without re-scanning characters.

use crate::span::Span;
use std::fmt;

/// A lexed token: what it is, and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Token { kind, span }
    }

    /// Is this the end-of-input marker?
    pub fn is_eof(&self) -> bool {
        matches!(self.kind, TokenKind::Eof)
    }
}

/// One piece of a string literal: either a literal run of text, or an
/// interpolation whose inner expression has already been tokenized (terminated
/// by its own `Eof` so a sub-parser knows where to stop).
#[derive(Clone, Debug, PartialEq)]
pub enum StrPart {
    Text(String),
    Interp(Vec<Token>),
}

/// The lexical category of a token.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    // ---- Literals ----
    Int(i64),
    Float(f64),
    Str(Vec<StrPart>),
    Ident(String),

    // ---- Keywords ----
    And,
    Break,
    Catch,
    Class,
    Const,
    Continue,
    Else,
    Export,
    False,
    Finally,
    Fn,
    For,
    If,
    Import,
    In,
    Is,
    Let,
    Match,
    Nil,
    Not,
    Or,
    Return,
    Super,
    This,
    Throw,
    True,
    Try,
    While,
    Yield,

    // ---- Operators & punctuation ----
    Plus,       // +
    Minus,      // -
    Star,       // *
    StarStar,   // ** (exponentiation)
    Slash,      // /
    Percent,    // %
    PlusEq,     // +=
    MinusEq,    // -=
    StarEq,     // *=
    SlashEq,    // /=
    PercentEq,  // %=
    Eq,         // =
    EqEq,       // ==
    BangEq,     // !=
    Lt,         // <
    LtEq,       // <=
    Gt,         // >
    GtEq,       // >=
    AmpAmp,     // &&
    PipePipe,   // ||
    Amp,        // &  (bitwise and)
    Pipe,       // |  (bitwise or)
    Caret,      // ^  (bitwise xor)
    Tilde,      // ~  (bitwise not)
    Shl,        // << (shift left)
    Shr,        // >> (shift right)
    Bang,       // !
    Dot,        // .
    DotDot,     // ..
    Comma,      // ,
    Semicolon,  // ;
    Colon,      // :
    Question,   // ?
    LParen,     // (
    RParen,     // )
    LBracket,   // [
    RBracket,   // ]
    LBrace,     // {
    RBrace,     // }
    FatArrow,   // =>

    /// End of input.
    Eof,
}

impl TokenKind {
    /// If `ident` is a reserved keyword, return its token kind; otherwise `None`
    /// (the caller makes it an [`TokenKind::Ident`]). `true`/`false`/`nil` are
    /// handled here too even though they are conceptually literals.
    pub fn keyword(ident: &str) -> Option<TokenKind> {
        Some(match ident {
            "and" => TokenKind::And,
            "break" => TokenKind::Break,
            "catch" => TokenKind::Catch,
            "class" => TokenKind::Class,
            "const" => TokenKind::Const,
            "continue" => TokenKind::Continue,
            "else" => TokenKind::Else,
            "export" => TokenKind::Export,
            "false" => TokenKind::False,
            "finally" => TokenKind::Finally,
            "fn" => TokenKind::Fn,
            "for" => TokenKind::For,
            "if" => TokenKind::If,
            "import" => TokenKind::Import,
            "in" => TokenKind::In,
            "is" => TokenKind::Is,
            "let" => TokenKind::Let,
            "match" => TokenKind::Match,
            "nil" => TokenKind::Nil,
            "not" => TokenKind::Not,
            "or" => TokenKind::Or,
            "return" => TokenKind::Return,
            "super" => TokenKind::Super,
            "this" => TokenKind::This,
            "throw" => TokenKind::Throw,
            "true" => TokenKind::True,
            "try" => TokenKind::Try,
            "while" => TokenKind::While,
            "yield" => TokenKind::Yield,
            _ => return None,
        })
    }

    /// A short human label for this kind, used in parser error messages
    /// (`expected ';'`). For literals it names the category.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Int(_) => "integer".into(),
            TokenKind::Float(_) => "float".into(),
            TokenKind::Str(_) => "string".into(),
            TokenKind::Ident(name) => format!("identifier '{name}'"),
            TokenKind::Eof => "end of input".into(),
            other => format!("'{}'", other.lexeme()),
        }
    }

    /// The canonical spelling of a fixed (non-literal) token. Literals return a
    /// placeholder; use [`describe`](Self::describe) for those.
    pub fn lexeme(&self) -> &'static str {
        match self {
            TokenKind::Int(_) => "<int>",
            TokenKind::Float(_) => "<float>",
            TokenKind::Str(_) => "<string>",
            TokenKind::Ident(_) => "<identifier>",
            TokenKind::And => "and",
            TokenKind::Break => "break",
            TokenKind::Catch => "catch",
            TokenKind::Class => "class",
            TokenKind::Const => "const",
            TokenKind::Continue => "continue",
            TokenKind::Else => "else",
            TokenKind::Export => "export",
            TokenKind::False => "false",
            TokenKind::Finally => "finally",
            TokenKind::Fn => "fn",
            TokenKind::For => "for",
            TokenKind::If => "if",
            TokenKind::Import => "import",
            TokenKind::In => "in",
            TokenKind::Is => "is",
            TokenKind::Let => "let",
            TokenKind::Match => "match",
            TokenKind::Nil => "nil",
            TokenKind::Not => "not",
            TokenKind::Or => "or",
            TokenKind::Return => "return",
            TokenKind::Super => "super",
            TokenKind::This => "this",
            TokenKind::Throw => "throw",
            TokenKind::True => "true",
            TokenKind::Try => "try",
            TokenKind::While => "while",
            TokenKind::Yield => "yield",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::StarStar => "**",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::PlusEq => "+=",
            TokenKind::MinusEq => "-=",
            TokenKind::StarEq => "*=",
            TokenKind::SlashEq => "/=",
            TokenKind::PercentEq => "%=",
            TokenKind::Eq => "=",
            TokenKind::EqEq => "==",
            TokenKind::BangEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::LtEq => "<=",
            TokenKind::Gt => ">",
            TokenKind::GtEq => ">=",
            TokenKind::AmpAmp => "&&",
            TokenKind::PipePipe => "||",
            TokenKind::Amp => "&",
            TokenKind::Pipe => "|",
            TokenKind::Caret => "^",
            TokenKind::Tilde => "~",
            TokenKind::Shl => "<<",
            TokenKind::Shr => ">>",
            TokenKind::Bang => "!",
            TokenKind::Dot => ".",
            TokenKind::DotDot => "..",
            TokenKind::Comma => ",",
            TokenKind::Semicolon => ";",
            TokenKind::Colon => ":",
            TokenKind::Question => "?",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::FatArrow => "=>",
            TokenKind::Eof => "<eof>",
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.describe())
    }
}
