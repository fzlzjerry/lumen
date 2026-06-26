//! The `math` module: numeric constants and functions.

use super::{num, Vm};
use crate::object::Arity::{self, Exact};
use crate::value::Value;

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
        f(vm, "log", Exact(1), log),
        f(vm, "log2", Exact(1), log2),
        f(vm, "log10", Exact(1), log10),
        f(vm, "sin", Exact(1), sin),
        f(vm, "cos", Exact(1), cos),
        f(vm, "tan", Exact(1), tan),
        f(vm, "asin", Exact(1), asin),
        f(vm, "acos", Exact(1), acos),
        f(vm, "atan", Exact(1), atan),
        f(vm, "atan2", Exact(2), atan2),
        f(vm, "abs", Exact(1), abs),
        f(vm, "floor", Exact(1), floor),
        f(vm, "ceil", Exact(1), ceil),
        f(vm, "round", Exact(1), round),
        f(vm, "trunc", Exact(1), trunc),
        f(vm, "sign", Exact(1), sign),
        f(vm, "min", Exact(2), min),
        f(vm, "max", Exact(2), max),
        f(vm, "hypot", Exact(2), hypot),
        f(vm, "gcd", Exact(2), gcd),
    ];
    vm.make_module("math", exports)
}

fn unary(vm: &mut Vm, args: &[Value], op: fn(f64) -> f64) -> Result<Value, Value> {
    let x = num(vm, args[0])?;
    Ok(Value::Float(op(x)))
}

fn sqrt(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::sqrt) }
fn cbrt(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::cbrt) }
fn exp(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::exp) }
fn log(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::ln) }
fn log2(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::log2) }
fn log10(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::log10) }
fn sin(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::sin) }
fn cos(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::cos) }
fn tan(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::tan) }
fn asin(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::asin) }
fn acos(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::acos) }
fn atan(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { unary(vm, a, f64::atan) }

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
fn floor(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { to_int(vm, a, f64::floor) }
fn ceil(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { to_int(vm, a, f64::ceil) }
fn round(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { to_int(vm, a, f64::round) }
fn trunc(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> { to_int(vm, a, f64::trunc) }

fn sign(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let x = num(vm, a[0])?;
    Ok(Value::Int(if x > 0.0 { 1 } else if x < 0.0 { -1 } else { 0 }))
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
