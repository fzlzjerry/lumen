# Lumen Standard Library — API Reference

This is the reference for every built-in function and module shipped with Lumen.
For language semantics (types, evaluation, scope, errors) see [`SPEC.md`](SPEC.md).

## How to read this document

- Signatures use `name(arg, arg) -> type`. `?` marks an optional argument.
- "Returns" and "Throws" describe runtime behavior.
- **Errors are thrown values.** Built-in failures throw an *error object* with two
  fields: `.kind` (a string — one of `TypeError`, `NameError`, `ArityError`,
  `IndexError`, `KeyError`, `DivisionByZero`, `ValueError`, `StackOverflow`,
  `AssertionError`) and `.message` (human text). Catch them with `try`/`catch`:

  ```lumen
  try {
      let x = [1, 2][9];
  } catch (e) {
      println("${e.kind}: ${e.message}");   // IndexError: index 9 out of bounds for length 2
  }
  ```

- Calling any function with the wrong number of arguments throws `ArityError`.
- String indexing and the `string` module operate on **Unicode scalar values**
  (characters), not bytes: `len("héllo")` is `5`, and `s[-1]` is the last char.

---

## Global built-in functions

These are always in scope without an `import` (they may be shadowed by a local or
global of the same name).

| Function | Signature | Description |
|---|---|---|
| `print` | `print(x...) -> nil` | Write the arguments (space-separated, no newline) to stdout. |
| `println` | `println(x...) -> nil` | Like `print`, with a trailing newline. |
| `str` | `str(x) -> string` | Convert any value to its string form (strings inside collections are quoted). |
| `type` | `type(x) -> string` | The value's type name: `nil`, `bool`, `int`, `float`, `string`, `array`, `map`, `function`, `class`, `module`, `error` — or, for a class **instance**, its class name (e.g. `"Point"`). |
| `len` | `len(x) -> int` | Length of a string (in characters), array, or map. **Throws** `TypeError` otherwise. |
| `int` | `int(x) -> int` / `int(s, base) -> int` | Convert: float→truncated toward zero, bool→`0`/`1`, numeric string→int. With a second argument, parse the string `s` in radix `base` (2..=36), e.g. `int("FF", 16) == 255`. **Throws** `ValueError` (bad string / base) or `TypeError` (nil / non-string with base). |
| `float` | `float(x) -> float` | Convert int/bool/numeric-string to float. **Throws** `ValueError`/`TypeError`. |
| `bool` | `bool(x) -> bool` | The truthiness of `x` (only `nil` and `false` are falsy). |
| `range` | `range(end)` / `range(start, end)` / `range(start, end, step) -> array` | Array of ints from `start` (default 0) up to but excluding `end`, stepping by `step` (default 1; may be negative). **Throws** `TypeError` (non-int args) or `ValueError` (zero step). |
| `assert` | `assert(cond, msg?) -> nil` | If `cond` is falsy, **throws** `AssertionError` with `msg` (or `"assertion failed"`). |
| `clock` | `clock() -> float` | Seconds since the Unix epoch (for timing). |
| `input` | `input(prompt?) -> string \| nil` | Print `prompt` (if given), read one line from stdin (newline stripped). Returns `nil` at EOF. |
| `chr` | `chr(code) -> string` | The one-character string for a Unicode code point. **Throws** `ValueError` (out of range) or `TypeError`. |
| `ord` | `ord(s) -> int` | The code point of a single-character string. **Throws** `ValueError` otherwise. |
| `push` | `push(arr, x) -> nil` | Append `x` to the array in place. **Throws** `TypeError` if not an array. |
| `pop` | `pop(arr) -> any` | Remove and return the last element. **Throws** `IndexError` if empty, `TypeError` if not an array. |
| `keys` | `keys(map) -> array` | The map's keys, in insertion order. **Throws** `TypeError` if not a map. |
| `values` | `values(map) -> array` | The map's values, in insertion order. |
| `has` | `has(map, key) -> bool` | Whether the map contains `key`. **Throws** `TypeError` if not a map. |
| `del` | `del(coll, key) -> nil` | Remove `key` from a map, or the element at index `key` from an array (negative indices allowed). |
| `next` | `next(gen) -> any` | Advance a generator to its next `yield` and return the value; returns `nil` once the generator is exhausted. **Throws** `TypeError` if not a generator. |

