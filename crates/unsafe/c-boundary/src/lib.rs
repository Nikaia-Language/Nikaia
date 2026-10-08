//! What C hands a library's entry point and what the entry point hands back
//! (ADR-284 D5-D7, D11). See `README.md` for every `unsafe` and its argument.

use core::ffi::{c_int, c_void};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{RefCell, UnsafeCell};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// The status of a call that went as asked (ADR-284 D7).
pub const OK: core::ffi::c_int = 0;
/// The caller's buffer is too small; `written` says the size that would do.
pub const E_TOO_SMALL: core::ffi::c_int = -2;
/// No handle where one is needed.
pub const E_ARGUMENT: core::ffi::c_int = -1;
/// A call on a handle this thread already holds (ADR-284 D11).
pub const E_REENTRANT: core::ffi::c_int = -5;

/// **A run of values C keeps**, as the slice it is: `None` where a length
/// comes with no address.
///
/// # Safety
///
/// `at` points at `len` initialised values of `T` that stay alive and unchanged
/// for `'a`, or `len` is `0`.
pub unsafe fn run<'a, T>(at: *const T, len: usize) -> Option<&'a [T]> {
    if len == 0 {
        return Some(&[]);
    }
    if at.is_null() {
        return None;
    }
    // SAFETY: the caller's contract, above.
    Some(unsafe { core::slice::from_raw_parts(at, len) })
}

/// **Text C keeps**, as `&str`: `None` where it is not UTF-8, or a length
/// comes with no address.
///
/// # Safety
///
/// [`run`]'s.
pub unsafe fn text<'a>(at: *const u8, len: usize) -> Option<&'a str> {
    // SAFETY: the caller's contract, which is `run`'s.
    let bytes = unsafe { run(at, len) }?;
    core::str::from_utf8(bytes).ok()
}

/// **One value, written where C asked for it**; nothing where `out` is null.
///
/// # Safety
///
/// `out` is null, or a valid, aligned place for one `T` that nothing else
/// reads or writes during the call.
pub unsafe fn put<T>(out: *mut T, value: T) {
    if out.is_null() {
        return;
    }
    // SAFETY: the caller's contract, above.
    unsafe { out.write(value) }
}

/// **Values into the caller's buffer, counted in values** (ADR-284 D16):
/// [`hand_back`] for a run of structs - `written` the full count, `out ==
/// NULL` the count alone, and a run longer than `cap` [`E_TOO_SMALL`] with
/// nothing copied.
///
/// # Safety
///
/// `written` is null or a valid place for one `usize`; `out` is null or room
/// for `cap` values of `T` that do not overlap `values`.
pub unsafe fn hand_back_run<T: Copy>(
    values: &[T],
    out: *mut T,
    cap: usize,
    written: *mut usize,
) -> core::ffi::c_int {
    // SAFETY: the caller's contract for `written`.
    unsafe { put(written, values.len()) };
    if out.is_null() {
        return OK;
    }
    if values.len() > cap {
        return E_TOO_SMALL;
    }
    // SAFETY: `out` has room for `cap >= values.len()` values that do not
    // overlap `values`, by the caller's contract.
    unsafe { core::ptr::copy_nonoverlapping(values.as_ptr(), out, values.len()) };
    OK
}

/// **One value C keeps, read**: `None` where `at` is null. What a method of a
/// struct C holds by value is handed as `self` (ADR-284 D16).
///
/// # Safety
///
/// `at` is null, or a valid, aligned, initialised `T` nothing writes during
/// the call.
pub unsafe fn read<T: Copy>(at: *const T) -> Option<T> {
    if at.is_null() {
        return None;
    }
    // SAFETY: the caller's contract, above.
    Some(unsafe { at.read() })
}

/// **Bytes into the caller's buffer** (ADR-284 D6): `written` is set to the
/// full length, `out == NULL` asks only that, and bytes that do not fit in
/// `cap` are [`E_TOO_SMALL`] with nothing copied.
///
/// # Safety
///
/// `written` is null or a valid place for one `usize`; `out` is null or
/// `cap` writable bytes that do not overlap `bytes`.
pub unsafe fn hand_back(
    bytes: &[u8],
    out: *mut u8,
    cap: usize,
    written: *mut usize,
) -> core::ffi::c_int {
    // SAFETY: the caller's contract for `written`.
    unsafe { put(written, bytes.len()) };
    if out.is_null() {
        return OK;
    }
    if bytes.len() > cap {
        return E_TOO_SMALL;
    }
    // SAFETY: `out` has `cap >= bytes.len()` writable bytes that do not
    // overlap `bytes`, by the caller's contract.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    OK
}

