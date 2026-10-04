//! **Bytes checked as UTF-8 once, and text from then on.**
//!
//! A [`CheckedText`] owns (or maps) bytes that were validated when it was
//! made, and hands them out as `&str` on every access without checking them
//! again - which is the point: a 13 GB mapping is validated once, not per
//! view. A large input is checked in chunks across the `rayon` pool (feature
//! `rayon`), cut only where a character begins.
//!
//! Every `unsafe` in this crate, and the argument for it, is listed in
//! `README.md`.

#![deny(unsafe_op_in_unsafe_fn)]

use std::ops::Deref;

/// Bytes that are UTF-8, checked once.
pub struct CheckedText<B: AsRef<[u8]>> {
    bytes: B,
}

impl<B: AsRef<[u8]>> CheckedText<B> {
    /// Check `bytes`, or say at which byte they stop being text.
    pub fn check(bytes: B) -> Result<Self, usize> {
        validate(bytes.as_ref())?;
        Ok(CheckedText { bytes })
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        // SAFETY: `check` validated these bytes as UTF-8 before this value
        // existed, and `B` is only ever reached through `&self` - nothing here
        // hands out a way to change them.
        unsafe { std::str::from_utf8_unchecked(self.bytes.as_ref()) }
    }

    /// The bytes, as they were checked.
    pub fn into_inner(self) -> B {
        self.bytes
    }
}

impl CheckedText<Vec<u8>> {
    /// The checked bytes as a `String`, without checking them again.
    pub fn into_string(self) -> String {
        // SAFETY: validated in `check`, and owned here since.
        unsafe { String::from_utf8_unchecked(self.bytes) }
    }
}

