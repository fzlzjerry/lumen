# Lumen Design Decisions

A running log of the non-obvious choices made while designing and building
Lumen, with the reasoning behind each. New entries are appended as later phases
surface new tradeoffs. The companion retrospective (what was hard, what changed)
lives in [`JOURNAL.md`](JOURNAL.md); this file is the *why* behind the design.

---

## D1 — Implementation language: Rust, std-only

Rust gives memory safety without a runtime, which matters because we hand-roll a
garbage collector and a bytecode VM and want to reason about correctness without
chasing use-after-free bugs in the host. We deliberately use **only the standard
library** for the core (no `logos`, no `lalrpop`, no `hashbrown`): the goal is to
build the lexer, parser, GC, and VM from scratch, and external crates would
either defeat that or hide the very mechanics we are demonstrating.

## D2 — Bytecode VM over a tree-walker

A tree-walking interpreter would be less code, but the goal explicitly calls for
a bytecode compiler, an instruction set, a disassembler, and a stack VM. A
register/stack split was considered; a **stack VM** (à la CPython/clox) was
chosen because operand management is implicit, the compiler is simpler, and the
disassembly reads clearly for teaching and debugging.

## D3 — Surface syntax: C-family braces

Chosen over indentation- and `end`-delimited styles because braces make the
grammar context-free almost everywhere, which makes robust *error recovery*
(sync to the next `}` or `;`) straightforward — significant-whitespace lexers
must synthesize INDENT/DEDENT and recover poorly. Semicolons terminate
statements (no automatic semicolon insertion) for the same reason: one
unambiguous statement boundary to resynchronize on.

## D4 — No parentheses around `if`/`while`/`for-in` conditions; `{` at statement start is a block

The chosen syntax writes `if cond { ... }`, with no parens around the condition.
The naive worry is the classic ambiguity between a controlling expression that
starts with `{` (a map literal) and the `{` that opens the block — the bug Rust
solves with its "no struct literal in condition" restriction.

**Implementation discovery (Phase 2):** Lumen does *not* need that restriction,
because Lumen's map literals are **prefix-less** (`{ k: v }`, with no leading
type name), whereas Rust struct literals are `Name { ... }`. The leading name is
exactly what makes `if foo { }` ambiguous in Rust (`foo` could name a struct).
With prefix-less maps, a `{` is disambiguated purely by parser position:

- **At the start of a statement, `{` always begins a block**, never a map. A map
  literal used as a bare statement-expression must be parenthesized: `({a: 1});`.
  (This is the JavaScript rule, and the only such rule Lumen needs.)
- **In atom position inside an expression, `{` is a map literal.**

A controlling expression is parsed as a normal expression; the operator-climbing
loop stops at the block's `{` because `{` never *continues* an expression after a
complete atom. So `if {a:1}.size > 0 { ... }` parses with no parentheses (cond =
`{a:1}.size > 0`, then the block), and `if ready { ... }` parses as cond `ready`
+ block. No threaded restriction flag is required — a strictly simpler outcome
than the SPEC's first draft anticipated, recorded here because it is the one
place implementation improved on the design.

## D5 — Logical operators have both symbolic and keyword spellings

The user asked for both `&&`/`||`/`!` and `and`/`or`/`not`. They are exact
synonyms produced as distinct tokens by the lexer and collapsed to the same AST
node by the parser, so downstream stages never see the difference. Cost is four
extra keywords; benefit is familiarity for both C- and Python-trained readers.

## D6 — `&&`/`||` are value-preserving and short-circuiting

`a && b` yields `a` when `a` is falsy, else `b` (and dually for `||`), matching
Lua/Python rather than coercing to a strict boolean. This makes idioms like
`x = cache || compute()` and `name && name.length` work, and it composes with
the truthiness rule (D-truth). `!`/`not` always yield a real bool.

## D7 — Numbers: distinct int (i64) and float (f64)

A single float type (like classic JS/Lua 5.1) is simpler but loses exact integer
arithmetic and clean array indices. We keep two numeric types with these rules:
`+ - *` are int when both operands are int (and **overflow throws** rather than
wrapping or silently widening — surprising wraps are worse than a clear error),
and float if either operand is float. `/` is **truncating integer division when
both operands are int**, float division otherwise; this gives `7/2 == 3` and
`7.0/2 == 3.5`. Division or `%` by zero **throws** `DivisionByZero` instead of
producing `inf`/`NaN`, because a thrown error is easier to debug than a silent
poison value. `1 == 1.0` is true so numeric code is not surprised by the int/
float distinction.

