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
        f(vm, "each", Exact(2), each),
        f(vm, "map", Exact(2), map_values),
        f(vm, "filter", Exact(2), filter),
        f(vm, "clear", Exact(1), clear),
        f(vm, "from_entries", Exact(1), from_entries),
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

/// Snapshot a map's `(key, value)` pairs in insertion order, releasing the heap
/// borrow before any re-entrant callback runs.
fn snapshot(vm: &mut Vm, r: crate::value::GcRef) -> Vec<(Value, Value)> {
    if let Obj::Map(m) = vm.heap.get(r) {
        m.iter().collect()
    } else {
        Vec::new()
    }
}

/// `each(m, f)` -> call `f(key, value)` for every entry in insertion order.
fn each(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    let f = a[1];
    for (k, v) in snapshot(vm, r) {
        vm.call_and_run(f, &[k, v])?;
    }
    Ok(Value::Nil)
}

/// `map(m, f)` -> a new map with the same keys and values replaced by
/// `f(key, value)`. The result is pinned as a temp root across the callbacks.
fn map_values(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    let f = a[1];
    let pairs = snapshot(vm, r);
    let result = Value::Obj(vm.heap.alloc_map(LumMap::new()));
    vm.push_temp_root(result);
    let rr = result.as_obj().unwrap();
    for (k, v) in pairs {
        // Resolve the key *before* the callback: the new value `nv` it returns is
        // only held in a Rust local, so no allocation (e.g. interning a key) may
        // run between getting `nv` and inserting it, or it could be collected.
        let key = match vm.map_key(k) {
            Ok(key) => key,
            Err(e) => {
                vm.pop_temp_root();
                return Err(e);
            }
        };
        match vm.call_and_run(f, &[k, v]) {
            Ok(nv) => {
                if let Obj::Map(m) = vm.heap.get_mut(rr) {
                    m.insert(key, k, nv);
                }
                vm.write_barrier(rr, k);
                vm.write_barrier(rr, nv);
            }
            Err(e) => {
                vm.pop_temp_root();
                return Err(e);
            }
        }
    }
    vm.pop_temp_root();
    Ok(result)
}

/// `filter(m, f)` -> a new map of the entries for which `f(key, value)` is truthy.
fn filter(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    let pred = a[1];
    let pairs = snapshot(vm, r);
    let result = Value::Obj(vm.heap.alloc_map(LumMap::new()));
    vm.push_temp_root(result);
    let rr = result.as_obj().unwrap();
    for (k, v) in pairs {
        match vm.call_and_run(pred, &[k, v]) {
            Ok(keep) if keep.is_truthy() => {
                let key = match vm.map_key(k) {
                    Ok(key) => key,
                    Err(e) => {
                        vm.pop_temp_root();
                        return Err(e);
                    }
                };
                if let Obj::Map(m) = vm.heap.get_mut(rr) {
                    m.insert(key, k, v);
                }
                vm.write_barrier(rr, k);
                vm.write_barrier(rr, v);
            }
            Ok(_) => {}
            Err(e) => {
                vm.pop_temp_root();
                return Err(e);
            }
        }
    }
    vm.pop_temp_root();
    Ok(result)
}

/// `clear(m)` -> remove every entry in place; returns the (now empty) map.
fn clear(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let r = require_map(vm, a[0])?;
    if let Obj::Map(m) = vm.heap.get_mut(r) {
        *m = LumMap::new();
    }
    Ok(a[0])
}

/// `from_entries(pairs)` -> a new map built from an array of `[key, value]`
/// arrays (the inverse of `entries`).
fn from_entries(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let pairs = super::array_of(vm, a[0])?;
    let mut out = LumMap::new();
    for pair in pairs {
        let kv = super::array_of(vm, pair)?;
        if kv.len() != 2 {
            return Err(err(vm, error_kind::VALUE, "from_entries() expects [key, value] pairs"));
        }
        let key = vm.map_key(kv[0])?;
        out.insert(key, kv[0], kv[1]);
    }
    Ok(Value::Obj(vm.heap.alloc_map(out)))
}
