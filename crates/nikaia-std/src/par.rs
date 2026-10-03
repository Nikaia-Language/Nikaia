//! **A list walked on every core at once** — `par_iter()`
//! ([ADR-235](../../../docs/specification/adr/adr-235.md), Part II 12.6).
//!
//! A `Par[T]` has `Seq[T]`'s surface, and this is that surface over rayon at
//! `user_parallelism = yes`; at `no` the compiler writes the list's own
//! `iter()` and nothing here is used. So a program means the same at both
//! settings: `collect` keeps the list's order, `count` counts in `usize` as
//! `Iterator::count` does, and what the lambda may do - not pause
//! (`NK2209`), not change a name outside it (`NK2107`) - is asked by the
//! checker at both.
//!
//! **What rayon has only for a walk whose length is known** - `take`, `skip`,
//! `step_by`, `rev`, `zip` - goes through a list here, so it works after a
//! `filter` too. That is the special case paying: a `map` or a `filter` over
//! the list allocates nothing it would not have.

use rayon::prelude::*;

/// A walk of a list on every core at once.
pub struct Par<I>(I);

/// A walk over a list this module made, for the walks rayon has only over one.
pub type Listed<T> = rayon::vec::IntoIter<T>;

/// Two listed walks, paired.
pub type Zipped<T, U> = rayon::iter::Zip<Listed<T>, Listed<U>>;

/// The elements of `items`, walked on every core at once.
pub fn iter<T: Sync>(items: &[T]) -> Par<rayon::slice::Iter<'_, T>> {
    Par(items.par_iter())
}

impl<I: ParallelIterator> Par<I> {
    /// Each element through a function, on every core.
    pub fn map<U: Send, F: Fn(I::Item) -> U + Sync + Send>(
        self,
        f: F,
    ) -> Par<rayon::iter::Map<I, F>> {
        Par(self.0.map(f))
    }

    /// Only the elements the function says yes to.
    pub fn filter<F: Fn(&I::Item) -> bool + Sync + Send>(
        self,
        f: F,
    ) -> Par<rayon::iter::Filter<I, F>> {
        Par(self.0.filter(f))
    }

    /// Each element through a function, for what the function does.
    pub fn for_each<F: Fn(I::Item) + Sync + Send>(self, f: F) {
        self.0.for_each(f)
    }

    /// Views of values that copy, as the values (ADR-231 D1).
    pub fn copied<'a, T: Copy + Send + Sync + 'a>(self) -> Par<rayon::iter::Copied<I>>
    where
        I: ParallelIterator<Item = &'a T>,
    {
        Par(self.0.copied())
    }

    /// Everything the walk produces, in the list's order, as what the place it
    /// goes declares - a list where nothing does (ADR-293 D25).
    pub fn collect<C: FromParallelIterator<I::Item>>(self) -> C {
        self.0.collect()
    }

    /// The elements as a list, for the walks rayon has only over one.
    fn listed(self) -> Vec<I::Item> {
        self.0.collect()
    }

    /// How many elements the walk produces.
    pub fn count(self) -> usize {
        self.0.count()
    }

    /// The element at a position, or nothing where there are fewer.
    pub fn nth(self, at: i64) -> Option<I::Item> {
        let at = usize::try_from(at).ok()?;
        self.listed().into_iter().nth(at)
    }

    /// Every element written one after another with this text between them.
    pub fn join(self, separator: &str) -> String
    where
        I::Item: std::fmt::Display,
    {
        self.0
            .map(|item| item.to_string())
            .collect::<Vec<_>>()
            .join(separator)
    }

    /// The same elements, from the last to the first.
    pub fn rev(self) -> Par<rayon::iter::Rev<Listed<I::Item>>> {
        Par(self.listed().into_par_iter().rev())
    }

    /// The first `n` elements.
    pub fn take(self, n: usize) -> Par<rayon::iter::Take<Listed<I::Item>>> {
        Par(self.listed().into_par_iter().take(n))
    }

    /// Everything after the first `n` elements.
    pub fn skip(self, n: usize) -> Par<rayon::iter::Skip<Listed<I::Item>>> {
        Par(self.listed().into_par_iter().skip(n))
    }

    /// The first element, and then every `step`-th after it. A step of `0`
    /// aborts, as the language below's does.
    pub fn step_by(self, step: usize) -> Par<rayon::iter::StepBy<Listed<I::Item>>> {
        assert!(step != 0, "step_by: a step of 0 would never move");
        Par(self.listed().into_par_iter().step_by(step))
    }

    /// Pairs, one element of each, until the shorter ends.
    pub fn zip<J: IntoIterator>(self, other: J) -> Par<Zipped<I::Item, J::Item>>
    where
        J::Item: Send,
    {
        let other: Vec<J::Item> = other.into_iter().collect();
        Par(self.listed().into_par_iter().zip(other.into_par_iter()))
    }
}

/// **A `for` over one** works out every element on every core, and then runs
/// the loop's body over them in the list's order.
impl<I: ParallelIterator> IntoIterator for Par<I> {
    type Item = I::Item;
    type IntoIter = std::vec::IntoIter<I::Item>;
    fn into_iter(self) -> Self::IntoIter {
        self.listed().into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_keeps_the_lists_order_and_seqs_surface() {
        let xs: Vec<i64> = (1..=100).collect::<Vec<_>>();
        let big = iter(&xs)
            .copied()
            .map(|x| x * 2)
            .filter(|x| *x > 150)
            .collect::<Vec<_>>();
        assert_eq!(big, (76..=100).map(|x| x * 2).collect::<Vec<_>>());
        assert_eq!(iter(&xs).filter(|x| **x % 2 == 0).count(), 50);
        assert_eq!(
            iter(&xs)
                .copied()
                .filter(|x| *x > 10)
                .take(2)
                .collect::<Vec<_>>(),
            vec![11, 12]
        );
        assert_eq!(iter(&xs).copied().skip(98).rev().join(","), "100,99");
        assert_eq!(iter(&xs).copied().step_by(40).nth(2), Some(81));
        assert_eq!(
            iter(&xs).copied().zip(["a", "b"]).collect::<Vec<_>>(),
            vec![(1, "a"), (2, "b")]
        );
        let seen = std::sync::atomic::AtomicI64::new(0);
        iter(&xs).for_each(|x| {
            seen.fetch_add(*x, std::sync::atomic::Ordering::Relaxed);
        });
        assert_eq!(seen.into_inner(), 5050);
        let looped: Vec<i64> = iter(&xs)
            .copied()
            .map(|x| x + 1)
            .into_iter()
            .take(3)
            .collect::<Vec<_>>();
        assert_eq!(looped, vec![2, 3, 4]);
    }
}
