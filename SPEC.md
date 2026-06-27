# The Lumen Language Specification

Version 0.1. This document defines Lumen precisely enough to implement it. It is
the contract that the lexer, parser, resolver, compiler, and virtual machine all
honor. Where the implementation makes a non-obvious choice, the rationale lives
in [`DESIGN.md`](DESIGN.md).

Notation for grammars: EBNF where `=` defines a rule, `|` is alternation, `{ x }`
is zero-or-more, `[ x ]` is optional, `( )` groups, `"x"` is a literal terminal,
`UPPER` is a lexical token class, and `(* *)` is a comment.

---

## 1. Source text

A Lumen program is a sequence of Unicode scalar values encoded as UTF-8.
Identifiers and keywords are ASCII; string and comment contents may be any
Unicode. Line terminators are `LF` (`\n`); a `CR` (`\r`) immediately before an
`LF` is folded into it, and a lone `CR` is treated as a line terminator. Source
positions are reported as 1-based `line` and `column`, where column counts
Unicode scalar values (a tab counts as one scalar but is rendered as four columns
in diagnostics).

---

## 2. Lexical grammar

### 2.1 Whitespace and comments

```
whitespace   = " " | "\t" | "\r" | "\n" ;
lineComment  = "//" { any character except LF } ;
blockComment = "/*" { blockComment | any character } "*/" ;   (* nestable *)
```

Whitespace separates tokens and is otherwise insignificant. Block comments nest:
`/* a /* b */ c */` is a single comment. An unterminated block comment is a
lexical error.

### 2.2 Identifiers and keywords

```
identifier = idStart { idContinue } ;
idStart    = "A".."Z" | "a".."z" | "_" ;
idContinue = idStart | "0".."9" ;
```

The following identifiers are reserved **keywords** and may not be used as
names:

```
and  break  catch  class  const  continue  else  export  false  finally
fn   for    if     import in     is        let    match  nil    not
or   return super  this   throw  true      try    while  yield
```

`print` is **not** a keyword; it is an ordinary built-in function and may be
shadowed.

### 2.3 Literals

```
intLit    = decInt | hexInt | binInt ;
decInt    = digit { digit | "_" } ;
hexInt    = "0x" hexDigit { hexDigit | "_" } ;
binInt    = "0b" ("0"|"1") { "0" | "1" | "_" } ;
floatLit  = digit { digit | "_" } "." digit { digit | "_" } [ exponent ]
          | digit { digit | "_" } exponent ;
exponent  = ("e"|"E") [ "+" | "-" ] digit { digit } ;
digit     = "0".."9" ;
hexDigit  = digit | "a".."f" | "A".."F" ;
```

Underscores are digit-group separators and carry no meaning (`1_000_000`). A
float requires digits on **both** sides of the `.` (`1.5`, not `1.`); this keeps
`1.method()` and the array rest token `..` unambiguous. Integer literals are
64-bit signed; a literal that does not fit is a lexical error. `1e9` is a float.

```
boolLit = "true" | "false" ;
nilLit  = "nil" ;
```

### 2.4 Strings and interpolation

```
string       = '"' { stringElem } '"' ;
stringElem   = escape | interpolation | rawChar ;
rawChar      = any character except '"', '\', '$', or unescaped end-of-input ;
escape       = "\" ( "n" | "t" | "r" | "0" | "\" | '"' | "$"
                   | "u" "{" hexDigit { hexDigit } "}" ) ;
interpolation = "$" "{" expression "}" ;
```

Strings are double-quoted and may span multiple lines (a literal newline is part
of the string). Recognized escapes: `\n \t \r \0 \\ \" \$` and `\u{XXXX}` for an
arbitrary Unicode scalar (1–6 hex digits, must be a valid scalar value). A `$`
that is **not** followed by `{` is a literal dollar sign. `${ expression }`
embeds an expression whose value is converted to a string (§6.3) and spliced in.
Interpolations may nest arbitrarily (the inner expression may contain more
strings). An unterminated string, an unknown escape, or an invalid `\u{...}` is a
lexical error.

### 2.5 Operators and punctuation

