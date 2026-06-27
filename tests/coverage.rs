//! Breadth tests that exercise the runtime and standard library widely — the
//! error paths, the debug API, and the many stdlib functions the feature tests
//! don't individually touch. Their purpose is twofold: real behavioral checks,
//! and pushing the core components (VM, builtins, stdlib) past the 90% coverage
//! bar (the compiler front end is already there).

use lumen::vm::{DebugStatus, Vm};
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

fn run(src: &str) -> String {
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty(), "front-end errors: {errs:?}");
    let proto = lumen::compiler::compile(&program).expect("compile");
    let buf = SharedBuf(Rc::new(RefCell::new(Vec::new())));
    let mut vm = Vm::with_output(Box::new(buf.clone()));
    lumen::stdlib::install(&mut vm);
    vm.interpret(proto)
        .unwrap_or_else(|e| panic!("runtime error:\n{e}"));
    let out = String::from_utf8(buf.0.borrow().clone()).unwrap();
    out
}

/// Run `try { body } catch (e) { println(e.kind); }` and return the printed kind.
fn kind(body: &str) -> String {
    let out = run(&format!(
        "try {{ {body} }} catch (e) {{ println(e.kind); }}"
    ));
    out.trim().to_string()
}

#[test]
fn vm_and_builtin_error_paths() {
    // Calls / operators on wrong types.
    assert_eq!(kind("let x = 5; x();"), "TypeError"); // call a non-callable
    assert_eq!(kind("print(5[0]);"), "TypeError"); // index a non-collection
    assert_eq!(kind("let x = 5; x[0] = 1;"), "TypeError"); // index-assign non-collection
    assert_eq!(kind("let x = 5; print(x.foo);"), "TypeError"); // property on non-object
    assert_eq!(kind("let x = 5; x.foo = 1;"), "TypeError"); // set field on non-instance
    assert_eq!(kind("print(-\"x\");"), "TypeError"); // negate a non-number
    assert_eq!(kind("print(1 < \"x\");"), "TypeError"); // compare mixed types
    assert_eq!(kind("print(\"abc\"[9]);"), "IndexError"); // string index OOB
    assert_eq!(kind("let a = [1]; a[5] = 2;"), "IndexError"); // array set OOB (not append)
    assert_eq!(
        kind("class A {} class B < A {} let b = B(1);"),
        "ArityError"
    ); // no init, args given
    assert_eq!(kind("let notc = 5; class Z < notc {}"), "TypeError"); // superclass not a class

    // Builtin argument errors.
    assert_eq!(kind("push(5, 1);"), "TypeError");
    assert_eq!(kind("pop([]);"), "IndexError");
    assert_eq!(kind("keys(5);"), "TypeError");
    assert_eq!(kind("len(5);"), "TypeError");
    assert_eq!(kind("int(nil);"), "TypeError");
    assert_eq!(kind("int(\"notanumber\");"), "ValueError");
    assert_eq!(kind("float(nil);"), "TypeError");
    assert_eq!(kind("chr(-1);"), "ValueError");
    assert_eq!(kind("ord(\"ab\");"), "ValueError");
    assert_eq!(kind("range(0, 10, 0);"), "ValueError"); // zero step
    assert_eq!(kind("assert(false, \"nope\");"), "AssertionError");

    // Stdlib module argument errors.
    assert_eq!(kind("import \"math\" as m; m.sqrt(\"x\");"), "TypeError");
    assert_eq!(kind("import \"string\" as s; s.upper(5);"), "TypeError");
    assert_eq!(
        kind("import \"array\" as a; a.sum([1, \"x\"]);"),
        "TypeError"
    );
    assert_eq!(kind("import \"array\" as a; a.min([]);"), "ValueError");
    assert_eq!(kind("import \"map\" as mp; mp.get(5, \"k\");"), "TypeError");
    assert_eq!(
        kind("import \"json\" as j; j.parse(\"{bad}\");"),
        "ValueError"
    );
    assert_eq!(
        kind("import \"json\" as j; j.stringify(fn(){});"),
        "TypeError"
    ); // unserializable
    assert_eq!(
        kind("import \"random\" as r; r.randint(5, 1);"),
        "ValueError"
    );
    assert_eq!(kind("import \"random\" as r; r.choice([]);"), "ValueError");
}

