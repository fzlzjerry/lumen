# The Lumen Tutorial

A guided tour of the language, from your first line to its advanced features.
Every snippet is real, runnable Lumen. Save one to `x.lum` and run it with:

```sh
lumen run x.lum
```

Or paste expressions into the REPL (`lumen repl`), where a trailing expression
prints its value.

---

## 1. Hello, world

```lumen
println("Hello, world!");
```

`println` prints its arguments and a newline; `print` omits the newline.
Statements end with a semicolon. Comments are `// line` or `/* block, /* nestable */ */`.

## 2. Values and variables

Lumen is dynamically typed. The primitive types are `int` (64-bit), `float`
(64-bit), `string`, `bool` (`true`/`false`), and `nil`.

```lumen
let count = 10;        // a mutable variable (int)
const PI = 3.14159;    // a constant — reassigning it is a compile error
let ratio = 3.0;       // float

let big = 1_000_000;   // underscores group digits
let hex = 0xFF;        // 255
let bin = 0b1010;      // 10
let sci = 1.5e3;       // 1500.0

println(type(count));  // "int"
println(type(ratio));  // "float"
```

Arithmetic follows two rules worth knowing:

```lumen
println(7 / 2);     // 3    — int / int truncates
println(7.0 / 2);   // 3.5  — any float operand gives float division
println(7 % 3);     // 1
println(2 * 3.0);   // 6.0  — mixing promotes to float
println(1 == 1.0);  // true — numbers compare across int/float
```

Division by zero throws, and integer overflow throws (it never silently wraps).

**Truthiness:** only `nil` and `false` are falsy. `0`, `0.0`, `""`, `[]`, and
`{}` are all *truthy* — so write `if x != 0`, not `if x`.

```lumen
println(!nil);      // true
println(bool(0));   // true
```

## 3. Strings and interpolation

Strings are immutable and indexed by character (Unicode scalar):

```lumen
let s = "Lumen";
println(len(s));    // 5
println(s[0]);      // "L"
println(s[-1]);     // "n"  — negative indices count from the end
```

`${expr}` embeds any expression, and interpolations nest:

```lumen
let name = "Ada";
let age = 36;
println("Hello, ${name}! Next year you'll be ${age + 1}.");
println("Nested: ${"inner ${age * 2}"}");
```

Escapes include `\n \t \r \\ \" \$` and `\u{...}`:

```lumen
println("tab:\tquote:\"\tunicode:\u{1F680}");
```

For heavy string building, collect parts and join them (it's O(n) instead of
O(n²)):

```lumen
import "string" as str;
println(str.join(["the", "quick", "brown", "fox"], " "));
```

## 4. Control flow

`if`/`else` (no parentheses around the condition), `while`, and two `for`
forms:

```lumen
fn classify(n) {
    if n < 0 {
        return "negative";
    } else if n == 0 {
        return "zero";
    } else {
        return "positive";
    }
}

let sum = 0;
for let i = 1; i <= 5; i = i + 1 {   // C-style for
    sum = sum + i;
}
println(sum);   // 15

for x in [10, 20, 30] {              // for-in over arrays, strings, maps, ranges
    print(x); print(" ");
}
println("");

let i = 0;
while i < 3 {
    if i == 1 { i = i + 1; continue; }
    print(i);
    i = i + 1;
}
println("");   // "02"
```

`range(n)`, `range(lo, hi)`, and `range(lo, hi, step)` produce arrays to iterate.

Two conveniences worth knowing early. The **conditional expression** `cond ? a : b`
is a compact `if`/`else` that *produces a value* — only the selected branch runs:

```lumen
fn sign(n) { return n < 0 ? "neg" : n == 0 ? "zero" : "pos"; }
println(sign(-3));   // neg
```

And **compound assignment** updates a variable, array element, or field in place:
`x += e` means `x = x + e` (likewise `-=`, `*=`, `/=`, `%=`), evaluating the
target exactly once:

```lumen
let total = 0;
for x in [10, 20, 30] { total += x; }
let counts = {a: 0};
counts["a"] += 5;
println(total);        // 60
println(counts["a"]);  // 5
```

Integers also support **bitwise** operators — `&` `|` `^` `~` and the shifts
`<<` `>>` (shift amounts must be `0..=63`); they bind tighter than comparison:

```lumen
println(5 & 3);        // 1
println(1 << 4);       // 16
println(~0);           // -1
```

