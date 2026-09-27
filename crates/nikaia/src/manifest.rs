//! `nikaia.toml`'s build switches, and the CLI overriding them for one build.
//!
//! ADR-037 D5 and [ADR-033](../../../docs/specification/adr/adr-033.md) D8 give
//! the same shape to all three settings: the manifest carries them, because
//! they are properties of a *project* rather than of an invocation and a
//! committed value is one a reviewer sees; a flag overrides for a single build,
//! which is what a benchmark and a bug hunt need.
//!
//! Nothing here decides what a setting *means* - `emit::Build` and
//! `emit::Ordering` own that, and get handed a string either way. This module
//! only answers "which string", and does it once per run so that two places
//! cannot resolve the same setting differently.
//!
//! It also reads the rest of the manifest, for
//! [ADR-002](../../../docs/specification/adr/adr-002.md) D1's translation into
//! a `Cargo.toml`: `[package]`, `[dependencies]` and the per-target codegen
//! tables `[build.<target>]`. One reader, because a manifest read twice is a
//! manifest that can be understood two ways.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::refused;

/// Where a `nikaia.toml` was found, and what it said.
///
/// Absent outside a project, which is not an error: a single `.nika` file
/// compiles with the built-in defaults and the flags, and littering a manifest
/// into someone's directory to make that work would be the wrong trade.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Manifest {
    build: BTreeMap<String, String>,
    package: BTreeMap<String, toml::Value>,
    dependencies: BTreeMap<String, Dependency>,
    /// `[build.<target>]` - `opt-level` and `lto`, per machine (Part III 13.3).
    codegen: BTreeMap<String, BTreeMap<String, toml::Value>>,
    /// What this manifest carries that the compiler no longer reads, and where
    /// it went. Printed once per build rather than returned as an error:
    /// failing a manifest somebody already wrote to the specification would
    /// punish them for the move ([ADR-038](../../../docs/specification/adr/adr-038.md) D5).
    notes: Vec<String>,
    /// The directory the manifest was found in, and therefore the project root.
    /// `None` when there was no manifest at all.
    root: Option<PathBuf>,
}

/// One entry of `[dependencies]`, and which of the three shapes it is.
///
/// The distinction is the whole of the translation: a native Rust crate is
/// passed to Cargo exactly as written (ADR-002 D1), a Nikaia package **by path**
/// is read as part of this program
/// ([ADR-047](../../../docs/specification/adr/adr-047.md) D2), and a Nikaia
/// package named any other way is something nothing has decided yet.
#[derive(Debug, Clone, PartialEq)]
pub enum Dependency {
    /// `regex = { type = "rust", version = "1.5" }` - the value with `type`
    /// removed, which is what Cargo is handed.
    Rust(toml::Value),
    /// `http = { path = "../http" }` - a Nikaia package, where it is.
    ///
    /// **The key is the name** a `use` writes, and the path says only where it
    /// comes from (ADR-047 D2 rule 1): two libraries that both want to be `http`
    /// are the consumer's to name apart, which is the authority
    /// [ADR-046](../../../docs/specification/adr/adr-046.md) D3 gives them anyway.
    /// Relative to the manifest's own directory, because that is the only
    /// interpretation that survives the project being checked out somewhere else.
    Path(PathBuf),
    /// `http-server = "1.2"`. Carried rather than resolved: a registry, a version
    /// grammar and a distribution format are the three things ADR-002 D1 §5
    /// refuses to guess at, and ADR-047 D2 keeps refusing.
    Nikaia(toml::Value),
}

/// The keys `[build]` may carry. A key outside this set is a typo until proven
/// otherwise, and saying so beats a switch that silently stayed at its default:
/// `user_parallelism` with an underscore is the mistake this catches.
///
/// The list is `target`, `user-parallelism` (ADR-037 D5) and
/// `reentrancy-check` ([ADR-039](../../../docs/specification/adr/adr-039.md)
/// D8), plus the one key that has **moved out** - see [`MOVED`]. A key that is
/// none of those, and that this compiler once had, is in [`WITHDRAWN`]:
/// refused, but in its own words rather than as a typo.
///
/// **Part I 1.2 names the options rather than counting them**, and this is why:
/// the count has been wrong twice (ADR-039 D8's own correction), so the page
/// lists them and this list is what a manifest is checked against.
const KNOWN: &[&str] = &[
    "target",
    "user-parallelism",
    "reentrancy-check",
    "cleanup-deadline",
];

