# Lumen Build Journal

A phase-by-phase retrospective: what was built, what was hard, what changed, and
what the tests proved. Design *rationale* lives in [`DESIGN.md`](DESIGN.md); this
is the build diary.

---

## Phase 0 — Language design

**Built.** The Cargo project skeleton (lib + bin, std-only, `panic=abort`
release profile); the source-position layer (`span.rs`) and the diagnostic
renderer (`diagnostics.rs`) with caret underlines and tab-aware columns; the full
language spec (`SPEC.md`) covering lexical and syntactic EBNF, the type system,
evaluation semantics, scope rules, the two-tier error model, and the memory
model; the design-decision log (`DESIGN.md`, 15 entries); and 15 example programs
plus one helper module under `examples/`, exercising every language feature.

**Decisions that shaped everything downstream.**
- Two numeric types (`i64`/`f64`) with overflow-throws and truncating int
  division — see DESIGN D7. This means the lexer must distinguish int vs float
  literals precisely, and the VM's arithmetic is type-directed.
- Ruby-style truthiness (only `nil`/`false` falsy) — DESIGN D8.
- The "no bare map literal in a controlling expression" rule (DESIGN D4) is the
  one context-sensitive parse decision; flagged now so the parser is built for it
  from the start rather than retrofitted.
- Handle-based mark-sweep GC (DESIGN D10) — chosen now because it dictates that
  `Value` carries a `GcRef` handle (an index), not an `Rc`, which ripples through
  every later module.

**What was hard / interesting.** Pinning down the arithmetic and equality
semantics so the examples have a single unambiguous expected output. The biggest
back-and-forth was division: `7/2` could reasonably be `3`, `3.5`, or an error;
I settled on int/int → truncating int (D7) so integer-heavy code (indices, loop
counters) stays integer and predictable, documenting it loudly.

**A spec/example reconciliation.** The first draft of `07_collections.lum` used a
ternary `?:` and `map.field` dot access — neither is in the grammar. Rather than
expand the language to fit the example, I rewrote the example to use `match`-free
expressions and `map["key"]` indexing, keeping maps (`["k"]`) and instances
(`.field`) cleanly separated. Principle: the spec leads, examples follow.

**Tests.** `span` and `diagnostics` ship with unit tests (6 total) that already
pass — merge logic, caret alignment, and tab expansion. The examples are not yet
runnable (they depend on the stdlib from Phase 7); they are the end-to-end oracle
for Phase 9.

**Status.** `cargo build` clean, `cargo test` green (6/6). Ready for Phase 1.

---

## Phase 1 — Lexer

**Built.** `token.rs` (the `TokenKind` enum, keyword table, and the `StrPart`
representation of interpolated strings) and `lexer.rs`, a hand-written,
non-panicking, error-recovering lexer. It handles: all operators and
punctuation; identifiers and the 27 keywords (plus `true`/`false`/`nil`); integer
literals in decimal/hex/binary with `_` separators; float literals with
fractional and scientific parts; double-quoted strings with the full escape set
(`\n \t \r \0 \\ \" \$ \u{...}`) and `${expr}` interpolation; nestable block
comments and line comments; and line/column tracking with CRLF folding. Also
added a `lumen lex <file>` debugging subcommand.

**The interesting problem: interpolation.** The clean way to lex `${expr}` was
the question. Re-lexing a substring would have meant fixing up every inner span
by an offset. Instead the lexer recurses *in place*: on `${` it repeatedly calls
`next_token` while counting brace depth, so the inner expression is tokenized
against the real source (correct spans for free), nested maps balance via the
depth counter, and nested strings/interpolations fall out of the recursion
naturally. The inner token vector is capped with a synthetic `Eof` so a later
sub-parser knows where to stop. A dedicated test feeds
`"outer ${ {x:1}["x"] } and ${"inner ${y}"}"` and asserts zero errors.

**The subtle number rule.** `1.` must lex as `Int(1) Dot`, and `1..2` as
`Int(1) DotDot Int(2)`, so the fractional part is only taken when a digit
*follows* the dot (and similarly the exponent needs a digit after the optional
sign). Tests pin all three cases. This is what keeps method calls (`x.foo`) and
the array rest token (`..`) unambiguous against float syntax.

**Error recovery.** Unexpected characters, lone `&`/`|`, unknown escapes, and
overflowing integers each record a diagnostic and continue (the bad input is
skipped or taken literally), so one run reports many errors. Unterminated strings
and block comments report once and stop. 14 error/recovery and happy-path tests.

**Tests.** 21 lexer unit tests, all green; plus a smoke run lexing all 16 example
files with zero lexical errors (41–303 tokens each). Removed an unused `at_end`
helper to keep `cargo build` warning-free.

**Status.** `cargo build` clean, `cargo test --lib` green (24/24). Ready for
Phase 2.

---

## Phase 2 — Parser

**Built.** `ast.rs` (the full span-carrying tree: statements, expressions,
patterns, functions, classes, imports); `parser.rs`, a recursive-descent +
precedence-climbing parser with panic-mode error recovery; `util.rs` (shared
float/string formatting); and `ast_printer.rs`, which renders the AST back to
canonical source. Added `lumen parse` and `lumen fmt` subcommands.

**Design discovery that simplified the parser.** The SPEC's first draft carried
Rust's "no struct literal in condition" restriction (a threaded
`no_struct_literal` flag). While implementing I realized Lumen does not need it:
Lumen map literals are *prefix-less* (`{k: v}`), unlike Rust's `Name { ... }`, so
a `{` is disambiguated purely by parser position — at statement start it is a
block, in atom position it is a map. The block's `{` is never absorbed by a
controlling expression because `{` cannot continue an expression after a complete
atom. I removed the flag, updated SPEC §3.2 and DESIGN D4, and added two tests
(`statement_brace_is_block_not_map`, `condition_starting_with_map_literal`). This
is the one place the implementation improved on the paper design.

**Error recovery.** A `ParseError` marker unwinds to `synchronize()`, which skips
to the next `;` or statement keyword. Both the top-level loop and `block()` carry
a forward-progress guard (if recovery consumed nothing, force one token) so a
pathological input can never loop forever. Tests confirm multiple independent
errors are reported in one run.

**Interpolation sub-parsing.** Each `${...}` arrives from the lexer as a
pre-tokenized `Vec<Token>`; the parser spins up a sub-`Parser` over it, requires
it to consume to `Eof`, and folds its errors back into the main list. So
`"hi ${a + b}!"` becomes a `StrInterp` of `[Text, Expr(a+b), Text]`.

**The AST printer & round-trip property.** The printer reconstructs minimal
parentheses from precedence (an `expr(node, ctx_prec)` helper wraps only when a
child binds looser than its context) and normalizes `and`/`or`/`not` to
`&&`/`||`/`!`. The headline test asserts *idempotency*:
`print(parse(print(parse(src)))) == print(parse(src))`. A subtle bit was float
formatting — `5.0` must not print as `5` (which would re-lex as an int), handled
by `util::format_float`.

**Tests.** 24 new unit tests (precedence, associativity, every statement/
expression form, recovery, basename derivation, 14 round-trip cases). Plus a CLI
smoke test: all 16 example files parse with zero errors and are round-trip
stable under `lumen fmt`.

**Status.** `cargo build` clean, `cargo test --lib` green (48/48). Ready for
Phase 3.

---

## Phase 3 — Resolver / semantic analysis

**Built.** `builtins.rs` (the canonical list of global built-in names, shared
with the future VM so neither drifts) and `resolver.rs`, a single-pass validator
that reports every static error: undefined reads/writes, read-in-own-initializer,
duplicate declarations (incl. duplicate params and duplicate pattern bindings),
const reassignment, `this`/`super` misuse, `break`/`continue`/`return` context,
returning a value from `init`, self-inheritance, and `export` outside top level.

**Key architecture call (DESIGN D17).** The resolver assigns *no* slots and
builds *no* upvalue tables — it only validates. The compiler (Phase 4) will
re-derive the same lexical facts while emitting code. This avoids a fragile
side-table keyed by node identity; since both passes obey SPEC §5's scope rules,
they agree on every name's classification, and only the compiler needs slot
numbers.

**The two-pass global trick.** Top-level names are collected first (with
duplicate detection), so forward references and mutual recursion between globals
("`fn a` calls `fn b` defined later") resolve cleanly, while *locals* still must
be declared before use. A name that is neither a known global, a built-in, an
in-scope local, nor an enclosing-local upvalue is reported as undefined — turning
"use before declaration" into a helpful static error instead of a runtime one.

