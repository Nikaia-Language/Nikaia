//! What goes between the brackets, converted where it has to be.
//!
//! **A length is an `i64`** ([ADR-048](../../../docs/specification/adr/adr-048.md)
//! D1), so `for i in 0..xs.len()` gives an `i64` and `xs[i]` is an `i64` index.
//! Rust indexes a sequence by `usize`, and this is the conversion - emitted, never
//! written. D1's trade is that the ceremony a user would write cannot fail while
//! the conversion the compiler writes can, in one direction only: a **negative**
//! index.
//!
//! **A negative index reports as an access out of bounds**, which D1 requires: it
//! *is* one, and a diagnostic about a failed conversion would be about something
//! the user did not write. `as usize` would not do - `-3 as usize` is
//! 18446744073709551613, and the message a reader gets is then about a number
//! nothing in their program contains.
//!
//! **Why a trait and not a function.** The emitter puts this around *every* index
//! it writes, because it does not know types (ADR-011 D2) and a map is indexed
//! too: `counts[path]` takes a `&str`. So the conversion has to be chosen by the
//! type of what is in the brackets, and choosing on a type is what a trait is. For
//! anything that is not an integer this is the identity, which is why a uniform
//! rule is safe here - and a uniform rule is the one that cannot be applied to the
//! wrong index.

/// What a value in brackets becomes.
pub trait At {
    type Out;
    fn at(self) -> Self::Out;
}

/// The one message, so a negative index reads the same however it arrived.
///
/// `#[track_caller]` all the way out to [`at`], so the location the panic hook is
/// handed is the **caller's** - the line of the generated file that wrote the
/// index - and not a line of this file. Without it,
/// [ADR-044](../../../docs/specification/adr/adr-044.md) D1's table has nothing
/// to look up and the reader is told about `nikaia_std/src/index.rs`, which is
/// the Part III C.1 defect one crate over.
#[cold]
#[inline(never)]
#[track_caller]
fn out_of_bounds(index: i64) -> ! {
    panic!("index out of bounds: the index is {index}")
}

macro_rules! signed {
    ($($t:ty),*) => {
        $(
            impl At for $t {
                type Out = usize;
                #[track_caller]
                fn at(self) -> usize {
                    match usize::try_from(self) {
                        Ok(index) => index,
                        Err(_) => out_of_bounds(self as i64),
                    }
                }
            }

            impl At for std::ops::Range<$t> {
                type Out = std::ops::Range<usize>;
                #[track_caller]
                fn at(self) -> std::ops::Range<usize> {
                    std::ops::Range { start: self.start.at(), end: self.end.at() }
                }
            }

            impl At for std::ops::RangeInclusive<$t> {
                type Out = std::ops::RangeInclusive<usize>;
                #[track_caller]
                fn at(self) -> std::ops::RangeInclusive<usize> {
                    let (start, end) = self.into_inner();
                    start.at()..=end.at()
                }
            }
        )*
    };
}

signed!(i8, i16, i32, i64, isize);

macro_rules! unsigned {
    ($($t:ty),*) => {
        $(
            impl At for $t {
                type Out = usize;
                fn at(self) -> usize {
                    self as usize
                }
            }

            impl At for std::ops::Range<$t> {
                type Out = std::ops::Range<usize>;
                fn at(self) -> std::ops::Range<usize> {
                    std::ops::Range { start: self.start.at(), end: self.end.at() }
                }
            }
        )*
    };
}

unsigned!(u8, u16, u32, u64, usize);

/// **Anything that is not a number goes through untouched** - a map's key, most
/// of all. `&T` covers `&str`, `&String` and a reference to any key type, which
/// is the shape Rust's `Index` for a map wants anyway.
impl<'a, T: ?Sized> At for &'a T {
    type Out = &'a T;
    fn at(self) -> &'a T {
        self
    }
}