## D8 — Truthiness: only `nil` and `false` are falsy

Considered the "0 and empty are falsy" rule (C/JS/Python) but rejected it: it
makes `if items.length` and `if count` ambiguous about intent and is a perennial
source of bugs. Lumen's rule (Ruby/Lua-like) is that *the only falsy values are
`nil` and `false`*; everything else, including `0` and `""`, is truthy. Explicit
is better: write `if x != 0`.

## D9 — Strings immutable and interned

Immutability lets strings be shared freely and used as map keys safely, and lets
the GC intern them: equal contents collapse to one heap object, so equality and
hashing can short-circuit on identity. The cost is that "mutating" a string
allocates a new one; for heavy building, the `string` module offers a builder-
style join. UTF-8 is stored as-is; indexing is by Unicode scalar (character),
not byte, to avoid handing back invalid fragments.

## D10 — GC: handle-based mark-sweep in safe Rust

A tracing collector needs to mutate object graphs with cycles, which fights
Rust's ownership rules. Two safe options: `Rc<RefCell<…>>` (reference counting,
but leaks cycles and isn't the mark-sweep the goal asks for) or an **arena of
handles**. We chose the arena: the `Heap` owns every object in a `Vec`, and a
`GcRef` is a typed index (`u32` + generation tag) into it. Marking flips a bit in
the object header; sweeping frees unmarked slots and reuses them via a free list.
This is a genuine tracing mark-sweep, collects cycles, uses **no `unsafe`**, and
makes collection events observable for the stress test. The tradeoff is one
indirection per object access (an index instead of a pointer) and that a stale
handle is detected by generation mismatch rather than being impossible — an
acceptable price for safety and clarity. Trigger policy: collect when bytes
allocated since the last GC exceed a threshold that grows ×2 after each
collection, bounding both pause frequency and footprint.

## D11 — Closures capture by reference, per-iteration in loops

Upvalues capture the *variable*, not a snapshot, so a closure sees later
mutations (needed for counters, memoization, mutual recursion). Loop bodies open
a fresh scope **each iteration** and close upvalues at the bottom of the
iteration, so closures made in a loop capture that iteration's binding — the
intuitive behavior (matching JS `let`), avoiding the classic "all closures see
the final value" trap.

## D12 — Globals are late-bound; locals are static slots

Top-level names live in a runtime globals table looked up by name, so functions
defined anywhere at top level can refer to each other (forward references, mutual
recursion) as long as the binding exists *when the code runs*. Locals are
resolved by the resolver to fixed stack slots for speed and must be declared
before use. This split is the clox model and is the simplest way to get both
flexible top-level ordering and fast locals.

## D13 — Exceptions as thrown values with `try/catch/finally`

Rather than a checked-exception or Result-returning model, Lumen uses unchecked
exceptions: any value can be `throw`n and caught by `catch (e)`. Runtime faults
(type errors, bad indices, …) throw built-in `error` objects carrying `.kind`
and `.message`, so user code can branch on `e.kind`. `finally` runs on every exit
path. The VM implements this by keeping a per-frame stack of active handlers and
unwinding frames until one matches, capturing a stack trace at throw time.

## D14 — `match` is an expression; patterns are tested top-to-bottom

