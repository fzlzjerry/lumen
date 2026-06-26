//! The `array` module: functional and aggregate operations over arrays.
//!
//! The higher-order functions (`map`/`filter`/`reduce`/`sort`) call back into
//! Lumen via `call_and_run`, so they pin their accumulators as temporary GC
//! roots (DESIGN D18) before the re-entrant call. Source elements stay alive via
//! the still-on-stack argument array.

use super::{array_of, err, int, num, Vm};
use crate::object::Arity::{self, Exact, Range};
use crate::object::Obj;
use crate::value::{error_kind, Value};
use std::cmp::Ordering;

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "sum", Exact(1), sum),
        f(vm, "min", Exact(1), arr_min),
        f(vm, "max", Exact(1), arr_max),
        f(vm, "map", Exact(2), map),
        f(vm, "filter", Exact(2), filter),
        f(vm, "reduce", Exact(3), reduce),
        f(vm, "each", Exact(2), each),
        f(vm, "sort", Range(1, 2), sort),
        f(vm, "reverse", Exact(1), reverse),
        f(vm, "contains", Exact(2), contains),
        f(vm, "index_of", Exact(2), index_of),
        f(vm, "slice", Exact(3), slice),
        f(vm, "concat", Exact(2), concat),
        f(vm, "first", Exact(1), first),
        f(vm, "last", Exact(1), last),
        f(vm, "flatten", Exact(1), flatten),
        f(vm, "find", Exact(2), find),
        f(vm, "find_index", Exact(2), find_index),
        f(vm, "any", Exact(2), any),
        f(vm, "all", Exact(2), all),
        f(vm, "unique", Exact(1), unique),
        f(vm, "zip", Exact(2), zip),
    ];
    vm.make_module("array", exports)
}

fn sum(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let mut int_sum: i64 = 0;
    let mut float_sum: f64 = 0.0;
    let mut is_float = false;
    for it in &items {
        match it {
            Value::Int(n) => {
                int_sum = int_sum.wrapping_add(*n);
                float_sum += *n as f64;
            }
            Value::Float(x) => {
                is_float = true;
                float_sum += *x;
            }
            _ => return Err(err(vm, error_kind::TYPE, "sum() expects an array of numbers")),
        }
    }
    Ok(if is_float { Value::Float(float_sum) } else { Value::Int(int_sum) })
}

fn arr_min(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    fold_extreme(vm, a, true)
}
fn arr_max(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    fold_extreme(vm, a, false)
}
fn fold_extreme(vm: &mut Vm, a: &[Value], want_min: bool) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    if items.is_empty() {
        return Err(err(vm, error_kind::VALUE, "min()/max() of an empty array"));
    }
    let mut best = items[0];
    let mut best_n = num(vm, best)?;
    for &it in &items[1..] {
        let n = num(vm, it)?;
        if (want_min && n < best_n) || (!want_min && n > best_n) {
            best = it;
            best_n = n;
        }
    }
    Ok(best)
}

