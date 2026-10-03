//! The build cache: `nikaia.lock` is the record, the key is derived from it.
//!
//! [ADR-021](../../../docs/specification/adr/adr-021.md) decides the shape of
//! this and the reasoning is there; what follows is the part that has to be
//! true in code.
//!
//! * **D2** - the lockfile records everything that *determines* a build, and
//!   only that: input hashes, the toolchain used, the compiler's own version,
//!   and the dependency versions Cargo resolved. That last one is *recorded*
//!   and not keyed, and [`Lockfile::dependencies`] says why.
//! * **D3** - the compiler version is in the key. `emit` is a function we edit,
//!   so identical input under an edited emitter is a different build.
//! * **D5** - the build switches and the backend are *choices*, hashed into the key
//!   and never written to the file. A lockfile that records them is rewritten
//!   by every change of switch, for a diff that means nothing.
//! * **D6** - one key per unit. Hashing the whole lockfile would make one
//!   changed asset invalidate the project.
//! * **D7** - every dimension is listed in one place ([`Key::build`]) and
//!   nothing is captured incidentally. Fields are length-prefixed so that no
//!   two different sets of dimensions can hash alike.
//! * **D8** - artifacts are content-addressed under `target/nikaia/cache/`,
//!   with no index of its own: the key is derived, never looked up.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Bumped when the key derivation changes in a way that would otherwise let an
/// artifact from an older scheme be served under a colliding key.
const KEY_DOMAIN: &str = "nikaia-cache-key-v1";

/// The domain of the *sysroot* key ([`Key::sysroot`]).
///
/// A separate domain and not a separate hash function: the two keys live in one
/// module under one [`KeyBuilder`] so that D7's "every dimension is named in one
/// place" still holds, and the domain is what makes a unit key and a sysroot key
/// unable to collide even if their dimensions ever lined up.
const SYSROOT_DOMAIN: &str = "nikaia-sysroot-key-v1";

/// The lockfile format version, so a future reader can refuse rather than
/// misread.
const LOCK_VERSION: u32 = 1;

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What was chosen at invocation time (D5).
///
/// These reach the key and never the lockfile. Adding a field here means
/// adding a line to [`Key::build`] - which is the point of keeping them in a
/// struct of their own rather than passing loose strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choices {
    pub build: String,
    pub backend: String,
    /// **The allowlist's own digest**, empty where no list is in effect
    /// ([ADR-072](../../../docs/specification/adr/adr-072.md) D7).
    ///
    /// The list is a file the build read, so it belongs in the key beside the
    /// files it names — and it belongs *here*, with the switches, rather than
    /// among the assets: those are per unit and this binds the whole build
    /// (D6). Changing a line has to invalidate, or a build that stops reading
    /// a file keeps the old answer.
    pub reads: String,
    /// **Every description this build reads at a foreign boundary**, as one
    /// digest, empty where the project describes nothing
    /// ([ADR-290](../../../docs/specification/adr/adr-290.md) D5).
    ///
    /// Here for `reads`' reason and with `reads`' polarity: the file binds the
    /// whole build rather than one unit, and it is **hand-edited on purpose** —
    /// D5 says the draft is committed and reviewed like code, and a reviewer's
    /// `crosses = false` has to reach the next build. Without this it did not,
    /// and it failed **open**: a description that *dropped* a claim was seen,
    /// because a refused build records nothing, while one that *added* a claim
    /// hit an entry recorded before the word was there.
    pub describes: String,
    /// **The names the output writes for its files** (0.0.239): the abort
    /// table names each file as it was handed to the compiler (ADR-300 D9),
    /// so `/abs/one.nika` and `one.nika` are two lowerings of one source.
    /// Here and not in the unit's `source`, because that hash is what the
    /// lockfile records, and a path the checkout moves has no place there
    /// (D7).
    pub names: String,
}

impl Choices {
    pub fn new(build: impl Into<String>, backend: impl Into<String>) -> Self {
        Self {
            build: build.into(),
            backend: backend.into(),
            reads: String::new(),
            describes: String::new(),
            names: String::new(),
        }
    }

    /// The same, with an allowlist in effect.
    pub fn reading(self, digest: impl Into<String>) -> Self {
        Self {
            reads: digest.into(),
            ..self
        }
    }

    /// The same, with the names the output writes for its files.
    pub fn naming(self, names: impl Into<String>) -> Self {
        Self {
            names: names.into(),
            ..self
        }
    }

    /// The same, with the descriptions this build reads at its boundaries.
    pub fn describing(self, digest: impl Into<String>) -> Self {
        Self {
            describes: digest.into(),
            ..self
        }
    }
}

/// What the lockfile records about one translation unit: its inputs, hashed.
///
/// The asset list is what makes the next build's lookup possible at all. A
/// compiler cannot know which external files a unit reads without expanding
/// its macros, so the list is carried over from the previous build - and that
/// is sound because a unit that would read a *different* file had to change
/// its own source to say so, which changes `source` and misses the key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitRecord {
    /// SHA256 of the unit's own source.
    pub source: String,
    /// Compile-time I/O: asset path -> SHA256 of its contents.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, String>,
}

