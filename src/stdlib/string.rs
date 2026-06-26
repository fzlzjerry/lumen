//! The `string` module: text manipulation. All indexing is by Unicode scalar
//! (character), consistent with `s[i]` in the core language.

use super::{array_of, err, int, string_of, Vm};
use crate::object::Arity::{self, Exact, Range};
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "upper", Exact(1), upper),
        f(vm, "lower", Exact(1), lower),
        f(vm, "trim", Exact(1), trim),
        f(vm, "trim_start", Exact(1), trim_start),
        f(vm, "trim_end", Exact(1), trim_end),
        f(vm, "split", Exact(2), split),
        f(vm, "join", Exact(2), join),
        f(vm, "contains", Exact(2), contains),
        f(vm, "starts_with", Exact(2), starts_with),
        f(vm, "ends_with", Exact(2), ends_with),
        f(vm, "replace", Exact(3), replace),
        f(vm, "repeat", Exact(2), repeat),
        f(vm, "index_of", Exact(2), index_of),
        f(vm, "substring", Exact(3), substring),
        f(vm, "char_at", Exact(2), char_at),
        f(vm, "reverse", Exact(1), reverse),
        f(vm, "chars", Exact(1), chars),
        f(vm, "pad_left", Range(2, 3), pad_left),
        f(vm, "pad_right", Range(2, 3), pad_right),
    ];
    vm.make_module("string", exports)
}

fn upper(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    Ok(vm.new_string(&s.to_uppercase()))
}
fn lower(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    Ok(vm.new_string(&s.to_lowercase()))
}
fn trim(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    Ok(vm.new_string(s.trim()))
}
fn trim_start(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    Ok(vm.new_string(s.trim_start()))
}
fn trim_end(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    Ok(vm.new_string(s.trim_end()))
}

fn split(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let sep = string_of(vm, a[1])?;
    let parts: Vec<String> = if sep.is_empty() {
        s.chars().map(|c| c.to_string()).collect()
    } else {
        s.split(&sep).map(|p| p.to_string()).collect()
    };
    let vals: Vec<Value> = parts.iter().map(|p| vm.new_string(p)).collect();
    Ok(vm.new_array(vals))
}

fn join(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let sep = string_of(vm, a[1])?;
    let mut parts = Vec::with_capacity(items.len());
    for it in items {
        parts.push(vm.to_display(it, false)?);
    }
    Ok(vm.new_string(&parts.join(&sep)))
}

fn contains(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (s, sub) = (string_of(vm, a[0])?, string_of(vm, a[1])?);
    Ok(Value::Bool(s.contains(&sub)))
}
fn starts_with(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (s, p) = (string_of(vm, a[0])?, string_of(vm, a[1])?);
    Ok(Value::Bool(s.starts_with(&p)))
}
fn ends_with(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (s, p) = (string_of(vm, a[0])?, string_of(vm, a[1])?);
    Ok(Value::Bool(s.ends_with(&p)))
}

fn replace(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (s, from, to) = (string_of(vm, a[0])?, string_of(vm, a[1])?, string_of(vm, a[2])?);
    Ok(vm.new_string(&s.replace(&from, &to)))
}

fn repeat(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let n = int(vm, a[1])?;
    if n < 0 {
        return Err(err(vm, error_kind::VALUE, "repeat count cannot be negative"));
    }
    Ok(vm.new_string(&s.repeat(n as usize)))
}

fn index_of(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (s, sub) = (string_of(vm, a[0])?, string_of(vm, a[1])?);
    // Report a character index, not a byte index.
    match s.find(&sub) {
        Some(byte) => Ok(Value::Int(s[..byte].chars().count() as i64)),
        None => Ok(Value::Int(-1)),
    }
}

fn substring(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let norm = |i: i64| -> usize {
        if i < 0 { ((len + i).max(0)) as usize } else { (i.min(len)) as usize }
    };
    let start = norm(int(vm, a[1])?);
    let end = norm(int(vm, a[2])?);
    let slice: String = if start < end { chars[start..end].iter().collect() } else { String::new() };
    Ok(vm.new_string(&slice))
}

fn char_at(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let i = int(vm, a[1])?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let idx = if i < 0 { len + i } else { i };
    if idx < 0 || idx >= len {
        return Err(err(vm, error_kind::INDEX, format!("char_at: index {i} out of bounds")));
    }
    Ok(vm.new_string(&chars[idx as usize].to_string()))
}

fn reverse(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let r: String = s.chars().rev().collect();
    Ok(vm.new_string(&r))
}

fn chars(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let vals: Vec<Value> = s.chars().map(|c| vm.new_string(&c.to_string())).collect();
    Ok(vm.new_array(vals))
}

fn pad(vm: &mut Vm, a: &[Value], left: bool) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let width = int(vm, a[1])?;
    let fill = if a.len() == 3 {
        let f = string_of(vm, a[2])?;
        f.chars().next().unwrap_or(' ')
    } else {
        ' '
    };
    let cur = s.chars().count() as i64;
    if cur >= width {
        return Ok(vm.new_string(&s));
    }
    let padding: String = std::iter::repeat_n(fill, (width - cur) as usize).collect();
    let result = if left { format!("{padding}{s}") } else { format!("{s}{padding}") };
    Ok(vm.new_string(&result))
}
fn pad_left(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { pad(vm, a, true) }
fn pad_right(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { pad(vm, a, false) }
