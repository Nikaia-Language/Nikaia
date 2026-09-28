//! **HTTP/1.1's text half**, as far as the MVP's protocol goes
//! ([ADR-194](../../../docs/specification/adr/adr-194.md) D5).
//!
//! `GET` and `POST`, bodies by `Content-Length`, `Connection: close`, no
//! chunked transfer and no TLS. That record's own scope, and its own staging:
//! **the parser was Rust here and is a Nikaia grammar now** (0.0.247,
//! [ADR-038](../../../docs/specification/adr/adr-038.md) D6), by
//! [ADR-196](../../../docs/specification/adr/adr-196.md)'s route:
//! `src/tools/http1.nika`. What stays here is the bytes - the buffer a socket
//! fills, where a head ends, whether it is text - and the `Head` a program is
//! handed.
//!
//! **Not named `http`**, and that is not a taste: a program reaches a *package*
//! by that word ([ADR-069](../../../docs/specification/adr/adr-069.md) D1), and
//! a `std` module of the same name would make `http::Response` mean two things.
//! `std`'s own ledger says what happens the day a crate's word collides with a
//! module's — *that is a refusal to write* — and this is the day not to create
//! one.
//!
//! ## Where the line between this and a package runs
//!
//! This module holds **framing**: where a head ends, what its lines say, and
//! how many bytes of body the head promised. A package holds **meaning**: what
//! a path is for, what a handler is, what goes back. `Buffer` is on this side
//! of the line because deciding that a head has not arrived yet is reading
//! HTTP/1.1, not reading a request.
//!
//! It reaches no socket. A program reads bytes with `net` and hands them here,
//! which is why every function below is `sync`: a server calls them between two
//! steps that do pause, and neither of them is this.
//!
//! ## What it refuses, and why a refusal is the feature
//!
//! A head longer than the cap, a `Content-Length` that is not a number, a
//! request line that is not three words. Each is a **400** or a **431** rather
//! than a guess: [ADR-010](../../../docs/specification/adr/adr-010.md) D2 says
//! these bytes are somebody else's, and a parser that guessed at them would be
//! guessing on their behalf.

use crate::io::IoError;

/// The bytes of one request as they arrive, and the questions HTTP/1.1 can ask
/// of them.
///
/// A `Vec[u8]` and not a `Bytes`: this one grows, and what makes a `Bytes` cheap
/// to hand on is that it does not
/// ([ADR-156](../../../docs/specification/adr/adr-156.md) D2). A caller reads a
/// chunk, `take`s it, and asks again.
#[derive(Debug, Default)]
pub struct Buffer {
    bytes: Vec<u8>,
}

impl Buffer {
    /// Nothing read yet. It is written `http1::Buffer()`.
    pub fn new() -> Buffer {
        Buffer { bytes: Vec::new() }
    }

    /// Add what the socket just handed over.
    ///
    /// `impl AsRef<[u8]>` for `net::Connection::write`'s reason: what a caller
    /// has is a `Bytes`, what a test has is text, and a signature that named one
    /// of them would refuse the other.
    pub fn take(&mut self, more: impl AsRef<[u8]>) {
        self.bytes.extend_from_slice(more.as_ref());
    }

    /// How many bytes have arrived.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether none have.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Where the head ends, or **`0` for *not yet***.
    ///
    /// Zero and not a missing value, which is a shape chosen for what it lets a
    /// caller write: a read loop over `while ended == 0` is one condition, where
    /// a loop over an absence is a condition plus the unwrapping that follows
    /// it. A head cannot end at byte zero — the shortest one there is is
    /// `GET / HTTP/1.0\r\n\r\n` — so the two readings can never collide.
    ///
    /// The **cap** is answered here rather than in `head`, because it is the
    /// question a *growing* buffer asks and `head` is asked once: a head that
    /// has not ended and is already over the cap never will.
    pub fn head_end(&self, cap: i64) -> Result<i64, IoError> {
        match find(&self.bytes, b"\r\n\r\n") {
            Some(at) => {
                let end = (at + 4) as i64;
                if end > cap {
                    return Err(IoError::Other("431 a request head over the cap".into()));
                }
                Ok(end)
            }
            None => {
                if self.bytes.len() as i64 > cap {
                    return Err(IoError::Other("431 a request head over the cap".into()));
                }
                Ok(0)
            }
        }
    }