`match` returns a value (the matching arm's body), which composes better than a
statement-only form. Arms are tried in source order with no compile-time
exhaustiveness check (v0.1) — an unmatched value throws `ValueError`. We compile
patterns to straightforward test-and-branch sequences rather than building a
decision tree; the decision-tree optimization is deferred because correctness and
clarity come first and the naive form is plenty fast for typical arm counts.

## D16 — `this`/`super` context: methods establish it, lambdas inherit, named functions reset

A lambda (`fn(...) {}`) defined inside a method can use `this`/`super` — it
captures the receiver like a JavaScript arrow function. A *named* function
declaration establishes a fresh context with no `this` (like a JS `function`).
So `class C { m() { let f = fn(){ return this.x; }; } }` is valid, but a named
`fn g(){ return this; }` nested in a method is not. This matches the arrow-vs-
function distinction programmers already know, and keeps the rule mechanical: the
resolver propagates `allows_this`/`allows_super` into lambdas and resets them for
named functions. `break`/`continue` similarly do **not** cross a function
boundary — you cannot break out of a loop from inside a nested function.

## D17 — The resolver validates; the compiler allocates

The resolver (Phase 3) is a pure validation pass: it reports every static error
but assigns no stack slots and builds no upvalue tables. The compiler (Phase 4)
re-derives the same lexical facts while emitting code, and is the sole authority
on memory layout. The alternative — have the resolver produce slot/upvalue tables
keyed by node identity and have the compiler consume them — was rejected because
it couples two passes through a fragile side-table and the AST has no node IDs.
Since both passes obey the same scope rules (SPEC §5), they never disagree on a
name's *classification* (local vs upvalue vs global); only the compiler cares
about slot *numbers*. Duplicating the (small) scope-walk is cheaper than the
coupling.

## D18 — GC safety: collect only at instruction boundaries; re-entrant natives must root their references

The collector runs **only at the top of the dispatch loop**, between
instructions, where every live object is reachable from a root (the value stack,
call frames, globals/builtins, open upvalues, the module cache, and any in-flight
thrown value). Inside a single instruction, intermediates live on the value
stack, so they are roots automatically.

The one hazard is **re-entrancy**: a native function (or `${...}` calling a
custom `str()`) that calls back into the VM via `call_and_run` runs a nested
dispatch loop, during which a collection can occur. Any heap reference the native
holds only in a Rust local — not on the value stack and not inside a rooted heap
object — would be freed. The invariant for native code is therefore: **keep heap
references rooted on the value stack (or inside a rooted object) across any
re-entrant call.** Found and fixed one violation in `INTERPOLATE` (it popped its
parts into a Rust `Vec` before stringifying); now it stringifies while the parts
are still on the stack. Phase 7's higher-order natives (`sort`, `map`, …) follow
the same rule. Stress mode (collect-before-every-instruction) plus the
dangling-handle panic in `Heap::get` make any future violation a loud test
failure rather than silent corruption.

## D19 — Per-module globals, resolved against a closure's defining module

Modules each have *their own* global namespace (SPEC §8), which means a function
defined in module A that reads a top-level name of A must keep seeing A's binding
even when called from module B. The first implementation swapped a single
`self.globals` table per module — correct while a module *ran*, but broken the
moment B called back into one of A's closures (B's globals were current). The fix
(Phase 7): the VM keeps a **vector of per-module global tables**, every `Closure`
carries the index of the module it was defined in, and each call frame resolves
`GET/SET/DEFINE_GLOBAL` against *its closure's* module. Built-ins remain a shared
fallback. This is the standard "function closes over its module environment"
semantics, and it is what makes `import "math"` inside `geometry.lum` work when
`geometry`'s functions are called from the main script.

## D20 — Generational GC: a nursery, a write barrier, and a remembered set

The base collector (D10) is a full mark-and-sweep — every collection traces the
whole heap, so cost scales with the *live set*, not the *garbage*. The
generational enhancement adds a young/old split so the common case is cheap. New
objects allocate into a **young nursery**; a **minor** collection traces and
sweeps only young objects and promotes survivors to **old**; a **major**
collection (run when the old generation grows past a threshold) traces
everything and reclaims old cycles.

Soundness rests entirely on the **write barrier**: a minor collection assumes
every live young object is reachable from a VM root *or* from an old object in
the **remembered set**. Any time the VM stores a young object into an old one
(array push/set, map insert, instance field, closed upvalue, class method/super,
and the `push`/`map.set` natives), `write_barrier` adds the old container to the
remembered set, which the next minor collection scans as a root. Old→young edges
created at construction time don't need the barrier because both objects are
young and get promoted together. A missed barrier is a use-after-free, so it's
guarded by a dedicated stress mode (`minor_stress`: a minor collection before
*every* instruction) plus the dangling-`GcRef` panic — `tests/generational.rs`
exercises every mutation path and all examples under it.

## D21 — An optional execution budget bounds otherwise-unbounded programs

The VM normally runs to completion — Lumen has no built-in timeout, and a
`while (true) {}` legitimately loops forever. But the VM fuzzer
(`tests/vm_fuzz.rs`) feeds it thousands of *generated* programs, some of which
loop or recurse without end, and a test that hangs is useless. So the VM carries
a `budget: u64`, defaulting to `u64::MAX` (unlimited — zero overhead beyond one
compare on the hot paths). `set_step_limit(n)` arms it; one unit is charged at
each loop back-edge (`LOOP`) and each closure call — the two places a program can
do unbounded work — and at zero the VM **throws** a `ValueError` rather than
aborting. Throwing (not panicking, not `process::exit`) means a budget hit
unwinds through the ordinary exception path and is indistinguishable, to the
harness, from any other Lumen error: the fuzzer's only failure condition is a
Rust panic. The charge points are deliberately coarse (back-edges and calls, not
every instruction) so straight-line code pays nothing and the common unbounded
shapes — infinite loops and infinite recursion — are still caught.

