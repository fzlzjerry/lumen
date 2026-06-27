//! The global built-in functions (SPEC §10): the always-in-scope primitives
//! (`print`, `len`, `range`, `push`, …) implemented in Rust. The richer native
//! *modules* (`math`, `string`, `json`, …) live in `src/stdlib/` (Phase 7);
//! these are the core runtime surface every program can use unqualified.
//!
//! Each builtin follows the same borrow discipline: read what it needs out of
//! the heap into owned/`Copy` data, *then* mutate or allocate, so an immutable
//! heap borrow never overlaps a `make_error`/allocation.

use super::Vm;
use crate::object::{Arity, Obj};
use crate::value::{error_kind, Value};

/// Register every global builtin into the VM's builtin table.
pub fn register(vm: &mut Vm) {
    use Arity::*;
    vm.define_native("print", AtLeast(0), print);
    vm.define_native("println", AtLeast(0), println);
    vm.define_native("str", Exact(1), str_fn);
    vm.define_native("type", Exact(1), type_fn);
    vm.define_native("len", Exact(1), len);
    vm.define_native("int", Exact(1), int_fn);
    vm.define_native("float", Exact(1), float_fn);
    vm.define_native("bool", Exact(1), bool_fn);
    vm.define_native("range", Range(1, 3), range);
    vm.define_native("assert", Range(1, 2), assert_fn);
    vm.define_native("clock", Exact(0), clock);
    vm.define_native("input", Range(0, 1), input);
    vm.define_native("chr", Exact(1), chr);
    vm.define_native("ord", Exact(1), ord);
    vm.define_native("push", Exact(2), push);
    vm.define_native("pop", Exact(1), pop);
    vm.define_native("keys", Exact(1), keys);
    vm.define_native("values", Exact(1), values);
    vm.define_native("has", Exact(2), has);
    vm.define_native("del", Exact(2), del);
}

fn join_display(vm: &mut Vm, args: &[Value]) -> Result<String, Value> {
    let mut parts = Vec::with_capacity(args.len());
    for &a in args {
        parts.push(vm.to_display(a, false)?);
    }
    Ok(parts.join(" "))
}

fn print(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    let s = join_display(vm, args)?;
    vm.write_out(&s);
    vm.flush_out();
    Ok(Value::Nil)
}

fn println(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    let mut s = join_display(vm, args)?;
    s.push('\n');
    vm.write_out(&s);
    Ok(Value::Nil)
}

fn str_fn(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    let s = vm.to_display(args[0], false)?;
    Ok(vm.new_string(&s))
}

fn type_fn(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    // For class instances, `type` reports the class name (reflection); primitives
    // and other reference types keep their built-in type name.
    if let Value::Obj(r) = args[0] {
        let class = match vm.heap.get(r) {
            Obj::Instance(inst) => Some(inst.class),
            _ => None,
        };
        if let Some(class) = class {
            let name = match vm.heap.get(class) {
                Obj::Class(c) => c.name.clone(),
                _ => String::new(),
            };
            return Ok(vm.new_string(&name));
        }
    }
    let t = vm.type_name(args[0]);
    Ok(vm.new_string(t))
}

fn len(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    let v = args[0];
    let n = match v.as_obj().map(|r| vm.heap.get(r)) {
        Some(Obj::Array(a)) => Some(a.len()),
        Some(Obj::Str(s)) => Some(s.chars().count()),
        Some(Obj::Map(m)) => Some(m.len()),
        _ => None,
    };
    match n {
        Some(n) => Ok(Value::Int(n as i64)),
        None => {
            let t = vm.type_name(v);
            Err(vm.make_error(error_kind::TYPE, format!("len() expects a string, array, or map, got {t}")))
        }
    }
}

