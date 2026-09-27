//! **What a lambda that pauses is handed to**
//! ([ADR-233](../../../docs/specification/adr/adr-233.md)).
//!
//! `std` takes a lambda in a handful of places, and the language below takes a
//! **synchronous** closure in every one of them: `Iterator::map`, `sort_by_key`,
//! `Entry::or_insert_with`. A lambda whose body pauses has no shape there, so
//! each gets its counterpart here, over the language below's own async closures
//! (`AsyncFnMut`):
//!
//! * [`then`] and [`then_filter`] make a **sequence whose step pauses**,
//!   [`Paused`], out of a plain one. It is walked the way `io::lines()` is
//!   ([ADR-172](../../../docs/specification/adr/adr-172.md) D1): a `for` over
//!   it gives its thread up at each step, and `collect`, `count`, `nth` and
//!   `join` are loops around its step. A `map`, `filter`, `take`, `skip`,
//!   `step_by` or `zip` on it makes another.
//! * [`map_list`], [`sort_by_key`], [`or_insert_with`] and [`and_modify`] are
//!   the eager ones: each runs the lambda, awaiting it, where the synchronous
//!   entry would have called it.
//!
//! Nothing here allocates that the synchronous entry would not, except
//! [`sort_by_key`], which works each key out once rather than on every
//! comparison - a pausing key could not be asked for again and again.

/// **`Seq::join` over a sequence that does not pause**: every item written one
/// after another with this text between them.
///
/// The language below's own `join` is a slice's, so a `map` or a `chars()`
/// had none and the call was `rustc`'s error about a file nobody wrote
/// ([ADR-235](../../../docs/specification/adr/adr-235.md) §3). In the prelude
/// under no name, so it is in scope and names nothing.
pub trait Join {
    /// The items, written with `separator` between them.
    fn join(self, separator: &str) -> String;
}

impl<I: Iterator> Join for I
where
    I::Item: std::fmt::Display,
{
    fn join(self, separator: &str) -> String {
        let mut out = String::new();
        for (at, item) in self.enumerate() {
            if at > 0 {
                out.push_str(separator);
            }
            out.push_str(&item.to_string());
        }
        out
    }
}

/// One step of a sequence that may pause.
pub trait Step {
    /// What each step produces.
    type Item;
    /// The next item, or nothing when the sequence is over.
    fn step(&mut self) -> impl Future<Output = Option<Self::Item>>;
}

/// A plain sequence, stepped as one that may pause.
pub struct Ready<I>(I);

impl<I: Iterator> Step for Ready<I> {
    type Item = I::Item;
    async fn step(&mut self) -> Option<I::Item> {
        self.0.next()
    }
}

/// Each item through a lambda that pauses.
pub struct Then<S, F> {
    inner: S,
    f: F,
}

impl<S: Step, U, F: AsyncFnMut(S::Item) -> U> Step for Then<S, F> {
    type Item = U;
    async fn step(&mut self) -> Option<U> {
        let item = self.inner.step().await?;
        Some((self.f)(item).await)
    }
}

/// Only the items a lambda that pauses says yes to.
pub struct ThenFilter<S, F> {
    inner: S,
    f: F,
}

impl<S: Step, F: AsyncFnMut(&S::Item) -> bool> Step for ThenFilter<S, F> {
    type Item = S::Item;
    async fn step(&mut self) -> Option<S::Item> {
        loop {
            let item = self.inner.step().await?;
            if (self.f)(&item).await {
                return Some(item);
            }
        }
    }
}

/// Each item through a plain lambda, after a step that pauses.
pub struct Map<S, F> {
    inner: S,
    f: F,
}

impl<S: Step, U, F: FnMut(S::Item) -> U> Step for Map<S, F> {
    type Item = U;
    async fn step(&mut self) -> Option<U> {
        let item = self.inner.step().await?;
        Some((self.f)(item))
    }
}

/// Only the items a plain lambda says yes to, after a step that pauses.
pub struct Filter<S, F> {
    inner: S,
    f: F,
}

