//! The Lumen standard library: native modules loaded via `import`.
//!
//! [`install`] wires a loader into the VM so `import "math"`, `import "json"`,
//! etc. resolve to Rust-backed modules (built once and cached). One module
//! (`seq`) is *self-hosted* — written in Lumen and embedded as source — to prove
//! the language can implement its own library. Each module's `build(vm)` returns
//! a module value; the small `args` helpers below give every native function a
//! uniform, type-checked way to read its arguments.

use crate::object::Obj;
use crate::value::{error_kind, Value};
use crate::vm::Vm;

mod array;
mod hash;
mod io;
mod json;
mod map;
mod math;
mod os;
mod random;
mod string;
mod time;

/// Self-hosted modules, written in Lumen and bundled with the binary.
const SEQ_SRC: &str = include_str!("../../std/seq.lum");
const SET_SRC: &str = include_str!("../../std/set.lum");
const FUNCTIONAL_SRC: &str = include_str!("../../std/functional.lum");
const TESTING_SRC: &str = include_str!("../../std/testing.lum");
const PATH_SRC: &str = include_str!("../../std/path.lum");

/// Install the standard library into a VM.
pub fn install(vm: &mut Vm) {
    vm.set_native_loader(load);
}

/// Resolve a built-in module by name, or `None` to fall back to file loading.
fn load(vm: &mut Vm, name: &str) -> Option<Result<Value, Value>> {
    Some(match name {
        "math" => Ok(math::build(vm)),
        "string" => Ok(string::build(vm)),
        "array" => Ok(array::build(vm)),
        "map" => Ok(map::build(vm)),
        "io" => Ok(io::build(vm)),
        "os" => Ok(os::build(vm)),
        "time" => Ok(time::build(vm)),
        "json" => Ok(json::build(vm)),
        "random" => Ok(random::build(vm)),
        "hash" => Ok(hash::build(vm)),
        "seq" => vm.load_source_module("seq", SEQ_SRC),
        "set" => vm.load_source_module("set", SET_SRC),
        "functional" => vm.load_source_module("functional", FUNCTIONAL_SRC),
        "testing" => vm.load_source_module("testing", TESTING_SRC),
        "path" => vm.load_source_module("path", PATH_SRC),
        _ => return None,
    })
}

// ---- shared argument helpers ----------------------------------------------

/// Extract a number argument as `f64`.
fn num(vm: &mut Vm, v: Value) -> Result<f64, Value> {
    v.as_f64().ok_or_else(|| vm.make_error(error_kind::TYPE, "expected a number"))
}

/// Extract an integer argument.
fn int(vm: &mut Vm, v: Value) -> Result<i64, Value> {
    match v {
        Value::Int(n) => Ok(n),
        _ => Err(vm.make_error(error_kind::TYPE, "expected an integer")),
    }
}

/// Extract a string argument (cloned).
fn string_of(vm: &mut Vm, v: Value) -> Result<String, Value> {
    match v.as_obj().map(|r| vm.heap.get(r)) {
        Some(Obj::Str(s)) => Ok(s.clone()),
        _ => Err(vm.make_error(error_kind::TYPE, "expected a string")),
    }
}

/// Extract an array argument's elements (cloned).
fn array_of(vm: &mut Vm, v: Value) -> Result<Vec<Value>, Value> {
    match v.as_obj().map(|r| vm.heap.get(r)) {
        Some(Obj::Array(a)) => Ok(a.clone()),
        _ => Err(vm.make_error(error_kind::TYPE, "expected an array")),
    }
}

/// Construct a runtime error value (terse alias used throughout the stdlib).
fn err(vm: &mut Vm, kind: &'static str, msg: impl Into<String>) -> Value {
    vm.make_error(kind, msg)
}