/// **A value C holds by an address** (ADR-284 D5, D11): its lock, shared for
/// a call that only reads it and exclusive for one that changes it.
pub struct Handle<T> {
    value: RwLock<T>,
}

std::thread_local! {
    /// The handles this thread holds a lock on, by address: a call on one of
    /// them from the same thread is [`E_REENTRANT`] rather than a deadlock.
    static HELD: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// **A new handle for `value`**, which C frees with [`free`].
///
/// **`Send + Sync`**, because C may call in from any thread, two at once
/// (ADR-284 D2, D11): the lock makes the calls take turns where one changes
/// the value, and two that only read it read it together - which is sound
/// only for a value that may be read from two threads and dropped on a third.
pub fn handle<T: Send + Sync>(value: T) -> *mut Handle<T> {
    Box::into_raw(Box::new(Handle {
        value: RwLock::new(value),
    }))
}

/// The mark that this thread holds a handle, taken off when the lock is let
/// go - also when the call it was held for panics.
struct Mark(usize);

impl Mark {
    fn take(at: usize) -> Result<Mark, core::ffi::c_int> {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            if held.contains(&at) {
                return Err(E_REENTRANT);
            }
            held.push(at);
            Ok(Mark(at))
        })
    }
}

impl Drop for Mark {
    fn drop(&mut self) {
        HELD.with(|held| held.borrow_mut().retain(|at| *at != self.0));
    }
}

/// A handle's value, read for a call. The lock is let go before the mark.
pub struct Shared<'a, T> {
    guard: RwLockReadGuard<'a, T>,
    _mark: Mark,
}

impl<T> core::ops::Deref for Shared<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

/// A handle's value, changed by a call.
pub struct Exclusive<'a, T> {
    guard: RwLockWriteGuard<'a, T>,
    _mark: Mark,
}

impl<T> core::ops::Deref for Exclusive<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> core::ops::DerefMut for Exclusive<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

/// **The handle's value, to read**: [`E_ARGUMENT`] for null,
/// [`E_REENTRANT`] where this thread holds it already. A lock a panic left
/// poisoned is taken all the same: the library answers `E_PANICKED` after
/// one, and never reaches here.
///
/// # Safety
///
/// `at` is null, or a handle [`handle`] made and [`free`] has not freed, alive
/// for `'a`.
pub unsafe fn shared<'a, T: Send + Sync>(
    at: *const Handle<T>,
) -> Result<Shared<'a, T>, core::ffi::c_int> {
    if at.is_null() {
        return Err(E_ARGUMENT);
    }
    let mark = Mark::take(at as usize)?;
    // SAFETY: the caller's contract, above.
    let handle = unsafe { &*at };
    let guard = handle
        .value
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(Shared { guard, _mark: mark })
}

/// **The handle's value, to change**: as [`shared`], and alone.
///
/// # Safety
///
/// [`shared`]'s.
pub unsafe fn exclusive<'a, T>(at: *const Handle<T>) -> Result<Exclusive<'a, T>, core::ffi::c_int> {
    if at.is_null() {
        return Err(E_ARGUMENT);
    }
    let mark = Mark::take(at as usize)?;
    // SAFETY: the caller's contract, above.
    let handle = unsafe { &*at };
    let guard = handle
        .value
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(Exclusive { guard, _mark: mark })
}

/// **The handle freed**, and its value dropped. Null frees nothing, as C's
/// `free` does; a handle this thread holds is [`E_REENTRANT`] and stays.
///
/// # Safety
///
/// `at` is null, or a handle [`handle`] made and nothing has freed, which no
/// other thread uses during the call or after it.
pub unsafe fn free<T: Send + Sync>(at: *mut Handle<T>) -> core::ffi::c_int {
    if at.is_null() {
        return OK;
    }
    if HELD.with(|held| held.borrow().contains(&(at as usize))) {
        return E_REENTRANT;
    }
    // SAFETY: the caller's contract: `handle` made it with `Box::into_raw`,
    // and nothing uses it any more.
    drop(unsafe { Box::from_raw(at) });
    OK
}

/// **What C handed a call, taken to the thread that runs it** (ADR-284 D9):
/// the addresses and values of an `_async` call, which C keeps until its
/// `done` is called.
pub struct Sent<T>(T);

// SAFETY: `sent`'s contract - what the addresses point at is the C caller's,
// kept alive and left alone until `done`, so the one thread that uses them
// is the library's.
unsafe impl<T> Send for Sent<T> {}