fn int_fn(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    match args[0] {
        Value::Int(n) => Ok(Value::Int(n)),
        Value::Float(f) => Ok(Value::Int(f.trunc() as i64)),
        Value::Bool(b) => Ok(Value::Int(b as i64)),
        Value::Obj(r) => {
            let parsed = match vm.heap.get(r) {
                Obj::Str(s) => s.trim().parse::<i64>().ok(),
                _ => None,
            };
            match parsed {
                Some(n) => Ok(Value::Int(n)),
                None => Err(vm.make_error(error_kind::VALUE, "int() could not convert this value")),
            }
        }
        Value::Nil => Err(vm.make_error(error_kind::TYPE, "int() cannot convert nil")),
    }
}

fn float_fn(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    match args[0] {
        Value::Int(n) => Ok(Value::Float(n as f64)),
        Value::Float(f) => Ok(Value::Float(f)),
        Value::Bool(b) => Ok(Value::Float(if b { 1.0 } else { 0.0 })),
        Value::Obj(r) => {
            let parsed = match vm.heap.get(r) {
                Obj::Str(s) => s.trim().parse::<f64>().ok(),
                _ => None,
            };
            match parsed {
                Some(f) => Ok(Value::Float(f)),
                None => Err(vm.make_error(error_kind::VALUE, "float() could not convert this value")),
            }
        }
        Value::Nil => Err(vm.make_error(error_kind::TYPE, "float() cannot convert nil")),
    }
}

fn bool_fn(_vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    Ok(Value::Bool(args[0].is_truthy()))
}

fn range(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    fn as_int(vm: &mut Vm, v: Value) -> Result<i64, Value> {
        match v {
            Value::Int(n) => Ok(n),
            _ => Err(vm.make_error(error_kind::TYPE, "range() arguments must be integers")),
        }
    }
    let (start, end, step) = match args.len() {
        1 => (0, as_int(vm, args[0])?, 1),
        2 => (as_int(vm, args[0])?, as_int(vm, args[1])?, 1),
        _ => (as_int(vm, args[0])?, as_int(vm, args[1])?, as_int(vm, args[2])?),
    };
    if step == 0 {
        return Err(vm.make_error(error_kind::VALUE, "range() step cannot be zero"));
    }
    let mut out = Vec::new();
    let mut i = start;
    if step > 0 {
        while i < end {
            out.push(Value::Int(i));
            i += step;
        }
    } else {
        while i > end {
            out.push(Value::Int(i));
            i += step;
        }
    }
    Ok(vm.new_array(out))
}

fn assert_fn(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    if args[0].is_truthy() {
        return Ok(Value::Nil);
    }
    let msg = if args.len() == 2 {
        vm.to_display(args[1], false)?
    } else {
        "assertion failed".to_string()
    };
    Err(vm.make_error("AssertionError", msg))
}

fn clock(_vm: &mut Vm, _args: &[Value]) -> Result<Value, Value> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    Ok(Value::Float(now))
}

fn input(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    if args.len() == 1 {
        let prompt = vm.to_display(args[0], false)?;
        vm.write_out(&prompt);
        vm.flush_out();
    }
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => Ok(Value::Nil), // EOF
        Ok(_) => {
            let trimmed = line.trim_end_matches(['\n', '\r']);
            Ok(vm.new_string(trimmed))
        }
        Err(_) => Ok(Value::Nil),
    }
}

fn chr(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    match args[0] {
        Value::Int(n) if (0..=0x10FFFF).contains(&n) => match char::from_u32(n as u32) {
            Some(c) => {
                let s = c.to_string();
                Ok(vm.new_string(&s))
            }
            None => Err(vm.make_error(error_kind::VALUE, format!("chr(): {n} is not a valid code point"))),
        },
        Value::Int(n) => Err(vm.make_error(error_kind::VALUE, format!("chr(): {n} is out of range"))),
        _ => Err(vm.make_error(error_kind::TYPE, "chr() expects an integer")),
    }
}

fn ord(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    let code = match args[0].as_obj().map(|r| vm.heap.get(r)) {
        Some(Obj::Str(s)) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c as i64),
                _ => None,
            }
        }
        _ => None,
    };
    match code {
        Some(n) => Ok(Value::Int(n)),
        None => Err(vm.make_error(error_kind::VALUE, "ord() expects a single-character string")),
    }
}

