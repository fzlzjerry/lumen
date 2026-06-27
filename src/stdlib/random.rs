//! The `random` module: pseudo-random numbers from the VM's xorshift64* PRNG.
//! Deterministic after `random.seed(n)`.

use super::{array_of, err, int, Vm};
use crate::object::Arity::{self, Exact};
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "random", Exact(0), random),
        f(vm, "randint", Exact(2), randint),
        f(vm, "choice", Exact(1), choice),
        f(vm, "shuffle", Exact(1), shuffle),
        f(vm, "seed", Exact(1), seed),
    ];
    vm.make_module("random", exports)
}

/// A float in `[0, 1)`.
fn random(vm: &mut Vm, _a: &[Value]) -> Result<Value, Value> {
    let bits = vm.next_random_u64();
    // 53 bits of mantissa gives a uniform double in [0, 1).
    Ok(Value::Float((bits >> 11) as f64 / (1u64 << 53) as f64))
}

/// An integer in `[lo, hi]` inclusive.
fn randint(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let lo = int(vm, a[0])?;
    let hi = int(vm, a[1])?;
    if lo > hi {
        return Err(err(
            vm,
            error_kind::VALUE,
            "randint(lo, hi) requires lo <= hi",
        ));
    }
    let span = (hi - lo) as u64 + 1;
    let r = vm.next_random_u64() % span;
    Ok(Value::Int(lo + r as i64))
}

fn choice(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    if items.is_empty() {
        return Err(err(vm, error_kind::VALUE, "choice() of an empty array"));
    }
    let idx = (vm.next_random_u64() % items.len() as u64) as usize;
    Ok(items[idx])
}

/// Return a new array, Fisher–Yates shuffled.
fn shuffle(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let mut items = array_of(vm, a[0])?;
    let n = items.len();
    for i in (1..n).rev() {
        let j = (vm.next_random_u64() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
    Ok(vm.new_array(items))
}

fn seed(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let n = int(vm, a[0])?;
    vm.set_random_seed(n as u64);
    Ok(Value::Nil)
}
