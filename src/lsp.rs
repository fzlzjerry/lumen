//! A minimal Language Server (`lumen lsp`), speaking LSP over stdio.
//!
//! Supports the essentials: `initialize`/`shutdown`/`exit`, document
//! open/change/close, **publishDiagnostics** (the front end's lexical/syntax/
//! semantic errors, mapped to LSP ranges), and **hover** (describing the token
//! under the cursor — keyword meaning, literal kind, identifier). It includes a
//! tiny self-contained JSON implementation so the crate stays dependency-free.

use crate::token::TokenKind;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

// ---- a minimal JSON value --------------------------------------------------

#[derive(Clone, Debug)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) => Some(*n as i64),
            _ => None,
        }
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.is_finite() {
                    out.push_str(&(*n as i64).to_string());
                } else {
                    out.push_str(&n.to_string());
                }
            }
            Json::Str(s) => {
                out.push('"');
                out.push_str(&crate::util::escape_string_json(s));
                out.push('"');
            }
            Json::Arr(items) => {
                out.push('[');
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    it.write(out);
                }
                out.push(']');
            }
            Json::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    out.push_str(&crate::util::escape_string_json(k));
                    out.push_str("\":");
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

}

impl std::fmt::Display for Json {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();
        self.write(&mut s);
        f.write_str(&s)
    }
}

fn parse_json(text: &str) -> Option<Json> {
    let chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    let v = parse_val(&chars, &mut pos)?;
    Some(v)
}

fn skip_ws(c: &[char], p: &mut usize) {
    while c.get(*p).is_some_and(|ch| ch.is_whitespace()) {
        *p += 1;
    }
}

fn parse_val(c: &[char], p: &mut usize) -> Option<Json> {
    skip_ws(c, p);
    match c.get(*p)? {
        '{' => parse_obj(c, p),
        '[' => parse_arr(c, p),
        '"' => Some(Json::Str(parse_str(c, p)?)),
        't' => lit(c, p, "true", Json::Bool(true)),
        'f' => lit(c, p, "false", Json::Bool(false)),
        'n' => lit(c, p, "null", Json::Null),
        _ => parse_num(c, p),
    }
}

fn lit(c: &[char], p: &mut usize, s: &str, v: Json) -> Option<Json> {
    for ch in s.chars() {
        if c.get(*p) != Some(&ch) {
            return None;
        }
        *p += 1;
    }
    Some(v)
}

fn parse_num(c: &[char], p: &mut usize) -> Option<Json> {
    let start = *p;
    while let Some(&ch) = c.get(*p) {
        if ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E') {
            *p += 1;
        } else {
            break;
        }
    }
    let s: String = c[start..*p].iter().collect();
    s.parse::<f64>().ok().map(Json::Num)
}

