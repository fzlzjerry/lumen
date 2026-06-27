//! The `os` module: process and environment access.

use super::{array_of, err, int, string_of, Vm};
use crate::object::Arity::{self, Exact, Range};
use crate::object::{LumMap, Obj};
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "args", Exact(0), args),
        f(vm, "env", Range(1, 2), env),
        f(vm, "platform", Exact(0), platform),
        f(vm, "cwd", Exact(0), cwd),
        f(vm, "exec", Exact(2), exec),
        f(vm, "exit", Range(0, 1), exit),
    ];
    vm.make_module("os", exports)
}

/// `exec(cmd, args)` runs `cmd` with the string arguments in `args` and returns
/// `{status, stdout, stderr}` (DESIGN: a zero-dependency `std::process` wrapper).
/// `status` is the process exit code (or -1 if killed by a signal).
fn exec(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let cmd = string_of(vm, a[0])?;
    let argv = array_of(vm, a[1])?;
    let mut args = Vec::with_capacity(argv.len());
    for v in &argv {
        args.push(string_of(vm, *v)?);
    }
    match std::process::Command::new(&cmd).args(&args).output() {
        Ok(out) => {
            let status = out.status.code().unwrap_or(-1) as i64;
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            let sout = vm.new_string(&stdout);
            let serr = vm.new_string(&stderr);
            make_map(vm, &[("status", Value::Int(status)), ("stdout", sout), ("stderr", serr)])
        }
        Err(e) => Err(err(vm, error_kind::VALUE, format!("failed to run '{cmd}': {e}"))),
    }
}

/// Build a map from `(key, value)` pairs, GC-safely: the result is pinned as a
/// temp root, and each freshly-interned key is inserted immediately.
fn make_map(vm: &mut Vm, entries: &[(&str, Value)]) -> Result<Value, Value> {
    let result = Value::Obj(vm.heap.alloc_map(LumMap::new()));
    vm.push_temp_root(result);
    let rr = result.as_obj().unwrap();
    for &(name, val) in entries {
        let key = vm.new_string(name);
        let norm = match vm.map_key(key) {
            Ok(n) => n,
            Err(e) => {
                vm.pop_temp_root();
                return Err(e);
            }
        };
        if let Obj::Map(m) = vm.heap.get_mut(rr) {
            m.insert(norm, key, val);
        }
        vm.write_barrier(rr, key);
        vm.write_barrier(rr, val);
    }
    vm.pop_temp_root();
    Ok(result)
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
