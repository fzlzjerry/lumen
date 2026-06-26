//! Garbage-collector stress and correctness tests.
//!
//! The headline test runs real programs with **stress GC** on (a full collection
//! before every instruction). If any GC root is missing, a live object gets
//! swept and the next access trips the dangling-`GcRef` panic — so these tests
//! turn "missed a root" into a hard, deterministic failure. The pressure tests
//! confirm collection actually happens and memory is reclaimed (the live set
//! stays bounded while a loop allocates megabytes of garbage).

use lumen::vm::Vm;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

#[derive(Clone)]
struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Outcome {
    output: String,
    collections: usize,
    live: usize,
    interned: usize,
}

fn run_with(src: &str, stress: bool) -> Outcome {
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty(), "front-end errors: {errs:?}");
    let proto = lumen::compiler::compile(&program).expect("compile ok");
    let buf = SharedBuf(Rc::new(RefCell::new(Vec::new())));
    let mut vm = Vm::with_output(Box::new(buf.clone()));
    lumen::stdlib::install(&mut vm);
    vm.set_stress_gc(stress);
    vm.interpret(proto).expect("no uncaught error");
    let output = String::from_utf8(buf.0.borrow().clone()).unwrap();
    // Force a final full collection so `live`/`interned` reflect truly-reachable
    // objects, not whatever happens to still be in the young nursery.
    vm.collect_major();
    Outcome {
        output,
        collections: vm.heap.collections(),
        live: vm.heap.live_count(),
        interned: vm.heap.intern_count(),
    }
}

/// Run a battery of feature-exercising programs with stress GC on. Correct
/// output here means every live object stayed rooted through aggressive sweeps.
#[test]
fn correctness_under_stress() {
    let cases: &[(&str, &str)] = &[
        // closures keeping captured state alive across collections
        (
            "fn mk() { let n = 0; return fn() { n = n + 1; return n; }; }
             let c = mk(); print(c()); print(c()); println(c());",
            "123\n",
        ),
        // arrays of arrays, then index in
        (
            "let g = []; for let i = 0; i < 20; i = i + 1 { push(g, [i, i*i]); }
             println(g[7][1]);",
            "49\n",
        ),
        // maps surviving and being read back
        (
            "let m = {}; for let i = 0; i < 30; i = i + 1 { m[\"k${i}\"] = i; }
             println(m[\"k15\"] + len(keys(m)));",
            "45\n",
        ),
        // class instances with fields + methods
        (
            "class Node { init(v) { this.v = v; this.next = nil; } }
             let head = nil;
             for let i = 0; i < 50; i = i + 1 { let n = Node(i); n.next = head; head = n; }
             let sum = 0; let cur = head; while cur != nil { sum = sum + cur.v; cur = cur.next; }
             println(sum);",
            "1225\n",
        ),
        // string building (interning churn) + interpolation
        (
            "let s = \"\"; for let i = 0; i < 10; i = i + 1 { s = s + \"${i},\"; } println(s);",
            "0,1,2,3,4,5,6,7,8,9,\n",
        ),
        // pattern matching allocating bound sub-arrays
        (
            "fn h(a) { return match a { [first, ..rest] => first + len(rest), _ => 0 }; }
             let t = 0; for let i = 0; i < 20; i = i + 1 { t = t + h([i, 1, 2, 3]); } println(t);",
            "250\n",
        ),
        // exceptions unwinding across allocations
        (
            "let caught = 0;
             for let i = 0; i < 25; i = i + 1 {
                 try { let tmp = [i, i, i]; throw tmp[0]; } catch (e) { caught = caught + e; }
             }
             println(caught);",
            "300\n",
        ),
    ];
    for (src, expected) in cases {
        let o = run_with(src, true);
        assert_eq!(&o.output, expected, "wrong output under stress GC for:\n{src}");
        assert!(o.collections > 0, "stress GC should have collected for:\n{src}");
    }
}

/// A long allocation loop must trigger collections and keep the live set
/// bounded (memory is actually reclaimed, no leak).
#[test]
fn allocation_pressure_is_bounded() {
    // Each iteration allocates a fresh array that immediately becomes garbage.
    let src = "let acc = 0;
               for let i = 0; i < 200000; i = i + 1 {
                   let tmp = [i, i + 1, i + 2];
                   acc = acc + tmp[1];
               }
               println(acc);";
    let o = run_with(src, false);
    assert_eq!(o.output, "20000100000\n");
    assert!(o.collections > 5, "expected several collections, got {}", o.collections);
    // 200k arrays were allocated; if they were leaking, live would be ~200k+.
    assert!(o.live < 5000, "live set not bounded: {} live objects", o.live);
}

/// Unique garbage strings must be reclaimed from the intern table (it is weak).
#[test]
fn interned_garbage_strings_are_reclaimed() {
    let src = "for let i = 0; i < 100000; i = i + 1 { let s = \"unique-${i}-string\"; }
               println(\"done\");";
    let o = run_with(src, false);
    assert_eq!(o.output, "done\n");
    assert!(o.collections > 0);
    // 100k distinct strings were interned and dropped; the table must not retain
    // them all.
    assert!(o.interned < 5000, "intern table not pruned: {} entries", o.interned);
}

/// A program that keeps growing a reachable structure should *not* have its data
/// collected (the collector must not be over-eager).
#[test]
fn reachable_data_survives() {
    let src = "let keep = [];
               for let i = 0; i < 1000; i = i + 1 { push(keep, i * 2); }
               let sum = 0; for x in keep { sum = sum + x; } println(sum);";
    let o = run_with(src, true);
    assert_eq!(o.output, "999000\n");
    // The 1000-element array and its contents are reachable the whole time.
    assert!(o.live >= 1, "reachable array was collected");
}
