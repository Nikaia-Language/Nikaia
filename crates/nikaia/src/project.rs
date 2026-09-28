//! A project build: `nikaia.toml` becomes a `Cargo.toml`, and `cargo` runs it.
//!
//! [ADR-002](../../../docs/specification/adr/adr-002.md) D1. Dependency
//! resolution and linking are solved problems, and a language that cannot reach
//! crates.io starts with no ecosystem - so the toolchain does not resolve
//! versions, does not link, and does not own a build graph. It translates a
//! manifest, hands the result to Cargo, and intercepts the one step Cargo
//! cannot do: turning a `.nika` file into something `rustc` accepts.
//!
//! The interception is `RUSTC_WORKSPACE_WRAPPER`, and the choice of *that*
//! variable rather than `RUSTC_WRAPPER` is the load-bearing part. It is applied
//! to **workspace members only**, so a crate from crates.io is compiled by the
//! real `rustc` with nothing in between - which is what makes "native Rust
//! dependencies pass through unchanged" true of the compilation and not only of
//! the manifest.
//!
//! What is generic about all this lives in
//! [`orchestrator::project`] ([ADR-003](../../../docs/specification/adr/adr-003.md)
//! D2): rendering a manifest, running `cargo`, splitting a `rustc` command
//! line. What is here is Nikaia's half - which file is the entry, which
//! switches were chosen, and what the lowering does.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use orchestrator::cache::{Artifacts, Cache, Choices, Layout, Lockfile};
use orchestrator::project::{
    Cargo, CargoProject, CrateKind, Invocation, Package, Profile, Workspace,
    record_extra_dependencies, resolved_versions, toolchain_can_build_for, toolchain_is_new_enough,
    write_if_changed,
};

use crate::contracts::{Ledger, STD, sync};
use crate::emit::{Build, Target};
use crate::manifest::{Dependency, Manifest};
use crate::sysroot::{Codegen, Sysroot};
use crate::{assets, check, diagnostics, modules, refuse, refused};

/// The extension a Nikaia source carries, and therefore the argument the
/// wrapper is looking for in a `rustc` command line.
pub const EXTENSION: &str = "nika";

/// The entry point of a project, relative to its root (Part III 13.1).
pub const ENTRY: &str = "src/main.nika";

/// Set on the wrapper's own process, and the only thing that tells it apart
/// from an ordinary run: Cargo owns the wrapper's argument list, so there is no
/// flag or subcommand available to say which mode this is.
pub const WRAPPER_MARKER: &str = "NIKAIA_RUSTC_WRAPPER";
const TARGET_VAR: &str = "NIKAIA_BUILD_TARGET";
const PARALLELISM_VAR: &str = "NIKAIA_USER_PARALLELISM";
/// The third switch's word, down the same channel
/// ([ADR-039](../../docs/specification/adr/adr-039.md) D8).
const REENTRANCY_VAR: &str = "NIKAIA_REENTRANCY_CHECK";
/// Set where the build is `nikaia test`'s
/// ([ADR-245](../../docs/specification/adr/adr-245.md) D1).
const TESTS_VAR: &str = "NIKAIA_TESTS";
const GEN_DIR_VAR: &str = "NIKAIA_GEN_DIR";
const NO_CACHE_VAR: &str = "NIKAIA_NO_CACHE";

/// How `--allow-read-from-list` reaches the `rustc` wrapper
/// ([ADR-072](../../docs/specification/adr/adr-072.md) D2, D6).
///
/// The wrapper is a **second process**, started by Cargo, and it lowers the
/// same file a second time — so it has to be told the same list or the two
/// halves would disagree about which files this build may read. The value is
/// an absolute path, because the wrapper runs in a directory Cargo chose.
///
/// **And it is how D6 holds.** Every member of the workspace is lowered under
/// one environment, so a dependency's `asset("…")` is checked against the list
/// of the build that is running rather than against anything the dependency
/// ships.
const READS_VAR: &str = "NIKAIA_ALLOW_READ_FROM_LIST";

/// A file the wrapper appends one line to per `rustc` it is handed: the crate
/// Cargo named, and whether this compiler lowered it or passed it through.
///
/// It exists because D1's central claim is otherwise invisible. A crate from
/// crates.io compiled by the real `rustc` and one passed *through* this wrapper
/// to the real `rustc` produce identical output, so "dependencies are untouched"
/// cannot be checked by looking at what was built - only by asking what was
/// asked for. Off unless the variable is set.
pub const TRACE_VAR: &str = "NIKAIA_WRAPPER_TRACE";

/// The build switches, resolved: flag over manifest over built-in default.
///
/// One value threaded through rather than three fields re-read at each use.
/// Every setting is kept both as the word that named it and as the parsed
/// thing, because the cache key and the diagnostics want the word (ADR-021 D5
/// hashes what was asked for) and the emitter wants the value.
#[derive(Debug, Clone)]
pub struct Settings {
    pub build: Build,
    pub target: String,
    pub user_parallelism: String,
    pub reentrancy_check: String,
    /// **Whether this is `nikaia test`'s build**
    /// ([ADR-245](../../docs/specification/adr/adr-245.md) D1): the program's
    /// `test` blocks are compiled, and its `main` runs them by number. A
    /// choice like the others, so the cache and the wrapper both see it.
    pub tests: bool,
}

impl Settings {
    /// Resolve every switch before anything is read, so a mistyped one fails on
    /// its own account rather than after a compile (ADR-037 D5).
    pub fn resolve(
        manifest: &Manifest,
        target: Option<&str>,
        user_parallelism: Option<&str>,
    ) -> Result<Settings> {
        let target = manifest
            .setting("target", target, Target::default().name())
            .to_string();
        let user_parallelism = manifest
            .setting("user-parallelism", user_parallelism, "no")
            .to_string();
        // **No flag of its own** ([ADR-039](../../docs/specification/adr/adr-039.md)
        // D8): it *lives in the manifest*, because otherwise a shipped build is
        // not reproducible, and Part I 1.2 names the two that a single build may
        // override. The environment is not a second front door — it is how the
        // resolved word reaches the wrapper, which Cargo owns the arguments of.
        let reentrancy_check = manifest
            .setting("reentrancy-check", None, "yes")
            .to_string();
        Ok(Settings {
            build: Build::parse(&target, &user_parallelism, &reentrancy_check)?,
            target,
            user_parallelism,
            reentrancy_check,
            tests: false,
        })
    }

    /// The words, for the wrapper. Cargo owns the wrapper's arguments, so the
    /// environment is the only channel the switches can travel down - and they
    /// travel already resolved, because a second resolution is a second chance
    /// to disagree.
    fn as_env(&self) -> Vec<(String, OsString)> {
        vec![
            (TARGET_VAR.to_string(), OsString::from(&self.target)),
            (
                PARALLELISM_VAR.to_string(),
                OsString::from(&self.user_parallelism),
            ),
            (
                REENTRANCY_VAR.to_string(),
                OsString::from(&self.reentrancy_check),
            ),
        ]
        .into_iter()
        .chain(
            self.tests
                .then(|| (TESTS_VAR.to_string(), OsString::from("1"))),
        )
        .collect()
    }

    /// What the driver put in the environment. Absent means the wrapper was run
    /// by hand, and the built-in defaults are the honest answer.
    fn from_env() -> Result<Settings> {
        let word =
            |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_string());
        let target = word(TARGET_VAR, Target::default().name());
        let user_parallelism = word(PARALLELISM_VAR, "no");
        let reentrancy_check = word(REENTRANCY_VAR, "yes");
        Ok(Settings {
            build: Build::parse(&target, &user_parallelism, &reentrancy_check)?,
            target,
            user_parallelism,
            reentrancy_check,
            tests: std::env::var_os(TESTS_VAR).is_some(),
        })
    }

    /// The cache's view of this build (ADR-021 D5).
    ///
    /// The backend is `"rust"` because it names **the lowering that produced the
    /// artifact**, not the flag the user typed, and a bare invocation *is*
    /// `--backend rust` ([ADR-004](../../docs/specification/adr/adr-004.md) D1),
    /// so the two spellings share one entry - which is correct, because it is
    /// one lowering. The dimension itself stays in
    /// [`orchestrator::cache::Key::build`] (D5, D7) so that the day a second
    /// backend caches, the literal here becomes that backend's name and the two
    /// cannot collide.
    pub fn choices(&self) -> Choices {
        Choices::new(
            format!(
                "{}/{}/{}{}",
                self.target,
                self.user_parallelism,
                self.reentrancy_check,
                if self.tests { "/tests" } else { "" }
            ),
            "rust",
        )
    }

    /// What a panic does on this machine (ADR-037 D1), in Cargo's vocabulary.
    ///
    /// Not a choice, which is why it is not in the codegen table: WebAssembly
    /// traps and has no stack to unwind, and a build that asked for `unwind`
    /// there would be asking for something the machine does not have.
    fn panic_strategy(&self) -> &'static str {
        match self.build.target {
            Target::X86_64Linux | Target::Aarch64Linux => "unwind",
            Target::Wasm32Unknown => "abort",
        }
    }
}

/// Everything one lowering produced.
pub struct Lowered {
    pub rust: String,
    pub ledger: String,
    /// Every `.nika` file that took part, entry first. The wrapper puts these
    /// back into Cargo's dependency info, or a changed module would not
    /// rebuild.
    pub sources: Vec<PathBuf>,
    /// Whether the build cache answered instead of the emitter.
    pub reused: bool,
}

/// Stage 0: parse, check, lower, infer - or serve all four from the cache.
///
/// The build cache (ADR-021) covers the whole of it: on a hit nothing is
/// checked, lowered or inferred, because only a build that passed every check
/// was recorded and the key holds everything those checks depend on (D13).
/// **The packages a project depends on by path**
/// ([ADR-047](../../docs/specification/adr/adr-047.md) D2), resolved against
/// the manifest's own directory.
///
/// A free function because two callers need the same answer: the project build,
/// and the `rustc` wrapper, which is handed a `.nika` path by Cargo and has to
/// find the manifest above it (`wrapper_main`). Two copies of a resolution rule
/// is two chances to disagree about what a program is.
///
/// Three of D2's five rules are kept here, because they are about the set of
/// dependencies rather than about any one of them:
///
/// * **rule 1**, the manifest key is the name: the map's key is what `use`
///   writes and the path is only where it comes from, so nothing inside a
///   package gets to name it;
/// * **rule 3**, the same path is the same package: two keys resolving to one
///   directory would be two `mod`s of one package and two types of one name,
///   so it is refused rather than unified;
/// * **rule 4**, a dependency's `[build]` is ignored, and the compiler says
///   so — a package is built with the settings of the program that uses it,
///   and anything else puts two answers to the parallelism question in one
///   build.
///
/// **rule 2** — transitive dependencies are not visible — is no longer checked
/// anywhere, because it is structural
/// ([ADR-053](../../docs/specification/adr/adr-053.md) D3): a package is
/// generated as its own crate naming only its own dependencies, so a name from
/// two levels down does not resolve and no rule of ours has to say so.
/// **rule 5** is the generated workspace's profile (D4): every Nikaia crate is
/// named on the program's side of the overflow checks, beside the exception
/// that turns them off for every foreign package.
pub fn packages_of(
    manifest: &Manifest,
    root: &Path,
    announce: bool,
) -> Result<Vec<modules::Dependency>> {
    let mut out: Vec<modules::Dependency> = Vec::new();
    let mut seen: BTreeMap<PathBuf, String> = BTreeMap::new();

    // **What this build calls each package**, before any of them is read, so
    // that a package reached twice is renamed to one name rather than to
    // whichever of the two was resolved first
    // ([ADR-053](../../docs/specification/adr/adr-053.md) D2).
    let mut here: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (name, value) in manifest.dependencies() {
        if let Dependency::Path(path) = value {
            let at = root.join(path);
            here.insert(at.canonicalize().unwrap_or(at), name.clone());
        }
    }

    for (name, value) in manifest.dependencies() {
        let Dependency::Path(path) = value else {
            continue;
        };
        let root = root.join(path);
        let root = root.canonicalize().unwrap_or(root);

        if let Some(first) = seen.get(&root) {
            refuse!(
                "`{name}` and `{first}` are the same package: both are {}.\n\
                 The same path is the same package (ADR-047 D2), so two names for it \
                 would be two copies of every type it declares - name it once.",
                root.display()
            );
        }
        seen.insert(root.clone(), name.clone());

        // **A package's own packages are resolved, not refused**
        // (ADR-053 D1): it is generated as its own crate, so they are its
        // dependencies and not ours. Read here only to fail early where one is
        // not there.
        let dependency = Manifest::read(&root.join("nikaia.toml"))?;
        // `announce`, because this resolution runs more than once per build - the
        // driver, and the `rustc` wrapper Cargo calls for the build and again for
        // a run - and a note a reader meets three times reads as three problems.
        // The driver is the one that reports (ADR-005 D7's shape: the work happens
        // where it happens, the saying happens once).
        if announce && dependency.has_build_section() {
            eprintln!(
                "note: `{}`'s own `[build]` is ignored: a package is built with the \
                 settings of the program that uses it (ADR-047 D2).",
                name
            );
        }

        out.push(modules::Dependency {
            name: name.clone(),
            renames: renames_in(&dependency, &root, &here)?,
            root,
            // **Its own keys**, so its `use` lines are read the way its own
            // build would read them (ADR-053 D3).
            reachable: dependency.dependencies().keys().cloned().collect(),
        });
    }
    Ok(out)
}

