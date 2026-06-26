//! A fast, non-cryptographic hasher (the FxHash algorithm, as used by rustc),
//! implemented from scratch so the crate stays dependency-free.
//!
//! The standard library's default `HashMap` hasher is SipHash — DoS-resistant
//! but slow for the short string keys Lumen hashes constantly (global names,
//! field/method names, map keys, interned strings). Swapping those maps to
//! [`FxHashMap`] speeds up global access, property lookup, and map operations
//! with no change in semantics. We don't need DoS resistance for an interpreter's
//! internal tables.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

const SEED: usize = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default)]
pub struct FxHasher {
    hash: usize,
}

impl FxHasher {
    #[inline]
    fn add(&mut self, word: usize) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        // Consume 8 bytes at a time, then 4, then the tail.
        while bytes.len() >= 8 {
            let word = usize::from_le_bytes(bytes[..8].try_into().unwrap());
            self.add(word);
            bytes = &bytes[8..];
        }
        if bytes.len() >= 4 {
            let word = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
            self.add(word);
            bytes = &bytes[4..];
        }
        for &b in bytes {
            self.add(b as usize);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as usize);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as usize);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i as usize);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash as u64
    }
}

/// A `HashMap` using [`FxHasher`].
pub type FxHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn works_as_a_map() {
        let mut m: FxHashMap<String, i32> = FxHashMap::default();
        m.insert("alpha".into(), 1);
        m.insert("beta".into(), 2);
        assert_eq!(m.get("alpha"), Some(&1));
        assert_eq!(m.get("beta"), Some(&2));
        assert_eq!(m.get("gamma"), None);
        // Same key updates, not duplicates.
        m.insert("alpha".into(), 9);
        assert_eq!(m.get("alpha"), Some(&9));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn distributes_short_keys() {
        // A trivial sanity check that distinct one-char keys don't all collide.
        let mut m: FxHashMap<String, usize> = FxHashMap::default();
        for c in 'a'..='z' {
            m.insert(c.to_string(), c as usize);
        }
        assert_eq!(m.len(), 26);
        assert_eq!(m.get("m"), Some(&('m' as usize)));
    }
}