## D22 — Raw-mode line editing via a hand-written termios binding, not a crate

The REPL's line editor (cursor movement, history recall, word/line kills) needs
the terminal in raw mode, which means `tcgetattr`/`tcsetattr`. The whole project
is dependency-free (std only), and rather than break that for a line-editing
crate, `src/lineedit.rs` declares the three libc functions and the Linux
`termios` struct directly — std already links libc, so this adds no dependency,
just an `extern "C"` block and a `#[repr(C)]` struct. Two decisions make it
coexist cleanly with the rest of the REPL: (1) an RAII `RawGuard` restores the
saved `termios` on drop, so the terminal is never left in raw mode even on an
early return or panic; (2) only the **input** and **local** flag groups are
cleared (`ICANON`/`ECHO`/`ISIG`/`IEXTEN`, `ICRNL`/`IXON`) — the **output** group
is untouched, so `ONLCR` still maps `\n`→`\r\n` and every ordinary `println!` in
the REPL keeps working while raw mode is active. Off Linux (or any non-TTY),
`is_tty()` is false and the REPL falls back to line-buffered reading, so the
feature degrades instead of failing. The editing logic itself is a pure
`LineBuffer` decoupled from the terminal, so it's unit-tested without a TTY.

## D23 — LSP name resolution: scope-tagged declarations, innermost wins

Goto-definition and completion need to answer "what does this name refer to
here?" without re-running the resolver's slot allocation. The LSP instead walks
the AST once (`collect_defs`) and records each statement-level binding as
`(name, name_span, scope)` where `scope` is the byte-offset interval the binding
is visible in: the whole file for top-level decls, the function's span for params
and locals, a block's span for block locals, a loop's span for its variable.
Goto-definition then filters to same-named decls whose scope contains the use and
takes the **innermost** (smallest interval) — which is precisely lexical
shadowing, so a parameter beats a global of the same name with no special-casing.
This is deliberately coarser than the compiler's resolver (it doesn't descend into
lambda expressions, a documented gap) but it's self-contained, fast, and correct
for the statement-level declarations that goto/completion are asked about in
practice.

## D15 — Modules execute once, in their own global scope

Each imported file is compiled and run a single time; its `export`ed names become
the module value, cached by resolved path so repeated imports are cheap and share
state. Built-in native modules (`math`, …) present the same interface as user
modules so `import` is uniform. Circular imports return the partially initialized
module rather than erroring, matching Python's pragmatic behavior.

---

## D30 — Tail-call optimization by frame reuse

`return f(args);` in **tail position** reuses the current call frame instead of
pushing a new one, so deep self- or mutual-recursion (`return loop(n - 1);`) runs
in constant stack space rather than overflowing at `MAX_FRAMES`.

The compiler emits `TAIL_CALL argc` **followed by** an ordinary `RETURN` for a
tail-position call (including `obj.m(...)` and `super.m(...)`, which first
materialize a bound method). The doubled instruction is what keeps the VM simple
and correct for *every* callee:

- **Closure / bound-to-closure** (the optimizable case): `TAIL_CALL` closes the
  current frame's open upvalues, moves `[receiver-or-closure, args…]` down over the
  frame's slots, pops the frame, and re-enters at the *same* `slot_base` — frame
  count is unchanged, so recursion never grows the stack. The trailing `RETURN` is
  then dead code (the frame now runs the callee's bytecode, never the caller's).
- **Native / class / generator function** (not optimizable): `TAIL_CALL` performs
  an ordinary call, and the trailing `RETURN` returns its result from the current
  frame as usual.

TCO is **suppressed when a `finally` is pending** in the function: the return value
must be parked and the `finally` blocks run before returning, which frame reuse
would skip. Generator-function callees are never reused (a call to one produces a
`Generator`); the resolver already forbids a value `return` in an `init`, so
constructors need no special case.

## D29 — Generators: stackful coroutines via a saved VM sub-context

