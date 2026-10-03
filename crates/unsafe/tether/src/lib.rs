//! Views that outlive the scope that read their buffer.
//!
//! A view of text is a plain `&str`. When views have to outlive the local that
//! read the buffer, the buffer moves somewhere that lives as long as they do:
//!
//! * [`Keep`] — an append-only home for buffers; every buffer in it stays at
//!   its address until the `Keep` is dropped.
//! * [`Held`] — one view of text carrying its own handle on the buffer, for a
//!   container that drops entries while it keeps reading.
//! * [`Holding`] — a whole struct of views carrying a handle on each buffer it
//!   points into, with a **safe** constructor: the struct is built inside a
//!   closure that is handed the keeps and nothing else with a lifetime it
//!   could keep, so every view in it provably points into them.
//! * [`Keeps`] / [`Rebase`] / [`Hold`] — how a value made *outside* such a
//!   closure is carried in: each view is found again in the keep it points
//!   into ([`Keep::view`]), checked by address, so a view that points anywhere
//!   else becomes a copy the keep owns rather than a dangling reference.
//!
//! Every `unsafe` in this crate, and the argument for it, is listed in
//! `README.md`.

#![deny(unsafe_op_in_unsafe_fn)]

use std::any::Any;
use std::sync::{Arc, Mutex, OnceLock};

/// **A value whose references may outlive what they point into, as far as the
/// compiler is told.**
///
/// A struct holding a `&'static str` promises the compiler that the text is
/// valid wherever the struct is: passed by value into a function, the
/// reference is taken to be valid for the whole call. A value here is dropped
/// *before* the buffer it points into, inside such a call - so the promise has
/// to be withdrawn. A `MaybeUninit` makes none, which is the whole of what this
/// wrapper is for (the same device as `yoke`'s `KindaSortaDangling`).
///
/// Safe in itself: it stores a value and hands it back by reference. It is
/// what a value holding stretched views has to be stored in, beside the handle
/// that keeps their buffer - as [`Held`] and [`Holding`] do.
pub struct Dangling<T>(std::mem::MaybeUninit<T>);

impl<T> Dangling<T> {
    pub fn new(value: T) -> Self {
        Dangling(std::mem::MaybeUninit::new(value))
    }

    pub fn get(&self) -> &T {
        // SAFETY: initialised in `new` and only taken apart in `drop`.
        unsafe { self.0.assume_init_ref() }
    }

    pub fn get_mut(&mut self) -> &mut T {
        // SAFETY: as in `get`.
        unsafe { self.0.assume_init_mut() }
    }
}

impl<T> Drop for Dangling<T> {
    fn drop(&mut self) {
        // SAFETY: initialised in `new`, dropped exactly once, here.
        unsafe { self.0.assume_init_drop() }
    }
}

impl<T: Clone> Clone for Dangling<T> {
    fn clone(&self) -> Self {
        Dangling::new(self.get().clone())
    }
}

/// Buffers whose views outlive the scope that made them.
///
/// **Append-only, and nothing in it ever moves**: a buffer is boxed when it
/// arrives and the box is freed only when the `Keep` is, so a view of an
/// earlier buffer stays valid while later ones arrive. That is what lets one
/// `Keep` serve a loop over many files or a recursive include, with no
/// per-view count: one per buffer (ADR-283 D11), and no table of buffers.
///
/// **The first buffer takes no lock and no list.** A keep of a buffer's own -
/// one per file a pruned list reads, held by the elements that point into it -
/// holds exactly one, so that one lives in a slot set once, and only a second
/// takes the lock, which is for [`Keep::put`] through `&self` from two tasks at
/// once: once per **buffer**, never per view.
pub struct Keep {
    first: OnceLock<Entry>,
    rest: Mutex<Vec<Entry>>,
}

/// One buffer.
///
/// An `Arc` and not a `Box`: moving a `Box` asserts it is the only way to its
/// contents, which would invalidate every view already handed out - moving the
/// `Arc` touches nothing it points at.
enum Entry {
    Opaque(#[allow(dead_code)] Arc<dyn Any + Send + Sync>),
    /// Put in with [`Keep::put_viewed`]: one [`Keep::find`] can find a view in.
    Viewed(Arc<dyn Viewed>),
}

impl Keep {
    pub fn new() -> Keep {
        Keep {
            first: OnceLock::new(),
            rest: Mutex::new(Vec::new()),
        }
    }

