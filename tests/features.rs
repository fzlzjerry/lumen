//! Tests for the Phase B language features: default parameters, rest
//! parameters, and destructuring `let`.

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

fn run(src: &str) -> Result<String, String> {
    let (program, errs) = lumen::check_source(src);
    if !errs.is_empty() {
        return Err(errs.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; "));
    }
    let proto = lumen::compiler::compile(&program).map_err(|_| "compile error".to_string())?;
    let buf = SharedBuf(Rc::new(RefCell::new(Vec::new())));
    let mut vm = Vm::with_output(Box::new(buf.clone()));
    lumen::stdlib::install(&mut vm);
    let result = vm.interpret(proto);
    let out = String::from_utf8(buf.0.borrow().clone()).unwrap();
    result.map(|()| out)
}

fn out(src: &str) -> String {
    run(src).unwrap_or_else(|e| panic!("error: {e}"))
}

#[test]
fn default_parameters() {
    assert_eq!(out("fn f(a, b = 10) { return a + b; } println(f(1)); println(f(1, 2));"), "11\n3\n");
    // A default referencing an earlier parameter.
    assert_eq!(out("fn rect(w, h = w) { return w * h; } println(rect(5)); println(rect(5, 3));"), "25\n15\n");
    // Multiple defaults, partial application.
    assert_eq!(
        out("fn g(a, b = 2, c = 3) { return \"${a},${b},${c}\"; } println(g(1)); println(g(1, 9)); println(g(1, 9, 8));"),
        "1,2,3\n1,9,3\n1,9,8\n"
    );
}

#[test]
fn rest_parameters() {
    assert_eq!(
        out("fn sum(..ns) { let t = 0; for n in ns { t = t + n; } return t; } println(sum()); println(sum(1,2,3,4));"),
        "0\n10\n"
    );
    assert_eq!(out("fn tag(label, ..xs) { return \"${label}: ${xs}\"; } println(tag(\"n\", 1, 2));"), "n: [1, 2]\n");
    // Default + rest together.
    assert_eq!(
        out("fn c(a, b = 1, ..r) { return \"${a} ${b} ${r}\"; } println(c(\"x\")); println(c(\"y\", 5, 8, 9));"),
        "x 1 []\ny 5 [8, 9]\n"
    );
}

#[test]
fn arity_errors_with_defaults_and_rest() {
    // Too few (below required).
    let e = run("fn f(a, b = 1) { return a; } f();").unwrap_err();
    assert!(e.contains("ArityError") || e.contains("expects"));
    // Too many (no rest).
    let e2 = run("fn f(a, b = 1) { return a; } f(1, 2, 3);").unwrap_err();
    assert!(e2.contains("ArityError") || e2.contains("expects"));
    // Rest accepts any number at/above required.
    assert_eq!(out("fn f(a, ..r) { return len(r); } println(f(1)); println(f(1, 2, 3));"), "0\n2\n");
}

#[test]
fn required_after_default_is_a_static_error() {
    let e = run("fn f(a = 1, b) { return a; }").unwrap_err();
    assert!(e.contains("cannot follow a parameter with a default"), "got: {e}");
}

#[test]
fn destructuring_arrays() {
    assert_eq!(out("let [a, b, c] = [1, 2, 3]; println(\"${a}${b}${c}\");"), "123\n");
    assert_eq!(out("let [first, ..rest] = [10, 20, 30]; println(\"${first} ${rest}\");"), "10 [20, 30]\n");
    assert_eq!(out("let [_, mid, _] = [1, 2, 3]; println(mid);"), "2\n");
    assert_eq!(out("let [a, ..mid, z] = [1, 2, 3, 4, 5]; println(\"${a} ${mid} ${z}\");"), "1 [2, 3, 4] 5\n");
}

#[test]
fn destructuring_maps() {
    assert_eq!(out("let {x, y} = {x: 1, y: 2}; println(x + y);"), "3\n");
    assert_eq!(out("let {name: n} = {name: \"Ada\"}; println(n);"), "Ada\n");
}

#[test]
fn destructuring_in_functions_and_from_returns() {
    assert_eq!(out("fn f(p) { let [x, y] = p; return x * y; } println(f([3, 4]));"), "12\n");
    let src = "fn pair() { return [\"a\", \"b\"]; } let [p, q] = pair(); println(p); println(q);";
    assert_eq!(out(src), "a\nb\n");
}

#[test]
fn destructuring_rejects_nested_patterns() {
    let e = run("let [a, [b, c]] = [1, [2, 3]];").unwrap_err();
    assert!(e.contains("must be variable names"), "got: {e}");
}

#[test]
fn formatter_roundtrips_new_features() {
    // Parse -> print -> parse must be stable for the new syntax.
    let srcs = [
        "fn f(a, b = 10, ..rest) { return a; }",
        "let [a, b, ..rest] = xs;",
        "let {x, y} = m;",
        "let g = fn(n = 5) { return n; };",
    ];
    for src in srcs {
        let (p1, e1) = lumen::parse_source(src);
        assert!(e1.is_empty(), "{src}: {e1:?}");
        let once = lumen::ast_printer::print_program(&p1);
        let (p2, e2) = lumen::parse_source(&once);
        assert!(e2.is_empty(), "reparse {once}: {e2:?}");
        let twice = lumen::ast_printer::print_program(&p2);
        assert_eq!(once, twice, "not idempotent for: {src}");
    }
}