fn parse_str(c: &[char], p: &mut usize) -> Option<String> {
    *p += 1; // opening quote
    let mut s = String::new();
    loop {
        match c.get(*p)? {
            '"' => {
                *p += 1;
                return Some(s);
            }
            '\\' => {
                *p += 1;
                match c.get(*p)? {
                    '"' => s.push('"'),
                    '\\' => s.push('\\'),
                    '/' => s.push('/'),
                    'n' => s.push('\n'),
                    'r' => s.push('\r'),
                    't' => s.push('\t'),
                    'b' => s.push('\u{8}'),
                    'f' => s.push('\u{c}'),
                    'u' => {
                        let mut code = 0u32;
                        for _ in 0..4 {
                            *p += 1;
                            code = code * 16 + c.get(*p)?.to_digit(16)?;
                        }
                        s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                    _ => return None,
                }
                *p += 1;
            }
            &ch => {
                s.push(ch);
                *p += 1;
            }
        }
    }
}

fn parse_arr(c: &[char], p: &mut usize) -> Option<Json> {
    *p += 1;
    let mut items = Vec::new();
    skip_ws(c, p);
    if c.get(*p) == Some(&']') {
        *p += 1;
        return Some(Json::Arr(items));
    }
    loop {
        items.push(parse_val(c, p)?);
        skip_ws(c, p);
        match c.get(*p)? {
            ',' => *p += 1,
            ']' => {
                *p += 1;
                return Some(Json::Arr(items));
            }
            _ => return None,
        }
    }
}

fn parse_obj(c: &[char], p: &mut usize) -> Option<Json> {
    *p += 1;
    let mut pairs = Vec::new();
    skip_ws(c, p);
    if c.get(*p) == Some(&'}') {
        *p += 1;
        return Some(Json::Obj(pairs));
    }
    loop {
        skip_ws(c, p);
        let key = parse_str(c, p)?;
        skip_ws(c, p);
        if c.get(*p) != Some(&':') {
            return None;
        }
        *p += 1;
        let val = parse_val(c, p)?;
        pairs.push((key, val));
        skip_ws(c, p);
        match c.get(*p)? {
            ',' => *p += 1,
            '}' => {
                *p += 1;
                return Some(Json::Obj(pairs));
            }
            _ => return None,
        }
    }
}

// ---- the server ------------------------------------------------------------

/// Run the LSP server over stdio until `exit`. Returns the process exit code.
pub fn run() -> i32 {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    // Open documents: URI -> text.
    let mut docs: HashMap<String, String> = HashMap::new();
    let mut shutting_down = false;

    while let Some(msg) = read_message(&mut reader) {
        let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Json::Null);

        match method {
            "initialize" => {
                let caps = Json::Obj(vec![
                    (
                        "capabilities".into(),
                        Json::Obj(vec![
                            ("textDocumentSync".into(), Json::Num(1.0)), // full sync
                            ("hoverProvider".into(), Json::Bool(true)),
                            ("definitionProvider".into(), Json::Bool(true)),
                            ("documentSymbolProvider".into(), Json::Bool(true)),
                            ("documentFormattingProvider".into(), Json::Bool(true)),
                            ("referencesProvider".into(), Json::Bool(true)),
                            ("renameProvider".into(), Json::Bool(true)),
                            (
                                "signatureHelpProvider".into(),
                                Json::Obj(vec![(
                                    "triggerCharacters".into(),
                                    Json::Arr(vec![Json::Str("(".into()), Json::Str(",".into())]),
                                )]),
                            ),
                            (
                                "completionProvider".into(),
                                Json::Obj(vec![(
                                    "triggerCharacters".into(),
                                    Json::Arr(vec![Json::Str(".".into())]),
                                )]),
                            ),
                        ]),
                    ),
                    (
                        "serverInfo".into(),
                        Json::Obj(vec![
                            ("name".into(), Json::Str("lumen-lsp".into())),
                            ("version".into(), Json::Str(env!("CARGO_PKG_VERSION").into())),
                        ]),
                    ),
                ]);
                send_response(&mut writer, id, caps);
            }
            "initialized" => {}
            "textDocument/didOpen" => {
                if let Some((uri, text)) = doc_open(&params) {
                    publish_diagnostics(&mut writer, &uri, &text);
                    docs.insert(uri, text);
                }
            }
            "textDocument/didChange" => {
                if let Some((uri, text)) = doc_change(&params) {
                    publish_diagnostics(&mut writer, &uri, &text);
                    docs.insert(uri, text);
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = params.get("textDocument").and_then(|d| d.get("uri")).and_then(Json::as_str) {
                    docs.remove(uri);
                }
            }
            "textDocument/hover" => {
                let result = hover(&params, &docs).unwrap_or(Json::Null);
                send_response(&mut writer, id, result);
            }
            "textDocument/definition" => {
                let result = goto_definition(&params, &docs).unwrap_or(Json::Null);
                send_response(&mut writer, id, result);
            }
            "textDocument/documentSymbol" => {
                let result = document_symbols(&params, &docs).unwrap_or(Json::Arr(vec![]));
                send_response(&mut writer, id, result);
            }
            "textDocument/completion" => {
                let result = completion(&params, &docs).unwrap_or(Json::Arr(vec![]));
                send_response(&mut writer, id, result);
            }
            "textDocument/formatting" => {
                let result = formatting(&params, &docs).unwrap_or(Json::Null);
                send_response(&mut writer, id, result);
            }
            "textDocument/references" => {
                let result = references(&params, &docs).unwrap_or(Json::Arr(vec![]));
                send_response(&mut writer, id, result);
            }
            "textDocument/rename" => {
                let result = rename(&params, &docs).unwrap_or(Json::Null);
                send_response(&mut writer, id, result);
            }
            "textDocument/signatureHelp" => {
                let result = signature_help(&params, &docs).unwrap_or(Json::Null);
                send_response(&mut writer, id, result);
            }
            "shutdown" => {
                shutting_down = true;
                send_response(&mut writer, id, Json::Null);
            }
            "exit" => return if shutting_down { 0 } else { 1 },
            _ => {
                // Unknown request: answer with null so the client isn't blocked.
                if id.is_some() {
                    send_response(&mut writer, id, Json::Null);
                }
            }
        }
    }
    0
}

fn doc_open(params: &Json) -> Option<(String, String)> {
    let doc = params.get("textDocument")?;
    let uri = doc.get("uri")?.as_str()?.to_string();
    let text = doc.get("text")?.as_str()?.to_string();
    Some((uri, text))
}

fn doc_change(params: &Json) -> Option<(String, String)> {
    let uri = params.get("textDocument")?.get("uri")?.as_str()?.to_string();
    // Full-sync: the last content change holds the whole document.
    let changes = params.get("contentChanges")?;
    if let Json::Arr(items) = changes {
        let text = items.last()?.get("text")?.as_str()?.to_string();
        return Some((uri, text));
    }
    None
}

/// Run the front end and publish diagnostics for `text`.
fn publish_diagnostics(writer: &mut impl Write, uri: &str, text: &str) {
    // `check_all` includes resolver warnings (unused vars, unreachable code,
    // wrong arity) alongside errors, so the editor surfaces them too.
    let (_program, all) = crate::check_all(text);
    let diagnostics: Vec<Json> = all
        .iter()
        .map(|d| {
            let (sl, sc) = (d.span.line.saturating_sub(1), d.span.col.saturating_sub(1));
            // Approximate the end on the same line by the span length.
            let ec = sc + d.span.len.max(1);
            let severity = match d.severity {
                crate::Severity::Error => 1.0,
                crate::Severity::Warning => 2.0,
                crate::Severity::Note => 3.0,
            };
            Json::Obj(vec![
                ("range".into(), range(sl, sc, sl, ec)),
                ("severity".into(), Json::Num(severity)),
                ("source".into(), Json::Str(format!("lumen:{}", d.stage))),
                ("message".into(), Json::Str(d.message.clone())),
            ])
        })
        .collect();
    let note = Json::Obj(vec![
        ("jsonrpc".into(), Json::Str("2.0".into())),
        ("method".into(), Json::Str("textDocument/publishDiagnostics".into())),
        (
            "params".into(),
            Json::Obj(vec![
                ("uri".into(), Json::Str(uri.to_string())),
                ("diagnostics".into(), Json::Arr(diagnostics)),
            ]),
        ),
    ]);
    send_raw(writer, &note);
}

fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Json {
    Json::Obj(vec![
        ("start".into(), position(sl, sc)),
        ("end".into(), position(el, ec)),
    ])
}
fn position(line: u32, ch: u32) -> Json {
    Json::Obj(vec![
        ("line".into(), Json::Num(line as f64)),
        ("character".into(), Json::Num(ch as f64)),
    ])
}

/// Describe the token under the hover position.
fn hover(params: &Json, docs: &HashMap<String, String>) -> Option<Json> {
    let uri = params.get("textDocument")?.get("uri")?.as_str()?;
    let pos = params.get("position")?;
    let line = pos.get("line")?.as_i64()? as u32 + 1; // back to 1-based
    let col = pos.get("character")?.as_i64()? as u32 + 1;
    let text = docs.get(uri)?;
    let (tokens, _errs) = crate::lexer::lex(text);
    for tok in &tokens {
        if tok.is_eof() {
            break;
        }
        let s = &tok.span;
        // Single-line token containment check.
        if s.line == line && col >= s.col && col < s.col + s.len.max(1) {
            let info = describe_token(&tok.kind);
            return Some(Json::Obj(vec![(
                "contents".into(),
                Json::Obj(vec![
                    ("kind".into(), Json::Str("markdown".into())),
                    ("value".into(), Json::Str(info)),
                ]),
            )]));
        }
    }
    None
}

fn describe_token(kind: &TokenKind) -> String {
    use TokenKind::*;
    match kind {
        Int(n) => format!("`{n}` — integer literal"),
        Float(f) => format!("`{f}` — float literal"),
        Str(_) => "string literal".to_string(),
        Ident(name) => format!("`{name}` — identifier"),
        True | False => "boolean literal".to_string(),
        Nil => "`nil` — the absence value".to_string(),
        Let => "`let` — declare a mutable variable".to_string(),
        Const => "`const` — declare a constant".to_string(),
        Fn => "`fn` — define a function".to_string(),
        Class => "`class` — define a class".to_string(),
        If | Else => "conditional".to_string(),
        While | For => "loop".to_string(),
        Match => "`match` — pattern matching expression".to_string(),
        Return => "`return` — return from a function".to_string(),
        Try | Catch | Finally | Throw => "exception handling".to_string(),
        Import | Export => "module system".to_string(),
        This => "`this` — the method receiver".to_string(),
        Super => "`super` — the superclass".to_string(),
        other => format!("`{}`", other.lexeme()),
    }
}

// ---- declarations: goto-definition, symbols, completion --------------------

use crate::ast::{
    ArrayElem, Block, ClassDecl, Expr, ExprKind, Function, MapKey, Pattern, PatternKind, Program,
    Stmt, StrSegment,
};
use crate::span::Span;

/// A name declaration and the source range over which it is visible. `scope` is
/// a byte-offset half-open interval `[start, end)`; the innermost scope
/// containing a use wins, which gives correct lexical shadowing.
struct Def {
    name: String,
    name_span: Span,
    scope: (u32, u32),
}

/// Collect every declaration in the program, each tagged with the scope it's
/// visible in. A first pass walks statement-level bindings; a second, additive
/// pass walks *expressions* for lambdas, adding each lambda's parameters and
/// locals scoped to the lambda body. Because the lambda's scope is tighter, a use
/// inside it correctly resolves to the lambda's binding (lexical shadowing), so
/// references/rename never reach across a same-named lambda parameter.
fn collect_defs(program: &Program) -> Vec<Def> {
    let mut out = Vec::new();
    // Top-level declarations are visible across the whole file (including its
    // very end), so the document scope is all-inclusive.
    for stmt in &program.items {
        walk_stmt(stmt, (0, u32::MAX), &mut out);
    }
    // Additive: find lambdas anywhere in expressions and add their scoped bindings.
    for stmt in &program.items {
        lambdas_in_stmt(stmt, &mut out);
    }
    out
}

/// Add the scoped bindings of every lambda reachable from a statement's
/// expressions (and recurse into nested statements to reach deeper expressions).
fn lambdas_in_stmt(stmt: &Stmt, out: &mut Vec<Def>) {
    match stmt {
        Stmt::Let { init: Some(e), .. } => lambdas_in_expr(e, out),
        Stmt::Const { init, .. } | Stmt::Destructure { init, .. } | Stmt::Throw { value: init, .. } => {
            lambdas_in_expr(init, out)
        }
        Stmt::Function(f) => lambdas_in_function(f, out),
        Stmt::Class(c) => {
            for m in &c.methods {
                lambdas_in_function(m, out);
            }
        }
        Stmt::Export { decl, .. } => lambdas_in_stmt(decl, out),
        Stmt::Expr { expr, .. } => lambdas_in_expr(expr, out),
        Stmt::Block(b) => {
            for s in &b.stmts {
                lambdas_in_stmt(s, out);
            }
        }
        Stmt::If { cond, then_block, else_branch, .. } => {
            lambdas_in_expr(cond, out);
            for s in &then_block.stmts {
                lambdas_in_stmt(s, out);
            }
            if let Some(e) = else_branch {
                lambdas_in_stmt(e, out);
            }
        }
        Stmt::While { cond, body, .. } => {
            lambdas_in_expr(cond, out);
            for s in &body.stmts {
                lambdas_in_stmt(s, out);
            }
        }
        Stmt::ForIn { iter, body, .. } => {
            lambdas_in_expr(iter, out);
            for s in &body.stmts {
                lambdas_in_stmt(s, out);
            }
        }
        Stmt::ForC { init, cond, step, body, .. } => {
            if let Some(i) = init {
                lambdas_in_stmt(i, out);
            }
            if let Some(c) = cond {
                lambdas_in_expr(c, out);
            }
            if let Some(s) = step {
                lambdas_in_expr(s, out);
            }
            for s in &body.stmts {
                lambdas_in_stmt(s, out);
            }
        }
        Stmt::Return { value: Some(e), .. } => lambdas_in_expr(e, out),
        Stmt::Try { body, catch, finally, .. } => {
            for s in &body.stmts {
                lambdas_in_stmt(s, out);
            }
            if let Some(c) = catch {
                for s in &c.body.stmts {
                    lambdas_in_stmt(s, out);
                }
            }
            if let Some(f) = finally {
                for s in &f.stmts {
                    lambdas_in_stmt(s, out);
                }
            }
        }
        _ => {} // Break, Continue, Import — no expressions
    }
}

/// Recurse into a (named or anonymous) function body and parameter defaults to
/// reach any lambdas they contain. The function's own bindings are added by the
/// statement-level pass / the lambda case below — this only chases expressions.
fn lambdas_in_function(f: &Function, out: &mut Vec<Def>) {
    for p in &f.params {
        if let Some(d) = &p.default {
            lambdas_in_expr(d, out);
        }
    }
    for s in &f.body.stmts {
        lambdas_in_stmt(s, out);
    }
}

/// Add the scoped bindings of every lambda within an expression.
fn lambdas_in_expr(e: &Expr, out: &mut Vec<Def>) {
    match &e.kind {
        ExprKind::Lambda(f) => {
            // The lambda owns its span as a scope: its params and body locals are
            // visible only inside it.
            let scope = (f.span.offset, f.span.offset + f.span.len);
            for p in &f.params {
                out.push(Def { name: p.name.clone(), name_span: p.span, scope });
            }
            for s in &f.body.stmts {
                walk_stmt(s, scope, out);
            }
            // Chase nested lambdas in the body and in parameter defaults.
            lambdas_in_function(f, out);
        }
        ExprKind::Assign { target, value } => {
            lambdas_in_expr(target, out);
            lambdas_in_expr(value, out);
        }
        ExprKind::CompoundAssign { target, value, .. } => {
            lambdas_in_expr(target, out);
            lambdas_in_expr(value, out);
        }
        ExprKind::Unary { operand, .. } => lambdas_in_expr(operand, out),
        ExprKind::Binary { left, right, .. } | ExprKind::Logical { left, right, .. } => {
            lambdas_in_expr(left, out);
            lambdas_in_expr(right, out);
        }
        ExprKind::Ternary { cond, then_branch, else_branch } => {
            lambdas_in_expr(cond, out);
            lambdas_in_expr(then_branch, out);
            lambdas_in_expr(else_branch, out);
        }
        ExprKind::Call { callee, args, .. } => {
            lambdas_in_expr(callee, out);
            for a in args {
                lambdas_in_expr(a, out);
            }
        }
        ExprKind::Index { object, index } => {
            lambdas_in_expr(object, out);
            lambdas_in_expr(index, out);
        }
        ExprKind::Get { object, .. } => lambdas_in_expr(object, out),
        ExprKind::Array(elems) => {
            for el in elems {
                match el {
                    ArrayElem::Item(x) | ArrayElem::Spread(x) => lambdas_in_expr(x, out),
                }
            }
        }
        ExprKind::Map(pairs) => {
            for (k, v) in pairs {
                if let MapKey::Computed(x) = k {
                    lambdas_in_expr(x, out);
                }
                lambdas_in_expr(v, out);
            }
        }
        ExprKind::StrInterp(segs) => {
            for s in segs {
                if let StrSegment::Expr(x) = s {
                    lambdas_in_expr(x, out);
                }
            }
        }
        ExprKind::Match { subject, arms } => {
            lambdas_in_expr(subject, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    lambdas_in_expr(g, out);
                }
                lambdas_in_expr(&arm.body, out);
            }
        }
        // Leaves: Int/Float/Str/Bool/Nil/Var/This/Super.
        _ => {}
    }
}

