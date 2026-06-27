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
    vm.interpret(proto)
        .unwrap_or_else(|e| panic!("runtime error:\n{e}"));
    let out = String::from_utf8(buf.0.borrow().clone()).unwrap();
    out
}

fn out(src: &str) -> String {
    run_with(src, false)
}

#[test]
fn math_module() {
    assert_eq!(out("import \"math\" as m; println(m.sqrt(144));"), "12.0\n");
    assert_eq!(
        out("import \"math\" as m; println(m.pow(2, 10));"),
        "1024.0\n"
    );
    assert_eq!(out("import \"math\" as m; println(m.floor(3.7));"), "3\n");
    assert_eq!(out("import \"math\" as m; println(m.ceil(3.2));"), "4\n");
    assert_eq!(out("import \"math\" as m; println(m.abs(-5));"), "5\n"); // preserves int
    assert_eq!(out("import \"math\" as m; println(m.gcd(48, 36));"), "12\n");
    assert_eq!(out("import \"math\" as m; println(m.max(3, 9));"), "9\n");
}

#[test]
fn selective_import() {
    assert_eq!(
        out("import \"math\".{sqrt, pow}; println(sqrt(81)); println(pow(3, 3));"),
        "9.0\n27.0\n"
    );
}

#[test]
fn string_module() {
    assert_eq!(
        out("import \"string\" as s; println(s.upper(\"abc\"));"),
        "ABC\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.split(\"a,b,c\", \",\"));"),
        "[\"a\", \"b\", \"c\"]\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.join([\"a\",\"b\"], \"-\"));"),
        "a-b\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.replace(\"a.b.c\", \".\", \"/\"));"),
        "a/b/c\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.pad_left(\"7\", 3));"),
        "  7\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.repeat(\"ab\", 3));"),
        "ababab\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.index_of(\"hello\", \"ll\"));"),
        "2\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.reverse(\"abc\"));"),
        "cba\n"
    );
}

#[test]
fn string_predicates() {
    // Character-class predicates: non-empty AND every character matches.
    assert_eq!(
        out("import \"string\" as s; println(s.is_digit(\"123\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.is_digit(\"\"));"),
        "false\n"
    ); // empty is always false
    assert_eq!(
        out("import \"string\" as s; println(s.is_digit(\"12a\"));"),
        "false\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.is_alpha(\"abcÉ\"));"),
        "true\n"
    ); // Unicode-aware
    assert_eq!(
        out("import \"string\" as s; println(s.is_alnum(\"ab12\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.is_space(\" \\t\\n\"));"),
        "true\n"
    );
    // is_upper/is_lower: >=1 cased char and none of the opposite case.
    assert_eq!(
        out("import \"string\" as s; println(s.is_upper(\"ABC1\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.is_upper(\"Abc\"));"),
        "false\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.is_upper(\"123\"));"),
        "false\n"
    ); // no cased char
    assert_eq!(
        out("import \"string\" as s; println(s.is_lower(\"abc\"));"),
        "true\n"
    );
    // capitalize: first upper, rest lower.
    assert_eq!(
        out("import \"string\" as s; println(s.capitalize(\"hELLO\"));"),
        "Hello\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.capitalize(\"\"));"),
        "\n"
    );
    // count: non-overlapping; empty needle throws ValueError.
    assert_eq!(
        out("import \"string\" as s; println(s.count(\"aaaa\", \"aa\"));"),
        "2\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.count(\"abcabc\", \"x\"));"),
        "0\n"
    );
    assert_eq!(
        out("import \"string\" as s; try { s.count(\"x\", \"\"); } catch (e) { println(e.kind); }"),
        "ValueError\n"
    );
    // lines: like str::lines (no trailing empty element; "" -> []).
    assert_eq!(
        out("import \"string\" as s; println(s.lines(\"a\\nb\\n\"));"),
        "[\"a\", \"b\"]\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.lines(\"\"));"),
        "[]\n"
    );
}