```lumen
println(type([1, 2]));          // array
println(len("héllo"));          // 5  (characters, not bytes)
println(int(3.9));              // 3
println(range(0, 10, 3));       // [0, 3, 6, 9]
let m = {x: 1, y: 2};
del(m, "x");
println(keys(m));               // ["y"]
```

### Operator overloading (dunder methods)

A class can customize the built-in operators by defining specially named
methods. When an operand is an instance whose class defines the relevant method,
the operator calls it; otherwise the operator keeps its built-in behavior and
throws `TypeError` as usual (overloading is purely additive). Dispatch is on the
left operand for the arithmetic hooks; the comparisons are all derived from
`__lt__`. `__eq__`/`__lt__` results are interpreted by truthiness.

| Operator | Method | Call |
|---|---|---|
| `a + b` | `__add__` | `a.__add__(b)` |
| `a - b` | `__sub__` | `a.__sub__(b)` |
| `a * b` | `__mul__` | `a.__mul__(b)` |
| `a / b` | `__div__` | `a.__div__(b)` |
| `a % b` | `__mod__` | `a.__mod__(b)` |
| `a == b` / `a != b` | `__eq__` | `a.__eq__(b)` (negated for `!=`) |
| `a < b` / `a > b` / `a <= b` / `a >= b` | `__lt__` | via `__lt__` with swapped operands / negation |
| `a[i]` | `__index__` | `a.__index__(i)` |
| `a[i] = v` | `__set_index__` | `a.__set_index__(i, v)` |
| `-a` | `__neg__` | `a.__neg__()` |

```lumen
class Vec2 {
    init(x, y) { this.x = x; this.y = y; }
    __add__(o) { return Vec2(this.x + o.x, this.y + o.y); }
    __index__(i) { if i == 0 { return this.x; } return this.y; }
    str() { return "(${this.x}, ${this.y})"; }
}
println(Vec2(1, 2) + Vec2(3, 4));   // (4, 6)
println(Vec2(7, 8)[0]);             // 7
```

---

## Native modules

Built-in modules are loaded by name; the runtime resolves them automatically.
Two import forms:

```lumen
import "math" as m;        // whole module under an alias:  m.sqrt(2)
import "math".{sqrt, pi};  // selected exports directly:     sqrt(2), pi
```

### `math` — numeric functions and constants

**Constants:** `pi`, `e`, `tau`, `inf`, `nan` (all floats).

| Function | Signature | Notes |
|---|---|---|
| `sqrt` `cbrt` `exp` | `(x) -> float` | Square root, cube root, eˣ. |
| `log` `log2` `log10` | `(x) -> float` | Natural / base-2 / base-10 logarithm. |
| `pow` | `(x, y) -> float` | xʸ. |
| `sin` `cos` `tan` | `(x) -> float` | Trig (radians). |
| `asin` `acos` `atan` | `(x) -> float` | Inverse trig. |
| `atan2` | `(y, x) -> float` | Two-argument arctangent. |
| `hypot` | `(x, y) -> float` | √(x²+y²). |
| `abs` | `(x) -> int\|float` | Absolute value; **preserves** int/float-ness. |
| `floor` `ceil` `trunc` | `(x) -> int` | Rounding; **return ints**. |
| `round` | `(x) -> int` / `(x, ndigits) -> float` | Round to the nearest integer, or — with `ndigits` — to that many decimal places (`round(3.14159, 2) == 3.14`). |
| `sign` | `(x) -> int` | `-1`, `0`, or `1`. |
| `min` `max` | `(a, b) -> int\|float` | Smaller/larger argument, unchanged (type preserved). |
| `gcd` | `(a, b) -> int` | Greatest common divisor (operands must be ints). |
| `lcm` | `(a, b) -> int` | Least common multiple (operands must be ints); `0` if either is `0`. |
| `is_nan` `is_finite` | `(x) -> bool` | Whether `x` is NaN / finite. |
| `degrees` `radians` | `(x) -> float` | Convert between radians and degrees. |

