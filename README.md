# Lumen

[![CI](https://github.com/fzlzjerry/lumen/actions/workflows/ci.yml/badge.svg)](https://github.com/fzlzjerry/lumen/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lumen-lang.svg)](https://crates.io/crates/lumen-lang)
[![Homebrew](https://img.shields.io/badge/homebrew-fzlzjerry%2Flumen-orange)](https://github.com/fzlzjerry/homebrew-lumen)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Lumen** is a small, dynamically-typed programming language with a hand-written
compiler, a bytecode virtual machine, and a generational garbage collector — all
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
- **Classes** with single inheritance, `super`, constructors, static methods,
  field declarations, a custom `str()` hook, and **operator overloading** via
  dunder methods (`__add__`, `__eq__`, `__index__`, …). `type(instance)` gives
  the class name, and `is` tests class membership.
- **Control flow**: `if`/`else`, `while`, C-style and `for-in` loops,
  `break`/`continue`, and **tail-call optimization** (deep recursion runs in
  constant stack).
- **Generators**: `yield` produces lazy sequences, driven by `for-in` / `next`.
- **Exceptions**: `throw` / `try` / `catch` / `finally` (catch is optional),
  with **typed catch** (`catch (IndexError e)`), built-in error objects, and
  full stack traces.
- **Collections**: array and map literals, **comprehensions**
  (`[e for x in it if c]`), spread (`..`) in literals *and* calls, negative
  indexing, insertion-ordered maps.
- **Operators**: arithmetic incl. `**` (exponentiation), bitwise/shift, string
  repeat (`"ab" * 3`), and hex/binary/octal int literals.
- **String interpolation**: `"${expr}"`, nestable, with escapes incl.
  `\u{...}`, plus rich `string.format` specifiers (`{:.2f}`, `{:>8}`, `{:x}`).
- **Pattern matching**: `match` over literals, bindings, wildcards, arrays
  (with `..rest`), maps, and `|` alternations, with arm guards.
- **Destructuring**: in `let` and as assignment (`[a, b] = [b, a]`).
- **Modules**: `import "name";`, aliasing, and selective imports; per-module
  global scope.
- **Local I/O**: file handles (`io.open` with line iteration) and subprocesses
  (`os.exec`).
- **A real GC**: a handle-based, generational mark-and-sweep collector, in safe
  Rust, that collects cycles and interns strings.
- **A full toolchain**: REPL, source-level debugger, formatter, language server,
  and a `lumen.toml` project/test runner.

See [`SPEC.md`](SPEC.md) for the complete language specification (lexical and
syntactic grammar in EBNF, type/evaluation/scope semantics, error and memory
models), and [`DESIGN.md`](DESIGN.md) for the rationale behind the design.

## Non-goals

Lumen is deliberately a **single-threaded language for computation and local I/O**.
Its VM and garbage collector are **not thread-safe** by design (the GC uses a
handle arena with no synchronization, and values are not `Send`/`Sync`), which
keeps the runtime simple and fast for its target use. As a direct consequence,
the following are **out of scope** and intentionally not provided:

- **Threads, concurrency, or parallelism** — no threads, no shared-memory
  concurrency, no parallel execution.
- **`async`/`await` or an event loop** — there is no asynchronous runtime.
- **Networking** — no sockets, HTTP, or other network I/O.

The supported surface is **computation plus local file and process I/O** (`io`,
`os`). Programs needing networking or concurrency should shell out via
`os.exec` to a tool built for that, rather than expect it in the language.
(See DESIGN D33.)

## Install

Lumen ships through several channels — the command is always `lumen`.

```sh
# Homebrew (macOS / Linux)
brew install fzlzjerry/lumen/lumen

# crates.io (any platform with a Rust toolchain) — installs the `lumen` binary
cargo install lumen-lang

# From source (stable Rust 1.82+, no dependencies)
cargo install --git https://github.com/fzlzjerry/lumen
```

**Debian / Ubuntu** — add the signed apt repository:

```sh
curl -fsSL https://lumen.moraxcheng.me/lumen-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/lumen.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/lumen.gpg] https://lumen.moraxcheng.me ./" \
  | sudo tee /etc/apt/sources.list.d/lumen.list
sudo apt update && sudo apt install lumen
```

Or download a pre-built binary — `.tar.gz` for macOS/Linux (arm64/x86_64) or a
`.deb` — straight from the
[latest release](https://github.com/fzlzjerry/lumen/releases/latest). Maintainers:
see [`docs/RELEASING.md`](docs/RELEASING.md) for how releases are cut.

## Quick start

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
| `add <name> <src>`     | Add a dependency — a local path, or `--git <url> [--rev <r>]` |
| `build`                | Static-check every source file in a project             |
| `test`                 | Run the project's `tests/*.lum` files                   |
| `lsp`                  | Language server (diagnostics, hover, go-to-def, completion, symbols, formatting, references, rename, signature help) |
| `bench`                | Run the micro-benchmarks                                |
| `lex` / `parse <file>` | Inspect tokens / run the front end                      |

## Dependencies

A project declares dependencies in `lumen.toml`, by **local path** or **git URL**
(there is no central registry). `lumen add` edits the manifest for you:

```sh
lumen add utils ../utils                              # a local path
lumen add mathlib --git https://github.com/u/mathlib  # a git repo (default branch)
lumen add mathlib --git https://github.com/u/mathlib --rev v1.2.0   # pinned ref
```

```toml
[dependencies]
utils   = "../utils"
mathlib = { git = "https://github.com/u/mathlib", rev = "v1.2.0" }
```

On `lumen run`/`build`/`test`, git sources are cloned into `.lumen/git/<name>`
and pinned to an exact commit in **`lumen.lock`** for reproducible builds — commit
the lockfile; the `.lumen/` cache is git-ignored. Each dependency's directory joins
the module search path, so its modules are importable by name (`import "mathlib";`).
A dependency runs third-party code, so only depend on sources you trust.

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
- **Compiler** ([`src/compiler.rs`](src/compiler.rs)) — emits a 67-instruction
  bytecode (documented in [`OPCODES.md`](OPCODES.md)) with constant pools, jump
  backpatching, local-slot allocation, and clox-style upvalues.
- **VM** ([`src/vm.rs`](src/vm.rs)) — a stack machine with call frames, closures,
  class/method dispatch, exception unwinding, and a re-entrant runner that lets
  native functions call back into Lumen.
- **GC** ([`src/gc.rs`](src/gc.rs)) — a generational mark-and-sweep collector over
  a handle-indexed heap; no `unsafe`, collects cycles, interns strings.

The standard library ([`src/stdlib/`](src/stdlib/)) provides native `math`,
`string`, `array`, `map`, `io`, `os`, `time`, `json`, `random`, `hash`
(non-cryptographic hashing + hex/base64), `datetime` (UTC calendar math), and
`regex` (a small from-scratch engine) modules, plus five self-hosted modules
written in Lumen itself ([`std/`](std/)): `seq` (sequence utilities), `set` (a
hash set), `functional` (closures: compose, curry, memoize), `path` (POSIX path
manipulation), and `testing` (a unit-test harness). Full reference:
[`API.md`](API.md).

## Building, testing, and contributing

```sh
cargo build            # debug build (zero warnings)
cargo test             # all 265 tests: unit + e2e snapshots + errors + fuzz + GC stress
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