/// How a dependency's **own** package names are read in this build
/// ([ADR-053](../../docs/specification/adr/adr-053.md) D2: a package reached
/// through two parents is one package).
///
/// A library writes its own manifest keys in its own signatures, and those keys
/// are its author's choice rather than this program's. So a program that depends
/// on `deep` directly, and on a library that depends on the same directory as
/// `c`, was told its `deep::Id` was not the `c::Id` that library takes - one
/// type, refused for having two spellings, which is the one thing D2 says it is
/// not. The types are identical in the generated Rust; only the name this
/// compiler gave them differed.
///
/// So the identity is the **package**, and this is the translation into the word
/// this build uses for it: the program's own key where the program names it too,
/// and otherwise the package's `[package] name`, which `members_of` has already
/// refused to let two packages share. Either way it is a function of the
/// directory and not of who asked.
///
/// One level deep, because one level is what is read: a dependency's own
/// dependencies are its crate's business (D3), and nothing below it is in this
/// program's ledger to be renamed.
fn renames_in(
    dependency: &Manifest,
    dependency_root: &Path,
    here: &BTreeMap<PathBuf, String>,
) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (key, value) in dependency.dependencies() {
        let Dependency::Path(path) = value else {
            continue;
        };
        let at = dependency_root.join(path);
        let at = at.canonicalize().unwrap_or(at);
        let canonical = match here.get(&at) {
            Some(ours) => ours.clone(),
            None => Manifest::read(&at.join("nikaia.toml"))?
                .package_name()
                .map(str::to_string)
                .unwrap_or_else(|| key.clone()),
        };
        if &canonical != key {
            out.insert(key.clone(), canonical);
        }
    }
    Ok(out)
}

/// One package of a build, and everything needed to generate a crate for it
/// ([ADR-053](../../docs/specification/adr/adr-053.md) D1).
#[derive(Debug, Clone)]
pub struct Member {
    /// The Cargo package name - the `[package] name` of its own `nikaia.toml`.
    /// Its **identity** is `root`; this is what the profile rows and the
    /// renaming form below name it by.
    pub name: String,
    /// The canonical package directory. Two manifest keys resolving here are
    /// one crate, which is [ADR-047](../../docs/specification/adr/adr-047.md)
    /// D2 rule 3 holding because Cargo already works that way (D2).
    pub root: PathBuf,
    pub manifest: Manifest,
    /// The `.nika` file Cargo is told is the crate root. `src/main.nika` where
    /// there is one; a library needs none, so the first source beside it stands
    /// in - the wrapper walks up from whatever it is handed, so any file of the
    /// package finds the same manifest.
    pub entry: PathBuf,
    /// Its **own** packages, under its **own** manifest keys.
    pub dependencies: Vec<modules::Dependency>,
}

/// The members in an order where every package comes after the packages it
/// depends on ([ADR-100](../../docs/specification/adr/adr-100.md) D5).
///
/// **Why not simply reverse `members_of`.** That walk is breadth-first from the
/// entry, and reversing it is a topological order only where nothing is reached
/// two ways: with `a → b`, `a → c` and `c → b`, breadth-first gives `a, b, c`
/// and the reverse puts `c` before `b`, which is exactly backwards. So this is
/// Kahn's, over the edges the members already carry.
///
/// **A cycle keeps the order it had.** Two packages that depend on each other
/// are refused below this — Cargo will not build the workspace — and a
/// compiler that hung or panicked on the way to that message would be reporting
/// the wrong thing. What comes out is every member exactly once either way,
/// which is what the caller needs of it.
fn dependencies_first(members: &[Member]) -> Vec<usize> {
    let at_of: BTreeMap<&Path, usize> = members
        .iter()
        .enumerate()
        .map(|(at, m)| (m.root.as_path(), at))
        .collect();

    // `needs[at]` is what `at` waits for; `feeds[at]` is who waits for it.
    let needs: Vec<BTreeSet<usize>> = members
        .iter()
        .map(|m| {
            m.dependencies
                .iter()
                .filter_map(|d| at_of.get(canonical(&d.root).as_path()).copied())
                .collect()
        })
        .collect();

    let mut left: Vec<usize> = needs.iter().map(|n| n.len()).collect();
    // Ready in `members_of`'s order, so the result is a function of the source
    // tree and not of a set's iteration (Part III 13.5).
    let mut ready: Vec<usize> = (0..members.len()).filter(|at| left[*at] == 0).collect();
    let mut out: Vec<usize> = Vec::with_capacity(members.len());

    while let Some(at) = ready.first().copied() {
        ready.remove(0);
        out.push(at);
        for (other, waits_for) in needs.iter().enumerate() {
            if waits_for.contains(&at) {
                left[other] -= 1;
                if left[other] == 0 {
                    ready.push(other);
                }
            }
        }
    }

    // A cycle leaves members unplaced. They go on the end in the order they
    // came, so every member is lowered exactly once and the refusal that is
    // really about this arrives from where it belongs.
    let unplaced: Vec<usize> = (0..members.len()).filter(|at| !out.contains(at)).collect();
    out.extend(unplaced);
    out
}

/// Every package of this build, the entry first
/// ([ADR-053](../../docs/specification/adr/adr-053.md) D1).
///
/// Breadth-first from the entry, so a diamond is visited once and the order is
/// the same on every machine. A package already seen is the same crate: the key
/// under which it was reached is the *depending* crate's business (D2), and
/// nothing here is named by it.
pub fn members_of(entry_manifest: &Manifest, entry_root: &Path) -> Result<Vec<Member>> {
    let mut out: Vec<Member> = Vec::new();
    let mut seen: BTreeMap<PathBuf, usize> = BTreeMap::new();
    let mut queue: Vec<(Manifest, PathBuf)> = vec![(entry_manifest.clone(), canonical(entry_root))];

    while let Some((manifest, root)) = queue.first().cloned() {
        queue.remove(0);
        if seen.contains_key(&root) {
            continue;
        }

        let name = manifest
            .package_name()
            .ok_or_else(|| {
                refused!(
                    "{}/nikaia.toml has no `[package] name`, and Cargo needs one to \
                     name the crate (Part III 13.3)",
                    root.display()
                )
            })?
            .to_string();

        // A crate name is a package's identity in the generated workspace, and
        // two of them would be two answers to which crate the profile rows and
        // the renaming form below mean. The canonical path is the identity, so
        // this is only ever two *different* packages wanting one name.
        if let Some(first) = out.iter().find(|m| m.name == name) {
            refuse!(
                "two packages are both named `{name}`: {} and {}.\n\
                 Each package is generated as its own crate (ADR-053 D1) and a crate is \
                 named by its `[package] name`, so give one of them a name of its own - \
                 the key a `use` line writes is set by whoever depends on it and does not \
                 have to change.",
                first.root.display(),
                root.display()
            );
        }

        // Announced only for the entry: this walk reads every manifest of the
        // build, and a note about an ignored `[build]` is about the package
        // that carries it, said once.
        let dependencies = packages_of(&manifest, &root, out.is_empty())?;
        for dependency in &dependencies {
            let manifest = Manifest::read(&dependency.root.join("nikaia.toml"))?;
            queue.push((manifest, canonical(&dependency.root)));
        }

        seen.insert(root.clone(), out.len());
        out.push(Member {
            name,
            entry: crate_root_of(&root)?,
            root,
            manifest,
            dependencies,
        });
    }
    Ok(out)
}