Numeric functions **throw** `TypeError` on non-number arguments.

```lumen
import "math" as m;
println(m.sqrt(144));   // 12.0
println(m.floor(3.7));  // 3   (an int)
println(m.abs(-5));     // 5   (still an int)
println(m.gcd(48, 36)); // 12
```

### `string` — text manipulation

All indices are character (Unicode scalar) offsets; negative indices count from
the end. Functions **throw** `TypeError` on non-string arguments.

| Function | Signature | Description |
|---|---|---|
| `upper` `lower` | `(s) -> string` | Case conversion. |
| `trim` `trim_start` `trim_end` | `(s) -> string` | Strip surrounding / leading / trailing whitespace. |
| `split` | `(s, sep) -> array` | Split into an array of strings; empty `sep` splits into characters. |
| `join` | `(arr, sep) -> string` | Join an array's elements (each rendered with `str`) with `sep`. |
| `contains` | `(s, sub) -> bool` | Whether `s` contains `sub`. |
| `starts_with` `ends_with` | `(s, p) -> bool` | Prefix / suffix test. |
| `replace` | `(s, from, to) -> string` | Replace all occurrences of `from`. |
| `repeat` | `(s, n) -> string` | `s` repeated `n` times (`n >= 0`, else `ValueError`). |
| `index_of` | `(s, sub) -> int` | Character index of the first `sub`, or `-1`. |
| `substring` | `(s, start, end) -> string` | Characters in `[start, end)` (negative indices ok). |
| `char_at` | `(s, i) -> string` | The character at index `i` (negative ok). **Throws** `IndexError` if out of range. |
| `reverse` | `(s) -> string` | Reversed by character. |
| `chars` | `(s) -> array` | Array of one-character strings. |
| `pad_left` `pad_right` | `(s, width, fill?) -> string` | Pad to `width` chars with `fill` (default a space). |
| `is_digit` `is_alpha` `is_alnum` `is_space` | `(s) -> bool` | True iff `s` is non-empty and **every** character is a digit / letter / alphanumeric / whitespace (empty `s` → `false`). Unicode-aware, except `is_digit` accepts only the ASCII digits `0`–`9`. |
| `is_upper` `is_lower` | `(s) -> bool` | True iff `s` has at least one cased character and none of the opposite case. |
| `capitalize` | `(s) -> string` | Upper-case the first character and lower-case the rest. |
| `count` | `(s, needle) -> int` | Number of **non-overlapping** occurrences of `needle` in `s`. **Throws** `ValueError` if `needle` is empty. |
| `lines` | `(s) -> array` | Split into lines like Rust's `str::lines` (split on `\n`, strip a trailing `\r`, no trailing empty element; `""` → `[]`). |
| `format` | `(template, args) -> string` | Substitute `{[index][:spec]}` placeholders. `{}` takes the next positional arg, `{N}` the indexed one; `{{`/`}}` are literal braces. The optional `:spec` is `[[fill]align][sign][#][0][width][.precision][type]` — align `<`/`>`/`^`, `sign` `+`, `#` base prefix, leading `0` zero-pad, `width`, `.precision`, and `type` `f`/`e`/`E`/`x`/`X`/`o`/`b`/`d`/`s`. Numbers default to right alignment, other values to left. **Throws** `ValueError` on a missing argument, out-of-range index, unmatched brace, or invalid spec. |

```lumen
import "string" as s;
println(s.split("a,b,c", ","));   // ["a", "b", "c"]
println(s.join(["a", "b"], "-")); // a-b
println(s.substring("hello", 1, 4)); // ell
println(s.pad_left("7", 3));      // "  7"
println(s.format("{} = {0}", [42]));   // "42 = 42"
println(s.format("{:.2f}", [3.14159])); // "3.14"
println(s.format("[{:>6}]", [42]));     // "[    42]"
println(s.format("{:#x}", [255]));      // "0xff"
```

### `array` — sequence operations

Higher-order functions take a callback and call back into Lumen. Functions
**throw** `TypeError` if the first argument is not an array.