/// The emitted spelling: `xs[nikaia_std::index::at(i)]`.
///
/// `#[track_caller]`, so a negative index is reported at the line that wrote it -
/// see [`out_of_bounds`].
#[track_caller]
pub fn at<I: At>(index: I) -> I::Out {
    index.at()
}

/// **A read through the brackets answers what the container can promise**
/// ([ADR-114](../../../docs/specification/adr/adr-114.md) D4).
///
/// One trait with an **output type per container**, which is the same
/// arrangement [`Set`] has and for the same reason: this emitter does not know
/// types ([ADR-011](../../../docs/specification/adr/adr-011.md) D2), so it
/// writes the same three tokens for a map and for a sequence and the language
/// below picks.
///
/// And what each can promise is different. A **sequence** has a `T` at every
/// index it has at all, and an index it does not have is the program's own
/// arithmetic gone wrong — [Part III A.2](../../../docs/specification/30-nikaia-tooling.md)'s
/// abort, kept (D3). A **map** has a `V` only where the key is, so what it
/// answers is a `T?`: *there is nothing there* is data about the world, not a
/// bug in the program, and a program that knows better says so with `??` (D1).
pub trait Get<K> {
    type Out<'a>
    where
        Self: 'a;

    fn get(&self, key: K) -> Self::Out<'_>;
}

/// **A sequence is indexed by a number or sliced by a range**, and one impl
/// covers both: `SliceIndex` already tells them apart in the language below —
/// its `Output` is the element for a number and a run of them for a range —
/// which is what keeps `xs[0]` and `xs[a..<b]` one rule here as they are one
/// rule in the source.
impl<V, I> Get<I> for Vec<V>
where
    // **`'static` on the index**, which costs nothing and is what lets the
    // output mention `I::Output` without the index's own lifetime leaking into
    // the trait: a sequence is read at a number or sliced at a range of them,
    // and both of those own everything they are.
    I: std::slice::SliceIndex<[V]> + 'static,
{
    type Out<'a>
        = &'a I::Output
    where
        Self: 'a;

    /// `#[track_caller]`, so an index past the end is reported at the line that
    /// wrote it rather than inside this file.
    #[track_caller]
    fn get(&self, key: I) -> &I::Output {
        &self[key]
    }
}

impl<V, I, const N: usize> Get<I> for [V; N]
where
    // **`'static` on the index**, which costs nothing and is what lets the
    // output mention `I::Output` without the index's own lifetime leaking into
    // the trait: a sequence is read at a number or sliced at a range of them,
    // and both of those own everything they are.
    I: std::slice::SliceIndex<[V]> + 'static,
{
    type Out<'a>
        = &'a I::Output
    where
        Self: 'a;

    #[track_caller]
    fn get(&self, key: I) -> &I::Output {
        &self[key]
    }
}

/// **A run of elements, which is what a `&[T]` is**
/// ([ADR-179](../../../docs/specification/adr/adr-179.md) D1).
///
/// The same body the two above have, and it has to be written a third time
/// because `[V]` is neither a `Vec<V>` nor a `[V; N]` to the language below —
/// a blanket impl over `SliceIndex` would overlap with the map's. What a
/// `const` holds is this one: `const ROWS: &[Row] = &[…];` reaches `Get`
/// through the `&'b T` impl further down, whose `T` is `[Row]`.
impl<V, I> Get<I> for [V]
where
    I: std::slice::SliceIndex<[V]> + 'static,
{
    type Out<'a>
        = &'a I::Output
    where
        Self: 'a;

    #[track_caller]
    fn get(&self, key: I) -> &I::Output {
        &self[key]
    }
}

/// **Text is sliced by a range** and never indexed by a number: a byte of
/// UTF-8 is not a character, which is why Part I 2.2 has no such read.
impl<I> Get<I> for str
where
    I: std::slice::SliceIndex<str> + 'static,
{
    type Out<'a>
        = &'a I::Output
    where
        Self: 'a;

    #[track_caller]
    fn get(&self, key: I) -> &I::Output {
        &self[key]
    }
}