/// The keys this compiler had and no longer has, with what to do instead.
///
/// Different from [`MOVED`] in the only way that matters to whoever wrote the
/// key: a moved key still decides something, somewhere else, so the manifest
/// keeps working and says where to look. A withdrawn key decides **nothing
/// anywhere**, and a build that quietly ignored it would be a program behaving
/// differently from the file that describes it.
///
/// `ordering` is [ADR-050](../../../docs/specification/adr/adr-050.md) D7. It
/// existed to turn off the automatic reordering D1 withdrew, and a switch
/// between two behaviours when there is one left is a question with no answer.
const WITHDRAWN: &[(&str, &str)] = &[(
    "ordering",
    "statements run in the order they are written, so there is nothing left for it to turn off \
     (ADR-050 D1 and D7). A program that wants two things to run together writes `overlap { … }`, \
     and `--overlaps` says which branches did",
)];

/// The keys the manifest still accepts and the compiler no longer reads,
/// with where each of them went.
///
/// `cleanup-deadline` is [ADR-038](../../../docs/specification/adr/adr-038.md)
/// D5's move: how long a program waits at exit for pending cleanup
/// ([ADR-006](../../../docs/specification/adr/adr-006.md) D5) is an operating
/// property, and a build-time key cannot be tuned by the operator - who is not
/// the person who compiled it. It stays accepted, with a note that says where
/// it went, because refusing it would fail a manifest written to the
/// specification that documented it.
const MOVED: &[(&str, &str)] = &[(
    "cleanup-deadline",
    "the runtime configuration file `nikaia-runtime.toml`, read when the program \
     starts by the person running it (ADR-038 D5). How long a program waits at \
     exit is an operating property, and the compiler no longer reads this key",
)];

/// What a `[build.<target>]` table may carry (Part III 13.3). Same rule as
/// `[build]`: a key nothing reads is a typo, and a silently ignored `opt_level`
/// leaves a build at a setting its author believed they had changed.
const KNOWN_CODEGEN: &[&str] = &["opt-level", "lto"];

/// The machines `[build.<target>]` may name (ADR-037 D1). A table for a target
/// that does not exist is codegen nothing will ever apply.
const TARGETS: &[&str] = &["x86_64-linux", "aarch64-linux", "wasm32-unknown"];

impl Manifest {
    /// Read the manifest governing `input`, if there is one.
    ///
    /// The search is the one the build cache already does - `Layout::resolve`
    /// walks to the nearest `nikaia.toml` at or above the input's directory -
    /// so a file cannot be cached as part of one project and compiled with
    /// another's switches. One root-finder, deliberately.
    pub fn find(input: &Path) -> Result<Manifest> {
        let layout = orchestrator::cache::Layout::resolve(input);
        if !layout.in_project {
            return Ok(Manifest::default());
        }
        Manifest::read(&layout.root.join("nikaia.toml"))
    }

