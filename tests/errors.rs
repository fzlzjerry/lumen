//! Error-message tests: one (or more) per error class, asserting the diagnostic
//! or thrown-error text. These pin the *quality* of error reporting — that each
//! failure mode is detected and described, with the right kind/stage.

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

/// Collect static (lex/parse/resolve) diagnostics for `src`.
fn static_errors(src: &str) -> Vec<String> {
    let (_p, errs) = lumen::check_source(src);
    errs.into_iter().map(|d| d.message).collect()
}

fn assert_static(src: &str, needle: &str) {
    let errs = static_errors(src);
    assert!(
        errs.iter().any(|m| m.contains(needle)),
        "expected a static error containing '{needle}', got {errs:?}"
    );
}

/// Run `src`; return the uncaught error message (or panic if it succeeds).
fn runtime_error(src: &str) -> String {
    let (program, errs) = lumen::check_source(src);
    assert!(errs.is_empty(), "unexpected static errors: {errs:?}");
    let proto = lumen::compiler::compile(&program).expect("compile");
    let buf = SharedBuf(Rc::new(RefCell::new(Vec::new())));
    let mut vm = Vm::with_output(Box::new(buf));
    lumen::stdlib::install(&mut vm);
    vm.interpret(proto).expect_err("expected a runtime error")
}

// ---- lexical errors --------------------------------------------------------

#[test]
fn lexical_errors() {
    assert_static("let s = \"oops", "unterminated string");
    assert_static("/* never closed", "unterminated block comment");
    assert_static("let n = 99999999999999999999999;", "does not fit");
    assert_static("let x = 1 @ 2;", "unexpected character");
    assert_static(r#"let s = "a\qb";"#, "unknown escape");
}

// ---- parser errors ---------------------------------------------------------

#[test]
fn parser_errors() {
    assert_static("let x = 1 let y = 2;", "expected ';'");
    assert_static("let x = ;", "expected an expression");
    assert_static("1 + 2 = 3;", "invalid assignment target");
    assert_static("let a = [1, 2;", "expected ']'");
    assert_static("fn f( { }", "expected a parameter name");
}

#[test]
fn parser_recovers_and_reports_many() {
    // Two independent broken statements both reported.
    let errs = static_errors("let = 1; let y 2;");
    assert!(errs.len() >= 2, "expected multiple errors, got {errs:?}");
}

// ---- resolver (semantic) errors -------------------------------------------

#[test]
fn resolver_errors() {
    assert_static("print(nope);", "undefined variable 'nope'");
    assert_static("const PI = 3; PI = 4;", "cannot assign to constant 'PI'");
    assert_static("break;", "'break' outside of a loop");
    assert_static("fn f() { continue; }", "'continue' outside of a loop");
    assert_static("return 1;", "'return' outside of a function");
    assert_static("print(this);", "'this' can only be used inside a method");
    assert_static(
        "class A { m() { return super.m(); } }",
        "'super' can only be used",
    );
    assert_static("class A < A { }", "cannot inherit from itself");
    assert_static(
        "fn f() { let x = 1; let x = 2; }",
        "already declared in this scope",
    );
    assert_static("let x = 1; let x = 2;", "already declared at the top level");
    assert_static(
        "fn f() { export let x = 1; }",
        "'export' is only allowed at the top level",
    );
    assert_static(
        "class A { init() { return 5; } }",
        "cannot return a value from an 'init'",
    );
}

// ---- runtime errors --------------------------------------------------------

/// Every built-in runtime error kind, observed via `e.kind` so the test is
/// precise about the classification.
fn caught_kind(body: &str) -> String {
    let src = format!("try {{ {body} }} catch (e) {{ println(e.kind); }}");
    let (program, errs) = lumen::check_source(&src);
    assert!(errs.is_empty(), "static errors: {errs:?}");
    let proto = lumen::compiler::compile(&program).expect("compile");
    let buf = SharedBuf(Rc::new(RefCell::new(Vec::new())));
    let mut vm = Vm::with_output(Box::new(buf.clone()));
    lumen::stdlib::install(&mut vm);
    vm.interpret(proto).expect("should be caught");
    let out = String::from_utf8(buf.0.borrow().clone()).unwrap();
    out.trim().to_string()
}

#[test]
fn runtime_error_kinds() {
    assert_eq!(caught_kind("print(1 + \"x\");"), "TypeError");
    assert_eq!(caught_kind("print([1,2][9]);"), "IndexError");
    assert_eq!(caught_kind("print(1 / 0);"), "DivisionByZero");
    assert_eq!(
        caught_kind("fn f(a) { return a; } f(1, 2, 3);"),
        "ArityError"
    );
    assert_eq!(caught_kind("print(9223372036854775807 + 1);"), "ValueError"); // overflow
    assert_eq!(caught_kind("let m = {}; m[[1,2]] = 3;"), "TypeError"); // unhashable key
}

#[test]
fn uncaught_runtime_errors_have_traces() {
    let msg = runtime_error("fn a() { b(); } fn b() { throw \"boom\"; } a();");
    assert!(msg.contains("Uncaught"));
    assert!(msg.contains("Stack trace"));
    assert!(msg.contains("fn b"));
    assert!(msg.contains("fn a"));

    // A *non-tail* recursion overflows the call stack. (`return rec();` would be
    // tail-call optimized into a constant-space infinite loop — DESIGN D30.)
    let so = runtime_error("fn rec() { return 1 + rec(); } rec();");
    assert!(so.contains("StackOverflow") || so.contains("stack overflow"));
}

#[test]
fn runtime_name_error_on_use_before_definition() {
    // `g` is a known global (declared below), so the resolver is happy; but it is
    // read at runtime before its `let` executes -> a runtime NameError.
    let msg = runtime_error("fn f() { return g; } f(); let g = 1;");
    assert!(msg.contains("NameError"), "got: {msg}");
}

#[test]
fn match_with_no_arm_throws() {
    let msg = runtime_error("let r = match 99 { 0 => \"z\", 1 => \"o\" };");
    assert!(msg.contains("no matching pattern"));
}