impl<I> Get<I> for String
where
    I: std::slice::SliceIndex<str> + 'static,
{
    type Out<'a>
        = &'a I::Output
    where
        Self: 'a;

    #[track_caller]
    fn get(&self, key: I) -> &I::Output {
        &self.as_str()[key]
    }
}

/// **A view of a container reads like the container**, because this emitter
/// writes `get(&x, …)` whether `x` is owned or already a view
/// ([ADR-011](../../../docs/specification/adr/adr-011.md) D2) — so a `&&str`
/// arrives here and has to answer what a `&str` would.
///
/// **The answer is tied to the inner reference and not to this borrow**, which
/// is the whole of what this impl has to get right: `&dna[a..<b]` on a
/// `dna: &str` parameter is a view of what `dna` points at, and answering with
/// the lifetime of the `&dna` the emitter wrote would make it a view of a local
/// — *cannot return value referencing function parameter*, about a file nobody
/// wrote ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
impl<'b, K, T> Get<K> for &'b T
where
    T: Get<K> + ?Sized,
{
    type Out<'a>
        = T::Out<'b>
    where
        Self: 'a;

    #[track_caller]
    fn get(&self, key: K) -> Self::Out<'_> {
        let inner: &'b T = self;
        inner.get(key)
    }
}

/// What a **map** read answers, before the `*` the emitter writes around every
/// read takes it apart ([ADR-161](../../../docs/specification/adr/adr-161.md)
/// D6).
///
/// A sequence read has to be a **value** — `return xs[at]` on a `-> i64` is the
/// shape that says so — and a map read has to be an **option**. One trait
/// cannot answer both unless the caller writes the same thing for each, so the
/// caller writes `*`: for a sequence that is the element in its place, and for
/// a map it is this, whose `Deref` hands back the option.
pub struct Found<'a, V>(Option<&'a V>);

impl<'a, V> std::ops::Deref for Found<'a, V> {
    type Target = Option<&'a V>;

    fn deref(&self) -> &Option<&'a V> {
        &self.0
    }
}

impl<K, Q, V, S> Get<&Q> for std::collections::HashMap<K, V, S>
where
    K: std::cmp::Eq + std::hash::Hash + std::borrow::Borrow<Q>,
    Q: std::cmp::Eq + std::hash::Hash + ?Sized,
    S: std::hash::BuildHasher,
{
    type Out<'a>
        = Found<'a, V>
    where
        Self: 'a;

    fn get(&self, key: &Q) -> Found<'_, V> {
        Found(std::collections::HashMap::get(self, key))
    }
}

impl<K, Q, V> Get<&Q> for std::collections::BTreeMap<K, V>
where
    K: Ord + std::borrow::Borrow<Q>,
    Q: Ord + ?Sized,
{
    type Out<'a>
        = Found<'a, V>
    where
        Self: 'a;

    fn get(&self, key: &Q) -> Found<'_, V> {
        Found(std::collections::BTreeMap::get(self, key))
    }
}

/// **A key read out of a map whose key type the checker could not pin**
/// (0.0.235): what `index::key(k)` wraps.
///
/// A map's read wants `&Q` where the map's key borrows as `Q`, and the key in
/// the brackets may be a number (lent here) or already a view (passed
/// through) - which one, the emitter cannot say, because nothing in the source
/// pinned the map's key type. [`AsKey`] answers it by the key's own type.
pub struct Key<T>(T);

/// Wrap a key for [`Key`]'s read.
pub fn key<T: AsKey>(key: T) -> Key<T> {
    Key(key)
}

/// What a key is looked up as.
pub trait AsKey {
    type Q: ?Sized;
    fn as_key(&self) -> &Self::Q;
}

macro_rules! keyed_by_value {
    ($($t:ty),*) => {
        $(
            impl AsKey for $t {
                type Q = $t;
                fn as_key(&self) -> &$t {
                    self
                }
            }
        )*
    };
}

keyed_by_value!(
    i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, char, bool, String
);