#[test]
fn array_module() {
    assert_eq!(
        out("import \"array\" as a; println(a.sum([1,2,3,4]));"),
        "10\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.map([1,2,3], fn(x) { return x*x; }));"),
        "[1, 4, 9]\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.filter([1,2,3,4], fn(x) { return x % 2 == 0; }));"),
        "[2, 4]\n"
    );
    assert_eq!(out("import \"array\" as a; println(a.reduce([1,2,3,4], fn(acc, x) { return acc + x; }, 0));"), "10\n");
    assert_eq!(
        out("import \"array\" as a; println(a.sort([3,1,2]));"),
        "[1, 2, 3]\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.sort([3,1,2], fn(x, y) { return y - x; }));"),
        "[3, 2, 1]\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.reverse([1,2,3]));"),
        "[3, 2, 1]\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.flatten([[1,2],[3],[4,5]]));"),
        "[1, 2, 3, 4, 5]\n"
    );
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
fn random_distributions() {
    // Degenerate uniform plus range/membership properties that hold for any draw,
    // and the error cases for randrange/sample.
    let src = "import \"random\" as r; import \"math\" as m; r.seed(7);
               println(r.uniform(2.0, 2.0));
               let u = r.uniform(5.0, 6.0); println(u >= 5.0 && u < 6.0);
               let x = r.randrange(0, 5); println(x >= 0 && x < 5);
               try { r.randrange(5, 5); } catch (e) { println(e.kind); }
               let s = r.sample([1, 2, 3, 4], 2); println(len(s)); println(s[0] != s[1]);
               try { r.sample([1, 2, 3], -1); } catch (e) { println(e.kind); }
               try { r.sample([1, 2, 3], 99); } catch (e) { println(e.kind); }
               println(m.is_finite(r.gauss(0.0, 1.0)));";
    assert_eq!(
        out(src),
        "2.0\ntrue\ntrue\nValueError\n2\ntrue\nValueError\nValueError\ntrue\n"
    );

    // Deterministic after seed: same seed -> same uniform sequence.
    let prog = "import \"random\" as r; r.seed(99);
                for let i = 0; i < 3; i = i + 1 { print(r.uniform(0.0, 1.0)); print(\" \"); } println(\"\");";
    assert_eq!(out(prog), out(prog));
}

#[test]
fn self_hosted_seq_module() {
    assert_eq!(
        out("import \"seq\" as q; println(q.take([1,2,3,4], 2));"),
        "[1, 2]\n"
    );
    assert_eq!(
        out("import \"seq\" as q; println(q.enumerate([\"a\",\"b\"]));"),
        "[[0, \"a\"], [1, \"b\"]]\n"
    );
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
                 println(a.intersection(b).values());
                 println(a.difference(b).values());
                 println(a.symmetric_difference(b).values());
                 println(a.is_subset(set.of([1, 2, 3, 4])));
                 println(a.is_superset(set.of([2, 3])));
                 println(a.is_superset(b));"#;
    assert_eq!(
        out(src),
        "3\ntrue\nfalse\n4\n[2, 3]\n[2, 3]\n[1]\n[1, 4]\ntrue\ntrue\nfalse\n"
    );
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
    assert_eq!(
        out(src),
        "true\nfalse\ndemo: 3/4 passed\n  FAIL: fails: expected 2, got 1\nfalse\n"
    );
}

#[test]
fn array_find_family() {
    assert_eq!(
        out("import \"array\" as a; println(a.find([1,2,3,4], fn(x) { return x > 2; }));"),
        "3\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.find([1,2], fn(x) { return x > 9; }));"),
        "nil\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.find_index([1,2,3,4], fn(x) { return x > 2; }));"),
        "2\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.find_index([1,2], fn(x) { return x > 9; }));"),
        "-1\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.any([1,2,3], fn(x) { return x == 2; }));"),
        "true\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.any([1,2,3], fn(x) { return x == 9; }));"),
        "false\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.all([2,4,6], fn(x) { return x % 2 == 0; }));"),
        "true\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.all([2,3], fn(x) { return x % 2 == 0; }));"),
        "false\n"
    );
}

#[test]
fn array_unique_and_zip() {
    assert_eq!(
        out("import \"array\" as a; println(a.unique([1,2,2,3,1,3]));"),
        "[1, 2, 3]\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.unique([\"a\",\"b\",\"a\"]));"),
        "[\"a\", \"b\"]\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.zip([1,2,3], [\"a\",\"b\"]));"),
        "[[1, \"a\"], [2, \"b\"]]\n"
    );
}

