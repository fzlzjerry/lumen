//! The virtual machine: a stack-based bytecode interpreter.
//!
//! [`Vm`] owns the [`Heap`], the value stack, the call-frame stack, the global
//! and built-in tables, the exception-handler stack, and the open-upvalue list.
//! [`Vm::interpret`] runs a compiled [`FnProto`] to completion. Runtime faults
//! and `throw`n values unwind through registered handlers (`try`/`catch`); an
//! uncaught throw produces a full stack trace.
//!
//! The heart is [`Vm::run_until`], a dispatch loop that executes the top call
//! frame until the frame stack shrinks to a floor — that floor mechanism is what
//! lets native functions (and string interpolation) call *back* into Lumen code
//! re-entrantly (`call_value` + `run_until`) for `sort` comparators, custom
//! `str()` methods, and the like.

use crate::chunk::{Constant, FnProto};
use crate::gc::Heap;
use crate::object::{
    Arity, BoundMethod, CallFrame, Class, Closure, ExecContext, GenState, Generator, Handler,
    Instance, LumError, LumMap, Module, Native, Obj, Upvalue,
};
use crate::opcode::OpCode;
use crate::util::{escape_string, format_float};
use crate::value::{error_kind, GcRef, MapKey, Value};
use crate::fxhash::FxHashMap;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

mod builtins;

/// How a module is loaded if it is not a `.lum` file (set by the stdlib, Phase 7).
pub type NativeModuleLoader = fn(&mut Vm, &str) -> Option<Result<Value, Value>>;

/// Result of a single debugger step.
pub enum DebugStatus {
    /// More instructions remain.
    Running,
    /// The program ran to completion.
    Finished,
    /// An uncaught error aborted the program (formatted message + trace).
    Error(String),
}

const MAX_FRAMES: usize = 1024;

pub struct Vm {
    pub heap: Heap,
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    /// Per-module global tables. Index 0 is the main script; each imported
    /// module gets its own. A closure resolves globals against *its* module so
    /// a function keeps seeing its own module's top-level bindings even when
    /// called from another module.
    module_globals: Vec<FxHashMap<String, Value>>,
    /// Built-in functions and modules, always in scope as a fallback.
    builtins: FxHashMap<String, Value>,
    handlers: Vec<Handler>,
    open_upvalues: Vec<GcRef>,
    /// Resolved-path -> module object (import cache).
    modules: FxHashMap<String, GcRef>,
    base_dir: PathBuf,
    out: Box<dyn Write>,
    native_loader: Option<NativeModuleLoader>,
    /// Holds a thrown value while it propagates out of [`Vm::run_until`].
    pending_throw: Option<Value>,
    /// Extra GC roots held by native functions across re-entrant calls (DESIGN
    /// D18). A native pushes a value here to keep it alive while it calls back
    /// into Lumen code (where a collection may occur).
    temp_roots: Vec<Value>,
    /// State for the `random` module's PRNG.
    rng_state: u64,
    /// Command-line arguments passed to the script (exposed via `os.args()`).
    script_args: Vec<String>,
    /// Extra directories to search when resolving an `import` (dependencies).
    search_paths: Vec<PathBuf>,
    /// Remaining execution budget: decremented at each loop back-edge and call;
    /// at zero the VM throws. `u64::MAX` means unlimited (the default). Used by
    /// the VM fuzzer to bound otherwise-unbounded programs.
    budget: u64,
    /// Set by `Yield` to suspend a running generator and carry the yielded value
    /// out of the dispatch loop (DESIGN D29).
    pending_yield: Option<Value>,
    /// Execution contexts swapped out while a generator runs (the callers'). The
    /// GC roots these exactly like the live stack so nothing is freed mid-resume.
    saved_contexts: Vec<ExecContext>,
}

impl Default for Vm {
    fn default() -> Self {
        Vm::new()
    }
}

impl Vm {
    pub fn new() -> Self {
        Vm::with_output(Box::new(std::io::stdout()))
    }