fn map(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let f = a[1];
    let result = vm.new_array(Vec::with_capacity(items.len()));
    vm.push_temp_root(result); // keep partial result alive across callbacks
    let r = result.as_obj().unwrap();
    for it in items {
        match vm.call_and_run(f, &[it]) {
            Ok(mapped) => {
                if let Obj::Array(arr) = vm.heap.get_mut(r) {
                    arr.push(mapped);
                }
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

fn filter(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let pred = a[1];
    let result = vm.new_array(Vec::new());
    vm.push_temp_root(result);
    let r = result.as_obj().unwrap();
    for it in items {
        match vm.call_and_run(pred, &[it]) {
            Ok(keep) => {
                if keep.is_truthy() {
                    if let Obj::Array(arr) = vm.heap.get_mut(r) {
                        arr.push(it);
                    }
                }
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

fn reduce(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let f = a[1];
    let mut acc = a[2];
    vm.push_temp_root(acc); // the accumulator may be a heap value
    for it in items {
        match vm.call_and_run(f, &[acc, it]) {
            Ok(next) => {
                acc = next;
                vm.update_top_temp_root(acc);
            }
            Err(e) => {
                vm.pop_temp_root();
                return Err(e);
            }
        }
    }
    vm.pop_temp_root();
    Ok(acc)
}

fn each(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let f = a[1];
    for it in items {
        vm.call_and_run(f, &[it])?;
    }
    Ok(Value::Nil)
}

/// Sort, optionally with a comparator returning a number (`<0`, `0`, `>0`).
/// Implemented as a stable merge sort so a misbehaving user comparator can never
/// trip a standard-library sort panic.
fn sort(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let cmp = a.get(1).copied();
    let sorted = merge_sort(vm, items, cmp)?;
    Ok(vm.new_array(sorted))
}

fn merge_sort(vm: &mut Vm, items: Vec<Value>, cmp: Option<Value>) -> Result<Vec<Value>, Value> {
    if items.len() <= 1 {
        return Ok(items);
    }
    let mid = items.len() / 2;
    let mut left = items;
    let right = left.split_off(mid);
    let left = merge_sort(vm, left, cmp)?;
    let right = merge_sort(vm, right, cmp)?;
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        let ord = compare(vm, left[i], right[j], cmp)?;
        if ord <= Ordering::Equal {
            out.push(left[i]);
            i += 1;
        } else {
            out.push(right[j]);
            j += 1;
        }
    }
    out.extend_from_slice(&left[i..]);
    out.extend_from_slice(&right[j..]);
    Ok(out)
}

fn compare(vm: &mut Vm, a: Value, b: Value, cmp: Option<Value>) -> Result<Ordering, Value> {
    match cmp {
        Some(f) => {
            let r = vm.call_and_run(f, &[a, b])?;
            let n = r.as_f64().ok_or_else(|| err(vm, error_kind::TYPE, "comparator must return a number"))?;
            Ok(n.partial_cmp(&0.0).unwrap_or(Ordering::Equal))
        }
        None => {
            // Default: numeric or lexicographic, else a type error.
            if let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) {
                Ok(x.partial_cmp(&y).unwrap_or(Ordering::Equal))
            } else {
                let sa = vm.to_display(a, false)?;
                let sb = vm.to_display(b, false)?;
                Ok(sa.cmp(&sb))
            }
        }
    }
}

fn reverse(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let mut items = array_of(vm, a[0])?;
    items.reverse();
    Ok(vm.new_array(items))
}

fn contains(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let target = a[1];
    Ok(Value::Bool(items.iter().any(|&it| vm.values_equal(it, target))))
}

fn index_of(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let target = a[1];
    let pos = items.iter().position(|&it| vm.values_equal(it, target));
    Ok(Value::Int(pos.map(|p| p as i64).unwrap_or(-1)))
}

fn slice(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let len = items.len() as i64;
    let norm = |i: i64| -> usize {
        if i < 0 { ((len + i).max(0)) as usize } else { (i.min(len)) as usize }
    };
    let start = norm(int(vm, a[1])?);
    let end = norm(int(vm, a[2])?);
    let out = if start < end { items[start..end].to_vec() } else { Vec::new() };
    Ok(vm.new_array(out))
}

fn concat(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let mut x = array_of(vm, a[0])?;
    let y = array_of(vm, a[1])?;
    x.extend(y);
    Ok(vm.new_array(x))
}

fn first(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    Ok(items.first().copied().unwrap_or(Value::Nil))
}
fn last(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    Ok(items.last().copied().unwrap_or(Value::Nil))
}

fn flatten(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let mut out = Vec::new();
    for it in items {
        match it.as_obj().map(|r| vm.heap.get(r)) {
            Some(Obj::Array(inner)) => out.extend(inner.clone()),
            _ => out.push(it),
        }
    }
    Ok(vm.new_array(out))
}

/// `find(arr, pred)` -> the first element for which `pred` is truthy, else `nil`.
/// The source array stays on the stack as an argument, keeping the candidate
/// elements rooted across the re-entrant predicate call (like `each`).
fn find(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let pred = a[1];
    for it in items {
        if vm.call_and_run(pred, &[it])?.is_truthy() {
            return Ok(it);
        }
    }
    Ok(Value::Nil)
}

/// `find_index(arr, pred)` -> the index of the first match, or `-1`.
fn find_index(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let pred = a[1];
    for (i, it) in items.into_iter().enumerate() {
        if vm.call_and_run(pred, &[it])?.is_truthy() {
            return Ok(Value::Int(i as i64));
        }
    }
    Ok(Value::Int(-1))
}

/// `any(arr, pred)` -> `true` iff `pred` is truthy for at least one element.
fn any(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let pred = a[1];
    for it in items {
        if vm.call_and_run(pred, &[it])?.is_truthy() {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

/// `all(arr, pred)` -> `true` iff `pred` is truthy for every element.
fn all(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let pred = a[1];
    for it in items {
        if !vm.call_and_run(pred, &[it])?.is_truthy() {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

/// `unique(arr)` -> a new array with duplicates removed, keeping first-seen
/// order. Equality is structural (`values_equal`), like `contains`/`index_of`.
fn unique(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let mut out: Vec<Value> = Vec::new();
    for it in items {
        if !out.iter().any(|&seen| vm.values_equal(seen, it)) {
            out.push(it);
        }
    }
    Ok(vm.new_array(out))
}

/// `zip(xs, ys)` -> an array of `[x, y]` pairs, truncated to the shorter input.
/// Roots the partial result across the pair allocations (like `map`).
fn zip(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let xs = array_of(vm, a[0])?;
    let ys = array_of(vm, a[1])?;
    let n = xs.len().min(ys.len());
    let result = vm.new_array(Vec::with_capacity(n));
    vm.push_temp_root(result);
    let r = result.as_obj().unwrap();
    for i in 0..n {
        let pair = vm.new_array(vec![xs[i], ys[i]]);
        if let Obj::Array(arr) = vm.heap.get_mut(r) {
            arr.push(pair);
        }
    }
    vm.pop_temp_root();
    Ok(result)
}
