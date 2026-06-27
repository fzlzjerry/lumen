//! Runtime values and heap handles.
//!
//! A [`Value`] is a small `Copy` enum: the immediate types (`nil`, `bool`,
//! `int`, `float`) are stored inline, and everything else is a [`GcRef`] handle
//! into the [`Heap`](crate::gc::Heap). Keeping `Value` `Copy` means the VM can
//! push/pop and pass arguments without clones and without fighting the borrow
//! checker — the actual heap data lives in one place (the heap) and values just
//! point at it.

use crate::span::Span;

/// A typed handle into the heap: an index into the object table. Cheap to copy
/// and compare. Reference identity (`==`) is object identity, which is exactly
/// what Lumen uses for arrays/maps/instances and — because strings are interned
/// — for strings too.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GcRef(pub u32);

/// A Lumen runtime value.
#[derive(Clone, Copy, Debug)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Obj(GcRef),
}

impl Value {
    /// SPEC §6.1: only `nil` and `false` are falsy.
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    /// If this is a heap object, its handle.
    pub fn as_obj(&self) -> Option<GcRef> {
        match self {
            Value::Obj(r) => Some(*r),
            _ => None,
        }
    }

    /// Coerce a number to f64 (for mixed arithmetic/comparison). `None` for
    /// non-numbers.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(n) => Some(*n as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// The inner `i64`, assuming this is known to be an `Int` (panics otherwise).
    /// Used in arithmetic fast paths where the caller already checked the type.
    pub fn int_unchecked(&self) -> i64 {
        match self {
            Value::Int(n) => *n,
            _ => unreachable!("int_unchecked on a non-int value"),
        }
    }
}

/// A normalized, hashable map key. SPEC: keys are restricted to the immediate
/// types plus strings, and numerically-equal int/float keys collapse (`1` and
/// `1.0` are the same key). Strings hash by their interned [`GcRef`], so string
/// keys compare by content (interning guarantees ref-equality == content-
/// equality).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MapKey {
    Nil,
    Bool(bool),
    Int(i64),
    /// A non-integral float key, keyed by its bit pattern.
    FloatBits(u64),
    Str(GcRef),
}

/// A runtime error carrying a kind tag and source location, used while a thrown
/// value propagates. The *thrown value* itself is a [`Value`] (often a built-in
/// error object); this struct is the VM's internal bookkeeping when it
/// synthesizes one.
#[derive(Clone, Debug)]
pub struct RuntimeError {
    pub kind: &'static str,
    pub message: String,
    pub span: Option<Span>,
}

impl RuntimeError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        RuntimeError {
            kind,
            message: message.into(),
            span: None,
        }
    }
}

/// The set of built-in error kinds the runtime can throw (SPEC §7).
pub mod error_kind {
    pub const TYPE: &str = "TypeError";
    pub const NAME: &str = "NameError";
    pub const ARITY: &str = "ArityError";
    pub const INDEX: &str = "IndexError";
    pub const KEY: &str = "KeyError";
    pub const DIV_ZERO: &str = "DivisionByZero";
    pub const VALUE: &str = "ValueError";
    pub const STACK_OVERFLOW: &str = "StackOverflow";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truthiness() {
        assert!(!Value::Nil.is_truthy());
        assert!(!Value::Bool(false).is_truthy());
        assert!(Value::Bool(true).is_truthy());
        assert!(Value::Int(0).is_truthy()); // 0 is truthy in Lumen
        assert!(Value::Float(0.0).is_truthy());
    }

    #[test]
    fn numeric_coercion() {
        assert_eq!(Value::Int(3).as_f64(), Some(3.0));
        assert_eq!(Value::Float(2.5).as_f64(), Some(2.5));
        assert_eq!(Value::Nil.as_f64(), None);
    }
}