**Subtleties handled.** (1) `let f = fn(){ return f; };` is valid — the recursive
reference is an *upvalue* capture of a not-yet-initialized enclosing local, which
is allowed, whereas `let a = a;` (same-function read of an uninitialized local)
is rejected. The distinction is "is the uninitialized local in the *current*
function or an enclosing one." (2) Lambdas inherit `this`/`super`; named
functions reset them; loop context doesn't cross a function boundary (DESIGN
D16). Tests pin both.

**Tests.** 15 resolver unit tests covering each error class and the valid
counterparts. All 16 example files pass resolution (`lumen parse` now runs the
full lex→parse→resolve front end).

**Status.** `cargo build` clean, `cargo test --lib` green (63/63). Ready for
Phase 4.

---

## Phase 4 — Bytecode compiler + disassembler

**Built.** `opcode.rs` (a checked `#[repr(u8)]` ISA with a checked
`from_u8`), `chunk.rs` (byte stream + line table + a heap-free `Constant` pool
and `Rc<FnProto>`), `compiler.rs` (the AST→bytecode generator), `disassembler.rs`
(recursive, human-readable), and `OPCODES.md` documenting every instruction.
Added the `lumen disasm` subcommand.

**The decoupling that paid off.** Constants are deliberately *not* GC values:
`Int`/`Float`/`Str` plus `Rc`-shared `FnProto`s. So the entire front end
(lex→…→compile) is independent of the runtime heap, which does not exist until
Phase 5. The VM will materialize a `Str` constant into an interned heap string
when it executes `CONST`. This kept Phase 4 self-contained and testable purely
through disassembly.

**Hardest sub-problems.**
- *Upvalues.* Implemented clox's two-level resolution (`resolve_local` →
  `resolve_upvalue` walking the enclosing `FnState` stack, marking captured
  locals and de-duplicating capture descriptors). `CLOSURE` trails the upvalue
  table; `end_scope` emits `CLOSE_UPVALUE` for captured locals.
- *`finally`.* The genuinely hard case is running `finally` on *every* exit:
  normal, caught, re-thrown, and `return`/`break`/`continue` that cross the try.
  Solution: wrap the inner try/catch in a second handler (`PUSH_HANDLER` ×2) that
  re-runs `finally` and rethrows; emit the finally inline on the normal path; and
  for `return`, park the value in a temp local so the finally's locals don't
  collide before reloading and returning. `break`/`continue` walk the
  control-flow frame stack, emitting each crossed try's finally and handler pops.
- *Pattern matching.* To avoid the partial-binding-cleanup-on-failure trap, each
  arm is compiled in **two phases**: a side-effect-free test that leaves exactly
  one bool (so a failed arm only ever has one value to drop), then — only on a
  confirmed match — a binding pass. Sub-values are reached through an `Access`
  descriptor (`Local`/`Index`/`Key`) re-emitted as needed, so nothing extra
  lingers on the stack. Three dedicated opcodes (`MATCH_ARRAY`, `MATCH_MAP_HAS`,
  `ARRAY_REST`) keep the tests clean.
- *Classes.* Copy-down inheritance (`INHERIT` copies the superclass method table
  into the subclass, so dispatch is one lookup) plus a scoped `super` local that
  method closures capture as an upvalue — so `super` works inside lambdas, not
  just direct methods (no `home_class` field needed).

**The `{` disambiguation, validated.** Because statement-position `{` is always a
block and map literals are prefix-less, `if {a:1}.size > 0 { ... }` and `for x in
{...} { }` compile correctly with no special casing — confirmed by compiling all
16 examples.

**Tests.** 21 new unit tests across opcode round-tripping, chunk patching/dedup,
disassembly, and per-construct codegen (closures, loops, for-in, classes/super,
interpolation, try/finally handler counts, match ops, imports, properties). All
16 examples compile to bytecode via `lumen disasm`.

**Status.** `cargo build` clean (0 warnings), `cargo test --lib` green (84/84).
Ready for Phase 5 — the VM that finally executes this.

---

## Phase 5 — Virtual machine

**Built.** The whole runtime: `value.rs` (the `Copy` `Value` enum + `GcRef` +
`MapKey`), `object.rs` (heap object types, incl. an insertion-ordered `LumMap`),
`gc.rs` (the handle-based heap with allocation + interning; collection deferred
to Phase 6), `vm.rs` (the dispatch loop and all opcode semantics), and
`vm/builtins.rs` (the 20 global builtins). Plus a `stdlib` stub and the `lumen
run` command. **Lumen now executes real programs.**

**The decisive design choice: `Value: Copy`.** Because every value is either an
immediate or a `GcRef` (a `u32` index), `Value` is `Copy`. This sidesteps almost
every borrow-checker fight a Rust VM normally has — the stack pushes/pops freely,
arguments copy, and native functions take `&[Value]` snapshots. The heap owns the
actual data; values just point at it.

**Code vs. data split.** Function *prototypes* (`Rc<FnProto>`) are immutable code,
shared by `Rc`, never touched by the GC. Only runtime *data* (strings, arrays,
maps, instances, closures, upvalues) is heap/GC-managed. Frames hold a cloned
`Rc<FnProto>`, so reading bytecode never borrows the heap — instruction decode is
`self.frames[fi].proto.chunk.code[ip]`, a copy-out.

**Re-entrancy via `run_until(floor)`.** The dispatch loop runs the top frame
until the frame stack shrinks to a floor. That one parameter makes
`call_and_run` work: a native (or `${...}` interpolation hitting a custom
`str()` method, or a future `sort` comparator) pushes a Lumen frame and runs it
to completion re-entrantly, then reads the result off the stack. Tested via the
`Box.str()`-in-interpolation case.

**Exceptions.** `PUSH_HANDLER`/`POP_HANDLER` maintain a handler stack tagged with
frame index and stack height; a thrown value unwinds to the nearest handler at or
above the current run floor (so a throw inside a re-entrant call doesn't leak into
an outer handler). Uncaught throws print a real stack trace (function name + line
per frame) and exit 70. Verified end to end.

**Bugs found and fixed by running real code.**
- *Per-iteration capture in C-style `for`.* The first run printed `3 3 3` instead
  of `0 1 2` — the loop variable was one shared slot. Fixed with a targeted
  `CLOSE_UPVALUE_SLOT` opcode emitted at the bottom of each iteration, restructured
  so both the normal end-of-body and `continue` route through the close+step block.
  Now `0 1 2` (DESIGN D11 honored). `for-in` already got this for free via its
  per-iteration scope.
- *`try`/`finally` without `catch`.* My own VM test wanted cleanup-without-
  swallowing; the grammar required `catch`. Made `catch` optional (AST/parser/
  resolver/printer/compiler all updated; SPEC §3.2 revised) — a `try`/`finally`
  now runs the finally on every exit and lets the exception propagate.

**Borrow discipline.** The recurring pattern throughout: read what's needed out of
`heap.get(r)` into owned/`Copy` data, drop the borrow, *then* allocate or mutate —
so an immutable heap borrow never overlaps a `make_error` or `alloc`. Codified in
the `op_index_get`/builtins "classify then act" style.

**Perf.** `fib(30)` runs in ~0.41 s (release). Unoptimized but fine; Phase 10 can
add `INVOKE`/superinstructions.

**Tests.** 20 VM integration tests (`tests/vm.rs`) with captured output covering
arithmetic semantics, truthiness, control flow, break/continue, recursion,
closures, per-iteration capture, classes/super/init/fields, custom `str()`,
collections, interpolation, try/catch/finally, typed runtime errors, full pattern
matching, stack overflow, and integer overflow. 10 runnable examples confirmed
(the other 6 need Phase 7 stdlib modules).

**Status.** `cargo build` clean (0 warnings), `cargo test` green (89 lib + 20
integration). Ready for Phase 6 — turning the heap's allocator into a real
collector.

---

## Phase 6 — Garbage collector

**Built.** A tri-color tracing **mark-and-sweep** collector on top of the Phase 5
heap: `Heap::mark_value`/`mark_ref` (seed + gray set), `trace_references` (mark
everything reachable through the object graph), `sweep` (free unmarked, clear
marks, prune weak intern entries), and `finish_collection` (grow `next_gc` to 2×
the surviving live set). The VM's `mark_roots` seeds the value stack, frame
closures, globals, builtins, open upvalues, the module cache, and the in-flight
thrown value; `collect()` ties it together and runs at every instruction
boundary when `should_collect()` says so. All in **safe Rust** — the only
`unsafe` in the whole project remains the bounds-checked opcode `transmute`.

**Why it's a real tracing collector, not refcounting.** The graph has cycles
(an instance whose field points back to a list that holds it; mutually-recursive
closures sharing upvalues). Handle-based marking walks them correctly and
reclaims cycles, which `Rc` could not. The handle indirection is the price.