| Function | Signature | Description |
|---|---|---|
| `sum` | `(arr) -> int\|float` | Sum of numbers (int unless any element is a float). |
| `min` `max` | `(arr) -> any` | The smallest / largest element (numeric). **Throws** `ValueError` if empty. |
| `map` | `(arr, f) -> array` | New array of `f(x)` for each element. |
| `filter` | `(arr, pred) -> array` | New array of elements where `pred(x)` is truthy. |
| `reduce` | `(arr, f, init) -> any` | Left fold: `acc = f(acc, x)`, starting at `init`. |
| `each` | `(arr, f) -> nil` | Call `f(x)` for each element (for side effects). |
| `sort` | `(arr, cmp?) -> array` | New sorted array (stable merge sort). `cmp(a, b)` returns a number (`<0`, `0`, `>0`); default is numeric/lexicographic. |
| `reverse` | `(arr) -> array` | New reversed array. |
| `contains` | `(arr, x) -> bool` | Membership test (uses value equality). |
| `index_of` | `(arr, x) -> int` | Index of the first matching element, or `-1`. |
| `slice` | `(arr, start, end) -> array` | Sub-array `[start, end)` (negative indices ok). |
| `concat` | `(a, b) -> array` | New array of `a` followed by `b`. |
| `first` `last` | `(arr) -> any` | First / last element, or `nil` if empty. |
| `flatten` | `(arr) -> array` | Concatenate one level of nested arrays. |
| `find` | `(arr, pred) -> any` | First element where `pred(x)` is truthy, or `nil`. |
| `find_index` | `(arr, pred) -> int` | Index of the first match, or `-1`. |
| `any` `all` | `(arr, pred) -> bool` | Whether `pred` holds for some / every element. |
| `unique` | `(arr) -> array` | New array with duplicates removed (first-seen order, value equality). |
| `zip` | `(a, b) -> array` | New array of `[a[i], b[i]]` pairs, truncated to the shorter input. |

```lumen
import "array" as a;
println(a.map([1, 2, 3], fn(x) { return x * x; }));        // [1, 4, 9]
println(a.filter([1, 2, 3, 4], fn(x) { return x % 2 == 0; })); // [2, 4]
println(a.reduce([1, 2, 3, 4], fn(acc, x) { return acc + x; }, 0)); // 10
println(a.sort([3, 1, 2], fn(x, y) { return y - x; }));    // [3, 2, 1]
```

### `map` — hash-table operations

`get`/`set`/`has`/`remove` mirror the global `keys`/`values`/`has`/`del` but as a
namespaced module. Functions **throw** `TypeError` if the first argument is not a
map.

| Function | Signature | Description |
|---|---|---|
| `get` | `(map, key, default?) -> any` | Value for `key`, or `default` (or `nil`) if absent. |
| `set` | `(map, key, value) -> map` | Insert/update; returns the map. |
| `has` | `(map, key) -> bool` | Membership test. |
| `remove` | `(map, key) -> bool` | Remove `key`; returns whether it was present. |
| `keys` `values` | `(map) -> array` | Keys / values in insertion order. |
| `len` | `(map) -> int` | Number of entries. |
| `entries` | `(map) -> array` | Array of `[key, value]` pairs, in order. |
| `merge` | `(a, b) -> map` | New map: `b`'s entries layered over a copy of `a`. |
| `each` | `(map, f) -> nil` | Call `f(key, value)` for each entry, in order. |
| `map` | `(map, f) -> map` | New map with each value replaced by `f(key, value)`. |
| `filter` | `(map, pred) -> map` | New map of entries where `pred(key, value)` is truthy. |
| `clear` | `(map) -> map` | Remove all entries in place; returns the map. |
| `from_entries` | `(pairs) -> map` | Build a map from an array of `[key, value]` pairs (inverse of `entries`). |

```lumen
import "map" as mp;
let d = {a: 1};
mp.set(d, "b", 2);
println(mp.get(d, "z", -1));  // -1
println(mp.entries(d));       // [["a", 1], ["b", 2]]
```

### `json` — JSON parse and serialize

