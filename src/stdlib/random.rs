//! The `random` module: pseudo-random numbers from the VM's xorshift64* PRNG.
//! Deterministic after `random.seed(n)`.

use super::{array_of, err, int, num, Vm};
use crate::object::Arity::{self, Exact};
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "random", Exact(0), random),
        f(vm, "randint", Exact(2), randint),
        f(vm, "uniform", Exact(2), uniform),
        f(vm, "randrange", Exact(2), randrange),
        f(vm, "choice", Exact(1), choice),
        f(vm, "sample", Exact(2), sample),
        f(vm, "shuffle", Exact(1), shuffle),
        f(vm, "gauss", Exact(2), gauss),
        f(vm, "seed", Exact(1), seed),
    ];
    vm.make_module("random", exports)
}

/// A uniform double in `[0, 1)` from the next PRNG word (53 mantissa bits).
fn unit_f64(vm: &mut Vm) -> f64 {
    let bits = vm.next_random_u64();
    (bits >> 11) as f64 / (1u64 << 53) as f64
}

/// A float in `[0, 1)`.
fn random(vm: &mut Vm, _a: &[Value]) -> Result<Value, Value> {
    Ok(Value::Float(unit_f64(vm)))
}

/// A float in `[a, b)`.
fn uniform(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let lo = num(vm, a[0])?;
    let hi = num(vm, a[1])?;
    Ok(Value::Float(lo + unit_f64(vm) * (hi - lo)))
}

/// An integer in `[lo, hi)` (hi exclusive). Throws `ValueError` if `lo >= hi`.
fn randrange(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let lo = int(vm, a[0])?;
    let hi = int(vm, a[1])?;
    if lo >= hi {
        return Err(err(
            vm,
            error_kind::VALUE,
            "randrange(lo, hi) requires lo < hi",
        ));
    }
    let span = (hi - lo) as u64;
    let r = vm.next_random_u64() % span;
    Ok(Value::Int(lo + r as i64))
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

/// `sample(seq, k)` -> a new array of `k` **distinct** elements drawn without
/// replacement. Throws `ValueError` if `k < 0` or `k > len(seq)`.
fn sample(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let mut items = array_of(vm, a[0])?;
    let k = int(vm, a[1])?;
    let n = items.len();
    if k < 0 || k as u64 > n as u64 {
        return Err(err(
            vm,
            error_kind::VALUE,
            "sample(seq, k) requires 0 <= k <= len(seq)",
        ));
    }
    let k = k as usize;
    // Partial Fisher–Yates: after `i` picks, `items[..i]` holds the sample so far.
    for i in 0..k {
        let j = i + (vm.next_random_u64() % (n - i) as u64) as usize;
        items.swap(i, j);
    }
    items.truncate(k);
    Ok(vm.new_array(items))
}

/// `gauss(mu, sigma)` -> a normal deviate via the Box–Muller transform.
fn gauss(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let mu = num(vm, a[0])?;
    let sigma = num(vm, a[1])?;
    // u1 in (0, 1] avoids ln(0); u2 in [0, 1).
    let u1 = 1.0 - unit_f64(vm);
    let u2 = unit_f64(vm);
    let z = (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos();
    Ok(Value::Float(mu + sigma * z))
}

fn seed(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let n = int(vm, a[0])?;
    vm.set_random_seed(n as u64);
    Ok(Value::Nil)
}
