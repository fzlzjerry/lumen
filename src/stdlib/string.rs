//! The `string` module: text manipulation. All indexing is by Unicode scalar
//! (character), consistent with `s[i]` in the core language.

use super::{array_of, err, int, num, string_of, Vm};
use crate::object::Arity::{self, Exact, Range};
use crate::util::format_float;
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "upper", Exact(1), upper),
        f(vm, "lower", Exact(1), lower),
        f(vm, "trim", Exact(1), trim),
        f(vm, "trim_start", Exact(1), trim_start),
        f(vm, "trim_end", Exact(1), trim_end),
        f(vm, "split", Exact(2), split),
        f(vm, "join", Exact(2), join),
        f(vm, "contains", Exact(2), contains),
        f(vm, "starts_with", Exact(2), starts_with),
        f(vm, "ends_with", Exact(2), ends_with),
        f(vm, "replace", Exact(3), replace),
        f(vm, "repeat", Exact(2), repeat),
        f(vm, "index_of", Exact(2), index_of),
        f(vm, "substring", Exact(3), substring),
        f(vm, "char_at", Exact(2), char_at),
        f(vm, "reverse", Exact(1), reverse),
        f(vm, "chars", Exact(1), chars),
        f(vm, "pad_left", Range(2, 3), pad_left),
        f(vm, "pad_right", Range(2, 3), pad_right),
        f(vm, "format", Exact(2), format),
    ];
    vm.make_module("string", exports)
}

/// `format(template, args)` — substitute placeholders in `template` with values
/// from `args`. A placeholder is `{[index][:spec]}`: `{}` takes the next
/// positional argument, `{N}` the indexed one. `{{` and `}}` are literal braces.
///
/// The optional `:spec` is a mini format spec
/// `[[fill]align][sign][#][0][width][.precision][type]`:
/// - **align** `<` `>` `^` (left/right/center), with an optional `fill` char;
/// - **sign** `+` always shows the sign of a number;
/// - `#` adds the base prefix (`0x`/`0o`/`0b`) for the integer base types;
/// - a leading `0` zero-pads a number to `width`;
/// - **width** is the minimum field width; **.precision** the digits after the
///   point (for `f`/`e`) or — with no type — the float precision;
/// - **type** `f`/`e`/`E` (float), `x`/`X`/`o`/`b`/`d` (integer base), `s`
///   (string). Numbers default to right alignment, everything else to left.
///
/// Throws `ValueError` on a missing argument, an out-of-range index, an unmatched
/// brace, or an invalid spec.
fn format(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let fmt = string_of(vm, a[0])?;
    let args = array_of(vm, a[1])?;
    let chars: Vec<char> = fmt.chars().collect();
    let mut result = String::with_capacity(fmt.len());
    let mut next = 0usize; // index of the next argument for a bare `{}`
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' if i + 1 < chars.len() && chars[i + 1] == '{' => {
                result.push('{');
                i += 2;
            }
            '}' if i + 1 < chars.len() && chars[i + 1] == '}' => {
                result.push('}');
                i += 2;
            }
            '}' => return Err(err(vm, error_kind::VALUE, "format: unmatched '}'")),
            '{' => {
                let mut j = i + 1;
                let mut field = String::new();
                while j < chars.len() && chars[j] != '}' {
                    field.push(chars[j]);
                    j += 1;
                }
                if j >= chars.len() {
                    return Err(err(vm, error_kind::VALUE, "format: unmatched '{'"));
                }
                let (index_part, spec_part) = match field.find(':') {
                    Some(p) => (&field[..p], Some(&field[p + 1..])),
                    None => (field.as_str(), None),
                };
                let idx = if index_part.is_empty() {
                    let k = next;
                    next += 1;
                    k
                } else {
                    index_part.parse::<usize>().map_err(|_| {
                        err(
                            vm,
                            error_kind::VALUE,
                            format!("format: invalid placeholder '{{{field}}}'"),
                        )
                    })?
                };
                let val = *args.get(idx).ok_or_else(|| {
                    err(
                        vm,
                        error_kind::VALUE,
                        format!("format: argument {idx} out of range (have {})", args.len()),
                    )
                })?;
                let rendered = match spec_part {
                    None => vm.to_display(val, false)?,
                    Some(sp) => {
                        let spec = FormatSpec::parse(sp)
                            .map_err(|m| err(vm, error_kind::VALUE, format!("format: {m}")))?;
                        spec.render(vm, val)?
                    }
                };
                result.push_str(&rendered);
                i = j + 1;
            }
            c => {
                result.push(c);
                i += 1;
            }
        }
    }
    Ok(vm.new_string(&result))
}

#[derive(Clone, Copy, PartialEq)]
enum Align {
    Left,
    Right,
    Center,
}

/// A parsed `string.format` field spec (the part after `:`).
struct FormatSpec {
    fill: char,
    align: Option<Align>,
    sign_plus: bool,
    alternate: bool,
    zero_pad: bool,
    width: Option<usize>,
    precision: Option<usize>,
    ty: Option<char>,
}