/// `nikaia.lock` (D2). Committed, and the answer to "does this build the same
/// thing for you as it does for me?".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    pub version: u32,
    /// The toolchain actually used - a record, not a constraint (D4). The
    /// constraint lives in the manifest.
    pub toolchain: String,
    /// The Nikaia compiler's own version (D3).
    pub compiler: String,
    /// The versions Cargo resolved, per dependency name (D2).
    ///
    /// **Recorded, and deliberately not in the key.** They belong in the file
    /// because the question it exists to answer - *does this build the same
    /// thing for you as for me?* - is only half answered without them: since
    /// ADR-002 D1 a project build generates a `Cargo.toml` and Cargo resolves a
    /// real graph beside it, and a reviewer or a second machine could not see
    /// any of it. They are **not** dimensions of [`Key`], because Stage 0 lowers
    /// name for name: a dependency bump changes the machine code Cargo
    /// produces, never the Rust this compiler emits, and the envelope a key
    /// holds is exactly that Rust and the ledger. Keying on them would
    /// invalidate every unit's lowering for a change that cannot alter one -
    /// D7's "too much" direction, and the half Cargo's own fingerprinting
    /// already covers (D1).
    ///
    /// A list per name because one name can resolve to two versions in one
    /// graph, and a record that drops one is not the whole answer.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, BTreeSet<String>>,
    /// `BTreeMap`, so the file is byte-identical for identical inputs rather
    /// than ordered by whatever a hash map felt like.
    #[serde(default)]
    pub units: BTreeMap<String, UnitRecord>,
}

impl Lockfile {
    pub fn new(toolchain: impl Into<String>, compiler: impl Into<String>) -> Self {
        Self {
            version: LOCK_VERSION,
            toolchain: toolchain.into(),
            compiler: compiler.into(),
            dependencies: BTreeMap::new(),
            units: BTreeMap::new(),
        }
    }

    /// Reads a lockfile, or starts an empty one if the path does not exist.
    ///
    /// A lockfile from a different toolchain or compiler is *kept*, not
    /// discarded: its entries simply cannot match, because the key is built
    /// from the current identity (D3). Throwing it away would lose the asset
    /// lists that make the next lookup possible.
    pub fn load(
        path: &Path,
        toolchain: impl Into<String>,
        compiler: impl Into<String>,
    ) -> Result<Self> {
        let toolchain = toolchain.into();
        let compiler = compiler.into();
        if !path.exists() {
            return Ok(Self::new(toolchain, compiler));
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read lockfile {}", path.display()))?;
        let mut lock: Lockfile = toml::from_str(&text)
            .with_context(|| format!("failed to parse lockfile {}", path.display()))?;
        anyhow::ensure!(
            lock.version == LOCK_VERSION,
            "lockfile {} has version {}, this compiler writes version {LOCK_VERSION}",
            path.display(),
            lock.version
        );
        lock.toolchain = toolchain;
        lock.compiler = compiler;
        Ok(lock)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let body = toml::to_string_pretty(self).context("failed to serialise lockfile")?;
        let text = format!(
            "# AUTO-GENERATED by `nikaia build`. Commit this file like a lockfile.\n\
             # It records what determined the build; build-time choices (the switches,\n\
             # backend) are deliberately absent - see ADR-021 D5. `[dependencies]` is\n\
             # what Cargo resolved, listed per name because one name can resolve twice.\n{body}"
        );
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, text)
            .with_context(|| format!("failed to write lockfile {}", path.display()))
    }
}

/// A cache key for one unit (D6).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key(String);

impl Key {
    /// **The one place every dimension of the key is named** (D7).
    ///
    /// Nothing is hashed that is not listed here, and nothing listed here is
    /// optional. Each field is length-prefixed and tagged, so that no
    /// rearrangement of values can produce the same digest as a different set
    /// of dimensions.
    pub fn build(
        compiler: &str,
        toolchain: &str,
        choices: &Choices,
        unit: &str,
        record: &UnitRecord,
    ) -> Self {
        let mut b = KeyBuilder::new(KEY_DOMAIN);
        b.field("compiler", compiler);
        b.field("toolchain", toolchain);
        b.field("build", &choices.build);
        b.field("backend", &choices.backend);
        // **The allowlist, whether or not anything was read from it** (D7). An
        // empty string is a build with no list, which is a different build from
        // one with an empty list: the first cannot read at all.
        b.field("reads", &choices.reads);
        // **And what the boundary says**
        // ([ADR-290](../../../docs/specification/adr/adr-290.md) D5): the
        // descriptions are files this build reads and a person edits, so a
        // change to one has to invalidate — for the reason the line above
        // exists, one boundary over.
        b.field("describes", &choices.describes);
        b.field("names", &choices.names);
        b.field("unit", unit);
        b.field("source", &record.source);
        // `BTreeMap` iterates in key order, so the same assets hash the same
        // regardless of the order they were discovered in.
        for (path, hash) in &record.assets {
            b.field("asset-path", path);
            b.field("asset-hash", hash);
        }
        Key(b.finish())
    }