| Function | Signature | Description |
|---|---|---|
| `parse` | `(text) -> any` | Parse JSON into Lumen values: `null`→`nil`, numbers→`int`/`float`, arrays→array, objects→map. **Throws** `ValueError` on malformed input. |
| `stringify` | `(value, indent?) -> string` | Serialize to JSON; with an `indent` width, pretty-print. **Throws** `TypeError` for unserializable values (functions, classes, instances). |

```lumen
import "json" as j;
let data = j.parse("{\"n\": 42, \"xs\": [1, 2, 3]}");
println(data["n"]);                    // 42
println(j.stringify([1, {"k": "v"}])); // [1,{"k":"v"}]
println(j.stringify(data, 2));         // pretty-printed with 2-space indent
```

### `random` — pseudo-random numbers

Backed by a seedable xorshift64\* generator. After `seed(n)` the sequence is
deterministic.

| Function | Signature | Description |
|---|---|---|
| `random` | `() -> float` | Uniform float in `[0, 1)`. |
| `randint` | `(lo, hi) -> int` | Uniform int in `[lo, hi]` inclusive. **Throws** `ValueError` if `lo > hi`. |
| `choice` | `(arr) -> any` | A random element. **Throws** `ValueError` if empty. |
| `shuffle` | `(arr) -> array` | A new Fisher–Yates-shuffled array. |
| `seed` | `(n) -> nil` | Seed the generator for reproducible sequences. |

```lumen
import "random" as r;
r.seed(42);
println(r.randint(1, 6));          // a die roll, reproducible after the seed
println(r.shuffle([1, 2, 3, 4, 5]));
```

### `io` — file and stream I/O

| Function | Signature | Description |
|---|---|---|
| `open` | `(path, mode) -> file` | Open a buffered **file handle**. Modes: `"r"` (read), `"w"` (truncate-write), `"a"` (append). **Throws** `ValueError` on I/O error or a bad mode. |
| `read_file` | `(path) -> string` | Read a file's contents. **Throws** `ValueError` on I/O error. |
| `write_file` | `(path, content) -> nil` | Write (truncating) `content` to `path`. |
| `append_file` | `(path, content) -> nil` | Append `content` to `path` (creating it if needed). |
| `exists` | `(path) -> bool` | Whether a path exists. |
| `lines` | `(path) -> array` | The file's lines as an array of strings. |
| `mkdir` | `(path) -> nil` | Create a directory and any missing parents (like `mkdir -p`). |
| `listdir` | `(path) -> array` | The entry names directly under `path`, sorted. **Throws** `ValueError` if `path` is not a readable directory. |
| `remove` | `(path) -> nil` | Delete a file. **Throws** `ValueError` on error (use `rmdir` for directories). |
| `rmdir` | `(path) -> nil` | Delete an **empty** directory (never recursive). |
| `is_dir` `is_file` | `(path) -> bool` | Whether `path` is a directory / regular file. |
| `eprint` `eprintln` | `(x) -> nil` | Like `print`/`println`, but to stderr. |

A **file handle** (from `io.open`) has these methods, and a read handle is
line-iterable with `for line in handle { ... }`:

| Method | Signature | Description |
|---|---|---|
| `read_line` | `() -> string \| nil` | Read the next line (trailing newline stripped), or `nil` at end of file. |
| `read` | `() -> string` | Read all remaining content. |
| `write` | `(s) -> nil` | Write `s` to the file (write/append handles). |
| `close` | `() -> nil` | Flush and close the handle. |

```lumen
import "io" as io;
let h = io.open("notes.txt", "w");
h.write("first\n"); h.write("second\n");
h.close();
for line in io.open("notes.txt", "r") {
    println(line);
}
```

### `hash` — non-cryptographic hashing and encodings

Hashes are deterministic 64-bit values returned as `int` (the unsigned result is
reinterpreted as a signed integer, so it may be negative). The encodings operate
on a string's UTF-8 bytes. **Not** suitable for security or cryptography.

