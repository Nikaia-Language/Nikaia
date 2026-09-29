// crates/nikaia/src/main.rs
//
// The whole compiler builds on stable: nothing here needs an unstable feature,
// and the Rust this binary emits needs none either (ADR-001 D1).

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use nikaia::emit;
use nikaia::manifest::Manifest;
use nikaia::project::{self, Project, Settings};
use nikaia::sysroot::{self, Sysroot};
use nikaia::{diagnostics, interpreter, parser, refuse};

/// The Nikaia compiler: `nikaia build` for a project, `--input` for one file.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// A project command (ADR-002 D1). Without one, `--input` compiles a single
    /// file - the shape the compiler had before there were projects, and the
    /// one every `.nika` file outside a project still uses.
    #[command(subcommand)]
    pub command: Option<Command>,

    #[arg(short, long)]
    pub input: Option<PathBuf>,

    /// `interpreter` or `rust` (the default).
    ///
    /// `rust` is what a bare invocation uses and is the only code generator
    /// (ADR-004 D1): a `.nika` file becomes Rust source text, which `rustc`
    /// then compiles. `interpreter` runs the program instead of emitting
    /// anything (ADR-002 D2).
    ///
    /// `cranelift` and `llvm` are named by ADR-002 but not implemented; they
    /// are rejected rather than silently treated as something else
    /// (ADR-021 D9).
    #[arg(long, default_value = "rust")]
    pub backend: String,

    /// The machine to build for (ADR-037 D1): `x86_64-linux`,
    /// `aarch64-linux` or `wasm32-unknown`. It decides what `std` can offer and
    /// what a panic does, and nothing about what a program means.
    ///
    /// Overrides `nikaia.toml`'s `[build] target` for this one build; the
    /// default is the machine the compiler runs on where neither says (D5).
    #[arg(long, global = true)]
    pub target: Option<String>,

    /// Whether *your* code may run concurrently at all (ADR-037 D2): `yes`
    /// or `no`.
    ///
    /// Not a count - how many threads serve a `yes` is the runtime's to
    /// decide. And it bounds the program, not the compiler: `fs::map` may
    /// still validate its text on several cores at `no`, because that is not
    /// code you wrote and it changes nothing the program prints.
    ///
    /// Overrides `nikaia.toml`'s `[build] user-parallelism`; the default is
    /// `no` where neither says (D5).
    #[arg(long, global = true)]
    pub user_parallelism: Option<String>,

    /// Where the `rust` backend writes. Defaults to `<input>.rs`.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Read `rustc --error-format=json` on stdin and report it against the
    /// `.nika` source instead of the emitted Rust (ADR-012).
    ///
    /// The map is not an artifact: the lowering is deterministic, so it is
    /// rebuilt from `--input` here rather than written out and kept in step.
    #[arg(long)]
    pub explain: bool,

    /// Verify the Borrow Contract Ledger instead of updating it (Part III,
    /// 13.5).
    ///
    /// The ledger is a pure function of source and toolchain, so this compares
    /// bytes: any difference fails the build and prints what changed. The
    /// recommended CI line, and the reason the file is committed.
    #[arg(long, global = true)]
    pub locked: bool,

    /// Lower from scratch, ignoring the build cache (ADR-021).
    ///
    /// The cache is **on**: reusing an unchanged lowering is the difference in
    /// feel between a Nikaia build and a Rust one, and a default nobody types
    /// is not that. This flag exists for the times that matters less than
    /// seeing the emitter run - debugging it, or comparing its output against
    /// what the cache holds.
    #[arg(long, global = true)]
    pub no_cache: bool,

    /// Print which adjacent statements run together and why the rest do not
    /// (ADR-033 D9).
    ///
    /// Nikaia has no `allow_parallel`: the overlap is the default and the
    /// refusals are the compiler's own. That is only fair if the refusals can
    /// be asked about, and this is the asking. Like `--trust`, it explains a
    /// decision rather than changing one.
    #[arg(long, global = true)]
    pub overlaps: bool,

    /// Print which reference count each `Shared` value gets, and why
    /// ([ADR-037](../../../docs/specification/adr/adr-037.md) D7).
    ///
    /// **An explanation and not a switch.** The atomic count is the floor (D6)
    /// and the analysis only ever takes one away, where it can prove nothing
    /// crosses a thread with a value. Nikaia has no way to *ask* for the cheaper
    /// count - D8 enumerates every fallback and answers "would an override
    /// help?" no for all of them - and that is only fair if the fallbacks can be
    /// asked about. This is the asking, and it is what a person reads when they
    /// want the 9 ns back. Like `--trust` and `--overlaps`, it explains a
    /// decision rather than changing one.
    #[arg(long, global = true)]
    pub sharing: bool,

    /// Print which of Part I 6.6's states each view in a signature is in
    /// ([ADR-008](../../../docs/specification/adr/adr-008.md) D6).
    ///
    /// **The half of D6 that survived, and the reason the state lands in the
    /// ledger** (D7). The assertion beside it — a word a struct wrote to forbid
    /// a transition — is gone
    /// ([ADR-201](../../../docs/specification/adr/adr-201.md) D1); this shows
    /// what the compiler solved without being asked, so a change in a
    /// representation is a ledger diff in review rather than a surprise in a
    /// profile.
    ///
    /// Like `--trust`, `--overlaps` and `--sharing`, it explains a decision
    /// rather than changing one — and today it explains one that changes no
    /// lowering, because only one of the three states is built.
    #[arg(long, global = true)]
    pub tethers: bool,

    /// Print every `assert`, and for each whether the compiler proved it,
    /// refuted it or left it to run time
    /// ([ADR-245](../../../docs/specification/adr/adr-245.md) D6).
    ///
    /// Nothing proves a claim yet, so every row says *run time*; the report is
    /// here so that a prover's progress is visible, and so that a reader can
    /// ask why a check is still emitted.
    #[arg(long, global = true)]
    pub asserts: bool,

    /// Print what a `T::fields` loop was unrolled to, for the types actually
    /// used ([ADR-088](../../../docs/specification/adr/adr-088.md) D6,
    /// [ADR-181](../../../docs/specification/adr/adr-181.md)).
    ///
    /// **The one readability problem every build-time system shares** is that
    /// you cannot see what a function becomes for a given type without
    /// unrolling it in your head. The usual answer is to invent syntax; this
    /// project already has the other one, and `--overlaps`, `--sharing`,
    /// `--tethers` and `--trust` are it. So this is the same information
    /// [ADR-181](../../../docs/specification/adr/adr-181.md) D3's diagnostic
    /// carries, offered **on demand instead of on failure** (D5).
    ///
    /// Like the other four, it explains a decision rather than changing one.
    #[arg(long, global = true)]
    pub comptime: bool,

    /// The file that names the files this build may read while it builds
    /// ([ADR-072](../../../docs/specification/adr/adr-072.md) D2).
    ///
    /// **Without it a build reads nothing** (D1), and that is the whole of why
    /// the default is worth having: *this build reads nothing while building*
    /// is what happens when nothing is passed, rather than a claim somebody has
    /// to make, keep true, and be believed about.
    ///
    /// One path per line, `#` begins a comment. A file has to be named in all
    /// three places — this flag, that list, and the `asset("…")` literal — and
    /// they are deliberately not derivable from one another (D3). Two of the
    /// three are committed, so a change to either is a diff in review; this one
    /// is not, which is what lets a build run with the reads switched off
    /// without editing anything.
    #[arg(long, global = true, value_name = "FILE")]
    pub allow_read_from_list: Option<PathBuf>,

    /// Print where this program's bytes came from and which hash its maps got
    /// (ADR-010 D7).
    ///
    /// The choice is visible, never a mystery: this names every source the
    /// program reads and what each one contributed.
    #[arg(long, global = true)]
    pub trust: bool,
}

