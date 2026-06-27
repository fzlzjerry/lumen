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
        return Err(errs
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
            .join("; "));
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
    assert_eq!(
        out("fn f(a, b = 10) { return a + b; } println(f(1)); println(f(1, 2));"),
        "11\n3\n"
    );
    // A default referencing an earlier parameter.
    assert_eq!(
        out("fn rect(w, h = w) { return w * h; } println(rect(5)); println(rect(5, 3));"),
        "25\n15\n"
    );
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
    assert_eq!(
        out("fn tag(label, ..xs) { return \"${label}: ${xs}\"; } println(tag(\"n\", 1, 2));"),
        "n: [1, 2]\n"
    );
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
    assert_eq!(
        out("fn f(a, ..r) { return len(r); } println(f(1)); println(f(1, 2, 3));"),
        "0\n2\n"
    );
}

#[test]
fn required_after_default_is_a_static_error() {
    let e = run("fn f(a = 1, b) { return a; }").unwrap_err();
    assert!(
        e.contains("cannot follow a parameter with a default"),
        "got: {e}"
    );
}

#[test]
fn destructuring_arrays() {
    assert_eq!(
        out("let [a, b, c] = [1, 2, 3]; println(\"${a}${b}${c}\");"),
        "123\n"
    );
    assert_eq!(
        out("let [first, ..rest] = [10, 20, 30]; println(\"${first} ${rest}\");"),
        "10 [20, 30]\n"
    );
    assert_eq!(out("let [_, mid, _] = [1, 2, 3]; println(mid);"), "2\n");
    assert_eq!(
        out("let [a, ..mid, z] = [1, 2, 3, 4, 5]; println(\"${a} ${mid} ${z}\");"),
        "1 [2, 3, 4] 5\n"
    );
}

#[test]
fn destructuring_maps() {
    assert_eq!(out("let {x, y} = {x: 1, y: 2}; println(x + y);"), "3\n");
    assert_eq!(out("let {name: n} = {name: \"Ada\"}; println(n);"), "Ada\n");
}

#[test]
fn destructuring_in_functions_and_from_returns() {
    assert_eq!(
        out("fn f(p) { let [x, y] = p; return x * y; } println(f([3, 4]));"),
        "12\n"
    );
    let src = "fn pair() { return [\"a\", \"b\"]; } let [p, q] = pair(); println(p); println(q);";
    assert_eq!(out(src), "a\nb\n");
}

#[test]
fn destructuring_rejects_nested_patterns() {
    let e = run("let [a, [b, c]] = [1, [2, 3]];").unwrap_err();
    assert!(e.contains("must be variable names"), "got: {e}");
}

#[test]
fn ternary_expression() {
    assert_eq!(out("println(true ? 1 : 2);"), "1\n");
    assert_eq!(out("println(false ? 1 : 2);"), "2\n");
    // Truthiness: only nil/false are falsy, so 0 and "" are truthy.
    assert_eq!(out("println(0 ? \"t\" : \"f\");"), "t\n");
    assert_eq!(out("println(nil ? \"t\" : \"f\");"), "f\n");
    // As a parenthesized subexpression of a larger expression.
    assert_eq!(
        out("let x = 5; println((x > 3 ? \"big\" : \"small\") + \"!\");"),
        "big!\n"
    );
    // Right-associative chaining: a ? b : c ? d : e  ==  a ? b : (c ? d : e).
    assert_eq!(
        out(
            "fn g(n) { return n < 0 ? \"neg\" : n == 0 ? \"zero\" : \"pos\"; }
             println(g(-1)); println(g(0)); println(g(7));"
        ),
        "neg\nzero\npos\n"
    );
    // Only the taken branch is evaluated (lazy): boom() must not run.
    assert_eq!(
        out("fn boom() { throw \"x\"; } println(true ? 42 : boom());"),
        "42\n"
    );
}