    fn rest(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.rest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn keep(&self, entry: Entry) {
        if let Err(entry) = self.first.set(entry) {
            self.rest().push(entry);
        }
    }

    /// Move a buffer in, and hand back the place it now lives, for as long as
    /// this `Keep` is borrowed.
    pub fn put<B: Any + Send + Sync>(&self, buffer: B) -> &B {
        let shared: Arc<B> = Arc::new(buffer);
        let at: *const B = Arc::as_ptr(&shared);
        self.keep(Entry::Opaque(shared));
        // SAFETY: the `Arc` was just stored and is never removed or handed out
        // until `self` is dropped, so the buffer it points at stays alive and
        // in place; nothing writes to it. So the reference is valid for as
        // long as `self` is borrowed, which is the lifetime it is given.
        unsafe { &*at }
    }

    /// [`Keep::put`], for a buffer views of text are cut from - which
    /// [`Keep::find`] can then find a view in.
    pub fn put_viewed<B: Viewed>(&self, buffer: B) -> &B {
        let shared: Arc<B> = Arc::new(buffer);
        let at: *const B = Arc::as_ptr(&shared);
        self.keep(Entry::Viewed(shared));
        // SAFETY: as in `put`.
        unsafe { &*at }
    }

    /// The same text, as a view of a buffer this `Keep` holds - or `None`
    /// where `text` points into none of them.
    ///
    /// **Found by address**, not by content: the view handed back is the very
    /// bytes `text` is, now borrowed from this `Keep` rather than from
    /// wherever `text` was borrowed from.
    pub fn find(&self, text: &str) -> Option<&str> {
        if text.is_empty() {
            return Some("");
        }
        let found = |entry: &Entry| match entry {
            Entry::Viewed(buffer) => {
                // SAFETY: as in `put` - the `Arc` stays in this `Keep` until it
                // is dropped, so the buffer lives, stays in place and is never
                // written for as long as `self` is borrowed; so a reference to
                // it may be given that lifetime rather than the lock's.
                let buffer: &dyn Viewed = unsafe { &*Arc::as_ptr(buffer) };
                within(buffer.bytes(), text)
            }
            Entry::Opaque(_) => None,
        };
        let first = self.first.get()?;
        found(first).or_else(|| self.rest().iter().find_map(found))
    }

    /// The same text, as a view of this `Keep`: found in the buffer it points
    /// into, or - where it points into none of them, such as a literal - a
    /// copy of it put here. Never a reference to anything this `Keep` does
    /// not own.
    pub fn view(&self, text: &str) -> &str {
        match self.find(text) {
            Some(found) => found,
            None => self.put_viewed(text.to_owned()).as_str(),
        }
    }

    /// How many buffers are kept, for `--tethers` and for tests.
    pub fn len(&self) -> usize {
        match self.first.get() {
            Some(_) => 1 + self.rest().len(),
            None => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for Keep {
    fn default() -> Keep {
        Keep::new()
    }
}

/// `text`, where its bytes lie inside `whole`: the same bytes, borrowed from
/// `whole`.
fn within<'a>(whole: &'a [u8], text: &str) -> Option<&'a str> {
    let from = text.as_ptr().addr().checked_sub(whole.as_ptr().addr())?;
    let bytes = whole.get(from..from.checked_add(text.len())?)?;
    // SAFETY: `bytes` are the very bytes `text` is made of - the same
    // addresses, read through two shared borrows while neither is written -
    // and `text` is a `str`, so they are UTF-8.
    Some(unsafe { std::str::from_utf8_unchecked(bytes) })
}

/// **A buffer views of text are cut from**: owned text, or bytes that hold it.
///
/// What [`Keep::put_viewed`] takes, so that [`Keep::view`] can find a view in
/// it again by address.
pub trait Viewed: Any + Send + Sync {
    /// The buffer's bytes, where its views point.
    fn bytes(&self) -> &[u8];
}

impl Viewed for String {
    fn bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl Viewed for Box<str> {
    fn bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl Viewed for Arc<str> {
    fn bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl Viewed for Vec<u8> {
    fn bytes(&self) -> &[u8] {
        self
    }
}

/// One view of text that carries its own handle on the buffer it points into
/// for a container that drops entries while the loop around it
/// keeps reading, where one `Keep` for the whole loop would keep every buffer
/// the loop ever read.
///
/// **Compared and hashed by content**, never by where it points: two files
/// that both say `Hamburg` are one key.
#[derive(Clone)]
pub struct Held {
    // Declared first, so it is dropped before the handle on its buffer.
    text: Dangling<&'static str>,
    _keep: Arc<Keep>,
}

impl Held {
    fn text(&self) -> &str {
        self.text.get()
    }
}

impl Held {
    /// `text`, held by the keep it points into: the first of `keeps` that
    /// holds it, or - where none does - a copy of it put into the first.
    ///
    /// # Panics
    ///
    /// Where `keeps` is empty.
    pub fn new(keeps: &[Arc<Keep>], text: &str) -> Held {
        let (keep, view) = keeps
            .iter()
            .find_map(|keep| keep.find(text).map(|view| (keep, view)))
            .unwrap_or_else(|| {
                let keep = keeps.first().expect("a held view needs a keep");
                (keep, keep.view(text))
            });
        Held {
            // SAFETY: `view` points into `keep` - `find` and `view` hand back
            // nothing else - and the clone stored beside it keeps `keep`, and
            // so the buffer, alive and in place as long as this value.
            text: Dangling::new(unsafe { std::mem::transmute::<&str, &'static str>(view) }),
            _keep: Arc::clone(keep),
        }
    }
}

impl std::ops::Deref for Held {
    type Target = str;
    fn deref(&self) -> &str {
        self.text()
    }
}

impl std::borrow::Borrow<str> for Held {
    fn borrow(&self) -> &str {
        self.text()
    }
}

impl AsRef<str> for Held {
    fn as_ref(&self) -> &str {
        self.text()
    }
}

impl PartialEq for Held {
    fn eq(&self, other: &Held) -> bool {
        self.text() == other.text()
    }
}

impl Eq for Held {}

impl PartialEq<str> for Held {
    fn eq(&self, other: &str) -> bool {
        self.text() == other
    }
}

impl PartialEq<&str> for Held {
    fn eq(&self, other: &&str) -> bool {
        self.text() == *other
    }
}

impl PartialOrd for Held {
    fn partial_cmp(&self, other: &Held) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Held {
    fn cmp(&self, other: &Held) -> std::cmp::Ordering {
        self.text().cmp(other.text())
    }
}

impl std::hash::Hash for Held {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.text().hash(state)
    }
}

impl std::fmt::Display for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.text(), f)
    }
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.text(), f)
    }
}

/// **A struct of views, over any lifetime** — what [`Holding`] holds.
///
/// Implemented once per struct, always the same way:
///
/// ```
/// struct Record<'a> { name: &'a str }
/// enum RecordViews {}
/// impl tether::Views for RecordViews {
///     type Of<'a> = Record<'a>;
///     fn shorten<'long: 's, 's>(x: &'s Record<'long>) -> &'s Record<'s> { x }
/// }
/// ```
///
/// The body `x` compiles only where the struct is **covariant** in its
/// lifetime, which is the one property [`Holding`] relies on: a struct that
/// could be written through (a `Cell<&'a str>`, a `&'a mut`) is refused by
/// `rustc` right here.
pub trait Views {
    type Of<'a>;
    fn shorten<'long: 's, 's>(x: &'s Self::Of<'long>) -> &'s Self::Of<'s>;
}

/// **The keeps a [`Holding`] is built from**, borrowed for a lifetime the
/// closure building it cannot name - so what it builds can point into them
/// and nowhere else.
pub struct Keeps<'a, const N: usize> {
    keeps: [&'a Keep; N],
}

impl<'a, const N: usize> Keeps<'a, N> {
    /// `text`, as a view of the keep it points into - or, where it points into
    /// none of them, a copy put into the first ([`Keep::view`]).
    pub fn view(&self, text: &str) -> &'a str {
        const { assert!(N > 0, "a holding needs a keep") };
        self.keeps
            .iter()
            .find_map(|keep| keep.find(text))
            .unwrap_or_else(|| self.keeps[0].view(text))
    }

    /// One of the keeps.
    pub fn get(&self, at: usize) -> &'a Keep {
        self.keeps[at]
    }
}

/// **A value whose views are carried into [`Keeps`]**: the same value, each
/// view in it found again in the keep it points into.
///
/// Implemented here for a view of text and the containers of one a struct of
/// views holds; the Nikaia compiler writes it for each such struct, one
/// [`Rebase::rebase`] per field that holds a view.
pub trait Rebase<'a> {
    /// The same type, over `'a`.
    type At;
    fn rebase<const N: usize>(self, keeps: &Keeps<'a, N>) -> Self::At;
}

impl<'a> Rebase<'a> for &str {
    type At = &'a str;
    fn rebase<const N: usize>(self, keeps: &Keeps<'a, N>) -> &'a str {
        keeps.view(self)
    }
}

impl<'a, T: Rebase<'a>> Rebase<'a> for Option<T> {
    type At = Option<T::At>;
    fn rebase<const N: usize>(self, keeps: &Keeps<'a, N>) -> Option<T::At> {
        self.map(|value| value.rebase(keeps))
    }
}

impl<'a, T: Rebase<'a>> Rebase<'a> for Vec<T> {
    type At = Vec<T::At>;
    fn rebase<const N: usize>(self, keeps: &Keeps<'a, N>) -> Vec<T::At> {
        self.into_iter().map(|value| value.rebase(keeps)).collect()
    }
}

