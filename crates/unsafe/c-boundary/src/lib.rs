//! What C hands a library's entry point and what the entry point hands back
//! (ADR-284 D5-D7). See `README.md` for every `unsafe` and its argument.

#![no_std]

/// The status of a call that went as asked (ADR-284 D7).
pub const OK: core::ffi::c_int = 0;
/// The caller's buffer is too small; `written` says the size that would do.
pub const E_TOO_SMALL: core::ffi::c_int = -2;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