#[test]
fn compound_assignment() {
    // Local variable, each operator.
    assert_eq!(out("let x = 10; x += 5; println(x);"), "15\n");
    assert_eq!(out("let x = 10; x -= 3; x *= 2; println(x);"), "14\n");
    assert_eq!(out("let x = 17; x /= 5; println(x);"), "3\n"); // int division truncates
    assert_eq!(out("let x = 17; x %= 5; println(x);"), "2\n");
    // Global variable; `+=` on strings concatenates.
    assert_eq!(
        out("let s = \"a\"; s += \"b\"; s += \"c\"; println(s);"),
        "abc\n"
    );
    // Index target — array and map.
    assert_eq!(
        out("let a = [1, 2, 3]; a[1] += 10; println(a);"),
        "[1, 12, 3]\n"
    );
    assert_eq!(
        out("let m = {n: 5}; m[\"n\"] *= 4; println(m[\"n\"]);"),
        "20\n"
    );
    // Property target.
    assert_eq!(
        out("class C { init() { this.v = 1; } } let c = C(); c.v += 41; println(c.v);"),
        "42\n"
    );
    // The whole expression yields the new value.
    assert_eq!(out("let x = 1; println(x += 9);"), "10\n");
    // Single-evaluation: the index expression runs exactly once.
    let src = "let calls = [0]; let a = [0, 0, 0];
               fn idx() { calls[0] = calls[0] + 1; return 1; }
               a[idx()] += 100;
               println(a[1]); println(calls[0]);";
    assert_eq!(out(src), "100\n1\n");
    // Robust when nested in a larger expression: the index target compiles with
    // top-relative ops, so pending stack temporaries don't corrupt it.
    assert_eq!(
        out("let a = [10, 20, 30]; println(a[1] += 5); println(a[1]);"),
        "25\n25\n"
    );
    assert_eq!(
        out("let b = [0, 0]; let z = (b[0] += 7) + 100; println(z); println(b[0]);"),
        "107\n7\n"
    );
    // Reassigning a const is a static error.
    let e = run("const x = 1; x += 1;").unwrap_err();
    assert!(e.contains("cannot assign to constant"), "got: {e}");
}

#[test]
fn lambda_shorthand() {
    // Single bare parameter; paren list; zero params.
    assert_eq!(out("let sq = x => x * x; println(sq(5));"), "25\n");
    assert_eq!(out("let add = (a, b) => a + b; println(add(3, 4));"), "7\n");
    assert_eq!(out("let answer = () => 42; println(answer());"), "42\n");
    // As higher-order arguments.
    assert_eq!(
        out("import \"array\" as a; println(a.map([1,2,3], x => x * 10));"),
        "[10, 20, 30]\n"
    );
    assert_eq!(
        out("import \"array\" as a; println(a.reduce([1,2,3,4], (acc, x) => acc + x, 0));"),
        "10\n"
    );
    // Curried (right-associative body).
    assert_eq!(out("let add = x => y => x + y; println(add(3)(4));"), "7\n");
    // Closure capture works (arrows are ordinary closures).
    assert_eq!(
        out("fn adder(n) { return x => x + n; } let inc = adder(1); println(inc(10));"),
        "11\n"
    );
    // Body may be any expression — a ternary, a map literal.
    assert_eq!(
        out("let s = n => n < 0 ? \"neg\" : \"pos\"; println(s(-2)); println(s(2));"),
        "neg\npos\n"
    );
    assert_eq!(
        out("let mk = v => {val: v}; println(mk(7)[\"val\"]);"),
        "7\n"
    );
    // Default parameters work (reuses the `fn` parameter parser).
    assert_eq!(
        out("let f = (a, b = 10) => a + b; println(f(1)); println(f(1, 2));"),
        "11\n3\n"
    );
    // Grouping is NOT mistaken for a lambda.
    assert_eq!(out("println((1 + 2) * 3);"), "9\n");
    // An arrow as a match-arm body (the arm `=>` is distinct from the arrow `=>`).
    assert_eq!(
        out("let g = match 1 { 1 => x => x + 100, _ => x => x }; println(g(5));"),
        "105\n"
    );
    // The formatter canonicalizes arrows to the `fn` form (still idempotent).
    let (p1, e1) = lumen::parse_source("let f = x => x + 1;");
    assert!(e1.is_empty(), "{e1:?}");
    let printed = lumen::ast_printer::print_program(&p1);
    assert!(
        printed.contains("fn(x)") && printed.contains("return x + 1"),
        "got: {printed}"
    );
}

