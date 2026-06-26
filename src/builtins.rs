//! The names of the global built-in functions.
//!
//! This is the single source of truth shared by the resolver (which must know
//! these names so it does not flag `print(...)` as an undefined variable) and by
//! the VM/stdlib (Phase 7), which registers an implementation for each. Keeping
//! the list here prevents the two from drifting apart.

/// Every globally-available built-in function, per SPEC §10.
pub const GLOBAL_BUILTINS: &[&str] = &[
    "print", "println", "str", "type", "len", "int", "float", "bool", "range",
    "assert", "clock", "input", "chr", "ord", "push", "pop", "keys", "values",
    "has", "del",
];

/// Is `name` a global built-in function?
pub fn is_builtin(name: &str) -> bool {
    GLOBAL_BUILTINS.contains(&name)
}
