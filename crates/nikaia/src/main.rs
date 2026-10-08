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

/// The Nikaia compiler: `nikaia build` for a project, `nikaia lower` for one
/// file.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// What to do: a project command (ADR-002 D1), or one of the verbs that
    /// take a single file (ADR-260 D3).
    ///
    /// **There is no `--input`** (D5). It named an argument and not what
    /// happened to it, and three jobs - lowering, interpreting, explaining -
    /// shared its switches; each is a verb now, and the help of each says in
    /// its first sentence what it does and what it does not.
    #[command(subcommand)]
    pub command: Option<Command>,

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

    /// An optimization that changes nothing a program means, as `NAME:on` or
    /// `NAME:off` (ADR-306 D14). There are two: `remove-bounds-checks:on`
    /// writes an index the compiler proved inside without its check, and
    /// `remove-overflow-checks:on` a `+`, `-` or `*` it proved stays inside its
    /// type. Both are `off` by default. What is proved does not depend on them,
    /// and a check nothing proves stays either way.
    ///
    /// May be given more than once; overrides `nikaia.toml`'s
    /// `[build] optimization`.
    #[arg(long, global = true, value_name = "NAME:LEVEL")]
    pub optimization: Vec<String>,

    /// What a claim the compiler shows false with values is
    /// ([ADR-269](../../../docs/specification/adr/adr-269.md) D8): `error`,
    /// the default, refuses the program; `warn` builds it and checks the
    /// claim where it is reached.
    ///
    /// The bypass for the day a better prover refutes what an older one let
    /// through. Overrides `nikaia.toml`'s `[build] refuted-claims`.
    #[arg(long, global = true, value_name = "error|warn")]
    pub refuted_claims: Option<String>,

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
    /// (ADR-292 D2).
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
    /// ([ADR-283](../../../docs/specification/adr/adr-283.md) D4).
    ///
    /// **The half of D6 that survived, and the reason the state lands in the
    /// ledger** (D7). The assertion beside it — a word a struct wrote to forbid
    /// a transition — is gone
    /// ([ADR-283](../../../docs/specification/adr/adr-283.md) D4); this shows
    /// what the compiler solved without being asked, so a change in a
    /// representation is a ledger diff in review rather than a surprise in a
    /// profile.
    ///
    /// Like `--trust`, `--overlaps` and `--sharing`, it explains a decision
    /// rather than changing one — and today it explains one that changes no
    /// lowering, because only one of the three states is built.
    #[arg(long, global = true)]
    pub tethers: bool,

    /// Print every `assert`, and for each what became of it: proved while the
    /// program was built, a precondition its callers prove, checked by a test
    /// when it runs, or refused
    /// ([ADR-269](../../../docs/specification/adr/adr-269.md) D7).
    ///
    /// The report is here so that a reader can see what the prover did with
    /// each claim, and ask why one was not proved.
    #[arg(long, global = true)]
    pub asserts: bool,

    /// Print every index into a list and every `+`, `-` and `*` the
    /// optimizations may drop the check of, and for each whether it was proved
    /// or why it is still checked (#389). At the build's
    /// `--optimization` levels, or at `aggressive` where it asks for none.
    ///
    /// Like `--asserts`, it explains a decision rather than changing one.
    #[arg(long, global = true)]
    pub bounds: bool,

    /// Print what a `T::fields` loop was unrolled to, for the types actually
    /// used ([ADR-304](../../../docs/specification/adr/adr-304.md) D9,
    /// [ADR-304](../../../docs/specification/adr/adr-304.md)).
    ///
    /// **The one readability problem every build-time system shares** is that
    /// you cannot see what a function becomes for a given type without
    /// unrolling it in your head. The usual answer is to invent syntax; this
    /// project already has the other one, and `--overlaps`, `--sharing`,
    /// `--tethers` and `--trust` are it. So this is the same information
    /// [ADR-304](../../../docs/specification/adr/adr-304.md) D8's diagnostic
    /// carries, offered **on demand instead of on failure** (D5).
    ///
    /// Like the other four, it explains a decision rather than changing one.
    #[arg(long, global = true)]
    pub comptime: bool,

    /// The file that names the files this build may read while it builds
    /// ([ADR-310](../../../docs/specification/adr/adr-310.md) D5).
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

