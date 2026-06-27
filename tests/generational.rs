//! Generational GC tests. The headline `write_barriers_under_minor_stress` runs
//! programs that make *old* objects point at freshly-allocated *young* objects
//! through every mutation path, with a minor collection forced before every
//! instruction. A missed write barrier leaves an old→young edge unrecorded, so
//! the minor GC frees the still-live young object and the next access trips the
//! dangling-`GcRef` panic — turning any gap into a hard failure.

use lumen::vm::Vm;
use std::cell::RefCell;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;

#[derive(Clone)]
struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn run_minor_stress(src: &str) -> String {
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty(), "front-end errors: {errs:?}");
    let proto = lumen::compiler::compile(&program).expect("compile");
    let buf = SharedBuf(Rc::new(RefCell::new(Vec::new())));
    let mut vm = Vm::with_output(Box::new(buf.clone()));
    lumen::stdlib::install(&mut vm);
    vm.set_minor_stress_gc(true); // a minor collection before every instruction
    vm.set_base_dir(Path::new("examples").to_path_buf());
    vm.interpret(proto)
        .unwrap_or_else(|e| panic!("runtime error:\n{e}"));
    let out = String::from_utf8(buf.0.borrow().clone()).unwrap();
    out
}

#[test]
fn write_barriers_under_minor_stress() {
    let cases: &[(&str, &str)] = &[
        // push into a long-lived (soon-old) array — old->young via ArrayPush
        (
            "let acc = []; for let i = 0; i < 100; i = i + 1 { push(acc, [i, i * i]); }
             let s = 0; for p in acc { s = s + p[1]; } println(s);",
            "328350\n",
        ),
        // index-assign into a long-lived array — old->young via IndexSet
        (
            "let a = [0, 0, 0]; for let i = 0; i < 100; i = i + 1 { a[i % 3] = [i]; }
             println(a[0][0] + a[1][0] + a[2][0]);",
            "294\n",
        ),
        // insert into a long-lived map — old->young via MapInsert
        (
            "let m = {}; for let i = 0; i < 100; i = i + 1 { m[\"k${i % 5}\"] = [i]; }
             println(len(keys(m)));",
            "5\n",
        ),
        // instance fields forming a linked list — old->young via SetProp
        (
            "class N { init(v) { this.v = v; this.next = nil; } }
             let head = nil;
             for let i = 0; i < 100; i = i + 1 { let n = N([i]); n.next = head; head = n; }
             let s = 0; let c = head; while c != nil { s = s + c.v[0]; c = c.next; } println(s);",
            "4950\n",
        ),
        // closures capturing fresh values — old->young via CLOSE_UPVALUE
        (
            "let fns = [];
             for let i = 0; i < 50; i = i + 1 { let cap = [i]; push(fns, fn() { return cap[0]; }); }
             let s = 0; for f in fns { s = s + f(); } println(s);",
            "1225\n",
        ),
        // map.set native — old->young via the native write barrier
        (
            "import \"map\" as mp; let m = {}; for let i = 0; i < 40; i = i + 1 { mp.set(m, i, [i]); }
             println(mp.len(m));",
            "40\n",
        ),
    ];
    for (src, expected) in cases {
        assert_eq!(
            &run_minor_stress(src),
            expected,
            "wrong output (or freed live object) for:\n{src}"
        );
    }
}

/// All example programs must run correctly under minor-GC stress, too.
#[test]
fn examples_correct_under_minor_stress() {
    let mut paths: Vec<_> = std::fs::read_dir("examples")
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("lum")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.chars().next().unwrap().is_ascii_digit())
                    .unwrap_or(false)
        })
        .collect();
    paths.sort();
    for path in paths {
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let expected = std::fs::read_to_string(format!("tests/expected/{stem}.txt")).unwrap();
        let src = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            run_minor_stress(&src),
            expected,
            "minor-stress drift for {}",
            path.display()
        );
    }
}

/// The generational machinery actually engages: minor collections run, objects
/// get promoted, and a major collection reclaims old cycles.
#[test]
fn minor_and_major_collections_engage() {
    let src = "let acc = 0;
               for let i = 0; i < 500000; i = i + 1 { let tmp = [i, i + 1]; acc = acc + tmp[0]; }
               println(acc);";
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty());
    let proto = lumen::compiler::compile(&program).unwrap();
    let mut vm = Vm::with_output(Box::new(std::io::sink()));
    lumen::stdlib::install(&mut vm);
    vm.interpret(proto).unwrap();
    // Lots of nursery garbage => many minor collections.
    assert!(
        vm.heap.minor_collections > 5,
        "expected minor GCs, got {}",
        vm.heap.minor_collections
    );
    // Force a major and confirm the live set is tiny (no leak through promotion).
    vm.collect_major();
    assert!(vm.heap.major_collections >= 1);
    assert!(
        vm.heap.live_count() < 5000,
        "live set not bounded: {}",
        vm.heap.live_count()
    );
}
