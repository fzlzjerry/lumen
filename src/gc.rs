//! The heap: object storage, allocation, string interning, and a **generational
//! tracing garbage collector**.
//!
//! Objects live in a slot table (`Vec<Option<GcBox>>`); a [`GcRef`] is just an
//! index. Freed slots are recycled through a free list. This handle-based design
//! lets the collector mutate a cyclic object graph without any `unsafe` and
//! without reference counting — the price is one index indirection per access.
//!
//! The collector has two generations. New objects are allocated into the
//! **young** nursery. A **minor** collection traces and sweeps only the young
//! generation — seeded by the VM roots plus a **remembered set** of old objects
//! that point at young ones (maintained by the [`write_barrier`](Heap::write_barrier))
//! — and promotes survivors to **old**. A **major** collection traces and sweeps
//! everything (and so reclaims old cycles). This is the generational hypothesis
//! at work: most objects die young, so the common case (a minor collection)
//! costs O(nursery), never rescanning the promoted live set.

use crate::fxhash::FxHashMap;
use crate::object::{Closure, LumMap, Obj, Upvalue};
use crate::value::{GcRef, Value};

/// An object plus its GC bookkeeping bits.
pub(crate) struct GcBox {
    pub(crate) obj: Obj,
    /// Set during marking; cleared during sweep. Unmarked objects are freed.
    pub(crate) marked: bool,
    /// 0 = young (in the nursery), 1 = old (survived a collection). Minor
    /// collections only touch young objects; survivors are promoted to old.
    pub(crate) generation: u8,
    /// True while this (old) object is in the remembered set, so the write
    /// barrier doesn't add it twice.
    pub(crate) remembered: bool,
}

pub struct Heap {
    pub(crate) slots: Vec<Option<GcBox>>,
    free: Vec<u32>,
    /// Interned strings: content -> handle. Treated as a *weak* table by the
    /// collector (Phase 6 drops entries whose string was not marked).
    intern: FxHashMap<String, GcRef>,
    /// Bytes allocated in the young generation (the nursery) since the last
    /// minor collection.
    pub young_bytes: usize,
    /// Bytes held by promoted (old) objects.
    pub old_bytes: usize,
    /// Run a minor collection once `young_bytes` exceeds this (nursery size).
    next_minor: usize,
    /// Run a major collection once `old_bytes` exceeds this (grows after each).
    next_major: usize,
    /// Old objects that have been written to point at a young object — the roots
    /// (besides the VM's) a minor collection must scan.
    remembered: Vec<GcRef>,
    /// Minor and major collection counts (observable by tests).
    pub minor_collections: usize,
    pub major_collections: usize,
    /// When true, force a **major** collection before every instruction (shakes
    /// out missing roots). When `minor_stress` is true, force a **minor** one
    /// (shakes out missing write barriers).
    pub stress: bool,
    pub minor_stress: bool,
    /// The mark phase's worklist (gray set), reused across collections.
    gray: Vec<GcRef>,
}

/// Initial nursery size: collect the young generation once this many bytes have
/// been allocated since the last minor GC.
const NURSERY_SIZE: usize = 1 << 20; // 1 MiB
/// Initial old-generation threshold for triggering a major collection.
const INITIAL_NEXT_MAJOR: usize = 1 << 21; // 2 MiB

impl Default for Heap {
    fn default() -> Self {
        Heap::new()
    }
}

impl Heap {
    pub fn new() -> Self {
        Heap {
            slots: Vec::new(),
            free: Vec::new(),
            intern: FxHashMap::default(),
            young_bytes: 0,
            old_bytes: 0,
            next_minor: NURSERY_SIZE,
            next_major: INITIAL_NEXT_MAJOR,
            remembered: Vec::new(),
            minor_collections: 0,
            major_collections: 0,
            stress: false,
            minor_stress: false,
            gray: Vec::new(),
        }
    }

    /// Allocate an object into the young generation and return its handle. Does
    /// **not** itself collect; the VM collects at safe points.
    pub fn alloc(&mut self, obj: Obj) -> GcRef {
        self.young_bytes += obj_size(&obj);
        let boxed = GcBox {
            obj,
            marked: false,
            generation: 0,
            remembered: false,
        };
        if let Some(idx) = self.free.pop() {
            self.slots[idx as usize] = Some(boxed);
            GcRef(idx)
        } else {
            let idx = self.slots.len() as u32;
            self.slots.push(Some(boxed));
            GcRef(idx)
        }
    }

    /// Should the VM collect now, and is a *major* collection due?
    pub fn should_collect(&self) -> bool {
        self.stress
            || self.minor_stress
            || self.young_bytes > self.next_minor
            || self.old_bytes > self.next_major
    }

