//! The `map` module: operations over maps (hash tables).

use super::{err, Vm};
use crate::object::Arity::{self, Exact, Range};
use crate::object::{LumMap, Obj};
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "get", Range(2, 3), get),
        f(vm, "set", Exact(3), set),
        f(vm, "has", Exact(2), has),
        f(vm, "remove", Exact(2), remove),
        f(vm, "keys", Exact(1), keys),
        f(vm, "values", Exact(1), values),
        f(vm, "len", Exact(1), len),
        f(vm, "entries", Exact(1), entries),
        f(vm, "merge", Exact(2), merge),
    ];
    vm.make_module("map", exports)
}

fn require_map(vm: &mut Vm, v: Value) -> Result<crate::value::GcRef, Value> {
    match v.as_obj() {
        Some(r) if matches!(vm.heap.get(r), Obj::Map(_)) => Ok(r),
        _ => Err(err(vm, error_kind::TYPE, "expected a map")),
    }
}

fn get(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    let key = vm.map_key(a[1])?;
    let found = if let Obj::Map(m) = vm.heap.get(r) { m.get(key) } else { None };
    Ok(found.unwrap_or_else(|| if a.len() == 3 { a[2] } else { Value::Nil }))
}

fn set(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    let key = vm.map_key(a[1])?;
    if let Obj::Map(m) = vm.heap.get_mut(r) {
        m.insert(key, a[1], a[2]);
    }
    vm.write_barrier(r, a[1]); // the map may be old and the key/value young
    vm.write_barrier(r, a[2]);
    Ok(a[0])
}

fn has(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    let key = vm.map_key(a[1])?;
    let present = matches!(vm.heap.get(r), Obj::Map(m) if m.has(key));
    Ok(Value::Bool(present))
}

fn remove(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    let key = vm.map_key(a[1])?;
    let removed = if let Obj::Map(m) = vm.heap.get_mut(r) { m.remove(key) } else { false };
    Ok(Value::Bool(removed))
}

fn keys(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    let ks = if let Obj::Map(m) = vm.heap.get(r) { m.keys() } else { Vec::new() };
    Ok(vm.new_array(ks))
}

fn values(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    let vs = if let Obj::Map(m) = vm.heap.get(r) { m.values() } else { Vec::new() };
    Ok(vm.new_array(vs))
}

fn len(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    let n = if let Obj::Map(m) = vm.heap.get(r) { m.len() } else { 0 };
    Ok(Value::Int(n as i64))
}

/// `entries(m)` -> array of `[key, value]` pairs in insertion order.
fn entries(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    let pairs: Vec<(Value, Value)> =
        if let Obj::Map(m) = vm.heap.get(r) { m.iter().collect() } else { Vec::new() };
    let mut out = Vec::with_capacity(pairs.len());
    let result = vm.new_array(Vec::new());
    vm.push_temp_root(result);
    for (k, v) in pairs {
        out.push(vm.new_array(vec![k, v]));
    }
    if let Obj::Array(arr) = vm.heap.get_mut(result.as_obj().unwrap()) {
        *arr = out;
    }
    vm.pop_temp_root();
    Ok(result)
}

/// `merge(a, b)` -> a new map with `b`'s entries layered over a copy of `a`.
fn merge(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let ra = require_map(vm, a[0])?;
    let rb = require_map(vm, a[1])?;
    let mut out = LumMap::new();
    let a_entries: Vec<(Value, Value)> =
        if let Obj::Map(m) = vm.heap.get(ra) { m.iter().collect() } else { Vec::new() };
    let b_entries: Vec<(Value, Value)> =
        if let Obj::Map(m) = vm.heap.get(rb) { m.iter().collect() } else { Vec::new() };
    for (k, v) in a_entries.into_iter().chain(b_entries) {
        let key = vm.map_key(k)?;
        out.insert(key, k, v);
    }
    Ok(Value::Obj(vm.heap.alloc_map(out)))
}