impl<S: Step, F: FnMut(&S::Item) -> bool> Step for Filter<S, F> {
    type Item = S::Item;
    async fn step(&mut self) -> Option<S::Item> {
        loop {
            let item = self.inner.step().await?;
            if (self.f)(&item) {
                return Some(item);
            }
        }
    }
}

/// The first `n` items.
pub struct Take<S> {
    inner: S,
    left: usize,
}

impl<S: Step> Step for Take<S> {
    type Item = S::Item;
    async fn step(&mut self) -> Option<S::Item> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        self.inner.step().await
    }
}

/// Everything after the first `n` items.
pub struct Skip<S> {
    inner: S,
    skip: usize,
}

impl<S: Step> Step for Skip<S> {
    type Item = S::Item;
    async fn step(&mut self) -> Option<S::Item> {
        while self.skip > 0 {
            self.skip -= 1;
            self.inner.step().await?;
        }
        self.inner.step().await
    }
}

/// The first item, and then every `step`-th after it.
pub struct StepBy<S> {
    inner: S,
    step: usize,
    first: bool,
}

impl<S: Step> Step for StepBy<S> {
    type Item = S::Item;
    async fn step(&mut self) -> Option<S::Item> {
        if !self.first {
            for _ in 1..self.step {
                self.inner.step().await?;
            }
        }
        self.first = false;
        self.inner.step().await
    }
}

/// Pairs, one item of each, until the shorter ends.
pub struct Zip<S, J> {
    inner: S,
    other: J,
}

impl<S: Step, J: Iterator> Step for Zip<S, J> {
    type Item = (S::Item, J::Item);
    async fn step(&mut self) -> Option<Self::Item> {
        let item = self.inner.step().await?;
        Some((item, self.other.next()?))
    }
}

/// **A sequence whose step pauses** (ADR-233 D1), walked as `io::lines()` is.
pub struct Paused<S>(S);

impl<S: Step> Paused<S> {
    /// The next item, pausing while the step does - the shape a `for` over a
    /// pausing sequence lowers to.
    pub async fn next(&mut self) -> Option<S::Item> {
        self.0.step().await
    }

    /// Everything the sequence produces, as a list.
    pub async fn collect(mut self) -> Vec<S::Item> {
        let mut out = Vec::new();
        while let Some(item) = self.next().await {
            out.push(item);
        }
        out
    }

    /// How many items the sequence produces.
    pub async fn count(mut self) -> i64 {
        let mut n = 0_i64;
        while self.next().await.is_some() {
            n += 1;
        }
        n
    }

    /// The item at a position, or nothing where the sequence is shorter.
    pub async fn nth(mut self, at: i64) -> Option<S::Item> {
        if at < 0 {
            return None;
        }
        let mut seen = 0_i64;
        while let Some(item) = self.next().await {
            if seen == at {
                return Some(item);
            }
            seen += 1;
        }
        None
    }

    /// Every item written one after another with this text between them.
    pub async fn join(mut self, separator: &str) -> String
    where
        S::Item: std::fmt::Display,
    {
        let mut out = String::new();
        let mut first = true;
        while let Some(item) = self.next().await {
            if !first {
                out.push_str(separator);
            }
            out.push_str(&item.to_string());
            first = false;
        }
        out
    }

    /// Each item through a plain lambda; the step still pauses.
    pub fn map<U, F: FnMut(S::Item) -> U>(self, f: F) -> Paused<Map<S, F>> {
        Paused(Map { inner: self.0, f })
    }

    /// Only the items a plain lambda says yes to; the step still pauses.
    pub fn filter<F: FnMut(&S::Item) -> bool>(self, f: F) -> Paused<Filter<S, F>> {
        Paused(Filter { inner: self.0, f })
    }

    /// The first `n` items, or all of them where there are fewer.
    pub fn take(self, n: usize) -> Paused<Take<S>> {
        Paused(Take {
            inner: self.0,
            left: n,
        })
    }

