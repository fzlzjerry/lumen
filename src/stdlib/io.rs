//! The `io` module: file and stream I/O.

use super::{array_of, err, string_of, Vm};
use crate::object::Arity::{self, Exact};
use crate::object::{FileHandle, LumMap, Obj};
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "open", Exact(2), open),
        f(vm, "read_file", Exact(1), read_file),
        f(vm, "write_file", Exact(2), write_file),
        f(vm, "append_file", Exact(2), append_file),
        f(vm, "exists", Exact(1), exists),
        f(vm, "lines", Exact(1), lines),
        f(vm, "mkdir", Exact(1), mkdir),
        f(vm, "listdir", Exact(1), listdir),
        f(vm, "remove", Exact(1), remove),
        f(vm, "rmdir", Exact(1), rmdir),
        f(vm, "is_dir", Exact(1), is_dir),
        f(vm, "is_file", Exact(1), is_file),
        f(vm, "read_bytes", Exact(1), read_bytes),
        f(vm, "write_bytes", Exact(2), write_bytes),
        f(vm, "stat", Exact(1), stat),
        f(vm, "walk", Exact(1), walk),
        f(vm, "copy", Exact(2), copy),
        f(vm, "rename", Exact(2), rename),
        f(vm, "eprint", Exact(1), eprint),
        f(vm, "eprintln", Exact(1), eprintln),
    ];
    vm.make_module("io", exports)
}

/// `open(path, mode)` returns a buffered file handle (DESIGN D32). Modes: `"r"`
/// (read), `"w"` (truncate-write), `"a"` (append).
fn open(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    use std::fs::OpenOptions;
    use std::io::{BufReader, BufWriter};
    let path = string_of(vm, a[0])?;
    let mode = string_of(vm, a[1])?;
    let opened = match mode.as_str() {
        "r" => std::fs::File::open(&path).map(|f| FileHandle::Reader(BufReader::new(f))),
        "w" => std::fs::File::create(&path).map(|f| FileHandle::Writer(BufWriter::new(f))),
        "a" => OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map(|f| FileHandle::Writer(BufWriter::new(f))),
        _ => {
            return Err(err(
                vm,
                error_kind::VALUE,
                format!("invalid file mode '{mode}' (use \"r\", \"w\", or \"a\")"),
            ))
        }
    };
    match opened {
        Ok(handle) => Ok(Value::Obj(vm.heap.alloc(Obj::FileHandle(handle)))),
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot open '{path}': {e}"),
        )),
    }
}

fn read_file(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(vm.new_string(&s)),
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot read '{path}': {e}"),
        )),
    }
}

fn write_file(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    let content = vm.to_display(a[1], false)?;
    match std::fs::write(&path, content) {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot write '{path}': {e}"),
        )),
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
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot append to '{path}': {e}"),
        )),
    }
}

fn exists(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    Ok(Value::Bool(std::path::Path::new(&path).exists()))
}

/// Create a directory and any missing parents (like `mkdir -p`).
fn mkdir(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    match std::fs::create_dir_all(&path) {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot create '{path}': {e}"),
        )),
    }
}

/// The entry names directly under a directory, sorted for determinism.
fn listdir(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    let entries = match std::fs::read_dir(&path) {
        Ok(rd) => rd,
        Err(e) => {
            return Err(err(
                vm,
                error_kind::VALUE,
                format!("cannot read dir '{path}': {e}"),
            ))
        }
    };
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => names.push(e.file_name().to_string_lossy().into_owned()),
            Err(e) => {
                return Err(err(
                    vm,
                    error_kind::VALUE,
                    format!("cannot read dir '{path}': {e}"),
                ))
            }
        }
    }
    names.sort();
    let vals: Vec<Value> = names.iter().map(|n| vm.new_string(n)).collect();
    Ok(vm.new_array(vals))
}

/// Remove a file (not a directory — use `rmdir`).
fn remove(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot remove '{path}': {e}"),
        )),
    }
}

/// Remove an *empty* directory (non-recursive, so it can never delete a tree).
fn rmdir(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    match std::fs::remove_dir(&path) {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot remove dir '{path}': {e}"),
        )),
    }
}