#[test]
fn more_vm_runtime_paths() {
    // Overflow on every integer operator.
    assert_eq!(kind("print(9223372036854775807 * 2);"), "ValueError");
    assert_eq!(kind("print(-9223372036854775807 - 2);"), "ValueError");
    // Float vs int division/modulo by zero.
    assert_eq!(kind("print(1.0 / 0.0);"), "DivisionByZero");
    assert_eq!(kind("print(1 % 0);"), "DivisionByZero");
    assert_eq!(kind("print(1.0 % 0.0);"), "DivisionByZero");
    // for-in over a string and over a map (iteration of each kind).
    assert_eq!(
        run("for ch in \"ab\" { print(ch); } println(\"\");"),
        "ab\n"
    );
    assert_eq!(
        run("for k in {x: 1, y: 2} { print(k); } println(\"\");"),
        "xy\n"
    );
    // Array spread of a string and of a map (ArrayExtend on each).
    assert_eq!(
        run("println([..\"ab\", \"c\"]);"),
        "[\"a\", \"b\", \"c\"]\n"
    );
    assert_eq!(run("println([..{p: 1, q: 2}]);"), "[\"p\", \"q\"]\n");
    assert_eq!(kind("println([..5]);"), "TypeError"); // spread a non-iterable
                                                      // super method *call* (GET_SUPER then CALL) and an undefined super method.
    assert_eq!(
        run("class A { greet(n) { return \"hi ${n}\"; } } class B < A { greet(n) { return super.greet(n) + \"!\"; } } println(B().greet(\"x\"));"),
        "hi x!\n"
    );
    assert_eq!(
        kind("class A {} class B < A { m() { return super.nope(); } } B().m();"),
        "NameError"
    );
    // Bound method stored as a value, then called.
    assert_eq!(
        run("class C { init(v) { this.v = v; } get() { return this.v; } } let g = C(42).get; println(g());"),
        "42\n"
    );
    // and/or value-preservation + short-circuit (no evaluation of RHS).
    assert_eq!(
        run("fn boom() { throw \"no\"; } println(true || boom()); println(false && boom());"),
        "true\nfalse\n"
    );
    // Equality across object identities and nil.
    assert_eq!(
        run("let a = [1]; let b = [1]; println(a == a); println(a == b); println(nil == nil);"),
        "true\nfalse\ntrue\n"
    );
}

#[test]
#[allow(unused_assignments)] // the macro refreshes `globals` after each eval; the last refresh is unread
fn vm_eval_persists_and_recovers() {
    use lumen::value::Value;
    let mut vm = Vm::with_output(Box::new(std::io::sink()));
    lumen::stdlib::install(&mut vm);
    let mut globals: Vec<String> = Vec::new();

    macro_rules! ev {
        ($src:expr) => {{
            let (program, mut errs) = lumen::parse_source($src);
            if errs.is_empty() {
                errs = lumen::resolver::resolve_with(&program, &globals);
            }
            errs.retain(|d| d.severity == lumen::Severity::Error); // ignore warnings
            assert!(errs.is_empty(), "static errors: {:?}", errs);
            let (proto, _print) = lumen::compiler::compile_repl(&program).unwrap();
            let r = vm.eval(proto);
            globals = vm.global_names();
            r
        }};
    }

    assert!(ev!("let x = 10;").is_ok());
    assert!(globals.contains(&"x".to_string()));
    // A trailing expression returns its value (REPL print path).
    assert!(matches!(ev!("x + 5;"), Ok(Value::Int(15))));
    // A runtime error returns Err and resets transient state...
    assert!(ev!("1 / 0;").is_err());
    // ...but globals persist and the VM is still usable afterward.
    assert!(matches!(ev!("x * 2;"), Ok(Value::Int(20))));
}