#[test]
fn bitwise_operators() {
    // Basic binary ops and unary complement.
    assert_eq!(out("println(5 & 3);"), "1\n");
    assert_eq!(out("println(5 | 2);"), "7\n");
    assert_eq!(out("println(5 ^ 1);"), "4\n");
    assert_eq!(out("println(~0);"), "-1\n");
    assert_eq!(out("println(~5);"), "-6\n");
    // Shifts (`>>` is arithmetic / sign-extending on signed ints).
    assert_eq!(out("println(1 << 4);"), "16\n");
    assert_eq!(out("println(256 >> 2);"), "64\n");
    assert_eq!(out("println((0 - 8) >> 1);"), "-4\n");
    assert_eq!(out("println(1 << 63);"), "-9223372036854775808\n"); // wraps, no error
                                                                    // Precedence (Lua/Python style): bitwise binds tighter than comparison;
                                                                    // additive binds tighter than shift.
    assert_eq!(out("println(1 & 3 | 4);"), "5\n"); // (1 & 3) | 4 = 1 | 4
    assert_eq!(out("println(1 << 2 + 1);"), "8\n"); // 1 << (2 + 1)
    assert_eq!(out("println(1 & 1 == 1);"), "true\n"); // (1 & 1) == 1, NOT a type error
                                                       // Logical &&/|| still lex correctly after repurposing &/|.
    assert_eq!(out("println(true && false);"), "false\n");
    assert_eq!(out("println(true || false);"), "true\n");
}

#[test]
fn bitwise_errors() {
    // Integer-only: float operands throw a TypeError.
    assert!(run("println(1 & 2.0);").is_err());
    assert!(run("println(~3.0);").is_err());
    // Shift amount must be in 0..=63.
    assert!(run("println(1 << 64);").is_err());
    assert!(run("println(1 << (0 - 1));").is_err());
}

