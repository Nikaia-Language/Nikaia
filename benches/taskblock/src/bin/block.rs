//! **A block per task** (ADR-327 D4): a task allocates by moving a pointer
//! through a block of its own, gives nothing back one by one, and the block
//! is empty again when the task ends. A full block ends the task (D5). Outside
//! a task the system allocator answers.

#[path = "../workload.rs"]
mod workload;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// The block's size: what a task may hold.
const BLOCK: usize = 64 * 1024;

thread_local! {
    /// Where the block starts, null outside a task.
    static START: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
    /// How much of it is taken.
    static TAKEN: Cell<usize> = const { Cell::new(0) };
    /// Where the last allocation starts, which `realloc` may grow in place.
    static LAST: Cell<usize> = const { Cell::new(usize::MAX) };
}

struct Blocks;

impl Blocks {
    /// Take `size` bytes aligned to `align` from the block, or null where no
    /// task is running.
    fn take(size: usize, align: usize) -> Option<*mut u8> {
        let start = START.with(Cell::get);
        if start.is_null() {
            return None;
        }
        let taken = TAKEN.with(Cell::get);
        let at = (start as usize + taken).next_multiple_of(align) - start as usize;
        if at + size > BLOCK {
            panic!("the task's block is full");
        }
        TAKEN.with(|t| t.set(at + size));
        LAST.with(|l| l.set(at));
        Some(unsafe { start.add(at) })
    }

    /// Whether `ptr` is in the running task's block.
    fn ours(ptr: *mut u8) -> bool {
        let start = START.with(Cell::get);
        !start.is_null() && ptr >= start && (ptr as usize) < start as usize + BLOCK
    }
}

unsafe impl GlobalAlloc for Blocks {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match Self::take(layout.size(), layout.align()) {
            Some(ptr) => ptr,
            None => unsafe { System.alloc(layout) },
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // Inside the block nothing is given back one by one: the block is
        // emptied whole when the task ends.
        if !Self::ours(ptr) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if !Self::ours(ptr) {
            return unsafe { System.realloc(ptr, layout, size) };
        }
        let start = START.with(Cell::get);
        let at = ptr as usize - start as usize;
        // The last allocation grows where it is.
        if LAST.with(Cell::get) == at {
            if at + size > BLOCK {
                panic!("the task's block is full");
            }
            TAKEN.with(|t| t.set(at + size));
            return ptr;
        }
        let moved = Self::take(size, layout.align()).expect("a task is running");
        unsafe { std::ptr::copy_nonoverlapping(ptr, moved, layout.size().min(size)) };
        moved
    }
}

#[global_allocator]
static HEAP: Blocks = Blocks;

/// The one block, reused by every task on this thread.
struct PerTask {
    block: *mut u8,
}

impl workload::Tasks for PerTask {
    fn begin(&self) {
        START.with(|s| s.set(self.block));
        TAKEN.with(|t| t.set(0));
        LAST.with(|l| l.set(usize::MAX));
    }
    fn end(&self) {
        START.with(|s| s.set(std::ptr::null_mut()));
    }
}

fn main() {
    let layout = Layout::from_size_align(BLOCK, 16).expect("a block's layout");
    let block = unsafe { System.alloc(layout) };
    assert!(!block.is_null());
    workload::run(workload::tasks(), &PerTask { block });
    unsafe { System.dealloc(block, layout) };
}