fn block_scope(b: &Block) -> (u32, u32) {
    (b.span.offset, b.span.offset + b.span.len)
}

fn walk_block(b: &Block, out: &mut Vec<Def>) {
    let scope = block_scope(b);
    for s in &b.stmts {
        walk_stmt(s, scope, out);
    }
}

fn walk_function(f: &Function, enclosing: (u32, u32), out: &mut Vec<Def>) {
    // The name (if any) is visible to siblings; params and locals are not.
    if let Some(name) = &f.name {
        out.push(Def { name: name.clone(), name_span: f.name_span, scope: enclosing });
    }
    let inner = (f.span.offset, f.span.offset + f.span.len);
    for p in &f.params {
        out.push(Def { name: p.name.clone(), name_span: p.span, scope: inner });
    }
    for s in &f.body.stmts {
        walk_stmt(s, inner, out);
    }
}

fn bind_pattern(pat: &Pattern, scope: (u32, u32), out: &mut Vec<Def>) {
    match &pat.kind {
        PatternKind::Binding(name) => {
            out.push(Def { name: name.clone(), name_span: pat.span, scope });
        }
        PatternKind::Array(elems) => {
            for e in elems {
                match e {
                    crate::ast::PatElem::Pattern(p) => bind_pattern(p, scope, out),
                    crate::ast::PatElem::Rest(Some(name)) => {
                        out.push(Def { name: name.clone(), name_span: pat.span, scope });
                    }
                    crate::ast::PatElem::Rest(None) => {}
                }
            }
        }
        PatternKind::Map(pairs) => {
            for (_k, p) in pairs {
                bind_pattern(p, scope, out);
            }
        }
        _ => {}
    }
}

