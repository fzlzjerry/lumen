//! Small shared formatting helpers used by the AST printer, the runtime value
//! display, and the JSON serializer. Keeping them in one place guarantees that a
//! float prints the same way everywhere (source, REPL, `str()`, JSON).

/// Render a float so that it always round-trips and always *looks* like a float
/// (carries a `.` or exponent) — `5.0`, not `5` (which would re-lex as an int).
/// `NaN`/infinities render as `nan`/`inf`/`-inf` (these cannot appear in source
/// but can arise from computation in error paths).
pub fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 {
            "-inf".to_string()
        } else {
            "inf".to_string()
        };
    }
    let s = format!("{f}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

/// Escape the contents of a string for re-emission inside double quotes. The
/// produced text, wrapped in `"`, re-lexes to exactly the original string. A
/// `$` is escaped only when it would otherwise start an interpolation (`${`),
/// keeping ordinary text (`"costs $5"`) readable.
pub fn escape_string(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 2);
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            '$' if chars.get(i + 1) == Some(&'{') => out.push_str("\\$"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a string's contents for embedding inside a JSON string literal.
/// Control characters become `\n`/`\t`/… or `\uXXXX`; `"` and `\` are escaped;
/// everything else (including non-ASCII) is passed through literally.
pub fn escape_string_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Is `s` a valid bare identifier (so a map key can be printed unquoted)?
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_always_look_like_floats() {
        assert_eq!(format_float(5.0), "5.0");
        assert_eq!(format_float(3.25), "3.25");
        assert_eq!(format_float(0.002), "0.002");
        assert_eq!(format_float(-1.5), "-1.5");
        assert_eq!(format_float(f64::INFINITY), "inf");
    }

    #[test]
    fn escapes_round_trip_dollar_brace() {
        assert_eq!(escape_string("a\nb"), "a\\nb");
        assert_eq!(escape_string("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_string("${x}"), "\\${x}");
        assert_eq!(escape_string("costs $5"), "costs $5"); // lone $ stays
    }

    #[test]
    fn identifier_check() {
        assert!(is_identifier("foo"));
        assert!(is_identifier("_x1"));
        assert!(!is_identifier("1x"));
        assert!(!is_identifier("a-b"));
        assert!(!is_identifier(""));
    }
}