    /// Whether the next collection should be major (full) rather than minor.
    pub fn major_due(&self) -> bool {
        self.stress || self.old_bytes > self.next_major
    }

    /// Total collections (minor + major), for tests/metrics.
    pub fn collections(&self) -> usize {
        self.minor_collections + self.major_collections
    }

    // ---- write barrier -----------------------------------------------------

    /// Record that `container` was made to point at `value`. If `container` is
    /// old and `value` is a young object, `container` joins the remembered set so
    /// the next minor collection treats it as a root for `value`. This is the
    /// invariant that makes minor collections sound: every old→young edge is
    /// either remembered here or created young→young and promoted together.
    pub fn write_barrier(&mut self, container: GcRef, value: Value) {
        let child = match value {
            Value::Obj(r) => r,
            _ => return,
        };
        let container_old = self
            .slots
            .get(container.0 as usize)
            .and_then(|s| s.as_ref())
            .map(|b| b.generation == 1)
            .unwrap_or(false);
        if !container_old {
            return;
        }
        let child_young = self
            .slots
            .get(child.0 as usize)
            .and_then(|s| s.as_ref())
            .map(|b| b.generation == 0)
            .unwrap_or(false);
        if !child_young {
            return;
        }
        if let Some(b) = self.slots[container.0 as usize].as_mut() {
            if !b.remembered {
                b.remembered = true;
                self.remembered.push(container);
            }
        }
    }

    // ---- mark & sweep ------------------------------------------------------
    //
    // A tracing mark-and-sweep with two generations. A *major* collection traces
    // and sweeps everything (`young_only = false`). A *minor* collection traces
    // and sweeps only the young nursery (`young_only = true`), seeded by the VM
    // roots plus the remembered set; survivors are promoted to old. A missed root
    // or write barrier surfaces as the dangling-`GcRef` panic in `get` (the
    // stress modes turn that into a deterministic test failure).

    /// Mark a value's object reachable. `young_only` skips old objects (minor GC).
    pub fn mark_value(&mut self, v: Value, young_only: bool) {
        if let Value::Obj(r) = v {
            self.mark_ref(r, young_only);
        }
    }

    /// Mark an object reachable and enqueue it for tracing.
    pub fn mark_ref(&mut self, r: GcRef, young_only: bool) {
        if let Some(b) = self.slots.get_mut(r.0 as usize).and_then(|s| s.as_mut()) {
            if young_only && b.generation != 0 {
                return; // old objects are live during a minor collection
            }
            if !b.marked {
                b.marked = true;
                self.gray.push(r);
            }
        }
    }

    /// Seed a minor collection with the remembered set: scan each remembered old
    /// object's references and mark the young objects it points at.
    pub fn mark_remembered(&mut self) {
        let remembered = std::mem::take(&mut self.remembered);
        for &r in &remembered {
            let mut values = Vec::new();
            let mut refs = Vec::new();
            if let Some(b) = self.slots[r.0 as usize].as_ref() {
                outgoing_edges(&b.obj, &mut values, &mut refs);
            }
            for v in values {
                self.mark_value(v, true);
            }
            for rf in refs {
                self.mark_ref(rf, true);
            }
            if let Some(b) = self.slots[r.0 as usize].as_mut() {
                b.remembered = false;
            }
        }
    }

    /// Trace from the gray set until empty.
    pub fn trace_references(&mut self, young_only: bool) {
        while let Some(r) = self.gray.pop() {
            let mut values = Vec::new();
            let mut refs = Vec::new();
            if let Some(b) = self.slots[r.0 as usize].as_ref() {
                outgoing_edges(&b.obj, &mut values, &mut refs);
            }
            for v in values {
                self.mark_value(v, young_only);
            }
            for rf in refs {
                self.mark_ref(rf, young_only);
            }
        }
    }

    /// Minor sweep: free unmarked young objects; promote marked young objects to
    /// old. Old objects are untouched.
    pub fn sweep_minor(&mut self) {
        for i in 0..self.slots.len() {
            enum D {
                Promote(usize),
                Free,
                Skip,
            }
            let d = match self.slots[i].as_ref() {
                Some(b) if b.generation == 0 && b.marked => D::Promote(obj_size(&b.obj)),
                Some(b) if b.generation == 0 => D::Free,
                _ => D::Skip,
            };
            match d {
                D::Promote(sz) => {
                    let b = self.slots[i].as_mut().unwrap();
                    b.marked = false;
                    b.generation = 1;
                    self.young_bytes = self.young_bytes.saturating_sub(sz);
                    self.old_bytes += sz;
                }
                D::Free => self.free_slot(i),
                D::Skip => {}
            }
        }
        self.minor_collections += 1;
    }