A function whose body contains `yield` is a **generator function**; calling it does
not run the body but returns a `Generator` object, and `yield expr;` produces
values lazily, consumed by `next(gen)` or `for x in gen`.

**Why stackful, not a state machine.** The textbook alternative is to CPS- /
state-machine-transform the generator body so `yield` becomes a `return` out of a
resumable closure. Doing that correctly for *arbitrary* control flow — loops,
`try/catch/finally`, nested calls, `break`/`continue` — means re-implementing the
compiler's control flow as an explicit state graph, which is far more code and far
more error-prone than the VM already is. Instead we snapshot the VM's *own*
execution state, which it already knows how to run and the GC already knows how to
trace. A generator owns its own `stack`, `frames`, `handlers`, and `open_upvalues`
(an `ExecContext`) stored inside the `Generator` heap object; `next`/`for-in`
**swaps** that context into the VM, runs until the body hits `Yield` (which records
the value and unwinds the dispatch loop) or returns (state → `Done`), then swaps it
back out. Because `yield` may not cross a function boundary (resolver-enforced,
like `return`), any helper the generator calls has returned by the time it yields,
so a suspended generator holds a self-consistent context.

**GC.** Two roots matter. (1) A *suspended* generator's `ExecContext` lives in its
heap object and is traced there. (2) While a generator is *running*, its context is
in the live VM fields and the **caller's** context has been swapped out — so the VM
keeps a stack of swapped-out contexts (`saved_contexts`) that the collector roots
exactly like the live stack. Either way every value stays reachable across a
collection (D18). When a generator finishes, its open upvalues are closed (its
stack is discarded).

**Known limitation.** Capture-by-reference (D11) across a suspension is sound as
long as a closure that captures a generator's local is used *within* that generator;
yielding/returning such a closure and calling it while the generator is suspended is
not supported (its upvalue would point into the parked stack). This is an exotic
pattern; ordinary generators (yielding computed values, loops, `for-in`, `next`,
take-n) are fully supported and GC-safe.

## D28 — Typed catch clauses dispatch on `error.kind`

`try` may now carry **multiple** catch clauses, each optionally typed:
`catch (IndexError e) { ... } catch (e) { ... }`. A typed clause `catch (Kind e)`
fires only when the thrown value is a built-in error object whose `.kind` equals
`"Kind"`; a bare `catch (e)` fires for anything (including user-thrown
non-errors). Clauses are tried top-to-bottom, first match wins; if none matches,
the value re-propagates (running any `finally` on the way out, unchanged).

The lowering reuses the existing single-handler/`finally` machinery rather than
adding per-kind handlers: one `PUSH_HANDLER` still guards the body, and the catch
target is a **dispatch chain** compiled from the clauses. The chain tests each
typed clause with a new `MATCH_ERROR kind` opcode (true iff the top value is an
error of that kind) and branches into the matching body; a trailing bare clause
binds and runs unconditionally, and when there is no bare clause the chain ends in
a `THROW` that re-raises the original value (which the enclosing `finally` handler,
if any, still catches). Keeping it to one handler means `break`/`continue`/`return`
unwinding (which counts handlers per `try`) is unchanged.

## D27 — Static methods and field declarations

A class body may now contain `static name(params) { ... }` static methods and
`name = expr;` (or `name;`) field declarations alongside instance methods.

**Static methods** live in a second per-class table (`Class::statics`, separate
from `methods`), are read by `Class.name` / called by `Class.name(args)`, and
have **no receiver** — they compile as ordinary functions (slot 0 is the closure,
not `this`), so using `this`/`super` inside one is a static error. Statics are
copied down to subclasses like methods (a subclass's static of the same name
overrides). `static` is a **contextual** keyword (only special before a method
name in a class body), so it remains usable as an ordinary identifier elsewhere.