/// The project commands of Part III 13.2.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Compile the project in this directory (Part III 13.2).
    ///
    /// `nikaia.toml` is translated to a `Cargo.toml` and `cargo` builds it,
    /// which is how a Nikaia project reaches crates.io without this toolchain
    /// resolving a single version itself (ADR-002 D1).
    Build {
        /// The project directory. Defaults to the working directory, and the
        /// search walks up from there to the nearest `nikaia.toml`.
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Compile the project and run it.
    ///
    /// Everything after `--` is the program's own arguments.
    Run {
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Run the project's `test` blocks, each in a process of its own
    /// ([ADR-245](../../../docs/specification/adr/adr-245.md) D1, D7).
    ///
    /// The program is built once with its tests compiled in; each test then
    /// runs by itself, and a failing one does not stop the others.
    Test {
        #[arg(long)]
        project: Option<PathBuf>,
        /// Run every test at `user_parallelism = no` and at `yes`, and fail a
        /// test whose outcome differs between them: a program means the same
        /// at both (Part I 1.2), so a difference is a compiler fault (D7).
        #[arg(long)]
        both_settings: bool,
        /// Write each output test's expectations from what the program did:
        /// `NAME.stdout`, and the files its `NAME.out/` names
        /// ([ADR-247](../../../docs/specification/adr/adr-247.md) D3). Only a
        /// run that ended successfully, and at `--both-settings` only one whose
        /// two runs agree. Review the change in version control.
        #[arg(long)]
        bless: bool,
    },
    /// Re-lower the sysroot's `std` from its `.nika` sources (ADR-002 D4).
    ///
    /// The release step, and the *only* way `std`'s Nikaia half is lowered. It
    /// is a command on this **binary** on purpose: the alternative was
    /// `nikaia-std` linking the compiler as a build dependency, which made Cargo
    /// build the compiler a second time inside every project's `target/`.
    ///
    /// A binary install never needs to run it - the `.rs` ships beside the
    /// `.nika`. A from-source install may.
    LowerStd {
        /// The sysroot. Defaults to `NIKAIA_SYSROOT`, or the checkout this
        /// compiler was built from.
        #[arg(long)]
        sysroot: Option<PathBuf>,
    },
    /// Write a draft ledger for a Rust crate's boundary
    /// ([ADR-104](../../../docs/specification/adr/adr-104.md) D2).
    ///
    /// The command `NK2504` names. It reads the crate's `pub` signatures,
    /// translates them by Part III 15.2's table, and writes
    /// `contracts/<crate>.contracts` — which is then **committed and reviewed
    /// like code**: what a signature cannot say is written fail-closed, and a
    /// `?` in the draft is a person's to fill (D5).
    Describe {
        /// The crate, under the name a program writes: `hyper-shim` in the
        /// manifest is `hyper_shim` here, because that is the crate name Cargo
        /// makes of the key.
        #[arg(value_name = "CRATE")]
        crate_name: String,
        /// The project directory. Defaults to the working directory, and the
        /// search walks up from there to the nearest `nikaia.toml`.
        #[arg(long)]
        project: Option<PathBuf>,
    },
}

/// What this build can be asked for, for a refusal to name (ADR-021 D9).
///
/// Every build of this compiler has both, so the list does not vary - but a
/// refusal still prints it, because a reader told only *no* has to guess.
const AVAILABLE: &str = "interpreter, rust";

/// `rustc --error-format=json … | nikaia --input x.nika --explain`
///
/// Every message rustc reports about the emitted file is placed back in the
/// `.nika` it came from. The text is left alone: because the lowering is name
/// for name (ADR-011 D2), only the position was ever wrong.
fn explain(input: &std::path::Path, args: &Cli, settings: &Settings, source: &str) -> Result<()> {
    use std::io::Read;

    // The same switches the build used, or the map would point into a file this
    // run did not emit - which is why they are resolved once and passed in.
    let parsed = parser::parse_to_ast(source)?;
    let lowered = emit::emit_program(&parsed, settings.build)?;

    let mut rustc_json = String::new();
    std::io::stdin().read_to_string(&mut rustc_json)?;

    let generated = args
        .output
        .clone()
        .unwrap_or_else(|| input.with_extension("rs"));
    let path = input.display().to_string();
    let generated = generated.display().to_string();

    let diagnostics = diagnostics::translate(&rustc_json, &lowered.map, source);
    let errors = diagnostics.iter().filter(|d| d.level == "error").count();

    for diagnostic in diagnostics
        .iter()
        .filter(|d| diagnostics::is_about_the_program(d))
    {
        print!(
            "{}",
            diagnostics::render(diagnostic, &path, source, &generated)
        );
    }

    if errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// The single-file `rust` backend: one `.nika` entry to one `.rs` file.
///
/// Unchanged by the project build, deliberately. A one-off transformation of a
/// file that belongs to no project is a real thing to want (ADR-021 D11), and
/// it is what every test that drives the emitter uses.
fn lower_to_rust(input: &std::path::Path, args: &Cli, settings: &Settings) -> Result<()> {
    let output_path = args
        .output
        .clone()
        .unwrap_or_else(|| input.with_extension("rs"));

    // `--trust`, `--overlaps` and `--sharing` are explanations, so they are
    // answered before the cache is consulted: a build that reuses a cached
    // lowering still answers the question, and the answer cannot differ from the
    // one that lowering was built with.
    //
    // Through `project::explain`, which is the same function `nikaia build` uses
    // (`docs/open-work.md`, the explain modes): a report that said one thing here and another
    // there would be worse than one that only existed in one place.
    project::explain(
        &nikaia::modules::Program::read_one(input)?,
        settings,
        project::Explain {
            overlaps: args.overlaps,
            sharing: args.sharing,
            tethers: args.tethers,
            trust: args.trust,
            comptime: args.comptime,
            asserts: args.asserts,
        },
    )?;

    // No packages: `--input` is one file outside a project (ADR-047 D1), and a
    // dependency is declared in a manifest there is none of.
    let lowered = project::lower_reading(
        input,
        settings,
        args.no_cache,
        &[],
        args.allow_read_from_list.as_deref(),
    )?;
    std::fs::write(&output_path, &lowered.rust)?;

    // The ledger goes beside the output, because that is where a build puts
    // what it produced. Part III 13.5 says the project root, which is where
    // `nikaia build` puts it.
    //
    // This runs on a hit too: `--locked` asks whether the *committed* file
    // still matches, and a cached build has as much to answer for there as a
    // fresh one.
    let ledger_path = output_path.with_file_name("nikaia.contracts");
    project::write_ledger(&ledger_path, &lowered.ledger, args.locked)?;
    project::write_ledger(
        &nikaia::contracts::derived_path(&ledger_path),
        &lowered.derived,
        args.locked,
    )?;

    println!(
        "Lowered {} to {} (target: {}{})",
        input.display(),
        output_path.display(),
        settings.target,
        if lowered.reused {
            ", lowering from cache"
        } else {
            ""
        }
    );

    Ok(())
}

/// `nikaia lower-std` (ADR-002 D4): `std`'s `.nika` half to the `.rs` beside it.
fn lower_std(sysroot: Option<PathBuf>) -> Result<i32> {
    let sysroot = match sysroot {
        Some(root) => Sysroot::new(root),
        None => Sysroot::resolve(),
    };
    let changed = sysroot::lower_std(&sysroot)?;
    if changed.is_empty() {
        println!(
            "{} is already what this compiler lowers its `.nika` sources to.",
            sysroot.std_dir().display()
        );
    }
    for path in &changed {
        println!("Lowered to {}", path.display());
    }
    Ok(0)
}

/// `nikaia describe <crate>` ([ADR-104](../../../docs/specification/adr/adr-104.md) D2).
///
/// **What it prints is what a reviewer does next**, which is the whole reason
/// the command exists rather than the file appearing during a build: the draft
/// is read before it is believed, and a line that only said *written* would
/// leave the reader to find out what is in it.
fn describe(crate_name: &str, project: Option<PathBuf>) -> Result<i32> {
    let start = match project {
        Some(directory) => directory,
        None => std::env::current_dir().context("finding the working directory")?,
    };
    // The same walk `--input` makes, from a directory rather than from a file:
    // a crate is described for a **project**, because the project is what
    // declares it (ADR-104 D1).
    let start = start.canonicalize().unwrap_or(start);
    let root = start
        .ancestors()
        .find(|dir| dir.join("nikaia.toml").is_file())
        .map(std::path::Path::to_path_buf)
        .with_context(|| {
            format!(
                "no `nikaia.toml` at or above {} - a crate is described for a project, \
                 because the project is what declares it (ADR-104 D1)",
                start.display()
            )
        })?;
    let written = nikaia::describe::describe(&root, crate_name)?;
    println!(
        "Wrote {} for {crate_name} {}: {} function{}, {} type{}",
        written.path.display(),
        written.version,
        written.functions,
        match written.functions {
            1 => "",
            _ => "s",
        },
        written.types,
        match written.types {
            1 => "",
            _ => "s",
        },
    );
    if !written.unanswered.is_empty() {
        println!(
            "\n{} name{} the program writes that no `pub` signature answered - each is a \n\
             `?` for a reviewer to fill, or a name a macro wrote (ADR-104 D4, D5):",
            written.unanswered.len(),
            match written.unanswered.len() {
                1 => "",
                _ => "s",
            }
        );
        for name in &written.unanswered {
            println!("    {name}");
        }
    }
    // **The two things the describer saw and did not claim**
    // ([ADR-193](../../docs/specification/adr/adr-193.md) D3, D5). They are in
    // the file as comments, where the reviewer meets them; they are said here
    // too, because a person who runs a command reads what it printed and may
    // not open the file at all.
    if !written.notes.about_the_crate.is_empty() {
        println!(
            "\nThis crate makes a promise the toolchain cannot check, and the file says \n\
             where (ADR-193 D5). A tool can see that the promise was made; whether it is \n\
             true is the line between a rule enforced and a rule inherited."
        );
    }
    let proposed = written.notes.about_a_function.len();
    if proposed > 0 {
        println!(
            "\n{proposed} entr{} {} something the signature does not say - a `Send` bound, \n\
             or a thread sink the calls reach - and the file asks beside {}: does this \n\
             put what it is given on a thread? The describer proposes and never claims \n\
             (ADR-193 D3, D4).",
            match proposed {
                1 => "y",
                _ => "ies",
            },
            match proposed {
                1 => "carries",
                _ => "carry",
            },
            match proposed {
                1 => "it",
                _ => "each",
            },
        );
    }
    println!(
        "\nRead it before you believe it: what a signature cannot say is written \n\
         fail-closed, and what it says wrongly is caught here or by nobody (ADR-104 D5)."
    );
    Ok(0)
}

/// `nikaia build` and `nikaia run` (Part III 13.2).
fn project_command(args: &Cli, command: &Command) -> Result<i32> {
    let (subcommand, directory, program_args) = match command {
        Command::Build { project } => ("build", project.clone(), Vec::new()),
        Command::Run { project, args } => ("run", project.clone(), args.clone()),
        Command::Test {
            project,
            both_settings,
            bless,
        } => return test_command(args, project.clone(), *both_settings, *bless),
        Command::LowerStd { sysroot } => return lower_std(sysroot.clone()),
        Command::Describe {
            crate_name,
            project,
        } => return describe(crate_name, project.clone()),
    };

    let start = match directory {
        Some(directory) => directory,
        None => std::env::current_dir().context("finding the working directory")?,
    };

    let project = Project::open(
        &start,
        args.target.as_deref(),
        args.user_parallelism.as_deref(),
    )?;
    project.drive(
        subcommand,
        &program_args,
        args.no_cache,
        args.locked,
        project::Explain {
            overlaps: args.overlaps,
            sharing: args.sharing,
            tethers: args.tethers,
            trust: args.trust,
            comptime: args.comptime,
            asserts: args.asserts,
        },
        args.allow_read_from_list.as_deref(),
    )
}

/// `nikaia test` ([ADR-245](../../docs/specification/adr/adr-245.md) D1, D7,
/// D8): the test build and the program, each once per setting asked for, then
/// every test run against them.
fn test_command(
    args: &Cli,
    directory: Option<PathBuf>,
    both_settings: bool,
    bless: bool,
) -> Result<i32> {
    let start = match directory {
        Some(directory) => directory,
        None => std::env::current_dir().context("finding the working directory")?,
    };
    let project = Project::open(
        &start,
        args.target.as_deref(),
        args.user_parallelism.as_deref(),
    )?;
    let tests =
        nikaia::modules::Program::read_for_tests(&project.entry(), &project.packages()?, true)?
            .tests;
    let outputs = project::output_tests(&project.root)?;
    if tests.is_empty() && outputs.is_empty() {
        println!(
            "no tests: nothing in this package is a `test \"…\" {{ … }}` block, and \
             `tests/` holds no `NAME.stdout` and no `NAME.out/`"
        );
        return Ok(0);
    }
    let settings: Vec<String> = match both_settings {
        true => vec!["no".to_string(), "yes".to_string()],
        false => vec![project.settings.user_parallelism.clone()],
    };
    // **Each binary is kept where the next build will not overwrite it**: the
    // test build and the program land at the same path, and so do the two
    // settings.
    let kept = project.root.join("target").join("nikaia").join("tests");
    std::fs::create_dir_all(&kept).context("making a place for the test builds")?;
    let mut test_builds = Vec::new();
    let mut programs = Vec::new();
    for setting in &settings {
        for (wanted, testing) in [(!tests.is_empty(), true), (!outputs.is_empty(), false)] {
            if !wanted {
                continue;
            }
            let mut built = Project::open(&start, args.target.as_deref(), Some(setting))?;
            built.settings.tests = testing;
            let (code, binary) = built.drive_to(
                "build",
                &[],
                args.no_cache,
                args.locked,
                project::Explain::default(),
                args.allow_read_from_list.as_deref(),
            )?;
            if code != 0 {
                return Ok(code);
            }
            let Some(binary) = binary else {
                anyhow::bail!("the build finished and named no program to run");
            };
            let name = format!("{}-{setting}", if testing { "tests" } else { "program" });
            let copy = kept.join(name);
            std::fs::copy(&binary, &copy)
                .with_context(|| format!("keeping {}", binary.display()))?;
            match testing {
                true => test_builds.push((setting.clone(), copy)),
                false => programs.push((setting.clone(), copy)),
            }
        }
    }
    project::run_tests(&project::Suite {
        tests: &tests,
        test_builds: &test_builds,
        outputs: &outputs,
        programs: &programs,
        root: &project.root,
        bless,
    })
}

/// What the manifest still accepts and the compiler no longer reads.
///
/// A note rather than a failure: [ADR-038](../../docs/specification/adr/adr-038.md)
/// D5 moved `cleanup-deadline` to the runtime configuration file, and a
/// manifest written to the specification that documented it must not stop
/// compiling because of the move. The note is what makes the move discoverable
/// instead of silent.
fn report_moved_keys(manifest: &Manifest) {
    for note in manifest.notes() {
        eprintln!("note: {note}");
    }
}

/// The single-file path, unchanged: `nikaia --input x.nika --backend …`.
fn single_file(args: &Cli, input: &std::path::Path) -> Result<()> {
    // Resolved before anything is read, so a mistyped switch - in the manifest
    // or on the command line - fails on its own account rather than after a
    // compile. A target the toolchain cannot build for is refused here too:
    // emitting code for a different machine than the one named would be worse
    // than any name this switch replaced (ADR-037 D1).
    let manifest = Manifest::find(input)?;
    report_moved_keys(&manifest);
    let settings = Settings::resolve(
        &manifest,
        args.target.as_deref(),
        args.user_parallelism.as_deref(),
    )?;
    if let Some(missing) = settings.build.target.unbuildable() {
        refuse!(
            "cannot build for `{}` yet: {missing}",
            settings.build.target.triple()
        );
    }

    let source = std::fs::read_to_string(input)?;

    if args.explain {
        return explain(input, args, &settings, &source);
    }

    match args.backend.as_str() {
        "interpreter" => {
            let parsed = parser::parse_to_ast(&source)?;
            let interpreter = interpreter::Interpreter::new(parsed.interner.clone());
            interpreter.run(&parsed);
            Ok(())
        }
        "rust" => lower_to_rust(input, args, &settings),
        // ADR-002 named these and nothing ever matched them, so they ran as
        // whatever the default was without saying so. Accepting a flag and
        // quietly doing something else is worse than either implementing or
        // refusing it (ADR-021 D9).
        backend @ ("cranelift" | "llvm") => refuse!(
            "backend `{backend}` is not implemented: no build of this compiler has it, \
             and there is nothing to install that would add it (ADR-002 names it; \
             ADR-021 D9 records it as an open item).\n\
             Available here: {AVAILABLE}."
        ),
        other => refuse!(
            "unknown backend `{other}` (expected interpreter or rust; \
             available here: {AVAILABLE})"
        ),
    }
}

/// **A refusal leaves quietly; a failure of this compiler keeps its backtrace.**
///
/// `main` used to be `-> Result<()>`, so Rust's own reporting printed
/// `Error: {:?}` for everything - and anyhow's `Debug` carries the backtrace where
/// `RUST_BACKTRACE` is set. For a program that does not compile that is this
/// compiler's internals on a user's screen after the diagnostics had already said
/// what was wrong (`diagnostics::Refused`). For a compiler that cannot read a file
/// the frames are the most useful thing there is, so those are untouched.
pub fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) if diagnostics::is_a_refusal(&error) => {
            // `{:#}` is the chain without the backtrace, so a refusal that was
            // wrapped in the path it came from still names the file.
            eprintln!("{error:#}");
            std::process::ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("Error: {error:?}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    // Cargo owns the wrapper's argument list, so this cannot be a flag or a
    // subcommand: the marker in the environment is the only channel available,
    // and it is read before `clap` sees an argument list it would not
    // recognise (ADR-002 D1).
    if std::env::var_os(project::WRAPPER_MARKER).is_some() {
        let code = project::wrapper_main()?;
        std::process::exit(code);
    }

    let args = Cli::parse();

    if let Some(command) = &args.command {
        let code = project_command(&args, command)?;
        std::process::exit(code);
    }

    let Some(input) = args.input.clone() else {
        refuse!(
            "nothing to build: give `--input <file.nika>` for a single file, or \
             `nikaia build` inside a project (Part III 13.2)"
        );
    };
    single_file(&args, &input)
}