impl FormatSpec {
    fn parse(s: &str) -> Result<FormatSpec, String> {
        let c: Vec<char> = s.chars().collect();
        let mut i = 0;
        let is_align = |ch: char| matches!(ch, '<' | '>' | '^');
        let to_align = |ch: char| match ch {
            '<' => Align::Left,
            '>' => Align::Right,
            _ => Align::Center,
        };
        let mut spec = FormatSpec {
            fill: ' ',
            align: None,
            sign_plus: false,
            alternate: false,
            zero_pad: false,
            width: None,
            precision: None,
            ty: None,
        };
        // [[fill]align]: a fill char is only recognized when an align follows it.
        if c.len() >= i + 2 && is_align(c[i + 1]) {
            spec.fill = c[i];
            spec.align = Some(to_align(c[i + 1]));
            i += 2;
        } else if i < c.len() && is_align(c[i]) {
            spec.align = Some(to_align(c[i]));
            i += 1;
        }
        // sign
        if i < c.len() && c[i] == '+' {
            spec.sign_plus = true;
            i += 1;
        } else if i < c.len() && c[i] == '-' {
            i += 1; // the default; accepted for symmetry
        }
        // alternate form (#) then zero-pad (0)
        if i < c.len() && c[i] == '#' {
            spec.alternate = true;
            i += 1;
        }
        if i < c.len() && c[i] == '0' {
            spec.zero_pad = true;
            i += 1;
        }
        // width
        let mut w = String::new();
        while i < c.len() && c[i].is_ascii_digit() {
            w.push(c[i]);
            i += 1;
        }
        if !w.is_empty() {
            spec.width = Some(w.parse().map_err(|_| "width too large".to_string())?);
        }
        // precision
        if i < c.len() && c[i] == '.' {
            i += 1;
            let mut p = String::new();
            while i < c.len() && c[i].is_ascii_digit() {
                p.push(c[i]);
                i += 1;
            }
            if p.is_empty() {
                return Err("expected digits after '.' in precision".to_string());
            }
            spec.precision = Some(p.parse().map_err(|_| "precision too large".to_string())?);
        }
        // type
        if i < c.len() {
            let t = c[i];
            if matches!(t, 'f' | 'e' | 'E' | 'x' | 'X' | 'o' | 'b' | 'd' | 's') {
                spec.ty = Some(t);
                i += 1;
            } else {
                return Err(format!("unknown format type '{t}'"));
            }
        }
        if i != c.len() {
            return Err(format!("invalid format spec '{s}'"));
        }
        Ok(spec)
    }

    fn render(&self, vm: &mut Vm, val: Value) -> Result<String, Value> {
        // Build the body and learn whether it is numeric (affects default
        // alignment, sign, and zero-padding) and whether it is negative.
        let (body, prefix, numeric, negative) = match self.ty {
            Some('f') | Some('e') | Some('E') => {
                let x = num(vm, val)?;
                let prec = self.precision.unwrap_or(6);
                let mag = x.abs();
                let s = match self.ty {
                    Some('e') => format!("{mag:.prec$e}"),
                    Some('E') => format!("{mag:.prec$E}"),
                    _ => format!("{mag:.prec$}"),
                };
                (s, String::new(), true, x.is_sign_negative() && x != 0.0)
            }
            Some('x') | Some('X') | Some('o') | Some('b') | Some('d') => {
                let n = int(vm, val)?;
                let mag = (n as i128).unsigned_abs();
                let (digits, pre) = match self.ty {
                    Some('x') => (format!("{mag:x}"), "0x"),
                    Some('X') => (format!("{mag:X}"), "0x"),
                    Some('o') => (format!("{mag:o}"), "0o"),
                    Some('b') => (format!("{mag:b}"), "0b"),
                    _ => (mag.to_string(), ""),
                };
                let prefix = if self.alternate {
                    pre.to_string()
                } else {
                    String::new()
                };
                (digits, prefix, true, n < 0)
            }
            Some('s') => (vm.to_display(val, false)?, String::new(), false, false),
            Some(_) => unreachable!("format type validated in FormatSpec::parse"),
            None => match val {
                Value::Int(n) => (
                    (n as i128).unsigned_abs().to_string(),
                    String::new(),
                    true,
                    n < 0,
                ),
                Value::Float(x) => {
                    let body = match self.precision {
                        Some(p) => format!("{:.*}", p, x.abs()),
                        None => format_float(x.abs()),
                    };
                    (body, String::new(), true, x.is_sign_negative() && x != 0.0)
                }
                _ => (vm.to_display(val, false)?, String::new(), false, false),
            },
        };

        // Sign goes before the alternate prefix and the digits.
        let sign = if negative {
            "-"
        } else if numeric && self.sign_plus {
            "+"
        } else {
            ""
        };
        let head_len = sign.chars().count() + prefix.chars().count();
        let content_len = head_len + body.chars().count();

        let width = self.width.unwrap_or(0);
        if content_len >= width {
            return Ok(format!("{sign}{prefix}{body}"));
        }
        let pad = width - content_len;

        // Zero-padding (only without an explicit alignment) goes between the
        // sign/prefix and the digits; otherwise pad with `fill` per alignment.
        if self.zero_pad && self.align.is_none() {
            return Ok(format!("{sign}{prefix}{}{body}", "0".repeat(pad)));
        }
        let align = self
            .align
            .unwrap_or(if numeric { Align::Right } else { Align::Left });
        let fillstr = |n: usize| self.fill.to_string().repeat(n);
        let core = format!("{sign}{prefix}{body}");
        Ok(match align {
            Align::Left => format!("{core}{}", fillstr(pad)),
            Align::Right => format!("{}{core}", fillstr(pad)),
            Align::Center => {
                let left = pad / 2;
                format!("{}{core}{}", fillstr(left), fillstr(pad - left))
            }
        })
    }
}