fn walk_stmt(stmt: &Stmt, scope: (u32, u32), out: &mut Vec<Def>) {
    match stmt {
        Stmt::Let { name, name_span, .. } => {
            out.push(Def { name: name.clone(), name_span: *name_span, scope });
        }
        Stmt::Const { name, name_span, .. } => {
            out.push(Def { name: name.clone(), name_span: *name_span, scope });
        }
        Stmt::Destructure { pattern, .. } => bind_pattern(pattern, scope, out),
        Stmt::Function(f) => walk_function(f, scope, out),
        Stmt::Class(c) => {
            out.push(Def { name: c.name.clone(), name_span: c.name_span, scope });
            for m in &c.methods {
                walk_function(m, (m.span.offset, m.span.offset + m.span.len), out);
            }
        }
        Stmt::Import(i) => match &i.kind {
            crate::ast::ImportKind::Module { alias } => {
                out.push(Def { name: alias.value.clone(), name_span: alias.span, scope });
            }
            crate::ast::ImportKind::Named(items) => {
                for it in items {
                    out.push(Def { name: it.value.clone(), name_span: it.span, scope });
                }
            }
        },
        Stmt::Export { decl, .. } => walk_stmt(decl, scope, out),
        Stmt::Block(b) => walk_block(b, out),
        Stmt::If { then_block, else_branch, .. } => {
            walk_block(then_block, out);
            if let Some(e) = else_branch {
                walk_stmt(e, scope, out);
            }
        }
        Stmt::While { body, .. } => walk_block(body, out),
        Stmt::ForIn { var, var_span, body, span, .. } => {
            let loop_scope = (span.offset, span.offset + span.len);
            out.push(Def { name: var.clone(), name_span: *var_span, scope: loop_scope });
            for s in &body.stmts {
                walk_stmt(s, loop_scope, out);
            }
        }
        Stmt::ForC { init, body, span, .. } => {
            let loop_scope = (span.offset, span.offset + span.len);
            if let Some(init) = init {
                walk_stmt(init, loop_scope, out);
            }
            for s in &body.stmts {
                walk_stmt(s, loop_scope, out);
            }
        }
        Stmt::Try { body, catch, finally, .. } => {
            walk_block(body, out);
            if let Some(c) = catch {
                let cscope = block_scope(&c.body);
                out.push(Def { name: c.name.clone(), name_span: c.name_span, scope: cscope });
                walk_block(&c.body, out);
            }
            if let Some(f) = finally {
                walk_block(f, out);
            }
        }
        _ => {}
    }
}

/// The identifier name and byte offset under an LSP position, if any.
fn ident_at(text: &str, line: u32, col: u32) -> Option<(String, u32)> {
    let (tokens, _errs) = crate::lexer::lex(text);
    for tok in &tokens {
        if tok.is_eof() {
            break;
        }
        let s = &tok.span;
        if s.line == line && col >= s.col && col < s.col + s.len.max(1) {
            if let TokenKind::Ident(name) = &tok.kind {
                return Some((name.clone(), s.offset));
            }
            return None;
        }
    }
    None
}

fn goto_definition(params: &Json, docs: &HashMap<String, String>) -> Option<Json> {
    let uri = params.get("textDocument")?.get("uri")?.as_str()?;
    let pos = params.get("position")?;
    let line = pos.get("line")?.as_i64()? as u32 + 1;
    let col = pos.get("character")?.as_i64()? as u32 + 1;
    let text = docs.get(uri)?;
    let (name, use_off) = ident_at(text, line, col)?;

    let (program, _errs) = crate::parse_source(text);
    let defs = collect_defs(&program);
    let best = resolve_def(&defs, &name, use_off)?;
    Some(Json::Obj(vec![
        ("uri".into(), Json::Str(uri.to_string())),
        ("range".into(), span_to_range(text, &best.name_span)),
    ]))
}

/// Resolve an identifier use (by name + byte offset) to the innermost in-scope
/// declaration — lexical shadowing. Shared by definition / references / rename.
fn resolve_def<'a>(defs: &'a [Def], name: &str, use_off: u32) -> Option<&'a Def> {
    defs.iter()
        .filter(|d| d.name == name && d.scope.0 <= use_off && use_off < d.scope.1)
        .min_by_key(|d| d.scope.1 - d.scope.0)
}