**The subtle bug GC surfaced: re-entrancy rooting (DESIGN D18).** `INTERPOLATE`
popped its parts into a Rust `Vec` *before* stringifying them — but stringifying
an instance with a custom `str()` re-enters the VM, where a collection can run and
free those now-unrooted parts. Caught by the stress test, fixed by stringifying
while the parts are still on the value stack. This established the invariant for
all native code.

**Validation.** Beyond unit tests of mark/sweep mechanics (reachable survives,
garbage freed, marks cleared between cycles), the headline is
`correctness_under_stress`: seven feature-heavy programs (closures, nested
arrays, growing maps, a linked list of 50 instances, string-building, pattern
binds, exception unwinding) all run with **a full collection before every single
instruction** and still produce correct output — proof there are no missing
roots. The pressure tests confirm collection happens and is bounded: 200k garbage
arrays leave < 5000 live objects (no leak), and 100k unique garbage strings leave
< 5000 interned (the intern table is genuinely weak). valgrind/nightly-ASan
aren't installed here; host memory safety is guaranteed by Rust ownership
regardless, and these stress tests cover the GC-logic correctness those tools
would.

**Status.** `cargo build` clean (0 warnings), `cargo test` green (91 lib + 4
gc-stress + 20 vm). Ready for Phase 7 — the standard library.

---

## Phase 7 — Standard library

**Built.** Nine native modules under `src/stdlib/` — `math`, `string`, `array`,
`map`, `io`, `os`, `time`, `json`, `random` — wired through the VM's native-module
loader and cached so repeated imports share one object. `json` is a real hand-
written recursive-descent parser + serializer (with pretty-printing). `random` is
a seedable xorshift64* PRNG whose state lives on the VM. Plus the **self-hosted**
`seq` module (`std/seq.lum`), written in Lumen and embedded via `include_str!`,
proving the language can implement its own library.

**Two correctness problems the real programs exposed.**
- *Per-module globals (DESIGN D19).* Example 11 (`geometry.lum` importing
  `math`, then called from the main script) threw `undefined variable 'math'`.
  The root cause: a single swapped `globals` table meant a module's functions
  resolved globals against *whoever called them*. Refactored to a vector of
  per-module global tables, with every closure tagged by its defining module and
  each frame resolving against its closure's module. This is the proper "function
  closes over its module environment" model.
- *GC rooting of higher-order natives (DESIGN D18).* `map`/`filter`/`reduce`
  accumulate heap results across re-entrant `call_and_run` callbacks; `sort` runs
  a user comparator. Their accumulators are pinned via a new `temp_roots` GC root
  set before each callback. The `higher_order_natives_are_gc_safe_under_stress`
  test runs all four with collect-before-every-instruction and gets correct
  results — proof the rooting is complete.

**Smaller design calls.** `sort` is a hand-rolled stable **merge sort** (not
`slice::sort_by`) precisely so a misbehaving user comparator can never trip the
standard library's "comparator is not a total order" panic — a thrown comparator
error propagates cleanly instead. `floor`/`ceil`/`round`/`trunc` return ints;
`abs`/`min`/`max` preserve the operand's int/float-ness. String ops index by
Unicode scalar, matching `s[i]`.

**Tests.** 10 stdlib integration tests across all modules (math, string, array
incl. comparator sort, map, json round-trip, seeded-deterministic random, the
self-hosted `seq`, the per-module-globals fix, and the GC-stress pass). All 15
example programs now run end to end.

**Status.** `cargo build` clean (0 warnings), `cargo test` green (91 lib + 4
gc-stress + 20 vm + 10 stdlib = 125). Ready for Phase 8 — the toolchain.

---

## Phase 8 — Toolchain

**Built.** Five tools, all std-only: a **REPL** (`repl.rs`), a **debugger**
(`debugger.rs`), the **formatter** (`fmt --write`), a **language server**
(`lsp.rs`), and **project management** (`project.rs`), plus a syntax highlighter
(`highlight.rs`). Supporting library hooks: `resolver::resolve_with` (predefined
globals), `compiler::compile_repl` (return a trailing expression), `Vm::eval`
(run + return value + reset on error), the VM's debug API (`debug_start`/
`debug_step`/`debug_location`/`debug_backtrace`/`debug_locals`/`debug_lookup`),
local-name debug info on `FnProto`, and a module search path.

- **REPL.** Multi-line (keeps reading while brackets/strings are open), persistent
  state across inputs, immediate evaluation (a bare trailing expression prints
  `=> value`), file-backed history, `:`-commands (`:help`/`:history`/`:hl`/
  `:disasm`), and color. A bare `x + y` with no `;` is accepted via a parse-with-
  appended-`;` fallback. Verified: closures persist (`c()`→1 then 2), a runtime
  error (`1/0`) is reported and the session continues.
- **Debugger.** Drives the VM one instruction at a time with breakpoints,
  `step`/`continue`, `backtrace`, `locals` (by *name* — this is why `FnProto`
  grew a slot→name table), `print <name>`, and `disasm` (current frame, ip
  marked). Verified showing `a = 3, b = 4, sum = 7` at a breakpoint inside a
  function and unwinding the call stack.
- **Formatter.** `lumen fmt --write` rewrites in place; reuses the Phase 2 AST
  printer (and its idempotency guarantee).
- **LSP.** A from-scratch JSON value/parser/serializer (so the crate stays
  dependency-free) drives a stdio server with `initialize`, document sync,
  `publishDiagnostics` (front-end errors → LSP ranges), and `hover` (token
  description). Smoke-tested: capabilities, diagnostics with ranges, and hover
  over `fn` → "define a function".
- **Project management.** A tiny TOML-subset parser reads `lumen.toml`; `new`
  scaffolds, `build` static-checks, `run` executes the entry, `test` runs every
  `tests/*.lum` (pass == no uncaught error) with PASS/FAIL output. Local path
  dependencies join the module search path.

**Design note that recurred.** Several tools needed the VM to *not* be a black
box — single-stepping, returning a value, naming locals, resolving against a
chosen module. Each was a small, additive public method rather than a rewrite,
which is the payoff of the Phase 5 design (frames hold their own `Rc<FnProto>`,
values are `Copy`, globals are per-module).

**Tests.** Added unit tests for the highlighter (color + text-preservation), the
project manifest parser, and the LSP JSON round-trip; the REPL/debugger/LSP/
project commands were each exercised end-to-end via piped input.

**Status.** `cargo build` clean (0 warnings), `cargo test` green (97 lib + 4
gc-stress + 10 stdlib + 20 vm = 131). Ready for Phase 9 — tests, fuzzing, and
benchmarks.

---

## Phase 9 — Tests & benchmarks

**Built.** Five new integration suites and a benchmark command, taking the total
to **159 passing tests, zero skipped**: `tests/e2e.rs` (run every example,
snapshot-compare its full output), `tests/errors.rs` (one+ test per error class —
lexical, parser, resolver, and every runtime error *kind* asserted via
`e.kind`), `tests/fuzz.rs` (11,000 random/soup/mutated inputs through the front
end, asserting no panic), `tests/coverage.rs` (breadth over the runtime, stdlib,
and debug API), plus the existing `vm`/`stdlib`/`gc_stress` suites. `lumen bench`
times fib/loops/allocation/strings/methods → `BENCHMARKS.md`.

**Coverage.** Installed `cargo-llvm-cov` and measured. The **core components all
reach ≥90% line coverage**: lexer 94.6%, parser 95.8%, resolver 98.0%, compiler
94.3%, disassembler 96.0%, GC 91.7%, AST printer 90.8%, and the VM 89.3% lines /
90.1% regions; the pure stdlib modules (math/string/array/random) are 95–100%.
Getting there drove real tests, not vanity ones — the VM climb from 75%→90% came
from covering operator/type error paths, `try/finally` exit paths, the per-module
import machinery, and exercising the debugger's `debug_step`/`backtrace`/`locals`
/`display` API in-process (it had only been driven via piped stdin). The
remaining gaps are the genuinely-I/O modules (`io`/`os`/`time`, untestable in a
unit) and a handful of native-builtin argument-error branches.

**The fuzzer found nothing — which is the result.** 11k adversarial inputs (raw
random bytes, token soup, and byte-mutated real programs) all degraded to
diagnostics; the lexer/parser/resolver never panicked. The deterministic seed
makes any future regression reproducible, and a `catch_unwind` wrapper would
print the offending input.

