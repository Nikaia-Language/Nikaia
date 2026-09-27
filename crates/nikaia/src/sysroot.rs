//! The sysroot: where a generated project finds `nikaia-std`, and where the
//! compiled `std` is kept.
//!
//! [ADR-002](../../../docs/specification/adr/adr-002.md) D4. `nikaia-std` is
//! not published, so a generated project cannot reach it through a registry. It
//! reaches it through a **sysroot** - a directory that travels with the
//! compiler and holds `std`'s *sources*, with the Nikaia half already lowered to
//! Rust. Nothing needs the compiler to build `std`, which is the whole of why
//! the 58 packages that used to exist only to build it are gone.
//!
//! Two things live here:
//!
//! * **Where `std` is.** [`Sysroot::resolve`] - `NIKAIA_SYSROOT`, or the
//!   checkout this compiler was built from, which is what makes a build inside
//!   the repository work with no configuration at all.
//! * **Where the compiled `std` is.** [`Sysroot::rlib_cache`] - a directory in
//!   the user's cache named by [`Key::sysroot`], so a second project on the
//!   same machine links what the first one built instead of compiling it again.
//!
//! The **ledger** is not one of them. `std.contracts` is baked into this binary
//! with `include_str!` ([`crate::contracts::STD`]) and is read from there, never
//! from the sysroot. [ADR-005](../../../docs/specification/adr/adr-005.md) D8
//! says ledger stability across toolchain versions is explicitly *not*
//! required, so a `std` that could be paired with a different compiler would let
//! the ledger describe a compiler that is not there. Keeping the copy the
//! compiler answers from inside the compiler makes that pairing impossible
//! rather than merely discouraged.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use orchestrator::cache::{self, Key, Layout};

/// The development override, named for what it is.
///
/// It replaced `NIKAIA_STD_PATH`, which named a *crate directory* and could
/// therefore only ever answer one of the questions this module answers.
pub const SYSROOT_VAR: &str = "NIKAIA_SYSROOT";

/// `std`'s directory inside a sysroot. The same name it has in the checkout, so
/// that `crates/` *is* a sysroot and the in-tree flow needs no special case.
pub const STD_DIR: &str = "nikaia-std";

/// The extension `std`'s Nikaia half carries.
const NIKA: &str = "nika";

/// Where the Nikaia that the **toolchain** uses lives, under `std`'s `src`.
///
/// A directory rather than a naming convention, because what separates the two
/// is not what the files are like but what `std` promises about them
/// ([`Sysroot::tool_modules`]).
const TOOLS: &str = "tools";

/// A sysroot: a directory with `nikaia-std/` in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sysroot {
    root: PathBuf,
}

impl Sysroot {
    /// `NIKAIA_SYSROOT`, or the checkout this compiler was built from.
    ///
    /// The default is `crates/` - the directory `nikaia-std` sits in - which is
    /// what makes `cargo test --workspace` and a build inside the repository
    /// work with nothing set. An installed compiler points the variable at its
    /// own `nikaia-std`'s parent.
    pub fn resolve() -> Sysroot {
        let root = match std::env::var_os(SYSROOT_VAR) {
            Some(dir) => PathBuf::from(dir),
            None => {
                let here = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
                here.canonicalize().unwrap_or(here)
            }
        };
        Sysroot { root }
    }

    pub fn new(root: impl Into<PathBuf>) -> Sysroot {
        Sysroot { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `nikaia-std` a generated project depends on by path.
    pub fn std_dir(&self) -> PathBuf {
        self.root.join(STD_DIR)
    }

    /// `std`'s Nikaia half, in a fixed order.
    ///
    /// Sorted, because [ADR-005](../../../docs/specification/adr/adr-005.md) D8
    /// bans consuming a directory listing in filesystem order anywhere output
    /// depends on it, and the release step below writes files from this list.
    ///
    /// **The top level only, and that is the rule that says what `std` is.**
    /// A `.nika` beside `lib.rs` is a module of `std` and
    /// `crates/nikaia/tests/contracts.rs` requires `std.contracts` to carry
    /// every `pub` thing it declares. What is in [`Self::tool_modules`] is not.
    pub fn std_modules(&self) -> Result<Vec<PathBuf>> {
        self.nika_in(&self.std_dir().join("src"), "std's Nikaia modules")
    }

    /// **Nikaia the toolchain uses and `std` does not publish**, in `src/tools`.
    ///
    /// `tools/rust.nika` is the reading half of `nikaia describe`
    /// ([ADR-195](../../../docs/specification/adr/adr-195.md) D3), lowered the
    /// same way `std`'s own Nikaia half is and reached by the compiler as an
    /// ordinary Rust module ([ADR-196](../../../docs/specification/adr/adr-196.md)
    /// D1). It lives in this crate because this is where the release step
    /// already looks, and in a directory of its own because **a `.nika` beside
    /// `lib.rs` means something**: that `std` offers it, and that
    /// `std.contracts` has to say so. This one is not offered and must not be
    /// in that file.
    pub fn tool_modules(&self) -> Result<Vec<PathBuf>> {
        let dir = self.std_dir().join("src").join(TOOLS);
        match dir.is_dir() {
            false => Ok(Vec::new()),
            true => self.nika_in(&dir, "the toolchain's Nikaia modules"),
        }
    }

    /// Every `.nika` directly in one directory, sorted.
    fn nika_in(&self, src: &Path, what: &str) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        let entries = std::fs::read_dir(src)
            .with_context(|| format!("reading {} for {what}", src.display()))?;
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some(NIKA) {
                out.push(path);
            }
        }
        out.sort();
        Ok(out)
    }

