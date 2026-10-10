// crates/nikaia-std/src/text_float.rs
//
// `text::parse_f64`, the one entry of `std::text` written in Rust: the shape
// of the text is checked here, against ADR-285 D35's rule, and the number it
// writes is read by Rust's `str::parse`, whose conversion rounds correctly
// (the nearest `f64`, ties to even). A conversion that rounds right is a
// library of its own, and this one is already in every program's core.

/// The number this text writes as an `f64`, or nothing where it writes none.
///
/// An optional `-` or `+`, decimal digits, an optional `.` followed by digits,
/// and an optional exponent: `e` or `E`, an optional sign, and digits. Or one
/// of `inf` and `nan` after the optional sign, in any case (`NaN`, `-Inf`).
/// Nothing else: no space, no `_`, no `.5`, no `1.`, no `infinity`.
pub fn parse_f64(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let mut at = 0;
    if matches!(bytes.first(), Some(b'-' | b'+')) {
        at = 1;
    }
    let rest = &text[at..];
    if rest.eq_ignore_ascii_case("inf") || rest.eq_ignore_ascii_case("nan") {
        return text.parse().ok();
    }
    let digits = |at: &mut usize| {
        let start = *at;
        while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
            *at += 1;
        }
        *at > start
    };
    if !digits(&mut at) {
        return None;
    }
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        if !digits(&mut at) {
            return None;
        }
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'-' | b'+')) {
            at += 1;
        }
        if !digits(&mut at) {
            return None;
        }
    }
    if at != bytes.len() {
        return None;
    }
    text.parse().ok()
}
