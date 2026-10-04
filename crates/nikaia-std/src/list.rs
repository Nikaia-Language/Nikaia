//! What a Nikaia `List` can do that a Rust `Vec` spells differently.

/// `xs.map fn { … }` (Kap 5.3) is a method on the list itself, not a step in an
/// iterator pipeline, so it is one here too.
pub trait ListExt<T> {
    /// Every element, transformed. Takes the list, because a mapped list
    /// replaces the one it was built from.
    fn map<U>(self, f: impl FnMut(T) -> U) -> Vec<U>;
}

impl<T> ListExt<T> for Vec<T> {
    fn map<U>(self, f: impl FnMut(T) -> U) -> Vec<U> {
        self.into_iter().map(f).collect()
    }
}

/// **Whether a list of text holds a text**: `xs.contains(name)` for a
/// `Vec[String]` and a `name: ref String`, which is a `&str` here, where the
/// list's own `contains` takes a `&String` - and for a `comptime NAMES:
/// Array[ref String, N]`, a `[&str; N]` here, and a `name: String`, where the
/// array's takes a `&&str` (#375). Compared as text, element by element, with
/// nothing copied to ask: the value is any of `&str`, `&String` and `&&str`,
/// each of which is a `&str` by the coercion a call makes.
pub fn contains<T: AsRef<str>>(items: &[T], value: &str) -> bool {
    items.iter().any(|item| item.as_ref() == value)
}

/// `text.chars().collect()` into a `Vec[char]`, with the room for every
/// character asked for once: a text's length in bytes is at least its count of
/// characters. `collect` over `Chars` asks for a quarter of that and grows,
/// which was the largest single cost of reading a ledger in Nikaia (ADR-294
/// step (c)).
pub fn chars(text: std::str::Chars<'_>) -> Vec<char> {
    let mut all = Vec::with_capacity(text.as_str().len());
    all.extend(text);
    all
}
