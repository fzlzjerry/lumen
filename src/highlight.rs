//! ANSI syntax highlighting for Lumen source.
//!
//! [`highlight`] re-lexes the source and wraps each token's exact source slice
//! in a color escape, leaving the gaps (whitespace and comments) verbatim. It is
//! used by the REPL to echo assembled multi-line input in color and by the
//! `:hl` command. Because it works off real token spans it never mangles the
//! text — worst case (a lexical error) it just leaves a span uncolored.

use crate::lexer::lex;
use crate::token::TokenKind;

const RESET: &str = "\x1b[0m";
const KEYWORD: &str = "\x1b[35m"; // magenta
const LITERAL: &str = "\x1b[33m"; // yellow (true/false/nil)
const STRING: &str = "\x1b[32m"; // green
const NUMBER: &str = "\x1b[36m"; // cyan
const OPERATOR: &str = "\x1b[37m"; // light gray

/// Return `src` with ANSI color escapes around its tokens.
pub fn highlight(src: &str) -> String {
    let (tokens, _errs) = lex(src);
    let mut out = String::with_capacity(src.len() + 32);
    let mut last = 0usize;
    for tok in &tokens {
        if tok.is_eof() {
            break;
        }
        let start = tok.span.offset as usize;
        let end = tok.span.end() as usize;
        if start > last && start <= src.len() {
            out.push_str(&src[last..start]); // whitespace/comments verbatim
        }
        if end > src.len() {
            break;
        }
        let slice = &src[start..end];
        match color_for(&tok.kind) {
            Some(color) => {
                out.push_str(color);
                out.push_str(slice);
                out.push_str(RESET);
            }
            None => out.push_str(slice),
        }
        last = end;
    }
    if last < src.len() {
        out.push_str(&src[last..]);
    }
    out
}

fn color_for(kind: &TokenKind) -> Option<&'static str> {
    use TokenKind::*;
    match kind {
        Int(_) | Float(_) => Some(NUMBER),
        Str(_) => Some(STRING),
        True | False | Nil => Some(LITERAL),
        Ident(_) => None,
        // Operators and punctuation.
        Plus | Minus | Star | Slash | Percent | Eq | EqEq | BangEq | Lt | LtEq | Gt | GtEq
        | AmpAmp | PipePipe | Bang | Dot | DotDot | Comma | Semicolon | Colon | LParen | RParen
        | LBracket | RBracket | LBrace | RBrace | FatArrow => Some(OPERATOR),
        Eof => None,
        // Everything else is a keyword.
        _ => Some(KEYWORD),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_keywords_and_preserves_text() {
        let src = "let x = 1;";
        let h = highlight(src);
        // Stripping all ANSI escapes recovers the original source.
        let stripped = strip_ansi(&h);
        assert_eq!(stripped, src);
        assert!(h.contains(KEYWORD)); // `let`
        assert!(h.contains(NUMBER)); // `1`
    }

    #[test]
    fn preserves_comments_and_whitespace() {
        let src = "fn f() { // hi\n  return 1;\n}";
        assert_eq!(strip_ansi(&highlight(src)), src);
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // skip until 'm'
                for c2 in chars.by_ref() {
                    if c2 == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}