/// **A value with no view in it is carried in as it is**: a number, a
/// `bool`, a `char`, owned text. What a task takes with it is often a map
/// from views to counts, and the counts have nowhere to point.
macro_rules! rebased_as_they_are {
    ($($ty:ty),* $(,)?) => {
        $(
            impl<'a> Rebase<'a> for $ty {
                type At = $ty;
                fn rebase<const N: usize>(self, _keeps: &Keeps<'a, N>) -> $ty {
                    self
                }
            }
        )*
    };
}

rebased_as_they_are!(
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    f32,
    f64,
    bool,
    char,
    String,
    ()
);

impl<'a, T: Rebase<'a>> Rebase<'a> for Box<T> {
    type At = Box<T::At>;
    fn rebase<const N: usize>(self, keeps: &Keeps<'a, N>) -> Box<T::At> {
        Box::new((*self).rebase(keeps))
    }
}

impl<'a, K, V, S> Rebase<'a> for std::collections::HashMap<K, V, S>
where
    K: Rebase<'a>,
    K::At: Eq + std::hash::Hash,
    V: Rebase<'a>,
    S: std::hash::BuildHasher + Default,
{
    type At = std::collections::HashMap<K::At, V::At, S>;
    fn rebase<const N: usize>(self, keeps: &Keeps<'a, N>) -> Self::At {
        self.into_iter()
            .map(|(key, value)| (key.rebase(keeps), value.rebase(keeps)))
            .collect()
    }
}

