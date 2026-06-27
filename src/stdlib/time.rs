//! The `time` module: wall-clock time and sleeping.

use super::{num, Vm};
use crate::object::Arity::{self, Exact};
use crate::value::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "now", Exact(0), now),
        f(vm, "now_millis", Exact(0), now_millis),
        f(vm, "sleep", Exact(1), sleep),
    ];
    vm.make_module("time", exports)
}

fn now(_vm: &mut Vm, _a: &[Value]) -> Result<Value, Value> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    Ok(Value::Float(secs))
}

fn now_millis(_vm: &mut Vm, _a: &[Value]) -> Result<Value, Value> {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(Value::Int(ms as i64))
}

fn sleep(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let secs = num(vm, a[0])?.max(0.0);
    std::thread::sleep(std::time::Duration::from_secs_f64(secs));
    Ok(Value::Nil)
}
