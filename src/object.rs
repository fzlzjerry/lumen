//! Heap object types.
//!
//! Everything that is not an immediate value lives in the heap as an [`Obj`].
//! The VM reaches them through [`GcRef`] handles. Functions are split into
//! immutable *code* ([`FnProto`], shared via `Rc`) and a runtime [`Closure`]
//! that pairs the code with captured upvalues — only the latter is GC-managed.

use crate::chunk::FnProto;
use crate::fxhash::FxHashMap;
use crate::value::{GcRef, MapKey, Value};
use std::fs::File;
use std::io::{BufReader, BufWriter};
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

/// A buffered file handle returned by `io.open` (DESIGN D32). Reads/writes go
/// through a buffer so they are incremental, not whole-file.
pub enum FileHandle {
    Reader(BufReader<File>),
    Writer(BufWriter<File>),
    Closed,
}

/// A built-in (Rust) method bound to a receiver — e.g. `handle.read_line`. Unlike
/// [`BoundMethod`] (a Lumen closure), the method here is a Rust handler selected
/// by [`NativeMethod`] (DESIGN D32).
pub struct BoundNative {
    pub receiver: GcRef,
    pub method: NativeMethod,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeMethod {
    FileReadLine,
    FileRead,
    FileWrite,
    FileClose,
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

/// One active call. Lives here (rather than in `vm`) so a suspended generator can
/// store its frames inside a heap object (DESIGN D29).
pub struct CallFrame {
    pub closure: GcRef,
    pub proto: Rc<FnProto>,
    pub ip: usize,
    /// Index in the value stack of this frame's slot 0.
    pub slot_base: usize,
    /// Which module's globals this frame resolves against.
    pub module: usize,
    /// How many fixed parameters the caller actually supplied (for `DefaultArg`).
    pub provided_argc: usize,
}

/// A registered `try` handler (no heap references, so the GC ignores it).
pub struct Handler {
    pub catch_ip: usize,
    pub stack_len: usize,
    pub frame: usize,
}

/// A swappable VM execution context — everything that is stack-relative. Swapped
/// in/out to run a generator (DESIGN D29).
pub struct ExecContext {
    pub stack: Vec<Value>,
    pub frames: Vec<CallFrame>,
    pub handlers: Vec<Handler>,
    pub open_upvalues: Vec<GcRef>,
}

impl ExecContext {
    pub fn new() -> Self {
        ExecContext {
            stack: Vec::new(),
            frames: Vec::new(),
            handlers: Vec::new(),
            open_upvalues: Vec::new(),
        }
    }
}

impl Default for ExecContext {
    fn default() -> Self {
        ExecContext::new()
    }
}

/// Lifecycle of a generator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GenState {
    /// Created but not yet started (its context holds `[closure, args…]`).
    Start,
    /// Parked at a `yield`, ready to resume.
    Suspended,
    /// Currently running (its context is live in the VM; guards re-entrancy).
    Running,
    /// Finished (returned or errored).
    Done,
}

/// A generator: a parked coroutine that yields values lazily (DESIGN D29).
pub struct Generator {
    pub closure: GcRef,
    pub state: GenState,
    /// The generator's own execution context (empty while it is running, because
    /// the context is then live in the VM).
    pub ctx: ExecContext,
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
        LumMap {
            entries: Vec::new(),
            index: FxHashMap::default(),
        }
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
            self.entries.push(Entry {
                key: key_val,
                value,
            });
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
    Generator(Generator),
    FileHandle(FileHandle),
    BoundNative(BoundNative),
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
            Obj::Generator(_) => "generator",
            Obj::FileHandle(_) => "file",
            Obj::BoundNative(_) => "function",
        }
    }
}
