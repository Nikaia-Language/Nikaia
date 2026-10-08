//! Wrapping Cargo: the generated `Cargo.toml`, and the `rustc` interception.
//!
//! [ADR-002](../../../docs/specification/adr/adr-002.md) D1 decides this and
//! [ADR-003](../../../docs/specification/adr/adr-003.md) D2 decides where it
//! lives: the CLI wrapping and the Cargo instrumentation are the *generic*
//! half, so nothing below knows what a `.nika` file is. A frontend hands over a
//! package description and the extension its own sources carry; this renders
//! the manifest, runs `cargo`, and rewrites one argument on the way to `rustc`.
//!
//! Three properties are what the whole thing rests on, and each was measured
//! against a real `cargo` rather than assumed:
//!
//! * **`RUSTC_WORKSPACE_WRAPPER` reaches workspace members only.** That is the
//!   point of choosing it over `RUSTC_WRAPPER`: a dependency from crates.io is
//!   compiled by the real `rustc`, untouched, and the language's pipeline never
//!   sees a crate it did not write.
//! * **A generated file is only written when it changed** ([`write_if_changed`]).
//!   Cargo decides freshness by comparing mtimes against the fingerprint it
//!   wrote last time, so a generated source rewritten unconditionally is a
//!   package that is dirty on every build, for ever.
//! * **The source `cargo` thinks it compiled is not the source the user
//!   edited**, so the dependency file `rustc` emits names the wrong input.
//!   [`record_extra_dependencies`] puts the real ones back, and without it a
//!   changed source would not rebuild.

use anyhow::{Context, Result, anyhow};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `[package]` of the generated manifest.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub edition: String,
    /// The oldest Rust the **emitted** code compiles under
    /// ([ADR-288](../../../docs/specification/adr/adr-288.md) D27), written
    /// where Cargo reads it.
    ///
    /// It is a different fact from the channel: `rust-toolchain.toml` names
    /// that and stays the only place that does
    /// ([ADR-001](../../../docs/specification/adr/adr-001.md) D1), while this
    /// is a version the lowering needs. It rises only by a record — a later
    /// feature of the language below that some lowering asks for — and never
    /// silently.
    pub rust_version: Option<String>,
}

/// The Cargo profile the build switches and the codegen table decide.
///
/// ADR-002 D1: the build switches "set `opt-level` and the panic strategy from
/// there". `opt-level` and `lto` are a project's own choice about output size
/// and speed (Part III 13.3); `panic` is not a choice at all - it follows from
/// the machine, which is why it arrives here already decided. `incremental`
/// is the third choice of the codegen table (ADR-002 D5): absent, Cargo's
/// `dev` default holds and it is on.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub opt_level: Option<toml::Value>,
    pub lto: Option<toml::Value>,
    pub incremental: Option<toml::Value>,
    pub panic: Option<String>,
}

/// Which target a generated crate carries. The entry package is the binary; a
/// package it depends on is a library beside it
/// ([ADR-286](../../../docs/specification/adr/adr-286.md) D19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrateKind {
    Bin,
    Lib,
    /// **A library C calls** ([ADR-284](../../../docs/specification/adr/adr-284.md)
    /// D4): the entry package of a build with `artifact = "c-library"`, made a
    /// shared and a static library.
    CLibrary,
}

/// One member's `Cargo.toml` to be written, and the one target it builds.
///
/// A member, always: a build emits a workspace even where the program depends
/// on no Nikaia package at all
/// ([ADR-286](../../../docs/specification/adr/adr-286.md) D19), because one
/// shape that is always taken is worth more than a second one taken rarely.
/// So nothing here writes a profile - profiles are the root's, and a member
/// that declared one would be ignored with a warning.
#[derive(Debug, Clone)]
pub struct CargoProject {
    pub package: Package,
    /// Binary or library: the entry package is the program, everything it
    /// reaches is a library beside it.
    pub kind: CrateKind,
    /// The target's name, and the file Cargo is told is its crate root - which
    /// may carry any extension at all. Cargo does not require `.rs` there, and
    /// that is what lets the wrapper below get a look at it.
    pub bin_name: String,
    pub bin_path: PathBuf,
    /// Rendered verbatim. A native Rust dependency passes through unchanged
    /// (D1), which means this map holds exactly what the author wrote.
    pub dependencies: BTreeMap<String, toml::Value>,
    /// `[features]`, each feature and what it turns on. Empty unless the
    /// generated code tests a feature of its own crate - a `grammar!` expansion
    /// does, for `trace` - because `rustc` warns about a `cfg` value the crate
    /// never declared.
    pub features: BTreeMap<String, Vec<String>>,
}