/// `path`, resolved where the filesystem lets us. The same fallback
/// `packages_of` uses, so the two agree about when two keys are one package.
/// A Rust dependency's table with a relative `path` resolved against the
/// project root ([ADR-197](../../docs/specification/adr/adr-197.md) D2).
///
/// **Absolute, because the file this is written into is not where the author
/// is.** The generated member manifest lives at
/// `target/nikaia/build/<package>/Cargo.toml`
/// ([ADR-053](../../docs/specification/adr/adr-053.md) D1), and a path relative
/// to *that* is a number a person derives from a build layout. The same
/// manifest already writes `nikaia-std` absolute for the same reason.
///
/// A table with no `path` is handed back unchanged, which is every `version`
/// and `git` dependency there is.
fn rooted(value: &toml::Value, root: &Path) -> toml::Value {
    let Some(table) = value.as_table() else {
        return value.clone();
    };
    let Some(declared) = table.get("path").and_then(toml::Value::as_str) else {
        return value.clone();
    };
    let at = Path::new(declared);
    if at.is_absolute() {
        return value.clone();
    }
    let mut table = table.clone();
    table.insert(
        "path".to_string(),
        toml::Value::String(canonical(&root.join(at)).display().to_string()),
    );
    toml::Value::Table(table)
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The `.nika` file handed to Cargo as a package's crate root.
///
/// `src/main.nika` where there is one. A library has none - what a package
/// offers is its public surface (`modules::package_at`) - and Cargo still needs
/// a path that exists, so the first source beside it stands in. Which file it
/// is changes nothing: the wrapper walks up from it to the same `nikaia.toml`,
/// and `package_at` reads the whole directory either way.
fn crate_root_of(root: &Path) -> Result<PathBuf> {
    let entry = root.join(ENTRY);
    if entry.is_file() {
        return Ok(entry);
    }

    let src = root.join("src");
    let mut beside: Vec<PathBuf> = std::fs::read_dir(&src)
        .with_context(|| format!("cannot read {}", src.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == EXTENSION))
        .collect();
    beside.sort();

    beside.into_iter().next().ok_or_else(|| {
        refused!(
            "{} has no `.nika` sources.\n\
             A package is a directory with a `src/` in it (Part III 13.1), and a package \
             with nothing in it is nothing to build.",
            src.display()
        )
    })
}

pub fn lower(
    input: &Path,
    settings: &Settings,
    no_cache: bool,
    packages: &[modules::Dependency],
) -> Result<Lowered> {
    lower_reading(input, settings, no_cache, packages, None)
}

/// **What a build may read, and where it compiles a parser**
/// ([ADR-072](../../docs/specification/adr/adr-072.md) D1 and D2,
/// [ADR-177](../../docs/specification/adr/adr-177.md) D2).
///
/// **One function rather than two constructions**, because both of this
/// build's lowerings have to answer the same question. The second one is
/// `Project::report`'s, which rebuilds the source map when the backend had
/// something to say — and when it was handed `Reads::none()` instead, every
/// `comptime` that read a file or ran a grammar evaluated to nothing on that
/// pass, so the emitter refused an item the build had already lowered. What
/// reached the user was this compiler's own internal sentence in place of the
/// backend's message, which is the opposite of what `report` exists for.
///
/// A list that cannot be read fails the build on its own account: a flag that
/// names a file and is quietly treated as *no list* would turn a build that
/// meant to read into one that says a path is not named, which is the wrong
/// sentence about the wrong thing.
fn reads_for(layout: &Layout, allowlist: Option<&Path>) -> Result<assets::Reads> {
    let reads = match allowlist {
        Some(list) => assets::Reads::with(&layout.root, assets::Allowlist::read(list)?),
        None => assets::Reads::at(&layout.root),
    };
    // **Where a grammar's parser is compiled** (`open-work.md` §2.9). Beside
    // the cache rather than under the root, which is the rule that file already
    // keeps: a loose `.nika` outside a project has nothing written next to it.
    Ok(reads.building_in(layout.store.with_file_name("build-time")))
}

/// The same, told which allowlist is in effect
/// ([ADR-072](../../docs/specification/adr/adr-072.md) D2).
///
/// `None` is D1, and D1 is what every caller that does not pass the flag gets:
/// a build given no list reads nothing while it builds.
pub fn lower_reading(
    input: &Path,
    settings: &Settings,
    no_cache: bool,
    packages: &[modules::Dependency],
    allowlist: Option<&Path>,
) -> Result<Lowered> {
    // `Layout` decides where the lock and the store go, and guarantees that
    // outside a `nikaia.toml` project nothing is written into the source tree.
    // It also names the unit relative to its root: an absolute path is D7's
    // first failure direction, a key that moves with the checkout.
    let layout = Layout::resolve(input);
    let unit = layout.unit_name(input);
    // **What this build may read, resolved before anything is read**
    // ([ADR-072](../../docs/specification/adr/adr-072.md) D1, D2).
    let reads = reads_for(&layout, allowlist)?;
    let choices = match reads.list_digest() {
        Some(digest) => settings.choices().reading(digest),
        None => settings.choices(),
    };
    // **And what this build's boundaries say**
    // ([ADR-104](../../docs/specification/adr/adr-104.md) D5). A description is
    // a file a person edits, so an edit has to reach the next build - which it
    // did not, and failed open while it did not.
    let choices = choices.describing(crate::describe::descriptions_digest(&layout.root));

    // A cache that cannot be opened is a slower build, never a failed one
    // (D12).
    let mut cache = if no_cache {
        None
    } else {
        match Cache::open(
            &layout.lock,
            &layout.store,
            env!("NIKAIA_RUSTC_VERSION"),
            env!("NIKAIA_COMPILER"),
        ) {
            Ok(cache) => Some(cache),
            Err(error) => {
                eprintln!("warning: the build cache is unavailable: {error:#}");
                None
            }
        }
    };

    // Part I 9.1: a package is a directory, so a build is every `.nika` beside
    // the entry - and the cache cannot be asked about a program until the
    // program is known.
    //
    // **Outside a project it is the one file.** A package is a directory *of a
    // project*, which is what a `nikaia.toml` declares; a directory of loose
    // examples is a directory of programs, and compiling one of them must not
    // pull in the other ten (ADR-047 D1, `modules::collect_one`).
    let program = match (settings.tests, layout.in_project) {
        (true, in_project) => modules::Program::read_for_tests(input, packages, in_project)?,
        (false, true) => modules::Program::read_with(input, packages)?,
        (false, false) => modules::Program::read_one(input)?,
    };
    let key_source = program.sources().join("\n// --- unit ---\n");
    // **What the output names is part of the key** (0.0.239): the abort table
    // names each file as it was handed to the compiler (ADR-044 D1), so a
    // lowering cached for `/abs/one.nika` and reused for `one.nika` named the
    // first path in the second program's aborts. A choice, and not part of
    // the source, because the source's hash is the lockfile's.
    let choices = choices.naming(
        program
            .units
            .iter()
            .map(|unit| unit.path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let sources: Vec<PathBuf> = program.units.iter().map(|unit| unit.path.clone()).collect();

    // An entry that predates an artifact this build needs is a miss, not a
    // gap: adding an output stays a safe change.
    let cached = cache
        .as_ref()
        .and_then(|cache| cache.lookup(&unit, &key_source, &choices, &layout.root))
        .filter(|artifacts| artifacts.has_all(&[RUST, CONTRACTS]));
    let reused = cached.is_some();

    let (rust, ledger) = match &cached {
        Some(artifacts) => (
            artifacts.get(RUST).expect("checked above").to_string(),
            artifacts.get(CONTRACTS).expect("checked above").to_string(),
        ),
        None => {
            // Every unit is checked against the *program's* contracts, not its
            // own: `utils::double` is a name `main.nika` may write, and the
            // type checker resolves it in the one place any name is resolved.
            let modules = program.package_names();
            // **Read once per lowering and not per unit** (ADR-104 D1): the
            // boundary is the build's, and every unit of it stands at the same
            // one. `layout.root` is the project this input belongs to, and
            // outside a project there is no manifest and therefore nothing
            // declared.
            let foreign = match layout.in_project {
                true => Foreign::of(&layout.root),
                false => Foreign::default(),
            };
            // **The committed ledger against the one this build inferred**
            // ([ADR-101](../../docs/specification/adr/adr-101.md) D1), read
            // once for the package rather than once per unit: the question is
            // whether a *contract* moved, and a contract belongs to the
            // package.
            let newly = newly(&layout.root, &program.contracts);
            // Collected once for the whole package, because every unit is
            // checked against the same program (`ADR-030`) and a `comptime` in
            // any of them may call into any other.
            let beside: Vec<&crate::parser::Parsed> =
                program.units.iter().map(|unit| &unit.parsed).collect();
            let around = Around {
                foreign: &foreign,
                newly: &newly,
                beside: &beside,
                reads: &reads,
            };
            // **A dependency's unit is checked as its own package writes it**
            // ([`open-work.md`](../../docs/open-work.md) §1.9). The program's
            // contracts are the right ledger for the program's own files and the
            // wrong one for a package's: absorbing qualified its keys *and the
            // types inside them*, and the package's file writes the bare word.
            // `Program::as_its_own` is the ledger that package was inferred
            // with, kept rather than reconstructed.
            for unit in &program.units {
                let contracts = match &unit.package {
                    Some(package) => program
                        .as_its_own
                        .get(package)
                        .unwrap_or(&program.contracts),
                    None => &program.contracts,
                };
                check(
                    &unit.parsed,
                    contracts,
                    &modules,
                    around,
                    &unit.path,
                    &unit.source,
                    &settings.user_parallelism,
                )?;
            }

            let lowered = {
                // **The described crates' boundary reaches the emitter**
                // (ADR-237 D1), which the checker above already read it for.
                let mut program = program;
                program.described = foreign.descriptions.clone();
                let lowered = program.emit_reading(settings.build, &reads)?;
                (lowered, program)
            };
            let (lowered, program) = lowered;
            let ledger = program.contracts.render();

            // **An entry nothing read** ([ADR-072](../../docs/specification/adr/adr-072.md)
            // D8): a list that may hold names nothing uses decays into
            // *everything we ever needed*, which is how an allowlist stops
            // being read. Said at the end of the build, where the whole set is
            // known, and a warning rather than a refusal — the line is a
            // permission that is no longer used, not a program that is wrong.
            for entry in reads.unused() {
                eprintln!(
                    "warning: the allowlist names `{entry}` and nothing read it (ADR-072 D8)"
                );
            }

            if let Some(cache) = &mut cache {
                // **What the build actually read** (ADR-021 D13, ADR-072 D7).
                // An empty map is correct and usual: with no list in effect
                // nothing can be read at all.
                let artifacts = Artifacts::new()
                    .with(RUST, &lowered.rust)
                    .with(CONTRACTS, &ledger);
                let stored = cache
                    .record(&unit, &key_source, reads.taken(), &choices, &artifacts)
                    .and_then(|()| cache.save());
                if let Err(error) = stored {
                    // The outputs are in hand; only the next build is slower.
                    eprintln!("warning: the build cache could not be updated: {error:#}");
                }
            }
            (lowered.rust, ledger)
        }
    };

    Ok(Lowered {
        rust,
        ledger,
        sources,
        reused,
    })
}

/// The names a lowering stores its outputs under.
pub const RUST: &str = "rust";
pub const CONTRACTS: &str = "contracts";

/// Everything the compiler decides for itself, before it emits a line of Rust.
///
/// Two rules today, and one mechanism under both: the ledger (ADR-020) is what
/// makes either possible across the `std` boundary. A call to
/// `io::read_to_string` is a `sync` violation only if something says that
/// function can pause, and it takes the wrong number of arguments only if
/// something says how many it takes - `std.contracts` is where both are said.
///
/// Types are reported before suspension because a call that passes the wrong
/// thing is usually why the rest of the file reads strangely.
///
/// **What this build links against, and what has been described**
/// ([ADR-104](../../docs/specification/adr/adr-104.md) D1).
///
/// Two sets rather than one map, because the question is a difference: a crate
/// the manifest declares and no ledger describes is the one D1 refuses. Both
/// empty is a loose file with no project around it, where nothing was declared
/// and therefore nothing is undescribed.
///
/// **And the entries themselves**, which is D1's first sentence rather than an
/// addition to it: *every analysis reaches to the boundary and reads an entry
/// there*. `NK2504`'s own message promises a reader that four questions — what
/// may cross a thread, what the call may reach, whether it pauses, whether it
/// can fail — are answered by the file they are told to write. For two records
/// the file answered none of them: it was parsed to see whether it parsed, the
/// answer was thrown away, and its only effect was silencing the refusal that
/// asked for it. Measured on a three-line project, where a description saying
/// `() -> i64` left the call's result untyped and `n.no_such_method()` went
/// through.
#[derive(Debug, Default, Clone)]
pub struct Foreign {
    /// `[dependencies]` with `type = "rust"`, under the name a program writes
    /// (`manifest::foreign_crates`).
    pub declared: BTreeSet<String>,
    /// The crates a `contracts/<crate>.contracts` was found for, beside the
    /// project's own ledger (D5).
    pub described: BTreeSet<String>,
    /// The described crates whose **sources have moved** since the description
    /// was reviewed, with the files that hash differently
    /// ([ADR-104](../../docs/specification/adr/adr-104.md) D5, on
    /// [ADR-100](../../docs/specification/adr/adr-100.md) D3's rule).
    ///
    /// **Empty where nothing can be compared**, which is the polarity and not
    /// an omission: a version dependency's sources are in Cargo's registry
    /// cache and a description with no `[sources]` recorded nothing, so in both
    /// cases there is no hash to disagree with. A refusal resting on an absence
    /// is what [ADR-169](../../docs/specification/adr/adr-169.md) D1 keeps
    /// `NK2201` from doing, and it would here refuse every crate that comes
    /// from a registry — [Part III C.4](../../docs/specification/30-nikaia-tooling.md)'s
    /// correct program refused.
    pub moved: BTreeMap<String, Vec<String>>,
    /// Every described crate's entries, in one ledger under the qualified names
    /// a program writes (`hyper_shim::serve_once`).
    ///
    /// **One ledger and not one per crate**, because a name is resolved in one
    /// place (ADR-011 D2) and the crate a name belongs to is the prefix on it.
    /// Two descriptions cannot disagree about a name without disagreeing about
    /// whose it is.
    pub descriptions: Ledger,
}

impl Foreign {
    /// What a build of `root` is at its boundary.
    ///
    /// A manifest that cannot be read is **no declarations**, not a failure: the
    /// build has read it already and said so, and a second refusal from here
    /// would be the same mistake twice.
    pub fn of(root: &Path) -> Foreign {
        let declared = crate::manifest::Manifest::read(&root.join("nikaia.toml"))
            .map(|manifest| manifest.foreign_crates())
            .unwrap_or_default();
        let mut described = BTreeSet::new();
        let mut descriptions = Ledger::empty();
        let mut moved = BTreeMap::new();
        for name in &declared {
            let Some(ledger) = description_at(root, name) else {
                continue;
            };
            described.insert(name.clone());
            let differing = sources_that_moved(root, name, &ledger);
            if !differing.is_empty() {
                moved.insert(name.clone(), differing);
            }
            // `None`: a description's names are already qualified with the crate
            // word a program writes (`hyper_shim::serve_once`), so there is
            // nothing to put in front of them.
            descriptions.absorb(None, ledger);
        }
        Foreign {
            declared,
            described,
            descriptions,
            moved,
        }
    }

    /// **`std`'s ledger and every described crate's, in one**
    /// ([ADR-104](../../docs/specification/adr/adr-104.md) D1). A described
    /// boundary is a library boundary: the analyses ask a ledger what a callee
    /// takes, hands back, pauses on and keeps, and where the callee is foreign
    /// the answer comes from the file the reader was told to write.
    ///
    /// **`std` wins a collision**, and the reason it can be decided in one line
    /// is that there is nothing to decide yet: a description's names carry the
    /// crate word in front of them, `std`'s carry a module's, and no manifest in
    /// this repository declares a crate whose word is one of `std`'s modules.
    /// The day one does, that is a refusal to write and not a silence to keep —
    /// `docs/open-work.md` carries it.
    pub fn library(&self) -> Result<Ledger> {
        let mut library = Ledger::parse(STD).context("std's shipped ledger")?;
        for (name, contract) in &self.descriptions.functions {
            library
                .functions
                .entry(name.clone())
                .or_insert_with(|| contract.clone());
        }
        for (name, contract) in &self.descriptions.types {
            library
                .types
                .entry(name.clone())
                .or_insert_with(|| contract.clone());
        }
        Ok(library)
    }

    /// The words a program may write in front of a `::` **because the manifest
    /// declared them**, added to the ones its own packages give it.
    ///
    /// Without this the merge above would make `hyper_shim` a **`std` module**,
    /// since that set is derived from the library ledger's own key prefixes —
    /// and the first thing a program calling one would be told is to write
    /// `use std::hyper_shim`, which is not a sentence about anything. A crate is
    /// reached because the manifest declares it (D1), not because a file
    /// imported it.
    pub fn packages(&self, modules: &BTreeSet<String>) -> BTreeSet<String> {
        let mut out = modules.clone();
        out.extend(self.described.iter().cloned());
        out
    }
}

/// Which of the files a description was derived from hash differently now
/// ([ADR-104](../../docs/specification/adr/adr-104.md) D5).
///
/// **Only what can be compared is compared.** A description that recorded no
/// `[sources]` and a crate whose sources this build cannot find both answer
/// *nothing moved*, because there is no hash to disagree with — the honest
/// answer, and the one that keeps a registry crate from being refused for
/// coming from a registry (Part III, C.4).
fn sources_that_moved(root: &Path, name: &str, ledger: &Ledger) -> Vec<String> {
    if ledger.sources.is_empty() {
        return Vec::new();
    }
    let Some(crate_root) = crate::describe::crate_root(root, name) else {
        return Vec::new();
    };
    ledger
        .sources
        .iter()
        .filter(|(file, recorded)| {
            // **A file that cannot be read is not a file that moved.** A
            // checkout without the path dependency beside it is a build that
            // will fail for its own reasons, and a second message about it
            // would be this compiler guessing at the first.
            std::fs::read(crate_root.join(file))
                .is_ok_and(|bytes| orchestrator::cache::sha256_hex(&bytes) != **recorded)
        })
        .map(|(file, _)| file.clone())
        .collect()
}

/// `contracts/<crate>.contracts`, where it is there **and parses as a ledger**.
///
/// Parsing is the test rather than existence, for
/// [ADR-100](../../docs/specification/adr/adr-100.md) D3's reason one file over:
/// a file that does not parse is not an answer, and treating it as one would
/// let a boundary be described by something nobody can read.
fn description_at(root: &Path, name: &str) -> Option<Ledger> {
    let path = root.join("contracts").join(format!("{name}.contracts"));
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| Ledger::parse(&text).ok())
}

/// `user_parallelism` reaches this and reaches **nothing inside the analyses**.
/// The verdict of every rule is a property of the program, so no switch may
/// change it (ADR-005 §1 Group B); what the switch does change is whether a
/// refusal is about *this* build, and that is a question about severity. See
/// [`lint_where_nothing_crosses`].
/// **What a build knows that one unit does not**, handed to the check together.
///
/// Two facts, and both are about the *build* rather than about the file being
/// checked — what the manifest links against and what was described beside it
/// ([ADR-104](../../docs/specification/adr/adr-104.md) D1), and what a callee
/// has newly gained since the ledger committed beside it
/// ([ADR-101](../../docs/specification/adr/adr-101.md) D1). Neither can be read
/// from a `Parsed`, and neither belongs to it.
///
/// **One argument and not two**, because they arrive together and for the same
/// reason: the day a third fact about the build is needed, it goes here rather
/// than onto a signature that is already long enough to be read wrong.
#[derive(Debug, Clone, Copy)]
pub struct Around<'a> {
    pub foreign: &'a Foreign,
    pub newly: &'a check::Newly,
    /// **What this build may read while it builds**
    /// ([ADR-072](../../docs/specification/adr/adr-072.md)).
    ///
    /// The fourth fact, and it belongs beside the other three for the reason
    /// the doc above gives: it is about the *build* rather than about a file,
    /// no ledger can carry it, and it binds the whole build — a dependency's
    /// `asset("…")` is checked against the list of the build that is running,
    /// never against anything the dependency ships (D6).
    pub reads: &'a crate::assets::Reads,
    /// **Every file of the program being built.**
    ///
    /// The third fact this carries, and the one that had to be a `Parsed`
    /// rather than a column: a `comptime` calling across a file boundary needs
    /// the callee's **body**, and a ledger records what a caller has to know
    /// about a function it cannot see the body of. Each file owns the interner
    /// its symbols resolve in, so the AST travels rather than the items.
    pub beside: &'a [&'a crate::parser::Parsed],
}