#[test]
fn super_invoke() {
    // super.method(args) calls the superclass method with `this` as the receiver.
    // Exercises 0-arg and 1-arg super calls, and dynamic dispatch of `this.speak()`
    // (called from the superclass method) back to the subclass override.
    let src = "class Animal {
                   init(name) { this.name = name; }
                   speak() { return this.name + \" makes a sound\"; }
                   describe(prefix) { return prefix + \": \" + this.speak(); }
               }
               class Dog < Animal {
                   speak() { return super.speak() + \" (woof)\"; }
                   describe(prefix) { return \"Dog \" + super.describe(prefix); }
               }
               let d = Dog(\"Rex\");
               println(d.speak());
               println(d.describe(\"info\"));";
    assert_eq!(
        out(src),
        "Rex makes a sound (woof)\nDog info: Rex makes a sound (woof)\n"
    );
    // Calling an undefined superclass method throws a NameError.
    let e = run("class A { greet() { return 1; } }
                 class B < A { f() { return super.missing(); } }
                 B().f();")
    .unwrap_err();
    assert!(
        e.contains("undefined method 'missing' in superclass"),
        "got: {e}"
    );
}

#[test]
fn spread_call_arguments() {
    // Spread an array into a fixed-arity function.
    assert_eq!(
        out("fn add(a, b, c) { return a + b + c; } println(add(..[1, 2, 3]));"),
        "6\n"
    );
    // Spread into a rest parameter, and mix spread with positional args.
    let rest = "fn total(..ns) { let t = 0; for n in ns { t = t + n; } return t; }";
    assert_eq!(
        out(&format!("{rest} println(total(..[1, 2, 3, 4]));")),
        "10\n"
    );
    assert_eq!(
        out(&format!("{rest} println(total(1, ..[2, 3], 4));")),
        "10\n"
    );
    // Spread cooperates with default parameters.
    let greet = "fn greet(name, g = \"Hello\") { return \"${g}, ${name}\"; }";
    assert_eq!(
        out(&format!("{greet} println(greet(..[\"Ada\"]));")),
        "Hello, Ada\n"
    );
    assert_eq!(
        out(&format!("{greet} println(greet(..[\"Bob\", \"Hi\"]));")),
        "Hi, Bob\n"
    );
    // Spread on a method call.
    let cls = "class S { init(b) { this.b = b; } of(a, c) { return this.b + a + c; } }";
    assert_eq!(
        out(&format!("{cls} let s = S(10); println(s.of(..[2, 3]));")),
        "15\n"
    );
    // Empty spread of an empty array calls with no args.
    assert_eq!(out("fn z() { return 7; } println(z(..[]));"), "7\n");
    // A wrong runtime arity still throws ArityError.
    let e = run("fn two(a, b) { return a; } two(..[1, 2, 3]);").unwrap_err();
    assert!(
        e.contains("argument") || e.to_lowercase().contains("arity"),
        "got: {e}"
    );
}

#[test]
fn destructuring_assignment() {
    // Swap via array destructuring assignment to existing variables.
    assert_eq!(
        out("let a = 1; let b = 2; [a, b] = [b, a]; println(\"${a} ${b}\");"),
        "2 1\n"
    );
    // Rest binding on the left.
    assert_eq!(
        out("let h = 0; let t = []; [h, ..t] = [1, 2, 3, 4]; println(\"${h} ${t}\");"),
        "1 [2, 3, 4]\n"
    );
    // Map destructuring assignment and a wildcard skip.
    assert_eq!(
        out("let x = 0; let y = 0; {x, y} = {x: 7, y: 9}; println(\"${x} ${y}\");"),
        "7 9\n"
    );
    assert_eq!(
        out("let p = 0; let q = 0; [p, _, q] = [1, 2, 3]; println(\"${p} ${q}\");"),
        "1 3\n"
    );
    // Assigns to upvalues, too.
    let up = "fn m() { let s = 0; let f = fn() { [s] = [42]; }; f(); return s; } println(m());";
    assert_eq!(out(up), "42\n");
    // Assigning a const target is a static error.
    let e = run("const c = 1; let d = 2; [c, d] = [3, 4];").unwrap_err();
    assert!(e.contains("constant"), "got: {e}");
    // A pattern with a literal value is rejected (clear error, not a misparse).
    let e2 = run("let z = 0; {k: 1} = {k: 2};").unwrap_err();
    assert!(!e2.is_empty());
    // A leading `{` that is really a block still parses as a block.
    assert_eq!(out("{ let inner = 5; println(inner); }"), "5\n");
}

#[test]
fn instance_reflection_and_is_operator() {
    // type() reports the class name for instances; primitives are unchanged.
    assert_eq!(out("class Foo {} println(type(Foo()));"), "Foo\n");
    assert_eq!(out("println(type(42));"), "int\n");
    assert_eq!(out("println(type(\"x\"));"), "string\n");
    assert_eq!(out("println(type([1]));"), "array\n");
    // `is` tests class membership, including across inheritance.
    assert_eq!(
        out("class Foo {} let f = Foo(); println(f is Foo);"),
        "true\n"
    );
    assert_eq!(
        out("class A {} class B < A {} println(B() is A);"),
        "true\n"
    );
    assert_eq!(out("class A {} class B {} println(A() is B);"), "false\n");
    assert_eq!(out("class A {} println(5 is A);"), "false\n");
    assert_eq!(out("class A {} println(nil is A);"), "false\n");
    // `is` binds tighter than `==`.
    assert_eq!(out("class A {} println((A() is A) == true);"), "true\n");
    // A non-class right operand is a TypeError.
    let e = run("class A {} A() is 5;").unwrap_err();
    assert!(e.contains("class"), "got: {e}");
}

#[test]
fn comprehensions() {
    // Array comprehension (the headline example).
    assert_eq!(
        out("let a = [x * 2 for x in range(3)]; println(a);"),
        "[0, 2, 4]\n"
    );
    // With a filter.
    assert_eq!(
        out("println([x for x in range(10) if x % 2 == 0]);"),
        "[0, 2, 4, 6, 8]\n"
    );
    // Works as a call argument (its own frame — DESIGN D31).
    assert_eq!(out("println(len([x for x in range(7)]));"), "7\n");
    // Map comprehension; a bare-ident key is the loop variable.
    assert_eq!(
        out("println({x: x * x for x in range(4)});"),
        "{0: 0, 1: 1, 2: 4, 3: 9}\n"
    );
    // Computed and string keys.
    assert_eq!(
        out("println({[x + 10]: x for x in range(2)});"),
        "{10: 0, 11: 1}\n"
    );
    // Captures an outer variable.
    assert_eq!(
        out("let k = 10; println([x * k for x in range(3)]);"),
        "[0, 10, 20]\n"
    );
    // `this` inside a comprehension in a method.
    let cls = "class S { init(k) { this.k = k; } f(xs) { return [x * this.k for x in xs]; } }";
    assert_eq!(
        out(&format!("{cls} println(S(3).f([1, 2, 3]));")),
        "[3, 6, 9]\n"
    );
    // Over a string.
    assert_eq!(
        out("println([c for c in \"abc\"]);"),
        "[\"a\", \"b\", \"c\"]\n"
    );
}

#[test]
fn round_radix_and_octal() {
    // math.round(x, ndigits) rounds to decimals (float); round(x) -> int.
    assert_eq!(
        out("import \"math\" as m; println(m.round(3.14159, 2));"),
        "3.14\n"
    );
    assert_eq!(
        out("import \"math\" as m; println(m.round(123.456, 1));"),
        "123.5\n"
    );
    assert_eq!(out("import \"math\" as m; println(m.round(3.7));"), "4\n");
    // int(s, base) parses in radix 2..=36.
    assert_eq!(out("println(int(\"FF\", 16));"), "255\n");
    assert_eq!(out("println(int(\"101\", 2));"), "5\n");
    assert_eq!(out("println(int(\"zz\", 36));"), "1295\n");
    assert_eq!(out("println(int(\"-1a\", 16));"), "-26\n");
    assert_eq!(out("println(int(\"42\"));"), "42\n"); // 1-arg still works
    let e = run("int(\"xyz\", 16);").unwrap_err();
    assert!(e.contains("ValueError"), "got: {e}");
    let e2 = run("int(\"10\", 99);").unwrap_err();
    assert!(e2.contains("ValueError"), "got: {e2}");
    // 0o octal literals.
    assert_eq!(out("println(0o17);"), "15\n");
    assert_eq!(out("println(0o755);"), "493\n");
    assert_eq!(out("println(0O10);"), "8\n");
}

#[test]
fn string_repeat() {
    assert_eq!(out("println(\"ab\" * 3);"), "ababab\n");
    assert_eq!(out("println(3 * \"ab\");"), "ababab\n"); // commutative
    assert_eq!(out("println(\"x\" * 0);"), "\n"); // empty
    assert_eq!(out("println(\"x\" * -2);"), "\n"); // negative -> empty
    assert_eq!(out("println(\"=\" * 5);"), "=====\n");
    // int * int is unaffected; string * string is still a TypeError.
    assert_eq!(out("println(3 * 4);"), "12\n");
    let e = run("\"a\" * \"b\";").unwrap_err();
    assert!(e.contains("TypeError"), "got: {e}");
}

#[test]
fn power_operator() {
    assert_eq!(out("println(2 ** 10);"), "1024\n");
    // Right-associative: 2 ** (3 ** 2) = 2 ** 9.
    assert_eq!(out("println(2 ** 3 ** 2);"), "512\n");
    // Binds above unary minus: -(2 ** 2).
    assert_eq!(out("println(-2 ** 2);"), "-4\n");
    assert_eq!(out("println((-2) ** 2);"), "4\n");
    // Negative exponent and float base yield floats.
    assert_eq!(out("println(2 ** -1);"), "0.5\n");
    assert_eq!(out("println(2.0 ** 3);"), "8.0\n");
    // Binds tighter than `*`.
    assert_eq!(out("println(2 ** 3 * 4);"), "32\n");
    assert_eq!(out("println(10 ** 0);"), "1\n");
    // Integer overflow throws (like `*`).
    let e = run("2 ** 1000;").unwrap_err();
    assert!(
        e.contains("overflow") || e.contains("ValueError"),
        "got: {e}"
    );
}

#[test]
fn formatter_roundtrips_new_features() {
    // Parse -> print -> parse must be stable for the new syntax.
    let srcs = [
        "fn f(a, b = 10, ..rest) { return a; }",
        "f(..xs);",
        "f(1, ..xs, 2);",
        "obj.m(..args);",
        "let r = x is Foo;",
        "let s = (a is B) == c;",
        "[a, b] = [b, a];",
        "[x, ..rest] = xs;",
        "{k} = m;",
        "let m = match x { 1 | 2 | 3 => \"a\", _ => \"b\" };",
        "try { f(); } catch (IndexError e) { a(); } catch (e) { b(); }",
        "class C { count = 0; static make() { return C(); } }",
        "fn gen(n) { let i = 0; while i < n { yield i; i = i + 1; } }",
        "let p = 2 ** 3 ** 2;",
        "let q = -2 ** 2;",
        "let r = (-2) ** 2;",
        "let s = foo()[0] ** 2 * 3;",
        "let ac = [x * 2 for x in range(3)];",
        "let af = [x for x in xs if x > 0];",
        "let mc = {k: k * k for k in range(4)};",
        "let mk = {[i + 1]: i for i in xs if i > 0};",
        "let [a, b, ..rest] = xs;",
        "let {x, y} = m;",
        "let g = fn(n = 5) { return n; };",
        "let t = a ? b : c;",
        "let u = p ? q : r ? s : v;",
        "x += 1;",
        "a[i] *= 2;",
        "o.f -= 3;",
        "let bw = 1 & 2 | 3 ^ 4;",
        "let sh = 1 << 2 >> 1;",
        "let bn = ~x & y;",
        "let pr = (a | b) & c;",
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
