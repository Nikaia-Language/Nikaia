//! [ADR-263](../../../docs/specification/adr/adr-263.md) D1 and D2 at
//! `user_parallelism = yes`, in a process of its own: the runtime is one per
//! process (ADR-303 D4), and D2 asks the whole of it - the pool included.

#[path = "idle/cases.rs"]
mod cases;

#[test]
fn a_file_operation_takes_the_calling_thread_exactly_when_main_is_alone() {
    let _runtime = nikaia_std::rt::start(nikaia_std::rt::UserCode::Concurrent);
    assert!(
        nikaia_std::rt::handle().user_pool().is_some(),
        "the pool is part of what D2 asks"
    );
    cases::run("yes", true);
}