/// **`value`, to be moved to the library thread that runs the call.**
///
/// # Safety
///
/// Every address in `value` stays valid, and nothing else touches what it
/// points at, until the call it is handed to has ended.
pub unsafe fn sent<T>(value: T) -> Sent<T> {
    Sent(value)
}

impl<T> Sent<T> {
    /// What was sent, on the thread it was sent to.
    pub fn into_inner(self) -> T {
        self.0
    }
}

/// **The caller's allocator**, as `<prefix>_set_allocator` takes it
/// (ADR-284 D6): room for `size` bytes aligned to `align`, or null.
pub type Alloc = unsafe extern "C" fn(size: usize, align: usize, ctx: *mut c_void) -> *mut c_void;
/// Gives back what [`Alloc`] handed out, with the same size and alignment.
pub type Free = unsafe extern "C" fn(at: *mut c_void, size: usize, align: usize, ctx: *mut c_void);

const UNDECIDED: u8 = 0;
const PLATFORM: u8 = 1;
const SETTING: u8 = 2;
const CALLER: u8 = 3;

/// **Where a library's memory comes from** (ADR-284 D6, D10): the caller's
/// allocator if it was set before anything was allocated, the platform heap
/// otherwise.
///
/// The choice is made once, by whichever comes first - `set` or the first
/// allocation - and never changes after, so every block is given back to the
/// allocator that handed it out. A `set` that comes too late is `E_ARGUMENT`.
pub struct Heap {
    state: AtomicU8,
    caller: UnsafeCell<Option<(Alloc, Free, *mut c_void)>>,
}

// SAFETY: `caller` is written once, by the one thread that moved `state` from
// UNDECIDED to SETTING, and read only after `state` was seen CALLER with
// `Acquire`, which the writer stored with `Release` after writing it. `ctx` is
// the C caller's, handed back to its own functions, which it made callable
// from any thread by handing them to a library.
unsafe impl Sync for Heap {}