    /// Major sweep: free every unmarked object; promote survivors to old. After
    /// a major collection the remembered set is rebuilt from scratch.
    pub fn sweep_major(&mut self) {
        for i in 0..self.slots.len() {
            enum D {
                PromoteYoung(usize),
                KeepOld,
                Free,
                Skip,
            }
            let d = match self.slots[i].as_ref() {
                Some(b) if b.marked && b.generation == 0 => D::PromoteYoung(obj_size(&b.obj)),
                Some(b) if b.marked => D::KeepOld,
                Some(_) => D::Free,
                None => D::Skip,
            };
            match d {
                D::PromoteYoung(sz) => {
                    let b = self.slots[i].as_mut().unwrap();
                    b.marked = false;
                    b.generation = 1;
                    b.remembered = false;
                    self.young_bytes = self.young_bytes.saturating_sub(sz);
                    self.old_bytes += sz;
                }
                D::KeepOld => {
                    let b = self.slots[i].as_mut().unwrap();
                    b.marked = false;
                    b.remembered = false;
                }
                D::Free => self.free_slot(i),
                D::Skip => {}
            }
        }
        self.remembered.clear();
        self.major_collections += 1;
        self.next_major = (self.old_bytes * 2).max(INITIAL_NEXT_MAJOR);
    }

    #[track_caller]
    pub fn get(&self, r: GcRef) -> &Obj {
        &self.slots[r.0 as usize]
            .as_ref()
            .expect("dangling GcRef (use-after-free) — GC bug")
            .obj
    }

    #[track_caller]
    pub fn get_mut(&mut self, r: GcRef) -> &mut Obj {
        &mut self.slots[r.0 as usize]
            .as_mut()
            .expect("dangling GcRef (use-after-free) — GC bug")
            .obj
    }

    /// Intern a string: equal contents share one heap object, so string equality
    /// and hashing can compare handles.
    pub fn intern(&mut self, s: &str) -> GcRef {
        if let Some(&r) = self.intern.get(s) {
            return r;
        }
        let r = self.alloc(Obj::Str(s.to_string()));
        self.intern.insert(s.to_string(), r);
        r
    }

    /// Convenience: allocate an array.
    pub fn alloc_array(&mut self, items: Vec<Value>) -> GcRef {
        self.alloc(Obj::Array(items))
    }

    /// Convenience: allocate a map.
    pub fn alloc_map(&mut self, map: LumMap) -> GcRef {
        self.alloc(Obj::Map(map))
    }

    /// Convenience: allocate a closure.
    pub fn alloc_closure(&mut self, closure: Closure) -> GcRef {
        self.alloc(Obj::Closure(closure))
    }

    /// Convenience: allocate an upvalue.
    pub fn alloc_upvalue(&mut self, uv: Upvalue) -> GcRef {
        self.alloc(Obj::Upvalue(uv))
    }