    /// Everything after the first `n` items.
    pub fn skip(self, n: usize) -> Paused<Skip<S>> {
        Paused(Skip {
            inner: self.0,
            skip: n,
        })
    }

    /// The first item, and then every `step`-th after it. A step of `0`
    /// aborts, as the language below's does, since it would never move.
    pub fn step_by(self, step: usize) -> Paused<StepBy<S>> {
        assert!(step != 0, "step_by: a step of 0 would never move");
        Paused(StepBy {
            inner: self.0,
            step,
            first: true,
        })
    }

    /// Pairs, one item of this and one of `other`, until the shorter ends.
    pub fn zip<J: IntoIterator>(self, other: J) -> Paused<Zip<S, J::IntoIter>> {
        Paused(Zip {
            inner: self.0,
            other: other.into_iter(),
        })
    }

    /// Each item through a lambda that pauses.
    pub fn then<U, F: AsyncFnMut(S::Item) -> U>(self, f: F) -> Paused<Then<S, F>> {
        Paused(Then { inner: self.0, f })
    }

    /// Only the items a lambda that pauses says yes to.
    pub fn then_filter<F: AsyncFnMut(&S::Item) -> bool>(self, f: F) -> Paused<ThenFilter<S, F>> {
        Paused(ThenFilter { inner: self.0, f })
    }
}

/// Each item that is there through a plain lambda; a failure passes through.
pub struct OkMap<S, F> {
    inner: S,
    f: F,
}

impl<S: Step<Item = Result<T, E>>, T, E, U, F: FnMut(T) -> U> Step for OkMap<S, F> {
    type Item = Result<U, E>;
    async fn step(&mut self) -> Option<Result<U, E>> {
        Some(self.inner.step().await?.map(&mut self.f))
    }
}

/// Only the items a plain lambda says yes to; a failure passes through.
pub struct OkFilter<S, F> {
    inner: S,
    f: F,
}

impl<S: Step<Item = Result<T, E>>, T, E, F: FnMut(&T) -> bool> Step for OkFilter<S, F> {
    type Item = Result<T, E>;
    async fn step(&mut self) -> Option<Result<T, E>> {
        loop {
            match self.inner.step().await? {
                Ok(item) if !(self.f)(&item) => continue,
                other => return Some(other),
            }
        }
    }
}

/// Each item that is there through a lambda that pauses; a failure passes
/// through.
pub struct OkThen<S, F> {
    inner: S,
    f: F,
}

impl<S: Step<Item = Result<T, E>>, T, E, U, F: AsyncFnMut(T) -> U> Step for OkThen<S, F> {
    type Item = Result<U, E>;
    async fn step(&mut self) -> Option<Result<U, E>> {
        match self.inner.step().await? {
            Ok(item) => Some(Ok((self.f)(item).await)),
            Err(e) => Some(Err(e)),
        }
    }
}

/// Only the items a lambda that pauses says yes to; a failure passes through.
pub struct OkThenFilter<S, F> {
    inner: S,
    f: F,
}

impl<S: Step<Item = Result<T, E>>, T, E, F: AsyncFnMut(&T) -> bool> Step for OkThenFilter<S, F> {
    type Item = Result<T, E>;
    async fn step(&mut self) -> Option<Result<T, E>> {
        loop {
            match self.inner.step().await? {
                Ok(item) if !(self.f)(&item).await => continue,
                other => return Some(other),
            }
        }
    }
}

/// Pairs, one item of each, until the shorter ends; a failure passes through
/// and takes nothing from `other`.
pub struct OkZip<S, J> {
    inner: S,
    other: J,
}

impl<S: Step<Item = Result<T, E>>, T, E, J: Iterator> Step for OkZip<S, J> {
    type Item = Result<(T, J::Item), E>;
    async fn step(&mut self) -> Option<Self::Item> {
        match self.inner.step().await? {
            Ok(item) => Some(Ok((item, self.other.next()?))),
            Err(e) => Some(Err(e)),
        }
    }
}