    /// What the head says, where `head_end` said there is one.
    ///
    /// It finds the end again rather than being told it, so that a caller
    /// cannot pass a number this did not produce — four bytes over a buffer a
    /// request head long, twice, which is not where a server spends its time.
    pub fn head(&self) -> Result<Head, IoError> {
        let Some(at) = find(&self.bytes, b"\r\n\r\n") else {
            return Err(IoError::Other(
                "400 a request head that has not ended".into(),
            ));
        };
        let end = at + 4;
        let Ok(text) = std::str::from_utf8(&self.bytes[..end]) else {
            return Err(IoError::Other("400 a request head that is not text".into()));
        };
        parse(text, end as i64)
    }

    /// The bytes from `at` on, as text.
    ///
    /// **Owned**, and the copy is not an oversight: this is the body, and a body
    /// goes into a request a handler is given, which owns it. The view this
    /// could hand back instead would be copied there anyway.
    pub fn text_from(&self, at: i64) -> Result<String, IoError> {
        let at = at.max(0) as usize;
        if at > self.bytes.len() {
            return Err(IoError::Other(
                "400 a body that starts past its bytes".into(),
            ));
        }
        match std::str::from_utf8(&self.bytes[at..]) {
            Ok(text) => Ok(text.to_string()),
            Err(_) => Err(IoError::Other("400 a request body that is not text".into())),
        }
    }
}

/// What the head of one request says.
///
/// **`Clone`, because a Nikaia `struct` that holds one derives it.** Every
/// emitted struct does ([ADR-011](../../../docs/specification/adr/adr-011.md)
/// D2's lowering), so a `std` type a program puts in a field has to, or `rustc`
/// says *the trait bound `Head: Clone` is not satisfied* about a file nobody
/// wrote ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)). It
/// is plain owned data, so the derive costs a copy only where one is written.
#[derive(Clone, Debug)]
pub struct Head {
    method: String,
    path: String,
    length: i64,
    /// How many bytes the head took, so a caller knows where the body starts.
    size: i64,
    /// Whether the client asked to keep the connection open. **Read and not
    /// honoured** in the MVP: `Connection: close` is the scope
    /// ([ADR-194](../../../docs/specification/adr/adr-194.md) D5), and a server
    /// that read the header and ignored it silently would be lying to a client
    /// that asked.
    keep_alive: bool,
    /// Every header, name lowercased, in the order they arrived.
    ///
    /// **Kept rather than thrown away** ([ADR-018](../../../docs/specification/adr/adr-018.md)
    /// D4: *`request.header("host")` yields …, case-insensitive, as the protocol
    /// is*). The parse already walked them for `content-length` and
    /// `transfer-encoding` and dropped the rest, so a handler could not ask.
    ///
    /// A `Vec` and not a map: a request head has a handful of headers, and a
    /// scan over a handful beats hashing one ([ADR-009](../../../docs/specification/adr/adr-009.md)
    /// D4 — and there is nothing here to measure yet, which is itself the
    /// reason to take the shape with no allocation behind it).
    headers: Vec<(String, String)>,
}

impl Head {
    /// `GET` or `POST`, as it was written.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The path, **without** the query string
    /// ([ADR-018](../../../docs/specification/adr/adr-018.md) D4, which writes
    /// `path()` and `query()` as two things).
    ///
    /// Not decoded: what a `%20` means is the program's question, and a `std`
    /// that decided it would be deciding a security question on the program's
    /// behalf.
    pub fn path(&self) -> &str {
        match self.path.split_once('?') {
            Some((path, _)) => path,
            None => &self.path,
        }
    }

    /// The whole request target as it was written, query string and all.
    ///
    /// What a log wants, and the one place the bytes the client chose are handed
    /// over untouched.
    pub fn target(&self) -> &str {
        &self.path
    }