| Function | Signature | Description |
|---|---|---|
| `fnv1a` | `(s) -> int` | FNV-1a 64-bit hash. |
| `djb2` | `(s) -> int` | djb2 hash. |
| `hex_encode` | `(s) -> string` | Lowercase hex of the UTF-8 bytes. |
| `hex_decode` | `(s) -> string` | Inverse of `hex_encode`. **Throws** `ValueError` on odd length, a non-hex digit, or non-UTF-8 result. |
| `base64_encode` | `(s) -> string` | Standard base64 with `=` padding. |
| `base64_decode` | `(s) -> string` | Inverse of `base64_encode` (ignores embedded newlines). **Throws** `ValueError` on malformed input or non-UTF-8 result. |

```lumen
import "hash" as h;
println(h.hex_encode("abc"));        // "616263"
println(h.base64_encode("Man"));     // "TWFu"
println(h.base64_decode("TWFu"));    // "Man"
```

### `os` — process and environment

| Function | Signature | Description |
|---|---|---|
| `args` | `() -> array` | Command-line arguments passed after the script. |
| `env` | `(name, default?) -> string \| nil` | An environment variable, or `default`/`nil` if unset. |
| `platform` | `() -> string` | The OS name (e.g. `"linux"`). |
| `cwd` | `() -> string` | The current working directory. |
| `exec` | `(cmd, args) -> map` | Run `cmd` with the string `args`, waiting for it to finish; returns `{status, stdout, stderr}` (`status` is the exit code, or -1 if killed by a signal). **Throws** `ValueError` if the program cannot be started. |
| `exit` | `(code?) -> never` | Exit the process with `code` (default 0). |

```lumen
import "os" as os;
let r = os.exec("echo", ["hello"]);
println(r["status"]);   // 0
print(r["stdout"]);     // "hello\n"
```

### `time` — clock and sleeping

| Function | Signature | Description |
|---|---|---|
| `now` | `() -> float` | Seconds since the Unix epoch. |
| `now_millis` | `() -> int` | Milliseconds since the Unix epoch. |
| `sleep` | `(seconds) -> nil` | Block for `seconds` (a float). |

### `datetime` — UTC calendar math

Operates on Unix epoch **seconds** (`int`), in UTC. Correct for any timestamp,
including negative (pre-1970) ones.

| Function | Signature | Description |
|---|---|---|
| `now` | `() -> int` | Current epoch seconds. |
| `is_leap_year` | `(year) -> bool` | Gregorian leap-year test. |
| `days_in_month` | `(year, month) -> int` | Days in `month` (1–12). **Throws** `ValueError` on a bad month. |
| `from_epoch` | `(secs) -> map` | `{year, month, day, hour, minute, second, weekday, yearday}` (weekday 0 = Sunday; yearday 1-based). |
| `to_epoch` | `(y, mo, d, h, mi, s) -> int` | Epoch seconds for a UTC date-time. |
| `weekday` | `(secs) -> int` | Day of week, 0 = Sunday. |
| `iso` | `(secs) -> string` | `"YYYY-MM-DDTHH:MM:SSZ"`. |
| `format` | `(secs, template) -> string` | strftime subset: `%Y %m %d %H %M %S %j %w %%`. |

```lumen
import "datetime" as dt;
println(dt.iso(0));                       // "1970-01-01T00:00:00Z"
println(dt.from_epoch(946684800)["year"]); // 2000
println(dt.format(0, "%Y/%m/%d"));        // "1970/01/01"
```

### `regex` — regular expressions

