//! The global allocator with a **budget per task** (ADR-327 D5 without a
//! block of its own): every allocation is counted against the task's bytes
//! and one past the budget ends the task. What checking a budget costs where
//! the memory still comes from the system.

#[path = "../workload.rs"]
mod workload;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// What a task may hold at once.
const BUDGET: usize = 64 * 1024;

thread_local! {
    /// What the running task holds; `usize::MAX` outside a task.
    static HELD: Cell<usize> = const { Cell::new(usize::MAX) };
}

struct Budgeted;

unsafe impl GlobalAlloc for Budgeted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        HELD.with(|held| {
            let now = held.get();
            if now != usize::MAX {
                let next = now + layout.size();
                if next > BUDGET {
                    panic!("the task is over its budget");
                }
                held.set(next);
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        HELD.with(|held| {
            let now = held.get();
            if now != usize::MAX {
                held.set(now.saturating_sub(layout.size()));
            }
        });
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        HELD.with(|held| {
            let now = held.get();
            if now != usize::MAX {
                let next = now - layout.size().min(now) + size;
                if next > BUDGET {
                    panic!("the task is over its budget");
                }
                held.set(next);
            }
        });
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static HEAP: Budgeted = Budgeted;

struct Counted;

impl workload::Tasks for Counted {
    fn begin(&self) {
        HELD.with(|held| held.set(0));
    }
    fn end(&self) {
        HELD.with(|held| held.set(usize::MAX));
    }
}

fn main() {
    workload::run(workload::tasks(), &Counted);
}
