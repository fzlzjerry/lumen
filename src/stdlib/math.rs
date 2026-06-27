//! The `math` module: numeric constants and functions.

use super::{err, int, num, Vm};
use crate::object::Arity::{self, Exact, Range};
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        ("pi", Value::Float(std::f64::consts::PI)),
        ("e", Value::Float(std::f64::consts::E)),
        ("tau", Value::Float(std::f64::consts::TAU)),
        ("inf", Value::Float(f64::INFINITY)),
        ("nan", Value::Float(f64::NAN)),
        f(vm, "sqrt", Exact(1), sqrt),
        f(vm, "cbrt", Exact(1), cbrt),
        f(vm, "pow", Exact(2), pow),
        f(vm, "exp", Exact(1), exp),
        f(vm, "log", Range(1, 2), log),
        f(vm, "log2", Exact(1), log2),
        f(vm, "log10", Exact(1), log10),
        f(vm, "sin", Exact(1), sin),
        f(vm, "cos", Exact(1), cos),
        f(vm, "tan", Exact(1), tan),
        f(vm, "asin", Exact(1), asin),
        f(vm, "acos", Exact(1), acos),
        f(vm, "atan", Exact(1), atan),
        f(vm, "atan2", Exact(2), atan2),
        f(vm, "sinh", Exact(1), sinh),
        f(vm, "cosh", Exact(1), cosh),
        f(vm, "tanh", Exact(1), tanh),
        f(vm, "asinh", Exact(1), asinh),
        f(vm, "acosh", Exact(1), acosh),
        f(vm, "atanh", Exact(1), atanh),
        f(vm, "clamp", Exact(3), clamp),
        f(vm, "factorial", Exact(1), factorial),
        f(vm, "abs", Exact(1), abs),
        f(vm, "floor", Exact(1), floor),
        f(vm, "ceil", Exact(1), ceil),
        f(vm, "round", Range(1, 2), round),
        f(vm, "trunc", Exact(1), trunc),
        f(vm, "sign", Exact(1), sign),
        f(vm, "min", Exact(2), min),
        f(vm, "max", Exact(2), max),
        f(vm, "hypot", Exact(2), hypot),
        f(vm, "gcd", Exact(2), gcd),
        f(vm, "lcm", Exact(2), lcm),
        f(vm, "is_nan", Exact(1), is_nan),
        f(vm, "is_finite", Exact(1), is_finite),
        f(vm, "degrees", Exact(1), degrees),
        f(vm, "radians", Exact(1), radians),
    ];
    vm.make_module("math", exports)
}

fn unary(vm: &mut Vm, args: &[Value], op: fn(f64) -> f64) -> Result<Value, Value> {
    let x = num(vm, args[0])?;
    Ok(Value::Float(op(x)))
}

fn sqrt(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::sqrt)
}
fn cbrt(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::cbrt)
}
fn exp(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::exp)
}
/// `log(x)` is the natural logarithm; `log(x, base)` is the base-`base` logarithm,
/// computed as `x.ln() / base.ln()`.
fn log(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let x = num(vm, a[0])?;
    if a.len() == 2 {
        let base = num(vm, a[1])?;
        Ok(Value::Float(x.ln() / base.ln()))
    } else {
        Ok(Value::Float(x.ln()))
    }
}
fn log2(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::log2)
}
fn log10(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::log10)
}
fn sin(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::sin)
}
fn cos(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::cos)
}
fn tan(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::tan)
}
fn asin(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::asin)
}
fn acos(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::acos)
}
fn atan(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::atan)
}
fn sinh(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::sinh)
}
fn cosh(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::cosh)
}
fn tanh(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::tanh)
}
fn asinh(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::asinh)
}
fn acosh(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::acosh)
}
fn atanh(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    unary(vm, a, f64::atanh)
}

fn pow(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (x, y) = (num(vm, a[0])?, num(vm, a[1])?);
    Ok(Value::Float(x.powf(y)))
}

fn atan2(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (y, x) = (num(vm, a[0])?, num(vm, a[1])?);
    Ok(Value::Float(y.atan2(x)))
}

fn hypot(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (x, y) = (num(vm, a[0])?, num(vm, a[1])?);
    Ok(Value::Float(x.hypot(y)))
}