/// `textDocument/formatting` — reformat the whole document with the canonical
/// printer. Returns one full-document `TextEdit`, or `None` if the source has
/// parse errors (we never reformat broken code, matching `lumen fmt`).
fn formatting(params: &Json, docs: &HashMap<String, String>) -> Option<Json> {
    let uri = params.get("textDocument")?.get("uri")?.as_str()?;
    let text = docs.get(uri)?;
    let (program, errors) = crate::parse_source(text);
    if !errors.is_empty() {
        return None;
    }
    let formatted = crate::ast_printer::print_program(&program);
    let (end_line, end_col) = pos_of_offset(text, text.len() as u32);
    Some(Json::Arr(vec![Json::Obj(vec![
        ("range".into(), range(0, 0, end_line, end_col)),
        ("newText".into(), Json::Str(formatted)),
    ])]))
}

/// Every identifier occurrence in `text` resolving to the same declaration as
/// the identifier under the (1-based) cursor, plus that declaration's name span.
/// Occurrences are in document order and include the declaration itself.
fn reference_spans(text: &str, line: u32, col: u32) -> Option<(Vec<Span>, Span)> {
    let (name, use_off) = ident_at(text, line, col)?;
    let (program, _errs) = crate::parse_source(text);
    let defs = collect_defs(&program);
    let target = resolve_def(&defs, &name, use_off)?.name_span;
    let (tokens, _errs) = crate::lexer::lex(text);
    let mut spans = Vec::new();
    for tok in &tokens {
        if tok.is_eof() {
            break;
        }
        if let TokenKind::Ident(n) = &tok.kind {
            if n == &name {
                if let Some(d) = resolve_def(&defs, n, tok.span.offset) {
                    if d.name_span.offset == target.offset {
                        spans.push(tok.span);
                    }
                }
            }
        }
    }
    Some((spans, target))
}

/// `textDocument/references` — all uses of the symbol under the cursor.
fn references(params: &Json, docs: &HashMap<String, String>) -> Option<Json> {
    let uri = params.get("textDocument")?.get("uri")?.as_str()?;
    let pos = params.get("position")?;
    let line = pos.get("line")?.as_i64()? as u32 + 1;
    let col = pos.get("character")?.as_i64()? as u32 + 1;
    let include_decl = params
        .get("context")
        .and_then(|c| c.get("includeDeclaration"))
        .map(|b| matches!(b, Json::Bool(true)))
        .unwrap_or(true);
    let text = docs.get(uri)?;
    let (spans, decl) = reference_spans(text, line, col)?;
    let locs: Vec<Json> = spans
        .iter()
        .filter(|s| include_decl || s.offset != decl.offset)
        .map(|s| {
            Json::Obj(vec![
                ("uri".into(), Json::Str(uri.to_string())),
                ("range".into(), span_to_range(text, s)),
            ])
        })
        .collect();
    Some(Json::Arr(locs))
}

/// `textDocument/rename` — a `WorkspaceEdit` replacing every occurrence of the
/// symbol under the cursor with `newName` (which must be a valid identifier).
fn rename(params: &Json, docs: &HashMap<String, String>) -> Option<Json> {
    let uri = params.get("textDocument")?.get("uri")?.as_str()?;
    let pos = params.get("position")?;
    let line = pos.get("line")?.as_i64()? as u32 + 1;
    let col = pos.get("character")?.as_i64()? as u32 + 1;
    let new_name = params.get("newName")?.as_str()?;
    if !is_ident(new_name) {
        return None;
    }
    let text = docs.get(uri)?;
    let (spans, _decl) = reference_spans(text, line, col)?;
    if spans.is_empty() {
        return None;
    }
    let edits: Vec<Json> = spans
        .iter()
        .map(|s| {
            Json::Obj(vec![
                ("range".into(), span_to_range(text, s)),
                ("newText".into(), Json::Str(new_name.to_string())),
            ])
        })
        .collect();
    Some(Json::Obj(vec![(
        "changes".into(),
        Json::Obj(vec![(uri.to_string(), Json::Arr(edits))]),
    )]))
}

/// Whether `s` is a syntactically valid Lumen identifier.
fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// The display form of a parameter in a signature label.
fn param_label(p: &crate::ast::Param) -> String {
    if p.is_rest {
        format!("..{}", p.name)
    } else if let Some(d) = &p.default {
        format!("{} = {}", p.name, crate::ast_printer::print_expr(d))
    } else {
        p.name.clone()
    }
}

/// Map every named function/method in the program to its parameter labels.
fn collect_signatures(program: &Program) -> HashMap<String, Vec<String>> {
    use crate::ast::Function;
    fn add_fn(f: &Function, sigs: &mut HashMap<String, Vec<String>>) {
        if let Some(name) = &f.name {
            sigs.insert(name.clone(), f.params.iter().map(param_label).collect());
        }
        for s in &f.body.stmts {
            walk(s, sigs);
        }
    }
    fn walk(stmt: &Stmt, sigs: &mut HashMap<String, Vec<String>>) {
        match stmt {
            Stmt::Function(f) => add_fn(f, sigs),
            Stmt::Class(c) => {
                for m in &c.methods {
                    add_fn(m, sigs);
                }
            }
            Stmt::Export { decl, .. } => walk(decl, sigs),
            Stmt::Block(b) => {
                for s in &b.stmts {
                    walk(s, sigs);
                }
            }
            Stmt::If { then_block, else_branch, .. } => {
                for s in &then_block.stmts {
                    walk(s, sigs);
                }
                if let Some(e) = else_branch {
                    walk(e, sigs);
                }
            }
            Stmt::While { body, .. } | Stmt::ForIn { body, .. } | Stmt::ForC { body, .. } => {
                for s in &body.stmts {
                    walk(s, sigs);
                }
            }
            Stmt::Try { body, catch, finally, .. } => {
                for s in &body.stmts {
                    walk(s, sigs);
                }
                if let Some(c) = catch {
                    for s in &c.body.stmts {
                        walk(s, sigs);
                    }
                }
                if let Some(f) = finally {
                    for s in &f.stmts {
                        walk(s, sigs);
                    }
                }
            }
            _ => {}
        }
    }
    let mut sigs = HashMap::new();
    for s in &program.items {
        walk(s, &mut sigs);
    }
    sigs
}

