//! `nikaia.contracts` is a pure function of (source tree, toolchain), byte for
//! byte (ADR-005 D8).
//!
//! D8 bans six documented nondeterminism sources and says how the ban is
//! enforced: build twice and compare bytes. The half that matters is **twice in
//! two processes**. A hash map's iteration order is stable within one process
//! and randomised between them, because `RandomState` seeds SipHash per
//! process - so a same-process double run would pass over the one banned item
//! that is easiest to write by accident, and hardest to see in review.
//!
//! The risk D8 names is not the mathematics. A monotone constraint system over
//! a lattice has a unique least fixpoint and solving order cannot change it.
//! The risk is multi-year discipline: any new emit path can smuggle in a
//! hash-map iteration, and this is what makes that a red build on the commit
//! that does it rather than a puzzle a year later.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the workspace root")
        .to_path_buf()
}

/// Lower `source` in a fresh process and hand back the ledger it wrote.
///
/// `--no-cache` because a cache hit would compare a stored artifact with
/// itself: the point is to run the inference twice, not to prove the store
/// works. `threads` goes to rayon, which D8 explicitly permits the inference to
/// use - the requirement is purity, not single-threadedness.
fn ledger(source: &Path, scratch: &Path, threads: &str) -> Vec<u8> {
    let output = scratch.join("out.rs");
    let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["--input", source.to_str().expect("utf-8 path")])
        .args(["--backend", "rust"])
        .args(["--output", output.to_str().expect("utf-8 path")])
        .args(["--no-cache"])
        .env("RAYON_NUM_THREADS", threads)
        .output()
        .expect("the nikaia binary runs");
    assert!(
        run.status.success(),
        "lowering {} failed\nstderr: {}",
        source.display(),
        String::from_utf8_lossy(&run.stderr)
    );
    std::fs::read(output.with_file_name("nikaia.contracts")).expect("the ledger was written")
}

/// Every example the bootstrap compiler can lower, twice, in two processes,
/// with a different thread count each time.
#[test]
fn the_ledger_is_the_same_bytes_in_a_second_process() {
    let mut compared = 0;

    let mut examples: Vec<PathBuf> = common::standalone_programs(&repo_root().join("examples"));
    // Sorted for the same reason D8 bans consuming a directory listing in
    // filesystem order: a test whose coverage depends on `readdir` reports a
    // different thing on ext4 and on APFS.
    examples.sort();

    for example in examples {
        let name = example.file_stem().expect("a name").to_string_lossy();

        // The source is copied per run so the two processes cannot reach each
        // other's output, and so nothing is written into the source tree.
        let first = common::scratch_dir(&format!("ledger-{name}-1"));
        let second = common::scratch_dir(&format!("ledger-{name}-2"));
        let (a, b) = (first.join("in.nika"), second.join("in.nika"));
        std::fs::copy(&example, &a).expect("copy the example");
        std::fs::copy(&example, &b).expect("copy the example");

        // Not every example lowers yet; one that does not is not this test's
        // business, and skipping it is honest as long as the count below shows
        // the test is not vacuous.
        let lowered = Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .args(["--input", a.to_str().expect("utf-8 path")])
            .args(["--backend", "rust"])
            .args(["--output", first.join("probe.rs").to_str().expect("path")])
            .output()
            .expect("the nikaia binary runs");
        if !lowered.status.success() {
            continue;
        }

        assert_eq!(
            ledger(&a, &first, "1"),
            ledger(&b, &second, "4"),
            "`nikaia.contracts` for {name} differs between two processes - \
             ADR-005 D8 says it may not. The usual cause is iterating a hash \
             map to produce output: the SipHash seed is randomised per process, \
             so this is exactly the failure a same-process comparison misses."
        );
        compared += 1;
    }

    assert!(
        compared >= 5,
        "only {compared} examples lowered, so this test is not saying much - \
         D8's guarantee is worth a corpus"
    );
}