impl Heap {
    /// A heap that has not chosen yet.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(UNDECIDED),
            caller: UnsafeCell::new(None),
        }
    }

    /// **Take the caller's allocator**: `OK`, or `E_ARGUMENT` where either
    /// function is missing or the heap has chosen already.
    pub fn set(&self, alloc: Option<Alloc>, free: Option<Free>, ctx: *mut c_void) -> c_int {
        let (Some(alloc), Some(free)) = (alloc, free) else {
            return E_ARGUMENT;
        };
        if self
            .state
            .compare_exchange(UNDECIDED, SETTING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return E_ARGUMENT;
        }
        // SAFETY: this thread moved `state` to SETTING, so nothing reads or
        // writes `caller` until it is CALLER (`Sync` above).
        unsafe { *self.caller.get() = Some((alloc, free, ctx)) };
        self.state.store(CALLER, Ordering::Release);
        OK
    }

    /// **Choose now**, so that a `set` after this is `E_ARGUMENT`: what
    /// `<prefix>_init` does (D10).
    pub fn settle(&self) {
        self.chosen();
    }

    /// The caller's functions, or `None` for the platform heap.
    fn chosen(&self) -> Option<(Alloc, Free, *mut c_void)> {
        loop {
            match self.state.load(Ordering::Acquire) {
                UNDECIDED => {
                    let _ = self.state.compare_exchange(
                        UNDECIDED,
                        PLATFORM,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
                SETTING => std::hint::spin_loop(),
                // SAFETY: seen CALLER with `Acquire`, after the write (`Sync`).
                CALLER => return unsafe { *self.caller.get() },
                _ => return None,
            }
        }
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: a block comes from the platform heap or from the caller's `alloc`,
// whichever the heap chose before handing out its first block, and goes back
// to the same one, since the choice never changes; the caller's functions keep
// `GlobalAlloc`'s contract, which `<prefix>_set_allocator`'s is.
unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match self.chosen() {
            // SAFETY: the caller's `alloc`, with the size and alignment asked.
            Some((alloc, _, ctx)) => unsafe { alloc(layout.size(), layout.align(), ctx) }.cast(),
            // SAFETY: `GlobalAlloc::alloc`'s contract, passed on.
            None => unsafe { System.alloc(layout) },
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        match self.chosen() {
            Some(_) => {
                // SAFETY: `GlobalAlloc::alloc`'s contract, passed on.
                let at = unsafe { self.alloc(layout) };
                if !at.is_null() {
                    // SAFETY: `at` is `layout.size()` bytes just handed out.
                    unsafe { at.write_bytes(0, layout.size()) };
                }
                at
            }
            // SAFETY: `GlobalAlloc::alloc_zeroed`'s contract, passed on.
            None => unsafe { System.alloc_zeroed(layout) },
        }
    }

    unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
        match self.chosen() {
            // SAFETY: `at` came from the caller's `alloc` with this layout.
            Some((_, free, ctx)) => unsafe { free(at.cast(), layout.size(), layout.align(), ctx) },
            // SAFETY: `at` came from `System` with this layout.
            None => unsafe { System.dealloc(at, layout) },
        }
    }

    unsafe fn realloc(&self, at: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        match self.chosen() {
            Some(_) => {
                // SAFETY: `size` is non-zero and fits `isize` with the
                // alignment, as `GlobalAlloc::realloc`'s contract says.
                let grown = unsafe { Layout::from_size_align_unchecked(size, layout.align()) };
                // SAFETY: as `alloc`.
                let moved = unsafe { self.alloc(grown) };
                if !moved.is_null() {
                    // SAFETY: both blocks hold at least the smaller size, and
                    // the new one is not the old one.
                    unsafe { core::ptr::copy_nonoverlapping(at, moved, layout.size().min(size)) };
                    // SAFETY: `at` came from here with `layout`.
                    unsafe { self.dealloc(at, layout) };
                }
                moved
            }
            // SAFETY: `GlobalAlloc::realloc`'s contract, passed on.
            None => unsafe { System.realloc(at, layout, size) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held<G>(got: Result<G, core::ffi::c_int>) -> G {
        got.unwrap_or_else(|status| panic!("status {status}"))
    }

    #[test]
    fn a_run_is_read_and_an_empty_one_needs_no_address() {
        let numbers = [1_i64, 2, 3];
        assert_eq!(unsafe { run(numbers.as_ptr(), 3) }, Some(&numbers[..]));
        assert_eq!(unsafe { run::<i64>(core::ptr::null(), 0) }, Some(&[][..]));
        assert_eq!(unsafe { run::<i64>(core::ptr::null(), 2) }, None);
    }

    #[test]
    fn text_that_is_not_utf8_is_refused() {
        let good = "héllo".as_bytes();
        assert_eq!(unsafe { text(good.as_ptr(), good.len()) }, Some("héllo"));
        let bad = [0xff_u8, 0xfe];
        assert_eq!(unsafe { text(bad.as_ptr(), bad.len()) }, None);
    }

    #[test]
    fn values_are_handed_back_counted() {
        let values = [1_i64, 2, 3];
        let mut room = [0_i64; 3];
        let mut written = 0;
        let asked = unsafe { hand_back_run(&values, core::ptr::null_mut(), 0, &mut written) };
        assert_eq!((asked, written), (OK, 3));
        let small = unsafe { hand_back_run(&values, room.as_mut_ptr(), 2, &mut written) };
        assert_eq!((small, room), (E_TOO_SMALL, [0, 0, 0]));
        let fits = unsafe { hand_back_run(&values, room.as_mut_ptr(), 3, &mut written) };
        assert_eq!((fits, room), (OK, [1, 2, 3]));
    }

    #[test]
    fn a_value_is_read_and_null_is_none() {
        let kept = 5_i64;
        assert_eq!(unsafe { read(&kept) }, Some(5));
        assert_eq!(unsafe { read::<i64>(core::ptr::null()) }, None);
    }

    #[test]
    fn a_value_is_put_and_null_takes_nothing() {
        let mut out = 0_i64;
        unsafe { put(&mut out, 42) };
        assert_eq!(out, 42);
        unsafe { put::<i64>(core::ptr::null_mut(), 7) };
    }

    #[test]
    fn bytes_are_handed_back_asked_for_and_refused_when_they_do_not_fit() {
        let mut room = [0_u8; 4];
        let mut written = 0;
        let asked = unsafe { hand_back(b"hey", core::ptr::null_mut(), 0, &mut written) };
        assert_eq!((asked, written), (OK, 3));
        let fits = unsafe { hand_back(b"hey", room.as_mut_ptr(), room.len(), &mut written) };
        assert_eq!((fits, written, &room[..3]), (OK, 3, &b"hey"[..]));
        let mut small = [0_u8; 2];
        let too_small = unsafe { hand_back(b"hey", small.as_mut_ptr(), small.len(), &mut written) };
        assert_eq!((too_small, written, small), (E_TOO_SMALL, 3, [0, 0]));
    }

    #[test]
    fn a_handle_is_read_changed_and_freed() {
        let counter = handle(1_i64);
        {
            let mut held = held(unsafe { exclusive(counter) });
            *held += 41;
        }
        assert_eq!(*held(unsafe { shared(counter) }), 42);
        assert_eq!(unsafe { free(counter) }, OK);
        assert_eq!(unsafe { free::<i64>(core::ptr::null_mut()) }, OK);
        assert!(matches!(
            unsafe { shared::<i64>(core::ptr::null()) },
            Err(E_ARGUMENT)
        ));
    }

    #[test]
    fn a_handle_this_thread_holds_is_reentrant_and_let_go_after() {
        let counter = handle(String::from("x"));
        {
            let _read = held(unsafe { shared(counter) });
            assert!(matches!(unsafe { shared(counter) }, Err(E_REENTRANT)));
            assert!(matches!(unsafe { exclusive(counter) }, Err(E_REENTRANT)));
            assert_eq!(unsafe { free(counter) }, E_REENTRANT);
        }
        assert!(unsafe { exclusive(counter) }.is_ok());
        assert_eq!(unsafe { free(counter) }, OK);
    }

    #[test]
    fn an_address_is_sent_to_the_thread_that_uses_it() {
        let mut out = 0_i64;
        let at = unsafe { sent(&mut out as *mut i64) };
        std::thread::spawn(move || unsafe { put(at.into_inner(), 9) })
            .join()
            .expect("joined");
        assert_eq!(out, 9);
    }

    #[test]
    fn two_threads_read_one_handle_at_once() {
        let counter = handle(7_i64) as usize;
        let reads: Vec<i64> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(move || *held(unsafe { shared(counter as *const Handle<i64>) }))
                })
                .collect();
            workers
                .into_iter()
                .map(|w| w.join().expect("joined"))
                .collect()
        });
        assert_eq!(reads, [7, 7]);
        assert_eq!(unsafe { free(counter as *mut Handle<i64>) }, OK);
    }

    /// The caller's allocator of the tests: the platform heap, counting the
    /// blocks it holds in `ctx`.
    unsafe extern "C" fn counted_alloc(size: usize, align: usize, ctx: *mut c_void) -> *mut c_void {
        let held = unsafe { &*(ctx as *const std::sync::atomic::AtomicIsize) };
        held.fetch_add(1, Ordering::SeqCst);
        unsafe { System.alloc(Layout::from_size_align_unchecked(size, align)) }.cast()
    }

    unsafe extern "C" fn counted_free(
        at: *mut c_void,
        size: usize,
        align: usize,
        ctx: *mut c_void,
    ) {
        let held = unsafe { &*(ctx as *const std::sync::atomic::AtomicIsize) };
        held.fetch_sub(1, Ordering::SeqCst);
        unsafe { System.dealloc(at.cast(), Layout::from_size_align_unchecked(size, align)) }
    }

    #[test]
    fn a_heap_set_first_uses_the_callers_allocator_for_every_block() {
        let held = std::sync::atomic::AtomicIsize::new(0);
        let heap = Heap::new();
        let ctx = &held as *const _ as *mut c_void;
        assert_eq!(heap.set(Some(counted_alloc), Some(counted_free), ctx), OK);
        let layout = Layout::from_size_align(24, 8).expect("a layout");
        let at = unsafe { heap.alloc_zeroed(layout) };
        assert_eq!(unsafe { *at.add(23) }, 0);
        let at = unsafe { heap.realloc(at, layout, 64) };
        assert_eq!(held.load(Ordering::SeqCst), 1);
        unsafe { heap.dealloc(at, Layout::from_size_align(64, 8).expect("a layout")) };
        assert_eq!(held.load(Ordering::SeqCst), 0);
        assert_eq!(
            heap.set(Some(counted_alloc), Some(counted_free), ctx),
            E_ARGUMENT
        );
    }

    #[test]
    fn a_heap_that_handed_out_a_block_keeps_the_platform_heap() {
        let held = std::sync::atomic::AtomicIsize::new(0);
        let heap = Heap::new();
        let ctx = &held as *const _ as *mut c_void;
        assert_eq!(heap.set(None, Some(counted_free), ctx), E_ARGUMENT);
        let layout = Layout::from_size_align(16, 16).expect("a layout");
        let at = unsafe { heap.alloc(layout) };
        assert_eq!(
            heap.set(Some(counted_alloc), Some(counted_free), ctx),
            E_ARGUMENT
        );
        unsafe { heap.dealloc(at, layout) };
        assert_eq!(held.load(Ordering::SeqCst), 0);
        let settled = Heap::new();
        settled.settle();
        assert_eq!(
            settled.set(Some(counted_alloc), Some(counted_free), ctx),
            E_ARGUMENT
        );
    }
}