impl<'a, K, V> Rebase<'a> for std::collections::BTreeMap<K, V>
where
    K: Rebase<'a>,
    K::At: Ord,
    V: Rebase<'a>,
{
    type At = std::collections::BTreeMap<K::At, V::At>;
    fn rebase<const N: usize>(self, keeps: &Keeps<'a, N>) -> Self::At {
        self.into_iter()
            .map(|(key, value)| (key.rebase(keeps), value.rebase(keeps)))
            .collect()
    }
}

impl<'a, T, S> Rebase<'a> for std::collections::HashSet<T, S>
where
    T: Rebase<'a>,
    T::At: Eq + std::hash::Hash,
    S: std::hash::BuildHasher + Default,
{
    type At = std::collections::HashSet<T::At, S>;
    fn rebase<const N: usize>(self, keeps: &Keeps<'a, N>) -> Self::At {
        self.into_iter().map(|value| value.rebase(keeps)).collect()
    }
}

impl<'a, T> Rebase<'a> for std::collections::BTreeSet<T>
where
    T: Rebase<'a>,
    T::At: Ord,
{
    type At = std::collections::BTreeSet<T::At>;
    fn rebase<const N: usize>(self, keeps: &Keeps<'a, N>) -> Self::At {
        self.into_iter().map(|value| value.rebase(keeps)).collect()
    }
}