```
+  -  *  /  %  **    (* arithmetic (** is exponentiation) *)
=                    (* assignment *)
+= -= *= /= %=       (* compound assignment *)
== != < <= > >=      (* comparison *)
&& ||  !             (* logical (symbolic) — and/or/not are keyword aliases *)
&  |  ^  ~  << >>     (* bitwise / shift (integer only) *)
?  :                 (* conditional (ternary) expression *)
. , ; :              (* member, separators *)
( ) [ ] { }          (* grouping, indexing, blocks/maps *)
=> ..                (* match arm / lambda arrow, array rest/spread *)
<                    (* also: superclass marker in class decls *)
```

---

## 3. Syntactic grammar

### 3.1 Program and declarations

```
program     = { declaration } EOF ;

declaration = letDecl | constDecl | funDecl | classDecl
            | importDecl | exportDecl | statement ;

letDecl     = "let" identifier [ "=" expression ] ";"
            | "let" pattern "=" expression ";" ;   (* destructuring; pattern is array/map *)
constDecl   = "const" identifier "=" expression ";" ;
funDecl     = "fn" identifier "(" [ paramList ] ")" block ;
paramList   = ( param { "," param } [ "," restParam ] | restParam ) [ "," ] ;
param       = identifier [ "=" expression ] ;   (* a defaulted param must follow required ones *)
restParam   = ".." identifier ;                 (* at most one, and last *)
classDecl   = "class" identifier [ "<" identifier ] "{" { classMember } "}" ;
classMember = method | staticMethod | field ;
method      = identifier "(" [ paramList ] ")" block ;
staticMethod = "static" method ;            (* "static" is a contextual keyword *)
field       = identifier [ "=" expression ] ";" ;
importDecl  = "import" string [ "as" identifier ] ";"
            | "import" string "." "{" identifier { "," identifier } "}" ";" ;
exportDecl  = "export" ( letDecl | constDecl | funDecl | classDecl ) ;
```

A `let` with no initializer binds `nil`. A `const` must be initialized and may
not be reassigned (a static error). Functions and classes are values bound to a
name in the enclosing scope. A class may name at most one superclass after `<`
(single inheritance). The method named `init` is the constructor.