fn is_dir(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    Ok(Value::Bool(std::path::Path::new(&path).is_dir()))
}
fn is_file(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    Ok(Value::Bool(std::path::Path::new(&path).is_file()))
}

fn lines(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            let vals: Vec<Value> = s.lines().map(|l| vm.new_string(l)).collect();
            Ok(vm.new_array(vals))
        }
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot read '{path}': {e}"),
        )),
    }
}

/// Read a file's raw bytes as an array of ints in `0..=255`.
fn read_bytes(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            let vals: Vec<Value> = bytes.iter().map(|&b| Value::Int(b as i64)).collect();
            Ok(vm.new_array(vals))
        }
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot read '{path}': {e}"),
        )),
    }
}

/// Write an array of ints (each in `0..=255`) to a file as raw bytes. Throws
/// `ValueError` if any element is not an integer in range.
fn write_bytes(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    let items = array_of(vm, a[1])?;
    let mut bytes = Vec::with_capacity(items.len());
    for it in items {
        match it {
            Value::Int(n) if (0..=255).contains(&n) => bytes.push(n as u8),
            _ => {
                return Err(err(
                    vm,
                    error_kind::VALUE,
                    "write_bytes: every element must be an integer in 0..=255",
                ))
            }
        }
    }
    match std::fs::write(&path, bytes) {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot write '{path}': {e}"),
        )),
    }
}

/// File metadata: `{size, is_dir, is_file, modified}` where `modified` is Unix
/// epoch seconds (an int) or `nil` if the platform does not report it.
fn stat(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) => {
            return Err(err(
                vm,
                error_kind::VALUE,
                format!("cannot stat '{path}': {e}"),
            ))
        }
    };
    let modified = match meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
    {
        Some(d) => Value::Int(d.as_secs() as i64),
        None => Value::Nil,
    };
    let entries = [
        ("size", Value::Int(meta.len() as i64)),
        ("is_dir", Value::Bool(meta.is_dir())),
        ("is_file", Value::Bool(meta.is_file())),
        ("modified", modified),
    ];
    make_map(vm, &entries)
}

/// Recursively list every path under `dir` (files and subdirectories), depth-first.
/// Each directory's entries are sorted by name for deterministic output.
fn walk(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let path = string_of(vm, a[0])?;
    let mut out: Vec<String> = Vec::new();
    if let Err(e) = walk_into(std::path::Path::new(&path), &mut out) {
        return Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot walk '{path}': {e}"),
        ));
    }
    let vals: Vec<Value> = out.iter().map(|p| vm.new_string(p)).collect();
    Ok(vm.new_array(vals))
}

/// Pre-order DFS helper for `walk`: a subdirectory's own path is emitted before
/// its contents.
fn walk_into(dir: &std::path::Path, out: &mut Vec<String>) -> std::io::Result<()> {
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(dir)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    entries.sort();
    for entry in entries {
        out.push(entry.to_string_lossy().into_owned());
        if entry.is_dir() {
            walk_into(&entry, out)?;
        }
    }
    Ok(())
}

/// Copy a file from `src` to `dst` (overwriting `dst`).
fn copy(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let src = string_of(vm, a[0])?;
    let dst = string_of(vm, a[1])?;
    match std::fs::copy(&src, &dst) {
        Ok(_) => Ok(Value::Nil),
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot copy '{src}' to '{dst}': {e}"),
        )),
    }
}

/// Rename/move a file or directory from `src` to `dst`.
fn rename(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let src = string_of(vm, a[0])?;
    let dst = string_of(vm, a[1])?;
    match std::fs::rename(&src, &dst) {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Err(err(
            vm,
            error_kind::VALUE,
            format!("cannot rename '{src}' to '{dst}': {e}"),
        )),
    }
}

/// Build a map from `(key, value)` pairs, GC-safely (mirrors `datetime::make_map`):
/// the result is pinned as a temp root and each freshly-interned key is inserted
/// immediately, so no allocating call runs between creating a key and storing it.
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