    /// Read a manifest at a known path. The project build already knows where
    /// the root is, and searching again from there could only find a different
    /// answer.
    pub fn read(path: &Path) -> Result<Manifest> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut manifest = Manifest::parse(&text)
            .with_context(|| format!("in {}", path.display()))
            .map_err(|e| refused!("{e:#}"))?;
        manifest.root = path.parent().map(Path::to_path_buf);
        Ok(manifest)
    }

    /// Whether this manifest says anything about how to build.
    ///
    /// What it is for is a **dependency's** `[build]`, which is ignored:
    /// a package is built with the settings of the program that uses it
    /// ([ADR-047](../../../docs/specification/adr/adr-047.md) D2 rule 4), and
    /// anything else would put two answers to the parallelism question in one
    /// build. Ignored **and said**, which is why there is a question to ask.
    pub fn has_build_section(&self) -> bool {
        !self.build.is_empty() || !self.codegen.is_empty()
    }

    /// `[package]`, `[dependencies]`, `[build]` and the `[build.<target>]`
    /// codegen tables.
    pub fn parse(text: &str) -> Result<Manifest> {
        let document: toml::Value = toml::from_str(text).context("this is not valid TOML")?;

        let mut manifest = Manifest {
            package: table_of(&document, "package"),
            dependencies: dependencies(&document)?,
            ..Manifest::default()
        };

        let Some(table) = document.get("build").and_then(toml::Value::as_table) else {
            return Ok(manifest);
        };

        let mut build = BTreeMap::new();
        for (key, value) in table {
            if let Some(sub) = value.as_table() {
                manifest.codegen.insert(key.clone(), codegen(key, sub)?);
                continue;
            }
            if let Some((_, why)) = WITHDRAWN.iter().find(|(gone, _)| *gone == key.as_str()) {
                return Err(refused!("`{key}` in `[build]` is withdrawn: {why}"));
            }
            if !KNOWN.contains(&key.as_str()) {
                return Err(refused!(
                    "unknown key `{key}` in `[build]` (expected one of: {})",
                    KNOWN.join(", ")
                ));
            }
            if let Some((_, went)) = MOVED.iter().find(|(moved, _)| *moved == key.as_str()) {
                manifest
                    .notes
                    .push(format!("`{key}` in `[build]` has moved to {went}"));
                // Not carried into `build`: a key nothing reads must not be
                // reachable through `setting`, or a later reader would resolve
                // it from the wrong file.
                continue;
            }
            // Every switch is a word, so a bare `no` (TOML's boolean) or a
            // count is the plausible mistake. Reporting the type here would
            // hide the *reason* - `Build::parse` explains why a count is not a
            // count - so the value is stringified and passed on to the
            // parser that owns the setting.
            let word = match value {
                toml::Value::String(word) => word.clone(),
                other => other.to_string(),
            };
            build.insert(key.clone(), word);
        }
        manifest.build = build;
        Ok(manifest)
    }

    /// The project root - the directory the manifest sits in.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// What this manifest says that the compiler no longer reads, and where it
    /// went. One line each, for a build to print once.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// `[package] name`, which becomes the Cargo package and the binary's name.
    pub fn package_name(&self) -> Option<&str> {
        self.package.get("name").and_then(toml::Value::as_str)
    }

    /// `[package] version`. Cargo requires one, so a project that does not say
    /// gets `0.0.0` - a version nobody typed, rather than a guess at one they
    /// meant.
    pub fn package_version(&self) -> &str {
        self.package
            .get("version")
            .and_then(toml::Value::as_str)
            .unwrap_or("0.0.0")
    }

    /// Everything `[dependencies]` declared, in the order Cargo will see it.
    pub fn dependencies(&self) -> &BTreeMap<String, Dependency> {
        &self.dependencies
    }

    /// **The Rust crates this build declares**, under the name a program writes
    /// ([ADR-104](../../../docs/specification/adr/adr-104.md) D1).
    ///
    /// `hyper-shim = { type = "rust", … }` is written `hyper_shim::serve_once`
    /// in a program, because that is the crate name Cargo makes of the key and
    /// the name the generated Rust carries. The manifest key is what a person
    /// types and the underscore form is what a call names, so the translation
    /// belongs here rather than at every reader.
    ///
    /// **Only `type = "rust"`.** A Nikaia package by path is read as part of
    /// this program and has a ledger of its own; what this names is the set at
    /// whose edge D1's question is asked.
    pub fn foreign_crates(&self) -> std::collections::BTreeSet<String> {
        self.dependencies
            .iter()
            .filter(|(_, declared)| matches!(declared, Dependency::Rust(_)))
            .map(|(name, _)| name.replace('-', "_"))
            .collect()
    }

    /// The codegen table for one machine, empty where the manifest is silent.
    pub fn codegen_for(&self, target: &str) -> BTreeMap<String, toml::Value> {
        self.codegen.get(target).cloned().unwrap_or_default()
    }

    /// The effective value: the flag if one was given, else the manifest, else
    /// the built-in default.
    ///
    /// The order is the whole of D5. A flag that the manifest could override
    /// would make `--target` useless for the one build it exists for, and a
    /// manifest that the built-in default could override would make a
    /// committed switch a suggestion.
    pub fn setting<'a>(&'a self, key: &str, flag: Option<&'a str>, default: &'a str) -> &'a str {
        flag.or_else(|| self.build.get(key).map(String::as_str))
            .unwrap_or(default)
    }
}