impl CargoProject {
    /// The member's manifest text. Generated, so it says so: a file a build
    /// wrote is one a person will find in a diff and wonder about.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# GENERATED from `nikaia.toml`. Do not edit - it is rewritten by every build.\n\
             # Native Rust dependencies pass through unchanged (ADR-002 D1); a Nikaia one\n\
             # is the crate generated beside this, under this crate's own name for it\n\
             # (ADR-286 D20). The profile is the workspace root's.\n\n",
        );
        out.push_str("[package]\n");
        out.push_str(&format!("name = {}\n", string(&self.package.name)));
        out.push_str(&format!("version = {}\n", string(&self.package.version)));
        out.push_str(&format!("edition = {}\n", string(&self.package.edition)));
        if let Some(floor) = &self.package.rust_version {
            out.push_str(&format!("rust-version = {}\n", string(floor)));
        }

        match self.kind {
            CrateKind::Bin => out.push_str("\n[[bin]]\n"),
            CrateKind::Lib => out.push_str("\n[lib]\n"),
            CrateKind::CLibrary => {
                out.push_str("\n[lib]\ncrate-type = [\"cdylib\", \"staticlib\"]\n")
            }
        }
        out.push_str(&format!("name = {}\n", string(&self.bin_name)));
        out.push_str(&format!(
            "path = {}\n",
            string(&self.bin_path.to_string_lossy())
        ));

        out.push_str("\n[dependencies]\n");
        for (name, value) in &self.dependencies {
            out.push_str(&format!("{} = {value}\n", key(name)));
        }

        if !self.features.is_empty() {
            out.push_str("\n[features]\n");
            for (name, enables) in &self.features {
                let enables: Vec<String> = enables.iter().map(|e| string(e)).collect();
                out.push_str(&format!("{} = [{}]\n", key(name), enables.join(", ")));
            }
        }

        out
    }

    /// Writes the manifest into `dir`, and returns its path.
    pub fn write_to(&self, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating the build directory {}", dir.display()))?;
        let path = dir.join("Cargo.toml");
        write_if_changed(&path, &self.render())?;
        Ok(path)
    }
}

/// A generated Cargo **workspace**: one member crate per Nikaia package
/// ([ADR-286](../../../docs/specification/adr/adr-286.md) D19).
///
/// The root is virtual - it carries no package of its own, only the members and
/// the profile. That is what makes D4 expressible: the overflow checks are on
/// for each Nikaia crate **by name** and off for `"*"`, so a library of this
/// language gets them and a foreign crate does not. While everything was one
/// crate that came for free; here it is generated, and getting it wrong is a
/// library computing silently wrong numbers while its caller aborts.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Directory name under the build directory, and the manifest for it. The
    /// entry package is first.
    pub members: Vec<(String, CargoProject)>,
    pub profile_name: String,
    pub profile: Profile,
}

impl Workspace {
    /// The root manifest's text.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# GENERATED from `nikaia.toml`. Do not edit - it is rewritten by every build.\n\
             # One member per Nikaia package (ADR-286 D19); the profile is the root's,\n\
             # because a member's would be ignored.\n\n",
        );
        out.push_str("[workspace]\n");
        out.push_str("resolver = \"2\"\n");
        out.push_str("members = [");
        for (at, (dir, _)) in self.members.iter().enumerate() {
            if at > 0 {
                out.push_str(", ");
            }
            out.push_str(&string(dir));
        }
        out.push_str("]\n");
        if let Some((dir, _)) = self.members.first() {
            // So that `cargo build` and `cargo run` mean the program rather
            // than the program and every library under it.
            out.push_str(&format!("default-members = [{}]\n", string(dir)));
        }

        // **The overflow check is not in `Profile`, and that is the point.**
        // `Profile` holds what the build switches and the codegen table decide;
        // an overflow aborting is what the language *means* (ADR-285 D5), so it
        // is written from the two rules below and there is no key that turns it
        // off. Which is also why the emitted code says `a + b` rather than
        // calling a checked helper per operation (D6): the arithmetic is the
        // same either way, and a reader comparing the two languages should find
        // `+` where they wrote `+`.
        //
        // **Off here, and on per Nikaia crate below.** A hash function in a Rust
        // crate wraps on purpose; with the check on it would abort, and it is
        // not our code to be right or wrong about. So the default is the foreign
        // one - `"*"` would reach the members too - and each crate of this
        // language is named back onto the program's side.
        out.push_str(&format!("\n[profile.{}]\n", self.profile_name));
        out.push_str("overflow-checks = false\n");
        // **No debug assertions, in any build** (ADR-002 D5). The profile is
        // Cargo's `dev` because a Nikaia build has one profile, not because it
        // is a debug build, and `dev` would otherwise switch on `debug_assert!`
        // in every dependency and the standard library's precondition checks
        // in all code inlined into the program - nothing a Nikaia program
        // means, and a fifth of 1BRC's instructions (516 -> 438 per row).
        out.push_str("debug-assertions = false\n");
        if let Some(opt) = &self.profile.opt_level {
            out.push_str(&format!("opt-level = {opt}\n"));
        }
        if let Some(lto) = &self.profile.lto {
            out.push_str(&format!("lto = {lto}\n"));
        }
        if let Some(incremental) = &self.profile.incremental {
            out.push_str(&format!("incremental = {incremental}\n"));
        }
        if let Some(panic) = &self.profile.panic {
            out.push_str(&format!("panic = {}\n", string(panic)));
        }

        // **ADR-285 D5 reaches every crate of this language** (ADR-286 D22). A
        // Nikaia dependency is part of the program, not a foreign package, so
        // an overflow in it aborts exactly as one in the program does.
        for (_, member) in &self.members {
            out.push_str(&format!(
                "\n[profile.{}.package.{}]\noverflow-checks = true\n",
                self.profile_name,
                key(&member.package.name)
            ));
        }
        out
    }

    /// Writes the root manifest and every member's, and returns the root's path.
    pub fn write_to(&self, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating the build directory {}", dir.display()))?;
        for (name, member) in &self.members {
            member.write_to(&dir.join(name))?;
        }
        let path = dir.join("Cargo.toml");
        write_if_changed(&path, &self.render())?;
        Ok(path)
    }
}