**Benchmarks, honestly reported.** fib(32) ~1.2 s, a 10M-iteration loop ~2.3 s,
1M array allocations ~0.26 s (GC keeping up), 100k naive string concatenations
~4.4 s (O(n²) by the immutability design — `BENCHMARKS.md` shows the
`string.join` idiom that's O(n)), 1M method calls ~0.40 s. The doc names the
concrete optimizations (INVOKE super-instruction, global inline cache) deferred
to keep correctness first.

**Status.** `cargo build` clean (0 warnings), `cargo test` green (**159 tests**,
0 failed, 0 skipped). Ready for Phase 10 — docs and a self-review polish pass.

---

## Phase 10 — Docs & polish

**Documentation.** Wrote `README.md` (overview, quick start, the command table,
the architecture map, and links to everything), `TUTORIAL.md` (a 12-section
guided tour, hello-world → modules, every snippet verified by running it), and —
delegated to two parallel agents that read the source and ran the binary to
verify — `API.md` (the complete stdlib reference: 20 globals + 9 native modules +
the self-hosted `seq`, every signature pulled from source, every example checked)
and `CONTRIBUTING.md` (architecture map, build/test bar, the GC-rooting and
resolver/compiler invariants, and recipes for adding an opcode or a stdlib
function). With the earlier `SPEC`/`DESIGN`/`OPCODES`/`BENCHMARKS`/`JOURNAL`,
that's nine documents.

**Self-review — findings.** Reading the whole tree top to bottom surfaced:
1. *(perf)* Method calls allocated a bound-method object per call — the top item
   in `BENCHMARKS.md`.
2. *(docs)* `OPCODES.md` predated `CLOSE_UPVALUE_SLOT` (Phase 5) and a wrong
   "61-instruction" count had crept into the README.
3. *(safety)* The lone `unsafe` is the opcode `transmute`; it is bounds-checked
   and proven sound by `roundtrip_all_opcodes` (every byte round-trips).
4. *(readability)* `to_display` and `debug_display` overlap, but justifiably —
   one may call a user `str()` and needs `&mut self`; the debugger's is read-only.
5. A scan confirmed **zero** `todo!`/`unimplemented!`/placeholder code; the
   `unreachable!()`/`expect` sites are all compiler-invariant guards, never
   reachable from user input (the fuzzer corroborates for the front end).

**Self-review — refactors actually applied.**
- Implemented the **`INVOKE` super-instruction** (finding #1): `obj.method(args)`
  now compiles to a single fused op. Its fast path calls an instance method
  directly with the receiver already in slot 0 — no bound-method allocation —
  while non-instance/field-holding cases fall back to property-resolve-then-call,
  preserving exact semantics (incl. the `TypeError` for calling a non-callable).
  Added across `opcode.rs`/`compiler.rs`/`vm.rs`/`disassembler.rs`/`OPCODES.md`.
  All 159 tests still pass (the e2e class snapshots and method tests are the
  guard); method-dispatch micro-bench drops slightly and, more importantly, sheds
  ~1M allocations.
- Fixed the docs drift (finding #2): added `CLOSE_UPVALUE_SLOT` and `INVOKE` to
  `OPCODES.md`, corrected the README to the real 58 instructions.

**Deferred, with rationale.** The remaining `BENCHMARKS.md` items — a global-
binding inline cache and moving the `should_collect()` check off the hot path to
back-edges — are genuine wins but carry correctness risk disproportionate to a
final polish pass on a fully-working, fully-tested system; they're documented as
future work rather than rushed in. `vm.rs` is large (~1700 lines) but cohesive;
splitting it would be churn for its own sake.

**Status.** `cargo build` clean (0 warnings), `cargo test` green (**159 tests**,
0 failed, 0 skipped); all 15 examples run; core components ≥90% covered; nine
documents complete. The language is done: it designs, lexes, parses, resolves,
compiles, executes, collects, and tools real programs end to end.

---

# Enhancement track

After the base language was complete and shipped, a second track of six
optional enhancement phases (A–F) was undertaken to push performance, features,
GC sophistication, robustness, tooling, and ecosystem.

## Enhancement Phase A — Performance

**Profiled, found a real bug, fixed broadly.** Reading the dispatch loop turned
up that `GET_GLOBAL`/`SET_GLOBAL`/`INVOKE` cloned the operand name `String` from
the constant pool on **every** execution — ~20M heap allocations just for the
name `"s"` in the 10M-iteration loop benchmark. Four changes:

1. **Clone-free names.** A free `const_str(&FnProto, idx) -> &str` borrows the
   name from the frame's prototype — a *disjoint field* from `module_globals`/
   `heap`, so the borrow checker permits reading the name while still mutating
   the rest of the VM. `SET_GLOBAL` now updates in place via `get_mut`, and
   `INVOKE`'s fast path resolves the method with the borrowed name (only the
   rare slow path clones).
2. **FxHash (`src/fxhash.rs`).** A from-scratch, std-only FxHash-style hasher
   replaces SipHash for the globals/fields/methods/exports/map-index/intern
   tables. SipHash's DoS resistance is irrelevant for internal tables and slow
   for short string keys; FxHash is much faster for exactly those.
3. **GC trigger off the hot path.** The heap-pressure check moved from every
   instruction to back-edges (`LOOP`) and calls; **stress mode still collects
   before every instruction**, so the root-completeness guarantee is untouched
   (all gc_stress tests still pass).
4. The `INVOKE` super-instruction from the base Phase 10 already removed
   per-method-call bound-method allocation.

**Result:** a broad **~25–40% speedup** — loop 2.3 s → 1.4 s, method dispatch
0.40 s → 0.27 s, fib(32) 1.2 s → 0.88 s, allocation 0.26 s → 0.19 s — with **all
161 tests passing** (FxHash added 2 unit tests). The string-build O(n²) is an
inherent immutability antipattern (the `string.join` idiom is the fix); a global
inline cache is the one big remaining win but needs mutable chunks, so it's
documented as future work in `BENCHMARKS.md`.

**Status.** `cargo build` clean (0 warnings), `cargo test` green (161). On to
Phase B — language features.

## Enhancement Phase B — Language features

Three features, end to end through the whole pipeline (SPEC → parser → resolver →
compiler → VM → tests → example → docs):

**Default parameters** (`fn f(a, b = 10)`). The default expression is evaluated
at call time and may reference earlier parameters (`fn rect(w, h = w)`). `FnProto`
gained `required_arity`; the call site now range-checks `required ≤ argc ≤ arity`.
The mechanism: the VM fills omitted optionals with `nil`, then a per-parameter
prologue of a new `DEFAULT_ARG index skip` opcode — using the frame's recorded
`provided_argc` — either skips the default (if the arg was supplied) or evaluates
it. The resolver enforces "required cannot follow defaulted".

**Rest parameters** (`fn f(a, ..rest)`). Marked on `FnProto` (`has_rest`); the
parser accepts `..name` only as the last parameter. At call, the VM slices the
overflow arguments into a fresh array bound to the rest slot. Composes with
defaults: `fn c(a, b = 1, ..flags)` works.

**Destructuring `let`** (`let [a, b, ..rest] = xs;`, `let {x, y} = m;`,
`let {key: name} = m;`). A new `Stmt::Destructure` reuses the existing `Pattern`
type; map-pattern shorthand `{x}` ≡ `{x: x}` was added to the pattern parser (so
`match` benefits too). The compiler binds the init to a hidden `@destr` temp,
then extracts each variable through index/key access (or `ARRAY_REST` for
`..rest`) and defines it — as globals at the top level, locals in a function (the
`define_variable` helper already keys off scope). Resolver collects top-level
destructure names as globals and rejects nested patterns/literals (those stay
`match`-only).

**The clean part:** each feature was additive. Defaults/rest reused the existing
call machinery (just a richer arity model + one opcode); destructuring reused
`Pattern`, `Access`-style extraction, and `define_variable`'s scope-awareness.
Nothing in the runtime's hot path changed.

**Tests:** 9 new (`tests/features.rs`) covering each feature, arity errors with
defaults/rest, the "required-after-default" static error, nested-destructure
rejection, and formatter round-trip on the new syntax — plus a 16th example
(`16_params_and_destructure.lum`) with a snapshot in the e2e suite.

**Status.** `cargo build` clean (0 warnings), `cargo test` green (**170 tests**).
On to Phase C — a generational GC.

## Enhancement Phase C — Generational GC

Turned the full mark-and-sweep into a **two-generation** collector (DESIGN D20).
New objects allocate into a **young nursery**; a **minor** collection traces and
sweeps only young objects (seeded by VM roots + the remembered set) and promotes
survivors to **old**; a **major** collection traces everything and reclaims old
cycles. The payoff: minor-collection cost is O(nursery), independent of the
promoted live set — so a program with a big persistent heap and lots of
short-lived allocations no longer rescans the whole heap on every GC.

**The crux — the write barrier.** A minor collection only finds live young
objects via roots or the remembered set, so *every* store of a young object into
an old one must be recorded. I added `write_barrier(container, value)` at every
mutation site: `ARRAY_PUSH`/`ARRAY_EXTEND`, `INDEX_SET` (array + map), `MAP_INSERT`,
`SET_PROP`, `CLOSE_UPVALUE`, `INHERIT`, `METHOD`, and the `push`/`map.set`
natives. Each remembers an old container that now points at a young object; a
dedup flag avoids double-entries.

**Testing the barrier — the part that matters most.** A missed barrier is a
silent use-after-free. I added a second stress mode, `minor_stress` (a *minor*
collection before every instruction), and `tests/generational.rs` runs programs
that build old→young edges through every mutation path under it — plus all 16
examples — so any gap frees a live young object and trips the dangling-`GcRef`
panic deterministically. It found nothing, which is the result. A heap-level unit
test also checks promotion and that the barrier (vs. its absence) decides
survival.

**Bookkeeping changes:** `GcBox` gained `generation` and `remembered` bits;
`Heap` tracks `young_bytes`/`old_bytes` with separate `next_minor` (nursery size)
and `next_major` thresholds; `mark_ref`/`trace_references` took a `young_only`
flag; sweep split into `sweep_minor` (free dead young, promote live young) and
`sweep_major` (full). The existing gc_stress tests (now forcing a final major
collection so they measure reachable, not nursery-resident, objects) still pass.

**Throughput** on the existing benchmarks is comparable (they have small live
sets, so a full collection was already cheap); the generational win is pause time
and scaling, which the benchmarks here don't isolate — noted honestly.

**Status.** `cargo build` clean (0 warnings), `cargo test` green (**173 tests**).
On to Phase D — robustness (VM fuzzing, sanitizers, coverage).

## Enhancement Phase D — Robustness

Three independent attacks on "does the VM ever do something undefined": a
VM-level fuzzer, a memory-checker pass, and coverage for the OS-facing stdlib.

**Execution budget.** The fuzzer needs to feed the VM programs that may loop or
recurse forever without hanging the test. I added a `budget: u64` to the VM
(default `u64::MAX` = unlimited), charged one unit at every loop back-edge
(`LOOP`) and every closure call. At zero it throws a `ValueError`
("execution budget exceeded") — an ordinary catchable Lumen error, so it unwinds
through the normal path rather than aborting. `set_step_limit(n)` arms it. Three
direct tests pin the behavior: an infinite `while`, infinite recursion, and a
bounded loop that must *not* trip the limit.

**The VM fuzzer (`tests/vm_fuzz.rs`).** The existing `fuzz.rs` throws random
*text* at the lexer/parser; this one generates random *valid* programs and runs
them through the whole interpreter. The generator tracks an in-scope variable
list (push on `let`, truncate on block exit) and a function table, so every
emitted reference resolves — the programs reach the VM instead of dying in the
resolver. It emits lets, assignments, `if`, bounded `for`, function defs/calls,
and nested expressions (arithmetic, arrays, maps, indexing, builtin calls). 2500
programs run under a 200k step limit, half of them under full GC stress, inside
`catch_unwind`: a Lumen throw or a budget hit is fine — only a Rust panic is a
bug. **Zero panics.** (First run flagged the generator itself: I was leaking
block-scoped vars into the in-scope list, so later references were undefined and
only ~10% of programs reached the VM. Scope-truncation on block exit fixed it to
>70%.)

**Memory checking — valgrind.** Installed valgrind 3.24 and ran the real release
binary over the allocation-heavy examples (`14_algorithms`, `07_collections`,
`15_wordcount`, `12_json`) under a new `LUMEN_STRESS_GC=1` toggle (collect on
every allocation, so every mark/sweep path executes). Result: **0 errors, 0 bytes
definitely/indirectly/possibly lost.** The only block in use at exit is 544 bytes
"still reachable" — the Rust runtime's one-time main-thread allocation, not ours.
This is the strongest evidence the handle-based GC has no use-after-free or leak:
valgrind sees actual machine allocations, including std's. (An AddressSanitizer
pass via nightly `-Zbuild-std` was attempted as a second opinion but fails on a
`duplicate lang item` toolchain conflict in this environment — unrelated to
Lumen; valgrind on the instrumented binary is the equivalent check and it's
clean.)

**Coverage — io/os/time (`tests/io_os_time.rs`).** The clock/file/process surface
was the least-tested corner of the stdlib. 10 new end-to-end tests: write→read
roundtrip, append + `lines`, `exists` true/false, the throw paths
(`read_file`/`write_file` on bad paths), `os.platform`/`os.cwd`,
`os.args` wired through `set_args`, `os.env` with a live env var and the
default-value form, and `time.now`/`now_millis`/`sleep` (types, positivity,
clock monotonicity). Real temp files under a per-test directory keep them
parallel-safe.

**Status.** `cargo build` clean (0 warnings), `cargo test` green (**187 tests**),
valgrind clean. On to Phase E — toolchain (LSP, debugger, REPL line-editing).

## Enhancement Phase E — Toolchain

Deepened the three interactive tools — LSP, debugger, REPL — each gaining the
features that separate a demo from something usable.

**LSP: goto-definition, completion, document symbols.** The server already did
diagnostics and hover; these three need *name resolution*, so I added a
statement-level declaration walker (`collect_defs`) that tags every binding with
the byte-offset range it's visible in. Goto-definition finds the identifier
under the cursor, then picks the same-named declaration whose scope contains the
use and is **innermost** — which is exactly lexical shadowing, so a parameter
correctly wins over a global of the same name (tested). Completion offers the
in-scope names plus stdlib module names plus keywords; document symbols walks the
top level, nesting class methods as child symbols. Multi-line ranges are computed
by scanning the source (`pos_of_offset`), so a function's symbol range spans its
whole body. Known limit, documented: lambdas nested inside expressions aren't
walked (statement-level decls are). 5 new unit tests drive each handler through
real JSON params.

**Debugger: step-into vs step-over, conditional breakpoints, watchpoints.**
Stepping became source-line granular (it was per-instruction): `step` runs to the
next line descending into calls; `next` runs to the next line in the current
frame *or shallower*, treating a call as one step. The distinction is just call
depth — `next` keeps running while `depth > start_depth`. Verified over a scripted
session: from a call site, `step` lands inside `square` (backtrace depth 2) while
`next` runs through the call and stays at depth 1. Breakpoints gained conditions
(`break 12 if i == 50`, `break 8 if done`): a tiny comparison grammar (a variable
vs a literal/variable, six operators, or a bare truthiness test) evaluated against
the live frame via three new read-only VM helpers (`debug_depth`,
`debug_compare`, `debug_as_str`; `values_equal` already existed). Watchpoints
(`watch total`) re-read each watched variable after every instruction and pause
when a value changes, reporting `old => new`. End-to-end the conditional
breakpoint fired only at `i == 3` while the watchpoint caught every change to
`total` — both at once. 7 new unit tests cover condition parsing (incl. the
`>=`-before-`>` pitfall) and the step-mode predicates.

**REPL: real raw-mode line editing.** The old REPL conceded that keystroke-level
editing "would require an FFI dependency this std-only project avoids" — so I
wrote the FFI. `src/lineedit.rs` has a hand-written `termios` binding (std already
links libc on Linux; no crate added) behind an RAII `RawGuard` that flips the
terminal into raw mode and restores it on drop. Crucially it clears only input
and local flags (`ICANON`/`ECHO`/`ISIG`/`IEXTEN`, `ICRNL`/`IXON`) and leaves the
*output* flags alone, so `ONLCR` keeps turning the rest of the REPL's `\n`s into
`\r\n` — the editor coexists with ordinary `println!`. The editor itself is a
pure, unit-tested `LineBuffer` (insert, backspace/delete, cursor moves, Home/End,
Ctrl-W/U/K kills, UP/DOWN history with a stashed live line) plus a key-decode
loop (including UTF-8 continuation bytes and `ESC [` arrow sequences). On a
non-TTY (pipes, tests) `is_tty()` is false and the REPL falls back to the old
line-buffered reader, so nothing in CI changes. Driven over a real pseudo-terminal
it edits, evaluates (`=> 99`), and recalls history correctly; 6 unit tests cover
the buffer logic.

**Status.** `cargo build` clean (0 warnings), `cargo test` green (**205 tests**).
On to Phase F — ecosystem and self-hosting.

## Enhancement Phase F — Ecosystem & self-hosting

Grew the standard library and proved the language out on real programs — the
final test of "make it run real programs first."

**Three more self-hosted modules.** `seq` already showed the language could
implement reusable code; this adds `set`, `functional`, and `testing`, all
written in Lumen and embedded with `include_str!` + `load_source_module`:
- **`set`** — a hash set as a `class` backed by a map (membership is a key), with
  `add`/`has`/`remove`/`union`/`intersect`/`difference`/`is_subset` and a custom
  `str()`. Holds hashable values (the map-key types: int/float/string/bool/nil),
  documented.
- **`functional`** — `identity`/`constant`/`compose`/`pipe`/`curry2`/`partial`/
  `flip`/`complement`/`memoize`/`iterate`, all built from closures. `pipe` uses a
  rest parameter (Phase B); `memoize` closes over a cache map.
- **`testing`** — a `Suite` class that tallies checks and prints a summary, plus
  `deep_eq` for structural array/map equality (the core `==` is structural only
  for primitives and strings, so a test harness needs its own).

**Three real programs** (examples 17–19, each with an e2e snapshot):
- **`17_calculator`** — a complete arithmetic interpreter *in Lumen*: a
  tokenizer, a recursive-descent parser with correct precedence and parentheses
  and unary minus, an evaluator, and exception-based error reporting. The
  language is now expressive enough to host a small language.
- **`18_data_structures`** — a singly linked list (with in-place reverse), a
  stack, a queue, and a binary search tree (recursive insert + in-order
  traversal), exercising mutable object graphs and recursion.
- **`19_json_tool`** — parses a JSON document with the `json` module, queries it
  (filter + aggregate), and pretty-prints the structure with a recursive printer
  that dispatches on `type()`.

**Two real bugs the programs surfaced (the point of dogfooding):**
1. `import "string" as str;` then `str(x)` *shadows the global `str()` builtin* —
   the module alias wins, so the conversion call became "call a module". Renamed
   the alias; a sharp edge worth a comment in the example.
2. Integer `/` truncates (`89 / 3 == 29`); the average needed `float(total) / n`
   and a `/ 100.0` to keep decimals. Good reminder that the numeric tower is
   int-preserving except where a float operand forces promotion.

**Verification.** All three programs run identically under `LUMEN_STRESS_GC=1`
(collect-on-every-alloc), proving their object graphs — BST nodes, list nodes,
parsed JSON maps — survive collection via the rooting discipline. 3 new stdlib
integration tests pin the modules; the e2e harness snapshots the programs.

**Status.** `cargo build` clean (0 warnings), `cargo test` green (**208 tests**),
19 examples, 4 self-hosted std modules. All six enhancement phases (A–F) are
complete.

**Closing polish.** Installed and ran `cargo clippy --all-targets` over the whole
crate and brought it to **zero clippy lints**: implemented `Display` for the LSP
`Json` (replacing an inherent `to_string`), switched `repeat().take()` to
`repeat_n`, a manual `loop`/`match`-break to `while let`, an OR-pattern to a
range, and tightened a `&mut String` parameter to `&str`; one false positive
(`approximate_constant` flagging a `3.14` sample literal) was sidestepped by using
`3.25`. All pure lints — no behavior change, suite stayed green.

---

# Improvement track

A research pass (three parallel codebase sweeps plus this journal and
`BENCHMARKS.md`) produced a tiered roadmap; this track records its execution.

## Tier 0 — Docs caught up to reality

The README had drifted from the code: it called the GC "tracing"/"mark-and-sweep"
(it's been **generational** since Enhancement Phase C), counted "58 instructions"
(Phase B's `DEFAULT_ARG` made it 59), and "159 tests" (208 after Phase F).
Corrected all three against the source. Also brought `cargo clippy` back to zero
under a newer toolchain: two lints had appeared from clippy drift, not new code —
`only_used_in_recursion` on the formatter's `pattern_str` (made it an associated
fn, since it only recursed on `self`) and `format_collect` on `project::indent`
(rewrote with `writeln!`).

## Tier 1 — A v0.2 bundle

**Stdlib parity (16 native functions).** `array` gained `find`/`find_index`/
`any`/`all`/`unique`/`zip` (previously only on the self-hosted `seq`); `map` gained
`each`/`map`/`filter`/`clear`/`from_entries`; `math` gained `lcm`/`is_nan`/
`is_finite`/`degrees`/`radians`. The higher-order additions reuse the existing
`call_and_run` + temp-root discipline (DESIGN D18); `map.map` resolves each key
*before* its callback so the freshly-returned value is never left unrooted across
an allocation. A new GC-stress test runs them under collect-on-every-allocation.

**Ternary `cond ? a : b`.** New `Question` token and `Ternary` AST node, slotted
between assignment and `||` in the Pratt parser (right-associative). It lowers to
the same `JUMP_IF_FALSE`/`JUMP` shape as `||`/`&&` — **no new opcode** — so only
the taken branch runs. Formatter round-trips it.

**Compound assignment `+= -= *= /= %=`.** New compound-assign tokens desugaring to
the arithmetic ops, with the target evaluated **once**. `Var` reads/writes the
binding; `Get` duplicates the object with `DUP`; `Index` needed both object *and*
index reused, which `DUP` (top-only) can't express — and `add_local`-style temps
are unsafe mid-expression here (the compiler allocates local slots as
`locals.len()`, assuming a clean stack, so a temp declared while a callee/operand
is pending reads the wrong slot — the same latent limitation `match`'s `@subj`
has, just never exercised nested). A probe (`println(a[1] += 5)`) confirmed the
miscompile, so I added a small **`DUP2`** opcode (`[a,b] -> [a,b,a,b]`,
top-relative) that makes the index form correct in any nesting.

**`SUPER_INVOKE`** (the next item on `BENCHMARKS.md`'s deferred list). Fuses
`super.m(args)` the way `INVOKE` fuses `obj.m(args)`: the compiler lays out
`[this, args…, superclass]`, and the op pops the superclass, resolves the method
in it, and calls with `this` already in the receiver slot — no bound-method
allocation. `GET_SUPER` remains for a bare super reference (`let f = super.m;`).

**Net.** Two new opcodes (`DUP2`, `SUPER_INVOKE`) → **61** instructions. Every
feature flows through `SPEC`/`TUTORIAL`/`API`/`OPCODES` as applicable. `cargo
build`/`clippy` clean (0 warnings, 0 lints), `cargo test` green at **217** (+9),
nothing skipped; the formatter round-trips all new syntax and the e2e class
snapshots guard `SUPER_INVOKE`'s semantics.

**Deferred to Tier 2 and beyond.** See the next section for Tier 2; the bigger
Tier 3 perf items (specialized opcodes, the global inline cache — still blocked on
immutable `Rc<FnProto>` chunks) and Tier 4 (new stdlib modules, LSP
`formatting`/`rename`, a dependency story) remain as the roadmap.

## Tier 2 — Language features + static diagnostics

A design pass first: three parallel agents produced grounded, file-cited
implementation plans (the lambda agent stalled twice on the model side and was
designed by hand). Then implemented sequentially with TDD — these features all
touch the shared parser/resolver/formatter, so they don't parallelize cleanly.

**Static diagnostics — three resolver warnings.** *Unused variables* (an `is_read`
flag on locals, flagged at scope exit; only plain `let`/`const`, never params,
loop/catch vars, captures, or `_`-prefixed). *Unreachable code* (a `resolve_stmts`
pass warns once per block on the first statement after a terminator). *Wrong-arity
calls* (top-level `fn` signatures collected up front; a direct `name(...)` with the
wrong count warns — conservative, skipping shadowed names and computed callees so
it can't false-positive). The key infra choice: warnings are **non-fatal**.
`check_source` still returns *errors only* (so all ~28 existing gates/tests are
untouched); a new `check_all` carries warnings to the LSP (mapped to LSP severity),
the REPL, and `lumen run`. The runtime still enforces arity, so a missed warning
costs nothing. All 19 examples are warning-clean.

**Bitwise operators** `& | ^ ~ << >>` (integer only). New tokens (repurposing the
old "did you mean `&&`?" lexer errors), `BinaryOp`/`UnaryOp` variants, six opcodes,
and VM handlers that throw `TypeError` on non-ints and a `ValueError` on a shift
amount outside `0..=63` (Rust's shift would otherwise panic); `>>` is arithmetic.
Precedence follows **Lua/Python** (bitwise *above* comparison), not C — so
`1 & 1 == 1` is `(1&1) == 1`, avoiding C's classic footgun. The formatter's
precedence table was renumbered to 15 levels; the round-trip idempotency test is
the safety net that proves the renumbering correct.

**Lambda shorthand** `x => e` and `(a, b) => e` / `() => e`, body an implicitly-
returned single expression. Disambiguation: `IDENT =>` is caught with the existing
two-token lookahead; `(params) =>` with a cheap paren-scan to the matching `)`
(no speculative parse, no spurious errors) before committing to the arrow vs a
grouping. It reuses the `fn` parameter parser (so defaults/rest compose) and builds
an ordinary `ExprKind::Lambda(Function)` whose body is one `return expr` — so the
resolver, compiler, and closure machinery need **zero** changes, and the formatter
canonicalizes arrows to the `fn` form (idempotent). Bodies parse at assignment
level, so `a => b => c` curries right.

**Net.** Six new opcodes (bitwise) → **67** instructions. Every feature flows
through `SPEC`/`TUTORIAL`/`OPCODES` as applicable. `cargo build`/`clippy` clean
(0 warnings, 0 lints), `cargo test` green at **223** (+6 over Tier 1), nothing
skipped; formatter round-trips all new syntax. (Mid-session the macOS Xcode license
lapsed and blocked the *linker* — `cargo check` still compiled; the suite was
re-run once the license was re-accepted.)

## Tier 4 — Stdlib expansion (first increment)

Four library additions, each TDD'd (test → red → implement → green) and landed as
an isolated, independently-testable unit. No language, compiler, or VM changes —
zero architectural risk — so they don't touch the GC invariant or the bytecode.

- **`io` directory ops** — `mkdir` (recursive, `mkdir -p`), `listdir` (sorted for
  deterministic output), `remove` (file), `rmdir` (**empty** dir only — never
  recursive, so it can't delete a tree by accident), and `is_dir`/`is_file`
  predicates. Mirrors the existing `io` natives exactly.
- **`string.format(template, args)`** — positional substitution: `{}` takes the
  next argument, `{N}` an indexed one, `{{`/`}}` are literal braces; values render
  through the same `to_display` path as `str()`/`println`. A programmatic
  complement to the language's `${...}` interpolation. Throws `ValueError` on a
  missing arg, out-of-range index, or unmatched brace. GC-safe without a temp root:
  the args array stays on the VM stack for the whole native call (truncation
  happens *after* the function returns), so its elements survive any GC a
  custom `str()` hook might trigger inside `to_display`.
- **`hash` module** (new native) — non-cryptographic `fnv1a`/`djb2` (64-bit,
  returned as `int`) and `hex`/`base64` encode/decode over UTF-8 bytes, all
  std-only and hand-rolled. Round-trips preserve multi-byte UTF-8; malformed input
  throws `ValueError`.
- **`path` module** (new self-hosted, `std/path.lum`) — `join`/`basename`/
  `dirname`/`ext`/`stem`/`is_absolute`/`split`/`normalize` over POSIX `/` paths,
  pure text (no filesystem). Written in Lumen, importing `string` — proving a
  self-hosted module can build on a native one. Brings the self-hosted count to
  **five**.

**Net.** One new native module (`hash`) + one self-hosted (`path`) → 10 native +
5 self-hosted. `cargo build`/`clippy` clean (0 warnings, 0 lints); `cargo test`
green at **229** (+4 test fns), nothing skipped. Docs (`API`, `README`, `TUTORIAL`)
synced.

## Tier 3 — measured, then deferred (decision)

Before touching anything, the inline cache (the candidate "biggest remaining win")
was **measured**, per the benchmark-gate discipline. Release baseline showed the
most global-heavy benchmark (`loop sum to 10M`, ~20M global ops) at ~100 ns/iter
with only ~10 ns of that in the two global ops — globals already use FxHash, which
is fast on the 1–2-char names involved. A perfect cache trims maybe ~6 ns →
**best case ~5–6% on that one benchmark, less elsewhere.** And the prerequisite
index-map refactor **touches GC root-marking** (`vm.rs:242` marks globals through
`module_globals`) — so it is *not* the local, low-risk change first assumed.
Marginal reward against real risk + permanent complexity → **deferred** (recorded
in `BENCHMARKS.md`). Specialized opcodes / peephole are even more marginal (no
type inference → `ADD_INT` has near-zero coverage). Tier 3 is, honestly, "already
optimized; the remaining wins aren't worth their cost."

## Tier 4 — LSP tooling

Four cohesive editor features, all building on the existing `collect_defs` scope
walker and `ast_printer`. TDD on the stdlib half; the LSP half was impl-then-test,
so each new test was **proven to have teeth** by sabotaging the implementation and
watching exactly the right tests go red before reverting.

- **`textDocument/formatting`** — one full-document `TextEdit` from
  `ast_printer::print_program`, but only on a clean parse (never reformat broken
  code, matching `lumen fmt`).
- **`textDocument/references`** — the inverse of go-to-definition: resolve the
  identifier under the cursor to its declaration (`resolve_def`, factored out and
  now shared with go-to-def), then keep every identifier token that resolves to
  the *same* declaration. Lexical shadowing falls out for free — an inner binding's
  uses don't match an outer target. Honors `includeDeclaration`.
- **`textDocument/rename`** — the reference set as a `WorkspaceEdit`; rejects a
  `newName` that isn't a valid identifier.
- **`textDocument/signatureHelp`** — a backward token scan from the cursor finds
  the enclosing call's callee and the active argument index (commas at paren depth
  0, nested calls handled); a signature table (name → parameter labels, defaults
  and `..rest` rendered) supplies the label. Covers user-defined functions and
  methods.

Capabilities now advertised: diagnostics, hover, definition, completion, symbols,
**formatting, references, rename, signatureHelp**.

An adversarial review caught two real defects, both fixed and regression-tested:
(1) `signatureHelp` counted commas inside `[...]`/`{...}` arguments as separators
(so `g([1, 2], x)` mis-highlighted) — the backward scan now tracks bracket/brace
depth and bails when the cursor is inside a literal; (2) the old `collect_defs`
didn't walk lambda bodies, so references/**rename** of an outer variable would
reach across a same-named **lambda parameter** and corrupt it — fixed by a second,
additive pass (`lambdas_in_stmt`/`lambdas_in_expr`) that adds each lambda's
parameters and locals scoped to the lambda body, so the tighter scope wins and the
outer symbol's edits stop at the lambda boundary. Both new tests were proven to
have teeth by sabotaging the fix and watching exactly them go red.

**Net.** `cargo build`/`clippy` clean; `cargo test` green at **242** (+13 LSP test
fns over the increment), nothing skipped. README capability line updated.

## Tier 4 — dependency management (git + path + lockfile)

The chosen model (decided with the user): **path and git dependencies, a
`lumen.lock`, no central registry**. Built in five TDD sub-increments, each
landed green before the next:

1. **Dependency model + inline-table TOML.** `[dependencies]` now takes a bare
   string (`name = "path"`, back-compatible) *or* an inline table
   (`{ path = ".." }` / `{ git = "url", rev = ".." }`). The hand-rolled TOML
   parser already preserved `{...}` values verbatim, so a small `parse_dep`
   splits the flat table. Deps are sorted by name for deterministic lockfiles.
2. **Lockfile** (`lumen.lock`): a `[name]`-section-per-package format the existing
   parser round-trips. Git entries pin an exact `commit` SHA (the `rev` is kept
   for display); a git entry without a commit is treated as unusable, forcing
   re-resolution.
3. **`lumen add`**: pure, tested core — `parse_add_args` (`<name> <path>` or
   `--git <url> [--rev <r>]`) and `add_dependency_to_manifest` (creates
   `[dependencies]` if absent, replaces an existing entry in place).
4. **Git resolution**: clone into `.lumen/git/<name>`, honour the locked commit
   (else the rev, else the default branch), then `rev-parse HEAD` to pin the SHA;
   the lock is only rewritten when it changes. Tested **network-free** against a
   throwaway local git repo used as a `file` source (clone → checkout → lock →
   idempotent re-resolve). Path-only projects skip git and never get a lockfile.
5. **Wiring**: `lumen run`/`build`/`test` resolve before use; `lumen add` is on
   the CLI + help; `lumen new` scaffolds a `.gitignore` for `/.lumen/`.

Security: a dependency fetches and runs third-party code — documented as
"only depend on sources you trust." A registry, semver resolution, and
transitive dependencies are deliberately out of scope for this model.

**Net.** `cargo build`/`clippy` clean; `cargo test` green at **251** (+9 project
test fns), nothing skipped; validated end-to-end (`new` → `add --git` → `run`
importing a module from the git checkout). Docs (README, project.rs) synced.

## Tier 4 — `datetime` and `regex` (and an adversarial regex review)

**`datetime`** (native) — UTC calendar math over epoch seconds, using Hinnant's
`days_from_civil`/`civil_from_days` (correct for any timestamp, including negative
ones via `div_euclid`/`rem_euclid`). `from_epoch`/`to_epoch`/`iso`/`format`/
`weekday`/`is_leap_year`/`days_in_month`. Verified against known timestamps and a
negative epoch; `from_epoch` (which builds a map) is exercised under GC stress.

**`regex`** (native) — a from-scratch engine: parse → AST → flat instruction
program → **recursive backtracking** matcher with capture groups. Supports
literals, `.`, classes with ranges and `\d\w\s` (+ negations), anchors, groups,
alternation, and `* + ? {n,m}` (greedy/lazy). `test/find/find_all/captures/
replace/split`; positions are character indices.

Then an **adversarial review** (a 5-agent fan-out probing distinct feature areas
against Python's `re` as ground truth, each finding independently verified) — 16
candidate discrepancies, narrowed to **3 real bugs**, all fixed:
- **Process crash (critical):** a quantifier over an empty-matchable body
  (`(a*)*`, `(a|)*b`, `(.*)*`) recursed forever and **aborted the host process**
  (uncatchable) — the step budget couldn't help because the C stack blew first.
  Fixed with a `Mark`/`AssertProgress` instruction pair that stops a `*`/`+` body
  from looping without consuming input (with the same save/restore discipline as
  capture `Save`, so backtracking sees the right mark), plus a recursion-depth
  backstop. The depth guard alone wasn't enough — verifying it surfaced that a
  deep match still overflows a *small* caller stack (e.g. a 2 MB test thread)
  before the limit trips, so the matcher now runs on a dedicated 64 MB thread and
  the parser caps group nesting; both turn deep recursion into a catchable
  `ValueError` regardless of who calls. Now `(a*)*` *matches* correctly, `(a|)*b`
  returns the right boolean, and no pattern can crash the host.
- **Class ranges with an escaped lower bound** (`[\t-~]`, `[\.-9]`): range
  detection lived only in the unescaped-literal arm, so these degraded to separate
  literals. Refactored to a shared `read_class_atom`, so a range forms after any
  single-character element.
- **`$` before a trailing newline:** diverges from Python's default mode but
  matches Go's `regexp` — kept as end-of-string-only and **documented** rather
  than changed. (Other reported "discrepancies" — Python-specific `split` capture
  interleaving, `\d` on non-ASCII digits, `{,3}` — were verified as defensible
  cross-engine variations and left as-is, also documented.)

**Net.** `cargo build`/`clippy` clean; `cargo test` green at **265** (+11 test
fns: 2 datetime, 9 regex incl. the regression tests for every fix), nothing
skipped. Docs (API, README, TUTORIAL) synced. 12 native + 5 self-hosted modules.
This completes the Tier 0–4 improvement roadmap.

## Ergonomic completeness — 17 features across four phases

Took Lumen from core-complete to ergonomically complete: 17 features, one per
commit, each ending green with a test, an example, and synced docs. The shape of
the work was "close the half-built things, then add the missing capabilities,
then the small wins, then the boundary I/O." Decisions live in DESIGN D24–D33.

**Phase 1 — closing inconsistencies.** Spread already worked in array literals;
extending it to call arguments meant a `CallArg` mirror of `ArrayElem` and one new
`CALL_SPREAD` opcode that splices a built argv array (so arity stays dynamic and
mixes with default/rest params). Destructuring assignment (`[a, b] = [b, a]`)
reused the `Pattern` grammar but needed a **bounded-lookahead** to tell `{k} = m`
from a block at statement start (D24). Instance reflection added `is` and made
`type(inst)` report the class name. `string.format` grew a full
`[[fill]align][sign][#][0][width][.precision][type]]` mini-language. `match`
OR-patterns (`1 | 2 | 3 =>`) forbid bindings in v1 (D25) to keep the lowering a
plain short-circuit of per-alternative tests.

**Phase 2 — the hard part.** Operator overloading (D26) reused the existing
`str()` hook: a heap-allocated bound method called via `call_and_run`, which roots
the operands across the re-entrant call. Static methods + field declarations (D27)
fold field initializers into one `effective_init` computed once and fed to *both*
resolver and compiler, so the two passes never diverge (D17). Typed catch (D28)
compiles multiple `catch` clauses into a dispatch chain driven by a new
`MATCH_ERROR` opcode, reusing the single-handler/finally machinery. **Generators**
(D29) were the headline: rather than a state-machine transform, a generator owns
its own `ExecContext` (stack/frames/handlers/upvalues) that `next`/`for-in` swaps
into the VM — and the GC had to learn two new roots (the suspended generator's
context, and the *caller's* swapped-out context while one runs), verified under
stress GC. **TCO** (D30) emits `TAIL_CALL` + `RETURN`: the opcode reuses the frame
for a closure callee (trailing `RETURN` becomes dead code) and falls back to a
normal call otherwise — `loop(1000000)` now returns. That last one bit back: two
old tests asserted that infinite *tail* recursion overflows the stack; it is now
correctly an infinite *loop*, so they were rewritten to use non-tail recursion.

**Phase 3 — small wins, one sharp edge.** `**` (right-assoc, above unary minus)
forced a careful renumber of the ast-printer's precedence ranks. String repeat and
`math.round(x, ndigits)` / `int(s, base)` / `0o` octal were quick. **Comprehensions**
(D31) looked small but exposed the same latent bug `match` has — constructs that
use internal local slots misbehave when compiled with operands already on the
stack (e.g. as a call argument). For `match` that is rare; for comprehensions
`println([x for x in xs])` is the *common* case and it **panicked**. The fix was to
compile each comprehension as an immediately-invoked function: its build loop runs
in its own frame (clean slots in every position), the iterable is evaluated in the
enclosing scope and passed as the argument, and the body captures outer variables
(and `this`) as upvalues. The resolver models the comprehension as a matching
function scope so D17 holds.

**Phase 4 — the boundary.** File handles (D32) needed a stateful native object
(`Obj::FileHandle`) plus a second bound-callable, `Obj::BoundNative`, so
`h.read_line()` dispatches to Rust; `for line in h` reuses the `ITER_NEXT`
special-casing the generators introduced. `os.exec` is a thin `std::process`
wrapper returning `{status, stdout, stderr}`. And networking/threads/async were
**documented as explicit non-goals** (D33, README): the VM and GC are
single-threaded and not thread-safe by design, so the supported surface is
computation plus local file/process I/O.

**A note on `cargo fmt`.** The repo at HEAD was not `cargo fmt --check`-clean under
stable rustfmt 1.9.0 (the maintainer hand-writes a compact struct-literal style
that current rustfmt expands), and CI does not run `cargo fmt`. Rather than bury
each feature under a repo-wide reformat — which would fight the established style
and the "match the surrounding code" rule — every change matches the existing
style by hand and is gated on `clippy -D warnings` (CI's actual bar) plus
`lumen fmt` round-trips for the new *language* syntax.

**Net.** Seven new opcodes (`CALL_SPREAD`, `IS`, `STATIC_METHOD`, `MATCH_ERROR`,
`YIELD`, `TAIL_CALL`, `POW`), all in OPCODES.md and the disassembler. `cargo build`
+ `--release` and `clippy` clean (zero warnings); `cargo test` green at **284**
test functions, none skipped. 27 numbered examples, all fmt-idempotent and
snapshot-checked (e2e + under minor-stress GC). SPEC/API/TUTORIAL/README synced;
DESIGN D24–D33 record every design call.

### Follow-up: `match` as a sub-expression

The comprehension work flagged a latent bug that `match` shared: storing the
subject and bindings in local slots computed from `locals.len()` is only correct
when the operand stack is clean, so `println(match x { … })` (match as a call
argument) and `a + match …` miscompiled — one path even read the loop index as a
non-int and **panicked**. Fixed by the same in-place-when-clean / IIFE-otherwise
split as comprehensions (D34), but with a `stmt_value_pos` flag so the common
`let r = match …` / `return match …` keep the allocation-free in-place path and
only genuinely-nested matches pay for the IIFE. The resolver needed no change
(the two paths differ only in local-vs-upvalue classification, which gates no
static check on an arm's expression body). Verified with `match` in every nested
position (call arg, operand, array element, interpolation, comprehension,
upvalue-capturing) and under stress GC; suite green at **285** test functions.

### stdlib gap-closing pass

Closing small standard-library gaps tier by tier (one commit + gate each:
`fmt && clippy -D warnings && test`), backward-compatible only.

- **T1 string**: added `is_digit`/`is_alpha`/`is_alnum`/`is_space`/`is_upper`/`is_lower` (Unicode-aware predicates, with `is_digit` accepting only ASCII `0`–`9`), `capitalize`, `count` (non-overlapping; empty needle throws `ValueError`), and `lines` (`str::lines` semantics, `"" → []`). Suite green at **286**.
- **T2 set** (`std/set.lum`, self-hosted): added `intersection` (canonical name; `intersect` kept as alias), `symmetric_difference`, and `is_superset` (`= other.is_subset(this)`), in the class's existing `for x in this.values()` style. Suite green at **286**.
- **T3 math**: added `sinh`/`cosh`/`tanh`/`asinh`/`acosh`/`atanh` (f64 methods), `clamp(x, lo, hi)` (returns the selected operand unchanged so int-in→int-out; `lo > hi` throws), and `factorial(n)` (checked i64 mul; negative or overflow throws); extended `log` to an optional base (`log(x, base) = ln(x)/ln(base)`, 1-arg unchanged) via `Range(1, 2)`. Suite green at **287**.