/// **A sequence whose step pauses and can fail**
/// ([ADR-234](../../../docs/specification/adr/adr-234.md) D1): what
/// `io::lines()` is, and what a lazy walk of it makes. Each step hands back the
/// item or the failure; a lambda handed to a walk sees the item, and the
/// failure goes on to whatever walks the result - the eager walks here end at
/// the first one and hand it back, which is where the program's `?` is.
pub struct Failing<S>(pub(crate) S);

impl<S: Step<Item = Result<T, E>>, T, E> Failing<S> {
    /// The next item or failure, pausing while the step does - the shape a
    /// `for` over the sequence lowers to, with the `?` after it.
    pub async fn next(&mut self) -> Option<Result<T, E>> {
        self.0.step().await
    }

    /// Everything the sequence produces, as a list, or the first failure.
    pub async fn collect(mut self) -> Result<Vec<T>, E> {
        let mut out = Vec::new();
        while let Some(item) = self.next().await {
            out.push(item?);
        }
        Ok(out)
    }

    /// How many items the sequence produces, or the first failure.
    pub async fn count(mut self) -> Result<i64, E> {
        let mut n = 0_i64;
        while let Some(item) = self.next().await {
            item?;
            n += 1;
        }
        Ok(n)
    }

    /// The item at a position, nothing where the sequence is shorter, or the
    /// first failure before it.
    pub async fn nth(mut self, at: i64) -> Result<Option<T>, E> {
        if at < 0 {
            return Ok(None);
        }
        let mut seen = 0_i64;
        while let Some(item) = self.next().await {
            let item = item?;
            if seen == at {
                return Ok(Some(item));
            }
            seen += 1;
        }
        Ok(None)
    }

    /// Every item written one after another with this text between them, or
    /// the first failure.
    pub async fn join(mut self, separator: &str) -> Result<String, E>
    where
        T: std::fmt::Display,
    {
        let mut out = String::new();
        let mut first = true;
        while let Some(item) = self.next().await {
            let item = item?;
            if !first {
                out.push_str(separator);
            }
            out.push_str(&item.to_string());
            first = false;
        }
        Ok(out)
    }

    /// Each item through a plain lambda.
    pub fn map<U, F: FnMut(T) -> U>(self, f: F) -> Failing<OkMap<S, F>> {
        Failing(OkMap { inner: self.0, f })
    }

    /// Only the items a plain lambda says yes to.
    pub fn filter<F: FnMut(&T) -> bool>(self, f: F) -> Failing<OkFilter<S, F>> {
        Failing(OkFilter { inner: self.0, f })
    }

    /// Each item through a lambda that pauses.
    pub fn then<U, F: AsyncFnMut(T) -> U>(self, f: F) -> Failing<OkThen<S, F>> {
        Failing(OkThen { inner: self.0, f })
    }

    /// Only the items a lambda that pauses says yes to.
    pub fn then_filter<F: AsyncFnMut(&T) -> bool>(self, f: F) -> Failing<OkThenFilter<S, F>> {
        Failing(OkThenFilter { inner: self.0, f })
    }

    /// The first `n` steps. A failure is a step, and the walk ends at it.
    pub fn take(self, n: usize) -> Failing<Take<S>> {
        Failing(Take {
            inner: self.0,
            left: n,
        })
    }

    /// Everything after the first `n` steps.
    pub fn skip(self, n: usize) -> Failing<Skip<S>> {
        Failing(Skip {
            inner: self.0,
            skip: n,
        })
    }

    /// The first step, and then every `step`-th after it. A step of `0`
    /// aborts, as the language below's does.
    pub fn step_by(self, step: usize) -> Failing<StepBy<S>> {
        assert!(step != 0, "step_by: a step of 0 would never move");
        Failing(StepBy {
            inner: self.0,
            step,
            first: true,
        })
    }