/// A TOML basic string. Paths and names both go through it, because a build
/// directory with a quote in its name is not a reason to emit a broken file.
fn string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

/// A bare key where TOML allows one, a quoted key otherwise.
fn key(name: &str) -> String {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        name.to_string()
    } else {
        string(name)
    }
}

/// Write `contents` to `path`, but **only if that changes the file**.
///
/// Returns whether anything was written. The condition is not an optimisation:
/// Cargo decides a target is fresh by comparing the mtimes of the files its
/// dependency info names against the fingerprint it wrote, so a generated
/// source rewritten with identical bytes is newer than the fingerprint and the
/// package rebuilds - every time, for ever. Measured before it was believed:
/// with an unconditional write, `cargo build` twice in a row recompiles twice.
pub fn write_if_changed(path: &Path, contents: &str) -> Result<bool> {
    if let Ok(existing) = std::fs::read_to_string(path)
        && existing == contents
    {
        return Ok(false);
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

/// How `cargo` is invoked for a generated project.
#[derive(Debug, Clone)]
pub struct Cargo {
    pub manifest: PathBuf,
    /// Where Cargo puts its own artifacts. `None` leaves Cargo to its default,
    /// which is what lets `CARGO_TARGET_DIR` still be honoured.
    pub target_dir: Option<PathBuf>,
    /// The program Cargo runs in place of `rustc`, **for workspace members
    /// only** (ADR-002 D1).
    pub wrapper: PathBuf,
    /// Handed to the wrapper, since Cargo owns its argument list and nothing
    /// else can reach it.
    pub env: Vec<(String, OsString)>,
}

/// **Refuse an old toolchain in this compiler's words**
/// ([ADR-288](../../../docs/specification/adr/adr-288.md) D27).
///
/// Cargo's own answer is *package `x` requires rustc 1.88 or newer*, naming a
/// package the author never wrote — [Part III
/// C.1](../../../docs/specification/30-nikaia-tooling.md)'s class. So the
/// comparison happens before Cargo is handed anything.
///
/// **A version this cannot read is not a refusal.** `rustc --version` that
/// fails to run, or prints something this does not parse, says nothing: the
/// build goes on and Cargo answers if it must. Refusing on a reading failure
/// would refuse a correct toolchain, which is the worse of the two mistakes
/// (C.4).
pub fn toolchain_is_new_enough(floor: &str) -> Result<()> {
    let Some(found) = installed_rustc() else {
        return Ok(());
    };
    let Some(floor_parts) = version_parts(floor) else {
        return Ok(());
    };
    if found >= floor_parts {
        return Ok(());
    }
    let (major, minor) = found;
    Err(anyhow!(
        "this toolchain is rustc {major}.{minor}, and the Rust this compiler emits needs \
         {floor} or newer.\n\
         A trait method that may pause is written `-> impl Future<…>` (ADR-288 D26), which \
         is stable from {floor}. Install a newer toolchain - `rustup update stable` - or \
         pin one with `rust-toolchain.toml` (ADR-001 D1)."
    ))
}

/// **Refuse a target the toolchain has no `std` for, in this compiler's words**
/// ([ADR-037](../../../docs/specification/adr/adr-037.md) D1).
///
/// Cargo's own answer is *can't find crate for `core`*, about a crate the
/// author never named - the same class as the floor above. The answer that
/// helps is the command that installs it.
///
/// **Only a missing `std` is a refusal.** A `rustc` that does not run, or does
/// not know the triple, says nothing here, and the build goes on for Cargo to
/// answer: refusing on a reading failure would refuse a toolchain that works.
/// Neither is a linker, which a machine other than the host also needs and
/// which nothing here can see.
pub fn toolchain_can_build_for(triple: &str) -> Result<()> {
    let Ok(out) = Command::new("rustc")
        .current_dir(crate::cache::Layout::where_tools_start())
        .args(["--print", "target-libdir", "--target", triple])
        .output()
    else {
        return Ok(());
    };
    let Ok(text) = String::from_utf8(out.stdout) else {
        return Ok(());
    };
    let libdir = text.trim();
    if !out.status.success() || libdir.is_empty() {
        return Ok(());
    }
    let has_std = std::fs::read_dir(libdir)
        .map(|entries| {
            entries
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().starts_with("libstd-"))
        })
        .unwrap_or(false);
    if has_std {
        return Ok(());
    }
    Err(anyhow!(
        "cannot build for `{triple}`: this toolchain has no `std` for it.\n\
         Install it with `rustup target add {triple}`. A machine other than the one \
         this runs on also needs a linker for it, named to Cargo in \
         `$CARGO_HOME/config.toml` (a project's own is not read) as `[target.{triple}] linker = \"…\"`."
    ))
}

/// `rustc --version`'s major and minor, where both can be read.
fn installed_rustc() -> Option<(u32, u32)> {
    let out = Command::new("rustc")
        .current_dir(crate::cache::Layout::where_tools_start())
        .arg("--version")
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    // `rustc 1.86.0 (05f9846f8 2025-03-31)`
    version_parts(text.split_whitespace().nth(1)?)
}

/// `1.75` and `1.86.0` alike, as a pair that compares.
fn version_parts(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.split(['.', '-']);
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

impl Cargo {
    /// Runs `cargo <subcommand>`, inheriting stdio so the user sees Cargo's own
    /// progress and `rustc`'s diagnostics as they happen.
    ///
    /// `after` is passed through following `--`, which is how `cargo run` takes
    /// a program's arguments.
    pub fn run(&self, subcommand: &str, args: &[String], after: &[String]) -> Result<i32> {
        let mut command = Command::new(cargo_binary());
        command.current_dir(crate::cache::Layout::where_tools_start());
        command.arg(subcommand);
        command.arg("--manifest-path").arg(&self.manifest);
        if let Some(dir) = &self.target_dir {
            command.arg("--target-dir").arg(dir);
        }
        command.args(args);
        if !after.is_empty() {
            command.arg("--").args(after);
        }
        command.env("RUSTC_WORKSPACE_WRAPPER", &self.wrapper);
        for (name, value) in &self.env {
            command.env(name, value);
        }

        let status = command
            .status()
            .with_context(|| format!("running `cargo {subcommand}`"))?;
        Ok(status.code().unwrap_or(1))
    }

    /// The same, with `--message-format=json`, handing back what the backend
    /// said instead of letting it reach the terminal.
    ///
    /// **The one channel a frontend can read a backend diagnostic on.** Cargo's
    /// human-readable rendering goes to stderr already formatted and spanned
    /// against a generated file, which is the thing a language frontend must not
    /// let a user see; `--message-format=json` puts every `rustc` diagnostic on
    /// *stdout*, one JSON object per line, where something can trade its byte
    /// offsets for places in the source the user wrote.
    ///
    /// Only stdout is captured. Cargo's progress ("Compiling", "Finished") and
    /// anything it says about resolution stay on stderr and still reach the
    /// user as they happen, because none of that is about the program's text.
    ///
    /// Never used for `run`: the program's own output is on stdout too, and a
    /// compiler that swallowed it to read diagnostics would be reading the wrong
    /// thing at the worst moment.
    pub fn messages(&self, subcommand: &str, args: &[String]) -> Result<(i32, String)> {
        let mut command = Command::new(cargo_binary());
        command.current_dir(crate::cache::Layout::where_tools_start());
        command.arg(subcommand);
        command.arg("--manifest-path").arg(&self.manifest);
        if let Some(dir) = &self.target_dir {
            command.arg("--target-dir").arg(dir);
        }
        command.arg("--message-format=json");
        command.args(args);
        command.env("RUSTC_WORKSPACE_WRAPPER", &self.wrapper);
        for (name, value) in &self.env {
            command.env(name, value);
        }
        // **stdout piped, stderr inherited, and spelling both out is the point.**
        // `Command::output` pipes *both*, which would swallow Cargo's progress
        // and - worse - anything it says that is not a `rustc` diagnostic at
        // all: a manifest it cannot parse, a dependency it cannot resolve. Those
        // are not about the program's text, nobody translates them, and a build
        // that failed in silence is the one outcome worse than an untranslated
        // error.
        command.stdout(Stdio::piped());
        command.stderr(Stdio::inherit());

        let mut child = command
            .spawn()
            .with_context(|| format!("running `cargo {subcommand}`"))?;
        let mut messages = String::new();
        if let Some(stdout) = child.stdout.as_mut() {
            stdout
                .read_to_string(&mut messages)
                .context("reading what the backend said")?;
        }
        let status = child
            .wait()
            .with_context(|| format!("waiting for `cargo {subcommand}`"))?;
        Ok((status.code().unwrap_or(1), messages))
    }
}

/// The `cargo` to drive. `CARGO` is set by Cargo itself when the compiler is
/// run from a build script or a test, and naming the same one avoids driving a
/// second toolchain by accident.
fn cargo_binary() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

/// One `rustc` invocation, as Cargo handed it to the wrapper.
///
/// Cargo calls the wrapper as `wrapper <rustc> <args…>`, and does so for its
/// own probes (`rustc -vV`) as well as for real compiles. `source` is `None`
/// for everything that is not a compile of a file with the frontend's
/// extension, and such an invocation is passed straight through.
#[derive(Debug, Clone)]
pub struct Invocation {
    pub rustc: OsString,
    pub args: Vec<OsString>,
    /// The index in `args` of the crate root carrying the frontend's extension.
    pub source: Option<usize>,
    pub crate_name: Option<String>,
    pub out_dir: Option<PathBuf>,
    pub extra_filename: Option<String>,
}

impl Invocation {
    /// Splits what Cargo passed. `argv` is the wrapper's own arguments, the
    /// program name already removed.
    pub fn parse(argv: &[OsString], extension: &str) -> Option<Invocation> {
        let (rustc, args) = argv.split_first()?;
        let args: Vec<OsString> = args.to_vec();

        let mut invocation = Invocation {
            rustc: rustc.clone(),
            source: None,
            crate_name: None,
            out_dir: None,
            extra_filename: None,
            args,
        };

        let suffix = format!(".{extension}");
        for index in 0..invocation.args.len() {
            let arg = invocation.args[index].to_string_lossy().into_owned();
            match arg.as_str() {
                "--crate-name" => {
                    invocation.crate_name = invocation
                        .args
                        .get(index + 1)
                        .map(|v| v.to_string_lossy().into_owned());
                }
                "--out-dir" => {
                    invocation.out_dir = invocation.args.get(index + 1).map(PathBuf::from);
                }
                "-C" => {
                    if let Some(value) = invocation.args.get(index + 1) {
                        let value = value.to_string_lossy();
                        if let Some(rest) = value.strip_prefix("extra-filename=") {
                            invocation.extra_filename = Some(rest.to_string());
                        }
                    }
                }
                _ => {
                    if let Some(rest) = arg.strip_prefix("--out-dir=") {
                        invocation.out_dir = Some(PathBuf::from(rest));
                    } else if let Some(rest) = arg.strip_prefix("-Cextra-filename=") {
                        invocation.extra_filename = Some(rest.to_string());
                    } else if arg.ends_with(&suffix) && invocation.source.is_none() {
                        invocation.source = Some(index);
                    }
                }
            }
        }

        Some(invocation)
    }

    /// The crate root Cargo named, if it is one this frontend owns.
    pub fn source_path(&self) -> Option<PathBuf> {
        self.source.map(|index| PathBuf::from(&self.args[index]))
    }

    /// Puts a different file in the crate root's place.
    pub fn replace_source(&mut self, path: &Path) {
        if let Some(index) = self.source {
            self.args[index] = path.as_os_str().to_os_string();
        }
    }

    /// Where `rustc` will write its dependency info, if Cargo asked for any.
    ///
    /// Cargo names the file after the crate and the metadata hash it chose, so
    /// this is derived rather than searched for: a directory scan would pick up
    /// the previous build's file when `--emit` did not include `dep-info`.
    pub fn dep_info(&self) -> Option<PathBuf> {
        let dir = self.out_dir.as_ref()?;
        let name = self.crate_name.as_ref()?;
        let extra = self.extra_filename.clone().unwrap_or_default();
        Some(dir.join(format!("{name}{extra}.d")))
    }

    /// Runs the real `rustc`, and returns its exit code.
    ///
    /// **A `--print` probe is given an empty standard input**
    /// ([ADR-166](../../../docs/specification/adr/adr-166.md) D1). Cargo asks
    /// every wrapper what the target looks like by running
    /// `rustc - --print=… ` — `-` meaning *the program is on standard input* —
    /// and it writes nothing there, because it wants the `--print` answers and
    /// not a compile. Inherited, that standard input is **whoever started the
    /// build**, and anything sitting in it is read as a Rust program: the build
    /// then fails with *failed to run `rustc` to learn about target-specific
    /// information* and a parse error about text nobody offered as source.
    ///
    /// So the probe is handed `/dev/null` and every other invocation keeps the
    /// standard input it had. A real compile names a **file**, so nothing that
    /// reads a program this way loses one.
    pub fn run(&self) -> Result<i32> {
        let mut command = Command::new(&self.rustc);
        command.args(&self.args);
        if self.is_a_probe() {
            command.stdin(std::process::Stdio::null());
        }
        let status = command
            .status()
            .with_context(|| format!("running {}", self.rustc.to_string_lossy()))?;
        Ok(status.code().unwrap_or(1))
    }

    /// Whether this invocation is Cargo asking what the target looks like
    /// rather than asking for a compile
    /// ([ADR-166](../../../docs/specification/adr/adr-166.md) D1).
    ///
    /// Both halves are required: `-` is what makes `rustc` read standard input,
    /// and a `--print` is what makes the answer something other than a compiled
    /// program. Neither alone is the probe.
    pub fn is_a_probe(&self) -> bool {
        self.args.iter().any(|arg| arg == "-")
            && self.args.iter().any(|arg| {
                arg.to_str()
                    .is_some_and(|arg| arg == "--print" || arg.starts_with("--print="))
            })
    }
}

/// What Cargo *resolved*, read out of the `Cargo.lock` it wrote beside the
/// generated manifest.
///
/// The toolchain does not resolve versions and must not start
/// ([ADR-002](../../../docs/specification/adr/adr-002.md) D1) - but
/// [ADR-021](../../../docs/specification/adr/adr-021.md) D2 asks the lockfile to
/// record them, and until this existed they lived only in a generated file under
/// `target/` that nobody commits. So this reads Cargo's answer rather than
/// computing one: a measurement, in D4's sense, of what the declaration in
/// `nikaia.toml` turned into.
///
/// **The whole graph, not the direct dependencies.** The question the record
/// exists to answer is *does this build the same thing for you as for me*, and a
/// transitive crate that resolved differently on the two machines is a different
/// build. That is also why the root package is left out: the generated package is
/// the project, not something it depends on.
///
/// **A list of versions per name, because one name can resolve twice.** Two
/// majors of one crate in a graph is ordinary, and a map from name to a single
/// version would silently drop one of them - a record that quietly stops being
/// the whole answer, which is the failure D2 is about.
///
/// A `Cargo.lock` that is not there is not an error: a `cargo` subcommand that
/// resolves nothing writes none, and there is then nothing to record.
pub fn resolved_versions(
    cargo_lock: &Path,
    root_package: &str,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(cargo_lock) else {
        return Ok(out);
    };
    let document: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", cargo_lock.display()))?;

    let packages = document
        .get("package")
        .and_then(toml::Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or_default();

    for package in packages {
        let (Some(name), Some(version)) = (
            package.get("name").and_then(toml::Value::as_str),
            package.get("version").and_then(toml::Value::as_str),
        ) else {
            continue;
        };
        if name == root_package {
            continue;
        }
        out.entry(name.to_string())
            .or_default()
            .insert(version.to_string());
    }

    Ok(out)
}

/// Adds `sources` to a `rustc` dependency file as prerequisites of everything
/// it lists as a target.
///
/// This is what keeps `cargo build` honest. `rustc` compiled a *generated*
/// file, so its dependency info names that and nothing else - and Cargo
/// believes it, which means editing the source the user actually wrote would
/// leave the package fresh and the binary stale. The sources go back in here,
/// in Make's own syntax, because that is the file Cargo reads.
///
/// A dependency file that is not there is not an error: `--emit` may simply not
/// have asked for one, and the build has already succeeded by this point.
pub fn record_extra_dependencies(dep_info: &Path, sources: &[PathBuf]) -> Result<()> {
    let Ok(text) = std::fs::read_to_string(dep_info) else {
        return Ok(());
    };
    if sources.is_empty() {
        return Ok(());
    }

    let extra: Vec<String> = sources
        .iter()
        .map(|path| escape_make(&path.to_string_lossy()))
        .collect();

    let mut out = String::new();
    for line in text.lines() {
        // `target: prerequisites` is a rule, and the sources join its
        // prerequisites. A line that starts with whitespace is a continuation,
        // and a bare `file:` line is the empty target `rustc` writes so Make
        // tolerates a deleted input - both are copied through untouched.
        let is_rule = !line.starts_with(char::is_whitespace) && line.contains(": ");
        out.push_str(line);
        if is_rule {
            for source in &extra {
                out.push(' ');
                out.push_str(source);
            }
        }
        out.push('\n');
    }
    for source in &extra {
        out.push_str(source);
        out.push_str(":\n");
    }

    std::fs::write(dep_info, out)
        .with_context(|| format!("recording sources in {}", dep_info.display()))
}

/// Make treats a space as a separator, so a path containing one is escaped.
fn escape_make(path: &str) -> String {
    path.replace(' ', "\\ ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Cargo's target-info probe, and a real compile, told apart**
    /// ([ADR-166](../../../docs/specification/adr/adr-166.md) D1).
    ///
    /// The argv is the one that failed builds for months, copied out of the
    /// error it produced. Both halves have to be present: `-` is what makes
    /// `rustc` read standard input at all, and a `--print` is what says the
    /// answer is not a compiled program.
    #[test]
    fn a_target_info_probe_is_told_from_a_compile() {
        let probe: Vec<OsString> = [
            "/usr/bin/rustc",
            "-",
            "--crate-name",
            "___",
            "--print=file-names",
            "--crate-type",
            "bin",
            "--print=sysroot",
            "--print=cfg",
            "-Wwarnings",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let probe = Invocation::parse(&probe, "nika").expect("a rustc is named");
        assert!(probe.is_a_probe());

        let compile: Vec<OsString> = [
            "/usr/bin/rustc",
            "--crate-name",
            "hyper_core",
            "src/main.nika",
            "--crate-type",
            "bin",
            "--out-dir",
            "/tmp/out",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let compile = Invocation::parse(&compile, "nika").expect("a rustc is named");
        assert!(!compile.is_a_probe());
    }

    /// **Neither half alone is the probe.** A compile that happens to ask for
    /// one `--print` still names a file, and a `-` with no `--print` is
    /// somebody genuinely handing `rustc` a program on standard input — which
    /// this must not take away.
    #[test]
    fn one_half_is_not_a_probe() {
        let printing: Vec<OsString> = ["/usr/bin/rustc", "src/main.nika", "--print=cfg"]
            .iter()
            .map(OsString::from)
            .collect();
        assert!(
            !Invocation::parse(&printing, "nika")
                .expect("a rustc is named")
                .is_a_probe()
        );

        let from_stdin: Vec<OsString> = ["/usr/bin/rustc", "-", "--crate-type", "bin"]
            .iter()
            .map(OsString::from)
            .collect();
        assert!(
            !Invocation::parse(&from_stdin, "nika")
                .expect("a rustc is named")
                .is_a_probe()
        );
    }

    fn project() -> CargoProject {
        let mut dependencies = BTreeMap::new();
        dependencies.insert(
            "regex".to_string(),
            toml::Value::try_from(BTreeMap::from([("version", "1.5")])).expect("a table"),
        );
        CargoProject {
            package: Package {
                name: "hyper-core".into(),
                version: "0.1.0".into(),
                edition: "2021".into(),
                rust_version: None,
            },
            kind: CrateKind::Bin,
            bin_name: "hyper-core".into(),
            bin_path: PathBuf::from("/p/src/main.nika"),
            dependencies,
            features: BTreeMap::new(),
        }
    }

    fn library(name: &str) -> CargoProject {
        CargoProject {
            package: Package {
                name: name.into(),
                version: "0.1.0".into(),
                edition: "2021".into(),
                rust_version: None,
            },
            kind: CrateKind::Lib,
            bin_name: name.replace('-', "_"),
            bin_path: PathBuf::from(format!("/p/{name}/src/main.nika")),
            dependencies: BTreeMap::new(),
            features: BTreeMap::new(),
        }
    }

    fn workspace() -> Workspace {
        Workspace {
            members: vec![
                ("hyper-core".to_string(), project()),
                ("maths".to_string(), library("maths")),
            ],
            profile_name: "dev".into(),
            profile: Profile {
                opt_level: Some(toml::Value::Integer(3)),
                lto: Some(toml::Value::Boolean(true)),
                incremental: Some(toml::Value::Boolean(false)),
                panic: Some("unwind".into()),
            },
        }
    }

    /// The whole of D1's first clause: what comes out has to be a manifest
    /// Cargo accepts, with the Rust dependency exactly as it was written.
    #[test]
    fn the_generated_manifest_is_valid_toml_and_keeps_the_dependency() {
        let text = project().render();
        let parsed: toml::Value = toml::from_str(&text).expect("the generated manifest parses");

        assert_eq!(parsed["package"]["name"].as_str(), Some("hyper-core"));
        assert_eq!(
            parsed["dependencies"]["regex"]["version"].as_str(),
            Some("1.5")
        );
        assert_eq!(parsed["bin"][0]["path"].as_str(), Some("/p/src/main.nika"));

        // **A member writes no profile** (ADR-286 D19): the root's is the one
        // Cargo reads, and one written here would be ignored with a warning -
        // which is a build telling somebody their overflow setting did nothing.
        assert!(
            parsed.get("profile").is_none(),
            "a member's profile is the root's:\n{text}"
        );
        assert!(
            parsed.get("workspace").is_none(),
            "and the workspace is the root's too:\n{text}"
        );
    }

    /// A package a program depends on is a **library** crate beside it
    /// ([ADR-286](../../../docs/specification/adr/adr-286.md) D19), so the two
    /// differ in exactly one place: the target table.
    #[test]
    fn a_dependency_is_a_library_and_the_entry_is_a_binary() {
        let program: toml::Value = toml::from_str(&project().render()).expect("parses");
        let library: toml::Value = toml::from_str(&library("maths").render()).expect("parses");

        assert!(program.get("bin").is_some() && program.get("lib").is_none());
        assert!(library.get("lib").is_some() && library.get("bin").is_none());
    }

    /// ADR-002 D5: `incremental` is written only where the codegen table says
    /// it, so a project that says nothing keeps Cargo's `dev` default - on.
    #[test]
    fn incremental_is_written_only_where_the_table_says_it() {
        let mut silent = workspace();
        silent.profile.incremental = None;
        let parsed: toml::Value = toml::from_str(&silent.render()).expect("parses");
        assert!(parsed["profile"]["dev"].get("incremental").is_none());

        let parsed: toml::Value = toml::from_str(&workspace().render()).expect("parses");
        assert_eq!(
            parsed["profile"]["dev"]["incremental"].as_bool(),
            Some(false)
        );
    }

    /// The workspace root: the members, the profile, and
    /// [ADR-285](../../../docs/specification/adr/adr-285.md) D8 written out per
    /// crate ([ADR-286](../../../docs/specification/adr/adr-286.md) D22).
    #[test]
    fn the_workspace_root_carries_the_profile_and_the_overflow_checks() {
        let text = workspace().render();
        let parsed: toml::Value = toml::from_str(&text).expect("the root manifest parses");

        assert_eq!(
            parsed["workspace"]["members"]
                .as_array()
                .map(|m| m.iter().filter_map(toml::Value::as_str).collect::<Vec<_>>()),
            Some(vec!["hyper-core", "maths"])
        );
        assert_eq!(
            parsed["workspace"]["default-members"][0].as_str(),
            Some("hyper-core"),
            "`cargo build` means the program, not the program and every library \
             under it:\n{text}"
        );

        assert_eq!(parsed["profile"]["dev"]["opt-level"].as_integer(), Some(3));
        assert_eq!(parsed["profile"]["dev"]["lto"].as_bool(), Some(true));
        assert_eq!(
            parsed["profile"]["dev"]["incremental"].as_bool(),
            Some(false)
        );
        assert_eq!(parsed["profile"]["dev"]["panic"].as_str(), Some("unwind"));
        assert_eq!(
            parsed["profile"]["dev"]["debug-assertions"].as_bool(),
            Some(false),
            "the one profile is not a debug build (ADR-002 D5):\n{text}"
        );

        // ADR-285 D8 in the shape D4 gives it: off by default, because `"*"`
        // would reach the members too, and on by name for every crate of this
        // language. Neither is a key anybody may set. Asserted here as well as
        // end to end in `crates/nikaia/tests/overflow.rs`, because this is where
        // the text is written and that is where it is felt.
        assert_eq!(
            parsed["profile"]["dev"]["overflow-checks"].as_bool(),
            Some(false)
        );
        for crate_name in ["hyper-core", "maths"] {
            assert_eq!(
                parsed["profile"]["dev"]["package"][crate_name]["overflow-checks"].as_bool(),
                Some(true),
                "`{crate_name}` is this language's code:\n{text}"
            );
        }
    }

    /// Cargo's freshness is mtime against mtime, so an unconditional write is a
    /// package that never goes fresh.
    #[test]
    fn an_unchanged_file_is_not_rewritten() {
        let dir = std::env::temp_dir().join(format!("nikaia-write-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("Cargo.toml");

        assert!(write_if_changed(&path, "a").expect("write"), "first write");
        assert!(
            !write_if_changed(&path, "a").expect("write"),
            "identical contents are not written again"
        );
        assert!(
            write_if_changed(&path, "b").expect("write"),
            "changed contents are"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    fn argv(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_compile_of_a_frontend_source_is_recognised_and_rewritten() {
        let mut invocation = Invocation::parse(
            &argv(&[
                "/bin/rustc",
                "--crate-name",
                "demo",
                "--edition=2021",
                "src/main.nika",
                "--out-dir",
                "/t/deps",
                "-C",
                "extra-filename=-abc123",
            ]),
            "nika",
        )
        .expect("an invocation");

        assert_eq!(
            invocation.source_path(),
            Some(PathBuf::from("src/main.nika"))
        );
        assert_eq!(
            invocation.dep_info(),
            Some(PathBuf::from("/t/deps/demo-abc123.d"))
        );

        invocation.replace_source(Path::new("/t/gen/demo.rs"));
        assert_eq!(
            invocation.source_path(),
            Some(PathBuf::from("/t/gen/demo.rs"))
        );
    }

    /// Cargo probes the compiler through the wrapper too (`rustc -vV`), and an
    /// invocation with nothing of ours in it must be passed through untouched.
    #[test]
    fn a_probe_carries_no_source_and_is_left_alone() {
        let invocation = Invocation::parse(&argv(&["/bin/rustc", "-vV"]), "nika").expect("parses");
        assert_eq!(invocation.source, None);
        assert_eq!(invocation.dep_info(), None);
    }

    /// Without this the build is silently stale: Cargo reads the file `rustc`
    /// wrote, which names the generated Rust and never the source.
    #[test]
    fn the_real_sources_go_back_into_the_dependency_file() {
        let dir = std::env::temp_dir().join(format!("nikaia-depinfo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("demo.d");
        std::fs::write(&path, "/t/demo: /t/gen/demo.rs\n\n/t/gen/demo.rs:\n").expect("write");

        record_extra_dependencies(&path, &[PathBuf::from("/p/src/main.nika")]).expect("record");
        let text = std::fs::read_to_string(&path).expect("read");

        assert!(
            text.lines()
                .next()
                .expect("a rule line")
                .ends_with("/p/src/main.nika"),
            "the source is a prerequisite of the binary:\n{text}"
        );
        assert!(
            text.contains("\n/p/src/main.nika:\n"),
            "and has a target line of its own, so a deleted file is not a Make error:\n{text}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// ADR-021 D2's resolved versions, read from Cargo's own answer.
    ///
    /// Two properties that are easy to get wrong and invisible afterwards: the
    /// root package is the project rather than a dependency of it, and **two
    /// versions of one name are both kept**. A graph holding two majors of one
    /// crate is ordinary, and a map from name to a single version would drop
    /// one of them - a record that silently answers less than it claims to.
    #[test]
    fn what_cargo_resolved_is_read_whole() {
        let dir = std::env::temp_dir().join(format!("nikaia-cargolock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("Cargo.lock");
        std::fs::write(
            &path,
            "version = 4\n\n\
             [[package]]\nname = \"greeter\"\nversion = \"0.1.0\"\n\n\
             [[package]]\nname = \"regex\"\nversion = \"1.13.1\"\n\
             source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n\
             [[package]]\nname = \"windows-sys\"\nversion = \"0.52.0\"\n\n\
             [[package]]\nname = \"windows-sys\"\nversion = \"0.48.0\"\n",
        )
        .expect("write");

        let resolved = resolved_versions(&path, "greeter").expect("reads");
        assert!(
            !resolved.contains_key("greeter"),
            "the root package is the project, not something it depends on: {resolved:?}"
        );
        assert_eq!(
            resolved["regex"],
            BTreeSet::from(["1.13.1".to_string()]),
            "the version Cargo chose, not the constraint the author wrote"
        );
        assert_eq!(
            resolved["windows-sys"],
            BTreeSet::from(["0.48.0".to_string(), "0.52.0".to_string()]),
            "one name can resolve twice, and both versions determine the build"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A `cargo` subcommand that resolved nothing wrote no lockfile, and there
    /// is then nothing to record - not an error to fail a finished build with
    /// (ADR-021 D12).
    #[test]
    fn a_missing_cargo_lock_records_nothing() {
        let resolved =
            resolved_versions(Path::new("/nonexistent/Cargo.lock"), "greeter").expect("no error");
        assert!(resolved.is_empty());
    }
}