/// Scanning backward from the cursor, find the call being typed: the callee name
/// and the 0-based index of the argument the cursor is in (commas at paren depth
/// 0). `None` when the cursor isn't inside a call's parentheses.
fn call_context(text: &str, cursor_off: u32) -> Option<(String, usize)> {
    let (tokens, _errs) = crate::lexer::lex(text);
    let toks: Vec<&crate::token::Token> =
        tokens.iter().filter(|t| !t.is_eof() && t.span.offset < cursor_off).collect();
    let mut depth = 0i32;
    let mut commas = 0usize;
    let mut i = toks.len();
    while i > 0 {
        i -= 1;
        match &toks[i].kind {
            // Any closer raises the depth; commas/parens below it are nested.
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => depth += 1,
            TokenKind::LParen => {
                if depth == 0 {
                    if i == 0 {
                        return None;
                    }
                    if let TokenKind::Ident(name) = &toks[i - 1].kind {
                        return Some((name.clone(), commas));
                    }
                    return None;
                }
                depth -= 1;
            }
            // An unmatched `[`/`{` at depth 0 means the cursor is inside an
            // array/map literal, not a call's argument list.
            TokenKind::LBracket | TokenKind::LBrace => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            TokenKind::Comma if depth == 0 => commas += 1,
            _ => {}
        }
    }
    None
}

/// `textDocument/signatureHelp` — the signature of the call being typed, with
/// the active parameter highlighted. Covers user-defined functions and methods.
fn signature_help(params: &Json, docs: &HashMap<String, String>) -> Option<Json> {
    let uri = params.get("textDocument")?.get("uri")?.as_str()?;
    let pos = params.get("position")?;
    let line = pos.get("line")?.as_i64()? as u32 + 1;
    let col = pos.get("character")?.as_i64()? as u32 + 1;
    let text = docs.get(uri)?;
    let cursor = offset_at(text, line, col);
    let (callee, active) = call_context(text, cursor)?;
    let (program, _errs) = crate::parse_source(text);
    let sigs = collect_signatures(&program);
    let plabels = sigs.get(&callee)?;
    let label = format!("{}({})", callee, plabels.join(", "));
    let parameters: Vec<Json> = plabels
        .iter()
        .map(|p| Json::Obj(vec![("label".into(), Json::Str(p.clone()))]))
        .collect();
    let signature = Json::Obj(vec![
        ("label".into(), Json::Str(label)),
        ("parameters".into(), Json::Arr(parameters)),
    ]);
    Some(Json::Obj(vec![
        ("signatures".into(), Json::Arr(vec![signature])),
        ("activeSignature".into(), Json::Num(0.0)),
        ("activeParameter".into(), Json::Num(active as f64)),
    ]))
}

fn completion(params: &Json, docs: &HashMap<String, String>) -> Option<Json> {
    let uri = params.get("textDocument")?.get("uri")?.as_str()?;
    let text = docs.get(uri)?;
    // Use position to scope-filter the offered identifiers when available.
    let use_off = params
        .get("position")
        .and_then(|pos| {
            let line = pos.get("line")?.as_i64()? as u32 + 1;
            let col = pos.get("character")?.as_i64()? as u32 + 1;
            Some(offset_at(text, line, col))
        })
        .unwrap_or(text.len() as u32);

    let (program, _errs) = crate::parse_source(text);
    let defs = collect_defs(&program);

    let mut items: Vec<Json> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // In-scope identifiers (kind 6 = Variable).
    for d in &defs {
        if d.scope.0 <= use_off && use_off < d.scope.1 && seen.insert(d.name.clone()) {
            items.push(completion_item(&d.name, 6, "in scope"));
        }
    }
    // Built-in stdlib modules (kind 9 = Module).
    for m in ["math", "string", "array", "map", "io", "os", "time", "json", "random"] {
        if seen.insert(m.to_string()) {
            items.push(completion_item(m, 9, "stdlib module"));
        }
    }
    // Keywords (kind 14 = Keyword).
    for kw in [
        "let", "const", "fn", "class", "if", "else", "while", "for", "return", "match", "import",
        "export", "try", "catch", "finally", "throw", "break", "continue", "true", "false", "nil",
        "this", "super", "and", "or", "not", "in",
    ] {
        if seen.insert(kw.to_string()) {
            items.push(completion_item(kw, 14, "keyword"));
        }
    }
    Some(Json::Arr(items))
}

fn completion_item(label: &str, kind: i64, detail: &str) -> Json {
    Json::Obj(vec![
        ("label".into(), Json::Str(label.to_string())),
        ("kind".into(), Json::Num(kind as f64)),
        ("detail".into(), Json::Str(detail.to_string())),
    ])
}

fn document_symbols(params: &Json, docs: &HashMap<String, String>) -> Option<Json> {
    let uri = params.get("textDocument")?.get("uri")?.as_str()?;
    let text = docs.get(uri)?;
    let (program, _errs) = crate::parse_source(text);
    let mut syms = Vec::new();
    for stmt in &program.items {
        symbol_for(stmt, text, &mut syms);
    }
    Some(Json::Arr(syms))
}

fn symbol_for(stmt: &Stmt, text: &str, out: &mut Vec<Json>) {
    match stmt {
        Stmt::Function(f) => {
            if let Some(name) = &f.name {
                out.push(symbol(name, 12, text, f.span, f.name_span, vec![]));
            }
        }
        Stmt::Class(c) => out.push(class_symbol(c, text)),
        Stmt::Let { name, name_span, span, .. } => {
            out.push(symbol(name, 13, text, *span, *name_span, vec![]));
        }
        Stmt::Const { name, name_span, span, .. } => {
            out.push(symbol(name, 14, text, *span, *name_span, vec![]));
        }
        Stmt::Export { decl, .. } => symbol_for(decl, text, out),
        _ => {}
    }
}

fn class_symbol(c: &ClassDecl, text: &str) -> Json {
    let children: Vec<Json> = c
        .methods
        .iter()
        .filter_map(|m| {
            m.name
                .as_ref()
                .map(|name| symbol(name, 6, text, m.span, m.name_span, vec![]))
        })
        .collect();
    symbol(&c.name, 5, text, c.span, c.name_span, children)
}

fn symbol(name: &str, kind: i64, text: &str, span: Span, sel: Span, children: Vec<Json>) -> Json {
    let mut fields = vec![
        ("name".into(), Json::Str(name.to_string())),
        ("kind".into(), Json::Num(kind as f64)),
        ("range".into(), span_to_range(text, &span)),
        ("selectionRange".into(), span_to_range(text, &sel)),
    ];
    if !children.is_empty() {
        fields.push(("children".into(), Json::Arr(children)));
    }
    Json::Obj(fields)
}

/// Convert a byte-offset span to an LSP range (0-based line/character),
/// scanning the source so multi-line spans get correct end positions.
fn span_to_range(text: &str, span: &Span) -> Json {
    let (sl, sc) = pos_of_offset(text, span.offset);
    let (el, ec) = pos_of_offset(text, span.offset + span.len);
    range(sl, sc, el, ec)
}

