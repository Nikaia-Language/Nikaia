//! `Bytes`: one shared buffer, and the thing a view of a file points into.
//!
//! Part III 17.2 says what it is: *`read` returns `Bytes`, not a `Vec[u8]`: it
//! is one shared buffer, and slices that outlive its scope are tethered to it
//! (Part I 6.6)*. Both halves of that sentence matter here and only the first
//! is this file's: a `Bytes` is a reference-counted run of bytes, so handing
//! one on costs a count rather than a copy. The second half — a slice that
//! *outlives* the buffer's scope — is the tether, which is not built, and the
//! compiler refuses the programs that would need it (`NK2303`) rather than
//! letting them reach this type.
//!
//! `Arc<[u8]>` and not `Arc<Vec<u8>>`: one allocation and one indirection, and
//! the buffer is immutable once it exists, which is what lets a clone be a
//! count. A program that wants to *build* bytes builds a `Vec[u8]` and hands it
//! over, which is the `From` below and the one copy in the whole type.

use std::sync::Arc;

/// A run of bytes that several holders may share
/// ([ADR-283](../../../docs/specification/adr/adr-283.md) D20).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Bytes {
    buffer: Arc<[u8]>,
}

/// Bytes are a buffer a view of text may be cut from (ADR-283 D15).
impl crate::tether::Viewed for Bytes {
    fn bytes(&self) -> &[u8] {
        &self.buffer
    }
}

impl Bytes {
    /// No bytes at all.
    pub fn new() -> Bytes {
        Bytes {
            buffer: Arc::from(Vec::new()),
        }
    }

    /// How many bytes there are.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// The bytes, as the language below spells a run of them.
    pub fn as_slice(&self) -> &[u8] {
        &self.buffer
    }

    /// The bytes as text, where they are text (ADR-320 D7).
    ///
    /// A **view** and not a copy: it points into the buffer and lives as long
    /// as the borrow does. `NotText` where the bytes are not UTF-8, the
    /// failure `fs::read_to_string` reports for the same bytes.
    pub fn text(&self) -> Result<&str, crate::io::IoError> {
        std::str::from_utf8(&self.buffer).map_err(|at| {
            crate::io::IoError::NotText(format!(
                "bytes that are not UTF-8 from byte {}",
                at.valid_up_to()
            ))
        })
    }

    /// The first `byte` at or after the position `from`, or `None`
    /// (ADR-320 D6): a byte search of the kind `memchr` does. A `from` before
    /// the start searches from the start; one past the end finds nothing.
    pub fn find(&self, byte: u8, from: i64) -> Option<i64> {
        let start = usize::try_from(from.max(0)).ok()?;
        let rest = self.buffer.get(start..)?;
        let at = rest.iter().position(|b| *b == byte)?;
        i64::try_from(start + at).ok()
    }

    /// A copy of the bytes, owned by the caller.
    ///
    /// The one place this type allocates on the way out, and it says so: Part I
    /// 6.6's promotion to `Owned` is never automatic.
    pub fn to_vec(&self) -> Vec<u8> {
        self.buffer.to_vec()
    }
}

impl std::ops::Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.buffer
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.buffer
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Bytes {
        Bytes {
            buffer: Arc::from(bytes),
        }
    }
}

impl From<&[u8]> for Bytes {
    fn from(bytes: &[u8]) -> Bytes {
        Bytes {
            buffer: Arc::from(bytes.to_vec()),
        }
    }
}

impl From<String> for Bytes {
    fn from(text: String) -> Bytes {
        Bytes::from(text.into_bytes())
    }
}

impl From<&str> for Bytes {
    fn from(text: &str) -> Bytes {
        Bytes::from(text.as_bytes().to_vec())
    }
}

impl FromIterator<u8> for Bytes {
    fn from_iter<I: IntoIterator<Item = u8>>(iter: I) -> Bytes {
        Bytes::from(iter.into_iter().collect::<Vec<u8>>())
    }
}

impl<'a> IntoIterator for &'a Bytes {
    type Item = &'a u8;
    type IntoIter = std::slice::Iter<'a, u8>;

    fn into_iter(self) -> Self::IntoIter {
        self.buffer.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::Bytes;

    /// **Handing one on is a count**, which is the whole claim of the type: two
    /// holders, one allocation.
    #[test]
    fn a_clone_shares_the_buffer() {
        let one = Bytes::from(vec![1u8, 2, 3]);
        let two = one.clone();
        assert_eq!(one.as_slice().as_ptr(), two.as_slice().as_ptr());
        assert_eq!(&*two, &[1u8, 2, 3]);
    }

    /// **It reads as a run of bytes**, so everything written against `&[u8]`
    /// takes one without knowing what it is.
    #[test]
    fn it_derefs_to_a_slice() {
        let bytes = Bytes::from("Ada");
        assert_eq!(bytes.len(), 3);
        assert!(!bytes.is_empty());
        assert_eq!(bytes.first(), Some(&b'A'));
    }

    /// **Text is a view of it**, and only where the bytes are text.
    #[test]
    fn text_is_a_view_and_may_not_be_there() {
        assert_eq!(Bytes::from("Ada").text().ok(), Some("Ada"));
        assert!(matches!(
            Bytes::from(vec![0xff_u8]).text(),
            Err(crate::io::IoError::NotText(_))
        ));
        let b = Bytes::from("a;b;c");
        assert_eq!(b.find(b';', 0), Some(1));
        assert_eq!(b.find(b';', 2), Some(3));
        assert_eq!(b.find(b';', 4), None);
        assert_eq!(b.find(b';', 9), None);
    }

    /// **And a copy is asked for**, never taken.
    #[test]
    fn a_copy_is_asked_for() {
        let bytes = Bytes::from(vec![7u8]);
        assert_eq!(bytes.to_vec(), vec![7u8]);
        assert_eq!(Bytes::new().len(), 0);
        assert_eq!((1u8..=3).collect::<Bytes>().to_vec(), vec![1, 2, 3]);
    }
}