pub fn check(
    parsed: &crate::parser::Parsed,
    own: &Ledger,
    modules: &BTreeSet<String>,
    around: Around<'_>,
    path: &Path,
    source: &str,
    user_parallelism: &str,
) -> Result<()> {
    let Around {
        foreign,
        newly,
        beside,
        reads,
    } = around;
    let library = foreign.library()?;

    // A described crate is a package by the spelling rule, which is what
    // `modules` is: a set of words that appear in front of a `::` and are not
    // `std`'s ([`Foreign::packages`]).
    let modules = foreign.packages(modules);
    let mut all =
        check::check_against(parsed, beside, own, &library, &modules, newly, reads).findings;
    // A separate walk, for the reason the three inside `check_program` are
    // separate: it asks about the **boundary** of the build rather than about a
    // type, and it needs the manifest rather than a ledger.
    all.extend(crate::foreign::check(
        parsed,
        &foreign.declared,
        &foreign.described,
        &foreign.moved,
    ));
    all.sort_by_key(|finding| finding.span.start);
    lint_where_nothing_crosses(&mut all, user_parallelism);
    let violations = sync::check(parsed, own, &library);
    if all.is_empty() && violations.is_empty() {
        return Ok(());
    }

    // A warning is printed and does not stop anything. There is one, and it is
    // a migration (ADR-035 D5): a string written before `f"…"` existed looks
    // exactly like one that meant its braces, and neither refusing it nor
    // saying nothing would be right.
    let path = path.display().to_string();
    for finding in &all {
        eprint!("{}", diagnostics::render_finding(finding, &path, source));
    }
    let findings: Vec<&check::Finding> = all
        .iter()
        .filter(|f| f.severity == check::Severity::Error)
        .collect();
    if findings.is_empty() && violations.is_empty() {
        return Ok(());
    }
    for violation in &violations {
        eprint!(
            "{}",
            diagnostics::render_sync_violation(violation, &path, source)
        );
    }

    let mut refused = Vec::new();
    // One line per family, because a family is what a reader can act on in one
    // go. The `NK1xxx` codes are types; `NK23xx` is a view kept past its call;
    // `NK25xx` is a value on the wrong thread; `NK2605` and `NK2701` are one
    // rule counted together, because they *are* one rule (ADR-025 D1) - a call
    // that can fail, written or not, in a function that does not declare
    // `throws`.
    let count = |family: &str| {
        findings
            .iter()
            .filter(|f| f.code.starts_with(family))
            .count()
    };
    let plural = |n: usize| if n == 1 { "" } else { "s" };

    let types = count("NK1");
    if types > 0 {
        refused.push(format!("{types} type error{}", plural(types)));
    }
    // **`NK2503` is counted apart from the other `NK25xx` codes**
    // ([ADR-039](../../docs/specification/adr/adr-039.md) D6), for the reason
    // every split in this tally has: it must say what it counted. The others
    // are a *value* on the wrong thread; this one is a *call* that can reach a
    // lock, and the way out is keeping the lock out of its reach rather than
    // anything about the value.
    let reaching = count("NK2503");
    if reaching > 0 {
        refused.push(format!(
            "{reaching} call{} that can reach a lock",
            plural(reaching)
        ));
    }
    // **And `NK2504` is not a crossing at all**, which the tally said it was:
    // it is a crate nobody described, and *a value that may not cross a thread*
    // is a sentence about no part of it. Same rule as the split above — the
    // tally has to say what it counted — measured on a project whose only
    // refusal was an undescribed crate.
    let undescribed = count("NK2504");
    if undescribed > 0 {
        refused.push(format!(
            "{undescribed} Rust crate{} nothing describes",
            plural(undescribed)
        ));
    }
    // **And `NK2505` is a third thing again**: a crate that *is* described, by
    // a file about a version of it that is no longer there
    // ([ADR-104](../../docs/specification/adr/adr-104.md) D5). What a reader
    // does about it is neither of the two above.
    let stale = count("NK2505");
    if stale > 0 {
        refused.push(format!(
            "{stale} description{} of a crate that has moved",
            plural(stale)
        ));
    }
    let crossings = count("NK25") - reaching - undescribed - stale;
    if crossings > 0 {
        refused.push(format!(
            "{crossings} value{} that may not cross a thread",
            plural(crossings)
        ));
    }
    // `NK23xx` is aliasing: who else points at the thing this one is about.
    // Counted on its own because the way out is a different one - a shape to
    // change rather than a declaration to add.
    let aliases = count("NK2302");
    if aliases > 0 {
        refused.push(format!(
            "{aliases} view{} kept past the call that was given it",
            plural(aliases)
        ));
    }
    // **The tether's own line** ([ADR-156](../../docs/specification/adr/adr-156.md)
    // D4). Counted apart from `NK2302` although both are `NK23xx`, because the
    // tally has to say what it counted and these are two different shapes: one
    // is a view given to a call and kept, the other a view of a buffer the body
    // made and handed back.
    let tethered = count("NK2303");
    if tethered > 0 {
        refused.push(format!(
            "{tethered} view{} that would keep a buffer alive without a declaration saying so",
            plural(tethered)
        ));
    }
    // **What a tether cannot be lowered to** (ADR-209), a line of its own for
    // the same reason: the way out is a shape to change, not a word to add.
    let untethered = count("NK2304");
    if untethered > 0 {
        refused.push(format!(
            "{untethered} buffer{} whose views have no one place to live",
            plural(untethered)
        ));
    }
    // `NK21xx` is running things at once, and its two rules want two lines: what
    // a `spawn` took with it is a value to clone, and an `overlap` whose
    // branches meet is a block to take apart. Counted apart from the line below
    // for the reason they are counted at all - the tally has to say what it
    // counted, and "a place that can fail without saying so" is neither of them.
    let moved = count("NK2101");
    if moved > 0 {
        refused.push(format!(
            "{moved} value{} a task took with it",
            plural(moved)
        ));
    }
    let together = count("NK2104");
    if together > 0 {
        refused.push(format!(
            "{together} branch{} that cannot run beside the others",
            if together == 1 { "" } else { "es" }
        ));
    }
    // **A name a parallel lambda changes** (ADR-235 D2): the way out is a
    // `map` and a `collect`, or a lock, which is neither line above.
    let raced = count("NK2107");
    if raced > 0 {
        refused.push(format!(
            "{raced} name{} changed from a lambda that runs on several cores",
            plural(raced)
        ));
    }
    let tasks = count("NK21");
    // **A sequence walked twice is not a place that can fail**
    // ([ADR-105](../../docs/specification/adr/adr-105.md) D2), so it does not
    // land in the line below: the way out is a `collect()` and not a `throws`,
    // which is the same reason the two `NK21xx` rules have lines of their own.
    let walked = count("NK2702");
    if walked > 0 {
        refused.push(format!(
            "{walked} sequence{} walked a second time",
            plural(walked)
        ));
    }
    // **`NK2202` is counted with the other `sync` violations and not below**,
    // wherever it came from. The free call is `contracts::sync`'s and the
    // method call is the type checker's
    // ([ADR-149](../../docs/specification/adr/adr-149.md) D2), because only one
    // of the two knows what `tx.send(1)` goes to — but they are one rule, and a
    // tally that has to say what it counted must not call one of them *a place
    // that can fail without saying so*.
    let pausing = count("NK2202");
    // **A pause inside a door is the same code and another sentence**
    // ([ADR-233](../../docs/specification/adr/adr-233.md) D3): it is no `sync`
    // function's, and the way out is moving the call out of the block.
    let paused_in_a_door = findings
        .iter()
        .filter(|f| f.code == "NK2202" && f.message.ends_with("with a lock held"))
        .count();
    if paused_in_a_door > 0 {
        refused.push(format!(
            "{paused_in_a_door} pause{} with a lock held",
            plural(paused_in_a_door)
        ));
    }
    let pausing = pausing - paused_in_a_door;
    // **A pause where nothing may pause** - a grammar's action, a fold, a
    // parallel lambda (ADR-142 D1, ADR-235 D2) - is not a place that can fail.
    let pauses_forbidden = count("NK2209");
    if pauses_forbidden > 0 {
        refused.push(format!(
            "{pauses_forbidden} call{} that can pause where nothing may",
            plural(pauses_forbidden)
        ));
    }
    // **A file read with a lock held is not a place that can fail either**
    // ([ADR-169](../../docs/specification/adr/adr-169.md) D2), and lands on the
    // same reasoning as the two above: the tally has to say what it counted,
    // and the way out here is moving the read rather than declaring anything.
    let under_a_lock = count("NK2201");
    if under_a_lock > 0 {
        refused.push(format!(
            "{under_a_lock} read{} of a file with a lock held",
            plural(under_a_lock)
        ));
    }
    let rules = findings.len()
        - types
        - crossings
        - reaching
        - undescribed
        - stale
        - aliases
        - tethered
        - untethered
        - tasks
        - walked
        - pausing
        - paused_in_a_door
        - pauses_forbidden
        - under_a_lock;
    if rules > 0 {
        refused.push(format!(
            "{rules} place{} that can fail without saying so",
            plural(rules)
        ));
    }
    let may_not = violations.len() + pausing;
    if may_not > 0 {
        refused.push(format!(
            "{may_not} call{} a `sync` function may not make",
            if may_not == 1 { "" } else { "s" }
        ));
    }
    // A refusal and not a failure of this compiler, so it leaves without a
    // backtrace: the diagnostics above have already said everything, and this is
    // the tally (`diagnostics::Refused`).
    Err(diagnostics::refuse(refused.join(", ")))
}

/// The `NK25xx` codes whose crossing this build does not perform, reported as a
/// lint rather than as an error.
///
/// Part III C.3 says the `Send` rules are "reported at `user_parallelism = no`
/// as a lint, so a library built there stays usable at `yes`", and the reason is
/// exact: at `no` nothing the program wrote runs concurrently (ADR-037 D2), so
/// the emitter writes no `task::both` and no task - the crossing `NK2501` is
/// about does not happen in *this* build. Refusing it would be refusing a
/// program that compiles, which is the one thing the compiler may never do; and
/// saying nothing would be ADR-005 §1 Group B's failure exactly, a library built
/// at `no` that turns out un-compilable at `yes`. A lint is the third answer, and
/// it is the one that record asked for.
///
/// **`NK2502` is not downgraded, and that is the decision.** A Rust dependency
/// may bring its own runtime (ADR-038 D7), and its threads are not the
/// program's: `user_parallelism` bounds what *you* wrote, which is ADR-037 D2's
/// load-bearing "user". So that crossing is real at both settings and so is the
/// refusal.
///
/// This is the only place a build switch meets a `NK25xx`, and it meets the
/// **severity** rather than the verdict. The verdict is a property of the
/// program and the analyses never see a switch.
fn lint_where_nothing_crosses(findings: &mut [check::Finding], user_parallelism: &str) {
    if user_parallelism != "no" {
        return;
    }
    for finding in findings.iter_mut().filter(|f| f.code == "NK2501") {
        finding.severity = check::Severity::Warning;
        finding.notes.push(
            "`user_parallelism = no` runs nothing you wrote concurrently, so this build does \
             not perform the crossing - it is reported so that the same source still builds at \
             `yes` (Part III, C.3)"
                .to_string(),
        );
    }
}

/// **What each function has newly gained in its `throws` set** since the
/// ledger committed beside it ([ADR-101](../../docs/specification/adr/adr-101.md)
/// D1).
///
/// The ledger records `throws` as a **set** ([ADR-023](../../docs/specification/adr/adr-023.md)
/// D1), so a callee that gains a failure is a set that gained a member — and
/// every `catch` over that callee now receives something it did not when it was
/// written. D1 asks for a line naming each of them; this is the half that
/// answers *which errors*.
///
/// **Three things answer *nothing gained*, and each is right rather than
/// cheap.** A build with no committed ledger beside it has nothing to compare,
/// and a first build is not a change. A ledger that does not parse is not an
/// answer (`ADR-100` D3's rule one file over). And a function the committed
/// ledger does not name is **new**, so nothing was ever written against its
/// set — a new function's whole set is not a set that grew.
fn newly(root: &Path, inferred: &Ledger) -> check::Newly {
    let Some(committed) = std::fs::read_to_string(root.join("nikaia.contracts"))
        .ok()
        .and_then(|text| Ledger::parse(&text).ok())
    else {
        return check::Newly::new();
    };
    let mut newly: check::Newly = inferred
        .functions
        .iter()
        .filter_map(|(name, contract)| {
            let before = committed.functions.get(name)?;
            let gained: Vec<String> = contract
                .throws
                .iter()
                .filter(|error| !before.throws.contains(error))
                .cloned()
                .collect();
            (!gained.is_empty()).then(|| (name.clone(), gained))
        })
        .collect();
    newly.keeps = keeps_that_moved(&committed, inferred);
    newly
}

