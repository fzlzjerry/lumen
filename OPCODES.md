# The Lumen Instruction Set

Lumen compiles to bytecode for a **stack machine**. Each instruction is a single
opcode byte, optionally followed by fixed-width operand bytes. This document is
the authoritative reference for every opcode: its operands, its effect on the
operand stack, and any side effects. The numeric encoding lives in
[`src/opcode.rs`](src/opcode.rs); the compiler emits it and the VM consumes it.

## Conventions

- **Operand widths.** `u8` is one byte; `u16` is two bytes, **big-endian**.
- **Stack notation.** `[a, b] -> [c]` means the instruction pops `b` then `a`
  (top is rightmost) and pushes `c`. `…` denotes the unchanged stack below.
- **Constants** are indices into the enclosing function's constant pool.
- **Slots** are indices into the current call frame's stack window (slot 0 is the
  callee/`this`). **Upvalues** are indices into the current closure's capture list.
- Jumps encode a **relative** distance; the VM adds it to (or, for `LOOP`,
  subtracts it from) the instruction pointer *after* the operand.

## Literals and stack

| Opcode | Operands | Stack | Effect |
|---|---|---|---|
| `NIL` | — | `… -> …, nil` | Push `nil`. |
| `TRUE` | — | `… -> …, true` | Push `true`. |
| `FALSE` | — | `… -> …, false` | Push `false`. |
| `CONST` | `u16` idx | `… -> …, k` | Push constant `k` (materializing strings/functions). |
| `POP` | — | `…, a -> …` | Discard the top. |
| `POP_N` | `u8` n | `…, x1..xn -> …` | Discard the top `n`. |
| `DUP` | — | `…, a -> …, a, a` | Duplicate the top. |
| `DUP2` | — | `…, a, b -> …, a, b, a, b` | Duplicate the top two (read-modify-write for `obj[i] op= v`). |

## Variables

| Opcode | Operands | Stack | Effect |
|---|---|---|---|
| `DEF_GLOBAL` | `u16` name | `…, v -> …` | Bind global `name = v`. |
| `GET_GLOBAL` | `u16` name | `… -> …, v` | Push global `name` (throws `NameError` if undefined). |
| `SET_GLOBAL` | `u16` name | `…, v -> …, v` | Assign existing global `name = v` (throws if undefined). |
| `GET_LOCAL` | `u8` slot | `… -> …, v` | Push frame slot. |
| `SET_LOCAL` | `u8` slot | `…, v -> …, v` | Store into frame slot (value left on stack). |
| `GET_UPVALUE` | `u8` idx | `… -> …, v` | Push captured upvalue. |
| `SET_UPVALUE` | `u8` idx | `…, v -> …, v` | Store into upvalue. |
| `CLOSE_UPVALUE` | — | `…, v -> …` | Hoist the top local to the heap and pop it. |
| `CLOSE_UPVALUE_SLOT` | `u8` slot | `…` | Close any upvalue capturing the frame slot **without** popping (per-iteration `for` capture). |

## Arithmetic, comparison, logic

All pop two operands (one for `NEG`/`NOT`) and push one result, per SPEC §6.4.

| Opcode | Stack | Effect |
|---|---|---|
| `ADD` `SUB` `MUL` `DIV` `REM` | `…, a, b -> …, c` | Numeric (or `+` on strings/arrays); int×int stays int; `/` by 0 throws. |
| `NEG` | `…, a -> …, b` | Numeric negation. |
| `NOT` | `…, a -> …, b` | Logical negation (always a bool). |
| `EQ` `NE` | `…, a, b -> …, c` | Equality per SPEC §6.2. |
| `LT` `LE` `GT` `GE` | `…, a, b -> …, c` | Ordered comparison of numbers or strings. |
| `IS` | `…, v, class -> …, c` | `v is class`: `true` iff `v` is an instance of `class` or a subclass; `class` must be a class (else TypeError). |
| `BIT_AND` `BIT_OR` `BIT_XOR` | `…, a, b -> …, c` | Integer bitwise and/or/xor (TypeError on non-int). |
| `SHL` `SHR` | `…, a, b -> …, c` | Integer shift left / arithmetic shift right; amount in `0..=63`. |
| `BIT_NOT` | `…, a -> …, b` | Integer bitwise complement (`~`). |

## Control flow

| Opcode | Operands | Stack | Effect |
|---|---|---|---|
| `JUMP` | `u16` d | `…` | `ip += d`. |
| `JUMP_IF_FALSE` | `u16` d | `…, c -> …, c` | If `c` is falsy, `ip += d` (condition **not** popped). |
| `LOOP` | `u16` d | `…` | `ip -= d` (backward jump). |

## Functions and calls