impl<B: AsRef<[u8]>> Deref for CheckedText<B> {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl<B: AsRef<[u8]>> AsRef<str> for CheckedText<B> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// The mapping [`map`] hands back (feature `map`).
#[cfg(feature = "map")]
pub use memmap2::Mmap;

/// **A file's contents, mapped and checked as text** (feature `map`).
///
/// `Ok(Err(at))` is a file that is not UTF-8 at byte `at`. A zero-length file
/// cannot be mapped and is the caller's to handle.
///
/// The one caveat every memory map has holds here too, and is accepted rather
/// than hidden: another process that truncates or rewrites the file while it is
/// mapped changes what this value reads. `README.md` says so in full.
#[cfg(feature = "map")]
pub fn map(file: &std::fs::File) -> std::io::Result<Result<CheckedText<memmap2::Mmap>, usize>> {
    // SAFETY: a private, read-only mapping. The type system cannot rule out
    // another process changing the file underneath it, which is the documented
    // caveat of every memory map (README.md).
    let pages = unsafe { memmap2::Mmap::map(file)? };
    Ok(CheckedText::check(pages))
}

/// Below this, the pool costs more than the check does.
pub const CHUNKED_ABOVE: usize = 1 << 20;

fn threads() -> usize {
    #[cfg(feature = "rayon")]
    {
        rayon::current_num_threads()
    }
    #[cfg(not(feature = "rayon"))]
    {
        1
    }
}

/// UTF-8 or the first byte where it stops being so - the same answer a serial
/// check gives, found in chunks.
fn validate(bytes: &[u8]) -> Result<(), usize> {
    if bytes.len() < CHUNKED_ABOVE || threads() < 2 {
        return piece(bytes);
    }
    let cuts = char_boundaries(bytes, threads());
    chunks(bytes, &cuts)
}

/// One piece checked: UTF-8, or the offset of the first byte that is not.
///
/// With `simd`, `simdutf8`'s fast check answers the common case - the bytes
/// are text - and only a piece that fails is checked again by its `compat`
/// half, which finds the offset the standard library would. Both halves give
/// the standard library's verdict; the tests hold them to it.
fn piece(bytes: &[u8]) -> Result<(), usize> {
    #[cfg(feature = "simd")]
    {
        if simdutf8::basic::from_utf8(bytes).is_ok() {
            return Ok(());
        }
        simdutf8::compat::from_utf8(bytes)
            .map(|_| ())
            .map_err(|e| e.valid_up_to())
    }
    #[cfg(not(feature = "simd"))]
    {
        std::str::from_utf8(bytes)
            .map(|_| ())
            .map_err(|e| e.valid_up_to())
    }
}

#[cfg(feature = "rayon")]
fn chunks(bytes: &[u8], cuts: &[usize]) -> Result<(), usize> {
    use rayon::prelude::*;
    cuts.par_windows(2)
        .filter_map(|w| piece(&bytes[w[0]..w[1]]).err().map(|at| w[0] + at))
        .min()
        .map_or(Ok(()), Err)
}

#[cfg(not(feature = "rayon"))]
fn chunks(bytes: &[u8], cuts: &[usize]) -> Result<(), usize> {
    cuts.windows(2)
        .filter_map(|w| piece(&bytes[w[0]..w[1]]).err().map(|at| w[0] + at))
        .min()
        .map_or(Ok(()), Err)
}

/// `n` cuts through `bytes`, none of them inside a character.
///
/// A continuation byte is `10xxxxxx`, so a cut walks forward off one - at most
/// three times, since a fourth would be malformed and the chunk that holds it
/// says so. Every chunk therefore begins where a character begins, which is
/// what makes checking them separately the same check as checking the whole:
/// no sequence is split, and no chunk can accept a truncated one that the
/// chunk before it left behind.
fn char_boundaries(bytes: &[u8], n: usize) -> Vec<usize> {
    let mut cuts = Vec::with_capacity(n + 1);
    cuts.push(0);
    for k in 1..n {
        let mut at = bytes.len() / n * k;
        for _ in 0..3 {
            if at < bytes.len() && bytes[at] & 0xC0 == 0x80 {
                at += 1;
            }
        }
        cuts.push(at);
    }
    cuts.push(bytes.len());
    cuts.dedup();
    cuts
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "rayon")]
    use super::CHUNKED_ABOVE;
    use super::{CheckedText, piece, validate};

    #[test]
    #[cfg(feature = "map")]
    #[cfg_attr(miri, ignore = "Miri cannot map a file")]
    fn a_mapped_file_is_checked_text() {
        let path = std::env::temp_dir().join(format!("checked-text-{}", std::process::id()));
        std::fs::write(&path, "Oslo;3\n").expect("write");
        let file = std::fs::File::open(&path).expect("open");
        let text = super::map(&file).expect("map").expect("text");
        assert_eq!(&*text, "Oslo;3\n");
        std::fs::write(&path, b"ok\xFF").expect("write");
        let file = std::fs::File::open(&path).expect("open");
        assert_eq!(super::map(&file).expect("map").err(), Some(2));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn checked_text_is_text_and_a_break_is_refused_where_it_is() {
        let text = CheckedText::check(b"Hamburg;12.0".to_vec()).expect("text");
        assert_eq!(&*text, "Hamburg;12.0");
        assert_eq!(text.into_string(), "Hamburg;12.0");
        assert_eq!(CheckedText::check(b"ok\xFFno".to_vec()).err(), Some(2));
        let view: CheckedText<&[u8]> = CheckedText::check("aä€𝄞b".as_bytes()).expect("text");
        assert_eq!(view.chars().count(), 5);
    }

    fn serial(bytes: &[u8]) -> Result<(), usize> {
        std::str::from_utf8(bytes)
            .map(|_| ())
            .map_err(|e| e.valid_up_to())
    }

    /// Big enough to be chunked, and multi-byte throughout so that cuts land
    /// inside characters rather than only between them.
    #[cfg(feature = "rayon")]
    fn big() -> Vec<u8> {
        let unit = "aä€𝄞b";
        unit.repeat(CHUNKED_ABOVE / unit.len() + 4096).into_bytes()
    }

    #[test]
    #[cfg(feature = "rayon")]
    #[cfg_attr(miri, ignore = "megabytes of input; Miri runs the small ones")]
    fn a_chunked_check_accepts_what_a_serial_one_accepts() {
        let bytes = big();
        assert!(bytes.len() > CHUNKED_ABOVE, "the test input is not chunked");
        assert_eq!(validate(&bytes), Ok(()));
        assert_eq!(validate(&bytes), serial(&bytes));
    }

    #[test]
    #[cfg(feature = "rayon")]
    #[cfg_attr(miri, ignore = "megabytes of input; Miri runs the small ones")]
    fn a_break_anywhere_is_found_where_a_serial_check_finds_it() {
        // Around each cut and at a stride that is coprime with the character
        // width, so the corruption lands on lead bytes and continuation bytes
        // alike - a truncated sequence at a chunk's end is the case chunking
        // could get wrong, and this is where it would show.
        let base = big();
        let cuts = super::char_boundaries(&base, super::threads());

        let mut probes: Vec<usize> = (0..base.len()).step_by(7919).collect();
        for cut in &cuts {
            probes.extend((cut.saturating_sub(8)..(cut + 8).min(base.len())).step_by(1));
        }

        for at in probes {
            let mut bytes = base.clone();
            bytes[at] = 0xFF; // in no position a valid UTF-8 byte
            assert_eq!(
                validate(&bytes),
                serial(&bytes),
                "byte {at} of {}",
                bytes.len()
            );
        }
    }

    #[test]
    #[cfg(feature = "rayon")]
    #[cfg_attr(miri, ignore = "megabytes of input; Miri runs the small ones")]
    fn the_earliest_break_is_the_one_reported() {
        // One break per chunk: whichever core finishes first, the offset is
        // the first one, so a rejected file names the same byte on every
        // machine.
        let mut bytes = big();
        let cuts = super::char_boundaries(&bytes, super::threads());
        assert!(cuts.len() > 2, "one chunk proves nothing here");
        for w in cuts.windows(2) {
            bytes[w[0] + (w[1] - w[0]) / 2] = 0xFF;
        }
        assert_eq!(validate(&bytes), serial(&bytes));
    }

    /// The check of one piece gives the standard library's answer - verdict
    /// and offset - for every one- and two-byte sequence, and for every
    /// two-byte sequence followed by a third and fourth byte drawn from the
    /// bytes UTF-8 treats differently. Each sits once at the start, once in
    /// the middle of a long ASCII text and once across the 64-byte blocks a
    /// SIMD check reads, so that with `simd` the vector path is the one
    /// answering and its block edges are crossed.
    #[test]
    fn one_piece_answers_as_the_standard_library_does() {
        let edges: [u8; 12] = [
            0x00, 0x41, 0x7F, 0x80, 0xBF, 0xC0, 0xC2, 0xDF, 0xE0, 0xED, 0xF0, 0xF4,
        ];
        let mut cases: Vec<Vec<u8>> = Vec::new();
        for a in 0..=255u8 {
            cases.push(vec![a]);
            for b in 0..=255u8 {
                cases.push(vec![a, b]);
            }
        }
        for &a in &[0xE0u8, 0xE1, 0xED, 0xEF, 0xF0, 0xF1, 0xF4, 0xF5] {
            for b in 0x7Fu8..=0xC1 {
                for &c in &edges {
                    cases.push(vec![a, b, c]);
                    for &d in &edges {
                        cases.push(vec![a, b, c, d]);
                    }
                }
            }
        }
        let ascii = |n: usize| vec![b'a'; n];
        for case in &cases {
            for (head, tail) in [(0, 0), (100, 0), (61, 70), (127, 3)] {
                let mut bytes = ascii(head);
                bytes.extend_from_slice(case);
                bytes.extend(ascii(tail));
                assert_eq!(piece(&bytes), serial(&bytes), "{bytes:02X?}");
            }
        }
    }

    #[test]
    fn a_small_input_is_checked_in_one_piece() {
        assert_eq!(validate("Hamburg;12.0\n".as_bytes()), Ok(()));
        assert_eq!(validate(b""), Ok(()));
        assert_eq!(validate(b"ok\xFFno"), Err(2));
    }
}

/// The README's example, run as a test.
#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDoctests;