/// **The parameters whose `keeps` changed**
/// ([ADR-236](../../docs/specification/adr/adr-236.md) D1): kept now and not
/// before, or kept before and only read now. A parameter the old signature
/// does not have is **new** rather than moved, as a new function's set is not
/// a set that grew. Whether the type's teardown does anything is the checker's
/// question, which has both ledgers.
fn keeps_that_moved(committed: &Ledger, inferred: &Ledger) -> BTreeMap<String, Vec<check::Kept>> {
    inferred
        .functions
        .iter()
        .filter_map(|(name, contract)| {
            let before = committed.functions.get(name)?;
            let (now, then) = (contract.signature.as_ref()?, before.signature.as_ref()?);
            let moved: Vec<check::Kept> = now
                .arguments()
                .iter()
                .filter(|(param, _)| then.arguments().iter().any(|(p, _)| p == param))
                .filter_map(|(param, ty)| {
                    let kept = contract.keeps.contains(param);
                    (kept != before.keeps.contains(param)).then(|| check::Kept {
                        param: param.clone(),
                        now: kept,
                        ty: ty.text(),
                    })
                })
                .collect();
            (!moved.is_empty()).then(|| (name.clone(), moved))
        })
        .collect()
}

/// Write the ledger, or - under `--locked` - check that it did not need
/// writing.
///
/// The determinism guarantee (13.5) is what lets this compare bytes rather than
/// meanings: the same sources and the same compiler produce the same file, so a
/// difference is a change in a contract and never in the formatting.
pub fn write_ledger(path: &Path, ledger: &str, locked: bool) -> Result<()> {
    if !locked {
        std::fs::write(path, ledger)?;
        return Ok(());
    }

    let committed = std::fs::read_to_string(path).with_context(|| {
        format!(
            "--locked, but {} is not there; run without --locked to write it",
            path.display()
        )
    })?;

    if committed != *ledger {
        refuse!(
            "--locked: the contracts changed and {} does not say so.\n\
             What the build inferred and the ledger does not have:\n{}\n\
             Run without --locked to record it, and read the diff.",
            path.display(),
            changed_lines(ledger, &committed).join("\n")
        );
    }

    Ok(())
}

/// The lines the build produced that the committed ledger does not have, each
/// under the entry it belongs to.
///
/// Naming the entry is the whole point. A contract line is short and repeats -
/// `sync = true` says nothing on its own, and since ADR-027 it is the commonest
/// line in the file - so a bare list of them tells you that *something* changed
/// and leaves you to find out what. 13.5 asks this diff to narrate a cause; it
/// cannot do that without saying whose contract moved.
pub fn changed_lines(ledger: &str, committed: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut entry: Option<&str> = None;
    let mut named: Option<&str> = None;

    for line in ledger.lines() {
        if line.starts_with('[') {
            entry = Some(line);
            named = None;
        }
        if line.is_empty() || line.starts_with('#') || committed.lines().any(|c| c == line) {
            continue;
        }
        match entry.filter(|e| *e != line) {
            // A line inside an entry, under the entry's name - printed once,
            // however many of its lines changed.
            Some(entry) => {
                if named != Some(entry) {
                    out.push(format!("  {entry}"));
                    named = Some(entry);
                }
                out.push(format!("      {line}"));
            }
            // The line *is* the entry - a whole contract that is new - or it is
            // a header line, which belongs to no entry. Either way it names
            // itself, and an entry that has named itself must not be named
            // again by the lines that follow it.
            None => {
                out.push(format!("  {line}"));
                named = entry;
            }
        }
    }

    out
}

/// Which of the three explanations a run was asked for
/// ([ADR-033](../../docs/specification/adr/adr-033.md) D9,
/// [ADR-037](../../docs/specification/adr/adr-037.md) D8,
/// [ADR-010](../../docs/specification/adr/adr-010.md) D7).
///
/// They exist because these analyses **cannot be asked for** - there is no way to
/// request the cheaper reference count, the overlap or the faster hash; every
/// fallback is enumerated instead. `--sharing`'s own help says why that is only
/// fair if the fallbacks can be asked about, *"and this is the asking"*.
///
/// So they have to reach a real program, and a person with a real program builds
/// it with `nikaia build`. They were on the single-file path only, which is
/// exactly where the asking is not done.
#[derive(Debug, Clone, Copy, Default)]
pub struct Explain {
    pub overlaps: bool,
    pub sharing: bool,
    /// [ADR-008](../../docs/specification/adr/adr-008.md) D6's inverse tool:
    /// inspection rather than assertion.
    pub tethers: bool,
    pub trust: bool,
    /// [ADR-088](../../docs/specification/adr/adr-088.md) D6's report: what a
    /// `T::fields` loop was unrolled to, for the types actually used.
    pub comptime: bool,
    /// [ADR-245](../../docs/specification/adr/adr-245.md) D6's report: every
    /// `assert`, and whether it was proved, refuted or left to run time.
    pub asserts: bool,
}

impl Explain {
    pub fn asked(&self) -> bool {
        self.overlaps
            || self.sharing
            || self.tethers
            || self.trust
            || self.comptime
            || self.asserts
    }
}

/// Print the explanations a run asked for, over every file of the package.
///
/// **Against the package's own ledger and not each file's**, which is the
/// difference from the single-file path doing this three times: `sync`, the touch
/// sets and the sharing classes are whole-program facts (ADR-027, ADR-033), and a
/// report built from one file's inferences would answer a different question from
/// the one the build answers.
///
/// The file name is printed where there is more than one, because a slot key
/// (`zaehle::counts`) does not say which file it is in - one namespace or not.
pub fn explain(program: &modules::Program, settings: &Settings, want: Explain) -> Result<()> {
    if !want.asked() {
        return Ok(());
    }
    let library = Ledger::parse(STD).context("std's shipped ledger")?;
    let several = program.units.len() > 1;

    // **Once for the program and not once per file**
    // ([ADR-181](../../docs/specification/adr/adr-181.md) D5), which is the
    // difference from the four reports below: an unrolling is a fact about a
    // **call**, and the call may stand in a different file from the function it
    // names. Printing it per unit would print each one as many times as the
    // program has files.
    if want.comptime {
        let beside: Vec<&crate::parser::Parsed> =
            program.units.iter().map(|unit| &unit.parsed).collect();
        // **`Reads::none()`**, because what is reported is which types a
        // function was **used with**, and no file a build reads changes that.
        // Handing it a real allowlist would run a grammar to answer a question
        // about call sites ([ADR-177](../../docs/specification/adr/adr-177.md)).
        print!(
            "{}",
            crate::check::unrolling_report(
                &beside,
                &program.contracts,
                &crate::assets::Reads::none(),
            )
        );
    }

    for unit in &program.units {
        if several {
            println!("--- {}", unit.path.display());
        }
        if want.overlaps {
            print!(
                "{}",
                crate::contracts::order::overlap_report(
                    &unit.parsed,
                    &program.contracts,
                    &library,
                    &crate::emit::branch_starts_first(
                        &unit.parsed,
                        settings.build,
                        &program.contracts,
                    ),
                )
            );
        }
        if want.sharing {
            print!(
                "{}",
                crate::contracts::sharing::report(
                    &unit.parsed,
                    &program.contracts,
                    &library,
                    settings.build.user_parallelism == crate::emit::UserParallelism::Yes,
                )
            );
        }
        if want.tethers {
            print!(
                "{}",
                crate::contracts::tether::report(&unit.parsed, &program.contracts)
            );
        }
        if want.asserts {
            print!(
                "{}",
                crate::check::claims_report(
                    &unit.parsed,
                    &unit.source,
                    &unit.path.display().to_string(),
                    &program.contracts,
                    &library,
                )
            );
        }
        if want.trust {
            print!(
                "{}",
                crate::contracts::trust::render(
                    &crate::contracts::trust::analyse(&unit.parsed, &library),
                    &unit.path.display().to_string(),
                    &unit.source,
                )
            );
        }
    }
    Ok(())
}

/// A project: its root, its manifest, and the switches this build resolved.
#[derive(Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub settings: Settings,
}

impl Project {
    /// Find the project governing `start` and read its manifest.
    ///
    /// The walk to the root is `Layout::resolve`'s - the same one the build
    /// cache uses - so a build and the cache entries it writes can never
    /// disagree about which project a file belongs to.
    pub fn open(
        start: &Path,
        target: Option<&str>,
        user_parallelism: Option<&str>,
    ) -> Result<Project> {
        // `Layout::resolve` searches from an input file's *parent*, so it is
        // handed the manifest's own path: the search then starts at `start`
        // itself, which is what "build the project I am standing in" means.
        let layout = Layout::resolve(&start.join("nikaia.toml"));
        if !layout.in_project {
            refuse!(
                "no `nikaia.toml` in {} or any directory above it.\n\
                 A project build needs a manifest (Part III 13.1); a single file \
                 is compiled with `nikaia --input {}`.",
                start.display(),
                start.join("main.nika").display()
            );
        }

        let manifest = Manifest::read(&layout.root.join("nikaia.toml"))?;
        // ADR-038 D5 moved `cleanup-deadline` out of the manifest. The key is
        // still accepted, and the note is what keeps the move from being
        // silent - a manifest whose setting stopped being read without saying
        // so is the mistake `[build]`'s unknown-key check already exists for.
        for note in manifest.notes() {
            eprintln!("note: {note}");
        }
        let settings = Settings::resolve(&manifest, target, user_parallelism)?;
        if let Some(missing) = settings.build.target.unbuildable() {
            refuse!(
                "cannot build for `{}` yet: {missing}",
                settings.build.target.triple()
            );
        }

        Ok(Project {
            root: layout.root,
            manifest,
            settings,
        })
    }

    /// `src/main.nika` (Part III 13.1).
    pub fn entry(&self) -> PathBuf {
        self.root.join(ENTRY)
    }

    /// Where the generated `Cargo.toml` goes. Under `target/`, because it is
    /// build output: it is regenerated from `nikaia.toml` on every build and
    /// editing it would be editing something the next build overwrites.
    pub fn build_dir(&self) -> PathBuf {
        self.root.join("target").join("nikaia").join("build")
    }

    /// Where the emitted Rust goes - **outside** [`Self::build_dir`], and that
    /// is not tidiness. Cargo lists the files of a path package to decide
    /// whether it is fresh, so a generated source inside the package would make
    /// the package dirty the moment it was written, and every build would
    /// recompile.
    pub fn gen_dir(&self) -> PathBuf {
        self.root.join("target").join("nikaia").join("gen")
    }

    /// **The switches, as a file Cargo can see** (ADR-021 D5).
    ///
    /// A build switch is not a source, so nothing Cargo watches changes when
    /// one does — and the lowering happens inside the `rustc` wrapper, which
    /// Cargo runs only for a package it thinks is stale. So flipping
    /// `user-parallelism` in `nikaia.toml` left the package fresh and the
    /// binary at the **old** setting, silently: the cache had the switch as a
    /// dimension (D5) and nothing ever got as far as asking it.
    ///
    /// This is the third member of the same family as the two freshness traps
    /// the project build already carries, and it takes the same answer: give
    /// Cargo a file. The driver writes the resolved switches here before
    /// `cargo` runs, and the wrapper puts it into the dependency file beside
    /// the `.nika` sources — so a switch that changed is an input that changed.
    ///
    /// Under `target/` and outside [`Self::build_dir`], for [`Self::gen_dir`]'s
    /// reason: a file *inside* the path package would make it dirty on every
    /// build rather than on a changed one.
    pub fn switches_path(&self) -> PathBuf {
        self.root
            .join("target")
            .join("nikaia")
            .join("gen")
            .join("switches")
    }

    /// The ledger, in the project root (Part III 13.5).
    pub fn ledger_path(&self) -> PathBuf {
        self.root.join("nikaia.contracts")
    }

    /// The packages this project depends on by path - see [`packages_of`].
    pub fn packages(&self) -> Result<Vec<modules::Dependency>> {
        packages_of(&self.manifest, &self.root, true)
    }

    /// Every package of this build, the entry first - see [`members_of`].
    pub fn members(&self) -> Result<Vec<Member>> {
        members_of(&self.manifest, &self.root)
    }

