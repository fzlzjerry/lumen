//! The `hash` module: non-cryptographic hashing and binary-to-text encodings.
//! Hashes are deterministic 64-bit values returned as `int` (the unsigned result
//! is reinterpreted as a signed integer, so it may be negative). The encodings
//! operate on a string's UTF-8 bytes.

use super::{err, string_of, Vm};
use crate::object::Arity::{self, Exact};
use crate::value::{error_kind, Value};

pub fn build(vm: &mut Vm) -> Value {
    let f = |vm: &mut Vm, name: &'static str, arity: Arity, func: crate::object::NativeFn| {
        (name, vm.make_native_value(name, arity, func))
    };
    let exports = vec![
        f(vm, "fnv1a", Exact(1), fnv1a),
        f(vm, "djb2", Exact(1), djb2),
        f(vm, "hex_encode", Exact(1), hex_encode),
        f(vm, "hex_decode", Exact(1), hex_decode),
        f(vm, "base64_encode", Exact(1), base64_encode),
        f(vm, "base64_decode", Exact(1), base64_decode),
    ];
    vm.make_module("hash", exports)
}

// ---- hashes ---------------------------------------------------------------

fn fnv1a(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a 64-bit offset basis
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3); // FNV prime
    }
    Ok(Value::Int(h as i64))
}

fn djb2(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let mut h: u64 = 5381;
    for &b in s.as_bytes() {
        h = h.wrapping_mul(33).wrapping_add(b as u64);
    }
    Ok(Value::Int(h as i64))
}

// ---- hex ------------------------------------------------------------------

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex_encode(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let mut out = String::with_capacity(s.len() * 2);
    for &b in s.as_bytes() {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    Ok(vm.new_string(&out))
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex_decode(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let bytes = s.as_bytes();
    if bytes.len() % 2 != 0 {
        return Err(err(vm, error_kind::VALUE, "hex_decode: odd-length input"));
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_val(bytes[i]);
        let lo = hex_val(bytes[i + 1]);
        match (hi, lo) {
            (Some(h), Some(l)) => out.push((h << 4) | l),
            _ => return Err(err(vm, error_kind::VALUE, "hex_decode: invalid hex digit")),
        }
        i += 2;
    }
    match String::from_utf8(out) {
        Ok(decoded) => Ok(vm.new_string(&decoded)),
        Err(_) => Err(err(vm, error_kind::VALUE, "hex_decode: result is not valid UTF-8")),
    }
}

// ---- base64 (standard alphabet, `=` padding) ------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    let data = s.as_bytes();
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { B64[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[(n & 63) as usize] as char } else { '=' });
    }
    Ok(vm.new_string(&out))
}

fn b64_val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn base64_decode(vm: &mut Vm, a: &[Value]) -> Result<Value, Value> {
    let s = string_of(vm, a[0])?;
    // Ignore embedded newlines (common in wrapped base64), then require a whole
    // number of 4-character groups.
    let bytes: Vec<u8> = s.bytes().filter(|&b| b != b'\n' && b != b'\r').collect();
    if bytes.len() % 4 != 0 {
        return Err(err(vm, error_kind::VALUE, "base64_decode: length is not a multiple of 4"));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let mut vals = [0u8; 4];
        let mut pad = 0;
        for (i, &c) in chunk.iter().enumerate() {
            if c == b'=' {
                pad += 1;
            } else if pad > 0 {
                // A non-`=` after padding is malformed.
                return Err(err(vm, error_kind::VALUE, "base64_decode: data after padding"));
            } else {
                match b64_val(c) {
                    Some(v) => vals[i] = v,
                    None => return Err(err(vm, error_kind::VALUE, "base64_decode: invalid character")),
                }
            }
        }
        if pad > 2 {
            return Err(err(vm, error_kind::VALUE, "base64_decode: too much padding"));
        }
        let n = ((vals[0] as u32) << 18) | ((vals[1] as u32) << 12) | ((vals[2] as u32) << 6) | (vals[3] as u32);
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    match String::from_utf8(out) {
        Ok(decoded) => Ok(vm.new_string(&decoded)),
        Err(_) => Err(err(vm, error_kind::VALUE, "base64_decode: result is not valid UTF-8")),
    }
}