A small dependency-free engine: literals, `.`, classes `[...]`/`[^...]` with
ranges and `\d \w \s \D \W \S`, anchors `^ $`, capturing groups `(...)`,
alternation `|`, and quantifiers `* + ?` / `{n}` `{n,}` `{n,m}` (greedy, or lazy
with a trailing `?`). Indices are character offsets. Anchors match the start/end
of the whole string only (no multiline; `$` does not match before a trailing
newline — like Go's `regexp`); `\d`/`\w` are ASCII. Word boundaries (`\b`) and
backreferences are not supported. Pathological backtracking **throws**
`ValueError` rather than hanging or crashing; for the same reason a *single*
match spanning more than a few thousand characters also throws (matching many
short spans in a long string is unaffected).

| Function | Signature | Description |
|---|---|---|
| `test` | `(pattern, s) -> bool` | Whether `pattern` matches anywhere in `s`. |
| `find` | `(pattern, s) -> map \| nil` | First match as `{start, end, text}`, or `nil`. |
| `find_all` | `(pattern, s) -> array` | All non-overlapping matches (each a match map). |
| `captures` | `(pattern, s) -> array \| nil` | `[whole, group1, …]` for the first match (unmatched groups are `nil`), or `nil`. |
| `replace` | `(pattern, s, repl) -> string` | Replace all matches; `repl` may use `$0`…`$9` and `$$`. |
| `split` | `(pattern, s) -> array` | Split `s` on matches of `pattern`. |
| An invalid pattern | | **Throws** `ValueError`. |

```lumen
import "regex" as re;
println(re.test("^\\d{4}-\\d{2}$", "2026-06"));        // true
println(re.find("\\d+", "abc123")["text"]);            // "123"
println(re.captures("(\\w+)@(\\w+)", "ada@lumen"));    // ["ada@lumen", "ada", "lumen"]
println(re.replace("(\\w+)@(\\w+)", "ada@lumen", "$2.$1")); // "lumen.ada"
println(re.split("\\s+", "a  b   c"));                 // ["a", "b", "c"]
```

---

## `seq` — self-hosted sequence utilities

The `seq` module is written **in Lumen itself** (see `std/seq.lum`) and bundled
with the runtime, demonstrating that the language can implement its own library.
Import it like any other module: `import "seq" as q;`.

| Function | Signature | Description |
|---|---|---|
| `take` | `(xs, n) -> array` | The first `n` elements (or all, if fewer). |
| `drop` | `(xs, n) -> array` | All but the first `n` elements. |
| `find` | `(xs, pred) -> any` | First element where `pred(x)` is truthy, or `nil`. |
| `any` | `(xs, pred) -> bool` | Whether any element satisfies `pred`. |
| `all` | `(xs, pred) -> bool` | Whether every element satisfies `pred`. |
| `count` | `(xs, pred) -> int` | How many elements satisfy `pred`. |
| `zip` | `(xs, ys) -> array` | Pairs `[x, y]`, stopping at the shorter input. |
| `enumerate` | `(xs) -> array` | Pairs `[index, x]`. |
| `repeat` | `(value, n) -> array` | `value` repeated `n` times. |
| `flat_map` | `(xs, f) -> array` | Map with `f`, then concatenate the resulting arrays. |
| `windows` | `(xs, size) -> array` | All contiguous sub-arrays of length `size`. |

```lumen
import "seq" as q;
println(q.zip([1, 2, 3], ["a", "b", "c"])); // [[1, "a"], [2, "b"], [3, "c"]]
println(q.take([1, 2, 3, 4, 5], 2));        // [1, 2]
println(q.windows([1, 2, 3, 4], 2));        // [[1, 2], [2, 3], [3, 4]]
```

## `path` — self-hosted path manipulation

Written in Lumen (`std/path.lum`). Pure text manipulation of POSIX-style paths
(`/` separator); no filesystem access — use `io` for that. Import it with
`import "path" as path;`.

| Function | Signature | Description |
|---|---|---|
| `join` | `(parts) -> string` | Join segments with a single `/`, skipping empty ones (a leading `/` is preserved). |
| `basename` | `(p) -> string` | The final component (after the last `/`). |
| `dirname` | `(p) -> string` | Everything before the final component (`"."` if none, `"/"` at root). |
| `ext` | `(p) -> string` | The extension of the final component without the dot (`""` for none or a dotfile). |
| `stem` | `(p) -> string` | The final component without its extension. |
| `is_absolute` | `(p) -> bool` | Whether `p` starts at the root (`/`). |
| `split` | `(p) -> array` | The non-empty path components. |
| `normalize` | `(p) -> string` | Collapse `.`, `..`, and duplicate slashes (preserving a leading `/`). |

```lumen
import "path" as path;
println(path.join(["/usr", "local", "bin"])); // "/usr/local/bin"
println(path.basename("/a/b/c.txt"));         // "c.txt"
println(path.ext("archive.tar.gz"));          // "gz"
println(path.normalize("/a/b/../c"));         // "/a/c"
```

## `set` — self-hosted hash set

Written in Lumen (`std/set.lum`). A `Set` class backed by a map, so it holds
hashable values (int, float, string, bool, nil). `set.of(xs)` builds one from a
sequence. Methods return the set where it makes sense, so calls chain.

| Member | Signature | Description |
|---|---|---|
| `Set` | `() -> Set` | An empty set. |
| `of` | `(xs) -> Set` | A set of the (de-duplicated) elements of `xs`. |
| `.add` | `(x) -> Set` | Add a member. |
| `.has` | `(x) -> bool` | Membership test. |
| `.remove` | `(x) -> Set` | Remove a member if present. |
| `.size` | `() -> int` | Number of members. |
| `.values` | `() -> array` | Members in insertion order. |
| `.union` | `(other) -> Set` | Members of either set. |
| `.intersect` `.intersection` | `(other) -> Set` | Members in both sets (`intersection` is the canonical name; `intersect` is a kept alias). |
| `.difference` | `(other) -> Set` | This set's members that `other` lacks. |
| `.symmetric_difference` | `(other) -> Set` | Members in exactly one of the two sets. |
| `.is_subset` | `(other) -> bool` | Whether every member is also in `other`. |
| `.is_superset` | `(other) -> bool` | Whether every member of `other` is also in this set. |

```lumen
import "set";
let a = set.of([1, 2, 3]);
println(a.union(set.of([3, 4])).values());  // [1, 2, 3, 4]
println(a.intersect(set.of([2, 3, 9])).values()); // [2, 3]
```

## `functional` — self-hosted higher-order helpers

Written in Lumen (`std/functional.lum`), all built from closures.

| Function | Signature | Description |
|---|---|---|
| `identity` | `(x) -> any` | Returns its argument. |
| `constant` | `(x) -> fn` | A function that always returns `x`. |
| `compose` | `(f, g) -> fn` | `compose(f, g)(x) == f(g(x))`. |
| `pipe` | `(..fns) -> fn` | Left-to-right pipeline through unary functions. |
| `curry2` | `(f) -> fn` | Curry a 2-arg function into `f(a)(b)`. |
| `partial` | `(f, a) -> fn` | Bind the first argument. |
| `flip` | `(f) -> fn` | Swap a 2-arg function's argument order. |
| `complement` | `(pred) -> fn` | Negate a unary predicate. |
| `memoize` | `(f) -> fn` | Cache a unary function on a hashable argument. |
| `iterate` | `(f, seed, n) -> any` | Apply `f` to `seed` `n` times. |

```lumen
import "functional" as fp;
let inc = fn(x) { return x + 1; };
let dbl = fn(x) { return x * 2; };
println(fp.pipe(inc, dbl, inc)(10)); // 23
println(fp.curry2(fn(a, b) { return a + b; })(3)(4)); // 7
```

## `testing` — self-hosted unit-test harness

Written in Lumen (`std/testing.lum`). A `Suite` tallies checks and prints a
summary; `deep_eq` provides structural equality for arrays and maps (the core
`==` is structural only for primitives and strings).

| Member | Signature | Description |
|---|---|---|
| `deep_eq` | `(a, b) -> bool` | Structural equality, recursing into arrays/maps. |
| `Suite` | `(name) -> Suite` | A new named suite. |
| `.check` | `(label, cond) -> bool` | Record a boolean check. |
| `.eq` | `(label, actual, expected) -> bool` | Assert structural equality. |
| `.truthy` | `(label, v) -> bool` | Assert `v` is truthy. |
| `.falsy` | `(label, v) -> bool` | Assert `v` is falsy. |
| `.report` | `() -> bool` | Print the summary; returns true if all passed. |

```lumen
import "testing" as t;
let s = t.Suite("math");
s.eq("adds", 1 + 1, 2);
s.eq("arrays", [1, 2], [1, 2]);
s.report(); // math: 2/2 passed
```