    /// One member's `nikaia.toml` translated (ADR-002 D1).
    ///
    /// `rust` is that package as this build lowered it, and it decides which of
    /// the compiler's own runtime crates the generated crate declares - a
    /// package that names none of them depends on nothing and builds in a
    /// second.
    ///
    /// `kind` is what puts the entry package's `fn main` in a binary and every
    /// package under it in a library beside it
    /// ([ADR-053](../../docs/specification/adr/adr-053.md) D1).
    fn cargo_member(&self, member: &Member, rust: &str, kind: CrateKind) -> Result<CargoProject> {
        let mut dependencies = runtime_dependencies(rust, self.settings.build.reentrancy_check)?;
        for (dependency, value) in member.manifest.dependencies() {
            match value {
                // The whole point of D1: whatever the author wrote reaches
                // Cargo, and Cargo resolves and links it as it would for any
                // Rust project - **narrowed by one key**
                // ([ADR-197](../../docs/specification/adr/adr-197.md) D2). A
                // relative `path` is resolved against `nikaia.toml` and written
                // absolute; `version`, `git`, `tag`, `features` and the rest
                // still travel untouched.
                Dependency::Rust(value) => {
                    dependencies.insert(dependency.clone(), rooted(value, &member.root));
                }
                // **Cargo's renaming form** (ADR-053 D2): the package is named
                // by its identity, the key is the name *this* crate uses for
                // it. So two packages that both want the key `c` are in two
                // different crates and never meet, and one package reached
                // through two parents is one crate - which is ADR-047 D2 rule 3
                // holding because Cargo already works that way.
                Dependency::Path(path) => {
                    let root = canonical(&member.root.join(path));
                    let named = self.crate_named(&root)?;
                    let mut table = toml::value::Table::new();
                    table.insert("package".to_string(), toml::Value::String(named.clone()));
                    // Beside this member, under the generated build directory:
                    // every crate of the build is written there, one directory
                    // each, named by the same name.
                    table.insert(
                        "path".to_string(),
                        toml::Value::String(format!("../{named}")),
                    );
                    dependencies.insert(dependency.clone(), toml::Value::Table(table));
                }
                // A registry, a name space and a distribution format are all
                // undecided, and guessing at one here would be inventing the
                // answer in the place it is hardest to review
                // ([ADR-002](../../docs/specification/adr/adr-002.md) D1 §5).
                Dependency::Nikaia(_) => refuse!(
                    "`{dependency}` is a Nikaia package named by a version, and a version \
                     is not how one is found: no ADR names a registry, a version grammar \
                     or a distribution format.\n\
                     A Nikaia package is depended on **by path** today - write it as \
                     `{dependency} = {{ path = \"../{dependency}\" }}` (ADR-047 D2) - and a \
                     crate from crates.io as \
                     `{dependency} = {{ type = \"rust\", version = \"…\" }}` \
                     (Part III 13.3, ADR-002 D1)."
                ),
            }
        }

        Ok(CargoProject {
            package: Package {
                name: member.name.clone(),
                version: member.manifest.package_version().to_string(),
                // What the emitter writes, and what the tests compile it as.
                edition: "2024".to_string(),
                // ADR-109 D4: the floor the lowering needs, from one constant
                // in the emitter.
                rust_version: Some(crate::emit::RUST_FLOOR.to_string()),
            },
            kind,
            // A `[lib] name` has to be an identifier; a package name does not.
            // Cargo does this same replacement for a lib it names itself.
            bin_name: match kind {
                CrateKind::Bin => member.name.clone(),
                CrateKind::Lib => member.name.replace('-', "_"),
            },
            bin_path: member.entry.clone(),
            features: runtime_features(&dependencies),
            dependencies,
        })
    }

    /// The generated workspace: one crate per Nikaia package
    /// ([ADR-053](../../docs/specification/adr/adr-053.md) D1).
    ///
    /// `rust` is each member as this build lowered it, in `members`' order.
    pub fn cargo_workspace(&self, members: &[Member], rust: &[String]) -> Result<Workspace> {
        let mut out = Vec::new();
        for (at, member) in members.iter().enumerate() {
            // The entry package is the program; everything it reaches is a
            // library beside it.
            let kind = match at {
                0 => CrateKind::Bin,
                _ => CrateKind::Lib,
            };
            let rust = rust.get(at).map(String::as_str).unwrap_or("");
            out.push((member.name.clone(), self.cargo_member(member, rust, kind)?));
        }

        let codegen = self.manifest.codegen_for(&self.settings.target);
        Ok(Workspace {
            members: out,
            // One profile, because a Nikaia build has no dev/release division
            // to map onto: `[build.<target>]` is the project's single statement
            // about output size and speed, and `dev` is the profile Cargo uses
            // when nothing asks otherwise.
            profile_name: "dev".to_string(),
            profile: Profile {
                opt_level: codegen.get("opt-level").cloned(),
                lto: codegen.get("lto").cloned(),
                panic: Some(self.settings.panic_strategy().to_string()),
            },
        })
    }

    /// What the package at `root` is called as a crate - its own `[package]
    /// name`, which is what every other crate of the build names it by (D2).
    fn crate_named(&self, root: &Path) -> Result<String> {
        let manifest = Manifest::read(&root.join("nikaia.toml"))?;
        Ok(manifest
            .package_name()
            .ok_or_else(|| {
                refused!(
                    "{}/nikaia.toml has no `[package] name`, and Cargo needs one to \
                     name the crate (Part III 13.3)",
                    root.display()
                )
            })?
            .to_string())
    }

    /// What this build asked the code generator for, as the compiled-`std` cache
    /// keys it (ADR-002 D4).
    ///
    /// The same two sources the Cargo profile above is written from - the
    /// machine's `[build.<target>]` table and the panic strategy that follows
    /// from the machine - because it is exactly those that decide what `std`'s
    /// machine code looks like.
    fn codegen(&self) -> Codegen {
        Codegen::new(
            &self.manifest.codegen_for(&self.settings.target),
            self.settings.panic_strategy(),
        )
    }

    /// Build the project, or build and run it.
    ///
    /// The lowering happens twice and that is deliberate: **here**, so the
    /// checks and the ledger are the compiler's own output rather than
    /// something buried in a `cargo` subprocess, and again inside the wrapper,
    /// where it is what `rustc` is actually handed. Both go through [`lower`]
    /// and the second is a cache hit, so the work is done once even though the
    /// decision is made in two places.
    #[allow(clippy::too_many_arguments)]
    pub fn drive(
        &self,
        subcommand: &str,
        program_args: &[String],
        no_cache: bool,
        locked: bool,
        want: Explain,
        // **The list this build reads under** (ADR-072 D2), or `None` for D1.
        allowlist: Option<&Path>,
    ) -> Result<i32> {
        self.drive_to(subcommand, program_args, no_cache, locked, want, allowlist)
            .map(|(code, _)| code)
    }

    /// [`Project::drive`], handing back the binary the build made as well -
    /// which is what `nikaia test` runs, once per test.
    pub fn drive_to(
        &self,
        subcommand: &str,
        program_args: &[String],
        no_cache: bool,
        locked: bool,
        want: Explain,
        allowlist: Option<&Path>,
    ) -> Result<(i32, Option<PathBuf>)> {
        let entry = self.entry();
        if !entry.is_file() {
            refuse!(
                "{} is not there. A project's entry point is `{ENTRY}` (Part III 13.1).",
                entry.display()
            );
        }

        // Before the build, because an explanation is about the program and a
        // build that reuses a cached lowering still answers the question. The
        // package is read a second time here rather than threaded out of
        // `lower`: reading it is parsing, and paying for it only when somebody
        // asked is cheaper than reshaping the build for a flag nobody usually
        // passes.
        if want.asked() {
            let program = modules::Program::read_with(&entry, &self.packages()?)?;
            explain(&program, &self.settings, want)?;
        }

        // Every package of this build, resolved before anything is read: a
        // dependency that is not there, or two names for one path, is a
        // statement about the project and should not wait for a lowering.
        let members = self.members()?;

        // **Every member is lowered here**, for the reason the doc comment
        // above gives for the entry: the checks and the diagnostics are the
        // compiler's own output rather than something buried in a `cargo`
        // subprocess. Each is lowered again inside the wrapper and each of
        // those is a cache hit, so a package is compiled once however many
        // crates now ask about it.
        //
        // **Dependencies first** ([ADR-100](../../docs/specification/adr/adr-100.md)
        // D5). A package's ledger is a build input of everything that depends
        // on it: `http`'s own build has `deeper` in view and a consumer's does
        // not (ADR-053 D3), so the consumer has to read the answer `http`
        // computed rather than derive a worse one. `members_of` is
        // breadth-first from the entry, which is the wrong end and is not a
        // topological order once a package is reached two ways — so the order
        // is computed here, and this loop is where the two builds of one
        // package stop being able to disagree.
        let order = dependencies_first(&members);
        let mut rust: Vec<Option<String>> = vec![None; members.len()];
        for at in order {
            let member = &members[at];
            let lowered = lower_reading(
                &member.entry,
                &self.settings,
                no_cache,
                &member.dependencies,
                allowlist,
            )?;
            // **Each package's ledger in that package's own root**
            // (Part III 13.5, ADR-100 D1): written here so the consumers
            // lowered after it read it, and committed with the package the way
            // a lockfile is. Under `--locked` this is D4 — each is compared
            // byte for byte rather than rewritten, which is the check CI wants
            // and the one a development build must not pay for.
            write_ledger(
                &member.root.join("nikaia.contracts"),
                &lowered.ledger,
                locked,
            )?;
            rust[at] = Some(lowered.rust);
        }
        // Back into `members_of`'s own order, which is what the generated
        // workspace is written in.
        let rust: Vec<String> = rust
            .into_iter()
            .map(|r| r.expect("every member is lowered"))
            .collect();

        // **Before `cargo`**, and written only when it differs, so an
        // unchanged switch does not dirty the package every build. What is in
        // it is what the cache keys on, so the two cannot disagree about which
        // switches decide an artifact.
        let switches = self.switches_path();
        if let Some(dir) = switches.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let choices = self.settings.choices();
        write_if_changed(
            &switches,
            &format!("{}\n{}\n", choices.build, choices.backend),
        )?;

        // **Before Cargo is handed anything** (ADR-109 D4): its own answer
        // about the floor names a package the author never wrote, which is
        // Part III C.1's class.
        toolchain_is_new_enough(crate::emit::RUST_FLOOR)?;
        // **The machine named is the machine built for** (ADR-037 D1): the
        // triple goes to Cargo as `--target`, at the host too, so there is one
        // path and not a second one that only a cross build takes. Where it is
        // not the host, the toolchain may not have its `std`, and Cargo's own
        // answer is `can't find crate for core` - so that is asked first.
        let target = self.settings.build.target;
        if Some(target) != Target::host() {
            toolchain_can_build_for(target.triple())?;
        }
        let target_args = ["--target".to_string(), target.triple().to_string()];

        let manifest = self
            .cargo_workspace(&members, &rust)?
            .write_to(&self.build_dir())?;

        let mut env = self.settings.as_env();
        env.push((WRAPPER_MARKER.to_string(), OsString::from("1")));
        env.push((GEN_DIR_VAR.to_string(), self.gen_dir().into_os_string()));
        if no_cache {
            env.push((NO_CACHE_VAR.to_string(), OsString::from("1")));
        }
        // **Absolute**, because the wrapper runs in a directory Cargo chose.
        if let Some(list) = allowlist {
            let list = list
                .canonicalize()
                .with_context(|| format!("reading the allowlist `{}`", list.display()))?;
            env.push((READS_VAR.to_string(), list.into_os_string()));
        }

        let cargo = Cargo {
            manifest,
            // **The compiled-`std` cache** (ADR-002 D4). Cargo builds into a
            // directory in the user's cache named by `Key::sysroot`, so the
            // second project on this machine links the `std` the first one built
            // instead of compiling it and everything under it again. It cannot
            // live under this project's `target/`, because "shared between
            // projects" is the whole of what it is for.
            //
            // `CARGO_TARGET_DIR` still wins where it is set. It is Cargo's own
            // documented override and a build that asked for a directory has to
            // get it.
            target_dir: match std::env::var_os("CARGO_TARGET_DIR") {
                Some(_) => None,
                None => Some(Sysroot::resolve().rlib_cache(
                    &self.settings.target,
                    &self.codegen(),
                    &self.settings.reentrancy_check,
                )),
            },
            wrapper: std::env::current_exe().context("finding this compiler's own path")?,
            env,
        };

        // **`build` first, always, and its diagnostics read rather than
        // relayed** (ADR-005 D7, Part III C.1). Cargo's stderr is Rust about a
        // file the author never opened; `--message-format=json` is the channel
        // the translator can read, and it is only usable for `build` - a `run`
        // writes the *program's* output to the same stdout.
        //
        // So a `nikaia run` is a build and then a run, and the run is a no-op
        // rebuild. That is the same shape the lowering already has (above): the
        // work happens once, and the decision is made where it can be reported.
        let (mut code, messages) = cargo.messages("build", &target_args)?;
        self.report(&messages, allowlist)?;
        let binary = executable_in(&messages);
        if code == 0 && subcommand == "run" {
            // **The built program is run directly, and that is Part III C.1
            // rather than a shortcut.** `cargo run` is a second invocation that
            // cannot use the JSON channel - the program's own output is on that
            // stdout - so Cargo renders its **cached** diagnostics to stderr
            // while it checks freshness, and a program with a warning saw it
            // twice: once in this language's words from `report` above, and
            // once as `rustc` about `target/nikaia/gen/….rs`. Measured on
            // Part I 2.3's own example.
            //
            // **The path is read and not computed.** Cargo decides where a
            // binary lands from the target directory, the profile and
            // `CARGO_TARGET_DIR`, and replicating those rules here would be a
            // second place that has to agree with Cargo forever. The build
            // above already captured the answer: a `compiler-artifact` line
            // whose `executable` is not null.
            //
            // Where there is no such line the old path is taken, so this can
            // only remove a duplicate and never lose a run.
            code = match executable_in(&messages) {
                Some(binary) => run_directly(&binary, program_args)?,
                None => cargo.run("run", &target_args, program_args)?,
            };
        }

        // Cargo has resolved by now, and only now: the versions do not exist
        // before it ran. A build that failed resolved nothing worth recording.
        if code == 0
            && let Err(error) = self.record_resolved_dependencies(no_cache)
        {
            // D12: the record costs the *next* reader some information. It
            // never costs this build, which has already succeeded.
            eprintln!(
                "warning: the resolved dependency versions could not be recorded \
                     in nikaia.lock: {error:#}"
            );
        }
        Ok((code, binary))
    }

