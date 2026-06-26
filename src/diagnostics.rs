//! Human-friendly diagnostics.
//!
//! Every stage of the pipeline (lexer, parser, resolver, compiler, VM) reports
//! problems as [`Diagnostic`] values. A diagnostic carries a [`Span`], a primary
//! message, an optional inline label rendered under the caret, and any number of
//! help notes. [`Diagnostic::render`] turns one into a multi-line, caret-
//! underlined message with the offending source line for context — the same
//! presentation a modern compiler gives.

use crate::span::Span;
use std::fmt;

/// How serious a diagnostic is. Lumen treats every static problem as a hard
/// error today, but warnings and notes are modelled so tooling (the LSP, the
/// resolver) can emit softer findings later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

/// A single reported problem, tied to a source location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Which pipeline stage produced this (`"lexer"`, `"parser"`, ...). Shown in
    /// brackets after the severity, e.g. `error[parser]`.
    pub stage: &'static str,
    pub message: String,
    pub span: Span,
    /// Short text drawn directly under the caret, e.g. `expected ';'`.
    pub label: Option<String>,
    /// Extra `help:`/`note:` lines printed below the snippet.
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(stage: &'static str, message: impl Into<String>, span: Span) -> Self {
        Diagnostic {
            severity: Severity::Error,
            stage,
            message: message.into(),
            span,
            label: None,
            notes: Vec::new(),
        }
    }

    pub fn warning(stage: &'static str, message: impl Into<String>, span: Span) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            stage,
            message: message.into(),
            span,
            label: None,
            notes: Vec::new(),
        }
    }

    /// Builder: attach the inline label drawn under the caret.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Builder: append a help/note line under the snippet.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Render this diagnostic against `source`, optionally naming the file in the
    /// location line. The output is several lines and ends without a trailing
    /// newline.
    pub fn render(&self, source: &str, file: Option<&str>) -> String {
        let mut out = String::new();
        // Header: `error[parser]: message`
        out.push_str(self.severity.label());
        out.push('[');
        out.push_str(self.stage);
        out.push_str("]: ");
        out.push_str(&self.message);
        out.push('\n');

        // Location line: ` --> file:line:col` (or ` --> line:col`).
        let where_ = match file {
            Some(name) => format!("{}:{}:{}", name, self.span.line, self.span.col),
            None => format!("{}:{}", self.span.line, self.span.col),
        };
        let gutter_w = self.span.line.to_string().len().max(1);
        let pad = " ".repeat(gutter_w);
        out.push_str(&format!("{} --> {}\n", pad, where_));

        // The source line containing the span's first byte.
        let (line_text, col0) = line_of(source, self.span.offset);
        out.push_str(&format!("{} |\n", pad));
        out.push_str(&format!("{} | {}\n", self.span.line, line_text));

        // Caret underline. Underline at least one column; never run past the end
        // of the rendered line, and stop at the first newline for multi-line
        // spans.
        let line_scalar_len = line_text.chars().count();
        let underline_len = caret_len(&line_text, col0, self.span.len as usize);
        let caret_pad = " ".repeat(col0.min(line_scalar_len));
        let carets = "^".repeat(underline_len.max(1));
        out.push_str(&format!("{} | {}{}", pad, caret_pad, carets));
        if let Some(label) = &self.label {
            out.push(' ');
            out.push_str(label);
        }
        out.push('\n');

        // Notes.
        for note in &self.notes {
            out.push_str(&format!("{} = help: {}\n", pad, note));
        }
        // Trim the final newline so callers control spacing.
        if out.ends_with('\n') {
            out.pop();
        }
        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}[{}] at {}: {}",
            self.severity.label(),
            self.stage,
            self.span,
            self.message
        )
    }
}

/// Return the text of the source line containing `offset`, plus the 0-based
/// column (in Unicode scalar values) of `offset` within that line.
fn line_of(source: &str, offset: u32) -> (String, usize) {
    let offset = offset as usize;
    let bytes = source.as_bytes();
    let clamped = offset.min(source.len());

    // Walk back to the start of the line.
    let mut start = clamped;
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    // Walk forward to the end of the line.
    let mut end = clamped;
    while end < source.len() && bytes[end] != b'\n' {
        end += 1;
    }
    let line_text = source[start..end].replace('\t', "    ");
    // Column = number of scalar values between the line start and the offset.
    // Account for tab expansion so the caret lines up with the rendered text.
    let prefix = &source[start..clamped];
    let mut col0 = 0usize;
    for ch in prefix.chars() {
        col0 += if ch == '\t' { 4 } else { 1 };
    }
    (line_text, col0)
}

/// How many caret characters to draw: the span's length in scalar values,
/// clamped so the underline stays on the first rendered line.
fn caret_len(line_text: &str, col0: usize, span_len_bytes: usize) -> usize {
    let line_scalar_len = line_text.chars().count();
    if span_len_bytes == 0 {
        return 1;
    }
    // We do not have the exact scalar length of the span without re-slicing the
    // source, so approximate using the remaining width of the line. This is
    // correct for single-line spans (the common case) and is clamped for
    // multi-line ones.
    let remaining = line_scalar_len.saturating_sub(col0);
    span_len_bytes.min(remaining).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_caret_under_token() {
        let src = "let x = ;\n";
        let span = Span::new(8, 1, 1, 9);
        let d = Diagnostic::error("parser", "expected expression", span)
            .with_label("unexpected ';'")
            .with_note("an expression is required after '='");
        let text = d.render(src, Some("t.lum"));
        assert!(text.contains("error[parser]: expected expression"));
        assert!(text.contains("t.lum:1:9"));
        assert!(text.contains("let x = ;"));
        assert!(text.contains("^"));
        assert!(text.contains("help: an expression is required after '='"));
    }

    #[test]
    fn caret_aligns_to_column() {
        let src = "abc\ndefg\n";
        // 'f' is at byte offset 6, line 2, col 3.
        let span = Span::new(6, 1, 2, 3);
        let d = Diagnostic::error("lexer", "bad char", span);
        let text = d.render(src, None);
        let caret_line = text.lines().find(|l| l.contains('^')).unwrap();
        // Two spaces of column padding before the caret.
        assert!(caret_line.contains("  ^"));
        assert!(text.contains("defg"));
    }

    #[test]
    fn tabs_are_expanded_consistently() {
        let src = "\tlet x = 1;\n";
        let span = Span::new(1, 3, 1, 2); // "let"
        let d = Diagnostic::error("lexer", "x", span);
        let text = d.render(src, None);
        // Tab expands to four spaces in both the source line and the caret pad.
        assert!(text.contains("    let x = 1;"));
    }
}