fn push(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    let (arr, x) = (args[0], args[1]);
    let is_array = matches!(arr.as_obj().map(|r| vm.heap.get(r)), Some(Obj::Array(_)));
    if !is_array {
        return Err(vm.make_error(error_kind::TYPE, "push() expects an array as its first argument"));
    }
    let r = arr.as_obj().unwrap();
    if let Obj::Array(a) = vm.heap.get_mut(r) {
        a.push(x);
    }
    vm.write_barrier(r, x); // the array may be old and `x` young
    Ok(Value::Nil)
}

fn pop(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    let arr = args[0];
    let is_array = matches!(arr.as_obj().map(|r| vm.heap.get(r)), Some(Obj::Array(_)));
    if !is_array {
        return Err(vm.make_error(error_kind::TYPE, "pop() expects an array"));
    }
    let r = arr.as_obj().unwrap();
    let popped = if let Obj::Array(a) = vm.heap.get_mut(r) { a.pop() } else { None };
    match popped {
        Some(v) => Ok(v),
        None => Err(vm.make_error(error_kind::INDEX, "pop() from an empty array")),
    }
}

fn keys(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    let ks = match args[0].as_obj().map(|r| vm.heap.get(r)) {
        Some(Obj::Map(m)) => Some(m.keys()),
        _ => None,
    };
    match ks {
        Some(ks) => Ok(vm.new_array(ks)),
        None => Err(vm.make_error(error_kind::TYPE, "keys() expects a map")),
    }
}

fn values(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    let vs = match args[0].as_obj().map(|r| vm.heap.get(r)) {
        Some(Obj::Map(m)) => Some(m.values()),
        _ => None,
    };
    match vs {
        Some(vs) => Ok(vm.new_array(vs)),
        None => Err(vm.make_error(error_kind::TYPE, "values() expects a map")),
    }
}

fn has(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    let (m, k) = (args[0], args[1]);
    let is_map = matches!(m.as_obj().map(|r| vm.heap.get(r)), Some(Obj::Map(_)));
    if !is_map {
        return Err(vm.make_error(error_kind::TYPE, "has() expects a map as its first argument"));
    }
    let key = vm.map_key(k)?;
    let r = m.as_obj().unwrap();
    let present = matches!(vm.heap.get(r), Obj::Map(map) if map.has(key));
    Ok(Value::Bool(present))
}

fn del(vm: &mut Vm, args: &[Value]) -> Result<Value, Value> {
    let (coll, k) = (args[0], args[1]);
    enum Kind {
        Map,
        Array,
        Other,
    }
    let kind = match coll.as_obj().map(|r| vm.heap.get(r)) {
        Some(Obj::Map(_)) => Kind::Map,
        Some(Obj::Array(_)) => Kind::Array,
        _ => Kind::Other,
    };
    match kind {
        Kind::Map => {
            let key = vm.map_key(k)?;
            let r = coll.as_obj().unwrap();
            if let Obj::Map(m) = vm.heap.get_mut(r) {
                m.remove(key);
            }
            Ok(Value::Nil)
        }
        Kind::Array => {
            let idx = match k {
                Value::Int(n) => n,
                _ => return Err(vm.make_error(error_kind::TYPE, "del() array index must be an integer")),
            };
            let r = coll.as_obj().unwrap();
            let len = if let Obj::Array(a) = vm.heap.get(r) { a.len() } else { 0 };
            let i = if idx >= 0 && (idx as usize) < len {
                idx as usize
            } else if idx < 0 && ((-idx) as usize) <= len {
                (len as i64 + idx) as usize
            } else {
                return Err(vm.make_error(error_kind::INDEX, format!("del(): index {idx} out of bounds")));
            };
            if let Obj::Array(a) = vm.heap.get_mut(r) {
                a.remove(i);
            }
            Ok(Value::Nil)
        }
        Kind::Other => Err(vm.make_error(error_kind::TYPE, "del() expects a map or array")),
    }
}