/// 0-based (line, character) of a byte offset, counting characters per line.
fn pos_of_offset(text: &str, offset: u32) -> (u32, u32) {
    let off = offset as usize;
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, ch) in text.char_indices() {
        if i >= off {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Byte offset of an LSP position given as 1-based (line, col) — the inverse of
/// `pos_of_offset`, used to scope-filter completions.
fn offset_at(text: &str, line: u32, col: u32) -> u32 {
    let (target_line, target_col) = (line.saturating_sub(1), col.saturating_sub(1));
    let mut cur_line = 0u32;
    let mut cur_col = 0u32;
    for (i, ch) in text.char_indices() {
        if cur_line == target_line && cur_col == target_col {
            return i as u32;
        }
        if ch == '\n' {
            cur_line += 1;
            cur_col = 0;
        } else {
            cur_col += 1;
        }
    }
    text.len() as u32
}

// ---- message framing -------------------------------------------------------

fn read_message(reader: &mut impl BufRead) -> Option<Json> {
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None; // EOF
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().ok()?;
        }
    }
    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf).ok()?;
    let text = String::from_utf8(buf).ok()?;
    parse_json(&text)
}

fn send_raw(writer: &mut impl Write, msg: &Json) {
    let body = msg.to_string();
    let _ = write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = writer.flush();
}

