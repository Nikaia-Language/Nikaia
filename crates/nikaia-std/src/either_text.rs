//! **Text that is a view or text of its own, per value**
//! ([ADR-282](../../../docs/specification/adr/adr-282.md) D14).
//!
//! What a `String` field or result is below when both kinds of text flow into
//! it: a view is borrowed where it is put in, and text of its own is owned
//! where it is put in, so neither line pays for the other's kind. Read, it is
//! text either way - it derefs to `str`, prints, compares and hashes as its
//! text - and `to_owned` hands back a `String`, which is what a program's
//! `.clone()` of text asks for.

use std::borrow::Cow;

/// A view of text, or text of its own.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EitherText<'a>(Cow<'a, str>);

impl<'a> EitherText<'a> {
    /// The text, as a view of it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A copy: text of its own, whichever this is.
    #[allow(
        clippy::inherent_to_string_shadow_display,
        clippy::wrong_self_convention
    )]
    pub fn to_owned(&self) -> String {
        String::from(&*self.0)
    }

    /// The same, under the name a reader from another language types.
    #[allow(clippy::inherent_to_string_shadow_display)]
    pub fn to_string(&self) -> String {
        self.to_owned()
    }

    /// Whether this is a view - for `--tethers` and for tests.
    pub fn is_view(&self) -> bool {
        matches!(self.0, Cow::Borrowed(_))
    }
}

impl std::ops::Deref for EitherText<'_> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for EitherText<'_> {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for EitherText<'_> {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl<'a> From<&'a str> for EitherText<'a> {
    fn from(view: &'a str) -> Self {
        EitherText(Cow::Borrowed(view))
    }
}

impl<'a> From<&'a String> for EitherText<'a> {
    fn from(view: &'a String) -> Self {
        EitherText(Cow::Borrowed(view))
    }
}

impl From<String> for EitherText<'_> {
    fn from(own: String) -> Self {
        EitherText(Cow::Owned(own))
    }
}

impl<'a> From<&'a EitherText<'_>> for EitherText<'a> {
    fn from(other: &'a EitherText<'_>) -> Self {
        EitherText(Cow::Borrowed(other))
    }
}

/// **What goes into a position both kinds of text flow into**
/// ([ADR-282](../../../docs/specification/adr/adr-282.md) D5): a view is
/// borrowed and text of its own moved in, and a value that may be absent stays
/// one - `Option<&str>` becomes `Option<EitherText>` - so one call is right for
/// a `String` position and for a `String?` one alike. The nullable wrap the
/// emitter writes around a value (`Some(…)`, `.into()`) goes around this.
pub trait IntoEither {
    /// `EitherText`, or `Option` of it.
    type Either;
    /// The value as it goes in: borrowed, moved, or absent.
    fn into_either(self) -> Self::Either;
}

impl<'a> IntoEither for &'a str {
    type Either = EitherText<'a>;
    fn into_either(self) -> EitherText<'a> {
        EitherText::from(self)
    }
}

impl<'b> IntoEither for &&'b str {
    type Either = EitherText<'b>;
    fn into_either(self) -> EitherText<'b> {
        EitherText::from(*self)
    }
}

impl<'a> IntoEither for &'a String {
    type Either = EitherText<'a>;
    fn into_either(self) -> EitherText<'a> {
        EitherText::from(self)
    }
}

impl IntoEither for String {
    type Either = EitherText<'static>;
    fn into_either(self) -> EitherText<'static> {
        EitherText::from(self)
    }
}

impl<'a> IntoEither for EitherText<'a> {
    type Either = EitherText<'a>;
    fn into_either(self) -> EitherText<'a> {
        self
    }
}

impl<'a> IntoEither for &'a EitherText<'_> {
    type Either = EitherText<'a>;
    fn into_either(self) -> EitherText<'a> {
        EitherText::from(self)
    }
}

impl<T: IntoEither> IntoEither for Option<T> {
    type Either = Option<T::Either>;
    fn into_either(self) -> Option<T::Either> {
        self.map(IntoEither::into_either)
    }
}

/// **The same into a `String?`**: present or absent whichever the value was,
/// so `"x"` goes in as `Some` and `lines().next()` as it is.
pub trait IntoEitherMaybe {
    /// `EitherText`.
    type Either;
    /// The value as it goes in, and whether it is there.
    fn into_either_maybe(self) -> Option<Self::Either>;
}

macro_rules! present {
    ($($t:ty),*) => {$(
        impl<'a> IntoEitherMaybe for $t {
            type Either = <$t as IntoEither>::Either;
            fn into_either_maybe(self) -> Option<Self::Either> {
                Some(self.into_either())
            }
        }
    )*};
}

present!(&'a str, &'a String, String, EitherText<'a>);

impl<'a, 'b> IntoEitherMaybe for &'a EitherText<'b> {
    type Either = EitherText<'a>;
    fn into_either_maybe(self) -> Option<EitherText<'a>> {
        Some(self.into_either())
    }
}