## 5. Functions and closures

Functions are values. Top-level functions can call each other in any order:

```lumen
fn is_even(n) { if n == 0 { return true; } return is_odd(n - 1); }
fn is_odd(n)  { if n == 0 { return false; } return is_even(n - 1); }
println(is_even(10));   // true
```

Anonymous functions (lambdas) and higher-order functions:

```lumen
let inc = fn(x) { return x + 1; };
fn apply_twice(f, x) { return f(f(x)); }
println(apply_twice(inc, 5));   // 7
```

For a one-expression function there's the **arrow shorthand** `params => expr`,
which implicitly returns the expression. Use `x => …` for a single parameter and
`(a, b) => …` (or `() => …`) for any other arity:

```lumen
import "array" as a;
println(a.map([1, 2, 3], x => x * x));            // [1, 4, 9]
println(a.reduce([1, 2, 3, 4], (acc, x) => acc + x, 0)); // 10
let add = x => y => x + y;                          // curried
println(add(3)(4));                                 // 7
```

The arrow body is a single expression — for a block body (multiple statements),
use the `fn(x) { … }` form.

Closures capture variables by reference, so they can hold private state:

```lumen
fn make_counter() {
    let n = 0;
    return fn() { n = n + 1; return n; };
}
let c = make_counter();
println("${c()} ${c()} ${c()}");   // "1 2 3"
```

Each loop iteration is a fresh binding, so closures made in a loop capture *that*
iteration's value (like JavaScript `let`):

```lumen
let fs = [];
for let i = 0; i < 3; i = i + 1 { push(fs, fn() { return i; }); }
println("${fs[0]()} ${fs[1]()} ${fs[2]()}");   // "0 1 2"
```

A function can collect extra arguments with a **rest parameter** (`..name`), and
a caller can **spread** an iterable into the argument list with `..expr` — the
mirror of array-literal spread. They compose with each other and with defaults:

```lumen
fn total(..xs) { let t = 0; for x in xs { t = t + x; } return t; }
let nums = [1, 2, 3];
println(total(..nums));        // 6
println(total(1, ..nums, 4));  // 11  (mix spread with positional args)
```

## 6. Collections

**Arrays** are mutable, ordered, and heterogeneous:

```lumen
let xs = [1, 2, 3];
push(xs, 4);           // [1, 2, 3, 4]
let last = pop(xs);    // 4, xs is [1, 2, 3] again
xs[0] = 99;            // index assignment
let ys = [0, ..xs, 5]; // spread: [0, 99, 2, 3, 5]
println([1, 2] + [3]); // concatenation: [1, 2, 3]
```

**Maps** are insertion-ordered hash tables indexed with `["key"]`:

```lumen
let person = {
    name: "Ada",       // identifier key (sugar for "name")
    "age": 36,         // string key
    [1 + 1]: "two",    // computed key
};
person["role"] = "engineer";
println(person["name"]);     // "Ada"
println(keys(person));       // ["name", "age", 2, "role"]
println(has(person, "age")); // true
del(person, "age");
```

## 7. Classes and inheritance

Classes have an `init` constructor, methods, single inheritance with `<`, and
`super`. A `str()` method customizes how an instance prints:

```lumen
class Animal {
    init(name) { this.name = name; }
    speak() { return "${this.name} makes a sound"; }
    str() { return "Animal(${this.name})"; }
}

class Dog < Animal {
    init(name, breed) {
        super.init(name);
        this.breed = breed;
    }
    speak() { return "${super.speak()} — woof!"; }
}

let d = Dog("Rex", "Husky");
println(d.speak());     // "Rex makes a sound — woof!"
d.age = 3;              // fields are dynamic
println(d.age);         // 3
println(d.weight);      // nil — reading a missing field yields nil

let bark = d.speak;     // methods are first-class once bound to a receiver
println(bark());
```

For runtime reflection, `type(x)` returns an instance's class name, and the `is`
operator tests class membership (including inherited classes):

```lumen
println(type(d));        // "Dog"
println(d is Dog);       // true
println(d is Animal);    // true — Dog inherits Animal
println(d is Cat);       // false
```

A class body may also declare **fields** (initialized per-instance at the top of
the constructor) and **static methods** (called on the class, with no `this`).
Operators can be **overloaded** with dunder methods (`__add__`, `__eq__`,
`__index__`, …):