impl<T: ?Sized> AsKey for &T {
    type Q = T;
    fn as_key(&self) -> &T {
        self
    }
}

impl<K, T, V, S> Get<Key<T>> for std::collections::HashMap<K, V, S>
where
    T: AsKey,
    K: std::cmp::Eq + std::hash::Hash + std::borrow::Borrow<T::Q>,
    T::Q: std::cmp::Eq + std::hash::Hash,
    S: std::hash::BuildHasher,
{
    type Out<'a>
        = Found<'a, V>
    where
        Self: 'a;

    fn get(&self, key: Key<T>) -> Found<'_, V> {
        Found(std::collections::HashMap::get(self, key.0.as_key()))
    }
}

impl<K, T, V> Get<Key<T>> for std::collections::BTreeMap<K, V>
where
    T: AsKey,
    K: Ord + std::borrow::Borrow<T::Q>,
    T::Q: Ord,
{
    type Out<'a>
        = Found<'a, V>
    where
        Self: 'a;

    fn get(&self, key: Key<T>) -> Found<'_, V> {
        Found(std::collections::BTreeMap::get(self, key.0.as_key()))
    }
}

/// The emitted spelling: `nikaia_std::index::get(&m, nikaia_std::index::at(k))`.
#[track_caller]
pub fn get<T, K>(target: &T, key: K) -> T::Out<'_>
where
    T: Get<K> + ?Sized,
{
    target.get(key)
}

/// What `??` does, once the left of it may be a **view into a container**
/// ([ADR-114](../../../docs/specification/adr/adr-114.md) D4).
///
/// A map read answers `Option<&V>`, because the value reached is the map's and
/// copying it is never something a compiler does on its own
/// ([ADR-008](../../../docs/specification/adr/adr-008.md) D5). But the fallback
/// is written as the value it stands for — `m[k] ?? 0` — so the two sides do
/// not have the same type, and `unwrap_or_else` cannot join them.
///
/// **Two impls, and the language below picks**, which is the same arrangement
/// [`Get`] has. They do not overlap: one is `Or<T> for Option<T>` and the other
/// `Or<T> for Option<&T>`, and a `T` is never a `&T`.
///
/// `T: Copy` on the second and not `Clone`: a copy of a number is what
/// `m[k] ?? 0` means, and a **clone** of a `String` is an allocation the
/// program did not write ([ADR-008](../../../docs/specification/adr/adr-008.md)
/// D5). Where the value does not copy, the fallback has to be a view too — or
/// the program says `.to_owned()`, which is the same sentence that section has
/// everywhere else.
pub trait Or<T> {
    fn or(self, fallback: impl FnOnce() -> T) -> T;
}

impl<T> Or<T> for Option<T> {
    fn or(self, fallback: impl FnOnce() -> T) -> T {
        self.unwrap_or_else(fallback)
    }
}

impl<T: Copy> Or<T> for Option<&T> {
    fn or(self, fallback: impl FnOnce() -> T) -> T {
        self.copied().unwrap_or_else(fallback)
    }
}

/// **A view of an optional copy value** is read as the copy, as a view of a
/// copy value is: `for x in xs { x ?? -1 }` over a `Vec[i64?]` binds each `x`
/// as `&Option<i64>`, which no `Or` took - `rustc` refused the file (#312).
impl<T: Copy> Or<T> for &Option<T> {
    fn or(self, fallback: impl FnOnce() -> T) -> T {
        (*self).unwrap_or_else(fallback)
    }
}

/// **A map's text, read, with a literal after `??`**
/// ([ADR-213](../../../docs/specification/adr/adr-213.md) D2): the map hands
/// out its `String` by reference and the literal is a `&str`, so what the two
/// can both be is a view of text - with nothing copied and nothing allocated.
impl<'a> Or<&'a str> for Option<&'a String> {
    fn or(self, fallback: impl FnOnce() -> &'a str) -> &'a str {
        self.map(String::as_str).unwrap_or_else(fallback)
    }
}

