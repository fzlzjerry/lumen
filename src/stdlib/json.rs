//! The `json` module: a real recursive-descent JSON parser and a serializer,
//! both written from scratch (no external crate).
//!
//! Mapping: `null`↔`nil`, JSON `true`/`false`↔`bool`, integral numbers↔`int`,
//! fractional/exponent numbers↔`float`, strings↔`string`, arrays↔`array`,
//! objects↔`map` (string keys). `json.stringify(v, indent?)` pretty-prints when
//! given an indent width. No collection runs during either operation (pure Rust,
//! no re-entry into Lumen), so intermediate values need no extra rooting.

use super::{int, string_of, Vm};
use crate::object::{LumMap, Obj};
use crate::util::{escape_string_json, format_float};
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    use crate::object::Arity::{Exact, Range};
    let parse_fn = vm.make_native_value("parse", Exact(1), parse);
    let stringify_fn = vm.make_native_value("stringify", Range(1, 2), stringify);
    vm.make_module("json", vec![("parse", parse_fn), ("stringify", stringify_fn)])
}

// ---- parsing ---------------------------------------------------------------

fn parse(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let text = string_of(vm, a[0])?;
    let chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    skip_ws(&chars, &mut pos);
    let value = parse_value(vm, &chars, &mut pos)?;
    skip_ws(&chars, &mut pos);
    if pos != chars.len() {
        return Err(json_err(vm, "trailing characters after JSON value", pos));
    }
    Ok(value)
}

fn skip_ws(chars: &[char], pos: &mut usize) {
    while let Some(c) = chars.get(*pos) {
        if c.is_whitespace() {
            *pos += 1;
        } else {
            break;
        }
    }
}

fn json_err(vm: &mut Vm, msg: &str, pos: usize) -> Value {
    vm.make_error(error_kind::VALUE, format!("invalid JSON at character {pos}: {msg}"))
}

fn parse_value(vm: &mut Vm, chars: &[char], pos: &mut usize) -> Result<Value, Value> {
    skip_ws(chars, pos);
    match chars.get(*pos) {
        Some('{') => parse_object(vm, chars, pos),
        Some('[') => parse_array(vm, chars, pos),
        Some('"') => {
            let s = parse_string(vm, chars, pos)?;
            Ok(vm.new_string(&s))
        }
        Some('t') | Some('f') => parse_bool(vm, chars, pos),
        Some('n') => parse_null(vm, chars, pos),
        Some(c) if *c == '-' || c.is_ascii_digit() => parse_number(vm, chars, pos),
        _ => Err(json_err(vm, "expected a value", *pos)),
    }
}

fn expect_literal(vm: &mut Vm, chars: &[char], pos: &mut usize, lit: &str, value: Value) -> Result<Value, Value> {
    for ch in lit.chars() {
        if chars.get(*pos) != Some(&ch) {
            return Err(json_err(vm, &format!("expected '{lit}'"), *pos));
        }
        *pos += 1;
    }
    Ok(value)
}

fn parse_bool(vm: &mut Vm, chars: &[char], pos: &mut usize) -> Result<Value, Value> {
    if chars.get(*pos) == Some(&'t') {
        expect_literal(vm, chars, pos, "true", Value::Bool(true))
    } else {
        expect_literal(vm, chars, pos, "false", Value::Bool(false))
    }
}

fn parse_null(vm: &mut Vm, chars: &[char], pos: &mut usize) -> Result<Value, Value> {
    expect_literal(vm, chars, pos, "null", Value::Nil)
}

fn parse_number(vm: &mut Vm, chars: &[char], pos: &mut usize) -> Result<Value, Value> {
    let start = *pos;
    let mut is_float = false;
    if chars.get(*pos) == Some(&'-') {
        *pos += 1;
    }
    while let Some(c) = chars.get(*pos) {
        match c {
            '0'..='9' => *pos += 1,
            '.' | 'e' | 'E' | '+' | '-' => {
                is_float = true;
                *pos += 1;
            }
            _ => break,
        }
    }
    let text: String = chars[start..*pos].iter().collect();
    if is_float {
        text.parse::<f64>()
            .map(Value::Float)
            .map_err(|_| json_err(vm, "malformed number", start))
    } else {
        match text.parse::<i64>() {
            Ok(n) => Ok(Value::Int(n)),
            // Integer too big for i64 — fall back to float.
            Err(_) => text.parse::<f64>().map(Value::Float).map_err(|_| json_err(vm, "malformed number", start)),
        }
    }
}

