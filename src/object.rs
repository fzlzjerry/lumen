//! Heap object types.
//!
//! Everything that is not an immediate value lives in the heap as an [`Obj`].
//! The VM reaches them through [`GcRef`] handles. Functions are split into
//! immutable *code* ([`FnProto`], shared via `Rc`) and a runtime [`Closure`]
//! that pairs the code with captured upvalues — only the latter is GC-managed.

use crate::chunk::FnProto;
use crate::value::{GcRef, MapKey, Value};
use crate::fxhash::FxHashMap;
use std::rc::Rc;

/// The native-function signature: given the VM and the already-evaluated
/// arguments, produce a value or a thrown value (`Err`).
pub type NativeFn = fn(&mut crate::vm::Vm, &[Value]) -> Result<Value, Value>;

/// How many arguments a native accepts.
#[derive(Clone, Copy, Debug)]
pub enum Arity {
    Exact(usize),
    AtLeast(usize),
    Range(usize, usize),
}

impl Arity {
    pub fn accepts(&self, n: usize) -> bool {
        match self {
            Arity::Exact(k) => n == *k,
            Arity::AtLeast(k) => n >= *k,
            Arity::Range(lo, hi) => n >= *lo && n <= *hi,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Arity::Exact(k) => format!("exactly {k}"),
            Arity::AtLeast(k) => format!("at least {k}"),
            Arity::Range(lo, hi) => format!("between {lo} and {hi}"),
        }
    }
}

/// A runtime closure: immutable code plus captured upvalues, tagged with the
/// module it was defined in so it resolves *its* module's globals (not the
/// currently-running module's) — see the VM's per-module globals.
pub struct Closure {
    pub proto: Rc<FnProto>,
    pub upvalues: Vec<GcRef>,
    pub module: usize,
}

/// A captured variable: open ones alias a live stack slot; once the slot leaves
/// scope the upvalue is closed and owns the value.
pub enum Upvalue {
    Open(usize),
    Closed(Value),
}

/// A class: its methods (including those copied down from a superclass) and a
/// link to the superclass for `super`.
pub struct Class {
    pub name: String,
    pub superclass: Option<GcRef>,
    /// method name -> `Closure` handle.
    pub methods: FxHashMap<String, GcRef>,
    /// static method name -> `Closure` handle (no receiver; DESIGN D27).
    pub statics: FxHashMap<String, GcRef>,
}

/// An instance: a class plus a bag of dynamically-added fields.
pub struct Instance {
    pub class: GcRef,
    pub fields: FxHashMap<String, Value>,
}

/// A method bound to its receiver, produced when a method is read off an
/// instance.
pub struct BoundMethod {
    pub receiver: Value,
    pub method: GcRef, // a Closure
}

/// A built-in function implemented in Rust.
pub struct Native {
    pub name: String,
    pub arity: Arity,
    pub func: NativeFn,
}

/// An imported module: the values it `export`ed.
pub struct Module {
    pub name: String,
    pub path: String,
    pub exports: FxHashMap<String, Value>,
}

/// A built-in error object with a kind tag and message (SPEC §7).
pub struct LumError {
    pub kind: String,
    pub message: String,
}

/// One map entry: the original key value (for ordered iteration and `keys()`)
/// and the value. The normalized hashable key lives in the side `index`.
struct Entry {
    key: Value,
    value: Value,
}

/// An insertion-ordered map. Order matters so `for k in m` and `keys(m)` are
/// deterministic (SPEC §6.7). A side `index` gives O(1) lookup by normalized
/// key while `entries` preserves order. Each entry remembers its own normalized
/// key so removal can rebuild the index without the heap.
pub struct LumMap {
    entries: Vec<Entry>,
    index: FxHashMap<MapKey, usize>,
}

impl Default for LumMap {
    fn default() -> Self {
        LumMap::new()
    }
}

impl LumMap {
    pub fn new() -> Self {
        LumMap { entries: Vec::new(), index: FxHashMap::default() }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert or update. `key_val` is the original key value (kept for ordered
    /// iteration); `key` is its normalized hashable form.
    pub fn insert(&mut self, key: MapKey, key_val: Value, value: Value) {
        if let Some(&pos) = self.index.get(&key) {
            self.entries[pos].value = value;
        } else {
            self.index.insert(key, self.entries.len());
            self.entries.push(Entry { key: key_val, value });
        }
    }

    pub fn get(&self, key: MapKey) -> Option<Value> {
        self.index.get(&key).map(|&pos| self.entries[pos].value)
    }

    pub fn has(&self, key: MapKey) -> bool {
        self.index.contains_key(&key)
    }

    /// Remove a key, preserving insertion order. Returns whether it was present.
    pub fn remove(&mut self, key: MapKey) -> bool {
        if let Some(pos) = self.index.remove(&key) {
            self.entries.remove(pos);
            // Re-point every index entry after `pos` (which shifted down by 1).
            for v in self.index.values_mut() {
                if *v > pos {
                    *v -= 1;
                }
            }
            true
        } else {
            false
        }
    }

    pub fn keys(&self) -> Vec<Value> {
        self.entries.iter().map(|e| e.key).collect()
    }

    pub fn values(&self) -> Vec<Value> {
        self.entries.iter().map(|e| e.value).collect()
    }

    pub fn nth_key(&self, i: usize) -> Option<Value> {
        self.entries.get(i).map(|e| e.key)
    }

    /// Iterate `(key, value)` pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (Value, Value)> + '_ {
        self.entries.iter().map(|e| (e.key, e.value))
    }
}

/// A heap object. The VM dereferences a [`GcRef`] to one of these.
pub enum Obj {
    Str(String),
    Array(Vec<Value>),
    Map(LumMap),
    Closure(Closure),
    Upvalue(Upvalue),
    Class(Class),
    Instance(Instance),
    Bound(BoundMethod),
    Native(Native),
    Module(Module),
    Error(LumError),
}

impl Obj {
    /// The SPEC type name used by `type(x)`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Obj::Str(_) => "string",
            Obj::Array(_) => "array",
            Obj::Map(_) => "map",
            Obj::Closure(_) | Obj::Native(_) => "function",
            Obj::Class(_) => "class",
            Obj::Instance(_) => "instance",
            Obj::Bound(_) => "function",
            Obj::Module(_) => "module",
            Obj::Error(_) => "error",
            Obj::Upvalue(_) => "upvalue",
        }
    }
}