    /// Pairs, one item of this and one of `other`, until the shorter ends.
    pub fn zip<J: IntoIterator>(self, other: J) -> Failing<OkZip<S, J::IntoIter>> {
        Failing(OkZip {
            inner: self.0,
            other: other.into_iter(),
        })
    }
}

/// **`map` with a lambda that pauses** (ADR-233 D1): a sequence whose step
/// pauses, one item at a time.
pub fn then<I: IntoIterator, U, F: AsyncFnMut(I::Item) -> U>(
    items: I,
    f: F,
) -> Paused<Then<Ready<I::IntoIter>, F>> {
    Paused(Then {
        inner: Ready(items.into_iter()),
        f,
    })
}

/// **`filter` with a lambda that pauses** (ADR-233 D1).
pub fn then_filter<I: IntoIterator, F: AsyncFnMut(&I::Item) -> bool>(
    items: I,
    f: F,
) -> Paused<ThenFilter<Ready<I::IntoIter>, F>> {
    Paused(ThenFilter {
        inner: Ready(items.into_iter()),
        f,
    })
}

/// **A list's own `map` with a lambda that pauses** (ADR-233 D2): the list,
/// each item awaited in turn.
pub async fn map_list<T, U>(items: Vec<T>, mut f: impl AsyncFnMut(T) -> U) -> Vec<U> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(f(item).await);
    }
    out
}

/// **`sort_by_key` with a key that pauses** (ADR-233 D2): each key is worked
/// out once, in order, and the list is sorted by them - stably, as the
/// synchronous entry sorts.
pub async fn sort_by_key<T, K: Ord>(items: &mut Vec<T>, mut key: impl AsyncFnMut(&T) -> K) {
    let mut keyed = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        let k = key(&item).await;
        keyed.push((k, item));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    items.extend(keyed.into_iter().map(|(_, item)| item));
}

/// **A map's entry, of either kind of map** — what [`or_insert_with`] and
/// [`and_modify`] need of one: the value where the key is there, and a place to
/// put one where it is not.
pub trait Slot<'a>: Sized {
    /// What the map holds.
    type Value: 'a;
    /// The value where the key is there, or the entry back where it is not.
    fn there(self) -> Result<&'a mut Self::Value, Self>;
    /// The value where the key is there, to change in place.
    fn there_mut(&mut self) -> Option<&mut Self::Value>;
    /// Put a value where the key is not there.
    fn put(self, value: Self::Value) -> &'a mut Self::Value;
}

impl<'a, K, V> Slot<'a> for std::collections::hash_map::Entry<'a, K, V> {
    type Value = V;
    fn there(self) -> Result<&'a mut V, Self> {
        match self {
            Self::Occupied(occupied) => Ok(occupied.into_mut()),
            vacant => Err(vacant),
        }
    }
    fn there_mut(&mut self) -> Option<&mut V> {
        match self {
            Self::Occupied(occupied) => Some(occupied.get_mut()),
            Self::Vacant(_) => None,
        }
    }
    fn put(self, value: V) -> &'a mut V {
        self.or_insert(value)
    }
}

impl<'a, K: Ord, V> Slot<'a> for std::collections::btree_map::Entry<'a, K, V> {
    type Value = V;
    fn there(self) -> Result<&'a mut V, Self> {
        match self {
            Self::Occupied(occupied) => Ok(occupied.into_mut()),
            vacant => Err(vacant),
        }
    }
    fn there_mut(&mut self) -> Option<&mut V> {
        match self {
            Self::Occupied(occupied) => Some(occupied.get_mut()),
            Self::Vacant(_) => None,
        }
    }
    fn put(self, value: V) -> &'a mut V {
        self.or_insert(value)
    }
}

/// **`or_insert_with` with a lambda that pauses** (ADR-233 D2): the value is
/// made only where the key is not there, as the synchronous entry makes it.
pub async fn or_insert_with<'a, E: Slot<'a>>(
    entry: E,
    make: impl AsyncFnOnce() -> E::Value,
) -> &'a mut E::Value {
    match entry.there() {
        Ok(value) => value,
        Err(vacant) => vacant.put(make().await),
    }
}