#[test]
fn debug_step_surfaces_errors() {
    let (program, _) = lumen::check_source("fn f() { throw \"boom\"; } f();");
    let proto = lumen::compiler::compile(&program).unwrap();
    let mut vm = Vm::with_output(Box::new(std::io::sink()));
    lumen::stdlib::install(&mut vm);
    vm.debug_start(proto).unwrap();
    let mut got_error = false;
    for _ in 0..1000 {
        match vm.debug_step() {
            DebugStatus::Error(m) => {
                assert!(m.contains("Uncaught"));
                got_error = true;
                break;
            }
            DebugStatus::Finished => break,
            DebugStatus::Running => {}
        }
    }
    assert!(got_error, "debugger never surfaced the throw");
}

#[test]
fn debug_renders_diverse_local_types() {
    // A function whose locals span every heap object kind, so stepping through it
    // and inspecting locals exercises every `debug_display` branch.
    let src = "class K { hi() { return 1; } }
        fn mk() { return K(); }
        fn f(modv) {
            let s = \"x\"; let a = [1, 2]; let m = {k: 1};
            let cl = fn() { return s; }; let n = nil; let b = true;
            let inst = mk(); let nat = print; let cls = K;
            let bm = inst.hi; let modl = modv;
            return 0;
        }
        import \"math\" as mathmod;
        f(mathmod);";
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty(), "{errs:?}");
    let proto = lumen::compiler::compile(&program).unwrap();
    let mut vm = Vm::with_output(Box::new(std::io::sink()));
    lumen::stdlib::install(&mut vm);
    vm.debug_start(proto).unwrap();
    let mut max_locals = 0;
    for _ in 0..5000 {
        let locals = vm.debug_locals();
        max_locals = max_locals.max(locals.len());
        for (name, _) in &locals {
            let _ = vm.debug_lookup(name);
        }
        match vm.debug_step() {
            DebugStatus::Finished => break,
            DebugStatus::Error(m) => panic!("{m}"),
            DebugStatus::Running => {}
        }
    }
    assert!(
        max_locals >= 10,
        "expected to see many typed locals, saw {max_locals}"
    );
}

#[test]
fn import_errors() {
    // Missing module file -> uncaught NameError with the path.
    let (program, errs) = lumen::check_source("import \"definitely_not_a_module_xyz\";");
    assert!(errs.is_empty());
    let proto = lumen::compiler::compile(&program).unwrap();
    let mut vm = Vm::with_output(Box::new(std::io::sink()));
    lumen::stdlib::install(&mut vm);
    let msg = vm
        .interpret(proto)
        .expect_err("missing module should error");
    assert!(msg.contains("cannot find module"));
}

#[test]
fn builtin_conversions_and_helpers() {
    assert_eq!(run("println(int(3.9));"), "3\n");
    assert_eq!(run("println(int(\"42\"));"), "42\n");
    assert_eq!(run("println(int(true));"), "1\n");
    assert_eq!(run("println(float(7));"), "7.0\n");
    assert_eq!(run("println(float(\"2.5\"));"), "2.5\n");
    assert_eq!(run("println(bool(0));"), "true\n");
    assert_eq!(run("println(bool(nil));"), "false\n");
    assert_eq!(run("println(type(1)); println(type(1.0)); println(type(\"x\")); println(type([])); println(type({})); println(type(nil)); println(type(true));"),
        "int\nfloat\nstring\narray\nmap\nnil\nbool\n");
    assert_eq!(run("println(chr(65)); println(ord(\"A\"));"), "A\n65\n");
    assert_eq!(run("println(range(3)); println(range(2, 5)); println(range(0, 10, 3)); println(range(5, 0, -1));"),
        "[0, 1, 2]\n[2, 3, 4]\n[0, 3, 6, 9]\n[5, 4, 3, 2, 1]\n");
    assert_eq!(
        run("println(len(\"héllo\")); println(len([1,2,3])); println(len({a: 1}));"),
        "5\n3\n1\n"
    );
    assert_eq!(
        run("let a = [3]; assert(pop(a) == 3); println(len(a));"),
        "0\n"
    );
    assert_eq!(run("let m = {a: 1, b: 2}; del(m, \"a\"); println(keys(m)); println(values(m)); println(has(m, \"b\"));"),
        "[\"b\"]\n[2]\ntrue\n");
    assert_eq!(
        run("let a = [1,2,3]; del(a, 1); println(a); del(a, -1); println(a);"),
        "[1, 3]\n[1]\n"
    );
    assert_eq!(
        run("println(str(nil)); println(str([1, \"a\", true]));"),
        "nil\n[1, \"a\", true]\n"
    );
}

