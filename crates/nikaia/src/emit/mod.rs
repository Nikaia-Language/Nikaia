// crates/nikaia/src/emit/mod.rs
//
// Stage 0 of the bootstrap compiler: Nikaia in, Rust out.
//
// The three things this file exists for are the three halves of the grammar
// protocol that the parser backend already implements and Nikaia could not yet
// reach (README, "Bootstrap Compiler"):
//
//   * `grammar Name { rule ... }`  ->  `grammar! { grammar Name { ... } }`
//   * `@frame(boundary: "\n")`     ->  `#[frame(boundary = "\n")]`
//   * `Name.rule(input)`            ->  the generated `par_fold` driver, with
//                                      the `Parallelism` the build asks for
//
// What it is *not* is a type checker. The lowering is syntactic: every action
// block, every `init`/`step`/`merge` and every function body is emitted as
// written. Where Nikaia and Rust disagree about what a name means - a `&mut
// self` method used as a monoid's merge, `std::fs` - the emitted program says
// so by not compiling, rather than this file guessing. That is deliberate:
// guessing a receiver would be a semantic decision made in a printer.
//
// It emits a [`SourceMap`] alongside the code, because a transpiler that only
// emits code can only be told about errors in a file nobody wrote (ADR-300).

use crate::contracts::LedgerOps;
use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};

pub mod template;
use winnow_grammar::Symbol;

use crate::assets::Reads;
use crate::ast::{
    BinaryOp, Block, Expr, FnArg, FoldSpec, FrameAttr, GrammarDef, GrammarRule, Item, MatchPattern,
    Pattern, Receiver, Repeat, SelectArm, Span, Spanned, Stmt, Type, UnaryOp, VariantFields,
};
use crate::check::PausingEntry;
use crate::contracts::SignatureOps;
use crate::parser::Parsed;
use crate::{refused, refused_at};

/// One branch of an `overlap { … }`, where the schedule and the written order
/// differ and the results have to be put back (ADR-292 D2, D6).
const BRANCH: &str = "__nikaia_branch_";

/// The name a `select` arm's winning value arrives under, where it has to be
/// unwrapped before the arm's own binding sees it (ADR-292 D12).
const WINNER: &str = "__nikaia_won";

/// What the variants of `std`'s `Race<n>` are called, in written order.
///
/// **Ordinals and not letters**, so the generated `match` reads as the source
/// does: `Race2::Second(_)` is the second arm of the block (Part III C.1).
const ORDINALS: [&str; MOST_BRANCHES] = [
    "First", "Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh", "Eighth",
];

/// How many branches an `overlap` block may have.
///
/// `std` writes one vehicle per arity, because the branches have different types
/// and a tuple of futures is what that means in the language below. Eight is
/// what is written; a block with more is refused **with its reason** rather than
/// miscompiled, which is the only thing a limit owes a reader.
const MOST_BRANCHES: usize = 8;

/// The machine a program is built for (ADR-037 D1).
///
/// It decides what `std` can offer and what a panic does. It decides nothing
/// about what a program means: the same source compiles for every target and
/// prints the same bytes.
///
/// **The default is the machine the compiler runs on** ([`Target::host`]), and
/// the triple is handed to Cargo as `--target` whichever one is named. Until
/// 0.0.218 it was only ever printed: every build was for the host, so on an ARM
/// machine `x86_64-linux` - the default - produced ARM code under an x86 name,
/// which is the one thing D1 says a target never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    X86_64Linux,
    Aarch64Linux,
    Wasm32Unknown,
}

impl Default for Target {
    fn default() -> Target {
        Target::host().unwrap_or(Target::X86_64Linux)
    }
}

/// Whether the **user's** code may run concurrently at all (ADR-037 D2).
///
/// A yes-or-no question, deliberately: *how many* threads or cores serve that
/// answer is the runtime's business, and a number in the language would be a
/// promise the language cannot keep on a machine it has not seen.
///
/// It does not bind the compiler - `fs::map` may still validate its text on
/// four cores at `No`, because that is not code the user wrote and it changes
/// nothing the program prints (ADR-016 D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserParallelism {
    /// `no` - the default. Nothing the user wrote ever runs concurrently.
    #[default]
    No,
    /// `yes` - it may, and the runtime decides how widely.
    Yes,
}

/// **Whether the program carries the runtime re-entrancy check**
/// ([ADR-281](../../docs/specification/adr/adr-281.md) D36).
///
/// Taking a lock while a lock is held is refused when the program is compiled
/// (`NK2203`, Part II 12.3), so under D2 this check cannot fire in a correct
/// compiler — and that is its role: **self-control of D2's rule, not error
/// handling.** No input can trigger it; if it fires, the compiler has a hole,
/// and without it such a hole is a silent hang instead.
///
/// **For every program that obeys the nesting rule, both builds behave
/// identically**, which is what keeps Part I 1.2's *how, never what* intact:
/// the switch decides only whether a violation is **noticed**.
///
/// It is not named *debug* and is not a development aid to be removed later. It
/// is a guarantee that can be declined, which is
/// [ADR-292](../../docs/specification/adr/adr-292.md) D7's precedent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReentrancyCheck {
    /// `yes` — the default. The program carries the check.
    #[default]
    Yes,
    /// `no` — it does not, and a hole in this compiler is a hang.
    No,
}

impl ReentrancyCheck {
    pub fn parse(value: &str) -> Result<ReentrancyCheck> {
        match value {
            "yes" => Ok(ReentrancyCheck::Yes),
            "no" => Ok(ReentrancyCheck::No),
            // **A third spelling is refused rather than guessed at**, which is
            // what `user-parallelism` does one switch over: `on` and `off` read
            // like this option and are not it, and a build that silently took
            // the default for a word it did not know would ship the guarantee
            // the manifest declined.
            other => Err(refused!(
                "`reentrancy-check` is `yes` or `no`, not `{other}`."
            )),
        }
    }

    /// Whether the emitted program carries it.
    pub fn is_on(self) -> bool {
        matches!(self, ReentrancyCheck::Yes)
    }
}

/// The build switches together (ADR-037, [ADR-281](../../docs/specification/adr/adr-281.md) D36).
///
/// One value rather than three parameters: a fourth switch is then a field, not
/// a change at every call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Build {
    pub target: Target,
    pub user_parallelism: UserParallelism,
    pub reentrancy_check: ReentrancyCheck,
    /// `--optimization=remove-bounds-checks:on`
    /// ([ADR-306](../../docs/specification/adr/adr-306.md) D1): how hard the
    /// compiler works to prove an index inside. Nothing a program means
    /// depends on it.
    pub bounds: crate::bounds::BoundsChecks,
    /// `--optimization=remove-overflow-checks:on`
    /// ([ADR-306](../../docs/specification/adr/adr-306.md) D6).
    pub overflow: crate::bounds::OverflowChecks,
}

impl Build {
    pub fn parse(target: &str, user_parallelism: &str, reentrancy_check: &str) -> Result<Build> {
        Ok(Build {
            target: Target::parse(target)?,
            user_parallelism: UserParallelism::parse(user_parallelism)?,
            reentrancy_check: ReentrancyCheck::parse(reentrancy_check)?,
            bounds: crate::bounds::BoundsChecks::default(),
            overflow: crate::bounds::OverflowChecks::default(),
        })
    }

    /// The default machine, with parallelism asked for.
    pub fn parallel() -> Build {
        Build {
            user_parallelism: UserParallelism::Yes,
            ..Build::default()
        }
    }

    /// How the generated driver is asked to cut the input.
    fn parallelism(self) -> &'static str {
        self.user_parallelism.parallelism(self.target)
    }

    /// **May two pieces of the program's own code run at the same time?**
    /// (ADR-037 D2.)
    ///
    /// This gates `task::both` and nothing else. `task::both(|| …, || …)` puts
    /// two user closures on two threads, so it is out at
    /// `user_parallelism = no` whatever the analysis says - ADR-292 decides
    /// whether two operations *may* overlap, and this decides whether there is
    /// anything to overlap them with. A target without threads answers no for
    /// the second reason: `rayon::join` does not link on `wasm32-unknown`
    /// (Part III 15.3).
    ///
    /// **Not the same question as [`Build::overlaps_operations`]**, and the
    /// whole of ADR-292 D6 is that difference. "Two pieces of your code at
    /// once" is forbidden at `no`; "two operations in flight at once" is not,
    /// and never was - Part I 8.4's runtime has had a second thread at `no`
    /// since ADR-303 D4, and it is safe precisely because *user code never
    /// runs there*. What `no` forbids is that anything **you** wrote is in
    /// flight twice at once (Part I 1.2), which is why the word in
    /// `user_parallelism` is `user`.
    pub fn overlaps_user_code(self) -> bool {
        self.user_parallelism.is_concurrent() && self.target.has_threads()
    }

    /// **May two operations be in flight at the same time?** (ADR-292 D6.)
    ///
    /// True at **both** settings of `user_parallelism`, because the vehicle it
    /// gates carries no code the program wrote: two reads handed to the
    /// runtime are two operations the kernel performs while the program is
    /// suspended at both of them, and `std` is what waits. Nothing of the
    /// user's is in flight twice, so ADR-037 D2's promise is untouched - and
    /// `nikaia_std::rt` keeps it by construction rather than by convention,
    /// since an I/O worker's inbox takes operations and no variant of one can
    /// carry a closure (ADR-303).
    ///
    /// What it asks of the build is therefore only whether `std`'s runtime is
    /// there at all. `user_parallelism` has no say in it; the *target* does.
    pub fn overlaps_operations(self) -> bool {
        self.target.has_runtime()
    }
}

impl Target {
    /// Every target, in the order a refusal lists them.
    pub const ALL: [Target; 3] = [
        Target::X86_64Linux,
        Target::Aarch64Linux,
        Target::Wasm32Unknown,
    ];

    pub fn parse(name: &str) -> Result<Target> {
        match Target::ALL.into_iter().find(|target| target.name() == name) {
            Some(target) => Ok(target),
            None => Err(refused!(
                "There's no target called `{name}`. Use x86_64-linux, aarch64-linux or \
                 wasm32-unknown."
            )),
        }
    }

    /// The word a manifest and `--target` use for it.
    pub fn name(self) -> &'static str {
        match self {
            Target::X86_64Linux => "x86_64-linux",
            Target::Aarch64Linux => "aarch64-linux",
            Target::Wasm32Unknown => "wasm32-unknown",
        }
    }

    /// The triple handed to the backend: Cargo's `--target`.
    pub fn triple(self) -> &'static str {
        match self {
            Target::X86_64Linux => "x86_64-unknown-linux-gnu",
            Target::Aarch64Linux => "aarch64-unknown-linux-gnu",
            Target::Wasm32Unknown => "wasm32-unknown-unknown",
        }
    }

    /// The machine this compiler is running on, where it is one of the
    /// targets. The compiler is a native program, so the machine it was built
    /// for is the machine it runs on.
    ///
    /// `None` on any other (macOS, a 32-bit ARM board, RISC-V): there is no
    /// name for it, and a build there has to name one of the targets and have
    /// the toolchain for it.
    pub fn host() -> Option<Target> {
        if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
            Some(Target::X86_64Linux)
        } else if cfg!(all(target_arch = "aarch64", target_os = "linux")) {
            Some(Target::Aarch64Linux)
        } else {
            None
        }
    }

    /// Whether this machine has threads at all. A target without them bounds
    /// `user_parallelism` to `0` however it is set.
    pub fn has_threads(self) -> bool {
        matches!(self, Target::X86_64Linux | Target::Aarch64Linux)
    }

    /// Whether `std`'s own runtime exists for this machine (ADR-303 D4): the
    /// I/O worker that is running before `main`, and the completion queue where
    /// the kernel has one.
    ///
    /// The same answer as [`Target::has_threads`] today and a different
    /// question, which is why it is a second method rather than a second
    /// caller. `wasm32-unknown` has neither `io_uring` nor a thread to fall
    /// back to; a machine with threads but no completion queue has the runtime,
    /// and `std` is what decides there what a pair costs.
    pub fn has_runtime(self) -> bool {
        matches!(self, Target::X86_64Linux | Target::Aarch64Linux)
    }

    /// What a target still needs before a program can be built for it.
    ///
    /// `Some(reason)` is a refusal that names the gap, never code emitted for
    /// a different machine (ADR-037 D1).
    pub fn unbuildable(self) -> Option<&'static str> {
        match self {
            Target::Wasm32Unknown => Some(
                "`std` reaches for a memory mapping and a thread pool, and neither \
                 exists on wasm32-unknown-unknown; what `std::fs` offers there is \
                 undecided",
            ),
            Target::X86_64Linux | Target::Aarch64Linux => None,
        }
    }
}

impl UserParallelism {
    pub fn parse(value: &str) -> Result<UserParallelism> {
        match value {
            "no" => Ok(UserParallelism::No),
            "yes" => Ok(UserParallelism::Yes),
            // A number is the plausible mistake, and it has a reason rather
            // than a typo behind it. Say which.
            other if other.parse::<u32>().is_ok() => Err(refused!(
                "`user-parallelism` is `yes` or `no`, not a number: the runtime decides \
                 how many threads to use."
            )),
            other => Err(refused!(
                "`user-parallelism` is `yes` or `no`, not `{other}`."
            )),
        }
    }

    /// Whether any code the user wrote may run concurrently.
    pub fn is_concurrent(self) -> bool {
        matches!(self, UserParallelism::Yes)
    }

    /// How the generated driver is asked to cut the input.
    ///
    /// A target without threads pins this to `Off` whatever was asked for: the
    /// switch bounds what may run at once, it cannot conjure a thread the
    /// machine does not have.
    fn parallelism(self, target: Target) -> &'static str {
        if !target.has_threads() {
            return "Parallelism::Off";
        }
        match self {
            UserParallelism::No => "Parallelism::Off",
            UserParallelism::Yes => "Parallelism::Auto",
        }
    }
}

/// The lifetime the parser backend gives its input. A Nikaia view (`&str`) is
/// tied to it, which is the whole of what `&` means here: ADR-283 - a view
/// marker, never an annotation the user writes.
const INPUT_LIFETIME: &str = "'a";

/// How a view's lifetime is spelled here. The source never says either way
/// (ADR-283); the position does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Lifetimes {
    /// What a `&` becomes.
    reference: &'static str,
    /// What a type that holds a view is parameterised with.
    params: &'static str,
}

impl Lifetimes {
    /// Inside the grammar module and on the structs it builds, where `'a` is
    /// the input's lifetime and has to be named to tie the two together.
    const NAMED: Lifetimes = Lifetimes {
        reference: "&'a ",
        params: "'a",
    };
    /// In a free function's signature, which declares no lifetime of its own.
    const ELIDED: Lifetimes = Lifetimes {
        reference: "&",
        params: "'_",
    };
    /// In a method of an `impl` that does declare `'a`: the reference itself is
    /// still elided, because a borrow of the subject need not live as long as
    /// what the subject points into.
    const INNER: Lifetimes = Lifetimes {
        reference: "&",
        params: "'a",
    };
    /// **In the result of a function with nothing to borrow from.**
    ///
    /// `fn name() -> &str { "Ada" }` has no reference among its arguments and no
    /// receiver, so there is no lifetime for Rust's elision to take - and
    /// `-> &str` is *"missing lifetime specifier"* about a file nobody wrote
    /// (Part III, C.1).
    ///
    /// **`'static` is the only lifetime that can be written there**, which is
    /// what makes this a derivation rather than a choice: a view handed back by
    /// a function that borrowed nothing can only point at something that
    /// outlives the program. And it is safe to write even where the body cannot
    /// honour it - `rustc` still checks the body, and refuses the Nikaia line
    /// through [ADR-300](../../../docs/specification/adr/adr-300.md) rather than
    /// the generated one. So this never accepts a wrong program and never
    /// refuses a right one.
    const STATIC: Lifetimes = Lifetimes {
        reference: "&'static ",
        params: "'static",
    };
    /// **The keep a tethered function is given**
    /// ([ADR-283](../../../docs/specification/adr/adr-283.md) D10): its views
    /// point into the caller's keep, and live as long as it.
    const KEPT: Lifetimes = Lifetimes {
        reference: "&'k ",
        params: "'k",
    };
    /// The same inside an `impl` that already names `'a`: the subject's buffer
    /// and the keep are one lifetime, because the subject is what keeps.
    const KEPT_BY_THE_SUBJECT: Lifetimes = Lifetimes {
        reference: "&'a ",
        params: "'a",
    };
    /// A tethered value read through its handle, for as long as it is borrowed
    /// (ADR-283 D11).
    const SHORTENED: Lifetimes = Lifetimes {
        reference: "&'s ",
        params: "'s",
    };
    /// What a `Views` type's `shorten` is handed: the value over the longer
    /// of its two lifetimes ([ADR-218](../../../docs/specification/adr/adr-218.md) D4).
    const LONG: Lifetimes = Lifetimes {
        reference: "&'long ",
        params: "'long",
    };
    /// What a `Views` type is over any lifetime, `Of<'a>`.
    const ANY: Lifetimes = Lifetimes {
        reference: "&'a ",
        params: "'a",
    };

    /// The same, with the reference named: a view **of the input buffer** and
    /// not of whatever the caller lends for the call.
    ///
    /// One parameter at a time, for the parameters `views::carried` says the
    /// subject's own buffer already covers. Nothing else about the position
    /// changes, which is why this is derived from the position rather than being
    /// a fourth constant.
    const fn of_the_input(self) -> Lifetimes {
        Lifetimes {
            reference: "&'a ",
            params: self.params,
        }
    }
}

// --- The output, and the way back ---

/// What the lowering produces: the Rust, and where each part of it came from.
#[derive(Debug, Clone)]
pub struct Lowered {
    pub rust: String,
    pub map: SourceMap,
    /// What the prover publishes of each function's contract, for the ledger
    /// ([ADR-269](../../../docs/specification/adr/adr-269.md) D18).
    pub published: std::collections::BTreeMap<String, crate::prove::Published>,
}

/// **[ADR-300](../../../docs/specification/adr/adr-300.md) D9's table, as a Rust
/// item to append to a program.**
///
/// One row per line of the generated file that came from a `.nika` line: the
/// generated line, the file, and the line there. Sorted by the generated line,
/// because the lookup at run time is a binary search and sorting once here costs
/// the program nothing.
///
/// **Every mapped line, and not the ones that "can abort".** Which constructs
/// abort is a list that would have to be kept correct as the emitter grows, and
/// getting it wrong means an abort with no Nikaia line - the defect this closes.
/// The source map already knows which lines came from somewhere, so the table is
/// that knowledge kept rather than a second judgement about it. What it costs is
/// a row per line in the binary, which the record accepts: *"paid by every
/// program and needed by the ones that fail, which is the same trade a panic
/// message itself already makes."*
///
/// **Appended, never prepended.** A table written above the program would move
/// every line it names, and correcting for its own height is a circle. Rust does
/// not care where an item sits.
///
/// `paths` are the program's files, indexed the way the map indexes them.
pub fn abort_table(rust: &str, map: &SourceMap, paths: &[String], sources: &[&str]) -> String {
    // Where each line of the generated file starts, so an offset becomes a line
    // in one binary search rather than by counting newlines per entry.
    let mut starts = vec![0usize];
    starts.extend(
        rust.char_indices()
            .filter(|(_, c)| *c == '\n')
            .map(|(i, _)| i + 1),
    );
    let line_of = |offset: usize| match starts.binary_search(&offset) {
        Ok(i) => i + 1,
        Err(i) => i,
    };

    // The `.nika` line of a span, per file.
    let nika: Vec<Vec<usize>> = sources
        .iter()
        .map(|source| {
            let mut starts = vec![0usize];
            starts.extend(
                source
                    .char_indices()
                    .filter(|(_, c)| *c == '\n')
                    .map(|(i, _)| i + 1),
            );
            starts
        })
        .collect();

    // **The outermost entry wins**, which is the opposite of what a lookup
    // wants. `SourceMap::locate` answers with the innermost span covering an
    // offset, and that is right for a diagnostic about a byte; a whole generated
    // *line* is better named by the statement it came from than by the
    // sub-expression that happens to start it. A `BTreeMap` keyed by the
    // generated line and filled in map order gives that: the emitter records a
    // node before anything it encloses, so the first entry for a line is the
    // outermost one.
    let mut rows: std::collections::BTreeMap<usize, (usize, usize)> =
        std::collections::BTreeMap::new();
    for entry in map.rows() {
        let generated = line_of(entry.0);
        let Some(starts) = nika.get(entry.2) else {
            continue;
        };
        let line = match starts.binary_search(&entry.1) {
            Ok(i) => i + 1,
            Err(i) => i,
        };
        rows.entry(generated).or_insert((entry.2, line));
    }

    let mut out = String::new();
    out.push_str("\n// ADR-300 D9: the generated line, and the `.nika` line it came from.\n");
    out.push_str(&format!(
        "const {ABORT_TABLE}: &[nikaia_std::abort::Site] = &[\n"
    ));
    for (generated, (unit, line)) in rows {
        let path = paths.get(unit).map(String::as_str).unwrap_or("<unknown>");
        out.push_str(&format!("    ({generated}, {path:?}, {line}),\n"));
    }
    out.push_str("];\n");
    out
}

/// Emitted byte range -> the `.nika` span that produced it.
///
/// This is the whole answer to "the error points at a line nobody wrote": the
/// compiler that consumes the emitted Rust reports offsets into it, and every
/// one of them can be traded back for a place in the source the user has open.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    /// Innermost first: a lookup returns the first range that contains the
    /// offset, and the emitter records a node before recording anything that
    /// encloses it.
    entries: Vec<MapEntry>,
}

#[derive(Debug, Clone)]
struct MapEntry {
    generated: std::ops::Range<usize>,
    source: Span,
    /// Which file it came from, as an index into the program's units. A
    /// single-file program has one, and it is 0.
    unit: usize,
}

impl SourceMap {
    /// The narrowest source span whose emitted text covers `offset`, and the
    /// file it is in.
    pub fn locate(&self, offset: usize) -> Option<(usize, Span)> {
        self.entries
            .iter()
            .find(|e| e.generated.contains(&offset))
            .map(|e| (e.unit, e.source))
    }

    /// The narrowest source span whose emitted text covers `offset`.
    pub fn source_span(&self, offset: usize) -> Option<Span> {
        self.locate(offset).map(|(_, span)| span)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// This map, as it reads once its module's text has been placed `by` bytes
    /// into a larger file - and tagged with which file it came from.
    ///
    /// A program of several modules is emitted one module at a time and joined;
    /// this is what keeps ADR-300's promise across the join. Without it a
    /// `rustc` message about the third module would be traded back for a place
    /// in the first.
    pub fn placed(mut self, by: usize, unit: usize) -> SourceMap {
        for entry in &mut self.entries {
            entry.generated = entry.generated.start + by..entry.generated.end + by;
            entry.unit = unit;
        }
        self
    }

    /// Take everything `other` holds, which is already placed.
    pub fn extend(&mut self, other: SourceMap) {
        self.entries.extend(other.entries);
    }

    /// Every entry as `(generated start, source start, unit)`, in the order the
    /// emitter recorded them - which is outermost first for any one place.
    ///
    /// For [`abort_table`], which needs all of them rather than a lookup.
    pub fn rows(&self) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
        self.entries
            .iter()
            .map(|e| (e.generated.start, e.source.at(), e.unit))
    }
}

/// The emitted text under construction, and the map being built with it.
#[derive(Debug, Default)]
struct Out {
    buf: String,
    map: SourceMap,
}

/// The name a `?.m()`'s reached value is bound to inside the `match`.
///
/// Prefixed like every other name this emitter invents, so it can never be one
/// a program wrote: `__nikaia_` is reserved by the same rule that reserves the
/// branch names an `overlap` uses.
const REACHED: &str = "__nikaia_it";

/// Kap 7.1: the name a `catch` handler sees the failure under.
///
/// One place, because the emitter writes it and
/// [`contracts::order::block_mentions`](crate::contracts::order::block_mentions)
/// is asked about it — and a second spelling of one name is a binding that goes
/// unread while the walk looks for the other
/// ([ADR-308](../../docs/specification/adr/adr-308.md)).
const CAUGHT: &str = "error";

/// Where the local holding a caught error's **site** is kept
/// ([ADR-280](../../docs/specification/adr/adr-280.md) D10).
///
/// A named channel's envelope is opened at the handler's binding, so the error
/// the source names is the author's own value — and `throw error` needs the
/// site back to pass the error on with the place it was **raised** rather than
/// the place it was caught ([ADR-023](../../docs/specification/adr/adr-023.md)
/// D6). Spelled so that no Nikaia name collides with it.
const SITE: &str = "__nikaia_site";

impl Out {
    fn push(&mut self, text: &str) {
        self.buf.push_str(text);
    }

    /// Record that everything `f` writes came from `span`.
    fn from<R>(&mut self, span: &Span, f: impl FnOnce(&mut Out) -> Result<R>) -> Result<R> {
        let start = self.buf.len();
        let value = f(self)?;
        self.map.entries.push(MapEntry {
            generated: start..self.buf.len(),
            source: *span,
            // The emitter writes one module at a time and does not know which:
            // `SourceMap::placed` stamps it when the driver joins them.
            unit: 0,
        });
        Ok(value)
    }

    /// Render into a buffer of its own, so the caller can decide what to do
    /// with the result before committing to it.
    /// **Takes back a `&` just written**, where what follows is already the
    /// reference it asked for (0.0.250): `&*get(&v, i)` is `get(&v, i)`, and
    /// the long form is what `clippy` refuses in `std`. Not `&&`, which is the
    /// operator, and not `&mut `, which ends in a space. A `&` at the end of the
    /// buffer is inside no recorded span, so the map is untouched.
    fn take_lend(&mut self) -> bool {
        if self.buf.ends_with('&') && !self.buf.ends_with("&&") {
            self.buf.pop();
            return true;
        }
        false
    }

    fn scratch(f: impl FnOnce(&mut Out) -> Result<()>) -> Result<Out> {
        let mut out = Out::default();
        f(&mut out)?;
        Ok(out)
    }

    /// Insert text just inside the first `{`, moving the map entries after it.
    fn insert_after_first_brace(&mut self, text: &str) {
        let Some(at) = self.buf.find('{').map(|at| at + 1) else {
            return;
        };
        self.buf.insert_str(at, text);
        for entry in &mut self.map.entries {
            if entry.generated.start >= at {
                entry.generated.start += text.len();
            }
            if entry.generated.end >= at {
                entry.generated.end += text.len();
            }
        }
    }

    /// Append a scratch buffer, moving its map entries into place. Order is
    /// preserved, so "innermost first" survives.
    fn append(&mut self, other: Out) {
        let offset = self.buf.len();
        self.buf.push_str(&other.buf);
        for mut entry in other.map.entries {
            entry.generated.start += offset;
            entry.generated.end += offset;
            self.map.entries.push(entry);
        }
    }
}

pub fn emit_program(parsed: &Parsed, build: Build) -> Result<Lowered> {
    let trust = crate::contracts::trust::analyse(parsed, std_ledger());
    Emitter::new(parsed, build, trust.provenance).program()
}

/// The same, told what this build may read while it builds
/// ([ADR-310](../../docs/specification/adr/adr-310.md)).
///
/// **A second entry point rather than a field on [`Build`]**, for the reason
/// `beside` is a parameter: `Build` is the machine and the switches, copied
/// freely, and what a build may read is a fact about the *invocation* with a
/// lifetime on it. Every caller that has nothing to say passes
/// [`assets::Reads::none`], which is D1.
pub fn emit_program_reading(parsed: &Parsed, build: Build, reads: &Reads) -> Result<Lowered> {
    let trust = crate::contracts::trust::analyse(parsed, std_ledger());
    Emitter::new_reading(parsed, build, trust.provenance, reads).program()
}

/// The same, with the provenance already decided.
///
/// The CLI analyses once and prints the answer under `--trust`; this is what it
/// hands back in so the emitted code and the explanation cannot disagree.
pub fn emit_program_with_trust(
    parsed: &Parsed,
    build: Build,
    provenance: crate::contracts::Provenance,
) -> Result<Lowered> {
    Emitter::new(parsed, build, provenance).program()
}

/// **`std`'s own Nikaia half**, which is the one program that must not import
/// the prelude: it *is* the prelude ([ADR-014](../../../docs/specification/adr/adr-014.md)
/// D1, [ADR-002](../../../docs/specification/adr/adr-002.md) D4).
///
/// Every other program gets `pub use nikaia_std::prelude::*;` whether or not it
/// asked, because what a program uses is not a list this emitter keeps and a name
/// the prelude provides has to resolve. `std` is where that stops being true, and
/// it is one program rather than a class - so it says so here rather than being
/// detected.
pub fn emit_std(parsed: &Parsed) -> Result<Lowered> {
    let own = crate::contracts::Ledger::infer_package(&[parsed], std_ledger());
    emit_std_against(parsed, &[], &own, &crate::contracts::Ledger::blank())
}

/// The same, for a module that reads Rust it was handed described: a tool of
/// `nikaia-std`'s, whose tree names are `winnow_grammar::Symbol`s
/// ([ADR-294](../../docs/specification/adr/adr-294.md) D9.3).
pub fn emit_std_against(
    parsed: &Parsed,
    beside: &[&Parsed],
    own: &crate::contracts::Ledger,
    described: &crate::contracts::Ledger,
) -> Result<Lowered> {
    // Named rather than defaulted: the default is the machine this runs on,
    // and the committed text must not depend on where it was lowered.
    let build = Build {
        target: Target::X86_64Linux,
        ..Build::default()
    };
    let mut library = std_ledger().clone();
    library.types.extend(described.types.clone());
    library.functions.extend(described.functions.clone());
    let trust = crate::contracts::trust::analyse(parsed, &library);
    Emitter::with_contracts(
        parsed,
        beside,
        build,
        trust.provenance,
        own.clone(),
        described,
        &Reads::none(),
    )
    .for_std()
    .program()
}

/// [`emit_std_against`]'s items and nothing else, for one file of the
/// toolchain's package (ADR-294): the package writes one preamble for all of
/// them.
pub fn emit_std_items_against(
    parsed: &Parsed,
    beside: &[&Parsed],
    own: &crate::contracts::Ledger,
    described: &crate::contracts::Ledger,
) -> Result<Lowered> {
    let build = Build {
        target: Target::X86_64Linux,
        ..Build::default()
    };
    emit_std_items_against_at(parsed, beside, own, described, build)
}

/// The same at a given build, for the test that holds the package to lowering
/// the same at both settings of `user_parallelism` (ADR-002 D4).
pub fn emit_std_items_against_at(
    parsed: &Parsed,
    beside: &[&Parsed],
    own: &crate::contracts::Ledger,
    described: &crate::contracts::Ledger,
    build: Build,
) -> Result<Lowered> {
    let mut library = std_ledger().clone();
    library.types.extend(described.types.clone());
    library.functions.extend(described.functions.clone());
    let trust = crate::contracts::trust::analyse(parsed, &library);
    Emitter::with_contracts(
        parsed,
        beside,
        build,
        trust.provenance,
        own.clone(),
        described,
        &Reads::none(),
    )
    .for_std()
    .items_only()
}

/// A module's items, with no preamble and no `mod` around them.
///
/// The driver writes the preamble once for the whole program and the `mod`
/// header itself, because only it knows what the program is made of
/// (`modules::collect`).
///
/// `contracts` are the **program's**, not this file's: a call to
/// `page::render(entries, total)` fills in Kap 5.1's defaults from the
/// declaration, and the declaration is in another file.
pub fn emit_module_body(
    parsed: &Parsed,
    build: Build,
    provenance: crate::contracts::Provenance,
    contracts: &crate::contracts::Ledger,
) -> Result<Lowered> {
    emit_module_body_at(
        parsed,
        &[],
        build,
        provenance,
        contracts,
        &crate::contracts::Ledger::blank(),
        false,
        &Reads::none(),
    )
}

/// The same, saying whether these items are the crate root's.
///
/// Only the crate root may carry the `fn main` Rust runs, and ADR-303 D4 makes
/// that one generated function rather than the program's own - so a module is
/// emitted with `false` and a `main` in it stays as written.
#[allow(clippy::too_many_arguments)]
pub fn emit_module_body_at(
    parsed: &Parsed,
    // **The program's other files**, for the one thing the emitter asks the
    // checker that needs a body: what a `comptime` came to. See
    // `check::propagation_against`.
    beside: &[&Parsed],
    build: Build,
    provenance: crate::contracts::Provenance,
    contracts: &crate::contracts::Ledger,
    // **The described crates' boundary** (ADR-290 D20), for whether a call into
    // one can fail. Empty where the program declares none.
    described: &crate::contracts::Ledger,
    entry: bool,
    // **And what it may read**, for the same reason one line up: the check the
    // emitter runs has to be the check that refused, or the two halves
    // disagree about one program ([ADR-310](../../docs/specification/adr/adr-310.md)).
    reads: &Reads,
) -> Result<Lowered> {
    Emitter::with_contracts(
        parsed,
        beside,
        build,
        provenance,
        contracts.clone(),
        described,
        reads,
    )
    .for_entry(entry)
    .items_only()
}

/// What a program's preamble has to say, over all of its files.
///
/// `uses_std` and the rest are per-file facts, and the preamble is written
/// once - so they are joined here rather than guessed at from the entry.
#[derive(Debug, Clone, Default)]
pub struct Needs {
    pub grammar: bool,
    pub driver: bool,
    pub std: bool,
    /// Kap 7.1: a function here declares `throws`, so the error surface is
    /// reachable and `std`'s is what carries it.
    pub fails: bool,
    /// Kap 4.7 across a package: every trait an `impl` in this unit names with
    /// a package in front of it
    /// ([ADR-286](../../docs/specification/adr/adr-286.md)).
    ///
    /// Rust needs a trait **in scope** before its methods can be called, and
    /// `impl http::Handler for Fixed` does not put it there. So a trait a
    /// package publishes could be implemented and then not called, in the
    /// backend's words about the generated file: *"trait `Handler` … is
    /// implemented but not in scope; perhaps you want to import it"* — a rule
    /// the program has no way to satisfy, because Nikaia has no import to
    /// write ([ADR-286](../../docs/specification/adr/adr-286.md) D11 brings no
    /// names in).
    ///
    /// The **same shape in one file compiles**, which is what says this is one
    /// missing emitted line rather than a question: there the trait is in the
    /// same module and needs no import.
    pub foreign_traits: std::collections::BTreeSet<String>,
}

impl Needs {
    pub fn of(parsed: &Parsed, build: Build) -> Needs {
        // What a program needs does not depend on which checks were proved,
        // and proving them is the slowest thing an emitter is built with.
        let build = Build {
            bounds: crate::bounds::BoundsChecks::Kept,
            overflow: crate::bounds::OverflowChecks::Kept,
            ..build
        };
        let emitter = Emitter::new(parsed, build, crate::contracts::Provenance::Trusted);
        Needs {
            grammar: parsed
                .program
                .items
                .iter()
                .any(|i| matches!(i.node, Item::Grammar(_))),
            driver: emitter.uses_driver(),
            std: emitter.uses_std,
            fails: emitter.fails,
            foreign_traits: foreign_traits(parsed),
        }
    }

    pub fn join(mut self, other: Needs) -> Needs {
        self.foreign_traits.extend(other.foreign_traits);
        Needs {
            grammar: self.grammar || other.grammar,
            driver: self.driver || other.driver,
            std: self.std || other.std,
            fails: self.fails || other.fails,
            foreign_traits: self.foreign_traits,
        }
    }

    /// The lines at the top of the emitted file.
    pub fn preamble(self) -> String {
        let mut out = String::new();
        if self.grammar {
            out.push_str("use winnow_grammar::grammar;\n");
        }
        if self.driver {
            out.push_str("use winnow_grammar::rt::Parallelism;\n");
            out.push_str("use winnow_grammar::ParseContext;\n");
        }
        // **Always, and allowed to be unused.** It used to be written only where
        // the program had a `use std::…` line of its own, which is not the same
        // question: `HashMap` is a name the prelude provides and a program may
        // write it without importing anything, and such a program lowered to a
        // file where `TrustedMap` was undeclared - `rustc` about a file nobody
        // wrote (Part III, C.1). What a program uses is not a list this emitter
        // keeps, and it does not need one: the import is one line, a locally
        // declared name shadows a glob, and `#[allow]` is what keeps an unused
        // one from reaching a reader (the same move the emitted `use super::*`
        // made).
        // Nikaia's `std` is a crate rather than a table in this file, so
        // `fs::map`, `cli::args` and `HashMap` resolve as written and what
        // they mean is code someone can read.
        // `pub use`, because the grammar module the backend generates
        // reaches these names through a glob of its own, and a private
        // import is not re-exported into one.
        out.push_str("#[allow(unused_imports)]\npub use nikaia_std::prelude::*;\n");
        if self.fails {
            // Kap 7.1: `error.full()` is a method on whatever a `catch` bound, so
            // the trait has to be in scope even in a program that imports no `std`
            // module of its own - `throw` needs `std` whether or not the source
            // mentions it.
            out.push_str("#[allow(unused_imports)]\npub use nikaia_std::error::Full;\n");
        }
        // **A trait reached across a package is imported**
        // ([ADR-286](../../docs/specification/adr/adr-286.md)). Written as the
        // path the program already wrote, because that is the path the
        // generated crate resolves: `impl http::Handler for Fixed` names the
        // crate `http`, and so does this.
        //
        // `allow(unused_imports)` for the same reason the two lines above carry
        // it - a program may implement a trait and never call the method, and a
        // warning about a line nobody wrote is
        // [Part III C.1](../../docs/specification/30-nikaia-tooling.md) one
        // severity down.
        for path in &self.foreign_traits {
            out.push_str(&format!("#[allow(unused_imports)]\npub use {path};\n"));
        }
        out
    }
}

/// Every trait an `impl` in this unit names with a package in front of it.
///
/// **Qualified only**, and that is the whole of the test: a trait of this
/// program's own is in the same module below and needs no import, while a
/// `::` in the name is exactly what says the declaration is in another crate.
/// A trait the compiler reads rather than a `.nika` file wrote - `impl Error
/// for ConfigError` ([ADR-023](../../docs/specification/adr/adr-023.md) D3) -
/// carries no `::` either, so it is left alone by the same test.
/// What each `extern "C"` declaration in this file takes
/// ([ADR-302](../../docs/specification/adr/adr-302.md) D5), by the name a call
/// writes.
fn foreign_params(parsed: &Parsed) -> std::collections::BTreeMap<String, Vec<Type>> {
    let mut out = std::collections::BTreeMap::new();
    for item in &parsed.program.items {
        let Item::Extern { declarations, .. } = &item.node else {
            continue;
        };
        for declaration in declarations {
            out.insert(
                parsed.text(declaration.node.name).to_string(),
                declaration
                    .node
                    .args
                    .iter()
                    .map(|a| a.ty.clone())
                    .collect::<Vec<_>>(),
            );
        }
    }
    out
}

/// The opaque handles this file declares, by name, with the function that ends
/// each one's life ([ADR-302](../../docs/specification/adr/adr-302.md) D7).
fn opaque_handles(parsed: &Parsed) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for item in &parsed.program.items {
        let Item::Extern { opaque, .. } = &item.node else {
            continue;
        };
        for handle in opaque {
            out.insert(
                parsed.text(handle.node.name).to_string(),
                parsed.text(handle.node.released_by).to_string(),
            );
        }
    }
    out
}

/// What each `extern "C"` declaration hands back, by the name a call writes
/// ([ADR-302](../../docs/specification/adr/adr-302.md) D12).
fn foreign_results(parsed: &Parsed) -> std::collections::BTreeMap<String, Type> {
    let mut out = std::collections::BTreeMap::new();
    for item in &parsed.program.items {
        let Item::Extern { declarations, .. } = &item.node else {
            continue;
        };
        for declaration in declarations {
            if let Some(ret) = &declaration.node.ret_type {
                out.insert(parsed.text(declaration.node.name).to_string(), ret.clone());
            }
        }
    }
    out
}

fn foreign_traits(parsed: &Parsed) -> std::collections::BTreeSet<String> {
    parsed
        .program
        .items
        .iter()
        .filter_map(|item| match &item.node {
            Item::Impl {
                trait_name: Some(name),
                ..
            } => Some(parsed.text(*name).to_string()),
            _ => None,
        })
        .filter(|name| name.contains("::"))
        .collect()
}

/// `std`'s shipped contracts, parsed once.
///
/// A malformed ledger is a bug in this repository rather than in a user's
/// program, and the tests read the same file - so failing to parse it here
/// means treating the input as untrusted, which is the safe direction
/// (ADR-010 D1) and never a silent upgrade.
fn std_ledger() -> &'static crate::contracts::Ledger {
    crate::contracts::std_ledger()
}

/// The words the **language below** reserves.
///
/// Most of them this language does not reserve. Four it does - `ref`, `trait`,
/// `extern` and `unsafe` - and they stay because a reserved word is still a name
/// after `.` and `::` ([ADR-298](../../../docs/specification/adr/adr-298.md)
/// D3): a member called `ref` that a library declares is written `x.r#ref`
/// below. `tests/reserved_below.rs` holds all four to the escape.
///
/// Measured rather than copied out of a reference, and measured **twice**: the
/// first sweep read `rustc`'s *"found keyword"* at a `let` and came back with
/// twenty-four, missing `crate`, `super` and `box` because those three fail
/// there with a different message each (`E0532`, `E0433`, *"expected pattern"*).
/// The sweep in `tests/reserved_below.rs` compiles every word in every position
/// instead, which is what found them.
///
/// `gen` is in since the emitted code is Edition 2024, which reserves it
/// (ADR-305). `union` is deliberately **absent**: Rust takes it as an
/// identifier, so escaping it would be a change nothing asked for.
///
/// `crate`, `super`, `self` and `Self` are absent for the opposite reason —
/// **Rust forbids a raw identifier for exactly those four**, so there is no
/// escape to write. They are refused instead (`NK1128`, and `NK1119` for
/// `self`), which is [ADR-298](../../../docs/specification/adr/adr-298.md) D11.
const RESERVED_BELOW: &[&str] = &[
    "abstract", "async", "await", "become", "box", "const", "do", "dyn", "extern", "final", "gen",
    "loop", "macro", "mod", "move", "override", "priv", "ref", "static", "trait", "try", "type",
    "typeof", "unsafe", "unsized", "virtual", "where", "yield",
];

/// A source name, written so the language below can read it.
///
/// **This is [ADR-296](../../../docs/specification/adr/adr-296.md) D17 paying one
/// of its bills.** The emitter writes a name for a name and resolves nothing, so
/// a Nikaia name that Rust happens to spell as a keyword went out verbatim and
/// `rustc` answered *"expected identifier, found keyword `type`"* about a file
/// nobody wrote, with *"escape `type` to use it as an identifier"* as the help —
/// advice that means nothing in this language
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// **Escaped rather than reserved**, which is
/// [ADR-298](../../../docs/specification/adr/adr-298.md) D9: reserving these
/// words in Nikaia would let the backend decide what this language's vocabulary
/// is, and `type` is the field name of every tagged record anybody has ever
/// written.
///
/// **`const` and `loop` joined the list when they left Nikaia's own**
/// ([ADR-298](../../../docs/specification/adr/adr-298.md) D6), which is that
/// record's *answered as it is for `type`*: they were not here before because a
/// Nikaia name could not be one of them, and now it can. `macro` was already
/// here, and `quote` is a keyword in neither language. The three the backend
/// cannot escape at all — `crate`, `super` and `Self` — are `NK1128`'s and not
/// this list's.
///
/// Borrowed where nothing changes, which is every name in every program written
/// today.
pub fn escaped(name: &str) -> std::borrow::Cow<'_, str> {
    match RESERVED_BELOW.contains(&name) {
        true => std::borrow::Cow::Owned(format!("r#{name}")),
        false => std::borrow::Cow::Borrowed(name),
    }
}

/// `<'a, T>`, or nothing at all where there is nothing to declare.
///
/// One place, because three positions write one - a `fn`, a `struct` and the
/// `impl` over it - and a list that is empty has to be written as the empty
/// string rather than as `<>`, which is not Rust.
fn angled(parts: &[String]) -> String {
    match parts.is_empty() {
        true => String::new(),
        false => format!("<{}>", parts.join(", ")),
    }
}

struct Emitter<'p> {
    parsed: &'p Parsed,
    build: Build,
    /// Structs that hold a view into the input, and so need the input lifetime
    /// wherever they are named.
    ///
    /// Read through [`Emitter::borrows`] and not directly: this half is what
    /// *this file* declares, and a package is several files (Part I, 9.1).
    borrowing: HashSet<Symbol>,
    /// The other half: the types the **package's** ledger records a `tethered`
    /// field for (`contracts::TypeContract`).
    ///
    /// `examples/inventory` is why. `Entry` is declared in `stock.nika` and
    /// named in `page.nika`, and each file is emitted from its own `Parsed`
    /// (`emit_module_body_ordered`) - so the syntactic set above is empty in the
    /// file that writes `Vec[Entry]`, and the lifetime went unwritten there.
    /// Rust's elision covered it in a plain `fn` and stops covering it in an
    /// `async fn` (E0726), which is how a latent defect became a message about
    /// the generated file (Part III, C.1).
    ///
    /// The ledger is the right place to read it from because a package has
    /// exactly one (Part III, 13.5) and its keys for the package's own types are
    /// unqualified - the same name the source writes.
    tethered: std::collections::BTreeSet<String>,
    /// The view parameters written as views of the *input* buffer rather than of
    /// whatever the caller lends for the call, by the byte their method starts
    /// at (`views::carried`).
    ///
    /// A parameter written `&str` says only "a view, for this call". Where the
    /// method stores it into its subject, and the subject already carries the
    /// input buffer, the view it stores is a view of **that** buffer - so the
    /// parameter is written with the input lifetime and the signature says what
    /// the body does. Where nothing names the buffer, `views::check` refuses the
    /// program instead (`NK2302`), so this set is never a guess.
    carries_input: HashMap<usize, HashSet<Symbol>>,
    grammars: HashMap<Symbol, &'p GrammarDef>,
    /// **What a `fold`'s or a `par_fold`'s step calls directly**, by name
    /// (#433): the step is the loop body over the input, so what it calls runs
    /// once per item, and is written `#[inline]`.
    fold_callees: HashSet<String>,
    /// Declared struct names: `Stats(x)` is a call to a constructor, and only
    /// the declarations say which names are types.
    structs: HashSet<Symbol>,
    /// The `for` statements whose step can fail, by the byte they start at
    /// (ADR-025 D1).
    fallible_loops: std::collections::BTreeSet<usize>,
    /// The `for`s whose **step pauses**
    /// ([ADR-172](../../docs/specification/adr/adr-172.md) D1), by the byte the
    /// statement starts at. Handed over exactly as `fallible_loops` is.
    pausing_loops: std::collections::BTreeSet<usize>,
    /// The `for`s over a name holding a sequence, which is handed over rather
    /// than lent ([ADR-293](../../docs/specification/adr/adr-293.md) D19).
    owned_loops: std::collections::BTreeSet<usize>,
    /// The lent `for` bindings read by value at the body's head
    /// ([`check::Checked::copied_loop_bindings`]).
    copied_loop_bindings: std::collections::BTreeSet<(usize, String)>,
    /// ([`check::Checked::copied_view_bindings`]).
    copied_view_bindings: std::collections::BTreeSet<(usize, String)>,
    /// [`check::Checked::unsigned_literals`].
    unsigned_literals: std::collections::BTreeMap<(usize, i128), String>,
    /// [`check::Checked::copied_tuple_parts`].
    copied_tuple_parts: std::collections::BTreeSet<(usize, String)>,
    /// [`check::Checked::number_lets`].
    number_lets: std::collections::BTreeMap<usize, String>,
    /// [`check::Checked::changed_elements`].
    changed_elements: std::collections::BTreeSet<(usize, String)>,
    /// The arguments that are a count in `usize` below, by the entry the checker
    /// resolved ([ADR-293](../../docs/specification/adr/adr-293.md) D20).
    count_args: std::collections::BTreeSet<(usize, String, usize)>,
    /// How a key goes into a map's brackets where the map's keys are owned
    /// ([ADR-293](../../docs/specification/adr/adr-293.md) D27).
    map_keys: std::collections::BTreeMap<(usize, String, bool), crate::check::KeyForm>,
    /// Indexes that are a range kept in a name (ADR-293 D23).
    slice_indices: std::collections::BTreeSet<(usize, String)>,
    /// [`check::Checked::list_indices`], kept only where [`crate::bounds`]
    /// proved the position inside: the indexes written without their check
    /// ([ADR-306](../../docs/specification/adr/adr-306.md) D5).
    proven_indices: std::collections::HashSet<usize>,
    /// [`check::Checked::arithmetic`], kept only where [`crate::bounds`]
    /// proved the operation stays inside its type: written `<T>::wrapping_*`
    /// ([ADR-306](../../docs/specification/adr/adr-306.md) D10).
    proven_arithmetic: std::collections::HashMap<usize, String>,
    /// `xs[i] op= v` on a list of numbers (`check::Checked::copied_slots`).
    copied_slots: std::collections::BTreeSet<usize>,
    /// `std` copies, written `to_owned` (ADR-293 D24).
    owned_copies: std::collections::BTreeSet<(usize, String)>,
    /// Whether the `?.` being written copies the view it reached, which is
    /// `owned_copies` asked of the receiver the `match` bound.
    reached_copy: std::cell::Cell<bool>,
    /// `.to_string()` on text, written as its receiver (ADR-282 D8).
    text_as_is: std::collections::BTreeSet<(usize, String)>,
    /// `??`s that lend their left side, and how it is opened (ADR-279 D5).
    lent_coalesces: std::collections::BTreeMap<(usize, String), &'static str>,
    /// A number, `bool` or `char` a `let` reads past a jump, copied (#456).
    copied_jump_reads: std::collections::BTreeSet<(usize, String)>,
    /// **`std`'s count of a sequence**, by statement and receiver shape, which
    /// gets the conversion a length gets (Part I 2.2).
    counted: std::collections::BTreeSet<(usize, String)>,
    /// **The names a `match` arm binds as a view**, by the arm's pattern
    /// (ADR-291): written `ref name`, so the scrutinee stays whole.
    lent_bindings: std::collections::BTreeMap<usize, std::collections::BTreeSet<String>>,
    /// **A guard that reads names bound inside a boxed part**, by the arm's
    /// pattern, and which of them copy (ADR-246 D5 item 2).
    guards_inside_boxes: std::collections::BTreeMap<usize, std::collections::BTreeSet<String>>,
    /// `check::Checked::lent_scrutinees`.
    lent_scrutinees: std::collections::BTreeSet<usize>,
    /// `collect()` into a declared map, set or text (ADR-293 D25).
    collected_into: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::copied_walks` (ADR-231 D1).
    copied_walks: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::filter_patterns` (ADR-231 D2).
    filter_patterns: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::kept_lambdas`.
    kept_lambdas: std::collections::BTreeMap<(usize, String), (bool, bool)>,
    /// The first lines of the function body being written: a published
    /// parameter both kinds of text flow into, taken as it goes in
    /// (ADR-282 D23).
    either_prelude: std::cell::RefCell<Vec<String>>,
    /// `check::Checked::kept_hulls` (ADR-230 D4).
    kept_hulls: std::collections::BTreeMap<(usize, String), Vec<String>>,
    /// `check::Checked::kept_calls`.
    kept_calls: std::collections::BTreeMap<(usize, String), (bool, bool)>,
    /// `check::Checked::kept_args`.
    kept_args: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::kept_functions`.
    kept_functions: std::collections::BTreeMap<(usize, String), (bool, bool, usize)>,
    /// Set while the call a kept named function stands for is written, whose
    /// callee is that same name.
    in_a_kept_call: std::cell::RefCell<bool>,
    /// `check::Checked::field_calls`.
    field_calls: std::collections::BTreeMap<(usize, String), bool>,
    /// Set while a parameter's type is written, where a function type the
    /// callee only runs is a closure argument rather than a kept value.
    in_parameter: std::cell::RefCell<bool>,
    /// The rules of the grammar being written and the type each hands back,
    /// lowered - for `list(item, sep)`, whose element type is the item rule's
    /// ([`LIST_RULE`]).
    grammar_rule_types: std::cell::RefCell<std::collections::BTreeMap<String, String>>,
    /// The walks of a pausing sequence that have no form
    /// ([ADR-172](../../docs/specification/adr/adr-172.md) D5), by the byte the
    /// statement starts at and the method's name.
    pausing_walks: std::collections::BTreeSet<(usize, String)>,
    /// The `let`s whose place-initialiser has to be lent
    /// ([ADR-094](../../docs/specification/adr/adr-094.md) D4), by the byte the
    /// statement starts at. Answered by the checker for the reason every set
    /// beside it is: it is a question about the type, and this has none
    /// (ADR-288).
    lent_lets: std::collections::BTreeSet<usize>,
    /// [`crate::check::Checked::lent_returns`]: the `return`s whose value is a
    /// view of the subject, so the `&` is this file's to write
    /// ([ADR-094](../../../docs/specification/adr/adr-094.md) D1's third
    /// position). Handed over exactly as `lent_lets` is, and for the same
    /// reason: which place is a view of what is a question about types, and
    /// this file keeps none ([ADR-288](../../../docs/specification/adr/adr-288.md)).
    lent_returns: std::collections::BTreeSet<usize>,
    /// [`crate::check::Checked::compares`] and its total half: which declared
    /// types derive `PartialEq`, and which of those also derive `Eq`
    /// ([ADR-204](../../../docs/specification/adr/adr-204.md) D1).
    ///
    /// Decided by the checker for the reason every set beside it is: whether
    /// every part of a type compares is a question about types, and this file
    /// keeps none ([ADR-288](../../../docs/specification/adr/adr-288.md)).
    compares: std::collections::BTreeSet<String>,
    compares_totally: std::collections::BTreeSet<String>,
    /// Which declared types derive `Copy` ([`check::Checked::copies`]).
    copies: std::collections::BTreeSet<String>,
    /// Where a number is read through a `for` binding and therefore through a
    /// **view** ([`check::Checked::viewed_numbers`]), by the statement's byte
    /// and the name. Handed over exactly as `lent_lets` is, and for the same
    /// reason: which name is a view is a question about the scope, and this
    /// keeps none (ADR-288).
    viewed_numbers: std::collections::BTreeSet<(usize, String)>,
    /// The list literals that are an **array**
    /// ([ADR-152](../../docs/specification/adr/adr-152.md) D4), by the byte the
    /// `[` stands at. `vec![…]` for one that is not in here and `[…]` for one
    /// that is; the checker decides, because the decision is a type's
    /// (ADR-288).
    array_literals: std::collections::BTreeSet<usize>,
    /// The text literals that lower to a `String` of their own
    /// ([ADR-282](../../docs/specification/adr/adr-282.md) D4), by the byte
    /// the opening quote stands at. `"x"` for one that is not in here and
    /// `String::from("x")` for one that is; the checker decides, for
    /// `array_literals`' reason.
    owned_texts: std::collections::BTreeSet<usize>,
    /// **Where each buffer lives**, per function
    /// ([ADR-283](../../docs/specification/adr/adr-283.md)): the keep plan,
    /// computed once over the unit against the package's ledger.
    keep_plans: HashMap<String, crate::contracts::keep::Plan>,
    /// `check::Checked::lent_list_reads`.
    lent_list_reads: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::view_fallbacks`.
    view_fallbacks: std::collections::BTreeSet<usize>,
    /// `check::Checked::view_coalesces`.
    view_coalesces: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::lent_map_fallbacks`.
    lent_map_fallbacks: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::optional_fallbacks`.
    optional_fallbacks: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::flat_reads`.
    flat_reads: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::copied_reads`.
    copied_reads: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::text_in_lists`.
    text_in_lists: std::collections::BTreeSet<(usize, String)>,
    /// The f-string hole being written, whose literals are keyed by
    /// `check::text_key`.
    hole: std::cell::RefCell<Option<(usize, String)>>,
    /// The statements whose keeps are already declared, because a `throws`
    /// body declares them before its `Ok(` rather than inside it.
    preluded: std::cell::RefCell<HashSet<usize>>,
    /// The handle types a task's tethered values are packed in (D3), written
    /// once at the end of the unit.
    wrappers: std::cell::RefCell<Vec<String>>,
    /// Writing a keeper whose views are held one handle per buffer (D4): `ref
    /// String` is `Held` while this is set, and a struct of views a `Holding`
    /// with this many keeps ([ADR-283](../../docs/specification/adr/adr-283.md)).
    holding: std::cell::RefCell<Option<usize>>,
    /// The arguments of the call being written that are held (D4): argument
    /// position and the buffer statements whose keeps hold it, one per keep
    /// its element carries.
    hold_args: std::cell::RefCell<Option<std::collections::BTreeMap<usize, Vec<usize>>>>,
    /// The function being written, for the keep plan: a lambda's body is
    /// written with a flow of its own (`Flow::PLAIN`), and its statements are
    /// still the function's to the plan that walked them.
    keep_function: std::cell::RefCell<String>,
    /// **The counters of each function that are never negative**, by its key
    /// ([ADR-270](../../docs/specification/adr/adr-270.md) D8 step 1): a
    /// local that starts at a non-negative literal and is only ever given or
    /// added one, and the binding of a range that starts at one. Indexed and
    /// compared with a length as the `usize` it is, so no sign is tested and
    /// the optimiser sees one unsigned value in the loop's test and the read.
    nonnegative: std::cell::RefCell<HashMap<String, HashSet<Symbol>>>,
    /// The method calls that can fail, by the byte their statement starts at
    /// and the method's name (ADR-023 D8).
    ///
    /// Handed over exactly as `fallible_loops` is, and for the reason stated
    /// there: `stats.add(5)` names `add` and says nothing about what `stats`
    /// is, so only the type checker can say what it calls (ADR-288). Nothing
    /// here resolves a receiver.
    fallible_methods: std::collections::BTreeSet<(usize, String)>,
    /// The `set` calls that carry a **witness**
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D26), by the byte
    /// their statement starts at (`check::Checked::witnessed_sets`).
    ///
    /// `kasse.set(neu; after: stand)` lowers to `set_after(neu, stand)`, which
    /// compares and stores while the lock is open once. Whether the receiver is
    /// a lock at all is the type checker's answer and not this file's (ADR-288),
    /// which is why it arrives rather than being worked out — a user-defined
    /// `set` with an `after:` option of its own is left alone.
    witnessed_sets: std::collections::BTreeSet<usize>,
    /// The lambda arguments written as a closure returning a **boxed future**
    /// ([ADR-277](../../../docs/specification/adr/adr-277.md) D11), by the byte
    /// their statement starts at and the argument's position
    /// (`check::Checked::future_lambdas`).
    ///
    /// Which shape a parameter takes is a question about its **type**, and this
    /// file has none ([ADR-288](../../../docs/specification/adr/adr-288.md)) —
    /// the same arrangement `lent_args` and `nullable_args` arrive by.
    future_lambdas: std::collections::BTreeSet<(usize, usize)>,
    /// Of those, the ones whose parameter the callee **runs**, which take an
    /// `async` closure rather than a boxed future
    /// (`check::Checked::run_lambdas`).
    run_lambdas: std::collections::BTreeSet<(usize, usize)>,
    /// **The functions whose body is a settle point's**, and whether the settle
    /// point can fail each ([ADR-297](../../docs/specification/adr/adr-297.md)
    /// D2, D3).
    settle_fns: std::collections::BTreeMap<String, bool>,
    /// **The `let`s whose block is settled where it ends**, and whether what
    /// they bind can fail its cleanup (ADR-297 D2).
    settle_lets: std::collections::BTreeMap<usize, bool>,
    /// **This program's types with a cleanup that can pause** (ADR-297 D1):
    /// written as `Cleaned<T>`, whose `Drop` parks it.
    cleaned_types: std::collections::BTreeSet<String>,
    /// The lambdas that pause, handed to a `std` entry, and what the call is
    /// lowered to ([ADR-233](../../docs/specification/adr/adr-233.md) D1, D2).
    pausing_lambdas: std::collections::BTreeMap<(usize, String), PausingEntry>,
    /// The method calls that **pause**, by the byte their statement starts at
    /// and the name written (`check::Checked::pausing_methods`).
    ///
    /// The same arrangement, one ledger column over: `throws` becomes a `?` and
    /// *can pause* becomes an `.await`. The emitter cannot ask this one itself
    /// for a method, because only a type checker knows what `stats.add(5)` goes
    /// to (ADR-288).
    pausing_methods: std::collections::BTreeSet<(usize, String)>,
    /// The conversions that **narrow**, by the byte their statement starts at
    /// and the type converted to
    /// ([ADR-285](../../../docs/specification/adr/adr-285.md) D12).
    ///
    /// Handed over for the reason the two above are: `as i32` narrows or widens
    /// depending on what it is *given*, and nothing here knows that (ADR-288).
    /// The emitter writes the checked conversion and never decides which one it
    /// is.
    narrowing_casts: std::collections::BTreeMap<(usize, String), crate::check::Narrowing>,
    /// Which reference count each `Shared` value got, by the slot key
    /// `function::value` ([ADR-037](../../../docs/specification/adr/adr-037.md)
    /// D7).
    ///
    /// **Per value and not per type**, which is why `map_name` cannot answer it:
    /// `Shared[T]` is one atomic count at both settings of `user_parallelism`
    /// (D6), and a value the analysis proves never crosses a thread gets a plain
    /// one instead. That is a question about where a particular handle goes, so
    /// it is computed by `contracts::sharing` over the whole unit and looked up
    /// here - the same arrangement `fallible_loops` and `fallible_methods` have,
    /// and for the same reason.
    ///
    /// A count belongs to the **allocation**, so every position of one
    /// union-find class carries the same answer and the types a call writes on
    /// both sides of it agree by construction. A slot this map does not have gets
    /// the atomic floor.
    shared: std::collections::BTreeMap<String, crate::contracts::sharing::Count>,
    /// Part I 2.3: the statements where a plain value stands in a nullable slot,
    /// and **which** of the two wraps this emitter writes there
    /// (`check::Checked::nullable_sites`,
    /// [ADR-278](../../../docs/specification/adr/adr-278.md)).
    nullable_sites: std::collections::BTreeMap<usize, crate::check::Wrap>,
    /// The `+`s that join text, by the byte their operator starts at
    /// ([ADR-081](../../docs/specification/adr/adr-081.md) D2).
    ///
    /// The one answer in this channel keyed by an **expression** rather than by
    /// a statement, because `a + b + c` is two of them and the statement they
    /// stand in is one.
    concatenations: std::collections::BTreeSet<usize>,
    /// What each `comptime` is written as below - its type and its value - by the
    /// byte its statement starts at (`check::Checked::comptime_values`,
    /// [ADR-287](../../docs/specification/adr/adr-287.md) D4, D5).
    ///
    /// Here for the reason every table beside it is: this has neither types nor
    /// a scope ([ADR-288](../../docs/specification/adr/adr-288.md)), and a
    /// `const` needs both - a type because Rust's `const` takes one, and a
    /// scope because `const PAIR = PAGE * 2` folds only for something that
    /// knows what `PAGE` is. A statement with no entry never arrives, because
    /// the checker refused it as `NK1127` first.
    comptime_values: std::collections::BTreeMap<usize, (String, String)>,
    /// **The functions the build runs** (#446): `built_at_build_time`.
    built_at_build_time: HashSet<String>,
    /// **The type each `with` copies**, by the byte the word stands at
    /// (`check::Checked::with_types`,
    /// [ADR-118](../../docs/specification/adr/adr-118.md) D1).
    ///
    /// Rust's functional update writes the name — `Point { x: 1, ..p }` — and
    /// this walk has no types, so it is told. A `with` with no entry never
    /// arrives: the checker refused it as `NK1173` first.
    with_types: std::collections::BTreeMap<usize, String>,
    /// **What a `T::fields` loop was unrolled over**
    /// ([ADR-304](../../docs/specification/adr/adr-304.md) D6), handed over by
    /// the checker because this emitter has no types
    /// ([ADR-288](../../docs/specification/adr/adr-288.md)) and a type argument
    /// is what decides which copy a call means.
    unrolled: std::collections::BTreeMap<(String, String), Vec<crate::contracts::FieldContract>>,
    /// The functions whose body walks a type's fields, whether or not anything
    /// called them ([`check::Checked::walks_fields`]).
    walks_fields: std::collections::BTreeMap<String, String>,
    /// The calls that go to a specialised copy, by the byte the call starts at.
    unrolled_calls: std::collections::BTreeMap<usize, String>,
    /// **The copy being written**, while one is: the concrete type the
    /// parameter stands for, and the parameter's own name.
    ///
    /// A `RefCell` because the emitter is `&self` everywhere — it writes rather
    /// than decides, and this is the one place it carries a position down a
    /// walk it does not own.
    specialising: std::cell::RefCell<Option<(String, String)>>,
    /// The field this unrolled turn stands at: the binding the `for` wrote, and
    /// the field's own name.
    at_field: std::cell::RefCell<Option<(String, String)>>,
    /// Whether the code parameter being written is one the body **runs**
    /// ([ADR-277](../../docs/specification/adr/adr-277.md) D11).
    ///
    /// Set around the one `ty_counted` call that writes a parameter's type, for
    /// the reason `specialising` above is a cell: the type writer takes a type
    /// and this is a fact about the **position**. Run is the absence of
    /// `keeps`, read off the contract the declaration writer already has in
    /// hand for `lends`.
    code_parameter_runs: std::cell::RefCell<bool>,
    /// Part I 3.5: the `?.` reaches whose field is itself nullable and which
    /// therefore flatten (`check::Checked::flattened_reaches`).
    flattened_reaches: std::collections::BTreeSet<(usize, String)>,
    /// Part I 3.5: the `?.` reaches whose field **copies**, so the receiver can
    /// be taken by `as_ref()` and left where it was
    /// (`check::Checked::copied_reaches`).
    copied_reaches: std::collections::BTreeSet<(usize, String)>,
    /// Part I 3.5: the `?.` reaches whose member comes out as a **view** of the
    /// receiver, and which of the two spellings it is taken with
    /// (`check::Checked::viewed_reaches`).
    viewed_reaches: std::collections::BTreeMap<(usize, String), crate::check::Viewed>,
    /// Part I 3.5: the `?.` reaches over a method that changes nothing, so the
    /// scrutinee can be taken by `as_ref()` (`check::Checked::lent_reaches`).
    lent_reaches: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::held_reaches` (ADR-278 D21).
    held_reaches: std::collections::BTreeSet<(usize, String)>,
    /// The receivers held before the statement being written, by address, and
    /// the name each is held under.
    held: std::cell::RefCell<HashMap<usize, String>>,
    /// Part I 2.3: the struct-literal fields where a plain value stands in a
    /// nullable slot (`check::Checked::nullable_fields`).
    nullable_fields: std::collections::BTreeMap<
        (usize, String, String),
        std::collections::BTreeMap<String, crate::check::Wrap>,
    >,
    /// **The arguments the compiler writes a `&` for**
    /// ([ADR-094](../../docs/specification/adr/adr-094.md) D1), keyed exactly
    /// as `nullable_args` is and for the same reason: a statement may call one
    /// function twice.
    ///
    /// The *declaration* side of this answer is read straight from the ledger
    /// (`contracts::keeps::lends`), because that half is a question about the
    /// callee alone. This half needed the checker, because it also asks whether
    /// this argument is already a view.
    lent_args:
        std::collections::BTreeMap<(usize, String, usize), std::collections::BTreeSet<String>>,
    /// `check::Checked::lookup_keys` (0.0.235).
    lookup_keys: std::collections::BTreeSet<(usize, String, String)>,
    /// `check::Checked::boxed_members`
    /// ([ADR-246](../../docs/specification/adr/adr-246.md)).
    boxed_members: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    /// `check::Checked::boxed_reads`.
    boxed_reads: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::copied_bindings` (0.0.236).
    copied_bindings: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::some_tails`: a plain arm beside a `null`, by address.
    some_tails: std::collections::BTreeSet<usize>,
    /// `check::Checked::lent_options`: a value an option of a `ref` type is
    /// lent, by address (#455).
    lent_options: std::collections::BTreeSet<usize>,
    /// `check::Checked::wrapped`: a plain value in a nullable slot, by address.
    wrapped: std::collections::BTreeMap<usize, crate::check::Wrap>,
    /// `check::Checked::compared_views` (0.0.238).
    compared_views: std::collections::BTreeSet<(usize, String)>,
    /// `check::Checked::claims` ([ADR-269](../../docs/specification/adr/adr-269.md) D2).
    claims: std::collections::BTreeMap<(usize, String), crate::check::Claim>,
    /// `check::Checked::entries` ([ADR-269](../../docs/specification/adr/adr-269.md) D20).
    entries: std::collections::BTreeMap<String, Vec<crate::prove::Check>>,
    /// `check::Checked::reaches` (ADR-269 D20).
    reaches: crate::prove::Reaches,
    /// `check::Checked::published` (ADR-269 D18).
    published: std::collections::BTreeMap<String, crate::prove::Published>,
    /// Which of the arguments `args` is writing go into a boxed part of a
    /// variant ([ADR-246](../../docs/specification/adr/adr-246.md) D3): set
    /// around one call by `call` and read by `args`, as `hold_args` is.
    boxed_args: std::cell::RefCell<Option<Vec<bool>>>,
    /// **The count each part of a variant was given**, set around one call by
    /// `call` and read by `args` as `boxed_args` is (0.0.244, issue #158
    /// issue #158): a `Shared(…)` written straight into a part takes the part's
    /// count, or the two would be an `Rc` and an `Arc` of one value.
    variant_counts: std::cell::RefCell<Option<Vec<crate::contracts::sharing::Count>>>,
    /// The count the next hull written takes, where its position decided it.
    hull_count: std::cell::Cell<Option<crate::contracts::sharing::Count>>,
    /// **The arguments the compiler writes a `&mut` for**
    /// ([ADR-094](../../docs/specification/adr/adr-094.md) D3), keyed as
    /// `lent_args` is. The third state, and the one that is a declaration
    /// rather than an inference.
    mut_args:
        std::collections::BTreeMap<(usize, String, usize), std::collections::BTreeSet<String>>,
    /// [`crate::check::Checked::copied_args`] (ADR-233 D4).
    copied_args:
        std::collections::BTreeMap<(usize, String, usize), std::collections::BTreeSet<String>>,
    /// Part I 2.3: the call arguments where a plain value stands in a nullable
    /// parameter, by statement, callee as written, and position
    /// (`check::Checked::nullable_args`).
    nullable_args: std::collections::BTreeMap<
        (usize, String, usize),
        std::collections::BTreeMap<String, crate::check::Wrap>,
    >,
    /// **The options a method call has**, in the declaration's order, by the
    /// byte the statement starts at and the method's written name
    /// (`check::Checked::method_options`).
    ///
    /// A call **by name** reads its options off the callee's contract here
    /// (`options_of`); a method cannot, because finding the entry means
    /// resolving the receiver and this has no types
    /// ([ADR-288](../../../docs/specification/adr/adr-288.md)).
    method_options: std::collections::BTreeMap<(usize, String), Vec<(String, String)>>,
    /// The handles a task's body uses, by the byte the statement starts at and
    /// the name (`check::Checked::task_handles`).
    ///
    /// A task takes what it names **by value**, and a handle handed on by value
    /// is duplicated ([ADR-312](../../../docs/specification/adr/adr-312.md) D1).
    /// For a call the callee's signature answers it; a task's body has no
    /// signature, so the checker answers it and this writes the step.
    task_handles: std::collections::BTreeSet<(usize, String)>,
    /// ADR-055 D6: what each pausing function can reach, over the pausing ones
    /// ([`pausing_reach`]).
    ///
    /// A recursive `async fn` is an infinitely sized future, so a call that
    /// closes a cycle has to put one behind a pointer - and *which* calls those
    /// are is this map's question: a call from `f` to `g` closes one exactly
    /// where `g` reaches `f` again. Read through
    /// [`Emitter::closes_a_pausing_cycle`].
    pausing_reach: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    /// This unit's own contracts, and `std`'s. A call's options come from the
    /// declaration, and a declaration is what a ledger records (Kap 5.1).
    own_contracts: crate::contracts::Ledger,
    library: &'static crate::contracts::Ledger,
    /// **The described crates' boundary**
    /// ([ADR-290](../../docs/specification/adr/adr-290.md) D20). Asked one
    /// question, whether a call into one can fail - and deliberately not
    /// merged into `library`, whose `sync = false` means *await it*: a
    /// described function that may block is an ordinary Rust call.
    described: crate::contracts::Ledger,
    /// The types **this unit declares and gives an `impl Error`**
    /// ([ADR-280](../../docs/specification/adr/adr-280.md) D9).
    ///
    /// What is thrown implements `Error` and the `impl` line says so (Part I
    /// 7.1, `NK1161`), so this is the set a failure channel may be named after:
    /// a name here resolves in the generated file, and one another package
    /// declares does not.
    declared_errors: std::collections::BTreeSet<String>,
    /// Every distinct error set of two or more named members in this unit, and
    /// the type that stands for it
    /// ([ADR-280](../../docs/specification/adr/adr-280.md) D15).
    ///
    /// Held rather than derived per question, because a `catch` deep inside a
    /// body needs the **name** and a name computed on the spot has nowhere to
    /// live.
    sums: std::collections::BTreeMap<Vec<String>, String>,
    /// ADR-292: whether two statements that meet on nothing may overlap.
    /// What the program's `impl` blocks declare, which is what makes the fold
    /// adapter of D2 a lookup rather than a guess.
    methods: HashMap<(Symbol, Symbol), Method>,
    /// The same, by method name alone, for the places where the receiver's type
    /// is not written down. `None` marks a name two impls disagree about - and
    /// an ambiguous name is left alone rather than resolved by preference.
    by_name: HashMap<Symbol, Option<Method>>,
    /// Whether the program imports anything from `std`. Kept because a
    /// **grammar** module's glob needs to know whether there is anything to
    /// re-export; the preamble no longer asks (see [`emit_std`]).
    uses_std: bool,
    /// Whether this *is* `std`, the one program that may not import the prelude.
    is_std: bool,
    fails: bool,
    /// ADR-010: nobody outside the program chose the bytes its maps are keyed
    /// by, so a map may have the fast hash.
    trusted_input: bool,
    /// ADR-296 D5: the functions that accept a DSL's deferred parameters, by
    /// name. A `;` at a call means options everywhere else, and this is what
    /// says which calls mean the other thing.
    dsl_drivers: HashSet<String>,
    /// **What an `extern "C"` declaration takes, by name and position**
    /// ([ADR-302](../../docs/specification/adr/adr-302.md) D5).
    ///
    /// A declaration says `&[u8]` and C wants a pointer, so the *call* is where
    /// the pointer is made — `bytes.as_ptr()` rather than `bytes`. It is read
    /// from this file's own items rather than handed over by the checker, which
    /// is the one place that arrangement works: the emitter resolves a free
    /// call by name already ([ADR-296](../../docs/specification/adr/adr-296.md)
    /// D2), and a foreign declaration is a name in this very file.
    foreign_params: std::collections::BTreeMap<String, Vec<Type>>,
    /// **The opaque handles this file declares, and what releases each**
    /// ([ADR-302](../../docs/specification/adr/adr-302.md) D7).
    ///
    /// A handle is **lent** to every declaration but one: it is the address by
    /// value, and the caller keeps the value so its `cleanup` still runs at the
    /// end of its scope. The exception is the type's own release function,
    /// which takes it — that call *is* the cleanup, and a handle given away
    /// must not be released twice.
    opaque_handles: std::collections::BTreeMap<String, String>,
    /// What each `extern "C"` declaration hands back, by the name a call writes
    /// ([ADR-302](../../docs/specification/adr/adr-302.md) D12). A handle needs
    /// its hull put on at the call, and whether the declaration said `?`
    /// decides which one.
    foreign_results: std::collections::BTreeMap<String, Type>,
    /// Whether these items are the crate root - the one file that may carry
    /// the program's entry point.
    ///
    /// [ADR-303](../../../docs/specification/adr/adr-303.md) D4 starts the
    /// runtime before the first statement the user wrote, and the way to do
    /// that is to write the `fn main` Rust runs and call the program's own
    /// `main` from inside it. That may only happen once per program, so a
    /// module's items are emitted with this `false` and a `main` in them is
    /// left exactly as written.
    entry: bool,
}

/// What an `impl` says about a method. Enough to adapt a `&mut self` method to
/// the monoid a `par_fold` needs, and never more than the declaration states.
#[derive(Debug, Clone)]
struct Method {
    receiver: Option<Receiver>,
    returns_value: bool,
    args: Vec<FnArg>,
}

impl Method {
    /// A method that mutates its subject and yields nothing - the shape that
    /// has to be threaded to become a fold's step or merge.
    fn mutates_in_place(&self) -> bool {
        !self.returns_value && self.receiver.is_some_and(|r| r.is_ref && r.is_mut)
    }
}

/// How `Self::dsl` is spelled in the source, and what it becomes below.
///
/// ADR-296 D5 gives a driver one way to name the type a DSL string generates,
/// and the emitted Rust needs a name for it that no program can collide with.
const SELF_DSL: &str = "Self::dsl";
const DSL_PARAMETER: &str = "NikaiaDsl";

/// **The name a length a parameter left open is declared under**
/// ([ADR-184](../../docs/specification/adr/adr-184.md) D3).
///
/// `xs: Array[T]` says *an array of any length*, and the call is what says
/// which — so the language below gets a `const` parameter and the position it
/// belongs to, which is what makes two arrays in one signature two lengths.
/// The word is this compiler's and no program can collide with it: a Nikaia
/// name cannot begin with `__`.
const LENGTH_PARAMETER: &str = "__NIKAIA_N";

/// The name a Nikaia program gives its entry point.
const MAIN: &str = "main";
/// **A precondition checked where it stands** (ADR-269 D20), and what its
/// failure says (ADR-269 D2): the condition, the `assert`'s message, and the
/// value of each parameter it reads.
fn failed_check(check: &crate::prove::Check) -> String {
    let message = match &check.message {
        Some(text) => format!("Some(&{text:?})"),
        None => "None".to_string(),
    };
    let operands: String = check
        .operands
        .iter()
        .map(|(name, value)| {
            format!("({name:?}, (&&nikaia_std::abort::Operand(&{value})).shown()), ")
        })
        .collect();
    format!(
        "if !{} {{ nikaia_std::abort::assertion_failed({:?}, {message}, &[{operands}]) }} ",
        check.rust, check.written
    )
}

/// The suffix of a function's unchecked entry
/// ([ADR-269](../../docs/specification/adr/adr-269.md) D20): `f` checks its
/// precondition and calls `f__unchecked`, which is the body.
const UNCHECKED: &str = "__unchecked";

/// The type several parts of a program own at once (Part I 6.2).
///
/// The one name whose lowering is decided per **value**: what it expands to is a
/// reference count, and which of the two a particular value gets is
/// `contracts::sharing`'s answer ([ADR-037](../../../docs/specification/adr/adr-037.md)
/// D7).
use crate::contracts::ty::ARRAY;

/// **The functions of this file the build calls** (#446): what a `comptime`
/// and an option's default call, and what those call in turn.
///
/// Their values reach the Rust as literals, so the Rust may never call them,
/// and `rustc` would say *never used* about a function the program used. Each
/// gets `#[allow(dead_code)]`; a function nothing calls is still warned.
fn built_at_build_time(parsed: &Parsed) -> HashSet<String> {
    use crate::contracts::sync::{visit_expr, visit_expr_blocks, visit_stmt, visit_stmt_blocks};

    fn calls(parsed: &Parsed, expr: &Expr, found: &mut Vec<String>) {
        visit_expr(parsed, expr, &mut |inner| {
            if let Expr::Call { func, .. } = inner
                && let Expr::Variable(name) = func.as_ref()
            {
                found.push(parsed.text(*name).to_string());
            }
        });
        visit_expr_blocks(expr, &mut |block| block_calls(parsed, block, found));
    }
    fn block_calls(parsed: &Parsed, block: &Block, found: &mut Vec<String>) {
        for stmt in &block.stmts {
            visit_stmt(parsed, &stmt.node, &mut |expr| calls(parsed, expr, found));
            visit_stmt_blocks(&stmt.node, &mut |inner| block_calls(parsed, inner, found));
        }
    }
    fn comptimes(parsed: &Parsed, block: &Block, found: &mut Vec<String>) {
        for stmt in &block.stmts {
            if let Stmt::Comptime { value, .. } = &stmt.node {
                calls(parsed, value, found);
            }
            visit_stmt_blocks(&stmt.node, &mut |inner| comptimes(parsed, inner, found));
        }
    }

    let mut bodies: HashMap<String, &Block> = HashMap::new();
    let mut waiting: Vec<String> = Vec::new();
    let mut functions: Vec<(&Item, bool)> = Vec::new();
    for item in &parsed.program.items {
        match &item.node {
            Item::Comptime { value, .. } => calls(parsed, value, &mut waiting),
            Item::Impl { methods, .. } => {
                functions.extend(methods.iter().map(|method| (&method.node, false)));
            }
            other => functions.push((other, true)),
        }
    }
    for (item, free) in functions {
        let Item::Fn {
            name, config, body, ..
        } = item
        else {
            continue;
        };
        for option in config {
            calls(parsed, &option.default, &mut waiting);
        }
        comptimes(parsed, body, &mut waiting);
        if let Some(name) = name
            && free
        {
            bodies.insert(parsed.text(*name).to_string(), body);
        }
    }
    let mut built = HashSet::new();
    while let Some(name) = waiting.pop() {
        if let Some(body) = bodies.get(&name)
            && built.insert(name)
        {
            block_calls(parsed, body, &mut waiting);
        }
    }
    built
}

/// **`std`'s own handle** ([ADR-302](../../docs/specification/adr/adr-302.md)
/// D4): text a C library owns, which no `extern "C"` block declares because
/// `std` does.
const C_STRING: &str = "CStr";

/// A written type's own name, with the module it is reached through taken off
/// ([ADR-313](../../docs/specification/adr/adr-313.md) D3).
///
/// `foreign::CStr` is what a declaration writes now; what this file asks about
/// is the **type**, which is the last segment. The full name is what goes into
/// the emitted Rust, because the module is in `std`'s own prelude there.
fn base(name: &str) -> &str {
    crate::contracts::ty::base(name)
}

const SHARED: &str = "Shared";
/// Part I 6.3's lock, whose shape is decided per value
/// ([ADR-281](../../../docs/specification/adr/adr-281.md)).
const LOCKED: &str = "Locked";

/// The shared mutable type, which is **one name here and two hulls below**
/// ([ADR-281](../../../docs/specification/adr/adr-281.md) D1).
///
/// It is a name the checker carries and only this module expands, which is the
/// whole of D1: a name that resolved away early would put a spelling nobody wrote
/// into every message about it ([ADR-281](../../../docs/specification/adr/adr-281.md)
/// D9 named that as what one name buys).
const SHARED_MUT: &str = "SharedMut";

/// Whose the one error type in a function's set is
/// ([ADR-280](../../docs/specification/adr/adr-280.md) D9, D13).
///
/// The difference decides the **envelope**. A type this unit declares is one
/// the program `throw`s, so the failure has a site and travels in a
/// `Thrown[E]`. A type a **library** declares arrives from a call, with no
/// `throw` in this program to have a site — so it travels as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named<'a> {
    Own(&'a str),
    Library(&'a str),
}

/// What a generated error sum is called
/// ([ADR-280](../../docs/specification/adr/adr-280.md) D15).
///
/// A program never writes it: a `catch` matches on the **members'** variants
/// ([ADR-023](../../docs/specification/adr/adr-023.md) D4), so this name exists
/// only in the generated file. Spelled so that nothing a program declares
/// collides with it.
const SUM: &str = "__NikaiaThrows_";

/// The call that never comes back
/// ([Part III A.2](../../docs/specification/30-nikaia-tooling.md), Part I 1.3).
const PANIC: &str = "panic";

/// Where a write through the brackets holds its value while the read in it
/// finishes ([ADR-293](../../docs/specification/adr/adr-293.md) D2).
const STORED: &str = "__nikaia_stored";

/// The name an arm's value is bound to before it is put back in an `Ok`
/// ([ADR-292](../../docs/specification/adr/adr-292.md) D18).
///
/// **The binding is what keeps `rustc` quiet.** `Ok({ … })` around a block
/// holding one expression is `unused_braces` — a warning about a file nobody
/// wrote, which is [Part III C.1](../../docs/specification/30-nikaia-tooling.md)
/// one severity down. A block with a `let` in it is a block the lint has
/// nothing to say about.
const ARM_VALUE: &str = "__nikaia_value";

/// The name a failure is bound to where an arm takes it apart itself.
const ARM_FAILED: &str = "__nikaia_failed";

/// The name a **pausing** sequence is bound to for the length of its loop
/// ([ADR-172](../../docs/specification/adr/adr-172.md) D1).
///
/// A `while let` steps a value rather than consuming an expression, so the
/// sequence needs a name — and a generated one, for the reason every other
/// generated name here has: a program may already have bound `lines`.
const SEQUENCE: &str = "__nikaia_sequence";

/// The slot a function's result is filed under, as `contracts::sharing` keys it.
const SHARED_RESULT: &str = "<result>";

/// The pseudo-function a `<struct>.<field>` slot is filed under, the same.
const SHARED_FIELDS: &str = "<field>";

/// The slot a part of a variant is filed under, beside a struct's
/// `<struct>.<field>`: `E.Add#0` (0.0.244).
fn variant_slot(owner: &str, variant: &str, at: usize) -> String {
    format!("{owner}.{variant}#{at}")
}

/// What the program's own `main` is called in the emitted Rust.
///
/// `fn main` belongs to the runtime now
/// ([ADR-303](../../../docs/specification/adr/adr-303.md) D4): it starts the
/// I/O worker, calls this, and drains. The name is spelled so that no Nikaia
/// program plausibly collides with it, and a `rustc` diagnostic about the
/// program's body still lands on the `.nika` source because the body's spans
/// are unchanged (ADR-300).
const PROGRAM_MAIN: &str = "__nikaia_main";

/// The generated name of [ADR-300](../../../docs/specification/adr/adr-300.md)
/// D1's table. `__nikaia_` for the same reason `__nikaia_main` is: a name a
/// program could also have chosen would be a name this compiler took from it.
const ABORT_TABLE: &str = "__NIKAIA_SITES";

/// What the **last** statement of a block is, which is two questions and not
/// one.
///
/// Nikaia's blocks are expressions (Part I, 3.1), so a block's last statement
/// is usually its value. That much was a `bool`. What a `bool` could not say is
/// *whose* value it is, and exactly one rewrite in this file needs to know:
/// `return x` at the end of a function body is written as `x`, because the
/// value a function body ends in **is** what the function hands back.
///
/// That is true of a function body. It is not true of a `match` arm, a `catch`
/// handler, or a block used as a value - their last statement is the value of
/// the *expression around them*, and a `return` inside one leaves the function
/// past it. Writing such a `return` as a bare value is a different program:
/// where the function returns something it is rustc's `E0308` about a file
/// nobody wrote, and where it returns nothing **nothing complains at all** and
/// the function runs on past the point the source said to leave
/// ([`docs/history/nightly-cost.md`](../../../docs/history/nightly-cost.md) §3.3).
///
/// So the rewrite is allowed where this says [`Tail::Return`] and nowhere else.
/// Everywhere else a `return` stays a `return`, which is always a legal Rust
/// statement and always means what the source meant - ADR-296 D17's "the
/// lowering is syntactic and never guesses a meaning", applied to the one place
/// it had been guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tail {
    /// Not a value position. The last statement is a statement like any other,
    /// and keeps its semicolon.
    Statement,
    /// The last statement is the block's **value**, and that value is handed to
    /// the expression around the block - not to whoever called the function.
    Value,
    /// The last statement is the **function's** (or the lambda's) return value.
    /// Only here may a `return x` be written as `x`.
    Return,
}

impl Tail {
    /// Whether the last statement is the block's value at all - which is what
    /// decides the semicolon, and what ADR-292's grouping may not take.
    fn is_value(self) -> bool {
        !matches!(self, Tail::Statement)
    }

    /// What the statement at `i` of a block whose last index is `last` is.
    fn at(self, i: usize, last: usize) -> Self {
        if i == last { self } else { Tail::Statement }
    }
}

/// Which type a method belongs to, and which trait declared it.
///
/// `target` is what makes the ledger key — `Counter::record` rather than
/// `record` — and the key is what a `Shared` position looks its count up by
/// ([`crate::contracts::sharing`]).
///
/// `declared_by` is the trait an `impl` names, where it names one, and it
/// exists because [ADR-288](../../docs/specification/adr/adr-288.md) D25 makes
/// the **declaration** the wider claim: a trait method without `sync` may
/// pause, so its implementations are `async fn` whether or not their own
/// bodies do. A body that never pauses under such a declaration is correct and
/// still has to hand back a future.
#[derive(Clone, Copy)]
struct MethodOf<'a> {
    target: &'a str,
    declared_by: Option<&'a str>,
}

/// What the declaration a body belongs to says about that body.
///
/// These four travel together because the statements inside a body cannot see
/// any of them: the `Ok` a `throws` function's value is wrapped in, whether
/// there is a value at all, which of its parameters a call has to await, and
/// the ledger key the body is filed under. They are one argument rather than
/// four for the same reason `Argument` is one - a function that takes eight
/// loose values is a function whose callers are easy to get wrong.
#[derive(Clone, Copy)]
struct Declared<'a> {
    /// The function may fail, so its value is handed back as an `Ok`.
    throws: bool,
    /// The function hands back a value at all, so its last statement is that
    /// value rather than a statement like any other.
    returns_value: bool,
    /// The code parameters of this declaration whose type may pause
    /// ([ADR-277](../../docs/specification/adr/adr-277.md) D11): a call to one
    /// of these carries an `.await`.
    awaited: &'a [Symbol],
    /// The ledger key of the function these statements are in: `main`, or
    /// `Counter::record` for a method. Two things read it - a `Shared` value's
    /// count is filed under it (`contracts::sharing`), and a `throw` raised
    /// here reports the function's **own** name, which is the key's last
    /// segment (ADR-023 D6: a `throw` in `main` says `main`).
    key: &'a str,
    /// The type this function's failures travel in, exactly as the signature
    /// above wrote it ([ADR-292](../../docs/specification/adr/adr-292.md) D17).
    ///
    /// A vehicle that takes fallible branches - `overlap`, `select` - has to
    /// name an error type for the `async` blocks it writes, because one with a
    /// `?` in it and nothing to infer from is *type annotations needed* about a
    /// file nobody wrote. Naming the box was right while every channel was one;
    /// since [ADR-280](../../docs/specification/adr/adr-280.md) a channel can
    /// be a `Thrown<E>`, a library's type bare or a generated sum, and the `?`
    /// on the branch has to convert into whichever it is.
    channel: &'a str,
}

/// **The oldest Rust a Nikaia installation works with**
/// ([ADR-305](../../docs/specification/adr/adr-305.md)).
///
/// **Measured, not derived**: the whole test suite - the compiler, `std`, and
/// every program the tests emit and compile - passes on this version, and CI
/// runs it there. It is the highest of what each part needs: the emitted code
/// writes `impl AsyncFn` and `async` closures (1.85), `std` uses
/// `NonNull::map_addr` (1.84) and its tests `io::pipe` (1.87), and the
/// compiler's dependencies ask for 1.88.
///
/// Every generated `Cargo.toml` carries it, and the build compares
/// `rustc --version` against it before handing anything to `cargo` - because
/// Cargo's own *package requires rustc 1.88 or newer* is a message about a
/// generated file, which [Part III C.1](../../docs/specification/30-nikaia-tooling.md)
/// forbids.
///
/// **It is a different fact from the channel.** `rust-toolchain.toml` names
/// that and stays the only place that does
/// ([ADR-001](../../docs/specification/adr/adr-001.md) D1). It rises by a
/// record and a measurement, never silently: `scripts/check-floor.sh` reads it
/// from here and runs the suite on it.
pub const RUST_FLOOR: &str = "1.88";

/// What surrounds the statements being emitted.
#[derive(Debug, Clone, Copy)]
struct Flow<'a> {
    /// Kap 7.1: the enclosing function is `throws`, so a `return` carries `Ok`.
    throws: bool,
    /// The function a `throw` inside this flow is raised from.
    ///
    /// ADR-023 D6 wants a raise site keyed by what the source says rather than
    /// by where it sits, so that an edit above it does not move it. A name is
    /// the part of that key this compiler has; the ordinal within the function
    /// waits on the mark table.
    origin: &'a str,
    /// Kap 7.1: what is being emitted is the guarded half of a `catch`, so a
    /// failure in it is handled here rather than propagated.
    ///
    /// It reaches inward for the same reason `sequential` does: in
    /// `outer(inner()) catch { … }` the handler runs for either call, so
    /// neither propagates. The handler's own body is emitted with the flow the
    /// `catch` was written in, because a failure raised *there* leaves the
    /// function like any other.
    caught: bool,
    /// What is being emitted is a `catch`'s **handler**, and the error it was
    /// handed came through a **named** channel
    /// ([ADR-280](../../../docs/specification/adr/adr-280.md) D10).
    ///
    /// One statement in a handler reads it: `throw error`, which passes the
    /// error on with the site it was raised at rather than wrapping it again.
    /// It does **not** reach inward past a function boundary, for the reason
    /// the binding does not either: a lambda inside a handler has its own
    /// `error` or none.
    caught_named: bool,
    /// The **sum** the error a handler was handed travels in, where it is one
    /// ([ADR-280](../../../docs/specification/adr/adr-280.md) D17).
    ///
    /// `match error { … }` reads it: the patterns the source writes name
    /// variants of the sum's **members**, so the match has to be taken apart by
    /// member, and the sum's own name is what the outer arms are written with.
    caught_sum: Option<&'a str>,
    /// The **member** of that sum whose arm is being written, where one is
    /// ([ADR-280](../../../docs/specification/adr/adr-280.md) D17).
    ///
    /// `throw error` reads it: inside a member's arm the handler holds the
    /// member, so passing the error on is putting it back in the variant it
    /// came out of.
    caught_member: Option<&'a str>,
    /// What is being emitted is a **place** rather than a value
    /// ([ADR-293](../../../docs/specification/adr/adr-293.md) D4).
    ///
    /// The left of an assignment is the only one: `self.bodies[0].vx = …`
    /// writes **through** the index, so the brackets there are the language
    /// below's own and not a read. A read is what answers a `T?` on a map; a
    /// place has to stay a place or there is nothing to assign to.
    in_a_place: bool,
    /// The `match` being emitted is a **statement**: its arms hand nothing
    /// back, so a block arm's last statement keeps its semicolon, as an `if`
    /// in statement position does. Without it `ledger.sources.insert(…)` at
    /// the end of an arm was the arm's value (found moving the ledger's reader
    /// into Nikaia, ADR-294).
    arms_are_statements: bool,
    /// The byte the statement being emitted starts at.
    ///
    /// It is here because the checker's answer about method calls is keyed by
    /// it (`check::Checked::fallible_methods`), and an expression has no span
    /// of its own to look itself up by. `stmt` sets it for every statement it
    /// emits, so a statement nested in a block carries its own and not the
    /// outer one's - which is what the checker records, because it walks
    /// statements the same way.
    statement: usize,
    /// The key the ledger records the function being emitted under - `main`, or
    /// `Counter::record` for a method.
    ///
    /// It is here for the same reason `statement` is: the answer about a
    /// `Shared` value is keyed by the function it is written in and the name the
    /// source gives it (`contracts::sharing`), and a `let` has no way to look
    /// itself up without knowing which function it stands in. `origin` is the
    /// name a `throw` reports and is deliberately not this: an error raised in a
    /// method says the method's own name (ADR-023 D6), and a ledger key says
    /// `Type::method`.
    function: &'a str,
    /// This expression **is** the guarded half of a `catch`, rather than
    /// sitting somewhere inside it
    /// ([ADR-292](../../../docs/specification/adr/adr-292.md) D18).
    ///
    /// `caught` reaches inward, because in `outer(inner()) catch { … }` the
    /// handler runs for either call. This does not: it is set on the one
    /// expression the `match` is written around, and only where that
    /// expression is a block that joins. A joining block's outcome is a
    /// `Result` the handler consumes, and one nested in an argument is a tuple
    /// like any other value.
    handled_here: bool,
    /// The type the enclosing function's failures travel in
    /// ([ADR-292](../../../docs/specification/adr/adr-292.md) D17).
    ///
    /// Read by the two vehicles that wrap a fallible branch in an `Ok`:
    /// `overlap` and `select` write the error type out, and it has to be the
    /// one the `?` below them converts into. It reaches inward like `function`
    /// does and for the same reason - a nested block is still that function -
    /// and `Flow::PLAIN`'s box is what a context with no function around it
    /// gets.
    channel: &'a str,
    /// What is being emitted sits inside a **lambda's body**
    /// ([ADR-055](../../../docs/specification/adr/adr-055.md) §6).
    ///
    /// A lambda is a closure in the language below, and Rust has no stable
    /// `async` closure - so a call that pauses cannot be written inside one, and
    /// the lowering says so in Nikaia's words rather than handing `rustc` a file
    /// nobody wrote (Part III, C.1).
    ///
    /// **It is here and not in the checker on purpose.** A lambda that reads a
    /// file is a correct Nikaia program - `examples/fortunes.nika`'s route
    /// handler is one - and refusing a correct program is the one thing the
    /// compiler may never do (Part III, C.4). So the type check accepts it and
    /// the *lowering* is what cannot write it, which is a limit of this compiler
    /// and is where a program that is only checked never meets it.
    ///
    /// It reaches inward the way `sequential` and `caught` do: a call written in
    /// a block inside the lambda is still written inside the lambda. A function
    /// body starts a `Flow` of its own, which is the boundary it does not cross.
    in_lambda: bool,
    /// The lambda parameters written **`mut`** that are in scope here
    /// ([ADR-281](../../docs/specification/adr/adr-281.md) D12).
    ///
    /// `kasse.update fn(mut v) { v += 100 }` lowers to a closure whose
    /// parameter is `&mut T`, because D1 hands the block the **address** in the
    /// lock. So every mention of `v` in the body is written `(*v)`: `+=` and an
    /// assignment need the dereference outright, and a method call and a field
    /// read would get it from Rust anyway — writing it everywhere is one rule
    /// rather than a list of positions, and a parenthesis nobody needs is a
    /// warning about the generated file (Part III C.1), which is why the one
    /// here is always needed.
    ///
    /// **It accumulates rather than replacing**, so a lambda written inside an
    /// `update` block does not lose the dereference for a name it captures.
    changed: &'a [Symbol],
    /// The **code parameters whose type may pause**
    /// ([ADR-277](../../../docs/specification/adr/adr-277.md) D11) that are in
    /// scope here.
    ///
    /// Such a parameter lowers to a closure returning a boxed future, so a call
    /// to it is a call that hands back a future and carries an `.await`. The
    /// emitter can answer this one itself — the type is written in the
    /// declaration it is standing in, which is not the case for anything
    /// `pausing_key` looks up ([ADR-288](../../../docs/specification/adr/adr-288.md)).
    ///
    /// **It does not accumulate the way `changed` does.** A lambda written
    /// inside the body has parameters of its own, and a name it binds is not
    /// this function's parameter any more; the list is the declaration's and
    /// stops at every boundary that starts a `Flow::PLAIN`.
    awaited: &'a [Symbol],
    /// The name the statement being emitted **binds**, where it binds one.
    ///
    /// It is here for the reason `function` is: the count a `Shared` was given is
    /// keyed by the function and the name (`contracts::sharing`), and a hull
    /// written by a **call** ([ADR-281](../../../docs/specification/adr/adr-281.md)
    /// D2) has to look that up from inside an expression. An expression carries no
    /// span and no name of its own.
    ///
    /// Empty where the statement binds nothing, which is the floor's case and
    /// never a wrong answer - only a slower one.
    bound: &'a str,
    /// What is being emitted is **inside a constant this pass decided to write
    /// in the wider type** ([ADR-285](../../../docs/specification/adr/adr-285.md)
    /// D1), so every integer literal in it carries the `i64` suffix.
    ///
    /// It reaches inward, and it can only reach literals: an expression folds
    /// only if it is built from literals and arithmetic, so nothing else is
    /// down there to be reached. It is set on the **outermost** expression that
    /// folds, which is why the arms below ask for it before folding again - one
    /// decision per expression, not one per operator.
    widen: bool,
    /// What is being emitted stands where **Rust's own inference gives the
    /// number its type** - a sequence index and a repeat count, both `usize`.
    ///
    /// Those are the two positions a literal is deliberately left bare in
    /// (`index::at` and `count::of` are skipped there, because `at(0)` has
    /// nothing to infer from), and a suffix written into one would pin the type
    /// the position is supposed to decide. So `widen` does not start here, and
    /// what an index that overflows an `i32` gets is the message it gets today
    /// rather than a worse one.
    inferred: bool,
    /// What is being emitted sits inside the **body of a loop** - so a `break`
    /// or a `continue` here has somewhere to go (Part I, 3.3).
    ///
    /// **The backstop, and the checker is the diagnostic.** `NK1132` is what a
    /// program actually meets, and it is the one worth writing because it names
    /// which of a lambda, a task, an `overlap` branch or a fold's step stands in
    /// the way. This is the guarantee underneath it: a walk that missed a
    /// corner would otherwise put `break;` into a closure and let `rustc` answer
    /// about a file nobody wrote (Part III, C.1), and *"the checker's walk is
    /// complete"* is a thing to hope for rather than a thing that holds. Every
    /// statement is emitted through one place, so asking here cannot be evaded.
    ///
    /// It reaches inward the way `caught` and `in_lambda` do, and it stops at
    /// exactly the constructs that are a function below: each of them starts
    /// from [`Flow::PLAIN`], where this is false.
    in_loop: bool,
}

impl<'a> Flow<'a> {
    const PLAIN: Flow<'static> = Flow {
        changed: &[],
        awaited: &[],
        throws: false,
        origin: "",
        caught: false,
        caught_named: false,
        caught_sum: None,
        caught_member: None,
        in_a_place: false,
        arms_are_statements: false,
        statement: usize::MAX,
        function: "",
        channel: "Box<dyn std::error::Error>",
        handled_here: false,
        in_lambda: false,
        bound: "",
        widen: false,
        inferred: false,
        in_loop: false,
    };

    /// The same surroundings, for the body of a loop.
    fn inside_a_loop(self) -> Self {
        Flow {
            in_loop: true,
            ..self
        }
    }

    /// The same surroundings, for the expression a `catch` guards.
    fn guarded(self) -> Self {
        Flow {
            caught: true,
            ..self
        }
    }

    /// The guarded half of a `catch`, where that half is a block that **joins**
    /// ([ADR-292](../../../docs/specification/adr/adr-292.md) D18).
    ///
    /// The `match` is written around this expression, so its outcome has to be
    /// the `Result` the handler takes apart rather than the value a
    /// propagating block hands on.
    fn joined_here(self) -> Self {
        Flow {
            caught: true,
            handled_here: true,
            ..self
        }
    }

    /// The left of an assignment
    /// ([ADR-293](../../../docs/specification/adr/adr-293.md) D4).
    fn place(self) -> Self {
        Flow {
            in_a_place: true,
            ..self
        }
    }

    /// The surroundings a `catch`'s **handler** is emitted in
    /// ([ADR-280](../../../docs/specification/adr/adr-280.md) D10).
    ///
    /// The handler runs outside the guard — a failure raised *there* leaves
    /// the function like any other — so this is the flow the `catch` was
    /// written in, plus what the handler alone knows: whether the error it was
    /// handed came through a named channel.
    fn handling(self, named: bool) -> Self {
        Flow {
            caught_named: named,
            ..self
        }
    }

    /// The same, for a handler whose error travels in a **sum**
    /// ([ADR-280](../../../docs/specification/adr/adr-280.md) D17).
    ///
    /// A shorter lifetime than the flow it came from, and that is the point:
    /// the sum's name is the emitter's and outlives the handler being written,
    /// which is all this needs.
    fn catching<'b>(self, sum: Option<&'b str>) -> Flow<'b>
    where
        'a: 'b,
    {
        Flow {
            caught_sum: sum,
            ..self
        }
    }

    /// The surroundings inside one member's arm of a sum's match (D3).
    fn inside<'b>(self, member: &'b str, named: bool) -> Flow<'b>
    where
        'a: 'b,
    {
        Flow {
            caught_member: Some(member),
            caught_named: named,
            ..self
        }
    }

    /// The same surroundings, for the statement that starts at this byte.
    fn at(self, statement: usize) -> Self {
        Flow { statement, ..self }
    }

    /// The same surroundings, for a statement that binds this name.
    fn binding<'b>(self, bound: &'b str) -> Flow<'b>
    where
        Self: 'b,
    {
        Flow { bound, ..self }
    }

    /// The same surroundings, inside a constant written in the wider type.
    fn widened(self) -> Self {
        Flow {
            widen: true,
            ..self
        }
    }

    /// The same surroundings, where the position decides the number's type.
    fn inferred(self) -> Self {
        Flow {
            inferred: true,
            ..self
        }
    }
}

impl<'p> Emitter<'p> {
    fn new(parsed: &'p Parsed, build: Build, provenance: crate::contracts::Provenance) -> Self {
        Self::new_reading(parsed, build, provenance, &Reads::none())
    }

    /// The same, told what this build may read
    /// ([ADR-310](../../docs/specification/adr/adr-310.md)).
    fn new_reading(
        parsed: &'p Parsed,
        build: Build,
        provenance: crate::contracts::Provenance,
        reads: &Reads,
    ) -> Self {
        let own = crate::contracts::Ledger::infer(parsed);
        Self::with_contracts(
            parsed,
            &[],
            build,
            provenance,
            own,
            &crate::contracts::Ledger::blank(),
            reads,
        )
    }

    /// The same, against contracts that already exist - a program's rather than
    /// a file's.
    fn with_contracts(
        parsed: &'p Parsed,
        beside: &[&Parsed],
        build: Build,
        provenance: crate::contracts::Provenance,
        own_contracts: crate::contracts::Ledger,
        // **The described crates' boundary** (ADR-290 D20): whether a call into
        // one can fail, and what it keeps, which the propagation below asks.
        described: &crate::contracts::Ledger,
        reads: &Reads,
    ) -> Self {
        let mut grammars = HashMap::new();
        let mut structs = HashSet::new();
        let mut methods = HashMap::new();
        let mut by_name: HashMap<Symbol, Option<Method>> = HashMap::new();
        let mut uses_std = false;
        let mut fails = false;

        for item in &parsed.program.items {
            match &item.node {
                Item::Grammar(def) => {
                    grammars.insert(def.name, def);
                }
                Item::Struct { name, .. } => {
                    structs.insert(*name);
                }
                Item::Import { .. } => uses_std = true,
                Item::Fn {
                    can_throw: true, ..
                } => fails = true,
                Item::Impl {
                    trait_name: _,
                    target,
                    methods: body,
                } => {
                    for method in body {
                        let Item::Fn {
                            name: Some(name),
                            receiver,
                            args,
                            ret_type,
                            ..
                        } = &method.node
                        else {
                            continue;
                        };

                        let info = Method {
                            receiver: *receiver,
                            returns_value: ret_type.is_some(),
                            args: args.clone(),
                        };
                        methods.insert((target.name, *name), info.clone());
                        by_name
                            .entry(*name)
                            .and_modify(|known| {
                                // Two impls, one name: nothing here can tell
                                // them apart, so neither is used.
                                if known.is_some() {
                                    *known = None;
                                }
                            })
                            .or_insert(Some(info));
                    }
                }
                _ => {}
            }
        }

        // One type check, both of its answers about where a failure leaves.
        //
        // ADR-025 D7 for the loops: the type checker knows which `for` iterates
        // something whose step can fail, because it infers the iterator's type
        // and reads the ledger. Matching on the name `io::lines` here would
        // have caught the one-line form and quietly missed
        // `let s = io::lines()` followed by `for line in s`. ADR-288 for the
        // method calls: a receiver's type is the type checker's to know, and
        // there is one type checker (ADR-288).
        let propagation =
            crate::check::propagation_against(parsed, beside, &own_contracts, described, reads);
        // **Proved at every build, written without the check only where the
        // build says so** (ADR-306 D14): what is proved does not depend on
        // `remove-bounds-checks` or `remove-overflow-checks`.
        let proven = crate::bounds::proven(
            parsed,
            &propagation.std_lengths,
            &propagation.sized_lengths,
            &propagation.arithmetic,
        );

        // ADR-037 D7: which count each `Shared` value gets. Computed over the
        // whole unit, because a count belongs to an allocation and a handle's
        // class may reach into another function.
        let library = std_ledger();
        let shared = crate::contracts::sharing::analyse_program(
            parsed,
            &own_contracts,
            library,
            build.user_parallelism == UserParallelism::Yes,
        )
        .counts;

        // ADR-055 D6, before `own_contracts` is moved into place.
        let reach = pausing_reach(parsed, &own_contracts);

        let mut made = Self {
            parsed,
            build,
            described: described.clone(),
            borrowing: borrowing_structs(parsed),
            tethered: tethered_types(&own_contracts),
            // **An error another file of the package declares is the
            // package's own** (Part I 9.1, ADR-294): its `impl Error` is in
            // that file, and a function here that throws it travels in its
            // envelope, as one written beside the `impl` does.
            declared_errors: std::iter::once(parsed)
                .chain(beside.iter().copied())
                .flat_map(declared_errors)
                .collect(),
            carries_input: crate::views::carried(parsed, &own_contracts, library),
            fold_callees: fold_callees(parsed, &grammars),
            grammars,
            structs,
            methods,
            by_name,
            uses_std,
            is_std: false,
            fails,
            trusted_input: provenance == crate::contracts::Provenance::Trusted,
            fallible_loops: propagation.loops,
            pausing_loops: propagation.pausing_loops,
            owned_loops: propagation.owned_loops,
            copied_loop_bindings: propagation.copied_loop_bindings,
            copied_view_bindings: propagation.copied_view_bindings,
            unsigned_literals: propagation.unsigned_literals,
            copied_tuple_parts: propagation.copied_tuple_parts,
            number_lets: propagation.number_lets,
            changed_elements: propagation.changed_elements,
            count_args: propagation.count_args,
            map_keys: propagation.map_keys,
            slice_indices: propagation.slice_indices,
            proven_indices: propagation
                .list_indices
                .iter()
                .copied()
                .filter(|node| {
                    build.bounds == crate::bounds::BoundsChecks::Removed
                        && proven.indices.contains(node)
                })
                .collect(),
            proven_arithmetic: propagation
                .arithmetic
                .iter()
                .filter(|(at, _)| {
                    build.overflow == crate::bounds::OverflowChecks::Removed
                        && proven.arithmetic.contains(at)
                })
                .map(|(at, ty)| (*at, ty.clone()))
                .collect(),
            copied_slots: propagation.copied_slots,
            owned_copies: propagation.owned_copies,
            text_as_is: propagation.text_as_is,
            lent_coalesces: propagation.lent_coalesces,
            copied_jump_reads: propagation.copied_jump_reads,
            counted: propagation.counted,
            lent_bindings: propagation.lent_bindings,
            guards_inside_boxes: propagation.guards_inside_boxes,
            lent_scrutinees: propagation.lent_scrutinees,
            collected_into: propagation.collected_into,
            copied_walks: propagation.copied_walks,
            filter_patterns: propagation.filter_patterns,
            kept_lambdas: propagation.kept_lambdas,
            kept_hulls: propagation.kept_hulls,
            either_prelude: std::cell::RefCell::new(Vec::new()),
            field_calls: propagation.field_calls,
            kept_calls: propagation.kept_calls,
            kept_args: propagation.kept_args,
            kept_functions: propagation.kept_functions,
            in_a_kept_call: std::cell::RefCell::new(false),
            in_parameter: std::cell::RefCell::new(false),
            grammar_rule_types: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            pausing_walks: propagation.pausing_walks,
            fallible_methods: propagation.methods,
            pausing_methods: propagation.pausing_methods,
            witnessed_sets: propagation.witnessed_sets,
            future_lambdas: propagation.future_lambdas,
            run_lambdas: propagation.run_lambdas,
            settle_fns: propagation.settle_fns,
            settle_lets: propagation.settle_lets,
            cleaned_types: own_contracts
                .implementations
                .get("Cleanup")
                .cloned()
                .unwrap_or_default(),
            pausing_lambdas: propagation.pausing_lambdas,
            narrowing_casts: propagation.narrowing,
            shared,
            nullable_sites: propagation.nullable,
            concatenations: propagation.concatenations,
            lent_lets: propagation.lent_lets,
            lent_returns: propagation.lent_returns,
            compares: propagation.compares,
            compares_totally: propagation.compares_totally,
            copies: propagation.copies,
            viewed_numbers: propagation.viewed_numbers,
            array_literals: propagation.array_literals,
            owned_texts: propagation.owned_texts,
            keep_plans: crate::contracts::keep::plans(parsed, &own_contracts, library)
                .into_iter()
                .map(|plan| (plan.key.clone(), plan))
                .collect(),
            preluded: std::cell::RefCell::new(HashSet::new()),
            lent_list_reads: propagation.lent_list_reads,
            view_fallbacks: propagation.view_fallbacks,
            view_coalesces: propagation.view_coalesces,
            lent_map_fallbacks: propagation.lent_map_fallbacks,
            optional_fallbacks: propagation.optional_fallbacks,
            flat_reads: propagation.flat_reads,
            copied_reads: propagation.copied_reads,
            text_in_lists: propagation.text_in_lists,
            hole: std::cell::RefCell::new(None),
            wrappers: std::cell::RefCell::new(Vec::new()),
            holding: std::cell::RefCell::new(None),
            hold_args: std::cell::RefCell::new(None),
            keep_function: std::cell::RefCell::new(String::new()),
            nonnegative: std::cell::RefCell::new(HashMap::new()),
            comptime_values: propagation.comptime_values,
            built_at_build_time: built_at_build_time(parsed),
            with_types: propagation.with_types,
            unrolled: propagation.unrolled,
            walks_fields: propagation.walks_fields,
            unrolled_calls: propagation.unrolled_calls,
            specialising: std::cell::RefCell::new(None),
            at_field: std::cell::RefCell::new(None),
            code_parameter_runs: std::cell::RefCell::new(false),
            reached_copy: std::cell::Cell::new(false),
            flattened_reaches: propagation.flattened,
            copied_reaches: propagation.copied,
            viewed_reaches: propagation.viewed,
            lent_reaches: propagation.lent_reaches,
            held_reaches: propagation.held_reaches,
            held: std::cell::RefCell::new(HashMap::new()),
            nullable_fields: propagation.nullable_in_fields,
            lent_args: propagation.lent_args,
            lookup_keys: propagation.lookup_keys,
            boxed_members: propagation.boxed_members,
            boxed_reads: propagation.boxed_reads,
            copied_bindings: propagation.copied_bindings,
            some_tails: propagation.some_tails,
            lent_options: propagation.lent_options,
            wrapped: propagation.wrapped,
            compared_views: propagation.compared_views,
            claims: propagation.claims,
            entries: propagation.entries,
            reaches: propagation.reaches,
            published: propagation.published,
            boxed_args: std::cell::RefCell::new(None),
            variant_counts: std::cell::RefCell::new(None),
            hull_count: std::cell::Cell::new(None),
            mut_args: propagation.mut_args,
            copied_args: propagation.copied_args,
            nullable_args: propagation.nullable_in_args,
            task_handles: propagation.task_handles,
            method_options: propagation.method_options,
            pausing_reach: reach,
            own_contracts,
            library,
            dsl_drivers: crate::dsl::drivers(parsed).into_iter().collect(),
            foreign_params: foreign_params(parsed),
            opaque_handles: opaque_handles(parsed),
            foreign_results: foreign_results(parsed),
            entry: true,
            sums: std::collections::BTreeMap::new(),
        };
        // After the rest, because it reads three of the fields above.
        made.sums = made.error_sums();
        made
    }

    /// The same, said of a module rather than of the crate root.
    fn for_entry(mut self, entry: bool) -> Self {
        self.entry = entry;
        self
    }

    /// This is `std` itself - see [`emit_std`].
    fn for_std(mut self) -> Self {
        self.is_std = true;
        self
    }

    fn text(&self, sym: Symbol) -> &str {
        self.parsed.text(sym)
    }

    /// The same text, written so the language below can read it (`escaped`).
    ///
    /// Deliberately **not** folded into `text` above, although that would be one
    /// line instead of every position below. `text` is also what the recorded
    /// answers from the checker are keyed by — `pausing_methods`,
    /// `fallible_methods`, `nullable_fields` are all keyed by the name the
    /// *source* wrote — so escaping there would make every one of those lookups
    /// miss, silently, on exactly the programs this is for.
    fn name(&self, sym: Symbol) -> std::borrow::Cow<'_, str> {
        escaped(self.text(sym))
    }

    /// The name of a function whose body walks a type's fields, where this item
    /// is one ([ADR-304](../../docs/specification/adr/adr-304.md) D6).
    ///
    /// Read off the **checker's** answer rather than off the body, which is
    /// this emitter's rule everywhere: what a type is, and therefore which
    /// functions were instantiated with what, is the checker's
    /// ([ADR-288](../../docs/specification/adr/adr-288.md)).
    fn walks_a_shape(&self, item: &Item) -> Option<String> {
        let Item::Fn {
            name: Some(name), ..
        } = item
        else {
            return None;
        };
        let name = self.text(*name).to_string();
        // **Off `walks_fields` and not off `unrolled`**, because the two differ
        // where it matters: a function that walks a shape and is never called
        // has no copies, and the generic original still may not be written —
        // its body holds a loop over a shape. Part II 10.3's own block is that
        // shape, and it reached `rustc` until this line read the right map.
        self.walks_fields.contains_key(&name).then_some(name)
    }

    /// The concrete type the parameter stands for, while a copy is being
    /// written.
    fn standing_for(&self) -> Option<String> {
        self.specialising
            .borrow()
            .as_ref()
            .map(|(_, on)| on.clone())
    }

    /// The fields a `for` over `T::fields` walks, where this is one
    /// ([ADR-304](../../docs/specification/adr/adr-304.md) D6).
    ///
    /// Asked of the **iterated expression** rather than of the statement,
    /// because that is where the shape is named — and answered from the
    /// checker's table, which is the only thing that knows which copy this is.
    fn unrolls_here(&self, iter: &Expr) -> Option<Vec<crate::contracts::FieldContract>> {
        let (name, on) = self.specialising.borrow().clone()?;
        let Expr::Path(segments) = iter else {
            return None;
        };
        let names: Vec<String> = segments.iter().map(|s| self.text(*s).to_string()).collect();
        let [parameter, member] = names.as_slice() else {
            return None;
        };
        if *parameter != name || !matches!(member.as_str(), "fields" | "variants") {
            return None;
        }
        self.unrolled
            .get(&(self.enclosing_shape_walk()?, on))
            .cloned()
    }

    /// The name of the function this copy is of, while one is being written.
    ///
    /// The parameter's name is what `specialising` holds beside the type, so
    /// the function's own name is looked up the way the copy was chosen: there
    /// is exactly one entry per (function, type) and the type is in hand.
    fn enclosing_shape_walk(&self) -> Option<String> {
        let (parameter, on) = self.specialising.borrow().clone()?;
        let _ = parameter;
        self.unrolled
            .keys()
            .find(|(_, held)| *held == on)
            .map(|(written, _)| written.clone())
    }

    /// **The field this unrolled turn stands at**, where `base.member` is a
    /// read of the loop's own binding ([ADR-304](../../docs/specification/adr/adr-304.md)
    /// D2).
    ///
    /// `None` everywhere else, which is every program that does not walk a
    /// shape: the binding's name has to match the one the `for` wrote and the
    /// member has to be the one asked for, so an ordinary `x.name` on a struct
    /// called `field` is untouched.
    fn reflected(&self, base: &Expr, member: Symbol, wanted: &str) -> Option<String> {
        let (bound, field) = self.at_field.borrow().clone()?;
        let Expr::Variable(name) = base else {
            return None;
        };
        (self.text(*name) == bound && self.text(member) == wanted).then_some(field)
    }

    /// The type parameter a shape walk stands on — the `T` of
    /// `fn describe[T: Struct](value: T)`.
    ///
    /// The **first** one, which is what the checker bound: a second parameter
    /// beside it is an ordinary generic and keeps its place in the signature.
    fn walked_parameter(&self, item: &Item) -> String {
        let Item::Fn { generics, .. } = item else {
            return String::new();
        };
        generics
            .first()
            .map(|g| self.text(g.name).to_string())
            .unwrap_or_default()
    }

    /// A module's items and nothing else - no preamble, no `mod` header.
    fn items_only(&self) -> Result<Lowered> {
        let mut out = Out::default();
        self.shadow_types(&mut out);
        self.write_error_sums(&mut out);
        for item in &self.parsed.program.items {
            // **One copy per type it was used with, and no generic original**
            // ([ADR-304](../../docs/specification/adr/adr-304.md) D6): the
            // generic body holds a loop over a shape, which has no form in the
            // language below. It is **unrolled**, which is
            // [ADR-304](../../docs/specification/adr/adr-304.md) D5's *one loop*
            // arrived at rather than added - `T::fields` is known while the
            // program is built, so there is no run-time reading to rule out.
            if let Some(name) = self.walks_a_shape(&item.node) {
                let copies: Vec<String> = self
                    .unrolled
                    .keys()
                    .filter(|(written, _)| *written == name)
                    .map(|(_, on)| on.clone())
                    .collect();
                for on in copies {
                    *self.specialising.borrow_mut() = Some((self.walked_parameter(&item.node), on));
                    let written =
                        out.from(&item.span, |out| self.item(out, &item.node, &item.span));
                    *self.specialising.borrow_mut() = None;
                    written?;
                    out.push("\n");
                }
                continue;
            }
            // **One copy per type it was used with, and no generic original**
            // ([ADR-304](../../docs/specification/adr/adr-304.md) D6): the
            // generic body holds a loop over a shape, which has no form in the
            // language below. It is **unrolled**, which is
            // [ADR-304](../../docs/specification/adr/adr-304.md) D5's *one
            // loop* arrived at rather than added — `T::fields` is known while
            // the program is built, so there is no run-time reading to rule
            // out.
            if let Some(name) = self.walks_a_shape(&item.node) {
                let copies: Vec<String> = self
                    .unrolled
                    .keys()
                    .filter(|(written, _)| *written == name)
                    .map(|(_, on)| on.clone())
                    .collect();
                for on in copies {
                    *self.specialising.borrow_mut() = Some((self.walked_parameter(&item.node), on));
                    let written =
                        out.from(&item.span, |out| self.item(out, &item.node, &item.span));
                    *self.specialising.borrow_mut() = None;
                    written?;
                    out.push("\n");
                }
                continue;
            }
            out.from(&item.span, |out| self.item(out, &item.node, &item.span))?;
            out.push("\n");
        }
        self.tether_wrappers(&mut out);
        self.entry_point(&mut out);
        Ok(Lowered {
            rust: out.buf,
            map: out.map,
            published: self.published.clone(),
        })
    }

    fn program(&self) -> Result<Lowered> {
        let mut out = Out::default();
        out.push("// Generated by the Nikaia bootstrap compiler (Stage 0).\n");
        out.push("// Edit the .nika source, not this file.\n\n");

        if self
            .parsed
            .program
            .items
            .iter()
            .any(|i| matches!(i.node, Item::Grammar(_)))
        {
            out.push("use winnow_grammar::grammar;\n");
        }
        if self.uses_driver() {
            out.push("use winnow_grammar::rt::Parallelism;\n");
            out.push("use winnow_grammar::ParseContext;\n");
        }
        // Always, except in `std` itself, which is the prelude - see
        // [`emit_std`] and [`Needs::preamble`], the copy of this that a program
        // of several files uses.
        if !self.is_std {
            out.push("#[allow(unused_imports)]\npub use nikaia_std::prelude::*;\n");
        }
        if self.fails {
            // Kap 7.1: `error.full()` is a method on whatever a `catch` bound, so
            // the trait has to be in scope even in a program that imports no `std`
            // module of its own - `throw` needs `std` whether or not the source
            // mentions it.
            out.push("#[allow(unused_imports)]\npub use nikaia_std::error::Full;\n");
        }
        out.push("\n");

        self.shadow_types(&mut out);
        self.write_error_sums(&mut out);

        for item in &self.parsed.program.items {
            // **One copy per type it was used with, and no generic original**
            // ([ADR-304](../../docs/specification/adr/adr-304.md) D6): the
            // generic body holds a loop over a shape, which has no form in the
            // language below. It is **unrolled**, which is
            // [ADR-304](../../docs/specification/adr/adr-304.md) D5's *one loop*
            // arrived at rather than added - `T::fields` is known while the
            // program is built, so there is no run-time reading to rule out.
            if let Some(name) = self.walks_a_shape(&item.node) {
                let copies: Vec<String> = self
                    .unrolled
                    .keys()
                    .filter(|(written, _)| *written == name)
                    .map(|(_, on)| on.clone())
                    .collect();
                for on in copies {
                    *self.specialising.borrow_mut() = Some((self.walked_parameter(&item.node), on));
                    let written =
                        out.from(&item.span, |out| self.item(out, &item.node, &item.span));
                    *self.specialising.borrow_mut() = None;
                    written?;
                    out.push("\n");
                }
                continue;
            }
            // **One copy per type it was used with, and no generic original**
            // ([ADR-304](../../docs/specification/adr/adr-304.md) D6): the
            // generic body holds a loop over a shape, which has no form in the
            // language below. It is **unrolled**, which is
            // [ADR-304](../../docs/specification/adr/adr-304.md) D5's *one
            // loop* arrived at rather than added — `T::fields` is known while
            // the program is built, so there is no run-time reading to rule
            // out.
            if let Some(name) = self.walks_a_shape(&item.node) {
                let copies: Vec<String> = self
                    .unrolled
                    .keys()
                    .filter(|(written, _)| *written == name)
                    .map(|(_, on)| on.clone())
                    .collect();
                for on in copies {
                    *self.specialising.borrow_mut() = Some((self.walked_parameter(&item.node), on));
                    let written =
                        out.from(&item.span, |out| self.item(out, &item.node, &item.span));
                    *self.specialising.borrow_mut() = None;
                    written?;
                    out.push("\n");
                }
                continue;
            }
            out.from(&item.span, |out| self.item(out, &item.node, &item.span))?;
            out.push("\n");
        }
        self.tether_wrappers(&mut out);
        self.entry_point(&mut out);
        if self.user_main().is_some() {
            // **An empty table, and the reason it is empty.** `fn main` installs
            // the hook that reads it (ADR-300 D10), so the name has to be defined
            // or the program does not compile - and a table needs the *file* each
            // line came from, which this path does not know: it is handed one
            // `Parsed` and no path. The path a user takes goes through
            // `modules::Program`, which knows every file and appends the real
            // table (`abort_table`); this one is `emit_program`, which the tests
            // and `lower-std` use.
            //
            // Empty is the fallback rather than a lie: a location the table does
            // not know is handed to the hook installed before ours, which is
            // Rust's own, so such a program is exactly as well off as it was.
            out.push(&format!(
                "\n// ADR-300 D9: no table - this program was emitted without its file name.\nconst {ABORT_TABLE}: &[nikaia_std::abort::Site] = &[];\n"
            ));
        }

        Ok(Lowered {
            rust: out.buf,
            map: out.map,
            published: self.published.clone(),
        })
    }

    /// The program's own `main`, if this file carries one the runtime can wrap.
    ///
    /// `Some(throws)` for the plain shape - `fn main()` or `fn main() throws`,
    /// no receiver, no parameters, no returned value - and `None` for anything
    /// else, including a `main` in a module rather than at the crate root. A
    /// `main` this does not recognise is emitted exactly as written and keeps
    /// its name, because a wrapper that guessed at a signature would be worse
    /// than no wrapper at all.
    fn user_main(&self) -> Option<bool> {
        if !self.entry {
            return None;
        }
        self.parsed
            .program
            .items
            .iter()
            .find_map(|item| match &item.node {
                Item::Fn {
                    name: Some(name),
                    receiver: None,
                    args,
                    config,
                    spread: None,
                    ret_type: None,
                    can_throw: throws,
                    ..
                } if self.text(*name) == MAIN && args.is_empty() && config.is_empty() => {
                    Some(*throws)
                }
                _ => None,
            })
    }

    /// The `fn main` Rust runs: ADR-303 D4, as five lines of generated code.
    ///
    /// The runtime is started *before* the program's first statement and
    /// drained after its last, so an operation inside the program costs no
    /// thread start and no thread wake-up - which is
    /// [ADR-292](../../../docs/specification/adr/adr-292.md)'s finding
    /// read the other way round.
    ///
    /// What starts is what
    /// [ADR-037](../../../docs/specification/adr/adr-037.md) D2 allows, and
    /// the *compiler* is what knows which: `user_parallelism` is a build
    /// switch, so it is written into this call rather than read from the
    /// operator's runtime configuration file (ADR-303 D5 has four settings and
    /// this is not one of them).
    fn entry_point(&self, out: &mut Out) {
        let Some(throws) = self.user_main() else {
            return;
        };
        let user_code = match self.build.user_parallelism {
            UserParallelism::No => "Sequential",
            UserParallelism::Yes => "Concurrent",
        };
        // **The runtime's `main` declares the program's own channel**
        // ([ADR-280](../../docs/specification/adr/adr-280.md) D9): it hands
        // `__nikaia_main`'s outcome straight back, so the two have to agree.
        // `Result<(), E>` is a `Termination` for any `E: Debug`, which every
        // envelope is.
        //
        // `main` has no parameters, so there is nothing to borrow from and an
        // error carrying a view can only be `'static` — D9's derivation, one
        // position over.
        let channel = self.error_channel(MAIN, Lifetimes::ELIDED, false);
        let ret = if throws {
            format!(" -> Result<(), {channel}>")
        } else {
            String::new()
        };

        for line in [
            "",
            "// ADR-303 D4: the runtime is running before the program's first",
            "// statement, so an operation inside it costs no thread wake-up. What",
            "// starts is what `user_parallelism` allows (ADR-037 D2): the I/O",
            "// worker always, a pool for user code only at `yes`.",
        ] {
            out.push(line);
            out.push("\n");
        }
        out.push(&format!("fn main(){ret} {{\n"));
        // **Before anything else** (ADR-300 D10): the hook has to be in place
        // before a line that can abort runs, and the first of those is inside
        // the runtime's own start. The table is defined at the end of this file
        // (`abort_table`), which Rust allows and which is what keeps the line
        // numbers in it true - a table written above the program would move
        // every line it names.
        out.push(&format!(
            "    nikaia_std::abort::report_in_nikaia_terms({ABORT_TABLE});\n"
        ));
        // **A `main` that cannot pause starts the runtime on demand**
        // ([ADR-270](../../docs/specification/adr/adr-270.md) D8 step 1): it
        // has no operation that needs the I/O workers, and starting them put a
        // second thread in the process, which makes every allocation take the
        // allocator's locks. `rt::handle` still starts it if something asks.
        let start = match (self.pauses(MAIN), self.build.user_parallelism) {
            (false, UserParallelism::No) => "on_demand",
            _ => "start",
        };
        out.push(&format!(
            "    let nikaia_runtime = nikaia_std::rt::{start}(nikaia_std::rt::UserCode::{user_code});\n"
        ));
        // **ADR-055 D3: `main` is what the executor drives.** Where the
        // program's own entry can pause it is an `async fn` (D1), so the one
        // place a future is driven from the outside is here - `block_on` is the
        // boundary between a program that can pause and an operating system
        // that cannot.
        //
        // A `main` the ledger says is `sync` is a plain call, and writing
        // `block_on` around it would be driving something that is not a future.
        match self.pauses(MAIN) {
            true => out.push(&format!(
                "    let outcome = nikaia_std::rt::exec::block_on({PROGRAM_MAIN}());\n"
            )),
            false => out.push(&format!("    let outcome = {PROGRAM_MAIN}();\n")),
        }
        out.push("    nikaia_runtime.finish();\n");
        out.push("    outcome\n");
        out.push("}\n");
    }

    /// Whether any grammar entry in the program reaches a **parallel** rule,
    /// which is what decides whether the piece driver's names are in the
    /// preamble.
    ///
    /// The entry is a call now ([ADR-296](../../docs/specification/adr/adr-296.md)
    /// D1), so the rule is the one the program **named** rather than the one
    /// this file used to pick. Found by being wrong: the lowering was right and
    /// the preamble was not, and `rustc` answered *"use of undeclared type
    /// `ParseContext`"* about the generated file.
    fn uses_driver(&self) -> bool {
        let mut found = false;
        let mut parallel = |grammar: &Symbol, entry: &Symbol| {
            if let Some(def) = self.grammars.get(grammar) {
                let rule = def.rules.iter().find(|r| r.is_entry && r.name == *entry);
                if rule.map(|r| par_fold_of(r).is_some()).unwrap_or(false) {
                    found = true;
                }
            }
        };
        for item in &self.parsed.program.items {
            if let Item::Fn { body, .. } = &item.node {
                visit_block(body, &mut |e| {
                    // **A grammar is entered through a path**
                    // ([ADR-140](../../docs/specification/adr/adr-140.md) D3),
                    // so this reads the shape the entry has rather than the one
                    // it used to have. Missing it emitted a `parse_…_pieces`
                    // call with `ParseContext` and `Parallelism` undeclared —
                    // `rustc` about a file nobody wrote (Part III C.1).
                    if let Expr::Call { func, .. } = e
                        && let Expr::Path(segments) = func.as_ref()
                        && let [grammar, rule] = segments.as_slice()
                    {
                        parallel(grammar, rule);
                    }
                });
            }
        }
        found
    }

    /// **What a declared type derives** ([ADR-204](../../../docs/specification/adr/adr-204.md)
    /// D1).
    ///
    /// `Debug` and `Clone` always: every emitted type is printable and copyable,
    /// which is what the rest of this file already assumes of one.
    ///
    /// `PartialEq` where the checker says every part of it compares, and `Eq`
    /// beside it where no part is a float. Without them `==` had no lowering at
    /// all: `rustc` answered *binary operation `==` cannot be applied to type
    /// `P`* about a file nobody wrote, with *consider annotating `P` with
    /// `#[derive(PartialEq)]`* as the help — [Part III C.1 and
    /// C.2](../../../docs/specification/30-nikaia-tooling.md) at once.
    ///
    /// **Never both without the first**, which is Rust's own rule: `Eq` is an
    /// `impl` over `PartialEq`, and `compares_totally` is a subset by
    /// construction.
    fn derives(&self, name: winnow_grammar::Symbol) -> String {
        let name = self.text(name);
        let mut parts = vec!["Debug", "Clone"];
        // **`Copy` where every part is one**
        // ([ADR-294](../../../docs/specification/adr/adr-294.md) D9.1): the
        // Rust below may copy it, which a Rust reader of a type Nikaia declares
        // needs, and nothing this language says about the value changes.
        if self.copies.contains(name) {
            parts.push("Copy");
        }
        if self.compares.contains(name) {
            parts.push("PartialEq");
            if self.compares_totally.contains(name) {
                parts.push("Eq");
            }
        }
        format!("#[derive({})]\n", parts.join(", "))
    }

    fn item(&self, out: &mut Out, item: &Item, span: &Span) -> Result<()> {
        match item {
            Item::Grammar(def) => self.grammar(out, def),
            Item::Enum {
                name,
                variants,
                is_public,
            } => {
                out.push(&self.derives(*name));
                let vis = if *is_public { "pub " } else { "" };
                let params = if self.borrowing.contains(name) {
                    format!("<{INPUT_LIFETIME}>")
                } else {
                    String::new()
                };
                out.push(&format!("{vis}enum {}{params} {{\n", self.name(*name)));
                let enum_name = self.text(*name);
                for variant in variants {
                    let name = self.name(variant.name);
                    match &variant.fields {
                        VariantFields::Unit => out.push(&format!("    {name},\n")),
                        VariantFields::Tuple(types) => {
                            let owner = self.text(variant.name);
                            let parts: Vec<String> = types
                                .iter()
                                .enumerate()
                                .map(|(at, t)| {
                                    let member = crate::check::boxed_member(owner, Some(at), None);
                                    let count = self.count_at(
                                        SHARED_FIELDS,
                                        &variant_slot(enum_name, owner, at),
                                    );
                                    self.boxed_ty(
                                        enum_name,
                                        &member,
                                        self.ty_counted(t, Lifetimes::NAMED, count),
                                    )
                                })
                                .collect();
                            out.push(&format!("    {name}({}),\n", parts.join(", ")));
                        }
                        VariantFields::Named(fields) => {
                            let owner = self.text(variant.name);
                            let parts: Vec<String> = fields
                                .iter()
                                .map(|f| {
                                    let member = crate::check::boxed_member(
                                        owner,
                                        None,
                                        Some(self.text(f.name)),
                                    );
                                    format!(
                                        "{}: {}",
                                        self.name(f.name),
                                        self.boxed_ty(
                                            enum_name,
                                            &member,
                                            self.ty(&f.ty, Lifetimes::NAMED)
                                        )
                                    )
                                })
                                .collect();
                            out.push(&format!("    {name} {{ {} }},\n", parts.join(", ")));
                        }
                    }
                }
                out.push("}\n");
                Ok(())
            }
            Item::Struct {
                name,
                generics,
                fields,
                is_public,
                ..
            } => {
                out.push(&self.derives(*name));
                let vis = if *is_public { "pub " } else { "" };
                // The input lifetime first and the type parameters after it,
                // which is the order Rust wants them in (ADR-295 D3).
                let mut parts: Vec<String> = Vec::new();
                if self.borrowing.contains(name) {
                    parts.push(INPUT_LIFETIME.to_string());
                }
                parts.extend(generics.iter().map(|g| self.bounded(g)));
                let params = angled(&parts);
                out.push(&format!("{vis}struct {}{params} {{\n", self.name(*name)));
                for field in fields {
                    // Public, because the actions that build this struct are
                    // generated into the grammar's own module.
                    let slot = format!("{}.{}", self.text(*name), self.text(field.name));
                    let lowered = self.ty_counted(
                        &field.ty,
                        Lifetimes::NAMED,
                        self.count_at(SHARED_FIELDS, &slot),
                    );
                    out.push(&format!(
                        "    {}{}: {},\n",
                        if field.is_public { "pub " } else { "" },
                        self.name(field.name),
                        self.boxed_ty(self.text(*name), self.text(field.name), lowered)
                    ));
                }
                out.push("}\n");
                Ok(())
            }
            Item::Fn { name, .. } => {
                if name.is_some_and(|name| self.built_at_build_time.contains(self.text(name))) {
                    out.push("#[allow(dead_code)]\n");
                }
                self.function(
                    out,
                    item,
                    0,
                    Lifetimes::ELIDED,
                    self.carries_input.get(&span.at()),
                    None,
                )
            }
            Item::Impl {
                trait_name,
                target,
                methods,
            } => {
                // A type that holds a view carries the input lifetime, and the
                // impl has to declare the lifetime its methods are written with.
                let borrows = self.borrows(target.name);
                let params = if borrows {
                    format!("<{INPUT_LIFETIME}>")
                } else {
                    String::new()
                };
                let lifetimes = if borrows {
                    Lifetimes::INNER
                } else {
                    Lifetimes::ELIDED
                };
                let target_name = self.text(target.name).to_string();

                // Kap 7.1: `Error` is the one trait the compiler reads rather
                // than relays. An error travels in the failure channel, which
                // the language below spells `Box<dyn std::error::Error>`, and a
                // type gets in there by being `Display` plus `Error` - neither
                // of which the `.nika` source mentions, because neither is a
                // decision the author makes. `message` is what they wrote.
                let is_error_impl = trait_name.is_some_and(|t| self.text(t) == "Error");
                if is_error_impl {
                    return self.error_impl(out, &target_name, &params, methods, lifetimes);
                }

                // `impl Stack[T]` declares `T` and writes `Stack<T>`; the
                // lifetime, where the type carries one, stands in front of it
                // (ADR-295 D4). The target's arguments are written as the
                // source wrote them, so `impl Stack[i64]` stays `Stack<i64>`
                // and declares nothing.
                let declared = crate::contracts::declared_types(self.parsed);
                let generics = crate::contracts::impl_parameters(self.parsed, target, &declared);
                let mut head_parts: Vec<String> = Vec::new();
                if borrows {
                    head_parts.push(INPUT_LIFETIME.to_string());
                }
                head_parts.extend(generics.iter().cloned());
                let declares = angled(&head_parts);
                let mut target_parts: Vec<String> = Vec::new();
                if borrows {
                    target_parts.push(INPUT_LIFETIME.to_string());
                }
                target_parts.extend(
                    target
                        .generics
                        .iter()
                        .map(|g| self.ty(g, Lifetimes::ELIDED)),
                );
                let applied = angled(&target_parts);
                // **`impl Cleanup for T` is the type's own `cleanup` and a
                // one-line bridge to std's trait** (ADR-297 D1): the method
                // is written as the author wrote it, paused and failing as the
                // ledger says, and `Cleaned<T>` reaches it through the bridge
                // when the value dies.
                let is_cleanup = trait_name.is_some_and(|t| self.text(t) == "Cleanup");
                if is_cleanup {
                    out.push(&format!("impl{declares} {target_name}{applied} {{\n"));
                    for method in methods {
                        let carries = self.carries_input.get(&method.span.at());
                        out.from(&method.span, |out| {
                            out.push("    ");
                            self.function(
                                out,
                                &method.node,
                                1,
                                lifetimes,
                                carries,
                                Some(MethodOf {
                                    target: &target_name,
                                    declared_by: None,
                                }),
                            )
                        })?;
                    }
                    out.push("}\n");
                    let key = format!("{target_name}::cleanup");
                    let awaited = if self.pauses(&key) { ".await" } else { "" };
                    let (bridge, send) = match self.build.overlaps_user_code() {
                        true => ("CleanupSend", " + Send"),
                        false => ("Cleanup", ""),
                    };
                    out.push(&format!(
                        "impl{declares} nikaia_std::cleanup::{bridge} for {target_name}{applied} {{\n\
                         \x20   fn cleanup(&mut self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>{send} + '_>> {{\n\
                         \x20       Box::pin(async move {{ nikaia_std::cleanup::Outcome::reported({target_name}::cleanup(self){awaited}) }})\n\
                         \x20   }}\n\
                         }}\n"
                    ));
                    return Ok(());
                }
                let head = match trait_name {
                    Some(t) => format!(
                        "impl{declares} {} for {target_name}{applied}",
                        self.text(*t)
                    ),
                    None => format!("impl{declares} {target_name}{applied}"),
                };
                out.push(&format!("{head} {{\n"));
                for method in methods {
                    let carries = self.carries_input.get(&method.span.at());
                    out.from(&method.span, |out| {
                        out.push("    ");
                        self.function(
                            out,
                            &method.node,
                            1,
                            lifetimes,
                            carries,
                            Some(MethodOf {
                                target: &target_name,
                                declared_by: trait_name.map(|t| self.text(t)),
                            }),
                        )
                    })?;
                }
                out.push("}\n");
                Ok(())
            }
            // Kap 4.7: `trait Summarize { … }` becomes the same trait below, and
            // the method is a signature with a `;` where an `impl`'s has a body
            // ([ADR-295](../../docs/specification/adr/adr-295.md) D6).
            //
            // **No method here is `async`.** The ledger's `sync` column decides
            // that for a function (ADR-055 D1), and a declaration has no body
            // for `sync::infer` to read - so ADR-295 D9 asserts `sync` for a
            // trait's methods, which is the only answer this position can
            // write: `async fn` in a trait is something the emitter has no way
            // to ask for. What that costs is a trait whose method genuinely
            // pauses, which issue #253 carries with its reproduction.
            Item::Trait {
                name,
                methods,
                is_public,
            } => {
                let vis = if *is_public { "pub " } else { "" };
                out.push(&format!("{vis}trait {} {{\n", self.name(*name)));
                for method in methods {
                    out.from(&method.span, |out| {
                        out.push("    ");
                        self.trait_method(out, &method.node, method.node.is_sync, false)
                    })?;
                }
                out.push("}\n");
                Ok(())
            }
            Item::Import { path, alias } => {
                // The names come in through `nikaia_std::prelude`, emitted once
                // in the preamble; the import itself is kept as a comment so the
                // generated file still says where they were asked for.
                //
                // An alias is **resolved away** rather than emitted as a Rust
                // `use … as …` (ADR-286 D12): it is one file's name for a package
                // and every file's items are in one crate, so a Rust alias at the
                // root would be the whole program's. `Parsed::unaliased` is where
                // the resolving happens, on the way to every name.
                let path = path
                    .iter()
                    .map(|s| self.text(*s))
                    .collect::<Vec<_>>()
                    .join("::");
                // **Inside `std` there is no prelude to bring it in**
                // (0.0.248): a module of `std`'s Nikaia half is lowered without
                // one (`emit_std`), so `use std::text` has to be the import
                // itself, or `text::parse_i64` below it names nothing and
                // `rustc` says so about the generated file.
                if self.is_std
                    && alias.is_none()
                    && let [first, module] = path.split("::").collect::<Vec<_>>()[..]
                    && first == "std"
                {
                    out.push(&format!(
                        "#[allow(unused_imports)]\nuse nikaia_std::{module};\n"
                    ));
                    return Ok(());
                }
                match alias {
                    Some(alias) => out.push(&format!("// use {path} as {}\n", self.text(*alias))),
                    None => out.push(&format!("// use {path}\n")),
                }
                Ok(())
            }
            // **The same `const` the body form writes, one level out**
            // ([ADR-287](../../docs/specification/adr/adr-287.md)). The
            // spelling is the checker's, handed over keyed by the item's own
            // byte offset the way a statement's is - this emitter knows no
            // types ([ADR-296](../../docs/specification/adr/adr-296.md) D17), so
            // what `1000` is called below is not a question it can answer.
            //
            // `pub` is Part I 9.2's rule for Constants and reaches Rust as
            // Rust's own ([ADR-286](../../docs/specification/adr/adr-286.md)
            // D2): a package's crate is what another package reads.
            Item::Comptime { name, public, .. } => {
                let bound = self.name(*name);
                let Some((below, written)) = self.comptime_values.get(&span.at()).cloned() else {
                    return Err(refused_at!(
                        span.at(),
                        "`{bound}` has no value computed at build time. This is a compiler \
                         bug: the check should have reported it first."
                    ));
                };
                let vis = match public {
                    true => "pub ",
                    false => "",
                };
                out.push(&format!("{vis}const {bound}: {below} = {written};\n"));
                Ok(())
            }
            // **Rust's own `extern` block**
            // ([ADR-302](../../docs/specification/adr/adr-302.md) D1), which is
            // the whole of the lowering: the form means the same thing on both
            // sides, and a declaration is `trait_method`'s shape with `sync`
            // said by the caller (D2).
            //
            // **Only `"C"`**, and refusing a second ABI is a check rather than a
            // shape: the grammar takes any string so that the message about an
            // unknown one is this compiler's rather than the backend's about a
            // file nobody wrote (Part III C.1).
            Item::Extern {
                abi,
                declarations,
                opaque,
            } => {
                if abi != "C" {
                    return Err(refused_at!(
                        span.at(),
                        "Nikaia doesn't support `extern \"{abi}\"`. Use `extern \"C\"`."
                    ));
                }
                // **A handle is a type of its own, outside the block**
                // ([ADR-302](../../docs/specification/adr/adr-302.md) D7). It
                // is written before the declarations, because they name it.
                for handle in opaque {
                    out.from(&handle.span, |out| {
                        self.opaque_type(out, &handle.node);
                        Ok(())
                    })?;
                }
                out.push(&format!("unsafe extern \"{abi}\" {{\n"));
                for declaration in declarations {
                    out.from(&declaration.span, |out| {
                        out.push("    ");
                        self.trait_method(out, &declaration.node, true, true)
                    })?;
                }
                out.push("}\n");
                Ok(())
            }
            other => Err(refused_at!(
                span.at(),
                "The compiler can't translate this item yet: {other:?}"
            )),
        }
    }

    /// **An opaque handle, and the `cleanup` that ends its life**
    /// ([ADR-302](../../docs/specification/adr/adr-302.md) D7).
    ///
    /// ```ignore
    /// #[repr(transparent)]
    /// struct sqlite3(*mut core::ffi::c_void);
    /// impl Drop for sqlite3 { fn drop(&mut self) { unsafe { sqlite3_close(…) } } }
    /// ```
    ///
    /// **`repr(transparent)` and not a plain newtype**, because the whole point
    /// of the type is its layout: a handle *is* the address, so `&mut T` at the
    /// boundary is `T**` the way C writes it, and a handle passed by value is
    /// the pointer. A newtype Rust may lay out as it likes would be a different
    /// program at the boundary.
    ///
    /// **`Drop` and not a call the author writes**, which is Part I 6.4's
    /// `cleanup` read at the C boundary: the release runs at the end of the
    /// handle's scope, so a handle cannot be forgotten. What it cannot answer
    /// for is a C function that keeps the address past its own call, which is
    /// the one thing this language cannot check and D3 says so.
    ///
    /// **The release takes a handle and is handed one**, which is what keeps
    /// this from recursing: a value moved into an `extern "C"` function is the
    /// callee's, and C runs no `Drop`. Measured rather than reasoned — the
    /// release fires once per scope and not twice.
    ///
    /// **The address inside is `NonNull`**, which is
    /// [ADR-302](../../docs/specification/adr/adr-302.md) D11: a handle holds an
    /// address and `T?` is the absence of one, and those are the two states C
    /// spells with a pointer and `NULL`. Rust lays `Option<T>` over the same
    /// word for a type shaped like this, so a nullable handle costs nothing and
    /// `&mut T?` is `T **` exactly as C writes it — the right words around the
    /// right memory, which at this boundary is the only kind that counts.
    fn opaque_type(&self, out: &mut Out, handle: &crate::ast::OpaqueType) {
        let name = self.name(handle.name);
        let release = self.name(handle.released_by);
        out.push(&format!(
            "#[repr(transparent)]\n\
             #[derive(Debug)]\n\
             // A C type keeps the name its header gives it, which is not the\n\
             // shape Rust's own lint expects (Part III, C.1: what `rustc` would\n\
             // say here is about a file nobody wrote).\n\
             #[allow(non_camel_case_types)]\n\
             pub struct {name}(core::ptr::NonNull<core::ffi::c_void>);\n\
             \n\
             #[allow(dead_code)]\n\
             impl {name} {{\n\
             \x20   /// What a declaration that says `-> {name}` hands back\n\
             \x20   /// (ADR-302 D12): the address, or an abort naming the\n\
             \x20   /// declaration that claimed it would be one.\n\
             \x20   #[track_caller]\n\
             \x20   fn from_c(declaration: &str, address: *mut core::ffi::c_void) -> {name} {{\n\
             \x20       match core::ptr::NonNull::new(address) {{\n\
             \x20           Some(address) => {name}(address),\n\
             \x20           None => nikaia_std::foreign::nothing_came_back(declaration),\n\
             \x20       }}\n\
             \x20   }}\n\
             \n\
             \x20   /// The same, where the declaration **does** say `?` (D1).\n\
             \x20   fn maybe(address: *mut core::ffi::c_void) -> Option<{name}> {{\n\
             \x20       core::ptr::NonNull::new(address).map({name})\n\
             \x20   }}\n\
             \n\
             \x20   /// The same address, without giving the handle away\n\
             \x20   /// (ADR-302 D7): what a C function is handed is the\n\
             \x20   /// pointer, and this handle stays the caller's to close.\n\
             \x20   fn lent(&self) -> {name} {{\n\
             \x20       {name}(self.0)\n\
             \x20   }}\n\
             }}\n\
             \n\
             impl Drop for {name} {{\n\
             \x20   fn drop(&mut self) {{\n\
             \x20       let _released = unsafe {{ {release}(self.lent()) }};\n\
             \x20   }}\n\
             }}\n\
             \n"
        ));
    }

    /// **A type in an `extern "C"` declaration, as the pointer C wants**
    /// ([ADR-302](../../docs/specification/adr/adr-302.md) D5).
    ///
    /// The four forms the record writes, and each lives **for the call**, which
    /// is what a view is everywhere else in this language
    /// ([ADR-094](../../docs/specification/adr/adr-094.md)):
    ///
    /// | written | below |
    /// | :--- | :--- |
    /// | `&T` | `*const T` |
    /// | `&mut T` | `*mut T` |
    /// | `&[T]` | `*const T` |
    /// | `&mut [T]` | `*mut T` |
    ///
    /// **A slice loses its length here, and that is the point.** C takes a
    /// pointer and a count as two parameters, and D2 is the rule that keeps
    /// them one fact: the declaration names both and the call is checked.
    /// Rust's own `&[T]` is a *fat* pointer, so writing it in a declaration
    /// would be a signature the two languages disagree about — and what a
    /// reader would get for it is `rustc`'s `improper_ctypes` about a file
    /// nobody wrote (Part III, C.1).
    ///
    /// Anything that is not a view is the ordinary lowering: an `i32` is an
    /// `i32` at both ends.
    fn foreign_ty(&self, ty: &Type) -> String {
        // **`ref Array[T]` is what a run is written as**
        // ([ADR-184](../../docs/specification/adr/adr-184.md) D3, D4), here as
        // everywhere else. At this boundary it is still D1's address beside
        // D2's count and not Rust's fat pointer — two writers for one spelling,
        // which is the arrangement [ADR-302](../../docs/specification/adr/adr-302.md)
        // chose and this only renames.
        if self.writes_a_run(ty) {
            let element = match ty.generics.first() {
                Some(element) => self.foreign_ty(element),
                None => "u8".to_string(),
            };
            return format!("*{} {element}", pointing(ty.is_mut));
        }
        if ty.is_view {
            let pointed = Type {
                is_view: false,
                is_mut: false,
                ..ty.clone()
            };
            return format!("*{} {}", pointing(ty.is_mut), self.foreign_ty(&pointed));
        }
        self.ty(ty, Lifetimes::ELIDED)
    }

    /// **The hull a call's result needs**, where the callee is a foreign
    /// declaration handing back a handle
    /// ([ADR-302](../../docs/specification/adr/adr-302.md) D11, D12): the
    /// handle's name, and whether the declaration said it **may be absent**.
    fn handle_from(&self, func: &Expr) -> Option<(String, bool)> {
        let Expr::Variable(name) = func else {
            return None;
        };
        let declared = self.foreign_results.get(self.text(*name))?;
        let handle = self.handle_named(declared)?;
        Some((handle.to_string(), declared.is_nullable))
    }

    /// The callee as the source wrote it, for a message that names it.
    fn called_name(&self, func: &Expr) -> String {
        match func {
            Expr::Variable(name) => self.text(*name).to_string(),
            _ => String::new(),
        }
    }

    /// The handle a type names, where it names one
    /// ([ADR-302](../../docs/specification/adr/adr-302.md) D7,
    /// [ADR-302](../../docs/specification/adr/adr-302.md) D10) — through a `?`,
    /// because `CStr?` is a handle that may be absent and is still a handle.
    ///
    /// `None` for a view or a slice of one: what a `&T` at the boundary is is
    /// D1's pointer, and the handle is what it points at.
    fn handle_named(&self, ty: &Type) -> Option<&str> {
        if ty.is_view || ty.is_slice {
            return None;
        }
        let name = self.text(ty.name);
        match self.opaque_handles.contains_key(name) || base(name) == C_STRING {
            true => Some(name),
            false => None,
        }
    }

    /// What the address inside a handle points at, which is the one thing this
    /// language never looks through.
    fn pointed_at(&self, ty: &Type) -> &'static str {
        match base(self.text(ty.name)) == C_STRING {
            true => "core::ffi::c_char",
            false => "core::ffi::c_void",
        }
    }

    /// Which shape a declared parameter takes at the C boundary, or `None`
    /// where it is an ordinary value an `i32` is at both ends
    /// ([ADR-302](../../docs/specification/adr/adr-302.md) D5, D7).
    fn pointer_for(&self, callee: &str, ty: &Type) -> Option<Pointer> {
        if let Some(release) = self.opaque_handles.get(self.text(ty.name))
            && !ty.is_view
            && !ty.is_slice
        {
            return Some(Pointer::Handle {
                give: release == callee,
            });
        }
        // **A run is an address beside a count and a view is one address**
        // ([ADR-302](../../docs/specification/adr/adr-302.md) D5, D6), and
        // which of the two a declaration wrote is [`Emitter::writes_a_run`]'s
        // question since `ref Array[T]` became the spelling
        // ([ADR-184](../../docs/specification/adr/adr-184.md) D3).
        if self.writes_a_run(ty) {
            return Some(Pointer::Run { mutable: ty.is_mut });
        }
        pointer_for(ty)
    }

    /// **Whether this written type is a run of elements somebody else keeps**
    /// ([ADR-184](../../docs/specification/adr/adr-184.md) D3).
    ///
    /// `ref Array[T]` is the spelling, and `Array[T, N]` is **not** one: the
    /// count is what makes an array a type laid out inline
    /// ([ADR-152](../../docs/specification/adr/adr-152.md) D4), so the number
    /// of arguments is what tells the two apart and nothing else has to.
    ///
    /// `is_slice` is the bracket form the grammar no longer reads (D4); it
    /// stays here because a ledger written before 0.0.134 still carries it and
    /// a description is read back through the same tree.
    fn writes_a_run(&self, ty: &Type) -> bool {
        ty.is_slice || (ty.is_view && self.text(ty.name) == ARRAY && ty.generics.len() == 1)
    }

    /// Whether an `extern "C"` declaration takes this position in `size_t`
    /// ([ADR-302](../../docs/specification/adr/adr-302.md) D6).
    ///
    /// The declaration's own word, and not a list of names: `is_count` beside
    /// it is a fact about *Rust's* library that nothing in this compiler can
    /// derive, where a foreign declaration is written in this very file.
    fn takes_a_size(&self, callee: &str, at: usize) -> bool {
        self.foreign_params
            .get(callee)
            .and_then(|params| params.get(at))
            .is_some_and(|ty| !ty.is_view && !ty.is_slice && self.text(ty.name) == "usize")
    }

    /// One method of a `trait`: a signature and a `;`.
    ///
    /// Deliberately **not** `function` with the body switched off. That one
    /// reads the ledger for `sync`, for the `Shared` counts of each position and
    /// for whether the call can fail, and every one of those answers is about a
    /// *body* - which a declaration does not have. A second, smaller writer says
    /// what a declaration is instead of what a definition happens to omit.
    ///
    /// `sync` is the caller's rather than the declaration's, because the same
    /// shape reads two ways: a **trait** method without the word may pause
    /// ([ADR-288](../../docs/specification/adr/adr-288.md) D24), and an
    /// **`extern "C"`** declaration never can
    /// ([ADR-302](../../docs/specification/adr/adr-302.md) D2) — C has no
    /// suspension point, and a C function that sleeps blocks a thread, which is
    /// `println`'s question and not this one.
    fn trait_method(
        &self,
        out: &mut Out,
        method: &crate::ast::TraitMethod,
        is_sync: bool,
        // **Whether this is a C declaration**
        // ([ADR-302](../../docs/specification/adr/adr-302.md) D5), which is
        // what decides how a view is written: at the C boundary it is the
        // pointer C wants, and everywhere else it is the borrow Rust wants.
        // The same `&str` is a thin pointer to C and a fat one to Rust, so a
        // declaration that wrote the second would be a signature the two
        // languages disagree about - and `rustc` says so, about a file nobody
        // wrote (Part III, C.1).
        foreign: bool,
    ) -> Result<()> {
        let mut params: Vec<String> = Vec::new();
        if let Some(receiver) = &method.receiver {
            params.push(
                match (receiver.is_ref, receiver.is_mut) {
                    (true, true) => "&mut self",
                    (true, false) => "&self",
                    _ => "self",
                }
                .to_string(),
            );
        }
        for arg in &method.args {
            params.push(format!(
                "{}: {}",
                self.name(arg.name),
                match foreign {
                    true => self.foreign_ty(&arg.ty),
                    false => self.ty(&arg.ty, Lifetimes::ELIDED),
                }
            ));
        }
        // Kap 5.1: the language below has neither named arguments nor defaults,
        // so an option is an ordinary parameter here too - the same rule a
        // definition follows, because a caller fills in one list either way.
        for option in &method.config {
            params.push(format!(
                "{}: {}",
                self.name(option.name),
                self.ty(&option.ty, Lifetimes::ELIDED)
            ));
        }
        let returned = match &method.ret_type {
            // **A handle comes back as the address it is**
            // ([ADR-302](../../docs/specification/adr/adr-302.md) D12), and the
            // call is where it becomes a handle. Declaring the *type* here
            // would be a lie the moment C hands back nothing: the hull under a
            // handle is non-null, so a null arriving in one is undefined
            // before any check could run.
            // **Except text a C library owns**, which comes back as the
            // `Option<CText>` it is: one word, `None` for `NULL`, and the
            // declaration is where *this is a C string* is promised (ADR-218).
            Some(ty) if foreign && base(self.text(ty.name)) == C_STRING => {
                "Option<nikaia_std::foreign::CText>".to_string()
            }
            Some(ty) if foreign && self.handle_named(ty).is_some() => {
                format!("*mut {}", self.pointed_at(ty))
            }
            Some(ty) => match foreign {
                true => self.foreign_ty(ty),
                false => self.ty(ty, Lifetimes::ELIDED),
            },
            None => "()".to_string(),
        };
        let outcome = if method.can_throw {
            format!("Result<{returned}, Box<dyn std::error::Error>>")
        } else {
            returned
        };
        // **A method that may pause is declared in the return-position form**
        // ([ADR-288](../../docs/specification/adr/adr-288.md) D26): `-> impl
        // Future<Output = …>`, which an `async fn` in the `impl` satisfies.
        //
        // **The sugar `async fn` is not written here**, and that is the reason
        // the long form is: `async_fn_in_trait` warns on a public trait whose
        // future carries no `Send` bound, and a warning about the generated file
        // is a defect in this project (Part III C.1).
        //
        // **`+ Send` follows the executor, not the type.** At
        // `user_parallelism = yes` a task crosses threads and a spawn needs a
        // `Send` future; at `no` nothing crosses, the counts are plain
        // ([ADR-037](../../docs/specification/adr/adr-037.md) D7) and a `Send`
        // demand would refuse them. It is a requirement of the executor this
        // program is built for, written here — not a claim about a type, which
        // is `contracts::send`'s and is the same at both settings.
        let ret = match (is_sync, method.ret_type.is_some() || method.can_throw) {
            (false, _) => {
                let send = match self.build.user_parallelism {
                    UserParallelism::Yes => " + Send",
                    UserParallelism::No => "",
                };
                format!(" -> impl std::future::Future<Output = {outcome}>{send}")
            }
            (true, true) => format!(" -> {outcome}"),
            (true, false) => String::new(),
        };
        let generics: Vec<String> = method.generics.iter().map(|g| self.bounded(g)).collect();
        out.push(&format!(
            "fn {}{}({}){ret};\n",
            self.name(method.name),
            angled(&generics),
            params.join(", ")
        ));
        Ok(())
    }

    /// A type parameter with its bounds: `T`, or `T: Summarize + Clone`
    /// ([ADR-295](../../docs/specification/adr/adr-295.md) D7).
    ///
    /// **A shape bound does not travel** ([`crate::types::SHAPE_BOUNDS`],
    /// [ADR-304](../../docs/specification/adr/adr-304.md) D2): `Struct` and
    /// `Enum` are answered by a **declaration** rather than by an `impl`, and
    /// the language below has no trait by either name — so one written into the
    /// generated file would come back as *cannot find trait `Struct` in this
    /// scope*, about a file nobody wrote
    /// ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **Two names and not types**, which is why this does not break
    /// [ADR-296](../../docs/specification/adr/adr-296.md) D17: the emitter is
    /// reading a word of this language, as it already reads `sync` and
    /// `throws`, and not asking what a value's type is.
    /// Whether this bound is one of [`crate::types::SHAPE_BOUNDS`] **and** the
    /// program did not declare a `trait` by that name.
    ///
    /// A program that writes `trait Struct { … }` has an ordinary trait, and
    /// the bound is its own — so it travels, and the `impl` that answers it
    /// travels with it. Checking the declaration here is the same order every
    /// other reader of the two names takes.
    fn is_a_shape_bound(&self, bound: &str) -> bool {
        crate::types::SHAPE_BOUNDS.contains(&bound)
            && !self.parsed.program.items.iter().any(|item| {
                matches!(&item.node, crate::ast::Item::Trait { name, .. }
                    if self.name(*name) == bound)
            })
    }

    fn bounded(&self, param: &crate::ast::GenericParam) -> String {
        let name = self.name(param.name);
        let bounds: Vec<String> = param
            .bounds
            .iter()
            .map(|b| self.name(*b).into_owned())
            .filter(|bound| !self.is_a_shape_bound(bound))
            .collect();
        match bounds.is_empty() {
            true => name.into_owned(),
            false => format!("{name}: {}", bounds.join(" + ")),
        }
    }

    /// A function or a method. Kap 4.2: a `pub fn` with no name is the
    /// anonymous constructor, called as `Type(…)`; Rust has no such thing, so
    /// it is emitted as `new` and the call sites follow.
    /// Kap 7.1: `impl Error for T` becomes the three impls the failure channel
    /// needs, from the one method the source wrote.
    ///
    /// The author writes `message`. Rust wants `Display` for the text, and
    /// `std::error::Error` for the value to be accepted as an error at all;
    /// `Box<dyn Error>` then takes it. None of that is a decision - it is the
    /// same transcription ADR-296 D17 asks of every other lowering - so the
    /// `.nika` file says the part that is one and the emitter supplies the rest.
    ///
    /// `Debug` comes along because `std::error::Error` requires it, and a
    /// derived one is what a user would have written.
    fn error_impl(
        &self,
        out: &mut Out,
        target: &str,
        params: &str,
        methods: &[Spanned<Item>],
        lifetimes: Lifetimes,
    ) -> Result<()> {
        out.push(&format!("impl{params} {target}{params} {{\n"));
        for method in methods {
            let carries = self.carries_input.get(&method.span.at());
            out.from(&method.span, |out| {
                out.push("    ");
                self.function(
                    out,
                    &method.node,
                    1,
                    lifetimes,
                    carries,
                    Some(MethodOf {
                        target,
                        declared_by: None,
                    }),
                )
            })?;
        }
        out.push("}\n");

        out.push(&format!(
            "impl{params} std::fmt::Display for {target}{params} {{\n\
             \x20   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n\
             \x20       f.write_str(&self.message())\n\
             \x20   }}\n\
             }}\n"
        ));
        out.push(&format!(
            "impl{params} std::error::Error for {target}{params} {{}}\n"
        ));
        Ok(())
    }

    fn function(
        &self,
        out: &mut Out,
        item: &Item,
        depth: usize,
        lifetimes: Lifetimes,
        // The parameters written as views of the input buffer rather than of
        // the call (`Emitter::carries_input`). `None` where there is no such
        // parameter, which is almost every function.
        carries_input: Option<&HashSet<Symbol>>,
        // The type this is a method of, where it is one. It is what makes the
        // ledger key - `Counter::record` rather than `record` - and the key is
        // what a `Shared` position looks its count up by (`contracts::sharing`).
        owner: Option<MethodOf<'_>>,
    ) -> Result<()> {
        let Item::Fn {
            name,
            generics,
            receiver,
            args,
            config,
            spread,
            ret_type,
            body,
            is_sync,
            is_public,
            can_throw: throws,
            ..
        } = item
        else {
            return Err(anyhow!("not a function"));
        };

        let pad = "    ".repeat(depth);
        if *is_sync {
            out.push(&format!(
                "// sync (Part II, 12.1): pure CPU, cannot pause. Checked before\n{pad}// this was written - see `contracts::sync`.\n{pad}"
            ));
        }

        // The key the ledger - and therefore `contracts::sharing` - records this
        // function under, arrived at the same way both of them arrive at it: the
        // anonymous constructor of Part I 4.2 is `new`, and a method carries its
        // type in front.
        let own_name = match name {
            Some(name) => self.text(*name).to_string(),
            None => "new".to_string(),
        };
        let key = match owner {
            Some(owner) => format!("{}::{own_name}", owner.target),
            None => own_name.clone(),
        };

        let mut params = Vec::new();
        if let Some(receiver) = receiver {
            params.push(
                match (receiver.is_ref, receiver.is_mut) {
                    (true, true) => "&mut self",
                    (true, false) => "&self",
                    _ => "self",
                }
                .to_string(),
            );
        }
        // A parameter the subject's buffer covers is written as a view of that
        // buffer, so the signature says what the body does with it; everything
        // else keeps the position's own spelling.
        // **A function that takes a keep** (ADR-283 D10) writes the positions
        // its views leave through with the keep's lifetime, so `rustc` holds
        // the body to exactly what the plan says.
        let kept = self.kept_lifetimes(&key, lifetimes);
        // **A view handed back out of a parameter that holds views**
        // ([ADR-282](../../docs/specification/adr/adr-282.md) D20):
        // `fn first(xs: Vec[ref String]) -> ref String` lends `xs`, and the
        // result is one of the views inside it, not a borrow of the list. Two
        // lifetimes are in that parameter - the loan and the buffer - and the
        // result's is the buffer's, so it is named: `fn first<'a>(xs:
        // &Vec<&'a str>) -> &'a str`. Only where that one parameter is all
        // there is to borrow from; with two, which buffer is ADR-283's
        // question and already its refusal.
        let inner_views = match (lifetimes, receiver, kept, ret_type) {
            (Lifetimes::ELIDED, None, None, Some(ty))
                if self.carries_a_view(ty) && carries_input.is_none_or(|set| set.is_empty()) =>
            {
                let carrying: Vec<_> = args
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| self.carries_a_view(&a.ty))
                    .collect();
                // **Lent**, by the declaration's own test below: a parameter
                // handed over is one lifetime, which the elision names.
                let lent = |at: usize| {
                    self.own_contracts.functions.get(&key).is_some_and(|c| {
                        crate::contracts::keeps::lends_in(c, at, &[self.library, &self.described])
                            && !c
                                .signature
                                .as_ref()
                                .and_then(|s| s.params.get(at))
                                .is_some_and(|(_, ty)| ty.is_a_view())
                    })
                };
                match carrying.as_slice() {
                    [(at, one)] if !one.ty.is_view && !one.ty.either && lent(*at) => Some(one.name),
                    _ => None,
                }
            }
            _ => None,
        };
        let how = |name: Symbol| {
            if let Some(kept) = kept.filter(|_| self.tethered_position(&key, self.text(name))) {
                return kept;
            }
            if inner_views == Some(name) {
                return Lifetimes::NAMED;
            }
            let viewed = args.iter().any(|a| a.name == name && a.ty.is_view);
            match (carries_input.is_some_and(|set| set.contains(&name)), viewed) {
                (true, true) => lifetimes.of_the_input(),
                // **A struct parameter that carries a view parameter's buffer**
                // is written over it: `s: &mut Summary<'a>, name: &'a str`.
                (true, false) => Lifetimes {
                    reference: lifetimes.reference,
                    params: "'a",
                },
                (false, _) => lifetimes,
            }
        };
        // **A parameter the body does not keep is a view**
        // ([ADR-094](../../docs/specification/adr/adr-094.md) D2). The `&` is
        // written here, in the declaration, and at the call — one answer read
        // twice (`contracts::keeps::lends`), because the two disagreeing is a
        // `&&T` or a moved value in the language below.
        let lent = self.own_contracts.functions.get(&key);
        // **`xs: Array[T]` is an array of any length, and the call says which**
        // ([ADR-184](../../docs/specification/adr/adr-184.md) D3). The language
        // below has the same shape and the same name for it — a `const`
        // parameter — so the function is written once and monomorphised per
        // length, which is what *beliebig, aber fest* means when it reaches a
        // machine.
        //
        // **One per parameter**, because two arrays in one signature are two
        // lengths: `fn zip(a: Array[i64], b: Array[i64])` takes any two and not
        // two of the same.
        //
        // The name carries the position rather than the parameter's own name,
        // for the reason the escape exists (ADR-298 D10): a parameter may be
        // called anything, including something the language below reserves.
        let lengths: Vec<Option<String>> = args
            .iter()
            .enumerate()
            .map(|(at, a)| {
                let any = !a.ty.is_view
                    && !a.ty.is_slice
                    && a.ty.count.is_none()
                    && a.ty.generics.len() == 1
                    && self.text(a.ty.name) == ARRAY;
                any.then(|| format!("{LENGTH_PARAMETER}{at}"))
            })
            .collect();
        let mut either_first: Vec<String> = Vec::new();
        params.extend(args.iter().enumerate().map(|(at, a)| {
            // The **source** name is what a `Shared` position is counted by
            // (`count_at`), and the **escaped** one is what is written: two uses
            // of one name that must not be collapsed, or the lookup misses on
            // exactly the programs the escape is for (ADR-298 D10).
            let name = self.text(a.name);
            // The receiver is a parameter of the contract and not of `args`, so
            // the position in the signature is one further along where there is
            // one.
            let at = at + usize::from(receiver.is_some());
            // **And only where the written type does not already carry one.**
            // `&Vec[Entry]` in a declaration is the assertion D2 keeps: the
            // parameter is a view either way, and what D1 changes is the
            // *call*, where the reference is written for both kinds alike.
            let written_as_a_view = lent
                .and_then(|c| c.signature.as_ref())
                .and_then(|s| s.params.get(at))
                .is_some_and(|(_, ty)| ty.is_a_view());
            // **`mut` is a third state and comes first**
            // ([ADR-094](../../docs/specification/adr/adr-094.md) D3): a
            // parameter the callee changes in place lowers to `&mut T`, and the
            // caller's value is what changes. `lends` withholds its own claim
            // on such a position, so the two never both answer.
            let changes = lent
                .and_then(|c| c.signature.as_ref())
                .is_some_and(|s| s.mutable.iter().any(|m| m == name));
            let reference = if changes {
                "&mut "
            } else if lent.is_some_and(|c| {
                crate::contracts::keeps::lends_in(c, at, &[self.library, &self.described])
            }) && !written_as_a_view
            {
                "&"
            } else {
                ""
            };
            // **An array of any length is written with the length this
            // signature declares for it** (ADR-184 D3), rather than through
            // `ty_counted`, which would write the name `Array` — and there is
            // no type by that name in the language below.
            let written = match lengths.get(at.wrapping_sub(usize::from(receiver.is_some()))) {
                Some(Some(length)) => {
                    let element =
                        a.ty.generics
                            .first()
                            .map(|e| self.ty_counted(e, how(a.name), self.count_at(&key, name)))
                            .unwrap_or_else(|| "u8".to_string());
                    format!("[{element}; {length}]")
                }
                _ => {
                    // **Run is the absence of `keeps`**, off the very contract
                    // `lends` above was read from
                    // ([ADR-277](../../docs/specification/adr/adr-277.md) D11,
                    // [ADR-277](../../docs/specification/adr/adr-277.md) D8).
                    // A contract this build does not have reads as run, which
                    // is the same answer the *call* writer gives an unresolved
                    // callee - the two have to agree or one parameter gets two
                    // shapes.
                    let runs = lent.is_none_or(|c| !c.keeps.iter().any(|k| k == name));
                    let held = self.code_parameter_runs.replace(runs);
                    let inside = self.in_parameter.replace(true);
                    let written = self.ty_counted(&a.ty, how(a.name), self.count_at(&key, name));
                    *self.in_parameter.borrow_mut() = inside;
                    *self.code_parameter_runs.borrow_mut() = held;
                    written
                }
            };
            // **A `String` the body only reads is a `&str`**
            // ([ADR-282](../../docs/specification/adr/adr-282.md) D5), not a
            // `&String`. Every caller's `String` reaches it through the `&` the
            // call already writes, and a literal reaches it as it is - which is
            // what makes `greet("Ada")` cost nothing, where a `&String` would
            // have needed a `String` built to be pointed at.
            //
            // A parameter both kinds of text flow into is one of these too
            // ([ADR-282](../../docs/specification/adr/adr-282.md) D14): a body
            // that only reads needs neither kind, only the text, so nothing is
            // wrapped for it and each caller lends what it has.
            let plain_text = self.text(a.ty.name) == "String"
                && a.ty.generics.is_empty()
                && (!a.ty.is_view || a.ty.either)
                && !a.ty.is_nullable
                && !a.ty.is_slice;
            // **A list the body only reads is a slice**, for the reason a
            // `String` is a `&str`: a `&Vec<T>` is a pointer to a pointer, and
            // every read through it loads the buffer and the length again
            // where a `&[T]` holds them in two registers. Every caller's `Vec`
            // reaches it through the `&` the call already writes.
            let plain_list = self.text(a.ty.name) == "Vec"
                && a.ty.generics.len() == 1
                && !a.ty.is_nullable
                && !a.ty.is_slice
                && written.ends_with('>');
            let written = match (reference, plain_text, plain_list) {
                ("&", true, _) => "str".to_string(),
                ("&", _, true) if written.starts_with("Vec<") => {
                    format!("[{}]", &written[4..written.len() - 1])
                }
                ("", _, true) if written.starts_with("&Vec<") => {
                    format!("&[{}]", &written[5..written.len() - 1])
                }
                _ => written,
            };
            // **A published parameter both kinds of text flow into takes
            // whatever hands over as either** (ADR-282 D23): a caller elsewhere
            // hands over text of its own, as the ledger says, and this
            // package's own calls hand over what they have. It is taken as it
            // goes in on the body's first line.
            if *is_public && a.ty.either && a.ty.generics.is_empty() && reference.is_empty() {
                let (by, into) = match a.ty.is_nullable {
                    true => ("IntoEitherMaybe", "into_either_maybe"),
                    false => ("IntoEither", "into_either"),
                };
                let name = escaped(name);
                either_first.push(format!("let {name} = {name}.{into}();"));
                return format!(
                    "{name}: impl nikaia_std::either_text::{by}<Either = \
                     nikaia_std::either_text::EitherText<'e>>"
                );
            }
            format!("{}: {reference}{written}", escaped(name))
        }));
        // Kap 5.1: the language below has neither named arguments nor defaults,
        // so an option becomes an ordinary parameter here - in declaration
        // order, which is the order every call site fills in. The names stay
        // the source's, so a `rustc` diagnostic about one still lands on the
        // parameter the programmer wrote (ADR-300).
        params.extend(config.iter().map(|c| {
            let name = self.text(c.name);
            format!(
                "{}: {}",
                escaped(name),
                self.ty_counted(&c.ty, how(c.name), self.count_at(&key, name))
            )
        }));

        // ADR-296 D5: `...args: Self::dsl` is the one parameter whose type the
        // *call site* decides, because the DSL string it comes from decides it.
        // A generic parameter is what that is in the language below, and Rust
        // monomorphises it per DSL string exactly as D5 asks - so the driver is
        // written once and pays no heap traffic per statement.
        let dsl = spread.as_ref().map(|name| {
            params.push(format!("{}: {DSL_PARAMETER}", self.text(*name)));
            DSL_PARAMETER.to_string()
        });
        // **The keep, last**, because every call writes it last - after the
        // options and the spread, which is the only order there is.
        if let Some(kept) = kept {
            params.push(format!(
                "{KEEP_PARAM}: {}nikaia_std::tether::Keep",
                kept.reference
            ));
        }

        // **`fn hand[T](x: T)` is `fn hand<T>(x: T)`**
        // ([ADR-295](../../docs/specification/adr/adr-295.md) D3). The `[T]` was
        // read by the parser and then fell out here, because this position had
        // no slot for one - so a program that passed every stage of this
        // compiler asked `rustc` about a type nobody had declared, which is
        // Part III C.1's class exactly.
        //
        // **A copy has no type parameter left**
        // ([ADR-304](../../docs/specification/adr/adr-304.md) D6): the whole of
        // what the parameter was for is the shape, and the shape is written out
        // here. What stands in the signature is the type itself.
        // **And a length a parameter left open is a `const` parameter**
        // (ADR-184 D3), declared beside the type parameters because that is
        // what it is: a name the call fills in.
        let open: Vec<String> = lengths
            .iter()
            .flatten()
            .map(|length| format!("const {length}: usize"))
            .collect();
        let declared: Vec<String> = match self.specialising.borrow().is_some() {
            true => dsl.clone().into_iter().chain(open).collect(),
            false => generics
                .iter()
                .map(|g| self.bounded(g))
                .chain(dsl.clone())
                .chain(open)
                .collect(),
        };
        let declared: Vec<String> = match kept {
            Some(Lifetimes::KEPT) => std::iter::once("'k".to_string()).chain(declared).collect(),
            _ => declared,
        };
        // **The buffer a struct parameter carries is named on the function**
        // where no `impl` names it already.
        let declared: Vec<String> = match lifetimes == Lifetimes::ELIDED
            && (carries_input.is_some_and(|set| !set.is_empty()) || inner_views.is_some())
        {
            true => std::iter::once("'a".to_string()).chain(declared).collect(),
            false => declared,
        };
        // The lifetime a published parameter's `EitherText` is of (ADR-282 D23).
        let declared: Vec<String> = match either_first.is_empty() {
            true => declared,
            false => std::iter::once("'e".to_string()).chain(declared).collect(),
        };
        *self.either_prelude.borrow_mut() = either_first;

        // Kap 7.1: `throws` becomes a `Result` in the emitted Rust, over
        // `Box<dyn Error>` because Nikaia's own error types are not lowered
        // yet - the `?` the DSL driver needs works against it, and every error
        // keeps its own type behind it.
        let returned = match ret_type {
            // `Self::dsl` in the result position is the same type the spread
            // parameter has: a driver that hands the bound parameters back
            // names them the only way D5 gives it to name them.
            Some(ty) if self.text(ty.name) == SELF_DSL => {
                if dsl.is_none() {
                    // **The body's first statement**, which is the nearest
                    // place this walk has: a declaration has no span of its
                    // own here, and a refusal on the first line of the body is
                    // the same function the reader is looking at. The checker
                    // reaches for the same statement when it needs to name a
                    // function rather than a line inside one.
                    return Err(match body.stmts.first() {
                        Some(first) => refused_at!(
                            first.span.at(),
                            "`Self::dsl` stands for the function's `...args: Self::dsl` \
                             parameters, but this function has none."
                        ),
                        None => refused!(
                            "`Self::dsl` stands for the function's `...args: Self::dsl` \
                             parameters, but this function has none."
                        ),
                    });
                }
                DSL_PARAMETER.to_string()
            }
            Some(ty) => {
                // The result's lifetimes are not always the parameters': a view
                // handed back by a function that borrowed nothing has to say
                // `'static`, because there is nothing for Rust's elision to take
                // and `-> &str` is a message about the generated file
                // (`Lifetimes::STATIC`).
                //
                // A receiver counts as something to borrow from, and so does any
                // parameter that is a view or holds one. Where either is there,
                // the position keeps the spelling it had.
                //
                // **And a struct that holds a view is one too**, which is the
                // half `holds_view` alone could not see: `-> Vec[Entry]` where
                // `Entry` holds a `&str` carries a lifetime exactly as `-> &str`
                // does, and a function with nothing to borrow from lowered it to
                // `Vec<Entry<'_>>` — *missing lifetime specifier* about a file
                // nobody wrote. `carries_a_view` asks the question about the
                // whole type, the declaration included, and it is asked on
                // **both** sides so that `fn f(r: Reading) -> Reading` keeps the
                // elision it needs.
                //
                // **And only where the position declares no lifetime of its
                // own**, which is `Lifetimes::ELIDED` and is D9's own case: *a
                // free function's signature*. Inside an `impl` that declares
                // `'a` the result is the subject's `'a`, and `'static` there
                // says the value outlives the program — which is a promise the
                // `impl` cannot keep. `examples/1brc.nika`'s `Summary()` is
                // what said so: an anonymous constructor has no parameters and
                // no receiver, so the widening reached it.
                let borrows_from_something =
                    receiver.is_some() || args.iter().any(|a| self.carries_a_view(&a.ty));
                let widens = lifetimes == Lifetimes::ELIDED
                    && self.carries_a_view(ty)
                    && !borrows_from_something;
                let result = match (widens, kept) {
                    (_, Some(kept))
                        if self.tethered_position(&key, crate::contracts::tether::RESULT) =>
                    {
                        kept
                    }
                    (true, _) => Lifetimes::STATIC,
                    (false, _) if inner_views.is_some() => Lifetimes::NAMED,
                    // **A published parameter's `'e` is the one lifetime there
                    // is** (ADR-282 D23): the elision does not read one out of
                    // an `impl` bound, so it is named.
                    (false, _)
                        if lifetimes == Lifetimes::ELIDED
                            && receiver.is_none()
                            && !self.either_prelude.borrow().is_empty()
                            && !args.iter().any(|a| {
                                self.carries_a_view(&a.ty)
                                    && !(a.ty.either && a.ty.generics.is_empty())
                            }) =>
                    {
                        Lifetimes {
                            reference: "&'e ",
                            params: "'e",
                        }
                    }
                    (false, _) => lifetimes,
                };
                self.ty_counted(ty, result, self.count_at(&key, SHARED_RESULT))
            }
            None => "()".to_string(),
        };
        // **The failure channel** ([ADR-280](../../docs/specification/adr/adr-280.md)
        // D1): the error type where the ledger names exactly one, the box
        // otherwise. A method that implements a **trait** keeps the box, and
        // has to: the trait's own declaration writes the channel, and an `impl`
        // answering with another type would not satisfy it.
        let channel = match owner.and_then(|o| o.declared_by) {
            Some(_) => "Box<dyn std::error::Error>".to_string(),
            None => self.error_channel(
                &key,
                lifetimes,
                receiver.is_some() || args.iter().any(|a| self.carries_a_view(&a.ty)),
            ),
        };
        let ret = if *throws {
            format!(" -> Result<{returned}, {channel}>")
        } else if ret_type.is_some() {
            format!(" -> {returned}")
        } else {
            String::new()
        };

        let vis = if *is_public { "pub " } else { "" };
        let name = own_name;
        // ADR-303 D4: `fn main` is the runtime's, and the program's own entry
        // point is called from inside it. Only at the crate root, and only for
        // the shape `entry_point` writes a wrapper for - `user_main` and this
        // ask the same question, so a renamed function always has a caller.
        //
        // The **source** name is kept for the body, because it is what an
        // error raised in here reports as its site (ADR-023 D6, ADR-280): a
        // `throw` in `main` says `main`, and the name this emitter chose for
        // the lowering is not something the author ever wrote.
        let emitted = if depth == 0 && name == MAIN && self.user_main().is_some() {
            PROGRAM_MAIN.to_string()
        } else {
            escaped(&name).into_owned()
        };
        // **One copy per type this was used with**
        // ([ADR-304](../../docs/specification/adr/adr-304.md) D6), named so the
        // call and the definition cannot drift: `check::specialised` writes
        // both.
        let emitted = match self.standing_for() {
            Some(on) => crate::check::specialised(&emitted, &on),
            None => emitted,
        };

        // ADR-055 D1: a function that can pause is an `async fn`, and one the
        // ledger's `sync` column says cannot is a plain `fn`. The property is
        // ADR-288 D1's, already inferred; this reads it.
        //
        // **And a trait's declaration is the wider claim**
        // ([ADR-288](../../docs/specification/adr/adr-288.md) D25): a method
        // declared without `sync` is lowered `-> impl Future<…>`, so every
        // implementation of it hands back a future — whether or not its own
        // body pauses. A body that never pauses under such a declaration is
        // correct and says nothing, which is D2's own sentence; what it cannot
        // do is hand back an `i64` where a future was promised.
        let declared_pausing = owner
            .and_then(|owner| owner.declared_by)
            .map(|t| format!("{t}::{name}"))
            .is_some_and(|key| {
                self.own_contracts
                    .functions
                    .get(&key)
                    .is_some_and(|c| !c.sync_claim.is_sync())
            });
        // **A function written `sync(f)` is the `async fn` its body is**
        // ([ADR-288](../../docs/specification/adr/adr-288.md) D30, D31): `f` may
        // pause, so the call of it inside is awaited. What the promise changes
        // is the caller's check, and how a caller that cannot pause drives it.
        let pausing = if self.pauses(&key) || declared_pausing || self.lambda_decides(&key) {
            "async "
        } else {
            ""
        };
        // **A name this compiler chose is not one the program wrote**
        // ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)):
        // `describe__User` is a copy's name ([ADR-304](../../docs/specification/adr/adr-304.md)
        // D2) and Rust's own lint would ask the author to rename a function
        // they wrote as `describe`.
        if self.specialising.borrow().is_some() {
            out.push("#[allow(non_snake_case)]\n");
            out.push(&pad);
        }
        // **A function takes the parameters the program wrote** (#459): the
        // language below's lint asks for fewer than eight inputs, which is a
        // rule about code written in it, and the warning would be about a file
        // nobody wrote.
        if args.len() + usize::from(receiver.is_some()) > 7 {
            out.push("#[allow(clippy::too_many_arguments)]\n");
            out.push(&pad);
        }
        // **What a fold's step calls runs once per item** (#433): LLVM left
        // 1BRC's `Summary::record` out of line - hashbrown's insert path makes
        // it too big for its heuristic - at 319 instructions a row, and 300
        // with the hint. Only a hint, and only here: every function `#[inline]`
        // would bloat the code and the build.
        if self.fold_callees.contains(&name) {
            out.push("#[inline]\n");
            out.push(&pad);
        }
        // **A function with a precondition has two entries**
        // ([ADR-269](../../docs/specification/adr/adr-269.md) D20): `f` checks
        // it and calls `f__unchecked`, which is the body. A caller the prover
        // saw prove it, or check it at the call, calls the second; every other
        // caller - a function value, another package, the language below -
        // reaches the first. A copy made per type is reached by name from the
        // call that made it, so it checks at the top of its body instead.
        let checks = self.entries.get(&key).filter(|c| !c.is_empty());
        let copy = self.standing_for().is_some();
        let signature = |name: &str| {
            format!(
                "{vis}{pausing}fn {name}{}({}){ret} ",
                angled(&declared),
                params.join(", ")
            )
        };
        let check_lines =
            |checks: &[crate::prove::Check]| checks.iter().map(failed_check).collect::<String>();
        let pending = match checks {
            Some(checks) if !copy => {
                out.push("#[allow(non_snake_case)]\n");
                out.push(&pad);
                out.push(&signature(&format!("{emitted}{UNCHECKED}")));
                // The checked entry is written after the body, below.
                Some(format!(
                    "{pad}#[allow(dead_code)]\n{pad}{}{{ {}{}{UNCHECKED}({}){} }}\n",
                    signature(&emitted),
                    check_lines(checks),
                    match (receiver.is_some(), owner.is_some()) {
                        (true, _) => format!("self.{emitted}"),
                        (false, true) => format!("Self::{emitted}"),
                        (false, false) => emitted.clone(),
                    },
                    params
                        .iter()
                        .filter(|p| !p.ends_with("self"))
                        .map(|p| p
                            .split(':')
                            .next()
                            .unwrap_or_default()
                            .trim()
                            .trim_start_matches("mut ")
                            .to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    if pausing.is_empty() { "" } else { ".await" },
                ))
            }
            Some(checks) => {
                out.push(&signature(&emitted));
                out.push(&format!("{{ {}", check_lines(checks)));
                Some("}".to_string())
            }
            None => {
                out.push(&signature(&emitted));
                None
            }
        };
        // **The parameters a call has to await**
        // ([ADR-277](../../docs/specification/adr/adr-277.md) D11): those whose
        // type is code and does not say `sync`, which is the default.
        let awaited: Vec<Symbol> = args
            .iter()
            .filter(|arg| (*arg.ty.code).as_ref().is_some_and(|code| !code.is_sync))
            .map(|arg| arg.name)
            .collect();
        let declared = Declared {
            throws: *throws,
            returns_value: ret_type.is_some(),
            awaited: &awaited,
            key: &key,
            channel: &channel,
        };
        match self.settle_fns.get(&key) {
            // **A body where a value with a cleanup may die ends at a settle
            // point** ([ADR-297](../../docs/specification/adr/adr-297.md) D2):
            // the body runs to its end as a block of its own - a `return` and
            // a failing call end the block and not the function - and what
            // died in it is settled before the function hands its value back.
            Some(&failing) => {
                let body = Out::scratch(|inner| self.function_body(inner, body, depth, declared))?;
                let settle = match failing {
                    true => "nikaia_std::cleanup::settle_after",
                    false => "nikaia_std::cleanup::settle_with",
                };
                let kind = self.settle_kind();
                let rest = match failing {
                    true => "_, _, _",
                    false => "_, _",
                };
                out.push(&format!("{{ {settle}::<{kind}, {rest}>(async "));
                out.append(body);
                // A cleanup's failure is raised where the settle point is
                // ([ADR-280](../../docs/specification/adr/adr-280.md) D29).
                match failing {
                    true => {
                        let origin = key.rsplit("::").next().unwrap_or(&key);
                        out.push(&format!(", &{origin:?}).await }}"));
                    }
                    false => out.push(").await }"),
                }
            }
            None => self.function_body(out, body, depth, declared)?,
        }
        // The checked entry after the unchecked one's body, or the brace that
        // closes a copy's checks around its body.
        match pending {
            Some(closing) if closing == "}" => out.push(" }\n"),
            Some(entry) => {
                out.push("\n");
                out.push(&entry);
            }
            None => out.push("\n"),
        }
        Ok(())
    }

    /// The body of a function, which differs from any other block in one way:
    /// a `throws` function has to hand back an `Ok`.
    fn function_body(
        &self,
        out: &mut Out,
        body: &Block,
        depth: usize,
        declared: Declared<'_>,
    ) -> Result<()> {
        let Declared {
            throws,
            returns_value,
            awaited,
            key,
            channel,
        } = declared;
        *self.keep_function.borrow_mut() = key.to_string();
        self.nonnegative
            .borrow_mut()
            .insert(key.to_string(), nonnegative_locals(body));
        let flow = Flow {
            changed: &[],
            awaited,
            throws,
            origin: key.rsplit("::").next().unwrap_or(key),
            caught: false,
            // A function body is not a handler, whatever the call to it sits
            // inside: the boundary `in_lambda` does not cross, this does not
            // cross either.
            caught_named: false,
            caught_sum: None,
            caught_member: None,
            in_a_place: false,
            arms_are_statements: false,
            statement: usize::MAX,
            function: key,
            channel,
            // A function body is not a `catch`'s guarded half.
            handled_here: false,
            // A function body was not written inside whatever lambda the call
            // to it sits in: this is the one boundary the flag does not cross.
            in_lambda: false,
            // A function body binds nothing until a statement in it does.
            bound: "",
            // Both are decided per expression, so a body starts with neither.
            widen: false,
            inferred: false,
            // And no loop encloses a function's first statement.
            in_loop: false,
        };

        // A function body's last statement is the *function's* value, which is
        // the one place a `return x` may be written as `x`.
        let tail = if returns_value {
            Tail::Return
        } else {
            Tail::Statement
        };

        // A published parameter taken as it goes in (ADR-282 D23), first.
        let first = std::mem::take(&mut *self.either_prelude.borrow_mut());
        if !throws {
            // The function's own settle point stands around the body, so the
            // body is not settled a second time as a block (ADR-297 D2).
            let settled = self.settle_fns.contains_key(key);
            let body_block = |out: &mut Out| match settled {
                true => self.block_plain(out, body, depth, flow, tail, None),
                false => self.block(out, body, depth, flow, tail),
            };
            if first.is_empty() {
                return body_block(out);
            }
            // Inside the body's own brace, so there is one block and not two.
            let mut written = Out::scratch(body_block)?;
            written.insert_after_first_brace(&format!(" {}", first.join(" ")));
            out.append(written);
            return Ok(());
        }

        let pad = "    ".repeat(depth);
        let inner_pad = "    ".repeat(depth + 1);

        out.push("{\n");
        for line in &first {
            out.push(&format!("{inner_pad}{line}\n"));
        }
        let last = body.stmts.len().saturating_sub(1);
        let mut i = 0;
        while i < body.stmts.len() {
            let stmt = &body.stmts[i];
            out.push(&inner_pad);
            // A value-returning `throws` function ends in its value; one that
            // returns nothing ends in the `Ok(())` below, so its last statement
            // is a statement like any other.
            let here = tail.at(i, last);
            // **A tail that is a `throw` is not wrapped**
            // ([ADR-292](../../docs/specification/adr/adr-292.md) D20): it writes
            // its own `Err(…)` and leaves, so an `Ok(` around it is
            // `Ok(return Err(…))` — *unreachable call*, about a file nobody
            // wrote. `fn f() -> String throws { throw E() }` is the whole
            // program it takes.
            //
            // A tail `return` is the other way round and stays wrapped: it is
            // rewritten to its bare value here (the one place Part I allows
            // that), and the `Ok(` is what makes that value the function's
            // outcome.
            let wrap = here == Tail::Return && !matches!(&stmt.node, Stmt::Expr(Expr::Throw(_)));
            // **And a tail that enters a grammar binds its value first**
            // ([ADR-185](../../docs/specification/adr/adr-185.md) D2). The
            // lowering of an entry is a block holding `let _source = &*data`
            // and a stream over it, and in `Ok({ … }?)` those temporaries live
            // to the end of the enclosing block — past the **local** that owns
            // the text. `rustc` then says *`data` does not live long enough*
            // about a file nobody wrote
            // ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)),
            // and its own hint is this fix: *save the expression's value in a
            // new local variable*.
            //
            // A `let` makes the block a **statement**, so its temporaries drop
            // at the `;` and ahead of the local — which is `ARM_VALUE`'s trick
            // one construct over ([ADR-292](../../docs/specification/adr/adr-292.md)
            // D1), for the same kind of reason.
            //
            // **Only where the lowering writes the borrow**, which is the one
            // shape this emitter can be sure of: it is the emitter's own
            // `_source`, not something a program's expression left behind. Every
            // other tail keeps `Ok(x)`, because a line the generated file does
            // not need is a line a reader has to skip (ADR-296 D17).
            let binds = wrap && self.a_tail_that_enters_a_grammar(&stmt.node);
            // **The keeps a statement needs are declared before it**
            // (ADR-283), and before the `Ok(` a tail is wrapped in: a `let`
            // inside it would not be Rust.
            self.write_keep_prelude(out, key, stmt.span.at(), depth + 1);
            out.from(&stmt.span, |out| {
                match (wrap, binds) {
                    (true, true) => out.push(&format!("let {ARM_VALUE} = ")),
                    (true, false) => out.push("Ok("),
                    (false, _) => {}
                }
                self.stmt(out, &stmt.node, &stmt.span, depth + 1, here, flow)?;
                match (wrap, binds) {
                    (true, true) => out.push(&format!(";\n{inner_pad}Ok({ARM_VALUE})")),
                    (true, false) => out.push(")"),
                    (false, _) => {}
                }
                Ok(())
            })?;
            out.push("\n");
            i += 1;
        }
        // **Not after a statement that never comes back** (0.0.240): a body
        // that ends in `panic(…)`, `throw` or `return` has left before the
        // `Ok(())`, and `rustc` said *unreachable expression* about a file
        // nobody wrote ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
        let leaves = body.stmts.last().is_some_and(|stmt| match &stmt.node {
            Stmt::Return(_) => true,
            Stmt::Expr(expr) => jumps(expr) || self.never_returns(expr),
            _ => false,
        });
        if !returns_value && !leaves {
            out.push(&format!("{inner_pad}Ok(())\n"));
        }
        out.push(&pad);
        out.push("}");
        Ok(())
    }

    /// **Whether this statement enters a grammar**
    /// ([ADR-185](../../docs/specification/adr/adr-185.md) D2).
    ///
    /// The one lowering that leaves a temporary borrowing a local behind: the
    /// entry's block binds `_source` to a view of its input and builds a stream
    /// over it. Read off the **statement** rather than off the type, because
    /// what has to know is the writer of the `Ok(` around it and this emitter
    /// has no types ([ADR-288](../../docs/specification/adr/adr-288.md)).
    fn a_tail_that_enters_a_grammar(&self, stmt: &Stmt) -> bool {
        let mut found = false;
        let mut look = |expr: &Expr| {
            if let Expr::Call { func, args, .. } = expr
                && let Expr::Path(segments) = func.as_ref()
                && let [grammar, _] = segments.as_slice()
                && self.grammars.contains_key(grammar)
                && args.len() == 1
            {
                found = true;
            }
        };
        match stmt {
            Stmt::Expr(expr) => visit_expr(expr, &mut look),
            Stmt::Return(Some(expr)) => visit_expr(expr, &mut look),
            _ => {}
        }
        found
    }

    // --- Grammars ---

    fn grammar(&self, out: &mut Out, def: &GrammarDef) -> Result<()> {
        out.push("grammar! {\n");
        out.push(&format!("    grammar {} {{\n", self.text(def.name)));

        *self.grammar_rule_types.borrow_mut() = def
            .rules
            .iter()
            .filter_map(|rule| {
                let ty = rule.ret_type.as_ref()?;
                Some((
                    self.text(rule.name).to_string(),
                    self.ty(ty, Lifetimes::NAMED),
                ))
            })
            .collect();
        for rule in &def.rules {
            out.push("\n");
            out.from(&rule.span, |out| self.grammar_rule(out, rule))?;
        }
        // **`list(item, sep)` is a rule the backend is given** (Part II
        // 10.8, issue #165 closed at 0.0.249): the backend has no
        // such element and read `list` as a rule of the grammar's own, so each
        // grammar that writes one gets the two template rules it lowers to -
        // the first item, then every `sep item` after it, or nothing at all.
        let lists = def
            .rules
            .iter()
            .flat_map(|rule| &rule.alts)
            .any(|alt| writes_a_list(self.parsed, &alt.pattern));
        if lists {
            out.push(&format!(
                "\n        rule {LIST_RULE}<T>(item: Rule<T>, sep) -> Vec<T> =\n\
                 \x20         first:item rest:{LIST_RULE}_more<T>(item = item, sep = sep)*\n\
                 \x20           -> {{ let mut all = vec![first]; all.extend(rest); all }}\n\
                 \x20       | \"\"\n\
                 \x20           -> {{ Vec::new() }}\n\
                 \n        rule {LIST_RULE}_more<T>(item: Rule<T>, sep) -> T =\n\
                 \x20         sep x:item\n\
                 \x20           -> {{ x }}\n"
            ));
        }

        out.push("    }\n");
        out.push("}\n");
        Ok(())
    }

    fn grammar_rule(&self, out: &mut Out, rule: &GrammarRule) -> Result<()> {
        if let Some(frame) = &rule.frame {
            out.push(&format!("        {}\n", frame_attribute(frame)));
        }

        // An entry is a `pub` rule to the backend: one function per entry
        // (ADR-296 D25).
        let vis = if rule.is_entry { "pub " } else { "" };
        let ret = match &rule.ret_type {
            Some(ty) => format!(" -> {}", self.ty(ty, Lifetimes::NAMED)),
            None => String::new(),
        };
        // The label sits between the return type and the `=`, in both
        // languages: `rule expr -> Expr # "expression" = …`.
        let label = match &rule.label {
            Some(text) => format!(" # {text:?}"),
            None => String::new(),
        };
        out.push(&format!(
            "        {vis}rule {}{ret}{label} =\n",
            self.text(rule.name)
        ));

        for (i, alt) in rule.alts.iter().enumerate() {
            let separator = if i == 0 { "  " } else { "| " };

            match &alt.action {
                Some(action) => {
                    out.push(&format!("          {separator}"));
                    self.pattern(out, &alt.pattern)?;
                    out.push("\n            -> ");
                    // A grammar action is the rule's own body: what it ends
                    // in is what the rule hands back, so a `return` there is
                    // that value (ADR-296).
                    self.block(out, action, 3, Flow::PLAIN, Tail::Return)?;
                    out.push("\n");
                }
                None => {
                    // The one body that needs no action: a `par_fold` is the
                    // whole rule (ADR-296 D10), so the value of the rule is the
                    // value of the fold. The binding the backend wants is
                    // supplied here rather than demanded from the user.
                    if rule.ret_type.is_some() {
                        out.push(&format!("          {separator}folded:"));
                        self.pattern(out, &alt.pattern)?;
                        out.push("\n            -> { folded }\n");
                    } else {
                        out.push(&format!("          {separator}"));
                        self.pattern(out, &alt.pattern)?;
                        out.push("\n");
                    }
                }
            }
        }

        Ok(())
    }

    fn pattern(&self, out: &mut Out, pattern: &Spanned<Pattern>) -> Result<()> {
        out.from(&pattern.span, |out| match &pattern.node {
            Pattern::Seq(parts) => self.patterns(out, parts, " "),
            Pattern::Choice(parts) => self.patterns(out, parts, " | "),
            Pattern::Bind { name, pat } => {
                out.push(&format!("{}:", self.text(*name)));
                self.pattern(out, pat)
            }
            Pattern::Literal(text) => {
                out.push(&format!("\"{text}\""));
                Ok(())
            }
            Pattern::Ref {
                name,
                generics,
                args,
            } if self.text(*name) == "list" && generics.is_empty() && args.len() == 2 => {
                out.push(LIST_RULE);
                // The element type is the item rule's where this grammar
                // declares it; a built-in is left to the backend to infer.
                if let Pattern::Ref {
                    name: item,
                    args: none,
                    ..
                } = &args[0].node
                    && none.is_empty()
                    && let Some(ty) = self.grammar_rule_types.borrow().get(self.text(*item))
                {
                    out.push(&format!("<{ty}>"));
                }
                out.push("(item = ");
                self.pattern(out, &args[0])?;
                out.push(", sep = ");
                self.pattern(out, &args[1])?;
                out.push(")");
                Ok(())
            }
            Pattern::Ref {
                name,
                generics,
                args,
            } => {
                out.push(self.text(*name));
                // `dec[i32](…)` -> `dec::<i32>(…)`? No: the backend's builtin
                // takes its type between angles, not after a turbofish, and a
                // grammar body is the backend's syntax once it is emitted.
                if !generics.is_empty() {
                    let params: Vec<String> = generics
                        .iter()
                        .map(|g| self.ty(g, Lifetimes::NAMED))
                        .collect();
                    out.push(&format!("<{}>", params.join(", ")));
                }
                if !args.is_empty() {
                    out.push("(");
                    self.patterns(out, args, ", ")?;
                    out.push(")");
                }
                Ok(())
            }
            Pattern::Repeat { pat, rep } => {
                // A sequence or a choice has to be grouped before a suffix can
                // apply to all of it rather than to its last element.
                let group = matches!(pat.node, Pattern::Seq(_) | Pattern::Choice(_));
                if group {
                    out.push("(");
                }
                self.pattern(out, pat)?;
                if group {
                    out.push(")");
                }
                out.push(&repeat_suffix(*rep));
                Ok(())
            }
            Pattern::Group(inner) => {
                out.push("(");
                self.pattern(out, inner)?;
                out.push(")");
                Ok(())
            }
            Pattern::Cut => {
                out.push("=>");
                Ok(())
            }
            Pattern::Fold(spec) => self.fold(out, spec),
        })
    }

    fn patterns(
        &self,
        out: &mut Out,
        patterns: &[Spanned<Pattern>],
        separator: &str,
    ) -> Result<()> {
        for (i, pattern) in patterns.iter().enumerate() {
            if i > 0 {
                out.push(separator);
            }
            self.pattern(out, pattern)?;
        }
        Ok(())
    }

    fn fold(&self, out: &mut Out, spec: &FoldSpec) -> Result<()> {
        let rule = self.text(spec.rule);
        let name = if spec.merge.is_some() {
            "par_fold"
        } else {
            "fold"
        };

        out.push(&format!("{name}({rule}, "));
        self.expr(out, &spec.init, 0, Flow::PLAIN)?;
        out.push(", ");
        self.fold_step(out, &spec.step)?;
        if let Some(merge) = &spec.merge {
            out.push(", ");
            self.fold_merge(out, merge)?;
        }
        out.push(")");
        Ok(())
    }

    /// A fold's step, adapted where the program's own declarations say it has
    /// to be.
    ///
    /// ADR-296 D17 refused to infer this from the shape of the body, and was
    /// right to: `acc.record(m)` and `acc.merged(m)` look identical. What
    /// changed is that `impl` blocks are lowered now, so this is a lookup in
    /// what the program declares - `record` takes `&mut self` and returns
    /// nothing, so the accumulator is changed in place; a method that returns a new
    /// accumulator is left exactly as written.
    fn fold_step(&self, out: &mut Out, step: &Expr) -> Result<()> {
        if let Expr::Closure { params, body, .. } = step
            && let [accumulator, item] = params.as_slice()
            && let Some(Stmt::Expr(Expr::MethodCall {
                receiver, method, ..
            })) = body.stmts.last().map(|s| &s.node)
        {
            let on_accumulator = matches!(&**receiver, Expr::Variable(name) if name == accumulator);
            let mutates = self
                .by_name
                .get(method)
                .and_then(|m| m.as_ref())
                .is_some_and(Method::mutates_in_place);

            // **Changed in place, not threaded** (`winnow-grammar` ADR 24
            // §11): a step whose accumulator is written `&mut` is run by a
            // fold that never moves it. Handed in and back by value, an
            // accumulator that holds a table was copied in and out once per
            // item - 7 instructions a row of 1BRC. `&mut _` is all the
            // generator needs to pick that fold; the type is the init's.
            if on_accumulator && mutates {
                let accumulator = self.text(*accumulator);
                out.push(&format!(
                    "|{accumulator}: &mut _, {}| {{ ",
                    self.text(*item)
                ));
                for stmt in &body.stmts {
                    self.stmt(out, &stmt.node, &stmt.span, 0, Tail::Statement, Flow::PLAIN)?;
                    out.push(" ");
                }
                out.push("}");
                return Ok(());
            }
        }

        self.expr(out, step, 0, Flow::PLAIN)
    }

    /// A fold's merge. `Summary::merge` names a method, and the `impl` says
    /// whether it mutates its subject and whether it takes the other by view.
    fn fold_merge(&self, out: &mut Out, merge: &Expr) -> Result<()> {
        if let Expr::Path(segments) = merge
            && let [owner, name] = segments.as_slice()
            && let Some(method) = self.methods.get(&(*owner, *name))
            && method.mutates_in_place()
            && method.args.len() == 1
        {
            let by_view = if method.args[0].ty.is_view { "&" } else { "" };
            // The accumulator's type is annotated because the path
            // names it: without that, the backend's `let merge = …`
            // leaves the closure with nothing to infer from.
            let owner = self.ty(
                &Type {
                    name: *owner,
                    generics: Vec::new(),
                    is_view: false,
                    is_tuple: false,
                    is_nullable: false,
                    code: Box::new(None),
                    count: None,
                    is_mut: false,
                    is_slice: false,
                    either: false,
                },
                Lifetimes::NAMED,
            );
            out.push(&format!(
                "|mut a: {owner}, b| {{ a.{}({by_view}b); a }}",
                self.text(*name)
            ));
            return Ok(());
        }

        self.expr(out, merge, 0, Flow::PLAIN)
    }

    // --- Types ---

    /// The shadow type of every deferred-parameter DSL in this unit
    /// (ADR-296 D5).
    ///
    /// One struct per parameter list, a field per `:name`, and **generic in
    /// every field**: nothing in `… WHERE id = :id …` says what `:id` is, so
    /// the type comes from the argument at the call site and is monomorphised
    /// there. Inventing `i32` here is exactly the guess ADR-296 D17 forbids an
    /// emitter to make.
    ///
    /// A plain struct, passed by value: D5 asks for the parameters on the
    /// stack, and a struct of the caller's own values is what that is.
    fn shadow_types(&self, out: &mut Out) {
        for (name, parameters) in crate::dsl::shadow_types(self.parsed) {
            let generics: Vec<String> = (0..parameters.len()).map(|i| format!("P{i}")).collect();
            let fields: Vec<String> = parameters
                .iter()
                .zip(&generics)
                .map(|(field, ty)| format!("    pub {field}: {ty},"))
                .collect();
            out.push(&format!(
                "// ADR-296 D5: the parameters of a `dsl … {{ … }} eod` statement, \
                 as a type.\n\
                 #[derive(Debug, Clone, Copy, PartialEq)]\n\
                 #[allow(non_camel_case_types, dead_code)]\n\
                 pub struct {name}<{}> {{\n{}\n}}\n\n",
                generics.join(", "),
                fields.join("\n"),
            ));
        }
    }

    /// A `dsl … { … } eod` body: the `html` template compiled here (ADR-017),
    /// or a statement whose holes the call site fills (ADR-296 D5).
    ///
    /// Which it is follows from the target and the holes, and not from anything
    /// inferred about the body. `html` is the one grammar this compiler *is*,
    /// so `:name` there is an immediate capture (ADR-296 D4); anywhere else a
    /// `:name` is a deferred parameter and the body reaches its driver intact.
    /// A body with neither is refused, because nothing here knows what it
    /// means.
    fn template(
        &self,
        out: &mut Out,
        target: Symbol,
        context: Option<&Symbol>,
        content: &str,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        let name = self.text(target);

        // ADR-296 D5: a body with `:name` holes and a target this compiler is
        // not itself the grammar for is a *statement*, and its value is its own
        // text. Nothing is substituted into it: a deferred parameter is not
        // string interpolation, so the value may not be spliced into the source
        // and change what it means (Part III, 15.3). The holes stay as written
        // and the driver binds them - which is also the only lowering that
        // needs to know nothing about the foreign syntax (ADR-296 D17).
        if crate::dsl::is_deferred(name, content) {
            if let Some(context) = context {
                return Err(refused_at!(
                    flow.statement,
                    "`dsl {name} {{ … }}` with `:name` holes can't take a context, but \
                     `{}` was given.",
                    self.text(*context)
                ));
            }
            out.push(&rust_string(content.trim()));
            return Ok(());
        }

        if name != "html" {
            return Err(refused_at!(
                flow.statement,
                "`dsl {name} {{ … }}` has no `:name` holes, and `{name}` isn't a \
                 language the compiler knows. Add `:name` holes for the parameters, or \
                 use `dsl html`."
            ));
        }
        if let Some(context) = context {
            return Err(refused_at!(
                flow.statement,
                "`dsl html` can't take a context, but `{}` was given.",
                self.text(*context)
            ));
        }

        // The framing whitespace is not markup: the newline after `{` and the
        // indentation before `} eod` are there because the template is written
        // in a file, and a block form that kept them would make every value it
        // produces carry the indentation of the function it was written in.
        // Whitespace *inside* the body is kept exactly.
        // **The template's own refusals get the statement's place too.**
        // `template.rs` works on text and never saw a file, which is exactly
        // what `at_the_statement` is for — five refusals, one handover.
        let segments = at_the_statement(flow, template::split(content.trim()))?;

        // ADR-017 D3. Every hole that escaping cannot make safe, at once: a
        // template with three of them should say so three times rather than
        // once per build.
        let illegal = template::illegal(&segments);
        if !illegal.is_empty() {
            let mut message = String::from("the template has holes escaping cannot make safe\n");
            for (expr, at) in &illegal {
                message.push_str(&format!("  {}\n", template::illegal_message(expr, at)));
            }
            return Err(crate::diagnostics::refuse(message));
        }

        let pad = "    ".repeat(depth + 1);
        let close = "    ".repeat(depth);
        // The compiler knows every literal byte of this template, so it knows
        // what the result is at least as long as - and a `String` that starts
        // that size does not double its way there (ADR-178 §1).
        //
        // It is a *floor*, not a guess: the holes and the bodies of `<for>`
        // add to it and never subtract, so reserving it can never be too much
        // to be worth it, and this needs no threshold and no measurement to
        // decide between two shapes. What it needed a measurement for is
        // whether it is worth doing at all, and that is in
        // `crates/nikaia/tests/measure.rs`.
        out.push(&format!(
            "{{\n{pad}let mut __html = String::with_capacity({});\n",
            template::literal_length(&segments)
        ));
        self.template_segments(out, &segments, depth + 1, flow)?;
        out.push(&format!("{pad}__html\n{close}}}"));
        Ok(())
    }

    /// The pieces of a template, appended to `__html` in order.
    fn template_segments(
        &self,
        out: &mut Out,
        segments: &[template::Segment],
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        let pad = "    ".repeat(depth);

        for segment in segments {
            match segment {
                template::Segment::Text(text) => {
                    out.push(&format!("{pad}__html.push_str({});\n", rust_string(text)));
                }
                template::Segment::Hole { expr, .. } => {
                    // Parsed as Nikaia and emitted as Nikaia: a hole holds an
                    // expression of this language, not a foreign one.
                    let parsed = self.parsed.hole(expr).map_err(|e| {
                        refused_at!(
                            flow.statement,
                            "The template hole `{{{expr}}}` can't be read. {e}"
                        )
                    })?;
                    out.push(&format!(
                        "{pad}__html.push_str(&::nikaia_std::html::Render::render(&"
                    ));
                    self.expr(out, &parsed, depth, flow)?;
                    out.push("));\n");
                }
                // The loop of the language below, over the captured collection:
                // it borrows rather than copies, exactly as it would in the
                // function around the template.
                template::Segment::For {
                    binding,
                    collection,
                    body,
                } => {
                    // **`.iter()` and not `&`**, for the reason the ordinary
                    // `for` has it ([ADR-094](../../docs/specification/adr/adr-094.md)
                    // D4): the captured name may already *be* a view — since
                    // D1 a parameter the body only reads is one — and
                    // `&entries` is then a `&&Vec<Entry>`, which Rust does not
                    // iterate. `.iter()` reads the same through any number of
                    // references.
                    out.push(&format!("{pad}for {binding} in {collection}.iter() {{\n"));
                    self.template_segments(out, body, depth + 1, flow)?;
                    out.push(&format!("{pad}}}\n"));
                }
            }
        }
        Ok(())
    }

    /// A path, with the map it names chosen the same way its type is.
    ///
    /// `HashMap::new` has no counterpart on a map with a hasher of its own -
    /// `new` exists only for the default one - so the trusted map is built with
    /// `default`, which is what every `HashMap<_, _, S>` is built with.
    fn path(&self, segments: &[&str]) -> String {
        if let [container, "new"] = segments {
            let chosen = self.map_name(container);
            if chosen != *container {
                return format!("{chosen}::default");
            }
        }
        // `unaliased` on the joined path, because an alias is this file's name
        // for a *package* and the head segment is where one can stand
        // ([ADR-286](../../../docs/specification/adr/adr-286.md) D12). Resolved
        // here rather than emitted as a Rust `use … as …`: every file's items are
        // in one crate root, so a Rust alias there would be the whole program's.
        self.parsed.unaliased(
            &segments
                .iter()
                .map(|s| self.map_name(s))
                .collect::<Vec<_>>()
                .join("::"),
        )
    }

    /// `HashMap` under the name the provenance of this program's input picked
    /// (ADR-010 D5).
    ///
    /// Trusted keys get a fast, fixed-seed hash; untrusted keys keep the keyed,
    /// randomly seeded one `std` gives every map by default. Nothing else about
    /// the map changes - same table, same API, same full-content equality - so
    /// this is a name and not a translation.
    fn map_name<'n>(&self, name: &'n str) -> &'n str {
        match (name, self.trusted_input) {
            ("HashMap", true) => "TrustedMap",
            ("HashSet", true) => "TrustedSet",
            _ => name,
        }
    }

    /// **One side of a text `+`**, where a side read out of a list is handed
    /// over as a view (0.0.234).
    ///
    /// A read at a number is `*index::get(&w, 0)`, a place, which is right
    /// wherever the value is only looked at; handed to `concat::plus` by value
    /// it is a **move** out of the list, and `w[0] + w[1]` over a
    /// `Vec[String]` was `rustc`'s *cannot move out of a shared reference*
    /// about a file nobody wrote. A concatenation makes new text and needs
    /// only to read its sides, so the side is lent as `&str` - which is
    /// `AsRef` whether the list holds owned text or views of it.
    fn concatenated(&self, out: &mut Out, side: &Expr, depth: usize, flow: Flow<'_>) -> Result<()> {
        if matches!(side, Expr::Index { .. }) {
            out.push("::core::convert::AsRef::<str>::as_ref(&");
            self.expr(out, side, depth, flow)?;
            out.push(")");
            return Ok(());
        }
        self.expr(out, side, depth, flow)
    }

    /// The same, for a name that may carry its module
    /// ([ADR-313](../../docs/specification/adr/adr-313.md) D3).
    ///
    /// `collections::HashMap` is the spelling a program writes once `HashMap`
    /// is outside the prelude, and the question this answers is about the
    /// **type** and not about the prefix: the prefix rides through untouched,
    /// so the annotation and the constructor cannot disagree about which hash a
    /// map got.
    fn mapped_path(&self, name: &str) -> String {
        match name.rsplit_once("::") {
            Some((module, last)) => format!("{module}::{}", self.map_name(last)),
            None => self.map_name(name).to_string(),
        }
    }

    /// The name a type is written with below, which for one name depends on the
    /// value and not only on the name.
    ///
    /// `Shared[T]` is `Rc<T>` or `Arc<T>`, and **which one is decided per value**
    /// (ADR-037 D7): the atomic count is the floor at both settings of
    /// `user_parallelism` (D6), and a value the analysis proves never crosses a
    /// thread gets the plain one. So the count has to arrive from outside -
    /// `map_name` above sees only a name and could not answer it.
    ///
    /// The **full path** and no `use`: `sharing::rust_name` already names both as
    /// paths, two names as common as `Rc` and `Arc` are two a program may have of
    /// its own, and nothing about the emitted file needs them shortened.
    fn written_name(&self, name: &str, count: crate::contracts::sharing::Count) -> String {
        match name {
            SHARED => crate::contracts::sharing::rust_name(count),
            // **The same question, the same answer**
            // ([ADR-281](../../../docs/specification/adr/adr-281.md) D7): a lock
            // is only reachable from two places through a shared handle, so the
            // count that handle was given decides the lock inside it. The count
            // rides down through the arguments already (`ty_counted`), which is
            // exactly the thread this needs.
            //
            // **And at one user thread it is always the cheap shape** (D2).
            // Nothing a user writes can cross there, so the safe shape buys
            // nothing and costs what `user_parallelism = no` exists to save -
            // and the cheap one keeps the diagnostic, which is the only failure
            // reachable at that setting.
            LOCKED => crate::contracts::sharing::lock_name(match self.build.user_parallelism {
                UserParallelism::No => crate::contracts::sharing::Count::Plain,
                UserParallelism::Yes => count,
            }),
            // `unaliased`, for the reason [`Emitter::path`] gives: a type may be
            // written with this file's own name for the package that declares it
            // ([ADR-286](../../../docs/specification/adr/adr-286.md) D12).
            _ => self.parsed.unaliased(&self.mapped_path(name)),
        }
    }

    /// Whether `owner::variant` is a variant of an enum this file declares.
    fn is_variant(&self, owner: &str, variant: &str) -> bool {
        self.parsed.program.items.iter().any(|item| {
            matches!(&item.node, Item::Enum { name, variants, .. }
                if self.text(*name) == owner
                    && variants.iter().any(|v| self.text(v.name) == variant))
        })
        // ...or another unit of the package declares (`ledger.nika` reading
        // `ty.nika`'s `State`), which its entry in the package's ledger says.
        || self
            .own_contracts
            .types
            .get(owner)
            .is_some_and(|entry| entry.variants.iter().any(|v| v.name == variant))
    }

    /// What `Shared(x)`, `SharedMut(x)` and `Locked(x)` allocate, outermost first
    /// ([ADR-281](../../../docs/specification/adr/adr-281.md) D2).
    ///
    /// Two for the shared mutable type, because it is one name and two hulls; one
    /// for the other two; `None` for a call that is not a hull's.
    fn hull_new(&self, name: &str, count: crate::contracts::sharing::Count) -> Option<Vec<String>> {
        match name {
            SHARED => Some(vec![self.written_name(SHARED, count)]),
            LOCKED => Some(vec![self.written_name(LOCKED, count)]),
            SHARED_MUT => Some(vec![
                self.written_name(SHARED, count),
                self.written_name(LOCKED, count),
            ]),
            _ => None,
        }
    }

    /// The count the analysis gave the `Shared` written at one position, with the
    /// atomic floor where it has no answer (ADR-037 D6).
    fn count_at(&self, function: &str, value: &str) -> crate::contracts::sharing::Count {
        // **At one user thread every count is plain**
        // ([ADR-312](../../../docs/specification/adr/adr-312.md) D10), including at
        // a position the analysis has no entry for. Without this the *floor* would
        // answer for such a position, and the floor is the atomic count - which at
        // `no` is exactly what that switch exists not to pay.
        if self.build.user_parallelism == UserParallelism::No {
            return crate::contracts::sharing::Count::Plain;
        }
        self.shared
            .get(&format!("{function}::{value}"))
            .copied()
            .unwrap_or(crate::contracts::sharing::Count::Atomic)
    }

    /// A type, with the atomic floor for any `Shared` inside it.
    ///
    /// Every position whose count is actually known names it with
    /// [`Emitter::ty_counted`] instead. The floor is what a position nothing
    /// decided gets, which is ADR-037 D6's answer and never a wrong one - only a
    /// slower one.
    fn ty(&self, ty: &Type, lifetimes: Lifetimes) -> String {
        self.ty_counted(ty, lifetimes, crate::contracts::sharing::Count::Atomic)
    }

    /// The same, where the count a `Shared` in this position was given is known.
    ///
    /// The count rides down through the arguments, because `Vec[Shared[i64]]` in
    /// a parameter is the parameter's class and not a class of its own - a count
    /// belongs to the allocation, and the analysis keys it by the position the
    /// source wrote.
    fn ty_counted(
        &self,
        ty: &Type,
        lifetimes: Lifetimes,
        count: crate::contracts::sharing::Count,
    ) -> String {
        let mut out = String::new();

        // **A function inside a container is a value the container holds**
        // (Part I 5.3), so it is kept, even in a parameter the body only runs:
        // `fs: ref Vec[fn(i64) -> i64]` is a list of kept functions, and the
        // run shape - `impl AsyncFn` - is a bound, which a list's element
        // cannot be. It was written as one, and `rustc` refused the call.
        if ty.code.is_none()
            && (ty.is_tuple || !ty.generics.is_empty())
            && *self.in_parameter.borrow()
        {
            let inside = self.in_parameter.replace(false);
            let written = self.ty_counted(ty, lifetimes, count);
            *self.in_parameter.borrow_mut() = inside;
            return written;
        }

        // `(A, B)` in both languages, and the parts are the arguments.
        if ty.is_tuple {
            let parts: Vec<String> = ty
                .generics
                .iter()
                .map(|g| self.ty_counted(g, lifetimes, count))
                .collect();
            return format!("({})", parts.join(", "));
        }

        // **The parameter stands for the type, while a copy is written**
        // ([ADR-304](../../docs/specification/adr/adr-304.md) D6). A specialised
        // copy has no `T` left in its signature, so every position that wrote
        // one writes the type instead — which is what makes the copy an
        // ordinary function the language below compiles.
        //
        // **The borrow is taken and let go before the recursion**, because the
        // call below reaches this line again and a `RefCell` held across it is
        // a panic rather than a message.
        let standing = self.specialising.borrow().clone();
        if let Some((parameter, on)) = standing
            && ty.generics.is_empty()
            && ty.code.is_none()
            && self.text(ty.name) == parameter
        {
            let concrete = crate::ast::Type {
                name: self.parsed.interner.intern_string(&on),
                ..ty.clone()
            };
            let held = self.specialising.replace(None);
            let written = self.ty_counted(&concrete, lifetimes, count);
            *self.specialising.borrow_mut() = held;
            return written;
        }

        // **A parameter that is code**
        // ([ADR-277](../../docs/specification/adr/adr-277.md) D6), lowered as
        // D5's **run** case: a closure argument, which is what `std`'s own
        // higher-order entries take and what costs nothing. The kept case — a
        // boxed closure over a boxed future — is the step of that record that
        // is not built, and `NK1142` is what a field or a result meets, so this
        // is only ever reached in a parameter.
        //
        // `throws` puts the same `Result` on the closure's result that a
        // `throws` function's own declaration puts on its (Kap 7.1), which is
        // what makes a lambda that fails fit it.
        if let Some(code) = &*ty.code {
            let params: Vec<String> = ty
                .generics
                .iter()
                .map(|g| a_list_read_as_a_slice(self.ty_counted(g, lifetimes, count)))
                .collect();
            let outcome = match (&*code.result, code.can_throw) {
                (Some(r), false) => self.ty_counted(r, lifetimes, count),
                (Some(r), true) => format!(
                    "Result<{}, Box<dyn std::error::Error>>",
                    self.ty_counted(r, lifetimes, count)
                ),
                (None, true) => "Result<(), Box<dyn std::error::Error>>".to_string(),
                (None, false) => "()".to_string(),
            };
            // **The type decides the shape**
            // ([ADR-277](../../docs/specification/adr/adr-277.md) D11), and
            // nothing inferred stands behind it: a parameter whose type may
            // pause — which is the **default**, since `sync` is what says
            // otherwise — is a closure returning a **boxed future**, whether the
            // callee runs it or keeps it. A reader can tell what a signature
            // costs by reading it, and a library author who wants no box writes
            // `sync`, which is what that word already promises.
            //
            // **The future is named rather than inferred**:
            // `Pin<Box<dyn Future<Output = …>>>`, which is the shape a handler
            // is taken in in practice and the one `|a| Box::pin(async move
            // { … })` produces.
            //
            // D1's own reason for it was that Rust has no stable `async`
            // closure, and that is **false**
            // ([ADR-277](../../docs/specification/adr/adr-277.md) D13):
            // `impl AsyncFn(A) -> R` compiles on this toolchain and costs 1.37
            // ns/call against this shape's 11.99, on a 0.31 floor. What holds
            // the shape up now is D1's second half alone - one spelling for a
            // run parameter and a kept one, so a reader can tell what a
            // signature costs by reading it - and whether that is worth 8.7× is
            // a question on `docs/open-decisions.md`.
            // **A parameter that may pause takes the shape its body needs,
            // and the body's answer is `keeps`**
            // ([ADR-277](../../docs/specification/adr/adr-277.md) D11).
            //
            // A **run** parameter is `impl AsyncFn(A) -> R`: no box, no dynamic
            // call, and 1.37 ns/call against the boxed future's 11.99 on a 0.31
            // floor (`benches/handler`). A **kept** one keeps the boxed
            // closure, because `AsyncFn` is a *bound* and a value stored in a
            // field needs a type.
            //
            // **One written type, two representations, chosen by an analysis of
            // the body** — which is what `Shared[T]` already does per value
            // ([ADR-037](../../docs/specification/adr/adr-037.md) D7), what
            // [ADR-283](../../docs/specification/adr/adr-283.md) D2 does per
            // construction site, and what
            // [Part I 5.4](../../docs/specification/10-nikaia-light.md) C
            // already says about this very construct: *the context of such a
            // parameter is inferred, not written*.
            //
            // **`sync` is untouched** and still writes the plain closure: the
            // word is an assertion about what the code may do
            // ([ADR-288](../../docs/specification/adr/adr-288.md)), and a
            // parameter that cannot pause has no future to hand back either
            // way.
            // **A kept function value is one shared closure**: a field, a
            // result, a `let`, and a parameter the callee keeps. What may
            // pause hands back a boxed future.
            let runs = *self.in_parameter.borrow() && *self.code_parameter_runs.borrow();
            if !runs {
                let outcome = match code.is_sync {
                    true => outcome,
                    false => format!("{}<{outcome}>", self.boxed_future()),
                };
                // Where tasks run on threads, what a task takes crosses one.
                let crossing = match self.build.overlaps_user_code() {
                    true => " + Send + Sync",
                    false => "",
                };
                // **What a kept function is handed is borrowed for the call
                // alone**: its parameters' references are elided, which makes
                // the closure take a borrow of any length. Written with the
                // struct's `'a`, `node_of: fn(ref Expr) -> i64` in a struct
                // that also holds a view took only what lives as long as that
                // view, and a call with a local was a borrow `rustc` refused
                // (found moving `text_tiers` into Nikaia, #125).
                let params: Vec<String> = ty
                    .generics
                    .iter()
                    .map(|g| a_list_read_as_a_slice(self.ty_counted(g, Lifetimes::ELIDED, count)))
                    .collect();
                return format!(
                    "nikaia_std::func::Kept<dyn Fn({}) -> {outcome}{crossing}>",
                    params.join(", ")
                );
            }
            if code.is_sync {
                let shape = match (&*code.result, code.can_throw) {
                    (None, false) => String::new(),
                    _ => format!(" -> {outcome}"),
                };
                return format!("impl Fn({}){shape}", params.join(", "));
            }
            if *self.code_parameter_runs.borrow() {
                return format!("impl AsyncFn({}) -> {outcome}", params.join(", "));
            }
            return format!(
                "impl Fn({}) -> std::pin::Pin<Box<dyn std::future::Future<Output = {outcome}>>>",
                params.join(", ")
            );
        }

        // **`Seen[T]` is erased**
        // ([ADR-281](../../docs/specification/adr/adr-281.md) D22): it is a type
        // here and in the ledger and **not** one in the language below, so a
        // `Seen[i64]` is an `i64`, a field declared `Seen[i64]` is an `i64`
        // field, and a signature with `Seen` in it is one without. No counter,
        // no marker, no check at run time, no bytes.
        //
        // Written where the name is read rather than at every position that
        // takes a type, so that a `Vec[Seen[i64]]` and a `Seen[i64]?` come out
        // right for the same reason `Shared` does one paragraph down.
        //
        // **Only with its argument.** A bare `Seen` is not the stamp - the stamp
        // always says what it stamps - and a program may declare a type of that
        // name; erasing it to itself recursed until the compiler's stack ran
        // out, on `struct Seen { … }` and a parameter of it.
        if self.text(ty.name) == crate::contracts::ty::SEEN
            && let [inner] = ty.generics.as_slice()
        {
            let inner = inner.clone();
            let inner = Type {
                is_nullable: ty.is_nullable || inner.is_nullable,
                is_view: ty.is_view || inner.is_view,
                ..inner
            };
            return self.ty_counted(&inner, lifetimes, count);
        }

        // Part I 2.3: `T?` is an `Option<T>`, which is the mapping Part III 15.2
        // writes the other way round. The `?` is peeled and the rest of this
        // function renders the type it is nullable *of* - so `&str?` is an
        // `Option<&'a str>` and the view still picks up its lifetime, which it
        // would not if the wrapper were applied by name.
        if ty.is_nullable {
            let inner = Type {
                is_nullable: false,
                ..ty.clone()
            };
            return format!("Option<{}>", self.ty_counted(&inner, lifetimes, count));
        }

        // **`&[T]` is a view of a run of elements**
        // ([ADR-179](../../docs/specification/adr/adr-179.md) D1), and Rust's
        // own `&[T]` is what it lowers to: a fat pointer that carries its
        // length, which is the half of it a `Vec` and an `Array` also carry and
        // a declaration at the C boundary does not.
        //
        // **The boundary has its own writer** (`foreign_ty`), which turns the
        // same type into an address beside a count
        // ([ADR-302](../../docs/specification/adr/adr-302.md) D5, D6). Two
        // writers for one spelling is the arrangement that record already
        // chose, and this is the second half of it arriving.
        // **`ref Array[T]` is the run's other spelling**
        // ([ADR-184](../../docs/specification/adr/adr-184.md) D3): an `Array`
        // under a `ref` with **one** argument carries no length in the type, so
        // there is nothing to lay out inline and what it names is a run
        // somebody else keeps. An `Array[T, N]` has two and is
        // [ADR-152](../../docs/specification/adr/adr-152.md) D4's `[T; N]`,
        // written further down.
        if self.writes_a_run(ty) {
            let element = match ty.generics.first() {
                Some(element) => self.ty_counted(element, lifetimes, count),
                None => "u8".to_string(),
            };
            return format!("{}[{element}]", lifetimes.reference);
        }

        // **Text both kinds of which flow into a position**
        // ([ADR-282](../../docs/specification/adr/adr-282.md) D14): a view
        // borrowed, text of its own owned, each where it is put in.
        if ty.either {
            return format!("nikaia_std::either_text::EitherText<{}>", lifetimes.params);
        }
        // A view is a borrow of the parser's input, and that is where the
        // lifetime comes from - the source never writes one (ADR-283).
        if ty.is_view {
            out.push_str(lifetimes.reference);
        }
        // **A view of text is a `&str`**
        // ([ADR-184](../../docs/specification/adr/adr-184.md) D2). Text is one
        // type and `ref String` is what a program writes for a view of it; the
        // language below spells that view with a noun of its own, and this is
        // the one place the two names differ. `&String` would compile and would
        // be the wrong thing: every `std` entry that takes text takes a `&str`,
        // and a `&String` at a call is a borrow of a borrow at the first one
        // that does not coerce.
        if ty.is_view && self.text(ty.name) == "String" && ty.generics.is_empty() {
            // **Held, where the keeper drops entries** (ADR-283 D12): a view
            // that carries its own handle on the buffer it points into.
            if self.holding.borrow().is_some() {
                return "nikaia_std::tether::Held".to_string();
            }
            out.push_str("str");
            return out;
        }
        // **`SharedMut[T]` is one name and two hulls**
        // ([ADR-281](../../../docs/specification/adr/adr-281.md) D1). It is
        // expanded here and nowhere earlier, so the checker, the ledger and every
        // message keep the name the source wrote. The two hulls are the ones the
        // count already decides - `written_name` answers for each.
        if self.text(ty.name) == SHARED_MUT
            && let Some(held) = ty.generics.first()
        {
            // `out` already carries the `&` of a view, so the expansion is
            // pushed rather than returned on its own.
            out.push_str(&format!(
                "{}<{}<{}>>",
                self.written_name(SHARED, count),
                self.written_name(LOCKED, count),
                self.ty_counted(held, lifetimes, count)
            ));
            return out;
        }
        // **`Array[T, N]` is `[T; N]`**
        // ([ADR-152](../../docs/specification/adr/adr-152.md) D1, D2): `N`
        // elements inline, no allocation, and the layout C gives it.
        //
        // After the view, so `&Array[f64, 3]` is a borrow of the array rather
        // than an array of borrows, and before the name is written, because the
        // arguments do not go where a generic's do.
        if self.text(ty.name) == ARRAY
            && let [element, length] = ty.generics.as_slice()
            && let Some(n) = length.count
        {
            out.push_str(&format!(
                "[{}; {n}]",
                self.ty_counted(element, lifetimes, count)
            ));
            return out;
        }
        // **A struct of views in a keeper that drops entries is held**
        // ([ADR-283](../../docs/specification/adr/adr-283.md) D14): beside a
        // handle on each buffer it points into, and read through them.
        if let Some(width) = *self.holding.borrow()
            && self.borrowing.contains(&ty.name)
            && ty.generics.is_empty()
        {
            out.push_str(&format!(
                "nikaia_std::tether::Holding<__Views_{}, {width}>",
                self.name(ty.name)
            ));
            return out;
        }
        out.push_str(&self.written_name(self.text(ty.name), count));

        let mut params: Vec<String> = ty
            .generics
            .iter()
            .map(|g| self.ty_counted(g, lifetimes, count))
            .collect();
        // A struct that holds a view carries the input lifetime with it.
        if self.borrows(ty.name) {
            params.insert(0, lifetimes.params.to_string());
        }
        if !params.is_empty() {
            out.push_str(&format!("<{}>", params.join(", ")));
        }
        // **A type with a cleanup that can pause is its value in a `Cleaned`**
        // (ADR-297 D1), whose `Drop` parks the cleanup where the value dies.
        if self.cleaned_types.contains(self.text(ty.name)) {
            return format!("{}<{out}>", self.cleaned());
        }

        out
    }

    /// The queues a settle point drains: at `user_parallelism = yes` only the
    /// one whose work may cross threads, so the settle point may too
    /// (ADR-297 D2).
    fn settle_kind(&self) -> &'static str {
        match self.build.overlaps_user_code() {
            true => "nikaia_std::cleanup::Sent",
            false => "nikaia_std::cleanup::Local",
        }
    }

    /// The wrapper a value with a cleanup is held in: the one whose parked work
    /// may move with its task where tasks run on threads (ADR-297 D1).
    fn cleaned(&self) -> &'static str {
        match self.build.overlaps_user_code() {
            true => "nikaia_std::cleanup::CleanedSend",
            false => "nikaia_std::cleanup::Cleaned",
        }
    }

    // --- Statements and expressions ---

    /// `if c { … } else { … }`, where `tail` says what the whole thing is in
    /// the position of ([`Tail`]).
    ///
    /// It decides what the last statement of each branch means, and the branches
    /// are in the same position the `if` is: as the tail of a function body they
    /// carry the function's value, as `let x = if c { a } else { b }` they carry
    /// the `let`'s, and as a statement they carry nothing.
    #[allow(clippy::too_many_arguments)]
    fn if_expr(
        &self,
        out: &mut Out,
        cond: &Expr,
        then_branch: &Block,
        else_branch: Option<&Block>,
        depth: usize,
        flow: Flow<'_>,
        tail: Tail,
    ) -> Result<()> {
        out.push("if ");
        self.expr(out, cond, depth, flow)?;
        out.push(" ");
        self.block(out, then_branch, depth, flow, tail)?;
        if let Some(block) = else_branch {
            out.push(" else ");
            // **`else if` and not `else { if … }`**
            // ([ADR-132](../../docs/specification/adr/adr-132.md) D2), so the
            // generated Rust reads as the source does. The condition is the
            // block holding **one** `if` and nothing else, which is exactly what
            // the parser makes of an `else if` - and an `else` whose block holds
            // an `if` *and* other statements stays a block, because that is what
            // the author wrote.
            //
            // It is the chain's depth this takes away rather than a brace: three
            // links nested three deep is a line the reader has to unwind, and
            // Part III C.1's rule is about what a reader of the generated file
            // meets.
            if let [only] = block.stmts.as_slice()
                && let Stmt::Expr(Expr::If {
                    cond,
                    then_branch,
                    else_branch,
                }) = &only.node
            {
                // **The `if` is a statement of the `else` block**, and the
                // checker recorded what it decided about the condition under
                // that statement: a lent argument written under the outer
                // one's was a `&` missing - `else if !has(r.items, x)` handed
                // a `Vec` where a slice was declared (found moving `touch`'s
                // walk into Nikaia, #125).
                return self.if_expr(
                    out,
                    cond,
                    then_branch,
                    else_branch.as_ref(),
                    depth,
                    flow.at(only.span.at()),
                    tail,
                );
            }
            self.block(out, block, depth, flow, tail)?;
        }
        Ok(())
    }

    /// A cast's operand, through the view a `for` binding is
    /// ([ADR-182](../../docs/specification/adr/adr-182.md) D1).
    ///
    /// **`num::value` and not a `*`**, which is `nikaia_std::index::at`'s own
    /// reasoning one construct over: for a number that is already a number it
    /// is the identity, so it cannot be written onto the wrong operand — and a
    /// `*` written onto one would be a `rustc` error about the generated file,
    /// which is the very thing this closes.
    ///
    /// **Inside the checked conversion and not around it**, because a
    /// narrowing cast over a lent binding is still a narrowing cast: `i8::from`
    /// of a `&i64` does not exist either, and the abort ADR-285 D12 puts there
    /// is not a thing a view may skip.
    fn operand(
        &self,
        out: &mut Out,
        expr: &Expr,
        viewed: bool,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        if !viewed {
            return self.expr(out, expr, depth, flow);
        }
        out.push("nikaia_std::num::value(");
        self.expr(out, expr, depth, flow)?;
        out.push(")");
        Ok(())
    }

    fn block(
        &self,
        out: &mut Out,
        block: &Block,
        depth: usize,
        flow: Flow<'_>,
        tail: Tail,
    ) -> Result<()> {
        self.block_opening_with(out, block, depth, flow, tail, None)
    }

    /// The same, with one line written before the statements.
    ///
    /// One caller: a `for` whose step can fail opens its body by unwrapping the
    /// step (ADR-025 D7). It is a parameter rather than a wrapping block
    /// because an extra brace level in the emitted Rust is a level the source
    /// map has to explain and the reader has to skip.
    fn block_opening_with(
        &self,
        out: &mut Out,
        block: &Block,
        depth: usize,
        flow: Flow<'_>,
        tail: Tail,
        opening: Option<&str>,
    ) -> Result<()> {
        // **A block where a value with a cleanup dies is settled where it
        // ends** ([ADR-297](../../docs/specification/adr/adr-297.md) D2): the
        // value dies at its closing brace and parks its cleanup, and the
        // settle point right after runs it. Not inside a closure that cannot
        // pause - there the function's own settle point runs it.
        let settles = block
            .stmts
            .iter()
            .filter_map(|stmt| self.settle_lets.get(&stmt.span.at()).copied())
            .reduce(|a, b| a || b);
        // **A failure is never settled away**: where a cleanup that can fail
        // has no channel to fail into here - a lambda's body, a branch that is
        // not wrapped, a guarded half - the value stays parked for the next
        // settle point out, which has one (ADR-297 D4).
        let propagates = flow.throws && !flow.caught;
        if let Some(fails) = settles
            && !flow.in_lambda
            && (!fails || propagates)
        {
            let inner =
                Out::scratch(|inner| self.block_plain(inner, block, depth, flow, tail, opening))?;
            let kind = self.settle_kind();
            let settle = match fails {
                true => format!(
                    "nikaia_std::cleanup::settle_at::<{kind}>(&{:?}).await?",
                    flow.origin
                ),
                false => format!("nikaia_std::cleanup::settle_quietly::<{kind}>().await"),
            };
            out.push("{ let __nikaia_block = ");
            out.append(inner);
            out.push(&format!("; {settle}; __nikaia_block }}"));
            return Ok(());
        }
        self.block_plain(out, block, depth, flow, tail, opening)
    }

    /// A block as it is written, with no settle point after it.
    fn block_plain(
        &self,
        out: &mut Out,
        block: &Block,
        depth: usize,
        flow: Flow<'_>,
        tail: Tail,
        opening: Option<&str>,
    ) -> Result<()> {
        if block.stmts.is_empty() {
            match opening {
                Some(line) => {
                    out.push(&format!("{{ {line} }}"));
                    return Ok(());
                }
                None => {
                    out.push("{ }");
                    return Ok(());
                }
            }
        }

        // A one-statement block stays on its line. Most action blocks are one
        // expression, and a grammar reads better when its actions do not push
        // the pattern three lines apart. Whether it fits is decided by
        // rendering it, not by guessing from the shape.
        if block.stmts.len() == 1 && opening.is_none() {
            let only = block.stmts.first().expect("one statement");
            let rendered = Out::scratch(|scratch| {
                scratch.from(&only.span, |s| {
                    self.stmt(s, &only.node, &only.span, depth, tail, flow)
                })
            })?;
            if !rendered.buf.contains('\n') {
                out.push("{ ");
                out.append(rendered);
                out.push(" }");
                return Ok(());
            }
        }

        let pad = "    ".repeat(depth);
        let inner_pad = "    ".repeat(depth + 1);

        out.push("{\n");
        if let Some(line) = opening {
            out.push(&inner_pad);
            out.push(line);
            out.push("\n");
        }
        let last = block.stmts.len() - 1;
        let mut i = 0;
        while i < block.stmts.len() {
            // **Statements run in the order they are written**
            // ([ADR-292](../../../docs/specification/adr/adr-292.md) D1), and no
            // analysis stands between the source and the schedule. A program
            // that wants overlap writes `overlap { … }` and has the claim
            // checked (D3).

            let stmt = &block.stmts[i];
            // **An empty list made `n` long at once is `vec![v; n]`**
            // ([ADR-270](../../docs/specification/adr/adr-270.md) D8 step 1):
            // `let mut xs: Vec[T] = []` followed by `xs.resize(n, v)` is one
            // allocation of a known length - zeroed by the system for zeros -
            // which is what Rust writes, and what lets the optimiser drop the
            // bounds checks of every read below that it can see is in range.
            if i < last
                && let Some((fill, length)) = self.filled_at_once(stmt, &block.stmts[i + 1])
            {
                let declared = Out::scratch(|scratch| {
                    scratch.from(&stmt.span, |s| {
                        self.stmt(s, &stmt.node, &stmt.span, depth + 1, tail.at(i, last), flow)
                    })
                })?;
                if let Some(head) = declared.buf.strip_suffix("vec![];") {
                    let next = &block.stmts[i + 1];
                    let flow = flow.at(next.span.at());
                    let value = Out::scratch(|v| self.expr(v, fill, depth + 1, flow))?;
                    let count = Out::scratch(|c| match only_literals(length) {
                        true => self.expr(c, length, depth + 1, flow.inferred()),
                        false => {
                            c.push("nikaia_std::count::of(");
                            self.expr(c, length, depth + 1, flow)?;
                            c.push(")");
                            Ok(())
                        }
                    })?;
                    out.push(&inner_pad);
                    out.from(&stmt.span, |o| {
                        o.push(&format!("{head}vec![{}; {}];", value.buf, count.buf));
                        Ok(())
                    })?;
                    out.push("\n");
                    i += 2;
                    continue;
                }
            }
            out.push(&inner_pad);
            out.from(&stmt.span, |out| {
                self.stmt(
                    out,
                    &stmt.node,
                    &stmt.span,
                    depth + 1,
                    tail.at(i, last),
                    flow,
                )
            })?;
            out.push("\n");
            i += 1;
        }
        out.push(&pad);
        out.push("}");
        Ok(())
    }

    /// Whether `expr` is never negative here: a non-negative literal, a
    /// counter of [`Emitter::nonnegative`], or a sum or product of those.
    fn never_negative(&self, flow: Flow<'_>, expr: &Expr) -> bool {
        match expr {
            Expr::LitInt { negative, .. } => !negative,
            Expr::Variable(name) => self
                .nonnegative
                .borrow()
                .get(flow.function)
                .is_some_and(|set| set.contains(name)),
            Expr::Binary {
                op: BinaryOp::Add | BinaryOp::Mul,
                lhs,
                rhs,
                ..
            } => self.never_negative(flow, lhs) && self.never_negative(flow, rhs),
            _ => false,
        }
    }

    /// `i < xs.len()` for a counter `i` that is never negative, written as
    /// `(i as usize) < xs.len()`; `None` where the comparison is not that.
    fn against_a_length(
        &self,
        out: &mut Out,
        op: &BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<Option<()>> {
        let a_length = |e: &Expr| {
            matches!(e, Expr::MethodCall { method, args, .. }
                if args.is_empty() && self.text(*method) == "len")
        };
        let (counter, length, counter_first) = match (lhs, rhs) {
            (c, l) if a_length(l) && !a_length(c) && self.never_negative(flow, c) => (c, l, true),
            (l, c) if a_length(l) && !a_length(c) && self.never_negative(flow, c) => (c, l, false),
            _ => return Ok(None),
        };
        let written = Out::scratch(|s| self.nested(s, length, u8::MAX, depth, flow))?;
        let Some(bare) = written.buf.strip_suffix(" as i64").or_else(|| {
            written
                .buf
                .strip_prefix('(')
                .and_then(|b| b.strip_suffix(" as i64)"))
        }) else {
            return Ok(None);
        };
        // **Empty or not is asked as such**: `xs.len() > 0` is `clippy`'s
        // `len_zero` about a file nobody wrote, and `!xs.is_empty()` is what
        // a person writes.
        if let (
            Expr::LitInt {
                value,
                negative: false,
            },
            Some(receiver),
        ) = (counter, bare.strip_suffix(".len()"))
        {
            // The comparison as `length op n`.
            let op = match (counter_first, op) {
                (false, op) => *op,
                (true, BinaryOp::Lt) => BinaryOp::Gt,
                (true, BinaryOp::Le) => BinaryOp::Ge,
                (true, BinaryOp::Gt) => BinaryOp::Lt,
                (true, BinaryOp::Ge) => BinaryOp::Le,
                (true, op) => *op,
            };
            let empty = match (op, *value) {
                (BinaryOp::Eq | BinaryOp::Le, 0) | (BinaryOp::Lt, 1) => Some(true),
                (BinaryOp::Ne | BinaryOp::Gt, 0) | (BinaryOp::Ge, 1) => Some(false),
                _ => None,
            };
            if let Some(empty) = empty {
                let not = if empty { "" } else { "!" };
                out.push(&format!("{not}{receiver}.is_empty()"));
                return Ok(Some(()));
            }
        }
        // A literal is written as it is: Rust gives it the length's type, and
        // a cast on it is `clippy`'s `unnecessary_cast` about a file nobody
        // wrote.
        let counter = match counter {
            Expr::LitInt {
                value,
                negative: false,
            } => value.to_string(),
            _ => {
                let counted = Out::scratch(|s| self.expr(s, counter, depth, flow))?;
                format!("(({}) as usize)", counted.buf)
            }
        };
        let (left, right) = match counter_first {
            true => (counter, bare.to_string()),
            false => (bare.to_string(), counter),
        };
        out.push(&format!("{left} {} {right}", binary_op(*op)));
        Ok(Some(()))
    }

    /// `let mut xs = []` and, right after it, `xs.resize(n, v)` with `std`'s
    /// `resize`: the value and the length, for `vec![v; n]`.
    fn filled_at_once<'s>(
        &self,
        declared: &'s Spanned<Stmt>,
        next: &'s Spanned<Stmt>,
    ) -> Option<(&'s Expr, &'s Expr)> {
        let Stmt::Let {
            names,
            mutable: true,
            value: Expr::ListLit { items, .. },
            ..
        } = &declared.node
        else {
            return None;
        };
        let ([name], true) = (names.as_slice(), items.is_empty()) else {
            return None;
        };
        let Stmt::Expr(Expr::MethodCall {
            receiver,
            method,
            args,
            config,
        }) = &next.node
        else {
            return None;
        };
        let filled = matches!(&**receiver, Expr::Variable(r) if r == name)
            && self.text(*method) == "resize"
            && args.len() == 2
            && config.is_empty()
            && self.own_contracts.candidates("resize").is_empty()
            && !args.iter().any(|a| mentions_symbol(a, *name));
        filled.then(|| (&args[1], &args[0]))
    }

    /// Whether a **branch of an `overlap`** can pause
    /// ([ADR-292](../../../docs/specification/adr/adr-292.md) D6).
    ///
    /// A walk and not a lookup, because the question is about *this statement*
    /// and the answers the ledger and the checker give are keyed one by callee
    /// and one by statement. Both are consulted here: a free call by the key it
    /// resolves to, a method call by what the checker said about the statement
    /// it stands in - which is the same pair of sources `call` and the method
    /// arm use, asked ahead of time.
    ///
    /// It was ADR-292's, for the pair the compiler chose to overlap; D1
    /// withdrew that and D6 asks the same question about a branch the
    /// programmer chose.
    fn branch_pauses(&self, stmt: &Spanned<Stmt>, flow: Flow<'_>) -> bool {
        let Stmt::Expr(value) = &stmt.node else {
            return false;
        };
        let flow = flow.at(stmt.span.at());
        let mut pauses = false;
        visit_expr(value, &mut |expr| match expr {
            Expr::Call { func, .. } => pauses |= self.pausing_key(func).is_some(),
            Expr::MethodCall { method, .. } | Expr::SafeMethod { method, .. } => {
                pauses |= self.method_pauses(flow, *method)
            }
            _ => {}
        });
        pauses
    }

    /// `tail` says what this statement is in the position of ([`Tail`]):
    /// Nikaia's blocks are expressions (Part I, 3.1), so a block's last
    /// statement is its value and keeps no semicolon - and where that value is
    /// the *function's*, a `return x` is written as `x`.
    /// **A `?.` view out of a temporary is held for the rest of the block**
    /// ([ADR-278](../../docs/specification/adr/adr-278.md) D21).
    ///
    /// `let a = find(1)?.label() ?? "none"` hands back a view of the `User`
    /// `find(1)` made, and that value dies at the `;` - `rustc`'s *temporary
    /// value dropped while borrowed* about a file nobody wrote. So the receiver
    /// is bound first, `let __nikaia_held_0 = find(1);`, and the reach reads
    /// the binding. Nothing is copied and nothing runs that did not run.
    ///
    /// **Only where binding it first changes nothing about what runs** (D2):
    /// not on the lazy side of `??`, `&&` or `||`, not in a `match` arm, not
    /// after a call earlier in the same statement, and not where the view is
    /// the block's value, which would outlive the binding. There the program is
    /// refused, with the `let` it can write.
    fn hold_temporaries(
        &self,
        out: &mut Out,
        stmt: &Stmt,
        span: &Span,
        depth: usize,
        tail: Tail,
        flow: Flow<'_>,
    ) -> Result<()> {
        if self.held_reaches.is_empty() {
            return Ok(());
        }
        let (value, holdable) = match stmt {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => (value, true),
            Stmt::Expr(value) => (value, tail == Tail::Statement),
            Stmt::Return(Some(value)) => (value, false),
            _ => return Ok(()),
        };
        let mut found = Vec::new();
        let mut called = false;
        self.held_in(value, span.at(), false, &mut called, &mut found);
        for (receiver, safe) in found {
            if !safe || !holdable {
                return Err(refused_at!(
                    span.at(),
                    "This `?.` returns a view of a value that's gone at the end of this \
                     line. Put the value in a `let` on the line before, and use `?.` on \
                     that name."
                ));
            }
            let name = format!("__nikaia_held_{}", self.held.borrow().len());
            out.push(&format!("let {name} = "));
            self.expr(out, receiver, depth, flow)?;
            out.push(&format!(";\n{}", "    ".repeat(depth)));
            self.held
                .borrow_mut()
                .insert(receiver as *const Expr as usize, name);
        }
        Ok(())
    }

    /// The receivers of held reaches in `expr`, in the order they run, each
    /// with whether holding it first is safe: not lazy, and no call ran before
    /// it in this statement.
    fn held_in<'e>(
        &self,
        expr: &'e Expr,
        statement: usize,
        lazy: bool,
        called: &mut bool,
        found: &mut Vec<(&'e Expr, bool)>,
    ) {
        match expr {
            Expr::SafeMethod {
                receiver,
                method,
                args,
                ..
            } => {
                let held = self
                    .held_reaches
                    .contains(&(statement, self.text(*method).to_string()))
                    && !matches!(receiver.as_ref(), Expr::Variable(_));
                if held {
                    found.push((receiver, !lazy && !*called));
                }
                self.held_in(receiver, statement, lazy, called, found);
                for arg in args {
                    self.held_in(arg, statement, lazy, called, found);
                }
                *called = true;
            }
            Expr::Call { func, args, .. } => {
                self.held_in(func, statement, lazy, called, found);
                for arg in args {
                    self.held_in(arg, statement, lazy, called, found);
                }
                *called = true;
            }
            Expr::MethodCall { receiver, args, .. } => {
                self.held_in(receiver, statement, lazy, called, found);
                for arg in args {
                    self.held_in(arg, statement, lazy, called, found);
                }
                *called = true;
            }
            Expr::Coalesce { value, fallback } => {
                self.held_in(value, statement, lazy, called, found);
                self.held_in(fallback, statement, true, called, found);
            }
            Expr::Binary { lhs, rhs, op, .. } => {
                self.held_in(lhs, statement, lazy, called, found);
                let short = matches!(op, crate::ast::BinaryOp::And | crate::ast::BinaryOp::Or);
                self.held_in(rhs, statement, lazy || short, called, found);
            }
            Expr::Match { value, arms } => {
                self.held_in(value, statement, lazy, called, found);
                for arm in arms {
                    self.held_in(&arm.body, statement, true, called, found);
                }
            }
            Expr::If { cond, .. } => self.held_in(cond, statement, lazy, called, found),
            Expr::Field { base, .. } | Expr::SafeField { base, .. } => {
                self.held_in(base, statement, lazy, called, found)
            }
            Expr::Index { base, index } => {
                self.held_in(base, statement, lazy, called, found);
                self.held_in(index, statement, lazy, called, found);
            }
            Expr::Unary { expr, .. } | Expr::Try(expr) | Expr::Cast { expr, .. } => {
                self.held_in(expr, statement, lazy, called, found)
            }
            Expr::Tuple(parts) | Expr::ListLit { items: parts, .. } => {
                for part in parts {
                    self.held_in(part, statement, lazy, called, found);
                }
            }
            Expr::StructLit { fields, .. } => {
                for value in fields.iter().filter_map(|f| f.value.as_ref()) {
                    self.held_in(value, statement, lazy, called, found);
                }
            }
            // A block, a lambda, a string's holes: statements of their own, or
            // code this walk cannot hold anything for.
            _ => {}
        }
    }

    fn stmt(
        &self,
        out: &mut Out,
        stmt: &Stmt,
        span: &Span,
        depth: usize,
        tail: Tail,
        flow: Flow<'_>,
    ) -> Result<()> {
        // Which statement this is, for the one thing that has to look itself up
        // by it: a method call that can fail (ADR-023 D8). Shadowed rather than
        // passed on separately, so that everything emitted from here - an
        // expression, a nested block, a `catch` handler - sees the statement it
        // is actually in.
        let flow = flow.at(span.at());
        self.write_keep_prelude(out, flow.function, span.at(), depth);
        self.hold_temporaries(out, stmt, span, depth, tail, flow)?;
        match stmt {
            Stmt::Let {
                names,
                mutable,
                ty,
                value,
            } => {
                let mutable = if *mutable { "mut " } else { "" };
                // **A tuple of names is Rust's own tuple pattern**
                // ([ADR-291](../../docs/specification/adr/adr-291.md)): the
                // language below takes a tuple apart by position exactly as
                // this one does, so there is nothing to translate. No count is
                // looked up and no annotation is written, because a hull's
                // count is decided per **value** and a destructure names none -
                // which is the same answer `contracts::sharing` gives, and the
                // two have to agree.
                if let [_, _, ..] = names.as_slice() {
                    let bound: Vec<String> = names
                        .iter()
                        .map(|n| escaped(self.text(*n)).into_owned())
                        .collect();
                    out.push(&format!("let {mutable}({}) = ", bound.join(", ")));
                    let (before, after) =
                        Self::around_value(self.nullable_sites.get(&span.at()).copied(), value);
                    out.push(before);
                    // **Out of a place, the tuple is lent** (#465), as one
                    // name's `let` over a place is.
                    if mutable.is_empty() && self.lent_lets.contains(&span.at()) {
                        out.push("&");
                    }
                    self.expr(out, value, depth, flow)?;
                    out.push(after);
                    out.push(";");
                    // A number among its parts is read out of its view.
                    for name in names {
                        if self
                            .copied_tuple_parts
                            .contains(&(span.at(), self.text(*name).to_string()))
                        {
                            let name = escaped(self.text(*name));
                            out.push(&format!(" let {name} = nikaia_std::num::value({name});"));
                        }
                    }
                    return Ok(());
                }
                let bound = self.text(names[0]);
                let count = self.count_at(flow.function, bound);
                // A hull written by a call inside this value looks its count up
                // by the name being bound (ADR-281 D2), and an expression has
                // none of its own.
                let flow = flow.binding(bound);
                let plan = self.keep_plan(flow.function);
                // **A buffer whose views outlive this scope goes into a keep**
                // (ADR-283 D9), and the binding is where it now lives.
                let put = plan.and_then(|p| p.puts.get(&span.at())).copied();
                // **A binding a task takes with it is packed with the task's
                // keep** (D3), and every use of it reads through the handle.
                let packed = plan
                    .and_then(|p| p.tethered.get(bound))
                    .filter(|at| **at == span.at())
                    .is_some();
                // **A keeper that drops entries holds each text view with its
                // own handle** (D4): `ref String` is `Held` in its type.
                let holding = plan
                    .filter(|p| p.element_keepers.contains(bound))
                    .map(|p| p.widths.get(bound).copied().unwrap_or(1));
                *self.holding.borrow_mut() = holding;
                let annotation = match ty {
                    Some(_) if put.is_some() || packed => String::new(),
                    Some(ty) => format!(": {}", self.ty_counted(ty, Lifetimes::ELIDED, count)),
                    // **A number its uses typed is written with the type**
                    // ([ADR-285](../../docs/specification/adr/adr-285.md) D27):
                    // this compiler decided it, and the language below's own
                    // inference is not asked a second time - which is what left
                    // `v[i].push(x)` over a bare `let i = 0` without an answer.
                    None => match self.number_lets.get(&span.at()) {
                        Some(ty) => format!(": {ty}"),
                        None => String::new(),
                    },
                };
                *self.holding.borrow_mut() = None;
                if packed {
                    let Some(held) = self.tethered_type(ty.as_ref(), value) else {
                        return Err(refused_at!(
                            span.at(),
                            "`{bound}` is passed to a task and keeps data alive, so it needs \
                             its type written out: `let {bound}: … = …`."
                        ));
                    };
                    // **Held by the task's keep** (ADR-218 D4): the value is
                    // built first, and every view in it is found again in the
                    // keep - which is where the buffers it was cut from were
                    // put - over a lifetime the `Holding` alone decides.
                    let wrapper = self.tether_wrapper(&held);
                    out.push(&format!(
                        "let {mutable}{} = {{ let __packed = ",
                        escaped(bound)
                    ));
                    self.expr(out, value, depth, flow)?;
                    out.push(&format!(
                        "; nikaia_std::tether::Holding::<{wrapper}, 1>::new([std::sync::Arc::clone(&{KEEP_TASK})], \
                         move |keeps| nikaia_std::tether::Rebase::rebase(__packed, &keeps)) }};"
                    ));
                    return Ok(());
                }
                if let Some(keep) = put {
                    // A buffer whose views are held one by one is one they
                    // are found in again (ADR-283 D14) - and so is one a task's
                    // `Holding` finds them in, whether the task's keep is this
                    // function's or a caller's handed in (ADR-218 D4).
                    let put = match keep {
                        crate::contracts::keep::KeepAt::Element(_)
                        | crate::contracts::keep::KeepAt::Task
                        | crate::contracts::keep::KeepAt::Param => "put_viewed",
                        _ => "put",
                    };
                    out.push(&format!(
                        "let {mutable}{} = {}.{put}(",
                        escaped(bound),
                        Self::keep_expr(keep, false)
                    ));
                    self.expr(out, value, depth, flow)?;
                    out.push(");");
                    return Ok(());
                }
                // **The annotation is no longer the constructor**
                // ([ADR-281](../../../docs/specification/adr/adr-281.md) D2). What
                // used to be allocated here, out of an answer the checker had to
                // compute and hand over, is now written where it happens - and the
                // two positions that could carry one stopped being a list.
                //
                // Part I 2.3: a plain value standing in a nullable slot. That hull
                // stays the compiler's, because it is one a program cannot observe
                // - the same value, possibly absent (ADR-281 D2's own line).
                let (before, after) =
                    Self::around_value(self.nullable_sites.get(&span.at()).copied(), value);
                out.push(&format!("let {mutable}{}{annotation} = ", escaped(bound)));
                out.push(before);
                // **A `let` over a place is a view of it**
                // ([ADR-094](../../docs/specification/adr/adr-094.md) D4).
                // `let s = totals.stations[name]` and `let name = config.name`
                // were never moves: the language below refuses to move a value
                // out of a container or out of a borrowed field, so what the
                // line meant is the `&` it would otherwise have been refused
                // for not writing.
                //
                // **`let y = x` over a whole variable stays a move** — it is a
                // rename — and a `let` whose initialiser is a call or a literal
                // owns what it is given. So this is narrower than `for`'s rule
                // by exactly one shape, and that shape is the reason both are
                // written here rather than in one test.
                if self.lent_lets.contains(&span.at()) {
                    out.push("&");
                }
                // **A `let` whose annotation is a number and whose value is a
                // `for` binding reads the number through the view**
                // ([ADR-182](../../docs/specification/adr/adr-182.md) D5):
                // `let q: i64 = m` over a lent `m` is a
                // `&i64` where an `i64` was declared, and *consider
                // dereferencing the borrow* is a way out the source cannot
                // take, because there is no borrow in it.
                let viewed = match value {
                    Expr::Variable(name) => self
                        .viewed_numbers
                        .contains(&(flow.statement, self.text(*name).to_string())),
                    _ => false,
                };
                self.operand(out, value, viewed, depth, flow)?;
                out.push(after);
                out.push(";");
            }
            // **The folded value is what is written**, not the expression that
            // folded ([ADR-287](../../docs/specification/adr/adr-287.md) D4).
            // That is the visible half of the demand: `const LIMIT = 4 * 1024`
            // reaches the language below as `4096`, so a reader of the generated
            // file can see that the arithmetic did not survive into the program.
            //
            // The type comes from the checker, because Rust's `const` takes one
            // and this has no types (ADR-288). A `comptime` with no entry there was
            // refused as `NK1127` and never arrives.
            Stmt::Comptime { name, .. } => {
                let bound = self.text(*name);
                let Some((below, written)) = self.comptime_values.get(&span.at()).cloned() else {
                    return Err(refused_at!(
                        span.at(),
                        "`{bound}` has no value computed at build time. This is a compiler \
                         bug: the check should have reported it first."
                    ));
                };
                out.push(&format!("const {bound}: {below} = {written};"));
            }

            // **A write through the brackets is not an index**
            // ([ADR-080](../../docs/specification/adr/adr-080.md) D2). Rust's
            // `Index` for a map is over whatever the key *borrows* as, so
            // `scores["Player1"] = 100` left the key type unpinned and the
            // program failed with `E0282` in a message naming `TrustedMap` and
            // a type parameter `K` - three spellings the program never wrote.
            // `index::set` is `insert` for a map and an indexed assignment for a
            // sequence, chosen by the language below on the container's type,
            // because this emitter does not know it (ADR-296 D17).
            //
            // Only the plain `=`: a compound `m[k] += 1` reads the slot as well
            // as writing it, so it is an `Index` either way and a second rule
            // would be needed to say what reading an absent key means. That is
            // a question of its own and not this one.
            Stmt::Assign {
                target: box_index @ Expr::Index { .. },
                op: None,
                value,
            } => {
                let Expr::Index { base, index } = box_index else {
                    unreachable!("matched as an index")
                };
                let (before, after) =
                    Self::around_value(self.nullable_sites.get(&span.at()).copied(), value);
                // **The value first, and then the write**
                // ([ADR-293](../../../docs/specification/adr/adr-293.md) D2).
                // Since D1 a read is `index::get(&m, …)`, so the written-out
                // counter D2 hands a reader — `m[k] = (m[k] ?? 0) + 1` — has a
                // `&m` inside the arguments of a `set(&mut m, …)`, which is
                // *cannot borrow as immutable because it is also borrowed as
                // mutable* about a file nobody wrote
                // ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
                // A `let` ends the read before the write begins, which is what
                // a reader would write by hand.
                out.push(&format!("{{ let {STORED} = "));
                out.push(before);
                self.expr(out, value, depth, flow)?;
                out.push(after);
                // **A `mut` parameter is already a `&mut`** (ADR-094 D3), so
                // it is lent on again rather than borrowed a second time:
                // `set(&mut out, …)` asks for the binding itself to be `mut`,
                // which a parameter is not ([ADR-270](../../docs/specification/adr/adr-270.md)
                // D8 step 1).
                let lent_on = matches!(&**base, Expr::Variable(name)
                    if self.changes_in_place(flow, self.text(*name)));
                // **A position proved inside is written without its check**
                // (ADR-306 D5), as it is read.
                if self
                    .proven_indices
                    .contains(&crate::check::value_node(base))
                    && self.map_key(span.at(), index, true).is_none()
                {
                    out.push(match lent_on {
                        true => "; unsafe { nikaia_std::proven::write(&mut *",
                        false => "; unsafe { nikaia_std::proven::write(&mut ",
                    });
                    self.expr(out, base, depth, flow.place())?;
                    out.push(", (");
                    match only_literals(index) {
                        true => self.index_expr(out, index, depth, flow.inferred())?,
                        false => self.index_expr(out, index, depth, flow)?,
                    }
                    out.push(&format!(") as usize, {STORED}) }}; }}"));
                    return Ok(());
                }
                out.push(match lent_on {
                    true => "; nikaia_std::index::set(&mut *",
                    false => "; nikaia_std::index::set(&mut ",
                });
                // **The container written into is a place** (0.0.238):
                // `d[i][j] = v` writes into the list `d[i]`, and read as
                // `*index::get(&d, …)` that list is behind a shared reference -
                // `rustc`'s *cannot borrow data in a `&` reference as mutable*
                // about a file nobody wrote. As a place it is `d[at(i)]`, which
                // the language below lends mutably.
                self.expr(out, base, depth, flow.place())?;
                // **A key the map keeps goes in as it is** (ADR-293 D27): it is
                // not a position, and `at` is for positions.
                match self.map_key(span.at(), index, true) {
                    Some(form) => {
                        out.push(", ");
                        self.key(out, form, index, depth, flow)?;
                        out.push(&format!(", {STORED}); }}"));
                    }
                    None if self.never_negative(flow, index) => {
                        out.push(", (");
                        self.index_expr(out, index, depth, flow)?;
                        out.push(&format!(") as usize, {STORED}); }}"));
                    }
                    None => {
                        out.push(", nikaia_std::index::at(");
                        self.index_expr(out, index, depth, flow)?;
                        out.push(&format!("), {STORED}); }}"));
                    }
                }
            }
            // **A field of a held struct is written through its handle**
            // ([ADR-283](../../docs/specification/adr/adr-283.md) D17): the
            // value first, as everywhere, and then a closure that is handed the
            // struct and its keeps - where a view made outside is found again
            // in them, or copied into them, before it is stored.
            Stmt::Assign {
                target:
                    Expr::Field {
                        base: element,
                        name: field,
                    },
                op,
                value,
            } if self.held_element(flow, element).is_some() => {
                let (keeper, index) = self
                    .held_element(flow, element)
                    .expect("matched as a held element");
                let (before, after) =
                    Self::around_value(self.nullable_sites.get(&span.at()).copied(), value);
                out.push(&format!("{{ let {STORED} = "));
                out.push(before);
                self.expr(out, value, depth, flow)?;
                out.push(after);
                out.push("; ");
                self.expr(out, keeper, depth, flow.place())?;
                match only_literals(index) {
                    true => {
                        out.push("[");
                        self.index_expr(out, index, depth, flow.inferred())?;
                        out.push("]");
                    }
                    false => {
                        out.push("[nikaia_std::index::at(");
                        self.index_expr(out, index, depth, flow)?;
                        out.push(")]");
                    }
                }
                let (keeps, stored) = match self.held_field_carries_view(flow, keeper, *field) {
                    true => (
                        "keeps",
                        format!("nikaia_std::tether::Rebase::rebase({STORED}, &keeps)"),
                    ),
                    false => ("_", STORED.to_string()),
                };
                let assign = match op {
                    Some(op) => format!("{}=", binary_op(*op)),
                    None => "=".to_string(),
                };
                out.push(&format!(
                    ".with_mut(|held, {keeps}| held.{} {assign} {stored}); }}",
                    self.name(*field)
                ));
            }
            // **`xs[i] += d` at a position proved inside** (#387): the slot
            // is a number, so it is read once and written back, neither
            // without its check - the value first, then the position, as the
            // plain write runs them (ADR-293 D2). Where the operation is
            // proved too it is the operation alone.
            Stmt::Assign {
                target: Expr::Index { base, index },
                op: Some(op),
                value,
            } if self.copied_slots.contains(&span.at())
                && self
                    .proven_indices
                    .contains(&crate::check::value_node(base))
                && self.map_key(span.at(), index, true).is_none() =>
            {
                let lent_on = matches!(&**base, Expr::Variable(name)
                    if self.changes_in_place(flow, self.text(*name)));
                out.push(&format!("{{ let {STORED} = "));
                self.expr(out, value, depth, flow)?;
                out.push("; let __nikaia_at = (");
                match only_literals(index) {
                    true => self.index_expr(out, index, depth, flow.inferred())?,
                    false => self.index_expr(out, index, depth, flow)?,
                }
                out.push(") as usize; ");
                let read = |emitter: &Self, out: &mut Out| -> Result<()> {
                    out.push("*unsafe { nikaia_std::proven::read(&");
                    emitter.expr(out, base, depth, flow.place())?;
                    out.push(", __nikaia_at) }");
                    Ok(())
                };
                out.push(&format!("let {STORED} = "));
                match (self.proven_arithmetic.get(&span.at()), op) {
                    (Some(ty), BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul) => {
                        let name = match op {
                            BinaryOp::Add => "wrapping_add",
                            BinaryOp::Sub => "wrapping_sub",
                            _ => "wrapping_mul",
                        };
                        out.push(&format!("<{ty}>::{name}("));
                        read(self, out)?;
                        out.push(&format!(", {STORED})"));
                    }
                    _ => {
                        read(self, out)?;
                        out.push(&format!(" {} {STORED}", binary_op(*op)));
                    }
                }
                out.push(match lent_on {
                    true => "; unsafe { nikaia_std::proven::write(&mut *",
                    false => "; unsafe { nikaia_std::proven::write(&mut ",
                });
                self.expr(out, base, depth, flow.place())?;
                out.push(&format!(", __nikaia_at, {STORED}) }}; }}"));
            }
            // **`x += d` proved to stay inside its type** (#387) is written as
            // the operation it is, without its check, as `x = x + d` would be
            // (ADR-306 D10).
            Stmt::Assign {
                target: target @ Expr::Variable(_),
                op: Some(op @ (BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul)),
                value,
            } if self.proven_arithmetic.contains_key(&span.at()) => {
                let ty = &self.proven_arithmetic[&span.at()];
                let name = match op {
                    BinaryOp::Add => "wrapping_add",
                    BinaryOp::Sub => "wrapping_sub",
                    _ => "wrapping_mul",
                };
                self.expr(out, target, depth, flow.place())?;
                out.push(&format!(" = <{ty}>::{name}("));
                self.expr(out, target, depth, flow)?;
                out.push(", ");
                self.expr(out, value, depth, flow)?;
                out.push(");");
            }
            Stmt::Assign { target, op, value } => {
                let (before, after) =
                    Self::around_value(self.nullable_sites.get(&span.at()).copied(), value);
                self.expr(out, target, depth, flow.place())?;
                match op {
                    Some(op) => out.push(&format!(" {}= ", binary_op(*op))),
                    None => out.push(" = "),
                }
                // **A kept buffer assigned again goes into its keep too**
                // (#293): the binding is a place in the keep, and the views of
                // the old value still point into the old one.
                let reput = self
                    .keep_plan(flow.function)
                    .and_then(|p| p.reputs.get(&span.at()))
                    .copied();
                if let Some(keep) = reput {
                    let put = match keep {
                        crate::contracts::keep::KeepAt::Element(_)
                        | crate::contracts::keep::KeepAt::Task
                        | crate::contracts::keep::KeepAt::Param => "put_viewed",
                        _ => "put",
                    };
                    out.push(&format!("{}.{put}(", Self::keep_expr(keep, false)));
                }
                out.push(before);
                self.expr(out, value, depth, flow)?;
                out.push(after);
                if reput.is_some() {
                    out.push(")");
                }
                out.push(";");
            }
            // Kap 3.3. Name for name (ADR-296 D17): the language below spells
            // this the same way, so there is nothing to decide here.
            Stmt::While { cond, body } => {
                // **`while true` is emitted as `loop`**, and this is the one
                // place the lowering translates what a loop *means* rather than
                // how it is spelled.
                //
                // It is not a departure from name for name (ADR-296 D17) but the
                // decided case of it:
                // [ADR-276](../../../docs/specification/adr/adr-276.md) D1 says
                // `while true { … }` **is** this language's unconditional loop
                // and that the absence of a second spelling is a decision. Rust's
                // name for the unconditional loop is `loop`. So `loop` is the
                // translation of the program and `while true` is a transcription
                // of its letters - and the letters are what the language below
                // objects to:
                //
                //     warning: denote infinite loops with `loop { ... }`
                //
                // a warning on a line nobody wrote, which is Part III C.1's class
                // one severity down. **The alternative was an `#![allow]` in the
                // preamble**, and it is worse twice over: it silences a symptom
                // on every file this compiler will ever write, and it leaves the
                // second reason below unbuilt.
                //
                // **The second reason, which is why this is not cosmetic.** The
                // two forms do not have the same *type* below. `while true { }`
                // is `()`; `loop { }` diverges and is `!`, so
                // `fn f() -> i32 { loop { } }` compiles and the `while` form is
                // an `E0308`. [ADR-276](../../docs/specification/adr/adr-276.md)
                // wants a function that never
                // returns to stop needing an unreachable `return`, and no
                // checker change can deliver that while the lowering emits the
                // form the language below refuses. This is that prerequisite.
                //
                // **The literal only**, never a name that happens to be true:
                // the equivalence is ADR-276 D1's and it is about the written
                // form, and the polarity is the usual one - claim it where it is
                // certain and nowhere else.
                match cond {
                    Expr::LitBool(true) => out.push("loop "),
                    _ => {
                        out.push("while ");
                        self.expr(out, cond, depth, flow)?;
                        out.push(" ");
                    }
                }
                self.block(out, body, depth, flow.inside_a_loop(), Tail::Statement)?;
            }

            Stmt::For {
                bindings,
                iter,
                body,
            } => {
                // **A loop over a type's fields is not a loop below**
                // ([ADR-304](../../docs/specification/adr/adr-304.md) D5,
                // [ADR-304](../../docs/specification/adr/adr-304.md) D6): it is
                // known while the program is built, so what is emitted is the
                // block once per field — the code somebody would have written
                // by hand, with no loop and no dispatch (D5's *at run time:
                // nothing*).
                if let Some(fields) = self.unrolls_here(iter) {
                    let bound = bindings
                        .first()
                        .map(|b| self.text(*b).to_string())
                        .unwrap_or_default();
                    for field in fields {
                        let outer = self
                            .at_field
                            .replace(Some((bound.clone(), field.name.clone())));
                        let written = self.block(out, body, depth, flow, Tail::Statement);
                        *self.at_field.borrow_mut() = outer;
                        written?;
                    }
                    return Ok(());
                }
                let names = bindings
                    .iter()
                    .map(|b| self.name(*b))
                    .collect::<Vec<_>>()
                    .join(", ");
                let bound = match bindings.len() > 1 {
                    true => format!("({names})"),
                    false => names.clone(),
                };
                // **A step that pauses is a loop that gives its thread up**
                // ([ADR-172](../../docs/specification/adr/adr-172.md) D1). The
                // language below has no `for` that awaits, so the shape is the
                // one a Rust programmer writes by hand for the same thing: the
                // sequence is bound once and stepped with a `.next().await`.
                //
                // **Off the checker's set and not off the shape**, exactly as
                // the `?` below is: which sequences pause is a claim in the
                // ledger about a *type*, and this emitter has none
                // ([ADR-288](../../docs/specification/adr/adr-288.md)).
                if self.pausing_loops.contains(&span.at()) {
                    // ADR-025 D1's `?`, unchanged: a step that can fail fails
                    // the enclosing function, and the checker has already made
                    // `throws` be there (`NK2701`).
                    let unwrap = self
                        .fallible_loops
                        .contains(&span.at())
                        .then(|| format!("let {bound} = {bound}?;"));
                    let pad = "    ".repeat(depth);
                    let inner = "    ".repeat(depth + 1);
                    out.push("{\n");
                    out.push(&inner);
                    out.push(&format!("let mut {SEQUENCE} = "));
                    self.expr(out, iter, depth + 1, flow)?;
                    // A produced sequence is walked **by value**
                    // ([ADR-105](../../docs/specification/adr/adr-105.md) D2),
                    // so nothing is lent here and there is no `.iter()`.
                    out.push(";\n");
                    out.push(&inner);
                    out.push(&format!(
                        "while let Some({bound}) = {SEQUENCE}.next().await "
                    ));
                    self.block_opening_with(
                        out,
                        body,
                        depth + 1,
                        flow.inside_a_loop(),
                        Tail::Statement,
                        unwrap.as_deref(),
                    )?;
                    out.push("\n");
                    out.push(&pad);
                    out.push("}");
                    return Ok(());
                }
                if bindings.len() > 1 {
                    out.push(&format!("for ({names}) in "));
                } else {
                    out.push(&format!("for {names} in "));
                }
                // **A `for` lends** ([ADR-094](../../docs/specification/adr/adr-094.md)
                // D4). `for e in entries { … }` leaves `entries` where it was,
                // so `entries.len()` on the next line is a program rather than
                // `rustc`'s *use of moved value* about a file nobody wrote.
                // Iteration that takes the elements away is **written** —
                // `for x in xs.drain()` — because removing a name from scope is
                // the rare case and the one worth a word.
                //
                // Off the shape of the expression and not off a column: a
                // **place** is lent, and a call, a range or a literal owns what
                // it made.
                //
                // **`.iter()` and not `&`**, which is one measurement rather
                // than a preference: `entries` may already *be* a view — a
                // parameter declared `&Vec[Entry]` — and `&entries` is then a
                // `&&Vec<Entry>`, which Rust does not iterate. `.iter()` reads
                // the same through any number of references, and this emitter
                // has no types to tell the two apart with (ADR-288).
                //
                // **A name holding a sequence is not a place that lends**
                // (ADR-293 D19): it is walked by value, and a range walks a copy
                // of itself. The checker says which loops those are.
                let lends = is_a_place(iter) && !self.owned_loops.contains(&span.at());
                match iter {
                    // **A range written into the `for` stays the language
                    // below's own** (ADR-293 D18): it is walked once, where it
                    // stands, and needs to be nothing more.
                    Expr::Range { .. } => self.bare_range(out, iter, depth, flow)?,
                    // **So does a list written into it**: walked once and
                    // dropped, it is an array, which Rust walks by value -
                    // `vec![..]` there is an allocation `clippy` calls useless
                    // (found moving `contracts::locks` into Nikaia). Not an
                    // empty one, whose element type nothing would say.
                    Expr::ListLit { items, .. } if !lends && !items.is_empty() => {
                        out.push("[");
                        for (i, item) in items.iter().enumerate() {
                            if i > 0 {
                                out.push(", ");
                            }
                            self.expr(out, item, depth, flow)?;
                        }
                        out.push("]");
                    }
                    // **An element walked is a read with a postfix after it**
                    // (0.0.250): `(*get(&v, i)).iter()`, parenthesised, where
                    // `*get(&v, i).iter()` dereferenced the iterator.
                    _ if lends => self.postfix_base(out, iter, depth, flow)?,
                    _ => self.expr(out, iter, depth, flow)?,
                }
                if lends {
                    out.push(".iter()");
                    if self.reads_through_handle(flow, iter) {
                        out.push(HELD_READ);
                    }
                }
                out.push(" ");

                // ADR-025 D1: a step that can fail fails the enclosing
                // function, exactly as a written call would. This is that,
                // and it is the line a Rust programmer writes by hand. The
                // compiler has already refused the program if the function
                // does not declare `throws` (`NK2701`), so the `?` always has
                // somewhere to go.
                let unwrap = self
                    .fallible_loops
                    .contains(&span.at())
                    .then(|| format!("let {names} = {names}?;"));
                // **A number, a `bool` or a `char` is read out of the view
                // once, where the body opens** (`Checked::copied_loop_bindings`):
                // then the name is the element in every position below, and
                // not only at a cast and an annotated `let`
                // ([ADR-182](../../docs/specification/adr/adr-182.md) D1, D5).
                let copied = bindings
                    .iter()
                    .filter(|b| {
                        self.copied_loop_bindings
                            .contains(&(span.at(), self.text(**b).to_string()))
                    })
                    .map(|b| {
                        let name = self.name(*b);
                        format!("let {name} = nikaia_std::num::value({name});")
                    })
                    .chain(
                        bindings
                            .iter()
                            .filter(|b| {
                                self.copied_view_bindings
                                    .contains(&(span.at(), self.text(**b).to_string()))
                            })
                            .map(|b| {
                                let name = self.name(*b);
                                format!("let {name} = *{name};")
                            }),
                    )
                    .collect::<String>();
                let opening = match (unwrap, copied.is_empty()) {
                    (unwrap, true) => unwrap,
                    (None, false) => Some(copied),
                    (Some(unwrap), false) => Some(format!("{unwrap} {copied}")),
                };
                self.block_opening_with(
                    out,
                    body,
                    depth,
                    flow.inside_a_loop(),
                    Tail::Statement,
                    opening.as_deref(),
                )?;
            }
            // A `return` that ends a *function body* is that function's
            // value, and is written as one: `fn f() -> T { return x }` is
            // `{ x }`. In a `throws` function the caller of this wraps the tail
            // in `Ok`, so the value is emitted bare here in both cases.
            //
            // `Tail::Return` and not merely "last": the last statement of a
            // `match` arm, a `catch` handler or a block used as a value is the
            // value of the expression around it, and a `return` there leaves
            // the function past that expression. It falls through to the arm
            // below and stays a `return`.
            // Kap 3.3 and [ADR-276](../../../docs/specification/adr/adr-276.md).
            // Name for name, like `while` above and for the same reason
            // (ADR-296 D17): the language below spells both of these the same way
            // and gives them the same meaning, so the lowering is a
            // transcription - which is also what makes the construct cost
            // nothing at run time (`docs/history/break-continue-cost.md` §2).
            //
            // **No `Tail` arm**, and the semicolon stands even where the
            // statement is a block's last: a block that ends in a jump diverges,
            // and Rust reads `{ break; }` in value position as the `!` it is -
            // the same way `{ return x; }` is read there.
            //
            // **And the `in_loop` test is D6's guarantee**, not a second opinion
            // on the checker's: `NK1132` is the message a program meets, and
            // this is what makes *"the checker's walk is complete"* something
            // that holds rather than something to hope for.
            Stmt::Break | Stmt::Continue => {
                let word = match stmt {
                    Stmt::Break => "break",
                    _ => "continue",
                };
                if !flow.in_loop {
                    return Err(refused_at!(
                        flow.statement,
                        "`{word}` only works inside a loop, and there's none here."
                    ));
                }
                out.push(&format!("{word};"));
            }
            Stmt::Return(Some(value)) if tail == Tail::Return => {
                self.handed_back(out, value, span, depth, flow)?;
            }
            Stmt::Return(value) => {
                // Kap 7.1: a `throws` function returns a `Result`, so what the
                // source hands back is what goes inside the `Ok`.
                match (value, flow.throws) {
                    (Some(value), true) => {
                        out.push("return Ok(");
                        self.handed_back(out, value, span, depth, flow)?;
                        out.push(");");
                    }
                    (Some(value), false) => {
                        out.push("return ");
                        self.handed_back(out, value, span, depth, flow)?;
                        out.push(";");
                    }
                    (None, true) => out.push("return Ok(());"),
                    (None, false) => out.push("return;"),
                }
            }
            // An `if` in *statement* position is not a value, and its branches
            // are not tails. Emitting them as tails is right for
            // `let x = if c { a } else { b }` and wrong here: a `return` at the
            // end of a branch would be written as the branch's value and stop
            // returning - `if seq.len() < k { return counts }` came out as
            // `if seq.len() < k { counts }`, which is a different program.
            Stmt::Expr(Expr::If {
                cond,
                then_branch,
                else_branch,
            }) => self.if_expr(
                out,
                cond,
                then_branch,
                else_branch.as_ref(),
                depth,
                flow,
                tail,
            )?,
            // A `match` in *statement* position, for the `if`'s reason above:
            // its arms are not the block's value.
            Stmt::Expr(expr @ Expr::Match { .. }) if tail == Tail::Statement => {
                self.expr(
                    out,
                    expr,
                    depth,
                    Flow {
                        arms_are_statements: true,
                        ..flow
                    },
                )?;
            }
            Stmt::Expr(expr) => {
                // **A tail is a `return` written without the word**, so the `&`
                // it may owe is the same one — `fn text(ref self) -> ref String
                // { self.text }` and the `return` form are one program.
                let either = tail == Tail::Return
                    && self.returns_either(flow.function)
                    && !matches!(expr, Expr::LitNull);
                if either {
                    out.push("nikaia_std::either_text::either(");
                }
                if tail == Tail::Return && self.lent_returns.contains(&span.at()) {
                    out.push("&");
                }
                self.expr(out, expr, depth, flow)?;
                if either {
                    out.push(")");
                }
                // `if x { … };` is legal and noisy; a block-shaped statement
                // ends where its brace does.
                let block_shaped = matches!(
                    expr,
                    Expr::If { .. } | Expr::Block(_) | Expr::Unsafe(_) | Expr::Overlap(_)
                );
                if !tail.is_value() && !block_shaped {
                    out.push(";");
                }
            }
        }
        Ok(())
    }

    /// **What goes before a wrapped value, and what goes after**
    /// ([ADR-278](../../../docs/specification/adr/adr-278.md)).
    ///
    /// One function for all four of Part I 2.3's positions, so the two forms are
    /// written in one place: `Some(` … `)` where the checker knows the value is
    /// a plain `T`, and `` … `.into()` where it could not work the type out —
    /// which is right whether the value turns out to be a `T` or already a `T?`.
    ///
    /// The target is pinned in every one of the four — a declared result, an
    /// annotation, an assignment's left side, a field's or a parameter's
    /// declared type — so the conversion has something to resolve against and
    /// never has to be inferred from the value alone.
    /// The same, around a value written out: a conversion is a method call, so
    /// a value an operator makes is held in parentheses - `a - 1` into an
    /// `i64?` went below as `a - 1.into()`, the conversion taking the `1` alone
    /// (found moving the build-time integers into Nikaia, #125).
    fn around_value(how: Option<crate::check::Wrap>, value: &Expr) -> (&'static str, &'static str) {
        let compound = matches!(
            value,
            Expr::Binary { .. }
                | Expr::Unary { .. }
                | Expr::Cast { .. }
                | Expr::Range { .. }
                | Expr::Coalesce { .. }
        );
        match (how, compound) {
            (Some(crate::check::Wrap::Conversion), true) => ("(", ").into()"),
            _ => Self::around(how),
        }
    }

    fn around(how: Option<crate::check::Wrap>) -> (&'static str, &'static str) {
        match how {
            Some(crate::check::Wrap::Constructor) => ("Some(", ")"),
            Some(crate::check::Wrap::Conversion) => ("", ".into()"),
            None => ("", ""),
        }
    }

    /// What a `return` hands back: the nullable wrap, and **the `&` the compiler
    /// owes** where the value is a view of the subject
    /// ([ADR-094](../../../docs/specification/adr/adr-094.md) D1's third
    /// position).
    ///
    /// The `&` goes **inside** the wrap, because `Some(&self.text)` is an
    /// `Option<&String>` and `&Some(self.text)` is a view of a value built
    /// here — which also moves the field, the very thing the reference is for.
    fn handed_back(
        &self,
        out: &mut Out,
        value: &Expr,
        span: &Span,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        let (before, after) =
            Self::around_value(self.nullable_sites.get(&span.at()).copied(), value);
        out.push(before);
        // `null` is no text of either kind (ADR-282 D5), and goes as `None`.
        let either = self.returns_either(flow.function) && !matches!(value, Expr::LitNull);
        if either {
            out.push("nikaia_std::either_text::either(");
        }
        if self.lent_returns.contains(&span.at()) {
            out.push("&");
        }
        self.expr(out, value, depth, flow)?;
        if either {
            out.push(")");
        }
        out.push(after);
        Ok(())
    }

    /// Whether parameter `at` of what `callee` names is text both kinds of
    /// which flow into (ADR-282 D14): a function of this unit by name, or a
    /// method by its name - where every method of that name here agrees, so a
    /// name two types use differently is never wrapped for the wrong one.
    fn either_param(&self, callee: &str, at: usize) -> bool {
        let own = callee.rsplit("::").next().unwrap_or(callee);
        let mut found = Vec::new();
        let mut look = |item: &Item| {
            if let Item::Fn {
                name: Some(n),
                args,
                ..
            } = item
                && self.text(*n) == own
            {
                found.push(args.get(at).is_some_and(|a| a.ty.either));
            }
        };
        for item in &self.parsed.program.items {
            match &item.node {
                Item::Impl { methods, .. } => methods.iter().for_each(|m| look(&m.node)),
                node => look(node),
            }
        }
        !found.is_empty() && found.iter().all(|e| *e)
    }

    /// Whether a struct's field is text both kinds of which flow into
    /// ([ADR-282](../../docs/specification/adr/adr-282.md) D14).
    fn either_field(&self, owner: &str, field: Symbol) -> bool {
        let wanted = self.text(field);
        let owner = owner.rsplit("::").next().unwrap_or(owner);
        self.parsed
            .program
            .items
            .iter()
            .any(|item| match &item.node {
                Item::Struct { name, fields, .. } if self.text(*name) == owner => fields
                    .iter()
                    .any(|f| f.ty.either && self.text(f.name) == wanted),
                _ => false,
            })
    }

    /// Whether the function being written hands back text both kinds of
    /// which flow into its result (ADR-282 D14).
    fn returns_either(&self, function: &str) -> bool {
        let (target, own) = match function.rsplit_once("::") {
            Some((target, own)) => (Some(target), own),
            None => (None, function),
        };
        let either = |item: &Item| {
            matches!(item, Item::Fn { name: Some(n), ret_type: Some(t), .. }
                if self.text(*n) == own && t.either)
        };
        self.parsed
            .program
            .items
            .iter()
            .any(|item| match (&item.node, target) {
                (
                    Item::Impl {
                        target: t, methods, ..
                    },
                    Some(target),
                ) if self.text(t.name) == target => methods.iter().any(|m| either(&m.node)),
                (node, None) => either(node),
                _ => false,
            })
    }

    /// Whether what stands in the brackets is a **run**: a range written there,
    /// or one the checker says is kept in a name (ADR-293 D23).
    fn slices(&self, statement: usize, index: &Expr) -> bool {
        matches!(index, Expr::Range { .. })
            || self
                .slice_indices
                .contains(&(statement, crate::check::argument_shape(index)))
    }

    /// How the checker said this key goes into a map's brackets, where the
    /// map's keys are owned ([ADR-293](../../docs/specification/adr/adr-293.md)
    /// D1).
    fn map_key(
        &self,
        statement: usize,
        index: &Expr,
        written: bool,
    ) -> Option<crate::check::KeyForm> {
        self.map_keys
            .get(&(statement, crate::check::argument_shape(index), written))
            .copied()
    }

    /// A key, handed over as it is or lent with a `&`.
    fn key(
        &self,
        out: &mut Out,
        form: crate::check::KeyForm,
        index: &Expr,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        if form == crate::check::KeyForm::Either {
            out.push("nikaia_std::index::key(");
            self.expr(out, index, depth, flow)?;
            out.push(")");
            return Ok(());
        }
        if form != crate::check::KeyForm::Lent {
            return self.expr(out, index, depth, flow);
        }
        let bare = matches!(
            index,
            Expr::Variable(_)
                | Expr::Field { .. }
                | Expr::LitInt { .. }
                | Expr::LitChar(_)
                | Expr::LitBool(_)
        );
        out.push(if bare { "&" } else { "&(" });
        self.expr(out, index, depth, flow)?;
        if !bare {
            out.push(")");
        }
        Ok(())
    }

    /// A range as the language below writes one, `a..b`, for the two places
    /// that walk or slice it where it stands
    /// ([ADR-293](../../docs/specification/adr/adr-293.md) D18).
    fn bare_range(&self, out: &mut Out, expr: &Expr, depth: usize, flow: Flow<'_>) -> Result<()> {
        let Expr::Range {
            start,
            end,
            inclusive,
        } = expr
        else {
            return self.expr(out, expr, depth, flow);
        };
        self.expr(out, start, depth, flow)?;
        out.push(if *inclusive { "..=" } else { ".." });
        self.expr(out, end, depth, flow)
    }

    /// What stands inside brackets: a range there is a **slice**, and stays
    /// the language below's own (ADR-293 D18).
    /// Whether `name` is a parameter of the function being emitted that it
    /// changes in place - a `mut` parameter, lowered as `&mut T` (ADR-094 D3).
    fn changes_in_place(&self, flow: Flow<'_>, name: &str) -> bool {
        self.own_contracts
            .functions
            .get(flow.function)
            .and_then(|c| c.signature.as_ref())
            .is_some_and(|s| s.mutable.iter().any(|m| m == name))
    }

    fn index_expr(&self, out: &mut Out, index: &Expr, depth: usize, flow: Flow<'_>) -> Result<()> {
        self.bare_range(out, index, depth, flow)
    }

    fn expr(&self, out: &mut Out, expr: &Expr, depth: usize, flow: Flow<'_>) -> Result<()> {
        // **A plain arm beside a `null` is `Some`** (`check::Checked::some_tails`).
        if self.some_tails.contains(&(expr as *const Expr as usize)) {
            out.push("Some(");
            self.expr_as_written(out, expr, depth, flow)?;
            out.push(")");
            return Ok(());
        }
        // **And a plain value in a nullable slot** (`check::Checked::wrapped`).
        if let Some(how) = self.wrapped.get(&(expr as *const Expr as usize)) {
            let (before, after) = Self::around_value(Some(*how), expr);
            out.push(before);
            self.expr_as_written(out, expr, depth, flow)?;
            out.push(after);
            return Ok(());
        }
        // **A map's read copied out where a `T?` of its own is kept**
        // ([ADR-293](../../docs/specification/adr/adr-293.md) D15). A read
        // through the brackets is `*get(…)`, whose `*` would take the whole
        // chain, so it is held in parentheses.
        let shape = (flow.statement, crate::check::argument_shape(expr));
        if self.copied_reads.contains(&shape) {
            let held = matches!(expr, Expr::Index { .. }) && !self.flat_reads.contains(&shape);
            if held {
                out.push("(");
            }
            self.expr_after_the_copy(out, expr, depth, flow)?;
            if held {
                out.push(")");
            }
            out.push(".copied()");
            return Ok(());
        }
        self.expr_after_the_copy(out, expr, depth, flow)
    }

    /// [`Emitter::expr`]'s last step, after a copy out of a map's read.
    fn expr_after_the_copy(
        &self,
        out: &mut Out,
        expr: &Expr,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        // **`get` on a map of `T?`** is one `T?`, as the brackets are
        // ([ADR-293](../../docs/specification/adr/adr-293.md) D14).
        if matches!(expr, Expr::MethodCall { .. })
            && self
                .flat_reads
                .contains(&(flow.statement, crate::check::argument_shape(expr)))
        {
            out.push("nikaia_std::index::flat(");
            self.expr_as_written(out, expr, depth, flow)?;
            out.push(")");
            return Ok(());
        }
        self.expr_as_written(out, expr, depth, flow)
    }

    /// [`Emitter::expr`], without the wrap an arm beside a `null` takes.
    fn expr_as_written(
        &self,
        out: &mut Out,
        expr: &Expr,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        match expr {
            // **A literal in an unsigned place carries that type's suffix**
            // (ADR-285 D3), ahead of the widening a bare one gets.
            Expr::LitInt { value, negative } => {
                let v = crate::ast::int_value(*value, *negative);
                match self
                    .unsigned_literals
                    .get(&(crate::check::value_node(expr), v))
                {
                    Some(ty) => out.push(&format!("{v}{ty}")),
                    None => out.push(&integer_literal(v, flow.widen)),
                }
            }
            Expr::LitFloat(v) => out.push(v),
            Expr::LitStr { .. } | Expr::LitInterpolated { .. } => {
                self.string(out, expr, depth, flow)?
            }
            Expr::LitChar(c) => out.push(&format!("'{c}'")),
            // **A range is a value**
            // ([ADR-293](../../docs/specification/adr/adr-293.md) D18): two
            // numbers, `Copy`, and walked as often as a program likes - which
            // the language below's `Range` is not, being its own iterator. So
            // one that is kept, handed on or called on is `nikaia_std`'s, and
            // only one written straight into a `for` or into brackets is Rust's
            // (`bare_range`).
            Expr::Range {
                start,
                end,
                inclusive,
            } => {
                out.push(match inclusive {
                    true => "nikaia_std::range::through(",
                    false => "nikaia_std::range::span(",
                });
                self.expr(out, start, depth, flow)?;
                out.push(", ");
                self.expr(out, end, depth, flow)?;
                out.push(")");
            }
            Expr::LitBool(b) => out.push(&b.to_string()),
            // Part I 2.3. `None` and nothing around it: `null` has no type of
            // its own, so the type beside it is what says what it is the
            // absence of - and a `None` the language below cannot type is a
            // program `rustc` asks an annotation for, which is the honest
            // answer rather than one this compiler invented.
            Expr::LitNull => out.push("None"),
            // **A `mut` lambda parameter is an address** (ADR-281 D12), so
            // every mention of it is dereferenced.
            Expr::Variable(name) if flow.changed.contains(name) => {
                out.push(&format!("(*{})", self.name(*name)))
            }
            // **A constructor handed over as a value**
            // ([ADR-140](../../docs/specification/adr/adr-140.md) D2). `Summary`
            // in value position is the anonymous constructor, and the language
            // below calls it `Summary::new` — the same two spellings the call
            // path reconciles, one position over. `par_fold(M, Summary, …)` is
            // where it is written.
            //
            // **Only a name this file declares as a type**, or one the library
            // publishes with a constructor: anything else is an ordinary name,
            // and a local that shadows a type is refused a declaration by
            // [ADR-286](../../docs/specification/adr/adr-286.md) rather than
            // guessed at here.
            Expr::Variable(name) if self.constructs_by_name(self.text(*name)) => {
                out.push(&self.path(&[self.text(*name), "new"]))
            }
            Expr::Variable(name)
                if !*self.in_a_kept_call.borrow()
                    && self
                        .kept_functions
                        .contains_key(&(flow.statement, self.text(*name).to_string())) =>
            {
                // **A named function where a function value is kept** is the
                // closure that calls it, written as the checker typed it.
                let (pauses, fails, arity) =
                    self.kept_functions[&(flow.statement, self.text(*name).to_string())];
                let (names, call) = crate::check::kept_function_call(self.parsed, *name, arity);
                out.push(&format!(
                    "nikaia_std::func::Kept(std::sync::Arc::new(move |{}| ",
                    names
                        .iter()
                        .map(|n| self.text(*n).to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                let inside = Flow {
                    throws: fails,
                    statement: flow.statement,
                    ..Flow::PLAIN
                };
                if pauses {
                    out.push("Box::pin(async move ");
                }
                out.push(match fails {
                    true => "{ Ok(",
                    false => "{ ",
                });
                let outer = self.in_a_kept_call.replace(true);
                let written = self.expr(out, &call, depth, inside);
                *self.in_a_kept_call.borrow_mut() = outer;
                written?;
                out.push(match fails {
                    true => ") }",
                    false => " }",
                });
                if pauses {
                    out.push(&format!(") as {}<_>", self.boxed_future()));
                }
                out.push("))");
            }
            Expr::Variable(name) => {
                out.push(&self.name(*name));
                // **A binding packed for a task is read through its handle**
                // (ADR-283 D11), for as long as the read borrows it - and so is
                // an element taken out of a keeper that holds structs of views
                // ([ADR-283](../../docs/specification/adr/adr-283.md) D16): it
                // carries its handles with it.
                let packed = self.keep_plan(flow.function).is_some_and(|p| {
                    p.tethered.contains_key(self.text(*name))
                        || p.held_locals.contains(self.text(*name))
                });
                if packed && !flow.in_a_place {
                    out.push(".get()");
                }
            }
            // ADR-017: the template is compiled where it is written. What comes
            // out is the string building a hand-written renderer would do, with
            // `html::Render` at every hole - which is what makes the escaping a
            // property of the template rather than of whoever filled it in.
            Expr::Dsl {
                target,
                content,
                context,
            } => self.template(out, *target, context.as_ref(), content, depth, flow)?,
            Expr::Path(segments) => {
                let path: Vec<&str> = segments.iter().map(|s| self.text(*s)).collect();
                out.push(&self.path(&path));
            }
            // A block in *expression* position: its last statement is its
            // own value, and a `return` inside it leaves the function around
            // it - so it is not written as the block's value (`Tail`).
            Expr::Block(block) => self.block(out, block, depth, flow, Tail::Value)?,
            // **Rust's own `unsafe`**
            // ([ADR-302](../../docs/specification/adr/adr-302.md) D3), which is
            // the whole of the lowering: the word means the same thing on both
            // sides and the block inside it is an ordinary one.
            Expr::Unsafe(block) => {
                out.push("unsafe ");
                self.block(out, block, depth, flow, Tail::Value)?;
            }

            // **Part I 8.1.2: every branch in flight, and the value is their
            // results in written order** (ADR-292 D2).
            Expr::Overlap(block) => self.overlap(out, block, depth, flow)?,
            // **Part II 12.4: every arm in flight, and the first to finish is
            // the one that is kept** (ADR-292 D12).
            Expr::Select(arms) => self.select(out, arms, depth, flow)?,
            // An `if` in *expression* position - `let x = if c { a } else { b }`
            // - hands its branch's value to whoever asked for it, so a `return`
            // in a branch is the function's and stays one.
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => self.if_expr(
                out,
                cond,
                then_branch,
                else_branch.as_ref(),
                depth,
                flow,
                Tail::Value,
            )?,
            Expr::Call { func, args, config } => self.call(out, func, args, config, depth, flow)?,
            Expr::MethodCall {
                receiver,
                method,
                args,
                config,
            } => {
                // **`field.of(value)` is the field read a program would have
                // written by hand** ([ADR-304](../../docs/specification/adr/adr-304.md)
                // D2, D5's *at run time: nothing*): `value.name`, with no
                // descriptor and no dispatch left.
                // **A field that holds a function is called through the
                // field**: `(button.on_click)(4)`, awaited where it may pause.
                let called = format!(
                    "{}.{}",
                    crate::check::argument_shape(receiver),
                    self.text(*method)
                );
                if let Some(pauses) = self
                    .field_calls
                    .get(&(flow.statement, called.clone()))
                    .copied()
                {
                    // **Inside a lambda, as a pausing function is**
                    // ([ADR-230](../../docs/specification/adr/adr-230.md) D3):
                    // the closure below is synchronous, and an `.await` in it
                    // was `rustc`'s error about a file nobody wrote.
                    if pauses && flow.in_lambda {
                        let named = match receiver.as_ref() {
                            Expr::Variable(name) => {
                                format!("{}.{}", self.text(*name), self.text(*method))
                            }
                            _ => self.text(*method).to_string(),
                        };
                        return Err(pausing_in_a_lambda(flow.statement, &named));
                    }
                    out.push("(");
                    self.postfix_base(out, receiver, depth, flow)?;
                    out.push(&format!(".{})(", self.name(*method)));
                    for (at, arg) in args.iter().enumerate() {
                        if at > 0 {
                            out.push(", ");
                        }
                        self.expr(out, arg, depth, flow)?;
                    }
                    out.push(")");
                    if pauses {
                        out.push(".await");
                    }
                    if flow.throws && !flow.caught && self.method_can_fail(flow, *method) {
                        out.push("?");
                    }
                    return Ok(());
                }
                // **`variant.is(value)` is the pattern test a program would
                // have written by hand**: `matches!(value, Op::Add { .. })`,
                // which fits a variant of every shape.
                if let Some(variant) = self.reflected(receiver, *method, "is")
                    && let (Some(value), Some((_, on))) =
                        (args.first(), self.specialising.borrow().clone())
                {
                    out.push("matches!(");
                    self.expr(out, value, depth, flow)?;
                    out.push(&format!(", {on}::{} {{ .. }})", escaped(&variant)));
                    return Ok(());
                }
                if let Some(field) = self.reflected(receiver, *method, "of")
                    && let Some(value) = args.first()
                {
                    self.postfix_base(out, value, depth, flow)?;
                    out.push(&format!(".{}", escaped(&field)));
                    return Ok(());
                }
                // **`error.full()` in a handler a named channel reached**
                // ([ADR-280](../../docs/specification/adr/adr-280.md) D10). Part
                // I 7.1 writes it as an ordinary call, and it is one — but the
                // envelope was opened at the binding, so the two halves of the
                // long form are `error` and the site beside it rather than one
                // value with a method on it.
                let long_form = flow.caught_named
                    && self.text(*method) == "full"
                    && args.is_empty()
                    && matches!(receiver.as_ref(), Expr::Variable(name) if self.text(*name) == CAUGHT);
                match long_form {
                    true => out.push(&format!("{SITE}.full_of(&{CAUGHT})")),
                    false => {
                        self.method_call(out, Some(receiver), *method, args, config, depth, flow)?;
                        // **Views of values that copy are the values**
                        // (ADR-231 D1).
                        if self
                            .copied_walks
                            .contains(&(flow.statement, crate::check::argument_shape(expr)))
                        {
                            out.push(".copied()");
                        }
                    }
                }
            }
            // Part I 3.5 onto a **method**
            // ([ADR-278](../../../docs/specification/adr/adr-278.md)): the call
            // happens only where there is something to call it on.
            //
            // **A `match` and not the field's `map`**, and the difference is the
            // difference between a field and a method. A field access can
            // neither pause nor fail, so a closure is somewhere it can happen; a
            // method call may do both, and inside a closure an `.await` does not
            // compile and a `?` has nowhere to go. So the reach is written out,
            // which is the one shape that lets the call be whatever a call is.
            //
            // `None => None` and not `.map(…)`'s implicit one, for the flattened
            // case: where the method's own result is a `T?`, the `Some` arm
            // hands it back as it is and the reach does not nest.
            Expr::SafeMethod {
                receiver,
                method,
                args,
                config,
            } => {
                let name = self.text(*method).to_string();
                let flattens = self
                    .flattened_reaches
                    .contains(&(flow.statement, name.clone()));
                // **The scrutinee is lent where the call changes nothing**
                // ([ADR-278](../../docs/specification/adr/adr-278.md) D18):
                // `find(1)?.greet("Hallo")` used to take `find(1)`'s value into
                // the `match`, so a receiver bound to a name was gone
                // afterwards and `rustc` said so about a file nobody wrote
                // (Part III C.1). What comes out is the **call's** result, so
                // nothing about the reach needs the state that is not built.
                let lent = match self.lent_reaches.contains(&(flow.statement, name.clone())) {
                    true => ".as_ref()",
                    false => "",
                };
                out.push("match ");
                let held = self
                    .held
                    .borrow()
                    .get(&(receiver.as_ref() as *const Expr as usize))
                    .cloned();
                match held {
                    Some(name) => out.push(&name),
                    // A map's read is not lent (0.0.246), so nothing follows
                    // it and a `match` needs no parentheses around the `*`.
                    None if lent.is_empty() && matches!(receiver.as_ref(), Expr::Index { .. }) => {
                        self.expr(out, receiver, depth, flow)?
                    }
                    None => self.postfix_base(out, receiver, depth, flow)?,
                }
                out.push(lent);
                out.push(&format!(
                    " {{
{}",
                    "    ".repeat(depth + 1)
                ));
                out.push(&format!("Some({REACHED}) => "));
                if !flattens {
                    out.push("Some(");
                }
                // **A copy of the view reached** is `to_owned`, as the plain
                // call's is (ADR-293 D24): the receiver the rewrite reads is the
                // one this `match` bound.
                let copy = self
                    .owned_copies
                    .contains(&(flow.statement, crate::check::argument_shape(receiver)));
                let held_copy = self.reached_copy.replace(copy);
                let written = self.method_call(out, None, *method, args, config, depth + 1, flow);
                self.reached_copy.set(held_copy);
                written?;
                if !flattens {
                    out.push(")");
                }
                out.push(&format!(
                    ",
{}None => None,
{}}}",
                    "    ".repeat(depth + 1),
                    "    ".repeat(depth)
                ));
            }
            Expr::Match { value, arms } => {
                // **A `match error` over a sum is taken apart by member**
                // ([ADR-280](../../docs/specification/adr/adr-280.md) D17). The
                // patterns the source writes name variants of the **members**
                // — `ConfigError::Empty(p)` and `io::IoError::NotFound(p)` in
                // one block — because the sum is a name no program writes
                // ([ADR-023](../../docs/specification/adr/adr-023.md) D4). So
                // one match becomes a match per member with the arms that
                // belong to it, and the catch-all is what every one of them
                // falls through to.
                if let Some(sum) = flow.caught_sum
                    && matches!(value.as_ref(), Expr::Variable(name) if self.text(*name) == CAUGHT)
                {
                    return self.match_over_a_sum(out, sum, arms, depth, flow);
                }
                // **A `match` that only answers `true` or `false` is a
                // `matches!`** (0.0.249): the arms that say one answer, and
                // `else` the other. The long form is what `clippy` refuses in
                // `std`, so a `.nika` file there wrote around it.
                if let Some(answer) = a_yes_or_no(arms)
                    && !arms.iter().any(|arm| self.looks_inside_a_box(&arm.pattern))
                {
                    if !answer {
                        out.push("!");
                    }
                    out.push("matches!(");
                    self.expr(out, value, depth, flow)?;
                    out.push(", ");
                    for (i, arm) in arms[..arms.len() - 1].iter().enumerate() {
                        if i > 0 {
                            out.push(" | ");
                        }
                        self.match_pattern(out, &arm.pattern, depth + 1, flow)?;
                    }
                    out.push(")");
                    return Ok(());
                }
                out.push("match ");
                // A field reached through a view is matched by reference
                // (`check::Checked::lent_scrutinees`).
                if self
                    .lent_scrutinees
                    .contains(&(&**value as *const Expr as usize))
                {
                    out.push("&");
                }
                self.expr(out, value, depth, flow)?;
                let pad = "    ".repeat(depth + 1);
                let close = "    ".repeat(depth);
                out.push(" {\n");
                for arm in arms {
                    out.push(&pad);
                    // **A part looked into through its box**
                    // ([ADR-246](../../docs/specification/adr/adr-246.md) D5):
                    // the language below matches no pattern through a box, so
                    // the part is bound to a fresh name, the guard asks
                    // whether it has the shape (the next arm is tried where it
                    // does not, as for any pattern), and the arm takes it
                    // apart before anything else.
                    let lent = self
                        .lent_bindings
                        .get(&(&arm.pattern as *const MatchPattern as usize));
                    let mut nests: Vec<&MatchPattern> = Vec::new();
                    self.pattern_written(
                        out,
                        &arm.pattern,
                        Names::Bind(lent),
                        &mut Some(&mut nests),
                        depth + 1,
                        flow,
                    )?;
                    // **A guard that reads a name bound in there** (ADR-246
                    // D5 item 2): the shape is asked by a `match` through the
                    // box that binds the names - by reference, and a number
                    // copied out - and answers the guard where it fits and
                    // `false` where it does not.
                    let reads_inside = self
                        .guards_inside_boxes
                        .get(&(&arm.pattern as *const MatchPattern as usize));
                    if let (Some(copied), Some(guard)) = (reads_inside, &arm.guard)
                        && !nests.is_empty()
                    {
                        out.push(" if ");
                        for (at, nest) in nests.iter().enumerate() {
                            out.push(&format!(
                                "match nikaia_std::boxed::peek(&__nikaia_box{at}) {{ "
                            ));
                            self.pattern_written(
                                out,
                                nest,
                                Names::Bind(None),
                                &mut None,
                                depth + 1,
                                flow,
                            )?;
                            out.push(" => ");
                        }
                        out.push("{ ");
                        for name in copied {
                            let name = escaped(name);
                            out.push(&format!("let {name} = *{name}; "));
                        }
                        self.guard_written(out, &arm.pattern, guard, depth + 1, flow)?;
                        out.push(" }");
                        for _ in &nests {
                            out.push(", _ => false }");
                        }
                        out.push(" => ");
                        self.arm_body_opening(
                            out,
                            &arm.pattern,
                            &nests,
                            &arm.body,
                            depth + 1,
                            flow,
                        )?;
                        out.push(",\n");
                        continue;
                    }
                    for (at, nest) in nests.iter().enumerate() {
                        out.push(match at {
                            0 => " if ",
                            _ => " && ",
                        });
                        out.push(&format!(
                            "matches!(nikaia_std::boxed::peek(&__nikaia_box{at}), "
                        ));
                        self.pattern_written(out, nest, Names::Wild, &mut None, depth + 1, flow)?;
                        out.push(")");
                    }
                    // **The guard is Rust's own**
                    // ([ADR-291](../../../docs/specification/adr/adr-291.md)
                    // D2), written with the same word, so this is a
                    // transcription like everything else about a pattern.
                    if let Some(guard) = &arm.guard {
                        // In parentheses only beside the conditions above,
                        // where an `||` in it would otherwise bind wider.
                        match nests.is_empty() {
                            true => {
                                out.push(" if ");
                                self.guard_written(out, &arm.pattern, guard, depth + 1, flow)?;
                            }
                            false => {
                                out.push(" && (");
                                self.guard_written(out, &arm.pattern, guard, depth + 1, flow)?;
                                out.push(")");
                            }
                        }
                    }
                    out.push(" => ");
                    self.arm_body_opening(out, &arm.pattern, &nests, &arm.body, depth + 1, flow)?;
                    out.push(",\n");
                }
                out.push(&format!("{close}}}"));
            }
            Expr::Tuple(parts) => {
                out.push("(");
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        out.push(", ");
                    }
                    self.expr(out, part, depth, flow)?;
                }
                out.push(")");
            }
            // **`vec![…]`, which is what a `Vec` already is below**
            // ([ADR-135](../../../docs/specification/adr/adr-135.md) D1). Part
            // I 2.2 offers one container and the literal writes that one, so
            // there is no second shape to choose between here.
            //
            // **Unless the use asked for an array**
            // ([ADR-152](../../../docs/specification/adr/adr-152.md) D4), which
            // is a fact about the literal's *type* and therefore the checker's
            // to hand over: the same three values are `vec![…]` in one position
            // and `[…]` in the other, and nothing in the source distinguishes
            // them.
            Expr::ListLit { items, at } => {
                let array = self.array_literals.contains(&(*at as usize));
                out.push(match array {
                    true => "[",
                    false => "vec![",
                });
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(", ");
                    }
                    self.expr(out, item, depth, flow)?;
                }
                out.push("]");
            }
            Expr::Field { base, name } => {
                // **`field.name` is the field's own name as text**
                // ([ADR-304](../../docs/specification/adr/adr-304.md) D2): a
                // literal, because the turn this copy stands at is known.
                if let Some(field) = self.reflected(base, *name, "name") {
                    out.push(&format!("\"{}\"", field));
                    return Ok(());
                }
                // **A boxed field is read through its box**
                // ([ADR-246](../../docs/specification/adr/adr-246.md) D2):
                // `(*base.field)` is a place that behaves as the unboxed field
                // would - read, lent, moved out of an owned value, assigned.
                let boxed = self
                    .boxed_reads
                    .contains(&(flow.statement, crate::check::argument_shape(expr)));
                if boxed {
                    out.push("(*");
                }
                // **A field of an element is read through the reference the
                // read answers**, as a read of a read is (0.0.238): `xs[0].a`
                // was `(*get(&xs, 0)).a`, a deref the language below does on
                // its own and `clippy` refuses in a `std` written in Nikaia
                // (`tools/ty.nika`, ADR-294).
                match &**base {
                    Expr::Index {
                        base: inner,
                        index: at,
                    } if !flow.in_a_place
                        && !self.slices(flow.statement, at)
                        && self.map_key(flow.statement, at, false).is_none()
                        && !self.reads_through_handle(flow, inner) =>
                    {
                        self.index_read(out, inner, at, depth, flow, false)?;
                    }
                    _ => self.postfix_base(out, base, depth, flow)?,
                }
                out.push(&format!(".{}", self.name(*name)));
                if boxed {
                    out.push(")");
                }
            }
            // Part I 3.5: `x?.name` reaches the field only where there is
            // something to reach it on.
            //
            // **`map` or `and_then`, and the checker says which.** Over a plain
            // field `map` is right; over a field that is *itself* a `T?` it
            // would make an `Option<Option<T>>`, and `and_then` is what
            // flattens - which is a question about the declared type, so it is
            // answered where the types are (ADR-288). `map` is the fallback,
            // because it is the one that cannot nest a plain field.
            Expr::SafeField { base, name } => {
                let field = self.text(*name).to_string();
                let flattens = self
                    .flattened_reaches
                    .contains(&(flow.statement, field.clone()));
                let how = if flattens { "and_then" } else { "map" };
                // **The receiver is lent where the member copies**
                // ([ADR-278](../../docs/specification/adr/adr-278.md) D18):
                // `user?.id` reads `user` where it lies and `user` is usable on
                // the next line, which is
                // [ADR-278](../../docs/specification/adr/adr-278.md) D16. Before
                // it, `Option::map` took its receiver and `rustc` said *use of
                // moved value* about a file nobody wrote (Part III C.1).
                //
                // **Only where the checker said the member copies**, which is
                // the safe direction: a member that would come out as a *view*
                // needs the state this compiler does not build, and a field
                // whose type this compiler could not work out is claimed
                // nothing about (Part III C.4). Both lower exactly as they did.
                let copies = self
                    .copied_reaches
                    .contains(&(flow.statement, field.clone()));
                // **And where it comes out as a view** (0.0.244, issue #167
                // issue #167): a view of the member is a view *into* the receiver,
                // so the receiver has to be lent for it as well. `a.b?.c?.v`
                // took `a.b` out of a lent `a` - *cannot move out of `a.b`* -
                // and handed back a view of the value it had just moved.
                let viewed = self
                    .viewed_reaches
                    .contains_key(&(flow.statement, field.clone()));
                let lent = match copies || viewed {
                    true => ".as_ref()",
                    false => "",
                };
                // **And how the member is taken out of that view**
                // ([ADR-278](../../docs/specification/adr/adr-278.md) D19). A
                // member that copies is read; one that does not comes out as a
                // view, and the two views are spelled differently below: a view
                // of `String` is `&str`
                // ([ADR-184](../../docs/specification/adr/adr-184.md) D2) and a
                // view of anything else is `&T`. The checker says which,
                // because this emitter has no types (ADR-296 D17).
                // **A boxed field is reached through its box**
                // ([ADR-246](../../docs/specification/adr/adr-246.md) D2).
                let place = match self
                    .boxed_reads
                    .contains(&(flow.statement, crate::check::argument_shape(expr)))
                {
                    true => format!("(*__nikaia_it.{field})"),
                    false => format!("__nikaia_it.{field}"),
                };
                let reach = match (
                    self.viewed_reaches.get(&(flow.statement, field.clone())),
                    flattens,
                ) {
                    (None, _) => place,
                    (Some(crate::check::Viewed::Text), false) => format!("{place}.as_str()"),
                    (Some(crate::check::Viewed::Text), true) => format!("{place}.as_deref()"),
                    (Some(crate::check::Viewed::Plain), false) => format!("&{place}"),
                    (Some(crate::check::Viewed::Plain), true) => format!("{place}.as_ref()"),
                };
                self.postfix_base(out, base, depth, flow)?;
                out.push(&format!("{lent}.{how}(|__nikaia_it| {reach})"));
            }
            Expr::Index { base, index } => {
                // **A length is an `i64`, so an index is one too**
                // ([ADR-285](../../../docs/specification/adr/adr-285.md) D1), and
                // Rust indexes a sequence by `usize`. The conversion is emitted
                // and never written, in both directions - which is the whole of
                // what D1 buys.
                //
                // Around **every** index but one, because this emitter does not
                // know types (ADR-296 D17) and a map is indexed too:
                // `counts[path]` takes a `&str`. `nikaia_std::index::at` chooses
                // on the type of what is in the brackets and is the identity for
                // anything that is not a number, so a rule applied everywhere
                // cannot be applied to the wrong index.
                //
                // **The one exception is an index written only in literals.**
                // `xs[0]` and `&text[1..3]` need no conversion - Rust's own
                // inference gives a literal the `usize` the sequence wants - and
                // they cannot *take* one: `at(0)` has nothing to infer `I` from,
                // since every integer type answers with the same `usize`, and
                // `cannot infer type` about a generated file is exactly what
                // Part III C.1 forbids.
                //
                // **A read is a call and a place is brackets**
                // ([ADR-293](../../../docs/specification/adr/adr-293.md) D4). A
                // read through the brackets answers what the container can
                // promise — a `T` for a sequence, a `T?` for a map, because a
                // key that is not there is data about the world rather than a
                // bug — and `index::get` is the one trait that lets this
                // emitter write the same three tokens for both. The **left of
                // an assignment** is not a read: `self.bodies[0].vx = …` writes
                // through the index, so the brackets there stay the language
                // below's own.
                if !flow.in_a_place {
                    // **A slice is already a view, so it takes no `*`**
                    // ([ADR-182](../../docs/specification/adr/adr-182.md) D2). A read
                    // at a number answers a `&T` and the `*` is what makes it
                    // the `T` the program asked for; a read at a **range**
                    // answers a `&str` or a `&[T]`, and `*` over one of those
                    // is a `str` or a `[T]` — a value whose size the language
                    // below does not know, which is what it said: *the size
                    // for values of type `str` cannot be known at compilation
                    // time*, about a noun nobody wrote
                    // ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
                    //
                    // **Off the shape of what is in the brackets**, which is
                    // the one place this needs no type: a range is a run and a
                    // key is not, in this language and in the one below alike.
                    //
                    // **And no parentheses here at all**
                    // ([ADR-293](../../docs/specification/adr/adr-293.md) D33).
                    // A pair around `*get(…)` is what makes `(*get(…)).len()`
                    // the length of the deref rather than the deref of the
                    // length - but only where a postfix follows, and a postfix
                    // is written through `postfix_base`, which adds it. Written
                    // here, it stood around every read: `m[k] ?? 0` and
                    // `let x = m[k]` were `rustc` warnings about a file nobody
                    // wrote ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
                    //
                    // **A read that is lent is the reference it already was**
                    // (0.0.250): the `&` in front and the `*` cancel.
                    // **A read of a map of `T?` is one `T?`**
                    // ([ADR-293](../../docs/specification/adr/adr-293.md) D14):
                    // `index::flat` over the read, which is the `Option<&T>`
                    // a `ref T?` is below - so a lend in front of it is
                    // already there, and is taken.
                    if self
                        .flat_reads
                        .contains(&(flow.statement, crate::check::argument_shape(expr)))
                    {
                        let _ = out.take_lend();
                        out.push("nikaia_std::index::flat(");
                        self.index_read(out, base, index, depth, flow, true)?;
                        out.push(")");
                        return Ok(());
                    }
                    // **A map's read lent is the option it already is**: `*get(…)`
                    // is the `Option<&V>` a `ref V?` is, and cancelling the `*`
                    // against the lend left `index::Found`, which no `ref V?`
                    // parameter takes (found moving `sharing`'s leaves into
                    // Nikaia, #125). A sequence's read keeps the cancelling.
                    if self.map_key(flow.statement, index, false).is_some() {
                        let _ = out.take_lend();
                        return self.index_read(out, base, index, depth, flow, true);
                    }
                    let lent = !self.slices(flow.statement, index) && out.take_lend();
                    return self.index_read(out, base, index, depth, flow, !lent);
                }
                self.postfix_base(out, base, depth, flow)?;
                match only_literals(index) {
                    true => {
                        out.push("[");
                        self.index_expr(out, index, depth, flow.inferred())?;
                        out.push("]");
                    }
                    false => {
                        out.push("[nikaia_std::index::at(");
                        self.index_expr(out, index, depth, flow)?;
                        out.push(")]");
                    }
                }
            }
            Expr::Cast { expr, ty } => {
                let into = self.ty(ty, Lifetimes::ELIDED);
                // **A `for` lends, and `as` does not see through a view**
                // ([ADR-182](../../docs/specification/adr/adr-182.md) D1).
                // The checker says which operands those are, because which name is
                // a view is a question about the scope (ADR-288).
                //
                // **`num::value` and not a `*`**, which is
                // `nikaia_std::index::at`'s own reasoning one construct over:
                // for a number that is already a number this is the identity,
                // so the rule cannot be written onto the wrong operand — and a
                // `*` written onto one would be a `rustc` error about the
                // generated file, which is the very thing this closes.
                let viewed = match expr.as_ref() {
                    Expr::Variable(name) => self
                        .viewed_numbers
                        .contains(&(flow.statement, self.text(*name).to_string())),
                    _ => false,
                };
                match self.narrows(flow.statement, &into) {
                    // **A narrowing conversion is checked, and an unchecked one
                    // aborts** (ADR-285 D12). Rust's `as` truncates by definition
                    // and no setting changes that, so this is the one place in
                    // the arithmetic decision where the emitted code differs from
                    // what was written - and it differs into what a Rust
                    // programmer would have written here anyway.
                    Some(crate::check::Narrowing::Integer) => {
                        // `unwrap_or_else` and not `expect`: `expect` appends
                        // the error's `Debug`, which is `TryFromIntError(())` -
                        // Rust internals in a sentence a Nikaia user reads, and
                        // the same leak the diagnostics filter exists to stop.
                        out.push(&format!("{into}::try_from("));
                        self.operand(out, expr, viewed, depth, flow)?;
                        out.push(&format!(
                            ").unwrap_or_else(|_| panic!(\"the value does not fit in an `{into}`\"))"
                        ));
                    }
                    // `i32::try_from(f64)` does not exist, so the range and
                    // "not a number" are tested by a `std` helper instead.
                    Some(crate::check::Narrowing::FromFloat) => {
                        out.push(&format!("nikaia_std::num::to_{into}("));
                        self.operand(out, expr, viewed, depth, flow)?;
                        out.push(")");
                    }
                    None if viewed => {
                        self.operand(out, expr, viewed, depth, flow)?;
                        out.push(&format!(" as {into}"));
                    }
                    None => {
                        self.nested(out, expr, u8::MAX, depth, flow)?;
                        out.push(&format!(" as {into}"));
                    }
                }
            }
            Expr::StructLit { name, fields } => {
                // `unaliased`, the same as a type: `h::Request(path: …)` builds
                // `http::Request` (ADR-286 D12).
                let owner = self.parsed.unaliased(self.text(*name));
                // A value with a cleanup is made in its `Cleaned` (ADR-297 D1).
                let cleaned = self.cleaned_types.contains(&owner);
                if cleaned {
                    out.push(&format!("{}::new(", self.cleaned()));
                }
                out.push(&format!("{owner} {{ "));
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(", ");
                    }
                    out.push(&self.name(field.name));
                    // Part I 2.3: a plain value in a field the struct declares
                    // nullable. Keyed by the field's own name, because a struct
                    // literal has one of these per field and the statement has
                    // only one span - **and by the type and the value's shape**,
                    // because a statement may build two literals
                    // (`check::argument_shape`).
                    let how = self
                        .nullable_fields
                        .get(&(
                            flow.statement,
                            owner.to_string(),
                            self.text(field.name).to_string(),
                        ))
                        .and_then(|by_shape| {
                            by_shape.get(
                                &field
                                    .value
                                    .as_ref()
                                    .map(crate::check::argument_shape)
                                    .unwrap_or_default(),
                            )
                        })
                        .copied();
                    let (before, after) = Self::around(how);
                    // **A boxed field is built in its box**
                    // ([ADR-246](../../docs/specification/adr/adr-246.md) D3),
                    // around whatever else the value is wrapped in.
                    let boxed = self.is_boxed_field(&owner, self.text(field.name));
                    let (before, after) = match boxed {
                        true => (format!("Box::new({before}"), format!("{after})")),
                        false => (before.to_string(), after.to_string()),
                    };
                    let (before, after) = (before.as_str(), after.as_str());
                    // **Into a field both kinds of text flow into, each value
                    // goes as it is** (ADR-282 D14): a view borrowed, text of its
                    // own moved in.
                    let either = self.either_field(&owner, field.name)
                        && !matches!(field.value, Some(Expr::LitNull));
                    // The nullable wrap goes around it: `Some(either(v))`
                    // (ADR-282 D5).
                    if either {
                        out.push(": ");
                        out.push(before);
                        out.push("nikaia_std::either_text::either(");
                        match &field.value {
                            Some(value) => self.expr(out, value, depth, flow)?,
                            None => out.push(&self.name(field.name)),
                        }
                        out.push(")");
                        out.push(after);
                    } else if let Some(value) = &field.value {
                        // **`name: name` is written `name`** (0.0.249), as the
                        // source may write it (Part I 4.1): the pair is what
                        // `clippy` refuses in `std`, and a lowering that only
                        // repeats the field's name adds nothing to say.
                        if before.is_empty()
                            && after.is_empty()
                            && matches!(value, Expr::Variable(v) if self.text(*v) == self.text(field.name))
                            && Out::scratch(|inner| self.expr(inner, value, depth, flow))?.buf
                                == self.name(field.name)
                        {
                            continue;
                        }
                        out.push(": ");
                        out.push(before);
                        self.expr(out, value, depth, flow)?;
                        out.push(after);
                    } else if how.is_some() || boxed {
                        // `Counter { db }` is the shorthand for `db: db`
                        // (Part I 4.1), and a wrapper has to be written around
                        // the name - which means writing the pair out.
                        let name = self.name(field.name);
                        let wrapped = match how.is_some() {
                            true => format!("Some({name})"),
                            false => name.to_string(),
                        };
                        match boxed {
                            true => out.push(&format!(": Box::new({wrapped})")),
                            false => out.push(&format!(": {wrapped}")),
                        }
                    }
                }
                out.push(" }");
                if cleaned {
                    out.push(")");
                }
            }
            // **`p with { x: p.x + 1 }`** is Rust's functional update
            // ([ADR-118](../../docs/specification/adr/adr-118.md) D1, D3): the
            // named fields, then `..base`, from which every field the program
            // did not name comes **by move**. No copy is inserted that the
            // program did not write ([ADR-282](../../docs/specification/adr/adr-282.md)
            // D3), which is why this is `..base` and not `..base.clone()`.
            //
            // The type is the checker's answer, read back under the byte the
            // `with` stands at. There is always one, because a `with` this
            // compiler could not name a type for was refused with `NK1173`.
            Expr::With { base, fields, at } => {
                let Some(owner) = self.with_types.get(&(*at as usize)).cloned() else {
                    return Err(refused_at!(
                        *at as usize,
                        "The type of this `with` is unknown. This is a compiler bug: the \
                         check should have reported it first."
                    ));
                };
                out.push(&format!("{owner} {{ "));
                for field in fields {
                    out.push(&self.name(field.name));
                    // **A boxed field is built in its box** (ADR-246 D3).
                    if self.is_boxed_field(&owner, self.text(field.name)) {
                        out.push(": Box::new(");
                        match &field.value {
                            Some(value) => self.expr(out, value, depth, flow)?,
                            None => out.push(&self.name(field.name)),
                        }
                        out.push("), ");
                        continue;
                    }
                    let either = self.either_field(&owner, field.name)
                        && !matches!(field.value, Some(Expr::LitNull));
                    match (&field.value, either) {
                        (Some(value), true) => {
                            out.push(": nikaia_std::either_text::either(");
                            self.expr(out, value, depth, flow)?;
                            out.push(")");
                        }
                        (Some(value), false) => {
                            out.push(": ");
                            self.expr(out, value, depth, flow)?;
                        }
                        (None, true) => out.push(&format!(
                            ": nikaia_std::either_text::either({})",
                            self.name(field.name)
                        )),
                        (None, false) => {}
                    }
                    out.push(", ");
                }
                out.push("..");
                self.expr(out, base, depth, flow)?;
                out.push(" }");
            }
            // A lambda's arguments are the ones it names (ADR-277): nothing is
            // read off the body, so `fn { … }` is `||`.
            Expr::Closure {
                params,
                mutable,
                body,
            } => {
                let names: Vec<String> =
                    params.iter().map(|p| self.name(*p).into_owned()).collect();
                // **A lambda where a function value is kept** is one shared
                // closure, and it takes what it captures with it.
                let kept = self
                    .kept_lambdas
                    .get(&(flow.statement, crate::check::argument_shape(expr)))
                    .copied();
                if let Some((pauses, fails)) = kept {
                    // Each hull it captures is a handle of its own
                    // (ADR-230 D4): cloned in a block around the closure, so
                    // the name outside is not moved into it.
                    let hulls = self
                        .kept_hulls
                        .get(&(flow.statement, crate::check::argument_shape(expr)))
                        .cloned()
                        .unwrap_or_default();
                    if !hulls.is_empty() {
                        out.push("{ ");
                        for hull in &hulls {
                            let hull = escaped(hull);
                            out.push(&format!("let {hull} = {hull}.clone(); "));
                        }
                    }
                    out.push(&format!(
                        "nikaia_std::func::Kept(std::sync::Arc::new(move |{}| ",
                        names.join(", ")
                    ));
                    let mut changed: Vec<Symbol> = flow.changed.to_vec();
                    changed.extend(mutable.iter().copied());
                    let inside = Flow {
                        in_lambda: !pauses,
                        throws: fails,
                        statement: flow.statement,
                        changed: &changed,
                        ..Flow::PLAIN
                    };
                    if pauses {
                        out.push("Box::pin(async move ");
                    }
                    // A type that may fail hands back a `Result`, as a
                    // `throws` function does: the value goes inside the `Ok`.
                    match fails {
                        true => {
                            out.push("{ Ok(");
                            self.block(out, body, depth, inside, Tail::Value)?;
                            out.push(") }");
                        }
                        false => self.block(out, body, depth, inside, Tail::Return)?,
                    }
                    if pauses {
                        out.push(&format!(") as {}<_>", self.boxed_future()));
                    }
                    out.push("))");
                    if !hulls.is_empty() {
                        out.push(" }");
                    }
                    return Ok(());
                }
                // **A lambda that pauses, where a `std` entry takes it**
                // ([ADR-233](../../docs/specification/adr/adr-233.md) D1): the
                // language below's own `async` closure, handed to the entry's
                // counterpart. Its body is an ordinary body that pauses.
                let pauses = self
                    .pausing_lambdas
                    .contains_key(&(flow.statement, crate::check::argument_shape(expr)));
                if pauses {
                    out.push("async ");
                }
                // **`filter` is handed a reference to its item** (ADR-231 D2):
                // where the item copies, the pattern takes it out.
                let pattern = self
                    .filter_patterns
                    .contains(&(flow.statement, crate::check::argument_shape(expr)));
                match (pattern, names.as_slice()) {
                    (true, [one]) => out.push(&format!("|&{one}| ")),
                    _ => out.push(&format!("|{}| ", names.join(", "))),
                }
                // **The `mut` ones reach inward** (ADR-281 D12), and the list is
                // the enclosing one plus this lambda's rather than this
                // lambda's alone: a lambda written inside an `update` block
                // still means the address when it names the outer `v`.
                let mut changed: Vec<Symbol> = flow.changed.to_vec();
                changed.extend(mutable.iter().copied());
                let flow = Flow {
                    changed: &changed,
                    ..flow
                };
                // A lambda's `return` leaves the lambda, not the function
                // around it, so it never carries the enclosing `Ok`. The
                // statement is kept, because the checker's answers about the
                // calls in here are keyed by it, and `in_lambda` is what makes
                // a pausing one refusable (ADR-055 §6).
                let inside = Flow {
                    in_lambda: !pauses,
                    statement: flow.statement,
                    // **And the `mut` parameters with it**: the body starts
                    // from `PLAIN` because a lambda's `return` leaves the
                    // lambda, and that must not throw away what D1 needs to
                    // write `(*v)`.
                    changed: &changed,
                    ..Flow::PLAIN
                };
                self.block(out, body, depth, inside, Tail::Return)?;
            }
            Expr::Unary { op, expr } => {
                // **`-2147483648` is an `i32`**, and its digits are not
                // ([ADR-285](../../../docs/specification/adr/adr-285.md) D23): the
                // question is about the value, so a negation is folded here
                // rather than left for `integer_literal` to answer about a number
                // that is one too large.
                if let (UnaryOp::Neg, Expr::LitInt { value, negative }) = (op, &**expr)
                    && let v = &crate::ast::int_value(*value, *negative)
                    && i32::try_from(-*v).is_ok()
                {
                    // The suffix still applies: a small negative number
                    // inside a constant written wide is written wide too,
                    // or the operands of one sum disagree.
                    let wide = match flow.widen {
                        true => "i64",
                        false => "",
                    };
                    out.push(&format!("-{v}{wide}"));
                    return Ok(());
                }
                // **A `&` over a slice read is the view it already is**
                // ([ADR-182](../../docs/specification/adr/adr-182.md) D2). The read
                // answers a `&str` or a `&[T]` since the `*` came off it, so
                // writing the `&` as well would make `&dna[i..<i + k]` a
                // `&&str` — which coerces in most places and is a type error
                // in the ones that matter, and is not what the source says
                // either: the source's `&` and the read's own view are one
                // claim written twice.
                if let (UnaryOp::Ref, Expr::Index { index, .. }) = (op, &**expr)
                    && self.slices(flow.statement, index)
                    && !flow.in_a_place
                {
                    return self.expr(out, expr, depth, flow);
                }
                out.push(unary_op(*op));
                self.nested(out, expr, u8::MAX, depth, flow)?;
            }
            Expr::Binary {
                op,
                lhs,
                rhs,
                span: at,
                ..
            } => {
                // **A constant sum takes the first type that holds it**, the
                // way a constant does
                // ([ADR-285](../../../docs/specification/adr/adr-285.md) D22).
                // Asked only on the outermost expression that folds - `widen`
                // reaching inward is what makes the operands agree - and never
                // where the position decides the type.
                let flow = match flow.widen || flow.inferred {
                    true => flow,
                    false => match crate::fold::constant_of(expr, &crate::fold::nothing_is_known) {
                        Some(folded) if crate::fold::wants_widening(&folded) => flow.widened(),
                        _ => flow,
                    },
                };
                // **A `+` that joins text is a call**
                // ([ADR-081](../../docs/specification/adr/adr-081.md) D2).
                // Rust's operator takes `String + &str` and nothing else, so
                // three of the four shapes a program can write went below and
                // were refused there. `concat::plus` has an impl per shape and
                // the language below picks, which is `index::at`'s arrangement
                // a third time - and it keeps `String + &str` exactly as fast
                // as it is today, which a `format!` for every shape would not.
                //
                // **Only where the checker said so**, by the operator's own
                // span: a number's `+` may not move into `std`, where
                // `overflow-checks` is off and inlining would not carry the
                // abort across (ADR-285 D5).
                if self.concatenations.contains(&at.at()) {
                    out.push("nikaia_std::concat::plus(");
                    self.concatenated(out, lhs, depth, flow)?;
                    out.push(", ");
                    self.concatenated(out, rhs, depth, flow)?;
                    out.push(")");
                    return Ok(());
                }
                // **`x == null` asks whether there is one** (0.0.250):
                // `x.is_none()`, which needs nothing of what `x` holds, where
                // `x == None` needs it to compare - and is what `clippy`
                // refuses in `std`.
                if matches!(op, BinaryOp::Eq | BinaryOp::Ne)
                    && let Some(side) = match (lhs.as_ref(), rhs.as_ref()) {
                        (Expr::LitNull, other) | (other, Expr::LitNull)
                            if !matches!(other, Expr::LitNull) =>
                        {
                            Some(other)
                        }
                        _ => None,
                    }
                {
                    self.postfix_base(out, side, depth, flow)?;
                    out.push(match op {
                        BinaryOp::Eq => ".is_none()",
                        _ => ".is_some()",
                    });
                    return Ok(());
                }
                // **A counter that is never negative is compared with a length
                // as a `usize`** ([ADR-270](../../docs/specification/adr/adr-270.md)
                // D8 step 1): `i < xs.len()` is `(i as usize) < xs.len()`, the
                // same comparison for `i >= 0`, and the one the optimiser can
                // match with the bounds check of `xs[i]` and drop it.
                if op.is_comparison()
                    && let Some(compared) = self.against_a_length(out, op, lhs, rhs, depth, flow)?
                {
                    return Ok(compared);
                }
                // **An operation proved to stay inside its type** is written
                // without its check, naming the type the proof was about
                // ([ADR-306](../../docs/specification/adr/adr-306.md) D10).
                if let Some(ty) = self.proven_arithmetic.get(&at.at())
                    && !flow.widen
                    && !flow.inferred
                {
                    let name = match op {
                        BinaryOp::Add => "wrapping_add",
                        BinaryOp::Sub => "wrapping_sub",
                        _ => "wrapping_mul",
                    };
                    out.push(&format!("<{ty}>::{name}("));
                    self.expr(out, lhs, depth, flow)?;
                    out.push(", ");
                    self.expr(out, rhs, depth, flow)?;
                    out.push(")");
                    return Ok(());
                }
                // Parenthesised only where precedence needs it: the operators
                // mean the same in both languages, so `value * 10 + n` should
                // come out the way it went in.
                let here = precedence(*op);
                // **A view compared with a value is read** (0.0.238,
                // `check::Checked::compared_views`).
                let read = |side: &str| self.compared_views.contains(&(at.at(), side.to_string()));
                if read("lhs") {
                    out.push("*");
                }
                self.nested(out, lhs, here, depth, flow)?;
                out.push(&format!(" {} ", binary_op(*op)));
                if read("rhs") {
                    out.push("*");
                }
                self.nested(out, rhs, here + 1, depth, flow)?;
            }
            // **A fallback that jumps is a `match` and not a closure**
            // ([ADR-276](../../../docs/specification/adr/adr-276.md) D20, and
            // [ADR-276](../../../docs/specification/adr/adr-276.md) D16's rule
            // about what a jump may not cross).
            //
            // `unwrap_or_else` takes a **closure**, so a `return` written in
            // the fallback would return from the closure and the program would
            // carry on — `let user = find(id) ?? throw NotFound(id)`, the shape
            // this record's own example is written in, would have thrown
            // nothing and bound a value nobody produced. The `match` puts the
            // jump in the function it was written in.
            //
            // **And so is one whose fallback is another `??` that jumps**:
            // `touches[key] ?? described(key) ?? continue` reads as
            // `touches[key] ?? (described(key) ?? continue)`, and the inner
            // `match` written inside the outer closure put the `continue` in a
            // closure all the same - `rustc`'s E0267 (found moving
            // `contracts::locks` into Nikaia).
            Expr::Coalesce { value, fallback }
                if jumps_at_the_end(fallback) || self.never_returns(fallback) =>
            {
                out.push("match ");
                self.expr(out, value, depth, flow)?;
                // **A loan on the left is opened, and the answer is a view**
                // ([ADR-279](../../docs/specification/adr/adr-279.md) D10).
                if let Some(opened) = self
                    .lent_coalesces
                    .get(&(flow.statement, crate::check::argument_shape(expr)))
                {
                    out.push(opened);
                }
                // **A number read past a jump is read out of the view**
                // (#456, `Checked::copied_jump_reads`).
                let copied = self
                    .copied_jump_reads
                    .contains(&(flow.statement, crate::check::argument_shape(expr)));
                out.push(if copied {
                    " { Some(__nikaia_value) => nikaia_std::num::value(__nikaia_value), None => "
                } else {
                    " { Some(__nikaia_value) => __nikaia_value, None => "
                });
                self.expr(out, fallback, depth, flow)?;
                out.push(" }");
            }
            // **A `??` whose answer is only read lends its left side**
            // ([ADR-279](../../docs/specification/adr/adr-279.md) D5): the
            // option is opened (`.as_deref()` for text, `.as_ref()` for the
            // rest) and the fallback is a view of what it names, so the answer
            // is a view and the name stays usable after the line.
            Expr::Coalesce { value, fallback }
                if self
                    .lent_coalesces
                    .contains_key(&(flow.statement, crate::check::argument_shape(expr))) =>
            {
                let opened =
                    self.lent_coalesces[&(flow.statement, crate::check::argument_shape(expr))];
                out.push("nikaia_std::index::or(");
                self.expr(out, value, depth, flow)?;
                out.push(opened);
                out.push(", || ");
                match (&**fallback, opened) {
                    (Expr::LitStr { .. }, _) => self.expr(out, fallback, depth, flow)?,
                    (Expr::Variable(_), ".as_deref()") => {
                        out.push("std::convert::AsRef::<str>::as_ref(&");
                        self.expr(out, fallback, depth, flow)?;
                        out.push(")");
                    }
                    _ => {
                        out.push("&");
                        self.expr(out, fallback, depth, flow)?;
                    }
                }
                out.push(")");
            }
            Expr::Coalesce { value, fallback } => {
                // Kap 3.5. `into()` because the fallback is written as the
                // value it stands for, not as the type the option holds -
                // `text ?? "none"` on a `String?` is the case it exists for.
                //
                // **Except for a number**, and that exception is Part I 2.4
                // rather than a special case: a number literal takes the type
                // its use asks for, so `0` already *is* whatever the option
                // holds and `0.into()` adds a conversion that has to be
                // resolved. Where the option's own type is still open - a
                // `channel::bounded` whose element type nothing has pinned yet
                // ([ADR-149](../../docs/specification/adr/adr-149.md)) - there
                // is nothing to resolve it from, and what a reader got was
                // *"type annotations needed"* about a file nobody wrote
                // (Part III, C.1). Without the `into` the literal is simply
                // one more use of the type, which is what decides it.
                //
                // **`index::or` and not `unwrap_or_else`**
                // ([ADR-293](../../docs/specification/adr/adr-293.md) D4): the
                // left of a `??` may now be a **view into a container**, because
                // a map read answers `Option<&V>` — the value reached is the
                // map's, and copying it is never the compiler's to do
                // ([ADR-283](../../docs/specification/adr/adr-283.md) D3). The
                // fallback is still written as the value it stands for, so the
                // two sides do not have the same type and the language below is
                // what joins them.
                // **And except for text where the left side is a view of text**
                // (ADR-283): the literal already is one.
                //
                // **And except for a variant of an enum this file declares**:
                // `Ty::Unknown` is already a `Ty`, and `.into()` on it is a
                // conversion to the same type, which the language below's
                // lint calls useless (found moving `Ty::parse` into Nikaia,
                // ADR-294).
                let a_declared_variant = match &**fallback {
                    Expr::Path(segments) => {
                        segments.len() == 2
                            && self.is_variant(self.text(segments[0]), self.text(segments[1]))
                    }
                    // ...and one built with its parts, `Ty::Tuple([])`.
                    Expr::Call { func, .. } => matches!(&**func, Expr::Path(segments)
                        if segments.len() == 2
                            && self.is_variant(self.text(segments[0]), self.text(segments[1]))),
                    _ => false,
                };
                // **And except for a constructor of a type this file
                // declares**, `FnContract::empty()`: its entry says it hands
                // back the type itself, so `.into()` is the same useless
                // conversion (found moving the ledger's reader, ADR-294).
                let a_declared_constructor = match &**fallback {
                    Expr::Call { func, .. } => match &**func {
                        Expr::Path(segments) if segments.len() == 2 => {
                            let owner = self.text(segments[0]);
                            let key = format!("{owner}::{}", self.text(segments[1]));
                            self.own_contracts
                                .functions
                                .get(&key)
                                .and_then(|contract| contract.signature.as_ref())
                                .and_then(|signature| signature.result.as_ref())
                                .is_some_and(|result| {
                                    matches!(result, crate::contracts::ty::Ty::Named { name, args, view: false }
                                        if name == owner && args.is_empty())
                                })
                        }
                        _ => false,
                    },
                    _ => false,
                };
                // **And a struct written out**, `Held { … }`, or a `bool`: it
                // is the type already, whatever the option holds.
                let a_struct_written =
                    matches!(&**fallback, Expr::StructLit { .. } | Expr::LitBool(_));
                let bare = a_number(fallback)
                    || a_declared_variant
                    || a_declared_constructor
                    || a_struct_written
                    || matches!(&**fallback, Expr::LitStr { at, .. } if self.view_fallbacks.contains(&self.text_at(*at as usize)))
                    || self
                        .view_coalesces
                        .contains(&(flow.statement, crate::check::argument_shape(expr)));
                // **A fallback that may be `null` too** (Part I 3.5): the
                // first option that has a value, which is the language
                // below's `or_else`, and the answer is an option still.
                if self
                    .optional_fallbacks
                    .contains(&(flow.statement, crate::check::argument_shape(expr)))
                {
                    out.push("nikaia_std::index::or_maybe(");
                    self.expr(out, value, depth, flow)?;
                    out.push(", || ");
                    self.expr(out, fallback, depth, flow)?;
                    out.push(")");
                    return Ok(());
                }
                out.push("nikaia_std::index::or(");
                // **A list's optional text is lent** (#342): the read is a
                // place, and `&` in front of it borrows the element.
                if self
                    .lent_list_reads
                    .contains(&(flow.statement, crate::check::argument_shape(expr)))
                {
                    out.push("&");
                }
                self.expr(out, value, depth, flow)?;
                out.push(", || ");
                // **A name lent beside what a map holds**
                // (`check::Checked::lent_map_fallbacks`): the answer is a
                // view, and so is the fallback.
                if self
                    .lent_map_fallbacks
                    .contains(&(flow.statement, crate::check::argument_shape(expr)))
                {
                    out.push("&");
                    self.expr(out, fallback, depth, flow)?;
                    out.push(")");
                    return Ok(());
                }
                self.expr(out, fallback, depth, flow)?;
                out.push(match bare {
                    true => ")",
                    false => ".into())",
                });
            }
            Expr::TryCatch { expr, handler } => {
                // Kap 7.1: the handler sees the error as `error`.
                out.push("match ");
                // `catch` needs the `Result`, not the value: a `dsl … from …`
                // propagates on its own everywhere else, and here the handler
                // is what handles it.
                // A written call inside the guarded half must not take the
                // `?`: the `match` below is what handles the failure. A grammar
                // entry is one of those calls now
                // ([ADR-296](../../docs/specification/adr/adr-296.md) D24), so it
                // needs no arm of its own — which is what that record meant by
                // *an ordinary call*.
                // **A joining block is told the handler is right here**
                // ([ADR-292](../../docs/specification/adr/adr-292.md) D18), and
                // only when it *is* the guarded half: one nested in an argument
                // hands back a tuple like any other value.
                let guarded = match &**expr {
                    Expr::Overlap(_) | Expr::Select(_) => flow.joined_here(),
                    _ => flow.guarded(),
                };
                self.expr(out, expr, depth, guarded)?;
                let pad = "    ".repeat(depth + 1);
                let close = "    ".repeat(depth);
                // **A handler that does not read the error binds `_error`**
                // ([ADR-308](../../docs/specification/adr/adr-308.md)). Kap 7.1
                // gives the failure that name whether or not the handler wants
                // it, so `catch { 1000 }` - the shape Part I 7.1 teaches first -
                // used to get `warning: unused variable: error` about a binding
                // that exists nowhere in the program, which is
                // [Part III C.1](../../docs/specification/30-nikaia-tooling.md)
                // one severity down.
                //
                // The question is `contracts::order`'s, already over-approximate
                // and already asked of this very block for the ordering: a false
                // *yes* keeps today's binding, and a false *no* would not
                // compile.
                let bound =
                    match crate::contracts::order::block_mentions(self.parsed, handler, CAUGHT) {
                        true => CAUGHT,
                        false => "_error",
                    };
                // **What the handler was handed**
                // ([ADR-280](../../docs/specification/adr/adr-280.md) D10). A
                // named channel hands it an **envelope** — the author's error
                // plus the site it was raised at — and what Part I 7.1 gives
                // the handler is the error: `match error { ConfigError::… }`
                // names variants, and `{error}` is the message the author
                // wrote. So the envelope is opened **once, at the binding**,
                // and the site is kept in a local beside it for the one
                // statement that needs it back (`throw error`, D3).
                let named = bound == CAUGHT && self.caught_channel_is_named(expr);
                let sum = match bound == CAUGHT {
                    true => self.caught_sum(expr),
                    false => None,
                };
                let flow = flow.handling(named).catching(sum);
                // **The envelope is opened as the handler's first statement**
                // ([ADR-292](../../docs/specification/adr/adr-292.md) D20), and
                // not as a block wrapped around it. A block holding one
                // expression, inside another block, is `unused_braces` — a
                // warning about a file nobody wrote, which is
                // [Part III C.1](../../docs/specification/30-nikaia-tooling.md)
                // one severity down, and `catch { println(f"{error}") }` over a
                // named channel is all it took.
                let opening = named.then(|| format!("let ({CAUGHT}, {SITE}) = {CAUGHT}.split();"));
                out.push(&format!(
                    " {{\n{pad}Ok(value) => value,\n{pad}Err({bound}) => "
                ));
                // Kap 7.1 and ADR-292: the handler's last statement is the
                // value of the `catch`, so a `return` in it is the *function's*
                // return and has to stay one. `contracts::order`'s `diverts`
                // counts exactly that `return` when it refuses to overlap the
                // guarded read, so dropping it here made the ordering analysis
                // reason about a control flow the emitted program did not have.
                self.block_opening_with(
                    out,
                    handler,
                    depth + 1,
                    flow,
                    Tail::Value,
                    opening.as_deref(),
                )?;
                out.push(&format!(",\n{close}}}"));
            }
            Expr::Try(inner) => {
                self.postfix_base(out, inner, depth, flow)?;
                out.push("?");
            }
            // Kap 7.1: `throw e` leaves the function with `e` in the failure
            // channel. `Box::new` is what puts it there, and it is the emitter's
            // to write rather than the author's - ADR-023 D2 gives the language
            // one way to raise an error, not one way plus a conversion.
            // Kap 7.1: `throw e` leaves the function with `e` in the failure
            // channel. `raise` is what puts it there, and it carries the site
            // the compiler knew and the author did not have to write (D6). The
            // trace it may attach is off unless the program asked - measured at
            // 28 300 instructions an error, which is not a default.
            // **The four jumps, written as the language below writes them**
            // ([ADR-276](../../../docs/specification/adr/adr-276.md) D20): each
            // is `!` there too, so an arm that returns beside an arm that hands
            // back a value is a `match` of that value's type by the backend's
            // own rule and not by a coercion this had to write.
            // **What it hands back is wrapped by its own address**
            // (`check::Checked::wrapped`, read in [`Emitter::expr`]): the
            // statement it stands in is a `let`'s or a call's, and that
            // statement's wrap is theirs.
            Expr::Return(value) => match (&**value, flow.throws) {
                (Some(value), true) => {
                    out.push("return Ok(");
                    self.expr(out, value, depth, flow)?;
                    out.push(")");
                }
                (Some(value), false) => {
                    out.push("return ");
                    self.expr(out, value, depth, flow)?;
                }
                (None, true) => out.push("return Ok(())"),
                (None, false) => out.push("return"),
            },
            Expr::Break | Expr::Continue => {
                let word = match expr {
                    Expr::Break => "break",
                    _ => "continue",
                };
                // **The same refusal the statement form has**
                // ([ADR-276](../../../docs/specification/adr/adr-276.md) D18):
                // the lowering states it too, so a path that reached here
                // without the checker's answer still writes no file `rustc`
                // would refuse.
                if !flow.in_loop {
                    return Err(refused_at!(
                        flow.statement,
                        "`{word}` only works inside a loop, and there's none here."
                    ));
                }
                out.push(word);
            }
            Expr::Throw(inner) => {
                // **`throw error` inside a handler passes the error on**
                // ([ADR-280](../../docs/specification/adr/adr-280.md) D11), with
                // the site it was **raised** at and not the one it was caught
                // at, which is what
                // [ADR-023](../../docs/specification/adr/adr-023.md) D6 asks
                // for. The handler's binding opened the envelope; this is where
                // it is closed again.
                let is_the_binding =
                    matches!(inner.as_ref(), Expr::Variable(name) if self.text(*name) == CAUGHT);
                let passing_on = flow.caught_named && is_the_binding && flow.caught_sum.is_none();
                if passing_on || (flow.caught_member.is_some() && is_the_binding) {
                    // **Back into the variant it came out of**, where the
                    // handler is inside a sum's member arm
                    // ([ADR-280](../../docs/specification/adr/adr-280.md) D17).
                    let put_back = |inner: String| match (flow.caught_sum, flow.caught_member) {
                        (Some(sum), Some(member)) => format!(
                            "{}::{}({inner})",
                            Self::sum_path(sum),
                            Self::sum_variant(member)
                        ),
                        _ => inner,
                    };
                    let held = match flow.caught_named {
                        true => format!("{SITE}.refill({CAUGHT})"),
                        false => CAUGHT.to_string(),
                    };
                    out.push(&format!("return Err({})", put_back(held)));
                    return Ok(());
                }
                // Three channels and three ways in. The **envelope** takes the
                // value and the site; the **box** takes it boxed, which is what
                // `raise` does and what needs `'static`; and a **library's
                // type** takes the value alone, because a channel that carries
                // no envelope has nowhere to put a site
                // ([ADR-280](../../docs/specification/adr/adr-280.md) D13).
                // **A sum takes the member it is**
                // ([ADR-280](../../docs/specification/adr/adr-280.md) D16), and
                // `into()` is what picks the variant: the `From` beside the
                // generated `enum` is written per member, so the thrown value
                // says which one it is by its own type.
                if self.sum_of(flow.function).is_some() {
                    let thrown = crate::contracts::throws::error_type(self.parsed, inner);
                    let own = thrown
                        .as_deref()
                        .and_then(|t| self.name_of_error(t))
                        .is_some();
                    match own {
                        true => {
                            out.push("return Err(nikaia_std::error::throwing(");
                            self.expr(out, inner, depth, flow)?;
                            out.push(&format!(", &{:?}).into())", flow.origin));
                        }
                        false => {
                            out.push("return Err(");
                            self.expr(out, inner, depth, flow)?;
                            out.push(".into())");
                        }
                    }
                    return Ok(());
                }
                // **A `throw` has a site whoever declared the error**
                // ([ADR-280](../../docs/specification/adr/adr-280.md) D14): a
                // library's error thrown by this program was raised here.
                match self.named_error(flow.function) {
                    Some(Named::Own(_) | Named::Library(_)) => {
                        out.push("return Err(nikaia_std::error::throwing(")
                    }
                    None => out.push("return Err(nikaia_std::error::raise("),
                }
                self.expr(out, inner, depth, flow)?;
                out.push(&format!(", &{:?}))", flow.origin));
            }
            // **Part I 8.2 and ADR-055 D5: a task.**
            //
            // `spawn fn { … }` is `TaskHandle::start(async move { … })`. Three
            // things are in that one line and each is a decision somewhere
            // else:
            //
            //   * an `async` **block** and not a closure, because a body is a
            //     block: wrapping it in a closure to call it once adds a call
            //     and takes nothing away - the same reason every vehicle in
            //     `task` takes futures (§6 step 3). Not because the language
            //     below has no `async` closure, which is what this line used to
            //     say and is false
            //     ([ADR-277](../../docs/specification/adr/adr-277.md) D13, D16);
            //   * `move`, which is Part I 8.3's implicit move: the captures go
            //     with the task because it may outlive the function that
            //     started it, and `NK2101` is what stands in front of that for
            //     data the parent still wanted;
            //   * a **handle**, always, whether or not the program keeps it. A
            //     task nobody joins still runs (D5), because the executor owns
            //     it - so an unused handle is a value dropped and not work
            //     cancelled.
            //
            // The lambda's `params` are not written: a task is handed nothing,
            // so a `spawn fn (x) { … }` has no argument to bind. `check`
            // refuses that rather than dropping it silently.
            Expr::Spawn { body, .. } => {
                let Expr::Closure { body, .. } = body.as_ref() else {
                    return Err(refused_at!(
                        flow.statement,
                        "`spawn` takes a lambda. Write `spawn fn {{ … }}`."
                    ));
                };
                // **And which of the two starters**, which is the one place
                // `user_parallelism` reaches a `spawn` (ADR-037 D2). At `yes` a
                // task may be polled on a thread that did not start it, so its
                // future has to be `Send` (§2 D6); at `no` it may not, so it
                // does not - and asking for `Send` there would refuse a task
                // holding the plain count ADR-312 D9 gives a `Shared` at one
                // user thread. One Nikaia line, two lowerings, and the switch
                // is what chooses.
                // **A handle the body names is duplicated, not moved**
                // ([ADR-312](../../../docs/specification/adr/adr-312.md) D1).
                // The step is written *outside* the future, in a block of its
                // own, so the name the body moves is the new handle and the
                // caller's own survives the `spawn` - which is the whole of D1
                // for the task half. Unconditional (D2): a line further down may
                // not decide what a line further up does to a cleanup point.
                let handles: Vec<&str> = self
                    .task_handles
                    .iter()
                    .filter(|(at, _)| *at == flow.statement)
                    .map(|(_, name)| name.as_str())
                    .collect();
                if !handles.is_empty() {
                    out.push("{ ");
                    for name in &handles {
                        out.push(&format!("let {name} = {name}.clone(); "));
                    }
                }
                match self.build.overlaps_user_code() {
                    true => out.push("nikaia_std::task::TaskHandle::start_on_pool(async move "),
                    false => out.push("nikaia_std::task::TaskHandle::start(async move "),
                }
                // `Flow::PLAIN`, like any other lambda body: a `return` inside
                // the task leaves the task, and a failure in it is the task's.
                // `in_lambda` is deliberately **not** set - the body is a
                // future, so a call that pauses is exactly what it may hold.
                let inside = Flow {
                    statement: flow.statement,
                    ..Flow::PLAIN
                };
                self.block(out, body, depth, inside, Tail::Return)?;
                out.push(")");
                if !handles.is_empty() {
                    out.push(" }");
                }
            }
            other => {
                return Err(refused_at!(
                    flow.statement,
                    "The compiler can't translate this expression yet: {other:?}"
                ));
            }
        }
        Ok(())
    }

    /// Whether `receiver.method(…)` **enters a grammar**: a receiver naming one
    /// of this file's grammars and a method naming one of its entries
    /// ([ADR-296](../../docs/specification/adr/adr-296.md) D24, D25).
    fn enters_a_grammar(&self, receiver: &Expr, method: Symbol) -> bool {
        let Expr::Variable(name) = receiver else {
            return false;
        };
        self.grammars
            .get(name)
            .is_some_and(|def| def.rules.iter().any(|r| r.is_entry && r.name == method))
    }

    /// A call. `Stats(temp)` is the anonymous constructor of Kap 4.2 - Rust has
    /// no such thing, so the declaration decides: a name that is a struct is a
    /// call to the `new` its `impl` provides.
    fn call(
        &self,
        out: &mut Out,
        func: &Expr,
        args: &[Expr],
        config: &[crate::ast::ConfigArg],
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        // **A call to a function with a precondition** reaches its checked
        // entry unless the prover saw this call prove it, or check it here
        // with the call's arguments in place of the parameters, so that a
        // failure names the caller
        // ([ADR-269](../../docs/specification/adr/adr-269.md) D20). The
        // arguments of a checked call are ones the prover reads, so reading
        // them once more changes nothing.
        // A function of this file with a checked entry, or one of another
        // package whose ledger states a precondition: that package was
        // lowered with the two entries (ADR-269 D18, D20).
        let callee = match func {
            Expr::Variable(name) if self.entries.contains_key(self.text(*name)) => {
                Some(self.text(*name).to_string())
            }
            Expr::Path(segments) => {
                let qualified = self.parsed.unaliased(
                    &segments
                        .iter()
                        .map(|s| self.text(*s))
                        .collect::<Vec<_>>()
                        .join("::"),
                );
                self.own_contracts
                    .functions
                    .get(&qualified)
                    .is_some_and(|c| !c.requires.is_empty())
                    .then_some(qualified)
            }
            _ => None,
        };
        if let Some(callee) = callee
            && let Some(reach) =
                self.reaches
                    .get(&crate::prove::call_key(self.parsed, &callee, args))
        {
            match reach {
                crate::prove::Reach::Proved => {
                    return self.call_inner(out, func, args, config, depth, flow, true);
                }
                crate::prove::Reach::Checked(checks) => {
                    // `(check, call).1`: an expression in every position the
                    // call could stand in, which a block at the start of a
                    // statement is not.
                    out.push(if checks.len() == 1 { "(" } else { "({ " });
                    for check in checks {
                        out.push(&failed_check(check));
                    }
                    out.push(if checks.len() == 1 { ", " } else { "}, " });
                    self.call_inner(out, func, args, config, depth, flow, true)?;
                    out.push(").1");
                    return Ok(());
                }
                crate::prove::Reach::Through => {}
            }
        }
        self.call_inner(out, func, args, config, depth, flow, false)
    }

    /// A call, past [`Self::call`]'s precondition check: to the unchecked
    /// entry where `unchecked` says so (ADR-269 D20).
    #[allow(clippy::too_many_arguments)]
    fn call_inner(
        &self,
        out: &mut Out,
        func: &Expr,
        args: &[Expr],
        config: &[crate::ast::ConfigArg],
        depth: usize,
        flow: Flow<'_>,
        unchecked: bool,
    ) -> Result<()> {
        // **An `assert` is a claim checked here** (ADR-269 D2, D4) unless it
        // was proved, and the failure is
        // written with what only this compiler knew - the claim as written
        // and its operands by name. The condition changes nothing (D3), which
        // is what makes reading an operand a second time, for the message,
        // the same value it compared.
        if let ([condition], Expr::Variable(name)) = (args, func)
            && self.text(*name) == crate::check::ASSERT
            && let Some(claim) = self
                .claims
                .get(&(flow.statement, crate::check::argument_shape(condition)))
        {
            // **A claim held before the program runs costs nothing when it
            // does** ([ADR-269](../../docs/specification/adr/adr-269.md) D4,
            // D5): a precondition is checked by the calls that do not prove
            // it, and every other claim here.
            if matches!(
                claim.held,
                Some(crate::prove::Held::Proved | crate::prove::Held::Precondition(..))
            ) {
                out.push("()");
                return Ok(());
            }
            out.push("if !(");
            self.expr(out, condition, depth, flow)?;
            out.push(&format!(
                ") {{ nikaia_std::abort::assertion_failed({:?}, ",
                claim.written
            ));
            match config.iter().find(|c| self.text(c.name) == "message") {
                Some(message) => {
                    out.push("Some(&(");
                    self.expr(out, &message.value, depth, flow)?;
                    out.push(")), ");
                }
                None => out.push("None, "),
            }
            out.push("&[");
            if let Expr::Binary { lhs, rhs, .. } = condition {
                let shown = [(&claim.left, lhs), (&claim.right, rhs)];
                for (label, operand) in shown {
                    let Some(label) = label else { continue };
                    out.push(&format!("({label:?}, (&&nikaia_std::abort::Operand(&("));
                    self.expr(out, operand, depth, flow)?;
                    out.push("))).shown()), ");
                }
            }
            out.push("]) }");
            return Ok(());
        }
        // **A kept function value called by name** is the closure it holds,
        // awaited where its type may pause and propagated where it may fail.
        if let Expr::Variable(name) = func {
            let key = (flow.statement, self.text(*name).to_string());
            if let Some((pauses, fails)) = self.kept_calls.get(&key).copied() {
                out.push(&format!("{}(", self.name(*name)));
                for (at, arg) in args.iter().enumerate() {
                    if at > 0 {
                        out.push(", ");
                    }
                    self.expr(out, arg, depth, flow)?;
                }
                out.push(")");
                if pauses {
                    out.push(".await");
                }
                if fails && flow.throws && !flow.caught {
                    out.push("?");
                }
                return Ok(());
            }
        }
        // ADR-055 D6: a recursive `async fn` is an infinitely sized future, and
        // Rust says so rather than guessing - so a call that closes a cycle
        // through pausing functions puts the future behind a pointer.
        // `pausing_reach` is computed once per unit (see that function for why
        // it over-approximates), and the emitter only looks names up in it: it
        // still resolves nothing (ADR-296 D17).
        // **A grammar is entered by an ordinary call**
        // ([ADR-296](../../docs/specification/adr/adr-296.md) D24), through a
        // **path** since [ADR-140](../../docs/specification/adr/adr-140.md) D3:
        // `Json::value(input)`, where `Json` names a grammar in this file and
        // `value` one of its `entry` rules. First, because it is not a call to
        // anything the recursion graph or the ledger knows.
        if let Expr::Path(segments) = func
            && let [grammar, rule] = segments.as_slice()
            && self.grammars.contains_key(grammar)
            && args.len() == 1
        {
            return self.grammar_entry(out, *grammar, *rule, &args[0], depth, flow);
        }
        // **A variant with a boxed part is built with the part in its box**
        // ([ADR-246](../../docs/specification/adr/adr-246.md) D3). The mask
        // is `args`' to read, so every other thing an argument is written
        // with still happens inside the box.
        if let Expr::Path(segments) = func
            && let [.., owner, variant] = segments.as_slice()
        {
            let (owner, variant) = (self.text(*owner), self.text(*variant));
            let mask: Vec<bool> = (0..args.len())
                .map(|at| {
                    self.is_boxed(owner, &crate::check::boxed_member(variant, Some(at), None))
                })
                .collect();
            let holds_a_hull = self.is_variant(owner, variant)
                && args.iter().any(|arg| {
                    matches!(arg, Expr::Call { func, .. }
                        if matches!(func.as_ref(), Expr::Variable(name)
                            if matches!(self.text(*name), SHARED | SHARED_MUT | LOCKED)))
                });
            if mask.iter().any(|b| *b) || holds_a_hull {
                *self.boxed_args.borrow_mut() = Some(mask);
                *self.variant_counts.borrow_mut() = Some(
                    (0..args.len())
                        .map(|at| self.count_at(SHARED_FIELDS, &variant_slot(owner, variant, at)))
                        .collect(),
                );
                out.push(&self.path(&segments.iter().map(|s| self.text(*s)).collect::<Vec<_>>()));
                out.push("(");
                let written = self.args(out, variant, args, &[], depth, flow);
                *self.boxed_args.borrow_mut() = None;
                *self.variant_counts.borrow_mut() = None;
                written?;
                out.push(")");
                return Ok(());
            }
        }
        let pausing = self.pausing_key(func);
        if let Some(key) = pausing.as_deref().filter(|_| flow.in_lambda) {
            return Err(pausing_in_a_lambda(flow.statement, key));
        }
        // **A call to a code parameter whose type may pause**
        // ([ADR-277](../../docs/specification/adr/adr-277.md) D11). It hands back
        // a boxed future, so it is awaited — and it is never *boxed* here: a
        // parameter is not a name the recursion graph knows, and what it holds
        // is already behind a pointer.
        let awaits_a_parameter =
            matches!(func, Expr::Variable(name) if flow.awaited.contains(name));
        // **A call of a `sync(f)` function** (ADR-288 D31): awaited where the
        // caller can pause, and driven once where it cannot - the checker has
        // then proved the lambda handed to it does not pause, so neither does
        // the call.
        let decided = pausing.is_none() && self.decided_by_its_lambda(func);
        let settled = decided && (flow.in_lambda || !self.lowered_pausing(flow.function));
        if settled {
            out.push("nikaia_std::rt::exec::settled(");
        }
        let boxed = pausing
            .as_deref()
            .is_some_and(|key| self.closes_a_pausing_cycle(flow.function, key));
        if boxed {
            out.push("Box::pin(");
        }

        // **A handle is made at the call and nowhere else**
        // ([ADR-302](../../docs/specification/adr/adr-302.md) D11, D12). The
        // declaration hands back the address C returned; the hull goes on here,
        // where the claim the declaration made can be checked — or, where it
        // said `?`, where `None` is C's own `NULL`.
        let hull = self.handle_from(func);
        if let Some((handle, absent)) = &hull {
            out.push(&match absent {
                true => format!("{handle}::maybe("),
                false => format!("{handle}::from_c(\"{}\", ", self.called_name(func)),
            });
        }

        self.called(out, func, args, config, depth, flow, unchecked)?;

        if hull.is_some() {
            out.push(")");
        }

        if boxed {
            out.push(")");
        }

        // **ADR-055 D2: the `.await` goes before the `?`**, and the order is not
        // a choice: the future is what can fail, so it has to be driven before
        // there is a `Result` to propagate. `f().await?` and never `f()?.await`.
        if pausing.is_some() || awaits_a_parameter || (decided && !settled) {
            out.push(".await");
        }
        if settled {
            out.push(")");
        }

        // ADR-023 D8: `throws` propagates on its own, so a call to something
        // that can fail is where the failure leaves - and in the language below
        // that is spelled `?`. This is the written-call twin of the `?` a
        // fallible loop's step gets above: the same rule, one line earlier
        // (ADR-025 D1).
        //
        // Three conditions, and each removes a way of being wrong. The
        // enclosing function must be `throws`, or there is nowhere for the `?`
        // to go - and `NK2605` has already refused the program where it is
        // not, so this is not a silent choice. The call must not be the
        // guarded half of a `catch`, which wants the `Result` itself. And the
        // callee's contract must **say** it can fail, so nothing is added on a
        // guess: a call no ledger describes is left exactly as it was.
        //
        // A lambda's body is emitted with `Flow::PLAIN`, so a fallible call
        // inside one never takes a `?` from the function around it - which is
        // right, because its `return` leaves the lambda and not the function.
        if flow.throws && !flow.caught && self.can_fail(func) {
            out.push("?");
        }
        Ok(())
    }

    /// The call itself, without ADR-023 D8's propagation.
    #[allow(clippy::too_many_arguments)]
    fn called(
        &self,
        out: &mut Out,
        func: &Expr,
        args: &[Expr],
        config: &[crate::ast::ConfigArg],
        depth: usize,
        flow: Flow<'_>,
        unchecked: bool,
    ) -> Result<()> {
        // **A call to a function that walks a type's fields goes to the copy**
        // ([ADR-304](../../docs/specification/adr/adr-304.md) D6), and which
        // copy is the checker's answer: this emitter has no types
        // ([ADR-288](../../docs/specification/adr/adr-288.md)), so the name is
        // handed over keyed by the byte the call stands at.
        if let Some(copy) = self.unrolled_calls.get(&flow.statement) {
            // **Only the call to the walking function**: the key is the
            // statement, and `println(describe(u))` holds two calls.
            let walker = matches!(func, Expr::Variable(name)
                if copy.starts_with(&format!("{}__", self.text(*name))));
            if walker {
                out.push(&format!("{copy}("));
                for (at, arg) in args.iter().enumerate() {
                    if at > 0 {
                        out.push(", ");
                    }
                    self.expr(out, arg, depth, flow)?;
                }
                out.push(")");
                return Ok(());
            }
        }
        if let Expr::Variable(name) = func {
            let text = self.text(*name);

            // **A door over several locks**
            // ([ADR-281](../../../docs/specification/adr/adr-281.md)): the locks
            // go in by reference and the block becomes the last argument, which
            // is what the trailing-lambda rule already made of it. The ordered
            // acquisition is `std`'s, because it is about addresses at run time
            // and not about anything this compiler can see.
            if let Some(door) = crate::check::MultiLock::named(text)
                && let Some((block, locks)) = args.split_last()
            {
                out.push(&format!("nikaia_std::lock::{}(", door.written()));
                for lock in locks {
                    out.push("&");
                    self.expr(out, lock, depth, flow)?;
                    out.push(", ");
                }
                self.expr(out, block, depth, flow)?;
                out.push(")");
                return Ok(());
            }

            // **A hull you can observe, you write**
            // ([ADR-281](../../../docs/specification/adr/adr-281.md) D2).
            // `Shared(x)`, `SharedMut(x)` and `Locked(x)` are the three, and each
            // expands to the shape `written_name` would give the *type* - so the
            // constructor and the annotation cannot disagree about a value.
            if let [held] = args
                && let Some(hulls) = self.hull_new(
                    text,
                    self.hull_count
                        .take()
                        .unwrap_or_else(|| self.count_at(flow.function, flow.bound)),
                )
            {
                for path in &hulls {
                    out.push(&format!("{path}::new("));
                }
                self.expr(out, held, depth, flow)?;
                for _ in &hulls {
                    out.push(")");
                }
                return Ok(());
            }

            // `println`, `print` and their `stderr` halves are macros in
            // Rust, and their argument is an interpolated string, which is a
            // format string already. `print` is here because output composed
            // piece by piece - a pretty-printer, a progress line - cannot be
            // written with the newline attached.
            if matches!(text, "println" | "eprintln" | "print" | "eprint") {
                if let [Expr::LitInterpolated { parts }] = args {
                    out.push(&format!("{text}!("));
                    self.format_string(out, parts, depth, flow)?;
                    out.push(")");
                    return Ok(());
                }
                // A plain string is text, and Rust's macro would read a brace
                // in it as a hole of its own - so the braces are escaped on the
                // way down rather than the argument being passed separately
                // (ADR-309 D4). `print("{")` prints a brace.
                if let [Expr::LitStr { text: literal, .. }] = args {
                    out.push(&format!("{text}!(\"{}\")", rust_format_escape(literal)));
                    return Ok(());
                }
                out.push(&format!("{text}!(\"{{}}\", "));
                self.args(out, text, args, &[], depth, flow)?;
                out.push(")");
                return Ok(());
            }

            if self.structs.contains(name) {
                out.push(&format!("{text}::new("));
                let takes = self.takes_a_handle(&format!("{text}::new"));
                self.args(out, text, args, &takes, depth, flow)?;
                out.push(")");
                return Ok(());
            }

            // **`std`'s own types are constructed the same way**
            // ([ADR-140](../../docs/specification/adr/adr-140.md) D2). `Vec()`
            // is the anonymous constructor and `Vec::new()` is what the
            // language below calls it — the same two spellings the arm above
            // reconciles for a type this file declares, one ledger over.
            //
            // **Read off the library rather than from a list of three names**,
            // because `std` gaining a type with a constructor should not need
            // an edit here: the entry `Name::new` *is* the fact.
            if self.library.functions.contains_key(&format!("{text}::new")) {
                // **Through `path`**, because one name below depends on more
                // than the name: a **trusted** map is `TrustedMap::default()`
                // and not `TrustedMap::new()`, since `new` exists only for the
                // default hasher ([ADR-010](../../docs/specification/adr/adr-010.md)
                // D5). Writing `{text}::new(` here would have taken that back
                // for every `HashMap()` in a trusted program.
                out.push(&self.path(&[text, "new"]));
                out.push("(");
                let takes = self.takes_a_handle(&format!("{text}::new"));
                self.args(out, text, args, &takes, depth, flow)?;
                out.push(")");
                return Ok(());
            }
        }

        // **The same constructor, written with its module in front**
        // ([ADR-313](../../docs/specification/adr/adr-313.md) D3):
        // `collections::HashMap()` is `HashMap()` reached the way the prelude's
        // list says a name outside it is reached. The arm above answers the bare
        // spelling and this one the qualified, off the same fact — the entry
        // `Name::new`.
        //
        // The module rides along into the emitted Rust, because `std`'s own
        // prelude publishes the module and the name a **trusted** program gets
        // is in it too (`collections::TrustedMap`). What the prefix must not do
        // is reach `map_name`, which answers about a *type*.
        if let Expr::Path(segments) = func {
            if let [module, name] = segments.as_slice() {
                let module = self.text(*module).to_string();
                let name = self.text(*name).to_string();
                // **The key carries the module**, because that is where the
                // ledger keeps a type that lives in one: `collections::HashMap`
                // and its `::new` beside it.
                let key = format!("{module}::{name}::new");
                if self.library.functions.contains_key(&key) {
                    out.push(&format!("{module}::{}", self.path(&[&name, "new"])));
                    out.push("(");
                    let takes = self.takes_a_handle(&key);
                    self.args(out, &key, args, &takes, depth, flow)?;
                    out.push(")");
                    return Ok(());
                }
            }
            // **And a type another *package* publishes**, which this arm did
            // not ask about: `tiny::Server()` is Part I 4.2's constructor
            // reached through the package that declares it, and the entry
            // `tiny::Server::new` is in the ledger the build merged
            // ([ADR-100](../../docs/specification/adr/adr-100.md) D1).
            //
            // Without it the call went out **verbatim** — `tiny::Server()` —
            // and `rustc` answered *use struct literal syntax instead* and
            // *type annotations needed* about a file nobody wrote
            // ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
            // And it is the very form `NK1149`'s help hands over — *write
            // `tiny::Server(…)`* — so the way out could not be taken either,
            // which is [C.2](../../docs/specification/30-nikaia-tooling.md)'s
            // rule that a way out that cannot be taken is not one.
            //
            // **Unaliased and whole**, for the reason the callee key below
            // gives: `use tiny as t` writes `t::Server` and the ledger keys
            // `tiny::Server::new`, and a path of any depth is one key.
            let qualified = self.parsed.unaliased(
                &segments
                    .iter()
                    .map(|s| self.text(*s))
                    .collect::<Vec<_>>()
                    .join("::"),
            );
            let key = format!("{qualified}::new");
            if self.own_contracts.functions.contains_key(&key) {
                out.push(&format!("{qualified}::new("));
                let takes = self.takes_a_handle(&key);
                self.args(out, &key, args, &takes, depth, flow)?;
                out.push(")");
                return Ok(());
            }
        }

        self.expr(out, func, depth, flow)?;
        if unchecked {
            out.push(UNCHECKED);
        }
        out.push("(");
        let takes = self.takes_a_handle_at(func);
        // The written callee, which is what the checker keyed its answers by —
        // and **a qualified name is one of them**. It used to be a bare name or
        // nothing, on the reasoning that nothing could have been recorded for
        // anything else; the checker keys by the name as written, so
        // `http::route(r)` recorded under `http::route` and was looked up under
        // the empty string. Found by ADR-094 D1's `&` going missing at a call
        // into a package; it was `nullable_args` and `lent_args` both, so a
        // plain value in a **qualified** callee's nullable parameter had been
        // reaching the language below unwrapped.
        let callee = match func {
            Expr::Variable(name) => self.text(*name).to_string(),
            // **Unaliased, because that is what the checker keyed by**:
            // `h::id_of()` is `http::id_of()` where the file wrote
            // `use http as h` ([ADR-286](../../docs/specification/adr/adr-286.md)
            // D3), and a key built from the source's own word never matched.
            Expr::Path(segments) => self.parsed.unaliased(
                &segments
                    .iter()
                    .map(|s| self.text(*s))
                    .collect::<Vec<_>>()
                    .join("::"),
            ),
            _ => String::new(),
        };
        self.args(out, &callee, args, &takes, depth, flow)?;

        if let Expr::Variable(name) = func {
            self.dsl_parameters(out, self.text(*name), args.len(), config, depth, flow)?;
        }
        // **The keep, last** (ADR-283 D10): where the callee's views leave it,
        // the caller says where they live.
        let keep = self
            .keeping_callee(func)
            .and_then(|key| self.keep_argument(flow, &key));

        // Kap 5.1: the language below has no named arguments and no defaults,
        // so the options become positional here, in the order the *declaration*
        // gives - which is the only order there is, and the reason this needs
        // the callee's contract rather than the call alone.
        if let Some(options) = self.options_of(func) {
            for (n, option) in options.iter().enumerate() {
                // **The comma belongs *between* arguments**, and a call whose
                // arguments are all options has nothing before the first one
                // ([ADR-133](../../docs/specification/adr/adr-133.md) D1).
                // `execute(; target_age: 30)` came out as `execute(, 30)` —
                // invalid Rust, reported by `rustc` about a file nobody wrote
                // (Part III C.1), for the only spelling such a call had. Older
                // than the record that names the shape: nothing in the corpus
                // declares a function whose parameters are all options.
                if n > 0 || !args.is_empty() {
                    out.push(", ");
                }
                match config.iter().find(|a| self.text(a.name) == option.name) {
                    Some(passed) => {
                        if self
                            .lent_options
                            .contains(&(&passed.value as *const Expr as usize))
                        {
                            out.push("&");
                        }
                        self.expr(out, &passed.value, depth, flow)?
                    }
                    // Not passed, so the declaration's default is the value.
                    // It is a literal, and Nikaia spells a literal the way the
                    // language below does (ADR-296 D17).
                    // A list crosses as the view the parameter is
                    // (ADR-318 D4): `[4, 5]` for a `ref Vec[i64]` is a list
                    // lent for the call, which a view of a slice takes too.
                    None if option.default.starts_with('[') && option.ty.is_a_view() => {
                        out.push("&vec!");
                        out.push(&option.default);
                    }
                    None => out.push(&option.default),
                }
            }
        }
        if let Some(keep) = keep {
            if !args.is_empty() || !config.is_empty() {
                out.push(", ");
            }
            out.push(&keep);
        }

        out.push(")");
        Ok(())
    }

    /// ADR-296 D5: the shadow struct a call to a DSL driver builds.
    ///
    /// A `;` at a call site means options everywhere else, and the callee's
    /// declaration is what tells the two apart: a driver said
    /// `...args: Self::dsl`, so what stands after its `;` is a DSL's deferred
    /// parameters and becomes one value rather than one argument each.
    ///
    /// A struct literal names its fields, so a call may write them in any
    /// order - the body's order decides the type's fields and nothing else.
    fn dsl_parameters(
        &self,
        out: &mut Out,
        callee: &str,
        args: usize,
        config: &[crate::ast::ConfigArg],
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        if config.is_empty() || !self.dsl_drivers.contains(callee) {
            return Ok(());
        }
        let names: Vec<String> = config
            .iter()
            .map(|a| self.text(a.name).to_string())
            .collect();
        if args > 0 {
            out.push(", ");
        }
        out.push(&format!("{} {{ ", crate::dsl::type_name(&names)));
        for (i, argument) in config.iter().enumerate() {
            if i > 0 {
                out.push(", ");
            }
            out.push(&format!("{}: ", self.text(argument.name)));
            self.expr(out, &argument.value, depth, flow)?;
        }
        out.push(" }");
        Ok(())
    }

    /// Kap 5.1 at a **method** call: the options become positional, in the order
    /// the declaration gives, and one that was left out becomes its default.
    ///
    /// The same thing `call` does from `options_of`, off the checker's answer
    /// instead ([ADR-288](../../../docs/specification/adr/adr-288.md)) — and a
    /// DSL driver is not this: what stands after *its* `;` is the typed spread
    /// of [ADR-296](../../../docs/specification/adr/adr-296.md) D5, which
    /// `dsl_parameters` has already written as one value.
    fn method_options(
        &self,
        out: &mut Out,
        method: Symbol,
        args: usize,
        config: &[crate::ast::ConfigArg],
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        let name = self.text(method).to_string();
        if self.dsl_drivers.contains(&name) {
            return Ok(());
        }
        let Some(options) = self.method_options.get(&(flow.statement, name)) else {
            return Ok(());
        };
        for (n, (option, default)) in options.iter().enumerate() {
            // **The comma belongs *between* arguments**, and a method call whose
            // arguments are all options has nothing before the first one
            // ([ADR-133](../../docs/specification/adr/adr-133.md) D1).
            if n > 0 || args > 0 {
                out.push(", ");
            }
            match config.iter().find(|a| self.text(a.name) == option) {
                Some(passed) => self.expr(out, &passed.value, depth, flow)?,
                None => out.push(default),
            }
        }
        Ok(())
    }

    /// Whether a branch of an `overlap` can fail out of itself
    /// ([ADR-292](../../../docs/specification/adr/adr-292.md) D5).
    ///
    /// **The guarded half of a `catch` does not count**, which is the whole
    /// reason this is its own walk rather than [`visit_expr`]: D5 says
    /// per-branch handling is a `catch` inside the branch, so a branch that
    /// catches its own failure hands back a value and fails nothing. The
    /// handler *is* walked — a `throw` in one leaves the branch like any other
    /// failure.
    fn branch_can_fail(&self, value: &Expr, flow: Flow<'_>) -> bool {
        match value {
            Expr::Call { func, args, config } => {
                self.can_fail(func)
                    || args.iter().any(|a| self.branch_can_fail(a, flow))
                    || config.iter().any(|a| self.branch_can_fail(&a.value, flow))
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
                config,
            }
            // A call that may not happen may still fail when it does, so the
            // branch it is in can fail.
            | Expr::SafeMethod {
                receiver,
                method,
                args,
                config,
            } => {
                // **A grammar entry propagates on its own** (Kap 7.1, and
                // [ADR-296](../../docs/specification/adr/adr-296.md) D25's
                // `throws`): it is an ordinary call now, and this is the arm
                // ordinary calls are in — which is what that record meant.
                self.enters_a_grammar(receiver, *method)
                    || self.method_can_fail(flow, *method)
                    || self.branch_can_fail(receiver, flow)
                    || args.iter().any(|a| self.branch_can_fail(a, flow))
                    || config.iter().any(|a| self.branch_can_fail(&a.value, flow))
            }
            // The guarded half is handled here; the handler is not.
            Expr::TryCatch { handler, .. } => handler.stmts.iter().any(
                |stmt| matches!(&stmt.node, Stmt::Expr(value) if self.branch_can_fail(value, flow)),
            ),
            Expr::Throw(_) => true,
            // **A branch written as a block** fails where a statement in it
            // can, and where a value it binds has a cleanup that can
            // ([ADR-297](../../docs/specification/adr/adr-297.md) D4): the
            // branch is where that failure belongs.
            Expr::Block(block) => block.stmts.iter().any(|stmt| {
                self.settle_lets.get(&stmt.span.at()) == Some(&true)
                    || match &stmt.node {
                        Stmt::Expr(value) | Stmt::Let { value, .. } => {
                            self.branch_can_fail(value, flow)
                        }
                        _ => false,
                    }
            }),
            Expr::Try(inner) | Expr::Unary { expr: inner, .. } => self.branch_can_fail(inner, flow),
            Expr::Binary { lhs, rhs, .. } => {
                self.branch_can_fail(lhs, flow) || self.branch_can_fail(rhs, flow)
            }
            Expr::Coalesce { value, fallback } => {
                self.branch_can_fail(value, flow) || self.branch_can_fail(fallback, flow)
            }
            Expr::Field { base, .. } | Expr::SafeField { base, .. } => {
                self.branch_can_fail(base, flow)
            }
            Expr::Index { base, index } => {
                self.branch_can_fail(base, flow) || self.branch_can_fail(index, flow)
            }
            _ => false,
        }
    }

    /// **`overlap { … }`** — Part I 8.1.2,
    /// [ADR-292](../../../docs/specification/adr/adr-292.md) D2 and D6.
    ///
    /// Each statement is a branch, each branch becomes an `async` block, and
    /// `task::overlap<n>` polls all of them in one pass. An `async` **block**
    /// and not a closure, for the reason a task's body is one: a branch is a
    /// block, and a closure around it would only be called once
    /// ([ADR-277](../../../docs/specification/adr/adr-277.md) D16 - the reason
    /// this line used to give, that Rust has no `async` closure, is false).
    /// No `move`, because D4 says
    /// nothing outlives the block — a branch borrows what is around it exactly
    /// as an ordinary statement does, and that is what makes the form lighter
    /// than two `spawn`s.
    ///
    /// **D6 is the argument order.** *"A branch is started up to its first
    /// suspension point before any branch that cannot suspend is run"* — so the
    /// branches that can pause are handed over first, and the join polls in the
    /// order it is given. Which those are is the ledger's `sync` column, read
    /// the same way [`Emitter::branch_pauses`] reads it for ADR-292's pairs.
    ///
    /// **And the value goes back into written order**, because D2 says it is the
    /// tuple in written order and the reordering above is a schedule. Where the
    /// two differ the tuple is rebuilt, which costs a move per branch and is
    /// visible in the emitted Rust as the permutation it is.
    fn overlap(&self, out: &mut Out, block: &Block, depth: usize, flow: Flow<'_>) -> Result<()> {
        if block.stmts.len() < 2 {
            return Err(refused_at!(
                flow.statement,
                "An `overlap` block needs at least two branches to run at the same time."
            ));
        }
        if block.stmts.len() > MOST_BRANCHES {
            return Err(refused_at!(
                flow.statement,
                "This `overlap` block has {} branches, but at most {MOST_BRANCHES} are \
                 supported.",
                block.stmts.len()
            ));
        }

        // D6: the branches that can pause, in written order among themselves,
        // then the ones that cannot. `sort_by_key` is stable, so written order
        // survives inside each half - which is what makes the schedule
        // reproducible rather than merely correct.
        let mut order: Vec<usize> = (0..block.stmts.len()).collect();
        order.sort_by_key(|&at| !self.branch_pauses(&block.stmts[at], flow));

        // **D5: a branch whose failure is uncaught fails the block, and the
        // first in written order wins.** The `?`s below are written in written
        // order, and `?` returns at the first `Err` - so the rule is the
        // language below's own control flow rather than a comparison this
        // compiler makes. Per-branch handling is a `catch` inside the branch,
        // which `branch_can_fail` does not count.
        //
        // All or none: a branch that cannot fail is wrapped in `Ok` too, so the
        // vehicle sees one shape and the `?`s line up. The error type is named
        // rather than inferred, because an `async` block with a `?` in it and
        // nothing to infer from is *"type annotations needed"* about a file
        // nobody wrote (Part III, C.1).
        //
        // **Or handled at the block itself**
        // ([ADR-292](../../docs/specification/adr/adr-292.md) D18): a `catch`
        // written on the block is where the failure stops, so the branches are
        // wrapped for it exactly as they are for a `throws` function - and the
        // outcome stays a `Result` for the handler to take apart.
        let fallible = (flow.throws || flow.handled_here)
            && block.stmts.iter().any(|stmt| match &stmt.node {
                Stmt::Expr(value) => self.branch_can_fail(value, flow.at(stmt.span.at())),
                _ => false,
            });

        // **Whose channel the branches travel in**
        // ([ADR-292](../../docs/specification/adr/adr-292.md) D19). Where the
        // failure leaves the function, it is the function's, as
        // [ADR-292](../../docs/specification/adr/adr-292.md) D17 made it. Where a
        // `catch` on the block handles it, the function may not be `throws` at
        // all — so it is the set the **branches** throw, which is also what the
        // handler binds.
        let joined = flow
            .handled_here
            .then(|| self.joined_throws(Self::branch_values(block)));
        let channel = match &joined {
            None => flow.channel.to_string(),
            // `main` has no parameters to borrow from and a block's outcome is
            // bound where it is written, so an error carrying a view can only be
            // `'static` here — [ADR-283](../../docs/specification/adr/adr-283.md)
            // D9's derivation, one position over.
            Some(set) => self.channel_of(set, Lifetimes::ELIDED, false),
        };

        // **Each branch settles what it parks, where a value with a cleanup
        // may die in this function** ([ADR-297](../../docs/specification/adr/adr-297.md)
        // D3, [ADR-292](../../docs/specification/adr/adr-292.md) D10): the
        // branches run at once in one task, so each gets a queue of its own. A
        // cleanup that fails joins **its branch's** error where the branch's
        // channel is the box, which is where such a failure travels; elsewhere
        // what the branch parked goes to the function's settle point.
        let own_queue = self.settle_fns.get(flow.function).map(|&failing| {
            let kind = self.settle_kind();
            match failing && fallible && channel == "Box<dyn std::error::Error>" {
                true => format!("nikaia_std::cleanup::branch_after::<{kind}, _, _, _>"),
                false => format!("nikaia_std::cleanup::branch::<{kind}, _, _>"),
            }
        });

        let pad = "    ".repeat(depth);
        let inner = "    ".repeat(depth + 1);
        let reordered = order.iter().enumerate().any(|(at, &from)| at != from);
        let bound = reordered || fallible;

        if bound {
            out.push("{\n");
            out.push(&inner);
            out.push(&format!(
                "// ADR-292 D6: started in this order, answered in the written one.\n{inner}"
            ));
            out.push("let (");
            for (at, _) in order.iter().enumerate() {
                if at > 0 {
                    out.push(", ");
                }
                out.push(&format!("{BRANCH}{at}"));
            }
            out.push(") = ");
        }

        out.push(&format!(
            "nikaia_std::task::overlap{}(\n",
            block.stmts.len()
        ));
        let body = "    ".repeat(depth + 1 + usize::from(bound));
        for &from in &order {
            let stmt = &block.stmts[from];
            out.push(&body);
            out.push("async { ");
            // A branch's own statement, as an expression. `Flow::PLAIN` for the
            // reason a lambda's body gets it: a `return` inside a branch would
            // leave the branch, and `catch` is how D5 says a branch handles its
            // own failure.
            let inside = Flow {
                statement: stmt.span.at(),
                throws: fallible,
                origin: flow.origin,
                // A branch is still inside this function, so a nested vehicle
                // in one names the same channel
                // ([ADR-292](../../docs/specification/adr/adr-292.md) D17).
                channel: flow.channel,
                ..Flow::PLAIN
            };
            match &stmt.node {
                Stmt::Expr(value) => {
                    if let Some(wrapper) = &own_queue {
                        out.push(&format!("{wrapper}(async {{ "));
                    }
                    if fallible {
                        // **The enclosing function's channel**
                        // ([ADR-292](../../docs/specification/adr/adr-292.md)
                        // D2), because the `?` below converts into it.
                        out.push(&format!("Ok::<_, {channel}>("));
                    }
                    out.from(&stmt.span, |out| self.expr(out, value, depth + 1, inside))?;
                    if fallible {
                        out.push(")");
                    }
                    match &own_queue {
                        Some(wrapper) if wrapper.contains("branch_after") => {
                            out.push(&format!(" }}, &{:?}).await", flow.origin));
                        }
                        Some(_) => out.push(" }).await"),
                        None => {}
                    }
                }
                _ => {
                    return Err(refused_at!(
                        flow.statement,
                        "Each branch of an `overlap` has to be an expression."
                    ));
                }
            }
            out.push(" },\n");
        }
        out.push(&"    ".repeat(depth + usize::from(bound)));
        out.push(")");

        if bound {
            out.push(&format!(".await;\n{inner}"));
            // **The branches become one outcome in `std`**
            // ([ADR-292](../../docs/specification/adr/adr-292.md) D18) — `combine<n>` takes them in
            // written order and answers the first `Err` among them
            // ([ADR-292](../../docs/specification/adr/adr-292.md) D5). A `?`
            // per branch written here would leave the *function* instead, which
            // is what a `catch` on the block cannot allow; and one function that
            // sees every outcome is where
            // [ADR-292](../../docs/specification/adr/adr-292.md)'s `secondary`
            // list goes the day the later failures stop being dropped.
            if fallible {
                out.push(&format!("nikaia_std::task::combine{}(", block.stmts.len()));
            } else {
                out.push("(");
            }
            // Written order out of start order: branch `written` was handed
            // over at `order.iter().position(…)`, so that is the name it came
            // back under. Where nothing was reordered the two are the same, and
            // this still writes the tuple out because the combination hangs off
            // it.
            for written in 0..block.stmts.len() {
                if written > 0 {
                    out.push(", ");
                }
                let at = order
                    .iter()
                    .position(|&from| from == written)
                    .expect("every branch is handed over exactly once");
                out.push(&format!("{BRANCH}{at}"));
            }
            out.push(")");
            // **And the `?` is one, on the block** — unless the handler is
            // right here, in which case the `match` around this is what takes
            // the outcome apart.
            if fallible && !flow.handled_here {
                out.push("?");
            }
            out.push(&format!("\n{pad}}}"));
        } else {
            out.push(".await");
        }
        Ok(())
    }

    /// **`select { … }`** — Part II 12.4,
    /// [ADR-292](../../../docs/specification/adr/adr-292.md) D12 and D13.
    ///
    /// Each arm's expression becomes an `async` block, `task::race<n>` polls all
    /// of them in one pass, and what comes back says **which** arm won. So the
    /// construct lowers to a `match` over a sum, which is what an arm binding a
    /// name and then running a block *is* in the language below.
    ///
    /// **The losers are cancelled, and nothing here writes that** (D2). The
    /// losing futures are dropped when `race<n>` returns; a future dropped at
    /// its suspension point tears its values down, and a `cleanup` that pauses
    /// is adopted by the runtime and bounded by the `cleanup-deadline`
    /// ([ADR-297](../../../docs/specification/adr/adr-297.md) D5). That is the
    /// mechanism this runtime already had, which is the whole argument of D3.
    ///
    /// **A failing arm**, where the enclosing function is `throws`: every arm is
    /// wrapped in `Ok` so the vehicle sees one shape, and the winner is
    /// unwrapped at the top of its own body — `?` there and not at the `match`,
    /// because only the arm that won has a value to propagate.
    fn select(
        &self,
        out: &mut Out,
        arms: &[SelectArm],
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        if arms.len() < 2 {
            return Err(refused_at!(
                flow.statement,
                "A `select` needs at least two arms to choose between."
            ));
        }
        if arms.len() > MOST_BRANCHES {
            return Err(refused_at!(
                flow.statement,
                "This `select` has {} arms, but at most {MOST_BRANCHES} are supported.",
                arms.len()
            ));
        }

        // The same all-or-none the `overlap` above writes, and for the same
        // reason: the vehicle sees one shape and the error type is named rather
        // than inferred, because an `async` block with a `?` in it and nothing
        // to infer from is *"type annotations needed"* about a file nobody
        // wrote (Part III, C.1).
        //
        // **Or handled at the block** ([ADR-292](../../docs/specification/adr/adr-292.md)
        // D1), exactly as an `overlap`'s is.
        let fallible = (flow.throws || flow.handled_here)
            && arms
                .iter()
                .any(|arm| self.branch_can_fail(&arm.value, flow.at(arm.at as usize)));

        // Whose channel the arms travel in (D2): the function's where the
        // failure leaves it, the **arms'** own set where a `catch` on the block
        // is what handles it.
        let joined = flow
            .handled_here
            .then(|| self.joined_throws(arms.iter().map(|arm| &arm.value)));
        let channel = match &joined {
            None => flow.channel.to_string(),
            Some(set) => self.channel_of(set, Lifetimes::ELIDED, false),
        };

        let pad = "    ".repeat(depth);
        let inner = "    ".repeat(depth + 1);
        out.push(&format!("match nikaia_std::task::race{}(\n", arms.len()));
        for arm in arms {
            out.push(&inner);
            out.push("async { ");
            // `Flow::PLAIN` for the reason an `overlap` branch gets it: a
            // `return` written inside the *raced expression* would leave that
            // block. What an arm's **body** does is the function's, and the body
            // is emitted below with the flow it was called with.
            let inside = Flow {
                statement: arm.at as usize,
                throws: fallible,
                origin: flow.origin,
                // As an `overlap`'s branch does.
                channel: channel.as_str(),
                ..Flow::PLAIN
            };
            if fallible {
                // The same as an `overlap`'s, one construct over
                // ([ADR-292](../../docs/specification/adr/adr-292.md) D17).
                out.push(&format!("Ok::<_, {channel}>("));
            }
            self.expr(out, &arm.value, depth + 1, inside)?;
            if fallible {
                out.push(")");
            }
            out.push(" },\n");
        }
        out.push(&format!("{pad}).await {{\n"));

        for (at, arm) in arms.iter().enumerate() {
            let bound = match (fallible, arm.binding) {
                (true, _) => WINNER.to_string(),
                (false, Some(name)) => self.text(name).to_string(),
                (false, None) => "_".to_string(),
            };
            out.push(&format!(
                "{inner}nikaia_std::task::Race{}::{}({bound}) => ",
                arms.len(),
                ORDINALS[at]
            ));
            // **The winner's `?`, at the top of the arm that won.** A `_` arm
            // still propagates, because an arm that ignores a value does not
            // ignore a failure.
            //
            // **Unless the handler is right here**
            // ([ADR-292](../../docs/specification/adr/adr-292.md) D18): a `?`
            // would leave the *function*, and what the `catch` around this
            // `match` takes apart is the block's own outcome. So the failure is
            // taken apart here instead, and the arm's value becomes the `Ok`
            // half — which is what makes the whole `match` a `Result`.
            if fallible && flow.handled_here {
                let name = match arm.binding {
                    Some(name) => self.text(name).to_string(),
                    None => "_".to_string(),
                };
                out.push(&format!(
                    "match {WINNER} {{\n{inner}    Err({ARM_FAILED}) => Err({ARM_FAILED}),\n{inner}    Ok({name}) => {{ let {ARM_VALUE} = "
                ));
                out.from(
                    &Span {
                        start: arm.at,
                        end: arm.at,
                    },
                    |out| {
                        self.block_opening_with(out, &arm.body, depth + 2, flow, Tail::Value, None)
                    },
                )?;
                out.push(&format!("; Ok({ARM_VALUE}) }},\n{inner}}}\n"));
                continue;
            }
            let opening = fallible.then(|| match arm.binding {
                Some(name) => format!("let {} = {WINNER}?;", self.text(name)),
                None => format!("{WINNER}?;"),
            });
            out.from(
                &Span {
                    start: arm.at,
                    end: arm.at,
                },
                |out| {
                    self.block_opening_with(
                        out,
                        &arm.body,
                        depth + 1,
                        flow,
                        Tail::Value,
                        opening.as_deref(),
                    )
                },
            )?;
            out.push("\n");
        }
        out.push(&format!("{pad}}}"));
        Ok(())
    }

    /// Whether a call from `caller` to `callee` closes a cycle of pausing
    /// functions, and so needs its future behind a pointer (ADR-055 D6).
    ///
    /// **The question is about the call and not about the callee.** A recursive
    /// `countdown` boxes the call it makes to itself; the call `main` makes to it
    /// holds that future exactly once and is finite, so it pays no allocation.
    /// Asking it the other way - boxing every call to anything recursive - was
    /// the first version, and it charged every caller for its callee's shape.
    ///
    /// `A -> A` beside `B -> B` where `B` also calls `A` is what makes "both are
    /// in a cycle" the wrong test: nothing about `B -> A` is recursive. So the
    /// test is that the callee can reach the caller again, which is what closing
    /// a cycle *is*.
    fn closes_a_pausing_cycle(&self, caller: &str, callee: &str) -> bool {
        self.pausing_reach
            .get(callee)
            .is_some_and(|seen| seen.contains(caller))
    }

    /// The same, for a call whose callee this emitter cannot name
    /// ([ADR-055](../../docs/specification/adr/adr-055.md) D6, the method
    /// half).
    ///
    /// `stats.add(5)` names `add` and says nothing about what `stats` is
    /// ([ADR-288](../../docs/specification/adr/adr-288.md)), so the candidates
    /// are **every** pausing key ending in that name — the same widening
    /// `pausing_reach` draws its edges with, asked here at the call. Where the
    /// two readings differ this takes the wider, for that function's own
    /// reason: a box nobody needed costs one allocation, and a box that was
    /// needed and is missing is a program that does not compile.
    fn method_closes_a_pausing_cycle(&self, caller: &str, method: &str) -> bool {
        let suffix = format!("::{method}");
        self.pausing_reach
            .iter()
            .filter(|(key, _)| key.ends_with(&suffix))
            .any(|(_, seen)| seen.contains(caller))
    }

    /// Whether a type named here holds a view, and so carries the input lifetime
    /// wherever it is written (Part II, 10.6).
    ///
    /// Two sources and one answer: the file's own declarations, and the
    /// package's ledger for the types another file declares
    /// ([`Emitter::tethered`]).
    fn borrows(&self, name: Symbol) -> bool {
        self.borrowing.contains(&name) || self.tethered.contains(self.text(name))
    }

    /// Whether a written type **carries** a view: it is one, it names a struct
    /// that holds one, or one of its arguments does
    /// ([ADR-283](../../docs/specification/adr/adr-283.md) D1, D7).
    ///
    /// [`holds_view`] asks the first of the three, off the written type alone,
    /// and that is all a walk without the declarations can do. This one has them:
    /// `Vec[Entry]` carries a view exactly when `Entry` does, and D9's
    /// derivation — *a function with nothing to borrow from can only write
    /// `'static`* — is about the **lifetime** in the signature rather than about
    /// the `&`, so it is this question it wants answered.
    fn carries_a_view(&self, ty: &Type) -> bool {
        // A function type carries what its result does ([`holds_view`]).
        if let Some(code) = &*ty.code {
            return (*code.result)
                .as_ref()
                .is_some_and(|result| self.carries_a_view(result));
        }
        ty.is_view || self.borrows(ty.name) || ty.generics.iter().any(|g| self.carries_a_view(g))
    }

    /// The **one error type** a function's failures can be, where the ledger
    /// names exactly one ([ADR-280](../../docs/specification/adr/adr-280.md)
    /// D1).
    ///
    /// `throws` in the source says *that* a function fails; the ledger's set
    /// says *with what*, derived over the call graph
    /// ([ADR-023](../../docs/specification/adr/adr-023.md) D1). Where that set
    /// has exactly one member and the member is a type **this unit declares**,
    /// the failure channel is that type, and a `catch` can match on its
    /// variants — which is Part I 7.1's own example and what a box makes
    /// impossible.
    ///
    /// `None` is the box, and it is the answer for everything else: a `"?"` in
    /// the set is *something this compiler cannot name*, a set with two members
    /// needs the generated sum that is not built, and a type another package
    /// declares is not this unit's to name in a signature.
    fn named_error(&self, key: &str) -> Option<Named<'_>> {
        self.named_error_of(&self.own_contracts.functions.get(key)?.fails_with)
    }

    /// [`Emitter::named_error`] asked of the **set** rather than of a function.
    ///
    /// A joining block has a set of its own — the union of what its branches
    /// throw — and no ledger key to look it up under
    /// ([ADR-292](../../docs/specification/adr/adr-292.md) D19).
    fn named_error_of<'t>(&'t self, throws: &'t [String]) -> Option<Named<'t>> {
        let [one] = throws else {
            return None;
        };
        if one == crate::contracts::UNNAMED_ERROR {
            return None;
        }
        if self.declared_errors.contains(one) {
            return Some(Named::Own(one.as_str()));
        }
        // **A type a ledger describes is a name too**
        // ([ADR-280](../../docs/specification/adr/adr-280.md) D9). `io::IoError`
        // is not this unit's to declare and is every bit as nameable: `std`
        // publishes it, the generated file reaches it through the prelude, and
        // a program that reads a file has it as its whole set.
        self.library
            .types
            .contains_key(one)
            .then_some(Named::Library(one.as_str()))
    }

    /// The type a function's failures travel in.
    ///
    /// `borrows` is the same question the result position asks
    /// ([ADR-283](../../docs/specification/adr/adr-283.md) D7): an error that
    /// carries a view — `ConfigError::NotFound(path)`, Part I 7.1's own — is a
    /// type with a lifetime, and a function with nothing to borrow from can
    /// only write `'static` for it.
    fn error_channel(&self, key: &str, lifetimes: Lifetimes, borrows: bool) -> String {
        match self.own_contracts.functions.get(key) {
            Some(contract) => self.channel_of(&contract.fails_with, lifetimes, borrows),
            None => "Box<dyn std::error::Error>".to_string(),
        }
    }

    /// [`Emitter::error_channel`] asked of the **set** rather than of a
    /// function ([ADR-292](../../docs/specification/adr/adr-292.md) D19).
    fn channel_of(&self, throws: &[String], lifetimes: Lifetimes, borrows: bool) -> String {
        // **A set of two or more is the generated sum**
        // ([ADR-280](../../docs/specification/adr/adr-280.md) D15), where every
        // member of it is a name.
        if let Some(sum) = self.sums.get(throws) {
            let params = match throws.iter().any(|m| self.borrows_named(m)) {
                false => String::new(),
                true => match lifetimes == Lifetimes::ELIDED && !borrows {
                    true => format!("<{}>", Lifetimes::STATIC.params),
                    false => format!("<{}>", lifetimes.params),
                },
            };
            return format!("{}{params}", Self::sum_path(sum));
        }
        match self.named_error_of(throws) {
            None => "Box<dyn std::error::Error>".to_string(),
            // **A library's error travels in an envelope too**
            // ([ADR-280](../../docs/specification/adr/adr-280.md) D13). It was
            // bare ([ADR-280](../../docs/specification/adr/adr-280.md) D13)
            // because no `throw` of this program raised it, and the envelope
            // says *no site recorded* for exactly that. What it carries is the
            // **list**: a caller that propagates a failure that joined others
            // has to be able to hand it on, and a bare channel could not take
            // one — `rustc` refused the file. Measured before it was decided
            // (`benches/envelope`): one word on the `Result`, and no time.
            Some(Named::Library(name)) => format!("nikaia_std::error::Thrown<{name}>"),
            Some(Named::Own(name)) => {
                let params = match self.borrows_named(name) {
                    false => String::new(),
                    true => match lifetimes == Lifetimes::ELIDED && !borrows {
                        true => format!("<{}>", Lifetimes::STATIC.params),
                        false => format!("<{}>", lifetimes.params),
                    },
                };
                format!("nikaia_std::error::Thrown<{name}{params}>")
            }
        }
    }

    /// [`Emitter::borrows`] for a type named as text rather than as a `Symbol`.
    ///
    /// The ledger's `throws` column carries names, and a name interned by
    /// another parse is not this one's `Symbol`.
    fn borrows_named(&self, name: &str) -> bool {
        self.tethered.contains(name)
            || self.borrowing.iter().any(|sym| self.text(*sym) == name)
            // **A library's type answers from the library's ledger**
            // ([ADR-280](../../docs/specification/adr/adr-280.md)): the two sets
            // above are this package's, and a type another one declares is
            // described where it is declared.
            || self
                .library
                .types
                .get(name)
                .is_some_and(|contract| !contract.tethered.is_empty())
    }

    /// Every distinct error **set of two or more named members** in this unit,
    /// and the name of the type that stands for it
    /// ([ADR-280](../../docs/specification/adr/adr-280.md) D15).
    ///
    /// **Per set and not per function**, which is what makes propagation free:
    /// two functions that fail the same way get the same type, so a `?` between
    /// them converts nothing. Keyed by the members in the ledger's own order,
    /// which is sorted, so the name is a function of the set and of nothing
    /// else.
    fn error_sums(&self) -> std::collections::BTreeMap<Vec<String>, String> {
        let mut out = std::collections::BTreeMap::new();
        for contract in self.own_contracts.functions.values() {
            let members = &contract.fails_with;
            if members.len() < 2 {
                continue;
            }
            // One member this compiler cannot name is the whole set unnamed:
            // a sum with a hole in it is the box by another spelling.
            if members.iter().any(|m| self.name_of_error(m).is_none()) {
                continue;
            }
            let name = format!(
                "{SUM}{}",
                members
                    .iter()
                    .map(|m| m.replace("::", "_"))
                    .collect::<Vec<_>>()
                    .join("__")
            );
            out.insert(members.clone(), name);
        }
        out
    }

    /// The generated sums, written into the file
    /// ([ADR-280](../../docs/specification/adr/adr-280.md) D15).
    ///
    /// One `enum` per distinct set, the `From` each member needs so that a `?`
    /// converts on its own, and `Display`/`Error` so that the sum can still go
    /// into the opaque channel where a caller has one.
    fn write_error_sums(&self, out: &mut Out) {
        // **Once, at the crate root.** A module is its own file below, so a sum
        // written into each of them would be a different type per file and a
        // failure could not cross one. Every mention of it is written
        // `crate::…` for the same reason ([`Emitter::sum_path`]).
        if !self.entry {
            return;
        }
        for (members, name) in &self.sums {
            let members: &[String] = members;
            let borrows = members.iter().any(|m| self.borrows_named(m));
            // **One lifetime for the whole sum**, and it is the members' own: a
            // member that carries a view carries the buffer's lifetime, and a
            // sum of such a member has it too
            // ([ADR-283](../../docs/specification/adr/adr-283.md) D1).
            let (decl, params) = match borrows {
                true => (format!("<{INPUT_LIFETIME}>"), format!("<{INPUT_LIFETIME}>")),
                false => (String::new(), String::new()),
            };
            let of = |m: &str| match self.borrows_named(m) {
                true => params.clone(),
                false => String::new(),
            };

            out.push(&format!(
                "// The failure channel of every function in this file that \
                 fails in exactly\n// these ways \
                 ([ADR-280](docs/specification/adr/adr-280.md) D15). A program \
                 never\n// writes this name: a `catch` matches on the \
                 **members'** variants.\n\
                 //\n\
                 // `non_camel_case_types` is allowed rather than avoided: the \
                 name is spelled\n// so that nothing a program declares \
                 collides with it, and a warning about\n// this file is a \
                 defect here ([Part III C.1](docs/specification/30-nikaia-tooling.md)).\n\
                 #[derive(Debug)]\n\
                 #[allow(non_camel_case_types)]\n\
                 enum {name}{decl} {{\n"
            ));
            for member in members {
                out.push(&format!(
                    "    {}({}),\n",
                    Self::sum_variant(member),
                    self.sum_member_type(member, &of(member))
                ));
            }
            out.push("}\n");

            for member in members {
                out.push(&format!(
                    "impl{decl} From<{}> for {name}{params} {{\n\
                     \x20   fn from(error: {}) -> {name}{params} {{ {name}::{}(error) }}\n\
                     }}\n",
                    self.sum_member_type(member, &of(member)),
                    self.sum_member_type(member, &of(member)),
                    Self::sum_variant(member)
                ));
                // **A `?` straight from `std` hands a library's error bare**,
                // and the member is its envelope (ADR-280 D13): put on here,
                // with no site, as `From<E> for Thrown<E>` puts it on.
                if matches!(self.name_of_error(member), Some(Named::Library(_))) {
                    out.push(&format!(
                        "impl{decl} From<{member}> for {name}{params} {{\n\
                         \x20   fn from(error: {member}) -> {name}{params} {{ \
                         {name}::{}(nikaia_std::error::Thrown::from(error)) }}\n\
                         }}\n",
                        Self::sum_variant(member)
                    ));
                }
            }

            out.push(&format!(
                "impl{decl} std::fmt::Display for {name}{params} {{\n\
                 \x20   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n\
                 \x20       match self {{\n"
            ));
            for member in members {
                out.push(&format!(
                    "            {name}::{}(e) => e.fmt(f),\n",
                    Self::sum_variant(member)
                ));
            }
            out.push(&format!(
                "        }}\n\x20   }}\n}}\n\
                 impl{decl} std::error::Error for {name}{params} {{}}\n"
            ));

            // **The long form asks the member** (D2): a member the program
            // declares carries its envelope and therefore its site, and a
            // library's says there is none. The sum adds nothing of its own,
            // because it is not where anything was raised.
            out.push(&format!(
                "impl{decl} nikaia_std::error::Full for {name}{params} {{\n\
                 \x20   fn full(&self) -> String {{\n\
                 \x20       match self {{\n"
            ));
            for member in members {
                let held = match self.name_of_error(member) {
                    Some(_) => "e.full()",
                    None => "nikaia_std::error::Full::full(e)",
                };
                out.push(&format!(
                    "            {name}::{}(e) => {held},\n",
                    Self::sum_variant(member)
                ));
            }
            out.push("        }\n    }\n}\n\n");
        }
    }

    /// The sum an expression a `catch` guards fails in, where it is one.
    ///
    /// The same question [`Emitter::caught_channel_is_named`] asks, one channel
    /// over, and answered the same way: only the outermost call, and only a
    /// call by name.
    fn caught_sum(&self, expr: &Expr) -> Option<&str> {
        let joined = match expr {
            Expr::Overlap(block) => Some(self.joined_throws(Self::branch_values(block))),
            Expr::Select(arms) => Some(self.joined_throws(arms.iter().map(|arm| &arm.value))),
            _ => None,
        };
        if let Some(set) = joined {
            return self.sums.get(&set).map(String::as_str);
        }
        self.sum_of(&self.callee_name(expr)?)
    }

    /// `match error { … }` in a handler whose error travels in a **sum**
    /// ([ADR-280](../../docs/specification/adr/adr-280.md) D17).
    ///
    /// One match per member, carrying the arms whose patterns name that
    /// member's variants, and the source's catch-all written into each of them
    /// — because Rust has no fall-through and the members are separate types.
    /// An arm that names no member at all (a bare name, `else`, a literal) is
    /// the catch-all.
    fn match_over_a_sum(
        &self,
        out: &mut Out,
        sum: &str,
        arms: &[crate::ast::MatchArm],
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        // **Every member of the sum gets an arm**, and not only the ones the
        // source named: the sum is a Rust `enum` and a `match` over it has to
        // cover it. A member the handler said nothing about is one where only
        // the catch-all runs, which is exactly what the source meant by not
        // naming it.
        let Some(members) = self.sum_members(sum) else {
            return Err(refused_at!(
                flow.statement,
                "There's no error set called `{sum}`."
            ));
        };
        let mut catch_all: Vec<&crate::ast::MatchArm> = Vec::new();
        let mut by_member: Vec<(&str, Vec<&crate::ast::MatchArm>)> =
            members.iter().map(|m| (m.as_str(), Vec::new())).collect();
        for arm in arms {
            match self.member_of(&arm.pattern) {
                Some(member) => match by_member.iter_mut().find(|(m, _)| *m == member) {
                    Some((_, list)) => list.push(arm),
                    // A variant of a type that is **not** in this set cannot
                    // arrive here. Left where it was written rather than
                    // dropped: the language below says it is unreachable, and
                    // that is a truer message than silence.
                    None => catch_all.push(arm),
                },
                None => catch_all.push(arm),
            }
        }
        // **The set of error types is open**
        // ([ADR-023](../../docs/specification/adr/adr-023.md) D4), so no
        // `match` over it can be exhaustive and every one of them needs a
        // catch-all. Refused here rather than below, because what `rustc` would
        // say is about a file nobody wrote
        // ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
        if catch_all.is_empty() {
            return Err(refused_at!(
                flow.statement,
                "This `match error` needs an `else` arm: it covers {} error types, but \
                 other errors can arrive at a `catch` too. Add `else => throw error` to \
                 pass them on, or `else => …` to handle them.",
                by_member.len().max(1)
            ));
        }

        let pad = "    ".repeat(depth + 1);
        let inner = "    ".repeat(depth + 2);
        let close = "    ".repeat(depth);
        out.push(&format!("match {CAUGHT} {{\n"));
        for (member, member_arms) in &by_member {
            let member: &str = member;
            // **A member the program declares travels in its envelope**
            // ([ADR-280](../../docs/specification/adr/adr-280.md) D13), so the
            // value the patterns are about is what the envelope holds — and the
            // site is kept in a local beside it, for the arm that passes the
            // error on. The split is the same one a single-typed handler makes;
            // what is new is that it happens per member.
            let own = self.name_of_error(member).is_some();
            let flow = flow.inside(member, own);
            if own {
                out.push(&format!(
                    "{pad}{}::{}({CAUGHT}) => {{ let ({CAUGHT}, {SITE}) = {CAUGHT}.split(); \
                     match {CAUGHT} {{\n",
                    Self::sum_path(sum),
                    Self::sum_variant(member)
                ));
            } else {
                out.push(&format!(
                    "{pad}{}::{}({CAUGHT}) => match {CAUGHT} {{\n",
                    Self::sum_path(sum),
                    Self::sum_variant(member)
                ));
            }
            for arm in member_arms.iter().chain(catch_all.iter()) {
                out.push(&inner);
                self.match_pattern(out, &arm.pattern, depth + 2, flow)?;
                if let Some(guard) = &arm.guard {
                    out.push(" if ");
                    self.expr(out, guard, depth + 2, flow)?;
                }
                out.push(" => ");
                self.arm_body(out, &arm.pattern, &arm.body, depth + 2, flow)?;
                out.push(",\n");
            }
            match own {
                true => out.push(&format!("{pad}}} }},\n")),
                false => out.push(&format!("{pad}}},\n")),
            }
        }
        out.push(&format!("{close}}}"));
        Ok(())
    }

    /// The error **type** a pattern is about, where it names one.
    ///
    /// `ConfigError::Empty(p)` is `ConfigError` and `io::IoError::NotFound(p)`
    /// is `io::IoError` — everything but the last segment, which is
    /// [ADR-280](../../docs/specification/adr/adr-280.md) D12's rule read off a
    /// pattern instead of off an expression. A one-segment path **binds**
    /// (Part I 3.4), so it names no member and is a catch-all.
    fn member_of(&self, pattern: &crate::ast::MatchPattern) -> Option<String> {
        use crate::ast::MatchPattern;
        let path = match pattern {
            MatchPattern::Path(path)
            | MatchPattern::Tuple { path, .. }
            | MatchPattern::Named { path, .. } => path,
            _ => return None,
        };
        if path.len() < 2 {
            return None;
        }
        let member = path[..path.len() - 1]
            .iter()
            .map(|s| self.text(*s))
            .collect::<Vec<_>>()
            .join("::");
        self.name_of_error(&member).is_some().then_some(member)
    }

    /// The generated sum a function's failures travel in, where its set has
    /// one ([ADR-280](../../docs/specification/adr/adr-280.md) D15).
    /// The members of a generated sum, by the name it is written with.
    fn sum_members(&self, sum: &str) -> Option<&[String]> {
        self.sums
            .iter()
            .find(|(_, name)| name.as_str() == sum)
            .map(|(members, _)| members.as_slice())
    }

    fn sum_of(&self, key: &str) -> Option<&str> {
        let contract = self.own_contracts.functions.get(key)?;
        self.sums.get(&contract.fails_with).map(String::as_str)
    }

    /// Whose one error type a **member** of a set is, asked of the name alone.
    fn name_of_error<'n>(&'n self, name: &'n str) -> Option<Named<'n>> {
        if name == crate::contracts::UNNAMED_ERROR {
            return None;
        }
        if self.declared_errors.contains(name) {
            return Some(Named::Own(name));
        }
        self.library
            .types
            .contains_key(name)
            .then_some(Named::Library(name))
    }

    /// What one member of a sum carries: its own single-member channel
    /// ([ADR-280](../../docs/specification/adr/adr-280.md) D16).
    ///
    /// A type this unit declares is one the program `throw`s, so it keeps the
    /// envelope and its site; a library's arrives from a call and travels bare
    /// ([ADR-280](../../docs/specification/adr/adr-280.md) D13). The sum changes
    /// neither — it only says which of them this failure was.
    fn sum_member_type(&self, name: &str, params: &str) -> String {
        match self.name_of_error(name) {
            Some(_) => format!("nikaia_std::error::Thrown<{name}{params}>"),
            None => name.to_string(),
        }
    }

    /// The variant a member is written as: its name with the module flattened,
    /// because a variant is one identifier.
    fn sum_variant(name: &str) -> String {
        name.replace("::", "_")
    }

    /// Whether an expression **never comes back**, so that nothing joins with
    /// it ([Part III A.2](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// `panic(…)` is the one call of that shape, and it is
    /// [ADR-293](../../docs/specification/adr/adr-293.md) D1's own written way
    /// out for a key the program knows is present: `m[k] ?? panic(f"…")`. It
    /// belongs beside [`jumps`] rather than inside it, because telling it apart
    /// needs the name and a free function has no parse to read one from.
    fn never_returns(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Call { func, .. }
            if matches!(func.as_ref(), Expr::Variable(name) if self.text(*name) == PANIC))
    }

    /// A code parameter `function` takes whole and does not keep: a
    /// method's, which `lends.nika` never lends (#463).
    fn a_code_parameter_taken_whole(&self, function: &str, name: &str) -> bool {
        let Some(contract) = self.own_contracts.functions.get(function) else {
            return false;
        };
        let Some(signature) = &contract.signature else {
            return false;
        };
        signature
            .params
            .iter()
            .enumerate()
            .any(|(at, (param, ty))| {
                param == name
                    && matches!(ty, crate::contracts::ty::Ty::Fn { .. })
                    && !contract.keeps.iter().any(|kept| kept == name)
                    && !crate::contracts::keeps::lends_in(
                        contract,
                        at,
                        &[self.library, &self.described],
                    )
            })
    }

    /// Whether the code a call hands `callee` at argument `i` is only run
    /// there, not kept - by its entry, or by every method of that name.
    fn runs_code_at(&self, callee: &str, i: usize) -> bool {
        let runs = |contract: &crate::contracts::FnContract| {
            contract
                .signature
                .as_ref()
                .and_then(|s| s.arguments().get(i))
                .is_some_and(|(name, ty)| {
                    matches!(ty, crate::contracts::ty::Ty::Fn { .. })
                        && !contract.keeps.iter().any(|kept| kept == name)
                })
        };
        match self.own_contracts.functions.get(callee) {
            Some(contract) => runs(contract),
            None => {
                let candidates = self.own_contracts.candidates(callee);
                !candidates.is_empty() && candidates.iter().all(|(_, contract)| runs(contract))
            }
        }
    }

    /// How a sum is **named** wherever it is used.
    ///
    /// `crate::…`, always: the type is defined once at the crate root
    /// ([`Emitter::write_error_sums`]), and a module is a file of its own below,
    /// so a bare name would be a different type in each of them — and a failure
    /// that crosses a module boundary would have nowhere to go.
    fn sum_path(name: &str) -> String {
        format!("crate::{name}")
    }

    /// Whether what a `catch` guards fails through a **named** channel.
    ///
    /// Only the outermost call, and only a call by name: a `catch` guards one
    /// call in every program in the tree, and a guess about a shape nobody
    /// writes would be a guess in the lowering. A `false` where the answer was
    /// yes is today's lowering, unchanged.
    /// The name a call resolves to, where the expression is one.
    ///
    /// The same walk `caught_channel_is_named` and `caught_sum` each did for
    /// themselves, written once because a third asker arrived
    /// ([ADR-292](../../docs/specification/adr/adr-292.md) D19).
    fn callee_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Call { func, .. } => match func.as_ref() {
                Expr::Variable(name) => Some(self.text(*name).to_string()),
                Expr::Path(segments) => Some(
                    segments
                        .iter()
                        .map(|s| self.text(*s))
                        .collect::<Vec<_>>()
                        .join("::"),
                ),
                _ => None,
            },
            Expr::Try(inner) => self.callee_name(inner),
            _ => None,
        }
        .map(|name| self.parsed.unaliased(&name))
    }

    /// **The set of error types a joining block can fail with**
    /// ([ADR-292](../../docs/specification/adr/adr-292.md) D19): the union of
    /// its branches', in the ledger's own order, which is sorted.
    ///
    /// A `catch` written on an `overlap` handles what the **branches** threw,
    /// and the enclosing function may not be `throws` at all — so the channel
    /// the branches are wrapped in and the one the handler binds are this set's
    /// and not the function's.
    ///
    /// **It resolves no more than the two askers above already did**
    /// ([ADR-296](../../docs/specification/adr/adr-296.md) D17): a branch's
    /// callee by the name the source wrote, and that callee's `throws` column
    /// off a ledger. A branch whose callee no ledger knows contributes the
    /// absence of a claim, which makes the set the box — the safe direction.
    fn joined_throws<'e>(&self, branches: impl Iterator<Item = &'e Expr>) -> Vec<String> {
        let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for value in branches {
            match self.callee_name(value) {
                Some(name) => match self
                    .own_contracts
                    .functions
                    .get(&name)
                    .or_else(|| self.library.functions.get(&name))
                {
                    Some(contract) => set.extend(contract.fails_with.iter().cloned()),
                    None => {
                        set.insert(crate::contracts::UNNAMED_ERROR.to_string());
                    }
                },
                // Not a call by name: a `throw` written straight into a branch,
                // an operator, a method. None of them is resolvable here, and
                // the box is what the absence of a claim lowers to.
                None => {
                    set.insert(crate::contracts::UNNAMED_ERROR.to_string());
                }
            }
        }
        set.into_iter().collect()
    }

    /// The expressions an `overlap`'s statements are, for [`Emitter::joined_throws`].
    fn branch_values(block: &Block) -> impl Iterator<Item = &Expr> + use<'_> {
        block.stmts.iter().filter_map(|stmt| match &stmt.node {
            Stmt::Expr(value) => Some(value),
            _ => None,
        })
    }

    fn caught_channel_is_named(&self, expr: &Expr) -> bool {
        // **A joining block answers from its own set**
        // ([ADR-292](../../docs/specification/adr/adr-292.md) D19).
        let joined = match expr {
            Expr::Overlap(block) => Some(self.joined_throws(Self::branch_values(block))),
            Expr::Select(arms) => Some(self.joined_throws(arms.iter().map(|arm| &arm.value))),
            _ => None,
        };
        if let Some(set) = joined {
            return self.named_error_of(&set).is_some();
        }
        let Some(name) = self.callee_name(expr) else {
            return false;
        };
        // A call of this program's hands its failure in an envelope whoever
        // declared the error (ADR-280 D13); a call of `std`'s hands it bare,
        // which `named_error` says by knowing no contract for it - and so does
        // a grammar's entry rule, whose contract is in the ledger (ADR-296 D24)
        // and whose body is the grammar's, which returns the parse's error.
        let a_grammar = name
            .split("::")
            .next()
            .is_some_and(|head| self.grammars.keys().any(|g| self.text(*g) == head));
        !a_grammar && self.named_error(&name).is_some()
    }

    /// Whether a function of **this program** can pause, and is therefore an
    /// `async fn` ([ADR-055](../../../docs/specification/adr/adr-055.md) D1).
    ///
    /// **The answer is the ledger's `sync` column and nothing computed here.**
    /// [ADR-288](../../../docs/specification/adr/adr-288.md) D1 infers it per
    /// function over the call graph, and its conservatism is already the safe
    /// direction (D2): an unresolved call costs a function its claim, which here
    /// makes it `async` - and an `async fn` that never awaits finishes on its
    /// first poll, while a plain `fn` that needed to pause does not compile.
    fn pauses(&self, key: &str) -> bool {
        self.own_contracts
            .functions
            .get(key)
            .is_some_and(|contract| !contract.sync_claim.is_sync())
    }

    /// Whether a call to a **library** entry pauses (ADR-055 §6 step 3), and
    /// under which key.
    ///
    /// **An exact lookup**, and after
    /// [ADR-150](../../docs/specification/adr/adr-150.md) that is a decision
    /// rather than an accident. A suffix match here — `read` finding
    /// `fs::read` — is the emitter *guessing* at a callee the unit does not
    /// describe, and a unit built by `nikaia lower` does not carry the ledger of
    /// the package beside it: `examples/inventory` writes its own `read`, and a
    /// name-for-name fallback put an `.await` on a call to a function that is
    /// not a future. So a `std` function a program writes **bare** is keyed
    /// bare, which `print` and `println` already were, and `sleep` now is.
    ///
    /// The **key** and not a yes, because D6's boxing needs to know which
    /// function is being called.
    ///
    /// Step 2 deliberately did not ask this: a pausing `std` entry blocked its
    /// thread then, so awaiting one would have been awaiting a value. Step 3 is
    /// what made `std`'s pausing entries `async fn`, and this is the line that
    /// reads them.
    fn library_pauses(&self, key: &str) -> Option<String> {
        self.library
            .functions
            .get(key)
            .filter(|contract| !contract.sync_claim.is_sync())
            .map(|_| key.to_string())
    }

    /// Whether a call to this callee carries an `.await` (ADR-055 D2).
    ///
    /// The same resolution [`Emitter::can_fail`] does, one ledger column over:
    /// `throws` becomes a `?` and *can pause* becomes an `.await`, both read off
    /// the callee's contract and neither resolving a name
    /// ([ADR-296](../../../docs/specification/adr/adr-296.md) D17).
    ///
    /// The ledger key a call resolves to, when that key pauses - `None` when the
    /// call takes no `.await` at all.
    ///
    /// D6's boxing needs to know *which* function is being called and not just
    /// that it pauses, so the answer is the name rather than a yes.
    ///
    /// **Both ledgers**, since §6 step 3 made `std`'s own pausing entries
    /// `async fn`. Step 2 asked only this program's own, because a `std` entry
    /// blocked its thread then and awaiting one would have been awaiting a value
    /// rather than a future - which is what let step 2 land on its own.
    /// Whether the own function under `key` was written `sync(f)`: the only
    /// way an own entry says `Sync::From`, since the inference never writes it
    /// ([ADR-288](../../docs/specification/adr/adr-288.md) D31).
    fn lambda_decides(&self, key: &str) -> bool {
        self.own_contracts
            .functions
            .get(key)
            .is_some_and(|contract| matches!(contract.sync_claim, crate::contracts::Sync::From(_)))
    }

    /// Whether the function under `key` is lowered as an `async fn`.
    fn lowered_pausing(&self, key: &str) -> bool {
        self.pauses(key) || self.lambda_decides(key)
    }

    /// Whether a call names an own function written `sync(f)`.
    fn decided_by_its_lambda(&self, func: &Expr) -> bool {
        let name = match func {
            Expr::Variable(name) => self.text(*name).to_string(),
            Expr::Path(segments) => segments
                .iter()
                .map(|s| self.text(*s))
                .collect::<Vec<_>>()
                .join("::"),
            _ => return false,
        };
        self.lambda_decides(&self.parsed.unaliased(&name))
    }

    fn pausing_key(&self, func: &Expr) -> Option<String> {
        let name = match func {
            Expr::Variable(name) => self.text(*name).to_string(),
            Expr::Path(segments) => segments
                .iter()
                .map(|s| self.text(*s))
                .collect::<Vec<_>>()
                .join("::"),
            _ => return None,
        };
        let name = self.parsed.unaliased(&name);
        // **This unit's own, then its constructors, then `std`'s — and the
        // *first* ledger that has the name is the one that answers.**
        //
        // The chain matters and not only the order: a program with its own
        // `fn read` has a name `std` also has, and falling through once the
        // local entry says *this does not pause* would put an `.await` on a call
        // to a function that is not a future. `can_fail`, one column over, has
        // read its two ledgers this way all along.
        //
        // Kap 4.2's anonymous constructor is the middle link: `Stats(temp)` is a
        // call to the `new` the `impl` provides, so that is the contract to
        // read.
        let constructor = format!("{name}::new");
        let resolved = self
            .own_contracts
            .functions
            .get(&name)
            .map(|contract| (name.clone(), contract))
            .or_else(|| {
                self.own_contracts
                    .functions
                    .get(&constructor)
                    .map(|contract| (constructor.clone(), contract))
            });
        match resolved {
            Some((key, contract)) => (!contract.sync_claim.is_sync()).then_some(key),
            None => self.library_pauses(&name),
        }
    }

    /// Whether a bare name in **value** position is a type's anonymous
    /// constructor ([ADR-140](../../docs/specification/adr/adr-140.md) D2).
    ///
    /// A type this file declares, or one the library publishes with a `::new`
    /// entry — read off the ledger rather than from a list of three names, for
    /// the reason the call path gives.
    fn constructs_by_name(&self, name: &str) -> bool {
        if self.structs.iter().any(|s| self.text(*s) == name) {
            return self
                .own_contracts
                .functions
                .contains_key(&format!("{name}::new"));
        }
        self.library.functions.contains_key(&format!("{name}::new"))
    }

    /// Whether the contracts say a call by name can fail (Kap 7.1).
    ///
    /// The same resolution `options_of` uses, and for the same reason: the
    /// callee's contract is the fact, and this unit's own comes before `std`'s
    /// because a local name shadows nothing in a library. `false` where the
    /// callee cannot be resolved - which is the emitter *not guessing* rather
    /// than an answer, and is safe here because the checker has already
    /// reported every resolvable fallible call the function did not declare
    /// (`NK2605`); a call nothing describes reaches `rustc` as before.
    ///
    /// A **method** call is not asked here, because the answer is not the
    /// emitter's to work out: `stats.add(5)` names `add` and only the type
    /// checker knows what it goes to (ADR-288). `method_can_fail` reads the
    /// answer the checker handed over instead.
    fn can_fail(&self, func: &Expr) -> bool {
        let name = match func {
            Expr::Variable(name) => self.text(*name).to_string(),
            Expr::Path(segments) => segments
                .iter()
                .map(|s| self.text(*s))
                .collect::<Vec<_>>()
                .join("::"),
            _ => return false,
        };
        self.own_contracts
            .functions
            .get(&name)
            .or_else(|| self.own_contracts.functions.get(&format!("{name}::new")))
            .or_else(|| self.library.lookup(&name).map(|(_, c)| c))
            // **And a described crate's** (ADR-290 D20): the checker made the
            // call a place that can fail off the same entry, so the `?` is
            // owed here - without it the crate's `Result` reached the rest of
            // the line, and `rustc` said so about a file nobody wrote.
            .or_else(|| self.described.functions.get(&name))
            .is_some_and(|contract| !contract.fails_with.is_empty())
    }

    /// Whether the checker said this conversion narrows, and which kind it is.
    ///
    /// A lookup, not an analysis: `as i32` narrows or widens by what it is given,
    /// and nothing here knows that (ADR-288). `None` where the checker had no
    /// answer - a type it does not recognise, or two conversions to the same type
    /// in one statement where one of them widens - which leaves the conversion
    /// exactly as it was rather than guessing at it.
    fn narrows(&self, statement: usize, into: &str) -> Option<crate::check::Narrowing> {
        self.narrowing_casts
            .get(&(statement, into.to_string()))
            .copied()
    }

    /// Whether the checker said this method call can fail (Kap 7.1).
    ///
    /// A lookup, not an analysis: `fallible_methods` was computed by the one
    /// type checker this compiler has, against the same ledgers the program is
    /// built with, and nothing here resolves a receiver type. `false` where the
    /// checker had no answer - an unresolved receiver, a method no ledger
    /// describes, or two calls in one statement writing the same name where one
    /// of them cannot fail - which is the emitter leaving the call exactly as it
    /// was rather than guessing at it.
    fn method_can_fail(&self, flow: Flow<'_>, method: Symbol) -> bool {
        self.fallible_methods
            .contains(&(flow.statement, self.text(method).to_string()))
    }

    /// Whether the checker said this method call pauses (ADR-055 D2).
    ///
    /// [`Emitter::method_can_fail`]'s twin, and a lookup for the same reason:
    /// the answer was computed once by the one type checker this compiler has,
    /// and `false` here is the emitter leaving the call as it was rather than
    /// guessing at it.
    fn method_pauses(&self, flow: Flow<'_>, method: Symbol) -> bool {
        self.pausing_methods
            .contains(&(flow.statement, self.text(method).to_string()))
    }

    /// The options a call by name has, from the contract that declares them.
    ///
    /// This unit's own first, then `std`'s - the order every other name
    /// resolution here uses. `None` where the callee has none, and where the
    /// callee cannot be resolved at all: an unresolvable call has no options to
    /// fill in, and the type checker is what says so in Nikaia's words.
    fn options_of(&self, func: &Expr) -> Option<&[crate::contracts::ConfigContract]> {
        let name = match func {
            Expr::Variable(name) => self.text(*name).to_string(),
            Expr::Path(segments) => segments
                .iter()
                .map(|s| self.text(*s))
                .collect::<Vec<_>>()
                .join("::"),
            _ => return None,
        };

        let contract = self
            .own_contracts
            .functions
            .get(&name)
            .or_else(|| self.own_contracts.functions.get(&format!("{name}::new")))
            .or_else(|| self.library.lookup(&name).map(|(_, c)| c))?;

        let config = &contract.signature.as_ref()?.config;
        (!config.is_empty()).then_some(config.as_slice())
    }

    /// A string literal. Which of the two it is decides what comes out, and
    /// that is read off the syntax rather than the text (ADR-309 D2).
    fn string(&self, out: &mut Out, expr: &Expr, depth: usize, flow: Flow<'_>) -> Result<()> {
        match expr {
            // Inert text, transcribed. A brace is a brace, so nothing has to be
            // escaped on the way into a Rust string literal - only a *format*
            // string treats one specially, and this is not one.
            //
            // **Unless its use wants text of its own** (ADR-282 D4), and then
            // it is constructed there, as `[1, 2]` is `vec![1, 2]` where a
            // `Vec` is wanted: a literal is a constant being built, not text
            // the program had being copied.
            Expr::LitStr { text: literal, at } => {
                match self.owned_texts.contains(&self.text_at(*at as usize)) {
                    true => out.push(&format!("String::from(\"{literal}\")")),
                    false => out.push(&format!("\"{literal}\"")),
                }
                Ok(())
            }
            // `f"…"` is a `String` whether or not anyone put a hole in it,
            // because the checker says it is and the two have to agree. With no
            // hole there is nothing to format, and `format!("x")` is a
            // roundabout way of writing what `.to_string()` says plainly.
            Expr::LitInterpolated { parts } => {
                // A malformed literal never gets here: the grammar refuses it
                // at its place (ADR-309 D8).
                if interpolated_holes(parts).is_empty() {
                    out.push(&format!("\"{}\".to_string()", format_of(parts)));
                    return Ok(());
                }
                out.push("format!(");
                self.format_string(out, parts, depth, flow)?;
                out.push(")");
                Ok(())
            }
            _ => Err(anyhow!("not a string literal")),
        }
    }

    fn text_at(&self, at: usize) -> usize {
        crate::check::text_key(self.hole.borrow().as_ref(), at)
    }

    /// The future a kept function that may pause hands back: one that crosses
    /// threads where tasks run on them.
    fn boxed_future(&self) -> &'static str {
        match self.build.overlaps_user_code() {
            true => "nikaia_std::func::SendBoxed",
            false => "nikaia_std::func::Boxed",
        }
    }

    /// The literal as a Rust format string, with its holes as arguments.
    fn format_string(
        &self,
        out: &mut Out,
        parts: &[crate::ast::FPart],
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        // **Built from the parts** (ADR-309 D9): the grammar has read the
        // literal, so there is nothing left to scan.
        out.push(&format!("\"{}\"", format_of(parts)));

        // **The holes the grammar parsed** (ADR-309 D6), each by its place.
        for (hole, expr) in interpolated_holes(parts) {
            // What the tier pass hands over inside it (ADR-229 D1), as every
            // other reader of a hole sees it - and the node itself where it
            // hands nothing, which is the one the checker read
            // ([`hole_as_read`]).
            let expr = hole_as_read(self.parsed, hole, expr);
            let expr: &Expr = &expr;
            out.push(", ");
            let hole = (flow.statement, crate::check::argument_shape(expr));
            let outer = self.hole.replace(Some(hole));
            let written = self.expr(out, expr, depth, flow);
            self.hole.replace(outer);
            written?;
        }
        Ok(())
    }

    /// One arm's pattern. Every shape is one the language below spells the
    /// same way, so this is a transcription rather than a translation - a bare
    /// name binds here because it binds there, and the rule is drawn once.
    /// **Whether a member is boxed below**
    /// ([ADR-246](../../docs/specification/adr/adr-246.md) D1), by the
    /// owner's last segment: a qualified name reaches the same type.
    fn is_boxed(&self, owner: &str, member: &str) -> bool {
        let owner = owner.rsplit("::").next().unwrap_or(owner);
        self.boxed_members
            .get(owner)
            .is_some_and(|members| members.contains(member))
    }

    /// **A field of a struct literal**, which may be a variant's written as a
    /// struct: `Expr::Neg { inner: … }` is the variant `Neg` of `Expr`, and its
    /// member is `Neg.inner`.
    fn is_boxed_field(&self, owner: &str, field: &str) -> bool {
        if let Some((ty, variant)) = owner.rsplit_once("::") {
            let ty = ty.rsplit("::").next().unwrap_or(ty);
            if self.boxed_members.contains_key(ty) {
                return self.is_boxed(ty, &crate::check::boxed_member(variant, None, Some(field)));
            }
        }
        self.is_boxed(owner, field)
    }

    /// A member's lowered type, in its box where it has one.
    fn boxed_ty(&self, owner: &str, member: &str, lowered: String) -> String {
        match self.is_boxed(owner, member) {
            true => format!("Box<{lowered}>"),
            false => lowered,
        }
    }

    /// **An arm, with each name bound out of a box taken out of it first**
    /// ([ADR-246](../../docs/specification/adr/adr-246.md) D2):
    /// `boxed::open` makes a `T` of a `Box<T>` and a `&T` of a `&Box<T>`, so
    /// the arm reads the name as the pattern's type says, whether the `match`
    /// took the value or lent it.
    fn arm_body(
        &self,
        out: &mut Out,
        pattern: &MatchPattern,
        body: &Expr,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        self.arm_body_opening(out, pattern, &[], body, depth, flow)
    }

    /// [`Self::arm_body`], first taking apart each boxed part the pattern
    /// looked inside ([ADR-246](../../docs/specification/adr/adr-246.md) D5):
    /// the guard has already said it has the shape, so the `else` is never
    /// taken.
    fn arm_body_opening(
        &self,
        out: &mut Out,
        pattern: &MatchPattern,
        nests: &[&MatchPattern],
        body: &Expr,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        let mut taken_apart = String::new();
        for (at, nest) in nests.iter().enumerate() {
            let mut written = Out::default();
            self.pattern_written(
                &mut written,
                nest,
                Names::Bind(None),
                &mut None,
                depth,
                flow,
            )?;
            taken_apart.push_str(&format!(
                "let __nikaia_box{at} = nikaia_std::boxed::open(__nikaia_box{at}); \
                 let {} = __nikaia_box{at} else {{ unreachable!(\"the guard matched it\") }}; ",
                written.buf
            ));
        }
        let mut opened = Vec::new();
        self.boxed_bindings(pattern, &mut opened);
        // **And a number bound out of a lent value is copied out** (0.0.236,
        // `check::Checked::copied_bindings`).
        let at = pattern as *const MatchPattern as usize;
        let copied: Vec<&String> = self
            .copied_bindings
            .range((at, String::new())..)
            .take_while(|(pattern, _)| *pattern == at)
            .map(|(_, name)| name)
            .collect();
        // Only the arm's own block is a statement; what it holds is emitted
        // as it would be anywhere.
        let statements = flow.arms_are_statements;
        let flow = Flow {
            arms_are_statements: false,
            ..flow
        };
        if statements
            && let Expr::Block(block) = body
            && opened.is_empty()
            && copied.is_empty()
            && nests.is_empty()
        {
            return self.block(out, block, depth, flow, Tail::Statement);
        }
        // **A `match` that is an arm's whole body is that arm's statement**,
        // so its own arms are statements too: an `if` without an `else` that
        // ends one of them is a statement and not the arm's value, and
        // `if a > b { out.insert(a) }` handed `rustc` the `bool` (found
        // moving `bounds`' basic level into Nikaia).
        let body_flow = match statements && matches!(body, Expr::Match { .. }) {
            true => Flow {
                arms_are_statements: true,
                ..flow
            },
            false => flow,
        };
        if opened.is_empty() && copied.is_empty() && nests.is_empty() {
            return self.expr(out, body, depth, body_flow);
        }
        // **A block arm opens with the names it binds**, inside its own
        // braces: a second pair around it was `unused_braces` in every
        // generated file with one (found moving `Ty::fits` into Nikaia).
        if let Expr::Block(block) = body {
            let mut opening = taken_apart.clone();
            for name in &opened {
                opening.push_str(&format!("let {name} = nikaia_std::boxed::open({name}); "));
            }
            for name in &copied {
                opening.push_str(&format!("let {name} = *{name}; "));
            }
            let tail = match statements {
                true => Tail::Statement,
                false => Tail::Value,
            };
            return self.block_opening_with(
                out,
                block,
                depth,
                flow,
                tail,
                Some(opening.trim_end()),
            );
        }
        out.push("{ ");
        out.push(&taken_apart);
        for name in &opened {
            out.push(&format!("let {name} = nikaia_std::boxed::open({name}); "));
        }
        for name in copied {
            out.push(&format!("let {name} = *{name}; "));
        }
        self.expr(out, body, depth, body_flow)?;
        out.push(" }");
        Ok(())
    }

    /// The names a pattern binds out of a boxed part.
    /// **The boxed parts a guard reads, read through the box** (ADR-246 D5
    /// item 2): the guard runs before the arm opens them, so each name it
    /// reads is bound again to a view of what its box holds.
    fn guard_reopening(&self, pattern: &MatchPattern, guard: &Expr) -> String {
        let mut boxed = Vec::new();
        self.boxed_bindings(pattern, &mut boxed);
        let mut read: Vec<String> = Vec::new();
        crate::contracts::sync::visit_expr(self.parsed, guard, &mut |expr| {
            if let Expr::Variable(name) = expr {
                let name = self.name(*name).to_string();
                if boxed.contains(&name) && !read.contains(&name) {
                    read.push(name);
                }
            }
        });
        read.iter()
            .map(|name| format!("let {name} = nikaia_std::boxed::peek(&{name}); "))
            .collect()
    }

    /// The guard, with the boxed parts it reads opened for it.
    fn guard_written(
        &self,
        out: &mut Out,
        pattern: &MatchPattern,
        guard: &Expr,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        let reopening = self.guard_reopening(pattern, guard);
        if reopening.is_empty() {
            return self.expr(out, guard, depth, flow);
        }
        out.push("{ ");
        out.push(&reopening);
        self.expr(out, guard, depth, flow)?;
        out.push(" }");
        Ok(())
    }

    fn boxed_bindings(&self, pattern: &MatchPattern, out: &mut Vec<String>) {
        let owner_of = |path: &[Symbol]| -> Option<(String, Option<String>)> {
            match path {
                [.., owner, variant] if self.boxed_members.contains_key(self.text(*owner)) => {
                    Some((
                        self.text(*owner).to_string(),
                        Some(self.text(*variant).to_string()),
                    ))
                }
                [.., owner] => Some((self.text(*owner).to_string(), None)),
                [] => None,
            }
        };
        match pattern {
            MatchPattern::Tuple { path, parts } => {
                let owner = owner_of(path);
                for (at, part) in parts.iter().enumerate() {
                    let boxed = owner.as_ref().is_some_and(|(owner, variant)| {
                        variant.as_deref().is_some_and(|variant| {
                            self.is_boxed(
                                owner,
                                &crate::check::boxed_member(variant, Some(at), None),
                            )
                        })
                    });
                    match (boxed, part) {
                        (true, MatchPattern::Path(one)) if one.len() == 1 => {
                            let name = self.text(one[0]);
                            if name != "_" {
                                out.push(self.name(one[0]).to_string());
                            }
                        }
                        _ => self.boxed_bindings(part, out),
                    }
                }
            }
            MatchPattern::Named { path, bindings, .. } => {
                let Some((owner, variant)) = owner_of(path) else {
                    return;
                };
                for binding in bindings {
                    let field = self.text(*binding);
                    let member = match &variant {
                        Some(variant) => crate::check::boxed_member(variant, None, Some(field)),
                        None => field.to_string(),
                    };
                    if self.is_boxed(&owner, &member) {
                        out.push(self.name(*binding).to_string());
                    }
                }
            }
            MatchPattern::Or(alternatives) => {
                if let Some(first) = alternatives.first() {
                    self.boxed_bindings(first, out);
                }
            }
            _ => {}
        }
    }

    fn match_pattern(
        &self,
        out: &mut Out,
        pattern: &MatchPattern,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        let lent = self
            .lent_bindings
            .get(&(pattern as *const MatchPattern as usize));
        self.pattern_lending(out, pattern, lent, depth, flow)
    }

    /// A pattern, with the names the arm only reads bound as views: `ref n`
    /// leaves the scrutinee whole, where a plain `n` would take the part out of
    /// it (ADR-291).
    fn pattern_lending(
        &self,
        out: &mut Out,
        pattern: &MatchPattern,
        lent: Option<&std::collections::BTreeSet<String>>,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        self.pattern_written(out, pattern, Names::Bind(lent), &mut None, depth, flow)
    }

    /// **A boxed part this pattern looks inside**
    /// ([ADR-246](../../docs/specification/adr/adr-246.md) D5): the part of a
    /// variant's tuple at `at` is behind a box, and what the pattern writes
    /// there is not a name.
    fn boxed_part(&self, path: &[Symbol], at: usize) -> bool {
        let [.., owner, variant] = path else {
            return false;
        };
        self.boxed_members.contains_key(self.text(*owner))
            && self.is_boxed(
                self.text(*owner),
                &crate::check::boxed_member(self.text(*variant), Some(at), None),
            )
    }

    /// Whether any arm's pattern looks inside a boxed part, at any depth the
    /// lowering reaches.
    fn looks_inside_a_box(&self, pattern: &MatchPattern) -> bool {
        match pattern {
            MatchPattern::Tuple { path, parts } => parts.iter().enumerate().any(|(at, part)| {
                (self.boxed_part(path, at) && !binds_a_part(part)) || self.looks_inside_a_box(part)
            }),
            MatchPattern::Or(alternatives) => {
                alternatives.iter().any(|a| self.looks_inside_a_box(a))
            }
            _ => false,
        }
    }

    /// The pattern as written, in one of two modes, collecting into `nests` -
    /// where it is given - each boxed part it looks inside and writing a fresh
    /// name there instead (`__nikaia_box0`, …, in the order they are met).
    fn pattern_written<'m>(
        &self,
        out: &mut Out,
        pattern: &'m MatchPattern,
        names_as: Names<'_>,
        nests: &mut Option<&mut Vec<&'m MatchPattern>>,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        let bound = |s: &Symbol| {
            let name = self.text(*s);
            match names_as {
                Names::Wild => "_".to_string(),
                Names::Bind(lent) => match lent.is_some_and(|lent| lent.contains(name)) {
                    true => format!("ref {name}"),
                    false => name.to_string(),
                },
            }
        };
        let path = |p: &[Symbol]| {
            p.iter()
                .map(|s| self.text(*s).to_string())
                .collect::<Vec<_>>()
                .join("::")
        };
        let names = |b: &[Symbol]| b.iter().map(bound).collect::<Vec<_>>().join(", ");
        match pattern {
            // Rust's own catch-all, which is what the arm was before
            // ([ADR-291](../../docs/specification/adr/adr-291.md) D18).
            MatchPattern::Otherwise => out.push("_"),
            MatchPattern::Literal(value) => self.expr(out, value, depth, flow)?,
            MatchPattern::Path(p) => match p.as_slice() {
                [one] => out.push(&bound(one)),
                _ => out.push(&path(p)),
            },
            // **The parts are patterns**, so this recurses — which is the whole
            // of what *nested* means
            // ([ADR-291](../../docs/specification/adr/adr-291.md) D10). An empty
            // path is the bare tuple `(0, 0)`, which names no type.
            MatchPattern::Tuple { path: p, parts } => {
                out.push(&path(p));
                out.push("(");
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        out.push(", ");
                    }
                    if let Some(nests) = nests.as_mut()
                        && self.boxed_part(p, i)
                        && !binds_a_part(part)
                    {
                        out.push(&format!("__nikaia_box{}", nests.len()));
                        nests.push(part);
                        continue;
                    }
                    self.pattern_written(out, part, names_as, nests, depth, flow)?;
                }
                out.push(")");
            }
            MatchPattern::Named {
                path: p,
                bindings,
                rest,
            } => {
                let inside = match (bindings.is_empty(), rest, names_as) {
                    (true, _, _) | (_, _, Names::Wild) => "..".to_string(),
                    (false, true, _) => format!("{}, ..", names(bindings)),
                    (false, false, _) => names(bindings),
                };
                out.push(&format!("{} {{ {inside} }}", path(p)));
            }
            MatchPattern::Or(alternatives) => {
                for (i, alternative) in alternatives.iter().enumerate() {
                    if i > 0 {
                        out.push(" | ");
                    }
                    self.pattern_written(out, alternative, names_as, nests, depth, flow)?;
                }
            }
            // **`..=`, because a pattern's range includes both ends** (D3) and
            // that is how the language below spells one. The two spellings mean
            // the same set; one of them is this language's and the other is
            // Rust's.
            MatchPattern::Range { start, end } => {
                self.expr(out, start, depth, flow)?;
                out.push("..=");
                self.expr(out, end, depth, flow)?;
            }
        }
        Ok(())
    }
    /// **A method call, for both of the ways a program may write one.**
    ///
    /// `x.m(…)` and `x?.m(…)` differ in *whether* the call happens and in
    /// nothing else ([ADR-278](../../../../docs/specification/adr/adr-278.md)),
    /// so what a call lowers to is written once: the truncating conversion, the
    /// `len` that becomes an `i64`, `collect`'s turbofish, the arguments, Kap
    /// 5.1's deferred parameters, ADR-055 D2's `.await` and ADR-023 D8's `?`.
    ///
    /// `receiver` is `None` for the safe form, where the receiver is the name
    /// the enclosing `match` already bound - so the call is written on
    /// [`REACHED`] rather than on an expression, and the value is reached
    /// exactly once whatever the receiver cost to produce.
    ///
    /// Eight arguments, and each is a part of the call this has to write: the
    /// receiver, the name, the positional arguments, Kap 5.1's zone, and the
    /// three the emitter carries everywhere (`out`, `depth`, `flow`). Splitting
    /// them into a struct would name a shape the AST already has.
    #[allow(clippy::too_many_arguments)]
    fn method_call(
        &self,
        out: &mut Out,
        receiver: Option<&Expr>,
        method: Symbol,
        args: &[Expr],
        config: &[crate::ast::ConfigArg],
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        // **`text.chars().collect()` is a list of characters asked for at its
        // size** (`nikaia_std::list::chars`): the same list, without the
        // growing a `collect` over `Chars` does. Only where nothing declares
        // another thing to build.
        if let Some(Expr::MethodCall {
            receiver: text,
            method: chars,
            args: none,
            ..
        }) = receiver
            && self.text(method) == "collect"
            && args.is_empty()
            && none.is_empty()
            && self.text(*chars) == "chars"
            && !self.collected_into.contains(&(
                flow.statement,
                crate::check::argument_shape(receiver.unwrap()),
            ))
        {
            out.push("nikaia_std::list::chars(");
            self.postfix_base(out, text, depth, flow)?;
            out.push(".chars())");
            return Ok(());
        }
        // **`x.truncating_i32()` is Rust's `as`** (ADR-285 D13): the
        // operation the name says. It is a name and not the operator
        // because keeping the low digits is said rather than assumed,
        // exactly as wrapping is (D2) - and since D4 made `as i32`
        // checked, this is the only way left to ask for truncation.
        //
        // Bare, and parenthesised by whoever needs it: `as` sits between
        // the unary operators and the binary ones in Rust's precedence
        // while a method call sits above all of them, so
        // `-x.truncating_i32()` and `x.truncating_i32().abs()` both need
        // the conversion to happen first. [`Emitter::emits_as_cast`] is
        // where that is decided, for the same reason a written `as`
        // decides it there - **a parenthesis nobody needs is a warning
        // about the generated file** (`let n = (big as i32);` is
        // "unnecessary parentheses around assigned value"), and Part III
        // C.1 says a reader must not meet one.
        // **A grammar is entered by an ordinary call**
        // ([ADR-296](../../docs/specification/adr/adr-296.md) D24):
        // `Json.value(input)`, where `Json` names a grammar in this file and
        // `value` one of its `entry` rules. It is a method call in the grammar
        // of this language and nothing else could have been — a grammar name is
        // not a value, so there is no receiver to resolve and no ambiguity to
        // settle.
        if let Some(Expr::Variable(name)) = receiver
            && self.grammars.contains_key(name)
            && args.len() == 1
        {
            // **The dot is gone** (ADR-140 D3). `NK1147` is what a program
            // meets; this is here so that a caller who lowers without
            // checking gets the same sentence rather than a method call on
            // a name that is not a value.
            return Err(refused_at!(
                flow.statement,
                "A grammar's rules are reached with `::`, not a dot. Write `{}::{}(…)`.",
                self.text(*name),
                self.text(method)
            ));
        }
        if let Some(into) = truncating(self.text(method)) {
            self.receiver(out, receiver, depth, flow, true)?;
            out.push(&format!(" as {into}"));
            return Ok(());
        }
        // **`x.checked_i32()` is Rust's `try_from`, kept or not**
        // ([ADR-315](../../docs/specification/adr/adr-315.md)): the number
        // where the destination holds it, `None` where it does not. A call
        // around the receiver, so nothing about precedence is left to decide.
        if let Some(into) = checked(self.text(method)) {
            out.push(&format!("{into}::try_from("));
            match receiver {
                // An argument: nothing binds looser than the call's own
                // parentheses, so it needs none of its own.
                Some(expr) => self.nested(out, expr, 0, depth, flow)?,
                None => self.receiver(out, receiver, depth, flow, false)?,
            }
            out.push(").ok()");
            return Ok(());
        }
        // **`len` hands back an `i64`** (ADR-285 D1), and Rust's hands
        // back a `usize`. Parenthesised where it has to be and nowhere
        // else, exactly as the conversion above is.
        //
        // A name and not a rule, because what it encodes is a fact about
        // *Rust's* library rather than about this language: four entries
        // in `std.contracts` return a length, `len` is what all four are
        // called, and `the_four_lengths_are_i64` in `tests/contracts.rs`
        // is what keeps the two from drifting apart.
        // **`xs.drain()` is the written form of taking the elements away**
        // ([ADR-094](../../docs/specification/adr/adr-094.md) D4). Since a
        // `for` lends, a body that hands an element to a callee which *keeps*
        // it has to say so — and what it says is this. Below, that is
        // `into_iter`: D4's words are *"removing a name from scope"*, which is
        // consuming the container rather than emptying one somebody still
        // holds, and Rust spells the first `into_iter` and the second `drain`.
        //
        // A name and not a rule, for `len`'s reason one paragraph down: it
        // encodes a fact about *Rust's* library rather than about this
        // language.
        // **D5's one door for a stamped value**
        // ([ADR-281](../../docs/specification/adr/adr-281.md)).
        // `kasse.set(neu; after: stand)` is written `set_after(neu, stand)`:
        // the compare and the store happen while the lock is open **once**,
        // which is the whole of what the door is for and what a `get` followed
        // by a `set` cannot do.
        //
        // Two questions and not one. The type checker's, that this receiver is
        // a lock (`witnessed_sets`, ADR-288); and this file's, that the `after:`
        // is written *here* — because the set is keyed by the statement, and a
        // statement may hold a second `set` that carries none.
        let witness =
            match self.text(method) == "set" && self.witnessed_sets.contains(&flow.statement) {
                true => config
                    .iter()
                    .find(|a| self.text(a.name) == "after")
                    .map(|a| &a.value),
                false => None,
            };
        // **The text form of text is the text** (ADR-282 D8): nothing is
        // called, and whether it is lent, built or handed over is decided
        // around it as for any other text.
        if let Some(text) = receiver.filter(|receiver| {
            self.text(method) == "to_string"
                && args.is_empty()
                && self
                    .text_as_is
                    .contains(&(flow.statement, crate::check::argument_shape(receiver)))
        }) {
            return self.postfix_base(out, text, depth, flow);
        }
        // **A list of text asked for a view of text** (`check::Checked::
        // text_in_lists`): compared element by element, which a `String` and
        // a `str` can be.
        if let Some(list) = receiver.filter(|receiver| {
            self.text(method) == "contains"
                && args.len() == 1
                && self
                    .text_in_lists
                    .contains(&(flow.statement, crate::check::argument_shape(receiver)))
        }) {
            // The value lent with `&` whichever text it is (#375): `&str`,
            // `String` and their references compare with each other.
            out.push("nikaia_std::list::contains(&");
            self.expr(out, list, depth, flow)?;
            out.push(", &");
            self.expr(out, &args[0], depth, flow)?;
            out.push(")");
            return Ok(());
        }
        let copies = match receiver {
            Some(receiver) => self
                .owned_copies
                .contains(&(flow.statement, crate::check::argument_shape(receiver))),
            None => self.reached_copy.get(),
        };
        let written = match self.text(method) {
            "drain" if args.is_empty() => "into_iter",
            // **A copy is `to_owned` below** (ADR-293 D24): `.clone()` of a
            // `&str` or a `&[T]` is the reference, and a read-only `String`
            // parameter *is* a `&str` (ADR-282 D5).
            "clone" if args.is_empty() && copies => "to_owned",
            "set" if witness.is_some() => "set_after",
            other => other,
        };
        let length = is_length(self.text(method), args)
            || receiver.is_some_and(|receiver| self.is_counted(receiver, method, args, flow));
        // **ADR-055 D6, the method half.** A recursive `async fn` is an
        // infinitely sized future, and a call that closes a cycle of pausing
        // functions puts it behind a pointer. `call` has asked this since D6's
        // first sharp edge; a *method* could not be asked, because the emitter
        // cannot name its callee — `stats.add(5)` names `add` and only the type
        // checker knows what it goes to (ADR-288). It does not have to name it:
        // `pausing_reach` already draws an edge to **every** pausing method of
        // that name, which is the over-approximation its own note describes, so
        // the same widening answers the question here. Boxing a call that did
        // not need it costs one allocation; missing one is `rustc`'s *recursion
        // in an async fn requires boxing*, about a file nobody wrote (C.1).
        let boxed = self.method_closes_a_pausing_cycle(flow.function, self.text(method));
        if boxed {
            out.push("Box::pin(");
        }
        // **`par_iter()` walks the list on every core at `yes`, and is the
        // list's `iter()` at `no`**
        // ([ADR-235](../../docs/specification/adr/adr-235.md) D1). What the
        // lambdas handed to it may do is the checker's, at both settings, so
        // the program means the same at each.
        if self.text(method) == "par_iter"
            && args.is_empty()
            && self.own_contracts.candidates("par_iter").is_empty()
        {
            match self.build.user_parallelism {
                UserParallelism::Yes => {
                    out.push("nikaia_std::par::iter(&");
                    self.receiver(out, receiver, depth, flow, false)?;
                    out.push(")");
                }
                UserParallelism::No => {
                    self.receiver(out, receiver, depth, flow, false)?;
                    out.push(".iter()");
                }
            }
            if boxed {
                out.push(")");
            }
            return Ok(());
        }
        // **A lambda that pauses, handed to a `std` entry**
        // ([ADR-233](../../docs/specification/adr/adr-233.md) D1, D2): the
        // entry's counterpart in `nikaia_std::seq`, which takes the `async`
        // closure the lambda is written as. The checker says which, because it
        // is the receiver's type that decides.
        let pausing = args
            .last()
            .filter(|arg| matches!(arg, Expr::Closure { .. }))
            .and_then(|arg| {
                self.pausing_lambdas
                    .get(&(flow.statement, crate::check::argument_shape(arg)))
            })
            .copied();
        match pausing {
            Some(PausingEntry::Function(name)) => {
                out.push(&format!("nikaia_std::seq::{name}("));
                self.receiver(out, receiver, depth, flow, false)?;
                out.push(", ");
            }
            Some(PausingEntry::Lent(name)) => {
                out.push(&format!("nikaia_std::seq::{name}(&mut "));
                self.receiver(out, receiver, depth, flow, false)?;
                out.push(", ");
            }
            Some(PausingEntry::Method(name)) => {
                self.receiver(out, receiver, depth, flow, false)?;
                out.push(&format!(".{name}("));
            }
            None => {
                self.receiver(out, receiver, depth, flow, false)?;
                out.push(&format!(".{written}"));
            }
        }
        // Nikaia's `collect` builds a List; Rust's needs to be told
        // what to build, and with no types here that is `Vec<_>`.
        //
        // **Except over a sequence whose step pauses**
        // ([ADR-172](../../docs/specification/adr/adr-172.md) D5), where the
        // walk is `std`'s own `async fn` and has nothing to be told: an
        // `Iterator::collect` never pauses, so the site pausing is exactly the
        // difference. Guessed from the method's name alone this would have
        // been a turbofish on a method that takes no type at all.
        //
        // **And except where the place it goes declares what it builds**
        // ([ADR-293](../../docs/specification/adr/adr-293.md) D25): a map, a
        // set or text, which the language below's own `collect`
        // builds when nothing overrides the type it reads off that place.
        let declared = receiver.is_some_and(|receiver| {
            self.collected_into
                .contains(&(flow.statement, crate::check::argument_shape(receiver)))
        });
        if self.text(method) == "collect"
            && args.is_empty()
            && !declared
            && !self.method_pauses(flow, method)
        {
            out.push("::<Vec<_>>");
        }
        if pausing.is_none() {
            out.push("(");
        }
        let takes = self.takes_a_handle(self.text(method));
        // **Views put into a keeper that drops entries are held** (ADR-283 D12),
        // each value with a handle on every buffer it points into - as many as
        // the keeper's widest value needs, so that every element is one type
        // (ADR-283 D15).
        let held = receiver
            .and_then(|receiver| crate::contracts::keep::root_of(self.parsed, receiver))
            .and_then(|root| {
                let plan = self.keep_plan(flow.function)?;
                let by_arg = plan.holds.get(&(flow.statement, root.clone()))?;
                let width = plan.widths.get(&root).copied().unwrap_or(1);
                Some(
                    by_arg
                        .iter()
                        .map(|(arg, buffers)| {
                            let mut keeps: Vec<usize> = buffers.iter().copied().collect();
                            while keeps.len() < width {
                                keeps.push(keeps[keeps.len() - 1]);
                            }
                            (*arg, keeps)
                        })
                        .collect(),
                )
            });
        *self.hold_args.borrow_mut() = held;
        let written_args = self.args(out, self.text(method), args, &takes, depth, flow);
        *self.hold_args.borrow_mut() = None;
        written_args?;
        // **And a method that takes a keep is given one** (D2).
        let keeping = crate::contracts::keep::keeping_method_key(
            &self.own_contracts,
            flow.function.rsplit_once("::").map(|(t, _)| t),
            matches!(receiver, Some(Expr::Variable(n)) if self.text(*n) == "self"),
            self.text(method),
        );
        if let Some(keep) = keeping.and_then(|key| self.keep_argument(flow, &key)) {
            if !args.is_empty() {
                out.push(", ");
            }
            out.push(&keep);
        }
        match witness {
            // The witness is an **argument** of the door and not an option of
            // it, so it is written where the signature puts it — after the
            // value, and never through `dsl_parameters`.
            //
            // **The `&` is written here rather than looked up**, and that is
            // the one place this file decides a reference for itself. `lends`
            // withholds its claim on every *method* argument, because the
            // emitter cannot resolve a receiver
            // ([ADR-288](../../docs/specification/adr/adr-288.md)) — and this
            // position needs no resolving: the checker has already said this
            // call is the door, and the door's witness is `seen: &$T` in the
            // ledger, always. A written `&` is `NK1137` before it gets here.
            Some(seen) => {
                out.push(", &");
                // Parenthesised exactly where a postfix would be: `&` binds
                // tighter than every binary operator, so `&a + b` is `(&a) + b`
                // and a witness that is an expression would mean something
                // else. A name gets no parentheses, which is where every
                // witness anybody writes lands.
                self.postfix_base(out, seen, depth, flow)?;
            }
            None => {
                self.dsl_parameters(out, self.text(method), args.len(), config, depth, flow)?;
                self.method_options(out, method, args.len(), config, depth, flow)?;
            }
        }
        out.push(")");
        // **What hands back an element of a held list reads it through its
        // handle** (ADR-283 D17), as the brackets and a `for` do.
        if matches!(self.text(method), "iter" | "first" | "last" | "get")
            && receiver.is_some_and(|r| self.reads_through_handle(flow, r))
        {
            out.push(HELD_READ);
        }

        // ADR-023 D8, the method half. The same three conditions the
        // call by name is given in `call` below, and the same rule -
        // only the last question is asked of a different source,
        // because the emitter cannot ask it itself. The enclosing
        // function must be `throws`, or there is nowhere for the `?` to
        // go, and `NK2605` has already refused the program where it is
        // not. The call must not be the guarded half of a `catch`,
        // which wants the `Result`. And the callee must be one the
        // checker established can fail.
        // ADR-055 D2, the method half, and **before the `?`** for the
        // reason `call` gives: the future is what can fail, so it has
        // to be driven before there is a `Result` to propagate.
        // **Before the `.await`**, because it is the future that is boxed and
        // not what awaiting it comes to.
        if boxed {
            out.push(")");
        }

        // **A walk of a pausing sequence that has no pausing form**
        // ([ADR-172](../../docs/specification/adr/adr-172.md) D5,
        // [ADR-234](../../docs/specification/adr/adr-234.md)). Every walk `Seq`
        // has today has one; an entry added without one is refused here with a
        // line, rather than left to `rustc` about a method the generated file's
        // receiver does not have (Part III, C.1).
        if self
            .pausing_walks
            .contains(&(flow.statement, self.text(method).to_string()))
        {
            let name = self.text(method);
            return Err(refused_at!(
                flow.statement,
                "`{name}` can't be used on a sequence whose steps pause. Write a \
                 `for x in … {{ … }}` loop instead, and do the work inside it."
            ));
        }
        if self.method_pauses(flow, method) {
            if flow.in_lambda {
                return Err(pausing_in_a_lambda(flow.statement, self.text(method)));
            }
            out.push(".await");
        }

        if flow.throws && !flow.caught && self.method_can_fail(flow, method) {
            out.push("?");
        }
        if length {
            out.push(" as i64");
        }
        Ok(())
    }

    /// The receiver of a [`Emitter::method_call`]: the expression, or the name
    /// a `?.` already bound.
    ///
    /// `tight` is for the one caller that needs the conversion to bind tighter
    /// than everything around it - `x.truncating_i32()` is Rust's `as`, which
    /// sits below the unary operators in its precedence.
    fn receiver(
        &self,
        out: &mut Out,
        receiver: Option<&Expr>,
        depth: usize,
        flow: Flow<'_>,
        tight: bool,
    ) -> Result<()> {
        // **An element the call changes is written as the place it is**
        // (`check::Checked::changed_elements`).
        let flow = match receiver {
            Some(expr)
                if self
                    .changed_elements
                    .contains(&(flow.statement, crate::check::argument_shape(expr))) =>
            {
                flow.place()
            }
            _ => flow,
        };
        match (receiver, tight) {
            (Some(expr), true) => self.nested(out, expr, u8::MAX, depth, flow),
            (Some(expr), false) => self.postfix_base(out, expr, depth, flow),
            (None, _) => {
                out.push(REACHED);
                Ok(())
            }
        }
    }

    /// **A read through the brackets** (ADR-293 D4): `*get(&base, at(i))`,
    /// or without the `*` where `deref` is off - which is how a read that is
    /// itself the base of a read is written (0.0.238).
    fn index_read(
        &self,
        out: &mut Out,
        base: &Expr,
        index: &Expr,
        depth: usize,
        flow: Flow<'_>,
        deref: bool,
    ) -> Result<()> {
        let slicing = self.slices(flow.statement, index);
        if deref && !slicing {
            out.push("*");
        }
        // **An index proved inside is read without its check**
        // ([ADR-306](../../docs/specification/adr/adr-306.md) D5): the proof
        // is the argument the `unsafe` stands on, and `proven::read` is the
        // one operation it licenses.
        if !slicing
            && self
                .proven_indices
                .contains(&crate::check::value_node(base))
            && self.map_key(flow.statement, index, false).is_none()
        {
            out.push("unsafe { nikaia_std::proven::read(");
            match base {
                Expr::Index {
                    base: inner,
                    index: at,
                } if !self.slices(flow.statement, at)
                    && self.map_key(flow.statement, at, false).is_none()
                    && !self.reads_through_handle(flow, inner) =>
                {
                    self.index_read(out, inner, at, depth, flow, false)?;
                }
                _ => {
                    out.push("&");
                    self.postfix_base(out, base, depth, flow)?;
                }
            }
            out.push(", (");
            match only_literals(index) {
                true => self.index_expr(out, index, depth, flow.inferred())?,
                false => self.index_expr(out, index, depth, flow)?,
            }
            out.push(") as usize) }");
            if self.reads_through_handle(flow, base) {
                out.push(".get()");
            }
            return Ok(());
        }
        // **A read of a read is handed the reference it already is**
        // (0.0.238): `d[i][j]` over a list of lists was
        // `get(&(*get(&d, i)), j)`, a borrow of a deref - what `clippy`
        // refuses in a `std` written in Nikaia (`tools/spelling.nika`), and
        // noise in every program. The inner read answers a `&T` before its
        // `*`, which is what the outer one takes; a map's read (a `T?`), a
        // run and a read through a handle keep the long way round.
        match base {
            Expr::Index {
                base: inner,
                index: at,
            } if !self.slices(flow.statement, at)
                && self.map_key(flow.statement, at, false).is_none()
                && !self.reads_through_handle(flow, inner) =>
            {
                out.push("nikaia_std::index::get(");
                self.index_read(out, inner, at, depth, flow, false)?;
            }
            _ => {
                out.push("nikaia_std::index::get(&");
                self.postfix_base(out, base, depth, flow)?;
            }
        }
        out.push(", ");
        // **The literal exception belongs to the read as well**, and
        // it was missing here: `xs[0]` as a *read* went through
        // `at(…)` whatever was in the brackets, and `at`'s `I` has
        // nothing to infer itself from. Alone that survived, because
        // an integer literal defaults late and `usize` is what every
        // `At` for a number answers with — but a **field read on the
        // element** needs the type *before* the defaulting, so
        // `rows[1].a` was `cannot infer type` about the generated
        // file, for both an `Array` and a `Vec`. That is [Part III
        // C.1](../../docs/specification/30-nikaia-tooling.md)'s
        // class and the very thing the paragraph above says this
        // exception exists to prevent.
        //
        // **And a range written in literals is in the
        // exception**, which it was not while the `*` above stood
        // over one. `at` cannot settle a bare `1..=2`: `At` is
        // implemented for a `RangeInclusive` of every signed type
        // and each answers the same `RangeInclusive<usize>`, so
        // there is nothing to infer `I` *from* and what came back
        // was `cannot infer type` about the generated file. Handed
        // over as written it settles itself, because
        // `RangeInclusive<usize>` is the only one of them that is a
        // `SliceIndex<[V]>` — which is what the exception says
        // everywhere else it applies.
        //
        // **Except where it counts from the end.** `xs[-2..-1]` is
        // an index out of bounds and says so at run time
        // ([ADR-285](../../../docs/specification/adr/adr-285.md)
        // D1) — but only if it reaches run time, and handed over as
        // written it does not: `-2` against a `usize` is *the trait
        // `Neg` is not implemented for `usize`*, about a type the
        // program never named. So a range with a negation in it
        // goes back through the conversion, **widened**, because an
        // `i64` is the one width this language indexes with and
        // `at` has nothing else to read it off.
        let counts_down = slicing && a_negation_inside(index);
        // **A key the map keeps nothing of is lent** (ADR-293 D27),
        // and a key is not a position, so `at` does not see it.
        let key = self.map_key(flow.statement, index, false);
        match (key, only_literals(index) && !counts_down) {
            (Some(form), _) => self.key(out, form, index, depth, flow)?,
            (None, true) => self.index_expr(out, index, depth, flow.inferred())?,
            (None, false) if !slicing && self.never_negative(flow, index) => {
                out.push("(");
                self.index_expr(out, index, depth, flow)?;
                out.push(") as usize");
            }
            (None, false) => {
                out.push("nikaia_std::index::at(");
                let flow = match counts_down {
                    true => flow.widened(),
                    false => flow,
                };
                self.index_expr(out, index, depth, flow)?;
                out.push(")");
            }
        }
        out.push(")");
        // **A held struct is read through its handle** (ADR-283
        // D4): what the brackets answer is then the struct, over
        // no longer than the read, exactly as for a list of plain
        // structs.
        if self.reads_through_handle(flow, base) {
            out.push(".get()");
        }
        Ok(())
    }

    /// The thing a `.` or a `[` is applied to, parenthesised where it binds
    /// looser than the postfix does.
    ///
    /// A postfix binds tighter than everything except another postfix in both
    /// languages, so `(a as f64).sqrt()` and `(a + b).len()` need their
    /// parentheses back: the parser drops them - a group is not a node, it is
    /// how the tree was written - and without them the emitted Rust means
    /// something else and often still compiles.
    fn postfix_base(&self, out: &mut Out, expr: &Expr, depth: usize, flow: Flow<'_>) -> Result<()> {
        // **A read through the brackets is a `*`**, which binds looser than a
        // postfix: `*get(…).len()` is the deref of the length. The one place
        // its parentheses belong (ADR-293 D33).
        let a_read = !flow.in_a_place
            && matches!(expr, Expr::Index { index, .. } if !self.slices(flow.statement, index));
        let parenthesise = a_read
            || self.emits_as_cast(expr, flow)
            || matches!(
                expr,
                Expr::Binary { .. }
                    | Expr::Unary { .. }
                    | Expr::Range { .. }
                    | Expr::If { .. }
                    | Expr::Match { .. }
                    // It lowers to a `match`, so it needs what a `match` needs.
                    | Expr::SafeMethod { .. }
                    | Expr::Block(_)
                    | Expr::Unsafe(_)
                    | Expr::Closure { .. }
                    | Expr::TryCatch { .. }
                    | Expr::Dsl { .. }
                    | Expr::Asm { .. }
                    // **A negative number before a `.`** (#417): `-7.abs()`
                    // is `-(7.abs())` below, so `(-7).wrapping_add(2)` came out
                    // as a different number.
                    | Expr::LitInt { negative: true, .. }
            );

        if parenthesise {
            out.push("(");
        }
        self.expr(out, expr, depth, flow)?;
        if parenthesise {
            out.push(")");
        }
        Ok(())
    }

    /// **Whether this comes out as a Rust `as` conversion**, whatever it was
    /// written as.
    ///
    /// Three things do: a written `as` (ADR-285 D12), `x.truncating_i32()`, which
    /// *is* Rust's `as` (D7), and `xs.len()`, whose `usize` becomes the `i64` the
    /// ledger promises ([ADR-285](../../../../docs/specification/adr/adr-285.md)
    /// D1). What they have in common is where they sit in Rust's precedence -
    /// below the unary operators, above the binary ones - which is not where a
    /// method call sits, so the two places that decide parentheses have to ask
    /// about the emitted shape rather than the written one.
    ///
    /// Asked in one place because the alternative is parenthesising always, and a
    /// parenthesis nobody needs is a Rust warning about a file nobody wrote
    /// (Part III, C.1).
    /// Whether `expr` is written ending in `as T`: a conversion, or an
    /// operation whose right operand is written bare and ends in one.
    fn ends_in_a_cast(&self, expr: &Expr, flow: Flow<'_>) -> bool {
        match expr {
            Expr::Binary { op, rhs, .. } => match &**rhs {
                Expr::Binary { op: inner, .. } if precedence(*inner) < precedence(*op) + 1 => false,
                rhs => self.ends_in_a_cast(rhs, flow),
            },
            _ => self.emits_as_cast(expr, flow),
        }
    }

    fn emits_as_cast(&self, expr: &Expr, flow: Flow<'_>) -> bool {
        match expr {
            Expr::Cast { .. } => true,
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                let name = self.text(*method);
                truncating(name).is_some()
                    || is_length(name, args)
                    || self.is_counted(receiver, *method, args, flow)
            }
            _ => false,
        }
    }

    /// **`std`'s count of a sequence**, which counts in `usize` below and is
    /// an `i64` here, as a length is (Part I 2.2): the checker says which
    /// `count()` resolved to it, so a program's own `count` is left alone.
    fn is_counted(
        &self,
        receiver: &Expr,
        method: winnow_grammar::Symbol,
        args: &[Expr],
        flow: Flow<'_>,
    ) -> bool {
        self.text(method) == "count"
            && args.is_empty()
            && self
                .counted
                .contains(&(flow.statement, crate::check::argument_shape(receiver)))
    }

    /// An operand of an operator, parenthesised only where it binds looser
    /// than the position it stands in.
    fn nested(
        &self,
        out: &mut Out,
        expr: &Expr,
        needs: u8,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        let parenthesise = match expr {
            // **A sum that ends in a conversion** reads the same way at the
            // left of a comparison: `at + w.len() as i64 < n` is `i64<n>` to
            // Rust as much as the conversion alone is (found moving
            // `contracts::spelling` into Nikaia, #125).
            Expr::Binary { op, .. } => {
                precedence(*op) < needs
                    || (needs == precedence(BinaryOp::Lt) && self.ends_in_a_cast(expr, flow))
            }
            // **An operand that comes out as a conversion**, in the two
            // positions where Rust reads it wrongly without parentheses.
            //
            // Beside a **unary** operator, because `as` binds looser: `-x as i64`
            // is `(-x) as i64`. And as the **left of a comparison**, where it is
            // not a precedence question at all - `x as i64 < k` is read as
            // `i64<k>`, the start of a generic argument list, and the program
            // does not parse. Both would otherwise be a `rustc` message about a
            // file nobody wrote (Part III, C.1).
            //
            // Nowhere else, and that is the same rule: `x as i64 + 1` needs no
            // parentheses and a parenthesis nobody needs is the other half of
            // C.1 - `unused_parens` is a warning about the generated file.
            _ => {
                self.emits_as_cast(expr, flow)
                    && (needs == u8::MAX || needs == precedence(BinaryOp::Lt))
            }
        };

        if parenthesise {
            out.push("(");
        }
        self.expr(out, expr, depth, flow)?;
        if parenthesise {
            out.push(")");
        }
        Ok(())
    }

    fn args(
        &self,
        out: &mut Out,
        // The callee as the source wrote it. Part I 2.3's wrap at an argument
        // is keyed by it, because a statement may hold several calls and one
        // call several arguments (`check::Checked::nullable_args`).
        callee: &str,
        args: &[Expr],
        // Which positional parameters of the callee take a **handle** on a shared
        // value by value, where a ledger describes the callee. Empty where nothing
        // does or nothing is known, which is almost every call.
        takes_a_handle: &[bool],
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        // **This call's mask and no other's**: taken here, so a call written
        // inside one of these arguments does not see it.
        let boxed_mask = self.boxed_args.borrow_mut().take();
        let part_counts = self.variant_counts.borrow_mut().take();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                out.push(", ");
            }
            self.hull_count.set(
                part_counts
                    .as_ref()
                    .and_then(|counts| counts.get(i).copied()),
            );
            // **A part of a variant that is boxed is built in its box**
            // ([ADR-246](../../docs/specification/adr/adr-246.md) D3), around
            // everything else this argument is written with.
            let boxed_here = boxed_mask
                .as_ref()
                .is_some_and(|parts| parts.get(i).copied().unwrap_or(false));
            if boxed_here {
                out.push("Box::new(");
            }
            // **A lookup's key of no known type** (0.0.235): the reference the
            // map wants, whichever of a number or a view the key turns out to
            // be (`check::Checked::lookup_keys`).
            if i == 0
                && self.lookup_keys.contains(&(
                    flow.statement,
                    callee.to_string(),
                    crate::check::argument_shape(arg),
                ))
            {
                out.push("nikaia_std::index::AsKey::as_key(&");
                self.expr(out, arg, depth, flow)?;
                out.push(")");
                continue;
            }
            // ADR-312 D1: a handle on a shared value is **duplicated, never
            // moved**, where it is handed on by value. There is no method for a
            // programmer to call, so this is where the step is written - and
            // unconditionally, not only where the name is used again (D2): a line
            // further down may not decide what a line further up does to a
            // cleanup point.
            //
            // Only where the argument **names** a handle. A temporary - a call
            // that hands one back - has no block of its own to die at the end of,
            // so there is no second owner and nothing to duplicate.
            let duplicate = takes_a_handle.get(i).copied().unwrap_or(false)
                && matches!(arg, Expr::Variable(_) | Expr::Field { .. });
            // Part I 2.3: a plain value in a parameter the callee declares
            // nullable.
            // **And which of the statement's calls this is**
            // (`check::argument_shape`): the three parts above name a
            // *parameter*, and a statement may call one function twice. The
            // shape is built only where something was recorded for this
            // statement, callee and position, so the common argument pays a
            // lookup and no allocation beyond the one this key already made.
            let (before, after) = Self::around_value(
                self.nullable_args
                    .get(&(flow.statement, callee.to_string(), i))
                    .and_then(|by_shape| by_shape.get(&crate::check::argument_shape(arg)))
                    .copied(),
                arg,
            );
            // **A count the language below wants in `usize`**
            // ([ADR-285](../../../docs/specification/adr/adr-285.md) D15), which
            // is the parameter direction of ADR-285 D1. The ledger writes such a
            // parameter as the `i64` a program can hold and the conversion is
            // emitted, so a user writes `"  ".repeat(indent)` and never
            // `indent as usize` - a conversion into a type Part I 2.2 does not
            // offer, which is what that line used to be.
            //
            // Not where the argument is written **only in literals**, for
            // `index::at`'s reason: `of(2)` has nothing to infer its argument
            // type from, every integer type answers with the same `usize`, and
            // `cannot infer type` about a generated file is what Part III C.1
            // forbids. Rust's own inference already gives a literal the `usize`.
            // **And a `usize` a C declaration names is one too**
            // ([ADR-302](../../docs/specification/adr/adr-302.md) D6): the
            // declaration says `size_t` and a caller hands over the `i64` this
            // language has, so the conversion is written here. Read off the
            // declaration rather than off a list of names, which is what
            // `is_count` is and has to be for a ledger's entries.
            let wants_a_size = is_count(callee, i)
                || self.takes_a_size(callee, i)
                || self
                    .count_args
                    .contains(&(flow.statement, callee.to_string(), i));
            let count = wants_a_size && !only_literals(arg);
            // **The caller writes no `&`** ([ADR-094](../../docs/specification/adr/adr-094.md)
            // D1): where the callee reads this argument rather than keeping it,
            // the reference is the compiler's, and it is written here. The
            // *declaration* reads the same column, so the two cannot disagree —
            // and where the source wrote one itself, `NK1137` has already
            // refused the program rather than letting a `&&T` reach the
            // language below.
            let shape = crate::check::argument_shape(arg);
            let key = (flow.statement, callee.to_string(), i);
            let lend = self
                .lent_args
                .get(&key)
                .is_some_and(|shapes| shapes.contains(&shape))
                // **A literal is a view already** (ADR-282 D5): lent to a
                // `&str` it is written as it is, and a `&` in front of it would
                // be a `&&str` the language below has to see through.
                && !matches!(arg, Expr::LitStr { at, .. } if !self.owned_texts.contains(&self.text_at(*at as usize)));
            // **And `&mut` for a parameter the callee declared `mut`** (D3),
            // which is the one of the three states the *author* wrote rather
            // than the inference. The two maps are disjoint by construction:
            // `lends` withholds its claim on a `mut` position.
            let change = self
                .mut_args
                .get(&key)
                .is_some_and(|shapes| shapes.contains(&shape));
            // **A lambda handed to a parameter whose type may pause**
            // ([ADR-277](../../docs/specification/adr/adr-277.md) D11): a
            // closure that returns a **boxed future**, which is what a callee
            // declared `impl Fn(A) -> Pin<Box<dyn Future<…>>>` takes, so the
            // shape is written out. It is written out because the **callee's**
            // declaration names it and not because the language below lacks an
            // `async` closure - it has one
            // ([ADR-277](../../docs/specification/adr/adr-277.md) D13), and
            // whether the callee should name it instead is the question that
            // record leaves open.
            //
            // `move`, because the future outlives the closure body it is made
            // in and what it captures has to go with it — which is Part I 5.4's
            // detached case, arrived at from the lowering rather than from a
            // word.
            let future = matches!(arg, Expr::Closure { .. })
                && self.future_lambdas.contains(&(flow.statement, i))
                && !self
                    .kept_lambdas
                    .contains_key(&(flow.statement, crate::check::argument_shape(arg)));
            // **The pointer a C declaration takes**
            // ([ADR-302](../../docs/specification/adr/adr-302.md) D5). The
            // declaration says `&[u8]` and C wants an address, so the address
            // is made here: `bytes.as_ptr()` for a run and a plain `&` for one
            // value, which Rust coerces to `*const T` on its own.
            //
            // It is exclusive with the two above by construction rather than by
            // an `else`: a boundary type is not a view in the ledger's type
            // language and not something that moves, so `keeps::lends` withholds
            // its claim and `mut_args` never names the position.
            let pointer = self
                .foreign_params
                .get(callee)
                .and_then(|params| params.get(i))
                .and_then(|ty| self.pointer_for(callee, ty));
            // **Into a parameter both kinds of text flow into, each argument
            // goes as it is** ([ADR-282](../../docs/specification/adr/adr-282.md)
            // D2): a view borrowed, text of its own moved in. Written here and
            // not in the program, so a call inside an `f"…"` hole gets it too.
            // A parameter the callee only reads is a `&str` (above), and the
            // `&` lends either kind to it.
            let either =
                !lend && !change && !matches!(arg, Expr::LitNull) && self.either_param(callee, i);
            out.push(before);
            if either {
                out.push("nikaia_std::either_text::either(");
            }
            // **Neither `&` at the boundary**, and that is not an ordering
            // choice: what a C declaration takes is decided by the declaration
            // ([ADR-302](../../docs/specification/adr/adr-302.md) D5, D7), and
            // a Rust reference in front of it would be the wrong address —
            // `&FILE` is `FILE**` where C wants `FILE*`.
            // **A nullable view is lent inside its option** (issue #171
            // issue #171): a `ref String?` parameter is an `Option<&str>` (Part I
            // 2.3), and what reaches it from a `String?` is `x.as_deref()` -
            // `&x` was an `&Option<String>`. A `null` is `None` as it is.
            // A method is keyed `Type::method` in the ledger, and a call
            // names only the method: its parameter is the one every method of
            // that name agrees on (#462).
            let opening_of = |ty: &crate::contracts::ty::Ty| match ty {
                crate::contracts::ty::Ty::Nullable(inner) if inner.is_a_view() => {
                    Some(match inner.as_ref() {
                        crate::contracts::ty::Ty::Named { name, .. }
                            if matches!(crate::contracts::ty::base(name), "String" | "str") =>
                        {
                            ".as_deref()"
                        }
                        _ => ".as_ref()",
                    })
                }
                _ => None,
            };
            let inside_the_option = lend
                .then(|| match self.own_contracts.functions.get(callee) {
                    Some(contract) => contract
                        .signature
                        .as_ref()
                        .and_then(|s| s.arguments().get(i))
                        .and_then(|(_, ty)| opening_of(ty)),
                    None => {
                        let openings: Vec<Option<&'static str>> = self
                            .own_contracts
                            .candidates(callee)
                            .iter()
                            .map(|(_, contract)| {
                                contract
                                    .signature
                                    .as_ref()
                                    .and_then(|s| s.arguments().get(i))
                                    .and_then(|(_, ty)| opening_of(ty))
                            })
                            .collect();
                        match openings.first() {
                            Some(first) if openings.iter().all(|o| o == first) => *first,
                            _ => None,
                        }
                    }
                })
                .flatten();
            // **A map's read is that option already**: `index::get` answers
            // an `Option<&T>`, and so does a read `index::flat` writes for a
            // map of `T?` ([ADR-293](../../docs/specification/adr/adr-293.md)
            // D3) - so nothing is opened, and text is read as `str`. It was
            // `(*get(&m, &k)).as_ref()`, an `Option<&&T>` (found writing
            // ADR-293's tests).
            let a_map_read = matches!(arg, Expr::Index { index, .. }
                    if self.map_key(flow.statement, index, false).is_some())
                || self
                    .flat_reads
                    .contains(&(flow.statement, crate::check::argument_shape(arg)));
            let inside_the_option = match (inside_the_option, a_map_read) {
                (Some(".as_deref()"), true) => Some(".map(String::as_str)"),
                (Some(_), true) => Some(""),
                (other, _) => other,
            };
            // **A plain value put into the option is lent inside it**:
            // `Some(&value)`, which reaches an `Option<&str>` from a `String`
            // by the coercion the constructor's argument gets
            // (`check::Checked::nullable_args`).
            let inside_the_option = match before {
                "Some(" => None,
                _ => inside_the_option,
            };
            let lend = lend && inside_the_option.is_none();
            // **A function value a method took whole is lent on to code that
            // only runs it** (#463): a method's code parameter is never lent
            // (`lends.nika`), so it is an `impl Fn` by value, and handing it
            // on moved it - in a loop, before the second turn. `&f` is a `Fn`
            // wherever `f` is.
            // Not to a call of the function it stands in (#464): each `&` is
            // a new type there, and the function would instantiate itself
            // without end. The value goes on whole, as it did before.
            let calls_itself = flow.function == callee
                || flow
                    .function
                    .strip_suffix(callee)
                    .is_some_and(|owner| owner.ends_with("::"));
            let lend = lend
                || (!change
                    && !calls_itself
                    && matches!(arg, Expr::Variable(name)
                        if self.a_code_parameter_taken_whole(flow.function, self.text(*name)))
                    && self.runs_code_at(callee, i));
            if matches!(arg, Expr::LitNull) && inside_the_option.is_some() {
                out.push("None");
                out.push(after);
                continue;
            }
            if pointer.is_none() {
                if change {
                    out.push("&mut ");
                } else if lend {
                    out.push("&");
                }
            }
            if inside_the_option.is_some_and(|how| !how.is_empty()) {
                out.push("(");
            }
            if let Some(Pointer::Reference { mutable }) = pointer {
                out.push(match mutable {
                    true => "&mut ",
                    false => "&",
                });
            }
            if count {
                out.push("nikaia_std::count::of(");
            }
            // A count written only in literals is left for Rust to infer as a
            // `usize`, so a suffix may not be written into it either.
            let inside = match wants_a_size && !count {
                true => flow.inferred(),
                false => flow,
            };
            match (future, arg) {
                // `|a| Box::pin(async move { … })`, which is what a parameter
                // declared `impl Fn(A) -> Pin<Box<dyn Future<…>>>` takes
                // ([ADR-277](../../docs/specification/adr/adr-277.md) D11).
                //
                // **`in_lambda` is deliberately not set**, and that is D2: the
                // refusal of a pausing body inside a lambda existed because the
                // lowering had no shape for one. This *is* the shape, so a body
                // that pauses is an ordinary body here — the `async` block is
                // where its `.await`s belong.
                (
                    true,
                    Expr::Closure {
                        params,
                        mutable,
                        body,
                    },
                ) => {
                    let names: Vec<String> =
                        params.iter().map(|p| self.name(*p).into_owned()).collect();
                    // **An `async` closure where the callee runs the
                    // parameter** ([ADR-277](../../docs/specification/adr/adr-277.md)
                    // D1), and the boxed future where it keeps one. The two are
                    // what the *declaration* writes at the same position, off
                    // the same `keeps` column.
                    //
                    // **No `move` on the run shape**, which is
                    // [Part I 5.4](../../docs/specification/10-nikaia-light.md)
                    // A: a lambda handed to a parameter the body only calls
                    // **borrows** what it captures, because the call is over
                    // before the caller's frame is. The boxed shape keeps its
                    // `move`, for the reason it always had - the future
                    // outlives the closure body it is made in.
                    let runs = self.run_lambdas.contains(&(flow.statement, i));
                    let opened = match runs {
                        true => format!("async |{}| ", names.join(", ")),
                        false => format!("|{}| Box::pin(async move ", names.join(", ")),
                    };
                    out.push(&opened);
                    let mut changed: Vec<Symbol> = inside.changed.to_vec();
                    changed.extend(mutable.iter().copied());
                    let body_flow = Flow {
                        changed: &changed,
                        statement: inside.statement,
                        ..Flow::PLAIN
                    };
                    self.block(out, body, depth, body_flow, Tail::Return)?;
                    if !runs {
                        out.push(")");
                    }
                }
                _ => {
                    let held = self
                        .hold_args
                        .borrow()
                        .as_ref()
                        .and_then(|h| h.get(&i).cloned());
                    match held {
                        // **Held by the keeps of the buffers it points into**
                        // (ADR-283 D12, ADR-283 D14): a view of text becomes a
                        // `Held`, a struct of views a `Holding`, and which one
                        // is the language below's to pick by the value's type.
                        // No `unsafe`: each view is found again in its keep by
                        // address, so the plan is checked rather than trusted.
                        Some(buffers) => {
                            let keeps = buffers
                                .iter()
                                .map(|b| format!("std::sync::Arc::clone(&__keep_{b})"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            out.push("{ use nikaia_std::tether::Hold as _; (");
                            self.expr(out, arg, depth, inside)?;
                            out.push(&format!(").hold([{keeps}]) }}"));
                        }
                        None => {
                            // A kept value handed to a parameter that only
                            // runs it: the closure it holds.
                            if self
                                .kept_args
                                .contains(&(flow.statement, crate::check::argument_shape(arg)))
                            {
                                out.push("&*");
                            }
                            // **A view of a value that copies, where the value
                            // is wanted** (ADR-233 D4): copied out of it.
                            if self
                                .copied_args
                                .get(&key)
                                .is_some_and(|shapes| shapes.contains(&shape))
                            {
                                out.push("*");
                                self.postfix_base(out, arg, depth, inside)?
                            } else {
                                self.expr(out, arg, depth, inside)?
                            }
                        }
                    }
                }
            }
            // `.as_ptr()` and `.as_mut_ptr()`, which a `Vec`, an `Array` and
            // text all answer - so one call writes the address of whatever a
            // caller lends a `&[T]` parameter (D1).
            if let Some(Pointer::Run { mutable }) = pointer {
                out.push(match mutable {
                    true => ".as_mut_ptr()",
                    false => ".as_ptr()",
                });
            }
            // **A handle is lent unless this call is its cleanup** (D3): the
            // address goes by value and the caller keeps the handle, so its
            // release still runs at the end of its scope. The release function
            // itself takes the handle, and Rust's own move is what keeps it
            // from being released twice.
            if let Some(Pointer::Handle { give: false }) = pointer {
                out.push(".lent()");
            }
            if let Some(how) = inside_the_option.filter(|how| !how.is_empty()) {
                out.push(")");
                out.push(how);
            }
            if count {
                out.push(")");
            }
            if either {
                out.push(")");
            }
            out.push(after);
            if duplicate {
                // `.clone()` and not `Rc::clone(&x)`: it is right under either
                // count, so it cannot disagree with the type the position was
                // lowered to. `Rc<T>` and `Arc<T>` implement `Clone` themselves,
                // so this steps the count and never copies `T`.
                out.push(".clone()");
            }
            if boxed_here {
                out.push(")");
            }
        }
        Ok(())
    }

    /// Which of a callee's positional parameters take a handle on a shared value
    /// **by value**, where a ledger describes the callee.
    ///
    /// Resolved the way `contracts::sharing::Analysis::parameters` resolves it -
    /// this program's own ledger first, then `std`'s, by name and then by suffix
    /// (ADR-296 D17) - so that the duplication written here and the duplication
    /// `--sharing` names are the same call's.
    ///
    /// A `&Shared[T]` parameter answers **no**, which is ADR-312 D1's correction:
    /// lending the inner value out hands no handle on.
    fn takes_a_handle(&self, callee: &str) -> Vec<bool> {
        let suffix = format!("::{callee}");
        let contract = [&self.own_contracts, self.library]
            .into_iter()
            .find_map(|ledger| {
                ledger.functions.get(callee).or_else(|| {
                    ledger
                        .functions
                        .iter()
                        .find(|(key, _)| key.ends_with(&suffix))
                        .map(|(_, contract)| contract)
                })
            });
        contract
            .and_then(|contract| contract.signature.as_ref())
            .map(|signature| {
                signature
                    .arguments()
                    .iter()
                    .map(|(_, ty)| {
                        // `SharedMut[T]` is a count around a lock
                        // ([ADR-281](../../../docs/specification/adr/adr-281.md)
                        // D1), so a handle on one is handed on the same way.
                        matches!(
                            ty,
                            crate::contracts::ty::Ty::Named { name, view: false, .. }
                                if name == SHARED || name == SHARED_MUT
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The same, for the callee a `Call` names.
    fn takes_a_handle_at(&self, func: &Expr) -> Vec<bool> {
        let name = match func {
            Expr::Variable(name) => self.text(*name).to_string(),
            Expr::Path(segments) => segments
                .iter()
                .map(|s| self.text(*s))
                .collect::<Vec<_>>()
                .join("::"),
            _ => return Vec::new(),
        };
        // Part I 4.2's anonymous constructor is reached as `Type(…)` and recorded
        // as `Type::new`, which is the one place the call's spelling and the
        // ledger's key differ.
        match self.structs.iter().any(|s| self.text(*s) == name) {
            true => self.takes_a_handle(&format!("{name}::new")),
            false => self.takes_a_handle(&name),
        }
    }

    /// `Measurements.file(data)` - the whole of what a user writes to run a
    /// grammar. Everything the parallel form needs is already in the grammar
    /// (ADR-296): the frame says where the input may be cut, the `par_fold`
    /// says how the pieces combine. What is left is choosing the executor, and
    /// that is the build's decision, not the program's.
    fn grammar_entry(
        &self,
        out: &mut Out,
        grammar: Symbol,
        entry: Symbol,
        input: &Expr,
        depth: usize,
        flow: Flow<'_>,
    ) -> Result<()> {
        // **The `?` is the call's, and a `catch` is what takes it away.** A
        // grammar entry propagates on its own everywhere else (Kap 7.1); inside
        // the half a `catch` guards, the `match` around it handles the failure,
        // so the `Result` has to arrive whole. Read off `flow` rather than
        // passed in, which is how every other call decides it.
        let question = match flow.caught {
            false => "?",
            true => "",
        };
        let name = self.text(grammar);
        let def = self.grammars.get(&grammar).ok_or_else(|| {
            refused_at!(
                flow.statement,
                "There's no grammar called `{name}` in this file."
            )
        })?;

        // **The rule is named at the call** ([ADR-296](../../docs/specification/adr/adr-296.md)
        // D1, D2). It used to be picked here — the first `entry` rule, a
        // `par_fold` one beating an earlier one — so a grammar with two of them
        // got one by source order, in silence. Every `entry` rule is an entry
        // now, and which one is what the program wrote.
        let rule = def
            .rules
            .iter()
            .find(|r| r.is_entry && r.name == entry)
            .ok_or_else(|| {
                let rule = self.text(entry);
                match def.rules.iter().any(|r| r.name == entry) {
                    // **A rule that is not an entry is the grammar's own**
                    // (D25), and saying *there is no such rule* about one
                    // written three lines up is the message a reader cannot
                    // act on.
                    true => refused_at!(
                        flow.statement,
                        "`{rule}` in grammar `{name}` isn't an entry, so the program can't \
                         call it. Write `entry rule {rule}`."
                    ),
                    false => refused_at!(
                        flow.statement,
                        "Grammar `{name}` has no entry called `{rule}`."
                    ),
                }
            })?;
        let rule_name = self.text(rule.name);

        // The input goes to `std` once, which needs it twice: once to parse
        // and once to say *where* a failure was. A `ParseError` knows the
        // offset and not the text, so only `render` can turn "at 1042" into
        // "at line 37, column 9" - and a program that reports a rejected file
        // without saying which line is not much better than one that panics.
        // `&*` because the parser takes the text: a mapping, an owned string
        // and a view all reach it the same way, and none of them has to be
        // named here.
        // Both are calls of `std`'s (`grammar::parse`, `grammar::parse_pieces`)
        // and not blocks, so the call is an expression wherever it stands - a
        // block of statements in a `match`'s scrutinee is refused by `clippy`.
        if par_fold_of(rule).is_some() {
            out.push("nikaia_std::grammar::parse_pieces(&*");
            self.expr(out, input, depth, flow)?;
            out.push(&format!(
                ", |source| {name}::parse_{rule_name}_pieces(source, \
                 &ParseContext::<()>::default(), {})){question}",
                self.build.parallelism()
            ));
            return Ok(());
        }

        // A sequential entry rule: no pieces to cut, so the parser is driven
        // over the whole input once.
        out.push("nikaia_std::grammar::parse(&*");
        self.expr(out, input, depth, flow)?;
        out.push(&format!(", {name}::parse_{rule_name}()){question}"));
        Ok(())
    }
}

// --- Free helpers ---

/// ADR-296 D9: the attribute is keyed in both languages, and the lowering is
/// one-to-one. A bare `@frame` stays bare - the boundary is then the rule's
/// trailing literal, which the backend infers and checks.
fn frame_attribute(frame: &FrameAttr) -> String {
    let mut keys = Vec::new();
    if let Some(boundary) = &frame.boundary {
        keys.push(format!("boundary = \"{boundary}\""));
    }
    if frame.unchecked {
        keys.push("unchecked".to_string());
    }

    if keys.is_empty() {
        "#[frame]".to_string()
    } else {
        format!("#[frame({})]", keys.join(", "))
    }
}

/// What every arm but the last answers, where a `match` is one that only says
/// `true` or `false`: those arms one answer, a last `else` the other, and no
/// guard anywhere. `None` for every other `match`.
fn a_yes_or_no(arms: &[crate::ast::MatchArm]) -> Option<bool> {
    let (last, rest) = arms.split_last()?;
    if rest.is_empty()
        || !matches!(last.pattern, MatchPattern::Otherwise)
        || arms.iter().any(|arm| arm.guard.is_some())
    {
        return None;
    }
    let Expr::LitBool(otherwise) = last.body else {
        return None;
    };
    rest.iter()
        .all(|arm| matches!(arm.body, Expr::LitBool(b) if b != otherwise))
        .then_some(!otherwise)
}

/// The template rule `list(item, sep)` lowers to (Part II 10.8).
const LIST_RULE: &str = "__nikaia_list";

/// Whether a pattern writes `list(item, sep)` anywhere in it.
fn writes_a_list(parsed: &Parsed, pattern: &Spanned<Pattern>) -> bool {
    match &pattern.node {
        Pattern::Seq(parts) | Pattern::Choice(parts) => {
            parts.iter().any(|p| writes_a_list(parsed, p))
        }
        Pattern::Bind { pat, .. } | Pattern::Repeat { pat, .. } | Pattern::Group(pat) => {
            writes_a_list(parsed, pat)
        }
        Pattern::Ref { name, args, .. } => {
            (parsed.text(*name) == "list" && args.len() == 2)
                || args.iter().any(|p| writes_a_list(parsed, p))
        }
        Pattern::Literal(_) | Pattern::Cut | Pattern::Fold(_) => false,
    }
}

fn repeat_suffix(rep: Repeat) -> String {
    match rep {
        Repeat::Star => "*".to_string(),
        Repeat::Plus => "+".to_string(),
        Repeat::Optional => "?".to_string(),
        Repeat::Exactly(n) => format!("{{{n}}}"),
        Repeat::AtLeast(n) => format!("{{{n},}}"),
        Repeat::Between(n, m) => format!("{{{n},{m}}}"),
    }
}

/// **An integer literal, and the one case it has to carry its own type**
/// ([ADR-285](../../../docs/specification/adr/adr-285.md) D21).
///
/// Part I 2.4 says a number takes the type its use asks for, and `i32` where
/// nothing asks. The first half is the language below's inference and works;
/// the second half was that language's *default*, inherited rather than chosen,
/// and it is what refused `let big = 3000000000` in the backend's words about a
/// type the program never wrote (Part III C.1).
///
/// So a literal an `i32` does not hold is written as an `i64`, and D3 is why
/// that needs no analysis at all: the integer types a program may write are
/// `i32` and `i64` ([ADR-285](../../../docs/specification/adr/adr-285.md)), an
/// integer literal is never a float, and `u8` is narrower — so a value an `i32`
/// cannot hold has no second answer a use could ask for. There is nothing to
/// find out.
///
/// **A literal that fits an `i32` is untouched**, which is what keeps every
/// program that compiles today compiling: the suffix would pin what the use is
/// supposed to decide, and `let small = 42` passed to a parameter taking an
/// `i64` is Part I 2.4's own example.
///
/// The value and not the digits, which is why `-2147483648` never reaches here
/// as `2147483648`: the negation is folded at the `Unary` arm above.
fn integer_literal(value: i128, widen: bool) -> String {
    match (i32::try_from(value), i64::try_from(value)) {
        (Ok(_), _) if !widen => value.to_string(),
        (_, Ok(_)) => format!("{value}i64"),
        // **Above an `i64` only a `u64` holds it** (ADR-285 D19), and the
        // checker has already refused it anywhere else.
        _ => format!("{value}u64"),
    }
}

/// Whether this expression is a **number written down**, sign and all.
///
/// The negation is part of the answer rather than a wrapper around it: the
/// unary arm above folds `-1` into one literal, and `-1.into()` in the language
/// below is `-(1.into())`, which is a second reason not to write the `into` at
/// all.
fn a_number(expr: &Expr) -> bool {
    match expr {
        Expr::LitInt { .. } | Expr::LitFloat(_) => true,
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
        } => a_number(expr),
        _ => false,
    }
}

fn unary_op(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
        UnaryOp::Ref => "&",
    }
}

/// Rust's binding strength, which Nikaia shares.
/// Whether an index is written **only in literals**, and therefore needs no
/// conversion and can take none.
///
/// `xs[0]`, `xs[-1]`, `&text[1..3]`, `xs[2 * 3]`: Rust's own inference gives each
/// of these the `usize` a sequence wants. And `nikaia_std::index::at` cannot help
/// here even where it would be harmless, because there is nothing to infer its
/// argument type *from* - every integer type answers with the same `usize`, so the
/// call is ambiguous and `cannot infer type` about a generated file is what
/// Part III C.1 forbids.
/// Whether this expression **leaves** rather than coming to a value
/// ([ADR-276](../../docs/specification/adr/adr-276.md) D20).
///
/// What it decides is where the jump is *written*: a closure is a function
/// boundary ([ADR-276](../../docs/specification/adr/adr-276.md) D16), so a
/// lowering that puts an expression inside one has to know whether that
/// expression is a jump before it does.
fn jumps(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Throw(_) | Expr::Return(_) | Expr::Break | Expr::Continue
    )
}

/// **A list a function value only reads is a slice** (#458), as a
/// function's own parameter is (`plain_list`): a `ref Vec[T]` parameter is
/// written `&[T]`, and a function type that took `&Vec<T>` could not be
/// handed it.
fn a_list_read_as_a_slice(written: String) -> String {
    match written
        .strip_prefix("&Vec<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        Some(element) => format!("&[{element}]"),
        None => written,
    }
}

/// Whether a `??`'s fallback ends in a jump: a jump itself, another `??`
/// whose own fallback does - the chain `a ?? b ?? continue` is one fallback
/// that may leave - or a block whose last statement does (#457).
fn jumps_at_the_end(expr: &Expr) -> bool {
    match expr {
        Expr::Coalesce { fallback, .. } => jumps_at_the_end(fallback),
        Expr::Block(block) => match block.stmts.last().map(|s| &s.node) {
            Some(Stmt::Return(_) | Stmt::Break | Stmt::Continue) => true,
            Some(Stmt::Expr(inner)) => jumps_at_the_end(inner),
            _ => false,
        },
        _ => jumps(expr),
    }
}

/// **How a call makes the address an `extern "C"` declaration takes**
/// ([ADR-302](../../docs/specification/adr/adr-302.md) D5).
///
/// Two shapes, because C's two are a run of elements and one value: a run is a
/// pointer to the first element and the length is a parameter of its own (D2),
/// and one value is an address Rust writes for a `&` on its own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pointer {
    /// `&[T]` and `&mut [T]`: `.as_ptr()`, `.as_mut_ptr()`.
    Run { mutable: bool },
    /// `&T` and `&mut T`: a plain `&`, which Rust coerces to `*const T`.
    Reference { mutable: bool },
    /// **An opaque handle** (D3), passed as the address by value.
    ///
    /// `give` is true for the type's own **release** function and false for
    /// every other declaration: that one call *is* the cleanup, so the handle
    /// goes with it and Rust's own move is what keeps it from being released
    /// twice. Everywhere else the caller keeps the handle, because a C function
    /// that is handed one does not take it — `fileno(f)` reads it and `f` is
    /// still the caller's to close.
    Handle { give: bool },
}

/// Which of the two a declared parameter is, or `None` where it is an ordinary
/// value an `i32` is at both ends.
fn pointer_for(ty: &Type) -> Option<Pointer> {
    match (ty.is_slice, ty.is_view) {
        (true, _) => Some(Pointer::Run { mutable: ty.is_mut }),
        (false, true) => Some(Pointer::Reference { mutable: ty.is_mut }),
        (false, false) => None,
    }
}

/// `const` or `mut`, the word Rust puts between the `*` and the type.
fn pointing(mutable: bool) -> &'static str {
    match mutable {
        true => "mut",
        false => "const",
    }
}

/// Whether a written index **counts from the end**
/// ([ADR-285](../../../docs/specification/adr/adr-285.md) D1).
///
/// One reader: a range. A negative one is an access out of bounds and reports
/// as one at run time — and it has to *reach* run time, which handed over as
/// written it does not, because the `usize` the slice wants has no negation.
fn a_negation_inside(index: &Expr) -> bool {
    match index {
        Expr::Unary {
            op: UnaryOp::Neg, ..
        } => true,
        Expr::Unary { expr, .. } => a_negation_inside(expr),
        Expr::Binary { lhs, rhs, .. } => a_negation_inside(lhs) || a_negation_inside(rhs),
        Expr::Range { start, end, .. } => a_negation_inside(start) || a_negation_inside(end),
        _ => false,
    }
}

fn only_literals(index: &Expr) -> bool {
    match index {
        Expr::LitInt { .. } => true,
        Expr::Unary { expr, .. } => only_literals(expr),
        Expr::Binary { lhs, rhs, .. } => only_literals(lhs) && only_literals(rhs),
        Expr::Range { start, end, .. } => only_literals(start) && only_literals(end),
        _ => false,
    }
}

/// `xs.len()` - the four `std` entries that hand back a length, by the name all
/// four of them have ([ADR-285](../../../docs/specification/adr/adr-285.md) D1).
fn is_length(method: &str, args: &[Expr]) -> bool {
    method == "len" && args.is_empty()
}

/// Whether a callee's argument at `at` is a **count** the language below takes in
/// `usize` ([ADR-285](../../../docs/specification/adr/adr-285.md) D15).
///
/// A name and a position, not a rule, for the reason `is_length` is one: what it
/// encodes is a fact about *Rust's* library rather than about this language. The
/// list is the entries of `std.contracts` whose Rust counterpart counts in
/// `usize`, and today it is one of them - `str::repeat`, which is the site
/// ADR-285 named when it wrote this direction down as open.
/// `a_count_parameter_is_an_i64` in `tests/contracts.rs` keeps the two from
/// drifting apart.
///
/// Being a name is safe here because `count::of` is the **identity** for
/// anything that is not a number: a method of the program's own called `repeat`
/// passes through it untouched, which is the same property that lets `index::at`
/// be written around every index.
fn is_count(callee: &str, at: usize) -> bool {
    matches!((callee, at), ("repeat", 0))
}

fn precedence(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => 14,
        BinaryOp::Add | BinaryOp::Sub => 13,
        // Rust's order below the sums (ADR-285 D10), so a bit operation is
        // parenthesised where the tree needs it and written bare where not.
        BinaryOp::Shl | BinaryOp::Shr => 12,
        BinaryOp::BitAnd => 11,
        BinaryOp::BitXor => 10,
        BinaryOp::BitOr => 9,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            7
        }
        BinaryOp::And => 4,
        BinaryOp::Or => 3,
    }
}

fn binary_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
    }
}

/// **The names a `fold`'s or a `par_fold`'s step calls directly** (#433): a
/// function it calls and a method it calls, by name, in any rule of any
/// grammar this unit declares. By name, because the step's receiver is the
/// accumulator, whose type this emitter does not know (ADR-296 D17) - and a
/// method of another type with the same name gains a hint, nothing more.
fn fold_callees(parsed: &Parsed, grammars: &HashMap<Symbol, &GrammarDef>) -> HashSet<String> {
    fn steps<'a>(pattern: &'a Pattern, found: &mut Vec<&'a Expr>) {
        match pattern {
            Pattern::Fold(spec) => found.push(&spec.step),
            Pattern::Seq(all) | Pattern::Choice(all) => {
                all.iter().for_each(|p| steps(&p.node, found));
            }
            Pattern::Bind { pat, .. } | Pattern::Repeat { pat, .. } => steps(&pat.node, found),
            Pattern::Group(inner) => steps(&inner.node, found),
            Pattern::Ref { args, .. } => args.iter().for_each(|p| steps(&p.node, found)),
            Pattern::Literal(_) | Pattern::Cut => {}
        }
    }
    let mut found = Vec::new();
    for def in grammars.values() {
        for rule in &def.rules {
            for alt in &rule.alts {
                steps(&alt.pattern.node, &mut found);
            }
        }
    }
    let mut names = HashSet::new();
    for step in found {
        let Expr::Closure { body, .. } = step else {
            continue;
        };
        for stmt in &body.stmts {
            crate::contracts::sync::visit_stmt(parsed, &stmt.node, &mut |e| match e {
                Expr::MethodCall { method, .. } => {
                    names.insert(parsed.text(*method).to_string());
                }
                Expr::Call { func, .. } => {
                    if let Expr::Variable(name) = &**func {
                        names.insert(parsed.text(*name).to_string());
                    }
                }
                _ => {}
            });
        }
    }
    names
}

/// The `par_fold` a rule *is*, if it is one. A fold with a merge is the only
/// body the backend generates a piece driver for.
fn par_fold_of(rule: &GrammarRule) -> Option<&FoldSpec> {
    rule.alts.iter().find_map(|alt| match &alt.pattern.node {
        Pattern::Fold(spec) if spec.parallel => Some(spec),
        Pattern::Bind { pat, .. } => match &pat.node {
            Pattern::Fold(spec) if spec.parallel => Some(spec),
            _ => None,
        },
        _ => None,
    })
}

/// **A lambda whose body pauses, which this lowering cannot write.**
///
/// **What is left of it is a callee nothing describes.** A lambda that pauses
/// is written as an `async` closure where the parameter is declared in this
/// language ([ADR-277](../../../docs/specification/adr/adr-277.md) D11), and
/// handed to the counterpart in `nikaia_std::seq` where it is one of `std`'s
/// entries ([ADR-233](../../../docs/specification/adr/adr-233.md)) - every one
/// of them: `map`, `filter`, a list's `map`, `sort_by_key`, `or_insert_with`,
/// `and_modify`, and the four doors, where a pause is `NK2202` by the
/// language's own rule, and `par_iter()`'s walks, where it is `NK2209`
/// ([ADR-235](../../../docs/specification/adr/adr-235.md)). What reaches this
/// is a lambda handed to a method whose receiver the checker could not
/// resolve, so there is no signature to say what shape the lambda takes, and a
/// plain closure holding an `.await` is a `rustc` error about a file nobody
/// wrote (Part III, C.1).
fn pausing_in_a_lambda(at: usize, callee: &str) -> anyhow::Error {
    refused_at!(
        at,
        "This lambda calls `{callee}`, which can pause, but the compiler knows nothing \
         about what the lambda is passed to, so it can't let it pause. Call `{callee}` \
         outside the lambda and pass in the value."
    )
}

/// Whether an expression names a **place** — something that already exists and
/// can be pointed at — rather than a value this expression makes
/// ([ADR-094](../../../docs/specification/adr/adr-094.md) D4).
///
/// A name, a field of one, an element of one. A call, a range, a literal and a
/// method call are not: `for i in 0..n` counts, `for line in io::lines()` reads
/// a stream, and `for x in xs.drain()` is the written form of taking the
/// elements away — none of the three has anything to lend.
pub(crate) fn is_a_place(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Variable(_) | Expr::Field { .. } | Expr::SafeField { .. } | Expr::Index { .. }
    )
}

/// The types a ledger records a view-holding field for.
///
/// The package-wide half of [`Emitter::borrows`]. `tethered` is non-empty for
/// exactly the structs [`borrowing_structs`] finds in the file that declares
/// them (`Ledger::infer_checked` computes it from that set), so this carries the
/// same fact across a file boundary rather than recomputing a different one.
/// The types this unit declares an `impl Error for …` for.
///
/// The trait's name and nothing more: `Error` is the one trait this compiler
/// reads by name (`NK1130`'s note says so), and Part I 7.1 makes the `impl`
/// line the marker — *what is thrown implements `Error`, and the `impl` line
/// says so*.
fn declared_errors(parsed: &Parsed) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for item in &parsed.program.items {
        if let Item::Impl {
            trait_name: Some(trait_name),
            target,
            ..
        } = &item.node
            && parsed.text(*trait_name) == "Error"
        {
            out.insert(parsed.text(target.name).to_string());
        }
    }
    out
}

fn tethered_types(contracts: &crate::contracts::Ledger) -> std::collections::BTreeSet<String> {
    contracts
        .types
        .iter()
        .filter(|(_, contract)| !contract.tethered.is_empty())
        .map(|(name, _)| name.clone())
        .collect()
}

/// Which structs hold a view into the parser's input.
///
/// Part II, 10.6: a view is a slice of the input, so a struct holding one is
/// tied to the input as well - transitively, which is why this is a fixpoint
/// and not one pass.
pub(crate) fn borrowing_structs(parsed: &Parsed) -> HashSet<Symbol> {
    let mut fields_of: HashMap<Symbol, Vec<&Type>> = HashMap::new();
    let mut borrowing = HashSet::new();

    for item in &parsed.program.items {
        // An enum holds views the same way a struct does - in the types of
        // what its variants carry - so it is walked the same way.
        if let Item::Enum { name, variants, .. } = &item.node {
            let types: Vec<&Type> = variants
                .iter()
                .flat_map(|v| match &v.fields {
                    VariantFields::Unit => Vec::new(),
                    VariantFields::Tuple(types) => types.iter().collect(),
                    VariantFields::Named(fields) => fields.iter().map(|f| &f.ty).collect(),
                })
                .collect();
            if types.iter().any(|t| holds_view(t)) {
                borrowing.insert(*name);
            }
            fields_of.insert(*name, types);
        }
        if let Item::Struct { name, fields, .. } = &item.node {
            let types: Vec<&Type> = fields.iter().map(|f| &f.ty).collect();
            if types.iter().any(|t| holds_view(t)) {
                borrowing.insert(*name);
            }
            fields_of.insert(*name, types);
        }
    }

    loop {
        let mut changed = false;
        for (name, types) in &fields_of {
            if borrowing.contains(name) {
                continue;
            }
            if types.iter().any(|t| names_borrowing(t, &borrowing)) {
                borrowing.insert(*name);
                changed = true;
            }
        }
        if !changed {
            return borrowing;
        }
    }
}

/// **A function type holds what its result holds, and nothing it is
/// handed**: a kept function's parameters are borrowed for the call alone and
/// written without a lifetime ([`Emitter::ty_counted`]), so a struct whose
/// only field is `fn(ref String) -> i64` names no lifetime either - with one,
/// it was *lifetime parameter `'a` is never used*.
pub(crate) fn holds_view(ty: &Type) -> bool {
    if let Some(code) = &*ty.code {
        return (*code.result).as_ref().is_some_and(holds_view);
    }
    ty.is_view || ty.generics.iter().any(holds_view)
}

pub(crate) fn names_borrowing(ty: &Type, borrowing: &HashSet<Symbol>) -> bool {
    if let Some(code) = &*ty.code {
        return (*code.result)
            .as_ref()
            .is_some_and(|result| names_borrowing(result, borrowing));
    }
    borrowing.contains(&ty.name) || ty.generics.iter().any(|g| names_borrowing(g, borrowing))
}

/// A Rust string literal for text the template wrote itself.
///
/// The text is markup by definition - the template author typed it - so nothing
/// is escaped here; what is quoted is what Rust needs quoted to read the same
/// bytes back.
fn rust_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The type a `truncating_` method converts to, if the name is one
/// ([ADR-285](../../../docs/specification/adr/adr-285.md) D13).
///
/// The target is **in the name** rather than inferred, for the reason the
/// `wrapping_` names have their own entry per type: there is one method per
/// destination, and the ledger has to be able to write its signature down.
///
/// The list is closed and matches the ledger's entries. A name this does not
/// recognise is an ordinary method call, so a program that writes
/// `truncating_u8` is refused by the type checker for the method not existing
/// rather than quietly emitting a conversion nobody described.
fn truncating(method: &str) -> Option<&'static str> {
    match method.strip_prefix("truncating_")? {
        "i32" => Some("i32"),
        "i64" => Some("i64"),
        // ADR-285 D3.
        "u32" => Some("u32"),
        "u64" => Some("u64"),
        _ => None,
    }
}

/// The type a `checked_` conversion converts to, if the name is one
/// ([ADR-315](../../../docs/specification/adr/adr-315.md)): the four
/// `truncating_` has. `checked_add` and the arithmetic are Rust's own methods
/// and lower as any method does.
fn checked(method: &str) -> Option<&'static str> {
    match method.strip_prefix("checked_")? {
        "i32" => Some("i32"),
        "i64" => Some("i64"),
        "u32" => Some("u32"),
        "u64" => Some("u64"),
        _ => None,
    }
}

/// Walk every expression in a block, including the ones inside statements.
/// The functions of this program that can pause **and take part in a cycle**
/// ([ADR-055](../../../docs/specification/adr/adr-055.md) D6's first sharp
/// edge).
///
/// A recursive `async fn` has an infinitely sized future, and `rustc` says so:
/// *"recursion in an async fn requires boxing"*. `examples/json/src/main.nika` has two of
/// them - `show` and `longest` both walk a tree - so this is not a case to meet
/// later, it is the corpus on day one.
///
/// **The graph is syntactic, and over-approximating is the safe direction.** A
/// free call names its callee outright; a method call does not name its receiver's
/// type, so an edge is drawn to *every* pausing method of that name. Boxing a
/// call that did not need it costs one allocation and nothing else, while missing
/// one is a program that does not compile - so where the two readings differ this
/// takes the wider.
///
/// **The cycle test is reachability from a function to itself**, computed to a
/// fixpoint. `n` is the number of functions in one program, and the obvious
/// algorithm is the one whose correctness a reader can check.
fn pausing_reach(
    parsed: &Parsed,
    contracts: &crate::contracts::Ledger,
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    use std::collections::{BTreeMap, BTreeSet};

    let pausing: BTreeSet<&str> = contracts
        .functions
        .iter()
        .filter(|(_, contract)| !contract.sync_claim.is_sync())
        .map(|(name, _)| name.as_str())
        .collect();
    if pausing.is_empty() {
        return BTreeMap::new();
    }

    // A bare method name to every pausing key that ends in it, which is the
    // over-approximation above.
    let mut by_last: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for key in &pausing {
        let last = key.rsplit("::").next().unwrap_or(key);
        by_last.entry(last).or_default().push(key);
    }

    let edges_of = |body: &Block| -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut note = |name: &str| {
            let name = parsed.unaliased(name);
            if pausing.contains(name.as_str()) {
                out.insert(name);
            } else if let Some(keys) = by_last.get(name.as_str()) {
                out.extend(keys.iter().map(|k| k.to_string()));
            }
        };
        visit_block(body, &mut |expr| match expr {
            Expr::Call { func, .. } => match func.as_ref() {
                Expr::Variable(name) => note(parsed.text(*name)),
                Expr::Path(segments) => {
                    let joined: Vec<&str> = segments.iter().map(|s| parsed.text(*s)).collect();
                    note(&joined.join("::"));
                }
                _ => {}
            },
            Expr::MethodCall { method, .. } | Expr::SafeMethod { method, .. } => {
                note(parsed.text(*method))
            }
            _ => {}
        });
        out
    };

    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for item in &parsed.program.items {
        match &item.node {
            Item::Fn {
                name: Some(name),
                body,
                ..
            } => {
                graph.insert(parsed.text(*name).to_string(), edges_of(body));
            }
            Item::Impl {
                target, methods, ..
            } => {
                let target = parsed.text(target.name).to_string();
                for method in methods {
                    if let Item::Fn { name, body, .. } = &method.node {
                        let own = match name {
                            Some(name) => parsed.text(*name).to_string(),
                            None => "new".to_string(),
                        };
                        graph.insert(format!("{target}::{own}"), edges_of(body));
                    }
                }
            }
            _ => {}
        }
    }

    // Reachability to a fixpoint, then "reaches itself".
    let mut reaches: BTreeMap<String, BTreeSet<String>> = graph.clone();
    loop {
        let mut changed = false;
        for key in graph.keys() {
            let grown: BTreeSet<String> = reaches[key]
                .iter()
                .filter_map(|next| reaches.get(next))
                .flatten()
                .cloned()
                .collect();
            let entry = reaches.get_mut(key).expect("every key was inserted");
            let before = entry.len();
            entry.extend(grown);
            changed |= entry.len() != before;
        }
        if !changed {
            break;
        }
    }

    // Only what pauses, because only a pausing call is a future at all - and a
    // caller that cannot pause holds no future to be infinitely sized.
    reaches.retain(|key, _| pausing.contains(key.as_str()));
    for seen in reaches.values_mut() {
        seen.retain(|key| pausing.contains(key.as_str()));
    }
    reaches
}

/// **Which branches an `overlap` starts first**, for `--overlaps`
/// ([ADR-292](../../../docs/specification/adr/adr-292.md) D6).
///
/// The report is `contracts::order`'s and the answer is the emitter's: whether a
/// branch can pause is the ledger's `sync` column and the checker's answers
/// about method calls, and no analysis in that file may reach for either
/// (ADR-292b, which is why nothing in that file takes a build setting). So
/// it is handed over as a function, closed over an emitter built exactly as the
/// one that will lower the program.
pub fn branch_starts_first<'p>(
    parsed: &'p Parsed,
    build: Build,
    contracts: &crate::contracts::Ledger,
) -> impl Fn(&Spanned<Stmt>) -> bool + 'p + use<'p> {
    let emitter = Emitter::with_contracts(
        parsed,
        &[],
        build,
        crate::contracts::Provenance::Trusted,
        contracts.clone(),
        &crate::contracts::Ledger::blank(),
        // This asks whether a branch pauses, which no file a build read can
        // change — so D1's default is the honest answer here.
        &Reads::none(),
    );
    move |stmt| emitter.branch_pauses(stmt, Flow::PLAIN)
}

pub(crate) fn visit_block<'a>(block: &'a Block, f: &mut impl FnMut(&'a Expr)) {
    for stmt in &block.stmts {
        match &stmt.node {
            Stmt::Let { value, .. } | Stmt::Comptime { value, .. } => visit_expr(value, f),
            Stmt::Assign { target, value, .. } => {
                visit_expr(target, f);
                visit_expr(value, f);
            }
            Stmt::For { iter, body, .. } => {
                visit_expr(iter, f);
                visit_block(body, f);
            }
            Stmt::While { cond, body } => {
                visit_expr(cond, f);
                visit_block(body, f);
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    visit_expr(value, f);
                }
            }
            // Neither holds an expression, so there is nothing here to walk.
            Stmt::Break | Stmt::Continue => {}
            Stmt::Expr(expr) => visit_expr(expr, f),
        }
    }
}

/// Every expression inside this one, this one included.
///
/// `pub(crate)` because the checker reads it too
/// ([ADR-281](../../docs/specification/adr/adr-281.md)): `NK2205` asks whether
/// a `get` is written anywhere inside a `set`'s argument, and a second walk
/// over the same shape is a second thing to keep in step with the AST.
pub(crate) fn visit_expr<'a>(expr: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    f(expr);
    match expr {
        Expr::Block(block) | Expr::Unsafe(block) | Expr::Overlap(block) => visit_block(block, f),
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            visit_expr(cond, f);
            visit_block(then_branch, f);
            if let Some(block) = else_branch {
                visit_block(block, f);
            }
        }
        Expr::Call { func, args, .. } => {
            visit_expr(func, f);
            args.iter().for_each(|a| visit_expr(a, f));
        }
        Expr::MethodCall { receiver, args, .. } | Expr::SafeMethod { receiver, args, .. } => {
            visit_expr(receiver, f);
            args.iter().for_each(|a| visit_expr(a, f));
        }
        Expr::Match { value, arms } => {
            visit_expr(value, f);
            for arm in arms {
                visit_expr(&arm.body, f);
            }
        }
        Expr::Tuple(parts) | Expr::ListLit { items: parts, .. } => {
            parts.iter().for_each(|p| visit_expr(p, f))
        }
        Expr::Field { base, .. } | Expr::SafeField { base, .. } => visit_expr(base, f),
        Expr::StructLit { fields, .. } => fields
            .iter()
            .filter_map(|field| field.value.as_ref())
            .for_each(|value| visit_expr(value, f)),
        Expr::Closure { body, .. } => visit_block(body, f),
        Expr::Unary { expr, .. } => visit_expr(expr, f),
        Expr::Binary { lhs, rhs, .. } => {
            visit_expr(lhs, f);
            visit_expr(rhs, f);
        }
        Expr::Try(inner) | Expr::Throw(inner) => visit_expr(inner, f),
        Expr::Return(value) => {
            if let Some(value) = &**value {
                visit_expr(value, f)
            }
        }
        Expr::Break | Expr::Continue => {}
        Expr::Index { base, index } => {
            visit_expr(base, f);
            visit_expr(index, f);
        }
        Expr::Cast { expr, .. } => visit_expr(expr, f),
        Expr::Coalesce { value, fallback } => {
            visit_expr(value, f);
            visit_expr(fallback, f);
        }
        Expr::TryCatch { expr, handler } => {
            visit_expr(expr, f);
            visit_block(handler, f);
        }
        Expr::Spawn { body, .. } => visit_expr(body, f),
        _ => {}
    }
}

/// Split an interpolated string into a Rust format string and its holes.
///
/// `"{a}={s.mean()}"` becomes `("{}={}", ["a", "s.mean()"])`. Braces are
/// doubled to be literal, as in every format string; the holes themselves are
/// Nikaia expressions and are parsed as such by the caller.
/// Every expression a literal holds - the holes of an interpolated string, and
/// the holes of a template.
///
/// **The emitter is not the only thing that needs these.** A hole is Nikaia
/// source, and until this existed it was source no analysis could see: the
/// splitting happened here, on the way out, so the type checker, the `sync`
/// check and the provenance analysis all walked past a `LitStr` as if it were
/// a string.
///
/// That was not only a missed diagnostic. `sync` is *inferred* from what a body
/// calls (ADR-288), so a function whose only pausing call sat inside a hole was
/// recorded `sync = "inferred"` - a claim, in a file a library ships, that it
/// cannot pause. ADR-288 D2 and ADR-010 D1 both say the same thing about that
/// direction: an analysis that fails open is a vulnerability generator.
///
/// A hole that does not parse yields nothing here. The emitter reports it, in
/// its own words, at the place it happens; an analysis has nothing to add.
/// The same as [`literal_expressions`], with the names a template's `<for>`
/// binds around each hole. Empty for anything that is not a template.
pub(crate) fn literal_expressions_bound(parsed: &Parsed, expr: &Expr) -> Vec<(Expr, Vec<String>)> {
    match expr {
        Expr::Dsl {
            target, content, ..
        } if crate::dsl::is_deferred(parsed.text(*target), content) => Vec::new(),
        Expr::Dsl { content, .. } => match template::split(content.trim()) {
            Ok(segments) => template_holes_bound(&segments)
                .into_iter()
                .filter_map(|(hole, bound)| parsed.hole(&hole).ok().map(|expr| (expr, bound)))
                .collect(),
            Err(_) => Vec::new(),
        },
        _ => literal_expressions(parsed, expr)
            .into_iter()
            .map(|hole| (hole, Vec::new()))
            .collect(),
    }
}

/// **One hole of an `f"…"` as its readers read it**: the node in the tree,
/// or a copy with what the tier pass hands over inside it (ADR-229 D1). The
/// node itself wherever nothing is wrapped, so that what the checker records
/// against a node's address is found by the emitter at the same address.
pub(crate) fn hole_as_read<'a>(
    parsed: &Parsed,
    at: u32,
    hole: &'a Expr,
) -> std::borrow::Cow<'a, Expr> {
    match parsed.hole_wraps.get(&at) {
        Some(wraps) => {
            let mut wrapped = hole.clone();
            crate::text_tiers::wrap_hole(&mut wrapped, wraps);
            std::borrow::Cow::Owned(wrapped)
        }
        None => std::borrow::Cow::Borrowed(hole),
    }
}

/// **An `f"…"`'s holes as the grammar parsed them**
/// ([ADR-309](../../docs/specification/adr/adr-309.md) D6), each beside the
/// byte its `{` stands at - the key the tier pass's wraps are recorded by
/// ([ADR-229](../../docs/specification/adr/adr-229.md) D1).
pub(crate) fn interpolated_holes(parts: &[crate::ast::FPart]) -> Vec<(u32, &Expr)> {
    parts
        .iter()
        .filter_map(|part| match part {
            crate::ast::FPart::Hole { expr, at, .. } => Some((*at, expr)),
            crate::ast::FPart::Text(_) => None,
        })
        .collect()
}

/// **An `f"…"` as the format string the language below reads**
/// ([ADR-309](../../docs/specification/adr/adr-309.md) D9): each run of text
/// as written - an escape and a doubled brace mean there what they mean here
/// - and each hole as `{}`, or `{:spec}` where a format follows its `:`.
pub(crate) fn format_of(parts: &[crate::ast::FPart]) -> String {
    let mut format = String::new();
    for part in parts {
        match part {
            crate::ast::FPart::Text(text) => format.push_str(text),
            crate::ast::FPart::Hole {
                spec: Some(spec), ..
            } => {
                format.push_str("{:");
                format.push_str(spec);
                format.push('}');
            }
            crate::ast::FPart::Hole { spec: None, .. } => format.push_str("{}"),
        }
    }
    format
}

pub(crate) fn literal_expressions(parsed: &Parsed, expr: &Expr) -> Vec<Expr> {
    match expr {
        // **The grammar parsed them** ([ADR-309](../../docs/specification/adr/adr-309.md)
        // D2): the holes are the literal's parts, read and not parsed again.
        // Each with what the tier pass hands over inside it (ADR-229 D1),
        // which is keyed by the hole's text - the hole at the same place among
        // the text's holes, so nothing is parsed to find it.
        Expr::LitInterpolated { parts } => interpolated_holes(parts)
            .into_iter()
            .map(|(at, hole)| {
                let mut expr = hole.clone();
                if let Some(wraps) = parsed.hole_wraps.get(&at) {
                    crate::text_tiers::wrap_hole(&mut expr, wraps);
                }
                expr
            })
            .collect(),
        // A template's holes are Nikaia too (ADR-017), and reach the emitter by
        // the same route: text, split on the way out. A deferred-parameter
        // statement has none: its braces are the foreign syntax's, and reading
        // them as holes would be this compiler speaking for a grammar it does
        // not have (ADR-296 D5).
        Expr::Dsl {
            target, content, ..
        } if crate::dsl::is_deferred(parsed.text(*target), content) => Vec::new(),
        Expr::Dsl { content, .. } => match template::split(content.trim()) {
            Ok(segments) => template_holes(&segments)
                .iter()
                .filter_map(|hole| parsed.hole(hole).ok())
                .collect(),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Every hole in a template, including the ones inside a `<for>` body - a hole
/// does not become invisible by being repeated, any more than it becomes safe
/// by it (ADR-017 D3).
fn template_holes(segments: &[template::Segment]) -> Vec<String> {
    template_holes_bound(segments)
        .into_iter()
        .map(|(hole, _)| hole)
        .collect()
}

/// The same, with **what a `<for>` binds around each hole**.
///
/// `<for r in :rows>{r.name}</for>`: `r` is declared by the template and named
/// in the hole, so a reader of a hole that does not know that sees a name
/// nothing declares. Which is what a checker asking *"does anything declare
/// this?"* needs, and nothing else does - the emitter only needs the text
/// ([ADR-017](../../../docs/specification/adr/adr-017.md)).
///
/// Innermost last, so a nested `<for>` shadowing the same word behaves the way
/// a nested block does.
pub(crate) fn template_holes_bound(segments: &[template::Segment]) -> Vec<(String, Vec<String>)> {
    let mut holes = Vec::new();
    walk_template_holes(segments, &mut Vec::new(), &mut holes);
    holes
}

fn walk_template_holes(
    segments: &[template::Segment],
    bound: &mut Vec<String>,
    out: &mut Vec<(String, Vec<String>)>,
) {
    for segment in segments {
        match segment {
            template::Segment::Text(_) => {}
            template::Segment::Hole { expr, .. } => out.push((expr.clone(), bound.clone())),
            template::Segment::For {
                binding,
                collection,
                body,
            } => {
                // The collection is captured from the enclosing scope with `:`
                // (ADR-296 D4), and it is a name this program wrote - so it is
                // read *outside* the binding, which is where it lives.
                out.push((collection.clone(), bound.clone()));
                bound.push(binding.clone());
                walk_template_holes(body, bound, out);
                bound.pop();
            }
        }
    }
}

/// A plain string on its way into a Rust *format* string.
///
/// Nikaia's plain string is inert - `print("{")` prints a brace (ADR-309 D1) -
/// and Rust's `println!` would read that brace as a hole of its own. Doubling
/// is what says "this one is text" to the macro, and it is the only place the
/// two languages disagree about a string literal.
/// An escape is copied whole, `\u{0041}` included: Rust's lexer turns that into
/// a character before the macro ever sees a brace, so doubling the braces
/// inside one would hand `println!` a `\u` with nothing after it. The same rule
/// the grammar's `F_TEXT_CHAR` follows for an `f"…"`, for the same reason.
fn rust_format_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('\\');
                let Some(escape) = chars.next() else { break };
                out.push(escape);
                if escape == 'u' && chars.peek() == Some(&'{') {
                    for c in chars.by_ref() {
                        out.push(c);
                        if c == '}' {
                            break;
                        }
                    }
                }
            }
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            _ => out.push(c),
        }
    }
    out
}

/// **A refusal with no place of its own, given the statement's**
/// ([ADR-171](../../docs/specification/adr/adr-171.md) D1).
///
/// A helper that works on text — a string literal's holes, say — knows what is
/// wrong and not where, because it never saw a file. The caller is emitting a
/// statement and knows exactly where. This is that handover, written once
/// rather than at each call.
fn at_the_statement<T>(flow: Flow<'_>, result: Result<T>) -> Result<T> {
    result.map_err(
        |error| match crate::diagnostics::refusal_at(&error).is_none() {
            true => crate::diagnostics::refuse_at(flow.statement, format!("{error}")),
            false => error,
        },
    )
}
// ---------------------------------------------------------------------------
// The tether: where a buffer lives, written out
// ([ADR-283](../../docs/specification/adr/adr-283.md))
// ---------------------------------------------------------------------------

/// The name of the keep a function was given.
const KEEP_PARAM: &str = "__keep";
/// A held struct's element, read through its handle (ADR-283 D17): appended to
/// what walks such a keeper.
const HELD_READ: &str = ".map(|held| held.get())";
/// The keep declared first in a function's body (D2).
const KEEP_FRAME: &str = "__keep_frame";
/// The keep a function's tasks share (D3).
const KEEP_TASK: &str = "__keep_task";

impl Emitter<'_> {
    /// The keep plan of the function being written.
    fn keep_plan(&self, function: &str) -> Option<&crate::contracts::keep::Plan> {
        match self.keep_plans.get(function) {
            Some(plan) => Some(plan),
            None => self.keep_plans.get(self.keep_function.borrow().as_str()),
        }
    }

    /// How a keep is reached where a buffer is put into it or a call is given
    /// it.
    fn keep_expr(keep: crate::contracts::keep::KeepAt, lent: bool) -> String {
        use crate::contracts::keep::KeepAt;
        match keep {
            KeepAt::Param => KEEP_PARAM.to_string(),
            KeepAt::Frame => match lent {
                true => format!("&{KEEP_FRAME}"),
                false => KEEP_FRAME.to_string(),
            },
            KeepAt::Local(at) | KeepAt::Element(at) => match lent {
                true => format!("&__keep_{at}"),
                false => format!("__keep_{at}"),
            },
            // **The task's keep, borrowed here** (ADR-218 D4): what is cut
            // from it lives as long as this function's handle, and what leaves
            // with a task is found in it again by its `Holding`.
            KeepAt::Task => format!("(&*{KEEP_TASK})"),
        }
    }

    /// The keeps declared before one statement: the function's own, before
    /// its first statement, and one per call or buffer that needs a keep of
    /// its own.
    fn keep_prelude(&self, function: &str, at: usize) -> Option<String> {
        use crate::contracts::keep::KeepAt;
        let plan = self.keep_plan(function)?;
        let mut lines = Vec::new();
        if plan.first == Some(at) {
            if plan.frame_keep {
                lines.push(format!(
                    "let {KEEP_FRAME} = nikaia_std::tether::Keep::new();"
                ));
            }
            if plan.task_keep {
                lines.push(format!(
                    "let {KEEP_TASK} = std::sync::Arc::new(nikaia_std::tether::Keep::new());"
                ));
            }
        }
        if plan.local_keeps.contains(&at) {
            lines.push(format!(
                "let __keep_{at} = nikaia_std::tether::Keep::new();"
            ));
        }
        let element = plan
            .puts
            .values()
            .chain(plan.calls.values())
            .any(|keep| *keep == KeepAt::Element(at));
        if element {
            lines.push(format!(
                "let __keep_{at} = std::sync::Arc::new(nikaia_std::tether::Keep::new());"
            ));
        }
        (!lines.is_empty()).then(|| lines.join(" "))
    }

    /// Declare the keeps a statement needs, once.
    fn write_keep_prelude(&self, out: &mut Out, function: &str, at: usize, depth: usize) {
        if self.preluded.borrow().contains(&at) {
            return;
        }
        if let Some(prelude) = self.keep_prelude(function, at) {
            self.preluded.borrow_mut().insert(at);
            out.push(&prelude);
            out.push("\n");
            out.push(&"    ".repeat(depth));
        }
    }

    /// The keep a call to `callee` is given in this statement, where the
    /// callee takes one.
    fn keep_argument(&self, flow: Flow<'_>, callee: &str) -> Option<String> {
        let plan = self.keep_plan(flow.function)?;
        let keep = plan
            .calls
            .get(&(flow.statement, callee.to_string()))
            .copied()?;
        Some(Self::keep_expr(keep, true))
    }

    /// The ledger key a written callee resolves to, the way the keep plan
    /// resolved it.
    fn keeping_callee(&self, func: &Expr) -> Option<String> {
        let name = crate::contracts::keep::callee_name(self.parsed, func)?;
        let (key, contract) = self
            .own_contracts
            .lookup(&name)
            .or_else(|| self.library.lookup(&name))?;
        crate::contracts::keep::takes_a_keep(contract).then_some(key)
    }

    /// The lifetimes a function that takes a keep writes its tethered
    /// positions with, where it takes one.
    fn kept_lifetimes(&self, key: &str, lifetimes: Lifetimes) -> Option<Lifetimes> {
        let contract = self.own_contracts.functions.get(key)?;
        if !crate::contracts::keep::takes_a_keep(contract) {
            return None;
        }
        Some(match lifetimes.params {
            "'a" => Lifetimes::KEPT_BY_THE_SUBJECT,
            _ => Lifetimes::KEPT,
        })
    }

    /// Whether this position of a function is one its views leave through.
    fn tethered_position(&self, key: &str, position: &str) -> bool {
        self.own_contracts.functions.get(key).is_some_and(|c| {
            c.views.iter().any(|h| {
                h.position == position && h.state == crate::contracts::tether::State::Tethered
            })
        })
    }

    /// The type a task's tethered binding holds, as the source wrote it: the
    /// `let`'s annotation, or the declared result of the function it calls.
    fn tethered_type(&self, ty: Option<&Type>, value: &Expr) -> Option<Type> {
        if let Some(ty) = ty {
            return Some(ty.clone());
        }
        let call = match value {
            Expr::Try(inner) => inner.as_ref(),
            other => other,
        };
        let Expr::Call { func, .. } = call else {
            return None;
        };
        let Expr::Variable(name) = func.as_ref() else {
            return None;
        };
        let wanted = self.text(*name);
        self.parsed
            .program
            .items
            .iter()
            .find_map(|item| match &item.node {
                Item::Fn {
                    name: Some(n),
                    ret_type: Some(ret),
                    ..
                } if self.text(*n) == wanted => Some(ret.clone()),
                _ => None,
            })
    }

    /// The `Views` type one tethered binding is held by (D3, and
    /// [ADR-218](../../docs/specification/adr/adr-218.md) D4), declared once
    /// per type: the value over any lifetime, and `shorten`, which `rustc`
    /// only accepts where the type is covariant - so the shortening is checked
    /// rather than trusted. The binding is a `Holding` over the task's keep,
    /// and nothing about it is `unsafe`.
    fn tether_wrapper(&self, ty: &Type) -> String {
        let any = self.ty(ty, Lifetimes::ANY);
        let long = self.ty(ty, Lifetimes::LONG);
        let shortened = self.ty(ty, Lifetimes::SHORTENED);
        let mut wrappers = self.wrappers.borrow_mut();
        let position = wrappers
            .iter()
            .position(|w| w.contains(&format!("type Of<'a> = {any};")));
        let n = match position {
            Some(n) => n,
            None => {
                let n = wrappers.len();
                wrappers.push(format!(
                    "/// A value that carries the keep its views point into \
                     (ADR-283 D11), as a `Holding` over it.\n\
                     #[allow(non_camel_case_types)]\n\
                     enum __Tethered{n} {{}}\n\n\
                     impl nikaia_std::tether::Views for __Tethered{n} {{\n    \
                     type Of<'a> = {any};\n    \
                     fn shorten<'long: 's, 's>(x: &'s {long}) -> &'s {shortened} {{\n        x\n    }}\n}}\n"
                ));
                n
            }
        };
        format!("__Tethered{n}")
    }

    /// The handle types, at the end of the unit.
    fn tether_wrappers(&self, out: &mut Out) {
        for wrapper in self.wrappers.borrow().iter() {
            out.push("\n");
            out.push(wrapper);
        }
        self.held_structs(out);
    }

    /// `kept[i]`, where `kept` holds structs of views one handle per buffer:
    /// the keeper and the index, for a write through the handle (ADR-283 D17).
    fn held_element<'e>(&self, flow: Flow<'_>, element: &'e Expr) -> Option<(&'e Expr, &'e Expr)> {
        let Expr::Index { base, index } = element else {
            return None;
        };
        self.reads_through_handle(flow, base)
            .then_some((base.as_ref(), index.as_ref()))
    }

    /// Whether the field of a held struct being written holds a view, which
    /// is then found again in the element's keeps before it is stored.
    fn held_field_carries_view(&self, flow: Flow<'_>, keeper: &Expr, field: Symbol) -> bool {
        let Expr::Variable(keeper) = keeper else {
            return true;
        };
        let Some(name) = self
            .keep_plan(flow.function)
            .and_then(|p| p.keeper_structs.get(self.text(*keeper)))
        else {
            return true;
        };
        let wanted = self.text(field);
        self.parsed
            .program
            .items
            .iter()
            .find_map(|item| match &item.node {
                Item::Struct {
                    name: n, fields, ..
                } if self.text(*n) == name => fields
                    .iter()
                    .find(|f| self.text(f.name) == wanted)
                    .map(|f| holds_view(&f.ty) || names_borrowing(&f.ty, &self.borrowing)),
                _ => None,
            })
            .unwrap_or(true)
    }

    /// Whether `expr` names a keeper whose elements are structs of views held
    /// one handle per buffer, so that an element is read through the handle
    /// ([ADR-283](../../docs/specification/adr/adr-283.md) D17).
    fn reads_through_handle(&self, flow: Flow<'_>, expr: &Expr) -> bool {
        let Expr::Variable(name) = expr else {
            return false;
        };
        self.keep_plan(flow.function)
            .is_some_and(|p| p.struct_keepers.contains(self.text(*name)))
    }

    /// **What `tether` needs to hold a struct of views**, once per struct, at
    /// the end of a unit that holds any
    /// ([ADR-283](../../docs/specification/adr/adr-283.md) D14, D16): the struct
    /// over any lifetime (`Views`, whose `shorten` compiles only where the
    /// struct is covariant), each of its views found again in the keeps
    /// (`Rebase`, one call per field that holds a view), and the value put
    /// into a `Holding` (`Hold`). Written by the compiler, and none of it
    /// `unsafe`.
    fn held_structs(&self, out: &mut Out) {
        // Only a unit that holds anything one handle per buffer pays for it.
        if self
            .keep_plans
            .values()
            .all(|p| p.element_keepers.is_empty() && !p.task_keep)
        {
            return;
        }
        for item in &self.parsed.program.items {
            let Item::Struct {
                name,
                generics,
                fields,
                ..
            } = &item.node
            else {
                continue;
            };
            if !self.borrowing.contains(name) || !generics.is_empty() {
                continue;
            }
            let ty = self.name(*name);
            let rebased = fields
                .iter()
                .map(|field| {
                    let field_name = self.name(field.name);
                    match holds_view(&field.ty) || names_borrowing(&field.ty, &self.borrowing) {
                        true => format!(
                            "{field_name}: nikaia_std::tether::Rebase::rebase(self.{field_name}, keeps)"
                        ),
                        false => format!("{field_name}: self.{field_name}"),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            out.push(&format!(
                "\n/// `{ty}` held one handle per buffer (ADR-283).\n\
                 #[allow(non_camel_case_types)]\n\
                 enum __Views_{ty} {{}}\n\n\
                 impl nikaia_std::tether::Views for __Views_{ty} {{\n    \
                 type Of<'a> = {ty}<'a>;\n    \
                 fn shorten<'long: 's, 's>(x: &'s {ty}<'long>) -> &'s {ty}<'s> {{\n        x\n    }}\n}}\n\n\
                 impl<'a> nikaia_std::tether::Rebase<'a> for {ty}<'_> {{\n    \
                 type At = {ty}<'a>;\n    \
                 fn rebase<const N: usize>(self, keeps: &nikaia_std::tether::Keeps<'a, N>) -> {ty}<'a> {{\n        \
                 {ty} {{ {rebased} }}\n    }}\n}}\n\n\
                 impl<const N: usize> nikaia_std::tether::Hold<N> for {ty}<'_> {{\n    \
                 type Held = nikaia_std::tether::Holding<__Views_{ty}, N>;\n    \
                 fn hold(self, keeps: [std::sync::Arc<nikaia_std::tether::Keep>; N]) -> Self::Held {{\n        \
                 nikaia_std::tether::Holding::new(keeps, move |keeps| nikaia_std::tether::Rebase::rebase(self, &keeps))\n    }}\n}}\n"
            ));
        }
    }
}

/// **How a pattern's names are written** (ADR-246 D5): bound, some of them by
/// reference, or as `_`, for a guard that only asks about a shape.
#[derive(Clone, Copy)]
enum Names<'a> {
    Bind(Option<&'a std::collections::BTreeSet<String>>),
    Wild,
}

/// Whether a pattern at a part binds it whole - a name, or `_` - rather than
/// asking it a question.
fn binds_a_part(part: &MatchPattern) -> bool {
    matches!(part, MatchPattern::Path(one) if one.len() == 1)
        || matches!(part, MatchPattern::Otherwise)
}

/// Whether `expr` reads the name `name` anywhere in it.
fn mentions_symbol(expr: &Expr, name: Symbol) -> bool {
    let mut found = false;
    visit_expr(expr, &mut |e| {
        if let Expr::Variable(v) = e {
            found |= *v == name;
        }
    });
    found
}

/// **The counters of a function body that are never negative**
/// ([`Emitter::nonnegative`]): a `let` of a non-negative literal whose name is
/// only ever given or added a non-negative literal, and a range's binding where
/// the range starts at one. A name declared twice, or handed to a call - which
/// might change it - is left out.
/// [`nonnegative_locals`], for [`crate::bounds`].
pub(crate) fn nonnegative_names(body: &Block) -> HashSet<Symbol> {
    nonnegative_locals(body)
}

fn nonnegative_locals(body: &Block) -> HashSet<Symbol> {
    let mut statements: Vec<&Stmt> = Vec::new();
    let mut seen: HashSet<*const Stmt> = HashSet::new();
    every_statement(body, &mut statements, &mut seen);
    let literal = |e: &Expr| {
        matches!(
            e,
            Expr::LitInt {
                negative: false,
                ..
            }
        )
    };
    let mut declared: HashMap<Symbol, usize> = HashMap::new();
    let mut candidates: HashSet<Symbol> = HashSet::new();
    let mut spoiled: HashSet<Symbol> = HashSet::new();
    for stmt in &statements {
        match stmt {
            Stmt::Let {
                names, ty, value, ..
            } => {
                for name in names {
                    *declared.entry(*name).or_default() += 1;
                }
                if let [name] = names.as_slice()
                    && ty.is_none()
                    && literal(value)
                {
                    candidates.insert(*name);
                }
            }
            Stmt::For { bindings, iter, .. } => {
                for name in bindings {
                    *declared.entry(*name).or_default() += 1;
                }
                if let ([name], Expr::Range { start, .. }) = (bindings.as_slice(), iter)
                    && literal(start)
                {
                    candidates.insert(*name);
                }
            }
            Stmt::Assign {
                target: Expr::Variable(name),
                op,
                value,
            } => {
                let keeps = match op {
                    None | Some(BinaryOp::Add) | Some(BinaryOp::Mul) => literal(value),
                    _ => false,
                };
                if !keeps {
                    spoiled.insert(*name);
                }
            }
            _ => {}
        }
    }
    // A name handed to a call may be changed by it (`mut` parameters).
    visit_block(body, &mut |expr| {
        let args = match expr {
            Expr::Call { args, .. }
            | Expr::MethodCall { args, .. }
            | Expr::SafeMethod { args, .. } => args,
            _ => return,
        };
        for arg in args {
            if let Expr::Variable(name) = arg {
                spoiled.insert(*name);
            }
        }
    });
    candidates
        .into_iter()
        .filter(|name| declared.get(name) == Some(&1) && !spoiled.contains(name))
        .collect()
}

/// Every statement of `block` and of every block inside it, once each.
fn every_statement<'b>(block: &'b Block, out: &mut Vec<&'b Stmt>, seen: &mut HashSet<*const Stmt>) {
    for stmt in &block.stmts {
        if !seen.insert(&stmt.node as *const Stmt) {
            continue;
        }
        out.push(&stmt.node);
        if let Stmt::For { body, .. } | Stmt::While { body, .. } = &stmt.node {
            every_statement(body, out, seen);
        }
    }
    let mut inner: Vec<&'b Block> = Vec::new();
    collect_blocks(block, &mut inner);
    for b in inner {
        every_statement(b, out, seen);
    }
}

/// Every block that stands inside the expressions of `block`'s statements, at
/// any depth: an `if`'s branches, a closure's body - one handed to a call too -
/// a block, an `unsafe` and an `overlap`.
fn collect_blocks<'b>(block: &'b Block, out: &mut Vec<&'b Block>) {
    visit_block(block, &mut |expr| match expr {
        Expr::Block(b) | Expr::Unsafe(b) | Expr::Overlap(b) => out.push(b),
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => {
            out.push(then_branch);
            if let Some(b) = else_branch {
                out.push(b);
            }
        }
        Expr::Closure { body, .. } => out.push(body),
        _ => {}
    });
    for stmt in &block.stmts {
        if let Stmt::For { body, .. } | Stmt::While { body, .. } = &stmt.node {
            out.push(body);
        }
    }
}