fn parse_string(vm: &mut Vm, chars: &[char], pos: &mut usize) -> Result<String, Value> {
    *pos += 1; // opening quote
    let mut s = String::new();
    loop {
        match chars.get(*pos) {
            None => return Err(json_err(vm, "unterminated string", *pos)),
            Some('"') => {
                *pos += 1;
                return Ok(s);
            }
            Some('\\') => {
                *pos += 1;
                match chars.get(*pos) {
                    Some('"') => s.push('"'),
                    Some('\\') => s.push('\\'),
                    Some('/') => s.push('/'),
                    Some('b') => s.push('\u{0008}'),
                    Some('f') => s.push('\u{000C}'),
                    Some('n') => s.push('\n'),
                    Some('r') => s.push('\r'),
                    Some('t') => s.push('\t'),
                    Some('u') => {
                        let mut code = 0u32;
                        for _ in 0..4 {
                            *pos += 1;
                            let h = chars.get(*pos).and_then(|c| c.to_digit(16));
                            match h {
                                Some(d) => code = code * 16 + d,
                                None => return Err(json_err(vm, "invalid \\u escape", *pos)),
                            }
                        }
                        s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                    _ => return Err(json_err(vm, "invalid escape", *pos)),
                }
                *pos += 1;
            }
            Some(&c) => {
                s.push(c);
                *pos += 1;
            }
        }
    }
}

fn parse_array(vm: &mut Vm, chars: &[char], pos: &mut usize) -> Result<Value, Value> {
    *pos += 1; // '['
    let mut items = Vec::new();
    skip_ws(chars, pos);
    if chars.get(*pos) == Some(&']') {
        *pos += 1;
        return Ok(vm.new_array(items));
    }
    loop {
        let v = parse_value(vm, chars, pos)?;
        items.push(v);
        skip_ws(chars, pos);
        match chars.get(*pos) {
            Some(',') => {
                *pos += 1;
            }
            Some(']') => {
                *pos += 1;
                return Ok(vm.new_array(items));
            }
            _ => return Err(json_err(vm, "expected ',' or ']' in array", *pos)),
        }
    }
}

fn parse_object(vm: &mut Vm, chars: &[char], pos: &mut usize) -> Result<Value, Value> {
    *pos += 1; // '{'
    let mut map = LumMap::new();
    skip_ws(chars, pos);
    if chars.get(*pos) == Some(&'}') {
        *pos += 1;
        return Ok(Value::Obj(vm.heap.alloc_map(map)));
    }
    loop {
        skip_ws(chars, pos);
        if chars.get(*pos) != Some(&'"') {
            return Err(json_err(vm, "expected a string key", *pos));
        }
        let key = parse_string(vm, chars, pos)?;
        skip_ws(chars, pos);
        if chars.get(*pos) != Some(&':') {
            return Err(json_err(vm, "expected ':' after key", *pos));
        }
        *pos += 1;
        let value = parse_value(vm, chars, pos)?;
        let key_val = vm.new_string(&key);
        let norm = vm.map_key(key_val)?;
        map.insert(norm, key_val, value);
        skip_ws(chars, pos);
        match chars.get(*pos) {
            Some(',') => {
                *pos += 1;
            }
            Some('}') => {
                *pos += 1;
                return Ok(Value::Obj(vm.heap.alloc_map(map)));
            }
            _ => return Err(json_err(vm, "expected ',' or '}' in object", *pos)),
        }
    }
}

// ---- serializing -----------------------------------------------------------

fn stringify(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let indent = if a.len() == 2 { Some(int(vm, a[1])?.max(0) as usize) } else { None };
    let mut out = String::new();
    write_value(vm, a[0], indent, 0, &mut out)?;
    Ok(vm.new_string(&out))
}

fn write_value(vm: &mut Vm, v: Value, indent: Option<usize>, depth: usize, out: &mut String) -> Result<(), Value> {
    match v {
        Value::Nil => out.push_str("null"),
        Value::Bool(b) => out.push_str(if b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Float(f) => {
            if f.is_finite() {
                out.push_str(&format_float(f));
            } else {
                out.push_str("null"); // JSON has no inf/nan
            }
        }
        Value::Obj(r) => match vm.heap.get(r) {
            Obj::Str(s) => {
                let escaped = escape_string_json(s);
                out.push('"');
                out.push_str(&escaped);
                out.push('"');
            }
            Obj::Array(items) => {
                let items = items.clone();
                write_array(vm, &items, indent, depth, out)?;
            }
            Obj::Map(m) => {
                let entries: Vec<(Value, Value)> = m.iter().collect();
                write_object(vm, &entries, indent, depth, out)?;
            }
            other => {
                let t = other.type_name();
                return Err(vm.make_error(error_kind::TYPE, format!("cannot serialize {t} to JSON")));
            }
        },
    }
    Ok(())
}

fn newline_indent(out: &mut String, indent: Option<usize>, depth: usize) {
    if let Some(w) = indent {
        out.push('\n');
        for _ in 0..(w * depth) {
            out.push(' ');
        }
    }
}

fn write_array(vm: &mut Vm, items: &[Value], indent: Option<usize>, depth: usize, out: &mut String) -> Result<(), Value> {
    if items.is_empty() {
        out.push_str("[]");
        return Ok(());
    }
    out.push('[');
    for (i, &it) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        newline_indent(out, indent, depth + 1);
        write_value(vm, it, indent, depth + 1, out)?;
    }
    newline_indent(out, indent, depth);
    out.push(']');
    Ok(())
}

fn write_object(vm: &mut Vm, entries: &[(Value, Value)], indent: Option<usize>, depth: usize, out: &mut String) -> Result<(), Value> {
    if entries.is_empty() {
        out.push_str("{}");
        return Ok(());
    }
    out.push('{');
    for (i, (k, val)) in entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        newline_indent(out, indent, depth + 1);
        let key_str = vm.to_display(*k, false)?;
        out.push('"');
        out.push_str(&escape_string_json(&key_str));
        out.push_str("\":");
        if indent.is_some() {
            out.push(' ');
        }
        write_value(vm, *val, indent, depth + 1, out)?;
    }
    newline_indent(out, indent, depth);
    out.push('}');
    Ok(())
}
