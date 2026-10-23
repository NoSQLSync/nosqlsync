//! String ordering that matches JavaScript's `String.prototype.localeCompare`
//! closely enough for identifiers.
//!
//! The TypeScript engine sorts field paths with `localeCompare`, which orders
//! `a < A < b` and puts punctuation before digits before letters. A plain byte
//! comparison would order `B < a` and `_` after `Z`, so the same documents would
//! produce columns in a different order here than on the platform.

use std::cmp::Ordering;

/// ASCII punctuation and symbols in ICU root collation order.
const PUNCTUATION: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

/// Primary collation key for one character: punctuation, then digits, then letters.
fn primary(c: char) -> (u8, u32) {
    if let Some(i) = PUNCTUATION.find(c) {
        (0, i as u32)
    } else if c.is_ascii_digit() {
        (1, c as u32)
    } else if c.is_alphabetic() {
        (2, c.to_lowercase().next().unwrap_or(c) as u32)
    } else {
        (0, 1000 + c as u32)
    }
}

/// Compare two strings the way `a.localeCompare(b)` does for ASCII identifiers.
pub fn locale_cmp(a: &str, b: &str) -> Ordering {
    let primary = |s: &str| -> Vec<(u8, u32)> { s.chars().map(primary).collect() };
    match primary(a).cmp(&primary(b)) {
        Ordering::Equal => {}
        other => return other,
    }
    // Tertiary strength: lowercase sorts before uppercase.
    for (x, y) in a.chars().zip(b.chars()) {
        if x != y {
            return match (x.is_lowercase(), y.is_lowercase()) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => x.cmp(&y),
            };
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_locale_compare() {
        // Expected order taken from Node's `localeCompare`.
        let mut v = vec![
            "b", "B", "a", "_x", "1", "A", "a_b", "a.b", "ab", "user_id", "userId", "a1", "aB",
            "Ab",
        ];
        v.sort_by(|a, b| locale_cmp(a, b));
        assert_eq!(
            v,
            [
                "_x", "1", "a", "A", "a_b", "a.b", "a1", "ab", "aB", "Ab", "b", "B", "user_id",
                "userId"
            ]
        );
    }
}
