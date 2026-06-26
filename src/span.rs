//! Source positions and spans.
//!
//! Every token and AST node carries a [`Span`] so that diagnostics can point at
//! the exact byte range, line, and column where something happened. Columns and
//! lines are 1-based (what an editor shows); `offset`/`len` are 0-based byte
//! offsets into the original source string and are what the diagnostic renderer
//! uses to slice out the surrounding line.

use std::fmt;

/// A half-open byte range `[offset, offset + len)` into a source file, annotated
/// with the 1-based line and column of its first byte.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// 0-based byte offset of the first byte of this span.
    pub offset: u32,
    /// Length of the span in bytes. May be zero (e.g. EOF markers).
    pub len: u32,
    /// 1-based line of the first byte.
    pub line: u32,
    /// 1-based column (counted in Unicode scalar values) of the first byte.
    pub col: u32,
}

impl Span {
    pub fn new(offset: u32, len: u32, line: u32, col: u32) -> Self {
        Span { offset, len, line, col }
    }

    /// A zero-length span at the very start of a file; useful as a placeholder
    /// for synthesized nodes that have no real source location.
    pub fn dummy() -> Self {
        Span { offset: 0, len: 0, line: 1, col: 1 }
    }

    /// The byte offset one past the end of this span.
    pub fn end(&self) -> u32 {
        self.offset + self.len
    }

    /// Produce a new span covering from the start of `self` to the end of
    /// `other`. Both spans are assumed to come from the same source and to be in
    /// order (`self` starts no later than `other`). Line/column are taken from
    /// `self`, the earlier span.
    pub fn to(&self, other: Span) -> Span {
        let start = self.offset.min(other.offset);
        let end = self.end().max(other.end());
        // Keep the line/col of whichever span starts earlier.
        let (line, col) = if self.offset <= other.offset {
            (self.line, self.col)
        } else {
            (other.line, other.col)
        };
        Span { offset: start, len: end - start, line, col }
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}", self.line, self.col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_is_offset_plus_len() {
        let s = Span::new(10, 5, 2, 3);
        assert_eq!(s.end(), 15);
    }

    #[test]
    fn to_merges_in_order() {
        let a = Span::new(4, 2, 1, 5);
        let b = Span::new(10, 3, 1, 11);
        let merged = a.to(b);
        assert_eq!(merged.offset, 4);
        assert_eq!(merged.end(), 13);
        assert_eq!((merged.line, merged.col), (1, 5));
    }

    #[test]
    fn to_merges_out_of_order() {
        let a = Span::new(10, 3, 2, 1);
        let b = Span::new(4, 2, 1, 5);
        let merged = a.to(b);
        assert_eq!(merged.offset, 4);
        assert_eq!(merged.end(), 13);
        // Line/col come from the earlier span (b).
        assert_eq!((merged.line, merged.col), (1, 5));
    }
}
