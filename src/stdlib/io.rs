//! The `io` module: file and stream I/O.

use super::{err, string_of, Vm};
use crate::object::Arity::{self, Exact};
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "read_file", Exact(1), read_file),
        f(vm, "write_file", Exact(2), write_file),
        f(vm, "append_file", Exact(2), append_file),
        f(vm, "exists", Exact(1), exists),
        f(vm, "lines", Exact(1), lines),
        f(vm, "eprint", Exact(1), eprint),
        f(vm, "eprintln", Exact(1), eprintln),
    ];
    vm.make_module("io", exports)
}

fn read_file(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(vm.new_string(&s)),
        Err(e) => Err(err(vm, error_kind::VALUE, format!("cannot read '{path}': {e}"))),
    }
}

fn write_file(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    let content = vm.to_display(a[1], false)?;
    match std::fs::write(&path, content) {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Err(err(vm, error_kind::VALUE, format!("cannot write '{path}': {e}"))),
    }
}

fn append_file(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    use std::io::Write;
    let path = string_of(vm, a[0])?;
    let content = vm.to_display(a[1], false)?;
    let res = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(content.as_bytes()));
    match res {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Err(err(vm, error_kind::VALUE, format!("cannot append to '{path}': {e}"))),
    }
}

fn exists(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    Ok(Value::Bool(std::path::Path::new(&path).exists()))
}

fn lines(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            let vals: Vec<Value> = s.lines().map(|l| vm.new_string(l)).collect();
            Ok(vm.new_array(vals))
        }
        Err(e) => Err(err(vm, error_kind::VALUE, format!("cannot read '{path}': {e}"))),
    }
}

fn eprint(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = vm.to_display(a[0], false)?;
    eprint!("{s}");
    Ok(Value::Nil)
}
fn eprintln(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = vm.to_display(a[0], false)?;
    eprintln!("{s}");
    Ok(Value::Nil)
}
