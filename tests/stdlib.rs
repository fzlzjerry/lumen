//! Standard-library integration tests: import each module and assert behavior.
//! Includes a GC-stress pass over the higher-order array functions to prove
//! their temporary-root discipline (DESIGN D18) is correct.

use lumen::vm::Vm;
use std::cell::RefCell;
use std::io::Write;
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

fn run_with(src: &str, stress: bool) -> String {
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty(), "front-end errors: {errs:?}");
    let proto = lumen::compiler::compile(&program).expect("compile ok");
    let buf = SharedBuf(Rc::new(RefCell::new(Vec::new())));
    let mut vm = Vm::with_output(Box::new(buf.clone()));
    lumen::stdlib::install(&mut vm);
    vm.set_stress_gc(stress);
    vm.interpret(proto).unwrap_or_else(|e| panic!("runtime error:\n{e}"));
    let out = String::from_utf8(buf.0.borrow().clone()).unwrap();
    out
}

fn out(src: &str) -> String {
    run_with(src, false)
}

#[test]
fn math_module() {
    assert_eq!(out("import \"math\" as m; println(m.sqrt(144));"), "12.0\n");
    assert_eq!(out("import \"math\" as m; println(m.pow(2, 10));"), "1024.0\n");
    assert_eq!(out("import \"math\" as m; println(m.floor(3.7));"), "3\n");
    assert_eq!(out("import \"math\" as m; println(m.ceil(3.2));"), "4\n");
    assert_eq!(out("import \"math\" as m; println(m.abs(-5));"), "5\n"); // preserves int
    assert_eq!(out("import \"math\" as m; println(m.gcd(48, 36));"), "12\n");
    assert_eq!(out("import \"math\" as m; println(m.max(3, 9));"), "9\n");
}

#[test]
fn selective_import() {
    assert_eq!(out("import \"math\".{sqrt, pow}; println(sqrt(81)); println(pow(3, 3));"), "9.0\n27.0\n");
}

#[test]
fn string_module() {
    assert_eq!(out("import \"string\" as s; println(s.upper(\"abc\"));"), "ABC\n");
    assert_eq!(out("import \"string\" as s; println(s.split(\"a,b,c\", \",\"));"), "[\"a\", \"b\", \"c\"]\n");
    assert_eq!(out("import \"string\" as s; println(s.join([\"a\",\"b\"], \"-\"));"), "a-b\n");
    assert_eq!(out("import \"string\" as s; println(s.replace(\"a.b.c\", \".\", \"/\"));"), "a/b/c\n");
    assert_eq!(out("import \"string\" as s; println(s.pad_left(\"7\", 3));"), "  7\n");
    assert_eq!(out("import \"string\" as s; println(s.repeat(\"ab\", 3));"), "ababab\n");
    assert_eq!(out("import \"string\" as s; println(s.index_of(\"hello\", \"ll\"));"), "2\n");
    assert_eq!(out("import \"string\" as s; println(s.reverse(\"abc\"));"), "cba\n");
}

#[test]
fn array_module() {
    assert_eq!(out("import \"array\" as a; println(a.sum([1,2,3,4]));"), "10\n");
    assert_eq!(out("import \"array\" as a; println(a.map([1,2,3], fn(x) { return x*x; }));"), "[1, 4, 9]\n");
    assert_eq!(out("import \"array\" as a; println(a.filter([1,2,3,4], fn(x) { return x % 2 == 0; }));"), "[2, 4]\n");
    assert_eq!(out("import \"array\" as a; println(a.reduce([1,2,3,4], fn(acc, x) { return acc + x; }, 0));"), "10\n");
    assert_eq!(out("import \"array\" as a; println(a.sort([3,1,2]));"), "[1, 2, 3]\n");
    assert_eq!(
        out("import \"array\" as a; println(a.sort([3,1,2], fn(x, y) { return y - x; }));"),
        "[3, 2, 1]\n"
    );
    assert_eq!(out("import \"array\" as a; println(a.reverse([1,2,3]));"), "[3, 2, 1]\n");
    assert_eq!(out("import \"array\" as a; println(a.flatten([[1,2],[3],[4,5]]));"), "[1, 2, 3, 4, 5]\n");
}

