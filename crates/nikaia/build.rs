// Binaries that link `rustc_driver` need to find it at run time. Cargo does not
// add the toolchain's lib directory to the rpath, so the linked binary would
// fail to start with "librustc_driver-*.so: cannot open shared object file".
fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let sysroot = std::process::Command::new(std::env::var("RUSTC").unwrap_or("rustc".into()))
        .arg("--print=sysroot")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok());

    if let Some(sysroot) = sysroot {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}/lib", sysroot.trim());
    }

    // The exact `rustc` cargo is building with. `tests/diagnostics.rs` compiles
    // emitted Rust with it: the crates it links against were built by this one,
    // and a different toolchain on PATH would only report that.
    println!(
        "cargo:rustc-env=NIKAIA_RUSTC={}",
        std::env::var("RUSTC").unwrap_or("rustc".into())
    );

    // The compiler's own identity for the cache key (ADR-021 D3), as a
    // fingerprint of the sources that decide the output rather than as a
    // version string. `CARGO_PKG_VERSION` stays "0.1.0" across every edit to
    // the emitter, so keying on it would leave the key still while the thing
    // it identifies moved - D7's second failure direction, where the cache
    // serves the previous emitter's output and calls it fresh. It lands only
    // on compiler developers, which is exactly why it would survive.
    println!("cargo:rustc-env=NIKAIA_COMPILER={}", compiler_fingerprint());

    // The toolchain identity that goes into the build cache key (ADR-021 D2).
    // Resolved here rather than at run time: it is the rustc that built this
    // emitter, which is the one whose output the cache would be serving, and
    // asking for it once at compile time costs no subprocess per build.
    let version = std::process::Command::new(std::env::var("RUSTC").unwrap_or("rustc".into()))
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=NIKAIA_RUSTC_VERSION={version}");

    runtime_dependencies();
}

/// The two crates besides `std` that *emitted Rust* can name, as the generated
/// `Cargo.toml` will have to declare them (ADR-002 D1).
///
/// Baked in here rather than read from the workspace at run time, and that is
/// ADR-002 D4's doing. The old code walked up from `nikaia-std`'s directory to
/// find `[workspace.dependencies]`, which is true of a checkout and of nothing
/// else: a sysroot is not a Cargo workspace and has no manifest above it. The
/// versions are the *compiler's* own, because it is the compiler's emitter that
/// writes `winnow::Parser` into a program - so they travel with the compiler,
/// the same argument that keeps `std.contracts` inside this binary.
///
/// Still read rather than written down a second time: a program that linked a
/// different `winnow` from the one `winnow-grammar` was built against is a type
/// error at every `Stream` bound, so the two must not be able to drift.
fn runtime_dependencies() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace = manifest.join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", workspace.display());

    let text = std::fs::read_to_string(&workspace).unwrap_or_else(|e| {
        panic!(
            "reading {} for the runtime's versions: {e}",
            workspace.display()
        )
    });
    let document: toml::Value =
        toml::from_str(&text).unwrap_or_else(|e| panic!("parsing {}: {e}", workspace.display()));

    for (crate_name, variable) in [
        ("winnow-grammar", "NIKAIA_RUNTIME_WINNOW_GRAMMAR"),
        ("winnow", "NIKAIA_RUNTIME_WINNOW"),
    ] {
        let value = document
            .get("workspace")
            .and_then(|w| w.get("dependencies"))
            .and_then(|d| d.get(crate_name))
            .unwrap_or_else(|| {
                panic!(
                    "{} does not declare `{crate_name}` in `[workspace.dependencies]`, \
                     and a generated program names it directly",
                    workspace.display()
                )
            });
        // One line, because a build script's output is line-oriented. Every
        // dependency value Cargo accepts renders on one line as an inline table.
        println!("cargo:rustc-env={variable}={value}");
    }
}

/// SHA256 over everything that decides what the compiler emits: its own
/// sources, the prover's logic layer - whether a claim is proved decides
/// whether its check is emitted (ADR-269 D4, ADR-270) - and the `std` ledger
/// `contracts::STD` bakes in with `include_str!`.
/// Deterministic - the files are visited in sorted order and each is hashed
/// with its path, length-prefixed so two different file sets cannot agree.
fn compiler_fingerprint() -> String {
    use sha2::{Digest, Sha256};

    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut inputs = Vec::new();
    collect(&manifest.join("src"), &mut inputs);
    collect(&manifest.join("../nikaia-logic/src"), &mut inputs);
    inputs.push(manifest.join("../nikaia-std/std.contracts"));
    inputs.sort();

    let mut hasher = Sha256::new();
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    for path in &inputs {
        println!("cargo:rerun-if-changed={}", path.display());
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let name = path
            .strip_prefix(&manifest)
            .unwrap_or(path)
            .to_string_lossy();
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}