**Field declarations** are sugar for assignments at the **top of the constructor**.
Rather than carry a separate runtime "field init" phase, the compiler and resolver
both build one *effective* `init`: the field initializers (`this.f = expr;` in
declaration order, `nil` when omitted) are **prepended** to the user `init`'s body
(a bare `init` is synthesized when the class has fields but no `init`). Computing
this `effective_init` once (`ClassDecl::effective_init`) and feeding it to *both*
the resolver and the compiler keeps the two passes in lockstep (D17) with no
side-table. Consequences: field initializers run per-instance before the rest of
`init`, may reference `this` and the constructor's parameters, and — in a subclass
— its own fields are set before its `init` body (and thus before its
`super.init()` call sets the parent's). A class with fields but no `init` accepts
no constructor arguments, exactly like a class with no `init` today.

## D26 — Operator overloading via dunder methods

Built-in operators dispatch to specially named instance methods ("dunders") when
an operand is a class instance, reusing the same re-entrant `call_and_run` + rooted
bound-method machinery as the existing `str()` hook. When the relevant operand is
**not** an instance, or the instance's class does not define the dunder, the
operator keeps its built-in behavior and throws `TypeError` exactly as before — so
overloading is purely additive. Dispatch is on the **left** operand for the
arithmetic dunders (no reflected `__radd__` in v1); comparisons are all expressed
through `__lt__` (see table). The table:

| Operator | Method | Receiver / call |
|---|---|---|
| `a + b` | `__add__` | `a.__add__(b)` |
| `a - b` | `__sub__` | `a.__sub__(b)` |
| `a * b` | `__mul__` | `a.__mul__(b)` |
| `a / b` | `__div__` | `a.__div__(b)` |
| `a % b` | `__mod__` | `a.__mod__(b)` |
| `a == b` | `__eq__` | `a.__eq__(b)` → truthiness; `!=` negates |
| `a < b` | `__lt__` | `a.__lt__(b)` |
| `a > b` | `__lt__` | `b.__lt__(a)` |
| `a <= b` | `__lt__` | `!(b.__lt__(a))` |
| `a >= b` | `__lt__` | `!(a.__lt__(b))` |
| `a[i]` | `__index__` | `a.__index__(i)` |
| `a[i] = v` | `__set_index__` | `a.__set_index__(i, v)` |
| `-a` | `__neg__` | `a.__neg__()` |

GC safety: the receiver is rooted by the heap-allocated bound method and the
arguments by `call_and_run`'s stack pushes, so a collection during the nested call
cannot free them (DESIGN D18). `__eq__`/`__lt__` results are interpreted by
truthiness, so a dunder may return any value.

## D25 — `match` OR-patterns forbid bindings (v1)

A pattern may be a `|`-separated alternation, `1 | 2 | 3 => "small"`, matching if
**any** alternative matches. For v1, alternatives may **not** bind variables (a
binding inside an alternation is a static error). The alternative — requiring
every alternative to bind exactly the same set of names so the body sees a
consistent environment regardless of which arm fired — is more machinery (a
binding-set equality check, and a compiler that funnels each alternative's
binds into one set of slots) than the feature earns at this stage. Forbidding
binds keeps the lowering trivial: an alternation compiles to a short-circuiting
OR of the per-alternative *tests* (no bind phase), and the rule is easy to teach
("alternatives are literal/wildcard tests"). Allowing same-name binds across all
alternatives is a clean future extension that does not change existing programs.

OR-patterns are parsed wherever a pattern is (so they nest in array/map
patterns); in a destructuring `let`/assignment an alternation is rejected by the
existing "targets must be names" check.

## D24 — Destructuring assignment is a statement, disambiguated by lookahead

`let [a, b] = xs;` already binds *new* variables. The mirror operation — assigning
to *existing* variables, `[a, b] = [b, a];` (a swap), `[x, ..rest] = xs;`,
`{k} = m;` — is added as a distinct statement form (`Stmt::DestructureAssign`),
not an expression. Keeping it statement-only avoids having to make patterns
double as both lvalues and rvalues inside arbitrary expressions; the targets are
existing variables (or `_` to skip), validated by the resolver to be mutable
(assigning a `const` target is a static error, just like `const x; x = 1`).

The parse-time wrinkle is the leading token. At statement start `[` already begins
an array-literal expression and `{` always begins a **block** (D4). Rather than
thread a restriction flag, the parser uses a **bounded lookahead**: when a
statement begins with `[` or `{`, it scans to the matching close bracket/brace
(tracking nesting) and checks whether the *next* token is a single `=`. If so, the
construct is parsed as a destructuring assignment via the existing `pattern()`
grammar; otherwise it falls through to the normal block / expression-statement
path. This is unambiguous because neither a complete array literal nor a complete
block is ever validly followed by `=` in statement position (`[a,b] = …` is not an
lvalue assignment, and a block is a complete statement). A map pattern whose entry
value is a literal (e.g. `{a: 1} = m`) parses but is rejected by the resolver, so
the failure mode is a clear diagnostic rather than a silent misparse. The RHS is
evaluated **once**, in full, before any target is written, so swaps work.
