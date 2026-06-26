//! Lumen — a dynamic, bytecode-compiled programming language.
//!
//! The crate is organised as a classic compiler pipeline:
//!
//! ```text
//! source ─▶ Lexer ─▶ Parser ─▶ Resolver ─▶ Compiler ─▶ Chunk ─▶ VM(+GC)
//! ```
//!
//! Each stage lives in its own module and is independently testable. This file
//! wires them together and re-exports the handful of types that callers (the
//! `lumen` binary, integration tests) need.

pub mod ast;
pub mod ast_printer;
pub mod builtins;
pub mod chunk;
pub mod compiler;
pub mod debugger;
pub mod diagnostics;
pub mod disassembler;
pub mod fxhash;
pub mod gc;
pub mod highlight;
pub mod lexer;
pub mod lineedit;
pub mod lsp;
pub mod object;
pub mod opcode;
pub mod parser;
pub mod project;
pub mod repl;
pub mod resolver;
pub mod span;
pub mod stdlib;
pub mod token;
pub mod util;
pub mod value;
pub mod vm;

pub use diagnostics::{Diagnostic, Severity};
pub use lexer::{lex, Lexer};
pub use parser::parse;
pub use span::Span;
pub use token::{StrPart, Token, TokenKind};

pub use resolver::resolve;

/// Lex then parse `src` into an AST, returning all lexical and syntax errors
/// together (lexical first). A non-empty error list means the program is
/// malformed and must not be executed.
pub fn parse_source(src: &str) -> (ast::Program, Vec<Diagnostic>) {
    let (tokens, mut errors) = lex(src);
    let (program, parse_errors) = parse(tokens);
    errors.extend(parse_errors);
    (program, errors)
}

/// Front-end of the compiler: lex, parse, and resolve. Returns the AST together
/// with every static error from all three stages. Later phases (compile, run)
/// should refuse to proceed when the error list is non-empty. If lexing or
/// parsing fails, resolution is skipped (it would only produce noise on a
/// malformed tree).
pub fn check_source(src: &str) -> (ast::Program, Vec<Diagnostic>) {
    let (program, mut errors) = parse_source(src);
    if errors.is_empty() {
        errors.extend(resolve(&program));
    }
    (program, errors)
}