#[test]
fn map_module() {
    let src = "import \"map\" as mp; let m = {a: 1}; mp.set(m, \"b\", 2);
               println(mp.get(m, \"a\")); println(mp.get(m, \"z\", -1)); println(mp.len(m));";
    assert_eq!(out(src), "1\n-1\n2\n");
}

#[test]
fn json_roundtrip() {
    let src = r#"import "json" as j;
                 let data = j.parse("{\"n\": 42, \"xs\": [1, 2, 3], \"ok\": true, \"e\": null}");
                 println(data["n"]); println(data["xs"]); println(data["ok"]); println(data["e"]);
                 println(j.stringify([1, 2, {"k": "v"}]));"#;
    assert_eq!(out(src), "42\n[1, 2, 3]\ntrue\nnil\n[1,2,{\"k\":\"v\"}]\n");
}

#[test]
fn random_is_deterministic_after_seed() {
    let prog = "import \"random\" as r; r.seed(42);
                for let i = 0; i < 3; i = i + 1 { print(r.randint(1, 100)); print(\" \"); } println(\"\");";
    // Two runs with the same seed produce the same sequence.
    let a = out(prog);
    let b = out(prog);
    assert_eq!(a, b);
    // randint stays in range.
    for tok in a.trim().split(' ') {
        let n: i64 = tok.parse().unwrap();
        assert!((1..=100).contains(&n));
    }
}

#[test]
fn self_hosted_seq_module() {
    assert_eq!(out("import \"seq\" as q; println(q.take([1,2,3,4], 2));"), "[1, 2]\n");
    assert_eq!(out("import \"seq\" as q; println(q.enumerate([\"a\",\"b\"]));"), "[[0, \"a\"], [1, \"b\"]]\n");
    assert_eq!(
        out("import \"seq\" as q; println(q.all([2,4,6], fn(x) { return x % 2 == 0; }));"),
        "true\n"
    );
}

#[test]
fn module_functions_see_their_own_module_globals() {
    // A module's function must resolve its module's imports even when called
    // from another module — the per-module-globals fix.
    let src = "import \"seq\" as q;
               let doubled = q.flat_map([1, 2], fn(x) { return [x, x]; });
               println(doubled);";
    assert_eq!(out(src), "[1, 1, 2, 2]\n");
}

#[test]
fn higher_order_natives_are_gc_safe_under_stress() {
    // map/filter/reduce/sort all re-enter the VM and accumulate heap results;
    // running under stress GC proves their temp-root rooting keeps results live.
    let src = "import \"array\" as a;
               let xs = [];
               for let i = 0; i < 40; i = i + 1 { push(xs, i); }
               let squared = a.map(xs, fn(x) { return [x, x * x]; });
               let evens = a.filter(xs, fn(x) { return x % 2 == 0; });
               let total = a.reduce(xs, fn(acc, x) { return acc + x; }, 0);
               let sorted = a.sort(xs, fn(p, q) { return q - p; });
               println(squared[5][1]);
               println(len(evens));
               println(total);
               println(sorted[0]);";
    assert_eq!(run_with(src, true), "25\n20\n780\n39\n");
}

#[test]
fn self_hosted_set_module() {
    // The `set` module: a hash set written in Lumen (dedup, set algebra).
    let src = r#"import "set";
                 let a = set.of([1, 2, 3, 2, 1]);
                 let b = set.of([2, 3, 4]);
                 println(a.size());
                 println(a.has(2));
                 println(a.has(9));
                 println(a.union(b).size());
                 println(a.intersect(b).values());
                 println(a.difference(b).values());
                 println(a.is_subset(set.of([1, 2, 3, 4])));"#;
    assert_eq!(out(src), "3\ntrue\nfalse\n4\n[2, 3]\n[1]\ntrue\n");
}

#[test]
fn self_hosted_functional_module() {
    // The `functional` module: composition, currying, memoization via closures.
    let src = r#"import "functional" as fp;
                 let inc = fn(x) { return x + 1; };
                 let dbl = fn(x) { return x * 2; };
                 println(fp.compose(inc, dbl)(10));
                 println(fp.pipe(inc, dbl, inc)(10));
                 let add = fn(x, y) { return x + y; };
                 println(fp.curry2(add)(3)(4));
                 println(fp.flip(fn(a, b) { return a - b; })(3, 10));
                 let calls = [0];
                 let memo = fp.memoize(fn(x) { calls[0] = calls[0] + 1; return x * x; });
                 println(memo(6));
                 println(memo(6));
                 println(calls[0]);
                 println(fp.iterate(dbl, 1, 5));"#;
    assert_eq!(out(src), "21\n23\n7\n7\n36\n36\n1\n32\n");
}

#[test]
fn self_hosted_testing_module() {
    // The `testing` module: a Suite with structural equality (deep_eq).
    let src = r#"import "testing" as t;
                 println(t.deep_eq([1, [2, 3]], [1, [2, 3]]));
                 println(t.deep_eq([1, 2], [1, 3]));
                 let s = t.Suite("demo");
                 s.eq("arith", 1 + 1, 2);
                 s.eq("arrays", [1, 2], [1, 2]);
                 s.truthy("truthy", 5);
                 s.eq("fails", 1, 2);
                 println(s.report());"#;
    assert_eq!(out(src), "true\nfalse\ndemo: 3/4 passed\n  FAIL: fails: expected 2, got 1\nfalse\n");
}
