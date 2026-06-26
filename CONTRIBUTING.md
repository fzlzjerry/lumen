# Contributing to Lumen

Lumen is a complete dynamic programming language implemented in **Rust, using
only the standard library** (no external crates — DESIGN D1). It is a classic
compiler pipeline feeding a stack-based bytecode VM with a hand-written
mark-and-sweep garbage collector, plus a standard library and a toolchain (REPL,
debugger, formatter, language server, project manager).

This guide is the map and the rules. The *what* of the language lives in
[`SPEC.md`](SPEC.md); the *why* of the implementation lives in
[`DESIGN.md`](DESIGN.md); the build diary is [`JOURNAL.md`](JOURNAL.md); the
instruction set is [`OPCODES.md`](OPCODES.md). Read those before making
non-trivial changes.

---

## The pipeline

```
source ─▶ Lexer ─▶ Parser ─▶ Resolver ─▶ Compiler ─▶ Chunk ─▶ VM (+ GC)
```

Each stage lives in its own module and is independently testable. `src/lib.rs`
wires them together and exposes `parse_source` (lex + parse) and `check_source`
(lex + parse + resolve).

### Project layout (`src/`)

| File | Responsibility |
|------|----------------|
| `span.rs` | Source positions and spans (1-based line/col + byte offsets). |
| `diagnostics.rs` | `Diagnostic` + the caret-underlined error renderer. |
| `token.rs` | `Token`/`TokenKind`, keyword table, interpolation `StrPart`s. |
| `lexer.rs` | The hand-written, non-panicking, error-recovering lexer. |
| `ast.rs` | The span-carrying AST (statements, expressions, patterns). |
| `parser.rs` | Recursive-descent + Pratt parser with panic-mode recovery. |
| `ast_printer.rs` | AST → normalized source (the formatter; round-trip oracle). |
| `resolver.rs` | Static semantic analysis: collects every static error. |
| `builtins.rs` | The canonical *names* of the global builtins (shared truth). |
| `opcode.rs` | The `#[repr(u8)]` instruction set + `from_u8`/`name`. |
| `chunk.rs` | Bytecode chunks, the constant pool, `FnProto`. |
| `compiler.rs` | AST → bytecode: slot allocation, upvalues, jump backpatching. |
| `disassembler.rs` | Chunk → readable assembly (debugging + tests). |
| `value.rs` | `Value` (a `Copy` enum), `GcRef`, `MapKey`, error kinds. |
| `object.rs` | Heap object types (`Obj`): strings, arrays, maps, closures, … |
| `gc.rs` | The heap: allocation, string interning, mark-and-sweep. |
| `vm.rs` | The stack VM: dispatch loop, calls, exceptions, modules, GC roots. |
| `vm/builtins.rs` | Implementations of the global builtins (`print`, `len`, …). |
| `stdlib/*.rs` | Native modules (`math`, `string`, `array`, `map`, `io`, `os`, `time`, `json`, `random`) loaded by `import`. |
| `repl.rs` | The interactive REPL (multi-line, persistent, eval-and-print). |
| `debugger.rs` | A source-level debugger (breakpoints, step, locals, disasm). |
| `highlight.rs` | ANSI syntax highlighting (used by the REPL and `:hl`). |
| `lsp.rs` | A minimal stdio language server (diagnostics + hover). |
| `project.rs` | `lumen.toml` manifest parsing and `new`/`build`/`run`/`test`. |
| `util.rs` | Shared formatting helpers (float/string formatting). |

`std/seq.lum` is a **self-hosted** stdlib module — written in Lumen, embedded via
`include_str!` — proving the language can implement its own library. Example
programs live in `examples/`.

---

## Building & testing

```sh
cargo build              # debug build (the lint bar: ZERO warnings)
cargo test               # the whole suite — must be 100% green
cargo build --release    # optimized build (panic=abort, thin-LTO)
./target/release/lumen bench   # micro-benchmarks → numbers in BENCHMARKS.md
```

The suite is library unit tests **plus** integration suites in `tests/`:

| Suite | What it guards |
|-------|----------------|
| `e2e.rs` | Runs every `examples/*.lum` and snapshot-compares its full output (`tests/expected/`). |
| `errors.rs` | One+ test per error class — lexical, parser, resolver, and each runtime error *kind*. |
| `fuzz.rs` | Thousands of random / token-soup / byte-mutated inputs through the front end: it must never panic. |
| `vm.rs` | End-to-end runtime behavior (closures, classes, exceptions, match, …). |
| `stdlib.rs` | The native + self-hosted modules. |
| `gc_stress.rs` | Correctness under collect-before-every-instruction, plus bounded-memory pressure tests. |
| `coverage.rs` | Breadth over runtime error paths, the stdlib, and the debug API. |

### Coverage

```sh
cargo install cargo-llvm-cov        # once
cargo llvm-cov --summary-only       # per-file region/function/line coverage
```

**The bar.** A change is not done until: `cargo build` produces **zero
warnings**, **all tests pass**, **no test is skipped/ignored**, and the **core
components stay at ≥90% line coverage** (lexer, parser, resolver, compiler,
disassembler, gc, ast_printer, vm). To update an `e2e` snapshot intentionally,
re-run the example and replace its `.txt` — never edit a snapshot to mask a
behavior change without understanding it.