macro_rules! rebased_tuples {
    ($(($($name:ident),+)),* $(,)?) => {
        $(
            #[allow(non_snake_case)]
            impl<'a, $($name: Rebase<'a>),+> Rebase<'a> for ($($name,)+) {
                type At = ($($name::At,)+);
                fn rebase<const N: usize>(self, keeps: &Keeps<'a, N>) -> Self::At {
                    let ($($name,)+) = self;
                    ($($name.rebase(keeps),)+)
                }
            }
        )*
    };
}

rebased_tuples!(
    (A),
    (A, B),
    (A, B, C),
    (A, B, C, D),
    (A, B, C, D, E),
    (A, B, C, D, E, F)
);

/// **A value put into a container that drops entries**, held by the keeps
/// of the buffers it points into: a view of text becomes a [`Held`], a
/// struct of views a [`Holding`].
///
/// The Nikaia compiler writes it for each struct of views, as
/// `Holding::new(keeps, move |k| Rebase::rebase(self, &k))`.
pub trait Hold<const N: usize> {
    type Held;
    fn hold(self, keeps: [Arc<Keep>; N]) -> Self::Held;
}

impl<const N: usize> Hold<N> for &str {
    type Held = Held;
    fn hold(self, keeps: [Arc<Keep>; N]) -> Held {
        Held::new(&keeps, self)
    }
}

impl<const N: usize, T: Hold<N>> Hold<N> for Option<T> {
    type Held = Option<T::Held>;
    fn hold(self, keeps: [Arc<Keep>; N]) -> Option<T::Held> {
        self.map(|value| value.hold(keeps))
    }
}

/// **A struct of views held beside the buffers it points into.**
///
/// One handle per buffer, `N` of them, and no allocation of its own: each
/// buffer lives in a [`Keep`] behind an `Arc` and does not move when this
/// value does. A read goes through [`Holding::get`], which hands the struct out
/// over a lifetime no longer than the read, so nothing derived from it can
/// outlive the buffers.
pub struct Holding<V: Views, const N: usize = 1> {
    // Declared first, so it is dropped before the buffers it points into.
    value: Dangling<V::Of<'static>>,
    keeps: [Arc<Keep>; N],
}