```lumen
class Counter {
    count = 0;                                    // field, defaults to 0
    static start() { return Counter(); }          // static factory
    bump() { this.count = this.count + 1; }
}
let c = Counter.start();
c.bump();
println(c.count);        // 1
```

## 8. Exceptions

`throw` any value; catch it with `try`/`catch`. `finally` always runs. The
`catch` clause is optional (a `try`/`finally` cleans up without swallowing).

```lumen
fn checked_div(a, b) {
    if b == 0 { throw "cannot divide ${a} by zero"; }
    return a / b;
}

try {
    println(checked_div(10, 0));
} catch (e) {
    println("caught: ${e}");
}
```

Runtime faults throw built-in **error objects** with `.kind` and `.message`:

```lumen
try {
    let xs = [1, 2, 3];
    println(xs[99]);
} catch (e) {
    println("${e.kind}: ${e.message}");   // "IndexError: index 99 ..."
}
```

The error kinds are `TypeError`, `NameError`, `ArityError`, `IndexError`,
`KeyError`, `DivisionByZero`, `ValueError`, `StackOverflow`, and
`AssertionError`. An uncaught throw prints a stack trace and exits with code 70.

## 9. Pattern matching

`match` is an expression. Arms are tried top to bottom; the first match wins.

```lumen
fn describe(v) {
    return match v {
        0          => "zero",
        [a, b]     => "pair: ${a}, ${b}",
        [x, ..xs]  => "head ${x}, tail ${xs}",
        {kind: k}  => "kind ${k}",
        n if n > 100 => "big",
        _          => "something else",
    };
}

println(describe(0));            // "zero"
println(describe([1, 2]));       // "pair: 1, 2"
println(describe([1, 2, 3]));    // "head 1, tail [2, 3]"
println(describe({kind: "x"}));  // "kind x"
println(describe(500));          // "big"
```

Patterns can be literals, a binding (`n`), a wildcard (`_`), arrays (with one
optional `..rest`), or maps (matching by key). An arm may have an `if` guard.

## 10. Modules

Split code across files and import by name. Only `export`ed names are visible.

`geometry.lum`:

```lumen
import "math" as math;

export fn circle_area(r) { return math.pi * r * r; }

export class Rect {
    init(w, h) { this.w = w; this.h = h; }
    area() { return this.w * this.h; }
}
```

`main.lum`:

```lumen
import "geometry" as geo;        // whole module under an alias
import "math".{sqrt, pow};       // selective import

println(geo.circle_area(2));
println(geo.Rect(3, 4).area());
println(sqrt(144));
```

Each module has its own global scope and runs once (results are cached), so a
module's functions always see *their* module's bindings.

## 11. The standard library

Beyond the global builtins (`print`, `len`, `push`, `range`, `str`, …), import a
native module:

```lumen
import "array" as arr;
import "string" as str;
import "json" as json;

println(arr.map([1, 2, 3], fn(x) { return x * x; }));   // [1, 4, 9]
println(arr.sort([3, 1, 2]));                            // [1, 2, 3]
println(str.upper("hi"));                                // "HI"

let data = json.parse("{\"a\": [1, 2, 3]}");
println(data["a"][1]);                                   // 2
println(json.stringify({x: 1, y: 2}));                   // {"x":1,"y":2}
```

The full set — `math`, `string`, `array`, `map`, `io`, `os`, `time`, `json`,
`random`, `hash`, `datetime`, `regex`, and the self-hosted `seq` and `path` — is
documented in [`API.md`](API.md).

## 12. The toolchain

```sh
lumen repl              # try things interactively
lumen fmt --write x.lum # canonical formatting
lumen disasm x.lum      # see the bytecode
lumen debug x.lum       # breakpoints, step, inspect locals/stack
```

Start a project:

```sh
lumen new myapp
cd myapp
lumen run               # runs src/main.lum
lumen test              # runs tests/*.lum (a test passes if it doesn't throw)
```

A test file just runs assertions:

```lumen
// tests/math_test.lum
assert(1 + 1 == 2, "addition works");
import "array" as a;
assert(a.sum([1, 2, 3]) == 6, "sum works");
println("all good");
```

---

That's the whole language. For the precise rules, read [`SPEC.md`](SPEC.md); for
every library function, [`API.md`](API.md); for how it's all built,
[`JOURNAL.md`](JOURNAL.md) and [`DESIGN.md`](DESIGN.md). Happy hacking.
