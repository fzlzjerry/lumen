//! The `datetime` module: UTC calendar math over Unix epoch seconds. All
//! conversions are proleptic-Gregorian and use Howard Hinnant's well-known
//! `days_from_civil` / `civil_from_days` algorithms, which are correct for
//! arbitrary (including negative, pre-1970) timestamps.

use super::{err, int, string_of, Vm};
use crate::object::Arity::{self, Exact};
use crate::object::{LumMap, Obj};
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "now", Exact(0), now),
        f(vm, "is_leap_year", Exact(1), is_leap_year),
        f(vm, "days_in_month", Exact(2), days_in_month),
        f(vm, "from_epoch", Exact(1), from_epoch),
        f(vm, "to_epoch", Exact(6), to_epoch),
        f(vm, "weekday", Exact(1), weekday),
        f(vm, "iso", Exact(1), iso),
        f(vm, "format", Exact(2), format),
    ];
    vm.make_module("datetime", exports)
}

// ---- calendar arithmetic --------------------------------------------------

/// Days since 1970-01-01 for a civil (proleptic Gregorian) date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The civil `(year, month, day)` for a count of days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Break epoch seconds into `(year, month, day, hour, minute, second, weekday,
/// yearday)`. `weekday` is 0 = Sunday; `yearday` is 1-based.
fn components(secs: i64) -> (i64, i64, i64, i64, i64, i64, i64, i64) {
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    // 1970-01-01 was a Thursday (4, with Sunday = 0).
    let weekday = (days.rem_euclid(7) + 4) % 7;
    let yearday = days - days_from_civil(y, 1, 1) + 1;
    (
        y,
        m,
        d,
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60,
        weekday,
        yearday,
    )
}

// ---- native functions -----------------------------------------------------

fn now(_vm: &mut Vm, _a: &[Value]) -> Result<Value, Value> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(Value::Int(secs))
}

fn is_leap_year(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    Ok(Value::Bool(is_leap(int(vm, a[0])?)))
}

fn days_in_month(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let y = int(vm, a[0])?;
    let m = int(vm, a[1])?;
    if !(1..=12).contains(&m) {
        return Err(err(
            vm,
            error_kind::VALUE,
            format!("month must be 1..=12, got {m}"),
        ));
    }
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
    };
    Ok(Value::Int(days))
}

fn from_epoch(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (y, m, d, h, mi, s, wd, yd) = components(int(vm, a[0])?);
    let entries = [
        ("year", Value::Int(y)),
        ("month", Value::Int(m)),
        ("day", Value::Int(d)),
        ("hour", Value::Int(h)),
        ("minute", Value::Int(mi)),
        ("second", Value::Int(s)),
        ("weekday", Value::Int(wd)),
        ("yearday", Value::Int(yd)),
    ];
    make_map(vm, &entries)
}

fn to_epoch(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let y = int(vm, a[0])?;
    let mo = int(vm, a[1])?;
    let d = int(vm, a[2])?;
    let h = int(vm, a[3])?;
    let mi = int(vm, a[4])?;
    let s = int(vm, a[5])?;
    if !(1..=12).contains(&mo) {
        return Err(err(
            vm,
            error_kind::VALUE,
            format!("month must be 1..=12, got {mo}"),
        ));
    }
    Ok(Value::Int(
        days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s,
    ))
}

fn weekday(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let days = int(vm, a[0])?.div_euclid(86_400);
    Ok(Value::Int((days.rem_euclid(7) + 4) % 7))
}

fn iso(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let (y, m, d, h, mi, s, _, _) = components(int(vm, a[0])?);
    Ok(vm.new_string(&format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")))
}

/// `format(secs, template)` — a small strftime subset: `%Y %m %d %H %M %S`,
/// `%j` (day-of-year), `%w` (weekday), and `%%`. Unknown `%x` is left verbatim.
fn format(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let secs = int(vm, a[0])?;
    let fmt = string_of(vm, a[1])?;
    let (y, m, d, h, mi, s, wd, yd) = components(secs);
    let mut out = String::with_capacity(fmt.len());
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&format!("{y:04}")),
            Some('m') => out.push_str(&format!("{m:02}")),
            Some('d') => out.push_str(&format!("{d:02}")),
            Some('H') => out.push_str(&format!("{h:02}")),
            Some('M') => out.push_str(&format!("{mi:02}")),
            Some('S') => out.push_str(&format!("{s:02}")),
            Some('j') => out.push_str(&format!("{yd:03}")),
            Some('w') => out.push_str(&wd.to_string()),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    Ok(vm.new_string(&out))
}

/// Build a map from `(key, value)` pairs, GC-safely: the result is pinned as a
/// temp root, and each freshly-interned key is inserted immediately (no
/// allocating call runs between creating a key and storing it).
fn make_map(vm: &mut Vm, entries: &[(&str, Value)]) -> Result<Value, Value> {
    let result = Value::Obj(vm.heap.alloc_map(LumMap::new()));
    vm.push_temp_root(result);
    let rr = result.as_obj().unwrap();
    for &(name, val) in entries {
        let key = vm.new_string(name);
        let norm = match vm.map_key(key) {
            Ok(n) => n,
            Err(e) => {
                vm.pop_temp_root();
                return Err(e);
            }
        };
        if let Obj::Map(m) = vm.heap.get_mut(rr) {
            m.insert(norm, key, val);
        }
        vm.write_barrier(rr, key);
        vm.write_barrier(rr, val);
    }
    vm.pop_temp_root();
    Ok(result)
}
