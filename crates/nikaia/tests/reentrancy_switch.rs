//! The re-entrancy check is a build option
//! ([ADR-039](../../../docs/specification/adr/adr-039.md) D8, Part I 1.2).
//!
//! Taking a lock while a lock is held is refused when the program is compiled
//! (`NK2203`, Part II 12.3), so under D2 the runtime check cannot fire in a
//! correct compiler. That is its role: **self-control of D2's rule, not error
//! handling.** No input can trigger it; if it fires, this compiler has a hole,
//! and without it such a hole is a silent hang instead.
//!
//! So it is a guarantee that may be **declined**, which is
//! [ADR-033](../../../docs/specification/adr/adr-033.md) D8's precedent: *a
//! semantic default that cannot be switched off is a decision imposed rather
//! than offered.* It lives in the manifest and not on the command line only,
//! because otherwise a shipped build is not reproducible.
//!
//! **Part I 1.2 is untouched by it.** For every program that obeys the nesting
//! rule both builds behave identically; the switch decides only whether a
//! violation is *noticed*, and nothing observable is switched.

use nikaia::manifest::Manifest;
use nikaia::project::Settings;

fn settings(manifest: &str) -> Settings {
    let manifest = Manifest::parse(manifest).expect("the manifest parses");
    Settings::resolve(&manifest, None, None).expect("the switches resolve")
}

/// **The key exists, and `yes` is the default.**
#[test]
fn the_manifest_carries_it_and_it_is_on_by_default() {
    assert_eq!(settings("").reentrancy_check, "yes");
    assert_eq!(
        settings("[build]\nreentrancy-check = \"no\"\n").reentrancy_check,
        "no"
    );
    assert!(settings("").build.reentrancy_check.is_on());
    assert!(
        !settings("[build]\nreentrancy-check = \"no\"\n")
            .build
            .reentrancy_check
            .is_on()
    );
}

/// **A third spelling is refused rather than guessed at**, which is what
/// `user-parallelism` does one switch over: `on` reads like this option and is
/// not it, and a build that took the default for a word it did not know would
/// ship the guarantee the manifest declined.
#[test]
fn a_third_spelling_is_refused() {
    let manifest = Manifest::parse("[build]\nreentrancy-check = \"on\"\n").expect("parses");
    let refused = Settings::resolve(&manifest, None, None).expect_err("`on` is not a value");
    assert!(
        refused.to_string().contains("is `yes` or `no`, not `on`"),
        "{refused}"
    );
}

/// **And a mistyped key is a mistyped key**, not a silently ignored one: the
/// manifest checks `[build]` against the list Part I 1.2 names.
#[test]
fn the_key_is_spelled_one_way() {
    let refused = Manifest::parse("[build]\nreentrancy_check = \"no\"\n")
        .expect_err("an underscore is the mistake this catches");
    assert!(
        refused.to_string().contains("reentrancy-check"),
        "{refused}"
    );
}

/// **It is a cache-key dimension** ([ADR-037](../../../docs/specification/adr/adr-037.md)
/// D4), like the other two — or a build that declined the guarantee would be
/// handed the artifact of one that did not.
#[test]
fn the_two_builds_do_not_share_an_artifact() {
    let on = settings("");
    let off = settings("[build]\nreentrancy-check = \"no\"\n");
    assert_ne!(on.choices().build, off.choices().build);
}

/// **And the compiled `std` does not share a tree either.** The check lives in
/// that crate, so without this the second project on the machine would link the
/// first one's answer.
#[test]
fn the_compiled_std_does_not_share_a_tree() {
    let sysroot = nikaia::sysroot::Sysroot::resolve();
    let codegen = nikaia::sysroot::Codegen::default();
    assert_ne!(
        sysroot.rlib_cache("x86_64-linux", &codegen, "yes"),
        sysroot.rlib_cache("x86_64-linux", &codegen, "no")
    );
}