    /// Where the compiled `std` for this build goes (ADR-002 D4).
    ///
    /// A directory in the user's cache named by [`Key::sysroot`]. It is a Cargo
    /// target directory and Cargo's own fingerprinting owns what is stale
    /// *inside* it ([ADR-021](../../../docs/specification/adr/adr-021.md) D1);
    /// the key decides only **which** directory, which is what lets two builds
    /// that differ in a way Cargo would answer by rebuilding - a different
    /// codegen table, a different toolchain - coexist instead of evicting one
    /// another (D7, §4's "dimensions coexist; they do not share").
    /// **Which tree, and the sweep that keeps the rest from piling up.** The key
    /// holds this compiler's own identity, so every rebuild of it starts a new
    /// tree and nothing ever took an old one away - measured, 1.7 GB in a user
    /// cache and 13 GB in the one the project tests share, with a build then
    /// failing for want of disk. Coexisting is right (D7); coexisting forever is
    /// the leak. See [`cache::sweep`].
    pub fn rlib_cache(&self, target: &str, codegen: &Codegen, features: &str) -> PathBuf {
        let key = Key::sysroot(
            env!("NIKAIA_COMPILER"),
            env!("NIKAIA_RUSTC_VERSION"),
            target,
            &codegen.render(),
            features,
        );
        let root = Layout::user_cache_dir().join("rlib");
        let tree = root.join(key.as_str());
        // Marked in use *before* the sweep, so this build's own tree is the
        // newest one and can never be the one that goes.
        cache::touch(&tree);
        cache::sweep(&root, cache::KEEP_TREES);
        tree
    }
}

/// What the machine's codegen table asked for, as a key dimension.
///
/// A rendered string rather than a struct of known keys on purpose: whatever
/// `[build.<target>]` grows - `target-cpu` being the obvious next one - becomes
/// a dimension of the compiled `std` in the same commit that adds it, with
/// nothing here to remember to update. That is [ADR-021](../../../docs/specification/adr/adr-021.md)
/// D13's obligation kept by construction rather than by discipline.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Codegen {
    table: BTreeMap<String, String>,
}

impl Codegen {
    /// The `[build.<target>]` table of the machine this build chose, plus the
    /// panic strategy - which is not a choice (ADR-037 D1) but does change the
    /// code, so it belongs in the key beside the choices.
    pub fn new(table: &BTreeMap<String, toml::Value>, panic: &str) -> Codegen {
        let mut rendered: BTreeMap<String, String> = table
            .iter()
            .map(|(key, value)| (key.clone(), value.to_string()))
            .collect();
        rendered.insert("panic".to_string(), panic.to_string());
        Codegen { table: rendered }
    }

    /// One line per key, in key order, so the same table renders the same way
    /// however it was read.
    fn render(&self) -> String {
        self.table
            .iter()
            .map(|(key, value)| format!("{key}={value}\n"))
            .collect()
    }
}