#[test]
fn new_higher_order_natives_are_gc_safe_under_stress() {
    // zip allocates pairs; map.map/map.filter build new maps across re-entrant
    // callbacks. Running under stress GC proves their temp-root + write-barrier
    // discipline keeps the partial results (and freshly returned values) live.
    let src = "import \"array\" as a; import \"map\" as mp;
               let xs = []; for let i = 0; i < 30; i = i + 1 { push(xs, i); }
               let pairs = a.zip(xs, a.map(xs, fn(x) { return x * x; }));
               println(len(pairs)); println(pairs[5][0]); println(pairs[5][1]);
               println(a.find(xs, fn(x) { return x == 17; }));
               println(a.any(xs, fn(x) { return x == 29; }));
               println(a.all(xs, fn(x) { return x >= 0; }));
               let m = {}; for let i = 0; i < 20; i = i + 1 { mp.set(m, str(i), i); }
               let doubled = mp.map(m, fn(k, v) { return [v, v]; });
               let evens = mp.filter(m, fn(k, v) { return v % 2 == 0; });
               println(mp.len(doubled)); println(mp.len(evens));";
    assert_eq!(run_with(src, true), "30\n5\n25\n17\ntrue\ntrue\n20\n10\n");
}

#[test]
fn math_extras() {
    assert_eq!(out("import \"math\" as m; println(m.lcm(4, 6));"), "12\n");
    assert_eq!(out("import \"math\" as m; println(m.lcm(0, 5));"), "0\n");
    assert_eq!(
        out("import \"math\" as m; println(m.is_nan(m.nan));"),
        "true\n"
    );
    assert_eq!(
        out("import \"math\" as m; println(m.is_nan(1.0));"),
        "false\n"
    );
    assert_eq!(
        out("import \"math\" as m; println(m.is_finite(1.0));"),
        "true\n"
    );
    assert_eq!(
        out("import \"math\" as m; println(m.is_finite(m.inf));"),
        "false\n"
    );
    assert_eq!(
        out("import \"math\" as m; println(m.round(m.degrees(m.pi)));"),
        "180\n"
    );
    assert_eq!(
        out("import \"math\" as m; println(m.round(m.degrees(m.radians(90))));"),
        "90\n"
    );
}

#[test]
fn math_hyperbolic_clamp_factorial() {
    // Hyperbolics return floats.
    assert_eq!(out("import \"math\" as m; println(m.sinh(0));"), "0.0\n");
    assert_eq!(out("import \"math\" as m; println(m.cosh(0));"), "1.0\n");
    assert_eq!(out("import \"math\" as m; println(m.tanh(0));"), "0.0\n");
    assert_eq!(
        out("import \"math\" as m; println(m.round(m.asinh(m.sinh(2))));"),
        "2\n"
    );
    // clamp preserves the operand's type (int-in -> int-out) and errors on lo > hi.
    assert_eq!(
        out("import \"math\" as m; println(m.clamp(5, 1, 10));"),
        "5\n"
    );
    assert_eq!(
        out("import \"math\" as m; println(m.clamp(15, 1, 10));"),
        "10\n"
    );
    assert_eq!(
        out("import \"math\" as m; println(m.clamp(0, 1, 10));"),
        "1\n"
    );
    assert_eq!(
        out("import \"math\" as m; try { m.clamp(1, 10, 1); } catch (e) { println(e.kind); }"),
        "ValueError\n"
    );
    // factorial: checked; negative and overflow both throw ValueError.
    assert_eq!(
        out("import \"math\" as m; println(m.factorial(5));"),
        "120\n"
    );
    assert_eq!(out("import \"math\" as m; println(m.factorial(0));"), "1\n");
    assert_eq!(
        out("import \"math\" as m; println(m.factorial(20));"),
        "2432902008176640000\n"
    );
    assert_eq!(
        out("import \"math\" as m; try { m.factorial(-1); } catch (e) { println(e.kind); }"),
        "ValueError\n"
    );
    assert_eq!(
        out("import \"math\" as m; try { m.factorial(21); } catch (e) { println(e.kind); }"),
        "ValueError\n"
    );
    // log gains an optional base: log(x, base) = ln(x) / ln(base); 1-arg unchanged.
    // (Rounded to avoid float-formatting fragility on the exact division result.)
    assert_eq!(
        out("import \"math\" as m; println(m.round(m.log(8, 2)));"),
        "3\n"
    );
    assert_eq!(
        out("import \"math\" as m; println(m.round(m.log(100, 10)));"),
        "2\n"
    );
    assert_eq!(
        out("import \"math\" as m; println(m.round(m.log(m.e)));"),
        "1\n"
    );
}

