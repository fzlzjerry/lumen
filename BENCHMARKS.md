# Lumen Benchmarks

Run with `lumen bench` (use a **release** build — `cargo build --release` — for
meaningful numbers; the debug build is ~10× slower). Each benchmark times the
`interpret()` call only (compilation excluded). Numbers below are from one
machine (8 cores, rustc 1.96, `--release`, `lto=thin`, `codegen-units=1`); treat
them as orders of magnitude, not guarantees — they vary run to run.

## Results

Two columns: the original implementation, and after the Phase A optimization
pass (clone-free name reads, the `INVOKE` super-instruction, a custom FxHash
hasher, and moving the GC trigger to back-edges/calls).

| Benchmark              | What it stresses                       | Before  | After   |
|------------------------|----------------------------------------|---------|---------|
| `fib(32)` recursive    | call/return + frame setup              | ~1.2 s  | ~0.88 s |
| loop sum to 10M        | dispatch loop + global access          | ~2.3 s  | ~1.4 s  |
| array alloc ×1M        | allocation + mark-sweep GC             | ~0.26 s | ~0.19 s |
| string build ×100k     | immutable-string concatenation (`+`)   | ~4.4 s  | ~4.0 s  |
| method dispatch ×1M    | property lookup + method call          | ~0.40 s | ~0.27 s |

A broad ~25–40% speedup, except the string benchmark (an O(n²) antipattern — see
below). What the pass did:

- **Clone-free names.** `GET_GLOBAL`/`SET_GLOBAL`/`INVOKE` used to clone the
  name `String` from the constant pool on *every* execution (≈20M allocations in
  the loop benchmark). They now read it as a `&str` borrowing the frame's
  prototype (a disjoint field), and `SET_GLOBAL` updates in place via `get_mut`.
- **`INVOKE` super-instruction.** `obj.method(args)` calls instance methods
  directly with the receiver already in slot 0 — no bound-method allocation.
- **FxHash.** Global/field/method/map-key/intern tables now use a custom
  FxHash-style hasher (std-only, in `src/fxhash.rs`) instead of SipHash, which is
  slow for the short string keys an interpreter hashes constantly.
- **GC trigger off the hot path.** The heap-pressure check moved from every
  instruction to back-edges (`LOOP`) and calls; stress mode still collects
  before every instruction to keep the root-completeness test strong.

## Reading the numbers

- **GC is healthy.** Allocating a million throwaway arrays takes ~0.26 s and the
  live set stays bounded (see `tests/gc_stress.rs`) — the collector reclaims
  garbage continuously rather than growing without bound.

- **Method dispatch is allocation-bound.** Each `obj.method()` currently builds a
  *bound method* object on the heap, so 1M calls = 1M short-lived allocations.
  This is the clearest target for an `INVOKE` super-instruction (look up + call in
  one op, no bound-method allocation) — a standard clox optimization deferred per
  the "make it run first" principle.

- **Global access dominates the loop benchmark.** `s = s + i` at top level reads
  and writes the global `s` every iteration, each a hash-map lookup keyed by the
  string `"s"`. Locals (stack-slot indexed) are much faster — a function-local
  loop runs noticeably quicker. A global-binding cache would close most of the
  gap.

- **String building is O(n²) — by design, with an idiom to avoid it.** Strings
  are immutable and interned (DESIGN D9), so `s + "x"` in a loop copies and
  re-interns the whole growing string each time. For heavy building, accumulate
  pieces in an array and `string.join` them once:

  ```lumen
  import "string" as str;
  let parts = [];
  for let i = 0; i < 100000; i = i + 1 { push(parts, "x"); }
  let s = str.join(parts, "");   // one allocation, O(n)
  ```

  The benchmark deliberately uses the naive form to make the cost visible.

## Remaining opportunities (future work)

Items 1, 3, and (partially) 4 from the original list were done in Phase A. What
remains:

1. **A global-variable inline cache** (resolve the name to a slot once, reuse).
   This is the biggest remaining win for global-heavy loops but needs *mutable*
   chunks (or a side cache table) to patch the resolved slot back, since Lumen's
   chunks are immutable `Rc<FnProto>`s — a real architectural change rather than a
   local edit, so it's deferred deliberately.
2. **`SUPER_INVOKE`** to fuse `super.m()` the way `INVOKE` fuses `obj.m()`.
3. **Specialized fast-path opcodes** (`ADD_INT`, `GET_LOCAL_0`).
4. **String building** stays O(n²): strings are immutable and interned (DESIGN
   D9), so making `+` produce non-interned strings would speed concatenation but
   force `map`-key use to intern on every access — a worse trade for map-heavy
   code. The idiom remains: accumulate in an array and `string.join` once.
