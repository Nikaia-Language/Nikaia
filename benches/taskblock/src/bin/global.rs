//! The system allocator, declared as the global one as the other two declare
//! theirs: without a declaration every allocation goes through `std`'s
//! default shims (`__rdl_alloc`, `__rdl_realloc`), which are not inlined and
//! cost about 420 instructions a task here - a difference of the declaration,
//! not of the strategy.

#[path = "../workload.rs"]
mod workload;

#[global_allocator]
static HEAP: std::alloc::System = std::alloc::System;

struct Nothing;

impl workload::Tasks for Nothing {
    fn begin(&self) {}
    fn end(&self) {}
}

fn main() {
    workload::run(workload::tasks(), &Nothing);
}