impl<V: Views, const N: usize> Holding<V, N> {
    /// Build the struct from the keeps. The closure is handed them and
    /// nothing else with a lifetime it could keep, and it has to work for
    /// **every** lifetime, so what it returns can point into them or at
    /// `'static` data and nowhere else.
    pub fn new(
        keeps: [Arc<Keep>; N],
        build: impl for<'a> FnOnce(Keeps<'a, N>) -> V::Of<'a>,
    ) -> Self {
        let value = build(Keeps {
            keeps: keeps.each_ref().map(|keep| &**keep),
        });
        let value = std::mem::ManuallyDrop::new(value);
        // SAFETY: `V::Of<'_>` and `V::Of<'static>` are one type up to a
        // lifetime, so the bits are the same value; `ManuallyDrop` keeps the
        // original from being dropped twice. The longer lifetime is never
        // observed: `value` is private, dropped before `keeps` (field order),
        // and handed out only through `get`, which shortens it back to a
        // borrow of `self` - and `self` holds the `Arc`s that keep each
        // `Keep`, and every buffer in it, where it is.
        let value = unsafe { std::mem::transmute_copy::<V::Of<'_>, V::Of<'static>>(&value) };
        Holding {
            value: Dangling::new(value),
            keeps,
        }
    }

    /// The struct, over a lifetime no longer than this borrow.
    pub fn get(&self) -> &V::Of<'_> {
        V::shorten(self.value.get())
    }

    /// The keeps the struct points into.
    pub fn keeps(&self) -> &[Arc<Keep>; N] {
        &self.keeps
    }

    /// **Write the struct**, through a closure handed it and the keeps over a
    /// lifetime it cannot name - so what it writes into a view field can only
    /// come from the keeps (a view made outside comes in through
    /// [`Keeps::view`]) or be `'static`. `self_cell`'s `with_dependent_mut`,
    /// for the same reason.
    ///
    /// A view of a local is refused by `rustc`, which is the whole argument:
    ///
    /// ```compile_fail
    /// # use std::sync::Arc;
    /// # use tether::{Hold, Holding, Keep, Keeps, Rebase, Views};
    /// # struct Name<'a>(&'a str);
    /// # enum NameViews {}
    /// # impl Views for NameViews {
    /// #     type Of<'a> = Name<'a>;
    /// #     fn shorten<'long: 's, 's>(x: &'s Name<'long>) -> &'s Name<'s> { x }
    /// # }
    /// let keep = Arc::new(Keep::new());
    /// let mut held: Holding<NameViews> = Holding::new([keep], |k| Name(k.view("a")));
    /// let local = String::from("gone soon");
    /// held.with_mut(|name, _| name.0 = &local);
    /// ```
    pub fn with_mut<R>(
        &mut self,
        write: impl for<'a> FnOnce(&mut V::Of<'a>, Keeps<'a, N>) -> R,
    ) -> R {
        let keeps = Keeps {
            keeps: self.keeps.each_ref().map(|keep| &**keep),
        };
        let value: *mut V::Of<'static> = self.value.get_mut();
        // SAFETY: the same value up to a lifetime, borrowed mutably for as
        // long as `self` is. The lifetime handed out is one the closure has to
        // treat as any lifetime at all: behind `&mut` it cannot be shortened,
        // so the closure can store into the struct only what lives at least as
        // long as the keeps do - a view of them, or `'static` data - and the
        // keeps live as long as `self`.
        let value: &mut V::Of<'_> = unsafe { &mut *value.cast() };
        write(value, keeps)
    }
}

impl<V: Views, const N: usize> Clone for Holding<V, N>
where
    for<'a> V::Of<'a>: Clone,
{
    fn clone(&self) -> Self {
        let copy = self.get().clone();
        let copy = std::mem::ManuallyDrop::new(copy);
        // SAFETY: as in `new` - the copy points where the original does, into
        // `self.keeps`, and the clones of the `Arc`s stored beside it keep
        // those buffers alive and in place.
        let value = unsafe { std::mem::transmute_copy::<V::Of<'_>, V::Of<'static>>(&copy) };
        Holding {
            value: Dangling::new(value),
            keeps: self.keeps.clone(),
        }
    }
}

