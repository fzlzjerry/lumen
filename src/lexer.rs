//! The hand-written lexer.
//!
//! [`Lexer::run`] turns a source string into a `Vec<Token>` (always terminated
//! by an `Eof` token) plus a `Vec<Diagnostic>` of any lexical errors. The lexer
//! never panics and never stops early: on an error it records a diagnostic and
//! resynchronizes (usually by skipping the offending character) so a single run
//! surfaces as many problems as it can.
//!
//! The one piece of real cleverness is string interpolation. When the lexer hits
//! `${`, it lexes the inner expression *in place* by recursively calling
//! [`Lexer::next_token`] while balancing braces, so nested maps, nested strings,
//! and nested interpolations all work and every inner token gets a correct span.

use crate::diagnostics::Diagnostic;
use crate::span::Span;
use crate::token::{StrPart, Token, TokenKind};

pub struct Lexer<'src> {
    src: &'src str,
    /// `(byte offset, char)` for every scalar in the source, in order. Indexing
    /// this gives us O(1) lookahead with both the character and its byte offset
    /// (needed to build spans) without re-walking the UTF-8.
    chars: Vec<(u32, char)>,
    /// Index into `chars` of the next character to consume.
    pos: usize,
    line: u32,
    col: u32,
    errors: Vec<Diagnostic>,
}

/// Lex `src` into tokens. Convenience wrapper over [`Lexer::run`].
pub fn lex(src: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    Lexer::new(src).run()
}

impl<'src> Lexer<'src> {
    pub fn new(src: &'src str) -> Self {
        let chars = src.char_indices().map(|(i, c)| (i as u32, c)).collect();
        Lexer { src, chars, pos: 0, line: 1, col: 1, errors: Vec::new() }
    }