/// The emitted spelling: `nikaia_std::index::or(value, || fallback)`.
///
/// A **free function** and not a method, because `Option` has an inherent `or`
/// of its own and an inherent method wins over a trait's.
pub fn or<S, T>(value: S, fallback: impl FnOnce() -> T) -> T
where
    S: Or<T>,
{
    value.or(fallback)
}

/// **A write through the brackets is not an index**, and that is what this
/// exists to say ([ADR-080](../../../docs/specification/adr/adr-080.md) D2).
///
/// `scores["Player1"] = 100` on a map used to lower to an indexed assignment,
/// and Rust's `Index` for a map is over anything the key **borrows** as — so
/// indexing a `HashMap<K, V>` with a `&str` leaves `K` unpinned and the program
/// failed with
///
/// ```text
/// error[E0282]: type annotations needed for
///               `HashMap<_, i32, BuildHasherDefault<FxHasher>>`
/// ```
///
/// naming `TrustedMap`, `BuildHasherDefault<FxHasher>` and a type parameter `K`:
/// three spellings the program never wrote, which is Part III C.1's class at its
/// worst. `insert` takes `K` by value and pins it exactly, and a sequence's
/// write is still an indexed assignment — so the two are one trait for the same
/// reason `At` above is one: **this emitter does not know types** (ADR-011 D2),
/// and a rule applied everywhere cannot be applied to the wrong container.
pub trait Set<K, V> {
    fn set(&mut self, key: K, value: V);
}

impl<V> Set<usize, V> for Vec<V> {
    #[track_caller]
    fn set(&mut self, key: usize, value: V) {
        self[key] = value;
    }
}

impl<K, V, S> Set<K, V> for std::collections::HashMap<K, V, S>
where
    K: std::cmp::Eq + std::hash::Hash,
    S: std::hash::BuildHasher,
{
    fn set(&mut self, key: K, value: V) {
        self.insert(key, value);
    }
}

impl<K, V> Set<K, V> for std::collections::BTreeMap<K, V>
where
    K: Ord,
{
    fn set(&mut self, key: K, value: V) {
        self.insert(key, value);
    }
}

/// The emitted spelling: `nikaia_std::index::set(&mut m, nikaia_std::index::at(k), v)`.
///
/// `#[track_caller]`, so a write past the end of a sequence is reported at the
/// line that wrote it, the same as a read.
#[track_caller]
pub fn set<T, K, V>(target: &mut T, key: K, value: V)
where
    T: Set<K, V> + ?Sized,
{
    target.set(key, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_becomes_a_usize() {
        assert_eq!(at(3_i64), 3_usize);
        assert_eq!(at(0_i32), 0_usize);
        let xs = [10, 20, 30];
        assert_eq!(xs[at(2_i64)], 30);
    }

    /// D1's one requirement: the message is about the **index**, not about a
    /// conversion the user never wrote.
    #[test]
    fn a_negative_index_reports_as_an_access_out_of_bounds() {
        let failed = std::panic::catch_unwind(|| at(-1_i64)).expect_err("it aborts");
        let said = failed
            .downcast_ref::<String>()
            .expect("the message is a String");
        assert!(
            said.contains("index out of bounds") && said.contains("-1"),
            "the message must name the index: {said}"
        );
        // And not the wrapped number `as usize` would have produced.
        assert!(!said.contains("18446744073709551615"), "{said}");
    }

    /// A key is not a number, and a uniform rule at every index is only safe
    /// because of this.
    #[test]
    fn a_key_goes_through_untouched() {
        use std::collections::HashMap;
        let mut counts: HashMap<&str, i64> = HashMap::new();
        counts.insert("host", 7);
        assert_eq!(counts[at("host")], 7);
    }

    #[test]
    fn a_slice_range_is_converted_at_both_ends() {
        let text = "abcdef";
        assert_eq!(&text[at(1_i64..3_i64)], "bc");
        assert_eq!(&text[at(1_i64..=3_i64)], "bcd");
    }
}