A class body may also contain **static methods** (`static name(params) { ... }`)
and **field declarations** (`name = expr;`, or `name;` for a `nil` default).
A static method belongs to the class itself — it is read as `Class.name` and
called as `Class.name(args)`, has no `this`/`super` (using either is a static
error), and is inherited by subclasses. Field initializers run per-instance at
the **top of the constructor**, in declaration order, before the rest of `init`
(so they may reference `this` and the constructor's parameters); a class with
fields but no `init` accepts no constructor arguments. A subclass initializes its
parent's fields by calling `super.init(...)`. (DESIGN D27.)

A **parameter** may have a default value (`fn f(x = 10)`), used when the caller
omits that argument; the default expression is evaluated at call time in the
function's scope and may reference earlier parameters. Defaulted parameters must
follow required ones. A **rest parameter** (`fn f(..args)`) must be last and
collects all trailing arguments into a fresh array; with a rest parameter a
function accepts any number of arguments at or above its required count.

A **destructuring `let`** binds the variables of a flat array or map pattern
from a value: `let [a, b, ..rest] = xs;`, `let {x, y} = m;` (shorthand for
`{x: x, y: y}`), or `let {key: name} = m;`. Wildcards (`_`) skip a position.
Nested patterns and literals are reserved for `match`.

A **destructuring assignment** uses the same patterns but assigns to *existing*
variables instead of declaring new ones: `[a, b] = [b, a];` (a swap),
`[head, ..rest] = xs;`, `{x, y} = m;`. Every target must name a mutable variable
already in scope (a `const` or undefined target is a static error). The
right-hand side is evaluated once, in full, before any target is written. A
statement that begins with `[` or `{` is a destructuring assignment when its
matching close bracket is immediately followed by `=`; otherwise the leading `[`
is an array literal and the leading `{` opens a block (DESIGN D24).

### 3.2 Statements

```
statement    = exprStmt | destructAssign | block | ifStmt | whileStmt | forStmt
             | returnStmt | breakStmt | continueStmt
             | tryStmt | throwStmt | yieldStmt ;
yieldStmt    = "yield" expression ";" ;   (* only inside a generator function *)

exprStmt     = expression ";" ;
destructAssign = pattern "=" expression ";" ;  (* assign to existing variables *)
block        = "{" { declaration } "}" ;
ifStmt       = "if" expression block [ "else" ( ifStmt | block ) ] ;
whileStmt    = "while" expression block ;
forStmt      = "for" identifier "in" expression block           (* for-in *)
             | "for" forInit [ expression ] ";" [ expression ] block ; (* C-style *)
forInit      = letDecl | exprStmt | ";" ;  (* each form supplies the first ';' *)
returnStmt   = "return" [ expression ] ";" ;
breakStmt    = "break" ";" ;
continueStmt = "continue" ";" ;
throwStmt    = "throw" expression ";" ;
tryStmt      = "try" block ( { catchClause } [ "finally" block ] ) ;  (* >=1 catch or a finally *)
catchClause  = "catch" "(" [ identifier ] identifier ")" block ;  (* `catch (Kind e)` is typed *)
```

A `try` must be followed by one or more `catch` clauses, a `finally` clause, or
both. A clause may be **typed** — `catch (Kind e)` fires only when the thrown
value is a built-in error whose `.kind` equals `"Kind"` — or **bare** —
`catch (e)` fires for any thrown value. Clauses are tried top-to-bottom, first
match wins; if no clause matches, the value re-propagates (running `finally`,
if present). A bare clause after which further clauses are unreachable is a
warning (DESIGN D28). A
`try`/`finally` without a `catch` runs the `finally` on every exit (including a
propagating exception) but does not handle the exception.

`if`, `while`, and `for-in` take their controlling expression with **no
surrounding parentheses**, followed directly by a brace block. Because Lumen map
literals are *prefix-less*, a `{` is disambiguated by position and needs no
special rule: **at the start of a statement, `{` always opens a block** (a map
literal used as a bare statement must be parenthesized, `({a: 1});`), while in
expression atom position `{` is a map literal. The block's opening `{` is never
absorbed by the controlling expression because `{` does not continue an
expression after a complete atom. (See DESIGN D4.)

`for ... in` distinguishes from C-style `for` by lookahead: `for` followed by an
identifier and then the keyword `in` is a for-in loop; otherwise the three-clause
C-style form is parsed (its first clause is a full `letDecl`/`exprStmt` that
consumes its own `;`, or an empty `;`).

### 3.3 Expressions and precedence

Expressions are parsed by a Pratt parser. Precedence from **lowest to highest**;
all binary operators are left-associative except assignment and the conditional,
which are right-associative. Compound assignment (`x += e`) evaluates its target
exactly once and otherwise behaves as `x = x + e`. Bitwise/shift operators are
**integer only** and — following Lua/Python — bind **tighter** than comparison.

| Level | Operators                     | Assoc  | Notes                              |
|-------|-------------------------------|--------|------------------------------------|
| 1     | `=` `+=` `-=` `*=` `/=` `%=`   | right  | target must be an lvalue           |
| 2     | `?` `:`                       | right  | conditional: `cond ? a : b`        |
| 3     | `\|\|` `or`                   | left   | short-circuit                      |
| 4     | `&&` `and`                    | left   | short-circuit                      |
| 5     | `==` `!=`                     | left   |                                    |
| 6     | `<` `<=` `>` `>=` `is`        | left   | `is`: instance-of test             |
| 7     | `\|`                          | left   | bitwise or (int)                   |
| 8     | `^`                           | left   | bitwise xor (int)                  |
| 9     | `&`                           | left   | bitwise and (int)                  |
| 10    | `<<` `>>`                     | left   | shift (int); amount in `0..=63`    |
| 11    | `+` `-`                       | left   |                                    |
| 12    | `*` `/` `%`                   | left   |                                    |
| 13    | `!` `not` `-` `~` (unary)     | right  | prefix                             |
| 14    | `**`                          | right  | exponentiation (binds above unary) |
| 15    | `()` `[]` `.`                 | left   | call, index, member (postfix)      |
| 16    | primary                       | —      | atoms                              |

```
expression  = assignment ;
assignment  = lvalue ( "=" | "+=" | "-=" | "*=" | "/=" | "%=" ) assignment
            | conditional ;
lvalue      = call "." identifier | call "[" expression "]" | identifier ;
conditional = logicOr [ "?" assignment ":" assignment ] ;
logicOr    = logicAnd { ("||"|"or") logicAnd } ;
logicAnd   = equality { ("&&"|"and") equality } ;
equality   = comparison { ("=="|"!=") comparison } ;
comparison = bitOr { ("<"|"<="|">"|">="|"is") bitOr } ;
bitOr      = bitXor { "|" bitXor } ;
bitXor     = bitAnd { "^" bitAnd } ;
bitAnd     = shift { "&" shift } ;
shift      = term { ("<<"|">>") term } ;
term       = factor { ("+"|"-") factor } ;
factor     = unary { ("*"|"/"|"%") unary } ;
unary      = ("!"|"not"|"-"|"~") unary | power ;
power      = postfix [ "**" unary ] ;  (* right-assoc; binds above unary minus *)
postfix    = primary { call | index | member } ;
call       = "(" [ argList ] ")" ;
argList    = arg { "," arg } [ "," ] ;
arg        = [ ".." ] expression ;     (* ".." spreads an iterable into the args *)
index      = "[" expression "]" ;
member     = "." identifier ;
primary    = intLit | floatLit | string | boolLit | nilLit
           | identifier | "this" | "super" "." identifier
           | "(" expression ")"
           | arrayLit | mapLit | lambda | matchExpr ;
arrayLit   = "[" [ element { "," element } [ "," ] ] "]" ;
element    = [ ".." ] expression ;     (* ".." spreads an array into this one *)
mapLit     = "{" [ entry { "," entry } [ "," ] ] "}" ;
entry      = ( string | identifier | "[" expression "]" ) ":" expression ;
lambda     = "fn" "(" [ paramList ] ")" block      (* block body, explicit return *)
           | identifier "=>" assignment            (* arrow: single param, expr body *)
           | "(" [ paramList ] ")" "=>" assignment ;
matchExpr  = "match" exprNoBrace "{" arm { "," arm } [ "," ] "}" ;
arm        = pattern [ "if" expression ] "=>" expression ;
```

In a map literal an `identifier` key is shorthand for the string of that name
(`{x: 1}` ≡ `{"x": 1}`); a `[expr]` key is computed. `super.method` is only
valid inside a method of a class that has a superclass. `match` is an expression:
each arm's body is an expression, and the whole `match` evaluates to the body of
the first matching arm. The conditional `cond ? a : b` evaluates `cond` (by
truthiness, §6.1) and then **only** the selected branch — the other is not
evaluated.

### 3.4 Patterns

```
pattern     = patternAtom { "|" patternAtom } ;     (* "|" alternation *)
patternAtom = "_"                                  (* wildcard *)
            | intLit | floatLit | string | boolLit | nilLit   (* literal *)
            | "-" (intLit | floatLit)              (* negative literal *)
            | identifier                            (* binding *)
            | "[" [ patElem { "," patElem } ] "]"  (* array pattern *)
            | "{" [ patEntry { "," patEntry } ] "}";(* map pattern *)
patElem     = ".." [ identifier ] | pattern ;       (* ".." or "..rest" *)
patEntry    = ( string | identifier ) ":" pattern ;
```

A wildcard `_` matches anything and binds nothing. A bare `identifier` matches
anything and binds it. A literal matches by equality (§6.2). An array pattern
matches an array of the given shape; a single `..` or `..rest` element matches
zero-or-more elements (at most one rest per array pattern), binding `rest` to a
new array. A map pattern matches a map that contains each named key, recursively
matching the value; extra keys in the subject are ignored. An **alternation**
`p1 | p2 | ...` matches if any alternative matches; for v1, alternatives may not
bind variables (a binding inside an alternation is a static error — DESIGN D25).
An optional arm guard `if expression` must also evaluate truthy for the arm to
fire.

---

## 4. Types and values

Lumen is dynamically typed. Every value has one of these runtime types:

| Type      | Kind      | Notes                                                   |
|-----------|-----------|---------------------------------------------------------|
| `nil`     | immediate | the unit/absence value                                  |
| `bool`    | immediate | `true` / `false`                                        |
| `int`     | immediate | 64-bit signed two's-complement                          |
| `float`   | immediate | 64-bit IEEE-754 double                                  |
| `string`  | reference | immutable, UTF-8, interned                              |
| `array`   | reference | mutable, ordered, 0-indexed, heterogeneous              |
| `map`     | reference | mutable hash map; keys are nil/bool/int/float/string    |
| `function`| reference | a closure (top-level functions and lambdas)             |
| `class`   | reference | constructs instances; holds methods                     |
| `instance`| reference | an object with fields and a class                       |
| `method`  | reference | a bound method (instance + function)                    |
| `native`  | reference | a built-in function implemented in Rust                 |
| `module`  | reference | an imported module's exported bindings                  |
| `error`   | reference | a built-in error object (`.kind`, `.message`)           |
| `generator`| reference | a parked coroutine produced by calling a `yield`-bearing function |

`type(x)` returns the type name as a string; for a class **instance** it returns
that instance's **class name** (so `type(Point(1, 2)) == "Point"`), and for every
other value it returns the type name from the table above. The `x is C` operator
tests class membership: it is `true` iff `x` is an instance whose class is `C` or
a subclass of it, and `false` for any non-instance; the right operand must be a
class (else `TypeError`). Map keys are restricted to the
*hashable* immediate types plus strings; using a mutable reference as a key is a
runtime error. `int` and `float` keys that are numerically equal (`1` and `1.0`)
are considered the **same** key.

---

## 5. Scope and binding

Lumen is lexically (statically) scoped with block scope.

- A `block` (`{ ... }`), a function body, a loop body, and each loop iteration
  introduce a new scope. Names bound with `let`/`const` are visible from the
  point of declaration to the end of their enclosing scope.
- Re-declaring a name already bound in the **same** scope is a static error.
  Shadowing a name from an **outer** scope is allowed.
- `const` bindings may not be the target of an assignment (static error).
- **Local** variables are resolved statically to a stack slot and must be
  declared textually before use. **Global** (top-level) names are resolved late,
  by name, at run time; this is what allows top-level functions to call each
  other regardless of declaration order, including mutual recursion.
- Referencing a name that is neither a local, an enclosing local (upvalue), nor
  a defined global is an error: a static error for locals known to be missing, or
  a runtime error for an undefined global at the point of use.
- A function closes over (captures) the variables it references from enclosing
  function scopes, by **reference** (an *upvalue*). Because each loop iteration is
  a fresh scope, a closure created inside a loop captures that iteration's
  binding, not a single shared one.
- `this` is bound inside methods to the receiver; using `this` outside a method
  is a static error. `super.m` is valid only inside a method of a subclass.
- `break` and `continue` are valid only inside a loop (static error otherwise);
  `return` only inside a function (static error otherwise).

---

## 6. Evaluation semantics

### 6.1 Truthiness

`nil` and `false` are **falsy**. Every other value — including `0`, `0.0`, `""`,
`[]`, and `{}` — is **truthy**. `if`, `while`, `&&`, `||`, `!`, and match guards
use this rule.

### 6.2 Equality

`==` compares as follows; `!=` is its negation.

- `nil == nil` is true; `nil` equals nothing else.
- Two numbers compare by numeric value, mixing `int` and `float` freely
  (`1 == 1.0` is true; `NaN == NaN` is false).
- Two booleans compare by value.
- Two strings compare by content.
- Arrays, maps, instances, functions, classes, methods, modules, and errors
  compare by **identity** (same object).
- Values of otherwise-different types are not equal.

### 6.3 Conversion to string

`str(x)` and string interpolation render values thus: `nil`→`nil`,
booleans→`true`/`false`, `int`→decimal, `float`→shortest round-trippable decimal
(always containing a `.` or exponent, e.g. `5.0`), strings→themselves, arrays→
`[a, b, c]` with elements rendered recursively, maps→`{k: v, ...}`, functions→
`<fn name>`, classes→`<class Name>`, instances→`<Name instance>` unless the class
defines a `str()` method, which is called instead.

### 6.4 Operators

Let `n` denote a number (int or float).

- `-x` (unary): numeric negation; error if `x` is not a number.
- `!x` / `not x`: logical negation; result is a bool per truthiness.
- `a + b`: if both are numbers, numeric addition (int+int→int with wrapping on
  overflow is **not** allowed — overflow throws; int+float or float→float). If
  both are strings, concatenation. If both are arrays, a new concatenated array.
  Any other combination is a runtime type error.
- `a - b`, `a * b`: numeric; int when both int (overflow throws), else float.
- `a ** b`: exponentiation. If both are int and `b >= 0`, the result is int (with
  the same overflow check as `*`); otherwise (a float operand, or a negative
  integer exponent) the result is a float. `**` is right-associative and binds
  tighter than unary minus, so `-2 ** 2` is `-4` and `2 ** 3 ** 2` is `512`.
- `string * int` / `int * string`: repeat the string that many times
  (`"ab" * 3` is `"ababab"`); a count `<= 0` yields `""`.
- `a / b`: if both operands are int, **truncating** integer division; otherwise
  float division. Divisor zero throws `DivisionByZero`.
- `a % b`: remainder with the sign of the dividend; divisor zero throws.
- `a < b` etc.: numbers compare numerically (mixed int/float allowed); strings
  compare lexicographically by Unicode scalar value. Other combinations are a
  runtime type error.
- `a is C`: `true` iff `a` is an instance whose class is `C` or a subclass of it;
  `false` for any non-instance. `C` must be a class, else a runtime type error.

**Operator overloading.** When an operand is a class instance, the arithmetic,
comparison, indexing, and unary-minus operators dispatch to "dunder" methods on
its class if present (`__add__`, `__sub__`, `__mul__`, `__div__`, `__mod__`,
`__eq__`, `__lt__`, `__index__`, `__set_index__`, `__neg__`); the comparisons
`<`/`>`/`<=`/`>=` are all derived from `__lt__`, and `==`/`!=` from `__eq__`. If
the method is absent the operator keeps its built-in behavior and throws
`TypeError` as usual. The full table and dispatch rules are in DESIGN D26 and
[`API.md`](API.md).
- `a && b` / `a and b`: evaluate `a`; if falsy, result is `a`; else result is
  `b` (short-circuit, value-preserving like Lua/Python).
- `a || b` / `a or b`: evaluate `a`; if truthy, result is `a`; else `b`.
- `a = b`: evaluates `b`, stores it in the lvalue `a` (a variable, `obj.field`,
  or `arr[i]`), and yields `b`. Assigning a field that does not exist creates it.
  Indexing assignment grows arrays only at exactly `length` (append); a larger
  index throws `IndexError`.

### 6.5 Calls

A call in **tail position** — `return f(args);`, where the call's value is
returned directly — reuses the current call frame rather than growing the stack
(tail-call optimization, DESIGN D30). This applies to function, closure, method
(`return this.m(...)`), and `super` (`return super.m(...)`) calls, and to mutual
recursion, so tail-recursive code runs in constant stack space. It does not apply
when a `finally` must run first (the `finally` runs and the call is ordinary).

`f(a, b)` evaluates `f` then the arguments left to right, then invokes. An
argument prefixed with `..` is a **spread**: its value must be an iterable
(array, string, map, or `range`), and its elements are unpacked in order into the
argument list. Spread arguments may be mixed freely with ordinary ones
(`f(1, ..xs, 2)`) and compose with default and rest parameters; the effective
argument count is determined at run time. Arity is checked: calling with the
wrong number of arguments throws `ArityError`. Calling a non-callable throws
`TypeError`. A class call `C(args)` allocates an instance,
runs `init` (if any) with the arguments, and yields the instance; if there is no
`init`, the call takes no arguments. Methods are looked up on the instance's
class then its superclasses; a bound method captures the receiver.

### 6.6 Indexing and members

- `arr[i]`: `i` must be an int; negative indices count from the end
  (`arr[-1]` is the last element); out of range throws `IndexError`.
- `map[k]`: returns the value for key `k`, or `nil` if absent.
- `str[i]`: returns the `i`-th character as a one-character string.
- `obj.field`: reads an instance field, or a bound method if `field` names a
  method; reading a missing field yields `nil`, but reading a missing method via
  call throws.
- `module.name`: reads an exported binding; missing export throws.

### 6.7 Control flow

`if`/`else`, `while`, and both `for` forms behave conventionally. `for x in it`
iterates: arrays by element, strings by character, maps by key, and any value
with an iterator protocol (`range(...)` yields an array). `break` exits the
nearest loop; `continue` proceeds to its next iteration (the C-style `for`'s
step still runs).

### 6.8 Functions, closures, classes

Functions are first-class values and may be returned, stored, and passed.
Closures capture upvalues by reference (§5). Classes support single inheritance;
a method may call `super.m(...)` to invoke the superclass implementation.

### 6.9 Generators

A function whose body contains `yield` is a **generator function**; `yield` is
valid only inside such a function (a static error elsewhere). Calling a generator
function does **not** run its body — it returns a `generator` object. Each call to
`next(gen)` runs the body until the next `yield expr;`, which produces `expr` as
the result and suspends with all state (locals, loops, open `try` handlers)
preserved; the following `next` resumes after the `yield`. When the body returns,
the generator is exhausted and `next` yields `nil`. `for x in gen` iterates a
generator lazily, one `yield` per step, ending when it is exhausted — so an
infinite generator is consumed safely by stopping early (`break`). Generators are
single-threaded coroutines (DESIGN D29), not OS threads.

---

## 7. Error model

There are two error regimes.

**Static errors** are produced by the lexer, parser, and resolver. The driver
collects as many as it reasonably can (the parser recovers at statement
boundaries) and prints them all; a program with any static error is never
executed.

**Runtime errors** are *thrown values*. The runtime throws a built-in `error`
object for: `TypeError`, `NameError` (undefined global), `ArityError`,
`IndexError`, `KeyError`, `DivisionByZero`, `ValueError`, and
`StackOverflow`. A program may `throw` any value of its own. `try { B } catch (e)
{ H } finally { F }` runs `B`; if a value is thrown during `B`, control transfers
to `H` with `e` bound to the thrown value; `F`, if present, runs afterward
whether or not an exception occurred (and before propagating an uncaught one). An
uncaught throw aborts the program, printing the value and a stack trace, with
process exit code 70.

A built-in `error` object exposes `.kind` (e.g. `"TypeError"`) and `.message`
(human text), and stringifies as `kind: message`.

---

## 8. Modules

`import "math";` loads the module named `math`: first `math.lum` relative to the
importing file's directory, otherwise a built-in module of that name. A module is
executed once (results cached) in its own global scope; its `export`ed names form
the module value. `import "m" as alias;` binds the whole module to `alias`;
`import "m".{a, b};` binds the named exports directly. A circular import in
progress reuses the partially-initialized module (its already-run exports are
visible).

---

## 9. Memory model

Values are either immediate (`nil`, `bool`, `int`, `float`), stored inline, or
references to heap objects managed by a tracing **mark-and-sweep** garbage
collector. Programs never free memory explicitly.

- Assignment and argument passing copy a value: for immediates the bits, for
  references the handle. Two variables holding the same array observe each
  other's mutations.
- Strings are immutable and **interned**: equal contents share one heap object,
  so string equality can short-circuit on identity.
- The collector's roots are the VM value stack, the call-frame closures, the
  globals table, the open-upvalue list, and the interned-string table. Anything
  reachable from a root survives a collection; everything else is reclaimed.
- Collection may occur at any allocation site when the live heap exceeds a
  growth threshold; it has no observable effect other than memory use. Cycles are
  reclaimed (the collector is tracing, not reference-counting). There are no
  finalizers in v0.1.

---

## 10. Built-in functions (global)

Always in scope (shadowable): `print(x...)`, `println(x...)`, `str(x)`,
`type(x)`, `len(x)`, `int(x)`, `float(x)`, `bool(x)`, `range(...)`, `assert(c,
msg?)`, `clock()`, `input(prompt?)`, `chr(i)`, `ord(s)`, `push(arr, x)`,
`pop(arr)`, `keys(map)`, `values(map)`, `has(map, k)`, `del(map_or_arr, k)`,
`next(gen)` (advance a generator; `nil` when exhausted). The
native modules (`math`, `string`, `array`, `map`, `io`, `os`, `time`, `json`,
`random`) are documented in [`API.md`](API.md).
