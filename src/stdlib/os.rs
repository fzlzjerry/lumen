//! The `os` module: process and environment access.

use super::{int, string_of, Vm};
use crate::object::Arity::{self, Exact, Range};
use crate::value::Value;

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "args", Exact(0), args),
        f(vm, "env", Range(1, 2), env),
        f(vm, "platform", Exact(0), platform),
        f(vm, "cwd", Exact(0), cwd),
        f(vm, "exit", Range(0, 1), exit),
    ];
    vm.make_module("os", exports)
}

fn args(vm: &mut Vm, _a: &[Value]) -> Result<Value, Value> {
    let owned: Vec<String> = vm.args().to_vec();
    let vals: Vec<Value> = owned.iter().map(|s| vm.new_string(s)).collect();
    Ok(vm.new_array(vals))
}

fn env(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let name = string_of(vm, a[0])?;
    match std::env::var(&name) {
        Ok(v) => Ok(vm.new_string(&v)),
        Err(_) => Ok(if a.len() == 2 { a[1] } else { Value::Nil }),
    }
}

fn platform(vm: &mut Vm, _a: &[Value]) -> Result<Value, Value> {
    Ok(vm.new_string(std::env::consts::OS))
}

fn cwd(vm: &mut Vm, _a: &[Value]) -> Result<Value, Value> {
    let dir = std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    Ok(vm.new_string(&dir))
}

fn exit(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let code = if a.is_empty() { 0 } else { int(vm, a[0])? as i32 };
    vm.flush_out();
    std::process::exit(code);
}