/// Lower one of `std`'s Nikaia modules to the Rust that is committed beside it.
///
/// **One setting, and that is now a promise rather than an accident.** The
/// build switches reach this as `target = x86_64-linux` (named, not defaulted,
/// since the default is the host) and `user_parallelism = no`, whatever the
/// consuming program is built at, because there is one compiled `std` per
/// machine and the Nikaia half of it is lowered
/// once, at release time. Nobody decided that while it was a build script; it is
/// decided now (ADR-002 D4), and it is sound for exactly as long as **nothing
/// switch-sensitive appears in `std`'s `.nika` files**.
///
/// **`Shared` is what would break it**, and it has been on both sides of that
/// sentence. [ADR-037](../../../docs/specification/adr/adr-037.md) D3 made it
/// `Rc` at `user_parallelism = no` and `Arc` at `yes`; D6 took the
/// representation off the switch and it was safe here for as long as that held;
/// [ADR-061](../../../docs/specification/adr/adr-061.md) D2 put the **emission**
/// back on it - at one user thread every count is the cheap one, because nothing
/// can cross there and D1 closed the last way out of the program. So a `Shared`
/// in a `.nika` file here would be lowered once with the cheap count and handed
/// to a program built at `yes`: a `std` that cannot cross a thread inside a
/// program that may.
///
/// What is *not* switch-sensitive is the **verdict** - whether a value may cross
/// at all - which `contracts::send` answers from a type and a destination and
/// never from the switch ([ADR-045](../../../docs/specification/adr/adr-045.md)
/// D1). That is what keeps `std`'s *ledger* sound across builds; it is the
/// lowered bytes this constraint is about. The constraint is checked rather than
/// only written down:
/// `tests/sysroot.rs::stds_nikaia_half_lowers_the_same_at_both_switches` lowers
/// every module at both settings and requires the bytes to agree, so the day
/// something switch-sensitive arrives it is a red build and not a miscompile.
pub fn lower_std_module(path: &Path) -> Result<String> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let parsed = crate::parser::parse_to_ast(&source)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    // **Checked before it is lowered**, which it was not, and
    // [Part III C.1](../../../docs/specification/30-nikaia-tooling.md) is what
    // that cost: a module this compiler would refuse a *program* for lowered in
    // silence, and `rustc` was left to complain about the generated file. It
    // happened — `tools/rust.nika` had a grammar action calling a function that
    // could pause, which `NK2209` exists to refuse, and what a reader saw was
    // *`await` is only allowed inside `async` functions*.
    //
    // The library is `std`'s own shipped ledger, which is what a module of it
    // is compiled against anyway, and the checks that need a project — a
    // manifest's boundary, an allowlist, a package's other units — have nothing
    // to say about a file that is one unit and depends on nothing.
    let own = crate::contracts::Ledger::infer(&parsed);
    let library =
        crate::contracts::Ledger::parse(crate::contracts::STD).context("std's shipped ledger")?;
    let checked = crate::check::check(&parsed, &own, &library);
    let refusals: Vec<&crate::check::Finding> = checked
        .findings
        .iter()
        .filter(|finding| matches!(finding.severity, crate::check::Severity::Error))
        .collect();
    if let Some(first) = refusals.first() {
        anyhow::bail!(
            "{}: {} {}{}",
            path.display(),
            first.code,
            first.message,
            match refusals.len() {
                1 => String::new(),
                more => format!(" (and {} more)", more - 1),
            }
        );
    }
    let lowered =
        crate::emit::emit_std(&parsed).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(lowered.rust)
}

/// Where `lower_std_module`'s result is committed: beside the source, same stem.
pub fn lowered_path(nika: &Path) -> PathBuf {
    nika.with_extension("rs")
}

/// Re-lower every Nikaia module in the sysroot's `std`, writing the `.rs` beside
/// the `.nika`. Returns the files that changed.
///
/// This is the release step, and the *only* thing that runs it is the `nikaia`
/// **binary** (`nikaia lower-std`). A from-source install may use it; a binary
/// install never has to, because the `.rs` is committed. What must not happen
/// again is `nikaia-std` linking the compiler as a library to do this, which is
/// what built the compiler a second time inside every project's `target/`.
pub fn lower_std(sysroot: &Sysroot) -> Result<Vec<PathBuf>> {
    let mut changed = Vec::new();
    let mut sources = sysroot.std_modules()?;
    sources.extend(sysroot.tool_modules()?);
    for nika in sources {
        let rust = lower_std_module(&nika)?;
        let rs = lowered_path(&nika);
        let current = std::fs::read_to_string(&rs).ok();
        if current.as_deref() != Some(rust.as_str()) {
            std::fs::write(&rs, &rust).with_context(|| format!("writing {}", rs.display()))?;
            changed.push(rs);
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default sysroot is the checkout, and `std` is in it. This is the
    /// in-tree flow's whole configuration.
    #[test]
    fn the_checkout_is_a_sysroot() {
        let sysroot = Sysroot::resolve();
        assert!(
            sysroot.std_dir().join("Cargo.toml").is_file(),
            "{} holds std",
            sysroot.root().display()
        );
    }

    /// D7. Two builds that differ only in their codegen table must not share a
    /// compiled `std`, or switching `opt-level` would evict the other one's
    /// artifacts instead of keeping its own beside them.
    #[test]
    fn the_codegen_table_changes_where_the_compiled_std_goes() {
        let sysroot = Sysroot::new("/nowhere");
        let plain = Codegen::new(&BTreeMap::new(), "unwind");
        let tuned = Codegen::new(
            &BTreeMap::from([("opt-level".to_string(), toml::Value::Integer(3))]),
            "unwind",
        );
        assert_ne!(
            sysroot.rlib_cache("x86_64-linux", &plain, ""),
            sysroot.rlib_cache("x86_64-linux", &tuned, ""),
        );
    }

    /// The machine is a dimension; it decides what `std` can offer at all
    /// (ADR-037 D1).
    #[test]
    fn the_machine_changes_where_the_compiled_std_goes() {
        let sysroot = Sysroot::new("/nowhere");
        let codegen = Codegen::new(&BTreeMap::new(), "unwind");
        assert_ne!(
            sysroot.rlib_cache("x86_64-linux", &codegen, ""),
            sysroot.rlib_cache("wasm32-unknown", &codegen, ""),
        );
    }

    /// The panic strategy is not a choice (ADR-037 D1), and it still changes the
    /// code - so it is in the key beside the choices.
    #[test]
    fn the_panic_strategy_is_part_of_the_codegen_dimension() {
        let table = BTreeMap::new();
        assert_ne!(
            Codegen::new(&table, "unwind").render(),
            Codegen::new(&table, "abort").render(),
        );
    }
}