    /// Borrow a string object's text.
    pub fn str_of(&self, r: GcRef) -> Option<&str> {
        match self.get(r) {
            Obj::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Total number of live (allocated, non-free) slots — for tests/metrics.
    pub fn live_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Number of interned strings.
    pub fn intern_count(&self) -> usize {
        self.intern.len()
    }

    // --- internals shared with the Phase 6 collector ---

    /// Free the slot at `idx`, removing any intern-table entry for it and
    /// adjusting the generation's byte counter.
    pub(crate) fn free_slot(&mut self, idx: usize) {
        if let Some(boxed) = self.slots[idx].take() {
            let sz = obj_size(&boxed.obj);
            if boxed.generation == 0 {
                self.young_bytes = self.young_bytes.saturating_sub(sz);
            } else {
                self.old_bytes = self.old_bytes.saturating_sub(sz);
            }
            if let Obj::Str(s) = &boxed.obj {
                // Drop the weak intern entry if it still points here.
                if self.intern.get(s) == Some(&GcRef(idx as u32)) {
                    self.intern.remove(s);
                }
            }
            self.free.push(idx as u32);
        }
    }
}

/// Append an object's outgoing edges (referenced values and handles) to the
/// given vectors. Shared by the tracer and the remembered-set scan.
fn outgoing_edges(obj: &Obj, values: &mut Vec<Value>, refs: &mut Vec<GcRef>) {
    match obj {
        Obj::Array(a) => values.extend(a.iter().copied()),
        Obj::Map(m) => {
            for (k, v) in m.iter() {
                values.push(k);
                values.push(v);
            }
        }
        Obj::Closure(c) => refs.extend(c.upvalues.iter().copied()),
        Obj::Upvalue(Upvalue::Closed(v)) => values.push(*v),
        Obj::Upvalue(Upvalue::Open(_)) => {}
        Obj::Class(c) => {
            refs.extend(c.methods.values().copied());
            refs.extend(c.statics.values().copied());
            if let Some(s) = c.superclass {
                refs.push(s);
            }
        }
        Obj::Instance(i) => {
            refs.push(i.class);
            values.extend(i.fields.values().copied());
        }
        Obj::Bound(b) => {
            values.push(b.receiver);
            refs.push(b.method);
        }
        Obj::Module(m) => values.extend(m.exports.values().copied()),
        Obj::Generator(g) => {
            // A suspended generator's whole saved context is reachable through it.
            refs.push(g.closure);
            values.extend(g.ctx.stack.iter().copied());
            refs.extend(g.ctx.frames.iter().map(|f| f.closure));
            refs.extend(g.ctx.open_upvalues.iter().copied());
        }
        Obj::BoundNative(b) => refs.push(b.receiver),
        // Strings, natives, errors, and file handles hold no heap references.
        Obj::Str(_) | Obj::Native(_) | Obj::Error(_) | Obj::FileHandle(_) => {}
    }
}

/// A rough byte estimate for an object, used to pace collections. Exactness is
/// not required — only monotonic-ish growth tracking.
fn obj_size(obj: &Obj) -> usize {
    use std::mem::size_of;
    let base = 32;
    base + match obj {
        Obj::Str(s) => s.len(),
        Obj::Array(a) => a.capacity() * size_of::<Value>(),
        Obj::Map(m) => m.len() * (size_of::<Value>() * 2 + 16),
        Obj::Instance(i) => i.fields.len() * (size_of::<Value>() + 24),
        Obj::Class(c) => c.methods.len() * 24,
        Obj::Module(m) => m.exports.len() * (size_of::<Value>() + 24),
        Obj::Closure(c) => c.upvalues.len() * size_of::<GcRef>(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_get() {
        let mut h = Heap::new();
        let r = h.alloc(Obj::Array(vec![Value::Int(1), Value::Int(2)]));
        match h.get(r) {
            Obj::Array(a) => assert_eq!(a.len(), 2),
            _ => panic!(),
        }
    }

    #[test]
    fn interning_dedups() {
        let mut h = Heap::new();
        let a = h.intern("hello");
        let b = h.intern("hello");
        let c = h.intern("world");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(h.intern_count(), 2);
    }

    #[test]
    fn freed_slots_are_recycled() {
        let mut h = Heap::new();
        let r = h.alloc(Obj::Array(vec![]));
        let idx = r.0 as usize;
        h.free_slot(idx);
        let r2 = h.alloc(Obj::Array(vec![]));
        assert_eq!(r2.0 as usize, idx); // reused the freed slot
    }

    #[test]
    fn major_keeps_reachable_frees_garbage_and_promotes() {
        let mut h = Heap::new();
        let child = h.alloc(Obj::Array(vec![Value::Int(1)]));
        let keep = h.alloc(Obj::Array(vec![Value::Obj(child)]));
        let garbage = h.alloc(Obj::Array(vec![Value::Int(99)]));
        assert_eq!(h.live_count(), 3);

        // Root only `keep`; a major collection reaches `child`, frees `garbage`,
        // and promotes survivors to old.
        h.mark_ref(keep, false);
        h.trace_references(false);
        h.sweep_major();

        assert_eq!(h.live_count(), 2);
        assert!(h.slots[keep.0 as usize].is_some());
        assert!(h.slots[child.0 as usize].is_some());
        assert!(h.slots[garbage.0 as usize].is_none());
        assert_eq!(h.slots[keep.0 as usize].as_ref().unwrap().generation, 1);
    }

    #[test]
    fn minor_collects_nursery_and_write_barrier_keeps_old_to_young() {
        let mut h = Heap::new();
        // An old container.
        let container = h.alloc(Obj::Array(vec![]));
        h.mark_ref(container, false);
        h.trace_references(false);
        h.sweep_major(); // promote container to old
        assert_eq!(
            h.slots[container.0 as usize].as_ref().unwrap().generation,
            1
        );

        // A fresh young object the old container now points at.
        let young = h.alloc(Obj::Array(vec![Value::Int(7)]));
        if let Obj::Array(a) = h.get_mut(container) {
            a.push(Value::Obj(young));
        }
        h.write_barrier(container, Value::Obj(young)); // record the old->young edge

        // A minor collection seeded only by the remembered set must keep `young`.
        h.mark_remembered();
        h.trace_references(true);
        h.sweep_minor();
        assert!(
            h.slots[young.0 as usize].is_some(),
            "write barrier failed: young freed"
        );

        // Without the barrier, an unremembered young object is collected.
        let orphan = h.alloc(Obj::Array(vec![]));
        h.mark_remembered();
        h.trace_references(true);
        h.sweep_minor();
        assert!(
            h.slots[orphan.0 as usize].is_none(),
            "unrooted young should be collected"
        );
    }
}