---

## Invariants you must respect

These come straight from `DESIGN.md`; breaking one tends to produce subtle,
non-local bugs.

- **GC collects only at instruction boundaries (D18).** The dispatch loop is the
  only safe point — there every live object is reachable from a root (the value
  stack, frames, per-module globals, builtins, open upvalues, the module cache,
  the in-flight thrown value). **Native code that re-enters the VM** (anything
  calling `call_and_run` — `sort`/`map`/`filter`/`reduce`, custom `str()` in
  interpolation) **must root any heap value it holds in a Rust local** across the
  call, via `vm.push_temp_root(v)` / `vm.update_top_temp_root(v)` /
  `vm.pop_temp_root()`. Source elements stay alive through the still-on-stack
  argument array; accumulators and intermediate results do not — root them.
  Stress mode plus the dangling-handle panic in `Heap::get` turn a missed root
  into a loud test failure.
- **The resolver validates; the compiler allocates (D17).** The resolver assigns
  no slots and builds no upvalue tables — it only reports static errors. The
  compiler re-derives the same lexical facts and is the sole authority on layout.
  Keep the scope rules (SPEC §5) identical in both so they never disagree on a
  name's classification (local / upvalue / global).
- **`Value` is `Copy` (it is an immediate or a `GcRef`).** Don't add a non-`Copy`
  field to it. The heap owns the data; values just point at it. This is what lets
  the VM push/pop and pass arguments without clones or borrow-checker fights.
- **Per-module globals (D19).** A closure resolves globals against *its* defining
  module, not the running one. Every `Closure` carries a module index; frames
  resolve `GET/SET/DEFINE_GLOBAL` against it. Don't reintroduce a single global
  table.
- **The SPEC leads, examples follow.** If an example needs a feature the SPEC
  doesn't describe, change the example (or update the SPEC deliberately) — don't
  grow the language to fit a sample.
- **Borrow discipline in runtime/stdlib code:** read what you need out of
  `vm.heap.get(r)` into owned/`Copy` data, drop the borrow, *then* allocate or
  mutate. An immutable heap borrow must never overlap a `make_error`/`alloc`.

---

## Recipes

### Add a new opcode

1. Add the variant to the `OpCode` enum in `src/opcode.rs` (append at the end so
   existing discriminants are stable), with a doc comment giving its operands and
   stack effect.
2. Update `OpCode::from_u8`'s upper bound (the `b <= OpCode::Last as u8` check)
   and add its mnemonic to `OpCode::name`.
3. Document it in [`OPCODES.md`](OPCODES.md) (operands, stack `[in] -> [out]`,
   effect).
4. Emit it from `src/compiler.rs` (use the `emit_op` / `emit_op_u8` /
   `emit_op_u16` / `emit_jump` helpers).
5. Execute it in `src/vm.rs`'s `step()` match.
6. Decode it in `src/disassembler.rs`'s `disassemble_instruction` (operand width
   and rendering).
7. Add a compiler test (assert the disassembly) and a VM behavior test.

Keep the four sites in sync — the `#[repr(u8)]` enum is the single source of
truth, but the disassembler must know each opcode's operand width.

### Add a standard-library function

Native functions have the signature
`fn(&mut Vm, &[Value]) -> Result<Value, Value>` (`Ok` = result, `Err` = a thrown
value). Inside a module (e.g. `src/stdlib/math.rs`):

```rust
pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        // constants are plain values; functions go through `f`
        f(vm, "myfn", Arity::Exact(2), myfn),
    ];
    vm.make_module("math", exports)
}

fn myfn(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let x = num(vm, a[0])?;          // typed-arg helpers from stdlib/mod.rs:
    let name = string_of(vm, a[1])?; // num / int / string_of / array_of
    // ... compute ...
    Ok(vm.new_string(&name))         // alloc helpers: new_string / new_array / make_module
}
```

- Pick an `Arity` (`Exact`, `AtLeast`, `Range`); the VM checks it before calling,
  so inside the function the count is valid.
- Use the arg helpers (`num`, `int`, `string_of`, `array_of`) and `err(vm, kind,
  msg)` for type errors — error `kind`s come from `value::error_kind`.
- If the function calls back into Lumen (a callback argument), follow the GC
  rooting rule above (see `stdlib/array.rs::map` for the pattern).
- To register a brand-new module, add a `mod` line and a `load` arm in
  `src/stdlib/mod.rs`; document the functions in `API.md`.
- Add an integration test in `tests/stdlib.rs` and an error-path case in
  `tests/coverage.rs`.

---

## Style

- **Match the surrounding code** — its naming, comment density, and idioms. Read
  the neighbours before adding.
- **Comment the *why*, not the *what*.** Module headers explain the design; inline
  comments justify non-obvious choices.
- **Keep the docs honest.** When you change observable behavior or make a design
  call, update `SPEC.md` / `DESIGN.md` (add a new `D##` entry) and add a
  retrospective note to `JOURNAL.md`. Treat a stale doc as a bug.
- **No shortcuts:** no `TODO`/placeholder/fake implementations, no parser
  generators, no skipped tests.
- **Finish green.** Every change ends with `cargo build` warning-free and
  `cargo test` fully passing.