fn upper(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    Ok(vm.new_string(&s.to_uppercase()))
}
fn lower(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    Ok(vm.new_string(&s.to_lowercase()))
}
fn trim(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    Ok(vm.new_string(s.trim()))
}
fn trim_start(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    Ok(vm.new_string(s.trim_start()))
}
fn trim_end(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    Ok(vm.new_string(s.trim_end()))
}

fn split(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let sep = string_of(vm, a[1])?;
    let parts: Vec<String> = if sep.is_empty() {
        s.chars().map(|c| c.to_string()).collect()
    } else {
        s.split(&sep).map(|p| p.to_string()).collect()
    };
    let vals: Vec<Value> = parts.iter().map(|p| vm.new_string(p)).collect();
    Ok(vm.new_array(vals))
}

fn join(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let items = array_of(vm, a[0])?;
    let sep = string_of(vm, a[1])?;
    let mut parts = Vec::with_capacity(items.len());
    for it in items {
        parts.push(vm.to_display(it, false)?);
    }
    Ok(vm.new_string(&parts.join(&sep)))
}

fn contains(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (s, sub) = (string_of(vm, a[0])?, string_of(vm, a[1])?);
    Ok(Value::Bool(s.contains(&sub)))
}
fn starts_with(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (s, p) = (string_of(vm, a[0])?, string_of(vm, a[1])?);
    Ok(Value::Bool(s.starts_with(&p)))
}
fn ends_with(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (s, p) = (string_of(vm, a[0])?, string_of(vm, a[1])?);
    Ok(Value::Bool(s.ends_with(&p)))
}

fn replace(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (s, from, to) = (
        string_of(vm, a[0])?,
        string_of(vm, a[1])?,
        string_of(vm, a[2])?,
    );
    Ok(vm.new_string(&s.replace(&from, &to)))
}

fn repeat(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let n = int(vm, a[1])?;
    if n < 0 {
        return Err(err(
            vm,
            error_kind::VALUE,
            "repeat count cannot be negative",
        ));
    }
    Ok(vm.new_string(&s.repeat(n as usize)))
}

fn index_of(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (s, sub) = (string_of(vm, a[0])?, string_of(vm, a[1])?);
    // Report a character index, not a byte index.
    match s.find(&sub) {
        Some(byte) => Ok(Value::Int(s[..byte].chars().count() as i64)),
        None => Ok(Value::Int(-1)),
    }
}

fn substring(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let norm = |i: i64| -> usize {
        if i < 0 {
            ((len + i).max(0)) as usize
        } else {
            (i.min(len)) as usize
        }
    };
    let start = norm(int(vm, a[1])?);
    let end = norm(int(vm, a[2])?);
    let slice: String = if start < end {
        chars[start..end].iter().collect()
    } else {
        String::new()
    };
    Ok(vm.new_string(&slice))
}

fn char_at(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let i = int(vm, a[1])?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let idx = if i < 0 { len + i } else { i };
    if idx < 0 || idx >= len {
        return Err(err(
            vm,
            error_kind::INDEX,
            format!("char_at: index {i} out of bounds"),
        ));
    }
    Ok(vm.new_string(&chars[idx as usize].to_string()))
}

fn reverse(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let r: String = s.chars().rev().collect();
    Ok(vm.new_string(&r))
}

fn chars(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let vals: Vec<Value> = s.chars().map(|c| vm.new_string(&c.to_string())).collect();
    Ok(vm.new_array(vals))
}

fn pad(vm: &mut Vm, a: &[Value], left: bool) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let width = int(vm, a[1])?;
    let fill = if a.len() == 3 {
        let f = string_of(vm, a[2])?;
        f.chars().next().unwrap_or(' ')
    } else {
        ' '
    };
    let cur = s.chars().count() as i64;
    if cur >= width {
        return Ok(vm.new_string(&s));
    }
    let padding: String = std::iter::repeat_n(fill, (width - cur) as usize).collect();
    let result = if left {
        format!("{padding}{s}")
    } else {
        format!("{s}{padding}")
    };
    Ok(vm.new_string(&result))
}
fn pad_left(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    pad(vm, a, true)
}
fn pad_right(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    pad(vm, a, false)
}