#[test]
fn vm_value_display_paths() {
    // Exercises to_display for each object kind via interpolation.
    assert_eq!(run("fn f(){} println(\"${f}\");"), "<fn f>\n");
    assert_eq!(run("class C {} println(\"${C}\");"), "<class C>\n");
    assert_eq!(
        run("class C {} let c = C(); println(\"${c}\");"),
        "<C instance>\n"
    );
    assert_eq!(run("println(\"${println}\");"), "<fn println>\n");
    assert_eq!(
        run("import \"math\" as m; println(\"${m}\");"),
        "<module math>\n"
    );
    assert_eq!(
        run("println(\"${ {a: 1, b: [2, 3]} }\");"),
        "{\"a\": 1, \"b\": [2, 3]}\n"
    );
}

#[test]
fn math_breadth() {
    let src = "import \"math\" as m;
        println(m.abs(-3.5)); println(m.cbrt(27)); println(m.sign(-4)); println(m.sign(0));
        println(m.round(2.5)); println(m.trunc(-2.7)); println(m.min(2.0, 1)); println(m.hypot(3, 4));
        println(m.log2(8)); println(m.log10(1000)); println(m.exp(0));
        println(m.sin(0)); println(m.cos(0)); println(m.tan(0));
        println(m.asin(0)); println(m.acos(1)); println(m.atan(0)); println(m.atan2(0, 1));
        println(m.gcd(12, 18));";
    let out = run(src);
    assert!(out.contains("3.5")); // abs
    assert!(out.contains("\n5.0\n")); // hypot(3,4) = 5.0 (float)
    assert!(out.contains("\n6\n")); // gcd(12,18) = 6 (int)
}