impl<V: Views, const N: usize> std::fmt::Debug for Holding<V, N>
where
    for<'a> V::Of<'a>: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.get(), f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug)]
    struct Record<'a> {
        name: &'a str,
        parts: Vec<&'a str>,
    }
    enum RecordViews {}
    impl Views for RecordViews {
        type Of<'a> = Record<'a>;
        fn shorten<'long: 's, 's>(x: &'s Record<'long>) -> &'s Record<'s> {
            x
        }
    }

    impl<'a> Rebase<'a> for Record<'_> {
        type At = Record<'a>;
        fn rebase<const N: usize>(self, keeps: &Keeps<'a, N>) -> Record<'a> {
            Record {
                name: self.name.rebase(keeps),
                parts: self.parts.rebase(keeps),
            }
        }
    }

    impl<const N: usize> Hold<N> for Record<'_> {
        type Held = Holding<RecordViews, N>;
        fn hold(self, keeps: [Arc<Keep>; N]) -> Self::Held {
            Holding::new(keeps, move |k| self.rebase(&k))
        }
    }

    /// A keep of its own holding one buffer, and that buffer's text.
    fn kept(text: &str) -> Arc<Keep> {
        let keep = Arc::new(Keep::new());
        keep.put_viewed(text.to_string());
        keep
    }

    #[test]
    fn a_struct_of_views_outlives_the_scope_that_read_its_buffer() {
        let mut kept: std::collections::VecDeque<Holding<RecordViews>> = Default::default();
        for i in 0..20 {
            let keep = Arc::new(Keep::new());
            let text = keep.put_viewed(format!("  a{i} b{i} c{i}  "));
            kept.push_back(Holding::new([Arc::clone(&keep)], |k| Record {
                name: k.view(text.trim()),
                parts: text.split_whitespace().map(|p| k.view(p)).collect(),
            }));
            // The element is now the buffer's only owner.
            drop(keep);
            if kept.len() > 3 {
                kept.pop_front();
            }
        }
        // Every element moves.
        let moved: Vec<Holding<RecordViews>> = kept.into_iter().collect();
        let names: Vec<&str> = moved.iter().map(|h| h.get().name).collect();
        assert_eq!(names, ["a17 b17 c17", "a18 b18 c18", "a19 b19 c19"]);
        assert_eq!(moved[2].get().parts, ["a19", "b19", "c19"]);
    }

    /// The case `Dangling` exists for: the last owner of a buffer handed by
    /// value into a function that drops it there - for `Held` and `Holding`.
    #[test]
    fn the_last_owner_is_dropped_inside_a_call() {
        fn consume_held(held: Held) -> usize {
            let n = held.len();
            drop(held);
            n
        }
        fn consume_holding(holding: Holding<RecordViews>) -> usize {
            let n = holding.get().parts.len();
            drop(holding);
            n
        }
        let keep = Arc::new(Keep::new());
        let text = keep.put_viewed("one two".to_string());
        let held = Held::new(&[Arc::clone(&keep)], &text[..3]);
        drop(keep);
        assert_eq!(consume_held(held), 3);
        let keep = Arc::new(Keep::new());
        let text = keep.put_viewed("a b c".to_string());
        let holding = Record {
            name: text,
            parts: text.split(' ').collect(),
        }
        .hold([Arc::clone(&keep)]);
        drop(keep);
        assert_eq!(consume_holding(holding), 3);
    }

    /// **A map from views to counts is carried in whole**: each key found
    /// again in the keep by address, each count as it is - what a task takes
    /// with it most often (Nikaia's #84).
    #[test]
    fn a_map_of_views_is_rebased_without_a_copy() {
        enum CountViews {}
        impl Views for CountViews {
            type Of<'a> = std::collections::HashMap<&'a str, (i64, Vec<&'a str>)>;
            fn shorten<'long: 's, 's>(
                x: &'s std::collections::HashMap<&'long str, (i64, Vec<&'long str>)>,
            ) -> &'s std::collections::HashMap<&'s str, (i64, Vec<&'s str>)> {
                x
            }
        }
        let keep = Arc::new(Keep::new());
        let text = keep.put_viewed("a b a".to_string());
        let mut counts: std::collections::HashMap<&str, (i64, Vec<&str>)> =
            std::collections::HashMap::new();
        for word in text.split(' ') {
            let entry = counts.entry(word).or_insert((0, Vec::new()));
            entry.0 += 1;
            entry.1.push(word);
        }
        let held: Holding<CountViews> =
            Holding::new([Arc::clone(&keep)], move |keeps| counts.rebase(&keeps));
        // Found, not copied: the keep holds the one buffer it was given.
        assert_eq!(keep.len(), 1);
        drop(keep);
        assert_eq!(held.get()["a"].0, 2);
        assert_eq!(held.get()["b"].1, ["b"]);
    }

    #[test]
    fn a_held_struct_crosses_a_thread() {
        let keep = Arc::new(Keep::new());
        let text = keep.put_viewed("x y".to_string());
        let held: Holding<RecordViews> = Record {
            name: text,
            parts: text.split(' ').collect(),
        }
        .hold([Arc::clone(&keep)]);
        let got = std::thread::spawn(move || held.get().parts.len())
            .join()
            .unwrap();
        assert_eq!(got, 2);
    }

    /// What the compiler writes: the value is made first, with views of the
    /// buffer borrowed the ordinary way, and carried in afterwards - each view
    /// found again in the keep by address, so nothing is copied.
    #[test]
    fn a_value_made_outside_is_carried_in_without_a_copy() {
        let keep = kept("  oslo 12  ");
        let before = keep.len();
        // The same text in a *different* allocation is not found: the search
        // is by address, not by content.
        let elsewhere = String::from("oslo");
        assert!(keep.find(&elsewhere).is_none());
        assert_eq!(keep.find(""), Some(""));
        let held = Record {
            name: "static",
            parts: vec![],
        }
        .hold([Arc::clone(&keep)]);
        assert_eq!(held.get().name, "static");
        // The literal pointed into no buffer of the keep, so a copy of it
        // went in - the one case that pays.
        assert_eq!(keep.len(), before + 1);
    }

    #[test]
    fn a_view_of_the_keep_is_found_where_it_points() {
        let keep = Arc::new(Keep::new());
        let text = keep.put_viewed("alpha beta".to_string());
        let beta = &text[6..];
        let found = keep.find(beta).expect("a view of the keep is found");
        assert_eq!(found.as_ptr(), beta.as_ptr());
        assert_eq!(found, "beta");
        let held = Record {
            name: beta,
            parts: text.split(' ').collect(),
        }
        .hold([Arc::clone(&keep)]);
        assert_eq!(held.get().name.as_ptr(), beta.as_ptr());
        // Nothing was copied in.
        assert_eq!(keep.len(), 1);
    }

    /// Two buffers, one handle on each: the struct keeps both alive and
    /// neither is copied.
    #[test]
    fn a_struct_of_views_into_two_buffers_holds_both() {
        let mut kept: Vec<Holding<RecordViews, 2>> = Vec::new();
        for i in 0..5 {
            let names = Arc::new(Keep::new());
            let parts = Arc::new(Keep::new());
            let name = names.put_viewed(format!("name{i}"));
            let list = parts.put_viewed(format!("p{i} q{i}"));
            kept.push(
                Record {
                    name,
                    parts: list.split(' ').collect(),
                }
                .hold([Arc::clone(&names), Arc::clone(&parts)]),
            );
            drop((names, parts));
            if kept.len() > 2 {
                kept.remove(0);
            }
        }
        assert_eq!(kept[1].get().name, "name4");
        assert_eq!(kept[1].get().parts, ["p4", "q4"]);
        assert_eq!(kept[1].keeps()[0].len(), 1);
        assert_eq!(kept[1].keeps()[1].len(), 1);
        let copied = kept[0].clone();
        drop(kept);
        assert_eq!(copied.get().parts, ["p3", "q3"]);
    }

    /// A write through the handle: a field of its own is written as it is, a
    /// view made outside is found again in the keeps first.
    #[test]
    fn a_held_struct_is_written_through_its_handle() {
        let keep = Arc::new(Keep::new());
        let text = keep.put_viewed("alpha beta".to_string());
        let mut held = Record {
            name: &text[..5],
            parts: vec![],
        }
        .hold([Arc::clone(&keep)]);
        let beta = &text[6..];
        held.with_mut(|record, keeps| {
            record.name = keeps.view(beta);
            record.parts.push("static");
        });
        drop(keep);
        assert_eq!(held.get().name, "beta");
        assert_eq!(held.get().parts, ["static"]);
        assert_eq!(held.keeps()[0].len(), 1);
    }

    #[test]
    fn a_later_buffer_does_not_move_an_earlier_one() {
        let keep = Keep::new();
        let first: &String = keep.put("first".to_string());
        let view: &str = &first[..3];
        for n in 0..1000 {
            keep.put(format!("buffer {n}"));
        }
        assert_eq!(view, "fir");
        assert_eq!(keep.len(), 1001);
    }

    // `Held` holds a handle on a `Keep`, whose lock is interior mutability -
    // but a key hashes and compares by its text alone, which never changes.
    #[allow(clippy::mutable_key_type)]
    #[test]
    fn a_held_view_outlives_the_scope_that_read_it_and_compares_by_content() {
        let mut seen: std::collections::HashSet<Held> = Default::default();
        for text in ["Hamburg;1", "Hamburg;2"] {
            let keep = Arc::new(Keep::new());
            let buffer = keep.put_viewed(text.to_string());
            let name = buffer.split(';').next().unwrap_or("");
            seen.insert(name.hold([Arc::clone(&keep)]));
        }
        assert_eq!(seen.len(), 1);
        assert!(seen.contains("Hamburg"));
    }
}

/// The README's example, run as a test.
#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDoctests;