/// The commands of Part III 13.2: the project's, and the verbs that take one
/// file (ADR-260 D3).
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Lower one `.nika` file to Rust: writes `<file>.rs` and the ledger
    /// beside it, and does not call `rustc`.
    ///
    /// A file inside a project is lowered as a file of that project - its
    /// `[build]` settings and its declared dependencies (ADR-260 D1); a file
    /// outside any project is standalone and has none (D2). To compile and run
    /// a program, use `nikaia build` or `nikaia run`.
    Lower {
        /// The `.nika` file.
        #[arg(value_name = "FILE")]
        file: PathBuf,
        /// Where the Rust is written. Defaults to the file with `.rs` for its
        /// extension.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Run one `.nika` file in the interpreter; nothing is written and
    /// nothing is compiled.
    ///
    /// The interpreter handles a part of the language (ADR-002 D2), so a
    /// program it runs may still mean more when built.
    Interpret {
        /// The `.nika` file.
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },
    /// Read `rustc --error-format=json` on stdin and report it against the
    /// `.nika` file instead of the emitted Rust (ADR-300); nothing is written.
    ///
    /// The map is not an artifact: the lowering is deterministic, so it is
    /// rebuilt from the file here rather than written out and kept in step.
    /// The Rust it names is the file with `.rs` for its extension, where
    /// `nikaia lower` writes it.
    Explain {
        /// The `.nika` file the Rust was lowered from.
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },
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
    /// Build the project as a library and write a binding over it for another
    /// language ([ADR-284](../../../docs/specification/adr/adr-284.md) D26).
    ///
    /// `python` writes `target/nikaia/c-library/<package>/__init__.py`, a
    /// `ctypes` module over the library beside it. The library stays the one
    /// artifact: a binding is generated source the host loads.
    Bind {
        /// The language: `python`.
        #[arg(value_name = "LANGUAGE")]
        language: String,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Compile the project and run it, or compile and run one `.nika` file.
    ///
    /// Everything after `--` is the program's own arguments. A file outside any
    /// project is built as a project of its own, kept in the user's cache
    /// directory (ADR-260 D4); a file inside one has to be that project's
    /// entry point.
    Run {
        #[arg(long)]
        project: Option<PathBuf>,
        /// One `.nika` file to compile and run instead of the project here.
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Run the project's `test` blocks, each in a process of its own
    /// ([ADR-269](../../../docs/specification/adr/adr-269.md) D1, D12).
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
    /// ([ADR-290](../../../docs/specification/adr/adr-290.md) D2).
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

/// `rustc --error-format=json … | nikaia explain x.nika`
///
/// Every message rustc reports about the emitted file is placed back in the
/// `.nika` it came from. The text is left alone: because the lowering is name
/// for name (ADR-296 D17), only the position was ever wrong.
fn explain(input: &std::path::Path, settings: &Settings, source: &str) -> Result<()> {
    use std::io::Read;

    // The same switches the build used, or the map would point into a file this
    // run did not emit - which is why they are resolved once and passed in.
    let parsed = parser::parse_to_ast(source)?;
    let lowered = emit::emit_program(&parsed, settings.build)?;

    let mut rustc_json = String::new();
    std::io::stdin().read_to_string(&mut rustc_json)?;

    let generated = input.with_extension("rs");
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
fn lower_to_rust(
    input: &std::path::Path,
    output: Option<&std::path::Path>,
    args: &Cli,
    settings: &Settings,
) -> Result<()> {
    let output_path = output
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| input.with_extension("rs"));

    // `--trust`, `--overlaps` and `--sharing` are explanations, so they are
    // answered before the cache is consulted: a build that reuses a cached
    // lowering still answers the question, and the answer cannot differ from the
    // one that lowering was built with.
    //
    // Through `project::explain`, which is the same function `nikaia build` uses
    // (issue #210, the explain modes): a report that said one thing here and another
    // there would be worse than one that only existed in one place.
    // **The file as its project reads it** (ADR-260 D1): its package and the
    // dependencies the manifest declares, or - outside a project - the file
    // alone, with none (D2).
    let (program, packages) = project::read_file(input)?;
    project::explain(
        &program,
        settings,
        project::Explain {
            overlaps: args.overlaps,
            sharing: args.sharing,
            tethers: args.tethers,
            trust: args.trust,
            comptime: args.comptime,
            asserts: args.asserts,
            bounds: args.bounds,
        },
    )?;

    // The cache key gains the dependencies with them: their units are part of
    // the program, and the key is made of the program's sources and paths.
    let lowered = project::lower_reading(
        input,
        settings,
        args.no_cache,
        &packages,
        args.allow_read_from_list.as_deref(),
    )?;
    std::fs::write(&output_path, &lowered.rust)?;
    eprint!("{}", lowered.notes);

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

/// `nikaia describe <crate>` ([ADR-290](../../../docs/specification/adr/adr-290.md) D2).
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
    // The same walk `nikaia lower` makes, from a directory rather than from a file:
    // a crate is described for a **project**, because the project is what
    // declares it (ADR-290 D1).
    let start = start.canonicalize().unwrap_or(start);
    let root = start
        .ancestors()
        .find(|dir| dir.join("nikaia.toml").is_file())
        .map(std::path::Path::to_path_buf)
        .with_context(|| {
            format!(
                "There's no `nikaia.toml` in {} or any folder above it. Run `nikaia \
                 describe` inside the project that depends on the crate.",
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
            "\n{} name{} the program uses that no `pub` signature covers. Each is a `?` \n\
             for you to fill in, or a name a macro wrote:",
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
    // ([ADR-290](../../docs/specification/adr/adr-290.md) D8, D10). They are in
    // the file as comments, where the reviewer meets them; they are said here
    // too, because a person who runs a command reads what it printed and may
    // not open the file at all.
    if !written.notes.about_the_crate.is_empty() {
        println!(
            "\nThis crate makes a promise the compiler can't check, and the file says \n\
             where. Check yourself whether the promise is true."
        );
    }
    let proposed = written.notes.about_a_function.len();
    if proposed > 0 {
        println!(
            "\n{proposed} entr{} {} something the signature doesn't say (a `Send` bound, \n\
             or a call that reaches another thread), and the file asks beside {}: does \n\
             this put what it's given on a thread? Answer it in the file.",
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
        "\nReview it before you commit it: what a signature can't say is written as \n\
         the safe answer, and a mistake in it is only caught by you."
    );
    Ok(0)
}

/// `nikaia build` and `nikaia run` (Part III 13.2).
fn project_command(args: &Cli, command: &Command) -> Result<i32> {
    let (subcommand, directory, program_args) = match command {
        Command::Build { project } => ("build", project.clone(), Vec::new()),
        Command::Bind { language, project } => {
            if language != "python" {
                refuse!(
                    "`nikaia bind {language}` is not built: `python` is (ADR-284 D26). JavaScript \
                     is served by the WebAssembly build, which is not built yet either."
                );
            }
            let start = match project {
                Some(directory) => directory.clone(),
                None => std::env::current_dir().context("finding the working directory")?,
            };
            let mut project = Project::open(
                &start,
                args.target.as_deref(),
                args.user_parallelism.as_deref(),
            )?;
            if !project.settings.library() {
                refuse!(
                    "`nikaia bind` writes a binding over a library: set `artifact = \"c-library\"` \
                     in `[build]` (ADR-284 D4, D26)."
                );
            }
            project.settings.bind = Some(language.clone());
            project.settings.optimize(&args.optimization)?;
            return drive(args, &project, "build", &[]);
        }
        Command::Run {
            project: None,
            file: Some(file),
            args: program_args,
        } => {
            let mut project = project::project_for_file(
                file,
                args.target.as_deref(),
                args.user_parallelism.as_deref(),
                args.no_cache,
                args.allow_read_from_list.as_deref(),
            )?;
            project.settings.optimize(&args.optimization)?;
            project
                .settings
                .refutations(args.refuted_claims.as_deref())?;
            return drive(args, &project, "run", program_args);
        }
        Command::Run {
            project: Some(_),
            file: Some(_),
            ..
        } => refuse!(
            "`nikaia run` takes a file or `--project`, not both: the file decides its \
             project (ADR-260 D1)."
        ),
        Command::Run { project, args, .. } => ("run", project.clone(), args.clone()),
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
        Command::Lower { .. } | Command::Interpret { .. } | Command::Explain { .. } => {
            unreachable!("the single-file verbs are dispatched by `run`")
        }
    };

    let start = match directory {
        Some(directory) => directory,
        None => std::env::current_dir().context("finding the working directory")?,
    };

    let mut project = Project::open(
        &start,
        args.target.as_deref(),
        args.user_parallelism.as_deref(),
    )?;
    project.settings.optimize(&args.optimization)?;
    project
        .settings
        .refutations(args.refuted_claims.as_deref())?;
    drive(args, &project, subcommand, &program_args)
}

/// A project's `build` or `run`, with the switches this command line set.
fn drive(args: &Cli, project: &Project, subcommand: &str, program_args: &[String]) -> Result<i32> {
    project.drive(
        subcommand,
        program_args,
        args.no_cache,
        args.locked,
        project::Explain {
            overlaps: args.overlaps,
            sharing: args.sharing,
            tethers: args.tethers,
            trust: args.trust,
            comptime: args.comptime,
            asserts: args.asserts,
            bounds: args.bounds,
        },
        args.allow_read_from_list.as_deref(),
    )
}

/// `nikaia test` ([ADR-269](../../docs/specification/adr/adr-269.md) D1, D12,
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
    let mut project = Project::open(
        &start,
        args.target.as_deref(),
        args.user_parallelism.as_deref(),
    )?;
    project.settings.optimize(&args.optimization)?;
    project
        .settings
        .refutations(args.refuted_claims.as_deref())?;
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
            built.variant = Some(format!(
                "{setting}-{}",
                if testing { "tests" } else { "program" }
            ));
            built.settings.optimize(&args.optimization)?;
            built.settings.refutations(args.refuted_claims.as_deref())?;
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
/// A note rather than a failure: [ADR-303](../../docs/specification/adr/adr-303.md)
/// D5 moved `cleanup-deadline` to the runtime configuration file, and a
/// manifest written to the specification that documented it must not stop
/// compiling because of the move. The note is what makes the move discoverable
/// instead of silent.
fn report_moved_keys(manifest: &Manifest) {
    for note in manifest.notes() {
        eprintln!("note: {note}");
    }
}

/// **What a verb that takes one file does with it** (ADR-260 D3).
enum Verb<'a> {
    Lower { output: Option<&'a std::path::Path> },
    Interpret,
    Explain,
}

/// The single-file verbs: `nikaia lower`, `interpret` and `explain`.
fn single_file(args: &Cli, input: &std::path::Path, verb: Verb<'_>) -> Result<()> {
    // Resolved before anything is read, so a mistyped switch - in the manifest
    // or on the command line - fails on its own account rather than after a
    // compile. A target the toolchain cannot build for is refused here too:
    // emitting code for a different machine than the one named would be worse
    // than any name this switch replaced (ADR-037 D1).
    let manifest = Manifest::find(input)?;
    report_moved_keys(&manifest);
    let mut settings = Settings::resolve(
        &manifest,
        args.target.as_deref(),
        args.user_parallelism.as_deref(),
    )?;
    settings.optimize(&args.optimization)?;
    settings.refutations(args.refuted_claims.as_deref())?;
    if let Some(missing) = settings.build.target.unbuildable() {
        refuse!(
            "cannot build for `{}` yet: {missing}",
            settings.build.target.triple()
        );
    }

    let source = std::fs::read_to_string(input)?;

    match verb {
        Verb::Explain => explain(input, &settings, &source),
        Verb::Interpret => {
            let parsed = parser::parse_to_ast(&source)?;
            let interpreter = interpreter::Interpreter::new(parsed.interner.clone());
            interpreter.run(&parsed);
            Ok(())
        }
        Verb::Lower { output } => lower_to_rust(input, output, args, &settings),
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

    let Some(command) = &args.command else {
        refuse!(
            "Nothing to do. Run `nikaia build` inside a project, or \
             `nikaia lower <file.nika>` for a single file. `nikaia --help` lists \
             every command."
        );
    };
    match command {
        Command::Lower { file, output } => single_file(
            &args,
            file,
            Verb::Lower {
                output: output.as_deref(),
            },
        ),
        Command::Interpret { file } => single_file(&args, file, Verb::Interpret),
        Command::Explain { file } => single_file(&args, file, Verb::Explain),
        command => {
            let code = project_command(&args, command)?;
            std::process::exit(code);
        }
    }
}