fn send_response(writer: &mut impl Write, id: Option<Json>, result: Json) {
    let msg = Json::Obj(vec![
        ("jsonrpc".into(), Json::Str("2.0".into())),
        ("id".into(), id.unwrap_or(Json::Null)),
        ("result".into(), result),
    ]);
    send_raw(writer, &msg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrips() {
        let v = parse_json(r#"{"a": 1, "b": [true, null, "x"], "c": 2.5}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_i64(), Some(1));
        assert_eq!(v.get("b").unwrap().to_string(), r#"[true,null,"x"]"#);
        assert_eq!(v.get("c").and_then(|c| match c { Json::Num(n) => Some(*n), _ => None }), Some(2.5));
    }

    #[test]
    fn diagnostics_have_ranges() {
        // Indirectly: a broken program yields a diagnostic with a 1-based span we
        // convert to 0-based LSP positions.
        let (_p, errs) = crate::check_source("let x = ;");
        assert!(!errs.is_empty());
        let d = &errs[0];
        assert!(d.span.line >= 1 && d.span.col >= 1);
    }

    fn docs(src: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("file:///t.lum".to_string(), src.to_string());
        m
    }

    fn pos_params(line: u32, ch: u32) -> Json {
        Json::Obj(vec![
            (
                "textDocument".into(),
                Json::Obj(vec![("uri".into(), Json::Str("file:///t.lum".into()))]),
            ),
            (
                "position".into(),
                Json::Obj(vec![
                    ("line".into(), Json::Num(line as f64)),
                    ("character".into(), Json::Num(ch as f64)),
                ]),
            ),
        ])
    }

    fn start_line(result: &Json) -> i64 {
        result.get("range").unwrap().get("start").unwrap().get("line").unwrap().as_i64().unwrap()
    }
    fn start_char(result: &Json) -> i64 {
        result.get("range").unwrap().get("start").unwrap().get("character").unwrap().as_i64().unwrap()
    }

    #[test]
    fn goto_definition_finds_function() {
        let src = "fn foo() { return 1; }\nlet x = foo();\n";
        let d = docs(src);
        // The `foo` in the call on line 1 (0-based), char 8.
        let result = goto_definition(&pos_params(1, 8), &d).expect("definition");
        assert_eq!(start_line(&result), 0, "defined on first line");
        assert_eq!(start_char(&result), 3, "at the `foo` after `fn `");
    }

    #[test]
    fn goto_definition_respects_shadowing() {
        // Inner `x` (param) should win over the outer top-level `x`.
        let src = "let x = 1;\nfn f(x) { return x; }\n";
        let d = docs(src);
        // The `x` in `return x` is line 1, char 17.
        let result = goto_definition(&pos_params(1, 17), &d).expect("definition");
        // Resolves to the parameter on line 1 (char 5), not the global on line 0.
        assert_eq!(start_line(&result), 1, "the parameter, not the global");
        assert_eq!(start_char(&result), 5);
    }

    #[test]
    fn goto_definition_misses_on_unknown() {
        let src = "let a = 1;\n";
        // Position on `a` resolves to its own declaration (a self-reference is fine);
        // a position on whitespace yields nothing.
        assert!(goto_definition(&pos_params(0, 7), &docs(src)).is_none());
    }

    #[test]
    fn document_symbols_lists_decls() {
        let src = "fn greet() { return 1; }\nclass Point { fn dist() { return 0; } }\nlet count = 3;\n";
        let result = document_symbols(
            &Json::Obj(vec![(
                "textDocument".into(),
                Json::Obj(vec![("uri".into(), Json::Str("file:///t.lum".into()))]),
            )]),
            &docs(src),
        )
        .expect("symbols");
        let Json::Arr(syms) = result else { panic!("expected array") };
        assert_eq!(syms.len(), 3, "function, class, variable");
        assert_eq!(syms[0].get("name").unwrap().as_str(), Some("greet"));
        assert_eq!(syms[0].get("kind").unwrap().as_i64(), Some(12)); // Function
        assert_eq!(syms[1].get("name").unwrap().as_str(), Some("Point"));
        assert_eq!(syms[1].get("kind").unwrap().as_i64(), Some(5)); // Class
        // The class carries its method as a child symbol.
        let Json::Arr(children) = syms[1].get("children").unwrap() else { panic!() };
        assert_eq!(children[0].get("name").unwrap().as_str(), Some("dist"));
        assert_eq!(children[0].get("kind").unwrap().as_i64(), Some(6)); // Method
        assert_eq!(syms[2].get("name").unwrap().as_str(), Some("count"));
    }

    #[test]
    fn completion_offers_scope_modules_and_keywords() {
        let src = "let apple = 1;\nfn banana() { return 2; }\n";
        let mut params = pos_params(2, 0); // end of file, both decls in scope
        if let Json::Obj(fields) = &mut params {
            fields.retain(|(k, _)| k != "position"); // also exercise the no-position path
        }
        let result = completion(&params, &docs(src)).expect("completion");
        let Json::Arr(items) = result else { panic!("expected array") };
        let labels: Vec<&str> =
            items.iter().filter_map(|i| i.get("label").and_then(Json::as_str)).collect();
        assert!(labels.contains(&"apple"), "in-scope var");
        assert!(labels.contains(&"banana"), "in-scope fn");
        assert!(labels.contains(&"math"), "stdlib module");
        assert!(labels.contains(&"let"), "keyword");
    }

    fn td_params() -> Json {
        Json::Obj(vec![(
            "textDocument".into(),
            Json::Obj(vec![("uri".into(), Json::Str("file:///t.lum".into()))]),
        )])
    }

    fn ref_params(line: u32, ch: u32, include_decl: bool) -> Json {
        let mut p = pos_params(line, ch);
        if let Json::Obj(fields) = &mut p {
            fields.push((
                "context".into(),
                Json::Obj(vec![("includeDeclaration".into(), Json::Bool(include_decl))]),
            ));
        }
        p
    }

    fn rename_params(line: u32, ch: u32, new_name: &str) -> Json {
        let mut p = pos_params(line, ch);
        if let Json::Obj(fields) = &mut p {
            fields.push(("newName".into(), Json::Str(new_name.into())));
        }
        p
    }

    #[test]
    fn formatting_replaces_whole_document() {
        let src = "let   x=1;\n";
        let result = formatting(&td_params(), &docs(src)).expect("formatting");
        let Json::Arr(edits) = result else { panic!("expected array") };
        assert_eq!(edits.len(), 1);
        let (program, _) = crate::parse_source(src);
        let expected = crate::ast_printer::print_program(&program);
        assert_eq!(edits[0].get("newText").unwrap().as_str(), Some(expected.as_str()));
        assert_ne!(expected, src, "the messy input is actually reformatted");
        assert_eq!(start_line(&edits[0]), 0, "edit covers from the document start");
        assert_eq!(start_char(&edits[0]), 0);
    }

    #[test]
    fn formatting_skips_broken_source() {
        assert!(formatting(&td_params(), &docs("let x = ;")).is_none());
    }

    #[test]
    fn references_finds_all_uses() {
        let src = "let x = 1;\nlet y = x + x;\nfn f() { return x; }\n";
        let result = references(&ref_params(0, 4, true), &docs(src)).expect("references");
        let Json::Arr(locs) = result else { panic!() };
        assert_eq!(locs.len(), 4, "declaration + three uses");
    }

    #[test]
    fn references_respect_shadowing() {
        let src = "let x = 1;\nfn f(x) { return x; }\n";
        // The outer `x`: just its own declaration (the inner x is a separate binding).
        let outer = references(&ref_params(0, 4, true), &docs(src)).expect("references");
        let Json::Arr(o) = outer else { panic!() };
        assert_eq!(o.len(), 1);
        // The inner parameter `x`: its declaration plus the use in `return x`.
        let inner = references(&ref_params(1, 5, true), &docs(src)).expect("references");
        let Json::Arr(i) = inner else { panic!() };
        assert_eq!(i.len(), 2);
    }

    #[test]
    fn references_can_exclude_declaration() {
        let src = "let x = 1;\nlet y = x + x;\n";
        let result = references(&ref_params(0, 4, false), &docs(src)).expect("references");
        let Json::Arr(locs) = result else { panic!() };
        assert_eq!(locs.len(), 2, "two uses, declaration excluded");
    }

    #[test]
    fn rename_edits_every_occurrence() {
        let src = "let x = 1;\nlet y = x + x;\n";
        let result = rename(&rename_params(0, 4, "z"), &docs(src)).expect("rename");
        let changes = result.get("changes").unwrap().get("file:///t.lum").unwrap();
        let Json::Arr(edits) = changes else { panic!() };
        assert_eq!(edits.len(), 3, "declaration + two uses");
        for e in edits {
            assert_eq!(e.get("newText").unwrap().as_str(), Some("z"));
        }
    }

    #[test]
    fn rename_rejects_invalid_name() {
        assert!(rename(&rename_params(0, 4, ""), &docs("let x = 1;\n")).is_none());
        assert!(rename(&rename_params(0, 4, "1bad"), &docs("let x = 1;\n")).is_none());
    }

    #[test]
    fn signature_help_marks_active_parameter() {
        let src = "fn add(a, b) { return a + b; }\nlet s = add(1, 2);\n";
        // Cursor on the second argument `2` (line 1, char 15).
        let result = signature_help(&pos_params(1, 15), &docs(src)).expect("sighelp");
        let Json::Arr(sigs) = result.get("signatures").unwrap() else { panic!() };
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0].get("label").unwrap().as_str(), Some("add(a, b)"));
        assert_eq!(result.get("activeParameter").unwrap().as_i64(), Some(1));
        // Cursor on the first argument `1` (line 1, char 12).
        let first = signature_help(&pos_params(1, 12), &docs(src)).expect("sighelp");
        assert_eq!(first.get("activeParameter").unwrap().as_i64(), Some(0));
    }

    #[test]
    fn signature_help_none_outside_call() {
        let src = "fn add(a, b) { return a + b; }\nlet s = 1;\n";
        assert!(signature_help(&pos_params(1, 8), &docs(src)).is_none());
    }

    #[test]
    fn signature_help_ignores_commas_in_literal_args() {
        let src = "fn g(a, b) { return a; }\nlet s = g([1, 2], 3);\n";
        // Cursor on the second argument `3` (line 1, char 18). The commas inside
        // the array literal `[1, 2]` must NOT be counted as argument separators.
        let result = signature_help(&pos_params(1, 18), &docs(src)).expect("sighelp");
        let Json::Arr(sigs) = result.get("signatures").unwrap() else { panic!() };
        assert_eq!(sigs[0].get("label").unwrap().as_str(), Some("g(a, b)"));
        assert_eq!(result.get("activeParameter").unwrap().as_i64(), Some(1));
        // Cursor inside the array literal itself (char 11, on `1`): no signature.
        assert!(signature_help(&pos_params(1, 11), &docs(src)).is_none());
    }

    #[test]
    fn references_exclude_lambda_shadowed_uses() {
        // A lambda parameter `x` shadows the outer `x`; references/rename of the
        // outer must NOT reach into the lambda (renaming there would corrupt code).
        let src = "let x = 1;\nlet f = fn(x) { return x; };\n";
        let result = references(&ref_params(0, 4, true), &docs(src)).expect("references");
        let Json::Arr(locs) = result else { panic!() };
        assert_eq!(locs.len(), 1, "only the outer declaration");
    }

    #[test]
    fn references_handle_arrow_lambda_scope() {
        let src = "let x = 1;\nlet f = x => x + 1;\n";
        let result = references(&ref_params(0, 4, true), &docs(src)).expect("references");
        let Json::Arr(locs) = result else { panic!() };
        assert_eq!(locs.len(), 1, "outer x only; the arrow parameter x is separate");
    }

    #[test]
    fn references_into_lambda_scope() {
        // Uses of a lambda parameter resolve to the parameter, not anything outer.
        let src = "let f = fn(x) { return x + x; };\n";
        let result = references(&ref_params(0, 11, true), &docs(src)).expect("references");
        let Json::Arr(locs) = result else { panic!() };
        assert_eq!(locs.len(), 3, "parameter declaration + two uses");
    }
}