/// `abs` preserves the operand's int/float-ness.
fn abs(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    match a[0] {
        Value::Int(n) => Ok(Value::Int(n.wrapping_abs())),
        Value::Float(f) => Ok(Value::Float(f.abs())),
        _ => Err(vm.make_error(crate::value::error_kind::TYPE, "abs() expects a number")),
    }
}

/// `floor`/`ceil`/`round`/`trunc` return integers (the conceptual result).
fn to_int(vm: &mut Vm, args: &[Value], op: fn(f64) -> f64) -> Result<Value, Value> {
    match args[0] {
        Value::Int(n) => Ok(Value::Int(n)),
        Value::Float(f) => Ok(Value::Int(op(f) as i64)),
        _ => Err(vm.make_error(crate::value::error_kind::TYPE, "expected a number")),
    }
}
fn floor(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    to_int(vm, a, f64::floor)
}
fn ceil(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    to_int(vm, a, f64::ceil)
}
/// `round(x)` rounds to the nearest integer; `round(x, ndigits)` rounds to
/// `ndigits` decimal places and returns a float (`round(3.14159, 2) == 3.14`).
fn round(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    if a.len() == 1 {
        return to_int(vm, a, f64::round);
    }
    let x = num(vm, a[0])?;
    let ndigits = int(vm, a[1])?;
    let factor = 10f64.powi(ndigits as i32);
    Ok(Value::Float((x * factor).round() / factor))
}
fn trunc(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    to_int(vm, a, f64::trunc)
}

fn sign(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let x = num(vm, a[0])?;
    Ok(Value::Int(if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }))
}

/// `min`/`max` return the smaller/larger argument unchanged (preserving type).
fn min(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (x, y) = (num(vm, a[0])?, num(vm, a[1])?);
    Ok(if x <= y { a[0] } else { a[1] })
}
fn max(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (x, y) = (num(vm, a[0])?, num(vm, a[1])?);
    Ok(if x >= y { a[0] } else { a[1] })
}

fn gcd(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (mut x, mut y) = (super::int(vm, a[0])?.abs(), super::int(vm, a[1])?.abs());
    while y != 0 {
        let t = y;
        y = x % y;
        x = t;
    }
    Ok(Value::Int(x))
}

/// `lcm(a, b)` -> least common multiple; `0` if either operand is `0`.
fn lcm(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (x, y) = (super::int(vm, a[0])?.abs(), super::int(vm, a[1])?.abs());
    if x == 0 || y == 0 {
        return Ok(Value::Int(0));
    }
    let (mut g, mut b) = (x, y);
    while b != 0 {
        let t = b;
        b = g % b;
        g = t;
    }
    Ok(Value::Int((x / g).wrapping_mul(y)))
}

/// `clamp(x, lo, hi)` constrains `x` to `[lo, hi]`, returning the selected operand
/// **unchanged** so its int/float type is preserved (like `min`/`max`). Throws
/// `ValueError` if `lo > hi`.
fn clamp(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let x = num(vm, a[0])?;
    let lo = num(vm, a[1])?;
    let hi = num(vm, a[2])?;
    if lo > hi {
        return Err(err(vm, error_kind::VALUE, "clamp: lo must be <= hi"));
    }
    Ok(if x < lo {
        a[1]
    } else if x > hi {
        a[2]
    } else {
        a[0]
    })
}

/// `factorial(n)` for a non-negative integer `n`, with checked multiplication.
/// Throws `ValueError` on a negative argument or on i64 overflow (never wraps).
fn factorial(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let n = int(vm, a[0])?;
    if n < 0 {
        return Err(err(
            vm,
            error_kind::VALUE,
            "factorial: argument must be non-negative",
        ));
    }
    let mut acc: i64 = 1;
    for k in 1..=n {
        acc = match acc.checked_mul(k) {
            Some(v) => v,
            None => return Err(err(vm, error_kind::VALUE, "factorial: result overflows")),
        };
    }
    Ok(Value::Int(acc))
}

fn is_nan(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    Ok(Value::Bool(num(vm, a[0])?.is_nan()))
}
fn is_finite(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    Ok(Value::Bool(num(vm, a[0])?.is_finite()))
}

/// Radians -> degrees and back, for any numeric operand.
fn degrees(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    Ok(Value::Float(num(vm, a[0])?.to_degrees()))
}
fn radians(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    Ok(Value::Float(num(vm, a[0])?.to_radians()))
}
