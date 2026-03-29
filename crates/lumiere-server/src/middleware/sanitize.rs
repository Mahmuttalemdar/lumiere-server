/// Input sanitization utilities for user-provided strings.
///
/// These functions are called in route handlers before processing input —
/// they are NOT a global middleware layer. Apply them selectively to
/// message content, usernames, server names, and other user-facing text.

/// Strip null bytes and control characters from a string.
///
/// Preserves newlines (`\n`), carriage returns (`\r`), and tabs (`\t`)
/// since those are valid in message content. All other ASCII/Unicode
/// control characters (including null bytes) are removed.
/// Also strips Unicode bidi override characters to prevent text reordering attacks.

/// Returns `true` for Unicode bidirectional override / isolate characters
/// that can be abused to reorder displayed text (CVE-2021-42574 "Trojan Source").
fn is_bidi_override(c: char) -> bool {
    matches!(c,
        '\u{202A}'..='\u{202E}' | // LRE, RLE, PDF, LRO, RLO
        '\u{2066}'..='\u{2069}' | // LRI, RLI, FSI, PDI
        '\u{200E}' | '\u{200F}'   // LRM, RLM
    )
}

pub fn sanitize_string(input: &str) -> String {
    input
        .chars()
        .filter(|c| {
            (!c.is_control() || *c == '\n' || *c == '\r' || *c == '\t')
                && !is_bidi_override(*c)
        })
        .collect()
}

/// Normalize a display name by trimming whitespace and collapsing runs
/// of whitespace into single spaces.
///
/// Suitable for usernames, server names, channel names, and role names.
/// Does NOT perform Unicode NFC normalization (add `unicode-normalization`
/// crate if confusable/homoglyph defence is needed).
pub fn normalize_display_name(input: &str) -> String {
    collapse_whitespace(&sanitize_string(input))
}

/// Collapse consecutive whitespace characters into a single space
/// and trim leading/trailing whitespace.
pub fn collapse_whitespace(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut last_was_space = false;

    for c in input.chars() {
        if c.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            result.push(c);
            last_was_space = false;
        }
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_string_strips_null_bytes() {
        assert_eq!(sanitize_string("hello\0world"), "helloworld");
    }

    #[test]
    fn test_sanitize_string_strips_control_chars() {
        // BEL (0x07), ESC (0x1B)
        assert_eq!(sanitize_string("he\x07ll\x1Bo"), "hello");
    }

    #[test]
    fn test_sanitize_string_preserves_newlines() {
        assert_eq!(sanitize_string("line1\nline2\r\nline3"), "line1\nline2\r\nline3");
    }

    #[test]
    fn test_sanitize_string_preserves_tabs() {
        assert_eq!(sanitize_string("col1\tcol2"), "col1\tcol2");
    }

    #[test]
    fn test_collapse_whitespace_basic() {
        assert_eq!(collapse_whitespace("hello   world"), "hello world");
    }

    #[test]
    fn test_collapse_whitespace_mixed() {
        assert_eq!(collapse_whitespace("  hello \t world  "), "hello world");
    }

    #[test]
    fn test_collapse_whitespace_empty() {
        assert_eq!(collapse_whitespace("   "), "");
    }

    #[test]
    fn test_normalize_display_name() {
        assert_eq!(normalize_display_name("  John\0   Doe  "), "John Doe");
    }

    #[test]
    fn test_normalize_display_name_with_control() {
        assert_eq!(normalize_display_name("admin\x07\x1B name"), "admin name");
    }

    #[test]
    fn test_sanitize_string_strips_bidi_overrides() {
        // LRO (U+202D), RLO (U+202E), LRI (U+2066), PDI (U+2069)
        assert_eq!(sanitize_string("hello\u{202D}\u{202E}world"), "helloworld");
        assert_eq!(sanitize_string("a\u{2066}b\u{2069}c"), "abc");
    }

    #[test]
    fn test_sanitize_string_strips_lrm_rlm() {
        assert_eq!(sanitize_string("a\u{200E}b\u{200F}c"), "abc");
    }
}
