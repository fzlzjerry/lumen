# Lumen

**Lumen** is a small, dynamically-typed programming language with a hand-written
compiler, a bytecode virtual machine, and a tracing garbage collector — all
implemented from scratch in **Rust**, using only the standard library (no
parser generators, no runtime crates). It is in the lineage of Lua and *Crafting
Interpreters*' clox, extended with closures, classes with single inheritance,
modules, exceptions, first-class collections, string interpolation, and pattern
matching.

```lumen
class Greeter {
    init(name) { this.name = name; }
    hello() { return "Hello, ${this.name}!"; }
}

fn map(xs, f) {
    let out = [];
    for x in xs { push(out, f(x)); }
    return out;
}

println(Greeter("world").hello());
println(map([1, 2, 3, 4], fn(n) { return n * n; }));   // [1, 4, 9, 16]

let label = match [3 % 3, 3 % 5] {
    [0, 0] => "FizzBuzz",
    [0, _] => "Fizz",
    _      => str(3),
};
println(label);
```

## Features

- **Dynamic types**: `int` (64-bit), `float` (64-bit), `string`, `bool`, `nil`,
  `array`, `map`, plus functions, classes, instances, and modules.
- **Variables & constants** (`let` / `const`), block-scoped.
- **First-class functions & closures** with per-iteration loop capture.
- **Classes** with single inheritance, `super`, constructors, and a custom
  `str()` hook.
- **Control flow**: `if`/`else`, `while`, C-style and `for-in` loops,
  `break`/`continue`.
- **Exceptions**: `throw` / `try` / `catch` / `finally` (catch is optional),
  with typed built-in error objects and full stack traces.
- **Collections**: array and map literals, spread (`..`), negative indexing,
  insertion-ordered maps.
- **String interpolation**: `"${expr}"`, nestable, with escapes incl.
  `\u{...}`.
- **Pattern matching**: `match` over literals, bindings, wildcards, arrays
  (with `..rest`), and maps, with arm guards.
- **Modules**: `import "name";`, aliasing, and selective imports; per-module
  global scope.
- **A real GC**: handle-based mark-and-sweep, in safe Rust, that collects cycles
  and interns strings.
- **A full toolchain**: REPL, source-level debugger, formatter, language server,
  and a `lumen.toml` project/test runner.

See [`SPEC.md`](SPEC.md) for the complete language specification (lexical and
syntactic grammar in EBNF, type/evaluation/scope semantics, error and memory
models), and [`DESIGN.md`](DESIGN.md) for the rationale behind the design.

## Quick start

Lumen builds with a stable Rust toolchain (1.96+) and has no dependencies.

```sh
cargo build --release          # binary at target/release/lumen

lumen run program.lum          # compile and execute a file
lumen repl                     # interactive session (multi-line, persistent)
lumen new myapp && cd myapp    # scaffold a project
lumen run                      # run the project (reads lumen.toml)
lumen test                     # run tests/*.lum
```

There are 19 worked example programs in [`examples/`](examples/), each exercising
a feature area — start with `examples/01_hello.lum` and read up. The later ones
are full programs: an arithmetic interpreter (`17_calculator`), classic data
structures (`18_data_structures`), and a JSON toolkit (`19_json_tool`). The
[`TUTORIAL.md`](TUTORIAL.md) walks from hello-world to advanced features.

## The `lumen` command

| Command                | What it does                                            |
|------------------------|---------------------------------------------------------|
| `run <file>`           | Compile and execute a program                           |
| `run`                  | Run the current project's entry point (`lumen.toml`)    |
| `repl`                 | Interactive REPL (multi-line, history, highlighting)    |
| `debug <file>`         | Source-level debugger (breakpoints, step, inspect)      |
| `fmt [--write] <file>` | Format source (canonical layout; in place with `--write`) |
| `disasm <file>`        | Disassemble a program to readable bytecode              |
| `new <name>`           | Scaffold a new project                                  |
| `build`                | Static-check every source file in a project             |
| `test`                 | Run the project's `tests/*.lum` files                   |
| `lsp`                  | Run the language server (stdio; diagnostics + hover)    |
| `bench`                | Run the micro-benchmarks                                |
| `lex` / `parse <file>` | Inspect tokens / run the front end                      |

## How it works

Lumen is a classic compiler pipeline feeding a stack VM:

```
source ─▶ Lexer ─▶ Parser ─▶ Resolver ─▶ Compiler ─▶ Chunk(bytecode) ─▶ VM (+ GC)
```

- **Lexer** ([`src/lexer.rs`](src/lexer.rs)) — hand-written, with line/column
  tracking, nestable block comments, full number/string syntax, and in-place
  recursive scanning of `${...}` interpolations.
- **Parser** ([`src/parser.rs`](src/parser.rs)) — recursive descent with Pratt
  precedence climbing and panic-mode error recovery (reports many errors per
  run). Produces a span-carrying AST.
- **Resolver** ([`src/resolver.rs`](src/resolver.rs)) — a validation pass:
  undefined variables, const reassignment, `this`/`super`/`break`/`continue`/
  `return` context, duplicate declarations.
- **Compiler** ([`src/compiler.rs`](src/compiler.rs)) — emits a 58-instruction
  bytecode (documented in [`OPCODES.md`](OPCODES.md)) with constant pools, jump
  backpatching, local-slot allocation, and clox-style upvalues.
- **VM** ([`src/vm.rs`](src/vm.rs)) — a stack machine with call frames, closures,
  class/method dispatch, exception unwinding, and a re-entrant runner that lets
  native functions call back into Lumen.
- **GC** ([`src/gc.rs`](src/gc.rs)) — a tracing mark-and-sweep collector over a
  handle-indexed heap; no `unsafe`, collects cycles, interns strings.

The standard library ([`src/stdlib/`](src/stdlib/)) provides native `math`,
`string`, `array`, `map`, `io`, `os`, `time`, `json`, and `random` modules, plus
four self-hosted modules written in Lumen itself ([`std/`](std/)): `seq`
(sequence utilities), `set` (a hash set), `functional` (closures: compose, curry,
memoize), and `testing` (a unit-test harness). Full reference: [`API.md`](API.md).

## Building, testing, and contributing

```sh
cargo build            # debug build (zero warnings)
cargo test             # all 159 tests: unit + e2e snapshots + errors + fuzz + GC stress
cargo build --release  # optimized build
cargo llvm-cov --summary-only   # coverage (core components are ≥90%)
```

The build is warning-free, all tests pass, and nothing is skipped. The
[`JOURNAL.md`](JOURNAL.md) is a phase-by-phase build diary, and
[`CONTRIBUTING.md`](CONTRIBUTING.md) explains the architecture and how to extend
the language safely (especially the GC rooting invariant for native code).

## Documentation

- [`SPEC.md`](SPEC.md) — the language specification
- [`TUTORIAL.md`](TUTORIAL.md) — a guided tour, hello-world to advanced
- [`API.md`](API.md) — the standard-library reference
- [`OPCODES.md`](OPCODES.md) — the bytecode instruction set
- [`DESIGN.md`](DESIGN.md) — design decisions and rationale
- [`BENCHMARKS.md`](BENCHMARKS.md) — performance numbers and analysis
- [`JOURNAL.md`](JOURNAL.md) — the build diary
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — architecture & contribution guide

## License

MIT.