    /// Say what the backend said, against the `.nika` line it was about.
    ///
    /// Part III C.1's Iron Rule: an untranslated backend error reaching the user
    /// is a bug in this compiler. Until this existed the project build had no
    /// interception at all - `cargo`'s stderr *was* the user interface - so every
    /// class ADR-005 D7 enumerates, `E0277` among them since the structural
    /// `Send` check landed, arrived as Rust about `target/nikaia/gen/….rs`.
    ///
    /// **The position is translated and the text is not**, which is ADR-012's
    /// own choice and is honest for most of what arrives: the lowering is name
    /// for name (ADR-011 D2), so "`xs` does not live long enough" is already a
    /// sentence about the Nikaia source and only its place was wrong. It is
    /// *not* honest for a trait-bound error, whose every noun is a Rust type the
    /// author did not write - and rewriting one into Nikaia vocabulary is a
    /// second question, recorded in ADR-005 D7 rather than answered here. What
    /// the frontend can decide about a crossing it now refuses itself, with
    /// `NK2501`/`NK2502` and Nikaia words.
    ///
    /// The map is rebuilt rather than carried: the lowering is deterministic, so
    /// a second one gives the same map (ADR-012), and this way the cost is paid
    /// only by a build the backend had something to say about.
    fn report(&self, messages: &str, allowlist: Option<&Path>) -> Result<()> {
        if messages
            .lines()
            .all(|line| !line.contains("\"compiler-message\""))
        {
            return Ok(());
        }

        // **With the packages**, or a `use` line would be read against an empty
        // set and the translation of somebody else's error would be a refusal of
        // a program that is fine.
        // **The program the build lowered**, test build included (ADR-245
        // D1): a test build's entry has its dispatcher, and a map rebuilt
        // from the other program places a warning in the wrong file.
        let mut program = match self.settings.tests {
            true => modules::Program::read_for_tests(&self.entry(), &self.packages()?, true)?,
            false => modules::Program::read_with(&self.entry(), &self.packages()?)?,
        };
        // **The same reads the build lowered under** (ADR-072, ADR-177). This
        // lowering has to be the lowering, or an item the build wrote a `const`
        // for is refused here and the backend's message never arrives.
        let layout = Layout::resolve(&self.entry());
        let reads = reads_for(&layout, allowlist)?;
        // …and the same descriptions (ADR-237 D1), for the same reason.
        let foreign = match layout.in_project {
            true => Foreign::of(&layout.root),
            false => Foreign::default(),
        };
        program.described = foreign.descriptions.clone();
        let lowered = program.emit_reading(self.settings.build, &reads)?;
        let sources: Vec<&str> = program.sources();
        let paths: Vec<String> = program
            .units
            .iter()
            .map(|unit| unit.path.display().to_string())
            .collect();
        let generated = self.gen_dir().display().to_string();

        let mut translated: Vec<_> = diagnostics::translate_units(messages, &lowered.map, &sources)
            .into_iter()
            .filter(diagnostics::is_about_the_program)
            .collect();
        // **A boundary mismatch is said as a stale ledger** (ADR-100 D6,
        // ADR-237 D2): the packages this program depends on and the crates it
        // describes are where a ledger the lowering believed can disagree with
        // the code below it.
        let boundaries: Vec<diagnostics::Boundary> = self
            .packages()?
            .into_iter()
            .map(|package| diagnostics::Boundary {
                word: package.name,
                described: false,
                moved: Vec::new(),
            })
            .chain(foreign.described.iter().map(|word| diagnostics::Boundary {
                word: word.clone(),
                described: true,
                moved: foreign.moved.get(word).cloned().unwrap_or_default(),
            }))
            .collect();
        for diagnostic in &mut translated {
            let at = diagnostic.location.as_ref().map_or(0, |l| l.unit);
            diagnostics::a_stale_boundary(diagnostic, sources[at], &boundaries);
        }

        for diagnostic in &translated {
            let at = diagnostic.location.as_ref().map_or(0, |l| l.unit);
            eprint!(
                "{}",
                diagnostics::render(diagnostic, &paths[at], sources[at], &generated)
            );
        }

        // **The backend's own words, kept** (ADR-056 D2). Shown above because
        // the reader is being told this is not their mistake, and written here
        // because a bug report is made later and from a file.
        if let Some(log) = diagnostics::log_internal(&translated, &self.gen_dir()) {
            eprintln!("note: the backend's own messages are in {}", log.display());
        }
        Ok(())
    }

    /// Copy the versions Cargo chose into `nikaia.lock` (ADR-021 D2).
    ///
    /// The toolchain does not resolve versions and must not (ADR-002 D1), so
    /// this reads Cargo's `Cargo.lock` for the generated package rather than
    /// computing anything - D4's distinction exactly: `nikaia.toml` declares
    /// `regex = { type = "rust", version = "1.5" }`, and this records the
    /// `1.13.1` that turned into.
    ///
    /// Two conditions, and neither is tidiness. **`--no-cache` writes nothing**,
    /// because `nikaia.lock` is the cache's own record and a run told not to
    /// keep one must not leave a file behind. **A lockfile that is not there is
    /// not created**, because a file holding resolved versions and no unit
    /// hashes would answer D2's question with half an answer while looking like
    /// a whole one.
    ///
    /// Nothing is written when the versions are unchanged, which is the common
    /// case: a committed file must not turn up as modified after a build that
    /// changed nothing about it.
    fn record_resolved_dependencies(&self, no_cache: bool) -> Result<()> {
        if no_cache {
            return Ok(());
        }
        // The same walk the cache and `Project::open` use, so the three can
        // never disagree about where the record lives.
        let lock_path = Layout::resolve(&self.root.join("nikaia.toml")).lock;
        if !lock_path.is_file() {
            return Ok(());
        }

        let name = self
            .manifest
            .package_name()
            .ok_or_else(|| refused!("the project has no `[package] name`"))?;
        let resolved = resolved_versions(&self.build_dir().join("Cargo.lock"), name)?;

        let mut lock = Lockfile::load(
            &lock_path,
            env!("NIKAIA_RUSTC_VERSION"),
            env!("NIKAIA_COMPILER"),
        )?;
        if lock.dependencies == resolved {
            return Ok(());
        }
        lock.dependencies = resolved;
        lock.save(&lock_path)
    }
}

/// What this particular program links against, out of the three crates the
/// emitter can name.
///
/// Declared because the *emitted Rust* names them, not because a program might:
/// a program with no grammar in it depends on nothing at all and builds without
/// fetching anything, and a manifest that declared three crates regardless would
/// make every project pay for the ones it does not use.
///
/// `std` is reached **by path into the sysroot** (ADR-002 D4). It is not
/// published, so there is no registry to reach it through; what changed is that
/// the path is a sysroot's rather than a checkout's, and that the sources there
/// need nothing but `rustc` to build.
///
/// The other two versions are baked into this binary by `build.rs` rather than
/// read from a workspace manifest at run time. A sysroot is not a Cargo
/// workspace and has no `[workspace.dependencies]` above it - and these are the
/// *compiler's* versions anyway, since it is the emitter that writes
/// `winnow::Parser` into a program. Read at the compiler's build time rather
/// than written down twice: a program that linked a different `winnow` from the
/// one `winnow-grammar` was built against is a type error at every `Stream`
/// bound, so the two must not be able to drift apart.
/// The program Cargo built, as its own build said where it was.
///
/// **Read rather than computed.** Cargo decides where a binary lands from the
/// target directory, the profile and `CARGO_TARGET_DIR`; a second place that
/// worked those rules out would have to keep agreeing with Cargo forever. A
/// `--message-format=json` build says it outright, on the `compiler-artifact`
/// line whose `executable` is not null - and this compiler already captures that
/// output to read the diagnostics.
///
/// `None` where nothing says, which is not an error: the caller falls back to
/// `cargo run` and the build behaves as it did before.
fn executable_in(messages: &str) -> Option<std::path::PathBuf> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|value| value["reason"] == "compiler-artifact")
        .filter_map(|value| value["executable"].as_str().map(std::path::PathBuf::from))
        .next_back()
}

/// **What one test did, at one setting** ([ADR-245](../../docs/specification/adr/adr-245.md) D1).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    passed: bool,
    stdout: String,
    stderr: String,
}

/// **An output test** ([ADR-245](../../docs/specification/adr/adr-245.md) D8):
/// `tests/NAME.stdout` in the package, and beside it, optionally, what the
/// program is given - `NAME.stdin`, and `NAME.args` with one argument a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputTest {
    pub name: String,
    pub expected: Vec<u8>,
    pub stdin: Vec<u8>,
    pub args: Vec<String>,
}

/// The package's output tests, by name. A `tests/` that is not there holds
/// none; a file that is there and cannot be read is an error, not a test that
/// quietly has no input.
pub fn output_tests(root: &Path) -> Result<Vec<OutputTest>> {
    let dir = root.join("tests");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|x| x == "stdout"))
        .filter_map(|path| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    let optional = |path: PathBuf| -> Result<Option<Vec<u8>>> {
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    };
    names
        .into_iter()
        .map(|name| {
            let expected = std::fs::read(dir.join(format!("{name}.stdout")))
                .with_context(|| format!("reading tests/{name}.stdout"))?;
            let stdin = optional(dir.join(format!("{name}.stdin")))?.unwrap_or_default();
            let args = optional(dir.join(format!("{name}.args")))?
                .map(|bytes| {
                    String::from_utf8_lossy(&bytes)
                        .lines()
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Ok(OutputTest {
                name,
                expected,
                stdin,
                args,
            })
        })
        .collect()
}

/// Run a built program with its arguments and its standard input.
fn outcome_of(binary: &Path, args: &[String], stdin: &[u8]) -> Result<std::process::Output> {
    use std::io::Write;
    let mut child = std::process::Command::new(binary)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("running {}", binary.display()))?;
    if let Some(mut input) = child.stdin.take() {
        // A program that stops reading early closes the pipe, which is its
        // business and not a failure of the run.
        let _ = input.write_all(stdin);
    }
    child
        .wait_with_output()
        .with_context(|| format!("running {}", binary.display()))
}

/// What `nikaia test` runs: the package's `test` blocks, against the test
/// build, and its output tests, against the program - each built once per
/// setting asked for.
pub struct Suite<'a> {
    pub tests: &'a [modules::TestCase],
    /// One test build per setting, where there are blocks.
    pub test_builds: &'a [(String, PathBuf)],
    pub outputs: &'a [OutputTest],
    /// One build of the program per setting, where there are output tests.
    pub programs: &'a [(String, PathBuf)],
    pub root: &'a Path,
}

/// **Run each test in a process of its own and say how each went**
/// ([ADR-245](../../docs/specification/adr/adr-245.md) D1, D7, D8).
///
/// A `test` block fails on an error that leaves it, a false `assert` or a
/// `panic` - each of which ends its process unsuccessfully. An output test
/// fails where the program does not end successfully or prints anything but
/// its `.stdout`, byte for byte. At two settings either fails as well where the
/// two outcomes differ, because the meaning of a program is the same at both
/// (Part I 1.2) and a difference is a fault of this compiler. A failing test
/// does not stop the others.
pub fn run_tests(suite: &Suite<'_>) -> Result<i32> {
    let total = suite.tests.len() + suite.outputs.len();
    let plural = if total == 1 { "" } else { "s" };
    println!("running {total} test{plural}");
    // (what it is called, what each setting did)
    let mut failed: Vec<(String, Vec<(String, Outcome)>, Option<&OutputTest>)> = Vec::new();
    let mut report = |label: String,
                      outcomes: Vec<(String, Outcome)>,
                      output: Option<&OutputTest>| {
        let agree = outcomes.windows(2).all(|pair| pair[0].1 == pair[1].1);
        let passed = agree && outcomes.iter().all(|(_, o)| o.passed);
        let verdict = match (passed, agree) {
            (true, _) => "ok",
            (false, true) => "FAILED",
            (false, false) => "FAILED (the settings disagree)",
        };
        println!("{label} ... {verdict}");
        if !passed {
            failed.push((label, outcomes, output));
        }
    };
    for (k, test) in suite.tests.iter().enumerate() {
        let mut outcomes = Vec::new();
        for (setting, binary) in suite.test_builds {
            let out = outcome_of(binary, &[k.to_string()], &[])?;
            outcomes.push((
                setting.clone(),
                Outcome {
                    passed: out.status.success(),
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                },
            ));
        }
        let label = format!(
            "test \"{}\" ({}:{})",
            test.title,
            shown_path(&test.path, suite.root),
            test.line
        );
        report(label, outcomes, None);
    }
    for output in suite.outputs {
        let mut outcomes = Vec::new();
        for (setting, binary) in suite.programs {
            let out = outcome_of(binary, &output.args, &output.stdin)?;
            outcomes.push((
                setting.clone(),
                Outcome {
                    passed: out.status.success() && out.stdout == output.expected,
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                },
            ));
        }
        let label = format!("output \"{}\" (tests/{}.stdout)", output.name, output.name);
        report(label, outcomes, Some(output));
    }
    if !failed.is_empty() {
        println!("\nfailures:");
        for (label, outcomes, output) in &failed {
            println!("\n--- {label}");
            let agree = outcomes.windows(2).all(|pair| pair[0].1 == pair[1].1);
            let shown = match agree {
                true => &outcomes[..1],
                false => &outcomes[..],
            };
            for (setting, outcome) in shown {
                if !agree {
                    println!("at user-parallelism = {setting}:");
                }
                match output {
                    Some(output) => {
                        println!(
                            "expected:\n{}\nprinted:\n{}",
                            String::from_utf8_lossy(&output.expected),
                            outcome.stdout
                        );
                    }
                    None => print!("{}", outcome.stdout),
                }
                eprint!("{}", outcome.stderr);
            }
        }
    }
    let passed = total - failed.len();
    println!(
        "\ntest result: {}. {passed} passed; {} failed",
        if failed.is_empty() { "ok" } else { "FAILED" },
        failed.len()
    );
    Ok(if failed.is_empty() { 0 } else { 1 })
}