/// **`and_modify` with a lambda that pauses** (ADR-233 D2): the value is
/// changed only where the key is there, and the entry goes on to the next call
/// of the chain.
pub async fn and_modify<'a, E: Slot<'a>>(
    mut entry: E,
    change: impl AsyncFnOnce(&mut E::Value),
) -> E {
    if let Some(value) = entry.there_mut() {
        change(value).await;
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run<T>(future: impl Future<Output = T>) -> T {
        crate::rt::exec::block_on(future)
    }

    #[test]
    fn a_plain_sequence_joins_what_it_produces() {
        assert_eq!(Join::join([1, 2, 3].iter().map(|x| x * 2), "-"), "2-4-6");
        assert_eq!(Join::join(std::iter::empty::<i32>(), ","), "");
    }

    #[test]
    fn a_sequence_whose_step_pauses_is_walked_and_chained() {
        let doubled = run(then(vec![1, 2, 3], async |x| x * 2).collect());
        assert_eq!(doubled, vec![2, 4, 6]);
        let big = run(then_filter(vec![1, 2, 3, 4], async |x: &i32| *x > 2).count());
        assert_eq!(big, 2);
        let chained = run(then(vec![1, 2, 3], async |x| x + 1)
            .map(|x| x * 10)
            .filter(|x| *x > 20)
            .join(","));
        assert_eq!(chained, "30,40");
        let cut = run(then(1..=10, async |x| x)
            .skip(1)
            .step_by(3)
            .take(2)
            .zip(["a", "b", "c"])
            .collect());
        assert_eq!(cut, vec![(2, "a"), (5, "b")]);
    }

    /// A step that fails at `3`, as a stream that breaks would.
    struct Breaks(i32);

    impl Step for Breaks {
        type Item = Result<i32, String>;
        async fn step(&mut self) -> Option<Result<i32, String>> {
            self.0 += 1;
            match self.0 {
                3 => Some(Err("broke".to_string())),
                n if n > 5 => None,
                n => Some(Ok(n)),
            }
        }
    }

    #[test]
    fn a_failing_sequence_hands_the_item_to_the_lambda_and_the_failure_on() {
        let first_two = run(Failing(Breaks(0)).map(|x| x * 10).take(2).collect());
        assert_eq!(first_two, Ok(vec![10, 20]));
        let broken = run(Failing(Breaks(0)).then(async |x| x + 1).collect());
        assert_eq!(broken, Err("broke".to_string()));
        let past = run(Failing(Breaks(0))
            .skip(3)
            .then_filter(async |x: &i32| *x > 4)
            .zip(["a"])
            .collect());
        assert_eq!(past, Ok(vec![(5, "a")]));
        let counted = run(Failing(Breaks(0)).filter(|x| *x > 1).count());
        assert_eq!(counted, Err("broke".to_string()));
        let after = run(Failing(Breaks(3)).step_by(2).nth(0));
        assert_eq!(after, Ok(Some(4)));
    }

    #[test]
    fn the_eager_ones_await_the_lambda_where_the_entry_would_call_it() {
        let mut xs = vec![3, 1, 2];
        run(sort_by_key(&mut xs, async |x: &i32| *x));
        assert_eq!(xs, vec![1, 2, 3]);
        assert_eq!(run(map_list(vec![1, 2], async |x| x + 1)), vec![2, 3]);
        let mut m = std::collections::HashMap::new();
        run(or_insert_with(m.entry("a"), async || 5));
        run(or_insert_with(m.entry("a"), async || 7));
        run(and_modify(m.entry("a"), async |v: &mut i32| *v += 1));
        assert_eq!(m["a"], 6);
        let mut b = std::collections::BTreeMap::new();
        *run(or_insert_with(b.entry(1), async || 2)) += 1;
        assert_eq!(b[&1], 3);
    }
}
