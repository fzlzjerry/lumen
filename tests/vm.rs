//! End-to-end VM behavior tests: compile a source string, run it with captured
//! output, and assert on what it printed (or the uncaught error). These exercise
//! the whole pipeline (lex → parse → resolve → compile → execute) and pin the
//! runtime semantics from SPEC §6–7.

use lumen::vm::Vm;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

/// A `Write` sink that keeps its bytes so the test can read them back.
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

/// Run `src`, returning captured stdout. Panics on a front-end error.
fn run(src: &str) -> Result<String, String> {
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty(), "front-end errors: {errs:?}");
    let proto = lumen::compiler::compile(&program).expect("compile ok");
    let buf = SharedBuf(Rc::new(RefCell::new(Vec::new())));
    let mut vm = Vm::with_output(Box::new(buf.clone()));
    lumen::stdlib::install(&mut vm);
    let result = vm.interpret(proto);
    let out = String::from_utf8(buf.0.borrow().clone()).unwrap();
    match result {
        Ok(()) => Ok(out),
        Err(msg) => Err(msg),
    }
}

/// Convenience: run and return stdout, asserting no uncaught error.
fn out(src: &str) -> String {
    run(src).unwrap_or_else(|e| panic!("unexpected runtime error:\n{e}"))
}

#[test]
fn arithmetic_and_number_semantics() {
    assert_eq!(out("println(1 + 2 * 3);"), "7\n");
    assert_eq!(out("println(7 / 2);"), "3\n"); // int division truncates
    assert_eq!(out("println(7.0 / 2);"), "3.5\n"); // float division
    assert_eq!(out("println(7 % 3);"), "1\n");
    assert_eq!(out("println(2 * 3.0);"), "6.0\n"); // mixed promotes to float
    assert_eq!(out("println(1 == 1.0);"), "true\n");
    assert_eq!(out("println(-5);"), "-5\n");
}

#[test]
fn truthiness_and_logical() {
    assert_eq!(out("println(0 || \"x\");"), "0\n"); // 0 is truthy
    assert_eq!(out("println(nil || \"x\");"), "x\n");
    assert_eq!(out("println(false && \"x\");"), "false\n");
    assert_eq!(out("println(\"a\" && \"b\");"), "b\n");
    assert_eq!(out("println(!nil);"), "true\n");
    assert_eq!(out("println(!0);"), "false\n");
}

#[test]
fn control_flow() {
    assert_eq!(out("if 1 < 2 { println(\"yes\"); } else { println(\"no\"); }"), "yes\n");
    assert_eq!(out("let s = 0; for let i = 1; i <= 5; i = i + 1 { s = s + i; } println(s);"), "15\n");
    assert_eq!(out("let s = 0; for x in [1,2,3,4] { s = s + x; } println(s);"), "10\n");
    assert_eq!(out("let i = 0; while i < 3 { print(i); i = i + 1; } println(\"\");"), "012\n");
}

#[test]
fn break_and_continue() {
    assert_eq!(
        out("for let i = 0; i < 10; i = i + 1 { if i == 5 { break; } print(i); } println(\"\");"),
        "01234\n"
    );
    assert_eq!(
        out("for let i = 0; i < 6; i = i + 1 { if i % 2 == 0 { continue; } print(i); } println(\"\");"),
        "135\n"
    );
}

#[test]
fn functions_and_recursion() {
    let src = "fn fib(n) { if n < 2 { return n; } return fib(n-1) + fib(n-2); } println(fib(20));";
    assert_eq!(out(src), "6765\n");
}

#[test]
fn closures_capture_by_reference() {
    let src = "fn counter() { let n = 0; return fn() { n = n + 1; return n; }; }
               let c = counter(); print(c()); print(c()); println(c());";
    assert_eq!(out(src), "123\n");
}