    /// **The one place every dimension of the *sysroot* key is named**
    /// (ADR-002 D4, which is D7 applied to a second store).
    ///
    /// The unit key above answers *what Rust does this `.nika` lower to*. This
    /// one answers *which compiled `std` may this build link*, and the two sets
    /// of dimensions are not the same set:
    ///
    /// * `compiler` and `toolchain` are in both. `std`'s Nikaia half is lowered
    ///   by this emitter, and an rlib is only loadable by the `rustc` that wrote
    ///   it.
    /// * `target` is in, and `user_parallelism` is deliberately **not**: the
    ///   switch reaches `std` as a runtime value, never as a `cfg`, so there is
    ///   one compiled `std` per machine rather than one per build switch
    ///   (ADR-002 D4).
    /// * `codegen` is in - the rendered `[build.<target>]` table and the panic
    ///   strategy. It is what makes a hardware-specific build a *dimension*
    ///   rather than a reason to rebuild: `std`'s code runs inside every user
    ///   program, so it is the one place where optimising for the machine in
    ///   front of you pays, and two such builds have to be able to coexist.
    ///
    /// What is **out**, and why each is D7's "too much" direction here:
    ///
    /// * **The `std` sources' content hash.** What this key names is a
    ///   directory, and *inside* it Cargo's own fingerprinting decides what is
    ///   stale (D1). Hashing the tree as well would start a fresh directory for
    ///   every edit to `crates/nikaia-std/src/fs.rs` and rebuild everything
    ///   `std` depends on, to protect against a staleness Cargo has already
    ///   ruled out.
    /// * **The project**, its path and its resolved dependency versions. None
    ///   of them can change a byte of the compiled `std`, and any of them would
    ///   turn the one shared entry into a per-project one - which is the entire
    ///   thing this cache exists to stop.
    /// * **The lowering backend.** It changes what a `.nika` file lowers to,
    ///   and `std`'s Rust is not lowered by this build at all.
    pub fn sysroot(
        compiler: &str,
        toolchain: &str,
        target: &str,
        codegen: &str,
        features: &str,
    ) -> Self {
        let mut b = KeyBuilder::new(SYSROOT_DOMAIN);
        b.field("compiler", compiler);
        b.field("toolchain", toolchain);
        b.field("target", target);
        b.field("codegen", codegen);
        // **A switch that changes what `std` itself contains is a dimension of
        // the compiled `std`** ([ADR-021](../../../docs/specification/adr/adr-021.md)
        // D7, [ADR-281](../../../docs/specification/adr/adr-281.md) D36): the
        // re-entrancy check lives in that crate, so without this the second
        // project on the machine links the first one's answer — a guarantee
        // declined in one manifest and shipped from another's cache.
        b.field("features", features);
        Key(b.finish())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

struct KeyBuilder {
    hasher: Sha256,
}

impl KeyBuilder {
    fn new(domain: &str) -> Self {
        let mut b = KeyBuilder {
            hasher: Sha256::new(),
        };
        b.field("domain", domain);
        b
    }

    fn field(&mut self, name: &str, value: &str) {
        // Length prefixes are what make this unambiguous: without them
        // ("ab", "c") and ("a", "bc") would hash identically.
        self.hasher.update((name.len() as u64).to_le_bytes());
        self.hasher.update(name.as_bytes());
        self.hasher.update((value.len() as u64).to_le_bytes());
        self.hasher.update(value.as_bytes());
    }

    fn finish(self) -> String {
        self.hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Everything one cached build produced, under names the caller chooses.
///
/// A build makes more than the emitted Rust - ADR-020's contract ledger is an
/// output too, and a later stage may add more. Storing them under separate keys
/// would let a "hit" be half a build: the Rust present, the ledger missing, and
/// nothing to notice it. One envelope per key keeps a hit meaning what it says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifacts(BTreeMap<String, String>);

impl Artifacts {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with(mut self, name: impl Into<String>, content: impl Into<String>) -> Self {
        self.0.insert(name.into(), content.into());
        self
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Whether every name the caller needs is present. A build that asks for
    /// more than an older entry holds must miss rather than proceed with a
    /// gap - which is what makes adding an artifact a safe change.
    pub fn has_all(&self, names: &[&str]) -> bool {
        names.iter().all(|name| self.0.contains_key(*name))
    }
}

/// The content-addressed artifact store (D8), rooted at `target/nikaia/cache/`.
///
/// No index: an entry's path is a function of its key, so the store cannot
/// disagree with the lockfile about what it holds.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Store { root: root.into() }
    }

    fn path_for(&self, key: &Key) -> PathBuf {
        // Two-level fan-out, so a large store does not become one directory
        // with a hundred thousand entries in it.
        let k = key.as_str();
        self.root.join(&k[..2]).join(&k[2..])
    }

    pub fn get(&self, key: &Key) -> Option<String> {
        std::fs::read_to_string(self.path_for(key)).ok()
    }

    pub fn put(&self, key: &Key, artifact: &str) -> Result<()> {
        let path = self.path_for(key);
        let dir = path.parent().expect("store paths always have a parent");
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create cache directory {}", dir.display()))?;
        // Write-then-rename: a reader never sees a half-written artifact, and
        // two builds racing on the same key both end up with a complete file.
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, artifact)
            .with_context(|| format!("failed to write cache entry {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("failed to commit cache entry {}", path.display()))
    }
}

/// The lockfile and the store, used together.
///
/// The division of labour is ADR-021 D10's: this decides *whether to start*.
/// It is consulted before any work and answers with an artifact or with
/// nothing. It is not the contract ledger, which can only be compared after
/// inference has run.
#[derive(Debug)]
pub struct Cache {
    lock_path: PathBuf,
    store: Store,
    lock: Lockfile,
}

impl Cache {
    pub fn open(
        lock_path: impl Into<PathBuf>,
        store_root: impl Into<PathBuf>,
        toolchain: &str,
        compiler: &str,
    ) -> Result<Self> {
        let lock_path = lock_path.into();
        let lock = Lockfile::load(&lock_path, toolchain, compiler)?;
        Ok(Cache {
            lock_path,
            store: Store::new(store_root),
            lock,
        })
    }

    pub fn lockfile(&self) -> &Lockfile {
        &self.lock
    }

    /// Hashes the assets this unit read on the previous build, as they are on
    /// disk *now*. A recorded asset that has since disappeared is a miss, not
    /// an error: the unit is simply rebuilt, and the build reports the real
    /// problem if the file is still needed.
    fn current_record(&self, unit: &str, source: &str, asset_root: &Path) -> Option<UnitRecord> {
        let previous = self.lock.units.get(unit);
        let mut assets = BTreeMap::new();
        if let Some(previous) = previous {
            for path in previous.assets.keys() {
                let bytes = std::fs::read(asset_root.join(path)).ok()?;
                assets.insert(path.clone(), sha256_hex(&bytes));
            }
        }
        Some(UnitRecord {
            source: sha256_hex(source.as_bytes()),
            assets,
        })
    }

    /// The skip decision. `Some(artifacts)` means nothing has to be built -
    /// not the lowering, and not the checks that ran before it, because only a
    /// build that passed them was ever recorded (D13).
    ///
    /// A stored entry that cannot be decoded reads as a miss: it was written by
    /// a different version of this format, and rebuilding is always the right
    /// answer to not understanding what is there.
    pub fn lookup(
        &self,
        unit: &str,
        source: &str,
        choices: &Choices,
        asset_root: &Path,
    ) -> Option<Artifacts> {
        let record = self.current_record(unit, source, asset_root)?;
        let key = Key::build(
            &self.lock.compiler,
            &self.lock.toolchain,
            choices,
            unit,
            &record,
        );
        serde_json::from_str(&self.store.get(&key)?).ok()
    }

    /// Records a freshly built unit: its inputs into the lockfile, everything
    /// it produced into the store. `assets` is what the build actually read -
    /// an empty map is correct for a unit that read nothing.
    ///
    /// Only ever called for a build that succeeded, which is what lets a later
    /// hit skip the checks as well as the work (D13).
    pub fn record(
        &mut self,
        unit: &str,
        source: &str,
        assets: BTreeMap<String, String>,
        choices: &Choices,
        artifacts: &Artifacts,
    ) -> Result<()> {
        let record = UnitRecord {
            source: sha256_hex(source.as_bytes()),
            assets,
        };
        let key = Key::build(
            &self.lock.compiler,
            &self.lock.toolchain,
            choices,
            unit,
            &record,
        );
        let encoded = serde_json::to_string(artifacts).context("failed to encode artifacts")?;
        self.store.put(&key, &encoded)?;
        self.lock.units.insert(unit.to_string(), record);
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        self.lock.save(&self.lock_path)
    }
}

/// Where the lockfile and the store live for a given input.
///
/// Caching is on by default - it is the difference in feel between a Nikaia
/// build and a Rust one, and a default nobody types is not that. Being on by
/// default is what makes *where* the files go a real question rather than a
/// detail: nothing may be written next to a source file the user did not ask
/// us to write next to.
///
/// So there are two shapes, and the manifest decides which:
///
/// * **In a project** (`nikaia.toml` found by walking up, as Cargo does) the
///   lock is the committed record ADR-021 D2 describes, and the store sits in
///   `target/` where build output belongs.
/// * **Outside one** the invocation is a one-off transformation. A *committed*
///   lockfile would be meaningless there, because there is no project to commit
///   it to, so the input record moves into the user's cache directory next to
///   the store and the source tree is left completely untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The directory the unit is named relative to, and assets are resolved
    /// against.
    pub root: PathBuf,
    pub lock: PathBuf,
    pub store: PathBuf,
    /// Whether a `nikaia.toml` was found. False means nothing is written into
    /// the source tree.
    pub in_project: bool,
}

impl Layout {
    /// `NIKAIA_CACHE_DIR` overrides the user cache location. It moves *where*
    /// artifacts are kept and never what they are keyed by, so it is not a
    /// dimension in D7's sense - and tests need somewhere that is not the
    /// developer's real cache.
    ///
    /// Public because the compiled-`std` cache of ADR-002 D4 lives here too.
    /// That cache is machine-wide by definition - the whole point is that a
    /// second project does not rebuild it - so it cannot live under any one
    /// project's `target/`, and this is the directory D11 already sanctions for
    /// state that outlives one project.
    pub fn user_cache_dir() -> PathBuf {
        for var in ["NIKAIA_CACHE_DIR", "XDG_CACHE_HOME"] {
            if let Some(dir) = std::env::var_os(var).filter(|v| !v.is_empty()) {
                let dir = PathBuf::from(dir);
                return if var == "NIKAIA_CACHE_DIR" {
                    dir
                } else {
                    dir.join("nikaia")
                };
            }
        }
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(home).join(".cache").join("nikaia");
        }
        std::env::temp_dir().join("nikaia-cache")
    }

    pub fn resolve(input: &Path) -> Layout {
        let start = input
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        // Canonicalised so that `./src/../src` and `src` are one directory, and
        // so the record name below is stable. A failure here (the path does not
        // exist yet) is not fatal - the uncanonicalised path still works.
        let start = start.canonicalize().unwrap_or(start);

        for dir in start.ancestors() {
            if dir.join("nikaia.toml").is_file() {
                return Layout {
                    root: dir.to_path_buf(),
                    lock: dir.join("nikaia.lock"),
                    store: dir.join("target").join("nikaia").join("cache"),
                    in_project: true,
                };
            }
        }

        let base = Self::user_cache_dir();
        // One record per directory, named by a hash of the directory rather
        // than placed in it. Two projects that both have a `main.nika` must not
        // share a record - but they may share *store* entries, because the key
        // is content-addressed and deliberately holds no path (D7).
        let record = format!("{}.lock", sha256_hex(start.as_os_str().as_encoded_bytes()));
        Layout {
            root: start,
            lock: base.join("records").join(record),
            store: base.join("cache"),
            in_project: false,
        }
    }

    /// The unit's name: its path below the root, with forward slashes so that
    /// a lockfile written on Windows and one written on Linux agree. Never an
    /// absolute path - that is D7's first failure direction, a key that moves
    /// with the checkout and therefore never hits.
    pub fn unit_name(&self, input: &Path) -> String {
        let canonical = input.canonicalize();
        let path = canonical.as_deref().unwrap_or(input);
        match path.strip_prefix(&self.root) {
            Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
            // Outside the root entirely: fall back to the file name rather than
            // leaking an absolute path into the key.
            Err(_) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        }
    }
}

/// How many compiled-`std` trees are kept beside the newest one.
///
/// Small on purpose. Each is a whole Cargo target directory - measured, around
/// 240 MB - and the key holds the compiler's own identity, so **every rebuild of
/// this compiler starts a new one**. A developer therefore leaves one behind per
/// working day, and nothing ever took one away: measured, 1.7 GB in a user cache
/// and 13 GB in the one the project tests share, with a build then failing on
/// "no space left on device".
///
/// Three, and not one: switching target or codegen table back and forth is a
/// normal thing to do, and evicting on every switch would turn a cache into a
/// tax.
pub const KEEP_TREES: usize = 3;

/// Don't touch a tree that has been written to within this. A concurrent build
/// is the thing being protected from, and a build that has not touched its own
/// tree in an hour is not one that is running.
const IN_USE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Keep the newest [`KEEP_TREES`] entries of a cache directory and remove the
/// rest.
///
/// **A cache with no eviction is a disk leak**, and this one leaks by design
/// rather than by accident: its key holds the compiler's identity so that two
/// builds which differ in a way Cargo would answer by rebuilding coexist instead
/// of evicting one another ([ADR-021](../../../docs/specification/adr/adr-021.md)
/// D7). Coexisting is right; coexisting *forever* is what makes the disk run out.
///
/// Newest by the marker file [`touch`] writes, not by the directory's own
/// timestamp: Cargo writes into subdirectories, so a tree that is used every day
/// can have a top-level mtime from the day it was created.
///
/// **Failure here is never a failed build.** A cache that cannot be swept is a
/// fuller disk, which is the situation this is trying to improve and not a reason
/// to refuse to compile - the same rule D12 gives for a cache that cannot be
/// opened.
pub fn sweep(root: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut trees: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .map(|path| (used_at(&path), path))
        .collect();

    // Newest first, so the tail is what goes.
    trees.sort_by_key(|(used, _)| std::cmp::Reverse(*used));
    for (used, path) in trees.into_iter().skip(keep) {
        let idle = now.duration_since(used).unwrap_or_default();
        if idle < IN_USE {
            continue;
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// Say that a cache directory is in use now.
pub fn touch(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(USED), b"");
}

/// When a cache directory was last said to be in use.
fn used_at(dir: &Path) -> std::time::SystemTime {
    std::fs::metadata(dir.join(USED))
        .or_else(|_| std::fs::metadata(dir))
        .and_then(|meta| meta.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
}

/// The marker `touch` writes. A file rather than the directory's own mtime,
/// because Cargo writes into subdirectories and leaves the top alone.
const USED: &str = ".nikaia-used";

#[cfg(test)]
mod tests {
    use super::*;

    fn choices() -> Choices {
        Choices::new("x86_64-linux/auto", "rust")
    }

    fn emitted() -> Artifacts {
        Artifacts::new().with("rust", "EMITTED")
    }

    fn record(source: &str) -> UnitRecord {
        UnitRecord {
            source: sha256_hex(source.as_bytes()),
            assets: BTreeMap::new(),
        }
    }

    fn key_with(compiler: &str, toolchain: &str, c: &Choices, unit: &str, src: &str) -> Key {
        Key::build(compiler, toolchain, c, unit, &record(src))
    }

    #[test]
    fn identical_inputs_produce_identical_keys() {
        let a = key_with("0.1.0", "rustc-x", &choices(), "a.nika", "fn main() {}");
        let b = key_with("0.1.0", "rustc-x", &choices(), "a.nika", "fn main() {}");
        assert_eq!(a, b);
    }

    /// D5. Without this the cache would hand one build's artifact to the
    /// other, and `the_switches_agree_on_every_example` would compare an
    /// artifact against itself.
    #[test]
    fn the_switches_change_the_key() {
        let sequential = Choices::new("x86_64-linux/0", "rust");
        let parallel = Choices::new("x86_64-linux/auto", "rust");
        assert_ne!(
            key_with("0.1.0", "rustc-x", &sequential, "a.nika", "src"),
            key_with("0.1.0", "rustc-x", &parallel, "a.nika", "src"),
        );
    }

    /// D3. The emitter is a function we edit; a changed compiler is a changed
    /// build even when nothing else moved.
    #[test]
    fn the_compiler_version_changes_the_key() {
        assert_ne!(
            key_with("0.1.0", "rustc-x", &choices(), "a.nika", "src"),
            key_with("0.2.0", "rustc-x", &choices(), "a.nika", "src"),
        );
    }

    #[test]
    fn the_toolchain_changes_the_key() {
        assert_ne!(
            key_with("0.1.0", "rustc-x", &choices(), "a.nika", "src"),
            key_with("0.1.0", "rustc-y", &choices(), "a.nika", "src"),
        );
    }

    #[test]
    fn the_backend_changes_the_key() {
        assert_ne!(
            key_with(
                "0.1.0",
                "rustc-x",
                &Choices::new("x86_64-linux/auto", "rust"),
                "a",
                "s"
            ),
            key_with(
                "0.1.0",
                "rustc-x",
                &Choices::new("x86_64-linux/auto", "interpreter"),
                "a",
                "s"
            ),
        );
    }

    /// D7. Without length prefixes, moving a character from one field to the
    /// next would leave the digest unchanged.
    #[test]
    fn fields_cannot_run_into_one_another() {
        let ab = Choices::new("ab", "c");
        let a_bc = Choices::new("a", "bc");
        assert_ne!(
            key_with("0.1.0", "t", &ab, "u", "s"),
            key_with("0.1.0", "t", &a_bc, "u", "s"),
        );
    }

    /// D6. The key covers one unit's inputs, so a sibling's change cannot
    /// invalidate it.
    #[test]
    fn a_sibling_unit_does_not_affect_this_key() {
        let mut lock = Lockfile::new("rustc-x", "0.1.0");
        lock.units.insert("a.nika".into(), record("a"));
        let before = Key::build(
            &lock.compiler,
            &lock.toolchain,
            &choices(),
            "a.nika",
            lock.units.get("a.nika").unwrap(),
        );
        lock.units.insert("b.nika".into(), record("b changed"));
        let after = Key::build(
            &lock.compiler,
            &lock.toolchain,
            &choices(),
            "a.nika",
            lock.units.get("a.nika").unwrap(),
        );
        assert_eq!(before, after);
    }

    /// Assets are hashed on content, and the order they were discovered in is
    /// not a dimension.
    #[test]
    fn asset_order_is_not_a_dimension_but_content_is() {
        let mut one = UnitRecord {
            source: sha256_hex(b"s"),
            assets: BTreeMap::new(),
        };
        one.assets.insert("x.sql".into(), sha256_hex(b"X"));
        one.assets.insert("y.sql".into(), sha256_hex(b"Y"));
        let mut two = UnitRecord {
            source: sha256_hex(b"s"),
            assets: BTreeMap::new(),
        };
        two.assets.insert("y.sql".into(), sha256_hex(b"Y"));
        two.assets.insert("x.sql".into(), sha256_hex(b"X"));
        assert_eq!(
            Key::build("0.1.0", "t", &choices(), "u", &one),
            Key::build("0.1.0", "t", &choices(), "u", &two),
        );

        two.assets.insert("y.sql".into(), sha256_hex(b"Y changed"));
        assert_ne!(
            Key::build("0.1.0", "t", &choices(), "u", &one),
            Key::build("0.1.0", "t", &choices(), "u", &two),
        );
    }

    /// Caching is on by default, so this is the promise that makes it
    /// acceptable: outside a `nikaia.toml` project, nothing goes into the
    /// directory the source lives in.
    ///
    /// Asserted as "not under the input directory" rather than against a
    /// specific fallback path, so the test neither depends on the environment
    /// nor has to mutate it - `set_var` is process-global and these tests run
    /// in parallel threads.
    #[test]
    fn outside_a_project_nothing_is_written_beside_the_source() {
        let dir = scratch("layout-loose");
        let input = dir.join("main.nika");
        std::fs::write(&input, "fn main() {}").unwrap();

        let layout = Layout::resolve(&input);
        assert!(!layout.in_project);
        let canonical = dir.canonicalize().unwrap();
        assert!(
            !layout.lock.starts_with(&canonical),
            "the lock must not land beside the source: {}",
            layout.lock.display()
        );
        assert!(
            !layout.store.starts_with(&canonical),
            "nor the store: {}",
            layout.store.display()
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// In a project the lock is the committed record at the root, found by
    /// walking up from the input the way Cargo finds `Cargo.toml`.
    #[test]
    fn a_manifest_makes_the_root_and_names_the_unit_below_it() {
        let dir = scratch("layout-project");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(dir.join("nikaia.toml"), "[package]\nname = \"demo\"\n").unwrap();
        let input = src.join("main.nika");
        std::fs::write(&input, "fn main() {}").unwrap();

        let layout = Layout::resolve(&input);
        let root = dir.canonicalize().unwrap();
        assert!(layout.in_project);
        assert_eq!(layout.root, root);
        assert_eq!(layout.lock, root.join("nikaia.lock"));
        assert_eq!(layout.store, root.join("target/nikaia/cache"));
        // Relative to the root, so the key does not move with the checkout.
        assert_eq!(layout.unit_name(&input), "src/main.nika");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A store that cannot be created reports an error rather than panicking,
    /// so the caller can carry on with a slower build. Enforced with a *file*
    /// where a directory would have to go, which even root cannot write into.
    #[test]
    fn an_unusable_store_reports_instead_of_panicking() {
        let dir = scratch("store-blocked");
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();

        let store = Store::new(blocker.join("cache"));
        let key = Key::build("0.1.0", "t", &choices(), "a.nika", &record("src"));
        assert!(store.get(&key).is_none(), "a missing entry reads as a miss");
        assert!(
            store.put(&key, "EMITTED").is_err(),
            "an unusable store must surface an error, not unwind"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    fn scratch(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "nikaia-cache-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_recorded_unit_is_found_again_and_a_changed_one_is_not() {
        let dir = scratch("roundtrip");
        let mut cache =
            Cache::open(dir.join("nikaia.lock"), dir.join("store"), "t", "0.1.0").unwrap();

        assert!(
            cache
                .lookup("a.nika", "fn main() {}", &choices(), &dir)
                .is_none()
        );

        cache
            .record(
                "a.nika",
                "fn main() {}",
                BTreeMap::new(),
                &choices(),
                &emitted(),
            )
            .unwrap();

        assert_eq!(
            cache.lookup("a.nika", "fn main() {}", &choices(), &dir),
            Some(emitted())
        );
        // A changed source is a different unit as far as the key is concerned.
        assert!(
            cache
                .lookup("a.nika", "fn other() {}", &choices(), &dir)
                .is_none()
        );
        // As is the same source under different switches.
        assert!(
            cache
                .lookup(
                    "a.nika",
                    "fn main() {}",
                    &Choices::new("x86_64-linux/0", "rust"),
                    &dir
                )
                .is_none()
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// D5, on disk: the switches and backend must not appear in the committed
    /// file, or changing one would rewrite it for nothing.
    #[test]
    fn the_lockfile_records_inputs_and_not_choices() {
        let dir = scratch("lockfile");
        let path = dir.join("nikaia.lock");
        let mut cache = Cache::open(&path, dir.join("store"), "rustc-x", "0.1.0").unwrap();
        cache
            .record(
                "a.nika",
                "fn main() {}",
                BTreeMap::new(),
                &choices(),
                &emitted(),
            )
            .unwrap();
        cache.save().unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("a.nika"), "the unit is recorded:\n{text}");
        assert!(
            text.contains("rustc-x"),
            "the toolchain is recorded:\n{text}"
        );
        assert!(
            text.contains("0.1.0"),
            "the compiler version is recorded:\n{text}"
        );
        assert!(
            !text.contains("x86_64-linux"),
            "the build must not be:\n{text}"
        );

        // And it round-trips.
        let reloaded = Lockfile::load(&path, "rustc-x", "0.1.0").unwrap();
        assert_eq!(reloaded.units, cache.lockfile().units);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// D2's resolved dependency versions: written to the file, absent from the
    /// key - and both halves are the decision.
    ///
    /// **Recorded**, because a lockfile that cannot say which `regex` was
    /// linked answers *does this build the same thing for you as for me* only
    /// about the Nikaia half. **Not keyed**, because Stage 0 lowers name for
    /// name: a dependency bump changes the machine code Cargo produces, never
    /// the Rust in the envelope the key holds. Keying on it would invalidate
    /// every unit's lowering for a change that cannot alter one, which is D7's
    /// "too much" direction - and the assertion below is what would fail if
    /// someone later threaded them into [`Key::build`] for symmetry.
    #[test]
    fn resolved_dependency_versions_are_recorded_and_not_keyed() {
        let dir = scratch("lockfile-dependencies");
        let path = dir.join("nikaia.lock");

        let mut lock = Lockfile::new("rustc-x", "0.1.0");
        lock.units.insert("src/main.nika".into(), record("fn main"));
        lock.dependencies
            .insert("regex".into(), BTreeSet::from(["1.13.1".to_string()]));
        lock.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("regex") && text.contains("1.13.1"),
            "a reviewer and a second machine can see what was linked:\n{text}"
        );
        assert!(
            !text.contains(&dir.display().to_string()),
            "and nothing absolute went in with it (ADR-005 D8):\n{text}"
        );

        let reloaded = Lockfile::load(&path, "rustc-x", "0.1.0").unwrap();
        assert_eq!(reloaded.dependencies, lock.dependencies);

        // The key is built from the unit's own inputs and the identity, and a
        // bumped dependency is none of those.
        let before = Key::build(
            &lock.compiler,
            &lock.toolchain,
            &choices(),
            "src/main.nika",
            lock.units.get("src/main.nika").unwrap(),
        );
        lock.dependencies
            .insert("regex".into(), BTreeSet::from(["1.14.0".to_string()]));
        let after = Key::build(
            &lock.compiler,
            &lock.toolchain,
            &choices(),
            "src/main.nika",
            lock.units.get("src/main.nika").unwrap(),
        );
        assert_eq!(
            before, after,
            "a dependency bump changes what Cargo compiles, not what this \
             compiler emits - keying on it would throw away every lowering for \
             nothing (ADR-021 D7)"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Byte-determinism, in the sense ADR-005 D8 requires: the same inputs
    /// discovered in a different order are the same file.
    ///
    /// The dependency record is a map, and D8's first banned item is iterating
    /// an unordered one to produce output. `BTreeMap`/`BTreeSet` is what makes
    /// that unrepresentable here; this is the test that says so, and the
    /// cross-process half is in `crates/nikaia/tests/project.rs`, where two
    /// `nikaia build` processes are compared.
    #[test]
    fn the_dependency_record_does_not_depend_on_discovery_order() {
        let names = ["regex", "aho-corasick", "memchr", "windows-sys"];

        let mut forwards = Lockfile::new("rustc-x", "0.1.0");
        for name in names {
            forwards
                .dependencies
                .insert(name.into(), BTreeSet::from(["1.0.0".to_string()]));
        }
        let mut backwards = Lockfile::new("rustc-x", "0.1.0");
        for name in names.iter().rev() {
            backwards
                .dependencies
                .insert((*name).into(), BTreeSet::from(["1.0.0".to_string()]));
        }

        let dir = scratch("lockfile-order");
        let (a, b) = (dir.join("a.lock"), dir.join("b.lock"));
        forwards.save(&a).unwrap();
        backwards.save(&b).unwrap();
        assert_eq!(
            std::fs::read(&a).unwrap(),
            std::fs::read(&b).unwrap(),
            "the order the dependencies were found in is not a fact about the build"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The lookup rehashes the recorded assets from disk, so editing one is a
    /// miss even though the `.nika` source is untouched.
    #[test]
    fn a_changed_asset_invalidates_the_unit() {
        let dir = scratch("assets");
        let asset = dir.join("schema.sql");
        std::fs::write(&asset, "CREATE TABLE a;").unwrap();

        let mut cache =
            Cache::open(dir.join("nikaia.lock"), dir.join("store"), "t", "0.1.0").unwrap();
        let mut assets = BTreeMap::new();
        assets.insert("schema.sql".to_string(), sha256_hex(b"CREATE TABLE a;"));
        cache
            .record("a.nika", "src", assets, &choices(), &emitted())
            .unwrap();

        assert_eq!(
            cache.lookup("a.nika", "src", &choices(), &dir),
            Some(emitted())
        );

        std::fs::write(&asset, "CREATE TABLE b;").unwrap();
        assert!(cache.lookup("a.nika", "src", &choices(), &dir).is_none());

        // A recorded asset that is gone is a miss, not a panic.
        std::fs::remove_file(&asset).unwrap();
        assert!(cache.lookup("a.nika", "src", &choices(), &dir).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::*;

    fn tree(root: &Path, name: &str, used_ago: std::time::Duration) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("debug")).expect("a cargo-shaped tree");
        std::fs::write(dir.join("debug/artifact"), b"x").expect("something in it");
        touch(&dir);
        set_used_at(&dir, std::time::SystemTime::now() - used_ago);
        dir
    }

    /// The marker's mtime, set directly - a test cannot wait an hour.
    fn set_used_at(dir: &Path, when: std::time::SystemTime) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.join(USED))
            .expect("the marker is there");
        file.set_modified(when).expect("set the time");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nikaia-sweep-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch root");
        dir
    }

    const DAY: std::time::Duration = std::time::Duration::from_secs(60 * 60 * 24);

    /// **The newest `keep` stay and the rest go.**
    ///
    /// The leak this is about: the key holds the compiler's own identity, so a
    /// rebuild of the compiler starts a new tree of some 240 MB and nothing ever
    /// removed one. Measured before this, 1.7 GB in one user cache and 13 GB in
    /// the one the project tests share.
    #[test]
    fn the_oldest_trees_are_removed_and_the_newest_are_kept() {
        let root = scratch("oldest");
        let newest = tree(&root, "aaa", DAY);
        let middle = tree(&root, "bbb", 2 * DAY);
        let old = tree(&root, "ccc", 3 * DAY);
        let older = tree(&root, "ddd", 4 * DAY);

        sweep(&root, 2);

        assert!(newest.is_dir(), "the newest stays");
        assert!(middle.is_dir(), "and the one below it");
        assert!(!old.is_dir(), "the third is gone");
        assert!(!older.is_dir(), "and so is the fourth");
        let _ = std::fs::remove_dir_all(root);
    }

    /// **A tree in use is never removed, however far down the order it is.**
    ///
    /// A second build running right now is the thing to be careful of: its tree
    /// was touched when it started, so an age floor is what keeps a sweep from
    /// deleting a directory Cargo is writing into.
    #[test]
    fn a_tree_touched_recently_is_left_alone() {
        let root = scratch("recent");
        let a = tree(&root, "aaa", DAY);
        let b = tree(&root, "bbb", 2 * DAY);
        let running = tree(&root, "ccc", std::time::Duration::from_secs(5));

        sweep(&root, 1);

        assert!(running.is_dir(), "a build is using it");
        assert!(!a.is_dir() && !b.is_dir(), "the idle ones went");
        let _ = std::fs::remove_dir_all(root);
    }

    /// A cache that cannot be swept is a fuller disk, not a failed build - the
    /// rule ADR-021 D12 already gives for a cache that cannot be opened.
    #[test]
    fn sweeping_what_is_not_there_is_not_an_error() {
        sweep(Path::new("/nowhere/at/all/nikaia"), 3);
    }
}
