#![allow(dead_code)]

//! **An allocator that may refuse** (ADR-327 D5): every byte is counted
//! against the running task's budget, and one past it is an `AllocError`
//! that the collection hands back instead of aborting. The memory itself
//! comes from the system, as it does for the `std` side.

use allocator_api2::alloc::{AllocError, Allocator, Global, Layout};
use std::cell::Cell;
use std::ptr::NonNull;

/// What a task may hold at once: high enough that nothing here is refused, so
/// what is counted is the check and not a refusal.
pub const BUDGET: usize = 1 << 30;

thread_local! {
    static HELD: Cell<usize> = const { Cell::new(0) };
}

#[derive(Clone, Copy, Default)]
pub struct Budget;

unsafe impl Allocator for Budget {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        HELD.with(|held| {
            let next = held.get() + layout.size();
            if next > BUDGET {
                return Err(AllocError);
            }
            held.set(next);
            Global.allocate(layout)
        })
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        HELD.with(|held| held.set(held.get() - layout.size()));
        unsafe { Global.deallocate(ptr, layout) }
    }

    #[inline]
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        HELD.with(|held| {
            let next = held.get() - old.size() + new.size();
            if next > BUDGET {
                return Err(AllocError);
            }
            held.set(next);
            unsafe { Global.grow(ptr, old, new) }
        })
    }
}

/// A list that grows only where the budget lets it.
pub type List<T> = allocator_api2::vec::Vec<T, Budget>;

/// Text, as the bytes of a list that may refuse.
pub type Text = List<u8>;

/// What a task does when its budget is spent: it ends.
#[cold]
#[inline(never)]
pub fn spent() -> ! {
    panic!("the task is over its budget")
}

/// Push, ending the task where the budget refuses.
#[inline]
pub fn push<T>(list: &mut List<T>, item: T) {
    if list.len() == list.capacity() && list.try_reserve(1).is_err() {
        spent()
    }
    list.push(item);
}

/// Append bytes, the same way.
///
/// **One copy, as `std`'s `Vec<u8>` makes it**: `allocator_api2`'s
/// `extend_from_slice` is `extend(iter().cloned())` on stable Rust, a loop of
/// one byte at a time, which doubled this file's tasks and is the crate's,
/// not the cost of a budget. A `std` built on it would copy as here.
#[inline]
pub fn extend(text: &mut Text, bytes: &[u8]) {
    if text.try_reserve(bytes.len()).is_err() {
        spent()
    }
    let len = text.len();
    // SAFETY: `try_reserve` left room for `bytes.len()` more, and a `u8` has
    // nothing to drop or run on copy.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), text.as_mut_ptr().add(len), bytes.len());
        text.set_len(len + bytes.len());
    }
}

/// `write!` into text that may refuse.
pub struct Into<'a>(pub &'a mut Text);

impl std::fmt::Write for Into<'_> {
    #[inline]
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        extend(self.0, s.as_bytes());
        Ok(())
    }
}

/// A new, empty text.
#[inline]
pub fn text() -> Text {
    List::new_in(Budget)
}

/// A map whose table comes from the budget.
pub type Map<K, V> =
    hashbrown::HashMap<K, V, std::hash::BuildHasherDefault<nikaia_std::hash::FxHasher>, Budget>;