#[test]
fn string_breadth() {
    let src = "import \"string\" as s;
        println(s.substring(\"hello\", 1, 4));
        println(s.char_at(\"hello\", -1));
        println(s.chars(\"abc\"));
        println(s.ends_with(\"file.lum\", \".lum\"));
        println(s.trim_start(\"  x\")); println(s.trim_end(\"x  \"));
        println(s.pad_right(\"7\", 3, \".\"));
        println(s.index_of(\"abc\", \"z\"));";
    assert_eq!(
        run(src),
        "ell\no\n[\"a\", \"b\", \"c\"]\ntrue\nx\nx\n7..\n-1\n"
    );
}

#[test]
fn array_breadth() {
    let src = "import \"array\" as a;
        println(a.min([3,1,2])); println(a.max([3,1,2]));
        println(a.slice([1,2,3,4,5], 1, 4)); println(a.slice([1,2,3], -2, 3));
        println(a.concat([1,2], [3,4]));
        println(a.first([9,8])); println(a.last([9,8]));
        println(a.first([])); println(a.last([]));
        println(a.contains([1,2,3], 2)); println(a.index_of([1,2,3], 3));
        a.each([1,2,3], fn(x) { print(x); }); println(\"\");
        println(a.sum([1.5, 2.5]));
        println(a.sort([\"banana\", \"apple\", \"cherry\"]));";
    let out = run(src);
    assert!(out.contains("[2, 3, 4]"));
    assert!(out.contains("[\"apple\", \"banana\", \"cherry\"]"));
    assert!(out.contains("4.0")); // sum of floats
}

#[test]
fn map_and_random_and_json_breadth() {
    assert_eq!(
        run("import \"map\" as m; let d = {a: 1}; println(m.entries(d)); let e = m.merge({x: 1}, {y: 2, x: 9}); println(m.get(e, \"x\")); println(m.len(e));"),
        "[[\"a\", 1]]\n9\n2\n"
    );
    // random: seeded determinism + shuffle/choice stay in-domain.
    let src = "import \"random\" as r; r.seed(7);
        let s = r.shuffle([1,2,3,4,5]); println(len(s));
        println(r.choice([42]) == 42);
        let x = r.random(); println(x >= 0.0 && x < 1.0);";
    assert_eq!(run(src), "5\ntrue\ntrue\n");
    // json edge cases: nested, escapes, pretty indent, empty containers.
    let src2 = "import \"json\" as j;
        println(j.stringify({})); println(j.stringify([]));
        println(j.parse(\"-12.5e2\"));
        let d = j.parse(\"{\\\"a\\\": [1, {\\\"b\\\": null}]}\");
        println(d[\"a\"][1][\"b\"]);
        println(j.stringify(\"a\\nb\"));";
    assert_eq!(run(src2), "{}\n[]\n-1250.0\nnil\n\"a\\nb\"\n");
}

#[test]
fn debug_api_drives_execution() {
    let src = "fn add(a, b) { let s = a + b; return s; } let r = add(2, 3);";
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty());
    let proto = lumen::compiler::compile(&program).expect("compile");
    let mut vm = Vm::with_output(Box::new(std::io::sink()));
    lumen::stdlib::install(&mut vm);
    vm.debug_start(proto).expect("start");

    let mut steps = 0;
    let mut saw_add_frame = false;
    let mut saw_named_local = false;
    loop {
        if let Some((name, _line, _ip)) = vm.debug_location() {
            if name.contains("add") {
                saw_add_frame = true;
                // Inside add, backtrace should include the script too.
                assert!(vm.debug_backtrace().len() >= 2);
                if vm.debug_locals().iter().any(|(n, _)| n == "a" || n == "s") {
                    saw_named_local = true;
                }
                // `print a` style lookup.
                let _ = vm.debug_lookup("a");
                // Disassembly of the current frame is available.
                assert!(vm.debug_current_frame().is_some());
            }
        }
        match vm.debug_step() {
            DebugStatus::Running => {}
            DebugStatus::Finished => break,
            DebugStatus::Error(m) => panic!("debug error: {m}"),
        }
        steps += 1;
        assert!(steps < 10_000, "runaway");
    }
    assert!(saw_add_frame, "never entered the add frame");
    assert!(saw_named_local, "named locals not visible");
}

#[test]
fn try_finally_without_catch_runs_on_all_paths() {
    // normal completion
    assert_eq!(
        run("try { println(\"body\"); } finally { println(\"fin\"); }"),
        "body\nfin\n"
    );
    // exception propagates but finally still runs (then caught by outer)
    let src = "try {
                   try { throw \"x\"; } finally { println(\"inner-fin\"); }
               } catch (e) { println(\"caught ${e}\"); }";
    assert_eq!(run(src), "inner-fin\ncaught x\n");
    // return through finally
    assert_eq!(
        run("fn f() { try { return 1; } finally { println(\"cleanup\"); } } println(f());"),
        "cleanup\n1\n"
    );
}

#[test]
fn nested_modules_and_caching() {
    // Importing the same native module twice yields the same object (cached).
    assert_eq!(
        run("import \"math\" as a; import \"math\" as b; println(a == b);"),
        "true\n"
    );
    // Self-hosted module functions work and are cached too.
    assert_eq!(
        run("import \"seq\" as q; println(q.find([1,2,3,4], fn(x) { return x > 2; }));"),
        "3\n"
    );
}

#[test]
fn assignment_forms_and_index_growth() {
    // index-assign at exactly length appends.
    assert_eq!(run("let a = [1, 2]; a[2] = 3; println(a);"), "[1, 2, 3]\n");
    // negative index assignment.
    assert_eq!(
        run("let a = [1, 2, 3]; a[-1] = 99; println(a);"),
        "[1, 2, 99]\n"
    );
    // map computed-key set/get.
    assert_eq!(
        run("let m = {}; m[1 + 1] = \"two\"; println(m[2]);"),
        "two\n"
    );
    // field create + chained access.
    assert_eq!(
        run("class P {} let p = P(); p.x = 5; p.y = p.x * 2; println(p.y);"),
        "10\n"
    );
}