/// A top-level table of the manifest, as a map. Absent is empty, because a
/// manifest without `[package]` is a manifest that only set switches.
fn table_of(document: &toml::Value, name: &str) -> BTreeMap<String, toml::Value> {
    document
        .get(name)
        .and_then(toml::Value::as_table)
        .map(|table| {
            table
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default()
}

/// `[build.<target>]`, checked.
fn codegen(target: &str, table: &toml::Table) -> Result<BTreeMap<String, toml::Value>> {
    if !TARGETS.contains(&target) {
        return Err(refused!(
            "`[build.{target}]` names no machine (expected one of: {})",
            TARGETS.join(", ")
        ));
    }
    let mut out = BTreeMap::new();
    for (key, value) in table {
        if !KNOWN_CODEGEN.contains(&key.as_str()) {
            return Err(refused!(
                "unknown key `{key}` in `[build.{target}]` (expected one of: {})",
                KNOWN_CODEGEN.join(", ")
            ));
        }
        out.insert(key.clone(), value.clone());
    }
    Ok(out)
}

/// `[dependencies]`, split into the two shapes 13.3 shows.
///
/// `type = "rust"` is the marker, and it is removed on the way through: it is
/// Nikaia's word about which ecosystem the name belongs to, and Cargo would
/// reject it as an unknown key.
fn dependencies(document: &toml::Value) -> Result<BTreeMap<String, Dependency>> {
    let mut out = BTreeMap::new();
    for (name, value) in table_of(document, "dependencies") {
        let kind = value.get("type").and_then(toml::Value::as_str);
        match kind {
            Some("rust") => {
                let mut table = value
                    .as_table()
                    .cloned()
                    .expect("a value with a `type` key is a table");
                table.remove("type");
                out.insert(name, Dependency::Rust(toml::Value::Table(table)));
            }
            Some(other) => {
                return Err(refused!(
                    "dependency `{name}` has `type = \"{other}\"`, which names no ecosystem \
                     (the only one spelled out is `rust`, for a crate from crates.io)"
                ));
            }
            // A Nikaia package, and `path` is the one way of naming one that
            // resolves (ADR-047 D2). Anything else is carried to the place that
            // refuses it, with the reason.
            None => match value.get("path").and_then(toml::Value::as_str) {
                Some(path) => {
                    out.insert(name, Dependency::Path(PathBuf::from(path)));
                }
                None => {
                    out.insert(name, Dependency::Nikaia(value));
                }
            },
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_beats_the_manifest_and_the_manifest_beats_the_default() {
        let manifest = Manifest::parse("[build]\ntarget = \"wasm32-unknown\"\n").expect("parses");
        assert_eq!(
            manifest.setting("target", None, "x86_64-linux"),
            "wasm32-unknown"
        );
        assert_eq!(
            manifest.setting("target", Some("x86_64-linux"), "x86_64-linux"),
            "x86_64-linux"
        );
        assert_eq!(
            manifest.setting("ordering", None, "effects"),
            "effects",
            "a key the manifest does not carry falls through to the default"
        );
    }

    /// No manifest is the single-file case, not a failure.
    #[test]
    fn a_manifest_without_a_build_table_decides_nothing() {
        let manifest = Manifest::parse("[package]\nname = \"x\"\n").expect("parses");
        assert_eq!(
            manifest.setting("target", None, "x86_64-linux"),
            "x86_64-linux"
        );
        assert_eq!(manifest.setting("ordering", None, "effects"), "effects");
        assert_eq!(manifest.package_name(), Some("x"));
    }

    /// The mistake this exists for: the switch is spelled with a hyphen in the
    /// manifest and an underscore on the CLI, and a silently ignored key would
    /// leave the build at a default the author thought they had changed.
    #[test]
    fn an_unknown_key_is_named_rather_than_ignored() {
        let error = Manifest::parse("[build]\nuser_parallelism = \"yes\"\n")
            .expect_err("an unknown key is refused");
        let text = format!("{error:#}");
        assert!(text.contains("user_parallelism"), "{text}");
        assert!(text.contains("user-parallelism"), "{text}");
    }

    /// `[build.x86_64-linux]` is codegen, not a switch. It has to survive, and
    /// since ADR-002 D1 it has to arrive somewhere: it becomes a Cargo profile.
    #[test]
    fn a_per_target_table_is_not_a_switch() {
        let manifest = Manifest::parse(
            "[build]\nuser-parallelism = \"yes\"\n\n[build.x86_64-linux]\nopt-level = 3\nlto = true\n",
        )
        .expect("parses");
        assert_eq!(manifest.setting("user-parallelism", None, "no"), "yes");

        let codegen = manifest.codegen_for("x86_64-linux");
        assert_eq!(codegen["opt-level"].as_integer(), Some(3));
        assert_eq!(codegen["lto"].as_bool(), Some(true));
        assert!(
            manifest.codegen_for("wasm32-unknown").is_empty(),
            "a machine the manifest says nothing about carries no codegen"
        );
    }

    /// The same rule `[build]` has, for the same reason: a key nothing reads
    /// leaves a build at a setting its author believed they had changed.
    #[test]
    fn an_unknown_codegen_key_is_named_rather_than_ignored() {
        let error = Manifest::parse("[build.x86_64-linux]\nopt_level = 3\n")
            .expect_err("an unknown key is refused");
        let text = format!("{error:#}");
        assert!(text.contains("opt_level"), "{text}");
        assert!(text.contains("opt-level"), "{text}");
    }

    /// `[build.x86_65-linux]` is a typo, and the codegen in it would apply to
    /// nothing at all.
    #[test]
    fn a_per_target_table_for_no_machine_is_refused() {
        let error = Manifest::parse("[build.x86_65-linux]\nopt-level = 3\n")
            .expect_err("an unknown machine is refused");
        assert!(format!("{error:#}").contains("x86_64-linux"), "{error:#}");
    }

    /// Part III 13.3's two shapes. `type = "rust"` says crates.io, and the
    /// marker is Nikaia's - Cargo would refuse it as an unknown key, so it does
    /// not travel (ADR-002 D1).
    #[test]
    fn a_rust_dependency_passes_through_without_its_marker() {
        let manifest = Manifest::parse(
            "[dependencies]\nhttp-server = \"1.2\"\nregex = { type = \"rust\", version = \"1.5\" }\n",
        )
        .expect("parses");

        match &manifest.dependencies()["regex"] {
            Dependency::Rust(value) => {
                assert_eq!(
                    value.get("version").and_then(toml::Value::as_str),
                    Some("1.5")
                );
                assert!(
                    value.get("type").is_none(),
                    "the marker does not reach Cargo"
                );
            }
            other => panic!("regex is a Rust crate, not {other:?}"),
        }
        assert!(matches!(
            manifest.dependencies()["http-server"],
            Dependency::Nikaia(_)
        ));
    }

    /// A `type` nothing implements is refused rather than guessed at.
    #[test]
    fn a_dependency_naming_no_ecosystem_is_refused() {
        let error = Manifest::parse("[dependencies]\nx = { type = \"c\", version = \"1\" }\n")
            .expect_err("an unknown ecosystem is refused");
        assert!(format!("{error:#}").contains("rust"), "{error:#}");
    }

    /// ADR-038 D5 moved it to the runtime configuration file. It is still
    /// accepted here - refusing it would fail a manifest written to the
    /// specification that documented it - and the note says where it went.
    #[test]
    fn the_cleanup_deadline_is_accepted_and_says_where_it_went() {
        let manifest = Manifest::parse("[build]\ncleanup-deadline = \"30s\"\n").expect("parses");
        let note = manifest
            .notes()
            .first()
            .expect("a moved key leaves a note rather than failing the build");
        assert!(note.contains("cleanup-deadline"), "{note}");
        assert!(note.contains("nikaia-runtime.toml"), "{note}");
        assert!(note.contains("ADR-038 D5"), "{note}");

        // …and nothing reads it from here any more, so a later reader cannot
        // resolve it out of the wrong file.
        assert_eq!(
            manifest.setting("cleanup-deadline", None, "unread"),
            "unread"
        );
    }

    /// A manifest that says nothing about a moved key has nothing to report,
    /// so an ordinary build prints no note at all.
    #[test]
    fn a_manifest_without_a_moved_key_has_nothing_to_say() {
        let manifest = Manifest::parse("[build]\ntarget = \"x86_64-linux\"\n").expect("parses");
        assert!(manifest.notes().is_empty());
    }

    /// A **withdrawn** key fails the build, and in its own words.
    ///
    /// The difference from the moved key above is the whole of why there are
    /// two lists ([ADR-050](../../../docs/specification/adr/adr-050.md) D7):
    /// `cleanup-deadline` still decides something somewhere else, so the
    /// manifest keeps working and says where to look, while `ordering` decides
    /// nothing anywhere — and a build that ignored it quietly would run a
    /// program that differs from the file describing it.
    ///
    /// It is also not reported as a typo, which is what removing it from
    /// `KNOWN` alone would have done: somebody who writes `ordering` meant it,
    /// and "expected one of: …" answers a question they did not ask.
    #[test]
    fn a_withdrawn_key_is_refused_saying_what_replaced_it() {
        let error = Manifest::parse("[build]\nordering = \"strict\"\n")
            .expect_err("a withdrawn key fails the build");
        let said = format!("{error:#}");
        assert!(said.contains("`ordering`"), "{said}");
        assert!(said.contains("withdrawn"), "{said}");
        assert!(said.contains("ADR-050"), "{said}");
        assert!(said.contains("overlap"), "the way out is named: {said}");
        assert!(
            !said.contains("expected one of"),
            "not reported as a typo: {said}"
        );
    }

    /// A bare `no` is TOML's boolean, and the reason it is wrong belongs to the
    /// switch's own parser rather than to a type error here.
    #[test]
    fn a_non_string_reaches_the_switch_that_can_explain_it() {
        let manifest = Manifest::parse("[build]\nuser-parallelism = false\n").expect("parses");
        assert_eq!(manifest.setting("user-parallelism", None, "no"), "false");
    }
}