/// A test's file as the project names it.
fn shown_path(path: &Path, root: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    path.strip_prefix(&root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

/// Run it, with the program's own arguments and every stream its own.
///
/// Nothing is captured and nothing is translated: what a running program writes
/// is the program's, and the last thing a compiler should do is stand between
/// the two.
fn run_directly(binary: &std::path::Path, args: &[String]) -> Result<i32> {
    let status = std::process::Command::new(binary)
        .args(args)
        .status()
        .with_context(|| format!("running {}", binary.display()))?;
    // A program killed by a signal has no exit code. `1` is what a shell would
    // report for one, and the alternative - claiming success - is the one answer
    // that must not be given.
    Ok(status.code().unwrap_or(1))
}

/// The same, for a program this compiler wrote for its own use — the parser a
/// grammar run compiles ([`crate::grammar_run`]).
///
/// **The same table a program gets**, rendered: a parser compiled against a
/// different `winnow` from the one the program links is a type error at every
/// `Stream`, and the point of compiling the generated parser at all is that it
/// *is* the program's parser.
pub fn runtime_dependencies_for(rust: &str) -> Vec<(String, String)> {
    runtime_dependencies(rust, crate::emit::ReentrancyCheck::default())
        .map(|table| {
            table
                .into_iter()
                .map(|(name, value)| (format!("{name:?}"), value.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// **The features the generated code tests in its own crate.** A `grammar!`
/// expansion writes `#[cfg(feature = "trace")]` into the crate that invokes it,
/// so that crate is the one that has to declare `trace`, or every build of a
/// program with a `grammar` carries `rustc`'s *unexpected `cfg` condition
/// value* (0.0.230, `open-work.md` §1.20 before it). It forwards to `winnow-grammar`'s
/// own `trace`, as `nikaia` and `nikaia-std` do, so turning it on traces the
/// parser end to end.
fn runtime_features(dependencies: &BTreeMap<String, toml::Value>) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    if dependencies.contains_key("winnow-grammar") {
        out.insert(
            "trace".to_string(),
            vec!["winnow-grammar/trace".to_string()],
        );
    }
    out
}

fn runtime_dependencies(
    rust: &str,
    reentrancy_check: crate::emit::ReentrancyCheck,
) -> Result<BTreeMap<String, toml::Value>> {
    let mut out = BTreeMap::new();

    if rust.contains("nikaia_std") {
        let mut table = toml::Table::new();
        table.insert(
            "path".to_string(),
            toml::Value::String(Sysroot::resolve().std_dir().to_string_lossy().into_owned()),
        );
        // **A declined guarantee reaches the crate that carries it**
        // ([ADR-039](../../docs/specification/adr/adr-039.md) D8). The check
        // lives in `std`, so the switch is a Cargo feature: on by default, and
        // a program that declined it builds `std` without it. The compiled-`std`
        // cache keys on the word too (`Sysroot::rlib_cache`), or the second
        // project on this machine would link the first one's answer.
        if !reentrancy_check.is_on() {
            table.insert("default-features".to_string(), toml::Value::Boolean(false));
        }
        out.insert("nikaia-std".to_string(), toml::Value::Table(table));
    }

    // `winnow_grammar` contains `winnow`, so a grammar pulls in both - which is
    // right: the expansion writes `winnow::Parser` as well.
    for (crate_name, declaration) in [
        ("winnow-grammar", env!("NIKAIA_RUNTIME_WINNOW_GRAMMAR")),
        ("winnow", env!("NIKAIA_RUNTIME_WINNOW")),
    ] {
        if !rust.contains(&crate_name.replace('-', "_")) {
            continue;
        }
        let value: toml::Value = toml::from_str(&format!("value = {declaration}"))
            .map(|table: toml::Table| table["value"].clone())
            .with_context(|| {
                format!("the `{crate_name}` declaration this compiler was built with")
            })?;
        out.insert(crate_name.to_string(), value);
    }

    Ok(out)
}

/// The wrapper: what Cargo runs in `rustc`'s place, for workspace members.
///
/// Cargo calls this as `nikaia <rustc> <args…>`, for its own probes as well as
/// for real compiles, so the first thing it does is decide whether this
/// invocation has anything to do with Nikaia at all. Everything else - every
/// crate from crates.io - is passed to the real `rustc` untouched, which is
/// exactly what `RUSTC_WORKSPACE_WRAPPER` already guarantees and what this
/// keeps true for a workspace member that happens to be Rust.
pub fn wrapper_main() -> Result<i32> {
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let mut invocation = Invocation::parse(&argv, EXTENSION)
        .ok_or_else(|| anyhow!("{WRAPPER_MARKER} is set but no `rustc` was named"))?;

    let Some(source) = invocation.source_path() else {
        trace(&invocation, "passed through");
        return invocation.run();
    };
    trace(&invocation, "lowered");

    let settings = Settings::from_env()?;
    // **The packages, found the same way the project build finds them.** Cargo
    // hands this a `.nika` path, so the manifest is the one above it
    // (`Manifest::find` walks to the same root the cache does) - and it has to be
    // read here rather than passed down, because the only channel from the build
    // into the wrapper is the environment and a dependency list is not a switch.
    // `packages_of` is the one resolution rule, so the two cannot disagree about
    // what the program is.
    let manifest = Manifest::find(&source)?;
    let packages = match manifest.root() {
        Some(root) => packages_of(&manifest, root, false)?,
        None => Vec::new(),
    };
    let allowlist = std::env::var_os(READS_VAR).map(PathBuf::from);
    let lowered = lower_reading(
        &source,
        &settings,
        std::env::var_os(NO_CACHE_VAR).is_some(),
        &packages,
        allowlist.as_deref(),
    )?;

    let name = invocation.crate_name.clone().unwrap_or_else(|| {
        source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "main".to_string())
    });
    let gen_dir = match std::env::var_os(GEN_DIR_VAR) {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("nikaia-gen"),
    };
    // **One directory per crate**, and that is not tidiness. Every generated
    // crate root used to sit beside every other, so `rustc` refusing a name from
    // a package this crate does not depend on offered to fix it with
    // `mod deeper` - a file it had found next door, and advice that means
    // nothing here. A crate root with no siblings cannot be offered any.
    let generated = gen_dir.join(&name).join(format!("{name}.rs"));
    write_if_changed(&generated, &lowered.rust)?;

    invocation.replace_source(&generated);
    let code = invocation.run()?;

    // Cargo believes the dependency file `rustc` wrote, and `rustc` only saw
    // the generated Rust. Without this an edit to a `.nika` source leaves the
    // package fresh and the binary stale.
    //
    // **And the switches, for the same reason one step further out**
    // (`Project::switches_path`): a build switch is not a source, so nothing
    // Cargo watches changes when one does — and this wrapper is only run for a
    // package Cargo already thinks is stale. Without the file in here, flipping
    // `user-parallelism` left the program running at the setting it was built
    // at, and said nothing.
    if code == 0
        && let Some(dep_info) = invocation.dep_info()
    {
        let mut inputs = lowered.sources.clone();
        if let Some(switches) = switches_beside(&gen_dir) {
            inputs.push(switches);
        }
        record_extra_dependencies(&dep_info, &inputs)?;
    }
    Ok(code)
}

/// The switches file the driver wrote, where there is one.
///
/// The wrapper is told the generated-source directory and nothing else
/// (`GEN_DIR_VAR` is the one channel), and the file sits in it — so this is a
/// lookup rather than a second answer to where it lives. Absent means the
/// wrapper was run by hand rather than by a project build, and then there is no
/// switch anybody could flip between two runs.
fn switches_beside(gen_dir: &Path) -> Option<PathBuf> {
    let path = gen_dir.join("switches");
    path.is_file().then_some(path)
}

/// Record one invocation, if anybody asked. A trace that cannot be written is
/// silent: this is a debugging facility, and a build must not fail because the
/// place it was told to write a note to is unwritable.
fn trace(invocation: &Invocation, verdict: &str) {
    use std::io::Write;

    let Some(path) = std::env::var_os(TRACE_VAR) else {
        return;
    };
    let crate_name = invocation.crate_name.as_deref().unwrap_or("-");
    // **And whether this one is Cargo's target-info probe**
    // ([ADR-166](../../docs/specification/adr/adr-166.md) D1), because that is
    // the invocation whose standard input decides whether a build starts at all
    // — and a trace that cannot say so cannot answer the question the trace
    // exists for.
    let verdict = match invocation.is_a_probe() {
        true => &format!("{verdict} (target-info probe)"),
        false => verdict,
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        writeln!(file, "{crate_name} {verdict}").ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--locked` names the entry a changed line belongs to.
    ///
    /// A contract line is short and repeats; since ADR-027 `sync` is the
    /// commonest line in the file. Reporting three bare `sync = true` lines
    /// tells a reader that something moved and leaves them to find out what,
    /// which is not the narrated diff 13.5 asks for.
    #[test]
    fn a_locked_diff_says_whose_contract_moved() {
        let committed = "version = 1\n\n[fn.\"a\"]\nsync = true\n\n[fn.\"b\"]\n";
        let built = "version = 1\n\n[fn.\"a\"]\nsync = true\n\n[fn.\"b\"]\nsync = \"inferred\"\n";

        assert_eq!(
            changed_lines(built, committed),
            ["  [fn.\"b\"]", "      sync = \"inferred\""]
        );
    }

    /// An entry is named once, however many of its lines changed.
    #[test]
    fn an_entry_is_named_once_for_all_of_its_changes() {
        let committed = "version = 1\n\n[fn.\"a\"]\n";
        let built = "version = 1\n\n[fn.\"a\"]\nsync = \"inferred\"\nthrows = true\n";

        assert_eq!(
            changed_lines(built, committed),
            [
                "  [fn.\"a\"]",
                "      sync = \"inferred\"",
                "      throws = true"
            ]
        );
    }

    /// A wholly new entry names itself, and is not named twice.
    #[test]
    fn a_new_entry_names_itself_once() {
        let committed = "version = 1\n";
        let built = "version = 1\n\n[fn.\"z\"]\nsync = \"inferred\"\nthrows = true\n";

        assert_eq!(
            changed_lines(built, committed),
            [
                "  [fn.\"z\"]",
                "      sync = \"inferred\"",
                "      throws = true"
            ]
        );
    }

    /// A header line belongs to no entry and is reported on its own - which is
    /// what a toolchain upgrade changing the inference looks like.
    #[test]
    fn a_changed_header_is_reported_without_an_entry() {
        let committed = "version = 1\ninference = \"stage0-signatures\"\n";
        let built = "version = 1\ninference = \"stage0-signatures+sync-bodies\"\n";

        assert_eq!(
            changed_lines(built, committed),
            ["  inference = \"stage0-signatures+sync-bodies\""]
        );
    }

    /// The runtime the emitted Rust names: `std` by path into the sysroot, the
    /// other two from the declarations `build.rs` baked in. Written down a second
    /// time, the copies would drift and a program would link two `winnow`s.
    #[test]
    fn the_runtime_travels_with_the_compiler() {
        let runtime = runtime_dependencies(
            "use winnow_grammar::grammar; use nikaia_std::prelude;",
            crate::emit::ReentrancyCheck::Yes,
        )
        .expect("the declarations this compiler was built with parse");
        assert!(runtime["nikaia-std"].get("path").is_some());
        assert!(runtime.contains_key("winnow-grammar"));
        assert!(
            runtime.contains_key("winnow"),
            "a grammar expansion writes `winnow::Parser` too"
        );
    }

    /// A `grammar!` expansion tests `feature = "trace"` in the crate that
    /// invokes it, so that crate declares it (0.0.230, `open-work.md` §1.20 before it), forwarding
    /// to `winnow-grammar`'s. A program without a grammar declares no feature.
    #[test]
    fn a_program_with_a_grammar_declares_trace() {
        let with = runtime_dependencies(
            "use winnow_grammar::grammar;",
            crate::emit::ReentrancyCheck::Yes,
        )
        .expect("resolves");
        assert_eq!(
            runtime_features(&with)["trace"],
            ["winnow-grammar/trace".to_string()]
        );
        let without = runtime_dependencies(
            "use nikaia_std::prelude;",
            crate::emit::ReentrancyCheck::Yes,
        )
        .expect("resolves");
        assert!(runtime_features(&without).is_empty());
    }

    /// A program that names none of them declares none of them, and fetches
    /// nothing at all.
    #[test]
    fn a_program_that_names_no_runtime_depends_on_nothing() {
        let runtime = runtime_dependencies(
            "fn main() { println!(\"hi\"); }",
            crate::emit::ReentrancyCheck::Yes,
        )
        .expect("resolves");
        assert!(runtime.is_empty(), "{runtime:?}");
    }

    /// ADR-037 D1: the machine decides what a panic does, and that reaches the
    /// Cargo profile (ADR-002 D1).
    #[test]
    fn the_machine_decides_the_panic_strategy() {
        let native = Settings::resolve(&Manifest::default(), None, None).expect("resolves");
        assert_eq!(native.panic_strategy(), "unwind");

        let wasm = Settings::resolve(&Manifest::default(), Some("wasm32-unknown"), None)
            .expect("resolves");
        assert_eq!(
            wasm.panic_strategy(),
            "abort",
            "WebAssembly traps and has no stack to unwind"
        );
    }
}