    /// What the query string says under this name, or nothing where it says
    /// nothing ([ADR-018](../../../docs/specification/adr/adr-018.md) D4:
    /// *Kap 3.5's nullable, not an empty string*).
    ///
    /// **Nothing is decoded**, and that is the same sentence `path` carries: a
    /// `%20` stays `%20` and a `+` stays a `+`. Which of the two a `+` means
    /// depends on who wrote the form, and `std` guessing it would be guessing on
    /// somebody else's bytes ([ADR-010](../../../docs/specification/adr/adr-010.md)
    /// D2).
    ///
    /// **A name with no `=` has an empty value**, which is not the same as being
    /// absent: `?debug` says the name was written.
    pub fn query(&self, name: impl AsRef<str>) -> Option<&str> {
        let name = name.as_ref();
        let (_, query) = self.path.split_once('?')?;
        query
            .split('&')
            .find_map(|pair| match pair.split_once('=') {
                Some((written, value)) if written == name => Some(value),
                Some(_) => None,
                None => (pair == name).then_some(""),
            })
    }

    /// What `Content-Length` said, or `0` where it said nothing.
    pub fn length(&self) -> i64 {
        self.length
    }

    /// How many bytes of the buffer the head took.
    pub fn size(&self) -> i64 {
        self.size
    }

    /// Whether the client asked for the connection to stay open.
    pub fn keep_alive(&self) -> bool {
        self.keep_alive
    }

    /// What the client sent under this name, or nothing where it sent none.
    ///
    /// **Case-insensitive, as the protocol is**
    /// ([ADR-018](../../../docs/specification/adr/adr-018.md) D4). The names
    /// were lowercased on the way in, so this lowercases what it is asked for
    /// and nothing else happens per call.
    ///
    /// `impl AsRef<str>` for `net::Connection::write`'s reason, which the ledger
    /// writes as a `?`: a **method's** argument is passed owned
    /// ([ADR-094](../../../docs/specification/adr/adr-094.md) §5 — the compiler
    /// cannot yet resolve which entry the call goes to), so a Nikaia method that
    /// hands its own `String` parameter through would otherwise be `rustc`
    /// saying *expected `&str`, found `String`* about a file nobody wrote.
    ///
    /// **The first, where a client sent the same name twice.** Joining them with
    /// a comma is what the protocol says a *list-valued* header means, and which
    /// headers those are is not something this module knows — so it hands back
    /// what arrived first and leaves the question to whoever needs it.
    pub fn header(&self, name: impl AsRef<str>) -> Option<&str> {
        let name = name.as_ref().to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(written, _)| *written == name)
            .map(|(_, value)| value.as_str())
    }

    /// How many headers there are, which is what a program counts before it
    /// decides a client is being unreasonable.
    pub fn headers(&self) -> usize {
        self.headers.len()
    }
}

/// The request line and the headers, where the head is already known to be text
/// and `size` bytes long.
///
/// **Read in Nikaia** (0.0.247): `src/tools/http1.nika` is the grammar and the
/// rules - which methods, which version, what a `Content-Length` has to be -
/// and this is the handover. The grammar reads every head that ended, so a
/// failure of it is a head this module has no words for, and it is refused as
/// one rather than guessed at.
fn parse(text: &str, size: i64) -> Result<Head, IoError> {
    let Ok(written) = crate::tools::http1::written(text) else {
        return Err(IoError::Other(
            "400 a request head this does not read".into(),
        ));
    };
    let read = crate::tools::http1::read(&written);
    if !read.refused.is_empty() {
        return Err(IoError::Other(read.refused.into()));
    }
    Ok(Head {
        method: read.method.to_string(),
        path: read.target.to_string(),
        length: read.length,
        size,
        keep_alive: read.keep_alive,
        headers: read
            .headers
            .into_iter()
            .map(|header| (header.name, header.value.to_string()))
            .collect(),
    })
}

/// Where `needle` starts in `haystack`.
///
/// A `memchr` here would be a dependency for a loop over a request head that
/// nobody has measured ([ADR-009](../../../docs/specification/adr/adr-009.md)
/// D4).
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&at| &haystack[at..at + needle.len()] == needle)
}