| Opcode | Operands | Stack | Effect |
|---|---|---|---|
| `CALL` | `u8` argc | `…, f, a1..aN -> …, r` | Call `f` with `argc` args; checks arity. |
| `CALL_SPREAD` | — | `…, f, argv -> …, r` | Call `f` spreading the freshly built array `argv` as the argument list (`f(..xs)`); argument count is determined at runtime and arity-checked. |
| `INVOKE` | `u16` name, `u8` argc | `…, recv, a1..aN -> …, r` | `recv.name(args)`: fused property-read + call; skips bound-method allocation for instance methods. |
| `SUPER_INVOKE` | `u16` name, `u8` argc | `…, this, a1..aN, super -> …, r` | `super.name(args)`: pops the superclass, resolves `name` in it, and calls with `this` as receiver; skips the bound-method allocation. |
| `DEFAULT_ARG` | `u8` index, `u16` d | `…` | At function entry: if parameter `index` was supplied, `ip += d` (skip its default expression); else fall through to evaluate it. |
| `CLOSURE` | `u16` proto, then `(u8 is_local, u8 index)×upvalues` | `… -> …, closure` | Build a closure, capturing the listed upvalues from the enclosing frame's locals (`is_local=1`) or upvalues (`is_local=0`). |
| `RETURN` | — | `…, v -> ` | Return `v` from the current function; close its open upvalues. |

## Collections

| Opcode | Operands | Stack | Effect |
|---|---|---|---|
| `NEW_ARRAY` | — | `… -> …, []` | Push an empty array. |
| `ARRAY_PUSH` | — | `…, arr, v -> …, arr` | Append `v` to `arr`. |
| `ARRAY_EXTEND` | — | `…, arr, it -> …, arr` | Append all elements of iterable `it`. |
| `NEW_MAP` | — | `… -> …, {}` | Push an empty map. |
| `MAP_INSERT` | — | `…, m, k, v -> …, m` | Insert `k -> v`. |
| `INDEX_GET` | — | `…, o, i -> …, o[i]` | Index an array/string/map. |
| `INDEX_SET` | — | `…, o, i, v -> …, v` | Assign `o[i] = v`. |

## Objects and classes

| Opcode | Operands | Stack | Effect |
|---|---|---|---|
| `GET_PROP` | `u16` name | `…, o -> …, v` | Field read or bound-method lookup. |
| `SET_PROP` | `u16` name | `…, o, v -> …, v` | Set field `o.name = v`. |
| `CLASS` | `u16` name | `… -> …, class` | Push a new empty class. |
| `INHERIT` | — | `…, super, class -> …, super, class` | Copy `super`'s methods into `class` (copy-down inheritance). |
| `METHOD` | `u16` name | `…, class, closure -> …, class` | Add a method to the class. |
| `STATIC_METHOD` | `u16` name | `…, class, closure -> …, class` | Add a static method (no receiver) to the class's static table. |
| `GET_SUPER` | `u16` name | `…, recv, super -> …, bound` | Resolve `name` in `super` and bind it to `recv`. |

## Exceptions

| Opcode | Operands | Stack | Effect |
|---|---|---|---|
| `PUSH_HANDLER` | `u16` d | `…` | Register a handler whose catch target is `ip + d`, recording the stack height and frame. |
| `POP_HANDLER` | — | `…` | Remove the most recent handler (try block ended normally). |
| `THROW` | — | `…, v -> ` | Throw `v`; unwind to the nearest handler (or abort with a stack trace). |

## Strings, modules, iteration, patterns

| Opcode | Operands | Stack | Effect |
|---|---|---|---|
| `INTERPOLATE` | `u8` n | `…, p1..pN -> …, s` | Stringify and concatenate the top `n` values. |
| `IMPORT` | `u16` path | `… -> …, module` | Load (compile + run once, cached) the named module. |
| `ITER_NEXT` | `u8` iter, `u8` idx, `u16` d | `… -> …, elem` or `…` | If `idx` slot is past the end of the `iter` slot, jump `d`; else push the current element and increment the index. |
| `MATCH_ARRAY` | `u8` len, `u8` exact | `…, v -> …, b` | Push `true` iff `v` is an array of exactly/at-least `len` elements. |
| `MATCH_MAP_HAS` | `u16` key | `…, v -> …, b` | Push `true` iff `v` is a map containing `key`. |
| `MATCH_ERROR` | `u16` kind | `…, v -> …, b` | Push `true` iff `v` is a built-in error whose `.kind` equals the string constant (typed `catch` dispatch). |
| `ARRAY_REST` | `u8` front, `u8` back | `…, arr -> …, sub` | Push `arr[front .. len-back]` (binds an array pattern's `..rest`). |
| `YIELD` | — | `…, v -> …` | Suspend the running generator, handing `v` to `next`/`for-in`; resume at the next instruction (DESIGN D29). |

## Worked example

`let x = 1 + 2 * 3;` compiles to (see `lumen disasm`):

```
CONST       0 ; 1
CONST       1 ; 2
CONST       2 ; 3
MUL                 ; 2 * 3
ADD                 ; 1 + (2 * 3)
DEF_GLOBAL  3 ; "x"
```

Operator precedence is already resolved by the parser, so the byte stream is a
flat post-order traversal of the expression tree.
