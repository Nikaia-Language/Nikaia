//! **`s.find(needle; from:)`** ([ADR-320](../../../docs/specification/adr/adr-320.md)
//! D6): the byte position of the first match at or after `from`, or `None`.
//!
//! The emitter writes `s.find(…)` on text as a call here, because the
//! language below's own `str::find` has another shape - one argument and a
//! `usize` - and an inherent method wins over anything a library adds.

/// The first match of `needle` in `text` at or after the byte `from`.
///
/// `from` may be any byte position: UTF-8 is self-synchronising, so a match
/// of a valid needle always starts on a scalar, and a search begun inside one
/// finds the next match. A `from` before the start searches from the start; one
/// past the end finds nothing. A one-byte needle is the byte search `memchr`
/// does, which is what the language below's `str::find` runs for it.
pub fn find(text: &str, needle: &str, from: i64) -> Option<i64> {
    let mut start = usize::try_from(from.max(0)).ok()?;
    if start > text.len() {
        return None;
    }
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let at = text[start..].find(needle)?;
    i64::try_from(start + at).ok()
}

#[cfg(test)]
mod tests {
    use super::find;

    #[test]
    fn a_match_at_or_after_the_position() {
        assert_eq!(find("Hamburg;12.0;x", ";", 0), Some(7));
        assert_eq!(find("Hamburg;12.0;x", ";", 8), Some(12));
        assert_eq!(find("Hamburg;12.0;x", ";", 13), None);
        assert_eq!(find("abc", "", 3), Some(3));
        assert_eq!(find("abc", "x", 4), None);
        assert_eq!(find("abc", "a", -5), Some(0));
    }

    #[test]
    fn a_search_begun_inside_a_scalar_finds_the_next_match() {
        // `ä` is two bytes; byte 1 is inside it.
        assert_eq!(find("äa;ä;", ";", 1), Some(3));
        assert_eq!(find("äöü", "ü", 1), Some(4));
    }
}