    /// Drive the lexer to completion.
    pub fn run(mut self) -> (Vec<Token>, Vec<Diagnostic>) {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            let done = tok.is_eof();
            tokens.push(tok);
            if done {
                break;
            }
        }
        (tokens, self.errors)
    }

    // ---- cursor primitives -------------------------------------------------

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).map(|&(_, c)| c)
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).map(|&(_, c)| c)
    }

    fn peek3(&self) -> Option<char> {
        self.chars.get(self.pos + 2).map(|&(_, c)| c)
    }

    /// Byte offset of the next character (or end-of-source).
    fn offset(&self) -> u32 {
        self.chars.get(self.pos).map(|&(o, _)| o).unwrap_or(self.src.len() as u32)
    }

    /// Consume and return the current character, advancing line/column.
    fn advance(&mut self) -> char {
        let (_, c) = self.chars[self.pos];
        self.pos += 1;
        match c {
            '\n' => {
                self.line += 1;
                self.col = 1;
            }
            '\r' => {
                if self.peek() == Some('\n') {
                    // CRLF: count the CR as a column; the following LF bumps the
                    // line so we don't double-count.
                    self.col += 1;
                } else {
                    // Lone CR is a line terminator (§1).
                    self.line += 1;
                    self.col = 1;
                }
            }
            _ => self.col += 1,
        }
        c
    }

    fn match_char(&mut self, want: char) -> bool {
        if self.peek() == Some(want) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn span_from(&self, start: u32, line: u32, col: u32) -> Span {
        Span::new(start, self.offset() - start, line, col)
    }

    fn error(&mut self, message: impl Into<String>, span: Span) {
        self.errors.push(Diagnostic::error("lexer", message, span));
    }

    /// Consume characters while `pred` holds, returning the consumed text.
    fn collect_while(&mut self, pred: impl Fn(char) -> bool) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if pred(c) {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    // ---- trivia ------------------------------------------------------------

    /// Skip whitespace, line comments, and (nestable) block comments. A `/` that
    /// does not begin a comment is left for [`next_token`] to read as `Slash`.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(' ') | Some('\t') | Some('\r') | Some('\n') => {
                    self.advance();
                }
                Some('/') if self.peek2() == Some('/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                Some('/') if self.peek2() == Some('*') => self.block_comment(),
                _ => break,
            }
        }
    }

    /// Consume a `/* ... */` block comment, honoring nesting.
    fn block_comment(&mut self) {
        let start = self.offset();
        let (line, col) = (self.line, self.col);
        self.advance(); // /
        self.advance(); // *
        let mut depth = 1u32;
        while depth > 0 {
            match self.peek() {
                None => {
                    self.error(
                        "unterminated block comment",
                        Span::new(start, 2, line, col),
                    );
                    return;
                }
                Some('/') if self.peek2() == Some('*') => {
                    self.advance();
                    self.advance();
                    depth += 1;
                }
                Some('*') if self.peek2() == Some('/') => {
                    self.advance();
                    self.advance();
                    depth -= 1;
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    // ---- the main dispatch -------------------------------------------------

    fn next_token(&mut self) -> Token {
        loop {
            self.skip_trivia();
            let start = self.offset();
            let (line, col) = (self.line, self.col);
            let c = match self.peek() {
                None => return Token::new(TokenKind::Eof, Span::new(start, 0, line, col)),
                Some(c) => c,
            };

            if c == '"' {
                return self.scan_string();
            }
            if c.is_ascii_digit() {
                return self.scan_number();
            }
            if is_ident_start(c) {
                return self.scan_ident();
            }

            self.advance();
            let kind = match c {
                '(' => TokenKind::LParen,
                ')' => TokenKind::RParen,
                '[' => TokenKind::LBracket,
                ']' => TokenKind::RBracket,
                '{' => TokenKind::LBrace,
                '}' => TokenKind::RBrace,
                ',' => TokenKind::Comma,
                ';' => TokenKind::Semicolon,
                ':' => TokenKind::Colon,
                '?' => TokenKind::Question,
                '+' => if self.match_char('=') { TokenKind::PlusEq } else { TokenKind::Plus },
                '-' => if self.match_char('=') { TokenKind::MinusEq } else { TokenKind::Minus },
                '*' => {
                    if self.match_char('*') {
                        TokenKind::StarStar
                    } else if self.match_char('=') {
                        TokenKind::StarEq
                    } else {
                        TokenKind::Star
                    }
                }
                '%' => if self.match_char('=') { TokenKind::PercentEq } else { TokenKind::Percent },
                // comments already consumed by skip_trivia
                '/' => if self.match_char('=') { TokenKind::SlashEq } else { TokenKind::Slash },
                '.' => {
                    if self.match_char('.') {
                        TokenKind::DotDot
                    } else {
                        TokenKind::Dot
                    }
                }
                '=' => {
                    if self.match_char('=') {
                        TokenKind::EqEq
                    } else if self.match_char('>') {
                        TokenKind::FatArrow
                    } else {
                        TokenKind::Eq
                    }
                }
                '!' => {
                    if self.match_char('=') {
                        TokenKind::BangEq
                    } else {
                        TokenKind::Bang
                    }
                }
                '<' => {
                    if self.match_char('=') {
                        TokenKind::LtEq
                    } else if self.match_char('<') {
                        TokenKind::Shl
                    } else {
                        TokenKind::Lt
                    }
                }
                '>' => {
                    if self.match_char('=') {
                        TokenKind::GtEq
                    } else if self.match_char('>') {
                        TokenKind::Shr
                    } else {
                        TokenKind::Gt
                    }
                }
                '&' => {
                    if self.match_char('&') {
                        TokenKind::AmpAmp
                    } else {
                        TokenKind::Amp
                    }
                }
                '|' => {
                    if self.match_char('|') {
                        TokenKind::PipePipe
                    } else {
                        TokenKind::Pipe
                    }
                }
                '^' => TokenKind::Caret,
                '~' => TokenKind::Tilde,
                other => {
                    self.error(
                        format!("unexpected character '{}'", other.escape_default()),
                        Span::new(start, 1, line, col),
                    );
                    continue;
                }
            };
            return Token::new(kind, self.span_from(start, line, col));
        }
    }

    // ---- identifiers & keywords -------------------------------------------

    fn scan_ident(&mut self) -> Token {
        let start = self.offset();
        let (line, col) = (self.line, self.col);
        let name = self.collect_while(is_ident_continue);
        let span = self.span_from(start, line, col);
        let kind = TokenKind::keyword(&name).unwrap_or(TokenKind::Ident(name));
        Token::new(kind, span)
    }

    // ---- numbers -----------------------------------------------------------

    fn scan_number(&mut self) -> Token {
        let start = self.offset();
        let (line, col) = (self.line, self.col);

        // Radix prefixes.
        if self.peek() == Some('0') && matches!(self.peek2(), Some('x') | Some('X')) {
            self.advance();
            self.advance();
            let digits = self.collect_while(|c| c.is_ascii_hexdigit() || c == '_');
            return self.finish_radix(&digits, 16, "hexadecimal", start, line, col);
        }
        if self.peek() == Some('0') && matches!(self.peek2(), Some('b') | Some('B')) {
            self.advance();
            self.advance();
            let digits = self.collect_while(|c| c == '0' || c == '1' || c == '_');
            return self.finish_radix(&digits, 2, "binary", start, line, col);
        }
        if self.peek() == Some('0') && matches!(self.peek2(), Some('o') | Some('O')) {
            self.advance();
            self.advance();
            let digits = self.collect_while(|c| matches!(c, '0'..='7' | '_'));
            return self.finish_radix(&digits, 8, "octal", start, line, col);
        }

        let mut lexeme = self.collect_while(|c| c.is_ascii_digit() || c == '_');
        let mut is_float = false;

        // Fractional part — only if a digit follows the dot, so `1.` and `1..2`
        // stay as integer-then-operator.
        if self.peek() == Some('.') && self.peek2().is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            lexeme.push(self.advance()); // '.'
            lexeme.push_str(&self.collect_while(|c| c.is_ascii_digit() || c == '_'));
        }

        // Exponent — only if a (optionally signed) digit follows.
        if matches!(self.peek(), Some('e') | Some('E')) {
            let has_sign = matches!(self.peek2(), Some('+') | Some('-'));
            let exp_digit = if has_sign {
                self.peek3().is_some_and(|c| c.is_ascii_digit())
            } else {
                self.peek2().is_some_and(|c| c.is_ascii_digit())
            };
            if exp_digit {
                is_float = true;
                lexeme.push(self.advance()); // e / E
                if matches!(self.peek(), Some('+') | Some('-')) {
                    lexeme.push(self.advance());
                }
                lexeme.push_str(&self.collect_while(|c| c.is_ascii_digit() || c == '_'));
            }
        }

        let span = self.span_from(start, line, col);
        let clean: String = lexeme.chars().filter(|&c| c != '_').collect();
        let kind = if is_float {
            match clean.parse::<f64>() {
                Ok(v) => TokenKind::Float(v),
                Err(_) => {
                    self.error(format!("malformed float literal '{lexeme}'"), span);
                    TokenKind::Float(0.0)
                }
            }
        } else {
            match clean.parse::<i64>() {
                Ok(v) => TokenKind::Int(v),
                Err(_) => {
                    self.error(
                        format!("integer literal '{lexeme}' does not fit in a 64-bit int"),
                        span,
                    );
                    TokenKind::Int(0)
                }
            }
        };
        Token::new(kind, span)
    }

    fn finish_radix(
        &mut self,
        digits: &str,
        radix: u32,
        name: &str,
        start: u32,
        line: u32,
        col: u32,
    ) -> Token {
        let span = self.span_from(start, line, col);
        let clean: String = digits.chars().filter(|&c| c != '_').collect();
        if clean.is_empty() {
            self.error(format!("{name} literal has no digits"), span);
            return Token::new(TokenKind::Int(0), span);
        }
        let kind = match i64::from_str_radix(&clean, radix) {
            Ok(v) => TokenKind::Int(v),
            Err(_) => {
                self.error(
                    format!("{name} literal does not fit in a 64-bit int"),
                    span,
                );
                TokenKind::Int(0)
            }
        };
        Token::new(kind, span)
    }

    // ---- strings & interpolation ------------------------------------------

    fn scan_string(&mut self) -> Token {
        let start = self.offset();
        let (line, col) = (self.line, self.col);
        self.advance(); // opening quote

        let mut parts: Vec<StrPart> = Vec::new();
        let mut text = String::new();

        loop {
            match self.peek() {
                None => {
                    self.error(
                        "unterminated string literal",
                        Span::new(start, 1, line, col),
                    );
                    break;
                }
                Some('"') => {
                    self.advance();
                    break;
                }
                Some('\\') => {
                    let (bs, bl, bc) = (self.offset(), self.line, self.col);
                    self.advance(); // backslash
                    self.scan_escape(&mut text, bs, bl, bc);
                }
                Some('$') if self.peek2() == Some('{') => {
                    if !text.is_empty() {
                        parts.push(StrPart::Text(std::mem::take(&mut text)));
                    }
                    self.advance(); // $
                    self.advance(); // {
                    let toks = self.scan_interpolation();
                    parts.push(StrPart::Interp(toks));
                }
                Some(c) => {
                    self.advance();
                    text.push(c);
                }
            }
        }
        if !text.is_empty() {
            parts.push(StrPart::Text(text));
        }
        Token::new(TokenKind::Str(parts), self.span_from(start, line, col))
    }

    fn scan_escape(&mut self, out: &mut String, bs: u32, bl: u32, bc: u32) {
        let c = match self.peek() {
            None => {
                self.error(
                    "unterminated escape at end of input",
                    Span::new(bs, 1, bl, bc),
                );
                return;
            }
            Some(c) => c,
        };
        self.advance();
        match c {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '0' => out.push('\0'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '$' => out.push('$'),
            'u' => self.scan_unicode_escape(out, bs, bl, bc),
            other => {
                self.error(
                    format!("unknown escape sequence '\\{}'", other.escape_default()),
                    self.span_from(bs, bl, bc),
                );
                out.push(other); // recover by taking it literally
            }
        }
    }

    fn scan_unicode_escape(&mut self, out: &mut String, bs: u32, bl: u32, bc: u32) {
        if !self.match_char('{') {
            self.error(
                "expected '{' after '\\u' in a unicode escape",
                self.span_from(bs, bl, bc),
            );
            return;
        }
        let hex = self.collect_while(|c| c.is_ascii_hexdigit());
        if !self.match_char('}') {
            self.error(
                "expected '}' to close '\\u{...}'",
                self.span_from(bs, bl, bc),
            );
            return;
        }
        if hex.is_empty() {
            self.error(
                "empty unicode escape '\\u{}'",
                self.span_from(bs, bl, bc),
            );
            return;
        }
        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
            Some(ch) => out.push(ch),
            None => self.error(
                format!("'\\u{{{hex}}}' is not a valid Unicode scalar value"),
                self.span_from(bs, bl, bc),
            ),
        }
    }

    /// Lex the expression inside a `${ ... }`, balancing braces. The opening
    /// `${` has already been consumed; we stop at the matching `}`. The returned
    /// vector ends with an `Eof` so a sub-parser knows where the expression ends.
    fn scan_interpolation(&mut self) -> Vec<Token> {
        let mut toks = Vec::new();
        let mut depth: i32 = 0;
        loop {
            let tok = self.next_token();
            match tok.kind {
                TokenKind::RBrace if depth == 0 => break, // the closing brace
                TokenKind::LBrace => {
                    depth += 1;
                    toks.push(tok);
                }
                TokenKind::RBrace => {
                    depth -= 1;
                    toks.push(tok);
                }
                TokenKind::Eof => {
                    self.error(
                        "unterminated string interpolation (missing '}')",
                        tok.span,
                    );
                    toks.push(tok);
                    return toks;
                }
                _ => toks.push(tok),
            }
        }
        toks.push(Token::new(
            TokenKind::Eof,
            Span::new(self.offset(), 0, self.line, self.col),
        ));
        toks
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::TokenKind::*;

    /// Lex and assert there were no errors, returning the token kinds without
    /// the trailing Eof.
    fn kinds(src: &str) -> Vec<TokenKind> {
        let (toks, errs) = lex(src);
        assert!(errs.is_empty(), "unexpected lex errors: {errs:?}");
        let mut ks: Vec<TokenKind> = toks.into_iter().map(|t| t.kind).collect();
        assert_eq!(ks.pop(), Some(Eof));
        ks
    }

    #[test]
    fn punctuation_and_operators() {
        assert_eq!(
            kinds("(){}[],;:.+-*/%"),
            vec![
                LParen, RParen, LBrace, RBrace, LBracket, RBracket, Comma, Semicolon, Colon,
                Dot, Plus, Minus, Star, Slash, Percent
            ]
        );
        assert_eq!(
            kinds("== != <= >= < > = => && || ! .."),
            vec![EqEq, BangEq, LtEq, GtEq, Lt, Gt, Eq, FatArrow, AmpAmp, PipePipe, Bang, DotDot]
        );
    }

    #[test]
    fn keywords_vs_identifiers() {
        assert_eq!(
            kinds("let const fn if while foo _bar baz123"),
            vec![
                Let,
                Const,
                Fn,
                If,
                While,
                Ident("foo".into()),
                Ident("_bar".into()),
                Ident("baz123".into())
            ]
        );
        assert_eq!(kinds("and or not"), vec![And, Or, Not]);
        assert_eq!(kinds("true false nil"), vec![True, False, Nil]);
    }

    #[test]
    fn integer_literals_all_radixes() {
        assert_eq!(kinds("0 42 1_000_000"), vec![Int(0), Int(42), Int(1_000_000)]);
        assert_eq!(kinds("0xFF 0x10 0xde_ad"), vec![Int(255), Int(16), Int(0xdead)]);
        assert_eq!(kinds("0b1010 0b1111_0000"), vec![Int(10), Int(0xF0)]);
        assert_eq!(kinds("0o17 0o755 0O10"), vec![Int(15), Int(493), Int(8)]);
    }

    #[test]
    fn float_literals() {
        assert_eq!(kinds("3.25"), vec![Float(3.25)]);
        assert_eq!(kinds("1.5e3"), vec![Float(1500.0)]);
        assert_eq!(kinds("1e9"), vec![Float(1e9)]);
        assert_eq!(kinds("2.0e-3"), vec![Float(0.002)]);
        // `1.` is int then dot; `1..2` is int dotdot int.
        assert_eq!(kinds("1 . 2"), vec![Int(1), Dot, Int(2)]);
        assert_eq!(kinds("1..2"), vec![Int(1), DotDot, Int(2)]);
    }

    #[test]
    fn simple_string() {
        let ks = kinds(r#""hello world""#);
        match &ks[0] {
            Str(parts) => assert_eq!(parts, &vec![StrPart::Text("hello world".into())]),
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn empty_string_has_no_parts() {
        let ks = kinds(r#""""#);
        assert_eq!(ks, vec![Str(vec![])]);
    }

    #[test]
    fn string_escapes() {
        let ks = kinds(r#""a\nb\t\"\$\\\u{41}""#);
        match &ks[0] {
            Str(parts) => {
                assert_eq!(parts, &vec![StrPart::Text("a\nb\t\"$\\A".into())]);
            }
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn string_interpolation_parts() {
        let ks = kinds(r#""sum is ${a + b}!""#);
        match &ks[0] {
            Str(parts) => {
                assert_eq!(parts.len(), 3);
                assert_eq!(parts[0], StrPart::Text("sum is ".into()));
                match &parts[1] {
                    StrPart::Interp(toks) => {
                        let kinds: Vec<&TokenKind> = toks.iter().map(|t| &t.kind).collect();
                        assert_eq!(
                            kinds,
                            vec![&Ident("a".into()), &Plus, &Ident("b".into()), &Eof]
                        );
                    }
                    other => panic!("expected interp, got {other:?}"),
                }
                assert_eq!(parts[2], StrPart::Text("!".into()));
            }
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn nested_interpolation_and_maps() {
        // Interpolation containing a map literal and a nested string with its
        // own interpolation. Should lex without errors and balance braces.
        let (_toks, errs) = lex(r#""outer ${ {x: 1}["x"] } and ${"inner ${y}"}""#);
        assert!(errs.is_empty(), "errors: {errs:?}");
    }

    #[test]
    fn nested_block_comments() {
        assert_eq!(kinds("1 /* a /* b */ c */ 2"), vec![Int(1), Int(2)]);
    }

    #[test]
    fn line_comments() {
        assert_eq!(kinds("1 // ignored\n2"), vec![Int(1), Int(2)]);
    }

    #[test]
    fn tracks_line_and_column() {
        let (toks, errs) = lex("a\n  bc");
        assert!(errs.is_empty());
        assert_eq!((toks[0].span.line, toks[0].span.col), (1, 1)); // a
        assert_eq!((toks[1].span.line, toks[1].span.col), (2, 3)); // bc
    }

    #[test]
    fn error_unterminated_string() {
        let (_toks, errs) = lex("\"oops");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("unterminated string"));
    }

    #[test]
    fn error_unterminated_block_comment() {
        let (_toks, errs) = lex("/* never closed");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("unterminated block comment"));
    }

    #[test]
    fn error_unknown_escape_recovers() {
        let (toks, errs) = lex(r#""a\qb""#);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("unknown escape"));
        // Still produces a string token (recovery), plus Eof.
        assert!(matches!(toks[0].kind, Str(_)));
    }

    #[test]
    fn error_unexpected_char_continues() {
        let (toks, errs) = lex("1 @ 2");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("unexpected character"));
        let ks: Vec<&TokenKind> = toks.iter().map(|t| &t.kind).collect();
        assert_eq!(ks, vec![&Int(1), &Int(2), &Eof]);
    }

    #[test]
    fn integer_overflow_is_reported() {
        let (_toks, errs) = lex("99999999999999999999999");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("does not fit"));
    }
}
