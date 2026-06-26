//! Resolver *warnings* (non-fatal, surfaced via `check_all`): unused local
//! variables, unreachable code, and provably-wrong-arity calls. Warnings never
//! fail `check_source` (the hard gate) — these tests use `check_all`.

use lumen::Severity;

fn warns(src: &str) -> Vec<String> {
    let (_p, diags) = lumen::check_all(src);
    diags
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| d.message.clone())
        .collect()
}

fn has_warning(src: &str, needle: &str) -> bool {
    warns(src).iter().any(|m| m.contains(needle))
}

fn no_warnings(src: &str) -> bool {
    warns(src).is_empty()
}

#[test]
fn unused_local_warns() {
    assert!(has_warning("fn f() { let x = 1; return 2; }", "unused variable 'x'"));
    // Read -> no warning.
    assert!(no_warnings("fn f() { let x = 1; return x; }"));
    // Underscore opt-out.
    assert!(no_warnings("fn f() { let _x = 1; return 2; }"));
    // Parameters are never flagged.
    assert!(no_warnings("fn f(a, b) { return a; }"));
    // Captured by a closure counts as read.
    assert!(no_warnings("fn f() { let x = 1; return fn() { return x; }; }"));
    // Top-level `let` is a global, not a tracked local — not flagged.
    assert!(no_warnings("let x = 1;"));
    // Loop variables are not flagged even if unused in the body.
    assert!(no_warnings("fn f() { for i in [1, 2] { print(0); } }"));
}

#[test]
fn unreachable_code_warns() {
    assert!(has_warning("fn f() { return 1; print(2); }", "unreachable"));
    assert!(has_warning("fn f() { throw \"e\"; print(2); }", "unreachable"));
    assert!(has_warning("while true { break; print(2); }", "unreachable"));
    assert!(has_warning("while true { continue; print(2); }", "unreachable"));
    // Warn once per block (only the first statement after the terminator).
    assert_eq!(
        warns("fn f() { return 1; print(2); print(3); }")
            .iter()
            .filter(|m| m.contains("unreachable"))
            .count(),
        1
    );
    // A `return` inside an `if` branch does NOT make later code unreachable.
    assert!(!has_warning("fn f(c) { if c { return 1; } return 2; }", "unreachable"));
}

#[test]
fn wrong_arity_warns() {
    assert!(has_warning("fn f(a, b) { return a + b; } f(1);", "expects"));
    assert!(has_warning("fn f(a, b) { return a + b; } f(1, 2, 3);", "expects"));
    // Correct arity -> no warning.
    assert!(no_warnings("fn f(a, b) { return a + b; } f(1, 2);"));
    // Defaults: required arity is 1.
    assert!(no_warnings("fn f(a, b = 1) { return a + b; } f(1);"));
    assert!(has_warning("fn f(a, b = 1) { return a + b; } f();", "expects"));
    // Rest accepts any count at/above required.
    assert!(no_warnings("fn f(a, ..r) { return a; } f(1); f(1, 2, 3);"));
    // Builtins have no known signature — never checked.
    assert!(no_warnings("println(1, 2, 3);"));
    // A name shadowed by a local is not the global function — not checked.
    assert!(no_warnings("fn f(a) { return a; } fn g() { let f = 5; return f; } g();"));
}

#[test]
fn arity_skips_reassigned_global() {
    // A top-level fn reassigned to a different-arity function must NOT warn — at
    // the call site it holds the new value, so the original signature is stale.
    assert!(no_warnings("fn f(a, b) { return a + b; } f = fn(x) { return x; }; println(f(10));"));
    assert!(no_warnings("fn f(a, b) { return a; } f = fn() { return 99; }; println(f());"));
    // An un-reassigned function still gets checked.
    assert!(has_warning("fn f(a, b) { return a + b; } f(1);", "expects"));
}

#[test]
fn unreachable_after_top_level_throw_warns() {
    // The script body is a block too: a statement after a top-level `throw` is
    // unreachable, consistent with `throw` inside any other block.
    assert!(has_warning("throw \"halt\"; let dead = 1;", "unreachable"));
}
