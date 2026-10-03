//! What a C library hands back that this language has to copy
//! ([ADR-147](../../../docs/specification/adr/adr-147.md) D4).
//!
//! `getenv` hands back a C string: memory the caller does not own, whose
//! lifetime is the library's, and which ends at a zero byte rather than
//! carrying a length. None of those three is something a `String` can be made
//! of without reading it, so the reading is here — **one function, written
//! once**, where every program would otherwise write the same loop inside its
//! own `unsafe` block.
//!
//! **The handle is opaque and has no `cleanup`**, which is what tells it from
//! [ADR-147](../../../docs/specification/adr/adr-147.md) D3's: a `FILE` is ours
//! to close and a C string is not ours at all. Nothing here frees anything.

/// **A C function said it hands back a handle, and handed back nothing**
/// ([ADR-155](../../../docs/specification/adr/adr-155.md) D3).
///
/// A declaration that does not say `?` is the author's **claim** that this
/// never happens — the arrangement `sync` on a declaration already has
/// ([ADR-124](../../../docs/specification/adr/adr-124.md) D2). A declaration is
/// written from a header by somebody reading it, and reading is where it can go
/// wrong, so the claim is checked: what a program gets is an abort in this
/// language's words rather than a handle that is secretly null.
///
/// `#[track_caller]` so that the location the panic hook is handed is the
/// **caller's** — the line of the generated file that made the call — and not a
/// line of this file, which
/// [ADR-300](../../../docs/specification/adr/adr-300.md) D9's table would have
/// nothing to look up for.
#[cold]
#[inline(never)]
#[track_caller]
pub fn nothing_came_back(declaration: &str) -> ! {
    panic!(
        "`{declaration}` handed back nothing, and its declaration does not say it can \
         - write `?` on the result to say that it may"
    )
}

/// What an `extern` declaration that says `-> foreign::CStr` returns: the
/// address C handed back, or `None` for C's `NULL`. The crate `c-text` holds
/// the one `unsafe` reading it takes (ADR-218), and the declaration is where
/// the promise *this is a C string* is made.
pub use c_text::CText;

/// **Text a C library owns** (D4): an address, and a zero byte somewhere after
/// it.
///
/// `#[repr(transparent)]` over [`CText`], which is over the address, so a
/// declaration may take or hand back one where C passes a `char *` - an
/// out-parameter included.
#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub struct CStr(CText);

impl CStr {
    /// What a declaration that says `-> CStr` hands back
    /// ([ADR-155](../../../docs/specification/adr/adr-155.md) D3): the text,
    /// or an abort naming the declaration that claimed there would be one.
    #[track_caller]
    pub fn from_c(declaration: &str, text: Option<CText>) -> CStr {
        match text {
            Some(text) => CStr(text),
            None => nothing_came_back(declaration),
        }
    }

    /// The same, where the declaration **does** say `?` (D1). `None` is C's
    /// `NULL`, which is the whole of D2: the two are one machine word.
    pub fn maybe(text: Option<CText>) -> Option<CStr> {
        text.map(CStr)
    }

    /// A copy of the text, owned by whoever asked for it.
    ///
    /// **It fails in one way**, and only one: the bytes may not be UTF-8, which
    /// text in this language is — the same failure `fs::read_to_string` has, for
    /// the same reason. *There is no text* is a **value**
    /// ([ADR-155](../../../docs/specification/adr/adr-155.md) D4): a declaration
    /// says `-> CStr?` where the library may find nothing, and the program
    /// writes `?? ""`.
    pub fn to_string(self) -> Result<String, crate::io::IoError> {
        self.0.to_string().map_err(|_| {
            crate::io::IoError::NotText("a C string this program was handed".to_string())
        })
    }
}