#[test]
fn map_iteration_helpers() {
    let src = "import \"map\" as mp;
               let m = {a: 1, b: 2, c: 3};
               let total = [0];
               mp.each(m, fn(k, v) { total[0] = total[0] + v; });
               println(total[0]);
               let doubled = mp.map(m, fn(k, v) { return v * 10; });
               println(doubled[\"a\"]); println(doubled[\"c\"]);
               let odds = mp.filter(m, fn(k, v) { return v % 2 == 1; });
               println(mp.has(odds, \"a\")); println(mp.has(odds, \"b\")); println(mp.has(odds, \"c\"));
               let m2 = mp.from_entries([[\"x\", 1], [\"y\", 2]]);
               println(m2[\"x\"] + m2[\"y\"]);
               mp.clear(m);
               println(mp.len(m));";
    assert_eq!(out(src), "6\n10\n30\ntrue\nfalse\ntrue\n3\n0\n");
}

#[test]
fn io_directory_ops() {
    // Exercise the directory-manipulation natives against a real temp directory.
    let dir = std::env::temp_dir().join("lumen_test_io_dir_ops");
    let _ = std::fs::remove_dir_all(&dir);
    let p = dir.to_str().unwrap().replace('\\', "/");
    let src = format!(
        "import \"io\" as io;\n\
         io.mkdir(\"{p}/sub\");\n\
         io.write_file(\"{p}/a.txt\", \"A\");\n\
         io.write_file(\"{p}/b.txt\", \"B\");\n\
         println(io.is_dir(\"{p}\"));\n\
         println(io.is_file(\"{p}/a.txt\"));\n\
         println(io.is_file(\"{p}/missing\"));\n\
         println(io.listdir(\"{p}\"));\n\
         io.remove(\"{p}/a.txt\");\n\
         println(io.exists(\"{p}/a.txt\"));\n\
         io.rmdir(\"{p}/sub\");\n\
         println(io.is_dir(\"{p}/sub\"));"
    );
    let result = out(&src);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        result,
        "true\ntrue\nfalse\n[\"a.txt\", \"b.txt\", \"sub\"]\nfalse\nfalse\n"
    );
}

#[test]
fn io_bytes_stat_walk() {
    // Byte I/O, stat, recursive walk, copy, and rename against a real temp tree.
    let dir = std::env::temp_dir().join("lumen_test_io_bytes_stat_walk");
    let _ = std::fs::remove_dir_all(&dir);
    let p = dir.to_str().unwrap().replace('\\', "/");
    let src = format!(
        "import \"io\" as io;\n\
         io.mkdir(\"{p}/sub\");\n\
         io.write_bytes(\"{p}/raw.bin\", [104, 105]);\n\
         println(io.read_bytes(\"{p}/raw.bin\"));\n\
         try {{ io.write_bytes(\"{p}/bad.bin\", [256]); }} catch (e) {{ println(e.kind); }}\n\
         let st = io.stat(\"{p}/raw.bin\");\n\
         println(st[\"is_file\"]);\n\
         println(st[\"size\"]);\n\
         println(st[\"is_dir\"]);\n\
         io.write_file(\"{p}/sub/c.txt\", \"C\");\n\
         println(io.walk(\"{p}\"));\n\
         io.copy(\"{p}/raw.bin\", \"{p}/copy.bin\");\n\
         println(io.exists(\"{p}/copy.bin\"));\n\
         io.rename(\"{p}/copy.bin\", \"{p}/moved.bin\");\n\
         println(io.exists(\"{p}/copy.bin\"));\n\
         println(io.exists(\"{p}/moved.bin\"));"
    );
    let result = out(&src);
    let _ = std::fs::remove_dir_all(&dir);
    let expected = format!(
        "[104, 105]\nValueError\ntrue\n2\nfalse\n\
         [\"{p}/raw.bin\", \"{p}/sub\", \"{p}/sub/c.txt\"]\ntrue\nfalse\ntrue\n"
    );
    assert_eq!(result, expected);
}

#[test]
fn string_format() {
    // `{}` consumes the next positional argument; `{N}` is indexed; `{{`/`}}` are
    // literal braces.
    assert_eq!(
        out("import \"string\" as s; println(s.format(\"{} + {} = {}\", [1, 2, 3]));"),
        "1 + 2 = 3\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.format(\"{0} {0} {1}\", [\"a\", \"b\"]));"),
        "a a b\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.format(\"100% {{x}}\", []));"),
        "100% {x}\n"
    );
    assert_eq!(
        out("import \"string\" as s; println(s.format(\"no slots\", [1]));"),
        "no slots\n"
    );
    // Values render the same way `str()`/`println` would (no surrounding quotes).
    assert_eq!(
        out("import \"string\" as s; println(s.format(\"<{}>\", [[1, 2]]));"),
        "<[1, 2]>\n"
    );
    // Errors are ValueErrors: too few args, index out of range, unmatched brace.
    assert_eq!(
        out("import \"string\" as s; try { s.format(\"{}\", []); } catch (e) { println(e.kind); }"),
        "ValueError\n"
    );
    assert_eq!(out("import \"string\" as s; try { s.format(\"{9}\", [1]); } catch (e) { println(e.kind); }"), "ValueError\n");
    assert_eq!(out("import \"string\" as s; try { s.format(\"a { b\", []); } catch (e) { println(e.kind); }"), "ValueError\n");
}

#[test]
fn string_format_specifiers() {
    // `call` is the `s.format(...)` argument list; raw strings avoid escaping.
    fn fmtout(call: &str) -> String {
        out(&(r#"import "string" as s; println(s.format("#.to_string() + call + "));"))
    }
    // Precision.
    assert_eq!(fmtout(r#""{:.2f}", [3.14159]"#), "3.14\n");
    assert_eq!(fmtout(r#""{:.0f}", [3.7]"#), "4\n");
    // Width and zero-pad.
    assert_eq!(fmtout(r#""[{:5}]", [42]"#), "[   42]\n");
    assert_eq!(fmtout(r#""[{:05}]", [42]"#), "[00042]\n");
    assert_eq!(fmtout(r#""[{:05}]", [-42]"#), "[-0042]\n");
    // Alignment, with and without a fill char.
    assert_eq!(fmtout(r#""[{:>8}]", ["hi"]"#), "[      hi]\n");
    assert_eq!(fmtout(r#""[{:<8}]", ["hi"]"#), "[hi      ]\n");
    assert_eq!(fmtout(r#""[{:^8}]", ["hi"]"#), "[   hi   ]\n");
    assert_eq!(fmtout(r#""[{:*^8}]", ["hi"]"#), "[***hi***]\n");
    // Sign.
    assert_eq!(fmtout(r#""{:+}", [42]"#), "+42\n");
    assert_eq!(fmtout(r#""{:+}", [-42]"#), "-42\n");
    // Bases, with and without the alternate prefix.
    assert_eq!(fmtout(r#""{:x}", [255]"#), "ff\n");
    assert_eq!(fmtout(r#""{:#x}", [255]"#), "0xff\n");
    assert_eq!(fmtout(r#""{:b}", [5]"#), "101\n");
    assert_eq!(fmtout(r#""{:o}", [8]"#), "10\n");
    // Width + precision together.
    assert_eq!(fmtout(r#""[{:8.2f}]", [3.14159]"#), "[    3.14]\n");
    // An invalid spec is a ValueError.
    assert_eq!(
        out(
            r#"import "string" as s; try { s.format("{:q}", [1]); } catch (e) { println(e.kind); }"#
        ),
        "ValueError\n"
    );
}

#[test]
fn hash_module() {
    // Non-cryptographic hashes are deterministic and distinguish inputs.
    assert_eq!(
        out("import \"hash\" as h; println(h.fnv1a(\"hello\") == h.fnv1a(\"hello\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"hash\" as h; println(h.fnv1a(\"hello\") == h.fnv1a(\"world\"));"),
        "false\n"
    );
    assert_eq!(
        out("import \"hash\" as h; println(type(h.fnv1a(\"x\")));"),
        "int\n"
    );
    assert_eq!(
        out("import \"hash\" as h; println(type(h.djb2(\"x\")));"),
        "int\n"
    );
    // Hex is lowercase, two characters per byte.
    assert_eq!(
        out("import \"hash\" as h; println(h.hex_encode(\"abc\"));"),
        "616263\n"
    );
    assert_eq!(
        out("import \"hash\" as h; println(h.hex_decode(\"616263\"));"),
        "abc\n"
    );
    // Base64 uses the standard alphabet and `=` padding.
    assert_eq!(
        out("import \"hash\" as h; println(h.base64_encode(\"Man\"));"),
        "TWFu\n"
    );
    assert_eq!(
        out("import \"hash\" as h; println(h.base64_encode(\"Ma\"));"),
        "TWE=\n"
    );
    assert_eq!(
        out("import \"hash\" as h; println(h.base64_encode(\"M\"));"),
        "TQ==\n"
    );
    assert_eq!(
        out("import \"hash\" as h; println(h.base64_decode(\"YWJj\"));"),
        "abc\n"
    );
    // Round-trips preserve multi-byte UTF-8.
    assert_eq!(
        out("import \"hash\" as h; println(h.hex_decode(h.hex_encode(\"héllo 🚀\")));"),
        "héllo 🚀\n"
    );
    assert_eq!(
        out("import \"hash\" as h; println(h.base64_decode(h.base64_encode(\"héllo 🚀\")));"),
        "héllo 🚀\n"
    );
    // Malformed input throws ValueError.
    assert_eq!(
        out("import \"hash\" as h; try { h.hex_decode(\"xyz\"); } catch (e) { println(e.kind); }"),
        "ValueError\n"
    );
    assert_eq!(out("import \"hash\" as h; try { h.base64_decode(\"@@@@\"); } catch (e) { println(e.kind); }"), "ValueError\n");
}

#[test]
fn regex_test_and_anchors() {
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"\\\\d+\", \"abc123\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"^\\\\d+$\", \"abc123\"));"),
        "false\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"^[a-z]+$\", \"hello\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"^[a-z]+$\", \"Hello\"));"),
        "false\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"colou?r\", \"color\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"colou?r\", \"colour\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"cat|dog\", \"i have a dog\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"^a{2,3}$\", \"aaa\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"^a{2,3}$\", \"aaaa\"));"),
        "false\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"[^0-9]+\", \"123\"));"),
        "false\n"
    );
}

#[test]
fn regex_find_and_captures() {
    let m = "import \"regex\" as re; let m = re.find(\"\\\\d+\", \"abc123def\");
             println(m[\"text\"]); println(m[\"start\"]); println(m[\"end\"]);";
    assert_eq!(out(m), "123\n3\n6\n");
    assert_eq!(
        out("import \"regex\" as re; println(re.find(\"xyz\", \"abc\"));"),
        "nil\n"
    );
    let c = "import \"regex\" as re; let c = re.captures(\"(\\\\d+)-(\\\\d+)\", \"12-34\");
             println(c[0]); println(c[1]); println(c[2]);";
    assert_eq!(out(c), "12-34\n12\n34\n");
    let all = "import \"regex\" as re; let a = re.find_all(\"\\\\d+\", \"a1b22c333\");
               println(len(a)); println(a[0][\"text\"]); println(a[2][\"text\"]);";
    assert_eq!(out(all), "3\n1\n333\n");
}

#[test]
fn regex_replace_and_split() {
    assert_eq!(
        out("import \"regex\" as re; println(re.replace(\"(\\\\w+)@(\\\\w+)\", \"ab@cd\", \"$2.$1\"));"),
        "cd.ab\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.replace(\"\\\\s+\", \"a  b   c\", \"_\"));"),
        "a_b_c\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.split(\",\", \"a,b,c\"));"),
        "[\"a\", \"b\", \"c\"]\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.split(\"\\\\s+\", \"a  b c\"));"),
        "[\"a\", \"b\", \"c\"]\n"
    );
}

#[test]
fn regex_backtracking_and_quantifiers() {
    // Greedy vs lazy `.*`.
    assert_eq!(
        out("import \"regex\" as re; println(re.find(\"a.*b\", \"axbxb\")[\"text\"]);"),
        "axbxb\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.find(\"a.*?b\", \"axbxb\")[\"text\"]);"),
        "axb\n"
    );
    // Greedy `a*` must give back one `a` so the trailing `a` can match.
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"^a*a$\", \"aaa\"));"),
        "true\n"
    );
    // Nested capture groups, left-to-right index assignment.
    let nest = "import \"regex\" as re; let c = re.captures(\"((a+)(b+))\", \"aaabb\");
                println(c[0]); println(c[1]); println(c[2]); println(c[3]);";
    assert_eq!(out(nest), "aaabb\naaabb\naaa\nbb\n");
    // Alternation in a group + an outer quantifier.
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"^(cat|dog)s?$\", \"dogs\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"^(ab)+$\", \"ababab\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"^(ab)+$\", \"aba\"));"),
        "false\n"
    );
    // `.` does not cross a newline.
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"a.b\", \"a\\nb\"));"),
        "false\n"
    );
}

#[test]
fn regex_nullable_loops_terminate() {
    // A quantifier over an empty-matchable body must terminate and match —
    // previously these recursed infinitely and crashed the process.
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"(a*)*\", \"aaa\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.find(\"(a*)*\", \"aaa\")[\"text\"]);"),
        "aaa\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"(a?)*b\", \"aab\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"(.*)*\", \"xyz\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"(a|)*b\", \"aab\"));"),
        "true\n"
    );
    // Negative case: must converge to a clean `false`, not loop/error (this is
    // what exposes a stale empty-loop mark under backtracking).
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"(a|)*b\", \"aaa\"));"),
        "false\n"
    );
}

#[test]
fn regex_class_ranges_with_escaped_bounds() {
    // A class range whose lower bound is an escaped char must still form a range.
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"[\\\\t-~]\", \"A\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"regex\" as re; println(re.test(\"[\\\\t-~]\", \"5\"));"),
        "true\n"
    );
    let fa = "import \"regex\" as re; let a = re.find_all(\"[\\\\.-9]\", \"./5a\");
              println(len(a)); println(a[0][\"text\"]); println(a[1][\"text\"]); println(a[2][\"text\"]);";
    assert_eq!(out(fa), "3\n.\n/\n5\n");
}

#[test]
fn regex_long_match_is_bounded_not_crashed() {
    // A single greedy match longer than the recursion limit must raise a
    // *catchable* ValueError — never crash the host (a SIGABRT would abort this
    // whole test binary). Runs in a cargo-test worker thread (smaller stack), so
    // this also guards the depth limit against the tightest real stack.
    let big = "import \"regex\" as re; import \"string\" as s;
               let a = s.repeat(\"a\", 30000);
               try { re.test(\"a*\", a); println(\"ok\"); } catch (e) { println(e.kind); }";
    assert_eq!(out(big), "ValueError\n");
    // Deeply-nested groups are bounded at parse time (same crash class), catchable.
    let nested = "import \"regex\" as re; import \"string\" as s;
                  let p = s.repeat(\"(\", 3000) + \"a\" + s.repeat(\")\", 3000);
                  try { re.test(p, \"a\"); println(\"ok\"); } catch (e) { println(e.kind); }";
    assert_eq!(out(nested), "ValueError\n");
}

#[test]
fn regex_bounds_pathological_backtracking() {
    // Catastrophic backtracking must hit the step budget and raise an error
    // rather than hang the interpreter.
    let src = "import \"regex\" as re;
               try { re.test(\"(a+)+$\", \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa!\"); println(\"finished\"); }
               catch (e) { println(e.kind); }";
    assert_eq!(out(src), "ValueError\n");
}

#[test]
fn regex_invalid_pattern_throws() {
    assert_eq!(
        out("import \"regex\" as re; try { re.test(\"(unclosed\", \"x\"); } catch (e) { println(e.kind); }"),
        "ValueError\n"
    );
    assert_eq!(
        out("import \"regex\" as re; try { re.test(\"[a-\", \"x\"); } catch (e) { println(e.kind); }"),
        "ValueError\n"
    );
}

#[test]
fn datetime_module() {
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.is_leap_year(2024));"),
        "true\n"
    );
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.is_leap_year(1900));"),
        "false\n"
    );
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.is_leap_year(2000));"),
        "true\n"
    );
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.days_in_month(2024, 2));"),
        "29\n"
    );
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.days_in_month(2023, 2));"),
        "28\n"
    );
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.days_in_month(2024, 4));"),
        "30\n"
    );
    // Epoch 0 is 1970-01-01T00:00:00Z, a Thursday (weekday 4, Sunday = 0).
    let comp = "import \"datetime\" as dt; let c = dt.from_epoch(0);
                println(c[\"year\"]); println(c[\"month\"]); println(c[\"day\"]);
                println(c[\"hour\"]); println(c[\"weekday\"]); println(c[\"yearday\"]);";
    assert_eq!(out(comp), "1970\n1\n1\n0\n4\n1\n");
    // Round-trips against known timestamps.
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.to_epoch(1970, 1, 1, 0, 0, 0));"),
        "0\n"
    );
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.to_epoch(2000, 1, 1, 0, 0, 0));"),
        "946684800\n"
    );
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.from_epoch(946684800)[\"weekday\"]);"),
        "6\n"
    );
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.iso(0));"),
        "1970-01-01T00:00:00Z\n"
    );
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.iso(1609459200));"),
        "2021-01-01T00:00:00Z\n"
    );
    // Negative epoch (before 1970) is handled by Euclidean division.
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.iso(-1));"),
        "1969-12-31T23:59:59Z\n"
    );
    assert_eq!(
        out("import \"datetime\" as dt; println(dt.format(0, \"%Y/%m/%d %H:%M:%S\"));"),
        "1970/01/01 00:00:00\n"
    );
}

#[test]
fn datetime_from_epoch_is_gc_safe_under_stress() {
    // Each from_epoch allocates a map + string keys; under stress GC the rooted
    // map-building discipline must keep the result (and its keys) live.
    let src = "import \"datetime\" as dt;
               let last = nil;
               for let i = 0; i < 50; i = i + 1 { last = dt.from_epoch(i * 86400); }
               println(last[\"month\"]); println(last[\"day\"]);";
    assert_eq!(run_with(src, true), "2\n19\n");
}

#[test]
fn path_module() {
    // `path` is self-hosted (std/path.lum) and uses POSIX `/` separators.
    assert_eq!(
        out("import \"path\" as p; println(p.join([\"a\", \"b\", \"c\"]));"),
        "a/b/c\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.join([\"/usr\", \"bin\"]));"),
        "/usr/bin\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.join([\"a/\", \"\", \"b\"]));"),
        "a/b\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.basename(\"/a/b/c.txt\"));"),
        "c.txt\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.dirname(\"/a/b/c.txt\"));"),
        "/a/b\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.dirname(\"file\"));"),
        ".\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.dirname(\"/x\"));"),
        "/\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.ext(\"archive.tar.gz\"));"),
        "gz\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.ext(\"noext\"));"),
        "\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.ext(\".hidden\"));"),
        "\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.stem(\"a.b.c\"));"),
        "a.b\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.is_absolute(\"/x\"));"),
        "true\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.is_absolute(\"x\"));"),
        "false\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.split(\"/a//b/c\"));"),
        "[\"a\", \"b\", \"c\"]\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.normalize(\"/a/b/../c\"));"),
        "/a/c\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.normalize(\"a/./b/../d\"));"),
        "a/d\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.normalize(\"../x\"));"),
        "../x\n"
    );
    assert_eq!(
        out("import \"path\" as p; println(p.normalize(\"/..\"));"),
        "/\n"
    );
}