    pub fn with_output(out: Box<dyn Write>) -> Self {
        let mut vm = Vm {
            heap: Heap::new(),
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(64),
            module_globals: vec![FxHashMap::default()], // module 0 = the main script
            builtins: FxHashMap::default(),
            handlers: Vec::new(),
            open_upvalues: Vec::new(),
            modules: FxHashMap::default(),
            base_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            out,
            native_loader: None,
            pending_throw: None,
            temp_roots: Vec::new(),
            // Seed from the wall clock; deterministic only after `seed()`.
            rng_state: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9E3779B97F4A7C15)
                | 1,
            script_args: Vec::new(),
            search_paths: Vec::new(),
            budget: u64::MAX,
            pending_yield: None,
            saved_contexts: Vec::new(),
        };
        builtins::register(&mut vm);
        vm
    }

    /// Bound total execution to `n` loop back-edges + calls (the VM throws when
    /// exhausted). Used by the fuzzer to terminate unbounded programs.
    pub fn set_step_limit(&mut self, n: u64) {
        self.budget = n;
    }

    /// Charge one unit of the execution budget at a back-edge or call. Returns an
    /// error to throw if the budget is exhausted.
    fn charge_budget(&mut self) -> Result<(), Value> {
        if self.budget != u64::MAX {
            if self.budget == 0 {
                return Err(self.throw(error_kind::VALUE, "execution budget exceeded"));
            }
            self.budget -= 1;
        }
        Ok(())
    }

    /// Set the command-line arguments visible to the program via `os.args()`.
    pub fn set_args(&mut self, args: Vec<String>) {
        self.script_args = args;
    }

    /// The script's command-line arguments.
    pub fn args(&self) -> &[String] {
        &self.script_args
    }

    /// Install a loader for non-file (built-in) modules (used by the stdlib).
    pub fn set_native_loader(&mut self, loader: NativeModuleLoader) {
        self.native_loader = Some(loader);
    }

    /// Turn on GC stress mode: a full (major) collection before every
    /// instruction, to surface any missing root.
    pub fn set_stress_gc(&mut self, on: bool) {
        self.heap.stress = on;
    }

    /// Turn on minor-GC stress mode: a minor collection before every
    /// instruction, to surface any missing write barrier (an old→young edge that
    /// wasn't remembered would let the young object be freed).
    pub fn set_minor_stress_gc(&mut self, on: bool) {
        self.heap.minor_stress = on;
    }

    /// Collect if heap pressure warrants it. Called at safe points (back-edges
    /// and calls). Picks a minor or major collection based on the thresholds.
    fn maybe_collect(&mut self) {
        if self.heap.stress || self.heap.minor_stress {
            return; // stress modes drive collection from the dispatch loop
        }
        if self.heap.should_collect() {
            self.collect();
        }
    }

    /// Run a collection, choosing minor (nursery) or major (full) by pressure.
    pub fn collect(&mut self) {
        if self.heap.major_due() {
            self.collect_major();
        } else {
            self.collect_minor();
        }
    }

    /// A full mark-and-sweep over both generations.
    pub fn collect_major(&mut self) {
        self.mark_roots(false);
        self.heap.trace_references(false);
        self.heap.sweep_major();
    }

    /// A minor collection: trace and sweep only the young nursery, seeded by the
    /// VM roots and the remembered set; survivors are promoted to old.
    pub fn collect_minor(&mut self) {
        self.mark_roots(true);
        self.heap.mark_remembered();
        self.heap.trace_references(true);
        self.heap.sweep_minor();
    }

    /// Record an old→young pointer write so a minor collection keeps the young
    /// object alive (the write barrier — DESIGN of the generational GC).
    pub fn write_barrier(&mut self, container: GcRef, value: Value) {
        self.heap.write_barrier(container, value);
    }

    /// Mark every GC root: the value stack, the call frames' closures, the
    /// global and built-in tables, the open upvalues, the module cache, the
    /// temporary roots, and any in-flight thrown value. `young_only` restricts
    /// marking to the nursery for a minor collection.
    fn mark_roots(&mut self, young_only: bool) {
        for i in 0..self.stack.len() {
            let v = self.stack[i];
            self.heap.mark_value(v, young_only);
        }
        for i in 0..self.frames.len() {
            let c = self.frames[i].closure;
            self.heap.mark_ref(c, young_only);
        }
        for mi in 0..self.module_globals.len() {
            let vals: Vec<Value> = self.module_globals[mi].values().copied().collect();
            for v in vals {
                self.heap.mark_value(v, young_only);
            }
        }
        for v in self.builtins.values() {
            self.heap.mark_value(*v, young_only);
        }
        for i in 0..self.open_upvalues.len() {
            let uv = self.open_upvalues[i];
            self.heap.mark_ref(uv, young_only);
        }
        for r in self.modules.values() {
            self.heap.mark_ref(*r, young_only);
        }
        if let Some(v) = self.pending_throw {
            self.heap.mark_value(v, young_only);
        }
        for i in 0..self.temp_roots.len() {
            let v = self.temp_roots[i];
            self.heap.mark_value(v, young_only);
        }
        if let Some(v) = self.pending_yield {
            self.heap.mark_value(v, young_only);
        }
        // Contexts of callers swapped out while a generator runs (DESIGN D29).
        for ci in 0..self.saved_contexts.len() {
            for vi in 0..self.saved_contexts[ci].stack.len() {
                let v = self.saved_contexts[ci].stack[vi];
                self.heap.mark_value(v, young_only);
            }
            for fi in 0..self.saved_contexts[ci].frames.len() {
                let c = self.saved_contexts[ci].frames[fi].closure;
                self.heap.mark_ref(c, young_only);
            }
            for ui in 0..self.saved_contexts[ci].open_upvalues.len() {
                let uv = self.saved_contexts[ci].open_upvalues[ui];
                self.heap.mark_ref(uv, young_only);
            }
        }
    }

    /// Set the directory used to resolve `import` paths.
    pub fn set_base_dir(&mut self, dir: PathBuf) {
        self.base_dir = dir;
    }

    /// Add a directory to the module search path (for project dependencies).
    /// `import "x"` is tried against the base dir first, then these in order.
    pub fn add_search_path(&mut self, dir: PathBuf) {
        self.search_paths.push(dir);
    }

    /// Register a native global function.
    pub fn define_native(&mut self, name: &str, arity: Arity, func: crate::object::NativeFn) {
        let r = self.heap.alloc(Obj::Native(Native { name: name.to_string(), arity, func }));
        self.builtins.insert(name.to_string(), Value::Obj(r));
    }

    /// Register a value as a built-in (used for native modules).
    pub fn define_builtin(&mut self, name: &str, value: Value) {
        self.builtins.insert(name.to_string(), value);
    }

    /// Allocate a native-function value (for building stdlib modules).
    pub fn make_native_value(&mut self, name: &str, arity: Arity, func: crate::object::NativeFn) -> Value {
        Value::Obj(self.heap.alloc(Obj::Native(Native { name: name.to_string(), arity, func })))
    }

    /// Build a module value from a list of `(export name, value)` pairs.
    pub fn make_module(&mut self, name: &str, exports: Vec<(&str, Value)>) -> Value {
        let map: FxHashMap<String, Value> = exports.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        Value::Obj(self.heap.alloc(Obj::Module(Module {
            name: name.to_string(),
            path: format!("native:{name}"),
            exports: map,
        })))
    }

    /// Compile and run Lumen `src` as a module named `name`, returning its module
    /// value. Used to bootstrap self-hosted stdlib modules from embedded source.
    pub fn load_source_module(&mut self, name: &str, src: &str) -> Result<Value, Value> {
        let (program, errs) = crate::check_source(src);
        if !errs.is_empty() {
            let first = errs[0].message.clone();
            return Err(self.throw(error_kind::VALUE, format!("module '{name}' has errors: {first}")));
        }
        let proto = match crate::compiler::compile(&program) {
            Ok(p) => p,
            Err(_) => return Err(self.throw(error_kind::VALUE, format!("module '{name}' failed to compile"))),
        };
        let module_obj = self.heap.alloc(Obj::Module(Module {
            name: name.to_string(),
            path: format!("self:{name}"),
            exports: FxHashMap::default(),
        }));
        self.modules.insert(format!("self:{name}"), module_obj);
        let module_idx = self.module_globals.len();
        self.module_globals.push(FxHashMap::default());
        let run_result = self.run_module_body(proto.clone(), module_idx);
        let exports = collect_exports(&self.module_globals[module_idx], &proto.exports, &run_result);
        run_result?;
        if let Obj::Module(m) = self.heap.get_mut(module_obj) {
            m.exports = exports;
        }
        Ok(Value::Obj(module_obj))
    }

    /// Next value from the VM's xorshift64* PRNG (for the `random` module).
    pub fn next_random_u64(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng_state = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    /// Seed the PRNG.
    pub fn set_random_seed(&mut self, seed: u64) {
        // Avoid the all-zero state, which xorshift cannot escape.
        self.rng_state = seed | 1;
    }

    /// Pin `v` as a temporary GC root (for native code holding a heap value
    /// across a re-entrant `call_and_run`). Pair with [`pop_temp_root`].
    pub fn push_temp_root(&mut self, v: Value) {
        self.temp_roots.push(v);
    }

    /// Replace the most recent temporary root (e.g. a `reduce` accumulator).
    pub fn update_top_temp_root(&mut self, v: Value) {
        if let Some(last) = self.temp_roots.last_mut() {
            *last = v;
        }
    }

    pub fn pop_temp_root(&mut self) {
        self.temp_roots.pop();
    }

    /// Run a top-level script prototype to completion. On an uncaught throw,
    /// returns `Err(message)` with a formatted value + stack trace.
    pub fn interpret(&mut self, proto: Rc<FnProto>) -> Result<(), String> {
        let closure = self.heap.alloc_closure(Closure { proto, upvalues: Vec::new(), module: 0 });
        self.stack.push(Value::Obj(closure));
        let callee_idx = self.stack.len() - 1;
        if let Err(thrown) = self.call_value(Value::Obj(closure), 0, callee_idx) {
            return Err(self.format_uncaught(thrown));
        }
        match self.run_until(0) {
            Ok(()) => Ok(()),
            Err(thrown) => Err(self.format_uncaught(thrown)),
        }
    }

    /// Run a script prototype and return its return value. Unlike [`interpret`],
    /// this is meant for the REPL: on an uncaught throw it returns `Err(message)`
    /// *and* resets the transient state (stack/frames/handlers) so the VM stays
    /// usable for the next input, while globals persist.
    pub fn eval(&mut self, proto: Rc<FnProto>) -> Result<Value, String> {
        let closure = self.heap.alloc_closure(Closure { proto, upvalues: Vec::new(), module: 0 });
        self.stack.push(Value::Obj(closure));
        let callee_idx = self.stack.len() - 1;
        let result = self
            .call_value(Value::Obj(closure), 0, callee_idx)
            .and_then(|()| self.run_until(0));
        match result {
            Ok(()) => Ok(self.stack.pop().unwrap_or(Value::Nil)),
            Err(thrown) => {
                let msg = self.format_uncaught(thrown);
                self.reset_transient();
                Err(msg)
            }
        }
    }

    /// Clear all transient execution state (keeping globals and the heap).
    fn reset_transient(&mut self) {
        self.stack.clear();
        self.frames.clear();
        self.handlers.clear();
        self.open_upvalues.clear();
        self.temp_roots.clear();
        self.pending_throw = None;
    }

    /// Names currently defined as globals in the main module (for the REPL's
    /// resolver seeding).
    pub fn global_names(&self) -> Vec<String> {
        self.module_globals[0].keys().cloned().collect()
    }

    // ---- debugger support --------------------------------------------------

    /// Set up the top-level frame for a script without running it (the debugger
    /// then drives execution one instruction at a time).
    pub fn debug_start(&mut self, proto: Rc<FnProto>) -> Result<(), String> {
        let closure = self.heap.alloc_closure(Closure { proto, upvalues: Vec::new(), module: 0 });
        self.stack.push(Value::Obj(closure));
        let callee_idx = self.stack.len() - 1;
        self.call_value(Value::Obj(closure), 0, callee_idx)
            .map_err(|t| self.format_uncaught(t))
    }

    /// Execute one instruction (collecting first if due), handling throws.
    pub fn debug_step(&mut self) -> DebugStatus {
        if self.frames.is_empty() {
            return DebugStatus::Finished;
        }
        if self.heap.should_collect() {
            self.collect();
        }
        match self.step() {
            Ok(()) => {
                if self.frames.is_empty() {
                    DebugStatus::Finished
                } else {
                    DebugStatus::Running
                }
            }
            Err(thrown) => {
                if self.unwind(thrown, 0) {
                    DebugStatus::Running
                } else {
                    let t = self.pending_throw.take().unwrap();
                    DebugStatus::Error(self.format_uncaught(t))
                }
            }
        }
    }

    /// `(function name, current line, instruction pointer)` of the next
    /// instruction to execute, or `None` if execution has finished.
    pub fn debug_location(&self) -> Option<(String, u32, usize)> {
        self.frames.last().map(|f| (f.proto.display_name(), f.proto.chunk.line_at(f.ip), f.ip))
    }

    /// The call stack as `(function name, line)`, innermost first.
    pub fn debug_backtrace(&self) -> Vec<(String, u32)> {
        let n = self.frames.len();
        self.frames
            .iter()
            .enumerate()
            .rev()
            .map(|(i, f)| {
                let line = if i == n - 1 {
                    f.proto.chunk.line_at(f.ip)
                } else {
                    f.proto.chunk.line_at(f.ip.saturating_sub(1))
                };
                (f.proto.display_name(), line)
            })
            .collect()
    }

    /// Named locals of the current frame (skipping reserved/internal slots).
    pub fn debug_locals(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(f) = self.frames.last() {
            let base = f.slot_base;
            let count = self.stack.len() - base;
            for slot in 0..count {
                let name = f.proto.local_names.get(slot).cloned().unwrap_or_default();
                if name.is_empty() || name.starts_with('@') {
                    continue;
                }
                out.push((name, self.debug_display(self.stack[base + slot])));
            }
        }
        out
    }

    /// The current frame's prototype (for disassembly) and instruction pointer.
    pub fn debug_current_frame(&self) -> Option<(Rc<FnProto>, usize)> {
        self.frames.last().map(|f| (f.proto.clone(), f.ip))
    }

    /// The current call depth (number of active frames). Used by the debugger's
    /// step-over to tell "still inside the called function" from "returned".
    pub fn debug_depth(&self) -> usize {
        self.frames.len()
    }

    /// Three-way ordering for the debugger's conditional breakpoints/watches.
    /// `None` if the operands aren't order-comparable (mismatched types or NaN).
    pub fn debug_compare(&self, a: Value, b: Value) -> Option<std::cmp::Ordering> {
        if let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) {
            x.partial_cmp(&y)
        } else if let (Value::Obj(ra), Value::Obj(rb)) = (a, b) {
            match (self.heap.get(ra), self.heap.get(rb)) {
                (Obj::Str(sa), Obj::Str(sb)) => Some(sa.cmp(sb)),
                _ => None,
            }
        } else {
            None
        }
    }

    /// The contents of `v` if it's a string, for comparing against string
    /// literals in debugger conditions.
    pub fn debug_as_str(&self, v: Value) -> Option<String> {
        match v {
            Value::Obj(r) => match self.heap.get(r) {
                Obj::Str(s) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// Look up a name in the current frame's locals, then its module globals,
    /// then builtins (for the debugger's `print` command).
    pub fn debug_lookup(&self, name: &str) -> Option<Value> {
        if let Some(f) = self.frames.last() {
            for (slot, n) in f.proto.local_names.iter().enumerate() {
                if n == name {
                    let idx = f.slot_base + slot;
                    if idx < self.stack.len() {
                        return Some(self.stack[idx]);
                    }
                }
            }
            if let Some(v) = self.module_globals[f.module].get(name) {
                return Some(*v);
            }
        }
        self.builtins.get(name).copied()
    }

    /// A read-only value rendering for the debugger (never calls user `str()`).
    pub fn debug_display(&self, v: Value) -> String {
        match v {
            Value::Nil => "nil".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => format_float(f),
            Value::Obj(r) => match self.heap.get(r) {
                Obj::Str(s) => format!("\"{}\"", escape_string(s)),
                Obj::Array(a) => {
                    let parts: Vec<String> = a.iter().map(|&x| self.debug_display(x)).collect();
                    format!("[{}]", parts.join(", "))
                }
                Obj::Map(m) => {
                    let parts: Vec<String> = m
                        .iter()
                        .map(|(k, val)| format!("{}: {}", self.debug_display(k), self.debug_display(val)))
                        .collect();
                    format!("{{{}}}", parts.join(", "))
                }
                Obj::Closure(c) => format!("<fn {}>", c.proto.name.as_deref().unwrap_or("anonymous")),
                Obj::Native(n) => format!("<fn {}>", n.name),
                Obj::Class(c) => format!("<class {}>", c.name),
                Obj::Bound(_) => "<bound method>".to_string(),
                Obj::Module(m) => format!("<module {}>", m.name),
                Obj::Error(e) => format!("{}: {}", e.kind, e.message),
                Obj::Upvalue(_) => "<upvalue>".to_string(),
                Obj::Instance(inst) => {
                    let cname = if let Obj::Class(c) = self.heap.get(inst.class) {
                        c.name.clone()
                    } else {
                        "?".into()
                    };
                    format!("<{cname} instance>")
                }
                Obj::Generator(_) => "<generator>".to_string(),
            },
        }
    }

    // ---- stack helpers -----------------------------------------------------

    fn push(&mut self, v: Value) {
        self.stack.push(v);
    }
    fn pop(&mut self) -> Value {
        self.stack.pop().expect("stack underflow — compiler bug")
    }
    fn peek(&self, depth: usize) -> Value {
        self.stack[self.stack.len() - 1 - depth]
    }

    fn read_byte(&mut self) -> u8 {
        let f = self.frames.last_mut().unwrap();
        let b = f.proto.chunk.code[f.ip];
        f.ip += 1;
        b
    }
    fn read_u16(&mut self) -> u16 {
        let hi = self.read_byte() as u16;
        let lo = self.read_byte() as u16;
        (hi << 8) | lo
    }
    fn read_constant(&mut self) -> Constant {
        let idx = self.read_u16() as usize;
        self.frames.last().unwrap().proto.chunk.constants[idx].clone()
    }
    /// Read a `u16` string-constant operand as an owned `String`.
    fn read_string(&mut self) -> String {
        match self.read_constant() {
            Constant::Str(s) => s,
            _ => unreachable!("operand was not a string constant — compiler bug"),
        }
    }

    // ---- error construction ------------------------------------------------

    /// Allocate a built-in error object and return it as a throwable value.
    pub fn make_error(&mut self, kind: &str, message: impl Into<String>) -> Value {
        let r = self.heap.alloc(Obj::Error(LumError {
            kind: kind.to_string(),
            message: message.into(),
        }));
        Value::Obj(r)
    }

    fn throw(&mut self, kind: &'static str, message: impl Into<String>) -> Value {
        self.make_error(kind, message)
    }

    // ---- the dispatch loop -------------------------------------------------

    /// Execute instructions until the frame stack shrinks to `floor`. Catches
    /// thrown values at handlers whose frame index is `>= floor`; otherwise the
    /// throw propagates out as `Err`.
    fn run_until(&mut self, floor: usize) -> Result<(), Value> {
        loop {
            if self.frames.len() <= floor {
                return Ok(());
            }
            // Stress modes collect before *every* instruction (full major to
            // surface missing roots; minor to surface missing write barriers);
            // the normal pressure trigger lives at back-edges and calls.
            if self.heap.stress {
                self.collect_major();
            } else if self.heap.minor_stress {
                self.collect_minor();
            }
            match self.step() {
                Ok(()) => {}
                Err(thrown) => {
                    if !self.unwind(thrown, floor) {
                        // Recover the thrown value for propagation.
                        return Err(self.pending_throw.take().unwrap());
                    }
                }
            }
            // A `Yield` suspends the running generator: unwind the dispatch loop
            // so `resume_generator` can hand the value back (DESIGN D29).
            if self.pending_yield.is_some() {
                return Ok(());
            }
        }
    }

    /// Find a handler at or above `floor` for `thrown`. If found, transfer
    /// control to its catch block and return `true`; otherwise stash the value
    /// in `pending_throw` and return `false`.
    fn unwind(&mut self, thrown: Value, floor: usize) -> bool {
        match self.handlers.last() {
            Some(h) if h.frame >= floor => {
                let h = self.handlers.pop().unwrap();
                self.frames.truncate(h.frame + 1);
                self.stack.truncate(h.stack_len);
                self.stack.push(thrown);
                self.frames[h.frame].ip = h.catch_ip;
                true
            }
            _ => {
                self.pending_throw = Some(thrown);
                false
            }
        }
    }

    /// Execute a single instruction.
    fn step(&mut self) -> Result<(), Value> {
        let op = match OpCode::from_u8(self.read_byte()) {
            Some(op) => op,
            None => return Err(self.throw(error_kind::VALUE, "corrupt bytecode: bad opcode")),
        };
        match op {
            OpCode::Nil => self.push(Value::Nil),
            OpCode::True => self.push(Value::Bool(true)),
            OpCode::False => self.push(Value::Bool(false)),
            OpCode::Const => {
                let c = self.read_constant();
                let v = self.materialize(c);
                self.push(v);
            }
            OpCode::Pop => {
                self.pop();
            }
            OpCode::PopN => {
                let n = self.read_byte() as usize;
                let len = self.stack.len();
                self.stack.truncate(len - n);
            }
            OpCode::Dup => {
                let v = self.peek(0);
                self.push(v);
            }
            OpCode::Dup2 => {
                let a = self.peek(1);
                let b = self.peek(0);
                self.push(a);
                self.push(b);
            }
            OpCode::DefineGlobal => {
                let name = self.read_string();
                let v = self.pop();
                let m = self.frames.last().unwrap().module;
                self.module_globals[m].insert(name, v);
            }
            OpCode::GetGlobal => {
                // Read the name by reference (no per-execution String clone); the
                // frame's proto is a disjoint field from the globals table.
                let idx = self.read_u16() as usize;
                let fi = self.frames.len() - 1;
                let m = self.frames[fi].module;
                let name = const_str(&self.frames[fi].proto, idx);
                let found = self.module_globals[m].get(name).or_else(|| self.builtins.get(name)).copied();
                match found {
                    Some(v) => self.push(v),
                    None => {
                        let owned = name.to_string();
                        return Err(self.throw(error_kind::NAME, format!("undefined variable '{owned}'")));
                    }
                }
            }
            OpCode::SetGlobal => {
                let idx = self.read_u16() as usize;
                let fi = self.frames.len() - 1;
                let m = self.frames[fi].module;
                let v = self.peek(0);
                let name = const_str(&self.frames[fi].proto, idx);
                // Update in place when the global exists (no clone in the hot path).
                if let Some(slot) = self.module_globals[m].get_mut(name) {
                    *slot = v;
                } else if self.builtins.contains_key(name) {
                    let owned = name.to_string();
                    self.module_globals[m].insert(owned, v);
                } else {
                    let owned = name.to_string();
                    return Err(self.throw(error_kind::NAME, format!("undefined variable '{owned}'")));
                }
            }
            OpCode::GetLocal => {
                let slot = self.read_byte() as usize;
                let base = self.frames.last().unwrap().slot_base;
                let v = self.stack[base + slot];
                self.push(v);
            }
            OpCode::SetLocal => {
                let slot = self.read_byte() as usize;
                let base = self.frames.last().unwrap().slot_base;
                let v = self.peek(0);
                self.stack[base + slot] = v;
            }
            OpCode::GetUpvalue => {
                let idx = self.read_byte() as usize;
                let closure = self.frames.last().unwrap().closure;
                let uv = self.closure_upvalue(closure, idx);
                let v = match self.heap.get(uv) {
                    Obj::Upvalue(Upvalue::Open(i)) => self.stack[*i],
                    Obj::Upvalue(Upvalue::Closed(v)) => *v,
                    _ => unreachable!("upvalue slot is not an upvalue"),
                };
                self.push(v);
            }
            OpCode::SetUpvalue => {
                let idx = self.read_byte() as usize;
                let closure = self.frames.last().unwrap().closure;
                let uv = self.closure_upvalue(closure, idx);
                let v = self.peek(0);
                match self.heap.get_mut(uv) {
                    Obj::Upvalue(Upvalue::Open(i)) => {
                        let i = *i;
                        self.stack[i] = v;
                    }
                    Obj::Upvalue(u @ Upvalue::Closed(_)) => *u = Upvalue::Closed(v),
                    _ => unreachable!(),
                }
            }
            OpCode::CloseUpvalue => {
                let top = self.stack.len() - 1;
                self.close_upvalues(top);
                self.pop();
            }
            OpCode::Add => self.binary_add()?,
            OpCode::Sub => self.binary_num(op)?,
            OpCode::Mul => self.binary_num(op)?,
            OpCode::Pow => self.binary_pow()?,
            OpCode::Div => self.binary_num(op)?,
            OpCode::Rem => self.binary_num(op)?,
            OpCode::Neg => {
                let v = self.pop();
                match v {
                    Value::Int(n) => self.push(Value::Int(n.wrapping_neg())),
                    Value::Float(f) => self.push(Value::Float(-f)),
                    _ => match self.dispatch_dunder(v, "__neg__", &[]) {
                        Some(r) => {
                            let res = r?;
                            self.push(res);
                        }
                        None => return Err(self.throw(error_kind::TYPE, "cannot negate a non-number")),
                    },
                }
            }
            OpCode::Not => {
                let v = self.pop();
                self.push(Value::Bool(!v.is_truthy()));
            }
            OpCode::Eq => self.op_equal(false)?,
            OpCode::Ne => self.op_equal(true)?,
            OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge => self.binary_compare(op)?,
            OpCode::Is => {
                let class = self.pop();
                let value = self.pop();
                let result = self.value_is_instance_of(value, class)?;
                self.push(Value::Bool(result));
            }
            OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor | OpCode::Shl | OpCode::Shr => {
                self.binary_bitwise(op)?
            }
            OpCode::BitNot => {
                let v = self.pop();
                match v {
                    Value::Int(n) => self.push(Value::Int(!n)),
                    _ => return Err(self.throw(error_kind::TYPE, "cannot apply '~' to a non-integer")),
                }
            }
            OpCode::Jump => {
                let off = self.read_u16() as usize;
                self.frames.last_mut().unwrap().ip += off;
            }
            OpCode::JumpIfFalse => {
                let off = self.read_u16() as usize;
                if !self.peek(0).is_truthy() {
                    self.frames.last_mut().unwrap().ip += off;
                }
            }
            OpCode::Loop => {
                let off = self.read_u16() as usize;
                self.frames.last_mut().unwrap().ip -= off;
                self.charge_budget()?; // bound infinite loops (fuzzing)
                self.maybe_collect(); // back-edge: a GC safe point
            }
            OpCode::Call => {
                let argc = self.read_byte() as usize;
                let callee_idx = self.stack.len() - argc - 1;
                let callee = self.stack[callee_idx];
                self.call_value(callee, argc, callee_idx)?;
            }
            OpCode::TailCall => {
                // Tail position: reuse the current frame for a closure callee,
                // else fall back to a normal call (the following RETURN returns
                // its result) — DESIGN D30.
                let argc = self.read_byte() as usize;
                let callee_idx = self.stack.len() - argc - 1;
                let callee = self.stack[callee_idx];
                match self.tail_target(callee) {
                    Some((slot0, closure, proto)) => {
                        self.tail_reuse_frame(slot0, closure, proto, argc, callee_idx)?
                    }
                    None => self.call_value(callee, argc, callee_idx)?,
                }
            }
            OpCode::CallSpread => {
                // Stack: [..., callee, argv]. `argv` is a freshly built array;
                // splice its elements in as the arguments, then call. No GC can
                // run between popping `argv` and `call_value` (single instruction),
                // and `call_value`'s own collection sees the args rooted on stack.
                let argv = self.pop();
                let elems = match argv.as_obj().map(|r| self.heap.get(r)) {
                    Some(Obj::Array(a)) => a.clone(),
                    _ => unreachable!("CALL_SPREAD argv is always an array"),
                };
                let callee_idx = self.stack.len() - 1;
                let callee = self.stack[callee_idx];
                let argc = elems.len();
                for e in elems {
                    self.push(e);
                }
                self.call_value(callee, argc, callee_idx)?;
            }
            OpCode::Closure => self.op_closure()?,
            OpCode::Return => {
                let result = self.pop();
                let frame = self.frames.pop().unwrap();
                self.close_upvalues(frame.slot_base);
                self.handlers.retain(|h| h.frame < self.frames.len());
                self.stack.truncate(frame.slot_base);
                self.push(result);
            }
            OpCode::NewArray => {
                let r = self.heap.alloc_array(Vec::new());
                self.push(Value::Obj(r));
            }
            OpCode::ArrayPush => {
                let v = self.pop();
                let arr = self.peek(0).as_obj().unwrap();
                if let Obj::Array(a) = self.heap.get_mut(arr) {
                    a.push(v);
                }
                self.write_barrier(arr, v);
            }
            OpCode::ArrayExtend => {
                let it = self.pop();
                let items = self.iterable_values(it)?;
                let arr = self.peek(0).as_obj().unwrap();
                for &item in &items {
                    self.write_barrier(arr, item);
                }
                if let Obj::Array(a) = self.heap.get_mut(arr) {
                    a.extend(items);
                }
            }
            OpCode::NewMap => {
                let r = self.heap.alloc_map(LumMap::new());
                self.push(Value::Obj(r));
            }
            OpCode::MapInsert => {
                let v = self.pop();
                let k = self.pop();
                let key = self.map_key(k)?;
                let m = self.peek(0).as_obj().unwrap();
                if let Obj::Map(map) = self.heap.get_mut(m) {
                    map.insert(key, k, v);
                }
                self.write_barrier(m, k);
                self.write_barrier(m, v);
            }
            OpCode::IndexGet => self.op_index_get()?,
            OpCode::IndexSet => self.op_index_set()?,
            OpCode::GetProp => {
                let name = self.read_string();
                let obj = self.pop();
                let v = self.get_property(obj, &name)?;
                self.push(v);
            }
            OpCode::SetProp => {
                let name = self.read_string();
                let v = self.pop();
                let obj = self.pop();
                self.set_property(obj, &name, v)?;
                self.push(v);
            }
            OpCode::Class => {
                let name = self.read_string();
                let r = self.heap.alloc(Obj::Class(Class {
                    name,
                    superclass: None,
                    methods: FxHashMap::default(),
                    statics: FxHashMap::default(),
                }));
                self.push(Value::Obj(r));
            }
            OpCode::Inherit => self.op_inherit()?,
            OpCode::Method => {
                let name = self.read_string();
                let method = self.pop().as_obj().unwrap();
                let class = self.peek(0).as_obj().unwrap();
                if let Obj::Class(c) = self.heap.get_mut(class) {
                    c.methods.insert(name, method);
                }
                self.write_barrier(class, Value::Obj(method));
            }
            OpCode::StaticMethod => {
                let name = self.read_string();
                let method = self.pop().as_obj().unwrap();
                let class = self.peek(0).as_obj().unwrap();
                if let Obj::Class(c) = self.heap.get_mut(class) {
                    c.statics.insert(name, method);
                }
                self.write_barrier(class, Value::Obj(method));
            }
            OpCode::GetSuper => {
                let name = self.read_string();
                let superclass = self.pop();
                let receiver = self.pop();
                let v = self.bind_super(superclass, receiver, &name)?;
                self.push(v);
            }
            OpCode::PushHandler => {
                let off = self.read_u16() as usize;
                let frame = self.frames.len() - 1;
                let catch_ip = self.frames.last().unwrap().ip + off;
                self.handlers.push(Handler { catch_ip, stack_len: self.stack.len(), frame });
            }
            OpCode::PopHandler => {
                self.handlers.pop();
            }
            OpCode::Throw => {
                let v = self.pop();
                return Err(v);
            }
            OpCode::Yield => {
                // Suspend: record the yielded value; `run_until` returns to
                // `resume_generator`, and execution resumes at the next
                // instruction on the next call (DESIGN D29).
                let v = self.pop();
                self.pending_yield = Some(v);
            }
            OpCode::Interpolate => {
                let n = self.read_byte() as usize;
                let start = self.stack.len() - n;
                // Stringify the parts *while they are still on the stack* so a
                // custom `str()` re-entering the VM (and possibly triggering GC)
                // cannot free them. Only truncate once the string is built.
                let mut s = String::new();
                for i in 0..n {
                    let p = self.stack[start + i];
                    s.push_str(&self.to_display(p, false)?);
                }
                self.stack.truncate(start);
                let r = self.heap.intern(&s);
                self.push(Value::Obj(r));
            }
            OpCode::Import => {
                let path = self.read_string();
                let m = self.import_module(&path)?;
                self.push(m);
            }
            OpCode::IterNext => self.op_iter_next()?,
            OpCode::MatchArray => {
                let exact = {
                    let min = self.read_byte() as usize;
                    let exact = self.read_byte() == 1;
                    (min, exact)
                };
                let (min, is_exact) = exact;
                let v = self.pop();
                let ok = match v.as_obj().map(|r| self.heap.get(r)) {
                    Some(Obj::Array(a)) => {
                        if is_exact {
                            a.len() == min
                        } else {
                            a.len() >= min
                        }
                    }
                    _ => false,
                };
                self.push(Value::Bool(ok));
            }
            OpCode::MatchMapHas => {
                let key_name = self.read_string();
                let v = self.pop();
                let ok = match v.as_obj().map(|r| self.heap.get(r)) {
                    Some(Obj::Map(_)) => {
                        let key = self.heap.intern(&key_name);
                        if let Some(Obj::Map(m)) = v.as_obj().map(|r| self.heap.get(r)) {
                            m.has(MapKey::Str(key))
                        } else {
                            false
                        }
                    }
                    _ => false,
                };
                self.push(Value::Bool(ok));
            }
            OpCode::MatchError => {
                let kind = self.read_string();
                let v = self.pop();
                let ok = matches!(
                    v.as_obj().map(|r| self.heap.get(r)),
                    Some(Obj::Error(e)) if e.kind == kind
                );
                self.push(Value::Bool(ok));
            }
            OpCode::ArrayRest => {
                let front = self.read_byte() as usize;
                let back = self.read_byte() as usize;
                let arr = self.pop();
                let slice: Vec<Value> = match arr.as_obj().map(|r| self.heap.get(r)) {
                    Some(Obj::Array(a)) => {
                        let end = a.len().saturating_sub(back);
                        a[front.min(end)..end].to_vec()
                    }
                    _ => Vec::new(),
                };
                let r = self.heap.alloc_array(slice);
                self.push(Value::Obj(r));
            }
            OpCode::CloseUpvalueSlot => {
                let slot = self.read_byte() as usize;
                let base = self.frames.last().unwrap().slot_base;
                self.close_upvalues(base + slot);
            }
            OpCode::Invoke => {
                // Fused `obj.name(args)`. The fast path (an instance method not
                // shadowed by a field) reads the name by reference, finds the
                // method, and calls it directly — no name clone, no bound-method
                // allocation. Only the slow path (modules/fields/non-objects)
                // clones the name to reuse `get_property`.
                let idx = self.read_u16() as usize;
                let argc = self.read_byte() as usize;
                let receiver_idx = self.stack.len() - argc - 1;
                let receiver = self.stack[receiver_idx];
                let fi = self.frames.len() - 1;
                let name = const_str(&self.frames[fi].proto, idx);
                let fast_class = match receiver {
                    Value::Obj(r) => match self.heap.get(r) {
                        Obj::Instance(inst) if !inst.fields.contains_key(name) => Some(inst.class),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(class) = fast_class {
                    if let Some(method) = self.find_method(class, name) {
                        let proto = self.closure_proto(method);
                        return self.call_closure(method, proto, argc, receiver_idx);
                    }
                }
                let owned = name.to_string();
                let callable = self.get_property(receiver, &owned)?;
                self.stack[receiver_idx] = callable;
                return self.call_value(callable, argc, receiver_idx);
            }
            OpCode::SuperInvoke => {
                // Fused `super.name(args)`. Stack: [this, args…, superclass]; pop
                // the superclass, resolve the method in it, and call directly with
                // `this` already in the receiver slot — no bound-method allocation.
                let idx = self.read_u16() as usize;
                let argc = self.read_byte() as usize;
                let superclass = self.pop();
                let receiver_idx = self.stack.len() - argc - 1;
                let fi = self.frames.len() - 1;
                let name = const_str(&self.frames[fi].proto, idx);
                let sr = superclass.as_obj().unwrap();
                if let Some(method) = self.find_method(sr, name) {
                    let proto = self.closure_proto(method);
                    return self.call_closure(method, proto, argc, receiver_idx);
                }
                let owned = name.to_string();
                return Err(
                    self.throw(error_kind::NAME, format!("undefined method '{owned}' in superclass"))
                );
            }
            OpCode::DefaultArg => {
                let param_index = self.read_byte() as usize;
                let skip = self.read_u16() as usize;
                // If the caller supplied this parameter, skip its default-value
                // expression; otherwise fall through to evaluate it.
                if self.frames.last().unwrap().provided_argc > param_index {
                    self.frames.last_mut().unwrap().ip += skip;
                }
            }
        }
        Ok(())
    }

    // ---- arithmetic & comparison ------------------------------------------

    /// The class of `v` if it is a class instance (for operator-overload dispatch).
    fn instance_class_of(&self, v: Value) -> Option<GcRef> {
        match v.as_obj() {
            Some(r) => match self.heap.get(r) {
                Obj::Instance(inst) => Some(inst.class),
                _ => None,
            },
            None => None,
        }
    }

    /// If `recv` is an instance whose class defines the dunder `method`, invoke
    /// `recv.method(args)` and return `Some(result)`; otherwise `None` so the
    /// caller can keep its built-in behavior / `TypeError` (DESIGN D26). The
    /// receiver is rooted by the heap bound method and the args by `call_and_run`,
    /// so a GC during the re-entrant call cannot free them (DESIGN D18).
    fn dispatch_dunder(
        &mut self,
        recv: Value,
        method: &str,
        args: &[Value],
    ) -> Option<Result<Value, Value>> {
        let class = self.instance_class_of(recv)?;
        let m = self.find_method(class, method)?;
        let bound = self.heap.alloc(Obj::Bound(BoundMethod { receiver: recv, method: m }));
        Some(self.call_and_run(Value::Obj(bound), args))
    }

    fn binary_add(&mut self) -> Result<(), Value> {
        let b = self.pop();
        let a = self.pop();
        let fast: Option<Value> = match (a, b) {
            (Value::Int(x), Value::Int(y)) => match x.checked_add(y) {
                Some(r) => Some(Value::Int(r)),
                None => return Err(self.throw(error_kind::VALUE, "integer overflow in addition")),
            },
            _ if a.as_f64().is_some() && b.as_f64().is_some() => {
                Some(Value::Float(a.as_f64().unwrap() + b.as_f64().unwrap()))
            }
            (Value::Obj(ra), Value::Obj(rb)) => match (self.heap.get(ra), self.heap.get(rb)) {
                (Obj::Str(sa), Obj::Str(sb)) => {
                    let mut s = sa.clone();
                    s.push_str(sb);
                    Some(Value::Obj(self.heap.intern(&s)))
                }
                (Obj::Array(aa), Obj::Array(ab)) => {
                    let mut v = aa.clone();
                    v.extend_from_slice(ab);
                    Some(Value::Obj(self.heap.alloc_array(v)))
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(v) = fast {
            self.push(v);
            return Ok(());
        }
        match self.dispatch_dunder(a, "__add__", &[b]) {
            Some(r) => {
                let v = r?;
                self.push(v);
                Ok(())
            }
            None => Err(self.type_error_binary("+", a, b)),
        }
    }

    fn binary_num(&mut self, op: OpCode) -> Result<(), Value> {
        let b = self.pop();
        let a = self.pop();
        // String repeat: `"ab" * 3` and `3 * "ab"` (a negative count gives "").
        if matches!(op, OpCode::Mul) {
            let rep = match (a, b) {
                (Value::Obj(r), Value::Int(n)) | (Value::Int(n), Value::Obj(r)) => {
                    match self.heap.get(r) {
                        Obj::Str(s) => Some((s.clone(), n)),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some((s, n)) = rep {
                let repeated = s.repeat(n.max(0) as usize);
                let v = Value::Obj(self.heap.intern(&repeated));
                self.push(v);
                return Ok(());
            }
        }
        let (x, y) = match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                // Operator overloading: a.__sub__/__mul__/__div__/__mod__(b).
                let method = match op {
                    OpCode::Sub => "__sub__",
                    OpCode::Mul => "__mul__",
                    OpCode::Div => "__div__",
                    OpCode::Rem => "__mod__",
                    _ => unreachable!("binary_num only handles - * / %"),
                };
                return match self.dispatch_dunder(a, method, &[b]) {
                    Some(r) => {
                        let v = r?;
                        self.push(v);
                        Ok(())
                    }
                    None => Err(self.type_error_binary(op_symbol(op), a, b)),
                };
            }
        };
        let both_int = matches!((a, b), (Value::Int(_), Value::Int(_)));
        let result = match op {
            OpCode::Sub if both_int => self.int_or_overflow(
                (a.int_unchecked()).checked_sub(b.int_unchecked()),
                "subtraction",
            )?,
            OpCode::Mul if both_int => self.int_or_overflow(
                (a.int_unchecked()).checked_mul(b.int_unchecked()),
                "multiplication",
            )?,
            OpCode::Div if both_int => {
                let (ix, iy) = (a.int_unchecked(), b.int_unchecked());
                if iy == 0 {
                    return Err(self.throw(error_kind::DIV_ZERO, "integer division by zero"));
                }
                Value::Int(ix.wrapping_div(iy))
            }
            OpCode::Rem if both_int => {
                let (ix, iy) = (a.int_unchecked(), b.int_unchecked());
                if iy == 0 {
                    return Err(self.throw(error_kind::DIV_ZERO, "integer modulo by zero"));
                }
                Value::Int(ix.wrapping_rem(iy))
            }
            OpCode::Sub => Value::Float(x - y),
            OpCode::Mul => Value::Float(x * y),
            OpCode::Div => {
                if y == 0.0 {
                    return Err(self.throw(error_kind::DIV_ZERO, "float division by zero"));
                }
                Value::Float(x / y)
            }
            OpCode::Rem => {
                if y == 0.0 {
                    return Err(self.throw(error_kind::DIV_ZERO, "float modulo by zero"));
                }
                Value::Float(x % y)
            }
            _ => unreachable!(),
        };
        self.push(result);
        Ok(())
    }

    fn int_or_overflow(&mut self, r: Option<i64>, what: &str) -> Result<Value, Value> {
        match r {
            Some(v) => Ok(Value::Int(v)),
            None => Err(self.throw(error_kind::VALUE, format!("integer overflow in {what}"))),
        }
    }

    /// Integer-only bitwise/shift operators. Non-int operands throw a TypeError;
    /// a shift amount outside `0..=63` throws a ValueError (Rust's shift would
    /// otherwise panic). `>>` is an arithmetic (sign-extending) shift.
    fn binary_bitwise(&mut self, op: OpCode) -> Result<(), Value> {
        let b = self.pop();
        let a = self.pop();
        let (ix, iy) = match (a, b) {
            (Value::Int(x), Value::Int(y)) => (x, y),
            _ => return Err(self.type_error_binary(op_symbol(op), a, b)),
        };
        let result = match op {
            OpCode::BitAnd => ix & iy,
            OpCode::BitOr => ix | iy,
            OpCode::BitXor => ix ^ iy,
            OpCode::Shl | OpCode::Shr => {
                if !(0..64).contains(&iy) {
                    return Err(self.throw(
                        error_kind::VALUE,
                        format!("shift amount out of range (0..=63), got {iy}"),
                    ));
                }
                let s = iy as u32;
                if op == OpCode::Shl {
                    ix << s
                } else {
                    ix >> s
                }
            }
            _ => unreachable!("non-bitwise opcode in binary_bitwise"),
        };
        self.push(Value::Int(result));
        Ok(())
    }

    /// `x is Class`: true iff `value` is an instance whose class is `class` or a
    /// subclass of it. The right operand must be a class (else `TypeError`).
    fn value_is_instance_of(&mut self, value: Value, class: Value) -> Result<bool, Value> {
        let class_ref = match class.as_obj() {
            Some(r) if matches!(self.heap.get(r), Obj::Class(_)) => r,
            _ => return Err(self.throw(error_kind::TYPE, "right operand of 'is' must be a class")),
        };
        let Some(obj) = value.as_obj() else {
            return Ok(false); // only instances can match
        };
        let mut cur = match self.heap.get(obj) {
            Obj::Instance(inst) => Some(inst.class),
            _ => return Ok(false),
        };
        while let Some(c) = cur {
            if c == class_ref {
                return Ok(true);
            }
            cur = match self.heap.get(c) {
                Obj::Class(cl) => cl.superclass,
                _ => None,
            };
        }
        Ok(false)
    }

    /// `==` / `!=`, honoring an `__eq__` dunder on the left operand (DESIGN D26).
    fn op_equal(&mut self, negate: bool) -> Result<(), Value> {
        let b = self.pop();
        let a = self.pop();
        let eq = match self.dispatch_dunder(a, "__eq__", &[b]) {
            Some(r) => r?.is_truthy(),
            None => self.values_equal(a, b),
        };
        self.push(Value::Bool(eq ^ negate));
        Ok(())
    }

    /// `a ** b`: `int ** nonneg-int` is an int (overflow throws like `*`);
    /// otherwise the result is a float.
    fn binary_pow(&mut self) -> Result<(), Value> {
        let b = self.pop();
        let a = self.pop();
        let result = match (a, b) {
            (Value::Int(base), Value::Int(exp)) if exp >= 0 => {
                match u32::try_from(exp).ok().and_then(|e| base.checked_pow(e)) {
                    Some(r) => Value::Int(r),
                    None => {
                        return Err(self.throw(error_kind::VALUE, "integer overflow in exponentiation"))
                    }
                }
            }
            _ => match (a.as_f64(), b.as_f64()) {
                (Some(x), Some(y)) => Value::Float(x.powf(y)),
                _ => return Err(self.type_error_binary("**", a, b)),
            },
        };
        self.push(result);
        Ok(())
    }

    fn binary_compare(&mut self, op: OpCode) -> Result<(), Value> {
        let b = self.pop();
        let a = self.pop();
        // Operator overloading: all four orderings go through `__lt__` by
        // swapping operands and/or negating (DESIGN D26).
        if self.instance_class_of(a).is_some() || self.instance_class_of(b).is_some() {
            let (recv, arg, negate) = match op {
                OpCode::Lt => (a, b, false),
                OpCode::Gt => (b, a, false),
                OpCode::Le => (b, a, true),
                OpCode::Ge => (a, b, true),
                _ => unreachable!("binary_compare only handles < <= > >="),
            };
            if let Some(r) = self.dispatch_dunder(recv, "__lt__", &[arg]) {
                let v = r?;
                self.push(Value::Bool(v.is_truthy() ^ negate));
                return Ok(());
            }
        }
        let ord = if let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) {
            x.partial_cmp(&y)
        } else if let (Value::Obj(ra), Value::Obj(rb)) = (a, b) {
            match (self.heap.get(ra), self.heap.get(rb)) {
                (Obj::Str(sa), Obj::Str(sb)) => Some(sa.cmp(sb)),
                _ => return Err(self.type_error_binary(op_symbol(op), a, b)),
            }
        } else {
            return Err(self.type_error_binary(op_symbol(op), a, b));
        };
        use std::cmp::Ordering;
        let result = match (op, ord) {
            (_, None) => false, // NaN: all comparisons are false
            (OpCode::Lt, Some(o)) => o == Ordering::Less,
            (OpCode::Le, Some(o)) => o != Ordering::Greater,
            (OpCode::Gt, Some(o)) => o == Ordering::Greater,
            (OpCode::Ge, Some(o)) => o != Ordering::Less,
            _ => unreachable!(),
        };
        self.push(Value::Bool(result));
        Ok(())
    }

    fn type_error_binary(&mut self, op: &str, a: Value, b: Value) -> Value {
        let ta = self.type_name(a);
        let tb = self.type_name(b);
        self.throw(error_kind::TYPE, format!("operator '{op}' cannot be applied to {ta} and {tb}"))
    }

    pub fn values_equal(&self, a: Value, b: Value) -> bool {
        match (a, b) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(x), Value::Bool(y)) => x == y,
            (Value::Int(x), Value::Int(y)) => x == y,
            (Value::Float(x), Value::Float(y)) => x == y,
            (Value::Int(x), Value::Float(y)) | (Value::Float(y), Value::Int(x)) => (x as f64) == y,
            (Value::Obj(x), Value::Obj(y)) => {
                if x == y {
                    return true;
                }
                match (self.heap.get(x), self.heap.get(y)) {
                    (Obj::Str(sa), Obj::Str(sb)) => sa == sb,
                    _ => false,
                }
            }
            _ => false,
        }
    }

    // ---- indexing & properties --------------------------------------------

    fn op_index_get(&mut self) -> Result<(), Value> {
        let index = self.pop();
        let obj = self.pop();
        let r = match obj {
            Value::Obj(r) => r,
            _ => return Err(self.throw(error_kind::TYPE, "cannot index a non-collection")),
        };
        // Classify and copy out what we need so no heap borrow spans the index
        // computation (which may allocate an error).
        enum Kind {
            Array(usize),
            Str(Vec<char>),
            Map,
            Bad,
        }
        let kind = match self.heap.get(r) {
            Obj::Array(a) => Kind::Array(a.len()),
            Obj::Str(s) => Kind::Str(s.chars().collect()),
            Obj::Map(_) => Kind::Map,
            _ => Kind::Bad,
        };
        let result = match kind {
            Kind::Array(len) => {
                let i = self.array_index(index, len)?;
                if let Obj::Array(a) = self.heap.get(r) {
                    a[i]
                } else {
                    Value::Nil
                }
            }
            Kind::Str(chars) => {
                let i = self.array_index(index, chars.len())?;
                let ch = chars[i].to_string();
                Value::Obj(self.heap.intern(&ch))
            }
            Kind::Map => {
                let key = self.map_key(index)?;
                if let Obj::Map(m) = self.heap.get(r) {
                    m.get(key).unwrap_or(Value::Nil)
                } else {
                    Value::Nil
                }
            }
            Kind::Bad => {
                // Operator overloading: obj.__index__(index).
                return match self.dispatch_dunder(obj, "__index__", &[index]) {
                    Some(r) => {
                        let v = r?;
                        self.push(v);
                        Ok(())
                    }
                    None => Err(self.throw(error_kind::TYPE, "value is not indexable")),
                };
            }
        };
        self.push(result);
        Ok(())
    }

    fn op_index_set(&mut self) -> Result<(), Value> {
        let value = self.pop();
        let index = self.pop();
        let obj = self.pop();
        let r = match obj {
            Value::Obj(r) => r,
            _ => return Err(self.throw(error_kind::TYPE, "cannot index-assign a non-collection")),
        };
        // Operator overloading: obj.__set_index__(index, value).
        if self.instance_class_of(obj).is_some() {
            if let Some(res) = self.dispatch_dunder(obj, "__set_index__", &[index, value]) {
                res?;
                self.push(value);
                return Ok(());
            }
        }
        match self.heap.get(r) {
            Obj::Array(a) => {
                let len = a.len();
                let i = match index {
                    Value::Int(n) if n >= 0 && (n as usize) < len => n as usize,
                    Value::Int(n) if n < 0 && ((-n) as usize) <= len => (len as i64 + n) as usize,
                    Value::Int(n) if n as usize == len => {
                        if let Obj::Array(a) = self.heap.get_mut(r) {
                            a.push(value);
                        }
                        self.write_barrier(r, value);
                        self.push(value);
                        return Ok(());
                    }
                    Value::Int(n) => {
                        return Err(self.throw(
                            error_kind::INDEX,
                            format!("index {n} out of bounds for array of length {len}"),
                        ))
                    }
                    _ => return Err(self.throw(error_kind::TYPE, "array index must be an integer")),
                };
                if let Obj::Array(a) = self.heap.get_mut(r) {
                    a[i] = value;
                }
                self.write_barrier(r, value);
            }
            Obj::Map(_) => {
                let key = self.map_key(index)?;
                if let Obj::Map(m) = self.heap.get_mut(r) {
                    m.insert(key, index, value);
                }
                self.write_barrier(r, index);
                self.write_barrier(r, value);
            }
            _ => return Err(self.throw(error_kind::TYPE, "value does not support index assignment")),
        }
        self.push(value);
        Ok(())
    }

    fn array_index(&mut self, index: Value, len: usize) -> Result<usize, Value> {
        match index {
            Value::Int(n) if n >= 0 && (n as usize) < len => Ok(n as usize),
            Value::Int(n) if n < 0 && ((-n) as usize) <= len => Ok((len as i64 + n) as usize),
            Value::Int(n) => Err(self.throw(
                error_kind::INDEX,
                format!("index {n} out of bounds for length {len}"),
            )),
            _ => Err(self.throw(error_kind::TYPE, "index must be an integer")),
        }
    }

    fn get_property(&mut self, obj: Value, name: &str) -> Result<Value, Value> {
        let r = match obj {
            Value::Obj(r) => r,
            _ => {
                let t = self.type_name(obj);
                return Err(self.throw(error_kind::TYPE, format!("{t} has no property '{name}'")));
            }
        };
        match self.heap.get(r) {
            Obj::Instance(inst) => {
                if let Some(v) = inst.fields.get(name) {
                    return Ok(*v);
                }
                let class = inst.class;
                if let Some(method) = self.find_method(class, name) {
                    let bound = self.heap.alloc(Obj::Bound(BoundMethod { receiver: obj, method }));
                    return Ok(Value::Obj(bound));
                }
                Ok(Value::Nil) // missing field reads as nil
            }
            Obj::Class(c) => match c.statics.get(name) {
                Some(m) => Ok(Value::Obj(*m)), // static methods have no receiver
                None => {
                    let cn = c.name.clone();
                    Err(self.throw(error_kind::NAME, format!("class '{cn}' has no static '{name}'")))
                }
            },
            Obj::Module(m) => match m.exports.get(name) {
                Some(v) => Ok(*v),
                None => {
                    let mn = m.name.clone();
                    Err(self.throw(error_kind::NAME, format!("module '{mn}' has no export '{name}'")))
                }
            },
            Obj::Error(e) => match name {
                "kind" => {
                    let s = e.kind.clone();
                    Ok(Value::Obj(self.heap.intern(&s)))
                }
                "message" => {
                    let s = e.message.clone();
                    Ok(Value::Obj(self.heap.intern(&s)))
                }
                _ => Ok(Value::Nil),
            },
            other => {
                let t = other.type_name();
                Err(self.throw(error_kind::TYPE, format!("{t} has no property '{name}'")))
            }
        }
    }

    fn set_property(&mut self, obj: Value, name: &str, value: Value) -> Result<(), Value> {
        let r = match obj {
            Value::Obj(r) if matches!(self.heap.get(r), Obj::Instance(_)) => r,
            _ => return Err(self.throw(error_kind::TYPE, "can only set fields on instances")),
        };
        if let Obj::Instance(inst) = self.heap.get_mut(r) {
            inst.fields.insert(name.to_string(), value);
        }
        self.write_barrier(r, value);
        Ok(())
    }

    fn find_method(&self, class: GcRef, name: &str) -> Option<GcRef> {
        if let Obj::Class(c) = self.heap.get(class) {
            c.methods.get(name).copied()
        } else {
            None
        }
    }

    fn bind_super(&mut self, superclass: Value, receiver: Value, name: &str) -> Result<Value, Value> {
        let sr = superclass.as_obj().unwrap();
        match self.find_method(sr, name) {
            Some(method) => {
                let bound = self.heap.alloc(Obj::Bound(BoundMethod { receiver, method }));
                Ok(Value::Obj(bound))
            }
            None => Err(self.throw(error_kind::NAME, format!("undefined method '{name}' in superclass"))),
        }
    }

    fn op_inherit(&mut self) -> Result<(), Value> {
        let class = self.peek(0).as_obj().unwrap();
        let superv = self.peek(1);
        let super_ref = match superv {
            Value::Obj(r) if matches!(self.heap.get(r), Obj::Class(_)) => r,
            _ => return Err(self.throw(error_kind::TYPE, "a superclass must be a class")),
        };
        let (methods, statics) = if let Obj::Class(sc) = self.heap.get(super_ref) {
            (sc.methods.clone(), sc.statics.clone())
        } else {
            (FxHashMap::default(), FxHashMap::default())
        };
        let inherited: Vec<GcRef> = methods.values().chain(statics.values()).copied().collect();
        if let Obj::Class(c) = self.heap.get_mut(class) {
            c.methods = methods; // copy-down; own methods/statics override later
            c.statics = statics;
            c.superclass = Some(super_ref);
        }
        self.write_barrier(class, Value::Obj(super_ref));
        for m in inherited {
            self.write_barrier(class, Value::Obj(m));
        }
        Ok(())
    }

    // ---- closures & upvalues ----------------------------------------------

    fn op_closure(&mut self) -> Result<(), Value> {
        let proto = match self.read_constant() {
            Constant::Fn(p) => p,
            _ => unreachable!("CLOSURE operand is not a function"),
        };
        let count = proto.upvalue_count;
        let mut upvalues = Vec::with_capacity(count);
        for _ in 0..count {
            let is_local = self.read_byte() == 1;
            let index = self.read_byte() as usize;
            let uv = if is_local {
                let base = self.frames.last().unwrap().slot_base;
                self.capture_upvalue(base + index)
            } else {
                let enclosing = self.frames.last().unwrap().closure;
                self.closure_upvalue(enclosing, index)
            };
            upvalues.push(uv);
        }
        let module = self.frames.last().unwrap().module;
        let r = self.heap.alloc_closure(Closure { proto, upvalues, module });
        self.push(Value::Obj(r));
        Ok(())
    }

    fn closure_upvalue(&self, closure: GcRef, idx: usize) -> GcRef {
        if let Obj::Closure(c) = self.heap.get(closure) {
            c.upvalues[idx]
        } else {
            unreachable!("frame closure is not a closure")
        }
    }

    fn capture_upvalue(&mut self, stack_idx: usize) -> GcRef {
        for &uv in &self.open_upvalues {
            if let Obj::Upvalue(Upvalue::Open(i)) = self.heap.get(uv) {
                if *i == stack_idx {
                    return uv;
                }
            }
        }
        let uv = self.heap.alloc_upvalue(Upvalue::Open(stack_idx));
        self.open_upvalues.push(uv);
        uv
    }

    fn close_upvalues(&mut self, from: usize) {
        let mut i = 0;
        while i < self.open_upvalues.len() {
            let uv = self.open_upvalues[i];
            let close_to = match self.heap.get(uv) {
                Obj::Upvalue(Upvalue::Open(idx)) if *idx >= from => Some(*idx),
                _ => None,
            };
            if let Some(idx) = close_to {
                let v = self.stack[idx];
                *self.heap.get_mut(uv) = Obj::Upvalue(Upvalue::Closed(v));
                self.write_barrier(uv, v); // a closed upvalue may now point old->young
                self.open_upvalues.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    // ---- calls -------------------------------------------------------------

    fn call_value(&mut self, callee: Value, argc: usize, callee_idx: usize) -> Result<(), Value> {
        let r = match callee {
            Value::Obj(r) => r,
            _ => return Err(self.throw(error_kind::TYPE, "can only call functions, classes, and methods")),
        };
        // Inspect the callee's kind without holding the borrow across mutation.
        enum Kind {
            Closure(Rc<FnProto>),
            Native(Native),
            Class,
            Bound(Value, GcRef),
            Bad,
        }
        let kind = match self.heap.get(r) {
            Obj::Closure(c) => Kind::Closure(c.proto.clone()),
            Obj::Native(n) => Kind::Native(Native { name: n.name.clone(), arity: n.arity, func: n.func }),
            Obj::Class(_) => Kind::Class,
            Obj::Bound(b) => Kind::Bound(b.receiver, b.method),
            _ => Kind::Bad,
        };
        match kind {
            Kind::Closure(proto) => self.call_closure(r, proto, argc, callee_idx),
            Kind::Native(n) => {
                if !n.arity.accepts(argc) {
                    return Err(self.throw(
                        error_kind::ARITY,
                        format!("'{}' expects {} argument(s), got {argc}", n.name, n.arity.describe()),
                    ));
                }
                let args: Vec<Value> = self.stack[callee_idx + 1..].to_vec();
                let result = (n.func)(self, &args)?;
                self.stack.truncate(callee_idx);
                self.push(result);
                Ok(())
            }
            Kind::Class => self.instantiate(r, argc, callee_idx),
            Kind::Bound(receiver, method) => {
                self.stack[callee_idx] = receiver; // slot 0 becomes `this`
                let proto = self.closure_proto(method);
                self.call_closure(method, proto, argc, callee_idx)
            }
            Kind::Bad => Err(self.throw(error_kind::TYPE, "can only call functions, classes, and methods")),
        }
    }

    fn closure_proto(&self, closure: GcRef) -> Rc<FnProto> {
        if let Obj::Closure(c) = self.heap.get(closure) {
            c.proto.clone()
        } else {
            unreachable!()
        }
    }

    fn call_closure(
        &mut self,
        closure: GcRef,
        proto: Rc<FnProto>,
        argc: usize,
        callee_idx: usize,
    ) -> Result<(), Value> {
        let fixed = proto.arity;
        let required = proto.required_arity;
        let has_rest = proto.has_rest;
        if argc < required || (!has_rest && argc > fixed) {
            let name = proto.name.clone().unwrap_or_else(|| "<anonymous>".to_string());
            let expected = if has_rest {
                format!("at least {required}")
            } else if required == fixed {
                format!("{fixed}")
            } else {
                format!("{required} to {fixed}")
            };
            return Err(self.throw(
                error_kind::ARITY,
                format!("'{name}' expects {expected} argument(s), got {argc}"),
            ));
        }
        // Calling a generator function does not run it: package the call into a
        // suspended Generator and yield that as the result (DESIGN D29).
        if proto.is_generator {
            return self.make_generator(closure, argc, callee_idx);
        }
        self.enter_frame(closure, proto, argc, callee_idx)
    }

    /// If `callee` is a tail-call-optimizable target (a non-generator closure, or
    /// a bound method to one), return `(slot0, closure, proto)` where `slot0` is
    /// the value for the reused frame's slot 0 (the closure, or the receiver for a
    /// bound method). Otherwise `None` (DESIGN D30).
    fn tail_target(&self, callee: Value) -> Option<(Value, GcRef, Rc<FnProto>)> {
        let r = callee.as_obj()?;
        match self.heap.get(r) {
            Obj::Closure(c) if !c.proto.is_generator => Some((callee, r, c.proto.clone())),
            Obj::Bound(b) => {
                let (receiver, mr) = (b.receiver, b.method);
                if let Obj::Closure(c) = self.heap.get(mr) {
                    if !c.proto.is_generator {
                        return Some((receiver, mr, c.proto.clone()));
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Reuse the current frame for a tail call: close its open upvalues, move the
    /// callee's `[slot0, args…]` down over the frame's slots, drop the frame, and
    /// re-enter at the same `slot_base` — so recursion does not grow the stack
    /// (DESIGN D30).
    fn tail_reuse_frame(
        &mut self,
        slot0: Value,
        closure: GcRef,
        proto: Rc<FnProto>,
        argc: usize,
        callee_idx: usize,
    ) -> Result<(), Value> {
        let base = self.frames.last().unwrap().slot_base;
        self.close_upvalues(base);
        let args: Vec<Value> = self.stack[callee_idx + 1..callee_idx + 1 + argc].to_vec();
        self.stack.truncate(base);
        self.stack.push(slot0);
        self.stack.extend(args);
        self.frames.pop(); // discard the current frame; enter_frame re-pushes at `base`
        self.enter_frame(closure, proto, argc, base)
    }

    /// Lay out a closure's parameter slots and push its call frame. Shared by the
    /// normal call path and a generator's first resume.
    fn enter_frame(
        &mut self,
        closure: GcRef,
        proto: Rc<FnProto>,
        argc: usize,
        callee_idx: usize,
    ) -> Result<(), Value> {
        let fixed = proto.arity;
        let has_rest = proto.has_rest;
        if self.frames.len() >= MAX_FRAMES {
            return Err(self.throw(error_kind::STACK_OVERFLOW, "call stack overflow (recursion too deep)"));
        }
        self.charge_budget()?; // bound runaway recursion (fuzzing)
        // Lay out the parameter slots: collect a rest array from the overflow,
        // and fill omitted optionals with nil (the `DefaultArg` prologue replaces
        // them with their default values).
        let provided = argc.min(fixed);
        if has_rest {
            let overflow_start = callee_idx + 1 + fixed;
            let rest: Vec<Value> = if argc > fixed {
                self.stack[overflow_start..callee_idx + 1 + argc].to_vec()
            } else {
                Vec::new()
            };
            self.stack.truncate(callee_idx + 1 + provided);
            for _ in provided..fixed {
                self.stack.push(Value::Nil);
            }
            let rest_arr = self.heap.alloc_array(rest);
            self.stack.push(Value::Obj(rest_arr));
        } else {
            for _ in argc..fixed {
                self.stack.push(Value::Nil);
            }
        }
        let module = if let Obj::Closure(c) = self.heap.get(closure) { c.module } else { 0 };
        self.frames.push(CallFrame {
            closure,
            proto,
            ip: 0,
            slot_base: callee_idx,
            module,
            provided_argc: provided,
        });
        self.maybe_collect(); // a call is a GC safe point
        Ok(())
    }

    /// Build a suspended generator from a generator-function call. Its initial
    /// context stack is `[closure, args…]`; the parameter layout and first frame
    /// are deferred to the first resume (DESIGN D29).
    fn make_generator(&mut self, closure: GcRef, argc: usize, callee_idx: usize) -> Result<(), Value> {
        let gen_stack: Vec<Value> = self.stack[callee_idx..callee_idx + 1 + argc].to_vec();
        self.stack.truncate(callee_idx);
        let mut ctx = ExecContext::new();
        ctx.stack = gen_stack;
        let r = self.heap.alloc(Obj::Generator(Generator { closure, state: GenState::Start, ctx }));
        self.push(Value::Obj(r));
        Ok(())
    }

    /// Resume a generator until it `yield`s (returns `Some(value)`) or finishes
    /// (returns `None`). Swaps the generator's saved execution context into the VM
    /// for the duration, keeping the caller's context GC-rooted (DESIGN D29).
    fn resume_generator(&mut self, gen_ref: GcRef) -> Result<Option<Value>, Value> {
        let (state, closure) = match self.heap.get(gen_ref) {
            Obj::Generator(g) => (g.state, g.closure),
            _ => return Err(self.throw(error_kind::TYPE, "next() expects a generator")),
        };
        match state {
            GenState::Done => return Ok(None),
            GenState::Running => {
                return Err(self.throw(error_kind::VALUE, "generator is already running"))
            }
            GenState::Start | GenState::Suspended => {}
        }
        // Take the generator's context and swap it in, saving the caller's.
        let gen_ctx = match self.heap.get_mut(gen_ref) {
            Obj::Generator(g) => {
                g.state = GenState::Running;
                std::mem::take(&mut g.ctx)
            }
            _ => unreachable!(),
        };
        let saved = ExecContext {
            stack: std::mem::replace(&mut self.stack, gen_ctx.stack),
            frames: std::mem::replace(&mut self.frames, gen_ctx.frames),
            handlers: std::mem::replace(&mut self.handlers, gen_ctx.handlers),
            open_upvalues: std::mem::replace(&mut self.open_upvalues, gen_ctx.open_upvalues),
        };
        self.saved_contexts.push(saved);

        // On the first resume the frame still has to be established.
        let run = if state == GenState::Start {
            let argc = self.stack.len() - 1;
            let proto = self.closure_proto(closure);
            match self.enter_frame(closure, proto, argc, 0) {
                Ok(()) => self.run_until(0),
                Err(e) => Err(e),
            }
        } else {
            self.run_until(0)
        };

        // Determine the outcome, then swap the caller's context back in.
        let yielded = self.pending_yield.take();
        let outcome = match run {
            Ok(()) if yielded.is_some() => Ok(Some(yielded.unwrap())),
            Ok(()) => Ok(None),  // the generator returned
            Err(e) => Err(e),
        };
        let saved = self.saved_contexts.pop().unwrap();
        let gen_ctx_back = ExecContext {
            stack: std::mem::replace(&mut self.stack, saved.stack),
            frames: std::mem::replace(&mut self.frames, saved.frames),
            handlers: std::mem::replace(&mut self.handlers, saved.handlers),
            open_upvalues: std::mem::replace(&mut self.open_upvalues, saved.open_upvalues),
        };
        let done = !matches!(outcome, Ok(Some(_)));
        if let Obj::Generator(g) = self.heap.get_mut(gen_ref) {
            g.ctx = gen_ctx_back;
            g.state = if done { GenState::Done } else { GenState::Suspended };
        }
        outcome
    }

    fn instantiate(&mut self, class: GcRef, argc: usize, callee_idx: usize) -> Result<(), Value> {
        let instance = self.heap.alloc(Obj::Instance(Instance { class, fields: FxHashMap::default() }));
        self.stack[callee_idx] = Value::Obj(instance); // slot 0 = the new instance (`this`)
        if let Some(init) = self.find_method(class, "init") {
            let proto = self.closure_proto(init);
            self.call_closure(init, proto, argc, callee_idx)
        } else if argc != 0 {
            let cname = if let Obj::Class(c) = self.heap.get(class) { c.name.clone() } else { String::new() };
            Err(self.throw(error_kind::ARITY, format!("'{cname}' takes no arguments (no init method), got {argc}")))
        } else {
            self.stack.truncate(callee_idx + 1); // leave the instance as the result
            Ok(())
        }
    }

    /// Call a Lumen callable from Rust and return its result, running any pushed
    /// frame to completion. Used by native higher-order functions and `str()`.
    pub fn call_and_run(&mut self, callee: Value, args: &[Value]) -> Result<Value, Value> {
        let base = self.frames.len();
        let callee_idx = self.stack.len();
        self.push(callee);
        for a in args {
            self.push(*a);
        }
        self.call_value(callee, args.len(), callee_idx)?;
        if self.frames.len() == base {
            Ok(self.pop()) // native completed synchronously
        } else {
            self.run_until(base)?;
            Ok(self.pop())
        }
    }

    // ---- iteration ---------------------------------------------------------

    fn op_iter_next(&mut self) -> Result<(), Value> {
        let iter_slot = self.read_byte() as usize;
        let idx_slot = self.read_byte() as usize;
        let exit = self.read_u16() as usize;
        let base = self.frames.last().unwrap().slot_base;
        let iter_val = self.stack[base + iter_slot];
        let r = match iter_val {
            Value::Obj(r) => r,
            _ => return Err(self.throw(error_kind::TYPE, "value is not iterable")),
        };
        // A generator iterates lazily: resume it for the next value (DESIGN D29).
        if matches!(self.heap.get(r), Obj::Generator(_)) {
            match self.resume_generator(r)? {
                Some(v) => self.push(v),
                None => self.frames.last_mut().unwrap().ip += exit,
            }
            return Ok(());
        }
        let idx = match self.stack[base + idx_slot] {
            Value::Int(n) => n,
            _ => unreachable!("for-in index is not an int"),
        };
        // Determine the next element (or that we are done) with a single
        // immutable borrow, copying out what we need.
        enum Step {
            Done,
            Value(Value),
            Char(char),
        }
        let step = match self.heap.get(r) {
            Obj::Array(a) => match a.get(idx as usize) {
                Some(v) => Step::Value(*v),
                None => Step::Done,
            },
            Obj::Map(m) => match m.nth_key(idx as usize) {
                Some(k) => Step::Value(k),
                None => Step::Done,
            },
            Obj::Str(s) => match s.chars().nth(idx as usize) {
                Some(c) => Step::Char(c),
                None => Step::Done,
            },
            _ => {
                return Err(self.throw(
                    error_kind::TYPE,
                    "value is not iterable (expected array, string, or map)",
                ))
            }
        };
        match step {
            Step::Done => {
                self.frames.last_mut().unwrap().ip += exit;
            }
            Step::Value(v) => {
                self.push(v);
                self.stack[base + idx_slot] = Value::Int(idx + 1);
            }
            Step::Char(c) => {
                let s = c.to_string();
                let sv = Value::Obj(self.heap.intern(&s));
                self.push(sv);
                self.stack[base + idx_slot] = Value::Int(idx + 1);
            }
        }
        Ok(())
    }

    /// Collect a value's elements for `..spread` / `ArrayExtend`.
    fn iterable_values(&mut self, v: Value) -> Result<Vec<Value>, Value> {
        match v {
            Value::Obj(r) => match self.heap.get(r) {
                Obj::Array(a) => Ok(a.clone()),
                Obj::Map(m) => Ok(m.keys()),
                Obj::Str(s) => {
                    let chars: Vec<String> = s.chars().map(|c| c.to_string()).collect();
                    Ok(chars.into_iter().map(|c| Value::Obj(self.heap.intern(&c))).collect())
                }
                _ => Err(self.throw(error_kind::TYPE, "value is not iterable for spread")),
            },
            _ => Err(self.throw(error_kind::TYPE, "value is not iterable for spread")),
        }
    }

    // ---- conversions -------------------------------------------------------

    fn materialize(&mut self, c: Constant) -> Value {
        match c {
            Constant::Int(n) => Value::Int(n),
            Constant::Float(f) => Value::Float(f),
            Constant::Str(s) => Value::Obj(self.heap.intern(&s)),
            Constant::Fn(_) => unreachable!("function constants are loaded via CLOSURE"),
        }
    }

    /// Normalize a value to a hashable map key (SPEC: int/float collapse;
    /// strings key by interned handle; other objects are unhashable).
    pub fn map_key(&mut self, v: Value) -> Result<MapKey, Value> {
        Ok(match v {
            Value::Nil => MapKey::Nil,
            Value::Bool(b) => MapKey::Bool(b),
            Value::Int(n) => MapKey::Int(n),
            Value::Float(f) => {
                if f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                    MapKey::Int(f as i64)
                } else {
                    MapKey::FloatBits(f.to_bits())
                }
            }
            Value::Obj(r) => match self.heap.get(r) {
                Obj::Str(_) => MapKey::Str(r),
                other => {
                    let t = other.type_name();
                    return Err(self.throw(error_kind::TYPE, format!("{t} is not a valid map key")));
                }
            },
        })
    }

    /// The SPEC type name of a value.
    pub fn type_name(&self, v: Value) -> &'static str {
        match v {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Obj(r) => self.heap.get(r).type_name(),
        }
    }

    /// Render a value to text. `quote` controls whether strings are shown with
    /// quotes (used inside collections; top-level `str()`/print uses `false`).
    pub fn to_display(&mut self, v: Value, quote: bool) -> Result<String, Value> {
        Ok(match v {
            Value::Nil => "nil".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => format_float(f),
            Value::Obj(r) => match self.heap.get(r) {
                Obj::Str(s) => {
                    if quote {
                        format!("\"{}\"", escape_string(s))
                    } else {
                        s.clone()
                    }
                }
                Obj::Array(a) => {
                    let items = a.clone();
                    let mut parts = Vec::with_capacity(items.len());
                    for it in items {
                        parts.push(self.to_display(it, true)?);
                    }
                    format!("[{}]", parts.join(", "))
                }
                Obj::Map(m) => {
                    let entries: Vec<(Value, Value)> = m.iter().collect();
                    let mut parts = Vec::with_capacity(entries.len());
                    for (k, val) in entries {
                        let ks = self.to_display(k, true)?;
                        let vs = self.to_display(val, true)?;
                        parts.push(format!("{ks}: {vs}"));
                    }
                    format!("{{{}}}", parts.join(", "))
                }
                Obj::Closure(c) => format!("<fn {}>", c.proto.name.as_deref().unwrap_or("anonymous")),
                Obj::Native(n) => format!("<fn {}>", n.name),
                Obj::Class(c) => format!("<class {}>", c.name),
                Obj::Bound(_) => "<fn bound method>".to_string(),
                Obj::Module(m) => format!("<module {}>", m.name),
                Obj::Error(e) => format!("{}: {}", e.kind, e.message),
                Obj::Upvalue(_) => "<upvalue>".to_string(),
                Obj::Instance(inst) => {
                    let class = inst.class;
                    let cname = if let Obj::Class(c) = self.heap.get(class) { c.name.clone() } else { "?".into() };
                    if let Some(method) = self.find_method(class, "str") {
                        let bound = Value::Obj(self.heap.alloc(Obj::Bound(BoundMethod { receiver: v, method })));
                        let result = self.call_and_run(bound, &[])?;
                        // Use the str() result directly (unquoted).
                        return self.to_display(result, false);
                    }
                    format!("<{cname} instance>")
                }
                Obj::Generator(_) => "<generator>".to_string(),
            },
        })
    }

    /// Write text to the VM's output sink (used by `print`/`println`).
    pub fn write_out(&mut self, s: &str) {
        let _ = self.out.write_all(s.as_bytes());
    }

    pub fn flush_out(&mut self) {
        let _ = self.out.flush();
    }

    /// Allocate/intern a string and return it as a value.
    pub fn new_string(&mut self, s: &str) -> Value {
        Value::Obj(self.heap.intern(s))
    }

    /// Allocate an array value.
    pub fn new_array(&mut self, items: Vec<Value>) -> Value {
        Value::Obj(self.heap.alloc_array(items))
    }

    // ---- modules -----------------------------------------------------------

    fn import_module(&mut self, path: &str) -> Result<Value, Value> {
        // 1. A built-in (native) module? Cache so repeated imports share one
        //    module object (and one set of native function objects).
        let native_key = format!("native:{path}");
        if let Some(&m) = self.modules.get(&native_key) {
            return Ok(Value::Obj(m));
        }
        if let Some(loader) = self.native_loader {
            if let Some(result) = loader(self, path) {
                if let Ok(Value::Obj(r)) = &result {
                    self.modules.insert(native_key, *r);
                }
                return result;
            }
        }
        // 2. Resolve to a file path and check the cache.
        let resolved = self.resolve_module_path(path);
        let key = resolved.to_string_lossy().to_string();
        if let Some(&m) = self.modules.get(&key) {
            return Ok(Value::Obj(m));
        }
        // 3. Read + compile.
        let src = match std::fs::read_to_string(&resolved) {
            Ok(s) => s,
            Err(_) => return Err(self.throw(error_kind::NAME, format!("cannot find module '{path}'"))),
        };
        let (program, errs) = crate::check_source(&src);
        if !errs.is_empty() {
            let first = errs[0].message.clone();
            return Err(self.throw(error_kind::VALUE, format!("module '{path}' has errors: {first}")));
        }
        let proto = match crate::compiler::compile(&program) {
            Ok(p) => p,
            Err(_) => return Err(self.throw(error_kind::VALUE, format!("module '{path}' failed to compile"))),
        };
        // 4. Pre-cache an empty module (supports cyclic imports), then run it
        //    with fresh globals and a base dir set to the module's directory.
        let module_name = resolved
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        let module_obj = self.heap.alloc(Obj::Module(Module {
            name: module_name,
            path: key.clone(),
            exports: FxHashMap::default(),
        }));
        self.modules.insert(key, module_obj);

        let module_idx = self.module_globals.len();
        self.module_globals.push(FxHashMap::default());
        let saved_base = self.base_dir.clone();
        if let Some(parent) = resolved.parent() {
            self.base_dir = parent.to_path_buf();
        }

        let run_result = self.run_module_body(proto.clone(), module_idx);
        let exports = collect_exports(&self.module_globals[module_idx], &proto.exports, &run_result);
        self.base_dir = saved_base;
        run_result?;

        if let Obj::Module(m) = self.heap.get_mut(module_obj) {
            m.exports = exports;
        }
        Ok(Value::Obj(module_obj))
    }

    fn run_module_body(&mut self, proto: Rc<FnProto>, module: usize) -> Result<(), Value> {
        let closure = self.heap.alloc_closure(Closure { proto, upvalues: Vec::new(), module });
        let base = self.frames.len();
        let callee_idx = self.stack.len();
        self.push(Value::Obj(closure));
        self.call_value(Value::Obj(closure), 0, callee_idx)?;
        self.run_until(base)?;
        self.pop(); // the module body returns nil
        Ok(())
    }

    fn resolve_module_path(&self, path: &str) -> PathBuf {
        let mut p = PathBuf::from(path);
        if p.extension().is_none() {
            p.set_extension("lum");
        }
        if p.is_absolute() {
            return p;
        }
        // Try the importer's directory first, then each search path; fall back to
        // the base-dir join (so the error names a sensible path).
        let base = self.base_dir.join(&p);
        if base.exists() {
            return base;
        }
        for dir in &self.search_paths {
            let candidate = dir.join(&p);
            if candidate.exists() {
                return candidate;
            }
        }
        base
    }

    // ---- diagnostics -------------------------------------------------------

    fn format_uncaught(&mut self, thrown: Value) -> String {
        let value_str = self.to_display(thrown, false).unwrap_or_else(|_| "<error>".to_string());
        let kind = match thrown {
            Value::Obj(r) => matches!(self.heap.get(r), Obj::Error(_)),
            _ => false,
        };
        let header = if kind {
            format!("Uncaught {value_str}")
        } else {
            format!("Uncaught exception: {value_str}")
        };
        let mut out = String::new();
        out.push_str(&header);
        out.push('\n');
        out.push_str("Stack trace (most recent call first):\n");
        for frame in self.frames.iter().rev() {
            let line = frame.proto.chunk.line_at(frame.ip.saturating_sub(1));
            out.push_str(&format!("  at {} (line {})\n", frame.proto.display_name(), line));
        }
        out
    }
}

/// Collect a module's `export`ed names from its globals after it ran (empty if
/// the run failed).
fn collect_exports(
    globals: &FxHashMap<String, Value>,
    names: &[String],
    run_result: &Result<(), Value>,
) -> FxHashMap<String, Value> {
    let mut exports = FxHashMap::default();
    if run_result.is_ok() {
        for name in names {
            if let Some(v) = globals.get(name) {
                exports.insert(name.clone(), *v);
            }
        }
    }
    exports
}

/// Borrow a string constant from a prototype's pool without cloning. Borrowing a
/// `&FnProto` (a disjoint field of the VM) lets the caller keep mutating other VM
/// fields — the basis of the clone-free global/property name reads.
fn const_str(proto: &FnProto, idx: usize) -> &str {
    match &proto.chunk.constants[idx] {
        Constant::Str(s) => s,
        _ => unreachable!("operand was not a string constant — compiler bug"),
    }
}

/// Symbol name of a binary opcode for error messages.
fn op_symbol(op: OpCode) -> &'static str {
    match op {
        OpCode::Add => "+",
        OpCode::Sub => "-",
        OpCode::Mul => "*",
        OpCode::Div => "/",
        OpCode::Rem => "%",
        OpCode::Lt => "<",
        OpCode::Le => "<=",
        OpCode::Gt => ">",
        OpCode::Ge => ">=",
        OpCode::BitAnd => "&",
        OpCode::BitOr => "|",
        OpCode::BitXor => "^",
        OpCode::Shl => "<<",
        OpCode::Shr => ">>",
        _ => "?",
    }
}