impl<'b> IntoEitherMaybe for &&'b str {
    type Either = EitherText<'b>;
    fn into_either_maybe(self) -> Option<EitherText<'b>> {
        Some(self.into_either())
    }
}

impl<T: IntoEither> IntoEitherMaybe for Option<T> {
    type Either = T::Either;
    fn into_either_maybe(self) -> Option<T::Either> {
        self.map(IntoEither::into_either)
    }
}

/// **Each item of an iterator handed over as it is**, for a `collect` into a
/// list whose element is text of both kinds
/// ([ADR-282](../../../docs/specification/adr/adr-282.md) D16): the list is
/// built once, with each item borrowed or moved in where it is.
pub trait EitherItems: Iterator + Sized
where
    Self::Item: IntoEither,
{
    /// The same iterator, each item as it goes in.
    fn either_items(self) -> std::iter::Map<Self, HandOver<Self::Item>> {
        self.map(IntoEither::into_either)
    }
}

/// **The same for pairs going into a map** ([ADR-293](../../../docs/specification/adr/adr-293.md)
/// D2): the key, the value or both handed over as either kind, whichever of the
/// map's two is text both kinds flow into.
#[allow(clippy::type_complexity)]
pub trait EitherPairs<K, V>: Iterator<Item = (K, V)> + Sized {
    /// Each key as it goes in.
    fn either_keys(self) -> std::iter::Map<Self, fn((K, V)) -> (K::Either, V)>
    where
        K: IntoEither,
    {
        self.map(|(k, v)| (k.into_either(), v))
    }

    /// Each value as it goes in.
    fn either_values(self) -> std::iter::Map<Self, fn((K, V)) -> (K, V::Either)>
    where
        V: IntoEither,
    {
        self.map(|(k, v)| (k, v.into_either()))
    }

    /// Both, as they go in.
    fn either_pairs(self) -> std::iter::Map<Self, fn((K, V)) -> (K::Either, V::Either)>
    where
        K: IntoEither,
        V: IntoEither,
    {
        self.map(|(k, v)| (k.into_either(), v.into_either()))
    }
}

impl<K, V, I: Iterator<Item = (K, V)>> EitherPairs<K, V> for I {}

/// What [`EitherItems`] does to each item.
pub type HandOver<T> = fn(T) -> <T as IntoEither>::Either;

impl<I: Iterator> EitherItems for I where I::Item: IntoEither {}

/// [`IntoEither`] as a function, for the emitter to write around a value.
pub fn either<T: IntoEither>(value: T) -> T::Either {
    value.into_either()
}

impl PartialEq<str> for EitherText<'_> {
    fn eq(&self, other: &str) -> bool {
        *self.0 == *other
    }
}

impl PartialEq<&str> for EitherText<'_> {
    fn eq(&self, other: &&str) -> bool {
        *self.0 == **other
    }
}

impl PartialEq<String> for EitherText<'_> {
    fn eq(&self, other: &String) -> bool {
        *self.0 == **other
    }
}

impl PartialEq<EitherText<'_>> for &str {
    fn eq(&self, other: &EitherText<'_>) -> bool {
        **self == *other.0
    }
}

impl std::fmt::Display for EitherText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&*self.0, f)
    }
}

impl std::fmt::Debug for EitherText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&*self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_is_borrowed_and_text_of_its_own_is_owned() {
        let buffer = String::from("  oslo  ");
        let view: EitherText<'_> = buffer.trim().into();
        let own: EitherText<'_> = format!("{}!", "bergen").into();
        assert!(view.is_view());
        assert!(!own.is_view());
        assert_eq!(view, "oslo");
        assert_eq!(own.len(), 7);
        let copy: String = view.to_owned();
        assert_eq!(copy, "oslo");
        assert_eq!(format!("{view} {own}"), "oslo bergen!");
    }

    #[test]
    fn a_value_that_may_be_absent_goes_in_as_one() {
        let buffer = String::from("oslo\nbergen");
        let first: Option<EitherText<'_>> = buffer.lines().next().into_either();
        let none: Option<EitherText<'_>> = None::<String>.into_either();
        let own: Option<EitherText<'_>> = Some(either(String::from("x")));
        assert!(first.as_ref().is_some_and(EitherText::is_view));
        assert_eq!(none, None);
        assert!(own.is_some_and(|o| !o.is_view()));
        let some: Option<EitherText<'_>> = "y".into_either_maybe();
        let absent: Option<EitherText<'_>> = None::<&str>.into_either_maybe();
        assert_eq!(some.as_deref(), Some("y"));
        assert_eq!(absent, None);
        let lines: Vec<EitherText<'_>> = buffer.lines().either_items().collect();
        let counted: std::collections::HashMap<EitherText<'_>, i64> =
            buffer.lines().map(|l| (l, 1)).either_keys().collect();
        assert!(counted.keys().all(EitherText::is_view));
        assert!(lines.iter().all(EitherText::is_view));
    }
}