#[test]
fn per_iteration_capture() {
    let src = "let fs = []; for let i = 0; i < 3; i = i + 1 { push(fs, fn() { return i; }); }
               println(\"${fs[0]()}${fs[1]()}${fs[2]()}\");";
    assert_eq!(out(src), "012\n");
}

#[test]
fn classes_inheritance_super() {
    let src = "class A { greet() { return \"A\"; } }
               class B < A { greet() { return super.greet() + \"B\"; } }
               println(B().greet());";
    assert_eq!(out(src), "AB\n");
}

#[test]
fn instances_fields_and_init() {
    let src = "class Point { init(x, y) { this.x = x; this.y = y; } sum() { return this.x + this.y; } }
               let p = Point(3, 4); println(p.sum()); println(p.x); println(p.missing);";
    assert_eq!(out(src), "7\n3\nnil\n");
}

#[test]
fn custom_str_method_used_in_interpolation() {
    let src = "class Box { init(v) { this.v = v; } str() { return \"Box(${this.v})\"; } }
               println(\"${Box(42)}\");";
    assert_eq!(out(src), "Box(42)\n");
}

#[test]
fn arrays_and_maps() {
    assert_eq!(out("let a = [1,2,3]; push(a, 4); println(a);"), "[1, 2, 3, 4]\n");
    assert_eq!(out("println([1,2] + [3,4]);"), "[1, 2, 3, 4]\n");
    assert_eq!(out("let m = {x: 1}; m[\"y\"] = 2; println(m[\"x\"] + m[\"y\"]);"), "3\n");
    assert_eq!(out("println([10,20,30][-1]);"), "30\n");
    assert_eq!(out("println(\"abc\"[1]);"), "b\n");
}

#[test]
fn string_concatenation_and_interpolation() {
    assert_eq!(out("println(\"a\" + \"b\" + \"c\");"), "abc\n");
    assert_eq!(out("let n = 5; println(\"n=${n}, n*2=${n*2}\");"), "n=5, n*2=10\n");
}

#[test]
fn exceptions_try_catch_finally() {
    let src = "try { throw \"boom\"; } catch (e) { println(\"caught: ${e}\"); }";
    assert_eq!(out(src), "caught: boom\n");

    let src2 = "fn f() { try { return \"a\"; } finally { println(\"cleanup\"); } }
                println(f());";
    assert_eq!(out(src2), "cleanup\na\n");
}

#[test]
fn builtin_runtime_errors_have_kind() {
    let src = "try { let a = [1]; print(a[5]); } catch (e) { println(e.kind); }";
    assert_eq!(out(src), "IndexError\n");

    let src2 = "try { print(1 / 0); } catch (e) { println(e.kind); }";
    assert_eq!(out(src2), "DivisionByZero\n");

    let src3 = "try { print(1 + \"x\"); } catch (e) { println(e.kind); }";
    assert_eq!(out(src3), "TypeError\n");
}

#[test]
fn pattern_matching() {
    let src = "fn describe(v) { return match v {
                   0 => \"zero\",
                   [a, b] => \"pair ${a} ${b}\",
                   [first, ..rest] => \"head ${first} rest ${rest}\",
                   {kind: k} => \"kind ${k}\",
                   n if n > 100 => \"big\",
                   _ => \"other\",
               }; }
               println(describe(0));
               println(describe([1, 2]));
               println(describe([9, 8, 7]));
               println(describe({kind: \"x\"}));
               println(describe(500));
               println(describe(3));";
    assert_eq!(
        out(src),
        "zero\npair 1 2\nhead 9 rest [8, 7]\nkind x\nbig\nother\n"
    );
}

#[test]
fn or_patterns() {
    let src = "fn size(n) { return match n {
                   0 => \"zero\",
                   1 | 2 | 3 => \"small\",
                   4 | 5 | 6 => \"medium\",
                   _ => \"large\",
               }; }
               for x in [0, 2, 3, 5, 9] { println(size(x)); }";
    assert_eq!(out(src), "zero\nsmall\nsmall\nmedium\nlarge\n");
    // Mixed literal types in one alternation.
    assert_eq!(
        out("let r = match nil { nil | false => \"falsy\", _ => \"other\" }; println(r);"),
        "falsy\n"
    );
    assert_eq!(
        out("let r = match false { nil | false => \"falsy\", _ => \"other\" }; println(r);"),
        "falsy\n"
    );
    // A binding inside an alternation is a static (resolver) error.
    let (_p, errs) = lumen::check_source("let r = match 5 { a | 2 => a, _ => 0 };");
    assert!(
        errs.iter().any(|d| d.message.contains("alternative patterns")),
        "got: {errs:?}"
    );
}

#[test]
fn tail_call_optimization() {
    // Deep tail recursion runs in constant stack space (no StackOverflow).
    assert_eq!(
        out("fn loop(n) { if n == 0 { return \"done\"; } return loop(n - 1); } println(loop(1000000));"),
        "done\n"
    );
    // Tail recursion with an accumulator.
    assert_eq!(
        out("fn sum(n, a) { if n == 0 { return a; } return sum(n - 1, a + n); } println(sum(100000, 0));"),
        "5000050000\n"
    );
    // Mutual tail recursion.
    let mutual = "fn ev(n) { if n == 0 { return true; } return od(n - 1); } \
                  fn od(n) { if n == 0 { return false; } return ev(n - 1); }";
    assert_eq!(out(&format!("{mutual} println(ev(400000));")), "true\n");
    // Tail call to a method.
    assert_eq!(
        out("class C { d(n) { if n == 0 { return 0; } return this.d(n - 1); } } println(C().d(200000));"),
        "0\n"
    );
    // A non-tail recursion still overflows gracefully (catchable), not a crash.
    let e = run("fn deep(n) { if n == 0 { return 0; } return 1 + deep(n - 1); } deep(100000);").unwrap_err();
    assert!(e.contains("StackOverflow"), "got: {e}");
    // `finally` suppresses TCO so the finally still runs.
    let fin = "fn h(n) { if n == 0 { return 0; } try { return h(n - 1); } finally { if n == 2 { print(\"f\"); } } } println(h(4));";
    assert_eq!(out(fin), "f0\n");
}

#[test]
fn generators() {
    // for-in drives a finite generator lazily.
    let g = "fn up(n) { let i = 0; while i < n { yield i; i = i + 1; } } ";
    assert_eq!(
        out(&format!("{g} let s = \"\"; for x in up(4) {{ s = s + \"${{x}}\"; }} println(s);")),
        "0123\n"
    );
    // next() steps manually and returns nil when exhausted.
    assert_eq!(out(&format!("{g} let it = up(2); println(\"${{next(it)}} ${{next(it)}} ${{next(it)}}\");")), "0 1 nil\n");
    // type() of a generator.
    assert_eq!(out(&format!("{g} println(type(up(1)));")), "generator\n");
    // An infinite generator, consumed with a break — must not loop forever / OOM.
    let inf = "fn nats() { let i = 0; while true { yield i; i = i + 1; } } ";
    let take5 = "let c = 0; let acc = 0; for x in nats() { acc = acc + x; c = c + 1; if c == 5 { break; } } println(acc);";
    assert_eq!(out(&format!("{inf} {take5}")), "10\n"); // 0+1+2+3+4
    // try/catch and finally survive a yield (handlers travel with the context).
    let tc = "fn gv() { try { yield 1; throw \"x\"; } catch (e) { yield 2; } } let r = []; for v in gv() { push(r, v); } println(r);";
    assert_eq!(out(tc), "[1, 2]\n");
    // yield outside a function is a static error.
    let (_p, errs) = lumen::check_source("yield 1;");
    assert!(errs.iter().any(|d| d.message.contains("yield")), "got: {errs:?}");
}

#[test]
fn typed_catch() {
    // A typed clause matches by e.kind; a later bare clause catches the rest.
    let prog = |body: &str| {
        format!(
            "fn run(t) {{ try {{ t(); return \"ok\"; }} \
             catch (IndexError e) {{ return \"index\"; }} \
             catch (DivisionByZero e) {{ return \"div\"; }} \
             catch (e) {{ return \"other\"; }} }} {body}"
        )
    };
    assert_eq!(out(&prog("println(run(fn() { let a = [1]; return a[9]; }));")), "index\n");
    assert_eq!(out(&prog("println(run(fn() { return 1 / 0; }));")), "div\n");
    assert_eq!(out(&prog("println(run(fn() { return nil.x.y; }));")), "other\n");
    assert_eq!(out(&prog("println(run(fn() { return 7; }));")), "ok\n");
    // No matching typed clause and no bare clause -> re-raise to the outer try.
    let reraise = "fn outer() { try { try { let a = [1]; let z = a[9]; } \
                   catch (DivisionByZero e) { return \"inner\"; } } \
                   catch (e) { return e.kind; } } println(outer());";
    assert_eq!(out(reraise), "IndexError\n");
    // finally runs even when the value re-raises past the typed clauses.
    let fin = "fn f() { try { try { throw \"boom\"; } \
               catch (IndexError e) { return \"wrong\"; } \
               finally { print(\"fin \"); } } catch (e) { return \"caught\"; } } println(f());";
    assert_eq!(out(fin), "fin caught\n");
    // A bare catch before a typed one makes the typed one unreachable (warning).
    let (_p, diags) = lumen::check_all("let g = 1; try { g; } catch (e) {} catch (IndexError x) {}");
    assert!(diags.iter().any(|d| d.message.contains("unreachable catch")), "got: {diags:?}");
}

#[test]
fn static_methods_and_fields() {
    // Field initializers run per-instance before init; C() == 0.
    assert_eq!(out("class C { count = 0; } println(C().count);"), "0\n");
    // Static method on the class itself.
    assert_eq!(out("class C { static make() { return C(); } } println(type(C.make()));"), "C\n");
    assert_eq!(out("class C { static answer() { return 42; } } println(C.answer());"), "42\n");
    // Multiple fields, and a field default that is a string.
    assert_eq!(
        out("class P { x = 1; y = 2; } let p = P(); println(\"${p.x},${p.y}\");"),
        "1,2\n"
    );
    // User init runs after field inits (and can override them).
    assert_eq!(
        out("class P { x = 0; init(v) { this.x = v; } } println(P(9).x);"),
        "9\n"
    );
    // A field with no initializer defaults to nil.
    assert_eq!(out("class C { v; } println(C().v);"), "nil\n");
    // Static methods are inherited.
    assert_eq!(
        out("class A { static who() { return \"a\"; } } class B < A {} println(B.who());"),
        "a\n"
    );
    // `this` in a static method is a resolver error.
    let (_p, errs) = lumen::check_source("class C { static bad() { return this.x; } }");
    assert!(errs.iter().any(|d| d.message.contains("this")), "got: {errs:?}");
    // A missing static is a NameError at runtime.
    let e = run("class C {} C.nope();").unwrap_err();
    assert!(e.contains("static") || e.contains("NameError"), "got: {e}");
}

#[test]
fn operator_overloading() {
    let vec2 = "class V {
        init(x, y) { this.x = x; this.y = y; }
        __add__(o) { return V(this.x + o.x, this.y + o.y); }
        __sub__(o) { return V(this.x - o.x, this.y - o.y); }
        __mul__(k) { return V(this.x * k, this.y * k); }
        __neg__() { return V(-this.x, -this.y); }
        __eq__(o) { return this.x == o.x and this.y == o.y; }
        __lt__(o) { return this.x * this.x + this.y * this.y < o.x * o.x + o.y * o.y; }
        __index__(i) { if i == 0 { return this.x; } return this.y; }
        __set_index__(i, v) { if i == 0 { this.x = v; } else { this.y = v; } }
        str() { return \"(${this.x},${this.y})\"; }
    } ";
    let p = |body: &str| format!("{vec2} {body}");
    assert_eq!(out(&p("println(V(1, 2) + V(3, 4));")), "(4,6)\n");
    assert_eq!(out(&p("println(V(3, 4) - V(1, 1));")), "(2,3)\n");
    assert_eq!(out(&p("println(V(1, 2) * 3);")), "(3,6)\n");
    assert_eq!(out(&p("println(-V(1, 2));")), "(-1,-2)\n");
    assert_eq!(out(&p("println(V(1, 2) == V(1, 2));")), "true\n");
    assert_eq!(out(&p("println(V(1, 2) == V(9, 9));")), "false\n");
    assert_eq!(out(&p("println(V(1, 2) != V(9, 9));")), "true\n");
    assert_eq!(out(&p("println(V(1, 1) < V(3, 3));")), "true\n");
    assert_eq!(out(&p("println(V(3, 3) < V(1, 1));")), "false\n");
    assert_eq!(out(&p("println(V(3, 3) > V(1, 1));")), "true\n");
    assert_eq!(out(&p("println(V(1, 1) <= V(1, 1));")), "true\n");
    assert_eq!(out(&p("let v = V(7, 8); println(\"${v[0]} ${v[1]}\");")), "7 8\n");
    assert_eq!(out(&p("let v = V(0, 0); v[0] = 5; v[1] = 6; println(v);")), "(5,6)\n");
    // Absent dunder keeps the built-in TypeError.
    let e = run(&p("V(1, 2) / 2;")).unwrap_err();
    assert!(e.contains("TypeError") || e.contains("'/'"), "got: {e}");
}

#[test]
fn higher_order_native_callback() {
    // `sort`/comparators arrive in Phase 7; here verify a closure passed to a
    // user function and invoked works (the call_and_run path is exercised by
    // str() above; this checks plain higher-order user code).
    let src = "fn apply(f, x) { return f(x); } println(apply(fn(n) { return n * n; }, 7));";
    assert_eq!(out(src), "49\n");
}

#[test]
fn uncaught_throw_reports_trace() {
    let err = run("fn boom() { throw \"x\"; } boom();").unwrap_err();
    assert!(err.contains("Uncaught"));
    assert!(err.contains("fn boom"));
    assert!(err.contains("Stack trace"));
}

#[test]
fn arity_mismatch_throws() {
    let err = run("fn f(a, b) { return a; } f(1);").unwrap_err();
    assert!(err.contains("ArityError") || err.contains("expects 2"));
}

#[test]
fn deep_recursion_is_stack_overflow_not_crash() {
    // Non-tail recursion (the `n +` keeps it from being tail-call optimized,
    // DESIGN D30) overflows the call stack with a catchable error, not a crash.
    let err = run("fn rec(n) { return n + rec(n + 1); } rec(0);").unwrap_err();
    assert!(err.contains("StackOverflow") || err.contains("stack overflow"));
}

#[test]
fn integer_overflow_throws() {
    let err = run("println(9223372036854775807 + 1);").unwrap_err();
    assert!(err.contains("overflow"));
}
