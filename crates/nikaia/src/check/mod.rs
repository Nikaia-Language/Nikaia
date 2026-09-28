// crates/nikaia/src/check/mod.rs
//
// The type checker.
//
// It answers one question per construct - "are these two types the same?" - and
// it answers it only where both sides are written down. That is the whole
// design, and `contracts::ty::Ty::Unknown` is what makes it honest: Stage 0 has
// no signatures for the Rust half of `std`, and a checker that guessed at
// `push_str`, `entry` or `chars` would report errors that are not there. So
// every rule below has the same shape - infer both sides, and report only when
// **both are known and they disagree**.
//
// What it therefore promises, exactly:
//
//   * it never rejects a program that is correct;
//   * what it catches grows as the ledger grows, without this file changing.
//
// The second is the point of building it on the ledger (ADR-020) rather than
// beside it. A program's own functions are checked because `Ledger::infer` read
// their signatures out of the source; `std`'s are checked because
// `std.contracts` writes them down; a package's will be because a package ships
// its ledger (Part III, 13.5). One mechanism, three sources.
//
// Spans are the enclosing statement's, as they are for the `sync` check:
// expression-level spans are open work in the parser, and a caret on the right
// line is worth more than none at all.

use std::collections::{BTreeMap, BTreeSet};

use crate::assets::{ASSET, Denied, Reads};
use crate::ast::{self, BinaryOp, Block, Expr, Item, MatchPattern, Span, Stmt, UnaryOp};
use crate::build_time;
use crate::contracts::{FieldContract, FnContract, Ledger, send, ty, ty::Ty};
use crate::fold::Constant;
use crate::parser::Parsed;
use crate::types::SHAPE_BOUNDS;
use winnow_grammar::Symbol as Ident;

/// The types Part I 2.2 offers, which is what an `as` may name
/// ([ADR-054](../../../docs/specification/adr/adr-054.md) D1).
///
/// Wider than `record_cast`'s `NUMERIC`, and the two answer different questions:
/// that one is which conversions are **checked** at run time
/// ([ADR-043](../../../docs/specification/adr/adr-043.md) D4), this one is which
/// types a program may **write**. `bool`, `char`, `String` and `&str` are types
/// of this language and no conversion between them narrows anything.
const OFFERED: [&str; 8] = ["i32", "i64", "u8", "f64", "bool", "char", "String", "str"];

/// The note every `NK25xx` carries, because it is the reason the code exists.
///
/// ADR-005 §1 Group B and ADR-037 §3: the verdict may not depend on
/// `user_parallelism`, or a library written at one setting would turn out
/// un-compilable at the other - which is the failure the check was decided to
/// prevent rather than a property of it.
const SAME_AT_BOTH: &str = "a value may cross a thread only if it may cross any thread, \
     so the answer is the same at both settings of `user_parallelism` and a library built at \
     one stays usable at the other (Part III, C.3)";

/// Whether a finding stops the build.
///
/// Everything the checker says is an **error** but one, and that one is a
/// migration: ADR-035 gave the interpolated string an `f`, and a string written
/// before it looks exactly like one that meant its braces. A warning is what
/// that deserves - it cannot be an error, because `"{ margin: 0 }"` is correct
/// CSS and rejecting it would break the property this checker is built on (it
/// never refuses a program that is right); and it cannot be silence, because a
/// silent change of meaning is what Part III C.1 calls a compiler bug.
///
/// **It is temporary on purpose.** The variant exists for one release, with the
/// one code that uses it, and goes when the corpus has moved (ADR-035 D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    /// The build stops.
    #[default]
    Error,
    /// The build goes on and the programmer is told.
    Warning,
}

/// One thing the checker is sure about.
#[derive(Debug, Clone)]
pub struct Finding {
    /// Whether it stops the build.
    pub severity: Severity,
    /// The statement it is in.
    pub span: Span,
    /// Its `NK` code, from the catalogue in Part III, C.3. Mostly `NK1xxx`,
    /// which is types; `NK2501`/`NK2502` are a value on the wrong thread,
    /// `NK2605` a written call that can fail in a function that does not say
    /// so, and `NK2701` the same for a loop's step.
    pub code: &'static str,
    /// The headline, which says what is wrong and never how to think about it.
    pub message: String,
    /// Why the compiler believes it - the two types, and where each came from.
    pub notes: Vec<String>,
    /// One concrete way out. Part III C.2 requires it of every diagnostic.
    pub help: Option<String>,
}

/// Where one function's method calls went (ADR-028).
///
/// A method call is the one shape neither `sync.rs` analysis can resolve on its
/// own: `stats.add(5)` names `add` and says nothing about what `stats` is, and
/// only a type checker knows. This is that answer, handed over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MethodCalls {
    /// The ledger keys its method calls resolved to.
    pub resolved: BTreeSet<String>,
    /// It calls a method whose receiver type is not known, or one no ledger
    /// has an entry for. **Not the same as calling nothing** - it is the
    /// absence of an answer, and an analysis that claims a property must treat
    /// it as such (ADR-027 D2).
    pub unresolved: bool,
    /// How many such calls, which the boolean above cannot say.
    ///
    /// Nothing in the compiler reads it: every analysis wants *is there one*,
    /// and a count would be a number to be wrong about. It is here for the
    /// **measurement** [ADR-105](../../docs/specification/adr/adr-105.md) §1
    /// makes a claim with — *35 unanswered method calls in the corpus, every one
    /// downstream of a `?` that is a sequence* — so that the claim can be
    /// checked again rather than remembered. `crates/nikaia/tests/sequences.rs`
    /// is what checks it.
    pub unanswered: usize,
    /// It calls a **kept function value** through a field whose type may
    /// pause, or may fail: the answer is the type's, not a callee's.
    pub code_pauses: bool,
    pub code_fails: bool,
    /// The function fields it calls through, `Type.field`, outside a `spawn`
    /// ([ADR-230](../../docs/specification/adr/adr-230.md) D1): what the code
    /// stored there does is reached from here.
    pub fields_called: BTreeSet<String>,
    /// Which of `resolved` were reached from inside a **`spawn`** body, and
    /// whether any unresolvable call was
    /// ([ADR-039](../../docs/specification/adr/adr-039.md) D3).
    ///
    /// A task started with `spawn` runs **later and elsewhere**, so what it
    /// does is not what the surrounding function does — where a trailing
    /// lambda's body *is*, because it runs during the call
    /// ([ADR-029](../../docs/specification/adr/adr-029.md) D4). An analysis
    /// that asks about the calling function's own reach subtracts this; one
    /// that asks what the whole text mentions does not, which is why it is a
    /// second set rather than a narrowing of the first.
    pub in_a_task: CallsInATask,
}

/// The half of [`MethodCalls`] that is inside a `spawn` body.
///
/// A type of its own rather than a second [`MethodCalls`], which would nest
/// forever — and a name that says which half it is, so that a reader of
/// `calls.in_a_task.resolved` does not have to hold the nesting in their head.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallsInATask {
    /// The ledger keys resolved inside a task's body.
    pub resolved: BTreeSet<String>,
    /// A call inside a task's body that could not be resolved.
    pub unresolved: bool,
}

/// **What code stored in a function field reaches**
/// ([ADR-230](../../docs/specification/adr/adr-230.md) D1): the methods a
/// lambda put there resolves to, whether one of its calls did not resolve, and
/// the free calls it makes - one entry per `Type.field`, over everything the
/// package stores there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredCode {
    pub resolved: BTreeSet<String>,
    pub unresolved: bool,
    pub free: BTreeSet<String>,
}

/// **One `assert`** ([ADR-245](../../docs/specification/adr/adr-245.md) D2):
/// what its failure says, which only the compiler knows - the claim as it was
/// written, and the operands of its comparison by the names they were written
/// with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claim {
    /// `assert(blank == 1; message: "…")`, rewritten from the tree.
    pub written: String,
    /// The comparison's left operand, where its value can be shown: `blank`.
    pub left: Option<String>,
    /// The same for the right.
    pub right: Option<String>,
}

/// Whether an expression names a **place that outlives the statement**
/// ([ADR-191](../../docs/specification/adr/adr-191.md) D1).
///
/// Stricter than [`crate::emit::is_a_place`] on purpose, and the difference is
/// the whole of what makes a view safe here. That one asks whether an
/// expression *can be written to*, which a `?.` reach can; this one asks whose
/// storage the value lives in, and follows the chain down to its **root**. A
/// root that is a name is a binding the enclosing block owns; a root that is a
/// call is a temporary that dies at the `;`, and a view of one bound past that
/// is `rustc`'s *temporary value dropped while borrowed* about a file nobody
/// wrote ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
///
/// Measured rather than reasoned: `find(1)?.home?.city` passes
/// [`crate::emit::is_a_place`] at every link and roots in a call.
fn roots_in_a_binding(expr: &Expr) -> bool {
    match expr {
        Expr::Variable(_) => true,
        Expr::Field { base, .. } | Expr::SafeField { base, .. } | Expr::Index { base, .. } => {
            roots_in_a_binding(base)
        }
        // A `?` and a cast hand the same place on; anything else - a call, a
        // literal, a `catch`, an operator - makes a value of its own.
        Expr::Try(inner) | Expr::Cast { expr: inner, .. } => roots_in_a_binding(inner),
        _ => false,
    }
}

/// Which of the two spellings a member's view is taken with.
///
/// The one place the distinction is made, so that the checker's answer and the
/// emitter's two accessors cannot come apart.
fn viewed_as(ty: &Ty) -> Viewed {
    match ty {
        Ty::Named { name, args, .. }
            if crate::contracts::ty::base(name) == crate::contracts::ty::TEXT
                && args.is_empty() =>
        {
            Viewed::Text
        }
        _ => Viewed::Plain,
    }
}

/// **What one call was handed**, positional and named, with the types found for
/// each.
///
/// One parameter rather than four, because the two callers of
/// [`Checker::crosses_into_an_unseen_call`] pass the same four and the question
/// they differ on is *why* — which is the argument beside this one.
struct Arguments<'a> {
    args: &'a [Expr],
    found: &'a [Ty],
    config: &'a [ast::ConfigArg],
    passed: &'a [(String, Ty)],
}

/// How a `?.` takes a member out of the view it reaches through
/// ([ADR-191](../../docs/specification/adr/adr-191.md) D1).
///
/// The emitter has no types ([ADR-011](../../docs/specification/adr/adr-011.md)
/// D2), and the two views are spelled differently in the language below, so the
/// answer travels the way [ADR-028](../../docs/specification/adr/adr-028.md)
/// hands over every other answer this emitter has none of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Viewed {
    /// Text. A view of `String` is `ref String`, which is `&str` below
    /// ([ADR-184](../../docs/specification/adr/adr-184.md) D2), so the member
    /// is taken with `.as_str()` — or `.as_deref()` where the field is itself a
    /// `T?` and the reach flattens.
    Text,
    /// Everything else that does not copy: `&it.a`, or `it.a.as_ref()` where
    /// the reach flattens.
    Plain,
}

/// **What a `std` entry handed a lambda that pauses is lowered to**
/// ([ADR-233](../../docs/specification/adr/adr-233.md) D1, D2): its counterpart
/// in `nikaia_std::seq`, which takes the language below's `async` closure where
/// the entry takes a synchronous one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PausingEntry {
    /// `nikaia_std::seq::name(receiver, lambda)`.
    Function(&'static str),
    /// `nikaia_std::seq::name(&mut receiver, lambda)`: the entry changes its
    /// receiver in place.
    Lent(&'static str),
    /// `receiver.name(lambda)`: the receiver is already a sequence whose step
    /// pauses, and has the adapter as a method.
    Method(&'static str),
}

/// What one pass of the checker learned.
#[derive(Debug, Clone, Default)]
pub struct Checked {
    /// Every mistake it is sure about.
    pub findings: Vec<Finding>,
    /// What each `comptime` is written as below - its type and its **value** - by
    /// the byte its statement starts at
    /// ([ADR-073](../../docs/specification/adr/adr-073.md) D3, D4).
    ///
    /// The same arrangement as `fallible_methods` and for the same reason, said
    /// twice over. The **type** may be left out and Rust's `const` demands one,
    /// so somebody has to infer it, and the emitter has no types
    /// ([ADR-028](../../docs/specification/adr/adr-028.md)). The **value** is
    /// here for the sharper half of the same reason: folding `PAGE * 2` means
    /// knowing what `PAGE` is, which is a scope - and the emitter has none of
    /// those either, so a constant built out of another would reach the
    /// language below unfolded and D3's demand would be `rustc`'s to keep.
    ///
    /// Both are spelled in the language below, so the emitter writes the pair
    /// and decides nothing.
    pub comptime_values: BTreeMap<usize, (String, String)>,
    /// **The type each `with` copies**, by the byte the word stands at
    /// ([ADR-118](../../docs/specification/adr/adr-118.md) D1).
    ///
    /// Rust's functional update writes the struct's name — `Point { x: 1, ..p }`
    /// — and the operand's type is this walk's answer rather than the parser's.
    /// So the emitter is **told**, the way it is told a `comptime`'s value:
    /// [ADR-011](../../docs/specification/adr/adr-011.md) D2 keeps it a walk
    /// that knows no types, and a second inference living in it would be the
    /// two halves free to disagree about one program.
    pub with_types: BTreeMap<usize, String>,
    /// **What a `T::fields` loop was unrolled over**
    /// ([ADR-181](../../docs/specification/adr/adr-181.md) D1), keyed by the
    /// function's name and the type argument a call gave it.
    ///
    /// The fields in **declaration order**, each with the type it has on that
    /// type - which is what the per-unrolling check reads and what the emitter
    /// writes the field reads from. One entry per (function, type) actually
    /// used, which is [ADR-088](../../docs/specification/adr/adr-088.md) D5's
    /// *fields × instantiations* counted honestly: a function nobody calls is
    /// not unrolled at all.
    pub unrolled: BTreeMap<(String, String), Vec<FieldContract>>,
    /// **The functions whose body walks a type's fields**, and which parameter
    /// each walks ([ADR-181](../../docs/specification/adr/adr-181.md) D2).
    ///
    /// Beside `unrolled` rather than derived from it, because the two differ in
    /// the case that matters: a function that walks a shape and is **never
    /// called** has no instantiation and still may not be emitted, since its
    /// body holds a loop over a shape and that has no form in the language
    /// below. Part II 10.3's own block is exactly that shape — a `describe`
    /// with no call under it — and without this it reached `rustc`, which is
    /// [Part III C.1](../../docs/specification/30-nikaia-tooling.md)'s class.
    pub walks_fields: BTreeMap<String, String>,
    /// The call sites that go to a specialised copy, by the byte the call
    /// starts at and the name to write there.
    ///
    /// **Keyed by the byte** for the reason `comptime_values` is: the emitter
    /// has no types ([ADR-028](../../docs/specification/adr/adr-028.md)) and
    /// cannot work out which copy `describe(u)` means.
    pub unrolled_calls: BTreeMap<usize, String>,
    /// The `for` statements whose **step can fail** (ADR-025 D1), by the byte
    /// the statement starts at.
    ///
    /// The emitter reads this. It is here rather than in the emitter because
    /// answering it means inferring the type of the iterator expression, which
    /// is what this module does - and because `let stream = io::lines()`
    /// followed by `for line in stream` has to be the same as the one-line
    /// form, which matching on a name would not give (ADR-025 D7).
    pub fallible_loops: BTreeSet<usize>,
    /// The method calls that walk a sequence whose step **pauses**, which have
    /// no form to be written as ([ADR-172](../../docs/specification/adr/adr-172.md)
    /// D5), by the byte the statement starts at and the method's name.
    ///
    /// The eager walks pause with the step (ADR-172 D5) and the lazy ones are
    /// adapters of their own ([ADR-233](../../docs/specification/adr/adr-233.md),
    /// [ADR-234](../../docs/specification/adr/adr-234.md)), so nothing lands
    /// here today: it is the net for an entry added without a pausing form,
    /// which the emitter refuses by name and line rather than letting `rustc`
    /// speak about a method the generated file's receiver does not have
    /// ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **And what it replaces was worse than a refusal.** `io::lines().count()`
    /// compiled before this, because `Lines` was an `Iterator` over
    /// `Result<String, …>` — so it counted the *failures* as lines, which is
    /// the bug Part I 6.4 refuses by name. The ledger said `-> i64` and the
    /// program got one; nothing anywhere said the number could be wrong.
    pub pausing_walks: BTreeSet<(usize, String)>,
    /// The `for`s whose step **pauses**, by the byte the statement starts at
    /// ([ADR-172](../../docs/specification/adr/adr-172.md) D1).
    ///
    /// The same arrangement as [`Checked::fallible_loops`] and for the same
    /// reason: which loop this is, is a question about the iterated
    /// expression's **type**, and the emitter has no types.
    pub pausing_loops: BTreeSet<usize>,
    /// The `for`s over a **name** that holds a sequence, by the byte the
    /// statement starts at ([ADR-212](../../docs/specification/adr/adr-212.md)
    /// D4).
    ///
    /// A `for` over a place lends it and the emitter writes `.iter()`; a
    /// sequence is walked by value and has no `.iter()`, so the name is handed
    /// over as it is. The emitter has no types, so which loops these are is
    /// answered here, as `pausing_loops` is.
    pub owned_loops: BTreeSet<usize>,
    /// The arguments that are a **count the language below takes in `usize`**,
    /// by the byte the statement starts at, the method as written and the
    /// position ([ADR-212](../../docs/specification/adr/adr-212.md) D5).
    pub count_args: BTreeSet<(usize, String, usize)>,
    /// How each key goes into the brackets of a map whose keys are owned, by
    /// the byte the statement starts at and the key's shape
    /// ([ADR-213](../../docs/specification/adr/adr-213.md) D1).
    pub map_keys: BTreeMap<(usize, String), KeyForm>,
    /// Indexes that are a **range kept in a name**, by the byte the statement
    /// starts at and the index's shape: a slice, like a range written in the
    /// brackets ([ADR-215](../../docs/specification/adr/adr-215.md) D3).
    pub slice_indices: BTreeSet<(usize, String)>,
    /// `clone` calls that went to a `std` entry, by statement and receiver
    /// shape: written `to_owned` below, which is a copy whether the receiver
    /// is a value or a view of one (ADR-215 D4).
    pub owned_copies: BTreeSet<(usize, String)>,
    /// `.to_string()` on **text**, by statement and receiver shape: the text
    /// form of text is the text itself, so the call is written as its receiver
    /// ([ADR-216](../../docs/specification/adr/adr-216.md) D4).
    pub text_as_is: BTreeSet<(usize, String)>,
    /// **`count()` calls that are `std`'s count of a sequence**, by statement
    /// and receiver shape: the language below counts in `usize`, and a count
    /// is an `i64` here, as a length is (Part I 2.2, ADR-048 D1).
    pub counted: BTreeSet<(usize, String)>,
    /// **The names a `match` arm binds as a view of its scrutinee**, by the
    /// arm's pattern: the scrutinee is a place this function owns, the part
    /// does not copy, and the arm does not keep it - so the name stays whole
    /// after the `match` (Part I 6.5, ADR-242).
    pub lent_bindings: BTreeMap<usize, BTreeSet<String>>,
    /// `collect()` calls whose target declares what they build - a map, a
    /// set, text - by statement and receiver shape
    /// ([ADR-227](../../docs/specification/adr/adr-227.md) D1): written
    /// without `::<Vec<_>>`, so the declared type is what the language below
    /// builds.
    pub collected_into: BTreeSet<(usize, String)>,
    /// Method calls that hand back a sequence of **views of values that copy**,
    /// numbers, truth values and characters, by statement and the call's shape
    /// ([ADR-231](../../docs/specification/adr/adr-231.md) D1): written with
    /// `.copied()`, so each item is the value.
    pub copied_walks: BTreeSet<(usize, String)>,
    /// `filter` lambdas whose item copies, by statement and the lambda's shape
    /// (ADR-231 D2): the language below hands the lambda a reference to the
    /// item, and the parameter is written `|&x|` so it is the item.
    pub filter_patterns: BTreeSet<(usize, String)>,
    /// **Lambdas put where a function value is kept** - a field, a result, an
    /// annotated `let`, an assignment, a parameter the callee keeps - by
    /// statement and shape, and whether the function type may pause. Written
    /// below as one shared closure, `nikaia_std::func::Kept`.
    pub kept_lambdas: BTreeMap<(usize, String), (bool, bool)>,
    /// The names a kept lambda captures that are **hulls** - a `Shared`, a
    /// `SharedMut`, a `Locked` - by the same key
    /// ([ADR-230](../../docs/specification/adr/adr-230.md) D4): the lambda
    /// takes its own handle, and the one it was written beside stays usable.
    pub kept_hulls: BTreeMap<(usize, String), Vec<String>>,
    /// **Calls of a field that holds a function**, `button.on_click(4)`, by
    /// statement and `receiver.field`, and whether the call may pause.
    pub field_calls: BTreeMap<(usize, String), bool>,
    /// **Calls of a kept function value by name**, `step(5)` where `step` is
    /// a `let`, by statement and name: whether it may pause, and fail.
    pub kept_calls: BTreeMap<(usize, String), (bool, bool)>,
    /// **A kept value handed to a parameter the callee only runs**, by
    /// statement and shape: lent to it as the closure it holds.
    pub kept_args: BTreeSet<(usize, String)>,
    /// **A named function put where a function value is kept**, by statement
    /// and name: whether the type may pause, may fail, and how many parameters
    /// it takes. Written below as a kept closure that calls it.
    pub kept_functions: BTreeMap<(usize, String), (bool, bool, usize)>,
    /// The **method calls that can fail**, as the byte the statement they
    /// stand in starts at and the method's name (ADR-023 D8).
    ///
    /// The same arrangement as `fallible_loops`, for the same reason and said
    /// once: a method call's callee is only known to something that knows the
    /// receiver's type, the emitter has no types (ADR-028), so the answer is
    /// computed here and handed over. The emitter writes the `?`.
    ///
    /// The statement and the name, because that is the most an `Expr` can be
    /// pointed at: expressions carry no spans and a method name is an interned
    /// symbol shared by every call that writes it. So a pair is recorded only
    /// where **every** method call of that name in that statement can fail -
    /// `a.read()` beside a `b.read()` that cannot is left out of the set
    /// entirely rather than resolved by position. That is the same
    /// fail-quietly this module's other answers keep: the set never claims a
    /// call fails that does not.
    pub fallible_methods: BTreeSet<(usize, String)>,
    /// The `set` calls that carry a **witness**
    /// ([ADR-111](../../docs/specification/adr/adr-111.md) D5), by the byte
    /// their statement starts at.
    ///
    /// `kasse.set(neu; after: stand)` is a different door from `set` — it
    /// compares, and it can fail — and only a type checker knows that this
    /// receiver is a lock, which is what `Locked::set(after)` in the ledger is
    /// keyed on. The emitter writes `set_after(neu, stand)` for a call in here
    /// and the ordinary `set` for one that is not, so a user-defined `set` with
    /// an option of its own is untouched.
    ///
    /// The statement and not the call, which is `fallible_methods`' shape and
    /// carries its limit: two `set`s in one statement, one witnessed and one
    /// not, is a program this cannot tell apart. The emitter asks for the
    /// written `after:` as well, so what that costs is bounded by the pair
    /// being written at all.
    /// The lambda arguments that lower to a closure returning a **boxed
    /// future** ([ADR-122](../../docs/specification/adr/adr-122.md) D1), by the
    /// byte their statement starts at and the argument's position.
    ///
    /// The parameter's **type** decides the shape — it may pause unless it says
    /// `sync` — and a type is what the emitter has none of
    /// ([ADR-028](../../docs/specification/adr/adr-028.md)), so the answer is
    /// computed where it is known and looked up where it is needed. The same
    /// arrangement `lent_args` and `nullable_args` have.
    pub future_lambdas: BTreeSet<(usize, usize)>,
    /// The lambda arguments whose parameter the callee **runs** rather than
    /// keeps, so the closure is an `async` one and not a boxed future
    /// ([ADR-192](../../docs/specification/adr/adr-192.md) D1).
    ///
    /// A subset of [`Checked::future_lambdas`], recorded beside it rather than
    /// instead of it: the two shapes differ only at a *run* parameter, and the
    /// emitter reads both keys at one position.
    ///
    /// **Run is the absence of `keeps`**, which is the column
    /// [ADR-102](../../docs/specification/adr/adr-102.md) D3 already points at:
    /// *the same analysis that decides whether a value is a view or kept, asked
    /// of a parameter that is code*. Nothing new is derived for it.
    pub run_lambdas: BTreeSet<(usize, usize)>,
    /// **The lambdas that pause, handed to a `std` entry**, by the byte the
    /// statement starts at and the lambda's shape, with what the call is
    /// lowered to ([ADR-233](../../docs/specification/adr/adr-233.md) D1, D2).
    /// The lambda is written as an `async` closure and the call as the
    /// entry's counterpart; the emitter has no types to tell which one.
    pub pausing_lambdas: BTreeMap<(usize, String), PausingEntry>,
    pub witnessed_sets: BTreeSet<usize>,
    /// The method calls that **pause**, keyed the same way and narrowed the same
    /// way ([ADR-055](../../docs/specification/adr/adr-055.md) D2).
    ///
    /// `throws` becomes a `?` and *can pause* becomes an `.await`, both read off
    /// the callee's contract - and for a method the emitter cannot read it,
    /// because `stats.add(5)` names `add` and only a type checker knows what it
    /// goes to (ADR-028). So this is the one-column-over twin of
    /// `fallible_methods`, and it keeps that set's fail-quietly: a name that in
    /// one statement is both a pausing call and a call that is not is in neither
    /// set.
    ///
    /// **Either ledger**, since §6 step 3 made `std`'s own pausing entries
    /// `async fn`. Before it this was narrowed to the program's own, because
    /// awaiting a `std` entry that blocked its thread would have been awaiting a
    /// value rather than a future.
    pub pausing_methods: BTreeSet<(usize, String)>,
    /// The statements where a **plain value stands in a nullable slot** and the
    /// emitter therefore writes the `Some(…)`, by the byte the statement starts
    /// at (Part I 2.3).
    ///
    /// `let mut m: &str? = null` then `m = "World"`: the second line hands a
    /// `&str` to an `Option<&str>`, and the language below needs the
    /// constructor written. The same arrangement as `shared_sites` and for the
    /// same reason - the emitter has no types (ADR-028), and whether the value
    /// beside the `=` is *already* nullable is the whole question.
    ///
    /// **Only where this checker is sure of both sides.** A value whose type is
    /// `Unknown` is left out, because wrapping a value that is already an
    /// `Option<T>` would make an `Option<Option<T>>` - so the set never claims
    /// a wrap that is not needed, which is the fail-closed direction
    /// ([ADR-010](../../../../docs/specification/adr/adr-010.md) D1).
    pub nullable_sites: BTreeMap<usize, Wrap>,
    /// The `+`s that join text, by the byte their **operator** starts at
    /// ([ADR-081](../../docs/specification/adr/adr-081.md) D2).
    ///
    /// Keyed by the operator and not by the statement, which is the whole reason
    /// `Expr::Binary` gained a span: `a + b + c` is two of them and the statement
    /// they stand in is one. Every other answer in this channel is keyed by a
    /// statement because every other answer is about one thing per statement.
    ///
    /// **Only text is in here.** A number's `+` stays exactly where it is,
    /// because arithmetic that moved into `std` would silently lose
    /// [ADR-043](../../docs/specification/adr/adr-043.md) D1's overflow abort -
    /// `overflow-checks` is on per Nikaia crate and off for the profile, and
    /// inlining does not carry the check across.
    pub concatenations: BTreeSet<usize>,
    /// The `?.` reaches whose field is **itself** nullable, as the byte the
    /// statement starts at and the field's name (Part I 3.5).
    ///
    /// `x?.a` over a plain `a` is `x.map(|v| v.a)`; over an `a` that is already
    /// a `T?` it has to be `and_then`, or the result holds a nullable of a
    /// nullable and `a?.b?.c` comes out wrong. Which of the two is a question
    /// about the declared type, so it is answered here and the emitter writes
    /// the word (ADR-028).
    ///
    /// The statement and the name, for the reason `fallible_methods` gives at
    /// length: an expression carries no span, so a pair is recorded only where
    /// **every** `?.` of that name in that statement flattens. A mixed
    /// statement is left out of the set entirely rather than resolved by
    /// position, which keeps `map` - the answer that cannot make a nested
    /// option out of a plain field - as the one it falls back to.
    pub flattened_reaches: BTreeSet<(usize, String)>,
    /// Part I 3.5: the `?.` reaches over a field this compiler knows to
    /// **copy**, as the byte the statement starts at and the field's name
    /// ([ADR-189](../../docs/specification/adr/adr-189.md) D1).
    ///
    /// What the emitter does with it is take the receiver by `as_ref()`, so the
    /// reach leaves the value where it was — which is
    /// [ADR-113](../../docs/specification/adr/adr-113.md) D1 for the half of D2
    /// that needs no representation: a number, a `bool` and a `char` come out
    /// of a view by being copied, and a member that would come out as a view
    /// needs the state that is not built.
    ///
    /// **Recorded only where this compiler knows**, which is why it is a set of
    /// the certain cases rather than the complement: a field whose type is
    /// `Unknown` is left where it was, and the reach lowers exactly as it did
    /// before this set existed.
    pub copied_reaches: BTreeSet<(usize, String)>,
    /// Part I 3.5: the `?.` reaches over a field that does **not** copy and
    /// whose receiver is a **place**, so the member comes out as a *view* of it
    /// ([ADR-191](../../docs/specification/adr/adr-191.md) D1,
    /// [ADR-113](../../docs/specification/adr/adr-113.md) D2).
    ///
    /// The value says how the view is taken, because the emitter has no types
    /// and the two spellings differ: `ref String` is `&str` below
    /// ([ADR-184](../../docs/specification/adr/adr-184.md) D2), and `ref T` is
    /// `&T`.
    ///
    /// **A receiver that is not a place is left out**, and that is D2's own
    /// line: the view would point into a temporary that dies at the `;`, and
    /// binding it is `rustc`'s *temporary value dropped while borrowed* about a
    /// file nobody wrote. A temporary has no next line to stay usable on, so
    /// leaving it owned keeps [ADR-113](../../docs/specification/adr/adr-113.md)
    /// D1's promise where it means anything.
    pub viewed_reaches: BTreeMap<(usize, String), Viewed>,
    /// Part I 3.5: the `?.` reaches over a **method** that changes nothing, as
    /// the byte the statement starts at and the method's name
    /// ([ADR-189](../../docs/specification/adr/adr-189.md) D2).
    ///
    /// The method half of the same sentence the field half writes: the reach
    /// takes its scrutinee by `as_ref()`, so the receiver is lent to the call
    /// and is usable afterwards. A **method** needs no representation for it at
    /// all — what comes out is the call's own result and not a view of the
    /// receiver — so this half of
    /// [ADR-113](../../docs/specification/adr/adr-113.md) D1 is whole.
    ///
    /// **Only where every candidate for the name says it changes nothing**, the
    /// rule `NK1138` already uses one construct over: a name this compiler
    /// cannot resolve is not claimed about, and the reach lowers as it did.
    pub lent_reaches: BTreeSet<(usize, String)>,
    /// A lent `?.` method reach whose result is a **view** and whose receiver
    /// is a temporary - rooted in a call - by statement and method name
    /// ([ADR-228](../../docs/specification/adr/adr-228.md) D1). The receiver is
    /// held in a binding of its own before the statement, so the view has
    /// something to point into past the `;`.
    pub held_reaches: BTreeSet<(usize, String)>,
    /// The **struct-literal fields** where a plain value stands in a nullable
    /// slot, as the byte the statement starts at and the field's name
    /// (Part I 2.3).
    ///
    /// `nullable_sites` covers the three positions a statement *is* - an
    /// annotated `let`, an assignment, a `return` - and a struct literal has
    /// one position per field, so it needs the name too.
    ///
    /// **And the type, and [`argument_shape`]**, for `nullable_args`' reason and
    /// found by the same probe: a statement may build two literals.
    /// `f(P { x: 1 }, P { x: null })` had one key for two fields and came out
    /// `P { x: Some(1) }, P { x: Some(None) }`; `f(P { x }, Q { x })` would have
    /// had one for two *structs*. The type is `unaliased` on both sides, so the
    /// two passes spell it the same.
    pub nullable_fields: BTreeMap<(usize, String, String), BTreeMap<String, Wrap>>,
    /// The **call arguments** where a plain value stands in a nullable
    /// parameter, as the byte the statement starts at, the callee as the source
    /// wrote it, and the argument's position (Part I 2.3).
    ///
    /// The third position D4's wrap needs a key for. An expression carries no
    /// span, so the key is built out of what both sides can see: the statement,
    /// the callee's **written** name - the emitter has only what the source
    /// says, and a method's key is `Type::method` while a constructor's is
    /// `Type::new`, neither of which stands at the call - the argument's
    /// position, and [`argument_shape`].
    ///
    /// **The shape is what says which *call***, and leaving it out was a
    /// defect rather than a simplification. The three outer parts name a
    /// *parameter*; a statement may call the same function twice, and then
    /// `let r = pick(1) + pick(null)` had one key for two arguments. The last
    /// one walked won, so one of them was emitted with the other's answer -
    /// `pick(Some(1)) + pick(Some(None))`, which `rustc` refuses about a file
    /// nobody wrote (Part III, C.1).
    pub nullable_args: BTreeMap<(usize, String, usize), BTreeMap<String, Wrap>>,
    /// The **narrowing conversions**, as the byte the statement they stand in
    /// starts at and the type converted to (ADR-043 D4).
    ///
    /// The fourth answer this module gives the emitter, and for the reason the
    /// other three have: `as i32` narrows or widens depending on what it is
    /// *given*, and the emitter has no types (ADR-028). The emitter writes the
    /// checked conversion.
    ///
    /// Narrow in the same way `fallible_methods` is narrow, and by the same
    /// limit: an `Expr` carries no span, so a pair is recorded only where
    /// **every** conversion to that type in that statement narrows. A
    /// `big as i32` beside a `small as i32` is left out entirely rather than
    /// resolved by position - a checked conversion on a widening one would be a
    /// `rustc` error about a file nobody wrote.
    pub narrowing_casts: BTreeMap<(usize, String), Narrowing>,
    /// The **handles a task's body uses**, by the byte the statement starts at
    /// and the name ([ADR-040](../../docs/specification/adr/adr-040.md) D1).
    ///
    /// A task takes what it names by value, and a handle handed on by value is
    /// **duplicated**. For a call the emitter reads that off the callee's
    /// signature; a task's body has no signature, so which names are handles is
    /// a question about types and is answered here - the same arrangement
    /// `fallible_methods` and the rest use.
    pub task_handles: BTreeSet<(usize, String)>,
    /// The **call arguments the compiler writes a `&` for**
    /// ([ADR-094](../../docs/specification/adr/adr-094.md) D1), keyed the way
    /// [`Checked::nullable_args`] is: the byte the statement starts at, the
    /// callee as the source wrote it, the argument's position, and
    /// [`argument_shape`].
    ///
    /// Whether an argument is lent or handed over is the **callee's** answer
    /// (`contracts::keeps::lends`), and the emitter reads it for the
    /// declaration too — one answer, two positions, because the two disagreeing
    /// is a `&&T` or a moved value in the language below. What the emitter
    /// cannot decide on its own is the other half: whether *this* argument is
    /// already a view, which is a question about its type (ADR-028).
    ///
    /// The four-part key is `nullable_args`' and is there for the same reason:
    /// a statement may call one function twice, and `f(a) + f(b)` has two
    /// arguments in one position.
    /// The **options a method call has**, in the order the declaration gives,
    /// by the byte the statement starts at and the method's written name
    /// (Part I 5.1, [ADR-133](../../docs/specification/adr/adr-133.md) D1).
    ///
    /// The language below has no named arguments and no defaults, so an option
    /// becomes positional at the call and a missing one becomes its default.
    /// For a call **by name** the emitter reads that off the callee's contract
    /// itself; for a **method** it cannot, because finding the entry means
    /// resolving the receiver and the emitter has no types
    /// ([ADR-028](../../docs/specification/adr/adr-028.md)) — the same
    /// arrangement `lent_args` and `nullable_args` have.
    ///
    /// Without it a method's options were **dropped**: `q.execute(target_age: 30)`
    /// came out as `q.execute()` and `rustc` answered about the arity of a file
    /// nobody wrote ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    /// Older than the spelling that makes it easy to hit, because the `;` form
    /// went down the same path.
    pub method_options: BTreeMap<(usize, String), Vec<(String, String)>>,
    pub lent_args: BTreeMap<(usize, String, usize), BTreeSet<String>>,
    /// **A lookup's key whose type this checker could not work out**
    /// (0.0.235), by statement, method as written and the argument's shape:
    /// the emitter writes `index::AsKey::as_key(&k)`, which is the reference
    /// the language below wants whether `k` turns out a number, owned text or
    /// a view ([`Checker::a_lookup_key_lent`]).
    pub lookup_keys: BTreeSet<(usize, String, String)>,
    /// **The members that go behind a `Box` below**
    /// ([ADR-246](../../docs/specification/adr/adr-246.md) D1), by type and
    /// member ([`boxed_member`]): the fields on a ring of types that hold each
    /// other inline.
    pub boxed_members: BTreeMap<String, BTreeSet<String>>,
    /// **The reads of a boxed field**, by statement and the read's shape: the
    /// emitter writes `(*base.field)`, the place that behaves as the unboxed
    /// field would ([ADR-246](../../docs/specification/adr/adr-246.md) D2).
    pub boxed_reads: BTreeSet<(usize, String)>,
    /// **A number an arm binds out of a value it was lent** (0.0.236), by the
    /// pattern's address and the name: the language below binds a `&i64`
    /// there, and the emitter copies the number out at the head of the arm, so
    /// `Expr::Num(n) => n` is the `i64` the pattern's type says.
    pub copied_bindings: BTreeSet<(usize, String)>,
    /// **A comparison's side that is a view of a copied value, where the other
    /// side is the value** (0.0.238), by statement and the side's shape: the
    /// language below compares a `char` with a `char` and not with a `&char`,
    /// so the emitter reads the view (`*c`).
    pub compared_views: BTreeSet<(usize, String)>,
    /// **Every `assert`, and what its failure shows**
    /// ([ADR-245](../../docs/specification/adr/adr-245.md) D2), by statement
    /// and the shape of its condition. Nothing proves a claim yet, so every
    /// one here is checked at run time (D4).
    pub claims: BTreeMap<(usize, String), Claim>,
    /// The call arguments the compiler writes a **`&mut`** for
    /// ([ADR-094](../../docs/specification/adr/adr-094.md) D3), keyed as
    /// [`Checked::lent_args`] is.
    ///
    /// The third state beside *lent* and *handed over*, and the one that is a
    /// **declaration** rather than an inference: `mut out: Vec[i64]` is the
    /// claim, and a parameter without the word does not become one whatever its
    /// body does. `keeps::lends` withholds its own answer on such a position,
    /// so the two never both write a reference.
    ///
    /// What the caller has to do is what it already does to call `xs.push(1)`:
    /// write `let mut xs`. The call itself shows nothing, which is D3's whole
    /// sentence — a language that hides mutation through a receiver and shows
    /// it through an argument has two rules for one thing.
    pub mut_args: BTreeMap<(usize, String, usize), BTreeSet<String>>,
    /// **The arguments that are a view of a value that copies, handed where
    /// the value is wanted**, keyed as [`Checked::mut_args`] is
    /// ([ADR-233](../../docs/specification/adr/adr-233.md) D4). The emitter
    /// writes the `*` that copies it out.
    pub copied_args: BTreeMap<(usize, String, usize), BTreeSet<String>>,
    /// **The functions in which a value with a cleanup may die**, by key, and
    /// whether their settle point can fail them - a cleanup that can fail, in
    /// a function that says `throws`
    /// ([ADR-239](../../docs/specification/adr/adr-239.md) D2, D3): their body
    /// is written as a settle point's.
    pub settle_fns: BTreeMap<String, bool>,
    /// **The `let`s that bind a value with a cleanup**, by the byte the
    /// statement starts at, and whether its cleanup can fail (ADR-239 D2): the
    /// block they stand in is settled where it ends.
    pub settle_lets: BTreeMap<usize, bool>,
    /// What each `spawn` body **binds and then holds across a pause**, by the
    /// byte the `spawn` starts at and the name
    /// ([ADR-055](../../docs/specification/adr/adr-055.md) §2 D6).
    ///
    /// **Written down because it is a computation and not a lookup.** The
    /// refusal beside it (`NK2501`) is silent today and will be until a type
    /// answers `MayNot` at `Destination::Ours` — no type does, and
    /// `contracts::send` says why. So the *liveness* half, which is an
    /// over-approximation that can be wrong, would otherwise be code nothing
    /// can see the answer of. `contracts::send::held_across_a_pause` is the
    /// rule itself and is tested on its own; this is what the walk feeding it
    /// found in a real program.
    pub held_across_a_pause: BTreeSet<(usize, String)>,
    /// The `let` statements whose initialiser is a **place** holding a value
    /// that would have to move, by the byte the statement starts at
    /// ([ADR-094](../../docs/specification/adr/adr-094.md) D4).
    ///
    /// `let s = totals.stations[name]` and `let name = config.name` are views:
    /// the language below refuses to move a value out of a container or out of
    /// a borrowed field, so a move there was never what the line meant, and the
    /// emitter writes the `&` it would otherwise have been refused for leaving
    /// out.
    ///
    /// **And only where the value would move.** `let mi = self.bodies[i].mass`
    /// over an `f64` is a *copy*, and a `&` there is a borrow held across the
    /// loop that writes the same field — `E0502` about a file nobody wrote.
    /// Which of the two is a question about the type, the emitter has none
    /// (ADR-028), and [`moves_away`] is the same answer `NK2101` reads.
    ///
    /// **Silence where the type is unknown**, which leaves the statement
    /// exactly where every program already is. The two wrong answers are both
    /// `rustc`'s words about the generated file — a borrow that conflicts, or a
    /// move out of a container — so this is a set that has to be *right* rather
    /// than one that can be safe.
    pub lent_lets: BTreeSet<usize>,
    /// The `return`s that hand back a **view of the subject**, by the byte the
    /// statement starts at ([ADR-094](../../docs/specification/adr/adr-094.md)
    /// D1's third position).
    ///
    /// `fn text(ref self) -> ref String { return self.text }` is a view of a
    /// borrowed subject and not a move out of it, so the `&` is the compiler's
    /// to write — the same sentence that decides an argument's, one position
    /// over. Written by the author it is `NK1137`; left out by both it was
    /// `NK1131` **and** `NK1104`, so the accessor a program most often writes
    /// had no spelling at all.
    ///
    /// **Only where the declared result is a view and the value is a place
    /// inside a borrowed subject.** Either half missing and the line is what it
    /// always was: a move out of a loan, which is `NK1131`.
    pub lent_returns: BTreeSet<usize>,
    /// The declared types whose values may be compared, so the emitter derives
    /// `PartialEq` for them ([ADR-204](../../docs/specification/adr/adr-204.md)
    /// D1).
    ///
    /// **Decided here and written there**, which is
    /// [ADR-028](../../docs/specification/adr/adr-028.md)'s division: whether
    /// every part of a type compares is a question about types, and the emitter
    /// keeps none.
    pub compares: BTreeSet<String>,
    /// The subset of [`Checked::compares`] that may also derive `Eq`.
    ///
    /// **A float is the whole of the difference.** Rust's `f64` is `PartialEq`
    /// and not `Eq`, because `NaN != NaN`; so a type holding one compares and is
    /// not an equivalence, and a derive that asked for both would refuse the
    /// declaration.
    pub compares_totally: BTreeSet<String>,
    /// **Where a number is read through a `for` binding**, by the byte the
    /// statement starts at and the name it was written under
    /// ([ADR-182](../../docs/specification/adr/adr-182.md) D1).
    ///
    /// A `for` lends ([ADR-094](../../docs/specification/adr/adr-094.md) D4),
    /// so the binding is a view of the element and Rust's `as` does not see
    /// through one. Answered here for the reason every set beside it is: which
    /// name is a view is a question about the **scope**, and the emitter keeps
    /// none (ADR-028).
    ///
    /// Keyed by the **name** as well as by the statement, because one
    /// statement may cast twice — `(n as i64) + (m as i64)` — and only one of
    /// the two may be a binding.
    pub viewed_numbers: BTreeSet<(usize, String)>,
    /// The list literals that are an **array** rather than a list
    /// ([ADR-152](../../docs/specification/adr/adr-152.md) D4), by the byte the
    /// `[` stands at.
    ///
    /// `[0.0, 0.0, 0.0]` is a `Vec` on its own and an `Array[f64, 3]` where the
    /// use asks for one, and which of the two it is decides what the emitter
    /// writes - `vec![…]` or `[…]`. The use is a *type*, and the emitter has
    /// none ([ADR-028](../../docs/specification/adr/adr-028.md)), so the answer
    /// is computed here and handed over.
    ///
    /// **The literal's own byte and not the statement's**, which is the one
    /// place this parts company with `lent_lets` and its neighbours: a
    /// statement may hold a list and an array both, and the two lower
    /// differently. `Expr::ListLit` is the one expression that carries a
    /// position, and it carries it for this.
    pub array_literals: BTreeSet<usize>,
    /// The text literals that stand where **text of its own** is wanted
    /// ([ADR-207](../../docs/specification/adr/adr-207.md) D2), by the byte
    /// the opening quote stands at.
    ///
    /// `"x"` is a view of static text on its own and a `String` where the use
    /// asks for one - a field, an annotated `let`, a `return`, an argument the
    /// callee keeps. The emitter writes `"x"` for one that is not in here and
    /// `String::from("x")` for one that is, and the question is a type's, so
    /// it is answered here and handed over, as `array_literals` is.
    ///
    /// **An argument the callee only reads is not in here**: the parameter is
    /// a `&str` below (D3), so the literal is lent as it is and allocates
    /// nothing.
    pub owned_texts: BTreeSet<usize>,
    /// The text literals that are the fallback of a `??` whose left side is a
    /// **view** of text, by the literal's byte
    /// ([ADR-209](../../docs/specification/adr/adr-209.md) §6). The emitter
    /// writes every other fallback as `"…".into()`, so that `?? "none"` on a
    /// `String?` is a `String` - and on a `ref String?` that conversion is the
    /// one thing the language below cannot resolve: *type annotations needed*
    /// about `m["host"] ?? "-"` over a map of views.
    pub view_fallbacks: BTreeSet<usize>,
    /// Per function - by the name the ledger records it under - where its
    /// method calls went (ADR-028).
    ///
    /// The second thing the checker answers for somebody else, after
    /// `fallible_loops`, and for the same reason: the question is about types,
    /// and this is the module that has them.
    ///
    /// **A `spawn` body's method calls land on the function around it**, where
    /// `sync.rs` would not walk into one at all. That is harmless rather than
    /// agreed: a function containing a `spawn` has already lost its claim to be
    /// `sync` on the strength of the `spawn`, so nothing is decided by what the
    /// task's body calls.
    pub methods: BTreeMap<String, MethodCalls>,
    /// [`StoredCode`] by `Type.field`.
    pub stored_code: BTreeMap<String, StoredCode>,
}

/// Every type mistake the ledgers are enough to see, and every loop that can
/// fail.
///
/// For one file. A program of several (Part I, 9.1) uses [`check_program`],
/// which additionally knows which names are *modules* - and therefore which
/// qualified calls cross a file boundary.
pub fn check(parsed: &Parsed, own: &Ledger, library: &Ledger) -> Checked {
    check_program(parsed, own, library, &BTreeSet::new())
}

/// **What moved in a contract since the committed ledger**, keyed by the
/// callee's name: the errors it newly throws
/// ([ADR-101](../../docs/specification/adr/adr-101.md) D1), and the parameters
/// whose `keeps` changed on a type whose teardown does something
/// ([ADR-236](../../docs/specification/adr/adr-236.md) D1).
///
/// Empty for every build that has nothing to compare — a first build, a loose
/// file, a package whose ledger is not committed yet — which is the honest
/// answer: nothing was written against the old contract, so nothing has
/// changed for anybody.
#[derive(Debug, Clone, Default)]
pub struct Newly {
    /// The errors each callee throws now and did not.
    pub throws: BTreeMap<String, Vec<String>>,
    /// The parameters each callee keeps now and did not, or keeps no longer.
    pub keeps: BTreeMap<String, Vec<Kept>>,
}

/// **A parameter whose `keeps` moved** (ADR-236 D1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// The parameter, as the callee's signature names it.
    pub param: String,
    /// `true` where the callee keeps it now and did not; `false` where it kept
    /// it and only reads it now.
    pub now: bool,
    /// The parameter's type, whose teardown does something.
    pub ty: String,
}

impl Newly {
    /// Nothing moved.
    pub fn new() -> Newly {
        Newly::default()
    }
}

/// A map of newly thrown errors is a `Newly` about errors alone.
impl FromIterator<(String, Vec<String>)> for Newly {
    fn from_iter<I: IntoIterator<Item = (String, Vec<String>)>>(items: I) -> Newly {
        Newly {
            throws: items.into_iter().collect(),
            keeps: BTreeMap::new(),
        }
    }
}

/// The same, for one file of a program made of several.
///
/// `modules` is what the program is made of, and it is the whole difference: a
/// qualified call is either into another **module**, where Part I 9.2 says
/// `pub` decides, or into a **type** (`Stats::new`), where it does not. Without
/// the set there is no telling those apart, and a private constructor called in
/// its own file would be reported as a privacy violation.
pub fn check_program(
    parsed: &Parsed,
    own: &Ledger,
    library: &Ledger,
    modules: &BTreeSet<String>,
) -> Checked {
    check_against(
        parsed,
        &[],
        own,
        library,
        modules,
        &Newly::new(),
        &Reads::none(),
    )
}

/// The same, told what a callee has newly gained since the committed ledger
/// ([ADR-101](../../docs/specification/adr/adr-101.md) D1).
///
/// **A second entry point rather than a parameter on the first**, because the
/// answer is a fact about the *build* — two ledgers, one of them on disk — and
/// every other caller of `check_program` is handed one program and no history.
/// An empty map is what they all pass, and an empty map says nothing.
pub fn check_against<'a>(
    parsed: &'a Parsed,
    // **The program's other files**, for the one question that needs a body
    // rather than a contract: a `comptime` calling across a file boundary
    // ([ADR-073](../../docs/specification/adr/adr-073.md) D5). Empty for a
    // caller that has one file, which is every test and every `--input`
    // outside a project.
    beside: &'a [&'a Parsed],
    own: &'a Ledger,
    library: &'a Ledger,
    modules: &BTreeSet<String>,
    newly: &'a Newly,
    // **What this build may read while it builds**
    // ([ADR-072](../../docs/specification/adr/adr-072.md)). The same shape as
    // `beside`, and for the same reason: it is a fact about the *build* that no
    // ledger can carry, and a caller with nothing to say passes
    // [`assets::Reads::none`], which is D1 — a build given no list reads
    // nothing.
    reads: &'a Reads,
) -> Checked {
    walked(parsed, beside, own, library, modules, newly, reads, false)
}

/// **Which types the program's other files called this unit's shape walks
/// with** ([ADR-181](../../docs/specification/adr/adr-181.md) D2).
///
/// A walk of `parsed` that stops as soon as the calls have been typed: nothing
/// past the item walk is read, so none of the separate passes below run and no
/// finding is kept. What comes back is one file's contribution to a question
/// about the whole program.
fn instantiations_in<'a>(
    parsed: &'a Parsed,
    beside: &'a [&'a Parsed],
    own: &'a Ledger,
    library: &'a Ledger,
    // **The same module names the asking unit was checked with**, because a
    // name this walk cannot resolve is a call whose argument it cannot type -
    // and an instantiation it misses is a copy nobody writes.
    modules: &BTreeSet<String>,
    reads: &'a Reads,
) -> BTreeMap<(String, String), Vec<FieldContract>> {
    walked(
        parsed,
        beside,
        own,
        library,
        modules,
        &Newly::new(),
        reads,
        true,
    )
    .unrolled
}

/// The walk both entry points above share.
#[allow(clippy::too_many_arguments)]
fn walked<'a>(
    parsed: &'a Parsed,
    beside: &'a [&'a Parsed],
    own: &'a Ledger,
    library: &'a Ledger,
    modules: &BTreeSet<String>,
    newly: &'a Newly,
    reads: &'a Reads,
    harvesting: bool,
) -> Checked {
    let mut checker = Checker {
        newly,
        parsed,
        beside,
        reads,
        said_rings: BTreeSet::new(),
        own,
        library,
        structs: BTreeMap::new(),
        enums: BTreeMap::new(),
        enum_payloads: BTreeMap::new(),
        variant_parts: BTreeMap::new(),
        variant_owner: BTreeMap::new(),
        walks_fields: BTreeMap::new(),
        unrolling: None,
        harvesting,
        grammars: parsed
            .program
            .items
            .iter()
            .filter_map(|item| match &item.node {
                Item::Grammar(def) => Some((
                    parsed.text(def.name).to_string(),
                    def.rules
                        .iter()
                        .filter(|r| r.is_public)
                        .map(|r| parsed.text(r.name).to_string())
                        .collect(),
                )),
                _ => None,
            })
            .collect(),
        scope: Vec::new(),
        inside_a_comptime: false,
        at_a_write_door: false,
        set_receiver: None,
        stamped_condition: None,
        inside_a_door: false,
        stored_frames: Vec::new(),
        inside_an_action: None,
        inside_a_sync_function: None,
        std_in_scope: parsed
            .program
            .items
            .iter()
            .filter_map(|item| match &item.node {
                Item::Import { path, .. } if path.len() == 2 => {
                    (parsed.text(path[0]) == "std").then(|| parsed.text(path[1]).to_string())
                }
                _ => None,
            })
            .collect(),
        std_modules: library
            .functions
            .keys()
            .chain(library.types.keys())
            .filter_map(|key| key.split_once("::").map(|(prefix, _)| prefix.to_string()))
            .filter(|prefix| {
                !library.types.contains_key(prefix)
                    && !library.functions.contains_key(&format!("{prefix}::new"))
            })
            .collect(),
        task_bindings: Vec::new(),
        said_mut: BTreeSet::new(),
        expected: None,
        expected_either: false,
        field_either: false,
        type_parameters: BTreeMap::new(),
        struct_parameters: BTreeMap::new(),
        borrowing_self: false,
        enclosing: BTreeMap::new(),
        subject_arguments: Vec::new(),
        declared_bounds: BTreeMap::new(),
        throwing: false,
        caught: false,
        guarded: None,
        loops: 0,
        barrier: None,
        current: None,
        modules: modules.clone(),
        fallible_methods: BTreeSet::new(),
        pausing_methods: BTreeSet::new(),
        settled_methods: BTreeSet::new(),
        handed_over: None,
        paused_args: Vec::new(),
        in_parallel: None,
        opaque_handles: parsed
            .program
            .items
            .iter()
            .filter_map(|item| match &item.node {
                Item::Extern { opaque, .. } => Some(opaque),
                _ => None,
            })
            .flatten()
            .map(|h| parsed.text(h.node.name).to_string())
            .collect(),
        foreign_names: parsed
            .program
            .items
            .iter()
            .filter_map(|item| match &item.node {
                Item::Extern { declarations, .. } => Some(declarations),
                _ => None,
            })
            .flatten()
            .map(|d| parsed.text(d.node.name).to_string())
            .collect(),
        inside_unsafe: false,
        moved_into_a_task: Vec::new(),
        walked: Vec::new(),
        branch: Vec::new(),
        next_choice: 0,
        reads_on_paths: Vec::new(),
        read_index: std::collections::HashMap::new(),
        lent_lets: BTreeSet::new(),
        handed: Vec::new(),
        changed: Vec::new(),
        writing_index: false,
        read_a_map: false,
        hole: None,
        run_code: BTreeSet::new(),
        last_resolved: None,
        receiver_name: None,
        caught_several: false,
        caught_one: None,
        read_at: Vec::new(),
        written_at: Vec::new(),
        repeats: Vec::new(),
        reading_only: false,
        read_seq: 0,
        empty_lists: BTreeMap::new(),
        grown_text: BTreeSet::new(),
        opaque_methods: BTreeSet::new(),
        widening_casts: BTreeSet::new(),
        checked: Checked::default(),
    };
    checker.collect_types();
    checker.program();
    // **Only the call sites were wanted**, and everything below this line
    // answers a different question at a cost the asking unit has already paid
    // for itself (ADR-181 D2).
    if harvesting {
        return checker.checked;
    }
    // **The calls that stand in the program's other files**, before the turns
    // are walked, because a copy is written by the unit that declares the
    // function and decided at the call - which may be a file away (Part I 9.1).
    checker.instantiations_beside();
    // **And once more per unrolled turn** (ADR-088 D5, ADR-181 D2), which the
    // walk above is what found: a call may stand above the function it names.
    checker.unroll();
    checker.checked.walks_fields = checker.walks_fields.clone();
    // ADR-007 D5: the DSL parameters a call forgot, and the ones it invented.
    // A separate walk because it answers a question about a *statement's
    // holes* rather than about a type, and it needs no ledger to answer it.
    checker.checked.findings.extend(crate::dsl::check(parsed));
    // A naked view parameter that is kept past the call (`NK2302`). Also a
    // separate walk, and for the same reason: it asks where a *value* goes
    // rather than what a type is. It reads both ledgers, because **a call says
    // which of its arguments its result may point into** and a parameter that
    // is none of them does not escape through it.
    checker
        .checked
        .findings
        .extend(crate::views::check(parsed, own, library));
    // A view handed back that points into a buffer the body owns (`NK2303`).
    // The tether's own refusal
    // ([ADR-156](../../docs/specification/adr/adr-156.md) D4), and a walk of
    // its own for the same reason `views` is: it asks where a *value* points
    // rather than what a type is. It reads both ledgers, because which calls
    // make a buffer is what they say.
    checker
        .checked
        .findings
        .extend(crate::contracts::tether::check(parsed, own, library));
    // Kap 4.7: what an `impl` owes the `trait` it names. Separate for a reason
    // of its own - it reads the ledger's **finished** `sync` column, and the
    // type walk runs before `sync::infer` fills that in
    // ([ADR-080](../../docs/specification/adr/adr-080.md)).
    checker
        .checked
        .findings
        .extend(crate::traits::check(parsed, own));
    // Part I 2.2: a **type** nothing declares, which had nothing where a value
    // has had `NK1117` since ADR-051
    // ([ADR-096](../../docs/specification/adr/adr-096.md)). Separate for the
    // same reason as the three above: it asks about a written *name* rather
    // than about a value's type, so it needs the item tree and neither the
    // scope stack nor the inference.
    checker
        .checked
        .findings
        .extend(crate::types::check(parsed, own, library));
    checker.checked.findings.sort_by_key(|f| f.span.start);
    // Only the calls that provably fail, and only where the name is not also a
    // call that does not: the emitter writes a `?` for each of these, and a `?`
    // on something that is not a failure is a `rustc` error about a file the
    // author never wrote.
    checker.checked.fallible_methods = checker
        .fallible_methods
        .difference(&checker.opaque_methods)
        .cloned()
        .collect();
    // The same subtraction one column over: an `.await` on something that is not
    // a future is the same kind of message about the same file nobody wrote.
    checker.checked.pausing_methods = checker
        .pausing_methods
        .difference(&checker.settled_methods)
        .cloned()
        .collect();
    // The same subtraction, for the same reason: a checked conversion written on
    // a widening one does not compile.
    checker
        .checked
        .narrowing_casts
        .retain(|at, _| !checker.widening_casts.contains(at));
    checker.checked
}

/// Which kind of checked conversion a narrowing one needs
/// ([ADR-043](../../../docs/specification/adr/adr-043.md) D4).
///
/// Two kinds because Rust gives one of them and not the other, which is measured
/// in that record: `i32::try_from` exists for an integer, and
/// `i32::try_from(f64)` does not - so a conversion out of a floating-point number
/// needs its own test for the range and for "not a number".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Narrowing {
    /// An integer that may not fit the smaller one: `i64` to `i32`.
    Integer,
    /// A floating-point number to an integer, silent three ways in Rust - it
    /// stops at the limit in both directions and turns "not a number" into zero.
    FromFloat,
}

/// Everything the emitter needs that only a type checker can answer: the loops
/// whose step can fail, the method calls that can fail, and the places where a
/// declared `Shared[T]` makes the first handle.
///
/// All three together because all three come out of one pass, and a second pass
/// Which wrap a value standing in `want` needs, and `None` where it needs none.
///
/// One function for all four of Part I 2.3's positions, so the rule is stated
/// once: the annotated `let`, the assignment, the `return`, and a struct
/// literal's field — and a call's argument, which is the fourth written a fifth
/// way. Before this they were four copies of the same three conditions, and the
/// copies had already drifted in what they did with an unknown type.
/// **Which of a statement's calls an argument belongs to**, as text both the
/// checker and the emitter can produce from the same tree.
///
/// A key of statement, callee and position names a *parameter*; a statement may
/// call one function more than once, and the argument itself is what tells the
/// two calls apart. The alternatives were weighed and each fails on something
/// this does not:
///
///   * **the argument's span** - an argument has none. One node type does:
///     [ADR-081](../../docs/specification/adr/adr-081.md) D1 gave `Expr::Binary`
///     a span of its own, for a key of exactly this kind. It is one node and an
///     argument is rarely that one, so there is nothing general to key by -
///     which is the reason this whole side table exists;
///   * **the address of the `Expr`** - stable for a statement walked twice, and
///     not for an `f"…"` hole, which is *text* until each pass parses it into a
///     tree of its own ([`crate::emit::literal_expressions`]);
///   * **the order the calls are walked in** - which would make the two passes
///     agree by assumption rather than by construction.
///
/// So it is structural: the same expression, from either pass, renders the
/// same. The interner is shared, so an identifier renders as the same `Symbol`
/// on both sides - including in a re-parsed hole, which is parsed with that
/// interner.
///
/// **Two textually identical arguments share an entry, and want the same
/// answer**: the wrap is a function of the argument's type and the parameter's,
/// and within one statement the same expression in the same position has the
/// same type.
///
/// It is built only where a wrap was recorded for this statement, callee and
/// position - which is rare - so the cost is not on the path every argument
/// takes.
/// Whether an expression **can only** be a value: a name, a literal, an
/// operator, a field, an index.
///
/// Whether this expression **leaves** rather than coming to a value
/// ([ADR-138](../../docs/specification/adr/adr-138.md) D1).
///
/// Its type is *never*, which is not a type this checker's `Ty` spells: what
/// never means here is *this is not one of the answers that have to agree*, and
/// that is a question about the expression rather than about a type. A block
/// counts where its last statement is one of the four, which is the shape an
/// arm written `=> { throw NotFound }` still has.
/// Whether an expression **leaves the function** on every path through it -
/// a `return` or a `throw` - which `leaves` says and more: a `break` and a
/// `continue` leave a branch and not the function, so what was taken before
/// them is still taken after the loop (ADR-213 D4).
fn exits(expr: &Expr) -> bool {
    match expr {
        Expr::Throw(_) | Expr::Return(_) => true,
        Expr::Block(block) => block_exits(block),
        _ => false,
    }
}

/// The same question about a block: its last statement leaves the function.
fn block_exits(block: &Block) -> bool {
    match block.stmts.last().map(|s| &s.node) {
        Some(Stmt::Return(_)) => true,
        Some(Stmt::Expr(inner)) => exits(inner),
        _ => false,
    }
}

fn leaves(expr: &Expr) -> bool {
    match expr {
        Expr::Throw(_) | Expr::Return(_) | Expr::Break | Expr::Continue => true,
        Expr::Block(block) => block_leaves(block),
        _ => false,
    }
}

/// The same question about a **block**, which is what a `select` arm's body is
/// ([ADR-148](../../docs/specification/adr/adr-148.md) D1).
///
/// Part II 12.4's own example has two arms and both of them jump, so this is
/// what decides that the `select` around them carries no value.
fn block_leaves(block: &Block) -> bool {
    match block.stmts.last().map(|s| &s.node) {
        Some(Stmt::Return(_)) | Some(Stmt::Break) | Some(Stmt::Continue) => true,
        Some(Stmt::Expr(inner)) => leaves(inner),
        _ => false,
    }
}

/// Whether a pattern matches every value of its type
/// ([ADR-137](../../docs/specification/adr/adr-137.md) D1,
/// [ADR-145](../../docs/specification/adr/adr-145.md) D1).
///
/// **A bare name is a catch-all, and covers**: one segment and no brackets is
/// the binding form — `Op::Times` is two, and `Message::Write(text)` is a
/// tuple. An **or-pattern** covers where one of its alternatives does, which
/// makes `Op::Plus | else` the odd thing it looks like and not a hole.
fn catches_everything(pattern: &MatchPattern) -> bool {
    match pattern {
        MatchPattern::Otherwise => true,
        MatchPattern::Path(path) => path.len() == 1,
        MatchPattern::Or(alternatives) => alternatives.iter().any(catches_everything),
        _ => false,
    }
}

/// What kind of value an element of a list literal is, where that much is known
/// without a type ([ADR-135](../../docs/specification/adr/adr-135.md) D1).
///
/// **A literal first**, because that is the case the type cannot answer: a bare
/// `1` fits every numeric type, so it arrives as `?` and two of those say
/// nothing about each other. A number and a piece of text do.
///
/// `i64` and `f64` are one kind here, deliberately: this is a **coarse** answer
/// used only to refuse, and a finer one would refuse `[1, 1.5]` on a reading of
/// the literals rather than of the program.
fn element_kind(expr: &Expr, ty: &Ty) -> Option<&'static str> {
    match expr {
        Expr::LitInt(_) | Expr::LitFloat(_) => return Some("a number"),
        Expr::LitStr { .. } | Expr::LitInterpolated(_) => return Some("text"),
        Expr::LitChar(_) => return Some("a character"),
        Expr::LitBool(_) => return Some("a `bool`"),
        _ => {}
    }
    let Ty::Named { name, .. } = ty else {
        return None;
    };
    match name.as_str() {
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" | "f32" | "f64" => {
            Some("a number")
        }
        "str" | "String" => Some("text"),
        "char" => Some("a character"),
        "bool" => Some("a `bool`"),
        _ => None,
    }
}

/// The half of `NK1141`'s question that is safe to answer from the shape. A
/// call and a method call are deliberately not here: whether one comes to a
/// value is a question about its callee, and answering it wrongly is a correct
/// program refused ([Part III C.4](../../docs/specification/30-nikaia-tooling.md)).
fn plainly_a_value(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Variable(_)
            | Expr::LitInt(_)
            | Expr::LitFloat(_)
            | Expr::LitStr { .. }
            | Expr::LitBool(_)
            | Expr::Binary { .. }
            | Expr::Field { .. }
            | Expr::Index { .. }
            | Expr::Tuple(_)
    )
}

/// **Where a text literal stands, as the emitter will find it.** A hole of an
/// f-string is parsed on its own, so a literal inside one has an offset into
/// the hole and not into the file - and two holes, or a hole and the file's own
/// text, would share it. Inside a hole the key is the offset mixed with the
/// statement and the hole's text, above every offset a file can have.
pub fn text_key(hole: Option<&(usize, String)>, at: usize) -> usize {
    use std::hash::{Hash, Hasher};
    let Some((statement, text)) = hole else {
        return at;
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (statement, text, at).hash(&mut hasher);
    (hasher.finish() as usize) | (1 << (usize::BITS - 1))
}

/// **The call a named function stands for where a function value is kept**:
/// `double` put in a field is `fn(__nikaia_arg0) { double(__nikaia_arg0) }`.
/// One construction for the checker, which types it, and the emitter, which
/// writes it, so the two agree about every key the call is looked up by.
pub(crate) fn kept_function_call(parsed: &Parsed, name: Ident, arity: usize) -> (Vec<Ident>, Expr) {
    let names: Vec<Ident> = (0..arity)
        .map(|at| parsed.interner.intern_string(&format!("__nikaia_arg{at}")))
        .collect();
    let call = Expr::Call {
        func: Box::new(Expr::Variable(name)),
        args: names.iter().map(|n| Expr::Variable(*n)).collect(),
        config: Vec::new(),
    };
    (names, call)
}

pub fn argument_shape(expr: &Expr) -> String {
    format!("{expr:?}")
}

/// A view of text going into text of its own, present or absent.
fn views_into_text(found: &Ty, want: &Ty) -> bool {
    (*found == Ty::view("str") && *want == Ty::named("String"))
        || views_into_nullable_text(found, want)
}

/// A view of text, present or absent, going into a `String?`
/// ([ADR-224](../../../docs/specification/adr/adr-224.md) D1).
fn views_into_nullable_text(found: &Ty, want: &Ty) -> bool {
    let present = match found {
        Ty::Nullable(found) => found.as_ref(),
        found => found,
    };
    matches!(want, Ty::Nullable(inner) if **inner == Ty::named("String"))
        && *present == Ty::view("str")
}

fn wrap_for(found: &Ty, want: &Ty, literal: bool) -> Option<Wrap> {
    if !matches!(want, Ty::Nullable(_)) || matches!(found, Ty::Nullable(_)) {
        return None;
    }
    match !found.is_unknown() || literal {
        true => Some(Wrap::Constructor),
        false => Some(Wrap::Conversion),
    }
}

/// **How a plain value is put into a nullable slot** (Part I 2.3,
/// [ADR-068](../../../docs/specification/adr/adr-068.md)).
///
/// The question the four positions of [ADR-052](../../../docs/specification/adr/adr-052.md)
/// D4 ask was never *whether* to wrap - it was **how**, and the second answer
/// was missing. A value this checker worked out to be a plain `T` takes the
/// constructor; one whose type it could not work out takes the conversion,
/// which is right whichever the value turns out to be.
///
/// A value already known to be a `T?` is in neither: it needs nothing, and the
/// position is simply not recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrap {
    /// `Some(value)`. The type is known and it is not a `T?`.
    Constructor,
    /// `value.into()`. The type is **not** known, and this is correct in both
    /// directions - Rust has `From<T> for Option<T>` and the identity
    /// `From<T> for T` - so the uncertainty stops needing an answer.
    Conversion,
}

/// would cost a whole type check to answer a question the first one already
/// answered.
#[derive(Debug, Clone, Default)]
pub struct Propagation {
    /// [`Checked::fallible_loops`].
    pub loops: BTreeSet<usize>,
    /// [`Checked::pausing_loops`].
    pub pausing_loops: BTreeSet<usize>,
    /// [`Checked::owned_loops`].
    pub owned_loops: BTreeSet<usize>,
    /// [`Checked::count_args`].
    pub count_args: BTreeSet<(usize, String, usize)>,
    /// [`Checked::map_keys`].
    pub map_keys: BTreeMap<(usize, String), KeyForm>,
    /// [`Checked::slice_indices`].
    pub slice_indices: BTreeSet<(usize, String)>,
    /// [`Checked::owned_copies`].
    pub owned_copies: BTreeSet<(usize, String)>,
    /// [`Checked::text_as_is`].
    pub text_as_is: BTreeSet<(usize, String)>,
    /// [`Checked::counted`].
    pub counted: BTreeSet<(usize, String)>,
    /// [`Checked::lent_bindings`].
    pub lent_bindings: BTreeMap<usize, BTreeSet<String>>,
    /// [`Checked::collected_into`].
    pub collected_into: BTreeSet<(usize, String)>,
    /// [`Checked::copied_walks`].
    pub copied_walks: BTreeSet<(usize, String)>,
    /// [`Checked::filter_patterns`].
    pub filter_patterns: BTreeSet<(usize, String)>,
    /// [`Checked::kept_lambdas`].
    pub kept_lambdas: BTreeMap<(usize, String), (bool, bool)>,
    /// [`Checked::kept_hulls`].
    pub kept_hulls: BTreeMap<(usize, String), Vec<String>>,
    /// [`Checked::field_calls`].
    pub field_calls: BTreeMap<(usize, String), bool>,
    /// [`Checked::kept_calls`].
    pub kept_calls: BTreeMap<(usize, String), (bool, bool)>,
    /// [`Checked::kept_args`].
    pub kept_args: BTreeSet<(usize, String)>,
    /// [`Checked::kept_functions`].
    pub kept_functions: BTreeMap<(usize, String), (bool, bool, usize)>,
    /// [`Checked::pausing_walks`].
    pub pausing_walks: BTreeSet<(usize, String)>,
    /// [`Checked::fallible_methods`].
    pub methods: BTreeSet<(usize, String)>,
    /// [`Checked::pausing_methods`].
    pub pausing_methods: BTreeSet<(usize, String)>,
    /// [`Checked::witnessed_sets`].
    pub witnessed_sets: BTreeSet<usize>,
    /// [`Checked::future_lambdas`].
    pub future_lambdas: BTreeSet<(usize, usize)>,
    /// [`Checked::run_lambdas`].
    pub run_lambdas: BTreeSet<(usize, usize)>,
    /// [`Checked::pausing_lambdas`].
    pub pausing_lambdas: BTreeMap<(usize, String), PausingEntry>,
    /// [`Checked::narrowing_casts`].
    pub narrowing: BTreeMap<(usize, String), Narrowing>,
    /// [`Checked::nullable_sites`].
    pub nullable: BTreeMap<usize, Wrap>,
    /// [`Checked::flattened_reaches`].
    pub flattened: BTreeSet<(usize, String)>,
    /// [`Checked::copied_reaches`].
    pub copied: BTreeSet<(usize, String)>,
    /// [`Checked::viewed_reaches`].
    pub viewed: BTreeMap<(usize, String), Viewed>,
    /// [`Checked::lent_reaches`].
    pub lent_reaches: BTreeSet<(usize, String)>,
    /// [`Checked::held_reaches`].
    pub held_reaches: BTreeSet<(usize, String)>,
    /// [`Checked::nullable_fields`].
    pub nullable_in_fields: BTreeMap<(usize, String, String), BTreeMap<String, Wrap>>,
    /// [`Checked::nullable_args`].
    pub nullable_in_args: BTreeMap<(usize, String, usize), BTreeMap<String, Wrap>>,
    /// [`Checked::task_handles`].
    pub task_handles: BTreeSet<(usize, String)>,
    /// [`Checked::comptime_values`].
    pub comptime_values: BTreeMap<usize, (String, String)>,
    /// [`Checked::with_types`].
    pub with_types: BTreeMap<usize, String>,
    /// [`Checked::unrolled`].
    pub unrolled: BTreeMap<(String, String), Vec<FieldContract>>,
    /// [`Checked::walks_fields`].
    pub walks_fields: BTreeMap<String, String>,
    /// [`Checked::unrolled_calls`].
    pub unrolled_calls: BTreeMap<usize, String>,
    /// [`Checked::concatenations`].
    pub concatenations: BTreeSet<usize>,
    /// [`Checked::lent_lets`].
    pub lent_lets: BTreeSet<usize>,
    /// [`Checked::lent_returns`].
    pub lent_returns: BTreeSet<usize>,
    /// [`Checked::compares`].
    pub compares: BTreeSet<String>,
    /// [`Checked::compares_totally`].
    pub compares_totally: BTreeSet<String>,
    /// **Where a number is read through a `for` binding**, by the byte the
    /// statement starts at and the name it was written under
    /// ([ADR-182](../../docs/specification/adr/adr-182.md) D1).
    ///
    /// A `for` lends ([ADR-094](../../docs/specification/adr/adr-094.md) D4),
    /// so the binding is a view of the element and Rust's `as` does not see
    /// through one. Answered here for the reason every set beside it is: which
    /// name is a view is a question about the **scope**, and the emitter keeps
    /// none (ADR-028).
    ///
    /// Keyed by the **name** as well as by the statement, because one
    /// statement may cast twice — `(n as i64) + (m as i64)` — and only one of
    /// the two may be a binding.
    pub viewed_numbers: BTreeSet<(usize, String)>,
    /// [`Checked::array_literals`].
    pub array_literals: BTreeSet<usize>,
    /// [`Checked::owned_texts`].
    pub owned_texts: BTreeSet<usize>,
    /// [`Checked::view_fallbacks`].
    pub view_fallbacks: BTreeSet<usize>,
    /// [`Checked::lent_args`].
    pub lent_args: BTreeMap<(usize, String, usize), BTreeSet<String>>,
    /// [`Checked::lookup_keys`].
    pub lookup_keys: BTreeSet<(usize, String, String)>,
    /// [`Checked::boxed_members`].
    pub boxed_members: BTreeMap<String, BTreeSet<String>>,
    /// [`Checked::boxed_reads`].
    pub boxed_reads: BTreeSet<(usize, String)>,
    /// [`Checked::copied_bindings`].
    pub copied_bindings: BTreeSet<(usize, String)>,
    /// [`Checked::compared_views`].
    pub compared_views: BTreeSet<(usize, String)>,
    /// [`Checked::claims`].
    pub claims: BTreeMap<(usize, String), Claim>,
    /// [`Checked::mut_args`].
    pub mut_args: BTreeMap<(usize, String, usize), BTreeSet<String>>,
    /// [`Checked::copied_args`].
    pub copied_args: BTreeMap<(usize, String, usize), BTreeSet<String>>,
    /// [`Checked::settle_fns`].
    pub settle_fns: BTreeMap<String, bool>,
    /// [`Checked::settle_lets`].
    pub settle_lets: BTreeMap<usize, bool>,
    /// [`Checked::method_options`].
    pub method_options: BTreeMap<(usize, String), Vec<(String, String)>>,
}

/// The loops whose step can fail, for a caller that wants only those.
///
/// The emitter's entry point: it builds the ledgers a program is compiled
/// against and asks this, rather than carrying the checker's findings around.
pub fn fallible_loops(parsed: &Parsed) -> BTreeSet<usize> {
    fallible_loops_against(parsed, &Ledger::infer(parsed))
}

/// The same, against contracts the caller already has - which for a program of
/// several files is the **program's** ledger and not this file's (Part I, 9.1).
pub fn fallible_loops_against(parsed: &Parsed, own: &Ledger) -> BTreeSet<usize> {
    propagation_against(parsed, &[], own, &Ledger::empty(), &Reads::none()).loops
}

/// **What a `T::fields` loop was unrolled to, for the types actually used**
/// ([ADR-088](../../docs/specification/adr/adr-088.md) D6, built as
/// [ADR-181](../../docs/specification/adr/adr-181.md) D5's `--comptime`).
///
/// Every build-time system shares one readability problem: you cannot see what
/// a function becomes for a given type without unrolling it in your head. The
/// usual answer is to invent syntax; this project already has the other one,
/// and `--overlaps`, `--sharing`, `--tethers` and `--trust` are it. So this is
/// the **same information** [ADR-181](../../docs/specification/adr/adr-181.md)
/// D3's diagnostic carries, offered on demand instead of on failure.
///
/// **A function that walks a shape and was never called is printed too**, with
/// its own line. It is the one thing a reader could not otherwise find out: no
/// copy is emitted for it at all, so nothing in the generated file says it
/// exists.
///
/// The whole program at once, because an instantiation is a fact about a
/// **call** and a call may stand in a different file from the function it names.
pub fn unrolling_report(beside: &[&Parsed], own: &Ledger, reads: &Reads) -> String {
    // **Every unit and not just the first**, because the two halves of one
    // line may stand in three different files: the function is declared in
    // one, called from a second, and the report is asked for by a build that
    // has a third. A report built from one file would say *nothing calls it*
    // about a function called twice.
    let mut walks_fields: BTreeMap<String, String> = BTreeMap::new();
    let mut unrolled: BTreeMap<(String, String), Vec<FieldContract>> = BTreeMap::new();
    for parsed in beside {
        let found = propagation_against(parsed, beside, own, &Ledger::empty(), reads);
        walks_fields.extend(found.walks_fields);
        unrolled.extend(found.unrolled);
    }
    let found = Propagation {
        walks_fields,
        unrolled,
        ..Propagation::default()
    };
    let mut out = String::new();
    for (function, parameter) in &found.walks_fields {
        let copies: Vec<(&String, &Vec<FieldContract>)> = found
            .unrolled
            .iter()
            .filter(|((written, _), _)| written == function)
            .map(|((_, on), fields)| (on, fields))
            .collect();
        if copies.is_empty() {
            out.push_str(&format!(
                "comptime: `{function}` walks `{parameter}::fields` and nothing calls it, \
                 so no copy is written\n"
            ));
            continue;
        }
        for (on, fields) in copies {
            out.push_str(&format!(
                "comptime: `{function}` unrolled over `{on}` as `{}`\n",
                specialised(function, on)
            ));
            for field in fields {
                out.push_str(&format!("    {}: {}\n", field.name, field.ty.text()));
            }
        }
    }
    out
}

/// Both halves of ADR-023 D8's propagation, against contracts the caller
/// already has.
pub fn propagation_against(
    parsed: &Parsed,
    beside: &[&Parsed],
    own: &Ledger,
    described: &Ledger,
    reads: &Reads,
) -> Propagation {
    let Ok(mut library) = Ledger::parse(crate::contracts::STD) else {
        return Propagation::default();
    };
    // **And the described crates' boundary**, as the check that refused had it
    // (`project::Foreign::library`, `std` winning a collision): what a call
    // into `hyper_shim` keeps decides how its argument is written, and a walk
    // that could not resolve the callee lowered a literal it had been told to
    // build as text of its own (0.0.230, `open-work.md` §1.19 before it).
    for (name, contract) in &described.functions {
        library
            .functions
            .entry(name.clone())
            .or_insert_with(|| contract.clone());
    }
    for (name, contract) in &described.types {
        library
            .types
            .entry(name.clone())
            .or_insert_with(|| contract.clone());
    }
    // **The same walk the refusal ran**, `beside` included: a `comptime` that
    // calls across a file boundary is answered by the checker, and what the
    // emitter writes is that answer. Handing it fewer files than the check had
    // would make the emitter refuse an item the checker accepted, which is the
    // two halves disagreeing about one program.
    // **And the same reads**, for the reason `beside` is here: a `comptime`
    // that reads a file is answered by the checker and what the emitter writes
    // is that answer, so a walk handed a different allowlist would refuse an
    // item the check accepted ([ADR-072](../../docs/specification/adr/adr-072.md)).
    let checked = check_against(
        parsed,
        beside,
        own,
        &library,
        &BTreeSet::new(),
        &Newly::new(),
        reads,
    );
    Propagation {
        loops: checked.fallible_loops,
        pausing_loops: checked.pausing_loops,
        owned_loops: checked.owned_loops,
        count_args: checked.count_args,
        map_keys: checked.map_keys,
        slice_indices: checked.slice_indices,
        owned_copies: checked.owned_copies,
        text_as_is: checked.text_as_is,
        counted: checked.counted,
        lent_bindings: checked.lent_bindings,
        collected_into: checked.collected_into,
        copied_walks: checked.copied_walks,
        filter_patterns: checked.filter_patterns,
        kept_lambdas: checked.kept_lambdas,
        kept_hulls: checked.kept_hulls,
        field_calls: checked.field_calls,
        kept_calls: checked.kept_calls,
        kept_args: checked.kept_args,
        kept_functions: checked.kept_functions,
        pausing_walks: checked.pausing_walks,
        methods: checked.fallible_methods,
        pausing_methods: checked.pausing_methods,
        witnessed_sets: checked.witnessed_sets,
        future_lambdas: checked.future_lambdas,
        run_lambdas: checked.run_lambdas,
        pausing_lambdas: checked.pausing_lambdas,
        narrowing: checked.narrowing_casts,
        nullable: checked.nullable_sites,
        flattened: checked.flattened_reaches,
        copied: checked.copied_reaches,
        viewed: checked.viewed_reaches,
        lent_reaches: checked.lent_reaches,
        held_reaches: checked.held_reaches,
        nullable_in_fields: checked.nullable_fields,
        nullable_in_args: checked.nullable_args,
        task_handles: checked.task_handles,
        comptime_values: checked.comptime_values,
        with_types: checked.with_types,
        unrolled: checked.unrolled,
        walks_fields: checked.walks_fields,
        unrolled_calls: checked.unrolled_calls,
        concatenations: checked.concatenations,
        lent_lets: checked.lent_lets,
        lent_returns: checked.lent_returns,
        compares: checked.compares,
        compares_totally: checked.compares_totally,
        viewed_numbers: checked.viewed_numbers,
        array_literals: checked.array_literals,
        owned_texts: checked.owned_texts,
        view_fallbacks: checked.view_fallbacks,
        lent_args: checked.lent_args,
        lookup_keys: checked.lookup_keys,
        boxed_members: checked.boxed_members,
        boxed_reads: checked.boxed_reads,
        copied_bindings: checked.copied_bindings,
        compared_views: checked.compared_views,
        claims: checked.claims,
        mut_args: checked.mut_args,
        copied_args: checked.copied_args,
        settle_fns: checked.settle_fns,
        settle_lets: checked.settle_lets,
        method_options: checked.method_options,
    }
}

/// How a `comptime`'s type is spelled in the language below, where this compiler
/// can spell it ([ADR-073](../../docs/specification/adr/adr-073.md) D5).
///
/// **A short list on purpose.** Rust's `const` takes a type and no inference,
/// so a Nikaia type this cannot name is a `comptime` this cannot write - and
/// `None` here becomes `NK1127` rather than a guess. The list grows with D5's
/// stages: a `String` is missing because Rust has no `const String`, and what
/// a literal string would become - a `&'static str` - is a different type from
/// the one Part I gives it, which is a decision rather than a mapping.
fn rust_constant_type(ty: &Ty) -> Option<String> {
    match ty {
        // **`u8` was missing**, which is the byte Part I 2.2 offers and the
        // shape a **buffer** is written in: `comptime B: Array[u8, 3] = [1, 2,
        // 3]` was `NK1127` — *this compiler cannot evaluate it* — for a line
        // Rust writes as `const B: [u8; 3] = [1, 2, 3];`. A correct program
        // refused ([Part III C.4](../../docs/specification/30-nikaia-tooling.md))
        // with a sentence that was not about the program.
        //
        // The rest of the list is what it was. Widening it to every Rust
        // integer would promise a surface Part I 2.2 does not offer, which is
        // the reason `constant_fits` gives for its own range table.
        Ty::Named { name, args, view } if args.is_empty() && !*view => match name.as_str() {
            "i32" | "i64" | "u8" | "u32" | "u64" | "f32" | "f64" | "bool" | "char" => {
                Some(name.clone())
            }
            _ => None,
        },
        // **`&str` is the crossed form of text**
        // ([ADR-079](../../docs/specification/adr/adr-079.md) D1). A `String`
        // allocates and `const X: String` is not a thing; `const X: &str` is,
        // and a `const`'s elision makes its lifetime `'static`.
        Ty::Named { name, args, view } if name == "str" && args.is_empty() && *view => {
            Some("&str".to_string())
        }
        // **`&[T]` is `&[T]`**, and it is the view form
        // [ADR-079](../../docs/specification/adr/adr-079.md) D1 named and
        // [ADR-179](../../docs/specification/adr/adr-179.md) D1 spelled. A
        // `const` promotes the array literal behind it to `'static`, so
        // `const XS: &[i64] = &[1, 2, 3];` needs no lifetime written anywhere.
        //
        // **Where an `Array[T, N]` says the length and this does not**, which
        // is the whole difference between them and the reason both exist: a
        // field of a `struct` whose list is a different length per value has no
        // `Array` to be and is exactly this.
        Ty::Pointed {
            item,
            slice: true,
            mutable: false,
        } => Some(format!("&[{}]", rust_constant_type(item)?)),
        // **`Array[T, N]` is `[T; N]`, and it is the one aggregate a `const`
        // holds** ([ADR-152](../../docs/specification/adr/adr-152.md)). A `Vec`
        // allocates, which is the whole of why a build-time table is an array.
        Ty::Named { name, args, view } if name == ty::ARRAY && !*view => match args.as_slice() {
            [element, Ty::Count(n)] => Some(format!("[{}; {n}]", rust_constant_type(element)?)),
            _ => None,
        },
        _ => None,
    }
}

/// **Whether this body writes `P::fields` anywhere in it**
/// ([ADR-181](../../docs/specification/adr/adr-181.md) D1).
///
/// The **body** and not the bound, because a bound says only that a shape may
/// be asked for: a `[T: Struct]` function that never asks is an ordinary
/// generic one and stays generic in the language below.
///
/// `crate::emit::visit_block` rather than a walk of this file's own, for the
/// reason that function is `pub(crate)` at all: a second walk over one shape is
/// a second thing to keep in step with the AST.
fn walks_the_fields_of(
    parsed: &Parsed,
    body: &crate::ast::Block,
    parameter: &str,
    shape: &str,
) -> bool {
    let mut found = false;
    crate::emit::visit_block(body, &mut |expr| {
        if let Expr::Path(segments) = expr {
            let names: Vec<&str> = segments.iter().map(|s| parsed.text(*s)).collect();
            if matches!(names.as_slice(), [ty, member]
                if *ty == parameter && *member == shape)
            {
                found = true;
            }
        }
    });
    found
}

/// **The name of one specialised copy** — `describe__User`
/// ([ADR-181](../../docs/specification/adr/adr-181.md) D2).
///
/// `pub(crate)` because the emitter writes the same name at the call and at the
/// definition, and two spellings of one name is the defect that arrangement
/// exists to prevent. A double underscore because a `.nika` name may hold one
/// and this is not a name a program can collide with by accident: `describe__User`
/// would have to be **written** to clash, and `NK1148` catches that.
pub(crate) fn specialised(function: &str, on: &str) -> String {
    format!("{function}__{on}")
}

/// The members Part II 10.3 writes: a `struct`'s fields under `[T: Struct]`,
/// an `enum`'s variants under `[T: Enum]`.
const FIELDS: &str = "fields";
const VARIANTS: &str = "variants";

/// The member a shape bound makes reachable, and the type of one element of it.
fn shape_member(bound: &str) -> Option<(&'static str, &'static str)> {
    match bound {
        "Struct" => Some((FIELDS, ty::FIELD)),
        "Enum" => Some((VARIANTS, ty::VARIANT)),
        _ => None,
    }
}

/// What a `&[T]` is a view of, and `None` for anything else.
///
/// **`&mut [T]` is not one.** It is the C boundary's
/// ([ADR-147](../../docs/specification/adr/adr-147.md) D1) and is refused away
/// from it, so a position that may be written through never reaches here — and
/// a `const` that could be written through would not be a `const`.
fn slice_element(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Pointed {
            item,
            slice: true,
            mutable: false,
        } => Some(item),
        _ => None,
    }
}

/// Whether a declared type is the table a `comptime` map crosses as
/// ([ADR-176](../../docs/specification/adr/adr-176.md) D1).
fn is_fixed(ty: &Ty) -> bool {
    matches!(ty, Ty::Named { name, args, view: false } if name == "Fixed" && args.len() == 2)
}

/// Whether a type is one this language grows — a `Vec` or a `List`.
///
/// The pair [ADR-079](../../docs/specification/adr/adr-079.md) D1 is about:
/// growable going in, and a `const` below that cannot hold one.
fn is_growable(ty: &Ty) -> bool {
    matches!(ty, Ty::Named { name, view, .. }
        if name == "Vec" || name == "List" || (name == "String" && !view))
}

/// The Rust element type of a growable list this checker **did** type.
fn element_below(found: &Ty) -> Option<String> {
    match found {
        Ty::Named { name, args, .. } if name == "Vec" || name == "List" => {
            rust_constant_type(args.first()?)
        }
        _ => None,
    }
}

/// The Rust element type of a build-time array nothing declared a type for.
///
/// **Every element has to agree**, which is the same rule
/// [ADR-135](../../docs/specification/adr/adr-135.md) D1 gives a list literal
/// one level up — and an empty one has nothing to read, so it says nothing and
/// the declaration has to.
fn rust_array_type(items: &[build_time::Value]) -> Option<String> {
    let mut found: Option<String> = None;
    for item in items {
        let ty = match item {
            build_time::Value::Bool(_) => "bool".to_string(),
            build_time::Value::Int(value) => match i32::try_from(*value) {
                Ok(_) => "i32".to_string(),
                Err(_) => "i64".to_string(),
            },
            // **A float is an `f64` where nothing declared otherwise**, which
            // is Part I 2.4's widest-holder rule read over the one shape it
            // has two of: `f32` is a *declaration*, never an inference.
            build_time::Value::Float(_) => "f64".to_string(),
            // **Text is a view where it crosses**
            // ([ADR-079](../../docs/specification/adr/adr-079.md) D1), and an
            // array of it is an array of views: `[&str; N]`, which a `const`
            // holds for the same reason `const X: &str` is one.
            build_time::Value::Text(_) => "&str".to_string(),
            // **A struct is its own name below**, which is the same answer the
            // single struct gets one shape in: a declaration the program wrote
            // is a type the language below has, and what a `const` needs is the
            // name. Whether the *fields* can be written is the declaration's
            // question, and `unwritable_field` asks it.
            build_time::Value::Struct { name, .. } => name.clone(),
            // **A variant is its `enum`'s name below**, which is the struct's
            // answer one shape over: the declaration is a type the language
            // below has, and what a `const` needs is the name.
            build_time::Value::Variant { ty, .. } => ty.clone(),
            // **An array of arrays is `[[T; M]; N]`**, and the inner lengths
            // have to agree — a length is part of the type
            // ([ADR-152](../../docs/specification/adr/adr-152.md) D4), so a
            // list of rows that are not the same length is not a list of one
            // type and there is nothing to write. Refused by saying nothing,
            // which reaches `NK1127`; the *value* is fine and it is the
            // **crossing** that has no shape for it.
            build_time::Value::List(inner) => {
                let held = rust_array_type(inner)?;
                format!("[{held}; {}]", inner.len())
            }
            // A tuple's `const` form is the pair it is, and the only place one
            // stands is inside a `Fixed` — where the two halves are written
            // into two tables rather than beside each other.
            build_time::Value::Tuple(_) => return None,
        };
        match &found {
            // `[1, 3_000_000_000]` is an `i64` array and not a mixed one: the
            // widest element decides, which is Part I 2.4's rule read over a
            // list rather than over one literal.
            Some(held) if held != &ty => found = Some("i64".to_string()),
            Some(_) => {}
            None => found = Some(ty),
        }
    }
    found
}

/// A build-time value, spelled as Rust writes one — for the shapes a `const`
/// can hold.
///
/// `None` where one of them cannot be written down, which is what sends the
/// caller to `NK1127`: a field that is a `Vec` has no `const` form, and a
/// struct is only as writable as its fields.
fn rust_value(value: &build_time::Value) -> Option<String> {
    match value {
        build_time::Value::Int(n) => Some(n.to_string()),
        build_time::Value::Float(n) => float_literal(*n),
        build_time::Value::Bool(yes) => Some(yes.to_string()),
        build_time::Value::Text(text) => Some(format!("\"{}\"", build_time::written(text))),
        // A tuple has no `const` form of its own: the one place a pair stands
        // is inside a `Fixed`, which writes its halves into two tables rather
        // than beside each other (ADR-176 D2).
        build_time::Value::Tuple(_) => None,
        // `Shade::Odd`, and `Json::Number(1.5)` where it carries something —
        // which is what a program writes, so it is what a `const` holds.
        build_time::Value::Variant {
            ty,
            variant,
            payload,
        } => {
            if payload.is_empty() {
                return Some(format!("{ty}::{variant}"));
            }
            let mut written = Vec::with_capacity(payload.len());
            for held in payload {
                written.push(rust_value(held)?);
            }
            Some(format!("{ty}::{variant}({})", written.join(", ")))
        }
        build_time::Value::Struct { name, fields } => {
            let mut written = Vec::with_capacity(fields.len());
            for (field, held) in fields {
                written.push(format!("{field}: {}", rust_value(held)?));
            }
            Some(format!("{name} {{ {} }}", written.join(", ")))
        }
        // **Inside a struct a list is spelled and nothing more**: the field's
        // declared type carries the `[T; N]`, so `[1, 2, 3]` is what Rust
        // wants there. Whether the field *may* be a list at all is the
        // declaration's question and `unwritable_field` asks it.
        build_time::Value::List(items) => {
            let mut written = Vec::with_capacity(items.len());
            for held in items {
                written.push(rust_value(held)?);
            }
            Some(format!("[{}]", written.join(", ")))
        }
    }
}

/// Why a `with` was refused — the four shapes
/// [ADR-118](../../docs/specification/adr/adr-118.md) gives one claim.
enum Copyable {
    /// An `enum`: which fields a copy carries depends on the variant (§4).
    AnEnum,
    /// A view: there is nothing here to move out of (D3).
    AView,
    /// A type with no fields this compiler has read.
    NotAStruct,
    /// A type this compiler could not name, and the lowering writes one.
    Unnamed,
}

/// A name in scope: what it is called, the type it holds, and - where this
/// checker could work it out - the constant integer it stands for.
///
/// **Only a `let` ever fills the third**, and only an immutable one whose value
/// folded ([`Checker::constant_of`]). A parameter, a `for` binding, a `match`
/// arm's name and a lambda's argument all name something no compile-time
/// evaluation reaches, so they carry `None` and the fold stops at them - which
/// is the fail-closed direction the refusal needs (ADR-010 D1): a name this
/// checker cannot evaluate makes the whole expression unevaluable, and an
/// unevaluable expression is never refused.
struct Local {
    name: String,
    ty: Ty,
    constant: Option<i128>,
    /// **The binding is a view the emitter lent**, which a `for` over a place
    /// is ([ADR-094](../../docs/specification/adr/adr-094.md) D4).
    ///
    /// One reader: a **cast** over it. `for n in NS { n as i64 }` reaches the
    /// language below as `n as i64` where `n` is a `&i32`, and `casting &i32
    /// as i64 is invalid` is a sentence about a noun the program does not
    /// contain ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// On the binding and not in a map, for the reason `immutable` gives one
    /// field down: the **scope** is the one `scope` already keeps, so a `let n
    /// = 5` inside the loop stops being a view exactly where it stops being
    /// the answer.
    lent: bool,
    /// Where the binding was written and what kind it is, when it did **not**
    /// say `mut` — `None` for one a change to is nobody's business here.
    ///
    /// It lives on the binding rather than in a map of its own so that the
    /// **scope** is the one `scope` already keeps: a `let` inside a block stops
    /// being the answer when the block closes, and an outer `mut` name is the
    /// answer again. A parallel map would have had to be pushed and popped at
    /// twenty-eight places, and getting one of them wrong is a *correct
    /// program refused* ([Part III
    /// C.4](../../docs/specification/30-nikaia-tooling.md)).
    immutable: Option<Immutable>,
    /// **A parameter whose declaration wrote `mut`**, which is already a
    /// `&mut T` in the lowering ([ADR-094](../../docs/specification/adr/adr-094.md)
    /// D3).
    ///
    /// Not the same question as `immutable` being `None`. A `let mut` is a
    /// *value*, and the `&mut` a call writes in front of one is the reference
    /// that call means; this is the binding where the reference is already
    /// there, so a second one would be `&mut &mut T` — and `rustc` refuses it
    /// one step earlier than that, because a `&mut` may only be taken of a
    /// binding that is itself `mut`. It said so about a file nobody wrote —
    /// *cannot borrow `connection` as mutable* — which is [Part III
    /// C.1](../../docs/specification/30-nikaia-tooling.md).
    ///
    /// **A bare name and nothing further in.** `&mut c.field` for a `mut c` is
    /// the reference the call wants, so this answers a question about the whole
    /// binding rather than about any place rooted at it.
    ///
    /// On the binding for `lent`'s reason: the scope is the one `scope` already
    /// keeps, so a `let connection = …` shadowing a `mut connection` parameter
    /// stops being the answer where it stops being the binding.
    changing: bool,
    /// **What the build worked out this name is**, whole — a `comptime`'s
    /// value, where there is one.
    ///
    /// `constant` above is the fold's and is an integer, which was the whole of
    /// what a build-time value could be. It is not any more: text, a list and a
    /// `struct` are values too, and a `comptime` that reads another one needs
    /// the value rather than the number it would have been. Kept beside
    /// `constant` rather than replacing it, because the fold asks a narrower
    /// question — *which integer type did a declaration pin* — that this does
    /// not answer.
    built: Option<build_time::Value>,
    /// **The text literal a `let` bound this name to**, where it did — for the
    /// one sentence that has to say so: `let s = "x"` and then `Person { name:
    /// s }` is refused, and a reader asks why the compiler that builds `"x"`
    /// into a `String` on that line does not do the same through `s`
    /// ([ADR-208](../../docs/specification/adr/adr-208.md) D2).
    literal: Option<String>,
    /// Where the `let` stands, for a binding whose value was an **empty list**
    /// and whose element type nothing has said yet
    /// ([ADR-135](../../docs/specification/adr/adr-135.md) D2).
    ///
    /// On the binding rather than in a map, for the reason `immutable` gives:
    /// the scope is the one `scope` already keeps, so an inner `let xs = []`
    /// and an outer `xs` are two questions and not one.
    empty_list: Option<usize>,
}

/// A binding that did not say `mut`: where it was written, and which of the two
/// kinds it is.
///
/// The two are one rule reached from two sides — *what is changed says `mut`* —
/// and they are two codes because the way out is written in a different place
/// and a reader is doing a different thing. `NK1138` is a **parameter**'s
/// ([ADR-094](../../docs/specification/adr/adr-094.md) D3), where the word also
/// decides what the caller sees; `NK1139` is a **`let`**'s, which [Part I
/// 2.1](../../docs/specification/10-nikaia-light.md) states outright and which
/// `rustc` had been answering instead.
#[derive(Clone)]
struct Immutable {
    at: Span,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Parameter,
    Let,
}

impl Local {
    /// A binding nothing here refuses a change to: `self`, an option, a `for`
    /// binding, a pattern's name. Each of those is either not a place a
    /// program assigns to or one whose `mut` is a question of its own, and
    /// silence is what [Part III
    /// C.4](../../docs/specification/30-nikaia-tooling.md) asks for.
    /// The same binding again, for the one frame walked twice (a constructor's
    /// parameters, checked once as written and once as `Type::new`).
    fn again(local: &Local) -> Self {
        Local {
            name: local.name.clone(),
            ty: local.ty.clone(),
            constant: local.constant,
            lent: local.lent,
            built: local.built.clone(),
            literal: local.literal.clone(),
            immutable: local.immutable.clone(),
            changing: local.changing,
            empty_list: local.empty_list,
        }
    }

    fn free(name: String, ty: Ty) -> Self {
        Local {
            literal: None,
            name,
            ty,
            constant: None,
            lent: false,
            changing: false,
            built: None,
            immutable: None,
            empty_list: None,
        }
    }
}

struct Checker<'a> {
    /// What each callee has **newly** gained since the committed ledger
    /// ([ADR-101](../../docs/specification/adr/adr-101.md) D1). Empty where
    /// there is nothing to compare, which says nothing.
    newly: &'a Newly,
    parsed: &'a Parsed,
    /// **The program's other files**, for the one question that needs a body
    /// rather than a contract — a `comptime` calling across a file boundary.
    ///
    /// Empty for a caller that has one file, which is every test and every
    /// `--input` outside a project. It carries `Parsed` and not just items,
    /// because each one owns the interner its symbols resolve in.
    beside: &'a [&'a Parsed],
    /// **What this build may read while it builds** (ADR-072), carried beside
    /// the files for the same reason: no ledger can say it.
    reads: &'a Reads,
    /// The rings of constants this walk has already reported, by their members.
    ///
    /// Every constant in a ring is circular, and each would report the same
    /// loop from a different corner — which is one mistake said as many times
    /// as it has members.
    said_rings: BTreeSet<Vec<String>>,
    /// This unit's own contracts, inferred from the source being checked.
    own: &'a Ledger,
    /// `std`'s, as `std` ships them.
    library: &'a Ledger,
    /// Every struct declared here, with its fields. A type whose fields are
    /// not known is simply absent, and an absent type is never an error.
    structs: BTreeMap<String, Vec<FieldContract>>,
    /// Every enum declared here, with its variant names.
    enums: BTreeMap<String, BTreeSet<String>>,
    /// What each declared `enum`'s variants **hold**, flattened across them.
    ///
    /// A `Named` variant's fields are in `structs` under the qualified key,
    /// because a struct literal wears that shape; a **positional** one's are in
    /// no map at all, which is what made `Json::Number(f64)` look like a variant
    /// holding nothing. This is the one question that needs all of them at once —
    /// does every part of this type compare — so it is a map of its own rather
    /// than a second meaning for `structs`.
    enum_payloads: BTreeMap<String, Vec<Ty>>,
    /// **What each positional variant of a declared `enum` holds**, by
    /// `Enum::Variant` and in order: what a `match` arm binds by position
    /// (`Shape::Named(n)` binds a `String`). A variant with named fields is in
    /// `structs`, as a library's variants are.
    variant_parts: BTreeMap<String, Vec<Ty>>,
    /// **The functions whose body walks a type's fields**, and which type
    /// parameter each one walks ([ADR-181](../../docs/specification/adr/adr-181.md)
    /// D1): `describe` → `T`.
    ///
    /// Collected before any body is walked, because a call may stand **above**
    /// the function it names - items are order-independent here - and what a
    /// call has to do about one of these is decided at the call.
    walks_fields: BTreeMap<String, String>,
    /// The fields the **current** unrolling is over, where this walk is one
    /// ([ADR-088](../../docs/specification/adr/adr-088.md) D5): the concrete
    /// type's name and the field this turn stands at.
    ///
    /// `None` in the generic body, which is walked once with nothing known -
    /// so `field.of(value)` answers `?` there and refuses nothing, and the
    /// refusals that matter come from the unrolled walks.
    unrolling: Option<(String, FieldContract)>,
    /// **This walk is one unit harvesting another's call sites**, so it does
    /// not harvest in turn ([ADR-181](../../docs/specification/adr/adr-181.md)
    /// D2).
    ///
    /// Which types a shape walk was used with is a fact about the **program**
    /// and not about the file the function stands in, and the calls may all
    /// stand somewhere else - so the unit that writes the copies asks the
    /// others. Without this flag that question asks itself back.
    harvesting: bool,
    /// `Shape::Spot` → `Shape`, for every variant that carries **named**
    /// fields — the ones a struct literal builds.
    ///
    /// Two facts at one key: that this name is a variant and not a type, and
    /// which `enum` a literal for it has. [`Self::structs`] holds its fields
    /// under the same key, so one field check serves both shapes.
    variant_owner: BTreeMap<String, String>,
    /// Every **grammar** declared here, with the names of its `pub` rules
    /// ([ADR-082](../../docs/specification/adr/adr-082.md) D1, D2).
    ///
    /// A grammar is entered by an ordinary call — `Json.value(input)` — so its
    /// name has to be something `NK1117` counts as declared, and which rules
    /// may stand after the dot is what says `Json.internal(x)` is not an entry.
    grammars: BTreeMap<String, BTreeSet<String>>,
    /// Names in scope, innermost frame last.
    scope: Vec<Vec<Local>>,
    /// **Whether the walk is inside a `comptime` initialiser**
    /// ([ADR-116](../../docs/specification/adr/adr-116.md) D2).
    ///
    /// `asset("…")` is the compiler's name for the file a build reads, and it
    /// stands there and nowhere else. The evaluator answers it where it
    /// belongs; this says where the ordinary walk must keep quiet, so that two
    /// walks over one expression do not both have an opinion about one call.
    inside_a_comptime: bool,
    /// What the function being walked declared it hands back.
    expected: Option<Ty>,
    /// The function being checked hands back text **both kinds** of which
    /// flow into its result ([ADR-222](../../docs/specification/adr/adr-222.md)
    /// D3): text of its own is moved in as it is, so it is not a view refused.
    expected_either: bool,
    /// The same for the field whose value is being checked.
    field_either: bool,
    /// The type parameters in scope where the body being walked stands - the
    /// function's own `[T]` and the `[T]` of the `impl` around it.
    ///
    /// A name in here is a **type** while the body is walked and a **variable**
    /// at every call site ([ADR-074](../../docs/specification/adr/adr-074.md)
    /// D1). Empty everywhere else, which is every function written today.
    type_parameters: BTreeMap<String, Vec<String>>,
    /// Every generic struct declared here, with its parameters in **declaration
    /// order** - which is what makes a type argument's position mean something.
    ///
    /// Absent for a struct with no parameters, which is every struct written
    /// today, so the two questions below cost a failed lookup and nothing else.
    struct_parameters: BTreeMap<String, Vec<String>>,
    /// Whether the method being walked took its subject by **reference**.
    ///
    /// `&self` and `&mut self` borrow it; a bare `self` owns it. What hangs on
    /// the difference is whether a field may be handed out by value at all
    /// ([ADR-083](../../docs/specification/adr/adr-083.md)). `false` for a free
    /// function, which has no subject to borrow.
    borrowing_self: bool,
    /// The `[T]` of the `impl` whose methods are being walked, on its own.
    ///
    /// Separate from the field above because `function` rebuilds that one per
    /// method and has to start from what the `impl` put in scope rather than
    /// from nothing.
    enclosing: BTreeMap<String, Vec<String>>,
    /// **What the `impl` header writes in its own brackets**, in declaration
    /// order - `[T]` for `impl Holder[T]`, `[i64]` for `impl Holder[i64]`.
    ///
    /// The receiver's type is built from this rather than from the bare name,
    /// and that is what lets a field reach the body as something
    /// ([ADR-074](../../docs/specification/adr/adr-074.md) D1). `self` typed as
    /// a bare `Holder` binds none of the struct's parameters, so `self.value`
    /// substituted a `T` nothing had bound and came out `?` - and a `?` is the
    /// one thing this checker says nothing about. Every refusal a `T` earns as
    /// a *parameter* - `NK1126` for a member on it, `NK1104` for a slot it does
    /// not fit, `NK1131` for handing one out of a borrowed subject - was silent
    /// through a field, and the program went to `rustc` about the generated
    /// file, which is [Part III C.1](../../docs/specification/30-nikaia-tooling.md)'s
    /// class.
    ///
    /// Empty outside an `impl`, and empty for one whose target takes no
    /// arguments - which is the same as what it was before.
    subject_arguments: Vec<Ty>,
    /// The bounds each function's parameters were declared with, by the key a
    /// call resolves to - `tell` for a free function, `Dog::tell` for a method.
    ///
    /// Not read off the ledger, which has no column for a bound: its signature
    /// writes `(x: $T) -> String` and the `: Speaks` is nowhere in it. So what
    /// this table reaches is a call to a function of **this build** — every unit
    /// of it is walked, so every such `fn` is in one of these ASTs — and a call
    /// into a package whose generic function carries a bound is not checked
    /// against it. That is the column a ledger would need, and nothing asks for
    /// it yet: a bound may name a path since
    /// [ADR-106](../../docs/specification/adr/adr-106.md) D1, and what a caller
    /// gets wrong is caught where the callee's body reads the parameter.
    declared_bounds: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    /// Whether it declared `throws` - which is what says a failure may leave
    /// it, whether the failing call was written or implicit (ADR-025 D1).
    throwing: bool,
    /// Whether the expression being walked is the guarded half of a `catch`.
    ///
    /// `fs::read_to_string(p, fs::Root::Anywhere) catch { … }` handles the failure where it
    /// happens, so nothing leaves the function and `NK2605` has nothing to
    /// say. It covers the **whole** guarded expression, because that is what
    /// the handler runs for: in `outer(inner())` both calls are caught. The
    /// handler's own body is not - a failure raised there propagates - so this
    /// goes back to what it was before the handler is walked.
    caught: bool,
    /// The handler being walked was handed an error that can be **more than
    /// one type** ([ADR-160](../../docs/specification/adr/adr-160.md) D4).
    ///
    /// `match error { … }` reads it. The set of error types arriving at a
    /// `catch` is **open** ([ADR-023](../../docs/specification/adr/adr-023.md)
    /// D4), so no `match` over it can be exhaustive — which is why the question
    /// is about the binding rather than about a type this checker could name.
    caught_several: bool,
    /// The **one** error type arriving at the nearest `catch`, where exactly one
    /// does — and `None` where several do, where none is written down, or
    /// outside a handler.
    ///
    /// **For one question only**: whether `match error { … }` covers every case.
    /// The binding itself stays `Ty::Unknown`, because giving a binding a type
    /// where it had none can turn a program that compiles into one that is
    /// refused, and [Part III
    /// C.4](../../docs/specification/30-nikaia-tooling.md) makes that its own
    /// piece of work with its own sweep. This one can only *move* a refusal:
    /// where the arms miss a case, the words were `rustc`'s about a generated
    /// file (*non-exhaustive patterns: `IoError::NotText(_)` not covered*),
    /// which is [Part III C.1](../../docs/specification/30-nikaia-tooling.md);
    /// where they cover everything, nothing was said before and nothing is said
    /// now.
    caught_one: Option<Ty>,
    /// What the guarded expression of the nearest `catch` turned out to hold,
    /// while that expression is being walked - and `None` everywhere else.
    ///
    /// The mirror of [`Self::caught`], asked in the other direction: that one
    /// says *a failure here needs no `throws`*, this one says *was there a
    /// failure at all*. A `catch` over an expression that cannot fail lowers to
    /// a `match` over something that is not a `Result`, which is `NK1134`
    /// ([ADR-091](../../../docs/specification/adr/adr-091.md)).
    guarded: Option<Guarded>,
    /// How many loops are open around the statement being walked, counted from
    /// the nearest boundary a jump may not cross rather than from the function.
    ///
    /// A `break` is legal where this is not zero, and `NK1132` is what it meets
    /// where it is (Part I 3.3,
    /// [ADR-084](../../../docs/specification/adr/adr-084.md) D4).
    loops: usize,
    /// What that nearest boundary is, where one stands between here and the
    /// function's own body: a lambda, a task, an `overlap` branch.
    ///
    /// Each of those is **a function of its own in the language below**, and a
    /// jump does not leave a function - so a `break` inside one whose loop is
    /// outside it is not a program this compiler may lower
    /// ([ADR-084](../../../docs/specification/adr/adr-084.md) D4). It is carried
    /// rather than derived because the message is the whole value of catching
    /// it here: without the word, the refusal would be `rustc`'s, about a file
    /// nobody wrote (Part III, C.1).
    ///
    /// **A `catch` handler is deliberately not one of these** (D5): it lowers to
    /// a `match` arm, and a jump in one reaches the loop around it.
    barrier: Option<&'static str>,
    /// The function being walked, by the name the ledger records it under.
    ///
    /// `None` inside a grammar action, a `test` or a `bench` - code that
    /// belongs to no function a caller can name, and whose method calls
    /// therefore have nowhere to be recorded.
    current: Option<String>,
    /// The modules this program is made of (Part I, 9.1). Empty for a single
    /// file, where no call crosses a file boundary.
    modules: BTreeSet<String>,
    /// Method calls whose callee's contract says it can fail, and method calls
    /// where that could not be established - both as the pair
    /// [`Checked::fallible_methods`] is keyed by. The difference is the answer;
    /// a name that is both in one statement is in neither.
    fallible_methods: BTreeSet<(usize, String)>,
    opaque_methods: BTreeSet<(usize, String)>,
    /// The same pair of sets for [`Checked::pausing_methods`]: the calls whose
    /// callee pauses, and the ones where it does not or could not be
    /// established. The difference is the answer.
    pausing_methods: BTreeSet<(usize, String)>,
    settled_methods: BTreeSet<(usize, String)>,
    /// **What the lambda being walked has been seen to do**
    /// ([ADR-102](../../docs/specification/adr/adr-102.md) D2).
    ///
    /// A function type carries two promises and the defaults are the
    /// language's: without `sync` the code may pause, without `throws` it
    /// cannot fail. A lambda handed to one has to keep them, and *keeping* is
    /// a question about its body — so the body is walked with this in hand and
    /// the calls it makes put their answers in it.
    ///
    /// **It does not cross a boundary.** A lambda written inside another one
    /// is a function of its own: what its body does happens when *its* callee
    /// runs it, not when this one does. So [`Checker::past_a_boundary`] clears
    /// it and puts the outer one back, the way it does with the loop count.
    ///
    /// `None` where nothing is being asked — outside a lambda, and inside one
    /// whose parameter's type nothing describes.
    handed_over: Option<Handed>,
    /// The lambdas of the call being walked that paused and were handed to a
    /// `std` entry, by their shape
    /// ([ADR-233](../../docs/specification/adr/adr-233.md) D1). Filled by
    /// [`Self::arguments_given`] and emptied by the call that asked.
    paused_args: Vec<String>,
    /// **Inside a lambda handed to a walk of a `Par[T]`**, the length of the
    /// scope where it began: a name bound below it is one every core shares
    /// ([ADR-235](../../docs/specification/adr/adr-235.md) D2). `None`
    /// elsewhere.
    in_parallel: Option<usize>,
    /// **The names an `extern "C"` block declares**
    /// ([ADR-124](../../docs/specification/adr/adr-124.md) D3).
    ///
    /// Collected from the item tree rather than read off the ledger, because
    /// the ledger records what a name *is* and this asks where it **came
    /// from**: a Nikaia function and a C declaration are both entries, and only
    /// one of them has to be called inside an `unsafe` block.
    foreign_names: BTreeSet<String>,
    /// **The opaque handles this file declares**
    /// ([ADR-147](../../docs/specification/adr/adr-147.md) D3), by name.
    ///
    /// A handle is an address the language never dereferences, so it has no
    /// fields and no indexing — and both are refused here rather than left to
    /// the language below, where the words would be about a file nobody wrote
    /// (Part III, C.1).
    opaque_handles: BTreeSet<String>,
    /// Whether what is being walked stands inside one.
    ///
    /// It reaches inward the way `caught` and `in_lambda` do, and it stops at
    /// nothing: a lambda written inside an `unsafe` block runs later, and D3
    /// asks about where the **call** is written rather than about when it runs.
    inside_unsafe: bool,
    /// **What a task took with it** (`NK2101`): the name, its type, and the byte
    /// the `spawn`'s statement starts at.
    ///
    /// Collected while the function is walked and answered at the end of it,
    /// because the question is about what comes *after* the `spawn` and a
    /// single pass reaches a later statement later. Cleared per function: a
    /// name is a name of one body.
    /// The `spawn` statement's own **end** is what is kept, not its start: the
    /// reads inside the task's body are the move itself and lie inside that
    /// span, so "after the task" means after the statement closes.
    moved_into_a_task: Vec<(String, Ty, usize)>,
    /// **A name whose sequence was walked** (`NK2702`,
    /// [ADR-105](../../docs/specification/adr/adr-105.md) D2): the name, the type
    /// it held, and the byte the walking statement **ends** on.
    ///
    /// `moved_into_a_task`'s shape and for its reason: the question is about what
    /// comes *after* the walk, and a single pass reaches a later statement later.
    /// The statement's end and not its start, because the walk itself is a read
    /// inside that statement.
    walked: Vec<Taken>,
    /// **Which branch of which `if` or `match` the walk is in**, outermost
    /// first ([ADR-212](../../docs/specification/adr/adr-212.md) D4).
    ///
    /// A sequence taken in the `then` of an `if` and read in its `else` was
    /// taken on neither's way to the other, and `NK2702` ordered by statement
    /// alone refused that correct program. Two places are on one path unless
    /// they sit in different arms of one choice.
    branch: Choices,
    /// The next number to give a choice: one per `if` and per `match` walked.
    next_choice: usize,
    /// Every read of a name or a part of one, in the order the walk met them
    /// (`NK2702`'s and `NK2105`'s reads, [`Read`]).
    reads_on_paths: Vec<Read>,
    /// Which read an expression **is**, by the expression's address, so that
    /// `p.name` extends the read of `p` it stands on and a hand-over names the
    /// read that was handed ([ADR-214](../../docs/specification/adr/adr-214.md)
    /// D1). Also what keeps an expression walked twice from counting as two
    /// reads.
    read_index: std::collections::HashMap<(usize, usize), usize>,
    /// The `let`s whose name is a **view** of a place (ADR-094 D4), by name and
    /// the frame it was bound in: a part of one cannot be handed over.
    lent_lets: BTreeSet<(String, usize)>,
    /// **Data handed to something that keeps it**
    /// ([ADR-213](../../docs/specification/adr/adr-213.md) D3): the name, its
    /// type, the byte the handing statement ends on, the branch, and the words
    /// for what took it. `NK2105` is a later read on the same path.
    handed: Vec<Taken>,
    /// **Names a method changed or took, or an assignment wrote**, in the
    /// order they were met: what a `match` arm does to a part other than
    /// read it (ADR-242).
    changed: Vec<String>,
    /// Set by an assignment whose target is an index, for the one `Index` it
    /// walks: a key **written** is handed to the map, a key read is lent
    /// (ADR-213 D1).
    writing_index: bool,
    /// Set where an `Index` read a **map**, for the `??` around it: a map read
    /// hands out a view of the value, whatever the value's type (ADR-213 D2).
    read_a_map: bool,
    /// The statement and the text of the f-string hole being walked, for
    /// `text_at`.
    hole: Option<(usize, String)>,
    /// The parameters of the function being walked that are code it only
    /// runs, which stay closure arguments below.
    run_code: BTreeSet<String>,
    /// The ledger key the last method call resolved to, for the arm around it
    /// (ADR-215 D4).
    last_resolved: Option<String>,
    /// The **name** of the receiver of the method call being walked, where it is
    /// a plain name ([ADR-105](../../docs/specification/adr/adr-105.md) D2).
    ///
    /// `set_receiver`'s arrangement and for its reason: the rule wants the name
    /// and runs in `call_on`, where the contract is in hand and the receiver's
    /// expression is not. `None` for a temporary — `map.keys().collect()` walks
    /// one, and a temporary has no second use to refuse.
    receiver_name: Option<String>,
    /// Every place a local name was read, by the byte its statement starts at.
    ///
    /// The other half of the same question. Reads and not writes: an assignment
    /// *revives* a name the task took - `message = "other"` after the `spawn` is
    /// a correct program - so those are collected separately.
    read_at: Vec<(String, usize)>,
    /// Every **path** assigned to - a name, or a part of one, `p.name` - by
    /// the byte its statement starts at.
    written_at: Vec<(String, usize)>,
    /// **The loops and lambdas around the expression being walked**, innermost
    /// last ([ADR-212](../../docs/specification/adr/adr-212.md) D4): where each
    /// begins in `scope`, and what would let a sequence taken inside it be
    /// taken again.
    ///
    /// A sequence declared outside a loop and taken inside it is taken on the
    /// first turn and gone on the second, and a lambda may be run more than
    /// once by whatever it is handed to. Both are one statement in the source,
    /// which is why the per-statement order `walked` reads cannot see them.
    repeats: Vec<Repeats>,
    /// A read that does not take what it reads
    /// ([ADR-212](../../docs/specification/adr/adr-212.md) D4): an
    /// assignment's target, and the operand of a `&`.
    reading_only: bool,
    /// How many reads the walk has met so far: each read's place in the order.
    read_seq: usize,
    /// The `let`s in the body being walked whose value was an **empty list**
    /// and whose element type nothing has said yet
    /// ([ADR-135](../../docs/specification/adr/adr-135.md) D2), by the `let`'s
    /// byte: the name, and where to put the caret.
    ///
    /// An entry **leaves** when the name is read, because a use is what gives
    /// an empty list its element type; what is left when the body ends is a
    /// list nothing will ever constrain, and that is `NK1153`.
    empty_lists: BTreeMap<usize, (String, Span)>,
    /// The names this function's body gives text of its own later on, with
    /// `+` or an f-string ([`grown_text`]): a `let mut` of a literal under one
    /// of them holds owned text (0.0.232).
    grown_text: BTreeSet<String>,
    /// The conversions that do **not** narrow, so a statement holding one of
    /// those beside a narrowing one to the same type is left alone entirely.
    widening_casts: BTreeSet<(usize, String)>,
    /// The `let`s made inside each enclosing `spawn` body, innermost last: the
    /// name, its type, and the byte its statement starts at
    /// ([ADR-055](../../docs/specification/adr/adr-055.md) D6).
    ///
    /// **`NK2501` asks about what a task *captures*; this is what it *binds*.**
    /// A value bound inside the body and still live at a suspension point
    /// further down is held **inside the future**, so it crosses a thread just
    /// as a captured one does — and the pool's starter asks for `Send` of the
    /// whole future. Nothing in the frame `scope` pushes for the body survives
    /// the walk, so the types are collected here while they are known.
    ///
    /// A stack, because a task may `spawn` another: each body's bindings are
    /// its own.
    task_bindings: Vec<Vec<(String, Ty, usize)>>,
    /// Whether the lambda being walked is a **write door**'s block — `update`
    /// or `update_all` ([ADR-110](../../docs/specification/adr/adr-110.md) D1,
    /// D6).
    ///
    /// It is what narrows `NK1138` at a lambda parameter to where D1 asks for
    /// it. Everywhere else a parameter without the word is left alone, because
    /// the emitter writes `mut` itself for a fold's accumulator and refusing
    /// there would refuse a program that compiles.
    at_a_write_door: bool,
    /// The name the `set` being typed was called on, for `NK2205`'s message
    /// ([ADR-111](../../docs/specification/adr/adr-111.md) D4).
    ///
    /// The rule wants the receiver's **name** and the argument's **type**, and
    /// those are known in two different places: the name where the method call
    /// is walked, the types after `arguments_given` has run. Carried rather
    /// than passed, the way the door flags beside it are.
    set_receiver: Option<String>,
    /// Where the condition this stands under was read from a lock, if it was
    /// ([ADR-111](../../docs/specification/adr/adr-111.md) D4's second shape).
    ///
    /// `if stand > 100 { kasse.set(0) }` stores a plain value; the **decision**
    /// is the stale thing. A `while`, a `match` and a nested `if` are
    /// conditions alike, which is why this is a flag over a block rather than a
    /// question asked of one statement.
    stamped_condition: Option<usize>,
    /// Whether what is being walked is **inside a door's block** — `access`,
    /// `update`, `access_all` or `update_all`
    /// ([ADR-039](../../docs/specification/adr/adr-039.md) D10).
    ///
    /// `at_a_write_door` above is the narrower question `NK1138` and `NK1141`
    /// ask; this is the one `NK2203` asks, because *a lock taken while a lock
    /// is held* is about **any** door being open and not about which
    /// ([ADR-039](../../docs/specification/adr/adr-039.md) D2).
    ///
    /// `get` and `set` are doors and are **not** here, and D10 says why: while
    /// the lock is open in either of them no code of the program's runs, so
    /// there is nothing that could take a second one.
    inside_a_door: bool,
    /// The lambdas being walked on their way into a function field, innermost
    /// last (ADR-230 D1): every method call resolved inside one is recorded in
    /// each.
    stored_frames: Vec<StoredCode>,
    /// The rule whose action is being walked, for
    /// [ADR-142](../../docs/specification/adr/adr-142.md) D1's refusal - which
    /// needs the rule's name, because a grammar is a page of rules and a caret
    /// on a call inside one is not enough to find it.
    inside_an_action: Option<String>,
    /// The name of the enclosing function, where its declaration **writes**
    /// `sync` ([ADR-027](../../docs/specification/adr/adr-027.md) D4: an
    /// assertion is checked, never overwritten).
    ///
    /// It is here because `NK2202` cannot see a **method** call: `contracts::sync`
    /// deliberately does not resolve one, on the ground that the type checker is
    /// the only thing that knows what `tx.send(1)` goes to
    /// ([ADR-028](../../docs/specification/adr/adr-028.md)) — and the type
    /// checker is here. Without it a `sync` function that sends on a channel
    /// lowered to an ordinary `fn` with an `.await` inside it, which is not Rust
    /// (Part III, C.1). Found by
    /// [ADR-149](../../docs/specification/adr/adr-149.md) D2, and reachable
    /// before it through `TaskHandle::join`.
    inside_a_sync_function: Option<String>,
    /// The `std` modules this file wrote a `use std::…` for
    /// ([ADR-154](../../docs/specification/adr/adr-154.md), Part I 9.1's
    /// *a prefix is introduced before it is used*).
    std_in_scope: BTreeSet<String>,
    /// Every module `std`'s ledger has entries in — the prefix of a key that is
    /// not a type's name.
    ///
    /// Read off the ledger and not written here, so a `std` that grows a module
    /// needs no edit in this file. `fs::Mapped` is a **type in** a module, so
    /// `fs` is one of these and `HashMap` is not.
    std_modules: BTreeSet<String>,
    /// The bindings `NK1138` and `NK1139` have already been said about, by the
    /// byte each declaration starts at.
    ///
    /// Said once per binding: a body that changes one usually does so several
    /// times, and three carets on one declaration is noise rather than
    /// information. It is not scoped, and must not be — two blocks that each
    /// declare an `xs` have two spans, and one that is walked twice is one
    /// span and one message.
    said_mut: BTreeSet<usize>,
    checked: Checked,
}

/// What a lambda's body was seen to do
/// ([ADR-102](../../docs/specification/adr/adr-102.md) D2).
#[derive(Debug, Clone, Copy)]
struct Handed {
    pauses: bool,
    fails: bool,
    /// What the type it was handed to allows, kept beside what it was seen to
    /// do so that the one message about a mismatch is the one that is said.
    promised: Promises,
}

/// What a function type **allows** the code it names to do (D2), which is the
/// reading a declaration already has: without `sync` it may pause, without
/// `throws` it cannot fail.
#[derive(Debug, Clone, Copy)]
struct Promises {
    may_pause: bool,
    may_fail: bool,
}

/// One argument as [`Checker::the_compiler_writes_the_reference`] reads it:
/// where it stands in the call, what was written there, the type that was
/// found, and the one the parameter wants. Four values that only ever travel
/// together, and a name for them so the rule reads as one question.
struct Argument<'a> {
    /// The position among the arguments, the receiver not counted.
    at: usize,
    /// What the source wrote there, where there is an expression to read.
    given: Option<&'a Expr>,
    /// The type of what was written.
    found: &'a Ty,
    /// The type the parameter declares.
    want: &'a Ty,
}

/// What the guarded expression of a `catch` turned out to hold.
///
/// Two answers and not one, because *nothing can fail here* and *nothing here
/// could be looked up* are different facts and only the first may be acted on.
/// [Part III C.4](../../../docs/specification/30-nikaia-tooling.md): a call no
/// ledger describes says nothing about whether it throws, and refusing a
/// `catch` over one would refuse a program that is right. The refusal fires on
/// **known not to fail**, never on *not known to fail* - which is
/// [`Checker::may_fail_here`]'s own standard, read from the other end.
#[derive(Default)]
struct Guarded {
    /// A call the ledger describes, whose contract carries a `throws`.
    fallible: bool,
    /// Something this compiler could not look up, or a nested `catch` - either
    /// way, no answer, and the refusal stays quiet.
    unanswered: bool,
}

/// Whether a body's last statement is a loop no jump leaves
/// ([ADR-093](../../../docs/specification/adr/adr-093.md)).
///
/// [ADR-070](../../../docs/specification/adr/adr-070.md) D3: a function that
/// genuinely never returns — an accept loop, an event loop, a supervisor — had
/// to end with a `return 0` that cannot be reached, and a reader of that line
/// could not tell dead code from a mistake.
///
/// **The literal `true` only**, never a name that happens to be true: the
/// equivalence is D1's and it is about the written form. `emit` makes exactly
/// this shape Rust's `loop`, which is `!` and fits any declared type
/// ([ADR-085](../../../docs/specification/adr/adr-085.md)) — so the two halves
/// agree by construction, and the checker could not have claimed this before
/// that record, because `while true { }` is `()` below and the refusal would
/// only have moved to `rustc`.
///
/// **And no `break` bound to this loop**, which is what
/// [ADR-084](../../../docs/specification/adr/adr-084.md) added to the question:
/// before it, the condition was the whole test. The walk over-approximates — it
/// descends into a lambda, where a `break` is not bound to this loop at all —
/// and that is the safe direction: a false *"a jump leaves it"* asks for the
/// `return` this record removes, which is where every program already is, while
/// a false *"nothing leaves it"* would let a body fall off its end and hand
/// `rustc` a file nobody wrote.
fn never_ends(body: &Block) -> bool {
    let Some(last) = body.stmts.last() else {
        return false;
    };
    let Stmt::While { cond, body } = &last.node else {
        return false;
    };
    matches!(cond, Expr::LitBool(true)) && !a_jump_leaves(body, 0)
}

/// Whether a `break` in this block is bound to the loop **around** it rather
/// than to one written inside.
///
/// A `continue` is not one: it starts the loop's next turn and never leaves.
fn a_jump_leaves(block: &Block, loops: usize) -> bool {
    block.stmts.iter().any(|stmt| match &stmt.node {
        Stmt::Break => loops == 0,
        Stmt::For { body, .. } | Stmt::While { body, .. } => a_jump_leaves(body, loops + 1),
        other => {
            let mut inner: Vec<&Block> = Vec::new();
            crate::contracts::sync::visit_stmt_blocks(other, &mut |b| inner.push(b));
            inner.iter().any(|b| a_jump_leaves(b, loops))
        }
    })
}

/// **What a `?.` reaches**, which Part I 3.5 calls a *member*
/// ([ADR-066](../../../docs/specification/adr/adr-066.md)).
///
/// The two spellings are one operator, and the only place the difference has to
/// be said out loud is the refusal: a reader who wrote a call is owed `.m(…)`
/// as the way out and not `.m`.
enum Reached<'a> {
    Field(&'a str),
    Method(&'a str),
}

/// **Something taken**: a sequence walked (`NK2702`) or data handed to what
/// keeps it (`NK2105`).
struct Taken {
    /// A name, or a part of one: `p.name` ([ADR-214](../../docs/specification/adr/adr-214.md) D2).
    path: String,
    ty: Ty,
    /// The bytes the statement that took it starts and ends at.
    from: usize,
    at: usize,
    /// **Where in that statement**: the order the walk read it in, so a later
    /// read in the same statement is after it (ADR-214 D1).
    seq: usize,
    /// The branch it was taken in.
    choices: Choices,
    /// Where a branch that left the function ends, for a taking inside it:
    /// nothing after that is on its path (ADR-213 D4).
    until: Option<usize>,
    /// What took it, in words (`NK2105`).
    to: String,
}

/// **One read**: of what, in which statement, on which path, in which order.
struct Read {
    path: String,
    at: usize,
    choices: Choices,
    seq: usize,
}

/// An expression's address, which is what tells two reads of one name apart
/// in one statement (ADR-214 D1).
fn address(expr: &Expr) -> usize {
    expr as *const Expr as usize
}

/// Whether reading `read` touches what taking `taken` took: the same path, a
/// part of it (`p.name` after `p`), or the whole it was part of (`p` after
/// `p.name`).
fn overlaps(read: &str, taken: &str) -> bool {
    read == taken
        || read
            .strip_prefix(taken)
            .is_some_and(|rest| rest.starts_with('.'))
        || taken
            .strip_prefix(read)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// Whether assigning `written` gives `taken` a value again: the same path, or
/// the whole it is a part of.
fn revives(written: &str, taken: &str) -> bool {
    written == taken
        || taken
            .strip_prefix(written)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// How a key goes into the brackets of a map whose keys are owned
/// ([ADR-213](../../docs/specification/adr/adr-213.md) D1), handed from the
/// checker to the emitter, which has no types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyForm {
    /// Written: the map keeps the key, so it goes in by value, as it is.
    Handed,
    /// Read: the map is asked about the key and keeps nothing, so it is lent.
    Lent,
    /// Read, and already a view: it goes in as it is.
    AsIs,
    /// Read, from a map whose key type nothing pinned, with a key that may
    /// be a number or a view: `index::key` lends the one and passes the other
    /// through (0.0.235).
    Either,
}

/// A path through the choices of a body: which arm of which `if` or `match`
/// ([ADR-212](../../docs/specification/adr/adr-212.md) D4).
type Choices = Vec<(usize, usize)>;

/// Whether two places are in different arms of one choice, so that no run
/// reaches both.
fn apart(a: &Choices, b: &Choices) -> bool {
    a.iter().any(|(choice, arm)| {
        b.iter()
            .any(|(other, taken)| other == choice && taken != arm)
    })
}

/// A body that may run more than once, around a sequence being taken
/// ([ADR-212](../../docs/specification/adr/adr-212.md) D4).
struct Repeats {
    /// Where the body's own names begin in `Checker::scope`: a name bound in a
    /// frame before this is from outside it.
    frame: usize,
    /// `"loop"` or `"lambda"`, for the sentence.
    what: &'static str,
    /// The names the body assigns, any of which it may give a new sequence
    /// before the next turn takes it - a loop only.
    revived: BTreeSet<String>,
    /// Whether a `break` or a `return` is anywhere in the body, which may end
    /// the loop on the turn that took it - a loop only. Over-approximated, in
    /// the direction that refuses less.
    leaves: bool,
}

impl Repeats {
    fn a_loop(parsed: &Parsed, frame: usize, body: &Block) -> Repeats {
        let mut revived = BTreeSet::new();
        let mut leaves = false;
        what_a_loop_does(parsed, body, &mut revived, &mut leaves);
        Repeats {
            frame,
            what: "loop",
            revived,
            leaves,
        }
    }

    fn a_lambda(frame: usize) -> Repeats {
        Repeats {
            frame,
            what: "lambda",
            revived: BTreeSet::new(),
            leaves: false,
        }
    }

    /// Whether taking `name` here could have left nothing for the next turn.
    fn takes_again(&self, name: &str) -> bool {
        !(self.leaves || self.revived.contains(name))
    }
}

/// The names a loop's body assigns, and whether anything in it leaves.
fn what_a_loop_does(
    parsed: &Parsed,
    block: &Block,
    revived: &mut BTreeSet<String>,
    leaves: &mut bool,
) {
    for stmt in &block.stmts {
        match &stmt.node {
            Stmt::Break | Stmt::Return(_) => *leaves = true,
            Stmt::Assign {
                target: Expr::Variable(name),
                ..
            } => {
                revived.insert(parsed.text(*name).to_string());
            }
            _ => {}
        }
        crate::contracts::sync::visit_stmt_blocks(&stmt.node, &mut |inner| {
            what_a_loop_does(parsed, inner, revived, leaves)
        });
    }
}

impl<'a> Checker<'a> {
    // --- the shape of a program ---------------------------------------------

    fn collect_types(&mut self) {
        for item in &self.parsed.program.items {
            match &item.node {
                Item::Struct {
                    name,
                    generics,
                    fields,
                    ..
                } => {
                    // In **declaration order**, because that is what a type
                    // argument's position means: `Pair[i64]`'s `i64` is the
                    // first parameter and nothing else says which
                    // ([ADR-074](../../docs/specification/adr/adr-074.md) D2).
                    let order: Vec<String> = generics
                        .iter()
                        .map(|g| self.parsed.text(g.name).to_string())
                        .collect();
                    let parameters: BTreeSet<String> = order.iter().cloned().collect();
                    for f in fields {
                        let name = self.parsed.text(f.name).to_string();
                        self.nameable(&name, &f.span, "a field");
                    }
                    let fields: Vec<FieldContract> = fields
                        .iter()
                        .map(|f| FieldContract {
                            name: self.parsed.text(f.name).to_string(),
                            ty: self.declared(&f.ty, &f.span).parameterise(&parameters),
                            public: f.is_public,
                        })
                        .collect();
                    let own = self.parsed.text(*name).to_string();
                    // **An item's own name was never asked**, which `not_self`
                    // did not have to be: `struct self` cannot parse, because
                    // the receiver takes the word. `struct crate` parses fine
                    // and went to the language below (ADR-076 D3).
                    self.nameable(&own, &item.span, "a struct");
                    if !order.is_empty() {
                        self.struct_parameters.insert(own.clone(), order);
                    }
                    self.structs.insert(own, fields);
                }
                Item::Enum { name, variants, .. } => {
                    let own = self.parsed.text(*name).to_string();
                    self.nameable(&own, &item.span, "an enum");
                    let named: Vec<String> = variants
                        .iter()
                        .map(|v| self.parsed.text(v.name).to_string())
                        .collect();
                    for name in &named {
                        self.nameable(name, &item.span, "a variant");
                    }
                    // **A variant with named fields wears a struct literal**,
                    // and until this was here nothing read it: `Shape::Spot {
                    // x: 1 }` found no fields, so every field went unchecked,
                    // and the literal's *type* came out as `Shape::Spot` -
                    // which no declaration can be written as, so a correct
                    // program was refused with a way out that cannot be taken
                    // ([Part III C.2](../../docs/specification/30-nikaia-tooling.md)
                    // and C.4 at once).
                    //
                    // The fields go in the same map a `struct`'s do, under the
                    // qualified key, so `fields_of` answers for both and there
                    // is one field check rather than two. **`pub` is not asked
                    // of them**: a variant carries no visibility word, so its
                    // fields are as reachable as the `enum` is (Part I 9.2).
                    for variant in variants {
                        let ast::VariantFields::Named(fields) = &variant.fields else {
                            continue;
                        };
                        let held: Vec<FieldContract> = fields
                            .iter()
                            .map(|f| FieldContract {
                                name: self.parsed.text(f.name).to_string(),
                                ty: self.declared(&f.ty, &f.span),
                                public: true,
                            })
                            .collect();
                        let key = format!("{own}::{}", self.parsed.text(variant.name));
                        self.variant_owner.insert(key.clone(), own.clone());
                        self.structs.insert(key, held);
                    }
                    for variant in variants {
                        if let ast::VariantFields::Tuple(types) = &variant.fields {
                            let key = format!("{own}::{}", self.parsed.text(variant.name));
                            let parts = types
                                .iter()
                                .map(|ty| Ty::from_ast(self.parsed, ty))
                                .collect();
                            self.variant_parts.insert(key, parts);
                        }
                    }
                    // **Every kind of payload**, positional included — see
                    // [`Checker::enum_payloads`] for why a map of its own.
                    let held: Vec<Ty> = variants
                        .iter()
                        .flat_map(|variant| match &variant.fields {
                            ast::VariantFields::Unit => Vec::new(),
                            ast::VariantFields::Tuple(types) => types
                                .iter()
                                .map(|ty| Ty::from_ast(self.parsed, ty))
                                .collect(),
                            ast::VariantFields::Named(fields) => fields
                                .iter()
                                .map(|f| Ty::from_ast(self.parsed, &f.ty))
                                .collect(),
                        })
                        .collect();
                    self.enum_payloads.insert(own.clone(), held);
                    let variants = variants
                        .iter()
                        .map(|v| self.parsed.text(v.name).to_string())
                        .collect();
                    self.enums
                        .insert(self.parsed.text(*name).to_string(), variants);
                }
                _ => {}
            }
        }
        self.collect_library_enums();
        for item in &self.parsed.program.items {
            match &item.node {
                Item::Fn { .. } => self.bounds_declared_by(&item.node, None),
                Item::Impl {
                    target, methods, ..
                } => {
                    let target = self.parsed.text(target.name).to_string();
                    for method in methods {
                        self.bounds_declared_by(&method.node, Some(&target));
                    }
                }
                _ => {}
            }
        }
        self.collect_field_walks();
        self.collect_comparisons();
    }

    /// **Which declared types compare**
    /// ([ADR-204](../../docs/specification/adr/adr-204.md) D1), for the derive
    /// the emitter writes and the refusal this file raises.
    ///
    /// After every type is collected, because the answer is structural: a
    /// `struct` compares when its fields do, and a field may name a type
    /// declared further down the file.
    fn collect_comparisons(&mut self) {
        let asking = Comparable {
            structs: &self.structs,
            enums: &self.enums,
            payloads: &self.enum_payloads,
            own: self.own,
            library: self.library,
        };
        // **Only what this unit declares.** A library's type answers from its
        // own column, and nothing here writes a derive for it.
        let declared: Vec<String> = self
            .parsed
            .program
            .items
            .iter()
            .filter_map(|item| match &item.node {
                Item::Struct { name, .. } | Item::Enum { name, .. } => {
                    Some(self.parsed.text(*name).to_string())
                }
                _ => None,
            })
            .collect();
        for name in declared {
            let parts: Vec<Ty> = match self.enum_payloads.get(&name) {
                Some(held) => held.clone(),
                None => self
                    .structs
                    .get(&name)
                    .map(|fields| fields.iter().map(|f| f.ty.clone()).collect())
                    .unwrap_or_default(),
            };
            if !parts.iter().all(|ty| asking.of(ty)) {
                continue;
            }
            self.checked.compares.insert(name.clone());
            if parts.iter().all(|ty| asking.totally(ty)) {
                self.checked.compares_totally.insert(name);
            }
        }
    }

    /// **The `enum`s of a package and of `std`**, under the names a consumer
    /// writes them.
    ///
    /// Part I 3.4 promises that a `match` handles every possible case, and that
    /// promise is only checkable where the cases are known. They come from the
    /// source for this unit's own types and from the ledger's `variants` column
    /// for every other — the same column, read one file over, which is what
    /// [ADR-028](../../docs/specification/adr/adr-028.md) D5 says the ledger is
    /// for.
    ///
    /// **This unit's own types win**, which is why this runs after the source
    /// walk and inserts nothing that is already there: a package's ledger is
    /// absorbed under qualified keys, so a collision would be a program that
    /// declares a type under a dependency's name — refused elsewhere, and not
    /// silently taken from the dependency here.
    fn collect_library_enums(&mut self) {
        for ledger in [self.own, self.library] {
            for (name, contract) in &ledger.types {
                if contract.variants.is_empty() || self.enums.contains_key(name) {
                    continue;
                }
                for variant in &contract.variants {
                    let key = format!("{name}::{}", variant.name);
                    self.variant_owner.insert(key.clone(), name.clone());
                    if !variant.holds.is_empty() {
                        self.structs.insert(key, variant.holds.clone());
                    }
                }
                self.enums.insert(
                    name.clone(),
                    contract.variants.iter().map(|v| v.name.clone()).collect(),
                );
            }
        }
    }

    /// **Which functions walk a type's fields, and which parameter each walks**
    /// ([ADR-181](../../docs/specification/adr/adr-181.md) D1).
    ///
    /// Read off the **body** rather than off the bound, because the bound says
    /// only that a shape *may* be asked for: `fn tell[T: Struct](v: T)` that
    /// never writes `T::fields` is an ordinary generic function and stays one,
    /// generic in the language below and emitted once.
    ///
    /// Collected before any body is walked, because a call may stand **above**
    /// the function it names — items are order-independent here — and what a
    /// call has to do about one of these is decided at the call.
    ///
    /// **Over the program's other files too**, for the same reason one file
    /// over: the files of a package share one namespace (Part I 9.1), so
    /// `describe` may be declared in `shapes.nika` and called from `main.nika`
    /// — and a call that did not know it names a shape walk would be left
    /// pointing at a generic original nobody emits.
    fn collect_field_walks(&mut self) {
        self.field_walks_in(self.parsed);
        for other in self.beside {
            self.field_walks_in(other);
        }
    }

    /// One file's shape walks, under the interner its symbols resolve in.
    fn field_walks_in(&mut self, parsed: &Parsed) {
        for item in &parsed.program.items {
            let Item::Fn {
                name: Some(name),
                generics,
                body,
                ..
            } = &item.node
            else {
                continue;
            };
            let walked = generics.iter().find_map(|g| {
                let parameter = parsed.text(g.name).to_string();
                let asked = g.bounds.iter().any(|b| {
                    shape_member(parsed.text(*b)).is_some_and(|(member, _)| {
                        walks_the_fields_of(parsed, body, &parameter, member)
                    })
                });
                asked.then_some(parameter)
            });
            if let Some(parameter) = walked {
                self.walks_fields
                    .insert(parsed.text(*name).to_string(), parameter);
            }
        }
    }

    /// One function's bounds, under the key a call to it resolves to.
    ///
    /// The key is built the way `function` builds its own, the anonymous
    /// constructor included - two spellings of one name would make this table
    /// silently miss whichever the call site used.
    /// The bounds a **ledger** records for this key, read off the `signature`.
    ///
    /// Both ledgers, because a package's entries and `std`'s arrive the same way
    /// and a call resolves to one key either way. `std` declares no bound today,
    /// so this is a package's answer in practice — and it is the same answer the
    /// source gives one file over, which is what
    /// [ADR-028](../../docs/specification/adr/adr-028.md) D5 says the ledger is
    /// for.
    ///
    /// **A parameter with no bound is not an entry here**, which keeps the map
    /// and this reader agreeing: `bounds_declared_by` skips one too, so a
    /// signature that records `[T]` for a bare parameter answers `None` rather
    /// than an empty claim.
    fn bounds_in_a_signature(&self, key: &str) -> Option<BTreeMap<String, Vec<String>>> {
        let contract = self
            .own
            .functions
            .get(key)
            .or_else(|| self.library.lookup(key).map(|(_, c)| c).as_ref().copied())?;
        let bounds: BTreeMap<String, Vec<String>> = contract
            .signature
            .as_ref()?
            .bounds
            .iter()
            .filter(|(_, traits)| !traits.is_empty())
            .map(|(name, traits)| {
                let traits = traits.iter().map(|one| self.as_this_program_says(key, one));
                (name.clone(), traits.collect())
            })
            .collect();
        match bounds.is_empty() {
            true => None,
            false => Some(bounds),
        }
    }

    /// A trait a package's signature names, spelled the way **this** program
    /// writes it.
    ///
    /// A package writes its own names bare — `[H: Handler]` inside `handler` —
    /// and a consumer writes `handler::Handler`, because privacy and naming are
    /// per package ([ADR-047](../../docs/specification/adr/adr-047.md) D1). So
    /// the bound arrives in the *declaring* package's namespace and has to be
    /// read in the consumer's, or [`Checker::answers_for`] fails open on a trait
    /// it has under another spelling and the refusal never lands.
    ///
    /// **Only a spelling this program actually has.** Every prefix of the entry's
    /// own key is tried and the first one the trait map knows wins; where none
    /// does, the name stands as written and the fail-open line below it does what
    /// it always did ([Part III
    /// C.4](../../docs/specification/30-nikaia-tooling.md)). It is the same
    /// *under both spellings* arrangement `absorb_renaming` already uses for the
    /// `implementations` map, asked from the reading side.
    fn as_this_program_says(&self, key: &str, trait_name: &str) -> String {
        if self.own.traits.contains_key(trait_name) {
            return trait_name.to_string();
        }
        let segments: Vec<&str> = key.split("::").collect();
        for take in (1..segments.len()).rev() {
            let qualified = format!("{}::{trait_name}", segments[..take].join("::"));
            if self.own.traits.contains_key(&qualified) {
                return qualified;
            }
        }
        trait_name.to_string()
    }

    fn bounds_declared_by(&mut self, item: &Item, target: Option<&str>) {
        let Item::Fn { name, generics, .. } = item else {
            return;
        };
        let bounds: BTreeMap<String, Vec<String>> = generics
            .iter()
            .filter(|g| !g.bounds.is_empty())
            .map(|g| {
                (
                    self.parsed.text(g.name).to_string(),
                    g.bounds
                        .iter()
                        .map(|b| self.parsed.text(*b).to_string())
                        .collect(),
                )
            })
            .collect();
        if bounds.is_empty() {
            return;
        }
        let own = match name {
            Some(name) => self.parsed.text(*name).to_string(),
            None => "new".to_string(),
        };
        let key = match target {
            Some(target) => format!("{target}::{own}"),
            None => own,
        };
        self.declared_bounds.insert(key, bounds);
    }

    fn program(&mut self) {
        // **A frame under every body, filled before any of them is walked**
        // ([ADR-097](../../../docs/specification/adr/adr-097.md)). A `comptime`
        // at item level is in scope for the whole file, and this checker's
        // scope is a stack pushed per function — so the name has to be there
        // before the first `fn` is entered, and it has to be there for a
        // function declared *above* the constant as much as below it. That is
        // the whole of why this is a pass and not an arm.
        self.item_constants();
        self.types_that_contain_themselves();
        for item in &self.parsed.program.items {
            match &item.node {
                // Walked by `item_constants` above, in a frame that stays.
                Item::Comptime { .. } => {}
                Item::Fn { .. } => self.function(&item.node, None),
                Item::Impl {
                    target,
                    methods,
                    trait_name,
                } => {
                    if trait_name.is_some_and(|t| self.parsed.text(t) == "Cleanup") {
                        self.a_cleanup_shaped(self.parsed.text(target.name), methods, &item.span);
                    }
                    if let Some(trait_name) = trait_name {
                        let written = self.parsed.text(*trait_name).to_string();
                        self.a_trait_nothing_declares(&written, &item.span);
                    }
                    // `impl Stack[T]` puts `T` in scope for every method in it,
                    // exactly as the ledger reads it (`Ledger::of`), so a method
                    // body sees the same names its signature was recorded with -
                    // one rule, called from both (`contracts::impl_parameters`).
                    let declared = crate::contracts::declared_types(self.parsed);
                    // An `impl`'s own parameters carry no bounds: the head
                    // writes `impl Stack[T]` and there is nowhere in it for a
                    // `: Summarize` to stand
                    // ([ADR-078](../../docs/specification/adr/adr-078.md) §4).
                    let outer: BTreeMap<String, Vec<String>> =
                        crate::contracts::impl_parameters(self.parsed, target, &declared)
                            .into_iter()
                            .map(|name| (name, Vec::new()))
                            .collect();
                    // **The head's own brackets, as types.** `impl Holder[T]`
                    // writes a parameter and `impl Holder[i64]` writes a type,
                    // and `Ty::from_ast` needs to know nothing about which: a
                    // parameter's in-body representation *is* its name as a
                    // type (`Ty::Named { name: "T" }`), which is what
                    // `type_parameters` above is keyed by and what `NK1126`
                    // reads. So one line answers both, and the receiver below
                    // binds the struct's parameters the way any other value of
                    // that type does (ADR-074 D2).
                    let arguments: Vec<Ty> = target
                        .generics
                        .iter()
                        .map(|g| Ty::from_ast(self.parsed, g))
                        .collect();
                    let target = self.parsed.text(target.name).to_string();
                    for method in methods {
                        self.enclosing = outer.clone();
                        self.subject_arguments = arguments.clone();
                        self.function(&method.node, Some(&target));
                    }
                    self.enclosing = BTreeMap::new();
                    self.subject_arguments = Vec::new();
                }
                // A test and a bench are code, and nothing about them is
                // exempt from the language's rules (Part III, 14.1 and 13.4).
                // Nothing produces these yet - the items are in the AST and the
                // grammar has no rule for either - so this is what stops their
                // bodies from arriving unchecked on the day it does.
                Item::Test { body, .. } | Item::Bench { body, .. } => {
                    let outer = self.expected.take();
                    self.scope.push(Vec::new());
                    self.block(body);
                    self.scope.pop();
                    self.expected = outer;
                }
                Item::Grammar(grammar) => self.grammar(grammar),
                // **A `use` brings no name in, for `std` as for a package**
                // ([ADR-140](../../docs/specification/adr/adr-140.md) D5).
                Item::Import { path, .. } => self.an_import_that_brings_a_name_in(path, &item.span),
                _ => {}
            }
        }
    }

    /// `NK1156`: a `use` that names a **type** rather than a module
    /// ([ADR-140](../../docs/specification/adr/adr-140.md) D5).
    ///
    /// [ADR-046](../../docs/specification/adr/adr-046.md) D2's rule is *no name
    /// is brought in*, and `std` was the one place it was not followed:
    /// `use std::collections::HashMap` parsed, and what it did was **nothing** —
    /// `HashMap` works with no `use` at all, because it is a name this compiler
    /// already knows. A line that reads like an import and does nothing is the
    /// shape this record is about.
    ///
    /// **Asked of the ledger and not of a list**, and the question is *does it
    /// have a receiver*. A module and a type read the same way in a key —
    /// `fs::read_to_string` and `HashMap::len` are both `X::y` — so what tells
    /// them apart is the **`self`**: a type's entries are called on a value and
    /// a module's are not. So `use std::fs` names a module and is left alone,
    /// and `use std::collections::HashMap` names a type and is not.
    ///
    /// `use std::db::postgres` and `use std::backend::x86` name modules nothing
    /// describes yet, and are left alone too: refusing on a surface that does
    /// not exist is [Part III
    /// C.4](../../docs/specification/30-nikaia-tooling.md)'s correct program
    /// refused.
    fn an_import_that_brings_a_name_in(&mut self, path: &[Ident], span: &Span) {
        let segments: Vec<&str> = path.iter().map(|s| self.parsed.text(*s)).collect();
        let ["std", _, ..] = segments.as_slice() else {
            return;
        };
        let last = segments[segments.len() - 1];
        // A method **of this name**, and not of something inside it:
        // `fs::Mapped::deref` is called on a value and says that `Mapped` is a
        // type, which is true and is not a fact about `fs`.
        let on_a_value = |key: &String| {
            key.strip_prefix(&format!("{last}::"))
                .is_some_and(|rest| !rest.contains("::"))
                && self.library.functions[key]
                    .signature
                    .as_ref()
                    .is_some_and(|s| s.params.first().is_some_and(|(name, _)| name == "self"))
        };
        // **By the last segment**, because a `std` type carries its module since
        // [ADR-154](../../docs/specification/adr/adr-154.md) D3: the key is
        // `collections::HashMap` and the word after the last `::` of the `use`
        // is `HashMap`.
        let a_type = self
            .library
            .types
            .keys()
            .any(|key| crate::contracts::ty::base(key) == last)
            || self.library.functions.keys().any(on_a_value);
        if !a_type {
            self.a_std_module_nobody_declared(segments[1], &segments, span);
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1156",
            message: format!("`use {}` brings no name in", segments.join("::")),
            notes: vec![format!(
                "a `use` names a module and brings no name in (Part I, 9.1), for `std` \
                 as for a package - and `{last}` is a type, so this line does nothing \
                 at all"
            )],
            // **Where the type lives in a module, the way out is the module**
            // ([ADR-154](../../docs/specification/adr/adr-154.md)): `use
            // std::fs::Mapped` is refused, and *drop the line* would be wrong
            // advice — `fs::Mapped` needs `use std::fs` in front of it like
            // every other name in that module. Only a type `std` keys **without**
            // a module needs no line at all, and Part I 1.3 is the list of what
            // that is.
            help: Some(
                match self.library.types.keys().find_map(|key| {
                    key.strip_suffix(&format!("::{last}"))
                        .map(|module| module.to_string())
                }) {
                    Some(module) => format!(
                        "write `use std::{module}`, and `{module}::{last}` wherever it is needed"
                    ),
                    None => format!(
                        "drop the line: `{last}` is a name that needs no `use` (Part I, 1.3), and \
                     it is written `{last}` wherever it is needed"
                    ),
                },
            ),
        });
    }

    /// **`use std::<anything>` is refused here and not by `rustc`** (`NK1186`,
    /// [Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// It used to lower: the `use` became a comment in the generated Rust, the
    /// call was emitted verbatim, and what the programmer read was *failed to
    /// resolve: use of unresolved module or unlinked crate `nosuchthing`* about a
    /// file they did not write, with a `help` telling them to `cargo add` a crate
    /// that does not exist.
    ///
    /// **The list is the join of two**, and neither half alone is right.
    /// [`Checker::std_modules`] is what `std`'s ledger declares, which is what
    /// `std` has; [`PROMISED`] is what a page or a record names and the compiler
    /// has not built, which is what `std` is going to have — and refusing that is
    /// [Part III C.4](../../docs/specification/30-nikaia-tooling.md)'s correct
    /// program refused, since the day the module lands the line is unchanged.
    ///
    /// **Only the second segment**, so a longer path is answered by the module it
    /// starts at: `use std::db::postgres` is `db`'s question and
    /// `use std::backend::x86` is `backend`'s, which is the reading
    /// [ADR-140](../../docs/specification/adr/adr-140.md) already gave them.
    fn a_std_module_nobody_declared(&mut self, module: &str, segments: &[&str], span: &Span) {
        // **What `std` has, as a caller may write it**, which is both the test and
        // the help: a name nobody declared is most often a name misremembered, and
        // the list is short enough to print.
        let mut offered: Vec<&str> = self
            .std_modules
            .iter()
            .map(|m| m.as_str())
            .filter(|m| !m.starts_with(|c: char| c.is_uppercase()) && !NOT_A_MODULE.contains(m))
            .collect();
        offered.sort_unstable();
        if offered.contains(&module) || PROMISED.contains(&module) {
            return;
        }
        let written = segments.join("::");
        let near = nearest(module, &offered);
        let why = NOT_STD
            .iter()
            .find(|(name, _)| *name == module)
            .map(|(_, why)| (*why).to_string());
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1186",
            message: format!("`use {written}` names a module `std` does not have"),
            notes: vec![why.unwrap_or_else(|| {
                format!(
                    "what may stand after `use std::` is what `std`'s ledger declares, and \
                 `{module}` is not one of them (Part I, 1.3 and 9.1) - a module a record \
                 names and this compiler has not built yet is accepted, because the day it \
                 lands the line is unchanged"
                )
            })],
            help: Some(match near {
                Some(near) => format!("did you mean `use std::{near}`?"),
                None => format!("`std` offers: {}", offered.join(", ")),
            }),
        });
    }

    /// A grammar's action blocks are Nikaia, and they build the rule's value.
    ///
    /// What a pattern binds has no type here - that is the parser backend's,
    /// and Stage 0 does not read it - so every binding is `?`. What is written
    /// down is the rule's **return type**, and an action that builds something
    /// else is the mistake worth catching: a rule is where a struct literal is
    /// most often typed out in full.
    /// **Two names the engine has and a grammar may not write** (`NK1187`,
    /// [ADR-120](../../docs/specification/adr/adr-120.md) D3).
    ///
    /// `tag("x")` is `"x"` and `digit1` is `digit+`: each is the engine's
    /// spelling for something the grammar can already say, and D1 makes Part II
    /// 10.8 the whole vocabulary — *an element that is not on the page is not in
    /// the language*. Refused **with the spelling**, because the alternative is
    /// what the reader used to get: `digit1` reached the engine, worked, and the
    /// page it is not on said nothing.
    ///
    /// The surface and the engine's own spelling are allowed to differ, as they
    /// already do for `dec[i64]`; what this closes is a second way to write one
    /// thing.
    fn a_name_the_page_does_not_have(&mut self, pattern: &ast::Spanned<ast::Pattern>) {
        const INSTEAD: &[(&str, &str, &str)] = &[
            ("tag", "`\"x\"`", "a literal is written as itself"),
            (
                "digit1",
                "`digit+`",
                "a run of a character class is the class and a `+`, and it yields \
                 the text it matched",
            ),
        ];
        match &pattern.node {
            ast::Pattern::Ref {
                name,
                args,
                generics,
            } => {
                let written = self.parsed.text(*name);
                if let Some((_, instead, why)) =
                    INSTEAD.iter().find(|(banned, ..)| *banned == written)
                {
                    self.checked.findings.push(Finding {
                        severity: Severity::Error,
                        span: pattern.span.clone(),
                        code: "NK1187",
                        message: format!("`{written}` is not a name a grammar writes"),
                        notes: vec![format!(
                            "Part II 10.8 is every element a grammar may write, and it does \
                             not name this one (ADR-120 D1, D3) - {why}"
                        )],
                        help: Some(format!("write {instead}")),
                    });
                }
                let _ = generics;
                for arg in args {
                    self.a_name_the_page_does_not_have(arg);
                }
            }
            ast::Pattern::Seq(parts) | ast::Pattern::Choice(parts) => {
                for part in parts {
                    self.a_name_the_page_does_not_have(part);
                }
            }
            ast::Pattern::Bind { pat, .. }
            | ast::Pattern::Repeat { pat, .. }
            | ast::Pattern::Group(pat) => self.a_name_the_page_does_not_have(pat),
            ast::Pattern::Literal(_) | ast::Pattern::Cut | ast::Pattern::Fold(_) => {}
        }
    }

    fn grammar(&mut self, grammar: &ast::GrammarDef) {
        let named = self.parsed.text(grammar.name).to_string();
        for rule in &grammar.rules {
            for alt in &rule.alts {
                self.a_name_the_page_does_not_have(&alt.pattern);
            }
        }
        for rule in &grammar.rules {
            let expected = rule.ret_type.as_ref().map(|t| Ty::from_ast(self.parsed, t));
            // **The rule's own key, so its answers land in its own entry.**
            // A `pub` rule *is* a ledger entry
            // ([ADR-082](../../docs/specification/adr/adr-082.md) D1), and the
            // walks that derive `touches` and `locks` read the method answers
            // this checker files under the caller's key — which was `None` for
            // an action, so a grammar's entry inherited nothing and every
            // caller inherited that
            // ([ADR-186](../../docs/specification/adr/adr-186.md)). What the key
            // must **not** do is make an action a *function*, and `NK2605` is
            // where that is said instead: an action's failure leaves the parser
            // rather than travelling to a caller.
            let outer_current = self
                .current
                .replace(format!("{named}::{}", self.parsed.text(rule.name)));
            for alt in &rule.alts {
                let mut frame = Vec::new();
                self.bindings_of(&alt.pattern.node, &mut frame);
                // **A fold's lambdas first, and in the pattern's own frame**
                // ([ADR-092](../../../docs/specification/adr/adr-092.md)),
                // because a fold may stand beside a binding in a sequence:
                // `head:N rest:fold(N, zero, fn(acc, m) { acc + m + head })`
                // parses, and without this `head` is refused. And outside
                // `expected`, which is the rule's declared type - what a fold's
                // `init` and `step` build is the parser backend's arithmetic on
                // the way to that type, not the type itself.
                // **An action may not pause**
                // ([ADR-142](../../docs/specification/adr/adr-142.md) D1), and
                // a fold's `init`, `step` and `merge` are action code too
                // ([ADR-092](../../docs/specification/adr/adr-092.md)) - so the
                // flag is set around both rather than around the block alone.
                let named = self.parsed.text(rule.name).to_string();
                let outer_action = self.inside_an_action.replace(named);
                self.scope.push(frame.iter().map(Local::again).collect());
                self.folds_in(grammar, &alt.pattern.node, &alt.pattern.span);
                self.scope.pop();
                let Some(action) = &alt.action else {
                    self.inside_an_action = outer_action;
                    continue;
                };
                let outer = std::mem::replace(&mut self.expected, expected.clone());
                self.scope.push(frame);
                let tail_span = action.stmts.last().map(|s| s.span.clone());
                let tail = self.block(action);
                self.scope.pop();
                self.inside_an_action = outer_action;
                if let (Some(expected), Some(span)) = (&expected, tail_span) {
                    self.expect(&tail, expected, span, "returns", |found, want| {
                        format!("this action builds `{found}`, and its rule declares `{want}`")
                    });
                }
                self.expected = outer;
            }
            self.current = outer_current;
        }
    }

    /// **A `fold`'s `init`, `step` and `merge`, walked like any other code**
    /// ([ADR-092](../../../docs/specification/adr/adr-092.md)).
    ///
    /// They are expressions inside a **pattern**, and the walk above takes a
    /// rule's *action block* and nothing else - so
    /// `fn(acc, m) { nothing_declares_this }` lowered without a word and the
    /// message was `rustc`'s about a file nobody wrote (Part III, C.1).
    ///
    /// [ADR-084](../../../docs/specification/adr/adr-084.md) D4 closed the half
    /// a **jump** can reach and deliberately no more, with its own walk of
    /// these same expressions. This subsumes that walk rather than standing
    /// beside it: [`Expr::Closure`]'s arm already crosses a boundary and counts
    /// loops from zero, so a jump here meets `NK1132` through the path every
    /// other lambda's does - and gets the better of the two messages, naming
    /// the lambda instead of saying there is no loop.
    fn folds_in(&mut self, grammar: &ast::GrammarDef, pattern: &ast::Pattern, span: &Span) {
        match pattern {
            ast::Pattern::Fold(spec) => self.fold(grammar, spec, span),
            ast::Pattern::Seq(parts) | ast::Pattern::Choice(parts) => {
                for part in parts {
                    self.folds_in(grammar, &part.node, &part.span);
                }
            }
            ast::Pattern::Bind { pat, .. }
            | ast::Pattern::Repeat { pat, .. }
            | ast::Pattern::Group(pat) => self.folds_in(grammar, &pat.node, &pat.span),
            ast::Pattern::Ref { args, .. } => {
                for arg in args {
                    self.folds_in(grammar, &arg.node, &arg.span);
                }
            }
            ast::Pattern::Literal(_) | ast::Pattern::Cut => {}
        }
    }

    /// **A fold's `init`, `step` and `merge`, typed** (0.0.230, `open-work.md` §1.17
    /// before it).
    ///
    /// The step is handed the accumulator `init` builds and one item of the
    /// rule it folds, so its parameters are typed from those two: without that,
    /// `fn(acc, m) { acc.record(m) }` resolved `record` to nothing and a
    /// pausing one compiled - into a future the fold dropped, so the parse
    /// counted nothing. Typed, the call meets `NK2209` like any action's.
    ///
    /// A `merge` is handed two accumulators. An `init` or a `merge` written as
    /// a function's name is a call the fold makes, so it is asked the same
    /// question of the function it names.
    fn fold(&mut self, grammar: &ast::GrammarDef, spec: &ast::FoldSpec, span: &Span) {
        let item = grammar
            .rules
            .iter()
            .find(|rule| rule.name == spec.rule)
            .and_then(|rule| rule.ret_type.as_ref())
            .map(|t| Ty::from_ast(self.parsed, t))
            .unwrap_or(Ty::Unknown);
        let named = self.a_named_fold_part_that_may_pause(&spec.init, span);
        let built = self.expr(&spec.init, span);
        // A function's name - `Summary` is its constructor, `Summary::new` -
        // builds what its signature hands back; anything else is the value.
        let acc = match (named, built) {
            (Some(result), _) => result,
            (
                None,
                Ty::Fn {
                    result: Some(result),
                    ..
                },
            ) => *result,
            (None, Ty::Fn { result: None, .. }) => Ty::Unknown,
            (None, other) => other,
        };
        // **`NK2209` is the rule here and not `NK2206`**: no parameter type
        // says `sync` about this lambda, the language does, and the action
        // rule inside it names the call that breaks it.
        let promised = Promises {
            may_pause: true,
            may_fail: true,
        };
        for (part, given) in [
            (Some(&spec.step), vec![acc.clone(), item]),
            (spec.merge.as_ref(), vec![acc.clone(), acc.clone()]),
        ] {
            let Some(part) = part else { continue };
            match part {
                Expr::Closure {
                    params,
                    mutable,
                    body,
                } => {
                    self.lambda(params, mutable, body, &given, promised, span);
                }
                _ => {
                    let _ = self.a_named_fold_part_that_may_pause(part, span);
                    self.expr(part, span);
                }
            }
        }
    }

    /// `Summary::merge` or `Summary` in a fold: a function the fold calls, so
    /// a pausing one is refused as a call in an action is. What the function
    /// hands back, where its signature says.
    fn a_named_fold_part_that_may_pause(&mut self, part: &Expr, span: &Span) -> Option<Ty> {
        let name = match part {
            Expr::Variable(name) => self.parsed.text(*name).to_string(),
            Expr::Path(parts) => parts
                .iter()
                .map(|p| self.parsed.text(*p))
                .collect::<Vec<_>>()
                .join("::"),
            _ => return None,
        };
        let (key, contract) = self.resolve(&name)?;
        self.a_pausing_call_in_an_action(&key, contract, span);
        contract
            .signature
            .as_ref()
            .map(|signature| signature.result_or_unit())
            .filter(|result| !result.is_unknown())
    }

    /// Every `name:pattern` in a pattern, all of them `?`.
    fn bindings_of(&self, pattern: &ast::Pattern, out: &mut Vec<Local>) {
        match pattern {
            ast::Pattern::Bind { name, pat } => {
                out.push(Local::free(
                    self.parsed.text(*name).to_string(),
                    Ty::Unknown,
                ));
                self.bindings_of(&pat.node, out);
            }
            ast::Pattern::Seq(parts) | ast::Pattern::Choice(parts) => {
                for part in parts {
                    self.bindings_of(&part.node, out);
                }
            }
            ast::Pattern::Ref { args, .. } => {
                for arg in args {
                    self.bindings_of(&arg.node, out);
                }
            }
            ast::Pattern::Repeat { pat, .. } | ast::Pattern::Group(pat) => {
                self.bindings_of(&pat.node, out)
            }
            ast::Pattern::Literal(_) | ast::Pattern::Cut | ast::Pattern::Fold(_) => {}
        }
    }

    fn function(&mut self, item: &Item, target: Option<&str>) {
        let Item::Fn {
            name,
            generics,
            receiver,
            args,
            config,
            ret_type,
            body,
            throws,
            is_sync,
            ..
        } = item
        else {
            return;
        };

        // The same key the ledger uses, arrived at the same way - the anonymous
        // constructor of Kap 4.2 included, which a caller reaches as
        // `Type::new`. Two spellings of one name would silently drop every
        // method call in a constructor.
        let own_name = match name {
            Some(name) => self.parsed.text(*name).to_string(),
            None => "new".to_string(),
        };
        // A function's own name, for the reason the struct's is asked: `fn self`
        // cannot parse and `fn crate` can (ADR-076 D3). The span is the body's
        // first statement where there is one, which is the nearest this walk has
        // - a declaration with a span of its own is `FnArg`'s and not the item's.
        if let Some(span) = body.stmts.first().map(|s| s.span.clone()) {
            self.nameable(&own_name.clone(), &span, "a function");
        }
        let key = match target {
            Some(target) => format!("{target}::{own_name}"),
            None => own_name,
        };
        let outer_current = self.current.replace(key);
        let outer_sync = match is_sync {
            true => self
                .inside_a_sync_function
                .replace(self.current.clone().unwrap_or_default()),
            false => self.inside_a_sync_function.take(),
        };

        // **Inside its own body a type parameter is a type**
        // ([ADR-074](../../docs/specification/adr/adr-074.md) D1). `T` is not a
        // hole here: the caller picked it, this body did not, and a body that
        // put an `i64` where its caller's `T` is wanted would be wrong. So the
        // name stays a name and `fits` compares it, which is what lets `NK1126`
        // below say that nothing describes what a `T` can do. Only at a *call*
        // is it a variable (`bindings`), and that is the whole of the split.
        let mut declared: BTreeMap<String, Vec<String>> = self.enclosing.clone();
        declared.extend(generics.iter().map(|g| {
            (
                self.parsed.text(g.name).to_string(),
                g.bounds
                    .iter()
                    .map(|b| self.parsed.text(*b).to_string())
                    .collect(),
            )
        }));
        // `Self` stands for the type the `impl` is on, and nothing here
        // resolves it. Unlike `T` it is bound by no call site either, so it
        // stays erased: a comparison against it could only be a false positive.
        let parameters: BTreeSet<String> = ["Self".to_string()].into_iter().collect();
        let outer_declared = std::mem::replace(&mut self.type_parameters, declared);

        let outer_borrowing = std::mem::replace(
            &mut self.borrowing_self,
            receiver.as_ref().is_some_and(|r| r.is_ref),
        );

        let mut frame: Vec<Local> = Vec::new();
        if let Some(receiver) = receiver {
            // With the `impl` head's arguments (`subject_arguments`): `self`
            // inside `impl Holder[T]` is a `Holder[T]` and not a bare `Holder`,
            // so `self.value` binds the declaration's `T` and reaches the body
            // as a type rather than as `?`.
            let ty = match target {
                Some(target) => Ty::Named {
                    name: target.to_string(),
                    args: self.subject_arguments.clone(),
                    view: receiver.is_ref,
                },
                None => Ty::Unknown,
            };
            frame.push(Local::free("self".to_string(), ty));
        }
        for arg in args {
            let name = self.parsed.text(arg.name).to_string();
            self.nameable(&name, &arg.span, "a parameter");
            frame.push(Local {
                literal: None,
                name,
                ty: self.declared(&arg.ty, &arg.span).erase(&parameters),
                constant: None,
                // A parameter is not a `for` binding: what a call lends is the
                // caller's business, and a cast over one needs no deref.
                lent: false,
                built: None,
                // **D3**: without the word, a body that changes this parameter
                // is `NK1138`.
                empty_list: None,
                immutable: (!arg.mutable).then(|| Immutable {
                    at: arg.span.clone(),
                    kind: Kind::Parameter,
                }),
                // **D3's third state, on the binding**: this name is a `&mut T`
                // already, so a call that lends it writes nothing.
                changing: arg.mutable,
            });
        }
        // **An option is a parameter** (Part I 5.1): it stands after the `;`,
        // it is named at the call rather than passed by position, and it has a
        // default - and none of that changes that the body may use it. It was
        // missing from this frame, which nothing noticed while an undeclared
        // name was only refused in statement position: measured on
        // `examples/tally/src/main.nika`, whose `f"{lines}{separator}{blank}"` names one.
        for option in config {
            frame.push(Local::free(
                self.parsed.text(option.name).to_string(),
                // Not `declared`: an option has no span of its own, and a
                // caret on the wrong line is worse than no message. Its default
                // is a literal (Part I 5.1), so a hull cannot stand here anyway.
                Ty::from_ast(self.parsed, &option.ty).erase(&parameters),
            ));
        }

        let expected = ret_type
            .as_ref()
            .map(|t| Ty::from_ast(self.parsed, t).erase(&parameters));
        let outer = std::mem::replace(&mut self.expected, expected.clone());
        let outer_either = std::mem::replace(
            &mut self.expected_either,
            ret_type.as_ref().is_some_and(|t| t.either),
        );
        let outer_throwing = std::mem::replace(&mut self.throwing, *throws);

        let keeps: Vec<String> = self
            .current
            .as_ref()
            .and_then(|key| self.own.functions.get(key))
            .map(|contract| contract.keeps.clone())
            .unwrap_or_default();
        // **A parameter the function keeps dies in it**, unless it is handed
        // on, as a value bound by a `let` does (ADR-239 D2).
        for arg in args {
            let name = self.parsed.text(arg.name).to_string();
            let kept = keeps.contains(&name).then(|| {
                frame
                    .iter()
                    .find(|local| local.name == name)
                    .map(|local| local.ty.clone())
            });
            if let Some(Some(ty)) = kept {
                self.a_value_with_a_cleanup_dying(&name, &ty, &arg.span);
            }
        }
        let run_code: BTreeSet<String> = frame
            .iter()
            .filter(|local| matches!(local.ty, Ty::Fn { .. }) && !keeps.contains(&local.name))
            .map(|local| local.name.clone())
            .collect();
        let outer_run_code = std::mem::replace(&mut self.run_code, run_code);
        self.scope.push(frame);
        let tail_span = body.stmts.last().map(|s| s.span.clone());
        let outer_lists = std::mem::take(&mut self.empty_lists);
        let outer_grown = std::mem::replace(&mut self.grown_text, grown_text(self.parsed, body));
        let tail = self.block(body);
        self.grown_text = outer_grown;
        self.scope.pop();

        // **`NK1153`, once the whole body has been seen**
        // ([ADR-135](../../docs/specification/adr/adr-135.md) D2), for
        // `NK2101`'s reason one page down: the use that gives an empty list its
        // element type stands *after* the `let`, and a single pass reaches a
        // later statement later.
        for (name, at) in std::mem::replace(&mut self.empty_lists, outer_lists).into_values() {
            self.an_empty_list_with_no_element_type(&name, &at);
        }

        // The last expression of a body is what the function hands back, so it
        // answers to the declared type exactly as a `return` does - unless the
        // body **cannot get there**
        // ([ADR-093](../../../docs/specification/adr/adr-093.md)).
        let ends = !never_ends(body);
        if let (Some(expected), Some(span)) = (&expected, tail_span.filter(|_| ends)) {
            // **A tail is a `return` written without the word**, so it owes the
            // same `&`: `fn text(ref self) -> ref String { self.text }` and the
            // `return` form are one program, and one of the two answering
            // differently would be a spelling that decides a refusal.
            let lending = matches!(body.stmts.last().map(|s| &s.node), Some(Stmt::Expr(value))
                if self.hands_back_a_view_of_the_subject(value));
            if lending {
                self.checked.lent_returns.insert(span.start);
            }
            let tail = match lending {
                true => view_of(&tail),
                false => tail,
            };
            // **And the tail is a use**, as `return` is (ADR-207 D2).
            let tail = match body.stmts.last().map(|s| &s.node) {
                Some(Stmt::Expr(value)) if !lending => self
                    .text_literal(expected, value, true)
                    .or_else(|| self.tuple_literal(&tail, expected, value))
                    .unwrap_or(tail),
                _ => tail,
            };
            let value = match body.stmts.last().map(|s| &s.node) {
                Some(Stmt::Expr(value)) => Some(value),
                _ => None,
            };
            self.expect_kept(
                &tail,
                expected,
                value,
                "the function hands it to its caller, who keeps it after this call has ended",
                span,
                "returns",
                |found, want| {
                    format!("this function hands back `{found}`, and it declares `{want}`")
                },
            );
        }

        // **`NK2101`, once the whole body has been seen.** The question is what
        // comes *after* a `spawn`, and a single pass reaches a later statement
        // later - so it is asked here rather than at the `spawn`.
        // **`NK2702` first, because it borrows what the next one takes**
        // ([ADR-105](../../docs/specification/adr/adr-105.md) D2). A second walk
        // is a later statement and a single pass reaches one later, so both
        // questions are asked here; `a_task_took_what_is_used_again` empties
        // `read_at`, and this one reads it.
        self.a_sequence_was_walked_twice();
        self.data_handed_over_is_used_again();
        self.reads_on_paths.clear();
        self.read_index.clear();
        self.lent_lets.clear();
        self.a_task_took_what_is_used_again();

        self.expected = outer;
        self.expected_either = outer_either;
        self.throwing = outer_throwing;
        self.type_parameters = outer_declared;
        self.borrowing_self = outer_borrowing;
        self.current = outer_current;
        self.inside_a_sync_function = outer_sync;
        self.run_code = outer_run_code;
    }

    /// Whether a conversion narrows, recorded for the emitter (ADR-043 D4).
    ///
    /// **Only among the numeric types this compiler knows the range of**, for the
    /// reason `constant_fits` gives: a claim about a type neither the
    /// specification nor the ledger describes would be a claim about a surface
    /// that is not there. Anything this does not recognise is recorded as
    /// widening, which leaves the conversion exactly as it is today.
    ///
    /// The list is the other way round - which conversions **always** fit - and
    /// that is the fail-closed direction (ADR-010 D1): a pair nobody thought
    /// about is checked at run time rather than truncated in silence.
    ///
    /// **The machine-width types are not in it, and no longer need to be**
    /// ([ADR-048](../../../../docs/specification/adr/adr-048.md) D1). They were,
    /// because `len` handed one back and what fits on a large machine does not on
    /// a small one. A length is an `i64` now and `usize` has left the surface a
    /// program can write, so a conversion out of one is a conversion out of a type
    /// nobody can hold - and this list is the writable surface, which is the
    /// reason the entry was there and the reason it is not.
    ///
    /// An integer to `f64` is the one entry that loses something and is still
    /// called fitting (D7): digits go at large values without anything
    /// overflowing, there is no sensible point to stop at, and every language
    /// does it this way. That is a written limit and not a check.
    fn record_cast(&mut self, from: &Ty, into: &Ty, span: &Span) {
        let (Ty::Named { name: from, .. }, Ty::Named { name: into, .. }) = (from, into) else {
            return;
        };
        let (from, into_name) = (from.as_str(), into.as_str());
        const NUMERIC: [&str; 4] = ["i32", "i64", "f64", "u8"];
        let always_fits =
            from == into_name || into_name == "f64" || (from, into_name) == ("i32", "i64");
        let checked = NUMERIC.contains(&from) && NUMERIC.contains(&into_name) && !always_fits;

        let at = (span.start, into.clone());
        match (from, checked) {
            (_, false) => {
                self.widening_casts.insert(at);
            }
            // Out of a floating-point number: silent three ways - it stops at the
            // limit in both directions and turns "not a number" into zero - and
            // `try_from` does not exist for it, so `std` carries the test.
            ("f64", true) => {
                self.checked
                    .narrowing_casts
                    .insert(at, Narrowing::FromFloat);
            }
            (_, true) => {
                self.checked.narrowing_casts.insert(at, Narrowing::Integer);
            }
        }
    }

    /// A name nothing declares (`NK1117`).
    ///
    /// **This exists because a misparse is otherwise a different program.** The
    /// grammar is scannerless, so a word this language does not know is read as a
    /// name and a name in statement position is a legal statement. Measured,
    /// before the keyword boundaries went in (`parser::KW_*`) and after:
    ///
    /// ```text
    /// assert c                 ->  assert;  c;
    /// unsafe { println("x") }  ->  unsafe;  { println!("x") }
    /// let n = 1_000            ->  let n = 1;  _000;
    /// ```
    ///
    /// **The third one is history now**
    /// ([ADR-136](../../docs/specification/adr/adr-136.md)): `1_000` is the
    /// number `1000`, so this message stopped naming it — a help text that
    /// explains a form the language has is worse than no help at all.
    ///
    /// Every one of those reached `rustc`, which refused it about a file nobody
    /// wrote - the Part III C.1 class. The boundary rules stop the *worse* half,
    /// where the word was swallowed into a neighbour; this stops the rest, here,
    /// in this language's words.
    ///
    /// **Only a statement that is exactly one name**, which is the narrowest rule
    /// that covers the class. A name inside a larger expression is refused by the
    /// type checker if it is refused at all, and a name that is the *value* of a
    /// block - `fn f() -> i64 { x }` - is this same shape, where an undeclared `x`
    /// is equally wrong.
    ///
    /// **Four things count as declaring it**, and the list is the fail-safe
    /// direction (ADR-010 D1 applied to a refusal rather than to a permission): a
    /// local or parameter in scope, a function either ledger describes, a type
    /// declared here, and a module of this program. Anything this cannot see is a
    /// name it must not refuse, because refusing a correct program is the one
    /// thing this checker may never do (Part III, C.4).
    fn nothing_declares_it(&mut self, expr: &Expr, span: &Span) {
        let Expr::Variable(name) = expr else {
            return;
        };
        let name = self.parsed.text(*name).to_string();
        let declared = self.lookup(&name).is_some()
            || self.resolve(&name).is_some()
            || self.structs.contains_key(&name)
            || self.enums.contains_key(&name)
            || self.own.types.contains_key(&name)
            || self.modules.contains(&name)
            // A grammar's name stands where a callee stands (ADR-082 D1), so
            // it is declared in exactly the way a module is.
            || self.grammars.contains_key(&name);
        if declared {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1117",
            message: format!("nothing declares `{name}`"),
            notes: vec![
                "this language has no word it does not know: one it has no rule for is \
                 read as a name, and a name has to be declared somewhere (Part I, 9.1)"
                    .to_string(),
            ],
            help: Some(match a_word_that_was_reserved(&name) {
                // **What each word used to be told a reserved one, this tells a
                // stray one** ([ADR-117](../../docs/specification/adr/adr-117.md)
                // D2). Reserving a word buys exactly one thing, which is the
                // sentence a reader who writes it gets; the four that left the
                // list were paying for nothing, because this message can say it
                // about a name.
                Some(instead) => instead.to_string(),
                None => format!(
                    "if `{name}` is meant to be a value, declare it with `let`; if it is \
                     meant to be a keyword, this language has no such keyword"
                ),
            }),
        });
    }

    /// **`NK1117` for a function nothing declares** (0.0.244).
    ///
    /// `frobnicate(1)` lowered as written and `rustc` said *cannot find
    /// function* about a file nobody wrote (Part III C.1). A call nothing
    /// describes is left alone where it is written through a path - a package
    /// or a crate may have it - but a **bare** name has nowhere else to come
    /// from: a `use` brings no name in (ADR-140 D5), so it is this program's,
    /// a value in scope, the prelude's or nothing. Everything this checker
    /// knows declares one is asked first, so what is left is certain (C.4).
    fn a_function_nothing_declares(&mut self, name: &str, span: &Span) {
        let declared = self.lookup(name).is_some()
            || self.structs.contains_key(name)
            || self.enums.contains_key(name)
            || self.own.types.contains_key(name)
            || self.modules.contains(name)
            || self.grammars.contains_key(name)
            || self.foreign_names.contains(name)
            || self.variant_parts.contains_key(name)
            || is_hull(name)
            || MultiLock::named(name).is_some()
            || matches!(name, "panic" | ASSET | ASSERT);
        if declared {
            return;
        }
        let mut known: Vec<&str> = self
            .own
            .functions
            .keys()
            .chain(self.library.functions.keys())
            .filter(|key| !key.contains("::"))
            .map(String::as_str)
            .collect();
        known.push("panic");
        let help = match nearest(name, &known) {
            Some(near) => format!("did you mean `{near}`?"),
            None => format!("declare it with `fn {name}(…)`, or call it through the package that has it"),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1117",
            message: format!("nothing declares a function `{name}`"),
            notes: vec![
                "a function called by its bare name is this program's, a value in scope or \
                 one the prelude offers; a `use` brings no name in, so one from a package \
                 is written with the package's name in front (Part I, 9.1)"
                    .to_string(),
            ],
            help: Some(help),
        });
    }

    /// **A lookup's key is lent** (0.0.235, `open-work.md` §1.21).
    ///
    /// `m.get(k)`, `m.contains_key(k)` and a set's `contains(k)` and
    /// `remove(k)` take the key by reference in the language below, and a
    /// method's arguments are not lent by the ledger's column
    /// (`contracts::keeps::lends` says why: the declaration would be written
    /// off it, and a call the checker does not walk would miss). These
    /// declarations are Rust's and are never written off anything, so the
    /// reason does not reach them: the `&` is recorded here, for exactly these
    /// entries, where the key is a known value that is not already a view.
    /// `m.get(k)` over a map keyed by numbers was `rustc`'s *mismatched types*.
    fn a_lookup_key_lent(
        &mut self,
        key: &str,
        written: &str,
        args: &[Expr],
        found: &[Ty],
        span: &Span,
    ) {
        if !is_a_lookup(key) || !self.library.functions.contains_key(key) {
            return;
        }
        let ([given], [ty]) = (args, found) else {
            return;
        };
        if ty.is_a_view() || matches!(given, Expr::LitStr { .. }) {
            return;
        }
        // **A key of no known type** - `let k = 1` has none until something
        // pins it - is written through `AsKey`, which lends a number and
        // passes a view through, since the `&` alone would be wrong for the
        // second.
        if ty.is_unknown() {
            self.checked.lookup_keys.insert((
                span.start,
                written.to_string(),
                argument_shape(given),
            ));
            return;
        }
        self.checked
            .lent_args
            .entry((span.start, written.to_string(), 0))
            .or_default()
            .insert(argument_shape(given));
    }

    /// **`NK1190`: a `std` type called as its constructor, where nothing
    /// describes one** (0.0.232).
    ///
    /// `collections::HashSet()` passed the check and lowered as it was written,
    /// and the language below said *expected function, found type alias* about
    /// a file nobody wrote ([Part III
    /// C.1](../../docs/specification/30-nikaia-tooling.md)). The constructor is
    /// the ledger entry `Name::new` ([ADR-140](../../docs/specification/adr/adr-140.md)
    /// D2), the emitter writes one only where that entry exists, and the ledger
    /// described `HashSet`, `BTreeMap` and `BTreeSet` as types without one.
    ///
    /// **Only a type the library names exactly, and only with nothing in the
    /// parentheses.** A type this compiler cannot see is not asked about, and a
    /// call with arguments may be a foreign tuple struct's own constructor.
    /// `NK1190` for a struct of the program's own: it declares no anonymous
    /// constructor (`impl T { pub fn(…) … }`, ADR-140 D2), so a call has
    /// nothing to be lowered to, and the literal is the way it is built.
    fn a_struct_called_without_a_constructor(&mut self, name: &str, span: &Span) {
        let fields = self
            .structs
            .get(name)
            .map(|fields| {
                fields
                    .iter()
                    .map(|f| format!("{}: …", f.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1190",
            message: format!("`{name}` declares no constructor, so it cannot be called"),
            notes: vec![format!(
                "a type is called through its anonymous constructor, `impl {name} {{ pub fn(…) … }}` (ADR-140 D2), and `{name}` has none"
            )],
            help: Some(format!("build it with a literal: `{name} {{ {fields} }}`")),
        });
    }

    fn a_constructor_nothing_describes(&mut self, name: &str, args: &[Expr], span: &Span) {
        if !args.is_empty()
            || !self.library.types.contains_key(name)
            || self.resolve(name).is_some()
        {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1190",
            message: format!("`{name}` is a type, and nothing describes a constructor for it"),
            notes: vec![format!(
                "a type is built by its anonymous constructor, `{name}()`, and what that \
                 does is the entry `{name}::new` in the ledger that describes the type \
                 (ADR-140 D2) - `{name}` has no such entry, so there is nothing this call \
                 could be lowered to"
            )],
            help: None,
        });
    }

    /// **Owned text assigned to a `let mut` that began as a literal**
    /// (0.0.232): `NK1105` with the way out that exists.
    ///
    /// The binding is a view because a literal is one, and `s + "…"` or an
    /// f-string assigned to it later makes it text of its own without a word
    /// ([`grown_text`]). A bare name assigned is not read that way, since it may
    /// be a view, so here the program has to say it - and the generic help,
    /// *write `ref` in front of it*, sent the reader to take a view of a value
    /// that is about to be dropped.
    fn owned_text_into_a_literal_binding(
        &mut self,
        target: &Expr,
        found: &Ty,
        into: &Ty,
        span: &Span,
    ) -> bool {
        let Expr::Variable(name) = target else {
            return false;
        };
        let name = self.parsed.text(*name).to_string();
        let Some(literal) = self.binding(&name).and_then(|l| l.literal.clone()) else {
            return false;
        };
        if *found != Ty::named("String") || !into.is_a_view() {
            return false;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1105",
            message: format!(
                "this is `String`, and `{name}` is a `ref String`, because it began as a literal"
            ),
            notes: vec![
                "a literal is a view of text built into the program, and a name bound to one \
                 is a view too - so what it is given later has to be one as well (ADR-216 D4)"
                    .to_string(),
            ],
            help: Some(format!(
                "say that `{name}` holds text of its own: `let mut {name}: String = \"{literal}\"`"
            )),
        });
        true
    }

    /// **`NK1191`: an arithmetic operator on a collection** (0.0.234).
    ///
    /// Only where a side is **known** to be a list, a map or a set: a value
    /// whose type this checker could not work out is not refused on a guess
    /// ([Part III C.4](../../docs/specification/30-nikaia-tooling.md)).
    fn arithmetic_on_a_collection(&mut self, op: BinaryOp, left: &Ty, right: &Ty, span: &Span) {
        let symbol = match op {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            _ => "%",
        };
        let (side, other) = match is_a_collection(left) {
            true => (left, right),
            false => (right, left),
        };
        let joining = matches!(op, BinaryOp::Add) && is_a_list(left) && is_a_list(right);
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1191",
            message: match is_a_list(side) {
                true => format!("`{symbol}` is not defined on a list"),
                false => format!("`{symbol}` is not defined on `{side}`"),
            },
            notes: vec![match joining {
                true => "a list has no operators (Part I, 4.5): whether `+` should join two \
                         lists is not decided, and it is on `open-decisions.md`"
                    .to_string(),
                false => format!(
                    "a list, a map or a set has no arithmetic (Part I, 4.5), and the other \
                     side is `{other}`"
                ),
            }],
            help: joining.then(|| {
                "to add one list's elements to the end of another: `a.extend(b)`".to_string()
            }),
        });
    }

    /// **A type that holds itself holds itself through a box the compiler
    /// writes** ([ADR-246](../../docs/specification/adr/adr-246.md) D1).
    ///
    /// `enum Expr { Add(Expr, Expr) }` is a program as written: a value that
    /// holds a whole value of its own type inline has no size, so the fields on
    /// such a ring are the ones that go behind a `Box` below, and this is the
    /// walk that finds them. It was `NK1192`'s refusal at 0.0.234, until the
    /// owner decided the question it named.
    ///
    /// **A field is boxed where what it holds inline reaches back to the type
    /// that declares it** - the type itself, or one on the same ring. Only an
    /// edge that is certainly inline counts: the type, nullable or not, a
    /// tuple's parts and an array's element. A view, a function, a list or
    /// anything this compiler cannot see through is an indirection already.
    ///
    /// Recorded for **every** type of the program, not only this file's: a
    /// literal in this file may build a type declared beside it.
    fn types_that_contain_themselves(&mut self) {
        let parsed = self.parsed;
        let files: Vec<&Parsed> = std::iter::once(parsed)
            .chain(self.beside.iter().copied())
            .collect();
        // Every declared type's members, with the types each holds inline.
        let mut members: BTreeMap<String, Vec<(String, BTreeSet<String>)>> = BTreeMap::new();
        for file in &files {
            for item in &file.program.items {
                let (name, held): (String, Vec<(String, &ast::Type)>) = match &item.node {
                    Item::Struct { name, fields, .. } => (
                        file.text(*name).to_string(),
                        fields
                            .iter()
                            .map(|f| (file.text(f.name).to_string(), &f.ty))
                            .collect(),
                    ),
                    Item::Enum { name, variants, .. } => (
                        file.text(*name).to_string(),
                        variants
                            .iter()
                            .flat_map(|v| {
                                let variant = file.text(v.name).to_string();
                                match &v.fields {
                                    ast::VariantFields::Unit => Vec::new(),
                                    ast::VariantFields::Tuple(types) => types
                                        .iter()
                                        .enumerate()
                                        .map(|(at, ty)| {
                                            (boxed_member(&variant, Some(at), None), ty)
                                        })
                                        .collect(),
                                    ast::VariantFields::Named(fields) => fields
                                        .iter()
                                        .map(|f| {
                                            let field = file.text(f.name);
                                            (boxed_member(&variant, None, Some(field)), &f.ty)
                                        })
                                        .collect(),
                                }
                            })
                            .collect(),
                    ),
                    _ => continue,
                };
                let entry = members.entry(name).or_default();
                for (member, ty) in held {
                    let mut inline = BTreeSet::new();
                    held_inline(file, ty, &mut inline);
                    entry.push((member, inline));
                }
            }
        }
        let holds: BTreeMap<String, BTreeSet<String>> = members
            .iter()
            .map(|(name, fields)| {
                let all = fields.iter().flat_map(|(_, inline)| inline.iter().cloned());
                (name.clone(), all.collect())
            })
            .collect();
        for (name, fields) in &members {
            for (member, inline) in fields {
                let back = inline
                    .iter()
                    .any(|held| held == name || reaches(held, name, &holds));
                if back {
                    self.checked
                        .boxed_members
                        .entry(name.clone())
                        .or_default()
                        .insert(member.clone());
                }
            }
        }
    }

    /// **`NK1193`: a pattern that looks inside a part the compiler boxed**
    /// ([ADR-246](../../docs/specification/adr/adr-246.md) D4).
    ///
    /// `Expr::Add(Expr::Num(n), b)` asks the part inside a box a question, and
    /// the language below matches no pattern through one: a name binds the
    /// part and the arm reads it, but a pattern there has nothing to stand
    /// on. Refused by name until the lowering rewrites it into a second
    /// `match` in the arm; a nested `match` says the same today.
    fn a_pattern_inside_a_box(&mut self, pattern: &MatchPattern, span: &Span) {
        match pattern {
            MatchPattern::Tuple { path, parts } => {
                let written: Vec<&str> = path.iter().map(|s| self.parsed.text(*s)).collect();
                let boxed: Vec<bool> = (0..parts.len())
                    .map(|at| {
                        let [.., owner, variant] = written.as_slice() else {
                            return false;
                        };
                        self.checked
                            .boxed_members
                            .get(*owner)
                            .is_some_and(|members| {
                                members.contains(&boxed_member(variant, Some(at), None))
                            })
                    })
                    .collect();
                for (at, part) in parts.iter().enumerate() {
                    let binds = matches!(part, MatchPattern::Path(one) if one.len() == 1)
                        || matches!(part, MatchPattern::Otherwise);
                    if boxed[at] && !binds {
                        let owner = written.join("::");
                        self.checked.findings.push(Finding {
                            severity: Severity::Error,
                            span: span.clone(),
                            code: "NK1193",
                            message: format!(
                                "the part of `{owner}` at position {at} holds its own type, and \
                                 a pattern cannot look inside it yet"
                            ),
                            notes: vec![
                                "a part that holds its own type is kept in a box the compiler \
                                 writes (ADR-246 D1), and a pattern cannot be matched through \
                                 one below: a name binds the part, and the arm reads it"
                                    .to_string(),
                            ],
                            help: Some(
                                "bind the part to a name here and `match` on the name in the arm"
                                    .to_string(),
                            ),
                        });
                        continue;
                    }
                    self.a_pattern_inside_a_box(part, span);
                }
            }
            MatchPattern::Or(alternatives) => {
                for alternative in alternatives {
                    self.a_pattern_inside_a_box(alternative, span);
                }
            }
            _ => {}
        }
    }

    /// Whether a call is to the **prelude's `assert`**
    /// ([ADR-245](../../docs/specification/adr/adr-245.md) D2), and not to a
    /// name the program declared itself.
    fn the_prelude_assert(&self, func: &Expr) -> bool {
        matches!(func, Expr::Variable(name) if self.parsed.text(*name) == ASSERT)
            && self.lookup(ASSERT).is_none()
            && !self.own.functions.contains_key(ASSERT)
    }

    /// **`assert(cond; message: …)`** ([ADR-245](../../docs/specification/adr/adr-245.md)
    /// D2, D3): one `bool`, one option, and a condition that changes nothing.
    ///
    /// Typed here and not through [`Checker::call`], because what the
    /// condition calls is the question: a frame of [`StoredCode`] around it
    /// collects every method it resolves, and its free calls are read off the
    /// tree. A condition that pauses, fails, touches anything or changes a
    /// value is `NK1194`: the rule that keeps a proof possible (D3), and the
    /// reason evaluating it a second time for the failure message is harmless.
    fn an_assert(&mut self, args: &[Expr], config: &[ast::ConfigArg], span: &Span) -> Ty {
        let unit = Ty::named("()");
        let [condition] = args else {
            self.an_assert_of_another_shape(
                format!(
                    "`assert` claims one condition, and this hands it {}",
                    args.len()
                ),
                span,
            );
            for arg in args {
                self.expr(arg, span);
            }
            return unit;
        };
        self.stored_frames.push(StoredCode::default());
        let found = self.expr(condition, span);
        let frame = self.stored_frames.pop().unwrap_or_default();
        self.expect_bool(&found, span, "an `assert` claims a `bool`");
        // A number's type is settled below, so the check above cannot see
        // `assert(5)`; the literal is its own answer.
        if is_a_literal_other_than(condition, "bool") {
            self.an_assert_of_another_shape(
                "an `assert` claims a `bool`, and this is a literal of another type".to_string(),
                span,
            );
        }
        let mut message = None;
        for option in config {
            let name = self.parsed.text(option.name).to_string();
            let found = self.expr(&option.value, span);
            if name != "message" {
                self.an_assert_of_another_shape(
                    format!("`assert` has no option `{name}`; its one option is `message:`"),
                    span,
                );
                continue;
            }
            let not_text = is_a_literal_other_than(&option.value, "text")
                || (!found.is_unknown()
                    && !found.fits(&Ty::named("String"))
                    && !found.fits(&Ty::view("str")));
            if not_text {
                self.an_assert_of_another_shape(
                    match found.is_unknown() {
                        true => "an `assert`'s `message:` is text, and this is a literal of \
                                 another type"
                            .to_string(),
                        false => format!("an `assert`'s `message:` is text, and this is `{found}`"),
                    },
                    span,
                );
            }
            message = Some(&option.value);
        }

        let mut not_pure: Vec<String> = Vec::new();
        if frame.unresolved {
            not_pure.push(
                "it calls a method nothing describes, so nothing says it changes nothing"
                    .to_string(),
            );
        }
        for key in &frame.resolved {
            if let Some((_, contract)) = self.method(key)
                && let Some(why) = not_pure_because(contract)
            {
                not_pure.push(format!("`{key}` {why}"));
            }
        }
        self.a_claim_reaches(condition, &mut not_pure);
        if !not_pure.is_empty() {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK1194",
                message: "the condition of an `assert` has to change nothing".to_string(),
                notes: not_pure
                    .into_iter()
                    .chain(std::iter::once(
                        "an `assert` is a claim the compiler may later prove, so its condition \
                         may not pause, fail, touch anything or change a value (ADR-245 D3)"
                            .to_string(),
                    ))
                    .collect(),
                help: Some(
                    "compute the value first, with `let`, and claim something about the name"
                        .to_string(),
                ),
            });
        }

        // **What the failure shows** (D2): the operands of a comparison, by
        // the names they were written with, where their value can be printed.
        let (left, right) = match condition {
            Expr::Binary {
                op:
                    BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge,
                lhs,
                rhs,
                ..
            } => (self.shown_operand(lhs), self.shown_operand(rhs)),
            _ => (None, None),
        };
        let mut claimed = format!("{ASSERT}({}", written(self.parsed, condition));
        if let Some(value) = message {
            claimed.push_str(&format!("; message: {}", written(self.parsed, value)));
        }
        claimed.push(')');
        self.checked.claims.insert(
            (span.start, argument_shape(condition)),
            Claim {
                written: claimed,
                left,
                right,
            },
        );
        unit
    }

    /// `NK1195`: an `assert` written in a shape it does not have.
    fn an_assert_of_another_shape(&mut self, message: String, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1195",
            message,
            notes: vec![
                "`assert(cond)` or `assert(cond; message: \"…\")` (ADR-245 D2)".to_string(),
            ],
            help: None,
        });
    }

    /// An operand whose value an `assert`'s failure can show: anything but a
    /// literal, which shows nothing it does not already say. Whether its type
    /// can be printed is the language below's to answer
    /// (`nikaia_std::abort::Operand`), because an integer literal's type is
    /// not settled here.
    fn shown_operand(&self, operand: &Expr) -> Option<String> {
        let literal = matches!(
            operand,
            Expr::LitInt(_)
                | Expr::LitFloat(_)
                | Expr::LitStr { .. }
                | Expr::LitChar(_)
                | Expr::LitBool(_)
                | Expr::LitNull
        ) || matches!(operand, Expr::Unary { op: UnaryOp::Neg, expr }
            if matches!(**expr, Expr::LitInt(_) | Expr::LitFloat(_)));
        (!literal).then(|| written(self.parsed, operand))
    }

    /// The free calls and the forms in a claim that are not a read
    /// ([ADR-245](../../docs/specification/adr/adr-245.md) D3). Methods are
    /// the frame's; this is what the tree says.
    fn a_claim_reaches(&self, condition: &Expr, not_pure: &mut Vec<String>) {
        let mut found: Vec<String> = Vec::new();
        crate::contracts::sync::visit_expr(self.parsed, condition, &mut |expr| {
            let form = match expr {
                Expr::Spawn { .. } => Some("starts a task"),
                Expr::Select(_) => Some("waits in a `select`"),
                Expr::Overlap(_) => Some("runs an `overlap`"),
                Expr::Dsl { .. } => Some("runs a `dsl` block"),
                Expr::Asm { .. } | Expr::Unsafe(_) => Some("runs `unsafe` code"),
                Expr::Try(_) | Expr::Throw(_) | Expr::TryCatch { .. } => Some("can fail"),
                Expr::Return(_) | Expr::Break | Expr::Continue => Some("jumps"),
                Expr::Block(_) | Expr::If { .. } | Expr::Match { .. } | Expr::Closure { .. } => {
                    Some("holds a block, which may change something")
                }
                _ => None,
            };
            if let Some(form) = form {
                found.push(format!("the condition {form}"));
            }
            let Expr::Call { func, .. } = expr else {
                return;
            };
            let name = match func.as_ref() {
                Expr::Variable(name) => self.parsed.text(*name).to_string(),
                Expr::Path(segments) => self.parsed.unaliased(
                    &segments
                        .iter()
                        .map(|s| self.parsed.text(*s))
                        .collect::<Vec<_>>()
                        .join("::"),
                ),
                _ => {
                    found.push("the condition calls something that is not a name".to_string());
                    return;
                }
            };
            // A value is made, not a call made: a variant, a type's
            // constructor, a hull.
            let constructs = self.variant_parts.contains_key(&name)
                || self.declares_a_type(&name)
                || is_hull(&name)
                || name
                    .split_once("::")
                    .is_some_and(|(ty, variant)| self.is_variant(ty, variant));
            if constructs {
                return;
            }
            match self.resolve(&name) {
                Some((_, contract)) => {
                    if let Some(why) = not_pure_because(contract) {
                        found.push(format!("`{name}` {why}"));
                    }
                }
                None => found.push(format!(
                    "nothing describes `{name}`, so nothing says it changes nothing"
                )),
            }
        });
        not_pure.extend(found);
    }

    /// **A view of a copied value compared with the value** (0.0.238).
    ///
    /// `for c in word { if c == 'x' { … } }` lends `word`, so `c` is a view of a
    /// `char`, and the language below compares a `char` with a `char` and not
    /// with a `&char`: *can't compare `char` with `&char`* about a file nobody
    /// wrote. ADR-233 D4 already reads a view of a number as the number for
    /// arithmetic; this is the same reading for a comparison. Recorded only
    /// where the other side is known and is not a view, so two views still
    /// compare as they are.
    fn a_view_compared_with_a_value(
        &mut self,
        lhs: &Expr,
        left: &Ty,
        rhs: &Expr,
        right: &Ty,
        span: &Span,
    ) {
        // **A name a `for` lends is a view below whatever its type says**
        // (`Local::lent`, the question `a_cast_over_a_lent_binding` asks for
        // the same reason): the checker types `c` in `for c in word` as the
        // element, and the language below binds a `&char`.
        let lent = |side: &Expr| -> bool {
            matches!(side, Expr::Variable(name)
                if self.binding(self.parsed.text(*name)).is_some_and(|local| local.lent))
        };
        let copied = |ty: &Ty| -> bool {
            matches!(ty, Ty::Named { name, view: false, .. }
                if is_number(name) || matches!(name.as_str(), "char" | "bool"))
        };
        let is_view = |side: &Expr, ty: &Ty| copies_as_a_view(ty) || (lent(side) && copied(ty));
        let is_value = |side: &Expr, ty: &Ty| !ty.is_unknown() && !ty.is_a_view() && !lent(side);
        let read_left = is_view(lhs, left) && is_value(rhs, right);
        let read_right = is_view(rhs, right) && is_value(lhs, left);
        if read_left {
            self.checked
                .compared_views
                .insert((span.start, argument_shape(lhs)));
        }
        if read_right {
            self.checked
                .compared_views
                .insert((span.start, argument_shape(rhs)));
        }
    }

    /// **An `impl` of a trait nothing declares** (0.0.231).
    ///
    /// `impl db::Connection for Fake` lowered as it was written, and the
    /// language below answered *unresolved import `db`* about a file nobody
    /// wrote ([Part III C.1](../../docs/specification/30-nikaia-tooling.md));
    /// a bare `impl Connection for Fake` was *cannot find trait* the same way.
    /// Nothing looked at the trait an `impl` names: a **bound** fails open on a
    /// trait it does not know ([`Checker::answers_for`]), and that is right for
    /// a bound, which only asks. An `impl` *writes* the name into the program.
    ///
    /// **A path is [`Checker::a_head_nothing_declares`]'s question**, asked of
    /// its head: a package, a module, a described crate or a `std` module may
    /// hold the trait, and past a head this compiler knows it does not claim
    /// to know the rest. **A bare name is `NK1117`'s**, and it is declared by
    /// the three traits the language itself names (`Error`, Part I 7.1; `Drop`
    /// and `Cleanup`, Part I 6), a `trait` in any ledger this program reads, a
    /// `use` that brings it in, or a foreign name. Everything this cannot see
    /// is a name it must not refuse ([Part III
    /// C.4](../../docs/specification/30-nikaia-tooling.md)).
    fn a_trait_nothing_declares(&mut self, written: &str, span: &Span) {
        if let Some((head, _)) = written.split_once("::") {
            return self.a_head_nothing_declares(head, written, span);
        }
        let suffix = format!("::{written}");
        let in_a_ledger = |traits: &BTreeMap<String, BTreeSet<String>>| {
            traits
                .keys()
                .any(|key| key == written || key.ends_with(&suffix))
        };
        let imported = |parsed: &Parsed| {
            parsed.program.items.iter().any(|item| {
                matches!(&item.node, Item::Import { path, alias }
                if alias.map_or_else(
                    || path.last().is_some_and(|last| parsed.text(*last) == written),
                    |alias| parsed.text(alias) == written,
                ))
            })
        };
        let shape = SHAPE_BOUNDS.contains(&written);
        let declared = (!shape && ["Error", "Drop", "Cleanup"].contains(&written))
            || in_a_ledger(&self.own.traits)
            || in_a_ledger(&self.library.traits)
            || declares_a_type(self.parsed, written)
            || self
                .beside
                .iter()
                .any(|other| declares_a_type(other, written))
            || imported(self.parsed)
            || self.foreign_names.contains(written);
        if declared {
            return;
        }
        let because = match shape {
            // **A shape bound is answered by the declaration**
            // ([ADR-088](../../docs/specification/adr/adr-088.md) D2): a
            // `struct` is a `Struct` by being one, so there is no trait to
            // implement and no `impl` that could say it.
            true => format!(
                "`{written}` is answered by what a type is declared as, not by an `impl`: a \
                 `struct` is a `Struct` and an `enum` is an `Enum` already (ADR-088 D2)"
            ),
            false => format!(
                "an `impl` names a trait the program declares with `trait {written} {{ … }}`, \
                 one a package it depends on declares, or `Error`, `Drop` or `Cleanup`, \
                 which the language names itself (Part I, 4.7)"
            ),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1117",
            message: format!("nothing declares a trait called `{written}`"),
            notes: vec![because],
            help: Some(match shape {
                true => "leave the `impl` out: the bound is already answered".to_string(),
                false => format!(
                    "declare `trait {written}`, or write the path of the package that does: \
                     `package::{written}`"
                ),
            }),
        });
    }

    /// **`NK1181`: nothing declares the head of this path.**
    ///
    /// [`Checker::nothing_declares_it`] one segment down, and the same
    /// fail-safe direction: `nowhere::wobble` used to lower, and the language
    /// below answered *failed to resolve: use of unresolved module or unlinked
    /// crate* — a sentence with *crate* in it about a file nobody wrote
    /// ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **What a head may legally be**, asked in order and every one of them
    /// something this compiler has already read: a **type** declared here or
    /// recorded by either ledger, a **module** of this package, a **package**
    /// the manifest declares, a **crate** a description covers, a `std`
    /// module, a **grammar**, an opaque handle, or a type parameter. A ledger
    /// that records *anything* under `head::` answers too, because a crate is
    /// known by its items rather than by a list of its names.
    ///
    /// **Anything this cannot see is a name it must not refuse**
    /// ([Part III C.4](../../docs/specification/30-nikaia-tooling.md)), which
    /// is why the list fails open rather than closed: a head nobody has told
    /// this compiler about is the one case, and every other reading of an
    /// absent key had to be ruled out first.
    fn a_head_nothing_declares(&mut self, head: &str, written: &str, span: &Span) {
        let head = self.parsed.unaliased(head).to_string();
        let head = head.as_str();
        let prefix = format!("{head}::");
        fn under<V>(table: &BTreeMap<String, V>, prefix: &str) -> bool {
            table
                .range(prefix.to_string()..)
                .next()
                .is_some_and(|(key, _)| key.starts_with(prefix))
        }
        let declared = self.structs.contains_key(head)
            || self.enums.contains_key(head)
            // **And a type the package's other files declare.** The files of a
            // package share one namespace (Part I 9.1), so `Shade::Even` in
            // `main.nika` names an `enum` that may stand in `shapes.nika` —
            // and the three tables above are this **unit's**. Without this the
            // refusal is a correct program refused across a file boundary,
            // which is [Part III C.4](../../docs/specification/30-nikaia-tooling.md)
            // and the shape [ADR-182](../../docs/specification/adr/adr-182.md)'s
            // package had one construct over.
            || self.beside.iter().any(|other| declares_a_type(other, head))
            || self.grammars.contains_key(head)
            || self.variant_owner.contains_key(head)
            || self.type_parameters.contains_key(head)
            || self.opaque_handles.contains(head)
            || self.foreign_names.contains(head)
            || self.own.types.contains_key(head)
            || self.library.types.contains_key(head)
            || self.modules.contains(head)
            || self.std_modules.contains(head)
            // A ledger that records anything under this head knows the head:
            // a described crate is a list of its **items**, and no table
            // carries the crate's own word on a line of its own.
            || under(&self.own.functions, &prefix)
            || under(&self.own.types, &prefix)
            || under(&self.library.functions, &prefix)
            || under(&self.library.types, &prefix);
        if declared {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1181",
            message: format!("nothing declares `{head}`, which `{written}` is written under"),
            notes: vec![
                "a name in front of a `::` is a type, a module of this package, a package the \
                 manifest declares, a crate a description covers, or a `std` module - and this \
                 compiler has read all five, so one that answers to none of them is a name \
                 nobody has written down (Part I, 9.1)"
                    .to_string(),
            ],
            help: Some(format!(
                "declare `{head}`: a `.nika` file beside this one gives the package a module of \
                 that name, a `[dependencies]` line gives it a package, and \
                 `nikaia describe {head}` gives it a foreign crate"
            )),
        });
    }

    /// Part I 3.5: `?.` is for a value that may be absent.
    ///
    /// `"Ada"?.len` reaches through something that cannot be missing, and the
    /// language below has no `map` on it - so this was `rustc`'s refusal about
    /// the generated file (Part III, C.1). **`NK1121`**, and the way out is the
    /// plain `.`, which is what the program meant.
    ///
    /// **Only where the receiver's type is known.** A receiver this checker
    /// could not work out says nothing: refusing there would refuse a correct
    /// program, which is the one thing it may never do (Part III, C.4).
    ///
    /// `member` is the field or the method, and `Member` says which: the way out
    /// is the plain `.`, and a reader is owed it in the spelling they wrote
    /// ([ADR-066](../../../docs/specification/adr/adr-066.md)).
    fn reaches_through_a_plain_value(&mut self, on: &Ty, member: Reached<'_>, span: &Span) {
        if on.is_unknown() {
            return;
        }
        let (what, plain) = match member {
            Reached::Field(name) => ("field", format!(".{name}")),
            Reached::Method(name) => ("method", format!(".{name}(…)")),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1121",
            message: format!("`?.` reaches through a `{on}`, which cannot be absent"),
            notes: vec![format!(
                "`?.` exists for a nullable type - it reaches the {what} only where there \
                 is something to reach it on, and answers `null` otherwise (Part I, 3.5). \
                 A type that is not `T?` always has a value"
            )],
            help: Some(format!("write `{plain}`")),
        });
    }

    /// **Part I 2.3: a `T?` is a type of its own, and `.` is not one of its
    /// members** ([ADR-066](../../../docs/specification/adr/adr-066.md) D6).
    ///
    /// `NK1125`, and it is [`Checker::reaches_through_a_plain_value`] the other
    /// way round: that one refuses a `?.` where there is nothing to reach
    /// through, this one refuses a plain `.` where there is.
    ///
    /// **What the other languages do here is crash.** In a language where
    /// `null` inhabits every reference type, `a?.b.c` guards `a` and leaves
    /// `a.b` unguarded, so a `null` there is a `NullReferenceException` at run
    /// time. This language has no such value to crash on: types are
    /// non-nullable by default and `T?` is a **separate type**, so `.c` on one
    /// is a member the type does not have — the same kind of mistake as `.c` on
    /// an `i64`, and answerable where it is written.
    ///
    /// So the short-circuit is unchanged and is not what this is about: `?.`
    /// still stops at its own member and still answers `null`. What changes is
    /// that the *next* access is refused rather than emitted, which is what
    /// `find(1)?.b.c` used to become —
    /// `find(1).map(|it| it.b).c`, a field read off an `Option` (Part III C.1).
    fn reaches_into_a_nullable(&mut self, on: &Ty, member: Reached<'_>, span: &Span) {
        let (what, safe) = match member {
            Reached::Field(name) => ("field", format!("?.{name}")),
            Reached::Method(name) => ("method", format!("?.{name}(…)")),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1125",
            // **A `?` whose inside has no type is not written `??`**
            // (0.0.230, `open-work.md` §1.18 before it): that is the operator, and a reader told
            // "`??` may be absent" is told about the wrong thing. What is known
            // is that the value may be absent, so that is what is said.
            message: match on {
                Ty::Nullable(inner) if inner.is_unknown() => {
                    format!("this value may be absent, so it has no {what} to reach")
                }
                _ => format!("`{on}` may be absent, so it has no {what} to reach"),
            },
            notes: vec![
                "a `T?` is a type of its own and not a `T` that might be missing \
                 (Part I, 2.3), so a member of `T` is not a member of it - which is \
                 why there is no null reference to fail on here"
                    .to_string(),
            ],
            help: Some(format!(
                "write `{safe}`, which answers `null` where there is nothing to reach \
                 on - or end the chain with `??` and reach into the value it gives"
            )),
        });
    }

    /// **`NK1126`: a member reached on a type parameter, which has no bound.**
    ///
    /// `fn shout[T](x: T) -> String { return x.to_uppercase() }` is the program.
    /// It has a `T`, the `T` has no bound, and a value of it can therefore be
    /// moved and passed and nothing else - so `to_uppercase` is not a member of
    /// it, in the same way that it is not a member of an `i64`.
    ///
    /// **This is the refusal that makes writing the `<T>` worth anything**
    /// ([ADR-074](../../docs/specification/adr/adr-074.md) D5). Emitting the
    /// parameter without it turns *"every generic function fails in `rustc`"*
    /// into *"every generic function whose body uses its parameter fails in
    /// `rustc`"* - the same [Part III C.1](../../docs/specification/30-nikaia-tooling.md)
    /// class one size smaller, because the message is still `rustc`'s, about a
    /// file nobody wrote, saying `T` in the current scope.
    ///
    /// Why it is not a warning: the alternative is a program this compiler
    /// accepts and the backend refuses, which is the state the whole of
    /// Part III C is about. And why it says *"no bound"* rather than *"no such
    /// method"*: the method may well exist on every type the caller will ever
    /// pass, and what is missing is the sentence that says so.
    /// The **bound** that answers a member reached on a type parameter, if one
    /// does ([ADR-078](../../docs/specification/adr/adr-078.md) D3).
    ///
    /// `fn shout[T: Summarize](x: T)` and `x.summary()`: `T`'s bound names
    /// `Summarize`, the trait declares `summary`, and the ledger records that
    /// signature under `Summarize::summary` — the same key shape an `impl`'s
    /// methods get, because a bound and a receiver ask one question. So this
    /// hands back a **type to look the call up on**, and the whole of the rest
    /// of a call is unchanged: arity, argument types, `sync`, `throws` and the
    /// binding of the signature's variables all happen exactly as they do for a
    /// receiver whose type was written down.
    ///
    /// The first bound that declares the member wins, and `[T: A + B]` where
    /// both declare it is not a question this can answer — Rust's own answer is
    /// that the call is ambiguous, and nothing here can write the
    /// disambiguation. That is §4's, not this.
    fn bound_that_answers(&self, parameter: &str, member: &str) -> Option<Ty> {
        let bounds = self.type_parameters.get(parameter)?;
        bounds
            .iter()
            .find(|bound| {
                self.own
                    .traits
                    .get(bound.as_str())
                    .is_some_and(|methods| methods.contains(member))
            })
            .map(Ty::named)
    }

    fn nothing_says_what_a_parameter_can_do(
        &mut self,
        parameter: &str,
        member: Reached<'_>,
        span: &Span,
    ) {
        if !self.type_parameters.contains_key(parameter) {
            return;
        }
        let (what, name) = match member {
            Reached::Field(name) => ("field", name),
            Reached::Method(name) => ("method", name),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1126",
            message: format!(
                "`{parameter}` stands for a type the caller picks, and nothing says it has a \
                 {what} `{name}`"
            ),
            notes: vec![
                "a type parameter with no bound can be moved and passed and nothing \
                 else, because every type the caller may pick has to answer for what \
                 the body does (Part I, 4.6)"
                    .to_string(),
            ],
            help: Some(format!(
                "write the type the value actually has, or take `{parameter}` out and \
                 declare the parameter as that type - a bound that says which types \
                 `{parameter}` may be is not built yet"
            )),
        });
    }

    /// **`NK1131`: a field of a borrowed subject, handed out by value.**
    ///
    /// ```nika
    /// impl User {
    ///     fn name_of(&self) -> String {
    ///         return self.username     // error[NK1131]
    ///     }
    /// }
    /// ```
    ///
    /// `&self` borrows the subject, so what the body has is a loan of it; giving
    /// the `username` away by value would take a piece out of something it does
    /// not own. The language below says
    /// *"cannot move out of `self.username` which is behind a shared reference"*
    /// about a file nobody wrote — and
    /// [Part I 6.8](../../docs/specification/10-nikaia-light.md) is what decides
    /// that this is a refusal rather than something to paper over: *ownership
    /// rules occasionally reject code, every such error explains itself in plain
    /// language, and a raw internal error reaching you is a Nikaia bug.*
    ///
    /// **Not a hidden `.clone()`**, which was the other option and is against a
    /// decision already made: [ADR-064](../../docs/specification/adr/adr-064.md)
    /// D2 wrote *a hull you can see is one you write*, and a copy the author
    /// cannot see is the same thing one position over. Both ways out already
    /// exist and both are one word — `.clone()`, written where it happens, or a
    /// `self` receiver where the method is meant to consume its subject.
    ///
    /// **Asked only where the field's type is known and does not copy.** A
    /// number, a `bool`, a `char` and a view copy, so handing one out takes
    /// nothing away; a field this compiler cannot type says nothing, and
    /// [Part III C.4](../../docs/specification/30-nikaia-tooling.md) is why that
    /// is silence rather than a guess.
    /// The ledger key `Json.value(input)` enters through, where the receiver
    /// names a grammar of this file and the method one of its `pub` rules
    /// ([ADR-082](../../docs/specification/adr/adr-082.md) D1, D2).
    ///
    /// `None` for everything else, which is every other method call: a grammar
    /// name is not a value, so there is nothing to confuse this with.
    fn grammar_entry(&self, receiver: &Expr, method: Ident) -> Option<String> {
        let Expr::Variable(name) = receiver else {
            return None;
        };
        let grammar = self.parsed.text(*name).to_string();
        let rule = self.parsed.text(method).to_string();
        self.grammars
            .get(&grammar)
            .filter(|rules| rules.contains(&rule))
            .map(|_| format!("{grammar}::{rule}"))
    }

    /// The same key off a **path**, which is how a grammar is entered
    /// ([ADR-140](../../docs/specification/adr/adr-140.md) D3): `Json::value(x)`
    /// names the grammar `Json` and its `pub` rule `value`.
    ///
    /// A grammar's name is a name and a rule of it is reached the way every
    /// other qualified name is. The dot is for a **value's** members, and a
    /// namespace behind one was the single place this language asked a reader
    /// to tell two things apart by what the left side happens to be.
    fn grammar_path(&self, name: &str) -> Option<String> {
        let (grammar, rule) = name.split_once("::")?;
        self.grammars
            .get(grammar)
            .filter(|rules| rules.contains(rule))
            .map(|_| name.to_string())
    }

    /// `NK1147`: a grammar's rule reached through a dot
    /// ([ADR-140](../../docs/specification/adr/adr-140.md) D3).
    ///
    /// Raised only where the receiver **is** a grammar of this file and the
    /// name **is** one of its rules, so the message can carry the whole
    /// rewrite — and so that a method call on an ordinary value named like a
    /// grammar is untouched.
    fn a_grammar_reached_through_a_dot(&mut self, grammar: &str, rule: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1147",
            message: format!("a rule of grammar `{grammar}` is reached with `::`, not with a dot"),
            notes: vec![
                "a grammar's name is a name, and a rule of it is a qualified name like \
                 every other (ADR-140 D3). The dot is for a **value's** members, and a \
                 namespace behind one was the single place this language asked a reader \
                 to tell two things apart by what the left side happens to be"
                    .to_string(),
            ],
            help: Some(format!("write `{grammar}::{rule}(…)`")),
        });
    }

    /// The entry call itself: the input is an expression like any other, and
    /// what comes back is what the rule declares.
    ///
    /// **Whether it can fail comes from the contract**, which is the whole of
    /// what D2 changed: a `pub` rule is an entry in the ledger with
    /// `throws = ["?"]`, because a rule past a commit point can fail
    /// ([ADR-023](../../docs/specification/adr/adr-023.md) D9). The old form
    /// was not a call and carried no contract, so `NK1134` had to be told in
    /// one line ([ADR-091](../../docs/specification/adr/adr-091.md) D4); that
    /// line is gone with it.
    fn grammar_call(&mut self, key: &str, args: &[Expr], span: &Span) -> Ty {
        for arg in args {
            self.expr(arg, span);
        }
        let fallible = self
            .own
            .functions
            .get(key)
            .is_some_and(|c| !c.throws.is_empty());
        if fallible && let Some(guarded) = &mut self.guarded {
            guarded.fallible = true;
        }
        self.own
            .functions
            .get(key)
            .and_then(|c| c.signature.as_ref())
            .and_then(|s| s.result.clone())
            .unwrap_or(Ty::Unknown)
    }

    /// **The compiler writes the reference at the call, and a written one is
    /// refused** ([ADR-094](../../docs/specification/adr/adr-094.md) D1).
    ///
    /// The callee's answer is `contracts::keeps::lends`, read here and read
    /// again by the emitter when it writes that callee's *declaration* — one
    /// answer in two positions, because the two disagreeing is a `&&T` or a
    /// moved value in the language below. What this adds is the half the
    /// emitter cannot see: whether the argument is **already** a view, which is
    /// a question about its type (ADR-028).
    ///
    /// **`NK1137` lands exactly where the compiler would have written one**,
    /// and nowhere else. A `&` in a position the callee keeps, or one in front
    /// of a value whose type this checker could not work out, is still the
    /// program's own and is left alone — refusing it would take away the only
    /// way to say what the line means before the inference that replaces it can
    /// answer.
    fn the_compiler_writes_the_reference(
        &mut self,
        contract: &FnContract,
        argument: Argument<'_>,
        written: &str,
        span: &Span,
    ) -> bool {
        let Argument {
            at,
            given,
            found,
            want,
        } = argument;
        // `arguments()` drops the receiver, and the contract's positions do
        // not, so a method's argument sits one further along.
        let position = at
            + usize::from(
                contract
                    .signature
                    .as_ref()
                    .is_some_and(|s| s.takes_a_receiver()),
            );
        // **`mut` is asked first, and it is a declaration** (D3): the
        // parameter lowers to `&mut T` and the argument gains a `&mut`, both
        // off the word the author wrote. `lends` withholds its claim on such a
        // position, so exactly one of the two answers.
        let changes = contract
            .signature
            .as_ref()
            .and_then(|s| s.params.get(position).map(|(name, _)| (s, name)))
            .is_some_and(|(s, name)| s.mutable.iter().any(|m| m == name));
        if changes {
            let Some(given) = given else {
                return false;
            };
            // **A reference that is already there is not written twice.** Where
            // the argument is a `mut` parameter of the function this call stands
            // in, the binding *is* a `&mut T`, so the call hands it straight on
            // and the language below reborrows it. Writing the `&mut` anyway is
            // not a `&mut &mut T` that works by deref: a `&mut` may only be
            // taken of a binding that is itself `mut`, and a parameter is not
            // one, so `rustc` refuses it — *cannot borrow `connection` as
            // mutable, as it is not declared as mutable* — about a file nobody
            // wrote ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
            //
            // `true` and not `false`: nothing is left for the positions below to
            // decide. This **is** the fit — the argument already has the shape
            // the parameter asked for — and falling through would measure a
            // `Connection` against a `Connection` and then look for a wrap.
            //
            // Only a bare name. `&mut c.field` for a `mut c` is the reference
            // the call wants, and `rooted_at` would have said `c` for it.
            if matches!(given, Expr::Variable(name)
                if self.binding(self.parsed.text(*name)).is_some_and(|l| l.changing))
            {
                return true;
            }
            self.checked
                .mut_args
                .entry((span.start, written.to_string(), at))
                .or_default()
                .insert(argument_shape(given));
            return true;
        }
        if !crate::contracts::keeps::lends(contract, position) {
            return false;
        }
        let Some(given) = given else {
            return false;
        };
        let wrote_a_reference = matches!(
            given,
            Expr::Unary {
                op: crate::ast::UnaryOp::Ref,
                ..
            }
        );
        // **Only where the call would be right with the reference in it.** A
        // `&i64` handed to a `&Request` is `NK1102` and stays one: saying *the
        // `&` here is the compiler's* about an argument that is the wrong value
        // would send the reader to fix the punctuation of a line whose type is
        // wrong.
        //
        // The hull's path counts as a fit: a `Shared[Conn]` reaches a `&Conn`
        // through its deref ([ADR-040](../../docs/specification/adr/adr-040.md)
        // D5), and that fit was the *source's* `&` before this.
        //
        // **Which fit to ask depends on which of the two kinds this is.** A
        // parameter written `&str` is a view in the declaration already, so
        // what has to fit it is the argument *with* the reference the compiler
        // writes — a `String` becomes a `&String` and reaches `&str` the way
        // the language below reaches it. A parameter written `Stats` and
        // inferred lent gains its `&` in the declaration too, so both sides
        // move together and the fit is the one it always was.
        let fits = match want.is_a_view() {
            true => {
                // **`view_of` and not a `&` pasted on the front**, because
                // a view of a `String` is a `&str` — the one rewrite Part I 6.5
                // makes, and the same one this checker performs for a `&`
                // somebody writes. Keeping the name refused `count(dna)` for a
                // `dna` every caller had been writing `count(&dna)` for.
                let lent = view_of(found);
                lent.fits(want) || self.fits_through_deref(&lent, want)
            }
            false => {
                found.fits(want)
                    || self.fits_through_deref(found, want)
                    // **A `&` the source wrote is already the view**, and this
                    // parameter is not one: what has to fit `want` is what the
                    // `&` was put in front of, because that is the expression
                    // the compiler would have written its own `&` before. Asked
                    // the other way, `width(&t)` for a `text: String` that
                    // `width` only reads came back `&str` against `String` and
                    // was answered with `NK1102`'s *write `.to_string()`* — a
                    // help that sends the reader the wrong way about a line
                    // whose only fault is a character the compiler writes.
                    || (wrote_a_reference
                        && viewed_from(found).iter().any(|owned| {
                            owned.fits(want) || self.fits_through_deref(owned, want)
                        }))
            }
        };
        if !fits {
            return false;
        }
        if wrote_a_reference {
            // **A type this compiler could not work out is not one to refuse
            // punctuation over.** `Ty::Unknown` fits everything, which is the
            // right answer for an absent claim
            // ([ADR-024](../../docs/specification/adr/adr-024.md) D1) and no
            // ground to stand on here: `route(&n)` for an `n` whose type was
            // never pinned would be refused on the strength of a fit that means
            // *nothing was checked*, which is [Part III
            // C.4](../../docs/specification/30-nikaia-tooling.md)'s correct
            // program refused. Left alone, the `&` the source wrote is the same
            // character the compiler would have written, so the lowering is the
            // one either way.
            //
            // **Which is why the test sits here and not above the fit.** The
            // declaration is written off `lends` alone, so anything this
            // refuses to record is a call whose callee takes a `&T` and whose
            // argument does not have one - `rustc` about a file nobody wrote.
            // A condition that skips the *recording* may only be one the
            // emitter shares; this one skips the *refusal*, and the source's
            // own `&` stands in for what would have been recorded.
            if matches!(found, Ty::Unknown) {
                return false;
            }
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK1137",
                message: "the `&` here is the compiler's to write".to_string(),
                notes: vec![format!(
                    "`{written}` reads this argument rather than keeping it, so the \
                     reference is what the call already means (ADR-094 D1) - and which \
                     of the two it is comes from the callee's body, not from this line"
                )],
                help: Some("take the `&` off".to_string()),
            });
            return true;
        }
        // A value that is already a view needs nothing: the declaration and the
        // argument agree without a second `&`.
        if found.is_a_view() {
            return false;
        }
        self.checked
            .lent_args
            .entry((span.start, written.to_string(), at))
            .or_default()
            .insert(argument_shape(given));
        true
    }

    /// The name a place is **rooted** at: `out` for `out`, `out.f` and
    /// `out[i].f`, and nothing for anything that is not a place.
    ///
    /// A field of a parameter is the parameter's, which is why the whole chain
    /// is followed rather than only its last step: `row.total = 0` changes
    /// `row` ([ADR-094](../../docs/specification/adr/adr-094.md) D3).
    fn rooted_at(&self, place: &Expr) -> Option<String> {
        match place {
            Expr::Variable(name) => Some(self.parsed.text(*name).to_string()),
            Expr::Field { base, .. } | Expr::SafeField { base, .. } => self.rooted_at(base),
            Expr::Index { base, .. } => self.rooted_at(base),
            _ => None,
        }
    }

    /// The name a free call names, unaliased — or nothing where this is not one.
    fn free_callee(&self, func: &Expr) -> Option<String> {
        let name = match func {
            Expr::Variable(name) => self.parsed.text(*name).to_string(),
            Expr::Path(segments) => segments
                .iter()
                .map(|s| self.parsed.text(*s))
                .collect::<Vec<_>>()
                .join("::"),
            _ => return None,
        };
        Some(self.parsed.unaliased(&name))
    }

    /// **`NK2203`: a lock taken while a lock is held**
    /// ([ADR-039](../../docs/specification/adr/adr-039.md) D2, D3).
    ///
    /// Part II 12.3 called manual nesting an anti-pattern and *"often a
    /// compile-time error"*; D2 makes *often* into **always**, and with that no
    /// program exists in which a lock's two representations behave differently
    /// — which is the whole reason the switch may pick one.
    ///
    /// **Written one inside the other, or reached through a chain of calls**,
    /// and the second is what needs the column: `contracts::locks` propagates
    /// *touches a lock* over the call graph, so a callee three deep is the same
    /// answer as one written here.
    ///
    /// **`Holds` and nothing else.** `Undecided` is not permission
    /// ([ADR-010](../../docs/specification/adr/adr-010.md) D1) and not a
    /// refusal either, because Stage 0 knows the type of rather less than half
    /// of what a program writes, and refusing on doubt is [Part III
    /// C.4](../../docs/specification/30-nikaia-tooling.md)'s correct program
    /// refused. What a silent `Undecided` costs is the runtime check, which is
    /// where every program already is.
    ///
    /// **A `println` is one of these**, and that is the case
    /// [ADR-067](../../docs/specification/adr/adr-067.md) D1 was written about:
    /// it never pauses, so `sync` says nothing about it, and it takes standard
    /// output's own lock while yours is open. Two conditions and not one.
    fn a_lock_inside_a_lock(&mut self, called: &str, holds: crate::contracts::Lock, span: &Span) {
        if !self.inside_a_door || !holds.holds() {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2203",
            message: format!("`{called}` takes a lock, and this runs with one already held"),
            notes: vec![
                "a door's block runs with the lock open, so a second one taken inside it \
                 is a lock inside a lock - which is a deadlock rather than a risk \
                 (ADR-039 D2)"
                    .to_string(),
                "what a function reaches is its own column in the ledger (13.5), so this \
                 is answered through a chain of calls as well as for one written here"
                    .to_string(),
            ],
            help: Some(
                "ask for both at once - `access_all(a, b) fn(x, y) { … }` - or compute \
                 outside the block and hand the value in"
                    .to_string(),
            ),
        });
    }

    /// **`NK2201`: I/O while holding locked data**
    /// ([ADR-067](../../docs/specification/adr/adr-067.md) D1,
    /// [ADR-169](../../docs/specification/adr/adr-169.md) D2).
    ///
    /// ADR-067 D1 split *no I/O while holding locked data* in two — what
    /// **pauses** is `NK2202`'s and what **takes a lock** is `NK2203`'s — and
    /// left the third case as a question: *is there I/O that does neither?*
    ///
    /// There is exactly one, and it is not a call. `fs::Mapped` is a file held
    /// as memory, so `mapped[i]` is a **page fault**: a disk read with nothing
    /// in the source to hang a `touches` on, which neither suspends nor takes a
    /// lock. Inside an open door that is a disk read with the lock held, and
    /// before this nothing said a word.
    ///
    /// **Read off the type's own column, never a list of names**
    /// ([ADR-169](../../docs/specification/adr/adr-169.md) D1): a type whose
    /// `touches` names a file says so in the ledger, and a second such type
    /// needs a line there and nothing here.
    ///
    /// **Silence is not a claim.** A type with no `touches` recorded is one
    /// nobody answered for, and refusing on that would refuse correct programs
    /// ([Part III C.4](../../docs/specification/30-nikaia-tooling.md)).
    fn io_inside_a_door(&mut self, on: &Ty, what: &str, span: &Span) {
        if !self.inside_a_door {
            return;
        }
        let Ty::Named { name, .. } = on else {
            return;
        };
        // **By the last segment**, which is what the index site above does and
        // for the same reason: a `std` type carries its module in the ledger's
        // key since [ADR-154](../../docs/specification/adr/adr-154.md) D3, while
        // a signature writes `-> Mapped` — so `fs::Mapped` and `Mapped` are the
        // one type reached from two sides.
        let base = crate::contracts::ty::base(name);
        let touched = self
            .library
            .types
            .iter()
            .filter(|(key, _)| crate::contracts::ty::base(key) == base)
            .any(|(_, contract)| contract.touches.iter().any(|t| t.starts_with("file")));
        if !touched {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2201",
            message: format!("{what} reads a file, and this runs with a lock held"),
            notes: vec![
                format!(
                    "`{name}` is a file held as memory, so reading it is a page fault - \
                     a disk read, which is what a door's block may not wait for \
                     (Part II, 12.2)"
                ),
                "it is neither a pause nor a second lock, which is why `NK2202` and \
                 `NK2203` say nothing about it (ADR-067 D1)"
                    .to_string(),
            ],
            help: Some(
                "read what you need before the door and hand the value in, so the block \
                 works on memory that is already there"
                    .to_string(),
            ),
        });
    }

    /// **`NK1141`: an `update` block hands a value back**
    /// ([ADR-110](../../docs/specification/adr/adr-110.md) D1).
    ///
    /// `update` used to take the value by value and put back what the block
    /// returned; D1 hands it the address instead, so *there is nothing to
    /// return*. Without this the old shape — `kasse.update fn(old) { old + 1 }`
    /// — lowers to a closure whose value is an `i64` where `()` is wanted, and
    /// the answer comes from `rustc` about a file nobody wrote
    /// ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **Asked of the shape and not of the type**, and only where the shape
    /// says so outright: a last statement that is a call, a method call or an
    /// assignment is left alone, because whether *those* come to a value is a
    /// question this walk would have to type the call to answer, and answering
    /// it wrongly refuses a correct program (C.4). What is refused is a last
    /// statement that can only be a value — a name, a literal, an operator, a
    /// field — and a `return` that carries one.
    fn an_update_block_returns_nothing(&mut self, body: &Block, span: &Span) {
        let hands_back = body.stmts.iter().any(|stmt| match &stmt.node {
            Stmt::Return(Some(_)) => true,
            Stmt::Expr(value) => {
                // The last statement is the block's value; an expression
                // statement anywhere else is evaluated and dropped.
                std::ptr::eq(stmt, body.stmts.last().expect("there is one"))
                    && plainly_a_value(value)
            }
            _ => false,
        });
        if !hands_back {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1141",
            message: "an `update` changes its value; there is nothing to return".to_string(),
            notes: vec![
                "the block is handed the value where it lies and changes it in place \
                 (ADR-110 D1), so what it comes to is not put anywhere"
                    .to_string(),
            ],
            help: Some(
                "change it instead: `fn(mut v) { v += 1 }` rather than `fn(v) { v + 1 }`"
                    .to_string(),
            ),
        });
    }

    /// One lambda parameter, bound — and refusable where it is changed without
    /// `mut` ([ADR-110](../../docs/specification/adr/adr-110.md) D1).
    ///
    /// `kasse.update fn(mut v) { v += 100 }` is where the word earns its place:
    /// the block changes the value in place and the caller whose value changes
    /// is the **lock**. D1 says a block that changes `v` without the word is
    /// `NK1138`, and it says it **about a door** — which is where this asks it
    /// and nowhere else.
    ///
    /// **Held for every lambda it would refuse correct programs**, and the
    /// corpus is what said so: `par_fold(…, fn(acc, m) { acc.record(m) })` in
    /// `examples/1brc.nika` changes `acc` and has no `mut`, and it compiles,
    /// because the **emitter** writes the word itself where it recognises a
    /// fold's accumulator; `and_modify fn(tally) { tally.bump() }` in
    /// `k-nucleotide.nika` is the same shape one library over. So *a lambda
    /// parameter changed without `mut` is already broken Rust* is false, and a
    /// rule built on it would be [Part III
    /// C.4](../../docs/specification/30-nikaia-tooling.md)'s correct program
    /// refused. Widening it is a question for whoever takes that `mut` out of
    /// the emitter, and it belongs with that work rather than ahead of it.
    fn lambda_parameter(
        &mut self,
        name: winnow_grammar::Symbol,
        mutable: &[winnow_grammar::Symbol],
        ty: Ty,
        span: &Span,
    ) -> Local {
        let asked = self.at_a_write_door && !mutable.contains(&name);
        Local {
            literal: None,
            name: self.parsed.text(name).to_string(),
            ty,
            constant: None,
            lent: false,
            changing: false,
            built: None,
            empty_list: None,
            immutable: asked.then(|| Immutable {
                at: span.clone(),
                kind: Kind::Parameter,
            }),
        }
    }

    /// **Whether a type's values have a cleanup**, and whether it can fail
    /// ([ADR-239](../../docs/specification/adr/adr-239.md) D1): an `impl
    /// Cleanup` for it in any ledger this build reads - or for what it holds,
    /// in a field, an element or a nullable, which dies with it. A view is not
    /// the value, and dies nowhere. The type named is the one with the
    /// cleanup; the answer fails where any cleanup it holds can.
    fn cleanup_of(&self, ty: &Ty) -> Option<(String, bool)> {
        self.cleanup_within(ty, &mut BTreeSet::new())
    }

    fn cleanup_within(&self, ty: &Ty, seen: &mut BTreeSet<String>) -> Option<(String, bool)> {
        let held: Vec<Ty> = match ty.unseen() {
            Ty::Named {
                name,
                args,
                view: false,
            } => {
                let base = crate::contracts::ty::base(&name).to_string();
                if !seen.insert(base.clone()) {
                    return None;
                }
                let owner = [self.own, self.library].into_iter().find_map(|ledger| {
                    ledger
                        .implementations
                        .get("Cleanup")?
                        .iter()
                        .find(|t| crate::contracts::ty::base(t) == base)
                        .cloned()
                });
                if let Some(owner) = owner {
                    let fails = [self.own, self.library].into_iter().any(|ledger| {
                        ledger
                            .functions
                            .get(&format!("{owner}::cleanup"))
                            .is_some_and(|contract| !contract.throws.is_empty())
                    });
                    return Some((owner, fails));
                }
                let fields = [self.own, self.library].into_iter().flat_map(|ledger| {
                    ledger
                        .types
                        .iter()
                        .filter(|(key, _)| crate::contracts::ty::base(key) == base)
                        .flat_map(|(_, contract)| contract.fields.iter().map(|f| f.ty.clone()))
                });
                args.into_iter().chain(fields).collect()
            }
            Ty::Nullable(inner) => vec![*inner],
            Ty::Tuple(parts) => parts,
            _ => return None,
        };
        held.iter()
            .filter_map(|inner| self.cleanup_within(inner, seen))
            .reduce(|(owner, fails), (_, more)| (owner, fails || more))
    }

    /// **A value with a cleanup may die in the function being walked**
    /// (ADR-239 D2): its body becomes a settle point's, and the settle is a call
    /// every analysis reads - one that pauses, and one that fails where the
    /// function says `throws` and the cleanup can fail.
    fn a_settle_in_this_function(&mut self, fails: bool) {
        let Some(current) = self.current.clone() else {
            return;
        };
        // **Whether the function's settle point can fail it**: only where a
        // cleanup can fail and the function says `throws`. Its channel is
        // otherwise whatever its body raises, and the settle point hands the
        // body's result back untouched.
        let failing = fails && self.throwing;
        *self.checked.settle_fns.entry(current).or_insert(false) |= failing;
        let key = match failing {
            true => "cleanup::settle_failing",
            false => "cleanup::settle",
        };
        self.reached_method(Some(key));
    }

    /// A value with a cleanup, made by a call or a literal (ADR-239 D2).
    fn a_value_with_a_cleanup_made(&mut self, ty: &Ty) {
        if let Some((_, fails)) = self.cleanup_of(ty) {
            self.a_settle_in_this_function(fails);
        }
    }

    /// **A value with a cleanup, bound by a `let`** (ADR-239 D2, D4): the
    /// block is settled where it ends, the settle is a pause wherever pausing
    /// is refused, and a cleanup that can fail is the function's to declare.
    fn a_value_with_a_cleanup_bound(&mut self, name: &str, ty: &Ty, span: &Span) {
        if let Some(fails) = self.a_value_with_a_cleanup_dying(name, ty, span) {
            self.checked.settle_lets.insert(span.start, fails);
        }
    }

    /// **A value with a cleanup dies in this function**, a `let`'s or a kept
    /// parameter's: the function settles it, the settle is a pause, and a
    /// cleanup that can fail is the function's to declare. Whether it can fail,
    /// where it has a cleanup at all.
    fn a_value_with_a_cleanup_dying(&mut self, name: &str, ty: &Ty, span: &Span) -> Option<bool> {
        let (owner, fails) = self.cleanup_of(ty)?;
        self.a_settle_in_this_function(fails);
        self.a_cleanup_that_may_pause(name, span);
        let is = match ty.unseen() {
            Ty::Named { name, .. }
                if crate::contracts::ty::base(&name) == crate::contracts::ty::base(&owner) =>
            {
                "is"
            }
            _ => "holds",
        };
        if fails && !self.throwing && !self.caught && self.current.is_some() {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2605",
                message: format!("this function can fail because the cleanup of `{name}` can fail"),
                notes: vec![format!(
                    "`{name}` {is} a `{owner}`, whose cleanup runs where it dies - at the \
                     end of this block - and can fail, so the function that owns it can \
                     fail with it (Part I 6.4, ADR-239 D3)"
                )],
                help: Some(match is {
                    "is" => format!(
                        "declare it with `throws`, or close it where the failure should be \
                         handled: `{name}.close() catch {{ … }}`"
                    ),
                    _ => format!(
                        "declare it with `throws`, or close the `{owner}` it holds where the \
                         failure should be handled, with `close() catch {{ … }}`"
                    ),
                }),
            });
        }
        Some(fails)
    }

    /// **`NK2601`: an `impl Cleanup` is the one method `fn cleanup(ref mut
    /// self)`** ([ADR-239](../../docs/specification/adr/adr-239.md) D1). The
    /// synchronous fallback a type may also have is its own `impl Drop`, which
    /// runs after the cleanup, or alone where the cleanup cannot run - so a
    /// `drop` written here is named and pointed there.
    fn a_cleanup_shaped(
        &mut self,
        target: &str,
        methods: &[crate::ast::Spanned<Item>],
        span: &Span,
    ) {
        let mut wrote_cleanup = false;
        for method in methods {
            let Item::Fn {
                name,
                receiver,
                args,
                config,
                spread,
                ret_type,
                ..
            } = &method.node
            else {
                continue;
            };
            let written = name.map(|n| self.parsed.text(n)).unwrap_or("");
            let (message, help) = match written {
                "cleanup" => {
                    wrote_cleanup = true;
                    let shaped = receiver.is_some_and(|r| r.is_ref && r.is_mut)
                        && args.is_empty()
                        && config.is_empty()
                        && spread.is_none()
                        && ret_type.is_none();
                    if shaped {
                        continue;
                    }
                    (
                        format!(
                            "`cleanup` of `{target}` takes `ref mut self` and nothing else, and \
                             hands nothing back"
                        ),
                        "write it `fn cleanup(ref mut self)`, with `throws` where it can fail"
                            .to_string(),
                    )
                }
                "drop" => (
                    format!("`drop` does not belong in `impl Cleanup for {target}`"),
                    format!(
                        "the synchronous fallback is its own `impl Drop for {target} {{ fn \
                         drop(ref mut self) {{ … }} }}`, which runs after `cleanup`, or alone \
                         where `cleanup` cannot run"
                    ),
                ),
                other => (
                    format!("`{other}` does not belong in `impl Cleanup for {target}`"),
                    format!("write it in `impl {target}`, where the type's own methods stand"),
                ),
            };
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: method.span.clone(),
                code: "NK2601",
                message,
                notes: vec![
                    "`impl Cleanup` has one method, `fn cleanup(ref mut self)`, which may \
                     pause and may fail (Part I 6.4)"
                        .to_string(),
                ],
                help: Some(help),
            });
        }
        if !wrote_cleanup {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2601",
                message: format!("`impl Cleanup for {target}` does not write `cleanup`"),
                notes: vec![
                    "`impl Cleanup` has one method, `fn cleanup(ref mut self)`, which may \
                     pause and may fail (Part I 6.4)"
                        .to_string(),
                ],
                help: Some("write `fn cleanup(ref mut self) { … }`".to_string()),
            });
        }
    }

    /// **`NK2107`: a lambda that runs on several cores at once changes a name
    /// they all share** (Part II 12.6,
    /// [ADR-235](../../docs/specification/adr/adr-235.md) D2).
    ///
    /// Every core runs the lambda on its own share of the elements, so a name
    /// bound outside it is one they would all write at once - a data race,
    /// which the language below refuses in its own words about a file nobody
    /// wrote (Part III, C.1). Asked at **both** settings of
    /// `user_parallelism`: at `no` the walk is one loop and the write would
    /// work, and a program that means something different at the two settings
    /// is what the setting promises never to make.
    fn a_shared_name_changed_in_parallel(&mut self, name: &str, span: &Span) {
        let Some(depth) = self.in_parallel else {
            return;
        };
        let declared = self
            .scope
            .iter()
            .rposition(|frame| frame.iter().any(|local| local.name == name));
        if declared.is_none_or(|at| at >= depth) {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2107",
            message: format!(
                "this lambda runs on several cores at once, and it changes `{name}`, which \
                 all of them share"
            ),
            notes: vec![
                "a `par_iter()` walk splits the elements across every core and runs the \
                 lambda on each share at the same time, so every core would write `{name}` \
                 together (Part II, 12.6)"
                    .replace("{name}", name),
            ],
            help: Some(format!(
                "hand each element's value out of the lambda and combine them after the \
                 walk - `map` and then `collect` or `count` - or keep `{name}` in a \
                 `SharedMut` and change it through `update`"
            )),
        });
    }

    /// **What is changed says `mut`** — `NK1138` for a parameter
    /// ([ADR-094](../../docs/specification/adr/adr-094.md) D3) and `NK1139` for
    /// a `let` ([Part I 2.1](../../docs/specification/10-nikaia-light.md)).
    ///
    /// One rule reached from two sides, and two codes because the word goes in
    /// a different place and a reader is doing a different thing. A parameter's
    /// `mut` also decides what the **caller** sees — the value it hands over is
    /// the one that changes — where a `let`'s is only about this body.
    ///
    /// **Both were `rustc`'s until now.** Part I 2.1 writes
    /// `// x = 20  <-- This would cause a Compiler Error` and this compiler was
    /// not the one giving it: the binding lowered without its `mut` and the
    /// answer came back about a file nobody wrote, which is [Part III
    /// C.1](../../docs/specification/30-nikaia-tooling.md).
    ///
    /// **Only where the change is certain.** A method whose entry no ledger
    /// has, or whose candidates do not agree, is not one to refuse on;
    /// answering *it might change* would refuse a correct program, which is C.4
    /// and the worse of the two mistakes.
    fn a_changed_binding_says_mut(&mut self, name: &str, how: &str) {
        self.changed.push(name.to_string());
        let Some(at) = self
            .binding(name)
            .and_then(|local| local.immutable.clone())
            .filter(|at| !self.said_mut.contains(&at.at.start))
        else {
            return;
        };
        let (code, what, where_the_word_goes, way_out) = match at.kind {
            Kind::Parameter => (
                "NK1138",
                "a parameter",
                "`mut` in the declaration is where in-place change is written, and \
                 the caller's value is what changes, exactly as it is for `&mut self` \
                 (ADR-094 D3)",
                // D3 names both ways out, and the second is the one that keeps
                // the caller's value as it was.
                format!(
                    "write `mut {name}` - or, if the caller's value should stay as it \
                     was, take a copy with `let mut {name}_own = {name}`"
                ),
            ),
            Kind::Let => (
                "NK1139",
                "a `let`",
                "a binding changes only where it says so (Part I, 2.1), and this one \
                 does not",
                format!("write `let mut {name}`"),
            ),
        };
        // Said once per binding: a body that changes one usually does so
        // several times, and three carets on one declaration is noise.
        self.said_mut.insert(at.at.start);
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: at.at,
            code,
            message: format!("`{name}` is changed, and {what} that is changed says `mut`"),
            notes: vec![format!("{how} - {where_the_word_goes}")],
            help: Some(way_out),
        });
    }

    /// **`NK1137`: the `&` is the compiler's to write**
    /// ([ADR-094](../../docs/specification/adr/adr-094.md) D4).
    ///
    /// A `for` lends what it iterates and a `let` over a place is a view of it,
    /// so a `&` written in front of either says what the line already means.
    /// Left alone it would be a second reference — `&&Vec<Entry>`, which Rust
    /// does not iterate — and reported about a file nobody wrote
    /// ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **Refused rather than absorbed**, which is D1's rule and is the whole
    /// point of the record: two spellings for one thing is the state a reader
    /// cannot tell a rule from a habit in. A `&` in a **declaration** is
    /// untouched (D6) — that is where it lives.
    ///
    /// **The `for` position only, and the `let` one is open work.** A `for`
    /// lends whatever place it is given, so a written `&` there is always the
    /// compiler's line said twice. A `let` lends only where the checker could
    /// *type* the place ([`Checked::lent_lets`]), and where it could not, the
    /// written `&` is the program's only way to say what the line means — so
    /// refusing it there would take away the escape hatch before the inference
    /// that replaces it exists. `open-work.md` carries that as the rest of D4.
    fn the_caller_writes_no_reference(&mut self, value: &Expr, span: &Span, because: &str) {
        if !matches!(
            value,
            Expr::Unary {
                op: crate::ast::UnaryOp::Ref,
                ..
            }
        ) {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1137",
            message: "the `&` here is the compiler's to write".to_string(),
            notes: vec![format!(
                "{because}, so the reference is already what this line means (ADR-094 D4) - written twice it is a reference to a reference, which the language below reports about a file nobody wrote"
            )],
            help: Some("take the `&` off".to_string()),
        });
    }

    /// Whether this value is a **view of the subject**: a place inside a
    /// borrowed `self`, handed back where the function declares a view.
    ///
    /// Purely about what the source wrote — the receiver, the declared result
    /// and the shape of the place — because it decides whether to *ask*
    /// `NK1131`, and asking that question types the expression. The types are
    /// still measured: [`Checker::returns`] compares the view against the
    /// declared result, so a field of the wrong type is `NK1104` as before.
    ///
    /// **A place inside the subject and not the subject itself.** `return self`
    /// out of a `ref self` method is a different sentence — the subject is
    /// already a view there, and nothing is owed.
    fn hands_back_a_view_of_the_subject(&self, value: &Expr) -> bool {
        if !self.borrowing_self {
            return false;
        }
        if !self.expected.as_ref().is_some_and(Ty::is_a_view) {
            return false;
        }
        let rooted_at_self = |place: &Expr| {
            matches!(place, Expr::Field { .. } | Expr::Index { .. })
                && self.rooted_at(place).as_deref() == Some("self")
        };
        rooted_at_self(value)
    }

    /// **`NK1188`: `==` on a type that does not compare**
    /// ([ADR-204](../../docs/specification/adr/adr-204.md) D3).
    ///
    /// The words were `rustc`'s about a file nobody wrote — *binary operation
    /// `==` cannot be applied to type `P`* — with *consider annotating `P` with
    /// `#[derive(PartialEq)]`* as the help, which is a way out the source cannot
    /// take: [Part III C.1 and
    /// C.2](../../docs/specification/30-nikaia-tooling.md) at once.
    ///
    /// **Asked of one side and only where its type is known.** A type this
    /// compiler could not work out compares, which is the answer an absent claim
    /// gets everywhere here ([Part III
    /// C.4](../../docs/specification/30-nikaia-tooling.md)); and the two sides
    /// disagreeing is `NK1102`'s business, one refusal over.
    fn a_type_that_does_not_compare(&mut self, left: &Ty, right: &Ty, span: &Span) {
        let asking = Comparable {
            structs: &self.structs,
            enums: &self.enums,
            payloads: &self.enum_payloads,
            own: self.own,
            library: self.library,
        };
        let Some(ty) = [left, right].into_iter().find(|ty| !asking.of(ty)) else {
            return;
        };
        let ty = ty.text();
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1188",
            message: format!("`{ty}` is not a type two values of which can be compared"),
            notes: vec![
                "a `struct` or an `enum` compares when every part of it does, and a type \
                 whose parts are the library's compares when the library says so - a \
                 lock, a mapping, a socket and a task's handle are the ones that do not \
                 (ADR-204 D1)"
                    .to_string(),
            ],
            help: Some(
                "compare the parts that carry the answer, or give the type a method that \
                 says what equality means for it"
                    .to_string(),
            ),
        });
    }

    fn a_field_of_a_borrowed_subject(&mut self, value: &Expr, span: &Span, what: &str) {
        if !self.borrowing_self {
            return;
        }
        let Expr::Field { base, name } = value else {
            return;
        };
        let Expr::Variable(subject) = base.as_ref() else {
            return;
        };
        if self.parsed.text(*subject) != "self" {
            return;
        }
        let field = self.parsed.text(*name).to_string();
        let ty = self.expr(value, span);
        if ty.is_unknown() || copies(&ty) {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1131",
            message: format!("`self` is borrowed here, so `{field}` cannot be {what} by value"),
            notes: vec![format!(
                "`&self` is a loan of the subject, and `{field}` is a `{}` - giving it \
                 away would take a piece out of something this method does not own \
                 (Part I, 6.5)",
                ty.text()
            )],
            // **[ADR-083](../../docs/specification/adr/adr-083.md) D2's two ways
            // out — and, where this is a `return`, the third that is now real.**
            //
            // The third stood here once and could not be taken: it said *declare
            // the result `&str` and write `return &self.field`*, and `&str`
            // stopped being a spelling at
            // [ADR-184](../../docs/specification/adr/adr-184.md) D4 while a `&` a
            // program writes is `NK1137` since
            // [ADR-094](../../docs/specification/adr/adr-094.md) D1. What it was
            // reaching for is built now: **declaring the result a view is
            // enough**, and the `&` is the compiler's
            // ([`Checked::lent_returns`], D1's third position). So the sentence
            // is back, with the half the author writes and without the half the
            // author cannot.
            //
            // **Only where the value is handed back.** A field *bound* to a name
            // or *passed* to a call is not a result, so a view of it has nowhere
            // declared to point — the two ways out are the whole answer there.
            help: Some(match what {
                "handed back" => format!(
                    "declare the result `ref {}` and the view is what this line means - the \
                     `&` is the compiler's to write (ADR-094 D1). Or `self.{field}.clone()` \
                     for a copy, where it happens, or `fn …(self)` where the method is meant \
                     to consume its subject",
                    ty.text()
                ),
                _ => format!(
                    "write `self.{field}.clone()` for a copy, where it happens, or \
                     `fn …(self)` where the method is meant to consume its subject"
                ),
            }),
        });
    }

    /// **What a method call is**, asked of a receiver whose type is already in
    /// hand.
    ///
    /// One function and not two, because there are two ways to write the call
    /// and only one thing a call *is*: `x.m()` and `x?.m()` differ in whether
    /// the call happens, never in what it throws, whether it pauses, what its
    /// arguments have to be, or what the receiver's type binds in its signature
    /// ([ADR-066](../../../docs/specification/adr/adr-066.md)). Two copies of
    /// these rules would be that many chances for the two spellings to drift.
    ///
    /// `on` is the receiver's type - for a `?.` the type **inside** the `T?`,
    /// which is the whole of what that operator changes here.
    ///
    /// `entry` is the name the **ledger** knows this call by, which is the
    /// written one everywhere but at
    /// [ADR-111](../../docs/specification/adr/adr-111.md) D5's witness door:
    /// `kasse.set(neu; after: stand)` is `set(after)`, a different operation
    /// from `set` and a different entry. `method` stays the name the *program*
    /// wrote, and everything the **emitter** is handed stays keyed by that —
    /// an argument it has to put a `&` in front of is found under `set`,
    /// because `set` is what is written on the line.
    fn call_on(&mut self, on: Ty, method: Ident, args: &[Expr], entry: &str, span: &Span) -> Ty {
        // **A produced sequence answers under its own word**
        // ([ADR-105](../../docs/specification/adr/adr-105.md) D1): `Seq` and
        // `Par` are what the ledger calls the receiver, so `Seq::collect` is
        // found exactly as `Vec::push` is. A `Par[T]` falls back to `Seq`'s
        // entries where it has none of its own, which is D3's *otherwise
        // `Par[T]` has `Seq[T]`'s surface* - one sentence rather than a second
        // copy of twelve entries.
        let named;
        let name = match &on {
            Ty::Seq { parallel, .. } => {
                named = match parallel {
                    true => ty::PAR,
                    false => ty::SEQ,
                };
                Some(named)
            }
            Ty::Named { name, .. } => Some(name.as_str()),
            _ => None,
        };
        let Some(name) = name else {
            // The receiver's type is not known, so neither is what this
            // calls. Recorded, because "I could not find out" is an
            // answer somebody downstream has to act on.
            args.iter().for_each(|a| {
                self.expr(a, span);
            });
            self.reached_method(None);
            self.method_propagates(method, false, span);
            self.method_pauses(method, false, span);
            return Ty::Unknown;
        };
        let key = format!("{name}::{entry}");
        let found = self.method(&key).or_else(|| match &on {
            // D3's *otherwise `Par[T]` has `Seq[T]`'s surface*.
            Ty::Seq { parallel: true, .. } => self.method(&format!("{}::{entry}", ty::SEQ)),
            // **Text of its own lends itself as a view**
            // ([ADR-225](../../docs/specification/adr/adr-225.md) D1), so a
            // method of a view is a method of a `String` too - which is how
            // the language below has it.
            Ty::Named { name, args, .. } if name == "String" && args.is_empty() => {
                self.method(&format!("str::{entry}"))
            }
            // **A list's own `map` is `std`'s `ListExt`** (ADR-233 D2): the
            // trait is what the language below calls on a `Vec`, and the entry
            // is written under its name.
            Ty::Named { name, .. } if ty::base(name) == "Vec" => {
                self.method(&format!("list::ListExt::{entry}"))
            }
            // **A value with a cleanup closes under std's one `close`**
            // ([ADR-239](../../docs/specification/adr/adr-239.md) D3): it
            // runs the cleanup now and hands its failure back at the call.
            Ty::Named { name, .. }
                if entry == "close"
                    && self.cleanup_of(&on).is_some_and(|(owner, _)| {
                        crate::contracts::ty::base(&owner) == crate::contracts::ty::base(name)
                    }) =>
            {
                self.method("cleanup::close")
            }
            _ => None,
        });
        let Some((key, contract)) = found else {
            // The type is known and no ledger describes this method of
            // it - `HashMap::entry` until something writes it down.
            // **A bound is looked up before anything is refused**
            // ([ADR-078](../../docs/specification/adr/adr-078.md) D3): where
            // `T: Summarize` and the trait declares this method, the call is
            // asked again on the trait and everything about it - arity,
            // argument types, `sync`, `throws` - is answered from the
            // declaration, exactly as it would be from a written-down receiver.
            if let Some(bound) = self.bound_that_answers(name, entry) {
                return self.call_on(bound, method, args, entry, span);
            }
            // Unless the type is a **parameter**, where nothing will ever
            // describe it and saying so now is the whole of `NK1126`.
            self.nothing_says_what_a_parameter_can_do(name, Reached::Method(entry), span);
            args.iter().for_each(|a| {
                self.expr(a, span);
            });
            self.reached_method(None);
            self.method_pauses(method, false, span);
            self.method_propagates(method, false, span);
            return Ty::Unknown;
        };
        self.reached_method(Some(&key));
        self.last_resolved = Some(key.clone());
        // **A receiver the method changes or takes is not only read**: a
        // `match` arm that does this to a part it bound needs the part itself.
        if let Some(name) = self.receiver_name.clone() {
            let takes = contract
                .signature
                .as_ref()
                .and_then(|s| s.params.first())
                .is_some_and(|(_, ty)| matches!(ty, Ty::Seq { .. }) || moves_away(ty));
            if contract.mutates || takes {
                self.changed.push(name);
            }
        }
        // **A method that walks a produced sequence consumes it**
        // ([ADR-105](../../docs/specification/adr/adr-105.md) D2): every `Seq`
        // entry writes its receiver `(Seq[$T], …)` and not `&Seq[$T]`, so the
        // signature is what says so rather than a list of method names. A
        // container's methods take a view and are untouched.
        if matches!(&on, Ty::Seq { shape, .. } if !shape.replays)
            && walks_by_value(contract)
            && let Some(name) = self.receiver_name.clone()
        {
            let taken = self.taken(name, on.clone(), self.read_seq, "", span);
            self.walked.push(taken);
        }
        // **And a list's own `map` takes the list** (ADR-233 D2): the mapped
        // list replaces it, so a read of it afterwards is one of a list that
        // is gone - `NK2105`, where it was `rustc`'s *borrow of moved value*.
        if key == "list::ListExt::map"
            && let Some(name) = self.receiver_name.clone()
        {
            self.hands_over_name(&name, "taken by `map`", span);
        }
        // **And `close` takes the value**: nothing of it is left to clean up
        // afterwards, and nothing of it to read.
        if key == "cleanup::close"
            && let Some(name) = self.receiver_name.clone()
        {
            self.hands_over_name(&name, "closed by `close`", span);
        }
        // ADR-023 D8: the failure leaves at the call, and the emitter
        // is what writes that. Recorded whether or not the function
        // around it declares `throws` - where it does not, `NK2605`
        // below refuses the program and nothing is emitted at all.
        self.method_options(method, contract, span);
        self.method_propagates(
            method,
            !contract.throws.is_empty() || self.walks_a_failing_sequence(&on, contract),
            span,
        );
        // ADR-055 D2, the method half. Either ledger since §6 step 3
        // made `std`'s own pausing entries `async fn`: before it, a
        // `std` entry blocked its thread and awaiting one would have
        // been awaiting a value rather than a future.
        // **A walk of a sequence whose step pauses, pauses**
        // ([ADR-172](../../docs/specification/adr/adr-172.md) D5). The entry's
        // own `sync` is about the walk — `count` adds one per element and does
        // nothing else — and what suspends is the **step** it asks for. So the
        // receiver's word decides it at the site, which is the same word the
        // `for` one construct over reads.
        //
        // **Except a `map` or a `filter` of a sequence this compiler made
        // pause** ([ADR-233](../../docs/specification/adr/adr-233.md) D1): that
        // one has both, as adapters of its own, and hands back another like it.
        let chained = a_paused_chain(&key, &on);
        let walks_a_pausing_step =
            matches!(&on, Ty::Seq { pauses: true, .. }) && walks_by_value(contract) && !chained;
        self.method_pauses(
            method,
            !contract.sync.is_sync() || walks_a_pausing_step,
            span,
        );
        // …and a **lazy** walk of one is another such sequence, as an adapter
        // of its own ([ADR-233](../../docs/specification/adr/adr-233.md) D1,
        // [ADR-234](../../docs/specification/adr/adr-234.md) D1) - every one
        // `Seq` has, which is what `chained` says. What is left here is an entry
        // that hands back a sequence and has no pausing form: none today, since
        // `rev` is `NK2703` first. The net stays for the next one.
        //
        // Recorded rather than refused here, because *this compiler cannot
        // build that yet* is a refusal from the lowering and not a rule of the
        // language ([ADR-171](../../docs/specification/adr/adr-171.md) §4).
        let hands_back_a_sequence = matches!(
            contract.signature.as_ref().and_then(|s| s.result.as_ref()),
            Some(Ty::Seq { .. })
        );
        if walks_a_pausing_step && hands_back_a_sequence {
            self.checked
                .pausing_walks
                .insert((span.start, self.parsed.text(method).to_string()));
        }
        // **A walk of a sequence whose step can fail, can fail**
        // ([ADR-025](../../docs/specification/adr/adr-025.md) D1, one construct
        // over from the `for` it was written about). The entry's own `throws`
        // is about the walk; what fails is the **step** it asks for, so the
        // receiver's word decides it at the site.
        //
        // **The eager walks and not the lazy ones**, for the reason above read
        // the other way: a `map` has produced nothing, so nothing of it has
        // failed yet, and the failure belongs to whatever walks the result.
        let walks_a_failing_step = matches!(&on, Ty::Seq { throws: true, .. })
            && walks_by_value(contract)
            && !hands_back_a_sequence;
        if walks_a_failing_step {
            self.a_walk_of_a_failing_sequence(method, span);
        }
        self.a_pausing_method_in_a_sync_body(&key, contract, span);
        self.a_call_that_may_pause(
            self.parsed.text(method),
            !contract.sync.is_sync() || walks_a_pausing_step,
            span,
        );
        self.a_pausing_call_in_an_action(self.parsed.text(method), contract, span);
        // A method call is a written call, so the rule reaches it too
        // (`NK2605`) - and here the receiver's type was known and a
        // ledger described the method, which is the only case this
        // compiler can answer at all.
        self.may_fail_here(&key, contract, span);

        // What the receiver's own type tells the signature (ADR-031).
        // `HashMap[&str, Stats]` against `&HashMap[$K, $V]` binds `$V`
        // to `Stats`, so `-> Entry[$V]` is an `Entry[Stats]` and the
        // next call in the chain has something to bind from in turn.
        let bound = bindings(contract, &on);

        // The arguments are walked **after** the contract is in hand,
        // which is what lets a lambda's parameters have types (ADR-029).
        // The old order walked them first and could not: `a` in
        // `.and_modify fn { a.add(t) }` is named nowhere and typed by
        // nothing but the callee's signature.
        let expected: Vec<Ty> = expected_arguments(contract)
            .iter()
            .map(|ty| ty::substitute(ty, &bound))
            .collect();
        let outer_paused = std::mem::take(&mut self.paused_args);
        // **A lambda handed to a walk of a `Par[T]` runs on several cores at
        // once** (Part II 12.6, ADR-235 D2): what it may do is asked while it
        // is walked, and a lambda inside it is inside it too.
        let outer_parallel = self.in_parallel;
        if matches!(&on, Ty::Seq { parallel: true, .. }) {
            self.in_parallel = outer_parallel.or(Some(self.scope.len()));
        }
        let found = self.arguments_given(
            args,
            &expected,
            self.own.functions.contains_key(&key),
            Some(contract),
            span,
        );
        self.in_parallel = outer_parallel;
        let paused_here = std::mem::replace(&mut self.paused_args, outer_paused);
        let pausing_lambda = args.iter().find(|arg| {
            matches!(arg, Expr::Closure { .. }) && paused_here.contains(&argument_shape(arg))
        });
        self.a_set_that_reads_what_it_writes(&on, entry, &found, span);
        // **A literal where the receiver says text is wanted** (ADR-215 D1):
        // `m.insert("a", 1)` on a map of `String` keys wants `$K`, which the
        // receiver binds and the per-argument check below does not see. A
        // method's argument is handed over, so the literal is built there.
        for (at, given) in args.iter().enumerate() {
            if let Some(want) = expected.get(at) {
                self.text_literal(want, given, true);
            }
        }
        self.a_sequence_that_cannot_do_this(&on, method, contract, &found, span);
        self.counts_in_usize(&key, method, span);
        // **The source's own name and not the ledger's**, because what
        // `arguments` records is read back by the emitter off the line as it is
        // written: a `&` the compiler owes the witness is looked up under
        // `set`, which is the word on the page.
        let written = self.parsed.text(method).to_string();
        let result = self.arguments(&key, &written, contract, args, &found, &[], span);
        self.a_lookup_key_lent(&key, &written, args, &found, span);
        self.a_view_kept_where_the_receiver_says_text(
            &key, contract, args, &found, &expected, span,
        );
        // The receiver first (ADR-031), then whatever the arguments can still
        // say (ADR-074 D2) - `or_insert` on a map that bound `$V` already has
        // its answer, and `bind` does not overwrite one.
        let mut bound = bound;
        for (name, ty) in from_arguments(contract, &found) {
            bound.entry(name).or_insert(ty);
        }
        // A method's own parameters carry bounds exactly as a free function's
        // do, and one written call is one rule (ADR-066).
        self.a_bound_the_argument_does_not_meet(&key, &bound, span);
        let result = ty::substitute(&result, &bound);
        let result = shape_through(contract, &on, &found, result);
        let result =
            self.a_lambda_that_pauses_at_an_entry(&key, method, &on, pausing_lambda, result, span);
        self.stamped_through(contract, &found, result)
    }

    /// **A lambda that pauses, handed to a `std` entry**
    /// ([ADR-233](../../docs/specification/adr/adr-233.md)).
    ///
    /// Every `std` entry that takes a lambda describes a **Rust** signature
    /// whose closure is synchronous, so the call is lowered to the entry's
    /// counterpart in `nikaia_std::seq`, which takes the language below's own
    /// `async` closure (D1, D2). Which counterpart is a question about the
    /// receiver's type, which the emitter has none of, so it is answered here.
    ///
    /// * `map` and `filter` of a sequence hand back **a sequence whose step
    ///   pauses** (D1): nothing is walked here, and whatever walks the result
    ///   pauses - a `for` gives its thread up at each step, and `collect`,
    ///   `count`, `nth` and `join` are awaited, exactly as for `io::lines()`.
    /// * A list's `map`, `sort_by_key`, `or_insert_with` and `and_modify` are
    ///   **eager** (D2): the call itself runs the lambda, so the call pauses.
    ///
    /// The four doors take their lambda **while a lock is held**, where a
    /// pause is `NK2202` by Part II 12.2's own rule - asked of every call
    /// inside a door, not here (D3).
    #[allow(clippy::too_many_arguments)]
    fn a_lambda_that_pauses_at_an_entry(
        &mut self,
        key: &str,
        method: Ident,
        on: &Ty,
        lambda: Option<&Expr>,
        result: Ty,
        span: &Span,
    ) -> Ty {
        // A `Par[T]`'s lambda may not pause at all: `NK2209` has said so.
        if matches!(on, Ty::Seq { parallel: true, .. }) {
            return result;
        }
        let chained = a_paused_chain(key, on);
        let Some(lambda) = lambda else {
            // A plain lambda over a sequence that already pauses: the step
            // still does, so the result is one too.
            return match chained {
                true => a_pausing_sequence(result, on),
                false => result,
            };
        };
        let entry = match (key, chained) {
            ("Seq::map", true) => PausingEntry::Method("then"),
            ("Seq::filter", true) => PausingEntry::Method("then_filter"),
            ("Seq::map", false) => PausingEntry::Function("then"),
            ("Seq::filter", false) => PausingEntry::Function("then_filter"),
            ("list::ListExt::map", _) => PausingEntry::Function("map_list"),
            // A `mut` parameter is a `&mut Vec` already, and handed on as it
            // is: a second `&mut` may not be taken of it (ADR-094 D3).
            ("Vec::sort_by_key", _) => match self
                .receiver_name
                .as_deref()
                .and_then(|name| self.binding(name))
                .is_some_and(|local| local.changing)
            {
                true => PausingEntry::Function("sort_by_key"),
                false => PausingEntry::Lent("sort_by_key"),
            },
            ("Entry::or_insert_with", _) => PausingEntry::Function("or_insert_with"),
            ("Entry::and_modify", _) => PausingEntry::Function("and_modify"),
            // An entry with no counterpart keeps the lowering's refusal, which
            // names the call and the way out.
            _ => return result,
        };
        self.checked
            .pausing_lambdas
            .insert((span.start, argument_shape(lambda)), entry);
        if matches!(key, "Seq::map" | "Seq::filter") {
            return a_pausing_sequence(result, on);
        }
        // The eager ones pause where they are called, and the call is one that
        // may pause for every rule that asks: the function around it, the
        // lambda around it, the door around it.
        let written = (span.start, self.parsed.text(method).to_string());
        self.settled_methods.remove(&written);
        self.pausing_methods.insert(written);
        let callee = key.to_string();
        self.a_call_that_may_pause(&callee, true, span);
        result
    }

    /// **`NK2703`: a sequence asked for something it is not**
    /// ([ADR-212](../../docs/specification/adr/adr-212.md) D2).
    ///
    /// `rev` writes its receiver `Seq[$T] ends`, and in a receiver's position
    /// the word is a demand: `io::lines().rev()` is refused here, in the
    /// program's words, where it used to be refused by `rustc` in the language
    /// below's (`the trait bound `Lines: DoubleEndedIterator` is not
    /// satisfied`, about a file nobody wrote).
    fn a_sequence_that_cannot_do_this(
        &mut self,
        on: &Ty,
        method: Ident,
        contract: &FnContract,
        found: &[Ty],
        span: &Span,
    ) {
        let Some(signature) = &contract.signature else {
            return;
        };
        let receiver = signature
            .params
            .first()
            .filter(|(name, _)| name == "self")
            .map(|(_, pattern)| (pattern, on));
        let arguments = signature
            .arguments()
            .iter()
            .zip(found)
            .map(|((_, pattern), actual)| (pattern, actual));
        let written = self.parsed.text(method).to_string();
        for (pattern, actual) in receiver.into_iter().chain(arguments) {
            let (
                Ty::Seq { shape: wanted, .. },
                Ty::Seq {
                    shape: has, item, ..
                },
            ) = (pattern, actual)
            else {
                continue;
            };
            let missing = has.missing(wanted);
            if missing.is_empty() {
                continue;
            }
            // In the program's words and not the ledger's: `ends` and `sized`
            // are not words a program can write (ADR-105 D4), so a message that
            // named them would name something its reader cannot type
            // (Part III C.1).
            let (needs, lacks, way) = match missing[0] {
                ty::ENDS => (
                    "walks a sequence from the back",
                    "this one can only be walked from the front",
                    "collect it into a list first, and walk the list from the back: \
                     `….collect()` and then `list.iter().rev()`",
                ),
                ty::SIZED => (
                    "needs to know how long the sequence is",
                    "this one does not know until it has been walked",
                    "collect it into a list first: `….collect()`, whose length is known",
                ),
                _ => (
                    "walks the sequence more than once",
                    "this one is used up by the first walk",
                    "collect it into a list first",
                ),
            };
            let item = match &**item {
                Ty::Unknown => "a sequence".to_string(),
                item => format!("a sequence of `{item}`"),
            };
            self.checked.findings.push(Finding {
                code: "NK2703",
                severity: Severity::Error,
                span: span.clone(),
                message: format!("`{written}` {needs}, and {lacks}"),
                notes: vec![format!(
                    "this is {item}, and what produces it says what it can do: a \
                         range, a list's `iter()`, `chars()` and `drain()` can be walked \
                         from either end; a map's `keys()` and `io::lines()` only from \
                         the front, and a `filter` does not know its length (ADR-212 D1)"
                )],
                help: Some(way.to_string()),
            });
            return;
        }
    }

    /// **A count the language below takes in `usize`**
    /// ([ADR-212](../../docs/specification/adr/adr-212.md) D5), recorded for
    /// the emitter by the **entry** the call resolved to - so a method of the
    /// program's own that happens to be called `take` is not converted.
    fn counts_in_usize(&mut self, key: &str, method: Ident, span: &Span) {
        for (entry, at) in COUNTS {
            if *entry == key {
                self.checked.count_args.insert((
                    span.start,
                    self.parsed.text(method).to_string(),
                    *at,
                ));
            }
        }
    }

    /// **`NK1164`: the type a call picked does not answer for the bound**
    /// ([ADR-174](../../docs/specification/adr/adr-174.md) D2).
    ///
    /// ```nika
    /// fn tell[T: Speaks](x: T) -> String {
    ///     return x.say()
    /// }
    ///
    /// tell(rock)     // error[NK1164]
    /// ```
    ///
    /// A bound was **declared and not enforced**: `[T: Speaks]` put `say` in
    /// reach of the body — which is `NK1126`, and is built — while nothing
    /// asked the other question a bound exists to ask, *may this type stand
    /// here*. So the call was accepted and the language below answered *the
    /// trait bound `Rock: Speaks` is not satisfied* about a file nobody wrote,
    /// which is [Part III C.1](../../docs/specification/30-nikaia-tooling.md)'s
    /// class exactly.
    ///
    /// **Fail-open in three places**, because C.4 is the other half of the same
    /// page and a correct program refused is worse than a wrong one passed on:
    /// a trait this unit does not declare is one nothing here can answer about
    /// (`traits::check` draws the same line); an argument this compiler could
    /// not type binds nothing; and a **parameter** in the caller's own scope
    /// answers for whatever its own bounds say, because there the caller is the
    /// one who did not pick the type either.
    fn a_bound_the_argument_does_not_meet(
        &mut self,
        key: &str,
        bound: &BTreeMap<String, Ty>,
        span: &Span,
    ) {
        let wanted = match self.declared_bounds.get(key) {
            Some(wanted) => wanted.clone(),
            // **A package's function answers from its signature**
            // ([ADR-205](../../docs/specification/adr/adr-205.md) D1). The map
            // above is built from the AST of the unit being checked, under the
            // key a call in *that* unit resolves to; a call written
            // `handler::dispatch` resolves to a key no unit's AST produced, and
            // what the bound was came back as `rustc`'s words about the type
            // this program picked ([`open-work.md`](../../docs/open-work.md)
            // §1.10).
            None => match self.bounds_in_a_signature(key) {
                Some(wanted) => wanted,
                None => return,
            },
        };
        for (parameter, traits) in wanted {
            let Some(Ty::Named { name: actual, .. }) = bound.get(&parameter) else {
                continue;
            };
            let actual = actual.clone();
            // **A parameter of the caller's own is a different sentence.**
            // `impl Speaks for V` is not something anybody can write - `V` is
            // a name a *further* caller fills in - so the way out is the
            // caller's own bound list and nowhere else (Part III, C.2).
            let of_the_caller = self.type_parameters.contains_key(&actual);
            for trait_name in traits {
                if self.answers_for(&actual, &trait_name) {
                    continue;
                }
                // **A shape bound is a third sentence**, because the other
                // two both end in an `impl` and no `impl` answers `Struct`:
                // what answers it is the declaration, so *write `impl Struct
                // for i64`* would be a way out that cannot be taken, which
                // [Part III C.2](../../docs/specification/30-nikaia-tooling.md)
                // says is not one.
                let shape = SHAPE_BOUNDS.contains(&trait_name.as_str())
                    && !self.own.traits.contains_key(&trait_name);
                let (message, note, way_out) = match (shape, of_the_caller) {
                    (true, of_the_caller) => (
                        format!(
                            "`{key}` asks for {} `{trait_name}` here, and `{actual}` is not one",
                            match trait_name.as_str() {
                                "Enum" => "an",
                                _ => "a",
                            }
                        ),
                        format!(
                            "`[{parameter}: {trait_name}]` asks what a type **is** rather than \
                             what it does, so what answers it is a declaration and not an `impl` \
                             (Part II, 10.3) - and `{actual}` is {}",
                            self.what_shape_it_is(&actual)
                        ),
                        match of_the_caller {
                            true => format!("add it to the bound: `[{actual}: … + {trait_name}]`"),
                            false => format!(
                                "pass a value of a type this program declares with `{}`, or leave \
                                 the bound off",
                                trait_name.to_lowercase()
                            ),
                        },
                    ),
                    (false, true) => (
                        format!(
                            "`{key}` asks for a `{trait_name}` here, and `{actual}` is not \
                             declared to be one"
                        ),
                        format!(
                            "`{actual}` stands for a type *this* function's caller picks, so \
                             what it can be asked to do is what its own bounds say - and \
                             `{trait_name}` is not among them (Part I, 4.7)"
                        ),
                        format!("add it to the bound: `[{actual}: … + {trait_name}]`"),
                    ),
                    (false, false) => (
                        format!(
                            "`{key}` asks for a `{trait_name}` here, and `{actual}` is not one"
                        ),
                        format!(
                            "`{key}`'s `{parameter}` is declared `[{parameter}: \
                             {trait_name}]`, so the type a caller picks has to answer for \
                             `{trait_name}`'s methods - and nothing in this program says \
                             `{actual}` does (Part I, 4.7)"
                        ),
                        format!(
                            "write `impl {trait_name} for {actual} {{ … }}`, or pass a type \
                             that already has one"
                        ),
                    ),
                };
                self.checked.findings.push(Finding {
                    severity: Severity::Error,
                    span: span.clone(),
                    code: "NK1164",
                    message,
                    notes: vec![note],
                    help: Some(way_out),
                });
            }
        }
    }

    /// What this compiler can say `{ty}` is, for a shape bound's note.
    fn what_shape_it_is(&self, ty: &str) -> String {
        if self.structs.contains_key(ty) {
            return format!("a `struct` - `[{ty}: Struct]` is the bound it answers");
        }
        if self.enums.contains_key(ty) {
            return format!("an `enum` - `[{ty}: Enum]` is the bound it answers");
        }
        "one of the types Part I 2.2 offers, which no declaration makes either shape".to_string()
    }

    /// Whether `ty` may stand where `trait_name` is asked for.
    ///
    /// **`true` where nothing says otherwise** — see the three fail-open cases
    /// on `NK1164` above.
    fn answers_for(&self, ty: &str, trait_name: &str) -> bool {
        // **A shape bound is answered by the declaration**
        // ([ADR-088](../../docs/specification/adr/adr-088.md) D2), which is why
        // it is asked before the trait map: no `impl` says `Struct`, so the
        // fail-open line below would let every argument through.
        //
        // **And it fails open everywhere this compiler has not read a
        // declaration.** A `std` type, a foreign one, a name no ledger
        // classifies — this walk cannot tell a struct from anything else there,
        // and [Part III C.4](../../docs/specification/30-nikaia-tooling.md)
        // says a correct program refused is the worse mistake. What is left is
        // the case D3 is about: a type this file declares as the *other* shape,
        // and a primitive, both of which it has read.
        if SHAPE_BOUNDS.contains(&trait_name) && !self.own.traits.contains_key(trait_name) {
            if let Some(bounds) = self.type_parameters.get(ty) {
                return bounds.iter().any(|declared| declared == trait_name);
            }
            let is_struct = self.structs.contains_key(ty);
            let is_enum = self.enums.contains_key(ty);
            if !is_struct && !is_enum && !is_one_of_part_one_2_2(ty) {
                return true;
            }
            return match trait_name {
                "Struct" => is_struct,
                _ => is_enum,
            };
        }
        if !self.own.traits.contains_key(trait_name) {
            return true;
        }
        if let Some(bounds) = self.type_parameters.get(ty) {
            return bounds.iter().any(|declared| declared == trait_name);
        }
        // **The program's ledger and not this file's walk** (ADR-174 D1).
        // `impl Speaks for Dog` may stand in a different file from the
        // `fn tell[T: Speaks]` that asks, and one file's walk sees one file —
        // measured on a two-file project, where the refusal was a **correct**
        // program refused, which [Part III
        // C.4](../../docs/specification/30-nikaia-tooling.md) says may not
        // happen.
        self.own
            .implementations
            .get(trait_name)
            .is_some_and(|types| types.contains(ty))
    }

    /// Part I 2.2: **`as` names a type this language offers**
    /// ([ADR-054](../../../docs/specification/adr/adr-054.md) D1).
    ///
    /// Nothing ever decided that a program may write `as u128`, and it could:
    /// the target of an `as` went to the language below unread, so a cast named
    /// any Rust type at all and was emitted verbatim. Two things followed, and
    /// the second is the reason this is a refusal rather than a tidy-up. A
    /// program could hold a value of a type Part I 2.2 does not offer and the
    /// page had no word for what it was. And `-3 as usize` is
    /// 18,446,744,073,709,551,613 - a silent reinterpretation, in a language
    /// where a conversion that does not fit aborts
    /// ([ADR-043](../../../docs/specification/adr/adr-043.md) D4). The same
    /// conversion at an index has reported as an access out of bounds since
    /// [ADR-048](../../../docs/specification/adr/adr-048.md) D1; written by hand
    /// it reported nothing.
    ///
    /// **`NK1122`.** A refusal costs nothing today and would break programs
    /// later, which is why it is made now rather than when somebody depends on
    /// the hole.
    fn cast_names_a_foreign_type(&mut self, into: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1122",
            message: format!("`{into}` is not a type this language offers, and `as` names one"),
            notes: vec![
                "The types are `i32`, `i64`, `u8`, `f64`, `bool`, `char`, `String` and \
                 `&str` (Part I, 2.2). A conversion into anything else went to \
                 the language below unread, so a value could have a type this \
                 page has no word for"
                    .to_string(),
            ],
            help: Some(match into {
                "usize" | "isize" | "u16" | "u32" | "u64" | "u128" | "i8" | "i16" | "i128" => {
                    "a length and an index are `i64` and the compiler writes the \
                     machine's conversion itself (ADR-048 D1), so the cast is \
                     not needed - remove it. Where a narrower type is meant, \
                     `truncating_i32` says so by name"
                        .to_string()
                }
                _ => "write one of the types above, or none".to_string(),
            }),
        });
    }

    /// Part I 2.3: record where the emitter has to write the `Some(…)`.
    ///
    /// A type is non-nullable unless it says otherwise, so a plain `T` standing
    /// where a `T?` is wanted is the one widening this language has - `let mut
    /// m: &str? = null` and then `m = "World"`. `Ty::fits` allows it; the
    /// language below needs the constructor written, and this is where the
    /// emitter is told.
    ///
    /// **Which of the two**, and the answer used to be *"one, or nothing"*
    /// ([ADR-068](../../../docs/specification/adr/adr-068.md)).
    ///
    /// The constructor may only be written where the value is known **not** to
    /// be a `T?` already, because wrapping one that is would make an
    /// `Option<Option<T>>`. Two ways it can be known, and a type is only the
    /// first:
    ///
    /// * its type says so — anything this checker worked out that is not a
    ///   `T?`; or
    /// * **it is a literal**, which no literal ever is. That second one is not
    ///   a convenience: a number has no type of its own on purpose (Part I 2.4,
    ///   so that `add(3)` is right wherever the parameter is numeric), so
    ///   `return 42` against a declared `i64?` answers `Unknown` and the type
    ///   alone would leave the commonest case in the section unwrapped.
    ///
    /// **And everything else takes the conversion**, which is what changed:
    /// `value.into()` is correct whether the value is a `T` or already a `T?`,
    /// so a type this checker could not work out stops being a position it has
    /// to stay silent about. It used to be left alone — right about the risk,
    /// and the program then failed in the language below with `rustc`'s *"try
    /// wrapping the expression in `Some`"* about a form Nikaia does not have
    /// (Part III C.1).
    ///
    /// `null` is in neither, being a `T?` itself.
    fn wraps_into_nullable(&mut self, found: &Ty, want: &Ty, value: &Expr, span: &Span) {
        let Some(how) = wrap_for(found, want, is_literal(value)) else {
            return;
        };
        self.checked.nullable_sites.insert(span.start, how);
    }

    /// Part I 2.2: a constant that does not fit the type it is given is a
    /// compile error rather than something the program finds out about at run
    /// time.
    ///
    /// **`NK1116`, and it exists to take a message back rather than to prevent an
    /// abort.** This was already refused where it was written - but by `rustc`, in
    /// Rust's words, down to the lint name `overflowing_literals` and the advice
    /// to use a `u32`, about a file nobody wrote (Part III, C.1). An out-of-range
    /// constant never reached run time and never will; what changes is who says so.
    ///
    /// **A literal alone is asked only where a type stands beside it**, because a
    /// literal has no type of its own on purpose: `Expr::LitInt` answers
    /// `Unknown`, so `add(3)` is right wherever the parameter is numeric, and
    /// `let m = 3000000000` is a correct program where the next line passes `m`
    /// to an `i64` (Part I 2.4, and `docs/open-work.md`'s out-of-range literal).
    /// So the places are an
    /// annotated `let`, a `return` against a declared result, and an argument
    /// whose parameter says what it takes.
    ///
    /// **A sum reaches further, and that is ADR-043 §3's gap closing.** Where an
    /// operand's *declaration* pins the type - `let a: i32 = …`, then `a + 1` -
    /// the arithmetic has a type whatever stands beside it, so the bare `let` is
    /// asked too. That is the one case `rustc` refused about the generated file
    /// with *"attempt to compute `i32::MAX + 1_i32`"*.
    ///
    /// **Only the integer types Part I 2.2 offers** — `i64`, `i32` and `u8`.
    /// `u32` and the rest are accepted by the compiler below and not offered
    /// here, so a range for them would be a claim about a surface that is not
    /// promised. The byte joined the list the day it gained a `const` form: a
    /// range that is not checked is `rustc` about the generated file the first
    /// time somebody writes one.
    fn constant_fits(&mut self, value: &Expr, want: Option<&Ty>, span: &Span) {
        let Some(folded) = self.constant_of(value) else {
            return;
        };
        // The type it answers to: what stands beside it if that is an integer
        // this language offers, and otherwise what an operand's declaration
        // pinned. Neither, and there is nothing to measure against - which is
        // the literal-alone case that must stay accepted (C.4).
        let named = want.and_then(integer_named);
        let ty = match named.or(folded.pinned) {
            Some(ty) => ty,
            // **Nothing beside it and nothing pinning it**, which is the
            // constant that decides its own type
            // ([ADR-063](../../docs/specification/adr/adr-063.md) D1): it takes
            // the first that holds it, so the only thing left to report here is
            // a value no type holds at all. Reported against the wider one,
            // because that is the one it fell out of.
            None if i64::try_from(folded.value).is_err() => "i64".to_string(),
            None => return,
        };
        let fits = match ty.as_str() {
            "i32" => i32::try_from(folded.value).is_ok(),
            "i64" => i64::try_from(folded.value).is_ok(),
            // **And the byte**, which needed no range here while it had no
            // crossed form: once `const B: u8 = …` is something this compiler
            // writes, `let x: u8 = 300` writing it is `rustc` about the
            // generated file ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
            "u8" => u8::try_from(folded.value).is_ok(),
            _ => return,
        };
        if fits {
            return;
        }
        let (low, high) = match ty.as_str() {
            "i32" => (i32::MIN as i128, i32::MAX as i128),
            "u8" => (u8::MIN as i128, u8::MAX as i128),
            _ => (i64::MIN as i128, i64::MAX as i128),
        };
        // A bare literal says its own digits; anything folded says what it came
        // to, because the expression is on the line the caret is under and the
        // number is the part the reader cannot see.
        let message = match value {
            Expr::LitInt(text) => format!("`{text}` does not fit in an `{ty}`"),
            _ => format!(
                "this comes to {}, which does not fit in an `{ty}`",
                folded.value
            ),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1116",
            message,
            // **`a u8` and `an i32`**, because the article is read off how the
            // name is *said*: a reader says "you-eight" and "eye-thirty-two",
            // and `an u8` is the kind of sentence that makes a message look
            // generated.
            notes: vec![format!(
                "{} `{ty}` holds {low} to {high} (Part I, 2.2)",
                match ty.starts_with('i') {
                    true => "an",
                    false => "a",
                }
            )],
            help: Some(match ty.as_str() {
                "i32" => "write `i64` where the number needs it".to_string(),
                "u8" => "a `u8` is one byte, so write `i32` or `i64` where the number \
                         is a count rather than a byte"
                    .to_string(),
                _ => "an `i64` is the widest number this language has, so this \
                      computation has to be arranged to stay inside it"
                    .to_string(),
            }),
        });
    }

    /// What a constant integer expression comes to, and the type an operand's
    /// declaration pinned.
    ///
    /// **The arithmetic is [`crate::fold`]'s**, and what this adds is the half
    /// the emitter cannot have: a name resolved to what it is worth
    /// ([ADR-063](../../docs/specification/adr/adr-063.md) D2). Only an
    /// **immutable** `let` carries a value forward to be found here, which
    /// `bind_with` decides; a declaration on the way pins the type, and that is
    /// what makes `let a: i32 = 2` then `a + a` arithmetic in an `i32`.
    fn constant_of(&self, expr: &Expr) -> Option<Constant> {
        crate::fold::constant_of(expr, &|name| {
            let (ty, constant) = self.local(self.parsed.text(name))?;
            let value = constant?;
            Some(Constant {
                // **A name pins, and a literal does not**
                // ([ADR-063](../../docs/specification/adr/adr-063.md) D2). A
                // declaration pins what it says; a bare `let` pins the type its
                // own value took, which is the first that holds it - so
                // `let a = 2000000000` is an `i32` and `a + a` is arithmetic in
                // one, the same as in every language that has both widths. The
                // way out is one word: `let a: i64 = …`.
                pinned: integer_named(&ty).or_else(|| {
                    Some(match i32::try_from(value) {
                        Ok(_) => "i32".to_string(),
                        Err(_) => "i64".to_string(),
                    })
                }),
                value,
            })
        })
    }

    /// `self` is a reserved word, and this is the one position the grammar
    /// cannot refuse it in ([ADR-051](../../docs/specification/adr/adr-051.md)).
    ///
    /// Every other reserved word is excluded from `NAME` itself, so `let fn = 3`
    /// does not parse. **`self` cannot be**, because it is the one keyword that
    /// *is* a name: `self.min` refers to it, and `NAME` is the rule both for
    /// declaring a name and for referring to one. So the refusal is here, where
    /// the declaration is - and it can say more than a parse error would.
    ///
    /// **Every position that declares a name**: a `let`, a `for` binding, a
    /// lambda's argument, a parameter and a struct field.
    ///
    /// The last two took a span of their own on `FnArg` and `FieldDef` to
    /// reach. Without one the nearest span each walk had was the body's first
    /// statement - a different line - and a caret on the wrong line is worse
    /// than no message, which is why they waited rather than being
    /// approximated.
    ///
    /// **`NK1119`**, and it is the same C.1 case as the rest of the list: `let
    /// self = 3` lowered to `let self = 3;` and `rustc` refused the generated
    /// file with *"expected identifier, found keyword `self`"*.
    /// The four words the **language below** reserves and cannot escape.
    ///
    /// [ADR-076](../../docs/specification/adr/adr-076.md) D1 escapes every other
    /// one — `type` becomes `r#type` and the Nikaia name stays legal. These four
    /// have no escape at all: *"`crate` cannot be a raw identifier"* is Rust's
    /// own answer to `r#crate`, and the same for `super`, `self` and `Self`. So
    /// the one rule has one forced exception, and it is the target's rather than
    /// this language's.
    ///
    /// `self` is not here because it is already refused by `NK1119`, which says
    /// more: it is a reserved word of *this* language, with a reason of its own.
    const UNESCAPABLE_BELOW: &'static [&'static str] = &["crate", "super", "Self"];

    /// Every question about whether a name may be one, in one place.
    ///
    /// Two rules with the same six call sites, kept together so that adding a
    /// position adds it to both: `self` is this language's own reserved word
    /// (`NK1119`) and `crate`, `super` and `Self` are the language below's
    /// (`NK1128`).
    fn nameable(&mut self, name: &str, span: &Span, what: &str) {
        self.not_self(name, span, what);
        self.not_unescapable(name, span, what);
    }

    /// **`NK1128`: a name the language below reserves and cannot escape.**
    ///
    /// `let crate = 3` used to lower to `let crate = 3;` and `rustc` answered
    /// `E0532` about a module — about a file nobody wrote, which is
    /// [Part III C.1](../../docs/specification/30-nikaia-tooling.md)'s class.
    /// Every other such word is escaped where it is written
    /// ([ADR-076](../../docs/specification/adr/adr-076.md) D1); these three have
    /// no escape, so a refusal here is the only thing left that is not a message
    /// in the backend's words.
    ///
    /// Three words rather than twenty-seven, and that is the whole point of D1:
    /// `type` is the field name of every tagged record anybody has written and
    /// it stays available. `crate` and `super` name a module tree this language
    /// does not have, and `Self` is already how it writes the type an `impl` is
    /// on — so the vocabulary this costs is one nobody reaches for.
    fn not_unescapable(&mut self, name: &str, span: &Span, what: &str) {
        if !Self::UNESCAPABLE_BELOW.contains(&name) {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1128",
            message: format!(
                "`{name}` is a name the language below reserves and cannot escape, \
                 so {what} may not be called that"
            ),
            notes: vec![
                "every other such name is written escaped and stays a name here - \
                 a field called `type` is fine. `crate`, `super` and `Self` are the \
                 three the language below refuses even escaped, which is why they are \
                 the three refused here (ADR-076 D3)"
                    .to_string(),
            ],
            help: Some(format!(
                "pick another name - `{}` is free, and so is anything else that is not \
                 one of those three",
                match name {
                    "crate" => "package",
                    "super" => "parent",
                    _ => "own",
                }
            )),
        });
    }

    fn not_self(&mut self, name: &str, span: &Span, what: &str) {
        if name != "self" {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1119",
            message: format!("`self` is a reserved word, so {what} may not be called that"),
            notes: vec![
                "`self` already names one thing - the value a method was called on - and it \
                 is the only reserved word that is a name at all, which is why the rest of \
                 the list is refused by the grammar and this one is refused here \
                 (Part I, 2.1)"
                    .to_string(),
            ],
            help: Some("pick another name; `it`, `this` and `me` are all free".to_string()),
        });
    }

    /// ADR-043 D5.5: a division whose divisor is a constant zero is refused
    /// here, in this language's words.
    ///
    /// It was already refused where it was written, and by the same mechanism as
    /// the sum `constant_fits` takes back: `rustc`'s `unconditional_panic`, with
    /// *"attempt to divide `1_i32` by zero"* about a file nobody wrote
    /// (Part III, C.1).
    ///
    /// **`NK1118`, and the polarity is the fold's.** A divisor that does not
    /// fold to a constant says nothing: a division by something this checker
    /// cannot evaluate is the ordinary case, and it aborts at run time with the
    /// Nikaia line the table names (ADR-044). Only a zero it can *prove* is
    /// refused.
    fn divisor_is_not_zero(&mut self, op: BinaryOp, rhs: &Expr, span: &Span) {
        if !matches!(op, BinaryOp::Div | BinaryOp::Rem) {
            return;
        }
        let Some(divisor) = self.constant_of(rhs) else {
            return;
        };
        if divisor.value != 0 {
            return;
        }
        let what = match op {
            BinaryOp::Div => "divides",
            _ => "takes the remainder",
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1118",
            message: format!("this {what} by zero"),
            notes: vec![
                "a division by zero is unrecoverable (Part III, A.2) - and this one is \
                 decidable where it is written, so it is said here rather than left to \
                 abort"
                    .to_string(),
            ],
            help: Some(
                "if the divisor is meant to be able to be zero, it cannot be a constant: \
                 test it before dividing"
                    .to_string(),
            ),
        });
    }

    // --- statements ---------------------------------------------------------

    /// Walk a block and hand back the type of its tail.
    fn block(&mut self, block: &Block) -> Ty {
        self.scope.push(Vec::new());
        let mut tail = Ty::Tuple(Vec::new());
        let last = block.stmts.len().saturating_sub(1);
        for (at, stmt) in block.stmts.iter().enumerate() {
            let ty = self.stmt(&stmt.node, &stmt.span);
            // **`break i` is two statements, and that is the point** - a jump
            // takes no value, so the value becomes a statement of its own and
            // the program written is not the program compiled. `while_stmt`
            // has the identical note about the identical failure: three
            // statements, no error, and `rustc` complaining about a file
            // nobody wrote. Asked here rather than in the grammar because the
            // grammar cannot see what follows without a lookahead over every
            // expression there is - and asked of the **next** statement's span,
            // because that is what the author would have to delete.
            if at < last && matches!(stmt.node, Stmt::Break | Stmt::Continue) {
                self.nothing_follows_a_jump(&stmt.node, &block.stmts[at + 1].span);
            }
            if at == last {
                tail = ty;
            }
        }
        self.scope.pop();
        tail
    }

    fn stmt(&mut self, stmt: &Stmt, span: &Span) -> Ty {
        match stmt {
            Stmt::Let {
                names,
                mutable,
                ty,
                value,
            } => {
                self.a_field_of_a_borrowed_subject(value, span, "bound");
                let found = self.expr(value, span);
                // **A value with a cleanup, bound here, dies where this block
                // ends** (ADR-239 D2).
                if let [single] = names.as_slice() {
                    let name = self.parsed.text(*single).to_string();
                    self.a_value_with_a_cleanup_bound(&name, &found, span);
                }
                // **`let t = s` is a rename, and a rename moves** (ADR-094 D4,
                // ADR-213 D3) - except `let _ = s`, which binds nothing and so
                // takes nothing.
                let bound: Vec<&str> = names.iter().map(|n| self.parsed.text(*n)).collect();
                if bound.iter().any(|n| *n != "_") {
                    let to = format!("bound to `{}` by a `let`", bound.join("`, `"));
                    self.hands_over(value, &found, &to, span);
                }
                // **And a `let` gives the name a value again**, as an
                // assignment does: a name handed over and then bound anew is a
                // new binding, so a read after it is not a read of what was
                // taken. Two loops that each bind `more` and hand it to
                // `take` - `examples/http` - were one name to the path walk.
                let rebound: Vec<String> = bound
                    .iter()
                    .filter(|n| **n != "_")
                    .map(|n| n.to_string())
                    .collect();
                for name in rebound {
                    self.written_at.push((name, span.start));
                }
                // **A `let` over a place is a view of it** (ADR-094 D4), and
                // the emitter needs to know before it writes the line. A bare
                // name is deliberately not a place here: `let y = x` is a
                // rename and stays a move, which is the one shape that
                // separates this rule from `for`'s.
                if matches!(value, Expr::Field { .. } | Expr::Index { .. }) && moves_away(&found) {
                    self.checked.lent_lets.insert(span.start);
                    for name in names {
                        let name = self.parsed.text(*name).to_string();
                        self.lent_lets
                            .insert((name, self.scope.len().saturating_sub(1)));
                    }
                }
                // **A tuple of names takes the value apart**
                // ([ADR-098](../../../docs/specification/adr/adr-098.md)). The
                // parts come from the value's own type where it is a tuple of
                // the right width, and are `?` otherwise - the same silence
                // every other unanswered question here keeps, and never a guess
                // that the widths match.
                if let [_, _, ..] = names.as_slice() {
                    return self.tuple_let(names, ty.as_ref(), &found, span);
                }
                let name = self.parsed.text(names[0]).to_string();
                // **`let _ = expr` is a statement wearing a `let`**
                // ([ADR-126](../../docs/specification/adr/adr-126.md) D2). A
                // binding that ignores its whole value binds nothing, so the
                // word `let` says something that does not happen - and below it
                // is Rust's `let _ =`, which **discards** the value where the
                // source said *bound*. For a file handle or a lock guard those
                // are different programs, which is why the one thing Rust's form
                // is used for is the one thing this refuses.
                //
                // Only the single-name form: `let (a, _) = pair()` is D1's tuple
                // position and binds `a`.
                if name == "_" {
                    self.a_let_that_binds_nothing(span);
                }
                self.nameable(&name, span, "a `let`");

                let bound = match ty {
                    Some(ty) => {
                        let want = self.declared(ty, span);
                        // **The annotation is no longer the constructor**
                        // ([ADR-064](../../docs/specification/adr/adr-064.md) D2):
                        // a hull is made by a call, so a plain value standing here
                        // is an ordinary mismatch - and `convert` is what names the
                        // way out.
                        self.constant_fits(value, Some(&want), span);
                        self.wraps_into_nullable(&found, &want, value, span);
                        // **The annotation is a use, and a use answers the
                        // literal** (ADR-152 D4).
                        let found = self
                            .literal_by_use(&found, &want, value, span)
                            .unwrap_or(found);
                        self.expect_kept(
                            &found,
                            &want,
                            Some(value),
                            "the `let` says `String`, which is text this binding owns",
                            span.clone(),
                            "let",
                            |found, want| format!("this is `{found}`, and the `let` says `{want}`"),
                        );
                        self.a_number_read_through_a_lent_binding(&want, value, span);
                        want
                    }
                    // **A `let mut` of a literal that the body later grows
                    // holds text of its own** (0.0.232). `let mut s = "a"`
                    // then `s = s + "b"` was `NK1105` - *this is `String`, and
                    // what it is assigned to is `ref String`* - with a help to
                    // take a view of the sum, which is no way out at all. A
                    // literal is built into text of its own wherever it is kept
                    // ([ADR-207](../../docs/specification/adr/adr-207.md) D2),
                    // and a binding the program appends to keeps it: the `let`
                    // reads as `let mut s: String = "a"` would.
                    //
                    // **Only where the growing is written as `+` or an
                    // f-string**, which can only be text of its own. A bare
                    // name assigned later may be a view - `best = w` over a
                    // list of views is the ordinary shape - and one read that
                    // way would refuse it.
                    None if *mutable
                        && self.grown_text.contains(&name)
                        && found.is_a_view()
                        && self
                            .text_literal(&Ty::named("String"), value, true)
                            .is_some() =>
                    {
                        Ty::named("String")
                    }
                    None => {
                        // **No annotation, and a type all the same** where an
                        // operand's declaration pinned one: `let a: i32 = …`
                        // then `let b = a + 1` is arithmetic in an `i32`, and
                        // that is the sum ADR-043 §3 left to `rustc`.
                        self.constant_fits(value, None, span);
                        found
                    }
                };
                // **An empty list has no element type, and D2 says where one
                // comes from**
                // ([ADR-135](../../docs/specification/adr/adr-135.md)): the
                // first use that needs one. An annotation is that use and
                // answers it here; without one the question is left open and
                // asked again when the body has been walked, because the use
                // that answers it stands *after* this line.
                let pending = match (ty, value) {
                    (None, Expr::ListLit { items, .. }) if items.is_empty() => {
                        self.empty_lists
                            .insert(span.start, (name.clone(), span.clone()));
                        Some(span.start)
                    }
                    _ => None,
                };
                // **Only an immutable `let` carries its value forward.** A
                // `mut` one may be given another before the name is read again,
                // and this checker does not follow assignments - so the value
                // it started with would be a claim about a program that no
                // longer holds (Part III, C.4).
                let constant = match mutable {
                    false => self.constant_of(value).map(|c| c.value),
                    true => None,
                };
                // **What a task binds for itself** (ADR-055 D6): recorded
                // where the type is known, because the frame goes when the body
                // does.
                if let Some(task) = self.task_bindings.last_mut() {
                    task.push((name.clone(), bound.clone(), span.start));
                }
                self.bind_local(Local {
                    name,
                    ty: bound,
                    constant,
                    // **A `let` is not lent here** even where D4 lends its
                    // initialiser: what the emitter writes there is a `&` in
                    // front of a place, and a cast over the name that comes
                    // out of one is the language below's own deref.
                    lent: false,
                    changing: false,
                    // A `let` binds a number where one folded; the whole value
                    // is a `comptime`'s, which is the one that *must* fold.
                    built: None,
                    empty_list: pending,
                    literal: match (ty, value) {
                        (None, Expr::LitStr { text, .. }) => Some(text.clone()),
                        _ => None,
                    },
                    // **Part I 2.1**: without the word, a change to this name
                    // is `NK1139`. The span is the statement's, which is the
                    // `let` itself — a binding has no narrower one.
                    immutable: (!mutable).then(|| Immutable {
                        at: span.clone(),
                        kind: Kind::Let,
                    }),
                });
                Ty::Tuple(Vec::new())
            }

            // **`comptime` is a `let` that has to fold**
            // ([ADR-073](../../docs/specification/adr/adr-073.md) D3). The fold
            // is the one [ADR-063](../../docs/specification/adr/adr-063.md)
            // already shares, so nothing new evaluates anything: what this arm
            // adds is the refusal where it comes back empty, and the type the
            // emitter will need (D4).
            //
            // **Refused by name rather than run at program time** (D5). Falling
            // back would break the promise the word is for, and quietly - the
            // program would still work and the guarantee would be gone.
            Stmt::Comptime { name, ty, value } => {
                self.comptime_binding(*name, ty.as_ref(), value, span)
            }

            Stmt::Assign {
                target, op, value, ..
            } => {
                // **Code assigned into a function field** (ADR-230 D1): the
                // lock column does not read it, so the field is undecided.
                if let Expr::Field { base, name } = target {
                    let field = self.parsed.text(*name).to_string();
                    // The owner where the base is a name; every struct with a
                    // function field of that name where it is not.
                    let owners: Vec<String> = match base.as_ref() {
                        Expr::Variable(bound) => match self.lookup(self.parsed.text(*bound)) {
                            Some(Ty::Named { name: owner, .. }) => vec![owner],
                            _ => Vec::new(),
                        },
                        _ => self
                            .parsed
                            .program
                            .items
                            .iter()
                            .filter_map(|item| match &item.node {
                                Item::Struct { name, .. } => {
                                    Some(self.parsed.text(*name).to_string())
                                }
                                _ => None,
                            })
                            .collect(),
                    };
                    for owner in owners {
                        let code = self.fields_of(&owner).is_some_and(|d| {
                            d.iter()
                                .any(|f| f.name == field && matches!(f.ty, Ty::Fn { .. }))
                        });
                        if code {
                            self.a_field_whose_code_is_not_seen(&owner, &field);
                        }
                    }
                }
                // `NK2101`: an assignment **revives** a name a task took with
                // it - `message = "other"` after the `spawn` is a correct
                // program - so the target is recorded before it is walked, and
                // walking it records a read this must not be confused with.
                // **And a part of one** (ADR-214 D2): `p.name = …` gives back
                // what `xs.push(p.name)` took.
                if let Some(path) = self.place_path(target) {
                    self.written_at.push((path, span.start));
                }
                // **D3**: an assignment into a parameter, or into a place
                // rooted at one, changes the caller's value and says `mut`.
                if let Some(root) = self.rooted_at(target) {
                    let how = match target {
                        Expr::Variable(_) => "this assigns to it",
                        _ => "this assigns into it",
                    };
                    self.a_changed_binding_says_mut(&root, how);
                    self.a_shared_name_changed_in_parallel(&root, span);
                }
                // **A write through the brackets is unchanged**
                // ([ADR-114](../../docs/specification/adr/adr-114.md) D2):
                // `m[k] = v` inserts or replaces, and what it inserts is a `V`.
                // Only the **read** answers a `T?`, so the slot being written
                // is the value type with that answer taken back off — without
                // this, the write would be handed a `Some(v)` for a map whose
                // values are plain.
                // The target is **written**, not taken: `s = xs.iter()` gives a
                // walked sequence a new one (ADR-212 D4). What is inside an
                // index is still read, and is walked by its own arm.
                let outer =
                    std::mem::replace(&mut self.reading_only, matches!(target, Expr::Variable(_)));
                // **A key written is handed to the map** (ADR-213 D1): the
                // `Index` arm reads this for the one index it is walking.
                self.writing_index = op.is_none() && matches!(target, Expr::Index { .. });
                let into = match target {
                    Expr::Index { .. } => match self.expr(target, span) {
                        Ty::Nullable(inner) => *inner,
                        other => other,
                    },
                    _ => self.expr(target, span),
                };
                self.reading_only = outer;
                self.writing_index = false;
                let found = self.expr(value, span);
                // **An assignment keeps what it is given**, so a text literal
                // assigned to a `String` place - a name, a field, a map's or a
                // list's slot - is built into text of its own there, as it is at
                // every other place that keeps it (ADR-207 D1, ADR-213 D2).
                if op.is_none() {
                    self.a_kept_lambda(&into, value, span);
                }
                let found = match op {
                    None => self.text_literal(&into, value, true).unwrap_or(found),
                    Some(_) => found,
                };
                // **What is assigned is given to the place** (ADR-213 D3).
                if op.is_none() {
                    let to = match target {
                        Expr::Index { .. } => "written into a container",
                        _ => "assigned to another name",
                    };
                    self.hands_over(value, &found, to, span);
                }
                // **A write to shared mutable state goes through a door**
                // ([ADR-099](../../../docs/specification/adr/adr-099.md)).
                self.a_write_that_skips_the_door(target, &into, value, span);
                // **A compound assignment on a map slot is written out**
                // ([ADR-114](../../docs/specification/adr/adr-114.md) D2).
                if op.is_some() {
                    self.a_compound_write_to_a_map_slot(target, span);
                }
                // Only a plain assignment: `n += 1` is whatever the operator
                // makes of the two, and Stage 0 does not model operators.
                if op.is_none() {
                    self.wraps_into_nullable(&found, &into, value, span);
                    if self.owned_text_into_a_literal_binding(target, &found, &into, span) {
                        return Ty::Tuple(Vec::new());
                    }
                    self.expect(&found, &into, span.clone(), "assign", |found, want| {
                        format!("this is `{found}`, and what it is assigned to is `{want}`")
                    });
                }
                Ty::Tuple(Vec::new())
            }

            Stmt::While { cond, body } => {
                let cond_ty = self.expr(cond, span);
                self.expect_bool(&cond_ty, span, "a `while` repeats while a `bool` holds");
                self.repeats
                    .push(Repeats::a_loop(self.parsed, self.scope.len(), body));
                self.scope.push(Vec::new());
                // **The body and not the condition.** A `break` written in the
                // condition is bound to this very loop in the language below,
                // which is legal there and is a program nobody writes; refusing
                // it is the direction that can be taken back later, and
                // accepting it is not.
                self.loops += 1;
                self.block(body);
                self.loops -= 1;
                self.scope.pop();
                self.repeats.pop();
                Ty::Tuple(Vec::new())
            }

            Stmt::For {
                bindings,
                iter,
                body,
            } => {
                self.the_caller_writes_no_reference(iter, span, "a `for` lends what it iterates");
                let over = self.expr(iter, span);
                self.fallible_step(&over, bindings.len(), span);
                self.pausing_step(&over, span);
                // **A `for` walks a produced sequence, and that consumes it**
                // ([ADR-105](../../docs/specification/adr/adr-105.md) D2). A
                // container is walked by view and as often as one likes, which
                // is why the record is keyed on the type being a `Seq`.
                self.a_sequence_is_walked(iter, &over, span);
                let element = element_of(&over, bindings.len());
                // **A name that holds a sequence is handed over, not lent**
                // (ADR-212 D4): the emitter writes no `.iter()` for it, and the
                // binding is each element itself.
                let owned = crate::emit::is_a_place(iter) && matches!(over, Ty::Seq { .. });
                if owned {
                    self.checked.owned_loops.insert(span.start);
                }
                // **A `for` over a place lends** (ADR-094 D4), which the
                // emitter writes as `.iter()` — so the binding is a *view* of
                // each element. Recorded on the binding because the one thing
                // that has to know is a **cast** over it, and Rust's `as` does
                // not see through a reference. The predicate is the emitter's
                // own, so the two cannot answer differently.
                let lent = crate::emit::is_a_place(iter) && !owned;
                let frame: Vec<Local> = bindings
                    .iter()
                    .map(|b| Local {
                        lent,
                        ..Local::free(self.parsed.text(*b).to_string(), element.clone())
                    })
                    .collect();
                for local in &frame {
                    self.nameable(&local.name.clone(), span, "a `for` binding");
                }
                self.repeats
                    .push(Repeats::a_loop(self.parsed, self.scope.len(), body));
                self.scope.push(frame);
                self.loops += 1;
                self.block(body);
                self.loops -= 1;
                self.scope.pop();
                self.repeats.pop();
                Ty::Tuple(Vec::new())
            }

            Stmt::Return(value) => {
                self.returns(value.as_ref(), span);
                Ty::Unknown
            }

            // Part I 3.3. **`Ty::Unknown` and not `()`**, for the reason a
            // `return` hands back one: a statement that jumps is not a value,
            // and a block that ends in one is a block nothing arrives at the
            // end of. `if c { break } else { 1 }` would otherwise be an `if`
            // whose halves disagree, which is a refusal about a program that is
            // right - the language below reads the jumping half as the `!` it
            // is and coerces it to the other.
            Stmt::Break | Stmt::Continue => {
                let word = match stmt {
                    Stmt::Break => "break",
                    _ => "continue",
                };
                if self.loops == 0 {
                    self.a_jump_with_nowhere_to_go(word, span);
                }
                Ty::Unknown
            }

            // **One site, not two.** A statement that is one name is an
            // expression statement, so the walk below reaches it - and the
            // refusal now lives where every name is read rather than only where
            // one stands alone.
            Stmt::Expr(expr) => self.expr(expr, span),
        }
    }

    // --- expressions --------------------------------------------------------

    fn expr(&mut self, expr: &Expr, span: &Span) -> Ty {
        let ty = self.value_of(expr, span);
        // **A value with a cleanup made here may die in this function**
        // (ADR-239 D2): by a call or a literal, which is where one comes from.
        // A read of a name or a field is not a death, and is not asked.
        if matches!(
            expr,
            Expr::Call { .. }
                | Expr::MethodCall { .. }
                | Expr::SafeMethod { .. }
                | Expr::StructLit { .. }
        ) {
            self.a_value_with_a_cleanup_made(&ty);
        }
        ty
    }

    fn value_of(&mut self, expr: &Expr, span: &Span) -> Ty {
        match expr {
            // A bare number fits every numeric type, exactly as it does in the
            // language below. Committing it to one here would make `add(3)`
            // wrong wherever the parameter is not that one.
            Expr::LitInt(_) | Expr::LitFloat(_) => Ty::Unknown,
            // Part I 2.4 calls a string literal a `String`; Stage 0 emits a
            // Rust string literal, which is a view of static text. The checker
            // says what is emitted - see ADR-024 D5.
            //
            // **The type comes from the syntax** (ADR-035 D3). `"…"` is a view
            // of static text and `f"…"` is a `format!`, which is a `String` -
            // and which one a literal is can be read off its first character
            // rather than worked out from whether somebody happened to type a
            // brace somewhere in it.
            Expr::LitStr { text, .. } => {
                self.an_escape_nothing_names(text, span);
                self.unmarked_hole(text, span);
                Ty::view("str")
            }
            // **A hole is checked like anything else** (ADR-032 D3). It is
            // Nikaia source written inside a literal, and until that walk
            // existed it was source no analysis could see - the same mistake
            // was caught outside a hole and silently passed inside one.
            Expr::LitInterpolated(text) => {
                // **The whole literal, holes and all**, because an escape
                // belongs to the *literal* and not to the hole: `f"{f(\"a\")}"`
                // writes `\"` twice inside a hole and the outer quotes are what
                // they escape.
                self.an_escape_nothing_names(text, span);
                self.holes(expr, span);
                Ty::named("String")
            }
            // A character literal is kept as written too, so `'\q'` is the
            // same refusal one literal over.
            Expr::LitChar(text) => {
                self.an_escape_nothing_names(text, span);
                Ty::named("char")
            }
            Expr::LitBool(_) => Ty::named("bool"),
            // **A nullable of it-does-not-say.** `null` names the absence of a
            // value without naming what value, so the inside is `Unknown` and
            // anything nullable fits it - which is what makes `let mut m: &str?
            // = null` right and what keeps `let m = null` from being refused
            // here: `let mut m = null` then `m = "hi"` is a correct program
            // (Part III, C.4), and it is `rustc` that asks for an annotation
            // where nothing ever says.
            Expr::LitNull => Ty::Nullable(Box::new(Ty::Unknown)),

            Expr::Variable(name) => {
                let name = self.parsed.text(*name);
                // `NK2101`: where this name is read, for the question a `spawn`
                // asks about what comes after it.
                self.read_at.push((name.to_string(), span.start));
                // **D2's question, answered where every name is read**
                // ([ADR-135](../../docs/specification/adr/adr-135.md)): an
                // empty list takes its element type from its first use, so a
                // use - any use - is what says one exists.
                if let Some(at) = self.binding(name).and_then(|local| local.empty_list) {
                    self.empty_lists.remove(&at);
                }
                match self.lookup(name) {
                    Some(ty) => {
                        self.a_read(expr, name.to_string(), span);
                        self.a_sequence_is_taken(name, &ty, span);
                        ty
                    }
                    // **`assert` is called** (ADR-245 D2): the statement
                    // form Part III 14.1 used to write, `assert c`, is the
                    // name alone and then `c`, and the name is only ever the
                    // head of a call.
                    None if self.the_prelude_assert(expr) => {
                        self.an_assert_of_another_shape(
                            "`assert` is a function, and is called: write `assert(cond)`"
                                .to_string(),
                            span,
                        );
                        Ty::Unknown
                    }
                    None => {
                        // **The specific message wins.** A free `a`, `b` or `c`
                        // is the mistake a reader of the old specification
                        // makes, and `withdrawn_automatic_name` says what
                        // happened to the form rather than only that the name
                        // is unknown (ADR-049 §5). Where it has spoken, the
                        // general refusal would be a second finding about one
                        // mistake.
                        if !self.withdrawn_automatic_name(name, span) {
                            self.nothing_declares_it(expr, span);
                        }
                        Ty::Unknown
                    }
                }
            }

            Expr::Path(segments) => {
                // `Op::Times` is a value of the enum that declares it. Anything
                // else a path can name, this compiler does not resolve.
                let names: Vec<&str> = segments.iter().map(|s| self.parsed.text(*s)).collect();
                // **A constructor handed over as a value** is the other half of
                // [ADR-140](../../docs/specification/adr/adr-140.md) D2, and the
                // one that record names: `par_fold(M, Summary::new, …)` is how
                // `1brc.nika` wrote it, against a type declaring an anonymous
                // constructor and no `new`.
                self.a_constructor_written_as_new(&names.join("::"), span);
                match names.as_slice() {
                    [ty, variant] if self.is_variant(ty, variant) => Ty::named(*ty),
                    // **`T::fields` is a list of the type's fields**
                    // ([ADR-088](../../docs/specification/adr/adr-088.md) D2,
                    // built by [ADR-181](../../docs/specification/adr/adr-181.md)).
                    // What it hands back is a sequence, so the `for` over it is
                    // the `for` this checker already reads — D4's *one loop*,
                    // arrived at rather than added.
                    [ty, member]
                        if self.walks_fields.values().any(|p| p == ty)
                            && self.shape_element(ty, member).is_some() =>
                    {
                        Ty::Named {
                            name: "Vec".to_string(),
                            args: vec![Ty::named(
                                self.shape_element(ty, member).unwrap_or_default(),
                            )],
                            view: false,
                        }
                    }
                    [ty, member] => {
                        self.a_member_this_type_does_not_have(ty, member, span);
                        Ty::Unknown
                    }
                    _ => Ty::Unknown,
                }
            }

            // A `seq` block is a block for every purpose but one: what it says
            // is about the *order* its statements run in (ADR-033 D7), not
            // about what any of them mean or what it hands back.
            // **`unsafe { … }` is a block with a value and no other rule**
            // ([ADR-124](../../docs/specification/adr/adr-124.md) D3): what is
            // inside is checked exactly as anything else is.
            Expr::Block(block) => self.block(block),

            // **`unsafe { … }` is a block with a value and no other rule**
            // ([ADR-124](../../docs/specification/adr/adr-124.md) D3): what is
            // inside is checked exactly as anything else is. The one thing it
            // changes is that a call to an `extern` name is allowed here, which
            // is the whole of what the word buys.
            Expr::Unsafe(block) => {
                let outer = std::mem::replace(&mut self.inside_unsafe, true);
                let value = self.block(block);
                self.inside_unsafe = outer;
                value
            }

            // **Part I 8.1.2: each statement is a branch, and the block's value
            // is the tuple of their results in written order**
            // ([ADR-050](../../docs/specification/adr/adr-050.md) D2).
            //
            // Walked here rather than through the `Block` arm above, because a
            // block hands back its *last* statement and this hands back all of
            // them. The scope frame is the block's own, so a name a branch binds
            // does not leak - and `branches_meet_on_nothing` is what refuses one
            // that binds at all, since the block's value already carries it.
            Expr::Overlap(block) => {
                self.scope.push(Vec::new());
                let parts: Vec<Ty> = block
                    .stmts
                    .iter()
                    .map(|stmt| {
                        // Each branch is an `async` block of its own (ADR-050
                        // D2), so the boundary is per branch rather than per
                        // block.
                        self.past_a_boundary("`overlap` branch", |me| match &stmt.node {
                            Stmt::Expr(value) => me.expr(value, &stmt.span),
                            other => {
                                me.stmt(other, &stmt.span);
                                Ty::Unknown
                            }
                        })
                    })
                    .collect();
                self.scope.pop();
                self.branches_meet_on_nothing(block, span);
                Ty::Tuple(parts)
            }

            // **Part II 12.4: every arm is started and the first to finish
            // wins** ([ADR-148](../../docs/specification/adr/adr-148.md) D1).
            //
            // Each raced expression is an `async` block of its own, exactly as
            // an `overlap` branch is, so the boundary is per arm. What the arm
            // *binds* is the value that came back, so the body is walked in a
            // scope holding that one name - and `_` binds nothing, which is
            // [ADR-126](../../docs/specification/adr/adr-126.md) D1's ignore
            // pattern rather than a catch-all arm.
            //
            // **The type is the `match` rule one construct over**: an arm that
            // jumps is not one of the types that have to agree
            // ([ADR-138](../../docs/specification/adr/adr-138.md) D1), and
            // where the rest do not say the same the answer is `Unknown`
            // rather than a guess. Part II 12.4's own example is two jumping
            // arms, so it is a `select` of no value at all.
            Expr::Select(arms) => {
                let mut result: Option<Ty> = None;
                let mut agree = true;
                for arm in arms {
                    let raced =
                        self.past_a_boundary("`select` arm", |me| me.expr(&arm.value, span));
                    let frame = match arm.binding {
                        Some(name) => {
                            vec![Local::free(self.parsed.text(name).to_string(), raced)]
                        }
                        None => Vec::new(),
                    };
                    self.scope.push(frame);
                    let ty = self.past_a_boundary("`select` arm", |me| me.block(&arm.body));
                    self.scope.pop();
                    if block_leaves(&arm.body) {
                        continue;
                    }
                    match &result {
                        None => result = Some(ty),
                        Some(seen) if *seen == ty => {}
                        Some(_) => agree = false,
                    }
                }
                match result {
                    Some(ty) if agree => ty,
                    _ => Ty::Unknown,
                }
            }

            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_ty = self.expr(cond, span);
                self.expect_bool(&cond_ty, span, "an `if` decides on a `bool`");
                // **A decision taken on what a lock said is stale too**
                // ([ADR-111](../../docs/specification/adr/adr-111.md) D4's
                // second shape). It reaches inward and **accumulates**: a
                // nested `if` under a stamped one is still under it, and a
                // plain condition inside does not clear the outer one.
                let outer_condition = self.stamped_condition;
                if cond_ty.is_seen() {
                    self.stamped_condition = Some(span.start);
                }
                let choice = self.next_choice;
                self.next_choice += 1;
                self.branch.push((choice, 0));
                let from = self.taken_so_far();
                let then = self.block(then_branch);
                self.a_branch_that_leaves(from, block_exits(then_branch), span);
                self.branch.pop();
                let branches = match else_branch {
                    Some(otherwise) => {
                        self.branch.push((choice, 1));
                        let from = self.taken_so_far();
                        let other = self.block(otherwise);
                        self.a_branch_that_leaves(from, block_exits(otherwise), span);
                        self.branch.pop();
                        // Only when both arms agree is there something to say
                        // - or when they meet at text (ADR-207 D2).
                        if then == other {
                            then
                        } else {
                            let arms = [(tail_of(then_branch), then), (tail_of(otherwise), other)];
                            self.arms_meet_at_text(&arms).unwrap_or(Ty::Unknown)
                        }
                    }
                    // An `if` with no `else` is a statement's worth of value.
                    None => Ty::Unknown,
                };
                self.stamped_condition = outer_condition;
                branches
            }

            Expr::Match { value, arms } => {
                let on = self.expr(value, span);
                // `error` in a handler is untyped at its binding; where one
                // type arrives, that is what its parts are read from.
                let typed = match (&on, &**value) {
                    (Ty::Unknown, Expr::Variable(name))
                        if self.parsed.text(*name) == "error" && !self.caught_several =>
                    {
                        self.caught_one.clone().unwrap_or(Ty::Unknown)
                    }
                    _ => on.clone(),
                };
                let lendable = self.an_owned_place(value, &typed);
                // **A `match` is a condition too** (ADR-111 D4).
                let outer_condition = self.stamped_condition;
                if on.is_seen() {
                    self.stamped_condition = Some(span.start);
                }
                let mut result: Option<Ty> = None;
                let mut agree = true;
                let mut answered: Vec<(Option<&Expr>, Ty)> = Vec::new();
                let choice = self.next_choice;
                self.next_choice += 1;
                for (taken, arm) in arms.iter().enumerate() {
                    // **Every alternative of an or-pattern binds the same
                    // names** ([ADR-137](../../docs/specification/adr/adr-137.md)
                    // D1), asked before the body is walked so that the body's
                    // scope is one this checker can stand behind.
                    self.an_or_pattern_that_binds_unevenly(&arm.pattern, span);
                    // **And a pattern that names a variant the type does not
                    // have**, which the exhaustiveness check below cannot say:
                    // it reads which variants were *covered*, and a misspelling
                    // covers none, so what it reports is the variant that is
                    // missing rather than the name that is wrong — and with an
                    // `else` arm beside it, nothing at all.
                    self.a_pattern_naming_a_member_a_type_does_not_have(&arm.pattern, span);
                    self.a_pattern_inside_a_box(&arm.pattern, span);
                    // **Over a place this function owns, a part is lent**
                    // (ADR-242): the bindings are typed from the
                    // variant, so that what the arm keeps is seen as kept.
                    let parts = match lendable {
                        true => self.pattern_parts(&arm.pattern, &typed),
                        false => BTreeMap::new(),
                    };
                    // **A number bound out of a value the `match` was lent**
                    // is copied out at the head of the arm (0.0.236): the
                    // language below binds a `&i64` there, and
                    // `Expr::Num(n) => n` handed back a reference where the
                    // function says `i64` - `rustc`'s *mismatched types*
                    // about a file nobody wrote.
                    if typed.is_a_view() {
                        let at = &arm.pattern as *const MatchPattern as usize;
                        for (name, ty) in self.pattern_parts(&arm.pattern, &typed) {
                            if copies(&ty) && !ty.is_a_view() {
                                self.checked.copied_bindings.insert((at, name));
                            }
                        }
                    }
                    let frame = self
                        .pattern_bindings(&arm.pattern)
                        .into_iter()
                        .map(|local| match parts.get(&local.name) {
                            Some(ty) => Local::free(local.name, ty.clone()),
                            None => local,
                        })
                        .collect();
                    self.scope.push(frame);
                    self.branch.push((choice, taken));
                    let from = self.taken_so_far();
                    let changed_from = self.changed.len();
                    // **The guard is walked inside the arm's scope** (D2): it
                    // reads the names the pattern bound, and a condition is a
                    // `bool` here exactly as anywhere else.
                    if let Some(guard) = &arm.guard {
                        let found = self.expr(guard, span);
                        self.expect_bool(&found, span, "a `match` arm's guard is a condition");
                    }
                    let ty = self.expr(&arm.body, span);
                    self.a_part_lent_where_it_is_not_kept(
                        &arm.pattern,
                        &arm.body,
                        &parts,
                        from,
                        changed_from,
                    );
                    self.a_branch_that_leaves(from, exits(&arm.body), span);
                    self.branch.pop();
                    self.scope.pop();
                    // **An arm that jumps is not one of the types that have to
                    // agree** ([ADR-138](../../docs/specification/adr/adr-138.md)
                    // D1): its type is *never*, which fits every expected type
                    // without widening anything, so an arm that throws sits
                    // beside an arm that hands back a `&str` and the `match` is
                    // a `&str`. The language below reads the jumping arm as the
                    // `!` it is and agrees by construction.
                    if leaves(&arm.body) {
                        continue;
                    }
                    answered.push((Some(&arm.body), ty.clone()));
                    match &result {
                        None => result = Some(ty),
                        Some(seen) if *seen == ty => {}
                        Some(_) => agree = false,
                    }
                }
                self.stamped_condition = outer_condition;
                self.a_match_over_several_error_types(value, arms, span);
                // **`match error { … }` is a `match` over the type that
                // arrived**, where exactly one does. The binding itself is
                // untyped (see [`Checker::caught_one`]); this hands the one
                // question that can only gain by knowing.
                let on = match (&on, &**value) {
                    (Ty::Unknown, Expr::Variable(name))
                        if self.parsed.text(*name) == "error" && !self.caught_several =>
                    {
                        self.caught_one.clone().unwrap_or(Ty::Unknown)
                    }
                    _ => on,
                };
                self.a_match_that_misses_a_case(&on, arms, span);
                // Every arm of a `match` is a value of the same type, but what
                // that type is, is only known when every arm says the same -
                // or when the arms meet at text (ADR-207 D2).
                match result {
                    Some(ty) if agree => ty,
                    _ => self.arms_meet_at_text(&answered).unwrap_or(Ty::Unknown),
                }
            }

            Expr::Call { func, args, config } => match self.the_prelude_assert(func) {
                true => self.an_assert(args, config, span),
                false => self.call(func, args, config, span),
            },

            Expr::MethodCall {
                receiver,
                method,
                args,
                config,
            } => {
                // **`after:` on a `set` is a witness and not an option**
                // ([ADR-111](../../docs/specification/adr/adr-111.md) D5): it
                // is walked below as an *argument*, where the door's signature
                // is what says it has to be a `$T`. Found here by name, and
                // confirmed to be the door once the receiver's type is in hand
                // - a `set` on something that is not a lock takes this back.
                let witness = match self.parsed.text(*method) == "set" {
                    true => config
                        .iter()
                        .position(|a| self.parsed.text(a.name) == "after"),
                    false => None,
                };
                // ADR-007 D5: a DSL's deferred parameters stand here. They are
                // expressions like any other, so they are walked - what checks
                // that they are the *right* names is `dsl::check`, which knows
                // which statement the receiver came from.
                config.iter().enumerate().for_each(|(at, a)| {
                    if Some(at) != witness {
                        self.expr(&a.value, span);
                    }
                });
                // **A grammar is entered by an ordinary call**
                // ([ADR-082](../../docs/specification/adr/adr-082.md) D1), so
                // this is that call: a receiver naming a grammar of this file
                // and a method naming one of its `pub` rules. Answered before
                // the receiver is typed, because a grammar name is not a value
                // and typing it would be asking the wrong question.
                // **Both ways out of here owe the witness its walk.** It was
                // held back above so that the door can walk it as an argument,
                // and neither of these reaches the door: a grammar rule is not
                // a lock, and a `T?` receiver is answered before the call is.
                // A value that nothing walks is a mistake nothing reports.
                if let Some(entered) = self.grammar_entry(receiver, *method) {
                    if let Some(at) = witness {
                        self.expr(&config[at].value, span);
                    }
                    // **The dot is gone** (ADR-140 D3), and the refusal is
                    // here rather than in the parser because only this side
                    // knows the receiver names a grammar: `Json.value(x)` and
                    // `text.value(x)` are the same five tokens.
                    let (grammar, rule) = entered.split_once("::").unwrap_or((&entered, ""));
                    self.a_grammar_reached_through_a_dot(grammar, rule, span);
                    return self.grammar_call(&entered, args, span);
                }
                let on = self.expr(receiver, span);
                // **The tier pass's own conversion**
                // ([ADR-224](../../docs/specification/adr/adr-224.md) D2):
                // `value.into_either()` is written into the program where a
                // value goes into text both kinds flow into, and it hands the
                // value on as it is - absent where it may be absent - so it
                // reads as the value's own type, and what the position accepts
                // is decided exactly as it was before the pass wrote it.
                // Into a `String?` it is `value.into_either_maybe()`, which is
                // absent-or-present whatever the value was, so no wrap is
                // written around it; before a `collect`, `either_items()`,
                // which is the same items.
                // What goes in is `EitherText`, which reads as text of its
                // own (ADR-223 D3): a view handed over this way is not a view
                // kept.
                let own = |ty: Ty| match ty {
                    ty if ty == Ty::view("str") => Ty::named("String"),
                    Ty::Nullable(inner) if *inner == Ty::view("str") => {
                        Ty::Nullable(Box::new(Ty::named("String")))
                    }
                    ty => ty,
                };
                if args.is_empty() && config.is_empty() {
                    match self.parsed.text(*method) {
                        "into_either" => return own(on),
                        "into_either_maybe" => {
                            return match own(on) {
                                Ty::Nullable(inner) => Ty::Nullable(inner),
                                other => Ty::Nullable(Box::new(other)),
                            };
                        }
                        "either_items" | "either_keys" | "either_values" | "either_pairs" => {
                            let which = self.parsed.text(*method);
                            // Of a pair, only the half the name says.
                            let own = |item: Ty| match (which, item) {
                                ("either_items", item) => own(item),
                                (_, Ty::Tuple(parts)) => Ty::Tuple(
                                    parts
                                        .into_iter()
                                        .enumerate()
                                        .map(|(at, part)| match (which, at) {
                                            ("either_keys", 0)
                                            | ("either_values", 1)
                                            | ("either_pairs", _) => own(part),
                                            _ => part,
                                        })
                                        .collect(),
                                ),
                                (_, item) => item,
                            };
                            return match on {
                                Ty::Seq {
                                    item,
                                    is_sync,
                                    pauses,
                                    throws,
                                    parallel,
                                    shape,
                                } => Ty::Seq {
                                    item: Box::new(own(*item)),
                                    is_sync,
                                    pauses,
                                    throws,
                                    parallel,
                                    shape,
                                },
                                other => other,
                            };
                        }
                        _ => {}
                    }
                }
                // **`field.of(value)`, the one method a reflected field has**
                // ([ADR-088](../../docs/specification/adr/adr-088.md) D2).
                //
                // What it answers is the field's type **on this unrolling**, and
                // `?` where there is none — which is the generic body, walked
                // once with nothing known. That walk must refuse nothing about a
                // field it cannot see, because a body wrong for one field is
                // wrong at one unrolled copy and right at the others (D5), and
                // saying so from the generic walk would be saying it about all
                // of them.
                // **`variant.is(value)`, the one method a reflected variant
                // has**: whether the value is that variant. A `bool` on every
                // turn, so the generic walk and the unrolled one agree.
                if matches!(&on, Ty::Named { name, .. } if name == ty::VARIANT) {
                    args.iter().for_each(|a| {
                        self.expr(a, span);
                    });
                    if let Some(at) = witness {
                        self.expr(&config[at].value, span);
                    }
                    let named = self.parsed.text(*method).to_string();
                    if named != "is" || args.len() != 1 {
                        self.a_reflected_variant_has_two_members(&format!("{named}(…)"), span);
                        return Ty::Unknown;
                    }
                    return Ty::named("bool");
                }
                if matches!(&on, Ty::Named { name, .. } if name == ty::FIELD) {
                    args.iter().for_each(|a| {
                        self.expr(a, span);
                    });
                    if let Some(at) = witness {
                        self.expr(&config[at].value, span);
                    }
                    let named = self.parsed.text(*method).to_string();
                    if named != "of" {
                        self.a_reflected_field_has_two_members(&format!("{named}(…)"), span);
                        return Ty::Unknown;
                    }
                    return match &self.unrolling {
                        Some((_, field)) => field.ty.clone(),
                        None => Ty::Unknown,
                    };
                }
                if let Ty::Nullable(_) = &on {
                    let name = self.parsed.text(*method).to_string();
                    self.reaches_into_a_nullable(&on, Reached::Method(&name), span);
                    // The arguments are still walked: a mistake inside one is a
                    // mistake whatever is wrong with the receiver.
                    args.iter().for_each(|a| {
                        self.expr(a, span);
                    });
                    if let Some(at) = witness {
                        self.expr(&config[at].value, span);
                    }
                    return Ty::Unknown;
                }
                // **The witness door, now that the receiver is typed**
                // ([ADR-111](../../docs/specification/adr/adr-111.md) D5).
                // `kasse.set(neu; after: stand)` is `Locked::set(after)` in the
                // ledger — a key no program can write as a method name, which
                // is what keeps D5's *one door* one (`NK2208` is what a program
                // that tries meets). The witness joins the arguments, so
                // everything a call is checked for reaches it through the one
                // path: the fit against `$T`, the lend, the stamp.
                //
                // **Before `NK2203`**, which names the entry it refuses and
                // would otherwise name `set` for a call that is not one.
                let door = witness
                    .filter(|_| locked_content_of(&on).is_some())
                    .map(|at| &config[at].value);
                if let (Some(at), None) = (witness, door) {
                    // Not a lock after all, so it was an option like any other
                    // and the walk above owed it a visit.
                    self.expr(&config[at].value, span);
                }
                let given: Vec<Expr>;
                let (args, written) = match door {
                    Some(seen) => {
                        self.the_witness_takes_no_reference(seen, span);
                        self.checked.witnessed_sets.insert(span.start);
                        given = args.iter().chain([seen]).cloned().collect();
                        (&given[..], "set(after)".to_string())
                    }
                    None => (&args[..], self.parsed.text(*method).to_string()),
                };
                // **A `set` given a stamped value** (D4). The
                // rule wants the receiver's **name** for its message and the
                // argument's **type** for its answer, and those are known in
                // two different places — so the name is carried the way the
                // door flags above are, and the rule itself runs in `call_on`
                // where the arguments have been typed.
                let outer_receiver = std::mem::replace(
                    &mut self.set_receiver,
                    match receiver.as_ref() {
                        Expr::Variable(name) => Some(self.parsed.text(*name).to_string()),
                        _ => Some("this lock".to_string()),
                    },
                );
                // The same carrying, for D2's once-only rule, and `None` where
                // the receiver is a temporary.
                let outer_named = std::mem::replace(
                    &mut self.receiver_name,
                    match receiver.as_ref() {
                        Expr::Variable(name) => Some(self.parsed.text(*name).to_string()),
                        _ => None,
                    },
                );
                self.a_door_that_is_not_written(&on, &written, span);
                // **`NK2203`, the method half**: which entry `other.get()` goes
                // to is the type checker's answer (ADR-028), so it is asked
                // here where the receiver's type is in hand.
                if self.inside_a_door
                    && let Ty::Named { name, .. } = &on
                {
                    let key = format!("{name}::{written}");
                    if let Some((key, contract)) = self.method(&key) {
                        let (key, holds) = (key.clone(), contract.touches_a_lock);
                        self.a_lock_inside_a_lock(&key, holds, span);
                    }
                }
                // **And a method on a mapping reads the file**
                // ([ADR-169](../../docs/specification/adr/adr-169.md) D2):
                // `mapped.lines()` walks pages that are not there yet.
                self.io_inside_a_door(&on, &format!("`{written}`"), span);
                // **`update`'s block is a write door's**
                // ([ADR-110](../../docs/specification/adr/adr-110.md) D1), and
                // that is where a parameter without `mut` is refused. Set
                // around the call and restored after it, so a lambda written
                // inside the block is not one.
                let at_a_door =
                    self.parsed.text(*method) == "update" && locked_content_of(&on).is_some();
                if at_a_door && let Some(Expr::Closure { params, body, .. }) = args.last() {
                    self.an_update_block_returns_nothing(body, span);
                    self.an_update_block_reads_what_it_writes(params, body, span);
                }
                let outer_door = std::mem::replace(&mut self.at_a_write_door, at_a_door);
                // **And `access` holds one open too** (ADR-039 D10): `get` and
                // `set` do not, because no code of the program's runs while the
                // lock is open in either.
                let holding = matches!(self.parsed.text(*method), "update" | "access")
                    && locked_content_of(&on).is_some();
                // Kept for the stamp below, since `call_on` takes `on` by value.
                let on_for_the_stamp = on.clone();
                let outer_inside = std::mem::replace(&mut self.inside_a_door, holding);
                // **D3**: a method that changes its subject, called on a
                // parameter. Asked of the `mutates` column (D3's own, recorded
                // because the ledger's `&T` cannot spell `&mut`), and only
                // where **every** candidate for the name agrees — one that does
                // not is a name this compiler cannot resolve, and refusing on
                // it would refuse a correct program (C.4).
                if let Some(root) = self.rooted_at(receiver) {
                    let name = self.parsed.text(*method);
                    let candidates: Vec<_> = self
                        .own
                        .candidates(name)
                        .into_iter()
                        .chain(self.library.candidates(name))
                        .collect();
                    let changes = !candidates.is_empty()
                        && candidates.iter().all(|(_, contract)| contract.mutates);
                    if changes {
                        self.a_changed_binding_says_mut(
                            &root,
                            &format!("`{name}` changes what it is called on"),
                        );
                        self.a_shared_name_changed_in_parallel(&root, span);
                    }
                }
                self.a_copy_under_another_name(receiver, *method, args, span);
                // **The text form of text is the text** (ADR-216 D4): a view
                // stays a view and a literal stays a literal, and whoever keeps
                // it asks for text of its own as anywhere else - by building a
                // literal there, or by a `.clone()` the program writes.
                let text = matches!(&on, Ty::Named { name, args: none, .. }
                    if none.is_empty() && matches!(name.as_str(), "String" | "str"));
                if text
                    && args.is_empty()
                    && self.parsed.text(*method) == "to_string"
                    && self.own.candidates("to_string").is_empty()
                {
                    self.checked
                        .text_as_is
                        .insert((span.start, argument_shape(receiver)));
                    self.receiver_name = outer_named;
                    self.at_a_write_door = outer_door;
                    self.inside_a_door = outer_inside;
                    return on;
                }
                // **A field that holds a function is called as one**:
                // `button.on_click(4)`, where `on_click` is a field and no
                // method has the name.
                if let Some(Ty::Fn {
                    params,
                    result,
                    is_sync,
                    throws,
                }) = self.a_field_that_is_code(&on, *method)
                {
                    self.arguments_given(args, &params, false, None, span);
                    self.kept_arguments_are_its_own(args, &params);
                    // **A call through a function field reaches what was
                    // stored there** (ADR-230 D1, D2): recorded for the lock
                    // column, and asked here where a lock is already open.
                    if let Ty::Named { name: owner, .. } = &on {
                        let key = format!("{owner}.{}", self.parsed.text(*method));
                        if let Some(current) = &self.current
                            && self.task_bindings.is_empty()
                        {
                            self.checked
                                .methods
                                .entry(current.clone())
                                .or_default()
                                .fields_called
                                .insert(key.clone());
                        }
                        // The door this call stands in, not this call: the flag
                        // was set above for the call's own block.
                        let holds = self.own.code_locks.get(&key).copied();
                        if let Some(holds) = holds {
                            let own = std::mem::replace(&mut self.inside_a_door, outer_inside);
                            self.a_lock_inside_a_lock(&key, holds, span);
                            self.inside_a_door = own;
                        }
                    }
                    self.checked.field_calls.insert(
                        (
                            span.start,
                            format!("{}.{}", argument_shape(receiver), self.parsed.text(*method)),
                        ),
                        !is_sync,
                    );
                    if let Some(current) = &self.current {
                        let entry = self.checked.methods.entry(current.clone()).or_default();
                        entry.code_pauses |= !is_sync;
                        entry.code_fails |= throws;
                    }
                    if throws {
                        self.fallible_methods
                            .insert((span.start, self.parsed.text(*method).to_string()));
                    }
                    self.receiver_name = outer_named;
                    self.at_a_write_door = outer_door;
                    self.inside_a_door = outer_inside;
                    return result.map(|r| *r).unwrap_or_else(|| Ty::named("()"));
                }
                // **A copy of a slice is a list** (ADR-216 D2): `to_owned`
                // below, where `.clone()` would copy the reference.
                let a_run = match &on {
                    // What a parameter declared `ref Array[T]` is (ADR-184 D3).
                    Ty::Pointed {
                        item, slice: true, ..
                    } => Some((**item).clone()),
                    Ty::Named {
                        name,
                        args: run,
                        view: true,
                    } if name == ty::ARRAY && run.len() == 1 => Some(run[0].clone()),
                    _ => None,
                };
                if let (Some(item), "clone", true) =
                    (&a_run, self.parsed.text(*method), args.is_empty())
                {
                    self.checked
                        .owned_copies
                        .insert((span.start, argument_shape(receiver)));
                    self.receiver_name = outer_named;
                    self.at_a_write_door = outer_door;
                    self.inside_a_door = outer_inside;
                    return Ty::Named {
                        name: "Vec".to_string(),
                        args: vec![item.clone()],
                        view: false,
                    };
                }
                self.last_resolved = None;
                let value = self.call_on(on, *method, args, &written, span);
                // **A copy of `std`'s is a copy whatever the receiver is**
                // (ADR-215 D4): below, `.clone()` of a view is the view.
                if let Some(key) = self.last_resolved.take() {
                    let library = key.ends_with("::clone")
                        && self.library.functions.contains_key(&key)
                        && !self.own.functions.contains_key(&key);
                    if library && args.is_empty() {
                        self.checked
                            .owned_copies
                            .insert((span.start, argument_shape(receiver)));
                    }
                    // **A sequence's count is an `i64`** (Part I 2.2): the
                    // emitter writes the conversion a length has.
                    let counts = [ty::SEQ, ty::PAR]
                        .iter()
                        .any(|of| key == format!("{of}::count"));
                    if counts && !self.own.functions.contains_key(&key) {
                        self.checked
                            .counted
                            .insert((span.start, argument_shape(receiver)));
                    }
                }
                self.receiver_name = outer_named;
                self.at_a_write_door = outer_door;
                self.inside_a_door = outer_inside;
                self.set_receiver = outer_receiver;
                // **A walk over views of values that copy yields the values**
                // ([ADR-231](../../docs/specification/adr/adr-231.md) D1): a
                // view of an `i64` is a reference below, and a number read
                // through one compared with a number was `rustc`'s error.
                // Copying one costs less than pointing at it.
                let value = match value {
                    Ty::Seq {
                        item,
                        is_sync,
                        pauses,
                        throws,
                        parallel,
                        shape,
                    } if copies_as_a_view(&item) => {
                        self.checked
                            .copied_walks
                            .insert((span.start, argument_shape(expr)));
                        Ty::Seq {
                            item: Box::new(unviewed(&item)),
                            is_sync,
                            pauses,
                            throws,
                            parallel,
                            shape,
                        }
                    }
                    other => other,
                };
                // **`filter` is handed a reference to the item** below
                // (ADR-231 D2); where the item copies, the parameter takes it
                // by pattern and is the item.
                if self.parsed.text(*method) == "filter"
                    && let (Ty::Seq { item, .. }, [lambda @ Expr::Closure { params, .. }]) =
                        (&on_for_the_stamp, args)
                    && params.len() == 1
                    && !item.is_unknown()
                    && (item.is_a_view() || !crate::contracts::keeps::moves(item))
                {
                    self.checked
                        .filter_patterns
                        .insert((span.start, argument_shape(lambda)));
                }
                // **And what `access` hands back came out of a lock** (D1).

                match self.parsed.text(*method) == "access"
                    && locked_content_of(&on_for_the_stamp).is_some()
                {
                    true => Ty::seen(value.unseen()),
                    false => value,
                }
            }

            // Part I 3.5: `x?.m(…)`. The receiver must be a `T?`, the call
            // happens only where there is something to call it on, and the
            // result is flattened for [`Expr::SafeField`]'s reason - a method
            // that hands back a `T?` would otherwise give a nullable of a
            // nullable ([ADR-066](../../../docs/specification/adr/adr-066.md)).
            Expr::SafeMethod {
                receiver,
                method,
                args,
                config,
            } => {
                // **The witness is held back here too**
                // ([ADR-111](../../docs/specification/adr/adr-111.md) D5), for
                // the reason a `?.` is one arm and not two: it decides
                // *whether* the call happens and never *what* a call is. A
                // `SharedMut[i64]?` reached with `?.set(neu; after: stand)` is
                // the same door, and the witness that stayed an option here
                // would have been **dropped in the lowering** rather than
                // compared — a silent wrong value, which is worse than
                // anything a refusal costs.
                let witness = match self.parsed.text(*method) == "set" {
                    true => config
                        .iter()
                        .position(|a| self.parsed.text(a.name) == "after"),
                    false => None,
                };
                config.iter().enumerate().for_each(|(at, a)| {
                    if Some(at) != witness {
                        self.expr(&a.value, span);
                    }
                });
                let on = self.expr(receiver, span);
                let name = self.parsed.text(*method).to_string();
                let Ty::Nullable(inner) = on else {
                    self.reaches_through_a_plain_value(&on, Reached::Method(&name), span);
                    // The arguments are still walked: a mistake inside one is
                    // a mistake whatever is wrong with the receiver, and a
                    // reader owed two messages should get two.
                    args.iter().for_each(|a| {
                        self.expr(a, span);
                    });
                    if let Some(at) = witness {
                        self.expr(&config[at].value, span);
                    }
                    return Ty::Unknown;
                };
                // **The same call, on the value inside.** Everything a method
                // call is checked for - what it may throw, whether it pauses,
                // what its arguments have to be, what the receiver's own type
                // binds - is unchanged by the reach: a `?.` decides *whether*
                // the call happens and never *what* a call is. The door is one
                // of those things, so it is resolved here exactly as it is
                // there, off the type **inside** the `T?`.
                let door = witness
                    .filter(|_| locked_content_of(&inner).is_some())
                    .map(|at| &config[at].value);
                if let (Some(at), None) = (witness, door) {
                    self.expr(&config[at].value, span);
                }
                let given: Vec<Expr>;
                let (args, written) = match door {
                    Some(seen) => {
                        self.the_witness_takes_no_reference(seen, span);
                        self.checked.witnessed_sets.insert(span.start);
                        given = args.iter().chain([seen]).cloned().collect();
                        (&given[..], "set(after)".to_string())
                    }
                    None => (&args[..], self.parsed.text(*method).to_string()),
                };
                // **The receiver is lent to the call where the call changes
                // nothing** ([ADR-189](../../docs/specification/adr/adr-189.md)
                // D2, [ADR-113](../../docs/specification/adr/adr-113.md) D1 and
                // D3). What comes out of a reached **method** is the call's own
                // result rather than a view of the receiver, so this half needs
                // no representation and is whole.
                //
                // Asked of the `mutates` column and only where **every**
                // candidate for the name agrees, which is the rule `NK1138`
                // uses one construct over: a name this compiler cannot resolve
                // is claimed nothing about, and the reach lowers exactly as it
                // did (Part III C.4).
                let candidates: Vec<_> = self
                    .own
                    .candidates(&name)
                    .into_iter()
                    .chain(self.library.candidates(&name))
                    .collect();
                let lent = !candidates.is_empty() && candidates.iter().all(|(_, c)| !c.mutates);
                if lent {
                    self.checked.lent_reaches.insert((span.start, name.clone()));
                }
                let reached = self.call_on(*inner, *method, args, &written, span);
                // **A view out of a temporary needs something to point into**
                // (ADR-228 D1): the receiver is held for the rest of the block.
                let a_view = match &reached {
                    Ty::Nullable(inner) => inner.is_a_view(),
                    other => other.is_a_view(),
                };
                if lent && a_view && !roots_in_a_binding(receiver) {
                    self.checked.held_reaches.insert((span.start, name.clone()));
                }
                match reached {
                    // The `and_then` case, recorded by name for the emitter
                    // exactly as a nullable field is (ADR-028: the emitter has
                    // no types and this is a question about one).
                    Ty::Nullable(result) => {
                        self.checked.flattened_reaches.insert((span.start, name));
                        Ty::Nullable(result)
                    }
                    Ty::Unknown => Ty::Unknown,
                    plain => Ty::Nullable(Box::new(plain)),
                }
            }

            Expr::Field { base, name } => {
                let on = self.expr(base, span);
                let field = self.parsed.text(*name).to_string();
                self.a_read_of_a_part(base, expr, &field, span);
                if let Ty::Nullable(_) = &on {
                    self.reaches_into_a_nullable(&on, Reached::Field(&field), span);
                    return Ty::Unknown;
                }
                let Ty::Named { name: ty, .. } = &on else {
                    return Ty::Unknown;
                };
                // **A boxed field is read through its box**
                // ([ADR-246](../../docs/specification/adr/adr-246.md) D2).
                if self
                    .checked
                    .boxed_members
                    .get(ty)
                    .is_some_and(|members| members.contains(&field))
                {
                    self.checked
                        .boxed_reads
                        .insert((span.start, argument_shape(expr)));
                }
                // **A handle has no fields** (ADR-147 D3): it is an address
                // this language never dereferences, so there is nothing inside
                // it to name. Before `fields_of`, which would answer `None` and
                // send the reader to `NK1126`'s *no bound* — a sentence about a
                // type parameter, which this is not.
                if self.opaque_handles.contains(ty) {
                    let ty = ty.clone();
                    self.a_handle_has_nothing_inside(&ty, &format!("the field `{field}`"), span);
                    return Ty::Unknown;
                }
                // **A reflected field answers two members and no others**
                // ([ADR-088](../../docs/specification/adr/adr-088.md) D2):
                // `.name` here, and `.of(value)` where a method is called.
                if ty == ty::VARIANT {
                    if field == "name" {
                        return Ty::view("str");
                    }
                    let held = field.clone();
                    self.a_reflected_variant_has_two_members(&held, span);
                    return Ty::Unknown;
                }
                if ty == ty::FIELD {
                    if field == "name" {
                        return Ty::Named {
                            name: "str".to_string(),
                            args: Vec::new(),
                            view: true,
                        };
                    }
                    let held = field.clone();
                    self.a_reflected_field_has_two_members(&held, span);
                    return Ty::Unknown;
                }
                let Some(fields) = self.fields_of(ty) else {
                    // `NK1126` where the type is a parameter: nothing will ever
                    // describe `T`, so a field on one is refused here rather
                    // than by `rustc` about the generated file.
                    let ty = ty.clone();
                    self.nothing_says_what_a_parameter_can_do(&ty, Reached::Field(&field), span);
                    return Ty::Unknown;
                };
                match fields.iter().find(|f| f.name == field) {
                    Some(found) => {
                        let (ty, declared) = (ty.clone(), found.clone());
                        self.field_is_reachable(&ty, &declared, span);
                        // `Pair[i64].first` is an `i64`: the receiver's own
                        // arguments bind the declaration's parameters, exactly
                        // as a method's receiver binds its signature's
                        // (ADR-031, and ADR-074 D2 for a `.nika` declaration).
                        ty::substitute(&declared.ty, &self.arguments_of(&on))
                    }
                    None => {
                        let ty = ty.clone();
                        self.no_such_field(&ty, &field, &fields, span);
                        Ty::Unknown
                    }
                }
            }

            // Part I 3.5: `x?.field`. The receiver must be a `T?` and the
            // result is a `U?` - flattened, because a field that is *itself*
            // nullable would otherwise give a nullable of a nullable.
            Expr::SafeField { base, name } => {
                let on = self.expr(base, span);
                let field = self.parsed.text(*name).to_string();
                let Ty::Nullable(inner) = &on else {
                    self.reaches_through_a_plain_value(&on, Reached::Field(&field), span);
                    return Ty::Unknown;
                };
                let Ty::Named { name: ty, .. } = inner.as_ref() else {
                    return Ty::Unknown;
                };
                // **A boxed field is read through its box here too**
                // ([ADR-246](../../docs/specification/adr/adr-246.md) D2).
                if self
                    .checked
                    .boxed_members
                    .get(ty)
                    .is_some_and(|members| members.contains(&field))
                {
                    self.checked
                        .boxed_reads
                        .insert((span.start, argument_shape(expr)));
                }
                let Some(fields) = self.fields_of(ty) else {
                    return Ty::Unknown;
                };
                match fields.iter().find(|f| f.name == field) {
                    Some(found) => {
                        let (ty, declared) = (ty.clone(), found.clone());
                        self.field_is_reachable(&ty, &declared, span);
                        // **The `and_then` case is the field that is already a
                        // `T?`**, and the emitter is told which by name: `map`
                        // over one would make an `Option<Option<T>>`, and that
                        // is a question about the declared type, which this
                        // module answers and the emitter cannot (ADR-028).
                        // **A view is taken of a place and never of a
                        // temporary** ([ADR-191](../../docs/specification/adr/adr-191.md)
                        // D1). Measured: the view of a temporary dies at the
                        // `;`, and binding it is `rustc`'s *temporary value
                        // dropped while borrowed* about a file nobody wrote
                        // (Part III C.1). A temporary has no next line to stay
                        // usable on, so leaving it owned keeps
                        // [ADR-113](../../docs/specification/adr/adr-113.md)
                        // D1's promise where it means anything.
                        let place = roots_in_a_binding(base);
                        match &declared.ty {
                            // A field that is itself a `T?` flattens, and a
                            // view of one is taken the same way one shape down.
                            Ty::Nullable(inner)
                                if place && crate::contracts::keeps::moves(inner) =>
                            {
                                self.checked
                                    .viewed_reaches
                                    .insert((span.start, field.clone()), viewed_as(inner));
                                self.checked.flattened_reaches.insert((span.start, field));
                                Ty::Nullable(Box::new(inner.as_a_view()))
                            }
                            Ty::Nullable(_) => {
                                self.checked.flattened_reaches.insert((span.start, field));
                                declared.ty
                            }
                            // **A member that copies comes out of a view**
                            // ([ADR-189](../../docs/specification/adr/adr-189.md)
                            // D1, [ADR-113](../../docs/specification/adr/adr-113.md)
                            // D1 and D2). A number, a `bool` and a `char` are
                            // read through the receiver and copied, so the
                            // reach leaves the receiver where it was and the
                            // result's type is what it always was.
                            //
                            // **The other half is not here**, and it is the
                            // representation rather than this walk: a member
                            // that does **not** copy comes out as a *view* of
                            // the receiver (D2) - which is not a **state**:
                            // three of the four shapes a `?.` has are Borrowed
                            // and the fourth is `NK2303`'s
                            // ([ADR-190](../../docs/specification/adr/adr-190.md)
                            // D1). What it waits on is one question about `??`,
                            // on `docs/open-decisions.md`. So that reach lowers
                            // exactly as it did, moving the receiver, and
                            // [ADR-052](../../docs/specification/adr/adr-052.md)
                            // D8's translation stays for it alone.
                            plain if !crate::contracts::keeps::moves(plain) => {
                                self.checked.copied_reaches.insert((span.start, field));
                                Ty::Nullable(Box::new(plain.clone()))
                            }
                            // **And a member that does not copy comes out as a
                            // view of the receiver**, which is
                            // [ADR-113](../../docs/specification/adr/adr-113.md)
                            // D2 and [ADR-191](../../docs/specification/adr/adr-191.md)
                            // D1. Borrowed and not Tethered: the view points
                            // into a place that outlives the statement, which
                            // is what [ADR-008](../../docs/specification/adr/adr-008.md)
                            // D2 calls the free case.
                            plain if place => {
                                let viewed = viewed_as(plain);
                                self.checked
                                    .viewed_reaches
                                    .insert((span.start, field.clone()), viewed);
                                self.checked.copied_reaches.insert((span.start, field));
                                Ty::Nullable(Box::new(plain.as_a_view()))
                            }
                            plain => Ty::Nullable(Box::new(plain.clone())),
                        }
                    }
                    None => {
                        let ty = ty.clone();
                        self.no_such_field(&ty, &field, &fields, span);
                        Ty::Unknown
                    }
                }
            }

            Expr::StructLit { name, fields } => {
                // `unaliased`, the same as a type: `h::Request(path: …)` builds
                // `http::Request` (ADR-046 D3).
                let name = self.parsed.unaliased(self.parsed.text(*name));
                let declared = self.fields_of(&name);
                // **A struct literal naming nothing this compiler declares**
                // ([ADR-096](../../docs/specification/adr/adr-096.md), `NK1135`),
                // and it is the silence that let
                // [ADR-133](../../docs/specification/adr/adr-133.md)'s collision
                // through: `Stats(min: first)` and `execute(target_age: 30)` are
                // one spelling, `ctor_lit` reads both as a literal, and a literal
                // for a struct nothing declares walked out of here on the
                // `continue` below and lowered verbatim. `rustc` answered
                // *cannot find struct `execute`* about a file nobody wrote, which
                // is Part III C.1's class and the very hole `NK1135` was built
                // to close for a written annotation.
                //
                // **Unqualified only**, which is `NK1135`'s own convention:
                // `pool::Conn(id: 1)` names a package's type, and whether this
                // build can see that package is a question with a message of its
                // own (ADR-046 D2).
                if declared.is_none() && !name.contains("::") && !self.declares_a_type(&name) {
                    self.a_struct_nothing_declares(&name, span);
                }
                // **What a generic struct's literal binds**
                // ([ADR-074](../../docs/specification/adr/adr-074.md) D2).
                // `Pair { first: 1, second: 2 }` is a `Pair[i64]` and nothing
                // else says so: the declaration writes `first: $T` and the
                // value is an `i64`, which is one `bind` per field.
                let mut bound: BTreeMap<String, Ty> = BTreeMap::new();
                // **A field named twice** is `NK1172`, here and in a `with` for
                // one reason ([ADR-118](../../docs/specification/adr/adr-118.md)
                // D1 restates the literal's rule): `Point { x: 1, x: 2 }` used
                // to lower, and `rustc` answered about the generated file.
                let mut seen: BTreeSet<String> = BTreeSet::new();
                for init in fields {
                    let field = self.parsed.text(init.name).to_string();
                    if !seen.insert(field.clone()) {
                        self.a_field_written_twice(&name, &field, span);
                    }
                    // **Code put into a function field is remembered with the
                    // field** (ADR-230 D1): what it calls is what a call
                    // through the field reaches, wherever that call is.
                    let code_field = declared.as_ref().is_some_and(|d| {
                        d.iter()
                            .any(|f| f.name == field && matches!(f.ty, Ty::Fn { .. }))
                    });
                    if code_field {
                        self.stored_frames.push(StoredCode::default());
                    }
                    // `Reading { name, temp }` is shorthand for `name: name`.
                    let found = match &init.value {
                        Some(value) => self.expr(value, span),
                        None => self.lookup(&field).unwrap_or(Ty::Unknown),
                    };
                    if code_field {
                        let mut stored = self.stored_frames.pop().unwrap_or_default();
                        match &init.value {
                            Some(Expr::Closure { body, .. }) => {
                                stored
                                    .free
                                    .extend(crate::contracts::locks::free_calls(self.parsed, body));
                            }
                            // A function of this program, named: it is what
                            // runs.
                            Some(Expr::Variable(named)) => {
                                let named = self.parsed.text(*named).to_string();
                                match self.lookup(&named).is_none() {
                                    true => {
                                        stored.free.insert(named);
                                    }
                                    false => stored.unresolved = true,
                                }
                            }
                            // Anything else - a value computed somewhere else -
                            // is code this walk did not see.
                            _ => stored.unresolved = true,
                        }
                        let entry = self
                            .checked
                            .stored_code
                            .entry(format!("{name}.{field}"))
                            .or_default();
                        entry.resolved.extend(stored.resolved);
                        entry.unresolved |= stored.unresolved;
                        entry.free.extend(stored.free);
                    }
                    let Some(declared) = &declared else { continue };
                    match declared.iter().find(|f| f.name == field) {
                        Some(found_field) => {
                            let want = found_field.ty.clone();
                            ty::bind(&want, &found, &mut bound);
                            // **A field holds what it is given** (ADR-213 D3),
                            // unless it is declared a view.
                            if !want.is_a_view() {
                                let to = format!("put into `{name}`'s `{field}`");
                                match &init.value {
                                    Some(value) => self.hands_over(value, &found, &to, span),
                                    None => self.hands_over_name(&field, &to, span),
                                }
                            }
                            let owner = name.clone();
                            self.field_is_reachable(&name, found_field, span);
                            // **No longer a constructor either**
                            // ([ADR-064](../../docs/specification/adr/adr-064.md)
                            // D2). It was the second of the two positions, and the
                            // two positions are what stopped being a list.
                            let _ = &owner;
                            // **A declared field is a use** (ADR-152 D4), and
                            // it is the position [ADR-127](../../docs/specification/adr/adr-127.md)
                            // §4's C value field will be written through.
                            let found = match init.value.as_ref() {
                                Some(value) => self
                                    .literal_by_use(&found, &want, value, span)
                                    .unwrap_or(found),
                                None => found,
                            };
                            // Part I 2.3's fourth position: a plain value in a
                            // field the struct declares nullable. The same rule
                            // as the other three (`wraps_into_nullable`), keyed
                            // by the field as well, because a struct literal
                            // has one of these per field and a statement only
                            // one span.
                            // **A run this body owns, in a field that views
                            // one** ([ADR-179](../../docs/specification/adr/adr-179.md)
                            // D2). `Vec[T]` *fits* `&[T]` — that is the C
                            // boundary's rule, where the call lends for its own
                            // duration ([ADR-147](../../docs/specification/adr/adr-147.md)
                            // D1) — and a **struct outlives the expression that
                            // fills it**, so the same fit here is a view of
                            // something already gone. Asked before `expect`,
                            // because `expect` is silent where the fit holds.
                            //
                            // **Inside a grammar action anything that is not
                            // already a view is refused**, and that is not
                            // caution: an action's bindings come from the
                            // parse, which **owns** the runs it built, and what
                            // this checker knows about one of them is nothing
                            // — so a `Section { name, settings }` shorthand
                            // would slip past a test on the value's type and
                            // reach `rustc` as *expected `&[Setting]`, found
                            // `Vec<Setting>`*, about a grammar line whose way
                            // out is not *write a `&`* at all.
                            //
                            // **And not inside a `comptime`**, which is the
                            // position this type was added for: there the run
                            // is **the build's**, the crossing writes the `&`
                            // and the array literal behind it is the program's
                            // own text ([ADR-079](../../docs/specification/adr/adr-079.md)
                            // D1). Refusing there would refuse the one line
                            // that is right, which is [Part III
                            // C.4](../../docs/specification/30-nikaia-tooling.md).
                            let owns_it = matches!(&found, Ty::Named { name, view: false, .. }
                                if name == "Vec" || name == "List")
                                || (self.inside_an_action.is_some() && !found.is_a_view());
                            if slice_element(&want).is_some() && owns_it && !self.inside_a_comptime
                            {
                                self.a_run_this_body_owns(&name, &field, &found.text(), span);
                                continue;
                            }
                            let value = init.value.as_ref();
                            let is_literal = value.is_some_and(is_literal);
                            let keeper = format!("`{owner}` keeps its `{field}` after this line");
                            if let Some(how) = wrap_for(&found, &want, is_literal) {
                                self.checked
                                    .nullable_fields
                                    .entry((span.start, owner.clone(), field.clone()))
                                    .or_default()
                                    .insert(value.map(argument_shape).unwrap_or_default(), how);
                                // A view into a `String?` is still a view kept
                                // (ADR-224 D1): asked as it would be without
                                // the `?`.
                                if views_into_nullable_text(&found, &want) {
                                    self.field_either = self.either_field(&owner, &field);
                                    let (o, f) = (owner.clone(), field.clone());
                                    self.expect_kept(
                                        &found,
                                        &want,
                                        value,
                                        &keeper,
                                        span.clone(),
                                        "field",
                                        move |found, want| {
                                            format!("`{o}.{f}` is `{want}`, and this is `{found}`")
                                        },
                                    );
                                }
                                continue;
                            }
                            self.field_either = self.either_field(&owner, &field);
                            self.expect_kept(
                                &found,
                                &want,
                                value,
                                &keeper,
                                span.clone(),
                                "field",
                                move |found, want| {
                                    format!("`{owner}.{field}` is `{want}`, and this is `{found}`")
                                },
                            );
                        }
                        None => self.no_such_field(&name, &field, declared, span),
                    }
                }
                // **A variant's literal is its `enum`** and not the variant:
                // `Shape` is a type a declaration can be written as and
                // `Shape::Spot` is not, so answering the latter refused a
                // correct program with a way out nobody can take (Part III
                // C.4). The map is built where the `enum` is read, so the
                // literal and the pattern agree by construction.
                if let Some(owner) = self.variant_owner.get(&name) {
                    return Ty::named(owner.clone());
                }
                match self.struct_parameters.get(&name) {
                    Some(order) => {
                        let args = order
                            .iter()
                            .map(|p| bound.get(p).cloned().unwrap_or(Ty::Unknown))
                            .collect();
                        Ty::Named {
                            name,
                            args,
                            view: false,
                        }
                    }
                    None => Ty::named(name),
                }
            }

            // **`value with { field: … }`**
            // ([ADR-118](../../docs/specification/adr/adr-118.md) D1): a copy of
            // a value with named fields changed, of the same type. Every rule
            // about *naming* a field is the literal's — the braces are the
            // literal's — so what is new here is the **operand**: it has to be
            // a struct this compiler can name, because what it lowers to is
            // Rust's `Point { x: 1, ..p }` and the type is written there.
            Expr::With { base, fields, at } => {
                let found = self.expr(base, span);
                let Ty::Named {
                    name,
                    view,
                    args: _,
                } = &found
                else {
                    self.a_with_over_something_else(&found.text(), Copyable::Unnamed, span);
                    return Ty::Unknown;
                };
                let name = self.parsed.unaliased(name);
                // **An enum is refused with a message naming `match`**
                // (ADR-118 §4): `m with { x: 1 }` cannot be typed without
                // knowing the variant, and inside a `match` arm it is known —
                // which is a decision that record deliberately left open.
                if self.enums.contains_key(&name) {
                    self.a_with_over_something_else(&name, Copyable::AnEnum, span);
                    return found;
                }
                // **A view is not something to move from** (D3): what `with`
                // does not name it takes from the operand *by move*, and no
                // copy is inserted that the program did not write (ADR-107 D3).
                if *view {
                    self.a_with_over_something_else(&name, Copyable::AView, span);
                    return Ty::named(name);
                }
                let Some(declared) = self.fields_of(&name) else {
                    self.a_with_over_something_else(&name, Copyable::NotAStruct, span);
                    return found;
                };
                // **A copy that changes nothing is the value** (D1).
                if fields.is_empty() {
                    self.a_with_that_changes_nothing(&name, span);
                }
                let mut seen: BTreeSet<String> = BTreeSet::new();
                for init in fields {
                    let field = self.parsed.text(init.name).to_string();
                    if !seen.insert(field.clone()) {
                        self.a_field_written_twice(&name, &field, span);
                    }
                    // `user with { name }` — the shorthand is the literal's.
                    let given = match &init.value {
                        Some(value) => self.expr(value, span),
                        None => self.lookup(&field).unwrap_or(Ty::Unknown),
                    };
                    // **Code put into a function field here is code the lock
                    // column does not read** (ADR-230 D1): the field is
                    // undecided rather than quiet.
                    if declared
                        .iter()
                        .any(|f| f.name == field && matches!(f.ty, Ty::Fn { .. }))
                    {
                        self.a_field_whose_code_is_not_seen(&name, &field);
                    }
                    match declared.iter().find(|f| f.name == field) {
                        Some(held) => {
                            // **D4: a field may be named only where a literal
                            // could name it.** The private field a copy merely
                            // *carries* is never named, so it never reaches
                            // this.
                            self.field_is_reachable(&name, held, span);
                            let want = held.ty.clone();
                            let owner = name.clone();
                            let field = field.clone();
                            self.expect(
                                &given,
                                &want,
                                span.clone(),
                                "field",
                                move |given, want| {
                                    format!("`{owner}.{field}` is `{want}`, and this is `{given}`")
                                },
                            );
                        }
                        None => self.no_such_field(&name, &field, &declared, span),
                    }
                }
                // **The type, for the emitter** (ADR-011 D2): Rust writes the
                // struct's name in a functional update and this node does not
                // carry one, so the answer travels under the byte the `with`
                // stands at rather than being worked out twice.
                self.checked.with_types.insert(*at, name.clone());
                found
            }

            // A lambda's arguments are the ones it names (ADR-049). There is
            // nothing to read off the body any more, so a `fn { … }` pushes an
            // empty frame - and a body reaching for `a` is then a body naming
            // something nothing declares, which `NK1117` refuses.
            Expr::Closure {
                params,
                mutable,
                body,
            } => {
                let frame: Vec<Local> = params
                    .iter()
                    .map(|p| self.lambda_parameter(*p, mutable, Ty::Unknown, span))
                    .collect();
                for local in &frame {
                    self.nameable(&local.name.clone(), span, "a lambda's argument");
                }
                self.repeats.push(Repeats::a_lambda(self.scope.len()));
                self.scope.push(frame);
                // Part I 3.3: a lambda is a closure below, and a jump does not
                // leave one.
                self.past_a_boundary("lambda", |me| me.block(body));
                self.scope.pop();
                self.repeats.pop();
                Ty::Unknown
            }

            Expr::Unary { op, expr } => {
                // `&s` looks at a sequence without taking it (ADR-212 D4). Only
                // a name directly under the `&`: `&f(s)` still hands `s` over.
                let looks = matches!(op, UnaryOp::Ref) && matches!(&**expr, Expr::Variable(_));
                let outer = std::mem::replace(&mut self.reading_only, looks);
                let inner = self.expr(expr, span);
                self.reading_only = outer;
                match op {
                    UnaryOp::Neg => inner,
                    // `!` is a `bool`'s, and the language below spells a
                    // bitwise complement the same way. Nikaia has no bitwise
                    // operator today, so this claims `bool` where the operand
                    // agrees and claims nothing where it does not - rather than
                    // insisting on `bool` and being wrong the day one arrives.
                    UnaryOp::Not => {
                        let boolean = Ty::named("bool");
                        if inner.fits(&boolean) {
                            boolean
                        } else {
                            Ty::Unknown
                        }
                    }
                    UnaryOp::Ref => view_of(&inner),
                }
            }

            Expr::Binary {
                op,
                lhs,
                rhs,
                span: at,
            } => {
                let left = self.expr(lhs, span);
                let right = self.expr(rhs, span);
                self.divisor_is_not_zero(*op, rhs, span);
                // **The stamp sticks**
                // ([ADR-111](../../docs/specification/adr/adr-111.md) D2):
                // `stand + 100` is a `Seen[i64]` and `stand > 100` a
                // `Seen[bool]`. An operator cannot put a value back into the
                // lock it came from, so what it makes still carries where it
                // came from — which is what lets `set` refuse it and every
                // other sink take it.
                let stamped = left.is_seen() || right.is_seen();
                let outcome = match op {
                    BinaryOp::And | BinaryOp::Or => {
                        self.expect_bool(&left, span, "`&&` and `||` join two `bool`s");
                        self.expect_bool(&right, span, "`&&` and `||` join two `bool`s");
                        Ty::named("bool")
                    }
                    BinaryOp::Eq | BinaryOp::Ne => {
                        self.a_type_that_does_not_compare(&left, &right, at);
                        self.a_view_compared_with_a_value(lhs, &left, rhs, &right, span);
                        Ty::named("bool")
                    }
                    BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                        self.a_view_compared_with_a_value(lhs, &left, rhs, &right, span);
                        Ty::named("bool")
                    }
                    // **A `+` where either side is text is a concatenation**
                    // ([ADR-081](../../docs/specification/adr/adr-081.md) D2),
                    // and it comes to a `String` whichever side was owned. This
                    // used to be the `_ => Ty::Unknown` below, with a comment
                    // saying that guessing which side names the result was the
                    // one guess this checker does not make. It was not a guess
                    // that was missing but a **decision**: a concatenation makes
                    // a new value, so `String` is the only thing it can be, and
                    // saying `&str` - which is what two borrowed sides used to
                    // come to - made `-> String` a false refusal (`NK1104`).
                    //
                    // Recorded by the operator's own span, because `a + b + c`
                    // is two of them and the statement they stand in is one.
                    BinaryOp::Add if is_text(&left) || is_text(&right) => {
                        self.checked.concatenations.insert(at.start);
                        Ty::named("String")
                    }
                    // **`NK1191`: arithmetic on a collection** (0.0.234).
                    // `[1] + [2]` lowered as it was written and the language
                    // below said *cannot add `Vec<i64>` to `Vec<i64>`* about a
                    // file nobody wrote. Part I gives a list no operator, and
                    // whether `+` should join two is the owner's question.
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Rem
                        if is_a_collection(&left) || is_a_collection(&right) =>
                    {
                        self.arithmetic_on_a_collection(*op, &left, &right, span);
                        Ty::Unknown
                    }
                    // Arithmetic on two of the same thing is that thing, and
                    // a bare number is neither - so one known side decides. Two
                    // known sides that disagree still decide nothing, now that
                    // the one case anybody met is taken above.
                    //
                    // **And a view of a number is the number here**
                    // ([ADR-233](../../docs/specification/adr/adr-233.md) D4):
                    // `0 - y` over a `ref i64` makes an `i64`, which is what
                    // the language below's operators make of a `&i64` too.
                    _ => match (
                        left.is_unknown(),
                        right.is_unknown(),
                        value_of_a_copy(left),
                        value_of_a_copy(right),
                    ) {
                        (true, _, _, right) => right,
                        (_, true, left, _) => left,
                        (_, _, left, right) if left == right => left,
                        _ => Ty::Unknown,
                    },
                };
                match stamped {
                    true => Ty::seen(outcome.unseen()),
                    false => outcome,
                }
            }

            Expr::Cast { expr, ty } => {
                let from = self.expr(expr, span);
                let into = self.declared(ty, span);
                self.a_cast_over_a_lent_binding(expr, span);
                if let Ty::Named { name, .. } = &into
                    && !OFFERED.contains(&name.as_str())
                {
                    self.cast_names_a_foreign_type(name, span);
                }
                self.record_cast(&from, &into, span);
                into
            }

            Expr::Tuple(parts) => {
                let found: Vec<Ty> = parts.iter().map(|p| self.expr(p, span)).collect();
                for (part, ty) in parts.iter().zip(&found) {
                    self.hands_over(part, ty, "put into a tuple", span);
                }
                Ty::Tuple(found)
            }

            // **`[1, 2, 3]` is a `Vec[T]`**
            // ([ADR-135](../../docs/specification/adr/adr-135.md) D1), and `T`
            // is what the elements agree on.
            Expr::ListLit { items, .. } => self.list_literal(items, span),

            // A `?` unwraps a failure, a `??` unwraps an absence, an index
            // reaches into a container and a range is an iterator: four things
            // Stage 0 has no signature for.
            Expr::Try(inner) => {
                self.expr(inner, span);
                Ty::Unknown
            }
            // Kap 7.1: `throw` leaves the function, so it has no value of its
            // own - the same shape a `return` has. What it throws is walked,
            // because a mistyped constructor inside it is still a mistake.
            Expr::Throw(inner) => {
                let thrown = self.expr(inner, span);
                self.a_thrown_value_that_is_not_an_error(inner, &thrown, span);
                Ty::Unknown
            }

            // **`return`, `break` and `continue` where an expression stands**
            // ([ADR-138](../../docs/specification/adr/adr-138.md) D1). Nothing
            // about what they do changes (D2), so each asks exactly what its
            // statement form asks — the answer to *what is a `return` worth* is
            // the same wherever it is written.
            Expr::Return(value) => {
                self.returns(value.as_deref(), span);
                Ty::Unknown
            }
            Expr::Break | Expr::Continue => {
                if self.loops == 0 {
                    let word = match expr {
                        Expr::Break => "break",
                        _ => "continue",
                    };
                    self.a_jump_with_nowhere_to_go(word, span);
                }
                Ty::Unknown
            }
            Expr::Coalesce { value, fallback } => {
                let outer = std::mem::replace(&mut self.read_a_map, false);
                let left = self.expr(value, span);
                let from_a_map = std::mem::replace(&mut self.read_a_map, outer)
                    && matches!(&**value, Expr::Index { .. });
                let other = self.expr(fallback, span);
                if let (Ty::Nullable(inner), Expr::LitStr { at, .. }) = (&left, &**fallback) {
                    if **inner == Ty::view("str") {
                        self.checked.view_fallbacks.insert(self.text_at(*at));
                    }
                    // **A map's text, read, is a view of it** (ADR-213 D2): the
                    // map hands out its `String` and keeps it, so the literal
                    // after `??` is the other view the answer can be, and the
                    // whole is a view of text - `m[k] ?? "-"`. It used to be
                    // built into a `String` below and handed to a place that
                    // wanted the map's reference: `rustc`'s words, about a file
                    // nobody wrote.
                    if **inner == Ty::named("String") && from_a_map {
                        self.checked.view_fallbacks.insert(self.text_at(*at));
                        return Ty::view("str");
                    }
                }
                if let Ty::Nullable(inner) = &left {
                    self.a_fallback_that_owns_what_the_left_side_views(
                        inner, &other, fallback, span,
                    );
                }
                // **`a ?? b` on a `T?` is a `T`** (Part I 3.5): that is what
                // ending the chain means, and claiming nothing about it cost
                // everything downstream — the day a map read became a `T?`
                // ([ADR-114](../../docs/specification/adr/adr-114.md) D1),
                // `let s = m[k] ?? panic(…)` made `s` unknown and `s.mean()`
                // one more method call nobody could answer.
                //
                // **Only that shape.** A `??` over something this checker could
                // not type claims nothing, which is
                // [Part III C.4](../../../docs/specification/30-nikaia-tooling.md):
                // the answer is more information than before and no new claim.
                match left {
                    Ty::Nullable(inner) => *inner,
                    _ => Ty::Unknown,
                }
            }
            // Indexing a container yields what the container holds - but only
            // where the container's type says so.
            //
            // This claimed nothing at all until ADR-028, and the cost was not
            // the missing type but everything downstream of it: in
            // `n-body.nika`, `let b = &self.bodies[i]` made `b` unknown, so
            // `b.x` was unknown, so `dx * dx + dy * dy` was unknown, so
            // `.sqrt()` could not be resolved and `energy` could not be shown
            // to be pure computation. One `Unknown` at the bottom of an
            // expression erases everything built on it.
            //
            // A shape it does not recognise still claims nothing: `s[i]` over
            // text is a slice in some languages and a byte in others, and
            // Nikaia has not said. `?` is the absence of a claim (ADR-024 D1).
            Expr::Index { base, index } => {
                let writing = std::mem::replace(&mut self.writing_index, false);
                let on = self.expr(base, span);
                let key = self.expr(index, span);
                // **An index of a mapping is a page fault**
                // ([ADR-169](../../docs/specification/adr/adr-169.md) D2).
                self.io_inside_a_door(&on, "this index", span);
                let Ty::Named { name, args, .. } = &on else {
                    return Ty::Unknown;
                };
                // **And no indexing** (ADR-147 D3), for the same reason: a
                // handle is one address and not a run of anything.
                if self.opaque_handles.contains(name) {
                    let name = name.clone();
                    self.a_handle_has_nothing_inside(&name, "an index", span);
                    return Ty::Unknown;
                }
                // The **last segment**, because a `std` type carries its module
                // since [ADR-154](../../docs/specification/adr/adr-154.md) D3 and
                // what is indexed is the type rather than where it is reached
                // from.
                // **A range in the brackets is a slice** (ADR-215 D3): a run of
                // the list, `ref Array[T]` - the type a function writes to take
                // one - or a view of the text. Written there, or kept in a name.
                let a_range = matches!(&**index, Expr::Range { .. })
                    || matches!(&key, Ty::Seq { shape, .. } if shape.replays);
                if a_range && !matches!(&**index, Expr::Range { .. }) {
                    self.checked
                        .slice_indices
                        .insert((span.start, argument_shape(index)));
                }
                if a_range {
                    return match (crate::contracts::ty::base(name), args.as_slice()) {
                        ("Vec" | "List" | "Array", [item, ..]) => Ty::Named {
                            name: ty::ARRAY.to_string(),
                            args: vec![item.clone()],
                            view: true,
                        },
                        ("String" | "str", []) => Ty::view("str"),
                        _ => Ty::Unknown,
                    };
                }
                match (crate::contracts::ty::base(name), args.as_slice()) {
                    // **A sequence keeps its `T` and its abort**
                    // ([ADR-114](../../docs/specification/adr/adr-114.md) D3):
                    // it has a `T` at every index it has at all, and an index
                    // it does not have is the program's own arithmetic gone
                    // wrong ([Part III A.2](../../../docs/specification/30-nikaia-tooling.md)).
                    ("Vec" | "List" | "Array", [item, ..]) => item.clone(),
                    // **A map answers a `T?`** (D1). It has a value only where
                    // the key is, and *there is nothing there* is data about
                    // the world rather than a bug in the program — so the
                    // bracket says what `get` says, and `??` is how a program
                    // that knows better says so.
                    ("HashMap" | "Map" | "BTreeMap", [keys, value]) => {
                        let (keys, value) = (keys.clone(), value.clone());
                        self.read_a_map = !writing;
                        self.a_map_key(&keys, &key, index, writing, span);
                        Ty::Nullable(Box::new(value))
                    }
                    _ => Ty::Unknown,
                }
            }
            // **A range is a sequence that replays**
            // ([ADR-212](../../docs/specification/adr/adr-212.md) D3): its
            // elements are produced as they are asked for, it can be walked
            // from either end, its length is known, and walking it walks a copy
            // - it is two numbers, and a value. Before, it had no type at all,
            // so `let r = 0..<3` and a `for` over `r` went to `rustc` and came
            // back as *no method named `iter`*, about a file nobody wrote.
            //
            // The item is whichever end says one; a literal says none
            // (Part I 2.4), so `0..<3` is a range of `?` and `0..<n` one of
            // what `n` is.
            Expr::Range { start, end, .. } => {
                let from = self.expr(start, span);
                let to = self.expr(end, span);
                let item = match from.is_unknown() {
                    true => to,
                    false => from,
                };
                Ty::Seq {
                    item: Box::new(item),
                    is_sync: true,
                    pauses: false,
                    throws: false,
                    parallel: false,
                    shape: ty::Shape {
                        ends: true,
                        sized: true,
                        replays: true,
                    },
                }
            }

            Expr::TryCatch { expr, handler } => {
                // Kap 7.1: the handler is what handles the failure, so the
                // guarded expression is where a fallible call needs no
                // `throws` on the function around it (`NK2605`). The handler
                // itself is ordinary code again - a failure raised inside one
                // leaves the function like any other.
                let outer = std::mem::replace(&mut self.caught, true);
                // **A guard of its own, and the enclosing one set aside**
                // ([ADR-091](../../../docs/specification/adr/adr-091.md)). A
                // `catch` inside another one's guarded expression handles its
                // own failures, so those are not the outer one's to count -
                // `(a.parse() catch { 1 }) catch { 2 }` has nothing left for the
                // second. The outer is told *no answer* rather than *nothing
                // fails*, because what a handler's own failure does is
                // [ADR-034](../../../docs/specification/adr/adr-034.md)'s
                // question and this refusal does not need it answered.
                let enclosing = self.guarded.replace(Guarded::default());
                let answers = self.expr(expr, span);
                let guarded = std::mem::replace(&mut self.guarded, enclosing);
                if self.guarded.is_some() {
                    self.guard_has_no_answer();
                }
                self.caught = outer;
                self.nothing_here_can_fail(guarded.unwrap_or_default(), span);
                self.scope
                    .push(vec![Local::free("error".to_string(), Ty::Unknown)]);
                let arriving = self.several_arrive(expr);
                let one = self.the_one_error(expr);
                let several = std::mem::replace(&mut self.caught_several, arriving);
                let single = std::mem::replace(&mut self.caught_one, one);
                let from = self.taken_so_far();
                let handled = self.block(handler);
                // **A handler's value stands where the guarded one would**, so
                // a text literal it ends in is built into text of its own where
                // the guarded value is (ADR-216 D4, one more of ADR-207 D1's
                // positions): `read() catch { "" }`.
                let literal =
                    tail_of(handler).and_then(|tail| self.text_literal(&answers, tail, true));
                self.a_branch_that_leaves(from, block_exits(handler), span);
                self.caught_several = several;
                self.caught_one = single;
                self.scope.pop();
                // **And so the `catch` is the guarded value's type**
                // (0.0.230, `open-work.md` §1.18 before it): where the handler leaves,
                // or hands back that type, or something this checker cannot
                // name. It used to be `?` always, so `let report = Log::file(d)
                // catch { return }` typed nothing that followed - a field read
                // on a lookup in `report.paths` reached `rustc` instead of
                // `NK1125`. A handler whose value is known to be something
                // else keeps the old answer, which claims nothing.
                let handled = literal.unwrap_or(handled);
                match block_leaves(handler) || handled.is_unknown() || handled.fits(&answers) {
                    true => answers,
                    false => Ty::Unknown,
                }
            }

            Expr::Spawn { body, .. } => {
                // **Before** the body is walked, so that the task's own `let`s
                // are not yet in scope: a name bound inside the task is the
                // task's own and crosses nothing.
                self.crosses_into_a_task(body, span);

                // **A `spawn` hands back a `TaskHandle` of what the task's body
                // comes to** (Part I 8.2, ADR-055 D5), and that is why the body
                // is walked here rather than through the generic closure arm:
                // what a lambda hands back is not written down anywhere
                // (ADR-029 D1), but a task's body is a *block*, and a block's
                // value is a thing this checker already knows. So `spawn fn {
                // work(21) }` is a `TaskHandle[i64]` and `handle.join()` is an
                // `i64`, with nothing inferred that a signature did not say.
                let Expr::Closure { params, body, .. } = body.as_ref() else {
                    self.expr(body, span);
                    return Ty::Unknown;
                };
                // A task is handed nothing, so a parameter has nothing to be
                // bound from. Refused rather than dropped: `spawn fn (x) { … }`
                // reads as though `x` arrives from somewhere.
                self.a_task_takes_no_arguments(params, span);
                // **What the task takes with it** (`NK2101`), read off the body
                // before it is walked: a name the body binds for itself is the
                // task's own, and the frame pushed below is what keeps the two
                // apart.
                self.a_task_takes_these(
                    &Expr::Closure {
                        params: params.clone(),
                        // A task is handed nothing, so no parameter of one is
                        // ever `mut` - `a_task_takes_no_arguments` above has
                        // already refused the list that is not empty.
                        mutable: Vec::new(),
                        body: body.clone(),
                    },
                    span,
                );
                self.scope.push(Vec::new());
                self.task_bindings.push(Vec::new());
                // **A task started with `spawn` runs later and elsewhere**
                // ([ADR-039](../../docs/specification/adr/adr-039.md) D3), so
                // taking a lock inside one is the ordinary case and not this
                // door's reach. `contracts::locks` makes the same split when it
                // builds the column; this is it at the refusal.
                //
                // A **scope**'s tasks are the other way round and are not this:
                // a scope waits for them, so they run during the call.
                let outer_inside = std::mem::replace(&mut self.inside_a_door, false);
                // A task's body is an `async` block below (ADR-055 §6), which
                // is a function too: `break` may not leave it either.
                let value = self.past_a_boundary("task", |me| me.block(body));
                self.scope.pop();
                self.inside_a_door = outer_inside;
                let bound = self.task_bindings.pop().unwrap_or_default();
                // **After** the walk, because the question needs both halves of
                // what the walk found: the types of what the body bound, and
                // which of its method calls pause (`pausing_methods`).
                self.a_task_holds_these_across_a_pause(body, &bound, span);
                Ty::Named {
                    name: "TaskHandle".to_string(),
                    args: vec![value],
                    view: false,
                }
            }

            // A template's holes are Nikaia too (ADR-017), and what the
            // template *produces* is still the emitter's business.
            Expr::Dsl { .. } => {
                // Which is also why `NK1134` says nothing about one: what this
                // becomes is decided after the checker has run, so whether it
                // can fail is not this walk's to answer.
                self.guard_has_no_answer();
                self.holes(expr, span);
                Ty::Unknown
            }

            // A grammar and an `asm` block: what these produce is the business
            // of the emitter that compiles them.
            Expr::Asm { .. } => {
                self.guard_has_no_answer();
                Ty::Unknown
            }
        }
    }

    /// **`NK1117`: a `std` name written without its module**
    /// ([ADR-154](../../docs/specification/adr/adr-154.md) D1).
    ///
    /// The list on Part I's first page is what needs no `use`, and every entry
    /// on it is keyed **bare** in the ledger — so a name nothing here declares,
    /// which `std` has exactly one module entry for, is a name written without
    /// its prefix. The message names the line to add, because a refusal whose
    /// way out is one line should hand that line over (Part III, C.2).
    ///
    /// **Only where nothing else could have declared it**, which is the same
    /// list `nothing_declares_it` keeps and for the same reason: refusing a
    /// correct program is the one thing this checker may never do
    /// (Part III, C.4). A name a local, a parameter, this unit's own ledger, a
    /// type declared here, a module or a grammar answers never reaches here.
    fn a_std_name_without_its_module(&mut self, name: &str, span: &Span) {
        if name.contains("::") {
            return;
        }
        let known = self.binding(name).is_some()
            || self.resolve(name).is_some()
            || self.structs.contains_key(name)
            || self.enums.contains_key(name)
            || self.own.types.contains_key(name)
            || self.modules.contains(name)
            || self.grammars.contains_key(name)
            || MultiLock::named(name).is_some()
            || is_hull(name);
        if known {
            return;
        }
        let suffix = format!("::{name}");
        let keys: Vec<&String> = self
            .library
            .functions
            .keys()
            .filter(|key| key.ends_with(&suffix))
            .collect();
        // **One entry and not several.** Where two modules have the name, the
        // compiler does not know which was meant and saying so would be a
        // guess; `NK1117` still says the name is undeclared, which is true.
        let [key] = keys.as_slice() else {
            return;
        };
        let Some((module, _)) = key.split_once("::") else {
            return;
        };
        if !self.std_modules.contains(module) {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1117",
            message: format!("nothing declares `{name}`"),
            notes: vec![
                format!("`std` has `{key}`, and a name that lives in a module is reached through it (Part I, 1.3)"),
                "what needs no `use` is the list on Part I's first page, and it is small on purpose: every name in it is one some program writes without importing, and removing one breaks that program (ADR-154 D1, D4)"
                    .to_string(),
            ],
            help: Some(format!(
                "write `use std::{module}` at the top of the file, and `{key}(…)` here"
            )),
        });
    }

    /// **`NK1117`: a module used before it is introduced**
    /// ([ADR-154](../../docs/specification/adr/adr-154.md), Part I 9.1's D4 for
    /// `std` rather than for a package).
    ///
    /// `fs::read_to_string(p, fs::Root::Anywhere)` without `use std::fs` at the top. A file lists
    /// what it depends on, and a reader should not have to know which module a
    /// prefix belongs to in order to find out.
    ///
    /// **Only a module `std`'s ledger has**, so a package's prefix and a name
    /// this compiler cannot see are both left alone (Part III, C.4).
    fn a_module_used_before_it_is_introduced(&mut self, name: &str, span: &Span) {
        let Some((module, _)) = name.split_once("::") else {
            return;
        };
        if !self.std_modules.contains(module) || self.std_in_scope.contains(module) {
            return;
        }
        // A module of **this program** wins: a package called `text` beside a
        // `std::text` is the consumer's to name apart, and until they collide
        // the local one is what the prefix means.
        if self.modules.contains(module) {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1117",
            message: format!("`{module}` is used here and introduced nowhere"),
            notes: vec![
                "a file lists what it depends on at the top, and `std` is reached the way a package is (Part I, 9.1, ADR-140 D5)"
                    .to_string(),
            ],
            help: Some(format!("write `use std::{module}` at the top of the file")),
        });
    }

    /// **`NK1163`: a name that needs no `use`, written with a module in front
    /// of it** ([ADR-167](../../docs/specification/adr/adr-167.md) D2).
    ///
    /// [ADR-154](../../docs/specification/adr/adr-154.md) §5 enforced the list
    /// in one direction — a name that lives in a module, written without it —
    /// and `NK1117` above is that half. This is the same rule read the other
    /// way: `io::println("x")` is a spelling a reader reaches for because every
    /// *other* `std` name wants its module, and what it got was `rustc` saying
    /// *cannot find function `println` in module `io`* about a file nobody
    /// wrote ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **It cannot refuse a correct program**, which is the test
    /// [C.4](../../docs/specification/30-nikaia-tooling.md) sets: the program
    /// this refuses does not compile today under any reading, because the
    /// module genuinely has no such name. That is what separates it from
    /// `use std::db::postgres` — a module nothing describes **yet**, which
    /// [ADR-140](../../docs/specification/adr/adr-140.md) D5 leaves alone for
    /// exactly this reason.
    fn a_prelude_name_with_a_module_in_front(&mut self, name: &str, span: &Span) {
        let Some((module, last)) = name.split_once("::") else {
            return;
        };
        // A module of **this program** wins, as it does for `NK1117`.
        if !self.std_modules.contains(module) || self.modules.contains(module) {
            return;
        }
        // The module genuinely has no such name, and the **bare** one is a name
        // `std` keys. Both halves, or this is a guess.
        if self.library.functions.contains_key(name) || !self.library.functions.contains_key(last) {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1163",
            message: format!("`{module}` has no `{last}`, and `{last}` needs no module"),
            notes: vec![
                format!(
                    "`{last}` is on the list of names that need no `use` (Part I, 1.3), so it is \
                     written on its own wherever it is needed"
                ),
                "every other `std` name is reached through its module, which is what makes \
                 this worth saying rather than guessing at (ADR-154 D1)"
                    .to_string(),
            ],
            help: Some(format!("write `{last}(…)` without the `{module}::`")),
        });
    }

    /// **`a`, `b` and `c` were a lambda's arguments, and are not** (`NK1117`).
    ///
    /// [ADR-049](../../../../docs/specification/adr/adr-049.md) D1 withdrew the
    /// three automatic names. Without this, `xs.map fn { a.id }` - the idiom that
    /// existed until that record - lowers to `|| { a.id }` and `rustc` says
    /// *cannot find value `a`* about a file nobody wrote, which is the one thing
    /// Part III C.1 forbids. A form that was in the specification deserves a
    /// sentence, which is the same ground [ADR-022](../../../../docs/specification/adr/adr-022.md)
    /// stands on for `fn: …`.
    ///
    /// **This is not the mechanism that was removed.** That one read a lambda's
    /// *arity* off which of the three its body mentioned, and every lambda in the
    /// language depended on it. This reads nothing: it is a message at the place a
    /// name was used and nothing declares it, and a lambda that declares `a` for
    /// itself never reaches it.
    ///
    /// Keyed on the three names, and **only** on them. A general "this expression
    /// names something nothing declares" is a much wider claim than the statement
    /// rule above makes, and this checker does not make it yet
    /// (`docs/open-work.md`).
    /// Hands back whether it said anything, so the general refusal can stand
    /// aside for it.
    fn withdrawn_automatic_name(&mut self, name: &str, span: &Span) -> bool {
        if !matches!(name, "a" | "b" | "c") {
            return false;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1117",
            message: format!("nothing declares `{name}`"),
            notes: vec![
                "`a`, `b` and `c` used to be a lambda's arguments without being written \
                 down, and that form is withdrawn (Part I, 5.3)"
                    .to_string(),
            ],
            help: Some(format!(
                "name the argument: `fn ({name}) {{ … }}`, or give it a name that says what \
                 it is - `fn (user) {{ user.id }}`"
            )),
        });
        true
    }

    /// **A `??` whose left side is a view and whose fallback owns** (`NK1185`,
    /// [ADR-191](../../docs/specification/adr/adr-191.md) D2).
    ///
    /// `user?.name` is a view of `user` where `name` does not copy
    /// ([ADR-113](../../docs/specification/adr/adr-113.md) D2), and
    /// `?? "nobody".to_owned()` asks the operator to hand back one value that is
    /// both. There are only three ways to do that and the language has ruled
    /// two of them out:
    ///
    /// * an **owned** result copies the borrowed branch, and a
    ///   compiler-inserted copy is what
    ///   [ADR-008](../../docs/specification/adr/adr-008.md) D5 bans outright;
    /// * a **view** result needs the fallback to be one — which `"nobody"`
    ///   already is and `"nobody".to_owned()` is not;
    /// * so the third is to say so here, in this language's words, rather than
    ///   let `rustc` say *expected `String`, found `&String`* about a file
    ///   nobody wrote ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **Both sides have to be known**, which is
    /// [C.4](../../docs/specification/30-nikaia-tooling.md): a `??` over
    /// something this checker could not type claims nothing, exactly as the
    /// result type does one line down.
    fn a_fallback_that_owns_what_the_left_side_views(
        &mut self,
        viewed: &Ty,
        fallback: &Ty,
        written: &Expr,
        span: &Span,
    ) {
        if !viewed.is_a_view() || fallback.is_a_view() {
            return;
        }
        // **`.to_owned()` and `.to_string()` by name**, which no ledger
        // describes and which this compiler already reads this way one file
        // over (`contracts::tether::makes_a_buffer`): *all four entries of
        // either name hand back owned text*. Without it the one spelling the
        // whole question is about - `?? "nobody".to_owned()` - types as `?` and
        // walks past the refusal into `rustc`'s *expected `String`, found
        // `&str`* about a file nobody wrote (Part III C.1).
        let names_a_copy = matches!(
            written,
            Expr::MethodCall { method, .. }
                if matches!(self.parsed.text(*method), "clone" | "to_owned" | "to_string")
        );
        let owned = match names_a_copy {
            true => Ty::named(crate::contracts::ty::TEXT),
            false => fallback.clone(),
        };
        if !matches!(owned, Ty::Named { .. }) || !crate::contracts::keeps::moves(&owned) {
            return;
        }
        let fallback = &owned;
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1185",
            message: format!(
                "this reach is a `{viewed}`, and the fallback beside it is a `{fallback}`"
            ),
            notes: vec![
                "a view and a value it points into are two types (Part I, 6.6), and `??` hands \
                 back one of them"
                    .to_string(),
                "a copy is written by the program and never inserted by the compiler \
                 ([ADR-008](docs/specification/adr/adr-008.md) D5), so there is nothing here \
                 that could make the two agree"
                    .to_string(),
            ],
            help: Some(format!(
                "give the fallback as a view too - a text literal already is one, so \
                 `?? \"…\"` reads the same and costs nothing - or take the receiver's member \
                 by a name of its own first, where a `{fallback}` is what is wanted"
            )),
        });
    }

    /// **An escape this language's set does not name** (`NK1184`,
    /// [ADR-188](../../docs/specification/adr/adr-188.md) D3).
    ///
    /// A `.nika` literal is written into the generated file verbatim, so what a
    /// `\` means is the language below's answer and always has been. What a
    /// program that writes `"a\qb"` *heard* about it was `rustc`'s, ending
    /// *for more information, visit doc.rust-lang.org/reference/tokens.html*:
    /// a Nikaia program sent to the Rust reference to find out what it may
    /// write, which is [Part III C.2](../../docs/specification/30-nikaia-tooling.md)'s
    /// *in the compiler's own words* not met and
    /// [C.1](../../docs/specification/30-nikaia-tooling.md)'s class besides.
    ///
    /// **It refuses nothing that was accepted**, and that is what
    /// [`build_time::an_escape_nothing_names`] sharing one table with
    /// [`build_time::decoded`] is for: everything this refuses is something
    /// `rustc` refuses one file later, so the only thing that moves is who says
    /// it and in whose vocabulary ([C.4](../../docs/specification/30-nikaia-tooling.md)).
    fn an_escape_nothing_names(&mut self, text: &str, span: &Span) {
        let Some(refused) = crate::build_time::an_escape_nothing_names(text) else {
            return;
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1184",
            message: format!("`{}` is not an escape this language has", refused.written),
            notes: vec![
                format!("{} (Part I, 2.5)", refused.why),
                format!("the set is: {}", crate::build_time::ESCAPES),
            ],
            // **Two ways out, and which one is offered is read off the
            // escape** ([Part III C.2](../../docs/specification/30-nikaia-tooling.md):
            // a way out that cannot be taken is not one). A `\\x` or a `\\u`
            // that is malformed was *meant* as a character, so the help names
            // the form it should have had; anything else is a backslash that
            // was meant literally, and the way out is to double it.
            help: Some(match refused.written.chars().nth(1) {
                Some('x') | Some('u') => {
                    "a character above `\\x7F` is written `\\u{…}`, with up to \
                     six hexadecimal digits: `\\u{80}`, `\\u{1F600}`"
                        .to_string()
                }
                _ => format!(
                    "write the backslash as `\\\\` where it is meant literally - `\"{}\"` - \
                     or use one of the escapes above",
                    text.replace('\\', "\\\\")
                ),
            }),
        });
    }

    /// **The one thing this checker warns about rather than refusing** - a
    /// plain string that was written when every string was a template
    /// (ADR-035 D5).
    ///
    /// Two shapes, because the change has two halves. `"hello {name}"` used to
    /// interpolate and is now text, and `"{{}}"` used to *be* `{}` and is now
    /// four characters. Both change what a program prints without changing
    /// whether it compiles, and Part III C.1 calls a silent change of meaning a
    /// bug in this compiler.
    ///
    /// **Why a warning and not an error.** `"{ margin: 0 }"` is correct CSS and
    /// `"\\d{3}"` a correct regular expression; refusing either would break the
    /// property the whole checker rests on - that it never rejects a program
    /// that is right. So the test is deliberately narrow: the braces have to
    /// hold something that **parses as an expression and resolves to something
    /// that is actually here** - a variable in scope, or a function a ledger
    /// knows. A name nobody declared is text, and text says nothing.
    fn unmarked_hole(&mut self, text: &str, span: &Span) {
        if text.contains("{{") || text.contains("}}") {
            self.warn_migration(
                span,
                "a doubled brace in a plain string is now two braces".to_string(),
                format!(
                    "`{{{{` escaped a brace while every string was a template. A plain string needs no escape: write `\"{}\"`",
                    text.replace("{{", "{").replace("}}", "}")
                ),
            );
            return;
        }

        for hole in brace_groups(text) {
            let Ok(parsed) = crate::parser::parse_expression(&self.parsed.interner, &hole) else {
                continue;
            };
            if !self.names_something_here(&parsed) {
                continue;
            }
            self.warn_migration(
                span,
                format!("`{{{hole}}}` here is text, and used to be a hole"),
                format!("write `f\"{text}\"` if the value was meant to appear (Part I, 2.5)"),
            );
            return;
        }
    }

    fn warn_migration(&mut self, span: &Span, message: String, help: String) {
        self.checked.findings.push(Finding {
            severity: Severity::Warning,
            span: span.clone(),
            code: "NK1111",
            message,
            notes: vec![
                "every string interpolated before ADR-035; now only `f\"…\"` does".to_string(),
            ],
            help: Some(help),
        });
    }

    /// Whether an expression names anything that exists here - a variable in
    /// scope or a function some ledger has. This is what keeps the warning off
    /// a stylesheet: `margin` is nobody's variable.
    /// The name an expression writes, where it writes one.
    ///
    /// For `NK1115`'s help line, which has to say `let db: Shared[…] = …` with
    /// the caller's own name in it. A value that is not a name has no such line
    /// to be pointed at, and the message says so instead of inventing one.
    fn names_of(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Variable(name) => Some(self.parsed.text(*name).to_string()),
            _ => None,
        }
    }

    fn names_something_here(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Variable(name) => {
                let name = self.parsed.text(*name);
                self.lookup(name).is_some() || self.resolve(name).is_some()
            }
            Expr::Field { base, .. } => self.names_something_here(base),
            Expr::MethodCall { receiver, .. } | Expr::SafeMethod { receiver, .. } => {
                self.names_something_here(receiver)
            }
            Expr::Call { func, args, .. } => {
                self.names_something_here(func) || args.iter().any(|a| self.names_something_here(a))
            }
            Expr::Path(segments) => {
                let name = segments
                    .iter()
                    .map(|s| self.parsed.text(*s))
                    .collect::<Vec<_>>()
                    .join("::");
                self.resolve(&name).is_some()
            }
            Expr::Binary { lhs, rhs, .. } => {
                self.names_something_here(lhs) || self.names_something_here(rhs)
            }
            Expr::Unary { expr, .. } => self.names_something_here(expr),
            _ => false,
        }
    }

    /// Every expression a literal hides, walked where it stands.
    ///
    /// The result is discarded: what a hole evaluates to is the emitter's
    /// business - `Render` decides for a template and `Display` for a string -
    /// and what the checker is here for is everything *inside* it.
    fn holes(&mut self, literal: &Expr, span: &Span) {
        // **A template's `<for>` declares a name**, and the holes inside it use
        // it: `<for r in :rows>{r.name}</for>` (ADR-017). That is the fifth
        // thing that counts as declaring one, after the four
        // `nothing_declares_it` lists - and the one a reader of a hole cannot
        // see, because the binding is in the markup around it.
        //
        // Measured: widening `NK1117` to a name in an expression refused
        // `examples/escaping/src/main.nika` for its `{r.shade}` until this frame existed.
        for (hole, bound) in crate::emit::literal_expressions_bound(self.parsed, literal) {
            let frame: Vec<Local> = bound
                .into_iter()
                // What the element's type is, is a question about the
                // collection, and this walk does not ask it: what it needs is
                // that the name is *declared*.
                .map(|name| Local::free(name, Ty::Unknown))
                .collect();
            self.scope.push(frame);
            // An f-string's holes, which the emitter walks the same way.
            let outer = match literal {
                Expr::LitInterpolated(_) => self.hole.replace((span.start, argument_shape(&hole))),
                _ => self.hole.clone(),
            };
            self.expr(&hole, span);
            self.hole = outer;
            self.scope.pop();
        }
    }

    /// `f(a, b)`, `Stats(first)`, `io::read_to_string()`, `write(p, d; append: true)`.
    /// A door over several locks: `access_all` reads them, `update_all` writes
    /// them ([ADR-065](../../docs/specification/adr/adr-065.md)).
    ///
    /// **It is typed here and not in the ledger**, for the reason the hull
    /// constructors are: a signature binds its type variables from the
    /// **receiver** (ADR-031), and a door over several locks has none - the
    /// locks are arguments, they hold different types, and how many there are is
    /// open. What a written signature cannot say, this says.
    ///
    /// `access_all` hands each value as a **view**, the way `access` does;
    /// `update_all` hands each in **by value** and takes a new one back per lock
    /// (D2). Both refuse anything that is not a lock, because a door that let a
    /// plain value through would be a door about nothing.
    fn locks(&mut self, door: MultiLock, args: &[Expr], span: &Span) -> Ty {
        let Some((last, locks)) = args.split_last() else {
            self.no_door(door, "it takes the locks and then the block", span);
            return Ty::Unknown;
        };
        let Expr::Closure {
            params,
            mutable,
            body,
        } = last
        else {
            self.no_door(door, "the block comes last: `fn(a, b) { … }`", span);
            return Ty::Unknown;
        };
        if locks.len() < 2 {
            self.no_door(door, "it is for **several** locks: name at least two", span);
            return Ty::Unknown;
        }

        let mut held = Vec::with_capacity(locks.len());
        for lock in locks {
            let found = self.expr(lock, span);
            match locked_content_of(&found) {
                Some(inside) => held.push(match (door, &inside) {
                    // A view of what it holds, the way `access` hands one over.
                    (MultiLock::Reading, Ty::Named { name, args, .. }) => Ty::Named {
                        name: name.clone(),
                        args: args.clone(),
                        view: true,
                    },
                    (MultiLock::Reading, _) => inside,
                    (MultiLock::Writing, _) => inside,
                }),
                // Nothing written down says this is a lock. A type nothing
                // describes is not claimed about at all (Part III, C.4) - but a
                // **number** is not such a type: a literal carries no type on
                // purpose (Part I 2.4), and reading that absence as "might be a
                // lock" would hand `rustc` a program about a trait bound.
                None if found.is_unknown() && !self.is_a_number(lock) => held.push(Ty::Unknown),
                None => {
                    self.checked.findings.push(Finding {
                        severity: Severity::Error,
                        span: span.clone(),
                        code: "NK1124",
                        message: format!(
                            "`{}` takes locks, and this is a `{}`",
                            door.written(),
                            found.text()
                        ),
                        notes: vec![
                            "a door over several locks is what keeps them in one order, and \
                             there is nothing to order about a value that is not one \
                             (Part II, 12.3)"
                                .to_string(),
                        ],
                        help: Some(format!(
                            "name `{SHARED_MUT}[T]` values, or `{LOCKED}[T]` fields"
                        )),
                    });
                    held.push(Ty::Unknown);
                }
            }
        }
        if !params.is_empty() && params.len() != locks.len() {
            self.no_door(
                door,
                "the block names one value per lock, in the order they are written",
                span,
            );
        }
        // D6 is D1 widened, so the same question is asked of this block.
        // D6 is D1 widened, so the same two questions are asked of this block.
        if matches!(door, MultiLock::Writing) {
            self.an_update_block_returns_nothing(body, span);
            self.an_update_block_reads_what_it_writes(params, body, span);
        }
        let outer_door = std::mem::replace(
            &mut self.at_a_write_door,
            matches!(door, MultiLock::Writing),
        );
        // Both kinds hold their locks open across the block, so both are one.
        let outer_inside = std::mem::replace(&mut self.inside_a_door, true);
        // **Nothing is asked of this block's promises**
        // ([ADR-102](../../docs/specification/adr/adr-102.md) D2), and that is
        // deliberate rather than an omission: `access_all` and `update_all` are
        // not ledger entries at all ([ADR-065](../../docs/specification/adr/adr-065.md)
        // D1), so there is no written type here to read a promise off. What
        // Part II 12.2 requires of the block — `sync`, and no lock — is
        // `NK2202`'s and `NK2203`'s, and a second refusal saying the same thing
        // in other words would be two rules for one mistake.
        self.lambda(
            params,
            mutable,
            body,
            &held,
            Promises {
                may_pause: true,
                may_fail: true,
            },
            span,
        );
        self.at_a_write_door = outer_door;
        self.inside_a_door = outer_inside;
        match door {
            // What a lambda hands back is not written down (ADR-029 D1) — but
            // that it came **out of a lock** is
            // ([ADR-111](../../docs/specification/adr/adr-111.md) D1), and the
            // stamp is what a `set` reads. `Seen[?]` is the honest pair: the
            // type is unknown and where it came from is not.
            MultiLock::Reading => Ty::seen(Ty::Unknown),
            MultiLock::Writing => Ty::Tuple(Vec::new()),
        }
    }

    /// Whether an expression is certainly a number, where its *type* says
    /// nothing.
    ///
    /// A literal has no type of its own (Part I 2.4), and neither has the name a
    /// bare `let` binds one to - but the constant fold watched both, so the
    /// absence of a type is not the absence of knowledge here.
    fn is_a_number(&self, expr: &Expr) -> bool {
        match expr {
            Expr::LitInt(_) | Expr::LitFloat(_) => true,
            Expr::Variable(name) => self
                .local(self.parsed.text(*name))
                .is_some_and(|(_, constant)| constant.is_some()),
            _ => false,
        }
    }

    /// The one message shape for a door written wrong.
    fn no_door(&mut self, door: MultiLock, wanted: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1124",
            message: format!("`{}` is written `{}`", door.written(), door.shape()),
            notes: vec![wanted.to_string()],
            help: None,
        });
    }

    /// `Shared(x)`, `SharedMut(x)` and `Locked(x)` make a hull
    /// ([ADR-064](../../docs/specification/adr/adr-064.md) D2).
    ///
    /// **One argument, and the hull takes the type of what it is handed.** A
    /// literal hands over `Unknown`, which is not a failure here: the hull is made
    /// by a *call* in the emitted code too, so the language below gives the number
    /// its type the way it gives one to any argument. That is the difference
    /// between a constructor and an annotation, and it is why the annotation could
    /// not answer `SharedMut(0)` at all.
    /// A type **as it is written**, checked for the one spelling this language
    /// does not have ([ADR-064](../../docs/specification/adr/adr-064.md) D3).
    ///
    /// `Shared[Locked[T]]` is what `SharedMut[T]` is, and two ways to write one
    /// type is the thing the short name was given a name *instead* of
    /// ([ADR-039](../../docs/specification/adr/adr-039.md) D9). Worse than
    /// untidy: the two are the same bytes below and two different types up here,
    /// so a value of one would not fit the other while the emitted Rust could not
    /// tell them apart.
    ///
    /// It runs at the positions a **program** writes a type, and the message
    /// carries the replacement.
    fn declared(&mut self, ty: &ast::Type, span: &Span) -> Ty {
        self.spelling(ty, span);
        Ty::from_ast(self.parsed, ty)
    }

    /// The walk under [`Checker::declared`], over a written type and everything
    /// inside it.
    fn spelling(&mut self, ty: &ast::Type, span: &Span) {
        if self.parsed.text(ty.name) == SHARED
            && let Some(held) = ty.generics.first()
            && self.parsed.text(held.name) == LOCKED
        {
            let inside = held
                .generics
                .first()
                .map(|t| self.parsed.text(t.name).to_string())
                .unwrap_or_else(|| "T".to_string());
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK1123",
                message: format!(
                    "a `{SHARED}` around a lock is what `{SHARED_MUT}[{inside}]` is called"
                ),
                notes: vec![
                    "the common case has the short name, and it is the only way \
                                     to write it - one type, one spelling (Part I, 6.2)"
                        .to_string(),
                ],
                help: Some(format!("write `{SHARED_MUT}[{inside}]`")),
            });
        }
        for inner in &ty.generics {
            self.spelling(inner, span);
        }
    }

    fn hull(&mut self, name: &str, args: &[Expr], found: &[Ty], span: &Span) -> Ty {
        if args.len() != 1 {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK1101",
                message: format!(
                    "`{name}` takes the value to put in it, and {} were passed",
                    args.len()
                ),
                notes: vec![format!(
                    "`{name}[T]` is made from a `T`: `{name}(value)` (Part I, 6.2)"
                )],
                help: None,
            });
            return Ty::Unknown;
        }
        let held = found.first().cloned().unwrap_or(Ty::Unknown);
        // A handle of a handle is two counts around one value and means nothing
        // the one count does not: it is refused where the type says so, and a
        // value nothing describes is not claimed about (C.4).
        if let Ty::Named { name: inner, .. } = &held
            && is_hull(inner)
        {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK1123",
                message: format!("this is already a `{inner}`, so `{name}` has nothing to add"),
                notes: vec![
                    "a handle is duplicated by being handed on, never by being wrapped \
                         again (Part I, 6.2)"
                        .to_string(),
                ],
                help: Some(format!("hand the `{inner}` on as it is")),
            });
            return held;
        }
        Ty::Named {
            name: name.to_string(),
            args: vec![held],
            view: false,
        }
    }

    fn call(&mut self, func: &Expr, args: &[Expr], config: &[ast::ConfigArg], span: &Span) -> Ty {
        // **A door over several locks types its own lambda**
        // ([ADR-065](../../docs/specification/adr/adr-065.md) D1), and it has to
        // be answered before the arguments are walked: what the lambda is handed
        // comes from what the *locks* hold, so the locks are typed first and the
        // lambda after. Every other call can walk its arguments in one pass.
        if let Expr::Variable(name) = func {
            if let Some(door) = MultiLock::named(self.parsed.text(*name)) {
                return self.locks(door, args, span);
            }
            // **`asset("…")` stands in a `comptime` initialiser and nowhere
            // else** ([ADR-116](../../docs/specification/adr/adr-116.md) D2).
            // The evaluator reads it where it belongs, so anything that reaches
            // *here* is one written somewhere it does not — and the file a
            // program reads while it **runs** has a name of its own.
            if self.parsed.text(*name) == ASSET && !self.inside_a_comptime {
                self.an_asset_outside_a_comptime(span);
                return Ty::Unknown;
            }
        }
        // **`NK2203`**: a free call inside a door's block, to something that
        // takes a lock (ADR-039 D2). `println` is the one every program writes,
        // and ADR-067 D1 is where it was pinned down: it never pauses, so
        // `sync` says nothing about it, and it takes standard output's own lock
        // while yours is open.
        if self.inside_a_door
            && let Some(name) = self.free_callee(func)
        {
            let holds = self
                .own
                .functions
                .get(&name)
                .or_else(|| self.library.functions.get(&name))
                .map(|c| c.touches_a_lock)
                .unwrap_or_default();
            self.a_lock_inside_a_lock(&name, holds, span);
        }
        // **The callee is named and resolved before the arguments are walked**
        // ([ADR-029](../../docs/specification/adr/adr-029.md), the free half).
        // A lambda's parameters are typed from the callee's signature, so the
        // signature has to be in hand first — which is exactly the ordering the
        // *method* path was given when that record landed, and which this one
        // did not have: `hand(fn(n) { … })` left `n` with no type at all, and
        // [ADR-102](../../docs/specification/adr/adr-102.md) D2's promises had
        // nothing to be asked of.
        let name = match func {
            Expr::Variable(name) => self.parsed.text(*name).to_string(),
            // `unaliased`: `h::serve()` is `http::serve()` where the file wrote
            // `use http as h` (ADR-046 D3), and the ledger knows only the
            // package's own name.
            Expr::Path(segments) => self.parsed.unaliased(
                &segments
                    .iter()
                    .map(|s| self.parsed.text(*s))
                    .collect::<Vec<_>>()
                    .join("::"),
            ),
            other => {
                self.expr(other, span);
                return Ty::Unknown;
            }
        };

        // **A kept function value called by name**: a `let`, or a parameter
        // the function keeps. What its type says is what the call does.
        if matches!(func, Expr::Variable(_))
            && !self.run_code.contains(&name)
            && let Some(Ty::Fn {
                params,
                result,
                is_sync,
                throws,
            }) = self.lookup(&name)
        {
            self.read_at.push((name.clone(), span.start));
            self.arguments_given(args, &params, false, None, span);
            self.kept_arguments_are_its_own(args, &params);
            self.checked
                .kept_calls
                .insert((span.start, name), (!is_sync, throws));
            if let Some(current) = &self.current {
                let entry = self.checked.methods.entry(current.clone()).or_default();
                entry.code_pauses |= !is_sync;
                entry.code_fails |= throws;
            }
            return result.map(|r| *r).unwrap_or_else(|| Ty::named("()"));
        }

        // **A variant is built by its constructor, and its parts are what it
        // keeps** (0.0.236). `Stmt::Say("a")` resolved to nothing, so its
        // argument was walked with no type to meet: the literal went below as
        // a `&str` where the variant holds a `String`, and `rustc` said
        // *mismatched types* about a file nobody wrote. The parts are the
        // variant's declared types, a literal handed to text is built into
        // text of its own there (ADR-207 D2), and a name handed in is given.
        if let Some(parts) = self.variant_parts.get(&name).cloned() {
            let found = self.arguments_given(args, &parts, false, None, span);
            for ((arg, want), ty) in args.iter().zip(&parts).zip(&found) {
                if self.text_literal(want, arg, true).is_none() {
                    self.hands_over(arg, ty, "put into a variant, which keeps it", span);
                }
            }
            let owner = name
                .rsplit_once("::")
                .map_or(name.as_str(), |(owner, _)| owner);
            return Ty::named(owner);
        }

        // **A call to an `extern` name is written inside `unsafe { … }`**
        // ([ADR-124](../../docs/specification/adr/adr-124.md) D3), and this is
        // where that is asked: the name is in hand and the block is a flag the
        // walk carries.
        self.a_foreign_call_outside_unsafe(&name, span);

        // **A type is constructed by its anonymous constructor**
        // ([ADR-140](../../docs/specification/adr/adr-140.md) D2), so a written
        // `Type::new` is the other spelling and is refused — in `std` as in a
        // `.nika` file, which is the whole of what D2 evens out.
        self.a_constructor_written_as_new(&name, span);

        // **What needs no `use` is the list on Part I's first page**
        // ([ADR-154](../../docs/specification/adr/adr-154.md) D1), and these two
        // are the halves of the rule: a name that lives in a module written
        // without it, and a module used without being introduced.
        self.a_std_name_without_its_module(&name, span);
        self.a_module_used_before_it_is_introduced(&name, span);
        self.a_prelude_name_with_a_module_in_front(&name, span);
        self.a_constructor_nothing_describes(&name, args, span);

        // **A grammar is entered by an ordinary call**
        // ([ADR-082](../../docs/specification/adr/adr-082.md) D1), through a
        // **path** since [ADR-140](../../docs/specification/adr/adr-140.md) D3.
        // Answered before anything is resolved, because a grammar is not a
        // ledger entry a `resolve` would find.
        if let Some(entered) = self.grammar_path(&name) {
            return self.grammar_call(&entered, args, span);
        }

        // **A call to a function that walks a type's fields is an
        // *instantiation*** ([ADR-181](../../docs/specification/adr/adr-181.md)
        // D2): the shape is the caller's to supply, so the loop is unrolled
        // once per type argument actually used and this is where the type
        // argument is known.
        //
        // Recorded here and walked afterwards, because the body has to be read
        // with the fields in hand and this walk is in the middle of another
        // one. What is refused at the call itself is `NK1164` — *you passed
        // something that is not a struct*
        // ([ADR-088](../../docs/specification/adr/adr-088.md) D3) — which the
        // bound already answers and which is why this can assume a struct.
        self.an_instantiation(&name, args, span);

        // **A struct literal written like a call**
        // ([ADR-140](../../docs/specification/adr/adr-140.md) D1). `Stats(min: 1)`
        // parsed as Kap 4.2's named literal until D1 took that spelling out of the
        // language; it is a call with options now, and where the name is a **type**
        // that is the old form rather than a call anybody meant. Asked here because
        // the parser cannot ask it - the two forms are one spelling and a name
        // denotes one of them - and asked before the resolution below, which would
        // otherwise answer about the anonymous constructor: the silent lowering was
        // `Stats::new()` with the fields dropped, which `rustc` then refused about a
        // file nobody wrote ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
        if !config.is_empty() && args.is_empty() && self.declares_a_type(&name) {
            self.a_literal_written_like_a_call(&name, config, span);
            for option in config {
                self.expr(&option.value, span);
            }
            return Ty::named(&name);
        }

        // **A type of the program's own with no constructor, called** (0.0.240):
        // `NotANumber(text)` against `struct NotANumber { text: String }`
        // lowered to `NotANumber::new(…)`, and `rustc` said *no function named
        // `new`* about a file nobody wrote. A struct is built with a literal.
        if !args.is_empty() && self.structs.contains_key(&name) && self.resolve(&name).is_none() {
            self.a_struct_called_without_a_constructor(&name, span);
        }

        // A tuple variant of an enum declared here - `Op::Plus(1)` - is a value
        // of that enum, not a call to a function. Its arguments are still
        // walked, below, with the rest.
        let variant = name
            .split_once("::")
            .filter(|(ty, variant)| self.is_variant(ty, variant))
            .map(|(ty, _)| ty.to_string());
        // Neither a variant nor a hull is a ledger entry, so neither has a
        // signature to type an argument from.
        let resolved = match variant.is_some() || is_hull(&name) {
            true => None,
            false => self.resolve(&name),
        };
        let expected: Vec<Ty> = resolved
            .as_ref()
            .map(|(_, contract)| expected_arguments(contract))
            .unwrap_or_default();
        // The same walk the method path uses, so the same questions are asked
        // in both - or `takes(self.name)` slips past `NK1131` while
        // `x.takes(self.name)` does not.
        let found = self.arguments_given(
            args,
            &expected,
            resolved
                .as_ref()
                .is_some_and(|(key, _)| self.own.functions.contains_key(key)),
            resolved.as_ref().map(|(_, contract)| &**contract),
            span,
        );
        let passed: Vec<(String, Ty)> = config
            .iter()
            .map(|a| {
                self.a_field_of_a_borrowed_subject(&a.value, span, "passed");
                (
                    self.parsed.text(a.name).to_string(),
                    self.expr(&a.value, span),
                )
            })
            .collect();

        if let Some(ty) = variant {
            return Ty::named(&ty);
        }

        // **A handle is not constructed** (ADR-147 D3): it is an address a C
        // function hands back, so there is no value of one this language can
        // make. Refused here rather than below, where `rustc` would say the
        // tuple struct takes one field and name a type the source never wrote.
        if self.opaque_handles.contains(&name) {
            self.a_handle_is_not_made_here(&name, span);
            return Ty::named(&name);
        }

        // **A hull you can observe, you write**
        // ([ADR-064](../../docs/specification/adr/adr-064.md) D2). The three hull
        // types are made by a call, and it is this compiler's to type rather than
        // the ledger's: the ledger binds `$T` from a **receiver** (ADR-031) and a
        // constructor has none, so the type argument would have to come from what
        // is handed in.
        if is_hull(&name) {
            return self.hull(&name, args, &found, span);
        }

        let Some((key, contract)) = resolved else {
            if matches!(func, Expr::Variable(_)) {
                self.a_function_nothing_declares(&name, span);
            }
            // A call nothing describes is a call this compiler cannot see the
            // end of, and a thread of its own is among the things it may do
            // (ADR-038 D7). What it is handed is therefore handed across.
            self.crosses_into_an_unseen_call(
                &name,
                Arguments {
                    args,
                    found: &found,
                    config,
                    passed: &passed,
                },
                span,
                &format!(
                    "nothing written down describes `{name}`, so this compiler cannot see the \
                     end of it - and starting a thread of its own is among the things it may do \
                     (Part III, 15.2)"
                ),
            );
            // And a call this compiler cannot see the end of says nothing about
            // whether it can **fail**, either
            // ([ADR-091](../../docs/specification/adr/adr-091.md)).
            self.guard_has_no_answer();
            return Ty::Unknown;
        };
        self.reachable(&name, contract, span);
        // **A described call is asked too, where the description says the word**
        // ([ADR-193](../../docs/specification/adr/adr-193.md) D2). It fires on
        // the **claim** and never on its absence: a description that does not
        // say is a description that was not asked, and the program keeps the
        // answer it has today. Before this, a crate that answered every other
        // question honestly turned `NK2502` off by being described, which is
        // the half of [ADR-038](../../docs/specification/adr/adr-038.md) D7
        // that was left open.
        if contract.threads.may() {
            self.crosses_into_an_unseen_call(
                &name,
                Arguments {
                    args,
                    found: &found,
                    config,
                    passed: &passed,
                },
                span,
                &format!(
                    "`{name}` is described as starting a thread of its own (`threads = true`), \
                     so what it is given may be looked at from one (Part III, 15.2)"
                ),
            );
        }
        self.may_fail_here(&key, contract, span);
        self.a_call_that_may_pause(&name, !contract.sync.is_sync(), span);
        self.a_pausing_call_in_an_action(&name, contract, span);
        // `Stats(first)` is the anonymous constructor of Kap 4.2, which the
        // lowering names `Stats::new` - and which hands back the type it is on,
        // whatever its declaration says about `Self`.
        //
        // **Unless the declaration already names it with its arguments**
        // ([ADR-140](../../docs/specification/adr/adr-140.md) D2). A `.nika`
        // file's constructor is on a type with no parameters, so naming the type
        // is the whole answer; `std`'s own entries are not — `Vec::new` declares
        // `-> Vec[?]`, and `Ty::named("Vec")` threw the `[?]` away, so
        // `let xs = Vec()` came out as a `Vec` and `NK1106` refused it against
        // every `Vec[T]` it was given to. The rule was right for the one case it
        // had and wrong for the one D2 brought in.
        let declared = contract
            .signature
            .as_ref()
            .and_then(|s| s.result.as_ref())
            .filter(|ty| !matches!(ty, Ty::Unknown))
            .filter(|ty| !matches!(ty, Ty::Named { name, .. } if name == "Self"));
        let constructed = key
            .strip_suffix("::new")
            .filter(|_| !name.ends_with("::new"))
            .map(|ty| match declared {
                Some(declared) => declared.clone(),
                None => Ty::named(ty),
            });
        let result = self.arguments(&key, &name, contract, args, &found, &passed, span);
        // **What the arguments tell the signature**
        // ([ADR-074](../../docs/specification/adr/adr-074.md) D2). A free
        // function has no receiver, so ADR-031's binding had nothing to work
        // from and `hand(7)` handed back `?`; the same `bind` pointed at the
        // parameters answers `i64`. After `arguments` rather than before it,
        // because a variable fits everything and the check is therefore the
        // same either way round - while the *types* are not: a lambda's
        // parameters are typed from the signature (ADR-029), so the signature
        // has to reach them unsubstituted.
        let bound = from_arguments(contract, &found);
        // …and what the arguments tell a **bound** (ADR-174 D2), which is the
        // same binding read for the other question a type parameter raises.
        self.a_bound_the_argument_does_not_meet(&key, &bound, span);
        let result = constructed.unwrap_or_else(|| ty::substitute(&result, &bound));
        self.stamped_through(contract, &found, result)
    }

    /// The count and the types of what a call passes, against what it takes.
    ///
    /// `given` is the argument expressions, for the one diagnostic that has to
    /// name the **value** rather than its type: `NK1115` points at the line where
    /// the sharing belongs, and that line starts with the name the caller wrote.
    #[allow(clippy::too_many_arguments)]
    fn arguments(
        &mut self,
        key: &str,
        // The callee as the source writes it, which is the part the emitter can
        // match a recorded argument against (`nullable_args`).
        written: &str,
        contract: &FnContract,
        given: &[Expr],
        found: &[Ty],
        passed: &[(String, Ty)],
        span: &Span,
    ) -> Ty {
        // No signature is no claim. The hand-written half of `std.contracts`
        // has some, and an entry that says nothing is checked against nothing.
        let Some(signature) = contract.signature.clone() else {
            return Ty::Unknown;
        };
        let wanted = signature.arguments();

        if wanted.len() != found.len() {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK1101",
                message: format!(
                    "`{key}` takes {}, and this call passes {}",
                    plural(wanted.len(), "argument"),
                    found.len()
                ),
                notes: vec![format!("`{key}{}`", signature.text())],
                help: Some(match wanted.len() {
                    0 => format!("call it as `{key}()`"),
                    _ => {
                        let takes = wanted
                            .iter()
                            .map(|(n, t)| format!("`{n}: {}`", t.text()))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("it takes {takes}{}", a_root_is_one_of_two(wanted))
                    }
                }),
            });
            return signature.result_or_unit();
        }

        // **A length beside a view is checked at the call**
        // ([ADR-147](../../docs/specification/adr/adr-147.md) D2), before the
        // arguments are measured one at a time: the question is about a *pair*
        // of them, and a `usize` that is measured on its own has already
        // passed.
        self.a_length_that_fits_its_buffer(written, wanted, given, found, span);

        // Kap 5.1: an option is named, so it is checked by name - that it
        // exists, and that what is passed is what it takes.
        for (name, found) in passed {
            match signature.config.iter().find(|c| c.name == *name) {
                Some(option) => {
                    if found.fits(&option.ty) {
                        continue;
                    }
                    let (want, ty) = (option.ty.clone(), option.ty.text());
                    let name = name.clone();
                    let key = key.to_string();
                    self.expect(found, &want, span.clone(), "field", move |found, _| {
                        format!("`{key}` takes `{name}: {ty}`, and this passes `{found}`")
                    });
                }
                None => self.no_such_option(key, name, &signature, span),
            }
        }

        let text_view = Ty::view(ty::TEXT_VIEW);
        for (at, ((name, want), found)) in wanted.iter().zip(found).enumerate() {
            // **A view of a `String` is a view of text here**
            // ([ADR-233](../../docs/specification/adr/adr-233.md) D4): an
            // element of `words.iter()` is one, and the language below lends it
            // as the `&str` a parameter takes at the call, by itself.
            let found = match is_a_view_of_a_string(found) {
                true => &text_view,
                false => found,
            };
            // A literal that cannot fit the parameter it is given, asked here
            // rather than where the argument was walked: a free call walks its
            // arguments before it resolves the callee, so the type to measure
            // against is not known yet at that point (ADR-029's ordering is what
            // gives a *method* call the answer earlier).
            if let Some(given) = given.get(at) {
                self.constant_fits(given, Some(want), span);
            }
            self.a_cleanup_that_moved(key, name, want, given.get(at), span);
            // **A parameter is a use** (ADR-152 D4), and a literal handed to one
            // that takes an `Array[T, N]` **is** that array. What it changes is
            // the type the rest of this loop reads, rather than ending it: the
            // `&` the compiler writes (ADR-094 D1) and the ordinary fit are
            // still this argument's questions, and they have to be asked of what
            // the literal turned out to be.
            //
            // **And one handed to a `String` is one** (ADR-207 D2) — text of its
            // own where the callee keeps it, and the literal as it is where the
            // callee only reads it, because that parameter is a `&str` below
            // (D3) and a constant needs no copy to be read.
            let lent = crate::contracts::keeps::lends(
                contract,
                at + usize::from(signature.takes_a_receiver()),
            ) || (at == 0 && is_a_lookup(key));
            let kept = !lent;
            // **What the callee keeps, it is given** (ADR-213 D3): no `&` is
            // written for this position, so a name here moves.
            // **Not across the C boundary**: what a C declaration takes it
            // takes by address or by copy, and the call writes that address
            // ([ADR-147](../../docs/specification/adr/adr-147.md) D1).
            if kept
                && !want.is_a_view()
                && !signature.mutable.contains(name)
                && !self.foreign_names.contains(written)
                && let Some(given) = given.get(at)
            {
                self.hands_over(
                    given,
                    found,
                    &format!("handed to `{written}`, which keeps it"),
                    span,
                );
            }
            // **A type variable only this argument decides takes the
            // argument's own type** (0.0.230, `open-work.md` §1.19 before it): a text
            // literal is a `String`, so where a kept, by-value `$T` is bound by
            // nothing else - no receiver, no other parameter - it is constructed
            // as one. `hyper_shim::across_a_thread(value: $T)` with
            // `T: Describe` implemented for `String` is the case: the literal's
            // `&str` was a type the program never wrote. A `$T` a receiver
            // binds (`Vec[$T]::push`) is the receiver's to answer and is left.
            let text = Ty::named("String");
            let literal_wants = match kept && self.decided_by_this_argument(&signature, wanted, at)
            {
                true => &text,
                false => want,
            };
            let array = given.get(at).and_then(|given| {
                self.array_literal(found, want, given, span)
                    .or_else(|| self.text_literal(literal_wants, given, kept))
            });
            let found = array.as_ref().unwrap_or(found);
            // **A view handed to a `String` the callee only reads is lent as it
            // is** ([ADR-208](../../docs/specification/adr/adr-208.md) D1). The
            // parameter is a `&str` below (ADR-207 D3), so the view already is
            // what the callee takes: asking for `.to_owned()` here asked for a
            // copy nothing would keep. Only for a function this compiler
            // declares, because only there is the `&str` its own writing.
            //
            // **Not a `ref` the source wrote**, which is `NK1137`'s below: the
            // reference at a lending call is the compiler's, and a view that
            // arrives as a view is the case this is for.
            let wrote_ref = matches!(
                given.get(at),
                Some(Expr::Unary {
                    op: crate::ast::UnaryOp::Ref,
                    ..
                })
            );
            if !kept
                && !wrote_ref
                && *found == Ty::view("str")
                && *want == Ty::named("String")
                && self.own.functions.contains_key(key)
            {
                continue;
            }
            // **A view handed to a parameter both kinds flow into** is
            // borrowed as it is ([ADR-223](../../docs/specification/adr/adr-223.md)
            // D2): the emitter hands it over, as it does into a mixed field.
            if views_into_text(found, want) && self.either_param(key, at) {
                continue;
            }
            // **A `usize` at the C boundary takes this language's own integer**
            // ([ADR-147](../../docs/specification/adr/adr-147.md) D2,
            // [ADR-048](../../docs/specification/adr/adr-048.md) D1). A length
            // here is an `i64` and the machine-width type left the surface a
            // program can write, so a declaration that says `size_t` is handed
            // an `i64` and the conversion is emitted — the same arrangement
            // `str::repeat` has, arrived at from the **declaration** rather
            // than from a name.
            //
            // `continue`, because this *is* the fit: what is left for the
            // positions below to decide is a `&` this argument cannot want, a
            // number being copied rather than moved.
            if self.a_count_at_the_boundary(written, want, found) {
                continue;
            }
            // Part I 2.3's third position for the wrap: a plain value in a
            // parameter the callee declares nullable. Recorded before `fits`
            // is consulted, because this *is* the fit - `Ty::fits` allows it,
            // and what is left is telling the emitter to write the
            // constructor.
            // **The compiler writes the `&` at the call** (ADR-094 D1), where
            // the callee lends this position and the value is not already a
            // view. Asked before the wrap and the deref for `nullable_args`'
            // reason: a lent argument *is* a fit, and what is left is telling
            // the emitter what to write. The rule asks the fit itself, so an
            // argument that is simply the wrong value falls through to the
            // message it always had.
            if self.the_compiler_writes_the_reference(
                contract,
                Argument {
                    at,
                    given: given.get(at),
                    found,
                    want,
                },
                written,
                span,
            ) {
                continue;
            }
            let is_literal = given.get(at).is_some_and(is_literal);
            if let Some(how) = wrap_for(found, want, is_literal) {
                self.checked
                    .nullable_args
                    .entry((span.start, written.to_string(), at))
                    .or_default()
                    .insert(given.get(at).map(argument_shape).unwrap_or_default(), how);
                continue;
            }
            if self.fits_through_deref(found, want) {
                continue;
            }
            // **A view of a value that copies, where the value is wanted**
            // ([ADR-233](../../docs/specification/adr/adr-233.md) D4): the copy
            // out of it is a machine word, and the compiler's to write. It is
            // what a lambda `sort_by_key` hands a `ref i64` meets first.
            // A **name**: what an operator makes of a view is a value below
            // whatever this checker calls it, and a `*` on it is not Rust.
            if copies_as_a_view(found)
                && unviewed(found) == *want
                && matches!(given.get(at), Some(Expr::Variable(_)))
            {
                self.checked
                    .copied_args
                    .entry((span.start, written.to_string(), at))
                    .or_default()
                    .insert(given.get(at).map(argument_shape).unwrap_or_default());
                continue;
            }
            // A plain value where a hull is wanted. It is refused by a code of
            // its own because the sentence is worth more than a type mismatch's:
            // it names the hull and the value. **What the way out is changed**
            // ([ADR-064](../../docs/specification/adr/adr-064.md) D2) - the word
            // goes here, at the call, where it used to have to go somewhere else.
            if becomes_shared(found, want) {
                let value = given.get(at).and_then(|arg| self.names_of(arg));
                self.checked.findings.push(Finding {
                    severity: Severity::Error,
                    span: span.clone(),
                    code: "NK1115",
                    message: match &value {
                        Some(value) => {
                            format!("`{key}` takes a shared value, and `{value}` is not one")
                        }
                        None => format!("`{key}` takes a shared value, and this is not one"),
                    },
                    notes: vec![format!("`{key}{}`", signature.text())],
                    help: Some(match (&value, want) {
                        (Some(value), Ty::Named { name, .. }) => {
                            format!("write `{name}({value})` - a hull you can see is one you write")
                        }
                        (None, Ty::Named { name, .. }) => {
                            format!("write `{name}(…)` around it (Part I, 6.2)")
                        }
                        _ => format!("make it a `{}`", want.text()),
                    }),
                });
                continue;
            }
            let (why, help) = match self.a_view_kept(
                found,
                want,
                given.get(at),
                &format!("`{key}` keeps its `{name}` after the call returns"),
            ) {
                Some((why, help)) => (why, help),
                None => (Vec::new(), convert(found, want)),
            };
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK1102",
                message: format!(
                    "`{key}` takes `{name}: {}`, and this call passes `{}`",
                    want.text(),
                    found.text()
                ),
                notes: std::iter::once(format!("`{key}{}`", signature.text()))
                    .chain(why)
                    .collect(),
                help: Some(help),
            });
        }

        signature.result_or_unit()
    }

    // --- the one shape every check has --------------------------------------

    /// Whether `found` fits `want` once a transparent container is seen through.
    ///
    /// A type whose ledger records a `deref` is one a caller is not meant to
    /// notice: Part III says of `fs::Mapped` that "a parser cannot tell the
    /// difference", and the entry spells it — `fs::Mapped::deref` is
    /// `(&Mapped) -> &str`. Nothing consulted that entry, so a function taking a
    /// text view refused a mapped file, which is the opposite of what the
    /// specification promises about it.
    ///
    /// **Only as a rescue, never as a rule of its own.** It is asked after a
    /// direct comparison has already failed, so seeing through a container can
    /// make a refusal into an acceptance and can never turn an acceptance into a
    /// refusal. One step only: a container inside a container is two questions,
    /// and nothing in the language has asked the second yet
    /// ([ADR-028](../../../docs/specification/adr/adr-028.md) D5).
    fn fits_through_deref(&self, found: &Ty, want: &Ty) -> bool {
        if found.fits(want) {
            return true;
        }
        let Ty::Named { name, .. } = found else {
            return false;
        };
        let Some((_, contract)) = self.method(&format!("{name}::deref")) else {
            return false;
        };
        // The receiver's own arguments bind the signature's variables, exactly
        // as they do for any other method (ADR-031), so `(&Shared[$T]) -> &$T`
        // answers with what *this* `Shared` holds rather than with a variable.
        let Some(signature) = contract.signature.as_ref() else {
            return false;
        };
        let Some(result) = signature.result.as_ref() else {
            return false;
        };
        let bound = bindings(contract, found);
        ty::substitute(result, &bound).fits(want)
    }

    /// [`Checker::expect`], for a position that has the value in hand, so a
    /// view of text kept where text of its own is wanted can say **why** here
    /// ([ADR-208](../../docs/specification/adr/adr-208.md) D2). `keeper` is the
    /// position's own words for what keeps it.
    #[allow(clippy::too_many_arguments)]
    fn expect_kept(
        &mut self,
        found: &Ty,
        want: &Ty,
        value: Option<&Expr>,
        keeper: &str,
        span: Span,
        what: &str,
        message: impl FnOnce(&str, &str) -> String,
    ) {
        // **A view into a position both kinds of text flow into** is borrowed
        // as it is (ADR-222 D3, ADR-223 D3): that is what the position is for.
        // The position reads as text of its own, so a view is what needs
        // letting through.
        let either = match what {
            "returns" => self.expected_either,
            "field" => std::mem::take(&mut self.field_either),
            _ => false,
        };
        if either && *found == Ty::view("str") && *want == Ty::named("String") {
            return;
        }
        // **Into a `String?` the same holds of what is there**
        // ([ADR-224](../../docs/specification/adr/adr-224.md) D1): a view,
        // present or absent, goes into a mixed one as it is, and is refused
        // from an owned one - which the `?` alone had let through to the
        // language below.
        if views_into_nullable_text(found, want) {
            if either {
                return;
            }
            let present = match found {
                Ty::Nullable(found) => found.as_ref(),
                found => found,
            };
            let text = Ty::named("String");
            let before = self.checked.findings.len();
            self.expect(present, &text, span, what, message);
            if self.checked.findings.len() > before
                && let Some((why, help)) = self.a_view_kept(present, &text, value, keeper)
                && let Some(finding) = self.checked.findings.last_mut()
            {
                finding.notes.extend(why);
                finding.help = Some(help);
            }
            return;
        }
        let before = self.checked.findings.len();
        self.expect(found, want, span, what, message);
        if self.checked.findings.len() == before {
            return;
        }
        if let Some((why, help)) = self.a_view_kept(found, want, value, keeper)
            && let Some(finding) = self.checked.findings.last_mut()
        {
            finding.notes.extend(why);
            finding.help = Some(help);
        }
    }

    /// **A view kept where the receiver says text of its own**
    /// ([ADR-225](../../docs/specification/adr/adr-225.md) D2):
    /// `names.push(line.trim())` on a `Vec[String]`. The signature says `$T`,
    /// which a view fits; the receiver binds `$T` to `String`, and that is the
    /// question - the one [`Checker::arguments`] asks of a parameter that says
    /// `String` itself. A position the tier pass made a view is `ref String`
    /// here and fits; one it made mixed is handed `value.into_either()`, which
    /// reads as text of its own.
    fn a_view_kept_where_the_receiver_says_text(
        &mut self,
        key: &str,
        contract: &FnContract,
        given: &[Expr],
        found: &[Ty],
        expected: &[Ty],
        span: &Span,
    ) {
        let Some(signature) = contract.signature.as_ref() else {
            return;
        };
        let arguments = signature.arguments();
        for (at, ((name, declared), want)) in arguments.iter().zip(expected).enumerate() {
            let generic = matches!(declared, Ty::Var { .. });
            let lent = crate::contracts::keeps::lends(
                contract,
                at + usize::from(signature.takes_a_receiver()),
            ) || (at == 0 && is_a_lookup(key));
            let kept = !lent;
            let Some(found) = found.get(at) else { continue };
            if !generic || !kept || !views_into_text(found, want) {
                continue;
            }
            // A literal is built into text where it is kept (ADR-207 D2),
            // which the caller has already recorded.
            let literal = given.get(at).and_then(|g| self.text_literal(want, g, true));
            let found = literal.as_ref().unwrap_or(found);
            if !views_into_text(found, want) {
                continue;
            }
            let keeper = format!("`{key}` keeps its `{name}` after the call returns");
            let (key, name) = (key.to_string(), name.clone());
            self.expect_kept(
                found,
                want,
                given.get(at),
                &keeper,
                span.clone(),
                "argument",
                move |found, want| {
                    format!("`{key}` takes `{name}: {want}` here, and this call passes `{found}`")
                },
            );
        }
    }

    /// Whether parameter `at` of a function this program declares is text both
    /// kinds of which flow into (ADR-223 D2): by its key, `f` or `Type::m`.
    fn either_param(&self, key: &str, at: usize) -> bool {
        let (target, own) = match key.rsplit_once("::") {
            Some((target, own)) => (Some(target.rsplit("::").next().unwrap_or(target)), own),
            None => (None, key),
        };
        let either = |item: &Item| {
            matches!(item, Item::Fn { name: Some(n), args, .. }
                if self.parsed.text(*n) == own && args.get(at).is_some_and(|a| a.ty.either))
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
                ) if self.parsed.text(t.name) == target => methods.iter().any(|m| either(&m.node)),
                (node, None) => either(node),
                _ => false,
            })
    }

    /// A function field code is put into where the lock column cannot read it
    /// (ADR-230 D1): undecided, never quiet.
    fn a_field_whose_code_is_not_seen(&mut self, owner: &str, field: &str) {
        self.checked
            .stored_code
            .entry(format!("{owner}.{field}"))
            .or_default()
            .unresolved = true;
    }

    /// Whether a struct's field is text both kinds of which flow into
    /// ([ADR-222](../../docs/specification/adr/adr-222.md) D3).
    fn either_field(&self, owner: &str, field: &str) -> bool {
        let owner = owner.rsplit("::").next().unwrap_or(owner);
        self.parsed
            .program
            .items
            .iter()
            .any(|item| match &item.node {
                Item::Struct { name, fields, .. } if self.parsed.text(*name) == owner => fields
                    .iter()
                    .any(|f| f.ty.either && self.parsed.text(f.name) == field),
                _ => false,
            })
    }

    /// **Why a view of text needs `.to_owned()` here, said for the case it is**
    /// ([ADR-208](../../docs/specification/adr/adr-208.md) D2).
    ///
    /// A language that hides ownership owes the reader the reason at the one
    /// place it does not: otherwise the question is *the compiler knows exactly
    /// what to write, so why is it asking me?* Three answers, because there
    /// are three situations and one sentence for all of them fits none:
    ///
    /// * **a name bound to a literal** — the compiler *would* build the literal
    ///   into a `String` written here, and says so, with the two ways to write
    ///   it that need nothing;
    /// * **a parameter declared `ref String`** — the text is the caller's, and
    ///   the better answer is usually to let the caller hand it over;
    /// * **any other view** — it points into something that stays where it is.
    ///
    /// And the reason itself, once: a copy costs as much as the text is long,
    /// and one the compiler made on its own would happen on every run with no
    /// line in the program to show for it ([ADR-005](../../docs/specification/adr/adr-005.md) §3).
    ///
    /// `None` where this is not that case, and the caller keeps its own words.
    fn a_view_kept(
        &self,
        found: &Ty,
        want: &Ty,
        value: Option<&Expr>,
        keeper: &str,
    ) -> Option<(Vec<String>, String)> {
        if *found != Ty::view("str") || *want != Ty::named("String") {
            return None;
        }
        let cost = "Nikaia copies text only where the program says so: a copy costs as much \
                    as the text is long, and one made on its own would run every time this \
                    line does, with nothing in the source to show it (ADR-005 §3)"
            .to_string();
        let named = match value {
            Some(Expr::Variable(name)) => Some(self.parsed.text(*name).to_string()),
            _ => None,
        };
        let binding = named.as_deref().and_then(|name| self.binding(name));
        if let (Some(name), Some(literal)) = (&named, binding.and_then(|b| b.literal.clone())) {
            return Some((
                vec![
                    format!(
                        "`{name}` is bound to the literal \"{literal}\", and a name bound to a \
                         literal is a view of it"
                    ),
                    format!(
                        "{keeper}, so it needs text of its own. Written here, the literal would \
                         be built into one on this line (ADR-207); through a name it is not, \
                         yet - that would change the type `{name}` was declared with, on a line \
                         above this one"
                    ),
                ],
                format!(
                    "write \"{literal}\" here, or declare `let {name}: String = \"{literal}\"` \
                     - either builds the text once, as the literal would"
                ),
            ));
        }
        let parameter = binding.is_some_and(|b| {
            b.immutable
                .as_ref()
                .is_some_and(|i| i.kind == Kind::Parameter)
        });
        if let (Some(name), true) = (&named, parameter) {
            return Some((
                vec![
                    format!(
                        "`{name}` is declared `ref String`: the text belongs to the caller, \
                         who still has it"
                    ),
                    format!("{keeper}, so it needs text of its own"),
                    cost,
                ],
                format!(
                    "declare `{name}: String`, and the caller hands its text over instead of \
                     lending it - or write `{name}.clone()` to copy it here"
                ),
            ));
        }
        Some((
            vec![
                "this is a view: it points into text something else owns, and that text \
                 stays where it is"
                    .to_string(),
                format!("{keeper}, so it needs text of its own"),
                cost,
            ],
            "write `.clone()` to copy it here".to_string(),
        ))
    }

    /// Report only when both sides are known and they disagree.
    fn expect(
        &mut self,
        found: &Ty,
        want: &Ty,
        span: Span,
        what: &str,
        message: impl FnOnce(&str, &str) -> String,
    ) {
        if self.fits_through_deref(found, want) {
            return;
        }
        let code = match what {
            "let" => "NK1103",
            "returns" => "NK1104",
            "assign" => "NK1105",
            "field" => "NK1106",
            // An argument a method's receiver types (ADR-225 D2), which is
            // the code `arguments` gives one its signature types.
            "argument" => "NK1102",
            // **`NK1166`, and it used to be an `unreachable!`.** A `comptime`
            // whose value disagrees with its declared type reached `expect`
            // with a word that had no code, and the compiler **panicked** -
            // which is [Part I 6.8](../../docs/specification/10-nikaia-light.md)'s
            // *a raw internal error reaching you is a Nikaia bug*, met by the
            // compiler itself. Found by writing
            // `comptime PRIMES: Array[i64, 4] = [2, 3, 5, 7]`, which is a
            // correct program and crashed the build.
            "const" => "NK1166",
            other => unreachable!("no code for `{other}`"),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span,
            code,
            message: message(&found.text(), &want.text()),
            notes: Vec::new(),
            help: Some(convert(found, want)),
        });
    }

    fn expect_bool(&mut self, found: &Ty, span: &Span, why: &str) {
        let bool_ty = Ty::named("bool");
        if found.fits(&bool_ty) {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1108",
            message: format!("this is `{}`, and a condition is a `bool`", found.text()),
            notes: vec![why.to_string()],
            help: Some(
                "compare it: `x != 0`, `text != \"\"`, `xs.len() > 0` (Part I, 3.2)".to_string(),
            ),
        });
    }

    /// A written call to something that can fail (`NK2605`).
    ///
    /// ADR-025 D1 states the rule for the calls **nobody wrote** - a block's
    /// closing brace, a loop's step - and says it is "exactly as a written
    /// call would" behave. The written call is the case the rule was
    /// generalised *from*, and it had no code: a function calling something
    /// that can fail and declaring nothing lowered in silence, and the ledger
    /// then published `[fn."ruft"] signature = "() -> String"` with no
    /// `throws` at all - a committed file that other programs read
    /// ([ADR-020](../../../../docs/specification/adr/adr-020.md)) saying a
    /// function cannot fail when its body can. `rustc` was what refused the
    /// program, about a file the author never wrote, which Part III C.1 calls
    /// a bug in this compiler.
    ///
    /// Reported only where the callee's contract **says** it can fail, which
    /// is what makes this need no guess: the ledger is the fact, and a callee
    /// no ledger describes says nothing here (the same silence `NK2502`'s
    /// method calls keep, C.4). So it never refuses a program that is right,
    /// and it grows as the ledger does.
    fn may_fail_here(&mut self, key: &str, contract: &FnContract, span: &Span) {
        // **Before the early return**, because the guard's question is asked of
        // exactly the calls this one declines to report: inside a `catch`,
        // `self.caught` is what sends this function home, and a call that
        // carries a `throws` is precisely what gives that `catch` something to
        // do ([ADR-091](../../../docs/specification/adr/adr-091.md)).
        if !contract.throws.is_empty() {
            if let Some(guarded) = &mut self.guarded {
                guarded.fallible = true;
            }
            self.newly_reaches_a_handler(key, span);
            // **And the lambda this may be inside**
            // ([ADR-102](../../docs/specification/adr/adr-102.md) D2). Here for
            // the same reason the line above is here: this is the one place
            // every written call — free or method — has its callee's contract
            // in hand, and a `catch` further out does not change what the
            // *lambda* was seen to do.
            if let Some(handed) = &mut self.handed_over {
                handed.fails = true;
                // **And `NK2606` is then the whole message.** Where the type
                // the lambda was handed to declares no failure, the function
                // *around* the lambda is not the one that has to answer for it
                // — the type is the contract, and telling the reader to write
                // `throws` on a function that does not fail would be a second
                // message for one mistake, in the wrong place. Where the type
                // **does** allow failing, the failure travels to the caller by
                // [ADR-029](../../docs/specification/adr/adr-029.md) D3 and
                // `NK2605` is right.
                if !handed.promised.may_fail {
                    return;
                }
            }
        }
        if contract.throws.is_empty() || self.throwing || self.caught {
            return;
        }
        // A grammar action is not code inside a function, and its failure does
        // not travel to one: past the `=>` it leaves the parser as the Nikaia
        // error it is and reaches the `catch` beside the `dsl`
        // ([ADR-023](../../../../docs/specification/adr/adr-023.md) D9). So
        // there is no `throws` to demand and no function to name.
        //
        // **Said here rather than by `current` being `None`**, which is what
        // said it until a rule's answers needed a key of their own to land
        // under ([ADR-186](../../../../docs/specification/adr/adr-186.md)):
        // the two facts are *this is not a
        // function* and *these calls belong to this entry*, and only the first
        // of them is this refusal's.
        if self.inside_an_action.is_some() {
            return;
        }
        // A `test` or a `bench` body is the other place with no function to
        // name, and there `current` is still `None`.
        let Some(function) = self.current.clone() else {
            return;
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2605",
            message: format!("this function can fail because `{key}` can fail"),
            notes: vec![
                format!(
                    "`{key}` carries `throws = {}` in the contracts this program is built \
                     against (Part III, 13.5)",
                    crate::contracts::throws_text(&contract.throws)
                ),
                "nothing marks a failing call, so a failure leaves at a call exactly as it \
                 leaves at a block's closing brace or a loop's step (ADR-023 D8, ADR-025 D1)"
                    .to_string(),
            ],
            help: Some(format!(
                "declare the error: add `throws` to `{function}` - or handle it at the \
                 call, `… catch {{ … }}` (Part I, 7.1)"
            )),
        });
    }

    /// **`NK2402`: an error that newly reaches a `catch` is named once**
    /// ([ADR-101](../../docs/specification/adr/adr-101.md) D1).
    ///
    /// A `catch` handles everything that reaches it and `throws` names no types
    /// at a signature, so the set arriving at a handler is **open**: it grows
    /// whenever a callee gains a failure. The handler is still a correct
    /// program and still handles the new error — as it handles everything. What
    /// was missing is not a refusal. It is that nobody was told.
    ///
    /// So this is a **warning**, printed and stopping nothing, and it is given
    /// **once**: the commit of the ledger diff is the acknowledgement (D2), and
    /// after it the new set is the baseline. No marker in the source, nothing
    /// to type at the handler.
    ///
    /// **A handler that matches and one that does not are treated alike** (D3),
    /// because the question is the same for all three shapes — is this new
    /// error right where it landed? What the author does about it is theirs,
    /// and both answers are legitimate.
    ///
    /// The caret is on the **call** rather than on the `catch`, because that is
    /// what the sentence is about: *this call brings something new in here*.
    fn newly_reaches_a_handler(&mut self, key: &str, span: &Span) {
        if self.guarded.is_none() {
            return;
        }
        let Some(gained) = self.newly.throws.get(key) else {
            return;
        };
        let named = list(&gained.iter().map(String::as_str).collect::<Vec<_>>());
        self.checked.findings.push(Finding {
            severity: Severity::Warning,
            span: span.clone(),
            code: "NK2402",
            message: format!("this `catch` receives {named} from `{key}` now"),
            notes: vec![
                format!(
                    "`{key}` did not carry {named} when the handler around this call was \
                     written, and a `catch` takes everything that reaches it - so it \
                     handles the new one as it handles everything (Part I, 7.1)"
                ),
                "nothing is wrong and nothing is refused: this is the note ADR-101 D1 asks \
                 for, and committing the ledger diff is what acknowledges it - after that \
                 the new set is the baseline and the note is not given again"
                    .to_string(),
            ],
            help: Some(
                "read it where it landed. A new arm in the handler and leaving it where it \
                 is are both answers; `nikaia build` without `--locked` records the new set"
                    .to_string(),
            ),
        });
    }

    /// **`NK2403`: a value's cleanup moved with a contract**
    /// ([ADR-236](../../docs/specification/adr/adr-236.md), ADR-094 D5).
    ///
    /// A callee that keeps what it is given runs its cleanup when it is done
    /// with it; one that only reads it lends it, and the cleanup runs at the end
    /// of the caller's block. The call site shows neither, which ADR-094 D5
    /// accepts - and what it owes for that is this: when a change to the callee
    /// moves the point, on a type whose teardown does something, every call
    /// that hands it a name is told once.
    ///
    /// A **warning**, as `NK2402` is, and for its reason: the program is
    /// correct either way, and committing the ledger diff is the
    /// acknowledgement. Only a **name**: a temporary's cleanup was at the end of
    /// its statement, and where inside a call that is, nobody can observe.
    fn a_cleanup_that_moved(
        &mut self,
        key: &str,
        param: &str,
        want: &Ty,
        given: Option<&Expr>,
        span: &Span,
    ) {
        let Some(moved) = self
            .newly
            .keeps
            .get(key)
            .and_then(|all| all.iter().find(|kept| kept.param == param))
        else {
            return;
        };
        let Some(Expr::Variable(value)) = given else {
            return;
        };
        let Some(how) = self.tears_down(want, &mut BTreeSet::new()) else {
            return;
        };
        let value = self.parsed.text(*value).to_string();
        let ty = &moved.ty;
        let (message, before, help) = match moved.now {
            true => (
                format!(
                    "`{key}` keeps `{param}` now, so `{value}`'s cleanup runs when `{key}` is \
                     done with it rather than at the end of this block"
                ),
                format!("`{key}` only read its `{param}` when this call was written"),
                format!(
                    "if the cleanup belongs here, hand `{key}` a copy - `{value}.clone()` - or \
                     have `{key}` read what it is given again"
                ),
            ),
            false => (
                format!(
                    "`{key}` only reads `{param}` now, so `{value}`'s cleanup runs at the end \
                     of this block rather than when `{key}` is done with it"
                ),
                format!("`{key}` kept its `{param}` when this call was written"),
                format!(
                    "if the cleanup belongs inside `{key}`, have it keep what it is given \
                     again; otherwise nothing is to be done here"
                ),
            ),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Warning,
            span: span.clone(),
            code: "NK2403",
            message,
            notes: vec![
                format!("{before}, and a `{ty}` does something when it is torn down ({how})"),
                "nothing is wrong and nothing is refused: this is the narration ADR-094 D5 \
                 owes for a cleanup point the call does not show, and committing the ledger \
                 diff acknowledges it - after that the note is not given again"
                    .to_string(),
            ],
            help: Some(help),
        });
    }

    /// **Whether a type's teardown does something**, and what says so
    /// (ADR-236 D1): an `impl Drop` or an `impl Cleanup` in any ledger this
    /// build reads, on the type or on the type of one of its fields.
    fn tears_down(&self, ty: &Ty, seen: &mut BTreeSet<String>) -> Option<String> {
        let Ty::Named {
            name, view: false, ..
        } = ty
        else {
            return None;
        };
        let base = crate::contracts::ty::base(name).to_string();
        if !seen.insert(base.clone()) {
            return None;
        }
        for teardown in ["Drop", "Cleanup"] {
            let written = [self.own, self.library].into_iter().any(|ledger| {
                ledger.implementations.get(teardown).is_some_and(|types| {
                    types.iter().any(|t| crate::contracts::ty::base(t) == base)
                })
            });
            if written {
                return Some(format!("`impl {teardown} for {base}`"));
            }
        }
        let fields: Vec<Ty> = [self.own, self.library]
            .into_iter()
            .flat_map(|ledger| {
                ledger
                    .types
                    .iter()
                    .filter(|(key, _)| crate::contracts::ty::base(key) == base)
                    .flat_map(|(_, contract)| contract.fields.iter().map(|f| f.ty.clone()))
            })
            .collect();
        fields.iter().find_map(|field| {
            self.tears_down(field, seen)
                .map(|how| format!("through its field of type `{}`, {how}", field.text()))
        })
    }

    /// **`NK2206` and `NK2606`: a handler that does more than its type allows**
    /// ([ADR-102](../../docs/specification/adr/adr-102.md) D2).
    ///
    /// A lambda that does **less** fits a type that allows more: one that never
    /// pauses goes where pausing is allowed, one that cannot fail goes where
    /// failing is. The other direction is the assertion
    /// [ADR-027](../../docs/specification/adr/adr-027.md) makes about a
    /// declaration, made about somebody else's code — a caller who writes
    /// `fn() sync` in a signature has promised their own callers something, and
    /// a handler that pauses takes the promise away without saying so.
    ///
    /// **Two codes and not one**, because the reader is doing two different
    /// things: `NK2206` is the shape `NK2202` has one level over — a body that
    /// pauses where the word says it does not — and `NK2606` is `NK2605`'s, a
    /// failure with nowhere declared to go.
    ///
    /// **Only where the ledger answered.** A call whose callee nothing
    /// describes leaves both flags where they were, which is the silence every
    /// other rule here keeps about an absent claim
    /// ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)): a
    /// refusal on a guess is a correct program refused, and it is the worse of
    /// the two mistakes.
    fn a_handler_that_does_more_than_the_type_allows(
        &mut self,
        promised: Promises,
        seen: Handed,
        span: &Span,
    ) {
        if seen.pauses && !promised.may_pause {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2206",
                message: "this lambda can pause, and the parameter it is given to says `sync`"
                    .to_string(),
                notes: vec![
                    "a function type says what the code it names may do, and `sync` is the \
                     same assertion a declaration makes - made about somebody else's code \
                     (ADR-102 D2, ADR-027)"
                        .to_string(),
                    "a lambda that does less fits a type that allows more, never the other \
                     way round"
                        .to_string(),
                ],
                help: Some(
                    "keep what pauses outside the lambda and hand its answer in - or take \
                     the `sync` off the parameter's type, which tells this function's own \
                     callers what changed"
                        .to_string(),
                ),
            });
        }
        if seen.fails && !promised.may_fail {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2606",
                message: "this lambda can fail, and the parameter it is given to does not say \
                          `throws`"
                    .to_string(),
                notes: vec![
                    "nothing marks a failing call, so a failure leaves at a call exactly as \
                     it leaves at a block's closing brace (ADR-023 D8) - and here there is \
                     nowhere for it to go, because the type it was handed to declares none \
                     (ADR-102 D2)"
                        .to_string(),
                ],
                help: Some(
                    "handle it here, `… catch { … }` - or write `throws` on the parameter's \
                     type, which is what says the callee has to answer for it"
                        .to_string(),
                ),
            });
        }
    }

    /// **What a lambda's body was seen to do, one call at a time**
    /// ([ADR-102](../../docs/specification/adr/adr-102.md) D2).
    ///
    /// The failing half is recorded in [`Self::may_fail_here`], where every
    /// written call already has its callee's contract in hand. This is the
    /// pausing half, and it needs its own line because only the *method* path
    /// records pausing for the emitter — a free call's `.await` is written from
    /// the name, which the emitter can resolve itself (ADR-028).
    ///
    /// **And `NK2202` inside a door** (Part II 12.2,
    /// [ADR-233](../../docs/specification/adr/adr-233.md) D3): a door's block
    /// runs with the lock open, and a pause there holds the lock for as long
    /// as the wait takes. `callee` is the call's name as the message says it.
    fn a_call_that_may_pause(&mut self, callee: &str, pauses: bool, span: &Span) {
        if !pauses {
            return;
        }
        let pause = Pause {
            what: format!("`{callee}`"),
            before_walk: format!(
                "call `{callee}` before the walk and hand the values in, or walk with \
                 `iter()`, whose lambda may pause"
            ),
            before_door: format!(
                "call `{callee}` before the door and hand its value in, or after it \
                 with what the block handed out"
            ),
        };
        self.a_pause(pause, span);
    }

    /// **A value's cleanup is a pause where the value dies**
    /// ([ADR-239](../../docs/specification/adr/adr-239.md) D4): the same rules
    /// as a written call, and the message names the value.
    fn a_cleanup_that_may_pause(&mut self, name: &str, span: &Span) {
        let pause = Pause {
            what: format!("the cleanup of `{name}`"),
            before_walk: format!(
                "bind `{name}` outside the walk and hand it in, or walk with `iter()`, \
                 whose lambda may pause"
            ),
            before_door: format!(
                "bind `{name}` outside the door, where it dies after the lock is released, \
                 or close it there"
            ),
        };
        self.a_pause(pause, span);
    }

    fn a_pause(&mut self, pause: Pause, span: &Span) {
        let Pause {
            what,
            before_walk,
            before_door,
        } = pause;
        if let Some(handed) = &mut self.handed_over {
            handed.pauses = true;
        }
        // **`NK2209` in a lambda that runs on several cores at once**
        // (Part II 12.6, ADR-235 D2): a core is not given up, so a pause there
        // holds one for as long as the wait takes.
        if self.in_parallel.is_some() {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2209",
                message: format!("{what} can pause, and this runs on several cores at once"),
                notes: vec![
                    "a lambda handed to a `par_iter()` walk is split across every core, \
                     and a core that waits holds its share of the work with it - so the \
                     lambda may not pause (Part II, 12.6)"
                        .to_string(),
                ],
                help: Some(before_walk),
            });
        }
        if !self.inside_a_door {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2202",
            message: format!("{what} can pause, and this runs with a lock held"),
            notes: vec![
                "a door's block runs with the lock open, and a pause there keeps it \
                 open for as long as the wait takes - every task that wants the \
                 value waits with it (Part II, 12.2)"
                    .to_string(),
            ],
            help: Some(before_door),
        });
    }

    /// **`NK2209`: a call that can pause, inside a grammar's action**
    /// ([ADR-142](../../docs/specification/adr/adr-142.md) D1).
    ///
    /// The demand [ADR-050](../../docs/specification/adr/adr-050.md) makes of an
    /// `overlap` branch and Part II 12.6 of a `par_iter` lambda, in the place a
    /// parser needs it: a parse that can be cut into pieces and run on several
    /// cores at once ([ADR-009](../../docs/specification/adr/adr-009.md)) is one
    /// whose steps do not wait on the world.
    ///
    /// **Asked where both call paths meet**, so the free call and the method
    /// call are answered by one rule - and **only where the ledger answered**,
    /// which is every other `sync` rule's convention
    /// ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)): a
    /// callee nothing describes is not refused, because a refusal on a guess is
    /// a correct program refused.
    ///
    /// Without it the emitter wrote `.await` inside the synchronous parser the
    /// `grammar!` macro generates, and the backend answered about it.
    fn a_pausing_call_in_an_action(&mut self, callee: &str, contract: &FnContract, span: &Span) {
        if contract.sync.is_sync() {
            return;
        }
        let Some(rule) = self.inside_an_action.clone() else {
            return;
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2209",
            message: format!("`{rule}`'s action calls `{callee}`, which can pause"),
            notes: vec![
                "a grammar's action may not pause (ADR-142 D1): a parser is computation \
                 over bytes that are already there, which is what makes `@frame`'s parallel \
                 parse sound (ADR-009) - and the generated parser is an ordinary function, \
                 so an `.await` inside it is not Rust either"
                    .to_string(),
            ],
            help: Some(
                "read what the parse needs before the parse and hand it in, or take the \
                 result apart afterwards - an action that waits on the world is a second \
                 pass wearing a grammar"
                    .to_string(),
            ),
        });
    }

    /// **`NK2202` for a method call** — the half `contracts::sync` cannot do
    /// ([ADR-027](../../docs/specification/adr/adr-027.md) D4,
    /// [ADR-149](../../docs/specification/adr/adr-149.md) D2).
    ///
    /// That analysis resolves a **free** call by name and stops at a method, on
    /// the ground that only a type checker knows what `tx.send(1)` goes to
    /// ([ADR-028](../../docs/specification/adr/adr-028.md)) — and it is right
    /// about that, which is why the rule is here instead. What it cost while
    /// nothing asked it: a function declaring `sync` and calling a pausing
    /// method lowered to an ordinary `fn` with an `.await` in its body, and the
    /// backend answered about a file nobody wrote (Part III, C.1).
    ///
    /// **Only where the ledger answered**, which is every other `sync` rule's
    /// convention ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)):
    /// a callee nothing describes is not refused, because a refusal on a guess
    /// is a correct program refused. An unresolved receiver never reaches here.
    fn a_pausing_method_in_a_sync_body(
        &mut self,
        callee: &str,
        contract: &FnContract,
        span: &Span,
    ) {
        if contract.sync.is_sync() {
            return;
        }
        let Some(caller) = self.inside_a_sync_function.clone() else {
            return;
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2202",
            message: format!("`{caller}` is `sync`, and `{callee}` can pause"),
            notes: vec![
                "a `sync` function promises it cannot pause and does no I/O (Part II, 12.1)"
                    .to_string(),
                format!("`{callee}` carries no `sync` in the contracts this program is built against (Part III, 13.5)"),
            ],
            help: Some(format!(
                "drop `sync` from `{caller}`, or move the call out of it"
            )),
        });
    }

    /// Where a method call stands in ADR-023 D8's propagation, for the emitter
    /// ([`Checked::fallible_methods`]).
    ///
    /// Every method call reaches this, and `fails` says whether the ledger
    /// describing its callee declares `throws`. A call whose receiver or whose
    /// method could not be resolved arrives with `false`, which is not "it
    /// cannot fail" but "there is no answer" - and it lands in the set that
    /// *removes* the pair, so an unresolved call leaves the statement's name
    /// alone rather than speaking for it.
    /// [`Checked::method_options`]: the callee's options, in the declaration's
    /// order, for the emitter to make positional.
    ///
    /// Recorded from the **contract** and not from the call, so a call that
    /// leaves an option out still gets its default — which is the half the
    /// emitter could not work out on its own either.
    fn method_options(&mut self, method: Ident, contract: &FnContract, span: &Span) {
        let Some(signature) = contract.signature.as_ref() else {
            return;
        };
        if signature.config.is_empty() {
            return;
        }
        let options = signature
            .config
            .iter()
            .map(|option| (option.name.clone(), option.default.clone()))
            .collect();
        self.checked
            .method_options
            .insert((span.start, self.parsed.text(method).to_string()), options);
    }

    fn method_propagates(&mut self, method: Ident, fails: bool, span: &Span) {
        let key = (span.start, self.parsed.text(method).to_string());
        if fails {
            self.fallible_methods.insert(key);
        } else {
            self.opaque_methods.insert(key);
        }
    }

    /// **What a task takes with it** (Part I 8.3, `NK2101`).
    ///
    /// Every name the body reads that is a local or a parameter **of the
    /// enclosing function**, with its type - because the type decides whether
    /// taking it is taking it away:
    ///
    ///   * **Data that is copied** - a number, a `bool`, a `char`, a view -
    ///     goes into the task and stays here too. Rust copies it, so there is
    ///     nothing to refuse, and `let n = 7` followed by a `spawn` that prints
    ///     `n` and a `println` that prints it again is a correct program.
    ///   * **A handle on a `Shared[T]`** is *duplicated* rather than moved
    ///     ([ADR-040](../../docs/specification/adr/adr-040.md) D1, D5), so the
    ///     name outside the task keeps working. Part I 8.3 says `NK2101`
    ///     belongs to the data case only, and this is where that holds.
    ///   * **A type nothing describes** is not claimed about at all. Refusing
    ///     one would be refusing on a guess (Part III, C.4).
    ///
    /// What is left is data that a move takes away, which is what the
    /// diagnostic is about.
    fn a_task_takes_these(&mut self, body: &Expr, span: &Span) {
        // The same walk `crosses_into_a_task` uses, and for the same reason: the
        // names a task's body reads are one question, asked once.
        for name in send::names_used(self.parsed, body) {
            let Some((ty, _)) = self.local(&name) else {
                continue;
            };
            // **A handle is duplicated into the task, not moved**
            // ([ADR-040](../../docs/specification/adr/adr-040.md) D1). It is
            // recorded rather than refused, because the emitter is what writes
            // the step and it has no types (ADR-028) - the same arrangement
            // every other answer this module hands over uses. Unconditional, and
            // not only where the name is used again (D2): a line further down
            // may not decide what a line further up does to a cleanup point.
            if is_a_handle(&ty) {
                self.checked.task_handles.insert((span.start, name.clone()));
            }
            if !moves_away(&ty) {
                continue;
            }
            self.moved_into_a_task.push((name, ty, span.end));
        }
    }

    /// `NK2101`: data a task took with it, used again afterwards.
    ///
    /// **The message `rustc` gave instead** is the reason this exists: *"borrow
    /// of moved value: `message`"*, with *"consider cloning the value before
    /// moving it into the closure"* - a closure the program does not have, about
    /// a file nobody wrote (Part III, C.1).
    ///
    /// An **assignment** between the two clears it, and that is not a leniency:
    /// `message = "other"` gives the name a value again, Rust accepts it, and
    /// refusing it would refuse a correct program (C.4).
    fn a_task_took_what_is_used_again(&mut self) {
        let moved = std::mem::take(&mut self.moved_into_a_task);
        let read = std::mem::take(&mut self.read_at);
        let written = std::mem::take(&mut self.written_at);

        for (name, ty, at) in moved {
            // At or after the byte the `spawn` statement **ends** on. The reads
            // inside the task's own body are the move itself and lie inside
            // that statement's span, so they are excluded; a statement span
            // runs up to the next one's first byte, so the next statement
            // starts exactly *at* the end and the comparison is not strict.
            let Some(&(_, used)) = read
                .iter()
                .filter(|(seen, when)| seen == &name && *when >= at)
                .min_by_key(|(_, when)| *when)
            else {
                continue;
            };
            if written
                .iter()
                .any(|(seen, when)| seen == &name && *when >= at && *when <= used)
            {
                continue;
            }
            self.checked.findings.push(Finding {
                code: "NK2101",
                severity: Severity::Error,
                span: Span {
                    start: used,
                    end: used,
                },
                message: format!("this background task takes ownership of `{name}`"),
                notes: vec![
                    format!(
                        "a task started with `spawn` may outlive this function, so it cannot \
                         merely borrow your variables - it takes them with it (Part I, 8.3). \
                         `{name}` is a `{ty}`, which a move takes away"
                    ),
                    "a number, a `bool`, a view or a handle on a `Shared[T]` would not be: \
                     the first three are copied and the last is duplicated (Part I, 6.2)"
                        .to_string(),
                ],
                help: Some(format!(
                    "clone before the task is built, and give the task the copy: \
                     `let copy = {name}.clone()`, then use `copy` inside the task"
                )),
            });
        }
    }

    /// Note that `iter`'s sequence was walked here, where it is a plain name
    /// ([ADR-105](../../docs/specification/adr/adr-105.md) D2).
    ///
    /// **A name and nothing else**, which is the narrowing `NK2101` has for the
    /// same reason: `map.keys().collect()` walks a temporary, and a temporary has
    /// no second use to refuse.
    fn a_sequence_is_walked(&mut self, iter: &Expr, over: &Ty, span: &Span) {
        if !matches!(over, Ty::Seq { shape, .. } if !shape.replays) {
            return;
        }
        let Expr::Variable(name) = iter else {
            return;
        };
        let name = self.parsed.text(*name).to_string();
        let taken = self.taken(name, over.clone(), self.read_seq, "", span);
        self.walked.push(taken);
    }

    /// **A key in the brackets of a map whose keys are owned**
    /// ([ADR-213](../../docs/specification/adr/adr-213.md) D1).
    ///
    /// The brackets went through `index::at` whatever was in them, and `at` is
    /// for a **position**: it turns a number into a `usize` and passes a view
    /// through. So a map keyed by an owned `String` could not be written with a
    /// `String` (no `at` for one) or with a literal (a `&str` where the map
    /// keeps a `String`), and a map keyed by an `i64` had its key turned into a
    /// `usize` - each `rustc`'s words about a file nobody wrote.
    ///
    /// A key is not a position. **Written**, the map keeps it: it goes in by
    /// value, a literal is built into text of its own where it stands (as
    /// ADR-207 does for every other place that keeps text), a name is handed
    /// over (`NK2105` if it is used again), and a **view** is refused with the
    /// sentence ADR-208 has for text kept where only a view was given.
    /// **Read**, the map keeps nothing: the key is lent, or goes in as it is
    /// where it is a view already.
    ///
    /// A map keyed by **views** is left exactly as it was: every example keys
    /// its maps that way, and a view goes through `at` untouched.
    fn a_map_key(&mut self, keys: &Ty, found: &Ty, index: &Expr, writing: bool, span: &Span) {
        // **A map whose key type nothing pinned is keyed by what goes in the
        // brackets** (0.0.235, `open-work.md` §1.21). `collections::HashMap()`
        // with no annotation has `?` keys, and the brackets went through `at`,
        // which made `m[1] = 2` a map keyed by `usize` and `m[k]` a read the
        // language below refused. Where the key is a known value that is not
        // a view, it is the map's key; a view or a key of no known type goes
        // through `at` as before, which is the path every map of views takes.
        // **A map whose key type nothing pinned** (0.0.235, `open-work.md`
        // §1.21). `collections::HashMap()` with no annotation has `?` keys,
        // and the brackets went through `at`, which is for a **position**: it
        // made `m[1] = 2` a map keyed by `usize` and `m[k]` a read the
        // language below refused. A key is not a position, and the receiver is
        // known to be a map, so neither side goes through `at` any more: a
        // write hands the key over as it is, and a read of a key that is not
        // already a view goes through `index::key`, which lends a number and
        // passes a view through - the one spelling that is right whatever the
        // map turns out to be keyed by, since this checker cannot say.
        if keys.is_unknown() {
            let form = match (writing, found.is_a_view()) {
                (true, _) => KeyForm::Handed,
                (false, true) => KeyForm::AsIs,
                (false, false) => KeyForm::Either,
            };
            self.checked
                .map_keys
                .insert((span.start, argument_shape(index)), form);
            return;
        }
        if keys.is_a_view() {
            return;
        }
        let form = match writing {
            true => {
                if self.text_literal(keys, index, true).is_none() {
                    self.expect_kept(
                        found,
                        keys,
                        Some(index),
                        "a map keeps its keys",
                        span.clone(),
                        "assign",
                        |found, want| {
                            format!("this key is `{found}`, and the map's keys are `{want}`")
                        },
                    );
                    self.hands_over(index, found, "written into the map as a key", span);
                }
                KeyForm::Handed
            }
            false => match found.is_a_view() || matches!(index, Expr::LitStr { .. }) {
                true => KeyForm::AsIs,
                false => KeyForm::Lent,
            },
        };
        self.checked
            .map_keys
            .insert((span.start, argument_shape(index)), form);
    }

    /// **`NK1189`: a copy under another name than `.clone()`**
    /// ([ADR-216](../../docs/specification/adr/adr-216.md) D1).
    ///
    /// `.to_owned()` is the language below's name for a copy of a view -
    /// needed there because `.clone()` of a reference copies the reference.
    /// This language has no reference to copy: a copy of text is text of its
    /// own, whatever the text was, and the compiler writes whichever call the
    /// language below needs (ADR-215 D4). So there is one word, and the other
    /// is refused naming it, as `..=` and `Type::new` were.
    ///
    /// **`.to_string()` is not this**, and stays: it is *the text form of a
    /// value*, for every type, which for text happens to be a copy. Refused on
    /// text alone, one expression would be right or wrong by its receiver's
    /// type - `x.to_string()` in a body generic over `x` among them.
    ///
    /// Not where the program declares a method of that name itself.
    fn a_copy_under_another_name(
        &mut self,
        receiver: &Expr,
        method: Ident,
        args: &[Expr],
        span: &Span,
    ) {
        let written = self.parsed.text(method);
        if !args.is_empty() || !self.own.candidates(written).is_empty() {
            return;
        }
        if written != "to_owned" {
            return;
        }
        let shown = match receiver {
            Expr::Variable(name) => self.parsed.text(*name).to_string(),
            _ => "…".to_string(),
        };
        self.checked.findings.push(Finding {
            code: "NK1189",
            severity: Severity::Error,
            span: span.clone(),
            message: format!("a copy is written `.clone()`, not `.{written}()`"),
            notes: vec![
                "Nikaia has one word for a copy, for text and for everything else: a copy \
                 of text is text of its own, whatever it was copied from, and the compiler \
                 writes what the language below needs for it (ADR-216)"
                    .to_string(),
            ],
            help: Some(format!("write `{shown}.clone()`")),
        });
    }

    /// **A value handed to something that keeps it**
    /// ([ADR-213](../../docs/specification/adr/adr-213.md) D3, which builds
    /// [ADR-094](../../docs/specification/adr/adr-094.md) D2's last paragraph).
    ///
    /// Called at the places a value is given away rather than lent: an argument
    /// the callee keeps, a key or a value written into a map or a list, a field
    /// of a struct being built, an element of a list or a tuple, a `let` that
    /// names it again and an assignment. Only a **name**, and only data a move
    /// takes away - a number, a `bool`, a `char` and a view are copied
    /// (`moves_away`, the rule `NK2101` has for a task).
    ///
    /// What is recorded is asked at the end of the body, as `NK2702` is: a later
    /// read on the same path is `NK2105`. And, as for a sequence, a name from
    /// outside a loop or a lambda handed over inside it is refused where it is
    /// handed, because the next turn hands over what is already gone.
    fn hands_over(&mut self, value: &Expr, ty: &Ty, to: &str, span: &Span) {
        // `name.to_string()` is `name` (ADR-216 D4): what is handed over is it.
        if let Expr::MethodCall { receiver, .. } = value
            && self
                .checked
                .text_as_is
                .contains(&(span.start, argument_shape(receiver)))
        {
            return self.hands_over(receiver, ty, to, span);
        }
        let Some(&at) = self.read_index.get(&(address(value), span.start)) else {
            return;
        };
        if !matches!(value, Expr::Variable(_) | Expr::Field { .. }) {
            return;
        }
        let (path, seq) = (
            self.reads_on_paths[at].path.clone(),
            self.reads_on_paths[at].seq,
        );
        self.hands_over_path(path, ty, seq, to, span);
    }

    /// The same for `P { name }`, whose field is its name and which no read
    /// stands for.
    fn hands_over_name(&mut self, name: &str, to: &str, span: &Span) {
        let Some(ty) = self.lookup(name) else {
            return;
        };
        self.hands_over_path(name.to_string(), &ty, self.read_seq, to, span);
    }

    fn hands_over_path(&mut self, path: String, ty: &Ty, seq: usize, to: &str, span: &Span) {
        // A task's body is `NK2101`'s: what it uses it took with it already.
        if !self.task_bindings.is_empty() {
            return;
        }
        // A stamp is not data: what is under it decides (ADR-111 D2).
        if !moves_away(&ty.unseen()) {
            return;
        }
        let root = path.split('.').next().unwrap_or_default().to_string();
        if path.contains('.') && self.lent_here(&root) {
            self.a_part_of_a_loan_handed_over(&path, &root, to, span);
            return;
        }
        let taken = self.taken(path.clone(), ty.clone(), seq, to, span);
        self.handed.push(taken);
        let Some(bound) = self
            .scope
            .iter()
            .rposition(|frame| frame.iter().any(|local| local.name == root))
        else {
            return;
        };
        let Some(around) = self
            .repeats
            .iter()
            .rev()
            .take_while(|r| r.frame > bound)
            .find(|r| r.takes_again(&root))
        else {
            return;
        };
        let again = match around.what {
            "loop" => "the next turn of the loop hands it over again, and it is gone",
            _ => "a lambda may be run more than once, and the second run finds it gone",
        };
        let what = around.what;
        self.checked.findings.push(Finding {
            code: "NK2105",
            severity: Severity::Error,
            span: span.clone(),
            message: format!("`{path}` is {to}, inside a {what} it was declared outside of"),
            notes: vec![
                format!(
                    "`{path}` is a `{ty}`, and what keeps a value is given it rather than a \
                     copy - {again}"
                ),
                "a number, a `bool`, a `char` and a view would not be: they are copied".to_string(),
            ],
            help: Some(format!(
                "hand over a copy each time: `{path}.clone()` where it is handed over"
            )),
        });
    }

    /// Whether a name is only **lent** to this body: a parameter or a `self`
    /// declared a view, a `for` binding over a place, a `let` over one.
    fn lent_here(&self, name: &str) -> bool {
        let Some(frame) = self
            .scope
            .iter()
            .rposition(|frame| frame.iter().any(|local| local.name == name))
        else {
            return false;
        };
        if self.lent_lets.contains(&(name.to_string(), frame)) {
            return true;
        }
        self.binding(name)
            .is_some_and(|local| local.lent || local.ty.is_a_view())
    }

    /// **`NK2106`: a part of something lent, handed to what keeps it**
    /// ([ADR-214](../../docs/specification/adr/adr-214.md) D2).
    ///
    /// `xs.push(p.name)` with `p` a view takes the field out of a value this
    /// body does not own: the language below said *cannot move out of
    /// `p.name` which is behind a shared reference*, about a file nobody wrote.
    fn a_part_of_a_loan_handed_over(&mut self, path: &str, root: &str, to: &str, span: &Span) {
        self.checked.findings.push(Finding {
            code: "NK2106",
            severity: Severity::Error,
            span: span.clone(),
            message: format!("`{path}` is {to}, and `{root}` is only lent here"),
            notes: vec![
                format!(
                    "`{root}` belongs to whoever lent it - a `ref` parameter, a `for` over a \
                     list, a `let` over a place - so a part of it cannot be given away: the \
                     owner still has it"
                ),
                "what keeps a value is given it rather than a copy, and a copy is the \
                 program's to write (ADR-005 §3, ADR-094 D2)"
                    .to_string(),
            ],
            help: Some(format!("hand over a copy: `{path}.clone()`")),
        });
    }

    /// A place as a path - `name`, `p.name`, `a.b.c` - where it is one.
    fn place_path(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Variable(name) => Some(self.parsed.text(*name).to_string()),
            Expr::Field { base, name } => Some(format!(
                "{}.{}",
                self.place_path(base)?,
                self.parsed.text(*name)
            )),
            _ => None,
        }
    }

    /// A read of a name, recorded once per expression and statement.
    fn a_read(&mut self, expr: &Expr, path: String, span: &Span) {
        let key = (address(expr), span.start);
        if self.read_index.contains_key(&key) {
            return;
        }
        self.read_seq += 1;
        self.read_index.insert(key, self.reads_on_paths.len());
        self.reads_on_paths.push(Read {
            path,
            at: span.start,
            choices: self.branch.clone(),
            seq: self.read_seq,
        });
    }

    /// `p.name` extends the read of `p` it stands on: what is read is the part
    /// ([ADR-214](../../docs/specification/adr/adr-214.md) D2).
    fn a_read_of_a_part(&mut self, base: &Expr, expr: &Expr, field: &str, span: &Span) {
        // Once per expression, as the read it extends: an argument walked a
        // second time would otherwise read `self.path.path`.
        if self.read_index.contains_key(&(address(expr), span.start)) {
            return;
        }
        let Some(&at) = self.read_index.get(&(address(base), span.start)) else {
            return;
        };
        let read = &mut self.reads_on_paths[at];
        read.path.push('.');
        read.path.push_str(field);
        self.read_index.insert((address(expr), span.start), at);
    }

    /// A taking, where it stands.
    fn taken(&self, path: String, ty: Ty, seq: usize, to: &str, span: &Span) -> Taken {
        Taken {
            path,
            ty,
            from: span.start,
            at: span.end,
            seq,
            choices: self.branch.clone(),
            until: None,
            to: to.to_string(),
        }
    }

    /// **The first read after a taking, on a path that passes both**
    /// ([ADR-212](../../docs/specification/adr/adr-212.md) D4,
    /// [ADR-213](../../docs/specification/adr/adr-213.md) D4,
    /// [ADR-214](../../docs/specification/adr/adr-214.md) D1): in a later
    /// statement, or later in the same one; not in another arm of a choice the
    /// taking stood in; not past a branch that left the function; and touching
    /// what was taken. `None` where an assignment gave it back first - in the
    /// same statement too, since `name = f(name)` writes after it reads.
    fn first_read_after(&self, taken: &Taken) -> Option<(usize, String)> {
        let read = self
            .reads_on_paths
            .iter()
            .filter(|read| {
                overlaps(&read.path, &taken.path)
                    && !apart(&taken.choices, &read.choices)
                    && taken.until.is_none_or(|end| read.at < end)
                    && (read.at >= taken.at || (read.at == taken.from && read.seq > taken.seq))
            })
            .min_by_key(|read| (read.at, read.seq))?;
        let used = read.at;
        let given_back = self.written_at.iter().any(|(written, when)| {
            revives(written, &taken.path) && *when >= taken.from && *when <= used
        });
        (!given_back).then(|| (used, read.path.clone()))
    }

    /// Where a walk or a hand-over recorded from here on stands.
    fn taken_so_far(&self) -> (usize, usize) {
        (self.walked.len(), self.handed.len())
    }

    /// **A branch that leaves the function takes its takings with it**
    /// ([ADR-213](../../docs/specification/adr/adr-213.md) D4).
    ///
    /// `if ended == 0 { refuse(connection, 408, …) return }` hands the
    /// connection over and leaves, so the `refuse(connection, 400, …)` after the
    /// `if` is not after that hand-over on any path that runs - which the order
    /// of statements alone cannot see, and the `http` example is written in
    /// exactly this shape. What was taken inside such a branch is asked about
    /// reads up to the end of the statement the branch is in, and no further.
    fn a_branch_that_leaves(&mut self, from: (usize, usize), exits: bool, span: &Span) {
        if !exits {
            return;
        }
        for walked in &mut self.walked[from.0..] {
            walked.until.get_or_insert(span.end);
        }
        for handed in &mut self.handed[from.1..] {
            handed.until.get_or_insert(span.end);
        }
    }

    /// **`NK2105`: data used after it was handed to something that keeps it**
    /// ([ADR-213](../../docs/specification/adr/adr-213.md) D3).
    ///
    /// `rustc` said *borrow of moved value* about a file nobody wrote. The
    /// shape is `NK2101`'s: the first read after the hand-over on a path that
    /// passes both, unless an assignment in between gave the name a value
    /// again.
    fn data_handed_over_is_used_again(&mut self) {
        let handed = std::mem::take(&mut self.handed);
        let mut said: BTreeSet<usize> = BTreeSet::new();
        let mut findings = Vec::new();
        for taken in handed {
            let Some((used, read)) = self.first_read_after(&taken) else {
                continue;
            };
            let Taken {
                path: name, ty, to, ..
            } = taken;
            // The part read where the whole was taken, or the whole where a
            // part was: the sentence says which (ADR-214 D2).
            let message = match read == name {
                true => format!("`{name}` was {to}, and is used here again"),
                false => format!("`{read}` is used here, and `{name}` was {to}"),
            };
            if !said.insert(used) {
                continue;
            }
            findings.push(Finding {
                code: "NK2105",
                severity: Severity::Error,
                span: Span {
                    start: used,
                    end: used,
                },
                message,
                notes: vec![
                    format!(
                        "`{name}` is a `{ty}`, and what keeps a value is given it rather than \
                         a copy: a copy costs as much as the value is large, and one made on \
                         its own would run every time with nothing in the source to show it \
                         (ADR-005 §3, ADR-094 D2)"
                    ),
                    "a number, a `bool`, a `char` and a view would not be: they are copied"
                        .to_string(),
                ],
                help: Some(format!(
                    "if both need it, hand over a copy: `{name}.clone()` where it is \
                     handed over"
                )),
            });
        }
        self.checked.findings.extend(findings);
    }

    /// **A name that holds a sequence, read where the read takes it**
    /// ([ADR-212](../../docs/specification/adr/adr-212.md) D4).
    ///
    /// Every read of one takes it, because a sequence has nothing that looks
    /// at it without walking it: a `let` that names it again moves it, an
    /// argument hands it over, a `return` hands it out, and every one of its
    /// methods walks it. Before this only a `for` and a method call counted, so
    /// `let t = lines` followed by `lines.count()` went to `rustc` as *use of
    /// moved value* about a file nobody wrote. The two that do not take are an
    /// assignment's target and a `&`, which `reading_only` marks.
    ///
    /// **A range replays** (D3) and is never taken.
    ///
    /// And one the per-statement order cannot see: a name from **outside** a
    /// loop or a lambda taken **inside** it is taken again on the next turn or
    /// the next call. Refused where it is taken, unless the loop may leave on
    /// that turn or gives the name a new sequence first.
    fn a_sequence_is_taken(&mut self, name: &str, ty: &Ty, span: &Span) {
        if self.reading_only || !matches!(ty, Ty::Seq { shape, .. } if !shape.replays) {
            return;
        }
        let taken = self.taken(name.to_string(), ty.clone(), self.read_seq, "", span);
        self.walked.push(taken);
        let Some(bound) = self
            .scope
            .iter()
            .rposition(|frame| frame.iter().any(|local| local.name == name))
        else {
            return;
        };
        let Some(around) = self
            .repeats
            .iter()
            .rev()
            .take_while(|r| r.frame > bound)
            .find(|r| r.takes_again(name))
        else {
            return;
        };
        let item = match ty {
            Ty::Seq { item, .. } => item.text(),
            _ => "?".to_string(),
        };
        let (again, help) = match around.what {
            "loop" => (
                "the next turn of the loop takes it again, and it is gone",
                format!(
                    "collect it before the loop and walk the collection: \
                     `let {name} = {name}.collect()`"
                ),
            ),
            _ => (
                "a lambda may be run more than once by what it is handed to, \
                 and the second run finds it gone",
                format!(
                    "collect it before the lambda and walk the collection inside: \
                     `let {name} = {name}.collect()`"
                ),
            ),
        };
        self.checked.findings.push(Finding {
            code: "NK2702",
            severity: Severity::Error,
            span: span.clone(),
            message: format!(
                "`{name}` is a sequence, taken inside a {} it was declared outside of",
                around.what
            ),
            notes: vec![
                format!(
                    "a sequence of `{item}` produces its elements as they are asked \
                     for, so walking it consumes it - and {again}"
                ),
                "a `Vec` is not this: a container has its elements already and is \
                 walked by view, as often as you like"
                    .to_string(),
            ],
            help: Some(help),
        });
    }

    /// **`NK2702`: a sequence walked a second time**
    /// ([ADR-105](../../docs/specification/adr/adr-105.md) D2).
    ///
    /// A produced sequence is walked **once**: the walk takes it by value, which
    /// is ADR-094 D2's *a value walked is a value kept*, and there is nothing
    /// left afterwards. A container is walked by view and as often as one likes,
    /// which is why nothing here asks about a `Vec`.
    ///
    /// **`NK2101`'s analysis with one word changed**, deliberately: the shape is
    /// the same question — a name used after something took it — and an
    /// assignment in between **revives** it, because giving the name a value
    /// again is a correct program.
    fn a_sequence_was_walked_twice(&mut self) {
        let walked = std::mem::take(&mut self.walked);
        let mut said: BTreeSet<usize> = BTreeSet::new();
        let mut findings = Vec::new();

        for taken in walked {
            let Some((used, _)) = self.first_read_after(&taken) else {
                continue;
            };
            let (name, ty) = (taken.path, taken.ty);
            // Once per site: a name walked twice and read three times is one
            // mistake, and three carets on it is three readings of the same line.
            if !said.insert(used) {
                continue;
            }
            let item = match &ty {
                Ty::Seq { item, .. } => item.text(),
                _ => "?".to_string(),
            };
            findings.push(Finding {
                code: "NK2702",
                severity: Severity::Error,
                span: Span {
                    start: used,
                    end: used,
                },
                message: format!("`{name}` is a sequence that was already walked"),
                notes: vec![
                    format!(
                        "a sequence of `{item}` produces its elements as they are asked \
                         for, so walking it consumes it - a `for`, a `collect()`, a \
                         `count()` and every other walk takes it by value (ADR-094 D2)"
                    ),
                    "a `Vec` is not this: a container has its elements already and is \
                     walked by view, as often as you like"
                        .to_string(),
                ],
                help: Some(format!(
                    "collect it first and walk the collection: \
                     `let {name} = {name}.collect()`"
                )),
            });
        }
        self.checked.findings.extend(findings);
    }

    /// **`NK2104`: two branches of an `overlap` meet on something**
    /// ([ADR-050](../../docs/specification/adr/adr-050.md) D3).
    ///
    /// **This is [ADR-033](../../docs/specification/adr/adr-033.md)'s analysis
    /// used the other way round**, and it is the return on machinery built for
    /// an inference that D1 withdraws. The touch sets no longer decide *whether
    /// the compiler may* overlap two statements; they decide *whether the
    /// programmer was right* to say so. Every mainstream form for "run these
    /// together" takes the programmer's word for the independence — here the
    /// claim is checked against what the ledger records each call touching.
    ///
    /// **A branch this compiler cannot account for is not refused.** `verdict`
    /// answers `NotAccountedFor` where a statement performs nothing it can
    /// name, and reading that as "they meet" would refuse a correct program on
    /// an absence (Part III, C.4) — which is the same polarity every other
    /// answer in this compiler takes about a thing nobody wrote down.
    fn branches_meet_on_nothing(&mut self, block: &Block, span: &Span) {
        use crate::contracts::order;

        // A branch that **binds** is refused on its own: the block's value
        // already carries every branch's result, so `let x = …` inside one
        // would name a thing that leaves by two doors (D2).
        for stmt in &block.stmts {
            if let Stmt::Let { names, .. } = &stmt.node {
                let bound: Vec<String> = names
                    .iter()
                    .map(|n| self.parsed.text(*n).to_string())
                    .collect();
                let name = bound.join("`, `");
                self.checked.findings.push(Finding {
                    code: "NK2104",
                    severity: Severity::Error,
                    span: stmt.span.clone(),
                    message: format!("a branch of an `overlap` binds `{name}`"),
                    notes: vec![
                        "each statement in the block is a branch, and the block's value is \
                         the tuple of their results in written order (Part I, 8.1.2)"
                            .to_string(),
                    ],
                    help: Some(format!(
                        "take the value from the block instead: \
                         `let ({name}, …) = overlap {{ … }}`"
                    )),
                });
            }
        }

        let operations: Vec<Option<order::Operation>> = block
            .stmts
            .iter()
            .map(|stmt| order::operation(self.parsed, &stmt.node, self.own, self.library))
            .collect();

        for (i, earlier) in operations.iter().enumerate() {
            for (j, later) in operations.iter().enumerate().skip(i + 1) {
                let (Some(earlier), Some(later)) = (earlier, later) else {
                    continue;
                };
                let verdict = order::verdict(earlier, later);
                if verdict.is_overlap() {
                    continue;
                }
                self.checked.findings.push(Finding {
                    code: "NK2104",
                    severity: Severity::Error,
                    span: block.stmts[j].span.clone(),
                    message: "these two branches cannot run together".to_string(),
                    notes: vec![
                        format!("{} (Part I, 8.1.2)", verdict.why()),
                        "`overlap` says the branches have no order between them, and two \
                         that meet on something do"
                            .to_string(),
                    ],
                    help: Some(
                        "if you meant them in order, write them as ordinary statements".to_string(),
                    ),
                });
                // One finding per branch: a branch that meets two others has
                // one thing wrong with it, and three messages about it is the
                // same mistake told three times.
                break;
            }
        }
        let _ = span;
    }

    /// `NK2103`: a `spawn`'s lambda names an argument, and a task is handed
    /// nothing.
    ///
    /// `spawn fn (x) { … }` reads as though `x` arrives from somewhere, and
    /// nothing gives it to a task (Part I 8.2). Dropping the name silently would
    /// be the worse answer: the body would then refer to something nothing
    /// declared, which `NK1117` reports about a name the author *did* write.
    fn a_task_takes_no_arguments(&mut self, params: &[Ident], span: &Span) {
        let Some(first) = params.first() else {
            return;
        };
        let named = params
            .iter()
            .map(|p| format!("`{}`", self.parsed.text(*p)))
            .collect::<Vec<_>>()
            .join(", ");
        self.checked.findings.push(Finding {
            code: "NK2103",
            severity: Severity::Error,
            span: span.clone(),
            message: format!(
                "this task's lambda names {named}, and a task is handed nothing (Part I, 8.2)"
            ),
            notes: vec![
                "`spawn` starts a body, it does not call it with arguments - what the body \
                 needs, it takes from around it, and `spawn` moves those in (Part I, 8.3)"
                    .to_string(),
            ],
            help: Some(format!(
                "drop the argument list: `spawn fn {{ … }}`. Where `{}` was meant to be a \
                 value from here, name it before the task and use it inside",
                self.parsed.text(*first)
            )),
        });
    }

    /// The same, for the `.await` ([`Checked::pausing_methods`]).
    ///
    /// Separate from `method_propagates` because the two answers are separate:
    /// a call can fail without pausing and pause without failing, and the two
    /// sets are narrowed independently. Both are recorded at every method call,
    /// including the ones nothing could be established about - `false` there is
    /// "there is no answer", and it lands in the set that *removes* the pair.
    fn method_pauses(&mut self, method: Ident, pauses: bool, span: &Span) {
        let key = (span.start, self.parsed.text(method).to_string());
        if pauses {
            self.pausing_methods.insert(key);
        } else {
            self.settled_methods.insert(key);
        }
    }

    /// Whether this method call is an **eager** walk of a sequence whose step
    /// can fail ([ADR-025](../../docs/specification/adr/adr-025.md) D1).
    ///
    /// Asked twice — once for the `?` the emitter writes and once for the
    /// refusal — so it is one sentence rather than two that have to agree.
    fn walks_a_failing_sequence(&self, on: &Ty, contract: &FnContract) -> bool {
        matches!(on, Ty::Seq { throws: true, .. })
            && walks_by_value(contract)
            && !matches!(
                contract.signature.as_ref().and_then(|s| s.result.as_ref()),
                Some(Ty::Seq { .. })
            )
    }

    /// `NK2701` for a **walk** rather than for a loop
    /// ([ADR-025](../../docs/specification/adr/adr-025.md) D1).
    ///
    /// The same rule and the same code as a `for` over the same sequence: a
    /// step that can fail fails the function around it, and nothing marks the
    /// call. The message says *a step of what it walks* rather than *a turn of
    /// this loop*, because that is what a reader is looking at.
    ///
    /// **It is the half whose absence was a wrong answer.** `io::lines().count()`
    /// used to compile and count the failures as lines, because the entry said
    /// `-> i64` and nothing asked what a step of the receiver does.
    fn a_walk_of_a_failing_sequence(&mut self, method: Ident, span: &Span) {
        if self.throwing || self.caught || self.inside_an_action.is_some() {
            return;
        }
        let Some(function) = self.current.clone() else {
            return;
        };
        let name = self.parsed.text(method).to_string();
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2701",
            message: format!("this function can fail because a step of what `{name}` walks can fail"),
            notes: vec![format!(
                "`{name}` asks the sequence for every element, and a step of this one reads as it goes - so the failure leaves this function exactly as a `for` over the same sequence would (ADR-025 D1)"
            )],
            help: Some(format!(
                "declare the error: add `throws` to `{function}` - or handle it at the call, `… catch {{ … }}` (Part I, 7.1)"
            )),
        });
    }

    /// A loop over something whose step **pauses**
    /// ([ADR-172](../../docs/specification/adr/adr-172.md) D1).
    ///
    /// One thing follows and it is not a refusal: the emitter writes the loop
    /// as one that gives its thread up. Nothing is reported, because nothing is
    /// wrong — a `for` over a stream is an ordinary program and the whole of
    /// D1 is that it stops holding a thread it does not need.
    ///
    /// **The positive word and not the absence of `sync`**, which is the
    /// decision this reads rather than a shortcut into it: the absence is
    /// *nobody said*, a `map`'s step is exactly that, and writing `.await` on
    /// it would be `rustc` refusing a correct program about a file nobody wrote
    /// (Part III, C.1 and C.4).
    fn pausing_step(&mut self, over: &Ty, span: &Span) {
        if matches!(over, Ty::Seq { pauses: true, .. }) {
            self.checked.pausing_loops.insert(span.start);
            // A lambda around the loop pauses with it, and a door around it
            // is held across the wait (ADR-233 D1, D3).
            self.a_call_that_may_pause("a step of this `for`", true, span);
        }
    }

    /// A loop over something whose step can fail (ADR-025 D1).
    ///
    /// Two things follow, and they are the two halves of the decision: the
    /// enclosing function must declare `throws`, and the emitter has to make
    /// the step propagate. This records the second and reports the first.
    fn fallible_step(&mut self, over: &Ty, bindings: usize, span: &Span) {
        // **Two spellings of one claim, and they are the same claim.**
        // `[type."io::Lines"] iterates = "throws"` says it of a named type, and
        // [ADR-105](../../docs/specification/adr/adr-105.md) D1's `throws` after
        // a `Seq[T]` says it of the sequence a signature hands back — *`throws`
        // on a `Seq` is the one spelling of a step can fail*, which is that
        // record's §3. The second is the one a produced sequence can use,
        // because it has no name in the `types` table to hang a column on.
        // The name a message uses is the **ledger's** for a named type and a
        // description for a `Seq`, because D4 says a program cannot write
        // `Seq[String]` — and a message that names a spelling its reader has no
        // way to type is the shape Part III C.1 is about.
        let name = match over {
            Ty::Named { name, .. } if self.iterates_fallibly(name) => format!("`{name}`"),
            Ty::Seq {
                throws: true, item, ..
            } => match &**item {
                Ty::Unknown => "a sequence".to_string(),
                item => format!("a sequence of `{item}`"),
            },
            _ => return,
        };
        let name = &name;

        // A stream of pairs does not exist in `std`, and taking one apart while
        // also unwrapping a failure is a shape to design rather than to guess
        // at (ADR-025 §7).
        if bindings != 1 {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2701",
                message: format!("a `for` over {name} binds one name, and this binds {bindings}"),
                notes: vec![format!(
                    "each turn of {name} can fail, and the failure is what the one binding unwraps"
                )],
                help: Some("bind one name and take the pair apart inside the loop".to_string()),
            });
            return;
        }

        self.checked.fallible_loops.insert(span.start);

        if self.throwing {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2701",
            message: "this function can fail because a turn of this loop can fail".to_string(),
            notes: vec![format!(
                "{name} reads as it goes, and a read can fail - so the failure leaves this \
                 function, exactly as a failing call would"
            )],
            help: Some("declare the error: add `throws` to this function".to_string()),
        });
    }

    /// **Part I 9.2, for a field** (`NK1110`).
    ///
    /// A type's fields are private to its package unless they say `pub`, and
    /// until the ledger recorded that ([`FieldContract`]) a type whose fields
    /// were private could be built by name from another package with nothing
    /// saying no. The language below cannot help here the way it does for an
    /// item: the emitted struct is in the **same crate**, so `pub` on a field
    /// buys nothing there ([ADR-047](../../../../docs/specification/adr/adr-047.md)
    /// D2).
    ///
    /// Only for a **qualified** type, which is the same spelling rule
    /// [`Checker::reachable`] uses: a type named `http::Request` is another
    /// package's, and one named `Request` is this package's, where every field is
    /// visible however it is declared.
    fn field_is_reachable(&mut self, ty: &str, field: &FieldContract, span: &Span) {
        if field.public {
            return;
        }
        let Some((package, _)) = ty.trim_start_matches('&').split_once("::") else {
            return;
        };
        if !self.modules.contains(package) {
            return;
        }
        let name = field.name.clone();
        let ty = ty.to_string();
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1110",
            message: format!("`{ty}.{name}` is private to `{package}`"),
            notes: vec![
                "a field is private to the package that declares its type unless it says \
                 `pub` (Part I, 9.2)"
                    .to_string(),
            ],
            help: Some(format!(
                "write `pub {name}` in `{package}`, or reach it through something that is \
                 public - a type may keep its fields private and offer methods (Part I, 9.3)"
            )),
        });
    }

    /// Part I 9.2: an item is private to its **package** unless it says `pub`.
    ///
    /// The language below enforces this too - `pub` becomes `pub` - but a reader
    /// should not meet the rule as a `rustc` message about a file they did not
    /// write, which is what Part III C.1 calls a bug in this compiler.
    ///
    /// Only a call written `package::item` can leave the package: a call inside
    /// it writes `secret()`, unqualified. So this needs no notion of "which
    /// package am I in" - the spelling says it.
    ///
    /// **No program reaches this today**, and that is [ADR-047](../../../../docs/specification/adr/adr-047.md)
    /// D1 rather than an oversight. The boundary used to be the file, and it is
    /// the package now: the files of one package share a namespace, so a
    /// qualified name is a name from *another* package - and depending on one is
    /// not built (D2). The check stays because the boundary it is about is the
    /// one that is left, and it is the day a package arrives that a private name
    /// needs refusing.
    fn reachable(&mut self, name: &str, contract: &FnContract, span: &Span) {
        let Some((module, item)) = name.split_once("::") else {
            return;
        };
        if !self.modules.contains(module) || contract.public {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1110",
            message: format!("`{item}` is private to `{module}`"),
            notes: vec!["an item is private to the package that declares it unless it says `pub` (Part I, 9.2)".to_string()],
            help: Some(format!(
                "write `pub fn {item}` in `{module}`, or reach it through something that is public"
            )),
        });
    }

    // --- crossing a thread (ADR-005 §1 Group B, `NK25xx`) -------------------
    //
    // Two places a value the *program* wrote reaches another thread, and **the
    // verdict is no longer the same in both** (ADR-045 D1): each asks
    // `contracts::send` about its own destination, and the lock is the type the
    // two answers differ on. Everything else answers alike wherever it is going,
    // so the split costs one argument and changes nothing else.
    //
    // The rule the destination does **not** displace is ADR-038 D7's: a value may
    // cross a thread only if it may cross any thread. What ADR-045 separates is
    // *which* thread is being talked about - one of ours, or one a library we
    // cannot read may start - and each answer is the same at both settings of
    // `user_parallelism`, which is all Group B ever asked. `contracts::send` owns
    // the walk and the reason; these two own the span and the sentence.
    //
    // Both report on `MayNot` and say nothing on `Undecided`, which is
    // `contracts::send`'s module header: refusing what this compiler cannot
    // decide would reject correct programs, and an undecided crossing is not
    // accepted here - `rustc` still type-checks the emitted crate, and
    // ADR-005 D7's `E0277` translation reports that refusal against this same
    // `.nika` line.
    //
    // **A method call is not asked**, and the limit is worth naming: a foreign
    // Rust function is reached by a qualified path (`hyper_shim::serve`), which
    // is what a name-for-name lowering makes callable at all (ADR-011 D2), so
    // the call form below is the one ADR-038 D7 is about. A method on a receiver
    // whose type no ledger describes would need the same question asked of its
    // arguments, and nothing in the corpus reaches it - ADR-028 D5's rule, that
    // an entry exists because a program asked for it.

    /// Part II 11.2: a task runs on a thread of its own, so everything it takes
    /// with it has to be able to cross one (`NK2501`).
    ///
    /// Over-approximate in the direction that costs nothing: the walk collects
    /// every name the body mentions, including function and field names, and a
    /// name that is not in scope is not found and says nothing.
    fn crosses_into_a_task(&mut self, body: &Expr, span: &Span) {
        for name in send::names_used(self.parsed, body) {
            let Some(ty) = self.lookup(&name) else {
                continue;
            };
            let crossing = send::crossing(&ty, self.own, self.library, send::Destination::Ours);
            if crossing.refused().is_none() {
                continue;
            }
            let notes = [
                Some(
                    "a task runs on a thread of its own, so everything it uses has to be able \
                     to cross one (Part II, 11.2)"
                        .to_string(),
                ),
                crossing.note(),
                Some(SAME_AT_BOTH.to_string()),
            ];
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2501",
                message: format!("`{name}` may not cross into a task, and this task uses it"),
                notes: notes.into_iter().flatten().collect(),
                help: crossing.way_out(),
            });
        }
    }

    /// **What a task holds across a pause** ([ADR-055](../../docs/specification/adr/adr-055.md)
    /// §2 D6's third sharp edge), which is `NK2501`'s question asked of what a
    /// body **binds** rather than of what it captures.
    ///
    /// A task's body is an `async` block below, and a value bound inside it and
    /// still live at a suspension point further down is held **inside the
    /// future**. The pool's starter asks for `Send` of that whole future, so
    /// such a value crosses a thread exactly as a captured one does — and until
    /// now the only thing that said so was the backend, about the generated
    /// file, which [Part III C.1](../../docs/specification/30-nikaia-tooling.md)
    /// calls a bug in this compiler.
    ///
    /// **Live is over-approximated, and the over-approximation is bounded by
    /// the verdict rather than by the walk.** *Bound before a pause and named
    /// after it* is all this asks; Rust's own answer is narrower. That would be
    /// a *correct program refused* ([Part III
    /// C.4](../../docs/specification/30-nikaia-tooling.md)) if the refusal
    /// stood on it alone — it does not. It stands on `contracts::send`'s
    /// `MayNot`, which is a claim about a **type** and is never `Undecided`'s
    /// silence, so a name this walk is too generous about is refused only if a
    /// value of its type could not have crossed from anywhere.
    ///
    /// **It reports nothing today, and that is why it is built now.** No type
    /// answers `MayNot` at `Destination::Ours`: `Shared` stopped being one when
    /// [ADR-037](../../docs/specification/adr/adr-037.md) D6 gave it one
    /// representation at both settings, and a lock is `MayNot` only at a
    /// *foreign* destination. A refusal costs nothing before there are programs
    /// it would reject, and the same refusal added afterwards breaks them. What
    /// reaches it first is a type from outside this language, which is what
    /// [ADR-104](../../docs/specification/adr/adr-104.md) is for.
    fn a_task_holds_these_across_a_pause(
        &mut self,
        body: &Block,
        bound: &[(String, Ty, usize)],
        span: &Span,
    ) {
        if bound.is_empty() {
            return;
        }
        let (pauses, named) = self.what_the_body_does(body);
        let places: Vec<(String, usize)> = bound
            .iter()
            .map(|(name, _, at)| (name.clone(), *at))
            .collect();
        let held = send::held_across_a_pause(&places, &pauses, &named);
        self.checked.held_across_a_pause.extend(
            held.iter()
                .map(|name| (span.start, name.clone()))
                .collect::<Vec<_>>(),
        );
        for (name, ty, _) in bound.iter().filter(|(name, ..)| held.contains(name)) {
            let crossing = send::crossing(ty, self.own, self.library, send::Destination::Ours);
            if crossing.refused().is_none() {
                continue;
            }
            let notes = [
                Some(
                    "a task's body is one function below, so a value it binds and still \
                     uses after a pause is held inside it - and the task runs on a thread \
                     of its own (Part II, 11.2)"
                        .to_string(),
                ),
                crossing.note(),
                Some(SAME_AT_BOTH.to_string()),
            ];
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2501",
                message: format!(
                    "`{name}` may not cross into a task, and this task holds it across a pause"
                ),
                notes: notes.into_iter().flatten().collect(),
                help: crossing.way_out(),
            });
        }
    }

    /// Where a block **pauses**, and where each name it mentions was last
    /// mentioned — both by the byte a statement starts at, and both reaching
    /// through the blocks a statement holds.
    ///
    /// A method call is answered from [`Checked::pausing_methods`], which the
    /// walk that just ran filled: which entry `db.load()` goes to is the type
    /// checker's answer ([ADR-028](../../docs/specification/adr/adr-028.md)). A
    /// free call is answered from the ledgers directly, the way
    /// [`contracts::sync`] answers it.
    fn what_the_body_does(&self, body: &Block) -> (Vec<usize>, BTreeMap<String, usize>) {
        let mut pauses = Vec::new();
        let mut named: BTreeMap<String, usize> = BTreeMap::new();
        self.walk_the_body(body, &mut pauses, &mut named);
        (pauses, named)
    }

    fn walk_the_body(
        &self,
        body: &Block,
        pauses: &mut Vec<usize>,
        named: &mut BTreeMap<String, usize>,
    ) {
        for stmt in &body.stmts {
            let at = stmt.span.start;
            crate::contracts::sync::visit_stmt(self.parsed, &stmt.node, &mut |expr| {
                if self.pauses_here(expr, at) {
                    pauses.push(at);
                }
                for name in crate::contracts::send::names_used(self.parsed, expr) {
                    let last = named.entry(name).or_insert(at);
                    *last = (*last).max(at);
                }
            });
            // The blocks a statement holds - an `if`, a loop, a lambda. Their
            // statements have spans of their own, so the order is the source's.
            crate::contracts::sync::visit_stmt_blocks(&stmt.node, &mut |inner| {
                self.walk_the_body(inner, pauses, named);
            });
        }
    }

    /// Whether this one expression is a suspension point.
    fn pauses_here(&self, expr: &Expr, at: usize) -> bool {
        match crate::contracts::sync::reached(self.parsed, expr, self.own, self.library) {
            Some(crate::contracts::sync::Reached::Method) => match expr {
                Expr::MethodCall { method, .. } | Expr::SafeMethod { method, .. } => self
                    .pausing_methods
                    .contains(&(at, self.parsed.text(*method).to_string())),
                _ => false,
            },
            Some(crate::contracts::sync::Reached::Own(name)) => self
                .own
                .functions
                .get(&name)
                .is_some_and(|c| !c.sync.is_sync()),
            Some(crate::contracts::sync::Reached::Library { sync, .. }) => !sync,
            // A call this compiler cannot name may do anything, pausing
            // included - which is the fail-closed direction for a question
            // whose wrong answer here is a value silently crossing a thread.
            Some(crate::contracts::sync::Reached::Opaque(_)) => true,
            None => false,
        }
    }

    /// ADR-038 D7: a value handed to a call this compiler cannot see the end of
    /// may reach a thread that call owns (`NK2502`).
    ///
    /// A Rust dependency may bring its own runtime, so "what does it do with
    /// what I gave it" has no answer here - and a thread of its own is among the
    /// answers. D7's first rule is therefore the same rule as `NK2501`'s, asked
    /// at a call instead of at a task.
    ///
    /// **Unlike `NK2501` this is not hypothetical at `user_parallelism = no`.**
    /// The switch bounds what the *program* runs at once (ADR-037 D2); a foreign
    /// runtime's threads are not the program's, so the crossing is real at both
    /// settings and so is the refusal.
    fn crosses_into_an_unseen_call(
        &mut self,
        callee: &str,
        handed: Arguments<'_>,
        span: &Span,
        why: &str,
    ) {
        let Arguments {
            args,
            found,
            config,
            passed,
        } = handed;
        let positional = args
            .iter()
            .zip(found)
            .enumerate()
            .map(|(at, (expr, ty))| (self.names_the_argument(expr, at), ty));
        let named = config.iter().zip(passed).map(|(arg, (_, ty))| {
            // The **option's** name in the headline, because that is what the
            // caller wrote at the `;`; the path is the value behind it, because
            // `state:` is not something a program can put a `.get()` on.
            let name = self.parsed.text(arg.name).to_string();
            ((format!("`{name}`"), self.dotted_path(&arg.value)), ty)
        });

        for ((what, path), ty) in positional.chain(named).collect::<Vec<_>>() {
            let crossing = send::crossing(ty, self.own, self.library, send::Destination::Foreign);
            if crossing.refused().is_none() {
                continue;
            }
            // **Which refusal it is decides which code it prints**
            // ([ADR-039](../../docs/specification/adr/adr-039.md) D6). A lock
            // reachable through an argument is `NK2503`, a refusal about the
            // **call**; everything else is `NK2502`, about the value crossing.
            if crossing.why() == Some(send::Refusal::Lock) {
                self.reaches_a_lock(callee, &what, path.as_deref(), &crossing, span);
                continue;
            }
            let notes = [
                Some(why.to_string()),
                crossing.note(),
                Some(SAME_AT_BOTH.to_string()),
            ];
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2502",
                message: format!("{what} may not cross a thread, and `{callee}` may put it on one"),
                notes: notes.into_iter().flatten().collect(),
                help: crossing.way_out(),
            });
        }
    }

    /// **A call into foreign code from which a lock is reachable** through its
    /// arguments, transitively and through the fields of a struct
    /// ([ADR-039](../../docs/specification/adr/adr-039.md) D6, Part III 15.2,
    /// worked through in C.6): `NK2503`.
    ///
    /// D6 says the check **is** `NK2502`'s walk generalised and never a copy of
    /// it, and this is that sentence built: the walk above is the only one,
    /// asked once per argument, and what arrives here is its verdict with the
    /// word `Lock` on it. Nothing is walked twice.
    ///
    /// **The difference from `NK2502` is what the refusal is about**, which is
    /// why it is a second code rather than a second sentence. `NK2502` refuses
    /// a **value** that may not cross being handed to a call that may start a
    /// thread; this refuses the **call**, because foreign code touches only
    /// what it reaches and a lock is what it must not be able to reach. A call
    /// that can reach no lock is allowed without a word (15.2).
    fn reaches_a_lock(
        &mut self,
        callee: &str,
        what: &str,
        path: Option<&str>,
        crossing: &send::Crossing,
        span: &Span,
    ) {
        let (part, at) = crossing.refused().expect("a lock is a refusal");
        // The path the note and the way out print: the argument's own name with
        // the field that decided it behind it, where both are there to be had.
        // Where the argument is not a name - a literal, a call - there is no
        // path to write and the note names the type instead.
        let reached = path.map(|path| match at {
            Some(field) => format!("{path}.{field}"),
            None => path.to_string(),
        });
        let notes = [
            Some(format!(
                "nothing written down describes `{callee}`, so this compiler cannot see what it \
                 does with what it is handed (Part III, 15.2)"
            )),
            Some(match &reached {
                Some(reached) => format!(
                    "`{reached}` is a `{part}`, and a lock is what the call must not be able to \
                     reach"
                ),
                None => format!(
                    "what this passes is a `{part}`, and a lock is what the call must not be \
                     able to reach"
                ),
            }),
            Some(SAME_AT_BOTH.to_string()),
        ];
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2503",
            message: format!("`{callee}` can reach a lock through {what}"),
            notes: notes.into_iter().flatten().collect(),
            // 15.2's way out is keeping the lock out of the call's reach, and
            // C.6 writes it as a line the program can be edited into.
            help: Some(match &reached {
                Some(reached) => format!(
                    "hand it a copy of what it needs instead of the container:\n\
                     {:11}{callee}({reached}.get())",
                    ""
                ),
                None => "open the lock where you are and hand over the value inside it - the \
                         called code then sees an ordinary value and no lock"
                    .to_string(),
            }),
        });
    }

    /// What to call one argument of a call in a message: its own name where it
    /// has one, and its place where it does not.
    ///
    /// **Two answers, because two readers want it.** The first is prose for the
    /// headline (*the `state` this passes*); the second is the bare name where
    /// there is one, which `NK2503`'s way out writes a path onto
    /// (`state.counts.get()`) and which is `None` for an argument that is not a
    /// name at all.
    fn names_the_argument(&self, expr: &Expr, at: usize) -> (String, Option<String>) {
        let prose = match expr {
            Expr::Variable(name) => format!("`{}`", self.parsed.text(*name)),
            Expr::Field { name, .. } => format!("the `{}` this passes", self.parsed.text(*name)),
            _ => format!("what this passes as argument {}", at + 1),
        };
        (prose, self.dotted_path(expr))
    }

    /// The argument written back as a path a program could paste, where it is
    /// one: `state`, `state.inner`, and `None` for anything else.
    ///
    /// Rebuilt from the tree rather than read from the source, because an
    /// expression has no span ([ADR-081](../../docs/specification/adr/adr-081.md)
    /// D2). So the answer is exact where it is `Some` - a name and a chain of
    /// fields is all of it - and absent where a guess would be needed, which is
    /// the same polarity as the ellipsis `NK2204` prints.
    fn dotted_path(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Variable(name) => Some(self.parsed.text(*name).to_string()),
            Expr::Field { base, name } => Some(format!(
                "{}.{}",
                self.dotted_path(base)?,
                self.parsed.text(*name)
            )),
            _ => None,
        }
    }

    /// An option the callee does not have (Kap 5.1).
    fn no_such_option(
        &mut self,
        key: &str,
        name: &str,
        signature: &crate::contracts::Signature,
        span: &Span,
    ) {
        let names: Vec<&str> = signature.config.iter().map(|c| c.name.as_str()).collect();
        let near = nearest(name, &names);
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1109",
            message: format!("`{key}` has no option `{name}`"),
            notes: vec![match names.is_empty() {
                true => format!("`{key}` takes no options at all - it has no `;`"),
                false => format!("`{key}` takes {}", list(&names)),
            }],
            help: Some(match near {
                Some(near) => format!("did you mean `{near}`?"),
                None if names.is_empty() => {
                    "everything before the `;` is positional (Part I, 5.1)".to_string()
                }
                None => "name one of the options it has".to_string(),
            }),
        });
    }

    /// Whether any ledger or this unit declares a **type** by this name.
    ///
    /// Presence and not fields, which is the difference from [`Self::fields_of`]:
    /// *an empty field list means nothing recorded and not nothing inside*
    /// (Part III 15.2), so a described foreign type has an entry and no fields,
    /// and it is declared.
    fn declares_a_type(&self, name: &str) -> bool {
        if self.structs.contains_key(name) || self.own.types.contains_key(name) {
            return true;
        }
        // `Self` inside an `impl`, and the type parameters an item brought in:
        // both are names a literal may wear and neither is in a `types` map.
        if name == "Self" || self.enums.contains_key(name) {
            return true;
        }
        let suffix = format!("::{name}");
        self.library
            .types
            .keys()
            .any(|key| key == name || key.ends_with(&suffix))
    }

    /// `NK1135` for a struct literal, with the sentence a *call* needs.
    ///
    /// The same code a written annotation gets, because it is the same claim -
    /// a name in type position that nothing declares. What differs is the way
    /// out, and it differs because of one shape: where the name is a **function**
    /// the author almost certainly meant
    /// [ADR-133](../../docs/specification/adr/adr-133.md) D1's options-only call,
    /// which this compiler does not parse yet, and *nothing declares a struct*
    /// would send them looking for a `struct` they never wanted.
    fn a_struct_nothing_declares(&mut self, name: &str, span: &Span) {
        let is_a_function = self.own.functions.contains_key(name)
            || self.library.lookup(name).is_some()
            || self.own.functions.contains_key(&format!("{name}::new"));
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1135",
            message: format!("nothing declares a struct called `{name}`"),
            notes: vec![match is_a_function {
                true => format!(
                    "a struct literal names a type, and `{name}` is a **function**. A \
                     call whose arguments are all options is written \
                     `{name}(option: value)` since ADR-133 D1 - with parentheses and no \
                     `;`, which is what the braces here would have been"
                ),
                false => "a struct literal names a type, and this name is not one Part I \
                          2.2 offers, nor one a ledger declares, nor one this file \
                          declares (ADR-096)"
                    .to_string(),
            }],
            help: Some(match is_a_function {
                true => format!("write `{name}(option: value)`"),
                false => "declare the `struct`, or correct the name".to_string(),
            }),
        });
    }

    /// `NK1146`: a struct literal written like a call
    /// ([ADR-140](../../docs/specification/adr/adr-140.md) D1).
    ///
    /// `Stats(min: first, max: first)` and `Reading { name, temp }` both built a
    /// struct, and `Stats(first)` called the anonymous constructor - so `Foo(x: 1)`
    /// went *round* a type's invariants and `Foo(1)` went *through* them, told
    /// apart by a colon. D1 keeps the braces and gives the parentheses to the
    /// call, which is also what freed [ADR-133](../../docs/specification/adr/adr-133.md)
    /// D1's options-only call: the two were one spelling.
    ///
    /// The message carries the whole rewrite rather than the rule, because the
    /// rewrite is mechanical and the reader has the fields in hand.
    fn a_literal_written_like_a_call(
        &mut self,
        name: &str,
        config: &[ast::ConfigArg],
        span: &Span,
    ) {
        let fields = config
            .iter()
            .map(|a| self.parsed.text(a.name).to_string())
            .collect::<Vec<_>>()
            .join(": …, ");
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1146",
            message: format!("a struct literal is written with braces, and `{name}` is a type"),
            notes: vec![
                "`Name(field: value)` was a second spelling of `Name { field: value }` \
                 and is gone (ADR-140 D1): the parentheses are a call now, so \
                 `Name(a, b)` reaches the anonymous constructor and nothing goes round \
                 it by carrying a colon"
                    .to_string(),
            ],
            help: Some(format!("write `{name} {{ {fields}: … }}`")),
        });
    }

    /// **`NK1151`: a `match` that misses a case**
    /// ([ADR-146](../../docs/specification/adr/adr-146.md) D1).
    ///
    /// Part I 3.4's own first sentence says a `match` *ensures that every
    /// possible case is handled*, and nothing here ensured it: Rust refuses a
    /// non-exhaustive `match`, so the reader got the backend's words on a Nikaia
    /// line ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **The question asked first is *does anything catch everything*,** and
    /// only where nothing does is the scrutinee's type asked about at all — an
    /// `else` ([ADR-145](../../docs/specification/adr/adr-145.md)) or a bare
    /// name, which binds and matches anything (D2).
    /// The variants one pattern names, recursing through an or-pattern
    /// ([ADR-137](../../docs/specification/adr/adr-137.md) D1).
    ///
    /// A **tuple's parts** are deliberately not walked: a part names a variant
    /// of some *other* type, and what this is collecting is the cases of the
    /// one being matched on.
    fn variants_named(&self, pattern: &MatchPattern, out: &mut BTreeSet<String>) {
        match pattern {
            MatchPattern::Path(path)
            | MatchPattern::Tuple { path, .. }
            | MatchPattern::Named { path, .. } => {
                if let Some(last) = path.last() {
                    out.insert(self.parsed.text(*last).to_string());
                }
            }
            MatchPattern::Or(alternatives) => {
                for alternative in alternatives {
                    self.variants_named(alternative, out);
                }
            }
            MatchPattern::Otherwise | MatchPattern::Literal(_) | MatchPattern::Range { .. } => {}
        }
    }

    /// **`NK1155`: an or-pattern whose alternatives do not bind the same
    /// names** ([ADR-137](../../docs/specification/adr/adr-137.md) D1).
    ///
    /// That rule is what keeps the arm's body answerable: a name the body reads
    /// has to be bound whichever alternative matched, and `(0, y) | (x, 0)` is
    /// a body that can read `y` or `x` and never knows which.
    ///
    /// **Refused here rather than below.** `rustc` refuses it too, in a message
    /// about a generated file ([Part III
    /// C.1](../../docs/specification/30-nikaia-tooling.md)) — and it is a rule
    /// of *this* language, stated by the record that added the form.
    ///
    /// Recursive, because a pattern nests: an or-pattern inside a tuple's part
    /// is the same rule one level down.
    fn an_or_pattern_that_binds_unevenly(&mut self, pattern: &MatchPattern, span: &Span) {
        match pattern {
            MatchPattern::Or(alternatives) => {
                let first: BTreeSet<String> =
                    self.pattern_names(&alternatives[0]).into_iter().collect();
                for alternative in &alternatives[1..] {
                    let names: BTreeSet<String> =
                        self.pattern_names(alternative).into_iter().collect();
                    if names != first {
                        let missing: Vec<String> =
                            first.symmetric_difference(&names).cloned().collect();
                        self.checked.findings.push(Finding {
                            severity: Severity::Error,
                            span: span.clone(),
                            code: "NK1155",
                            message: format!(
                                "the alternatives of this pattern bind different names: \
                                 `{}`",
                                missing.join("`, `")
                            ),
                            notes: vec![
                                "every alternative of an `|` pattern binds the same set of \
                                 names, because the arm's body reads them and does not know \
                                 which alternative matched (Part I, 3.4)"
                                    .to_string(),
                            ],
                            help: Some(
                                "bind the same names in each alternative, or write one arm \
                                 per shape"
                                    .to_string(),
                            ),
                        });
                        break;
                    }
                }
                for alternative in alternatives {
                    self.an_or_pattern_that_binds_unevenly(alternative, span);
                }
            }
            MatchPattern::Tuple { parts, .. } => {
                for part in parts {
                    self.an_or_pattern_that_binds_unevenly(part, span);
                }
            }
            _ => {}
        }
    }

    /// **`NK1162`: `m[k] += 1` on a map**
    /// ([ADR-114](../../docs/specification/adr/adr-114.md) D2).
    ///
    /// A compound assignment reads the slot and writes it, and since D1 the
    /// read is a **`T?`** — so the form has to say what an absent key counts
    /// as, which is the question [ADR-080](../../docs/specification/adr/adr-080.md)
    /// D2 left open. `m[k] = (m[k] ?? 0) + 1` is that sentence, and the message
    /// carries it.
    ///
    /// **A sequence is untouched**: `xs[i] += 1` reads a `T` and writes a `T`,
    /// because a list has a value at every index it has at all (D3).
    fn a_compound_write_to_a_map_slot(&mut self, target: &Expr, span: &Span) {
        let Expr::Index { base, index } = target else {
            return;
        };
        // **Only where the read is nullable**, which is only a map. Where the
        // container's type is not known, nothing is claimed
        // ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)).
        if !matches!(self.expr(target, span), Ty::Nullable(_)) {
            return;
        }
        let slot = format!("{}[{}]", self.written(base), self.written(index));
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1162",
            message: format!("`{slot}` reads the slot as well as writing it, and reading a map through the brackets is a `T?`"),
            notes: vec![
                "a key that is not there is data about the world rather than a bug \
                 (Part I, 4.5), so what an absent key counts as is something this line \
                 has to say (ADR-114 D2)"
                    .to_string(),
            ],
            help: Some(format!(
                "write it out, with the fallback saying what an absent key counts as: \
                 `{slot} = ({slot} ?? 0) + 1`"
            )),
        });
    }

    /// Whether more than one error **type** can arrive at a handler
    /// ([ADR-160](../../docs/specification/adr/adr-160.md) D4).
    ///
    /// Read off the guarded call's own set, the way the lowering reads it: only
    /// the outermost call and only one by name, because that is what a `catch`
    /// guards in every program in the tree.
    fn several_arrive(&self, guarded: &Expr) -> bool {
        let name = match guarded {
            Expr::Call { func, .. } => match func.as_ref() {
                Expr::Variable(name) => self.parsed.text(*name).to_string(),
                Expr::Path(segments) => segments
                    .iter()
                    .map(|s| self.parsed.text(*s))
                    .collect::<Vec<_>>()
                    .join("::"),
                _ => return false,
            },
            Expr::Try(inner) => return self.several_arrive(inner),
            _ => return false,
        };
        let key = self.parsed.unaliased(&name);
        self.own
            .functions
            .get(&key)
            .is_some_and(|contract| contract.throws.len() > 1)
    }

    /// The **one** error type a guarded expression can fail with, where exactly
    /// one is written down.
    ///
    /// The same resolution [`Checker::several_arrive`] does, asked for the other
    /// answer and over **both** ledgers: a `std` call's failure is in the
    /// library's, and that is the one a corpus handler actually catches.
    ///
    /// `None` where several arrive, where the callee is not one this compiler
    /// can resolve, or where the single member is `"?"` — the absence of a claim
    /// ([ADR-024](../../docs/specification/adr/adr-024.md) D1), which names no
    /// type and therefore no cases.
    fn the_one_error(&self, expr: &Expr) -> Option<Ty> {
        let name = match expr {
            Expr::Call { func, .. } => match &**func {
                Expr::Variable(name) => self.parsed.text(*name).to_string(),
                Expr::Path(segments) => segments
                    .iter()
                    .map(|s| self.parsed.text(*s))
                    .collect::<Vec<_>>()
                    .join("::"),
                _ => return None,
            },
            Expr::Try(inner) => return self.the_one_error(inner),
            _ => return None,
        };
        let key = self.parsed.unaliased(&name);
        let contract = self
            .own
            .functions
            .get(&key)
            .or_else(|| self.library.lookup(&key).map(|(_, c)| c).as_ref().copied())?;
        match contract.throws.as_slice() {
            [one] if one != "?" => Some(Ty::Named {
                name: one.clone(),
                args: Vec::new(),
                view: false,
            }),
            _ => None,
        }
    }

    /// **`NK1151` for a `match` over a `catch`'s error**
    /// ([ADR-160](../../docs/specification/adr/adr-160.md) D4).
    ///
    /// The variants **within** one error type are closed and the set of error
    /// **types** is open ([ADR-023](../../docs/specification/adr/adr-023.md)
    /// D4), so a handler that names variants of two of them has covered no set
    /// at all — a callee that gains a failure sends a third type here, and the
    /// `match` has nowhere to put it. `else` is what says the rest, which is
    /// the same answer this code gives for every type whose cases cannot be
    /// enumerated in arms.
    ///
    /// It is the caret for a refusal the **lowering** also states, which is the
    /// arrangement `NK1132` already has: the emitter cannot write the file
    /// either way, and this is where a reader finds out why.
    fn a_match_over_several_error_types(
        &mut self,
        value: &Expr,
        arms: &[ast::MatchArm],
        span: &Span,
    ) {
        if !self.caught_several {
            return;
        }
        if !matches!(value, Expr::Variable(name) if self.parsed.text(*name) == "error") {
            return;
        }
        let caught = arms
            .iter()
            .filter(|arm| arm.guard.is_none())
            .any(|arm| catches_everything(&arm.pattern));
        if !caught {
            self.a_case_is_missing("else", span);
        }
    }

    fn a_match_that_misses_a_case(&mut self, on: &Ty, arms: &[ast::MatchArm], span: &Span) {
        // **A guarded arm covers nothing** (
        // [ADR-137](../../docs/specification/adr/adr-137.md) D2): the pattern
        // says which values reach it and the guard says which of those it
        // takes, so the rest of them reach the arms below. Rust reads it the
        // same way, which is what keeps this compiler's answer and the
        // backend's from disagreeing.
        let catches_everything = arms
            .iter()
            .filter(|arm| arm.guard.is_none())
            .any(|arm| catches_everything(&arm.pattern));
        if catches_everything {
            return;
        }
        // **Where the type is not known, nothing is claimed** (D3), which is
        // [Part III C.4](../../../docs/specification/30-nikaia-tooling.md): a
        // refusal on a guess is a correct program refused.
        let Ty::Named { name, .. } = on else {
            return;
        };

        // **`bool` is the case that is neither** an enum nor open-ended: `true`
        // and `false` are two arms and a complete `match`, which no enum map
        // knows. Without this, a program Rust accepts would be refused here.
        if name == "bool" {
            let mut seen = BTreeSet::new();
            for arm in arms {
                if let MatchPattern::Literal(Expr::LitBool(value)) = &arm.pattern {
                    seen.insert(*value);
                }
            }
            if !seen.contains(&true) || !seen.contains(&false) {
                let missing: Vec<String> = [true, false]
                    .into_iter()
                    .filter(|value| !seen.contains(value))
                    .map(|value| value.to_string())
                    .collect();
                self.a_case_is_missing(&missing.join("`, `"), span);
            }
            return;
        }

        // An **enum**, from the map `NK1135` and the variant rule already read.
        let Some(variants) = self.enums.get(name).cloned() else {
            // Every other known type: the set is not enumerable in arms, so
            // `else` is what says the rest.
            self.a_case_is_missing("else", span);
            return;
        };
        // **An or-pattern names every variant in it**
        // ([ADR-137](../../docs/specification/adr/adr-137.md) D1, and
        // [ADR-146](../../docs/specification/adr/adr-146.md) §4's question
        // answered by building it): `Op::Plus | Op::Minus => …` is two cases
        // covered by one arm. A **guarded** arm names none, for the reason
        // above.
        let mut named: BTreeSet<String> = BTreeSet::new();
        for arm in arms.iter().filter(|arm| arm.guard.is_none()) {
            self.variants_named(&arm.pattern, &mut named);
        }
        let missing: Vec<String> = variants
            .iter()
            .filter(|variant| !named.contains(*variant))
            .map(|variant| format!("{name}::{variant}"))
            .collect();
        if !missing.is_empty() {
            self.a_case_is_missing(&missing.join("`, `"), span);
        }
    }

    /// The message [`a_match_that_misses_a_case`] raises, with what is missing
    /// already spelled: the variants for an enum, `else` for everything else.
    fn a_case_is_missing(&mut self, missing: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1151",
            message: format!("this `match` does not cover `{missing}`"),
            notes: vec![
                "a `match` handles every possible case (Part I, 3.4), so that a type gaining \
                 a variant is a refusal here rather than a branch nobody took \
                 (ADR-146 D1)"
                    .to_string(),
            ],
            help: Some(
                "add the arm, or write `else => …` for the rest - a bare name catches too, \
                 and binds what it caught"
                    .to_string(),
            ),
        });
    }

    /// **The interpreter, and the two refusals it can raise**
    /// ([ADR-073](../../docs/specification/adr/adr-073.md) D5's second stage,
    /// bounded by [ADR-075](../../docs/specification/adr/adr-075.md)).
    ///
    /// `None` is *this compiler cannot evaluate it*, which the caller turns
    /// into `NK1127` — the refusal that has always been there and whose note
    /// says what the stage knows. A body the rule **forbids** is a different
    /// claim and gets `NK1152`: the shape is understood and the answer is no.
    ///
    /// The second half of the pair is **whether a refusal was already
    /// reported**. Two errors for one mistake is what this used to print:
    /// `NK1152` naming the callee the rule forbids, and then `NK1127` saying
    /// the compiler cannot evaluate it — which is not a second fact, it is the
    /// first one said again with less in it. The caller keeps `NK1127` for the
    /// case it is about: a shape this evaluator does not read, which nothing
    /// else has a sentence for.
    fn build_time_value(
        &mut self,
        value: &Expr,
        bound: &str,
        span: &Span,
    ) -> (Option<build_time::Value>, bool) {
        let outcome = {
            // A name outside the body: a `comptime` already evaluated, or a
            // `let` whose value folded. Integers only, because that is what
            // the scope records — a `bool` constant is not visible here yet
            // and reaches the same `NK1127` it always did.
            // **The whole value where the build has one**, and the fold's
            // integer otherwise. `comptime BIG = ORIGIN.scaled(10)` needs
            // `ORIGIN` to be a `Point` here and not the number it is not.
            //
            // **Read off the scope before the evaluation starts**, because
            // it runs on a thread of its own (`BuildTime::evaluate`): later
            // bindings of a name go in after earlier ones, so the innermost
            // is the one that answers, as `binding` would.
            let mut held: BTreeMap<&str, build_time::Value> = BTreeMap::new();
            for local in self.scope.iter().flatten() {
                let value = local
                    .built
                    .clone()
                    .or_else(|| local.constant.map(build_time::Value::Int));
                match value {
                    Some(value) => held.insert(local.name.as_str(), value),
                    None => held.remove(local.name.as_str()),
                };
            }
            let known = |name: &str| -> Option<build_time::Value> { held.get(name).cloned() };
            build_time::BuildTime::new(self.parsed, self.beside, self.own, self.reads, &known)
                .evaluate(value)
        };
        match outcome {
            Ok(value) => (Some(value), false),
            Err(build_time::Refusal::Unevaluable) => (None, false),
            Err(build_time::Refusal::NotAllowed { callee, because }) => {
                self.a_body_that_may_not_run_at_build_time(&callee, because, span);
                (None, true)
            }
            Err(build_time::Refusal::TooDeep { callee }) => {
                self.a_build_time_call_went_too_deep(&callee, span);
                (None, true)
            }
            Err(build_time::Refusal::OutOfBounds { at, len }) => {
                self.a_build_time_index_is_not_there(at, len, span);
                (None, true)
            }
            Err(build_time::Refusal::Circular { ring }) => {
                self.a_constant_built_from_itself(bound, &ring, span);
                (None, true)
            }
            Err(build_time::Refusal::NotHere { what, why, way_out }) => {
                self.a_build_time_body_that_is_not_here(bound, &what, why, way_out, span);
                (None, true)
            }
            Err(build_time::Refusal::MayNotRead { path, why }) => {
                self.a_file_this_build_may_not_read(&path, &why, span);
                (None, true)
            }
            Err(build_time::Refusal::PathIsComputed) => {
                self.a_path_that_is_not_a_literal(span);
                (None, true)
            }
            Err(build_time::Refusal::GrammarWall { grammar, rule, why }) => {
                self.a_grammar_that_did_not_run(&grammar, &rule, &why, span);
                (None, true)
            }
        }
    }

    /// **A list of pairs, crossing as a table**
    /// ([ADR-176](../../docs/specification/adr/adr-176.md) D2).
    ///
    /// Hands back the two halves the `const` needs — `Fixed<i64>` and a
    /// `Fixed::new(…)` — or `None`. The whole table fits in one expression
    /// because a `const` promotes an array literal to `'static`, so the emitter
    /// writes nothing it would not have written for an integer.
    ///
    /// The second half of the pair is **whether a refusal was already
    /// reported**, the same handover `build_time_value` makes: a duplicate key
    /// is `NK1169` and a key that is not text is `NK1170`, and neither wants
    /// `NK1127` after it saying the compiler cannot evaluate what it just read
    /// well enough to name the mistake in.
    fn a_table_that_crosses(
        &mut self,
        bound: &str,
        want: &Ty,
        pairs: &[build_time::Value],
        span: &Span,
    ) -> (Option<(String, String)>, bool) {
        let Ty::Named { args, .. } = want else {
            return (None, false);
        };
        // **Text keys, and the rest by name** (D5). A number wants a dense
        // array, which is a different table and a different measurement.
        let (held, value_below, borrow) = match args.as_slice() {
            [
                Ty::Named {
                    name, view: true, ..
                },
                value,
            ] if name == "str" => {
                match rust_constant_type(value) {
                    Some(below) => (value, below, ""),
                    // **A `struct` or an `enum` this program declares is held
                    // as a *view* of one** ([ADR-180](../../docs/specification/adr/adr-180.md)
                    // D1). `Fixed::get` hands back a **value**, which is what
                    // the ledger promises and what `ROUTES.get(k) ?? 0` means,
                    // and it needs that value to be `Copy` — 0.0.118's own
                    // correction, made because `Option<&V>` put a `&&str` in
                    // the generated file and `??` over one is ambiguous.
                    //
                    // A Nikaia `struct` is not `Copy` and **a reference to one
                    // is**, so the table holds `&'static Row` and nothing about
                    // `get` changes. What the program reads is a view of the
                    // row, which is what a `const` of a table could ever have
                    // handed it.
                    None => match self.declared_below(value) {
                        Some(below) => (value, format!("&'static {below}"), "&"),
                        None => return (None, false),
                    },
                }
            }
            [key, _] => {
                let key = key.text();
                self.a_table_key_that_is_not_text(bound, &key, span);
                return (None, true);
            }
            _ => return (None, false),
        };

        let mut keys: Vec<String> = Vec::with_capacity(pairs.len());
        let mut values: Vec<String> = Vec::with_capacity(pairs.len());
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for (at, pair) in pairs.iter().enumerate() {
            let build_time::Value::Tuple(parts) = pair else {
                return (None, false);
            };
            let [build_time::Value::Text(key), value] = parts.as_slice() else {
                return (None, false);
            };
            // **A duplicate is refused at the build** (D4): there is no meaning
            // a compiler may pick between, and the whole of what a `comptime`
            // buys is that the failure moves to the line that wrote it.
            if let Some(first) = seen.insert(key.clone(), at) {
                self.a_table_with_one_key_twice(bound, key, first, at, span);
                return (None, true);
            }
            keys.push(key.clone());
            // **Written against the declaration**, so a `&[T]` inside a row
            // gets its `&` ([ADR-179](../../docs/specification/adr/adr-179.md)
            // D2) - and the table's own `&` goes in front, where the value is
            // a declared type.
            let Some(value) = self.written_below(value, Some(held)) else {
                return (None, false);
            };
            values.push(format!("{borrow}{value}"));
        }

        let Some(table) = crate::fixed::build(&keys) else {
            return (None, false);
        };
        let disps: Vec<String> = table
            .disps
            .iter()
            .map(|(d1, d2)| format!("({d1}, {d2})"))
            .collect();
        let written_keys: Vec<String> = table
            .keys
            .iter()
            .map(|key| format!("\"{}\"", build_time::written(key)))
            .collect();
        // **In slot order**, which is the whole of what the table is: the
        // generator decided where each key landed, and a value that stayed in
        // written order would be the wrong one for every key it moved.
        let written_values: Vec<String> =
            table.order.iter().map(|at| values[*at].clone()).collect();

        (
            Some((
                format!("Fixed<{value_below}>"),
                format!(
                    "Fixed::new({}, &[{}], &[{}], &[{}])",
                    table.seed,
                    disps.join(", "),
                    written_keys.join(", "),
                    written_values.join(", ")
                ),
            )),
            false,
        )
    }

    /// **`NK1169`: one key, written twice** (D4).
    fn a_table_with_one_key_twice(
        &mut self,
        bound: &str,
        key: &str,
        first: usize,
        again: usize,
        span: &Span,
    ) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1169",
            message: format!("`{bound}` writes the key `{key}` twice"),
            notes: vec![format!(
                "pair {} and pair {} name it, and a table has one value per key - there \
                 is no meaning a compiler may pick between (ADR-176 D4)",
                first + 1,
                again + 1
            )],
            help: Some("take one of them out, or make the key tell them apart".to_string()),
        });
    }

    /// **`NK1170`: a key that is not text** (D5).
    fn a_table_key_that_is_not_text(&mut self, bound: &str, key: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1170",
            message: format!("`{bound}` is keyed by `{key}`, and a table is keyed by text"),
            notes: vec![
                "a `Fixed[&str, V]` hashes and compares text; a number or a `bool` \
                 wants a different table - a dense array, most likely - which is a \
                 different decision with a measurement of its own (ADR-176 D5)"
                    .to_string(),
            ],
            help: Some(
                "write the keys as text, or keep it a `collections::HashMap` built \
                 while the program runs"
                    .to_string(),
            ),
        });
    }

    /// **`NK1168`: a constant worked out from itself.**
    ///
    /// The cost of making a constant behave like the item it is: once
    /// `comptime A = B * 2` may stand above `comptime B = 21`, the ring
    /// `comptime A = B` beside `comptime B = A` becomes writable — and it has
    /// no base case to reach, so nothing would end it.
    ///
    /// **Not the call depth** ([ADR-075](../../docs/specification/adr/adr-075.md)
    /// D4's neighbour), which catches a recursion that *would* terminate if the
    /// stack were deeper and says so. This one never would, and the message
    /// names the ring rather than a limit that has nothing to do with it.
    fn a_constant_built_from_itself(&mut self, bound: &str, ring: &[String], span: &Span) {
        // **One ring, one error.** Every constant in it is circular and each
        // would report the same loop from a different corner — which is one
        // mistake said as many times as it has members.
        let mut sorted: Vec<String> = ring.to_vec();
        sorted.sort();
        sorted.dedup();
        if !self.said_rings.insert(sorted) {
            return;
        }
        let named: Vec<String> = ring.iter().map(|name| format!("`{name}`")).collect();
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1168",
            // **The constant on this line**, which is the one the reader
            // declared; the ring below says where it goes.
            message: format!("`{bound}` is worked out from itself"),
            notes: vec![format!(
                "the ring is {} - a `comptime` must fold (ADR-073 D3), and nothing in \
                 this one reaches a value that does not need the next",
                named.join(" → ")
            )],
            help: Some(
                "give one of them a value that stands on its own, or make the \
                 dependent one a `let`, where it is computed while the program runs"
                    .to_string(),
            ),
        });
    }

    /// **`NK1127`, with the wall it met named** rather than a catalogue of what
    /// does work.
    ///
    /// The generic note is right for a shape this evaluator does not read —
    /// the reader wants to know what it *does* read. It is the wrong answer for
    /// `"a".to_uppercase()`, where the shape is understood and the body is
    /// Rust: no amount of rewriting the line helps, and a list of working forms
    /// invites the reader to go looking for the one that does.
    ///
    /// **Three walls, three sentences.** `std`'s body is not this language's; a
    /// function in another file of this program is not in the file this walk
    /// reads; a method is not a shape it reads at all. Each ends somewhere
    /// different, and only the last is the catalogue's business.
    fn a_build_time_body_that_is_not_here(
        &mut self,
        bound: &str,
        what: &str,
        why: &str,
        way_out: &str,
        span: &Span,
    ) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1127",
            // **The binding's name and not the callee's**, which is the same
            // sentence the generic `NK1127` writes: one code, one headline, and
            // the reader's own name in it. What it met is the note's.
            message: format!("this compiler cannot evaluate `{bound}` while it builds"),
            // **One note, written per wall.** The sentence about `sync` belongs
            // to the three walls that are about a **body** and not to the one
            // about text, so it lives in each `why` rather than under all of
            // them — a note that does not apply is one the reader has to rule
            // out.
            notes: vec![format!("`{what}`: {why} (Part II, 10.2)")],
            // **The wall's way out, and then the one every `comptime` has.**
            // The second half is the generic refusal's own sentence, with the
            // reader's name in it: a value meant to be computed while the
            // program runs was never a constant (ADR-073 D3).
            help: Some(format!(
                "{way_out}. Or write `let {bound} = …`, where the value is meant to be \
                 computed while the program runs"
            )),
        });
    }

    /// **`NK1165`: a build-time index the array does not have.**
    ///
    /// The same mistake a running program makes, met at the one moment there is
    /// no run to abort: `xs[7]` of five elements while the program is being
    /// built. [ADR-048](../../docs/specification/adr/adr-048.md) D1 aborts with
    /// this sentence at run time, and saying *this compiler cannot evaluate it*
    /// instead would send the reader looking for a missing feature rather than
    /// at the line ([Part III C.2](../../docs/specification/30-nikaia-tooling.md)).
    fn a_build_time_index_is_not_there(&mut self, at: i128, len: usize, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1165",
            message: format!("this reads element {at} of {}", plural(len, "element")),
            notes: vec![
                "the whole of it is computed while the program is built (Part II, 10.2), \
                 so this is the read happening - there is no run left for it to abort in \
                 (ADR-048 D1)"
                    .to_string(),
            ],
            help: Some(match len {
                0 => "the array is empty, so no index is in it".to_string(),
                _ => format!("the indices are 0 to {}", len - 1),
            }),
        });
    }

    /// **`[1, 2, 3]`**, and what its type is
    /// ([ADR-135](../../docs/specification/adr/adr-135.md) D1).
    ///
    /// The elements are walked in order and the first one that has a type is
    /// what the rest answer to. A type this checker could not work out is
    /// **silence** and not a disagreement: Stage 0 knows the type of rather
    /// less than half of what a program writes, and a refusal on doubt is
    /// [Part III C.4](../../docs/specification/30-nikaia-tooling.md)'s correct
    /// program refused.
    ///
    /// **Once per literal.** Three elements that disagree with the first are
    /// one mistake, so the walk stops reporting after the first pair - but it
    /// keeps *walking*, because an element is an expression and a mistake
    /// inside one is still a mistake.
    fn list_literal(&mut self, items: &[Expr], span: &Span) -> Ty {
        let mut agreed = Ty::Unknown;
        let mut kind: Option<(&'static str, String)> = None;
        let mut said = false;
        let founds: Vec<Ty> = items.iter().map(|item| self.expr(item, span)).collect();
        for (item, ty) in items.iter().zip(&founds) {
            self.hands_over(item, ty, "put into a list", span);
        }
        // **A text literal takes its neighbours' text** (ADR-207 D2): where
        // the list already holds text of its own, `["a", f"c"]` is a list of
        // `String` and the literal is constructed as one. A list holds one
        // type, so the literal has no other answer - and a **view** among the
        // elements is not a literal, so it still disagrees below.
        let holds_text = founds.iter().any(|found| *found == Ty::named("String"));
        let founds: Vec<Ty> = items
            .iter()
            .zip(founds)
            .map(|(item, found)| match (item, holds_text) {
                (Expr::LitStr { at, .. }, true) => {
                    self.checked.owned_texts.insert(self.text_at(*at));
                    Ty::named("String")
                }
                _ => found,
            })
            .collect();
        for (item, found) in items.iter().zip(founds) {
            // **A number beside text, where neither has a type yet.** A bare
            // `1` fits every numeric type and so it arrives here as `?` - which
            // is right, and which would let `[1, "two"]` past to be answered by
            // the backend about a file nobody wrote
            // ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
            // The **kind** is the part of a literal that is known without a
            // type, and two kinds that differ is a disagreement whatever the
            // types say.
            if let Some(shape) = element_kind(item, &found) {
                match &kind {
                    None => kind = Some((shape, found.text())),
                    Some((first, written)) if *first != shape && !said => {
                        said = true;
                        let (first, written) = (*first, written.clone());
                        self.a_list_whose_elements_disagree(first, &written, shape, &found, span);
                    }
                    Some(_) => {}
                }
            }
            if found.is_unknown() {
                continue;
            }
            if agreed.is_unknown() {
                agreed = found;
                continue;
            }
            if !found.fits(&agreed) && !said {
                said = true;
                let first = agreed.clone();
                self.a_list_whose_elements_disagree(
                    element_kind(item, &first).unwrap_or("a value"),
                    &first.text(),
                    "a value",
                    &found,
                    span,
                );
            }
        }
        Ty::Named {
            name: "Vec".to_string(),
            args: vec![agreed],
            view: false,
        }
    }

    /// `NK1161`: a `throw` of something that is not an error (Part I 7.1).
    ///
    /// *"What is thrown implements `Error`, and the `impl` line says so."* A
    /// number, a `bool`, a `char` and **text** are Part I 2.2's own types and
    /// none of them does — nor ever will, because an `impl` for one would have
    /// to be written somewhere and there is nowhere.
    ///
    /// **Only those**, which is the whole rule and is what makes it safe. A
    /// type this file declares may have its `impl Error` in another file of the
    /// same package ([ADR-047](../../docs/specification/adr/adr-047.md)), a
    /// package's type is not this compiler's to answer for
    /// ([ADR-046](../../docs/specification/adr/adr-046.md) D2), and a caught
    /// error re-thrown is a `?` — so each of those is left alone, which is
    /// [Part III C.4](../../docs/specification/30-nikaia-tooling.md)'s rule.
    ///
    /// **Found by writing `sqlite3` end to end**
    /// ([ADR-147](../../docs/specification/adr/adr-147.md) §5 step 5), which is
    /// what that step is for: `throw "no database"` read like a program and
    /// `rustc` answered *the trait bound `str: Error` is not satisfied* about a
    /// file nobody wrote. It had been writable since `throw` existed and no
    /// program in the tree had written one.
    fn a_thrown_value_that_is_not_an_error(&mut self, value: &Expr, thrown: &Ty, span: &Span) {
        // **The *kind* and not the type**, which is `NK1154`'s own machinery one
        // construct over: a bare `3` fits every numeric type and so it arrives
        // here as `?` (Part I 2.4), and the kind is the part of a literal that
        // is known without one. `element_kind` answers for a literal *or* for a
        // type it recognises, which is exactly the set that can never be an
        // error.
        let Some(what) = element_kind(value, thrown) else {
            return;
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1161",
            message: format!("this throws {what}, and what is thrown is an error"),
            notes: vec![
                "an error is a type with an `impl Error` on it, which is where its \
                 `message` is written - the type carries no marker and the `impl` line \
                 is what says so (Part I, 7.1)"
                    .to_string(),
            ],
            help: Some(
                "declare one: `enum Refused { NotFound }` with an \
                 `impl Error for Refused { fn message(&self) -> String { … } }`, and \
                 throw a value of it - an error carries what belongs to it"
                    .to_string(),
            ),
        });
    }

    /// **Whether this argument is a count a C declaration takes in `size_t`**
    /// ([ADR-147](../../docs/specification/adr/adr-147.md) D2).
    ///
    /// `usize` is not a type this language's own values have
    /// ([ADR-048](../../docs/specification/adr/adr-048.md) D1) — a length is an
    /// `i64` and the machine-width type left the surface a program can write —
    /// so a foreign declaration that writes one is naming C's `size_t`, and
    /// what a caller hands it is the integer this language does have. The
    /// emitter writes the conversion, exactly as it does for `str::repeat`.
    ///
    /// **Only a foreign declaration**, because only there does `usize` mean
    /// *the other language's word for this*. A `usize` anywhere else is a type
    /// like any other and is measured like one.
    fn a_count_at_the_boundary(&self, written: &str, want: &Ty, found: &Ty) -> bool {
        if !self.foreign_names.contains(written) {
            return false;
        }
        let wants_a_size = matches!(
            want,
            Ty::Named { name, args, .. } if name == "usize" && args.is_empty()
        );
        let hands_a_number = match found {
            Ty::Unknown => true,
            Ty::Named { name, args, .. } => args.is_empty() && is_number(name),
            _ => false,
        };
        wants_a_size && hands_a_number
    }

    /// `NK1160`: a field or an index reached on an **opaque handle**
    /// ([ADR-147](../../docs/specification/adr/adr-147.md) D3).
    ///
    /// A handle is an address this language never dereferences. It is moved and
    /// stored like any value, and that is all it is — there is nothing inside
    /// it to name and nothing to count, because what it points at belongs to
    /// the library that made it.
    ///
    /// **Refused here rather than below**, which is the choice this compiler
    /// makes everywhere ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)):
    /// what `rustc` would say about a field of a `#[repr(transparent)]` newtype
    /// is about a file nobody wrote, and it would name the wrapper rather than
    /// the handle.
    fn a_handle_has_nothing_inside(&mut self, handle: &str, what: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1160",
            message: format!("`{handle}` is a handle, and {what} reaches inside it"),
            notes: vec![format!(
                "`{handle}` is declared `opaque` - an address this language never \
                 dereferences, so what it points at belongs to the library that made it \
                 and has no shape here (ADR-147 D3)"
            )],
            help: Some(format!(
                "hand the handle to a function the `extern \"C\"` block declares - that \
                 is what a library's own surface is for, and `{handle}` is released by \
                 the function the block names"
            )),
        });
    }

    /// `NK1160`, the third shape: a **handle** written as a call
    /// ([ADR-147](../../docs/specification/adr/adr-147.md) D3).
    ///
    /// An opaque handle is an address a C function hands back. There is no
    /// value of one this language can make, so a constructor would have to
    /// invent an address — and the one thing a handle may never be is a number
    /// somebody chose.
    ///
    /// **The out-parameter shape is why a reader reaches for this.**
    /// `sqlite3_open(path, db)` wants a handle to fill, and C writes it as an
    /// uninitialised pointer. What this language does there is a question the
    /// record does not answer, and it is in
    /// [`open-decisions.md`](../../docs/open-decisions.md) rather than guessed
    /// at here.
    fn a_handle_is_not_made_here(&mut self, handle: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1160",
            message: format!("`{handle}` is a handle, and nothing here makes one"),
            notes: vec![format!(
                "`{handle}` is declared `opaque` - an address a C function hands back - so \
                 a constructor would have to invent one, and an address somebody chose is \
                 the one thing a handle may never be (ADR-147 D3)"
            )],
            help: Some(format!(
                "call the function in the `extern \"C\"` block that hands a `{handle}` \
                 back, and let its `cleanup` end it"
            )),
        });
    }

    /// **A length beside a view is checked at the call**
    /// ([ADR-147](../../docs/specification/adr/adr-147.md) D2).
    ///
    /// `read(fd, buf, count)` declares `count: usize`, and the two parameters
    /// are **one fact** in C: the pointer says where and the count says how far.
    /// A call that passes a longer count is the buffer overrun the boundary
    /// exists to stop, and it is refused here rather than by the operating
    /// system.
    ///
    /// **What makes a pair is the declaration's own types**: a `&[T]` and, right
    /// after it, a `usize`. The type is what says *length* rather than the name,
    /// because `usize` is not a type this language's own values have
    /// ([ADR-048](../../docs/specification/adr/adr-048.md) D1) — a length here
    /// is an `i64` and the machine-width type left the surface a program can
    /// write. A declaration that writes `usize` beside a buffer is therefore
    /// saying C's `size_t`, and this is what that says.
    ///
    /// **Two shapes are accepted and everything else is refused** (D2's *narrow
    /// on purpose*): the buffer's own `len()`, and a constant the buffer's
    /// length is known to cover — which is an `Array[T, N]`, whose length is
    /// part of its type ([ADR-152](../../docs/specification/adr/adr-152.md) D1).
    /// A **zero** is accepted against any buffer, because no length is smaller.
    ///
    /// Nothing is claimed where the callee is not a foreign declaration, or
    /// where this checker could not work the buffer's type out (Part III, C.4).
    fn a_length_that_fits_its_buffer(
        &mut self,
        written: &str,
        wanted: &[(String, Ty)],
        given: &[Expr],
        found: &[Ty],
        span: &Span,
    ) {
        if !self.foreign_names.contains(written) {
            return;
        }
        for (at, (_, want)) in wanted.iter().enumerate() {
            if !matches!(want, Ty::Pointed { slice: true, .. }) {
                continue;
            }
            let counts = matches!(
                wanted.get(at + 1).map(|(_, t)| t),
                Some(Ty::Named { name, args, .. }) if name == "usize" && args.is_empty()
            );
            if !counts {
                continue;
            }
            let (Some(buffer), Some(count)) = (given.get(at), given.get(at + 1)) else {
                continue;
            };
            // `buf.len()` on the very expression the buffer position was given.
            // Compared by shape, which is how this module tells two arguments of
            // one statement apart everywhere else.
            if let Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } = count
                && self.parsed.text(*method) == "len"
                && args.is_empty()
                && argument_shape(receiver) == argument_shape(buffer)
            {
                continue;
            }
            let folded = self.constant_of(count).map(|c| c.value);
            // A constant the buffer's own length covers. `Array[T, N]` is the
            // one buffer whose length this compiler knows, and a zero is
            // covered by every buffer there is.
            let room = match found.get(at) {
                Some(Ty::Named { name, args, .. }) if name == ty::ARRAY => match args.as_slice() {
                    [_, Ty::Count(n)] => Some(*n as i128),
                    _ => None,
                },
                _ => None,
            };
            let fits = match (folded, room) {
                (Some(0), _) => true,
                (Some(c), Some(n)) => c >= 0 && c <= n,
                _ => false,
            };
            if fits {
                continue;
            }
            self.a_length_that_may_not_fit(written, buffer, span);
        }
    }

    /// `NK1159`: a length handed to a C declaration beside a buffer it cannot be
    /// shown to fit ([ADR-147](../../docs/specification/adr/adr-147.md) D2).
    ///
    /// **The help names both ways out**, because D2 accepts exactly two: the
    /// buffer's own `len()`, and a constant a known length covers. Anything
    /// else is refused rather than guessed at, which is the polarity every
    /// check at this boundary keeps — the alternative is the operating system
    /// answering, about memory the program did not mean to touch.
    fn a_length_that_may_not_fit(&mut self, written: &str, buffer: &Expr, span: &Span) {
        let named = self.names_of(buffer);
        let buffer = named.as_deref().unwrap_or("the buffer");
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1159",
            message: format!(
                "`{written}` reads this as the length of `{buffer}`, and it cannot be \
                 shown to fit"
            ),
            notes: vec![
                "a pointer and a count are one fact in C - the first says where and the \
                 second says how far - and a count that is longer than the buffer is the \
                 overrun this boundary exists to stop (ADR-147 D2)"
                    .to_string(),
            ],
            help: Some(format!(
                "write `{buffer}.len()`, or a constant the buffer's own length covers - an \
                 `Array[T, N]` carries its length and a `Vec[T]` does not"
            )),
        });
    }

    /// **A literal takes the use's type, whichever literal it is**: an array
    /// where an `Array[T, N]` is wanted (ADR-152 D4), and text of its own where
    /// a `String` is ([ADR-207](../../docs/specification/adr/adr-207.md) D2).
    /// One entry for every position that is a use, so a position cannot ask
    /// one of the two questions and forget the other.
    fn literal_by_use(&mut self, found: &Ty, want: &Ty, value: &Expr, span: &Span) -> Option<Ty> {
        self.a_kept_lambda(want, value, span);
        self.array_literal(found, want, value, span)
            .or_else(|| self.text_literal(want, value, true))
            .or_else(|| self.tuple_literal(found, want, value))
            .or_else(|| self.collect_by_use(found, want, value, span))
    }

    /// **`collect()` builds what the place it goes into declares**
    /// ([ADR-227](../../docs/specification/adr/adr-227.md) D1): a map from
    /// pairs, a set from items, text from characters or pieces of text. The ledger says `Vec[$T]`, which is what it builds where nothing
    /// says otherwise; where a `let`, a field or a result declares another
    /// container the items fit, that is the value's type, and the language
    /// below is told nothing more - its own `collect` builds the declared type.
    fn collect_by_use(&mut self, found: &Ty, want: &Ty, value: &Expr, span: &Span) -> Option<Ty> {
        let Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } = value
        else {
            return None;
        };
        if self.parsed.text(*method) != "collect" || !args.is_empty() {
            return None;
        }
        let Ty::Named {
            name, args: made, ..
        } = found
        else {
            return None;
        };
        let [item] = made.as_slice() else {
            return None;
        };
        if ty::base(name) != "Vec" {
            return None;
        }
        let Ty::Named {
            name: wanted,
            args: parts,
            view: false,
        } = want
        else {
            return None;
        };
        let fits = match (ty::base(wanted), parts.as_slice()) {
            ("HashMap" | "BTreeMap" | "TrustedMap", [key, value]) => {
                item.fits(&Ty::Tuple(vec![key.clone(), value.clone()]))
            }
            ("HashSet" | "BTreeSet" | "TrustedSet", [element]) => item.fits(element),
            ("String", []) => {
                *item == Ty::named("char")
                    || *item == Ty::view("str")
                    || *item == Ty::named("String")
            }
            _ => false,
        };
        if !fits {
            return None;
        }
        self.checked
            .collected_into
            .insert((span.start, argument_shape(receiver)));
        Some(want.clone())
    }

    /// **A lambda where a function value is kept** is one shared closure
    /// below, and one that may pause hands back a boxed future.
    fn a_kept_lambda(&mut self, want: &Ty, value: &Expr, span: &Span) {
        let want = match want {
            Ty::Nullable(inner) => inner.as_ref(),
            other => other,
        };
        if let (
            Ty::Fn {
                is_sync, throws, ..
            },
            Expr::Closure { .. },
        ) = (want, value)
        {
            self.checked
                .kept_lambdas
                .insert((span.start, argument_shape(value)), (!is_sync, *throws));
            self.a_lambdas_text_is_its_own(want, value);
            // **A hull it captures, it captures a handle of** (ADR-230 D4,
            // ADR-040 D5): handing a hull on duplicates the count, so the name
            // outside is still there on the next line.
            if let Expr::Closure { params, body, .. } = value {
                let mut named: BTreeSet<String> = BTreeSet::new();
                crate::emit::visit_block(body, &mut |e| {
                    if let Expr::Variable(name) = e {
                        named.insert(self.parsed.text(*name).to_string());
                    }
                });
                let own: BTreeSet<String> = params
                    .iter()
                    .map(|p| self.parsed.text(*p).to_string())
                    .collect();
                let hulls: Vec<String> = named
                    .into_iter()
                    .filter(|name| !own.contains(name))
                    .filter(|name| {
                        matches!(self.lookup(name), Some(Ty::Named { name, .. }) if is_hull(ty::base(&name)))
                    })
                    .collect();
                if !hulls.is_empty() {
                    self.checked
                        .kept_hulls
                        .insert((span.start, argument_shape(value)), hulls);
                }
            }
        }
        // **A function of this program, named**, is the closure that calls it:
        // the call is typed here so that it is lowered as a written one is.
        if let (
            Ty::Fn {
                params,
                is_sync,
                throws,
                ..
            },
            Expr::Variable(name),
        ) = (want, value)
        {
            let named = self.parsed.text(*name).to_string();
            if self.lookup(&named).is_none() && self.own.functions.contains_key(&named) {
                self.checked
                    .kept_functions
                    .insert((span.start, named), (!is_sync, *throws, params.len()));
                let (names, call) = kept_function_call(self.parsed, *name, params.len());
                let frame = names
                    .iter()
                    .zip(params)
                    .map(|(n, ty)| Local::free(self.parsed.text(*n).to_string(), ty.clone()))
                    .collect();
                self.scope.push(frame);
                self.expr(&call, span);
                self.scope.pop();
            }
        }
    }

    /// **A lambda's text literal is built where its type keeps text**: the
    /// value a lambda ends in is handed to whoever calls it, as a function's
    /// is (ADR-207 D2).
    fn a_lambdas_text_is_its_own(&mut self, want: &Ty, value: &Expr) {
        if let (
            Ty::Fn {
                result: Some(result),
                ..
            },
            Expr::Closure { body, .. },
        ) = (want, value)
            && let Some(tail) = tail_of(body)
        {
            self.text_literal(result, tail, true);
        }
    }

    /// **A tuple's text literals are built where the tuple is kept**, part by
    /// part: `return (1, "x")` for a `(i64, String)` is the program it reads
    /// as. The parts that are not literals keep the type they were found to
    /// have, so a view among them is still refused.
    fn tuple_literal(&mut self, found: &Ty, want: &Ty, value: &Expr) -> Option<Ty> {
        let (Ty::Tuple(wants), Ty::Tuple(founds), Expr::Tuple(parts)) = (want, found, value) else {
            return None;
        };
        if wants.len() != parts.len() || founds.len() != parts.len() {
            return None;
        }
        let mut built = false;
        let mut answered = Vec::with_capacity(parts.len());
        for ((want, found), part) in wants.iter().zip(founds).zip(parts) {
            match self
                .text_literal(want, part, true)
                .or_else(|| self.tuple_literal(found, want, part))
            {
                Some(ty) => {
                    built = true;
                    answered.push(ty);
                }
                None => answered.push(found.clone()),
            }
        }
        built.then_some(Ty::Tuple(answered))
    }

    /// **A text literal where a `String` is wanted is a `String`**
    /// ([ADR-207](../../docs/specification/adr/adr-207.md) D2), and hands back
    /// the type it became.
    ///
    /// `"x"` is a view of static text, and a view put where text of its own
    /// is declared was refused until the program wrote `.to_string()` - the
    /// most common refusal a newcomer met, and one that protected nothing: the
    /// text is a constant in the program, so making a `String` of it is
    /// *constructing* a value, as `[1, 2]` constructs the `Vec` it stands in,
    /// and not copying text the program had. ADR-005 §3's rule is about the
    /// second, and it stands: a **view** of text the program has still needs
    /// its `.clone()`.
    ///
    /// `owned` says whether the position keeps what it is given. It is `false`
    /// for an argument the callee only reads, whose parameter is a `&str` below
    /// (D3): the literal fits it as it is, the answer is still *a `String`*
    /// for the fit, and nothing is recorded, so nothing is allocated.
    ///
    /// Also through `T?`, whose wrap is the caller's (`wraps_into_nullable`),
    /// and through a list literal where a `Vec[String]` is wanted, element by
    /// element - but only where **every** element is a text literal, so a list
    /// that mixes in a view keeps the refusal it had.
    /// The function type of `on`'s field called `method`, where `on` is a
    /// struct with such a field and no method of that name.
    fn a_field_that_is_code(&self, on: &Ty, method: Ident) -> Option<Ty> {
        let Ty::Named { name, .. } = on else {
            return None;
        };
        let method = self.parsed.text(method);
        if self.resolve(&format!("{name}::{method}")).is_some() {
            return None;
        }
        self.fields_of(name)?
            .into_iter()
            .find(|field| field.name == method)
            .map(|field| field.ty)
            .filter(|ty| matches!(ty, Ty::Fn { .. }))
    }

    fn text_at(&self, at: usize) -> usize {
        text_key(self.hole.as_ref(), at)
    }

    fn text_literal(&mut self, want: &Ty, value: &Expr, owned: bool) -> Option<Ty> {
        let text = Ty::named("String");
        match (want, value) {
            (Ty::Nullable(inner), _) => self.text_literal(inner, value, owned),
            // `"x".to_string()` is `"x"` (ADR-216 D4), so it is built where it
            // is kept exactly as the literal is.
            (
                _,
                Expr::MethodCall {
                    receiver,
                    method,
                    args,
                    ..
                },
            ) if args.is_empty()
                && self.parsed.text(*method) == "to_string"
                && matches!(receiver.as_ref(), Expr::LitStr { .. }) =>
            {
                self.text_literal(want, receiver, owned)
            }
            (_, Expr::LitStr { at, .. }) if *want == text => {
                if owned {
                    self.checked.owned_texts.insert(self.text_at(*at));
                }
                Some(text)
            }
            // **A choice between literals is a literal in each arm**: the use
            // reaches through `if`, `match` and a block to the value each one
            // ends in, so `-> String { match r { A => "a" B => "b" } }` is the
            // program it reads as. Only where **every** arm ends in text this
            // answers, for the list's reason below.
            (
                _,
                Expr::If {
                    then_branch,
                    else_branch: Some(else_branch),
                    ..
                },
            ) => {
                let arms = [tail_of(then_branch), tail_of(else_branch)];
                self.every_arm(want, &arms, owned)
            }
            (_, Expr::Match { arms, .. }) => {
                let arms: Vec<Option<&Expr>> = arms.iter().map(|arm| Some(&arm.body)).collect();
                self.every_arm(want, &arms, owned)
            }
            (_, Expr::Block(block)) => self.every_arm(want, &[tail_of(block)], owned),
            (
                Ty::Named {
                    name,
                    args,
                    view: false,
                },
                Expr::ListLit { items, .. },
            ) if name == "Vec" && args.as_slice() == [text.clone()] && !items.is_empty() => {
                let all_text = items.iter().all(|item| matches!(item, Expr::LitStr { .. }));
                if !all_text {
                    return None;
                }
                for item in items {
                    self.text_literal(&text, item, true);
                }
                Some(want.clone())
            }
            _ => None,
        }
    }

    /// **Arms that disagree only because some are text literals and some are
    /// text of their own agree on `String`** (ADR-207 D2): the literals take
    /// their neighbours' type, as they do in a list. `if c { name } else
    /// { "anonymous" }` over a `name: String` is a `String`, and the literal
    /// arm is constructed as one. A **view** arm is not a literal, so an arm
    /// holding one keeps the disagreement.
    fn arms_meet_at_text(&mut self, arms: &[(Option<&Expr>, Ty)]) -> Option<Ty> {
        let text = Ty::named("String");
        if !arms.iter().any(|(_, ty)| *ty == text) {
            return None;
        }
        let meets = arms
            .iter()
            .all(|(arm, ty)| *ty == text || arm.is_some_and(ends_in_text));
        if !meets {
            return None;
        }
        for (arm, ty) in arms {
            if let (Some(arm), false) = (arm, *ty == text) {
                self.text_literal(&text, arm, true);
            }
        }
        Some(text)
    }

    /// [`Checker::text_literal`] for each arm of a choice, and an answer only
    /// where every arm gave one. Asked of all of them first and recorded after,
    /// so an `if` with one literal arm and one view records nothing: half a
    /// construction would be a `String` beside a `&str` below.
    fn every_arm(&mut self, want: &Ty, arms: &[Option<&Expr>], owned: bool) -> Option<Ty> {
        let texts = |arm: &Option<&Expr>| arm.is_some_and(ends_in_text);
        if arms.is_empty() || !arms.iter().all(texts) {
            return None;
        }
        let mut answered = None;
        for arm in arms.iter().flatten() {
            answered = self.text_literal(want, arm, owned);
        }
        answered
    }

    /// **A literal takes the array type where the use asks for one**
    /// ([ADR-152](../../docs/specification/adr/adr-152.md) D4), and hands back
    /// the array it is.
    ///
    /// This is [ADR-135](../../docs/specification/adr/adr-135.md) D2's rule for
    /// the **empty** list extended to a full one: the literal takes the type its
    /// use gives it, and a use that asks for `Array[f64, 3]` gets an array
    /// rather than a `Vec`.
    ///
    /// **Asked after the literal has been walked** rather than before it, which
    /// is what keeps it one function instead of a second argument threaded
    /// through every expression that has a type: `[1.0, 2.0]` holds two `f64`
    /// whichever container it turns out to be, so the walk answers the elements
    /// and this answers only the container.
    ///
    /// **And it hands back the array of what the elements *agreed* on** rather
    /// than the one the use asked for, which is what leaves the element
    /// question with its caller: `[1, 2]` where an `Array[&str, 2]` is wanted
    /// comes back `Array[i64, 2]` and is refused by the `let`, the argument or
    /// the field in that position's own words, under the code that position
    /// always used. One message, and no code of its own for a mismatch that is
    /// not new.
    ///
    /// **And it descends with the literal.** The first reading of *the use* read
    /// the type it was given whole, which refused
    /// `let grid: Vec[Array[f64, 2]] = [[1.0, 2.0], [3.0, 4.0]]` — a correct
    /// program, which is [Part III
    /// C.4](../../docs/specification/30-nikaia-tooling.md)'s class. So a
    /// `Vec[T]` is walked *through* where an array is what it holds, and an
    /// array's own elements are walked the same way.
    ///
    /// `None` where the use asks for anything else, and the caller keeps the
    /// `Vec` the walk produced.
    fn array_literal(&mut self, found: &Ty, want: &Ty, value: &Expr, span: &Span) -> Option<Ty> {
        let (
            Ty::Named {
                name, args: wanted, ..
            },
            Expr::ListLit { items, at },
        ) = (want, value)
        else {
            return None;
        };
        // **`Array[T, N]` is the container this is about**, and `Vec[T]` is here
        // only to be walked *through*: `Vec[Array[f64, 2]]` asks for an array
        // one level down, and a rule that read the type it was given whole
        // refused `[[1.0, 2.0], [3.0, 4.0]]` — a correct program, which is the
        // one thing this may not do (Part III, C.4).
        //
        // A `Vec` is walked only where an array is somewhere inside it, so a
        // list of anything else reaches this and leaves it untouched.
        let (element, count) = match (name.as_str(), wanted.as_slice()) {
            (ty::ARRAY, [element, Ty::Count(n)]) => (element, Some(*n)),
            ("Vec", [element]) if holds_an_array(element) => (element, None),
            _ => return None,
        };
        // **The length is part of the type** (D4), so a literal that does not
        // match `N` is refused naming both numbers - and `want` goes back, so
        // that the caller's own fit stays quiet about a type it would otherwise
        // report a second time in numbers the reader has to compare by eye.
        if let Some(n) = count {
            if items.len() as i64 != n {
                self.a_list_the_wrong_length_for_its_array(items.len(), n, span, Counted::Written);
                return Some(want.clone());
            }
            self.checked.array_literals.insert(*at);
        }
        // What the walk agreed this container's elements are - `Vec[E]`'s `E`,
        // and `Unknown` where they said nothing, which is the silence every
        // unanswered question here keeps (Part III, C.4).
        let held = match found {
            Ty::Named { args, .. } => args.first().cloned().unwrap_or(Ty::Unknown),
            _ => Ty::Unknown,
        };
        // **And an element that is itself an array takes its own shape, one
        // level down.** Every element of one literal is the same type, so the
        // first answer is the argument — but all of them are walked, because
        // each one's length is its own refusal and each one's byte is its own
        // line for the emitter.
        let mut agreed: Option<Ty> = None;
        for item in items {
            if let Some(inner) = self.array_literal(&held, element, item, span) {
                agreed.get_or_insert(inner);
            }
        }
        let agreed = agreed.unwrap_or(held);
        Some(Ty::Named {
            name: name.clone(),
            args: match count {
                Some(n) => vec![agreed, Ty::Count(n)],
                None => vec![agreed],
            },
            view: false,
        })
    }
}

/// Where the element count in `NK1157` came from.
///
/// Two sentences under one code, because the rule is one — an `Array[T, N]`
/// takes exactly `N` — and the **way out** is not: a literal's length is on the
/// page and can be edited there, where a computed one came out of a body and
/// *write five elements* is advice nobody can take.
#[derive(Debug, Clone, Copy)]
enum Counted {
    Written,
    Computed,
}

/// What [`Checker::crosses_as_fixed`] found, which is four things and not two.
#[derive(Debug, Clone, Copy)]
enum Crossing {
    /// A computed list of exactly the declared length. It fits.
    Fits,
    /// Refused here, by name — the lengths differ, and the sentence says both.
    Said,
    /// The declaration is a fixed type and the value is growable, and **nothing
    /// computed it**. The mismatch is not the program's: `NK1152` or `NK1127`
    /// has already said what is, and a second sentence about the declaration
    /// would send the reader to a line that is right
    /// ([Part III C.4](../../docs/specification/30-nikaia-tooling.md)).
    Unanswered,
    /// Not this record's pair. The ordinary mismatch applies.
    Other,
}

impl<'a> Checker<'a> {
    /// **Growable going in, fixed coming out**
    /// ([ADR-079](../../docs/specification/adr/adr-079.md) D1) — whether a
    /// value this build **computed** crosses into the program as the fixed type
    /// its declaration names.
    ///
    /// ```nika
    /// fn squares() -> Vec[i64] sync {
    ///     let mut xs = []
    ///     for i in 0..<5 { xs.push(i * i) }
    ///     return xs
    /// }
    ///
    /// comptime TABLE: Array[i64, 5] = squares()
    /// ```
    ///
    /// The body works with a list that does not know its length; the `const`
    /// below cannot hold one, because a `Vec` allocates. **But the value has
    /// been computed by the time this is asked**, so its length is a fact and
    /// `Array[T, N]` is exactly what it crosses as — which is that record's own
    /// title, and the reason `push` did not need a second shape to be built.
    ///
    /// **Only for a value that evaluated.** A `Vec` this compiler could not
    /// compute has no length, so nothing here can say it is five long; that
    /// pair goes to `NK1166` the way it always did, and `NK1127` says the rest.
    ///
    /// See [`Crossing`] for what the four answers mean.
    fn crosses_as_fixed(
        &mut self,
        found: &Ty,
        want: &Ty,
        evaluated: Option<&build_time::Value>,
        span: &Span,
    ) -> Crossing {
        // **A list of pairs crossing as a table**
        // ([ADR-176](../../docs/specification/adr/adr-176.md) D1), which is the
        // same crossing one container out: what a body wrote is growable and
        // what the program holds is fixed, and the **declared type** is what
        // says so. The keys are known here because the build computed them, so
        // a table is a thing this value can be.
        if is_fixed(want) {
            let Ty::Named { args, .. } = want else {
                return Crossing::Other;
            };
            let [key, value] = args.as_slice() else {
                return Crossing::Other;
            };
            let held = match found {
                Ty::Named { name, args, .. } if name == "Vec" || name == "List" => args.first(),
                _ => return Crossing::Other,
            };
            // **`?` fits everything** ([ADR-024](../../docs/specification/adr/adr-024.md)
            // D1), here as in the array crossing below — and that is not a
            // corner: `[]` is a `Vec[?]`, so a table declared over an empty
            // list would otherwise be *this is a `Vec[?]` and the `const` says
            // `Fixed[&str, i64]`*, whose way out asks the reader to write what
            // they already wrote. A table of nothing is a table.
            let pairs = match held {
                None | Some(Ty::Unknown) => true,
                Some(Ty::Tuple(parts)) => {
                    parts.len() == 2 && parts[0].fits(key) && parts[1].fits(value)
                }
                Some(_) => false,
            };
            if !pairs {
                return Crossing::Other;
            }
            return match evaluated {
                Some(build_time::Value::List(_)) => Crossing::Fits,
                // Nothing computed it, so the declaration is not what is wrong
                // — the same reading the other two crossings get.
                _ => Crossing::Unanswered,
            };
        }
        // **A `&[T]` is D1's own view form**
        // ([ADR-179](../../docs/specification/adr/adr-179.md) D1), and it is
        // the crossing without the length: a `Vec[T]` arrives as a view of a
        // run, the way a `String` arrives as a `&str` one arm down. There is no
        // count to check, which is the whole reason this spelling exists — a
        // field whose list is a different length per value has no `Array[T, N]`
        // to be.
        if let Some(element) = slice_element(want) {
            let held = match found {
                Ty::Named { name, args, .. } if name == "Vec" || name == "List" => args.first(),
                _ => return Crossing::Other,
            };
            // `?` fits everything (ADR-024 D1), so an empty list crosses — the
            // corner the table above spells out, for the same reason.
            if matches!(held, Some(held) if !held.fits(element)) {
                return Crossing::Other;
            }
            return match evaluated {
                Some(build_time::Value::List(_)) => Crossing::Fits,
                _ => Crossing::Unanswered,
            };
        }
        // **Text is the other half of D1**, and the simpler one: a `String`
        // arrives as a `&str`, there is no length in the type, and `const X:
        // &str` is what the language below has where `const X: String` is not.
        if matches!(want, Ty::Named { name, args, view: true } if name == "str" && args.is_empty())
            && matches!(found, Ty::Named { name, view: false, .. } if name == "String")
        {
            return match evaluated {
                Some(build_time::Value::Text(_)) => Crossing::Fits,
                // Nothing computed it, so the declaration is not what is wrong
                // — the same reading the list half gets, one type over.
                _ => Crossing::Unanswered,
            };
        }
        let Ty::Named {
            name: wanted,
            args,
            view: false,
        } = want
        else {
            return Crossing::Other;
        };
        if wanted != ty::ARRAY {
            return Crossing::Other;
        }
        let [element, Ty::Count(n)] = args.as_slice() else {
            return Crossing::Other;
        };
        // A growable list of this language, by either spelling. Anything else
        // is not the crossing this record is about.
        let held = match found {
            Ty::Named { name, args, .. } if name == "Vec" || name == "List" => args.first(),
            _ => return Crossing::Other,
        };
        let Some(build_time::Value::List(items)) = evaluated else {
            // **Nothing computed it, so the declaration is not what is wrong.**
            // Saying *this is a `Vec[i64]` and the `const` says `Array[i64, 1]`*
            // would send the reader to a line that is right — the body is what
            // could not be run, and `NK1152` or `NK1127` has just said so.
            return Crossing::Unanswered;
        };
        // **The length is the whole of the type's other half**, so it is asked
        // first and it is asked of the *computed* value - which is the number
        // the reader cannot count off the page, and is why the sentence says it.
        if items.len() as i64 != *n {
            self.a_list_the_wrong_length_for_its_array(items.len(), *n, span, Counted::Computed);
            return Crossing::Said;
        }
        // The element type is the ordinary comparison, and `?` fits everything
        // as it does everywhere else (ADR-024 D1).
        match held {
            Some(held) if !held.fits(element) => Crossing::Other,
            _ => Crossing::Fits,
        }
    }

    /// **What a declared type is called below**, where `rust_constant_type`
    /// cannot say.
    ///
    /// That function knows the types Part I 2.2 offers and nothing else, which
    /// is right for a free function: it has no program to ask. This has one —
    /// a `struct` a `.nika` file declares is a `struct` in the generated file
    /// under the same name, so `const P: Point = …` and
    /// `const ROWS: [Setting; 2] = …` are both things the language below holds.
    ///
    /// **Only the shape, never the fields.** Whether a `Point`'s fields can be
    /// written into a `const` is [`Checker::unwritable_field`]'s question, and
    /// it asks the declaration.
    fn declared_below(&self, ty: &Ty) -> Option<String> {
        match ty {
            // A `struct` or an `enum` this program declares: both are types the
            // generated file has, under the same name.
            Ty::Named {
                name,
                args,
                view: false,
            } if args.is_empty()
                && (self.fields_of(name).is_some() || self.enums.contains_key(name)) =>
            {
                Some(name.clone())
            }
            Ty::Named {
                name,
                args,
                view: false,
            } if name == ty::ARRAY => match args.as_slice() {
                [element, Ty::Count(n)] => {
                    let element =
                        rust_constant_type(element).or_else(|| self.declared_below(element))?;
                    Some(format!("[{element}; {n}]"))
                }
                _ => None,
            },
            // …and a `&[T]` over one, for the same reason one type over
            // ([ADR-179](../../docs/specification/adr/adr-179.md) D1):
            // `&[Setting]` is what a field holding a run of a declared `struct`
            // crosses as, and `rust_constant_type` cannot know `Setting`.
            ty => match slice_element(ty) {
                Some(element) => {
                    let element =
                        rust_constant_type(element).or_else(|| self.declared_below(element))?;
                    Some(format!("&[{element}]"))
                }
                None => None,
            },
        }
    }

    /// **The first field of a declared `struct` that has no `const` form.**
    ///
    /// Asked of the **declaration** and not of the value, which is the whole of
    /// why it is a method: `Bag { items: [1, 2, 3] }` is the same literal
    /// whether `items` is a `Vec[i64]` or an `Array[i64, 3]`, and only one of
    /// those is something a `const` holds. Asking the value instead produced a
    /// way out that could not be taken — *declare it `Array[T, N]`*, on a
    /// program that already had.
    ///
    /// One level, deliberately: a struct inside a struct reports the outer
    /// field, which is the one the reader wrote on this line.
    fn unwritable_field(&self, ty: &Ty) -> Option<(String, String)> {
        let Ty::Named { name, .. } = ty else {
            return None;
        };
        self.fields_of(name)?.into_iter().find_map(|field| {
            // **The types Part I 2.2 offers, and the ones this program
            // declares.** `rust_constant_type` knows the first set and cannot
            // know the second — it has no program to ask — so an
            // `Array[Row, 2]` used to be refused although `[Row; 2]` is exactly
            // the one aggregate a `const` holds.
            if rust_constant_type(&field.ty).is_some() || self.declared_below(&field.ty).is_some() {
                // …and a declared type is only as writable as *its* fields,
                // which is the same question one level in.
                return self.unwritable_field(&self.element_of(&field.ty));
            }
            // A field that is itself a declared `struct` is fine where *its*
            // fields are, which is the same question one level in.
            if matches!(&field.ty, Ty::Named { name, .. } if self.fields_of(name).is_some())
                && self.unwritable_field(&field.ty).is_none()
            {
                return None;
            }
            Some((field.name.clone(), field.ty.text()))
        })
    }

    /// What an `Array[T, N]` holds, and the type itself where it is not one.
    ///
    /// One step, because that is what the question above needs: whether the
    /// fields of what a field *holds* can be written.
    fn element_of(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Named { name, args, .. } if name == ty::ARRAY => {
                args.first().cloned().unwrap_or_else(|| ty.clone())
            }
            // …and a `&[T]` holds its `T` the same way
            // ([ADR-179](../../docs/specification/adr/adr-179.md) D1). Both
            // carry a run, and what the walk above asks is about the run's
            // element rather than about which of the two carries the length.
            _ => match slice_element(ty) {
                Some(item) => item.clone(),
                None => ty.clone(),
            },
        }
    }

    /// **The first part of a computed value the language below cannot write**,
    /// as the **declaration** at that position says
    /// ([ADR-079](../../docs/specification/adr/adr-079.md) D2).
    ///
    /// [`Checker::unwritable_field`] asks a *type*, which is right for a
    /// `struct`: every field is there whichever value it holds. An `enum` is
    /// not like that — `Shape::Empty` is a `const` and `Shape::Many([1, 2])` is
    /// not, and the two have the same declared type — so the variant has to be
    /// read off the **value**, and the walk follows it down.
    ///
    /// Hands back *where* and *what*: `Shape::Many` and `Vec[i64]`.
    fn unwritable_in(&self, value: &build_time::Value) -> Option<(String, String)> {
        match value {
            build_time::Value::Struct { name, fields } => self
                .unwritable_field(&Ty::named(name.clone()))
                .or_else(|| fields.values().find_map(|held| self.unwritable_in(held))),
            build_time::Value::Variant {
                ty,
                variant,
                payload,
            } => self
                .unwritable_payload(ty, variant)
                .or_else(|| payload.iter().find_map(|held| self.unwritable_in(held))),
            build_time::Value::List(items) => {
                items.iter().find_map(|held| self.unwritable_in(held))
            }
            // **A pair, which is what a table's rows are**
            // ([ADR-176](../../docs/specification/adr/adr-176.md) D1). Without
            // this a `Fixed[&str, Bad]` whose `Bad` holds a `Vec` lowered and
            // `rustc` answered about the generated file — the same hole the
            // `enum` had one shape over, and it opened the moment a table
            // learned to hold a declared type
            // ([ADR-180](../../docs/specification/adr/adr-180.md) D1).
            build_time::Value::Tuple(parts) => {
                parts.iter().find_map(|held| self.unwritable_in(held))
            }
            _ => None,
        }
    }

    /// **The value spelled below, read against the declaration at each
    /// position** ([ADR-179](../../docs/specification/adr/adr-179.md) D2).
    ///
    /// [`rust_value`] beside it writes a value on its own, which is right for
    /// every shape whose spelling the value decides: an integer is its digits
    /// wherever it stands. **A list is the one that is not.** The same
    /// `Value::List` is `[1, 2, 3]` against an `Array[i64, 3]` and `&[1, 2, 3]`
    /// against a `&[i64]`, and nothing in the value says which — so the
    /// *declaration* is walked beside it, down through a struct's fields and a
    /// variant's payload, and the `&` is written exactly where a slice was
    /// declared.
    ///
    /// **`&` and not `&[…] as &[T]`**: a `const` promotes an array literal to
    /// `'static`, so `const XS: &[i64] = &[1, 2, 3];` is what Rust already
    /// does with the shorter spelling ([ADR-011](../../docs/specification/adr/adr-011.md)
    /// D2 — the generated file says what the program said).
    ///
    /// Where nothing is declared it falls back to [`rust_value`], which is the
    /// case a `comptime` with no annotation is in.
    fn written_below(&self, value: &build_time::Value, want: Option<&Ty>) -> Option<String> {
        let Some(want) = want else {
            return rust_value(value);
        };
        match value {
            // **The one shape the declaration decides.** `slice_element` is
            // `Some` only for a `&[T]`, so an `Array[T, N]` and a `Vec[T]` take
            // the arm below and come out as `[…]`.
            build_time::Value::List(items) => {
                let (held, borrow) = match slice_element(want) {
                    Some(element) => (element.clone(), "&"),
                    None => (self.element_of(want), ""),
                };
                let mut written = Vec::with_capacity(items.len());
                for item in items {
                    written.push(self.written_below(item, Some(&held))?);
                }
                Some(format!("{borrow}[{}]", written.join(", ")))
            }
            build_time::Value::Struct { name, fields } => {
                let declared = self.fields_of(name);
                let mut written = Vec::with_capacity(fields.len());
                for (field, held) in fields {
                    let want = declared
                        .as_ref()
                        .and_then(|fields| fields.iter().find(|f| &f.name == field))
                        .map(|f| f.ty.clone());
                    written.push(format!(
                        "{field}: {}",
                        self.written_below(held, want.as_ref())?
                    ));
                }
                Some(format!("{name} {{ {} }}", written.join(", ")))
            }
            build_time::Value::Variant {
                ty,
                variant,
                payload,
            } => {
                if payload.is_empty() {
                    return Some(format!("{ty}::{variant}"));
                }
                let declared = self.payload_of(ty, variant).unwrap_or_default();
                let mut written = Vec::with_capacity(payload.len());
                for (at, held) in payload.iter().enumerate() {
                    written.push(self.written_below(held, declared.get(at))?);
                }
                Some(format!("{ty}::{variant}({})", written.join(", ")))
            }
            _ => rust_value(value),
        }
    }

    /// What one variant declares it carries, by position.
    fn payload_of(&self, ty: &str, variant: &str) -> Option<Vec<Ty>> {
        std::iter::once(self.parsed)
            .chain(self.beside.iter().copied())
            .find_map(|parsed| {
                parsed
                    .program
                    .items
                    .iter()
                    .find_map(|item| match &item.node {
                        Item::Enum { name, variants, .. } if parsed.text(*name) == ty => variants
                            .iter()
                            .find(|held| parsed.text(held.name) == variant)
                            .map(|held| match &held.fields {
                                ast::VariantFields::Unit => Vec::new(),
                                ast::VariantFields::Tuple(types) => {
                                    types.iter().map(|t| Ty::from_ast(parsed, t)).collect()
                                }
                                ast::VariantFields::Named(fields) => {
                                    fields.iter().map(|f| Ty::from_ast(parsed, &f.ty)).collect()
                                }
                            }),
                        _ => None,
                    })
            })
    }

    /// What one variant **declares** it carries, where a `const` cannot hold it.
    ///
    /// Read from the item tree rather than from a ledger, because a variant's
    /// payload is not a column: a ledger records what a caller has to know
    /// about a type it cannot see, and this is the type's own shape.
    fn unwritable_payload(&self, ty: &str, variant: &str) -> Option<(String, String)> {
        let carried = std::iter::once(self.parsed)
            .chain(self.beside.iter().copied())
            .find_map(|parsed| {
                parsed
                    .program
                    .items
                    .iter()
                    .find_map(|item| match &item.node {
                        Item::Enum { name, variants, .. } if parsed.text(*name) == ty => variants
                            .iter()
                            .find(|held| parsed.text(held.name) == variant)
                            .map(|held| match &held.fields {
                                ast::VariantFields::Unit => Vec::new(),
                                ast::VariantFields::Tuple(types) => {
                                    types.iter().map(|t| Ty::from_ast(parsed, t)).collect()
                                }
                                ast::VariantFields::Named(fields) => {
                                    fields.iter().map(|f| Ty::from_ast(parsed, &f.ty)).collect()
                                }
                            }),
                        _ => None,
                    })
            })?;
        carried
            .into_iter()
            .find(|held| rust_constant_type(held).is_none() && self.declared_below(held).is_none())
            .map(|held| (format!("{ty}::{variant}"), held.text()))
    }

    /// **`NK1167` one level in**: the value is a `struct` a `const` could hold,
    /// and a **field** of it is not
    /// ([ADR-079](../../docs/specification/adr/adr-079.md) D2).
    ///
    /// The field is named because a reader cannot see which half of
    /// `Bag { items: [1, 2, 3] }` the language below refuses — the struct is
    /// fine and the `Vec` in it is not, and *this cannot be evaluated* leaves
    /// them to work that out.
    fn a_field_that_owns_memory(&mut self, bound: &str, field: &str, held: &str, span: &Span) {
        // **A variant is named whole** — `Shape::Many` — where a field is named
        // under its binding. The `::` is what tells them apart, and it is worth
        // a different sentence: what a reader changes is the *declaration*, and
        // the two declarations do not look alike.
        let carries = field.contains("::");
        let message = match carries {
            true => format!("`{field}` carries a `{held}`, and a `const` cannot hold one"),
            false => {
                format!("`{bound}`'s `{field}` is declared `{held}`, and a `const` cannot hold one")
            }
        };
        let note = match carries {
            true => "an `enum` crosses into the program as a `const` only where the \
                     variant the value *is* can - what owns memory has no `const` form, \
                     which is why a build-time value crosses in its view form (ADR-079 \
                     D1). Another variant of the same `enum` may cross perfectly well"
                .to_string(),
            false => "a `struct` crosses into the program as a `const` only where every \
                      field can - what owns memory has no `const` form, which is why a \
                      build-time value crosses in its view form (ADR-079 D1)"
                .to_string(),
        };
        let help = match carries {
            true => format!(
                "declare what `{field}` carries as the fixed form of it - `Array[T, N]` \
                 for a `Vec`, `&str` for a `String` - or take it out of the `comptime` \
                 and build it while the program runs"
            ),
            false => format!(
                "declare `{field}` as the fixed form of what it holds - `Array[T, N]` \
                 for a `Vec`, `&str` for a `String` - or take it out of the `comptime` \
                 and build it while the program runs"
            ),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1167",
            message,
            notes: vec![note],
            help: Some(help),
        });
    }

    /// **`NK1167`: a `comptime` whose value owns memory**
    /// ([ADR-079](../../docs/specification/adr/adr-079.md) D2).
    ///
    /// *"A `comptime` binding whose value owns memory is refused because of
    /// what it **is**, and the way out is the view-shaped equivalent."* A `Vec`
    /// allocates and `const X: Vec<T>` is not a thing the language below has,
    /// where `const X: [T; N]` is.
    ///
    /// **And the way out names the number**, which is the whole reason this is
    /// worth a code rather than a note on `NK1127`: the build has just computed
    /// the value, so it knows the length the reader would otherwise have to
    /// work out by reading the body.
    fn a_constant_that_owns_memory(
        &mut self,
        bound: &str,
        held: &Ty,
        computed: &build_time::Value,
        span: &Span,
    ) {
        let (owns, fixed, way_out) = match computed {
            build_time::Value::List(items) => {
                let element = match held {
                    Ty::Named { args, .. } => args.first().map(|ty| ty.text()),
                    _ => None,
                };
                let element = element
                    .filter(|ty| ty != "?")
                    .unwrap_or_else(|| "T".to_string());
                (
                    "a `Vec`",
                    "`[T; N]`",
                    format!(
                        "declare it `Array[{element}, {}]` - the build computed {}, \
                         so that is the length",
                        items.len(),
                        plural(items.len(), "element")
                    ),
                )
            }
            // **Text is the case with no length in it**, which is why the way
            // out is shorter: `&str` says the whole thing.
            _ => (
                "a `String`",
                "`&str`",
                "declare it `&str` - the text is the build's, so what the program \
                 holds is a view of it"
                    .to_string(),
            ),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1167",
            message: format!(
                "`{bound}` is a `{}`, and a `const` cannot hold one",
                held.text()
            ),
            notes: vec![format!(
                "{owns} owns memory and allocates, and the language below has no \
                 `const` that holds one - what it does have is {fixed}, which is why a \
                 build-time value crosses in its view form (ADR-079 D1)"
            )],
            help: Some(way_out),
        });
    }

    /// `NK1157`: a list literal standing where an `Array[T, N]` is wanted, with
    /// a different number of elements
    /// ([ADR-152](../../docs/specification/adr/adr-152.md) D4).
    ///
    /// **Both numbers**, because the length *is* part of the type and what the
    /// reader has to do about it is count: a message saying only that one array
    /// type is not another leaves the counting undone.
    fn a_list_the_wrong_length_for_its_array(
        &mut self,
        written: usize,
        wanted: i64,
        span: &Span,
        how: Counted,
    ) {
        let had = plural(wanted.unsigned_abs() as usize, "element");
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1157",
            message: match how {
                Counted::Written => format!(
                    "this writes {}, and the array holds {wanted}",
                    plural(written, "element")
                ),
                Counted::Computed => format!(
                    "this computed {}, and the array holds {wanted}",
                    plural(written, "element")
                ),
            },
            notes: vec![
                "the length is part of the type, so an `Array[T, N]` takes exactly `N` \
                 elements (ADR-152 D4)"
                    .to_string(),
            ],
            help: Some(match how {
                Counted::Written => {
                    format!("write {had}, or declare the array the length this literal is")
                }
                // **Not *write five elements***, which is what the other half
                // says and is advice nobody can take here: the number came out
                // of a body, so the two things a reader can change are the body
                // and the declaration.
                Counted::Computed => format!(
                    "declare the array the length this computes, or have the body \
                     produce {had}"
                ),
            }),
        });
    }

    /// `NK1154`: two elements of one list literal are not the same type
    /// ([ADR-135](../../docs/specification/adr/adr-135.md) D1).
    ///
    /// **Named rather than widened.** A list holds one type, so the two are a
    /// mistake in one of them and the message says which two - exactly as a
    /// `let` with an annotation does, which is the shape this borrows.
    fn a_list_whose_elements_disagree(
        &mut self,
        first_kind: &str,
        first: &str,
        other_kind: &str,
        other: &Ty,
        span: &Span,
    ) {
        // What to call each side: its **type** where this checker worked one
        // out, and its **kind** where it did not - `a number`, `text` - because
        // a caret with `?` on both sides of *and a list holds one type* says
        // nothing a reader can act on.
        let name = |kind: &str, ty: &str| match ty {
            "?" | "" => kind.to_string(),
            ty => format!("`{ty}`"),
        };
        let (first, other) = (name(first_kind, first), name(other_kind, &other.text()));
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1154",
            message: format!("this list holds {first} and {other}, and a list holds one type"),
            notes: vec![
                "the element type is what the elements agree on, and the first one that \
                 has a type is what the rest answer to (Part I, 2.2)"
                    .to_string(),
            ],
            help: Some("convert the one that does not fit, or write two lists".to_string()),
        });
    }

    /// `NK1153`: an empty list whose element type nothing ever says
    /// ([ADR-135](../../docs/specification/adr/adr-135.md) D2).
    ///
    /// **Refused and not guessed.** A default element type would be a type
    /// nobody wrote, and claiming something the program does not say is the one
    /// thing this checker may never do
    /// ([Part III C.4](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **Asked only where nothing at all uses the name**, which is the case D2
    /// writes down. A use this checker cannot read a type out of -
    /// `xs.len()` and nothing else - leaves the question to the language below
    /// rather than answering it wrongly, for C.4's reason again: the cost of
    /// silence is a backend message, and the cost of speaking is a correct
    /// program refused.
    fn an_empty_list_with_no_element_type(&mut self, name: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1153",
            message: format!("`{name}` is an empty list with no element type"),
            notes: vec![
                "an empty list carries no element type, so it takes one from the first use \
                 that needs one - and nothing here uses it (Part I, 2.2)"
                    .to_string(),
            ],
            help: Some(format!(
                "write the type - `let {name}: Vec[i64] = []` - or give it a first element"
            )),
        });
    }

    /// `NK1152`: a build-time body the rule forbids
    /// ([ADR-075](../../docs/specification/adr/adr-075.md) D1, D2).
    ///
    /// **Not `NK1127`**, which says *this compiler cannot evaluate it*. Here it
    /// can: the shape is understood, the body is in hand, and the ledger says
    /// the call may not be made while the program is built. Two claims, two
    /// codes, because a reader does two different things about them — wait for
    /// a stage, or change the callee.
    fn a_body_that_may_not_run_at_build_time(&mut self, callee: &str, because: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1152",
            message: format!("`{callee}` may not be called while the program is built"),
            notes: vec![
                because.to_string(),
                "the rule is two ledger columns and not a list of allowed functions, so \
                 what a build-time body may do is what the compiler already derives about \
                 every function it sees"
                    .to_string(),
            ],
            help: Some(
                "call it while the program runs - a `let` rather than a `comptime` - or \
                 make the callee `sync` and reach nothing outside the build"
                    .to_string(),
            ),
        });
    }

    /// The call-depth limit, which is **not** a step budget
    /// ([ADR-075](../../docs/specification/adr/adr-075.md) D4).
    ///
    /// That record deliberately has none and wrote down what it costs: a body
    /// that does not terminate hangs the build. A *recursion* that does not
    /// terminate is a different failure — it takes this compiler's stack down
    /// with it, and a compiler that falls over is not the hang D4 accepted.
    fn a_build_time_call_went_too_deep(&mut self, callee: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1152",
            message: format!("`{callee}` calls itself too deeply to evaluate while building"),
            notes: vec![
                "there is no step budget on a build-time body (ADR-075 D4) and a loop that \
                 does not end hangs the build; what this bounds is the **call depth**, so \
                 that a recursion without a base case says so rather than taking this \
                 compiler's stack with it"
                    .to_string(),
            ],
            help: Some(
                "give the recursion a base case, or compute it while the program runs".to_string(),
            ),
        });
    }

    /// `NK1149`: a type's constructor written `Type::new`
    /// ([ADR-140](../../docs/specification/adr/adr-140.md) D2).
    ///
    /// The anonymous constructor is what a `.nika` file writes
    /// (`pub fn(first: i32)`, Part I 4.2) and `new` is Rust's convention
    /// reaching through a hand-written ledger. One convention, and it is this
    /// language's own: `Vec()`, `String()`, `HashMap()`, `Stats(first)`.
    ///
    /// **Asked of the name and not of the position**, so the value form is
    /// refused too — `par_fold(M, Summary::new, …)` is how `1brc.nika` wrote it,
    /// which is the case D2 names.
    ///
    /// The ledger's key stays `Type::new`, because that is what the **lowering**
    /// writes and the lowering is name for name
    /// ([ADR-011](../../docs/specification/adr/adr-011.md) D2). What this
    /// refuses is the *source* spelling.
    fn a_constructor_written_as_new(&mut self, name: &str, span: &Span) {
        let Some(ty) = name.strip_suffix("::new") else {
            return;
        };
        if !self.own.functions.contains_key(name) && self.library.lookup(name).is_none() {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1149",
            message: format!("`{ty}` is constructed by its anonymous constructor, not by `new`"),
            notes: vec![
                "a type is constructed the way a `.nika` file declares one - `pub fn(first: \
                 i32)`, Part I 4.2 - and `new` is the neighbouring language's convention \
                 reaching through a hand-written ledger (ADR-140 D2). One convention, and \
                 it is this language's own"
                    .to_string(),
            ],
            help: Some(format!(
                "write `{ty}(…)`, or `{ty}` where the constructor is the value"
            )),
        });
    }

    /// **`NK1171`: `X::y`, where `X` is a type this program declares and `y` is
    /// nothing it has.**
    ///
    /// **Only where the compiler *knows*.** A path whose head names nothing at
    /// all — `nowhere::wobble` — is a different question and is not this one:
    /// a module of a package, a foreign crate's item and a name the ledger has
    /// not been told about all look the same from here, and refusing on absence
    /// would refuse correct programs. That case is
    /// [`open-work.md`](../../docs/open-work.md) §1.2, with the measurement
    /// that says how rare it is. What is answered here is the case where this
    /// compiler has read the declaration and can see that the name is not in
    /// it: the same knowledge `NK1135`'s map and the exhaustiveness check
    /// already read, asked one question earlier.
    ///
    /// **What it was before.** `Op::Mul` beside `enum Op { Add, Sub }` lowered,
    /// and `rustc` refused the **generated file** — [Part III
    /// C.1](../../docs/specification/30-nikaia-tooling.md)'s class. The
    /// exhaustiveness check could not say it: it reads which variants an arm
    /// *covered*, so a misspelling covers none and what it reports is the
    /// variant that is missing; with an `else` arm beside it, it reports
    /// nothing.
    fn a_member_this_type_does_not_have(&mut self, ty: &str, member: &str, span: &Span) {
        // Anything a ledger records under this exact key is a real item —
        // `Summary::merge`, a `new` a hand-written ledger carries — and says so
        // for itself.
        if self.resolve(&format!("{ty}::{member}")).is_some() {
            return;
        }
        // **A type parameter is a type this compiler has not read either**, so
        // it fails open the way an unknown head does — with one exception,
        // which is the name a reader of Part II 10.3 writes. `[T: Struct]`
        // became a legal bound in the same package as this line, and without it
        // `T::fields` went from `NK1135` on the bound to **silence**, and from
        // there to `rustc` about the generated file
        // ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
        if self.type_parameters.contains_key(ty) {
            if member == "fields" || member == "variants" {
                self.a_shape_that_is_not_reachable_yet(ty, member, span);
            }
            return;
        }
        if let Some(variants) = self.enums.get(ty) {
            let known: Vec<&str> = variants.iter().map(String::as_str).collect();
            let listed = known
                .iter()
                .map(|v| format!("`{ty}::{v}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let help = match nearest(member, &known) {
                Some(near) => format!("did you mean `{ty}::{near}`?"),
                None if known.is_empty() => {
                    format!("`{ty}` declares no variants, so there is no name to write here")
                }
                None => format!("write one of {listed}"),
            };
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK1171",
                message: format!("`{ty}` has no variant `{member}`"),
                notes: vec![format!(
                    "this compiler read the declaration, so a name beside it is a misspelling \
                     rather than something nobody has told it about - `{ty}` is {listed} and \
                     nothing else (Part I, 4.3)"
                )],
                help: Some(help),
            });
            return;
        }
        let Some(fields) = self.fields_of(ty) else {
            // **And if the head is not a name this program has either**, the
            // path is `NK1181` rather than silence.
            self.a_head_nothing_declares(ty, &format!("{ty}::{member}"), span);
            return;
        };
        // **`T::fields` is specified and unbuilt**, and that is worth its own
        // sentence: a reader who wrote it read Part II 10.3, and *`Point` has
        // no member `fields`* would send them looking for a spelling that does
        // not exist ([ADR-088](../../docs/specification/adr/adr-088.md) §5).
        if member == "fields" || member == "variants" {
            self.a_shape_that_is_not_reachable_yet(ty, member, span);
            return;
        }
        let named: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
        let listed = named
            .iter()
            .map(|field| format!("`{field}`"))
            .collect::<Vec<_>>()
            .join(", ");
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1171",
            message: format!("`{ty}` is a struct, and nothing it declares is called `{member}`"),
            notes: vec![format!(
                "`::` reaches an **item** a type declares - a method of an `impl`, a variant of an \
                 `enum` - and a field is not one: it is read from a value. `{ty}` holds {listed}"
            )],
            help: Some(match nearest(member, &named) {
                Some(near) => format!("read it from a value: `value.{near}`"),
                None => format!("write `impl {ty} {{ … }}` if `{member}` is meant to be a method"),
            }),
        });
    }

    /// **`NK1171`, for a shape member written where no bound reaches it.**
    ///
    /// `T::fields` is reached under `[T: Struct]` and `T::variants` under
    /// `[T: Enum]` (Part II 10.3). A reader who writes either has read that
    /// section, so the message names the bound that makes it reachable rather
    /// than saying the type has no such member.
    fn a_shape_that_is_not_reachable_yet(&mut self, ty: &str, member: &str, span: &Span) {
        let bound = match member {
            VARIANTS => "Enum",
            _ => "Struct",
        };
        let note = match self.type_parameters.get(ty) {
            Some(_) => format!(
                "a type's shape is reached through a **bound**: `{ty}::{member}` needs \
                 `[{ty}: {bound}]` (Part II 10.3)"
            ),
            None => format!(
                "a type's shape is reached through a **bound** rather than by name - \
                 `fn describe[T: {bound}](value: T)`, and then `T::{member}` inside it \
                 (Part II 10.3)"
            ),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1171",
            message: format!("`{ty}::{member}` is reached under a `{bound}` bound"),
            notes: vec![note],
            help: Some(format!(
                "write `fn describe[T: {bound}](value: T)` and `for x in T::{member}` inside it"
            )),
        });
    }

    /// One pattern's path, where it names a type and a member of it.
    ///
    /// One segment **binds a name** rather than naming a variant, which is the
    /// rule [`Checker::pattern_names`] already reads, and an empty one is the
    /// bare tuple `(0, 0)`.
    fn a_path_in_a_pattern(&mut self, path: &[Ident], span: &Span) {
        let names: Vec<String> = path
            .iter()
            .map(|s| self.parsed.text(*s).to_string())
            .collect();
        let [ty, member] = names.as_slice() else {
            return;
        };
        if self.is_variant(ty, member) {
            return;
        }
        self.a_member_this_type_does_not_have(ty, member, span);
    }

    /// The same question asked of a `match` arm's pattern
    /// ([ADR-137](../../docs/specification/adr/adr-137.md) D1's shapes), which
    /// is where a misspelled variant is most likely to be written and least
    /// likely to be noticed.
    fn a_pattern_naming_a_member_a_type_does_not_have(
        &mut self,
        pattern: &MatchPattern,
        span: &Span,
    ) {
        match pattern {
            MatchPattern::Path(path) | MatchPattern::Named { path, .. } => {
                self.a_path_in_a_pattern(path, span);
            }
            // **The parts are patterns**, so the walk goes into them: a
            // misspelling inside `Event::Click(Op::Mul)` is the same mistake one
            // level down, and stopping at the outer path would find the shallow
            // half of a rule.
            MatchPattern::Tuple { path, parts } => {
                self.a_path_in_a_pattern(path, span);
                for part in parts {
                    self.a_pattern_naming_a_member_a_type_does_not_have(part, span);
                }
            }
            MatchPattern::Or(alternatives) => {
                for alternative in alternatives {
                    self.a_pattern_naming_a_member_a_type_does_not_have(alternative, span);
                }
            }
            MatchPattern::Otherwise | MatchPattern::Literal(_) | MatchPattern::Range { .. } => {}
        }
    }

    /// `NK1144`: a `let` whose only name is the ignore pattern
    /// ([ADR-126](../../docs/specification/adr/adr-126.md) D2).
    fn a_let_that_binds_nothing(&mut self, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1144",
            message: "`_` ignores a value inside a pattern, and this `let` has nothing else \
                      to bind"
                .to_string(),
            notes: vec![
                "a call made for its effect is written as the call, `f()`; a resource torn \
                 down at a moment of the program's choosing is closed by name or lives in a \
                 scope it can end with (Part I, 6.4)"
                    .to_string(),
                "and what this lowers to is Rust's `let _ =`, which **discards** the value \
                 rather than binding it - so the cleanup runs here and not at the end of the \
                 block, which is the one thing a reader cannot see in the line"
                    .to_string(),
            ],
            help: Some("write the expression as a statement, or bind it to a name".to_string()),
        });
    }

    /// **`NK1172`: one field, named twice.**
    ///
    /// `Point { x: 1, x: 2 }` lowered, and `rustc` answered about the
    /// **generated file** — [Part III
    /// C.1](../../docs/specification/30-nikaia-tooling.md)'s class. The rule is
    /// the literal's and [ADR-118](../../docs/specification/adr/adr-118.md) D1
    /// restates it for `with`, which borrows the braces; both come here, because
    /// one rule written twice is two rules waiting to disagree.
    fn a_field_written_twice(&mut self, ty: &str, field: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1172",
            message: format!("`{field}` is named twice here, and `{ty}` has one of it"),
            notes: vec![
                "a field list gives each field its value once - there is no meaning a \
                 compiler may pick between, and the later one silently winning is the \
                 reading this language does not offer (Part I, 4.2)"
                    .to_string(),
            ],
            help: Some(format!("take one of the two `{field}` out")),
        });
    }

    /// **`NK1175`: a file this build may not read**
    /// ([ADR-072](../../docs/specification/adr/adr-072.md) D1, D3).
    ///
    /// One claim and four reasons. The first is the one a project that never
    /// intends to read anything still gets, and it costs nothing to keep: with
    /// no list in effect the whole class is off, so *this build reads nothing
    /// while building* is what happens rather than something somebody has to
    /// promise.
    ///
    /// Each reason carries the way out that can actually be taken
    /// ([Part III C.2](../../docs/specification/30-nikaia-tooling.md)), and the
    /// three namings are why they differ: *add the flag* is not the answer to a
    /// path missing from the list, and *add the line* is not the answer to a
    /// build run with the reads switched off.
    fn a_file_this_build_may_not_read(&mut self, path: &str, why: &Denied, span: &Span) {
        let (note, way_out) = match why {
            Denied::NoList => (
                "a build given no allowlist reads nothing while it builds (ADR-072 D1), and \
                 that is the default rather than a mode: *this build reads nothing* is what \
                 happens when nothing is passed, not a claim somebody keeps true"
                    .to_string(),
                "pass `--allow-read-from-list=<file>`, and name this path in that file".to_string(),
            ),
            Denied::NotListed { list } => (
                format!(
                    "a file is named in three places and a read missing any of them is \
                     refused (ADR-072 D3): the flag says a list is in effect, the list says \
                     which files, and the literal says which one this line reads. `{list}` \
                     does not name `{path}`"
                ),
                format!("write `{path}` on a line of `{list}`"),
            ),
            Denied::OutsideTheRoot => (
                "a build reads under the project root and nowhere else, and this path \
                 leaves it - it is absolute, or it climbs with `..`. The check is on the \
                 literal rather than on where a symlink points, because what a reader can \
                 decide by looking at the line is the property the three namings buy \
                 (ADR-072 D4)"
                    .to_string(),
                "write the path relative to the project root".to_string(),
            ),
            Denied::Unreadable { because } => (
                format!(
                    "the allowlist names `{path}` and this build could not read it: \
                     {because}"
                ),
                "add the file, or take the line out of the allowlist".to_string(),
            ),
            Denied::NotText => (
                format!(
                    "`{path}` was read and is not text. What crosses from build time to \
                     run time is a `&str` (ADR-079 D1), so the bytes have to be UTF-8"
                ),
                "read a text file here; bytes that are not text have no crossed form yet"
                    .to_string(),
            ),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1175",
            message: format!("this build may not read `{path}`"),
            notes: vec![note],
            help: Some(way_out),
        });
    }

    /// **The instantiations that stand in the program's other files**
    /// ([ADR-181](../../docs/specification/adr/adr-181.md) D2).
    ///
    /// A copy is written by the unit that declares the function, and which
    /// copies there are is decided at the **call** — which may stand in any
    /// file of the package, because they share one namespace (Part I 9.1). So
    /// this unit asks the others what they called its shape walks with, and
    /// without it `describe` in `shapes.nika` called from `main.nika` is a
    /// function with no copies and no generic original: a name that reaches
    /// the language below undeclared, which is
    /// [Part III C.1](../../docs/specification/30-nikaia-tooling.md)'s class.
    ///
    /// **Nothing to ask about costs nothing**: a unit that declares no shape
    /// walk returns before a second file is looked at, which is every unit of
    /// every program in the corpus.
    fn instantiations_beside(&mut self) {
        if self.harvesting || self.beside.is_empty() {
            return;
        }
        let mine: BTreeSet<String> = self
            .parsed
            .program
            .items
            .iter()
            .filter_map(|item| match &item.node {
                Item::Fn {
                    name: Some(name), ..
                } => Some(self.parsed.text(*name).to_string()),
                _ => None,
            })
            .filter(|name| self.walks_fields.contains_key(name))
            .collect();
        if mine.is_empty() {
            return;
        }
        let mut found: Vec<((String, String), Vec<FieldContract>)> = Vec::new();
        for other in self.beside {
            if std::ptr::eq(*other, self.parsed) {
                continue;
            }
            found.extend(
                instantiations_in(
                    other,
                    self.beside,
                    self.own,
                    self.library,
                    &self.modules,
                    self.reads,
                )
                .into_iter()
                .filter(|((name, _), _)| mine.contains(name)),
            );
        }
        // **This unit's own calls win**, because they were typed by the walk
        // that also holds the spans: the two agree, and a harvest that
        // disagreed would be a second answer to a question already answered.
        for (instantiation, fields) in found {
            self.checked.unrolled.entry(instantiation).or_insert(fields);
        }
    }

    /// **The body, once per unrolled turn**
    /// ([ADR-088](../../docs/specification/adr/adr-088.md) D5).
    ///
    /// `field.of(value)` has a different type in each turn, so there is no one
    /// type to check the body against — and a body that is wrong for one field
    /// is **right** for the others, on the same line. So the walk is repeated
    /// with the field bound, and every finding it makes carries the note that
    /// says which turn it came from. Without that line this is the error class
    /// C++ templates carried for twenty years.
    ///
    /// **After the ordinary walk**, because the instantiations are what that
    /// walk found: a call may stand above the function it names, and which
    /// types a function is used with is not knowable until every call has been
    /// read.
    ///
    /// The cost is fields × instantiations, and only for the functions that
    /// walk a shape and only for the types actually used (D5). A function
    /// nobody calls is not unrolled at all.
    fn unroll(&mut self) {
        let instantiations: Vec<(String, String, Vec<FieldContract>)> = self
            .checked
            .unrolled
            .iter()
            .map(|((name, on), fields)| (name.clone(), on.clone(), fields.clone()))
            .collect();
        for (name, on, fields) in instantiations {
            let Some(item) = self.parsed.program.items.iter().find(|item| {
                matches!(&item.node, Item::Fn { name: Some(written), .. }
                    if self.parsed.text(*written) == name)
            }) else {
                continue;
            };
            let item = item.clone();
            for field in fields {
                let before = self.checked.findings.len();
                let outer = self.unrolling.replace((on.clone(), field.clone()));
                self.function(&item.node, None);
                self.unrolling = outer;
                // **What the generic walk already said is not said again.**
                // The same body was walked once with nothing known, and a
                // message from that walk is about the **function** — a
                // misspelled member, say — so repeating it once per field
                // would turn one mistake into as many as the type has fields.
                // What is left is what this turn alone found, which is D5's
                // whole point.
                let already: BTreeSet<(&'static str, usize)> = self.checked.findings[..before]
                    .iter()
                    .map(|f| (f.code, f.span.start))
                    .collect();
                let mut fresh: Vec<Finding> = self.checked.findings.split_off(before);
                fresh.retain(|f| !already.contains(&(f.code, f.span.start)));
                for found in &mut fresh {
                    let (member, one) = match self.enums.contains_key(&on) {
                        true => (VARIANTS, "variant"),
                        false => (FIELDS, "field"),
                    };
                    found.notes.push(format!(
                        "unrolling `{}::{member}` for `{on}`, at {one} `{}`",
                        self.walks_fields
                            .get(&name)
                            .map(String::as_str)
                            .unwrap_or("T"),
                        field.name
                    ));
                }
                self.checked.findings.extend(fresh);
            }
        }
    }

    /// **One instantiation of a function that walks a type's fields**
    /// ([ADR-181](../../docs/specification/adr/adr-181.md) D2).
    ///
    /// The type argument is read off the **first argument**, which is the shape
    /// Part II 10.3 writes and the only one this compiler can read: a bound
    /// binds `T` from a parameter's type, and `describe(u)` is where `u`'s type
    /// says which struct. A call whose argument this checker did not type is
    /// left alone rather than guessed at — `?` fits everything
    /// ([ADR-024](../../docs/specification/adr/adr-024.md) D1) and an
    /// instantiation made from one would be a wrong answer where a missing one
    /// is right ([ADR-010](../../docs/specification/adr/adr-010.md) D1).
    /// The element type of `T::member`, where `T` is a type parameter whose
    /// shape bound makes that member reachable.
    fn shape_element(&self, ty: &str, member: &str) -> Option<&'static str> {
        self.type_parameters.get(ty)?.iter().find_map(|bound| {
            shape_member(bound)
                .and_then(|(reached, element)| (reached == member).then_some(element))
        })
    }

    /// What a shape walk is unrolled over: a `struct`'s fields, or an
    /// `enum`'s variants, each a name and nothing a turn has to type.
    fn shape_of(&self, on: &str) -> Option<Vec<FieldContract>> {
        if let Some(known) = self.enums.get(on) {
            // **In the order the declaration wrote them**, which the set of
            // names does not keep: this unit's own `enum`, else the ledger's.
            let declared = self
                .parsed
                .program
                .items
                .iter()
                .find_map(|item| match &item.node {
                    Item::Enum { name, variants, .. } if self.parsed.text(*name) == on => Some(
                        variants
                            .iter()
                            .map(|v| self.parsed.text(v.name).to_string())
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                });
            let variants = declared
                .or_else(|| {
                    [self.own, self.library].iter().find_map(|ledger| {
                        ledger
                            .types
                            .get(on)
                            .map(|c| c.variants.iter().map(|v| v.name.clone()).collect())
                    })
                })
                .filter(|written: &Vec<String>| written.len() == known.len())
                .unwrap_or_else(|| known.iter().cloned().collect());
            return Some(
                variants
                    .iter()
                    .map(|variant| FieldContract {
                        name: variant.clone(),
                        ty: Ty::named(ty::VARIANT),
                        public: true,
                    })
                    .collect(),
            );
        }
        self.fields_of(on)
    }

    fn an_instantiation(&mut self, name: &str, args: &[Expr], span: &Span) {
        if !self.walks_fields.contains_key(name) {
            return;
        }
        let Some(first) = args.first() else {
            return;
        };
        // **Walked with the walk switched off**, because the argument is walked
        // again by the ordinary path below and a message said twice is two
        // problems to a reader.
        let before = self.checked.findings.len();
        let given = self.expr(first, span);
        self.checked.findings.truncate(before);
        let Ty::Named { name: on, .. } = &given else {
            return;
        };
        let Some(fields) = self.shape_of(on) else {
            return;
        };
        let (on, name) = (on.clone(), name.to_string());
        self.checked
            .unrolled_calls
            .insert(span.start, specialised(&name, &on));
        self.checked.unrolled.insert((name, on), fields);
    }

    /// **A cast whose operand is a `for` binding**
    /// ([ADR-182](../../docs/specification/adr/adr-182.md) D1).
    ///
    /// `for n in NS { sum = sum + (n as i64) }` reached the language below as
    /// `n as i64` over a `&i32`, and what came back was *casting `&i32` as
    /// `i64` is invalid* with a way out that reads *dereference the
    /// expression* — a noun and an instruction about a file nobody wrote
    /// ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **Only a bare name**, because that is the only operand shape that can
    /// arrive as a view: a call, a literal and an arithmetic expression each
    /// come to a value, and a field read or an index is already taken apart by
    /// the lowering that wrote it.
    fn a_cast_over_a_lent_binding(&mut self, operand: &Expr, span: &Span) {
        let Expr::Variable(name) = operand else {
            return;
        };
        let name = self.parsed.text(*name).to_string();
        if self.binding(&name).is_some_and(|local| local.lent) {
            self.checked.viewed_numbers.insert((span.start, name));
        }
    }

    /// **A `let` whose declared type is a number and whose value is a `for`
    /// binding** ([ADR-182](../../docs/specification/adr/adr-182.md) D5).
    ///
    /// `for m in xs { let q: i64 = m }` reached the language below as
    /// `let q: i64 = m;` over a `&i64`, and what came back was *mismatched
    /// types*, with *consider dereferencing the borrow* as the way out — about
    /// a borrow the source does not contain
    /// ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **Only where the annotation is a number**, because that is the whole of
    /// what `num::value` answers and the whole of what may be read through a
    /// view without a copy being **inserted**
    /// ([ADR-008](../../docs/specification/adr/adr-008.md) D5). A `let r: Row =
    /// <binding>` is a different question with a different answer, and it is
    /// [`open-work.md`](../../docs/open-work.md) §1.6's to carry until
    /// somebody decides it.
    fn a_number_read_through_a_lent_binding(&mut self, want: &Ty, value: &Expr, span: &Span) {
        let Ty::Named {
            name: want,
            args,
            view,
        } = want
        else {
            return;
        };
        if *view {
            return;
        }
        const NUMBERS: [&str; 6] = ["i32", "i64", "u8", "f64", "bool", "char"];
        if args.is_empty() && NUMBERS.contains(&want.as_str()) {
            self.a_cast_over_a_lent_binding(value, span);
            return;
        }
        // **And everything else is a refusal**
        // ([ADR-185](../../docs/specification/adr/adr-185.md) D1): a number is
        // `Copy` and reading one through a view inserts nothing, and a `struct`
        // is not — so the same answer here would be a copy the source did not
        // write, which [ADR-008](../../docs/specification/adr/adr-008.md) D5
        // forbids in as many words.
        let Expr::Variable(name) = value else {
            return;
        };
        let name = self.parsed.text(*name).to_string();
        if !self.binding(&name).is_some_and(|local| local.lent) {
            return;
        }
        let want = want.clone();
        self.a_copy_the_source_did_not_write(&name, &want, span);
    }

    /// **`NK1183`: a `let` that declares the element's type over a `for`
    /// binding** ([ADR-185](../../docs/specification/adr/adr-185.md) D1).
    ///
    /// A `for` lends ([ADR-094](../../docs/specification/adr/adr-094.md) D4),
    /// so the binding is a **view** of the element and an annotation naming the
    /// element is a type the value does not have. What came back was `rustc`'s
    /// *mismatched types*, with *consider using clone here* as the way out —
    /// an instruction to insert exactly the copy
    /// [ADR-008](../../docs/specification/adr/adr-008.md) D5 says is written
    /// and never inserted, about a file nobody wrote
    /// ([Part III C.1](../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **The way out is the annotation coming off**, and it is a way out the
    /// program can take: a view reads the same — `copy.a` reaches through it —
    /// which is what makes this [C.2](../../docs/specification/30-nikaia-tooling.md)'s
    /// shape rather than C.1's alone. The compiler knew both that the
    /// annotation was wrong and what to write instead, and said neither.
    ///
    /// **And the copy is named as the other answer**, because sometimes it is
    /// the one that was meant — but it is the *program's* to write.
    fn a_copy_the_source_did_not_write(&mut self, name: &str, want: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1183",
            message: format!(
                "`{name}` is a view of a `{want}`, and this `let` declares a `{want}`"
            ),
            notes: vec![format!(
                "a `for` lends what it walks (Part I, 6.5), so `{name}` points at an element the \
                 collection still owns - and a `{want}` here would be a **copy**, which this \
                 language writes and never inserts (ADR-008 D5)"
            )],
            help: Some(format!(
                "take the annotation off: `let … = {name}` binds the view, and reading through \
                 one is the same reading. Where a copy is what was meant, write it - \
                 `{name}.clone()` for a type that offers one"
            )),
        });
    }

    /// **`NK1180`: a reflected field answers `.name` and `.of(value)`**
    /// ([ADR-088](../../docs/specification/adr/adr-088.md) D2).
    ///
    /// The two are the whole of what Part II 10.3 gives one, and the list is
    /// short enough to print — which is what makes this a misspelling rather
    /// than something nobody has told the compiler about.
    /// **`NK1180` for a variant**: a reflected variant answers `.name` and
    /// `.is(value)`.
    fn a_reflected_variant_has_two_members(&mut self, member: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1180",
            message: format!("a variant of `T::variants` has no `{member}`"),
            notes: vec![
                "what a reflected variant answers is `.name`, the variant's own name as text, \
                 and `.is(value)`, whether a value is that variant (Part II 10.3)"
                    .to_string(),
            ],
            help: Some(
                "write `.name` or `.is(value)`; what a variant carries is read with a `match`"
                    .to_string(),
            ),
        });
    }

    fn a_reflected_field_has_two_members(&mut self, member: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1180",
            message: format!("a field of `T::fields` has no `{member}`"),
            notes: vec![
                "what a reflected field answers is `.name`, the field's own name as text, \
                 and `.of(value)`, what that field holds on this value (Part II 10.3, \
                 ADR-088 D2) - and nothing else, because a field descriptor is a shape \
                 this compiler makes rather than a type a program declares"
                    .to_string(),
            ],
            help: Some("write `.name` or `.of(value)`".to_string()),
        });
    }

    /// **`NK1179`: a run this body owns, put where a view of one is declared**
    /// ([ADR-179](../../docs/specification/adr/adr-179.md) D2).
    ///
    /// `&[T]` is the **crossed** form: what a build hands the program, and what
    /// another view may be copied from. A `Vec[T]` a body just built is not
    /// one, and there is no `&` the compiler may write here — a struct outlives
    /// the expression that fills it, so a view into a local would be a
    /// reference to something already gone
    /// ([ADR-107](../../docs/specification/adr/adr-107.md) D3: no copy and no
    /// borrow the program did not write).
    ///
    /// **A parameter is the case where the `&` *is* the compiler's**
    /// ([ADR-094](../../docs/specification/adr/adr-094.md) D1): the callee reads
    /// and the caller keeps, so `total(xs)` for a `Vec[i64]` needs no word. This
    /// is the other side of that line, and it is worth a message of its own
    /// because the two look identical on the page.
    ///
    /// Without it `rustc` answered *expected `&[Setting]`, found
    /// `Vec<Setting>`* about the generated file, which is
    /// [Part III C.1](../../docs/specification/30-nikaia-tooling.md)'s class —
    /// and the line it answered about was a **grammar action**, where the way
    /// out is not *write a `&`* at all.
    fn a_run_this_body_owns(&mut self, owner: &str, field: &str, held: &str, span: &Span) {
        // **A type this checker did not work out is not named in the way out**
        // ([Part III C.2](../../docs/specification/30-nikaia-tooling.md): *a way
        // out that cannot be taken is not one*). Inside a grammar action that
        // is the usual case - a rule's binding has no type here - so the
        // sentence names what the **parse** owns rather than printing a `?`
        // the reader would have to write.
        let known = held != "?";
        let owned = match known {
            true => format!("a `{held}` this body owns"),
            false => "a value this body owns".to_string(),
        };
        let way_out = match known {
            true => format!(
                "declare `{field}` as `{held}` where the program builds it while it runs, \
                 or fill it from a `comptime` - a build-time value crosses into a `&[T]` \
                 and the run it views is the program's own text"
            ),
            false => format!(
                "declare `{field}` as `Vec[T]`, which is what a parse builds - and where \
                 the program wants the view, cross it: a rule handing back a `Vec[T]` \
                 reaches a `comptime` declared `&[T]`"
            ),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1179",
            message: format!("`{owner}.{field}` is a view of a run, and this is {owned}"),
            notes: vec![
                "a `&[T]` is what a **build** hands the program (ADR-079 D1) or what \
                 another view is read from - it points at a run somebody else keeps, and \
                 a value built here is gone when the expression ends (ADR-107 D3)"
                    .to_string(),
                "a **parameter** is where the `&` is the compiler's to write (ADR-094 \
                 D1), which is why `total(xs)` for a `Vec[i64]` needs no word and this \
                 line does"
                    .to_string(),
            ],
            help: Some(way_out),
        });
    }

    /// **`NK1178`: a grammar this compiler could not run while it built**
    /// ([`open-work.md`](../../docs/open-work.md) §2.9).
    ///
    /// One code and six sentences, because what a reader can do about it
    /// differs completely: input the parser refused is theirs to fix, a parser
    /// that did not compile is this compiler's, and a result with no crossed
    /// form is a decision [ADR-079](../../docs/specification/adr/adr-079.md)
    /// has not taken yet.
    fn a_grammar_that_did_not_run(
        &mut self,
        grammar: &str,
        rule: &str,
        why: &crate::grammar_run::Wall,
        span: &Span,
    ) {
        use crate::grammar_run::Wall;
        let entry = format!("{grammar}::{rule}");
        let (message, note, way_out) = match why {
            // **The input's own diagnostic, relayed whole.** It is in the
            // grammar's vocabulary and counts its line and column against the
            // bytes that were parsed, which is what Part II 10.2 A means by
            // *invalid input fails the build*.
            Wall::Refused { detail } => (
                format!("`{entry}` refused the bytes this build gave it"),
                format!(
                    "the parser's own words, against the input:\n{}",
                    indented(detail)
                ),
                "fix the input, or widen the grammar to accept it".to_string(),
            ),
            Wall::NoCrossedForm { ty, because } => (
                format!("`{entry}` hands back a `{ty}`, which has no build-time form"),
                format!(
                    "a grammar runs here and its result has to **cross** into the program \
                     (ADR-079 D1): {because}"
                ),
                "write a rule whose result is a whole number, a `bool`, text, a list of \
                 those or a `struct` whose fields are those"
                    .to_string(),
            ),
            Wall::InsideAnother { grammar: outer } => (
                format!("`{entry}` would run while `{outer}` is running"),
                "running a grammar compiles a parser, so one inside another would have \
                 this compiler start a second compiler inside the first - which is a \
                 build that does not end rather than one that is slow"
                    .to_string(),
                "run the inner grammar in a `comptime` of its own, and read its value here"
                    .to_string(),
            ),
            Wall::NowhereToBuild => (
                format!("`{entry}` has nowhere to compile its parser"),
                "a grammar is run by compiling the parser it generates, which needs a \
                 directory to build in - and this build has none"
                    .to_string(),
                "run this through `nikaia build` or `nikaia --input`, which both have one"
                    .to_string(),
            ),
            Wall::DidNotBuild { detail } => (
                format!("the parser `{entry}` generates did not compile"),
                format!(
                    "that is this compiler's fault and not this program's: the parser built \
                     here is the parser the program links, so a program that runs cannot \
                     have a parser that does not.\n{}",
                    indented(detail)
                ),
                "please report it - a `.nika` file and this message are the whole of what \
                 is needed"
                    .to_string(),
            ),
            Wall::Unreadable { detail } => (
                format!("`{entry}` ran and printed something this compiler could not read"),
                format!(
                    "the encoder is generated and the decoder is written by hand, which is \
                     two halves of one format: {detail}"
                ),
                "please report it - this is a defect in the compiler rather than in the \
                 program"
                    .to_string(),
            ),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1178",
            message,
            notes: vec![note],
            help: Some(way_out),
        });
    }

    /// **`NK1177`: `asset("…")` written where it cannot stand**
    /// ([ADR-116](../../docs/specification/adr/adr-116.md) D2).
    ///
    /// It is the compiler's name and not `std`'s: it is recognised inside a
    /// `comptime` initialiser, where the read happens while the program is
    /// built. Written anywhere else there is nothing to recognise it, and the
    /// message says what a file read **while the program runs** is called —
    /// because that is what a reader who wrote it here almost certainly meant.
    fn an_asset_outside_a_comptime(&mut self, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1177",
            message: "`asset` reads a file while the program is **built**, and this is not a \
                      `comptime`"
                .to_string(),
            notes: vec![
                "three words each decide one thing (ADR-116 D2): `comptime` says **when**, \
                 `asset(\"…\")` says **where the bytes come from**, and the call around it \
                 says what is done with them. Without the first there is no build-time \
                 evaluation for the other two to happen in"
                    .to_string(),
            ],
            help: Some(
                "write `comptime NAME = …` if the bytes belong in the program, or \
                 `fs::read(…, root)` to read the file while the program runs"
                    .to_string(),
            ),
        });
    }

    /// **`NK1176`: a path that is not a literal**
    /// ([ADR-072](../../docs/specification/adr/adr-072.md) D4).
    ///
    /// The cost is real and the record accepts it: a build that wants
    /// `config/linux.toml` and `config/wasm.toml` writes both, in the code and
    /// in the list. What it buys is that *named in the code* stays decidable by
    /// looking at the line — which is the whole of D3, and which a path assembled
    /// from a constant would quietly take away.
    fn a_path_that_is_not_a_literal(&mut self, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1176",
            message: "`asset` takes a written path, and this one is worked out".to_string(),
            notes: vec![
                "the allowlist is checked against the **literal** (ADR-072 D4), so a path \
                 the build computes - even one that folds to text this evaluator can read - \
                 would make *named in the code* something a reader cannot decide by looking \
                 at the line"
                    .to_string(),
            ],
            help: Some(
                "write the path out: two files wanted is two `asset(\"…\")` and two lines \
                 in the list"
                    .to_string(),
            ),
        });
    }

    /// **`NK1174`: a `with` that names no field**
    /// ([ADR-118](../../docs/specification/adr/adr-118.md) D1).
    ///
    /// *A copy that changes nothing is a line the reader would puzzle over* —
    /// which is the record's own reason, and it is about **meaning** rather
    /// than syntax, so it is read here. The parser could refuse `{ }` and did
    /// for an afternoon; its caret landed on the line *after* the braces,
    /// because by then it had consumed them and the whitespace behind them.
    fn a_with_that_changes_nothing(&mut self, ty: &str, span: &Span) {
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1174",
            message: format!("this `with` names no field, so it is the `{ty}` it copies"),
            notes: vec![
                "`with` is read as *this value, with these fields different* (ADR-118 D1) \
                 - and one that names none says nothing a reader can act on, which is a \
                 line they would stop at looking for what it does"
                    .to_string(),
            ],
            help: Some("name the fields that change, or drop the `with`".to_string()),
        });
    }

    /// **`NK1173`: a `with` over a value it cannot copy**
    /// ([ADR-118](../../docs/specification/adr/adr-118.md) D1, D3, §4).
    ///
    /// One claim — *this is not a value `with` copies* — and four reasons, each
    /// with a way out that can be taken
    /// ([Part III C.2](../../docs/specification/30-nikaia-tooling.md)). The
    /// enum's is the one the record names by hand: `m with { x: 1 }` cannot be
    /// typed without knowing the variant, and `match` is where a variant is
    /// known.
    fn a_with_over_something_else(&mut self, ty: &str, why: Copyable, span: &Span) {
        let (note, way_out) = match why {
            Copyable::AnEnum => (
                format!(
                    "`{ty}` is an `enum`, and which fields a copy would carry depends on the \
                     variant - which the type does not say (ADR-118 §4)"
                ),
                "match on it first, and build the variant's value in the arm where it is \
                 known"
                    .to_string(),
            ),
            Copyable::AView => (
                format!(
                    "`with` takes the fields it does not name from the value **by move** \
                     (ADR-118 D3), and `&{ty}` is a view - there is nothing here to move \
                     out of, and a copy this compiler inserted would be one the program \
                     did not write (ADR-107 D3)"
                ),
                format!(
                    "take the value rather than a view of it, or write a `{ty} {{ … }}` \
                     naming every field"
                ),
            ),
            Copyable::NotAStruct => (
                format!(
                    "`with` copies a **struct**, field by field, and `{ty}` is not one this \
                     program declares with fields (Part I, 4.1)"
                ),
                "write the value the type's own constructor takes".to_string(),
            ),
            Copyable::Unnamed => (
                "`with` lowers to a copy that **writes the type's name** - `Point { x: 1, \
                 ..p }` - so a value whose type this compiler has not worked out has \
                 nothing to write"
                    .to_string(),
                "write the type on the binding this reads, or name every field in a \
                 literal"
                    .to_string(),
            ),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1173",
            message: match why {
                // A type this compiler could not work out prints as `?`, which
                // is a headline about nothing. What the reader needs to know is
                // that it is the *type* that is missing, not the value.
                Copyable::Unnamed => {
                    "`with` copies a struct, and this compiler could not work out what type \
                     this is"
                        .to_string()
                }
                _ => format!("`with` copies a struct, and this is `{ty}`"),
            },
            notes: vec![note],
            help: Some(way_out),
        });
    }

    fn no_such_field(&mut self, ty: &str, field: &str, declared: &[FieldContract], span: &Span) {
        let names: Vec<&str> = declared.iter().map(|f| f.name.as_str()).collect();
        let near = nearest(field, &names);
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1107",
            message: format!("`{ty}` has no field `{field}`"),
            notes: vec![format!("`{ty}` has {}", list(&names))],
            help: Some(match near {
                Some(near) => format!("did you mean `{near}`?"),
                None => format!("add `{field}` to `{ty}`, or use one of the fields it has"),
            }),
        });
    }

    // --- looking things up ---------------------------------------------------

    /// Bind a name, with the constant it stands for where there is one
    /// (ADR-043 D5). Only [`Stmt::Let`] ever passes anything but `None`.
    fn bind_with(&mut self, name: String, ty: Ty, constant: Option<i128>) {
        self.bind_local(Local {
            literal: None,
            name,
            ty,
            constant,
            lent: false,
            changing: false,
            built: None,
            immutable: None,
            empty_list: None,
        });
    }

    fn bind_local(&mut self, local: Local) {
        if let Some(frame) = self.scope.last_mut() {
            frame.push(local);
        }
    }

    fn lookup(&self, name: &str) -> Option<Ty> {
        self.local(name).map(|(ty, _)| ty)
    }

    /// The innermost binding of a name, whole.
    fn binding(&self, name: &str) -> Option<&Local> {
        self.scope
            .iter()
            .rev()
            .find_map(|frame| frame.iter().rev().find(|local| local.name == name))
    }

    /// The innermost binding of a name: its type, and the constant it stands
    /// for where the checker could evaluate one.
    fn local(&self, name: &str) -> Option<(Ty, Option<i128>)> {
        self.scope
            .iter()
            .rev()
            .find_map(|frame| frame.iter().rev().find(|local| local.name == name))
            .map(|local| (local.ty.clone(), local.constant))
    }

    /// A function by the name a call wrote: this unit's, then a constructor,
    /// then a library's - the same order the `sync` check resolves in.
    fn resolve(&self, name: &str) -> Option<(String, &'a FnContract)> {
        if let Some(contract) = self.own.functions.get(name) {
            return Some((name.to_string(), contract));
        }
        let constructed = format!("{name}::new");
        if let Some(contract) = self.own.functions.get(&constructed) {
            return Some((constructed, contract));
        }
        if let Some(found) = self.library.lookup(name) {
            return Some(found);
        }
        // **`std`'s own types are constructed the same way**
        // ([ADR-140](../../docs/specification/adr/adr-140.md) D2): `Vec()` is
        // the anonymous constructor, exactly as `Stats(first)` is for a type a
        // `.nika` file declares. The ledger's key stays `Vec::new`, because
        // that is the name the **lowering** writes and the lowering is name for
        // name ([ADR-011](../../docs/specification/adr/adr-011.md) D2) - what
        // changes is that the fallback above reaches the library too, where it
        // used to stop at this unit.
        self.library.lookup(&constructed)
    }

    /// Walk a call's arguments, telling a lambda what it will be handed.
    ///
    /// The types come from the callee's signature, so this can only run once
    /// the callee is known - which is why the resolution moved ahead of the
    /// walk (ADR-029). An argument whose parameter says nothing is walked
    /// exactly as it was before.
    ///
    /// `declared_here` says whether the signature came from **this program's**
    /// ledger rather than from `std`'s. It decides one thing and
    /// [ADR-122](../../docs/specification/adr/adr-122.md) D3 is why: a `std`
    /// entry's lambda type is a hand-written description of a **Rust**
    /// signature, which takes a plain closure whatever its `sync` column says,
    /// so the future shape must not be written for one.
    fn arguments_given(
        &mut self,
        args: &[Expr],
        expected: &[Ty],
        declared_here: bool,
        // The callee's contract, where one was resolved. Read for one question:
        // whether it **keeps** the parameter a lambda is landing in
        // ([ADR-192](../../docs/specification/adr/adr-192.md) D1).
        callee: Option<&FnContract>,
        span: &Span,
    ) -> Vec<Ty> {
        args.iter()
            .enumerate()
            .map(|(at, arg)| match (arg, expected.get(at)) {
                (
                    Expr::Closure {
                        params,
                        mutable,
                        body,
                    },
                    Some(Ty::Fn {
                        params: given,
                        result,
                        is_sync,
                        throws,
                    }),
                ) => {
                    // **A lambda handed to a parameter whose type may pause is
                    // written as a closure returning a boxed future**
                    // ([ADR-122](../../docs/specification/adr/adr-122.md) D1).
                    // Recorded here because it is a question about the
                    // parameter's **type**, which the emitter has no way to ask
                    // ([ADR-028](../../docs/specification/adr/adr-028.md)) — the
                    // same arrangement `lent_args` and `nullable_args` have, and
                    // keyed the same way.
                    if !is_sync && declared_here {
                        self.checked.future_lambdas.insert((span.start, at));
                        // **And an `async` closure where the callee *runs* it**
                        // ([ADR-192](../../docs/specification/adr/adr-192.md)
                        // D1). The box is what a **kept** parameter needs, and
                        // a run parameter never did: `impl AsyncFn(A) -> R` is
                        // 1.37 ns/call against the box's 11.99, on a 0.31
                        // floor.
                        //
                        // **Run is the absence of `keeps`**, and a callee this
                        // walk could not resolve is read as run - which agrees
                        // with what the *declaration* writer does with the same
                        // absent answer. The two reading one column the same
                        // way is the whole of what keeps them from writing two
                        // shapes for one parameter.
                        let runs = callee.is_none_or(|c| {
                            c.signature
                                .as_ref()
                                .and_then(|s| s.arguments().get(at).cloned())
                                .is_none_or(|(name, _)| !c.keeps.contains(&name))
                        });
                        if runs {
                            self.checked.run_lambdas.insert((span.start, at));
                        }
                    }
                    // **A parameter the callee keeps is a kept function
                    // value**, `sync` or not: the callee stores it.
                    if declared_here && !self.runs_the_parameter(callee, at) {
                        self.checked
                            .kept_lambdas
                            .insert((span.start, argument_shape(arg)), (!is_sync, *throws));
                    }
                    if let Some(want) = expected.get(at) {
                        self.a_lambdas_text_is_its_own(want, arg);
                    }
                    let (came_to, paused) = self.lambda(
                        params,
                        mutable,
                        body,
                        given,
                        Promises {
                            may_pause: !is_sync,
                            may_fail: *throws,
                        },
                        span,
                    );
                    // **A lambda that paused, handed to a `std` entry**
                    // ([ADR-233](../../docs/specification/adr/adr-233.md) D1):
                    // kept for the call to lower to the entry's pausing
                    // counterpart, which only the call knows.
                    if paused && !declared_here {
                        self.paused_args.push(argument_shape(arg));
                    }
                    // **The lambda as the type it was handed to, with what its
                    // body comes to where that type does not say** (ADR-212
                    // D5): `fn($T) -> $U` binds `$U` from it, which is how a
                    // `map` knows its elements. Where the type does say, what it
                    // says stands - this is an answer for a variable, not a
                    // second opinion on a written result.
                    let result = match result.as_deref() {
                        Some(written) if !written.is_unknown() => Some(Box::new(written.clone())),
                        _ => (!came_to.is_unknown()).then(|| Box::new(came_to)),
                    };
                    Ty::Fn {
                        params: given.clone(),
                        result,
                        is_sync: *is_sync,
                        throws: *throws,
                    }
                }
                _ => {
                    self.a_field_of_a_borrowed_subject(arg, span, "passed");
                    let found = self.expr(arg, span);
                    // **A kept value handed to a parameter that only runs
                    // it** is lent as the closure it holds.
                    if declared_here
                        && matches!(expected.get(at), Some(Ty::Fn { .. }))
                        && matches!(found, Ty::Fn { .. })
                        && self.runs_the_parameter(callee, at)
                        && !self.a_run_parameter(arg)
                    {
                        self.checked
                            .kept_args
                            .insert((span.start, argument_shape(arg)));
                    }
                    found
                }
            })
            .collect()
    }

    /// Whether parameter `at` is a by-value type variable that nothing else in
    /// the signature binds: no receiver, and no other parameter names it.
    fn decided_by_this_argument(
        &self,
        signature: &crate::contracts::Signature,
        wanted: &[(String, Ty)],
        at: usize,
    ) -> bool {
        let Some((_, Ty::Var { name, view: false })) = wanted.get(at) else {
            return false;
        };
        !signature.takes_a_receiver()
            && wanted
                .iter()
                .enumerate()
                .all(|(other, (_, ty))| other == at || !ty.text().contains(&format!("${name}")))
    }

    /// **A kept function takes its arguments by value**, so a text literal
    /// handed to one where it keeps text is built there (ADR-207 D2).
    fn kept_arguments_are_its_own(&mut self, args: &[Expr], params: &[Ty]) {
        for (arg, want) in args.iter().zip(params) {
            self.text_literal(want, arg, true);
        }
    }

    /// Whether `expr` names a parameter of the function being walked that is
    /// code it only runs: a closure argument below, not a kept value.
    fn a_run_parameter(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Variable(name) if self.run_code.contains(self.parsed.text(*name)))
    }

    /// Whether the callee only **runs** its `at`th parameter: the absence of
    /// `keeps`, and run where no contract was resolved.
    fn runs_the_parameter(&self, callee: Option<&FnContract>, at: usize) -> bool {
        callee.is_none_or(|c| {
            c.signature
                .as_ref()
                .and_then(|s| s.arguments().get(at).cloned())
                .is_none_or(|(name, _)| !c.keeps.contains(&name))
        })
    }

    /// A lambda whose parameters have types, because the callee said so.
    ///
    /// The implicit form is the one that matters and the one that had no way
    /// to work: `fn { a.add(t) }` records **no parameters at all** in the AST -
    /// Part I 5.3 settles how many it takes from which of `a`, `b`, `c` the
    /// body mentions, and that is decided when it is emitted. So the names are
    /// bound here, in order, to whatever the signature says the lambda is
    /// handed. A body that mentions fewer of them simply leaves the later
    /// bindings unused.
    fn lambda(
        &mut self,
        params: &[winnow_grammar::Symbol],
        mutable: &[winnow_grammar::Symbol],
        body: &Block,
        given: &[Ty],
        promised: Promises,
        span: &Span,
    ) -> (Ty, bool) {
        let frame = params
            .iter()
            .enumerate()
            .map(|(at, p)| {
                self.lambda_parameter(
                    *p,
                    mutable,
                    given.get(at).cloned().unwrap_or(Ty::Unknown),
                    span,
                )
            })
            .collect();

        self.repeats.push(Repeats::a_lambda(self.scope.len()));
        self.scope.push(frame);
        // The other door into a lambda's body, and it needs the same boundary
        // as the one in `expr`: which of the two a lambda arrives through is
        // whether the callee's signature typed its parameters, and that has
        // nothing to do with what a `break` in it may reach.
        let (tail, seen) = self.past_a_boundary("lambda", |me| {
            me.handed_over = Some(Handed {
                pauses: false,
                fails: false,
                promised,
            });
            let tail = me.block(body);
            (tail, me.handed_over.take())
        });
        self.scope.pop();
        self.repeats.pop();
        let paused = seen.as_ref().is_some_and(|seen| seen.pauses);
        if let Some(seen) = seen {
            self.a_handler_that_does_more_than_the_type_allows(promised, seen, span);
        }
        // **What its body comes to** ([ADR-212](../../docs/specification/adr/adr-212.md)
        // D5), which is not a claim about the lambda's type but a fact about a
        // block this checker has just walked - the same one `spawn` reads for
        // a task. ADR-029 D1 was about *writing* a lambda's result down, which
        // nothing does; a `map` still has to know what its elements are.
        //
        // **And whether it paused** ([ADR-233](../../docs/specification/adr/adr-233.md)
        // D1): a `std` entry that takes the lambda is lowered to its pausing
        // counterpart where it did.
        (tail, paused)
    }

    /// Walk something the language below makes **a function of its own**.
    ///
    /// A lambda, a task, an `overlap` branch: three constructs that are one
    /// block above and a closure or an `async` block below. A loop outside one
    /// of them is not reachable from inside it, so the count starts again at
    /// zero and the word for what stands in the way is carried for the message.
    fn past_a_boundary<T>(&mut self, what: &'static str, walk: impl FnOnce(&mut Self) -> T) -> T {
        let loops = std::mem::replace(&mut self.loops, 0);
        let barrier = self.barrier.replace(what);
        // **And what a lambda was seen to do stops here too**
        // ([ADR-102](../../docs/specification/adr/adr-102.md) D2), for the same
        // reason the loop count does: what a body written *inside* this one
        // does happens when its own callee runs it. `lambda` sets its own
        // accumulator inside the walk and reads it back there.
        let handed = self.handed_over.take();
        // **And the enclosing `sync` stops here too.** What is inside this body
        // is a body of its own — a lambda, an `overlap` branch, a `select` arm —
        // and what *it* may do is said by its own type rather than by the
        // declaration around it ([ADR-102](../../docs/specification/adr/adr-102.md)
        // D2, whose `NK2206` is the rule for a lambda).
        let promised = self.inside_a_sync_function.take();
        let value = walk(self);
        self.inside_a_sync_function = promised;
        self.handed_over = handed;
        self.loops = loops;
        self.barrier = barrier;
        value
    }

    /// `NK1134`: a `catch` over an expression that cannot fail
    /// ([ADR-091](../../../docs/specification/adr/adr-091.md)).
    ///
    /// The lowering makes a `catch` a `match` over a `Result`, so a guarded
    /// expression that is not one produces `E0308` about the generated file -
    /// [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class,
    /// naming a `match` and an `Ok` arm nobody wrote.
    ///
    /// **Refused rather than dropped**, and that is the decision rather than
    /// the smaller change. Emitting the expression without the `match` would
    /// take the program the author wrote and quietly delete a block from it -
    /// including a `return` inside the handler, which
    /// [ADR-034](../../../docs/specification/adr/adr-034.md) makes the
    /// *function's* return. A handler that never runs is a belief about the
    /// program, and a belief that is wrong is worth a sentence.
    ///
    /// **Only where the ledger says so.** [`Self::may_fail_here`] refuses a
    /// call that can fail outside a `catch`, reported "only where the callee's
    /// contract **says** it can fail"; this is that standard read from the
    /// other end, and [`Guarded`] carries the two answers apart so that *could
    /// not look it up* never becomes *cannot fail*.
    fn nothing_here_can_fail(&mut self, guarded: Guarded, span: &Span) {
        if guarded.fallible || guarded.unanswered {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1134",
            message: "nothing in this expression can fail, so the `catch` has nothing to handle"
                .to_string(),
            notes: vec![
                "a failure comes from a call whose contract carries a `throws` (Part I, 7.1)"
                    .to_string(),
                "every call here is one this compiler could look up, and none of them \
                 declares one"
                    .to_string(),
                "a call no contract describes would leave this unsaid rather than \
                 refused (Part III, C.4)"
                    .to_string(),
            ],
            help: Some(
                "delete the `catch` and its handler: the expression is the value".to_string(),
            ),
        });
    }

    /// `NK2205`: a `set` whose argument reads the same container with `get`
    /// ([ADR-039](../../../docs/specification/adr/adr-039.md) D10,
    /// [ADR-099](../../../docs/specification/adr/adr-099.md)).
    ///
    /// `kasse.set(kasse.get() + 100)` takes the lock **twice** — once to read
    /// and once to store — and between the two the value can change, so what is
    /// stored is computed from a state that may no longer hold. Making a new
    /// value out of the old one is what the third door is for, and it takes the
    /// lock once.
    ///
    /// **Syntactic, on purpose** (D10's own line): it catches the `get`
    /// written *inside* the `set`, which is the shape people write, and not the
    /// same pair spread over two lines. The second is a question about what
    /// happened between two statements, and nothing here answers that — so
    /// widening this rule would mean guessing, and the narrow one is right
    /// about what it does see.
    /// **The stamp passes through a call the callee cannot put it back with**
    /// ([ADR-111](../../docs/specification/adr/adr-111.md) D2).
    ///
    /// A call whose argument is stamped is allowed where the callee's `touches`
    /// column names no lock — it cannot write the value into one — and **its
    /// result is stamped**. That is what makes `kasse.set(bumped(stand))` the
    /// same answer as `kasse.set(stand + 1)` without an analysis following the
    /// value through `bumped`.
    ///
    /// **A callee that touches a lock, or that nothing describes, does not
    /// pass one on.** An absent `touches` reads as *touches everything*
    /// ([ADR-033](../../docs/specification/adr/adr-033.md)), so a call nobody
    /// wrote down hands back a plain value rather than a stamped one: the
    /// wrong answer that way costs a refusal that is not raised, and the wrong
    /// answer the other way refuses a program (C.4).
    fn stamped_through(&self, contract: &FnContract, given: &[Ty], result: Ty) -> Ty {
        if result.is_seen() || !given.iter().any(|ty| ty.is_seen()) {
            return result;
        }
        let reaches_a_lock =
            !contract.touches_known || contract.touches.iter().any(|t| t.names_a_lock());
        match reaches_a_lock {
            true => result,
            false => Ty::seen(result.unseen()),
        }
    }

    /// **`NK2207`: an `update` block that assigns to its `mut v` without
    /// reading it** ([ADR-111](../../docs/specification/adr/adr-111.md) D4).
    ///
    /// `update fn(mut v) { v = n }` is a `set` through the back door and is
    /// refused as one: nothing about it decides *inside* the lock, which is
    /// what the door is for. A block that **reads** `v` — `v += n`,
    /// `v.push(e)`, `if v > 100 { v = 0 }` — is the door working.
    ///
    /// **Read means mentioned anywhere but as the whole target**, which is the
    /// safe direction for a refusal: `v += n` mentions it, `v.f = n` mentions
    /// it, and only `v = …` does not. Asking a narrower question would refuse a
    /// block that reads `v` in a way this walk did not recognise, and that is
    /// [Part III C.4](../../docs/specification/30-nikaia-tooling.md)'s correct
    /// program refused.
    fn an_update_block_reads_what_it_writes(
        &mut self,
        params: &[winnow_grammar::Symbol],
        body: &Block,
        span: &Span,
    ) {
        for name in params {
            let name = self.parsed.text(*name).to_string();
            let mut assigned = false;
            let mut read = false;
            for stmt in &body.stmts {
                // A whole-name assignment is the shape the rule is about; a
                // mention anywhere else is a read.
                if let Stmt::Assign { target, op, value } = &stmt.node
                    && op.is_none()
                    && matches!(target, Expr::Variable(n) if self.parsed.text(*n) == name)
                {
                    assigned = true;
                    // **And the value being stored is still a read**:
                    // `v = v + 1` mentions `v`, and it decides inside the
                    // lock exactly as `v += 1` does.
                    let mentioned = crate::contracts::send::names_used(self.parsed, value);
                    read |= mentioned.contains(&name);
                    continue;
                }
                let mentioned = crate::contracts::send::names_used_in_stmt(self.parsed, &stmt.node);
                read |= mentioned.contains(&name);
            }
            if !assigned || read {
                continue;
            }
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2207",
                message: format!(
                    "this `update` block replaces `{name}` without reading it, which is a `set`"
                ),
                notes: vec![
                    "the door exists to decide **inside** the lock; a block that only stores \
                     takes the lock for nothing the `set` door does not already do \
                     (ADR-111 D4)"
                        .to_string(),
                ],
                help: Some(
                    "use `set` if the value is computed outside - or read the old value \
                     here, which is what the block is for: `v += n`, `v.push(e)`, \
                     `if v > 100 { v = 0 }`"
                        .to_string(),
                ),
            });
        }
    }

    /// **The witness takes no `&`, and a written one is `NK1137`**
    /// ([ADR-094](../../docs/specification/adr/adr-094.md) D1, at
    /// [ADR-111](../../docs/specification/adr/adr-111.md) D5's door).
    ///
    /// The ledger declares `seen: &$T` — the witness is read and never stored,
    /// so the caller keeps it — and the `&` that says so is the compiler's, as
    /// it is everywhere else. It is refused here rather than through `lends`
    /// because `lends` withholds its claim on every *method* argument
    /// ([ADR-028](../../docs/specification/adr/adr-028.md): the emitter cannot
    /// resolve a receiver), and this one position the emitter is told about
    /// outright. Without the refusal a written `&` would come out `&&`, which
    /// is `rustc`'s words about a file nobody wrote (Part III C.1).
    fn the_witness_takes_no_reference(&mut self, seen: &Expr, span: &Span) {
        if !matches!(
            seen,
            Expr::Unary {
                op: ast::UnaryOp::Ref,
                ..
            }
        ) {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1137",
            message: "the `&` here is the compiler's to write".to_string(),
            notes: vec![
                "`after:` is a witness the door only reads, so the reference is what the \
                 call already means (ADR-094 D1, ADR-111 D5)"
                    .to_string(),
            ],
            help: Some("take the `&` off".to_string()),
        });
    }

    /// **`NK1143`: a call to an `extern` name outside an `unsafe` block**
    /// ([ADR-124](../../docs/specification/adr/adr-124.md) D3).
    ///
    /// That is the whole of what the word buys, and it is why it is a word
    /// rather than an attribute on the declaration: the boundary is visible
    /// **at the call**, in the body somebody reads, rather than in a file
    /// beside it.
    ///
    /// **This compiler's refusal and not the language below's.** Rust makes the
    /// functions of an `extern` block `unsafe fn`, so the program would be
    /// refused either way — with a message about a generated file, which is
    /// [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class.
    ///
    /// **Only a name this file declared.** A Nikaia function and a C
    /// declaration are both ledger entries, and only one of them is this; the
    /// set comes from the item tree, so a name nothing here declared is not
    /// this refusal's business.
    fn a_foreign_call_outside_unsafe(&mut self, name: &str, span: &Span) {
        if self.inside_unsafe || !self.foreign_names.contains(name) {
            return;
        }
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1143",
            message: format!(
                "`{name}` is declared `extern` and this call is not in an `unsafe` block"
            ),
            notes: vec![
                "C is not memory-safe, so the boundary is written where it is crossed \
                 rather than once beside the declaration (Part III, 15.1; ADR-124 D3)"
                    .to_string(),
                "the block makes no other rule: what is inside it is checked exactly as \
                 anything else is"
                    .to_string(),
            ],
            help: Some(format!("write `unsafe {{ {name}(…) }}`")),
        });
    }

    /// **`NK2208`: the lowering of a door, written as a door**
    /// ([ADR-111](../../docs/specification/adr/adr-111.md) D5).
    ///
    /// `kasse.set(neu; after: stand)` lowers to `set_after(neu, stand)`, and
    /// what is below is reachable by name from above: `kasse.set_after(a, b)`
    /// is a Rust method that exists, takes two arguments, and hands back a
    /// `Result`. Nothing in the ledger describes it, so nothing would refuse it
    /// and nothing would write the `?` — the failure would be **dropped**, and
    /// `rustc` would then warn about an unused `Result` in a file nobody wrote
    /// (Part III C.1).
    ///
    /// **So it is refused by name**, which is the cheap half of D5's *one
    /// door*: a program has one way to ask for a compare-and-store, the message
    /// says what it is, and the stamp discipline has no second entrance. The
    /// receiver has to be a lock — `set_after` on a type of the program's own
    /// is that program's own method and nothing to do with this.
    fn a_door_that_is_not_written(&mut self, on: &Ty, written: &str, span: &Span) {
        if written != "set_after" {
            return;
        }
        let Ty::Named { name, .. } = on else {
            return;
        };
        if !is_hull(name) {
            return;
        }
        let container = match &self.set_receiver {
            Some(name) => name.clone(),
            None => "this lock".to_string(),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2208",
            message: format!(
                "`{container}` has no `set_after`; it is how `set(…; after: …)` is written below"
            ),
            notes: vec![
                "the compare and the store happen while the lock is open once, and the door \
                 that asks for that is `set` with a witness (ADR-111 D5)"
                    .to_string(),
                "written this way the failure would be dropped rather than propagated, \
                 because nothing in the contracts describes this name"
                    .to_string(),
            ],
            help: Some(format!(
                "write `{container}.set(neu; after: seen)`, where `seen` is what the lock \
                 handed out"
            )),
        });
    }

    /// **`NK2205`: a `set` given a stamped value, or standing under a stamped
    /// condition** ([ADR-111](../../docs/specification/adr/adr-111.md) D4).
    ///
    /// `kasse.set(stand + 100)` is a read-modify-write through two doors: the
    /// lock is taken once to read and once to store, and anything may happen
    /// between. It is refused **whether `stand` was read on the line above, in
    /// another function, or in another request** — which is what the stamp
    /// buys, since the value carries where it came from and no analysis has to
    /// follow it.
    ///
    /// **The second shape is the condition.** `if stand > 100 { kasse.set(0) }`
    /// stores a plain value, and the *decision* is the stale thing. A `while`,
    /// a `match` and a nested `if` are conditions alike.
    ///
    /// **There is no way around either** (D4): no `.value`, no `overwrite`, no
    /// word that takes a stamp off. `set` is for a starting value, a
    /// configuration that arrived from outside, a reset an operator asked for —
    /// and `set(neu; after: seen)` is the one door for a stamped one, which is
    /// D5 and is not built.
    ///
    /// **It used to ask a narrower question**: whether the argument contained a
    /// `get` **on the same container**, which caught the one line and nothing
    /// else. The stamp is what widened it, and widened it without an analysis.
    fn a_set_that_reads_what_it_writes(
        &mut self,
        on: &Ty,
        written: &str,
        found: &[Ty],
        span: &Span,
    ) {
        // **`set(after)` is not `set`**, and that is D5's whole point: the
        // witness covers both shapes, so neither is refused there. It is the
        // ledger's name that decides rather than a flag, because the name is
        // what already says which operation this is.
        if written != "set" {
            return;
        }
        let Ty::Named { name, .. } = on else {
            return;
        };
        if !is_hull(name) {
            return;
        }
        let container = self
            .set_receiver
            .clone()
            .unwrap_or_else(|| "this lock".to_string());

        // **The value**, wherever it was read.
        if found.iter().any(|ty| ty.is_seen()) {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2205",
                message: format!(
                    "this `set` stores a value that was read from a lock, so `{container}` is \
                     taken twice"
                ),
                notes: vec![
                    "`set` is for a value computed outside the lock; a value a lock handed \
                     out is stale the moment the lock is let go, and anything may happen \
                     between the read and the store (ADR-111 D4)"
                        .to_string(),
                    "the stamp travels with the value, so this is the same answer whether it \
                     was read on the line above, in another function, or in another request"
                        .to_string(),
                ],
                help: Some(format!(
                    "write `{container}.update fn(mut v) {{ … }}`, which decides inside the \
                     lock (ADR-110 D1)"
                )),
            });
            return;
        }

        // **And the decision.** The value stored is plain; what is stale is the
        // condition this stands under.
        if let Some(at) = self.stamped_condition {
            let _ = at;
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK2205",
                message: format!(
                    "this `set` stands under a condition read from a lock, so `{container}` is \
                     taken twice"
                ),
                notes: vec![
                    "the value stored is plain and the **decision** is not: what the lock \
                     said may have changed before this line runs (ADR-111 D4)"
                        .to_string(),
                ],
                help: Some(format!(
                    "write `{container}.update fn(mut v) {{ … }}` and take the decision inside \
                     the lock (ADR-110 D1)"
                )),
            });
        }
    }

    /// `NK2204`: an assignment straight into a `SharedMut`
    /// ([ADR-039](../../../docs/specification/adr/adr-039.md) D10,
    /// [ADR-099](../../../docs/specification/adr/adr-099.md)).
    ///
    /// `kasse = 42` looks like an ordinary assignment and is not: the lock has
    /// to be taken for the write, and what takes it is a **door**. Without this
    /// the program meets `rustc` about a type it never wrote — the hull, not
    /// the value inside it — which is
    /// [Part III C.1](../../../docs/specification/30-nikaia-tooling.md).
    ///
    /// **The message names the door with the value in it**, because the repair
    /// is mechanical and a help line that says *"use a door"* leaves the reader
    /// to work out which of the four.
    fn a_write_that_skips_the_door(&mut self, target: &Expr, into: &Ty, value: &Expr, span: &Span) {
        let Ty::Named { name, .. } = into else {
            return;
        };
        if name != SHARED_MUT {
            return;
        }
        // The name, where the target is one. A field or an index into a hull is
        // a shape nothing decides yet, and saying nothing is the direction that
        // cannot refuse a program that is right.
        let Expr::Variable(bound) = target else {
            return;
        };
        let bound = self.parsed.text(*bound).to_string();
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK2204",
            message: format!(
                "`{bound}` holds shared mutable state, and this assigns to it directly"
            ),
            notes: vec![
                "a write goes through a door, because the lock has to be taken for it \
                 (Part II, 12.2)"
                    .to_string(),
            ],
            help: Some(format!("write `{bound}.set({})`", self.written(value))),
        });
    }

    /// The value as the author wrote it, where this compiler can say so.
    ///
    /// **An expression has no span** — [ADR-081](../../../docs/specification/adr/adr-081.md)
    /// D2 gave one to `Binary` and to nothing else — so there is no source text
    /// to quote back. What can be rebuilt is rebuilt, and the rest is an
    /// ellipsis rather than a guess: a help line that quoted the wrong thing
    /// would be worse than one that quotes nothing, and the shape the
    /// specification's own example writes (`kasse = 42`) is a literal.
    fn written(&self, value: &Expr) -> String {
        match value {
            Expr::LitInt(n) => n.to_string(),
            Expr::LitBool(yes) => yes.to_string(),
            Expr::LitStr { text, .. } => format!("{text:?}"),
            Expr::Variable(name) => self.parsed.text(*name).to_string(),
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
            } => format!("-{}", self.written(expr)),
            _ => "…".to_string(),
        }
    }

    /// **A `let` that binds several names at once**
    /// ([ADR-098](../../../docs/specification/adr/adr-098.md)).
    ///
    /// Part I 8.1.2 writes `let (user, rights, prefs) = overlap { … }` and Part
    /// II 12.5 writes `let (tx, rx) = channel::bounded(100)`. Both take a value
    /// apart by **position**, and neither writes a type - so this binds each
    /// name to the part at its index where the value's type is a tuple of the
    /// right width, and to `?` where it is not.
    ///
    /// **The widths are not compared**, and that is deliberate rather than
    /// unfinished. A value whose type this compiler cannot see is `?`, and
    /// refusing a destructure of one because it *looks* like the wrong width
    /// would refuse a correct program
    /// ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)). What
    /// a wrong width costs today is the language below saying so about a `let`
    /// the author did write, which is a smaller and honest failure - and the
    /// day the tuple's own width is known for certain is the day to refuse it
    /// here.
    ///
    /// **A written type is refused**, because `let (a, b): T = …` would have to
    /// say which name `T` is about and nothing decides that. Refused rather than
    /// ignored: ignoring it takes something the author wrote and drops it.
    fn tuple_let(
        &mut self,
        names: &[Ident],
        ty: Option<&crate::ast::Type>,
        found: &Ty,
        span: &Span,
    ) -> Ty {
        if ty.is_some() {
            self.checked.findings.push(Finding {
                severity: Severity::Error,
                span: span.clone(),
                code: "NK1136",
                message: "a `let` that binds several names takes no type".to_string(),
                notes: vec![
                    "the names are taken apart by position, and one written type cannot say \
                     which of them it is about (Part I, 2.1)"
                        .to_string(),
                ],
                help: Some(
                    "take the type off, or bind one name and read the parts from it".to_string(),
                ),
            });
        }
        let parts = match found {
            Ty::Tuple(parts) if parts.len() == names.len() => parts.clone(),
            _ => vec![Ty::Unknown; names.len()],
        };
        for (name, part) in names.iter().zip(parts) {
            let bound = self.parsed.text(*name).to_string();
            self.nameable(&bound, span, "a `let`");
            self.bind_with(bound, part, None);
        }
        Ty::Tuple(Vec::new())
    }

    /// **Every item-level `comptime`, in a frame that stays under the whole
    /// file** ([ADR-097](../../../docs/specification/adr/adr-097.md)).
    ///
    /// A pass of its own, and both halves of that are load-bearing. It runs
    /// *before* the item loop, because a `fn` declared **above** a constant
    /// sees it as much as one below — an item is visible in its whole scope,
    /// which is what makes it an item. And the frame is pushed and never
    /// popped, because this checker's scope is a stack pushed per function and
    /// there is nothing under all of them otherwise.
    ///
    /// The bodies go through [`Self::comptime_binding`], the same function the
    /// statement form uses, so the fold, the refusal and the spelling the
    /// emitter is handed are one thing in one place
    /// ([ADR-073](../../docs/specification/adr/adr-073.md) D2: the difference
    /// between the two places is where the name is visible and nothing else).
    fn item_constants(&mut self) {
        self.scope.push(Vec::new());
        // **Every name first, because a constant is an item.** A function
        // declared below its caller has always been callable — items are
        // order-independent — and a constant was not, because this walk goes
        // down the file and binds as it goes. So `comptime A = B * 2` above
        // `comptime B = 21` was `NK1117`, *nothing declares `B`*: a correct
        // program refused ([Part III C.4](../../docs/specification/30-nikaia-tooling.md))
        // with a sentence that was not true, since the next line declares it.
        //
        // The type is the **declared** one where there is one and `?` where
        // there is not — which says nothing, and is what a name whose value
        // this walk has not reached yet honestly is. The binding below shadows
        // it with the answer.
        for item in &self.parsed.program.items {
            let Item::Comptime { name, ty, .. } = &item.node else {
                continue;
            };
            let declared = ty
                .as_ref()
                .map(|ty| self.declared(ty, &item.span))
                .unwrap_or(Ty::Unknown);
            self.bind_with(self.parsed.text(*name).to_string(), declared, None);
        }
        for item in &self.parsed.program.items {
            let Item::Comptime {
                name, ty, value, ..
            } = &item.node
            else {
                continue;
            };
            self.comptime_binding(*name, ty.as_ref(), value, &item.span);
        }
    }

    /// **One `comptime`, wherever it stands**
    /// ([ADR-097](../../../docs/specification/adr/adr-097.md)).
    ///
    /// [ADR-073](../../docs/specification/adr/adr-073.md) D2 decided both
    /// places and said the difference is *"where the name is visible, never
    /// what may stand to the right of the `='"*. So there is one function and
    /// the two callers differ only in which frame the name lands in - a second
    /// copy of the fold, the refusal and the emitter's spelling would be three
    /// chances for the two places to disagree about what a constant is.
    fn comptime_binding(
        &mut self,
        name: Ident,
        ty: Option<&crate::ast::Type>,
        value: &Expr,
        span: &Span,
    ) -> Ty {
        // **`asset("…")` is a name only here** (ADR-116 D2), so the ordinary
        // walk's refusal is off while the initialiser is read: the evaluator
        // answers it, and two walks over one expression must not both have an
        // opinion about the same call.
        let outside = std::mem::replace(&mut self.inside_a_comptime, true);
        let found = self.expr(value, span);
        self.inside_a_comptime = outside;
        let bound = self.parsed.text(name).to_string();
        self.nameable(&bound, span, "a `comptime`");
        let want = ty.as_ref().map(|ty| self.declared(ty, span));

        // What the emitter writes, spelled in the language below. An
        // integer takes the type its declaration pinned, and otherwise
        // the first one that holds it - Part I 2.4's rule, applied here
        // because Rust's `const` will not take the absence.
        //
        // **Before the type check and not after it**, which is
        // [ADR-079](../../docs/specification/adr/adr-079.md) D1's doing: whether
        // a growable value crosses as a fixed one is a question about the
        // **value**, because the length that makes an `Array[T, N]` a type is
        // the one the build computed.
        let folded = self.constant_of(value);
        // **The second stage of
        // [ADR-073](../../docs/specification/adr/adr-073.md) D5**: a call, and
        // with it everything a called body can reach. The fold above is the
        // first stage and stays in front of it, because it is what says which
        // integer type a *declaration* pinned — a question the interpreter does
        // not ask and does not need to.
        let (evaluated, said) = match &folded {
            Some(folded) => (Some(build_time::Value::Int(folded.value)), false),
            None => self.build_time_value(value, &bound, span),
        };
        // Counted rather than returned, so that every refusal below - the
        // crossing's, the ordinary mismatch's, and whatever `constant_fits`
        // makes of a literal - suppresses `NK1127` the same way. One mistake,
        // one error, and the rule does not have to be remembered at each site.
        let before = self.checked.findings.len();
        if let Some(want) = &want {
            self.constant_fits(value, Some(want), span);
            // **The annotation is a use, and a use answers the literal**
            // (ADR-152 D4) - the same line a `let` runs one construct over, and
            // it was missing here: `comptime PRIMES: Array[i64, 4] = [2, 3, 5, 7]`
            // is a `Vec[?]` against an `Array[i64, 4]` without it.
            let narrowed = self
                .array_literal(&found, want, value, span)
                .unwrap_or_else(|| found.clone());
            // **Growable going in, fixed coming out**
            // ([ADR-079](../../docs/specification/adr/adr-079.md) D1). A body
            // that builds its table with `push` has a `Vec`, and a `const`
            // below cannot hold one - but the value has been computed by the
            // line above, so its length is known and `Array[T, N]` is exactly
            // what it crosses as.
            match self.crosses_as_fixed(&narrowed, want, evaluated.as_ref(), span) {
                Crossing::Fits | Crossing::Said | Crossing::Unanswered => {}
                Crossing::Other => {
                    // …and D2's other half: a value that owns memory is refused
                    // for **what it is**, with the view-shaped equivalent as the
                    // way out. Only where the build computed one, because that
                    // is what lets the way out name the length.
                    match (&evaluated, is_growable(want)) {
                        // **A `struct` is only as writable as its fields**, and
                        // the one that is not is worth naming: a reader looking
                        // at `Bag { items: [1, 2, 3] }` cannot see which half
                        // of it a `const` cannot hold.
                        (Some(build_time::Value::Struct { .. }), _)
                            if self.unwritable_field(want).is_some() =>
                        {
                            let (field, held) = self.unwritable_field(want).expect("just asked");
                            self.a_field_that_owns_memory(&bound, &field, &held, span);
                        }
                        (
                            Some(
                                computed
                                @ (build_time::Value::List(_) | build_time::Value::Text(_)),
                            ),
                            true,
                        ) => {
                            let (want, computed) = (want.clone(), computed.clone());
                            self.a_constant_that_owns_memory(&bound, &want, &computed, span);
                        }
                        _ => {
                            self.expect(&narrowed, want, span.clone(), "const", |found, want| {
                                format!("this is `{found}`, and the `const` says `{want}`")
                            });
                        }
                    }
                }
            }
        } else {
            self.constant_fits(value, None, span);
        }
        let said_a_type = self.checked.findings.len() > before;

        let below = match (&want, &folded, value) {
            // **A type this program declares is its own name below**, and the
            // *value* is what says it is one: `rust_constant_type` knows the
            // types Part I 2.2 offers and cannot know a `Point`, where a
            // `Value::Struct` names the very type the declaration did.
            (Some(want), _, _) => rust_constant_type(want)
                .or_else(|| self.declared_below(want))
                .or_else(|| match &evaluated {
                    Some(build_time::Value::Struct { name, .. })
                        if matches!(want, Ty::Named { name: wanted, args, view: false }
                        if wanted == name && args.is_empty()) =>
                    {
                        Some(name.clone())
                    }
                    _ => None,
                }),
            (None, Some(folded), _) => Some(match &folded.pinned {
                Some(pinned) => pinned.clone(),
                None => match i32::try_from(folded.value) {
                    Ok(_) => "i32".to_string(),
                    Err(_) => "i64".to_string(),
                },
            }),
            (None, None, _) => match &evaluated {
                // A float with nothing declaring which one it is takes the
                // wider, as an integer does (Part I 2.4).
                Some(build_time::Value::Float(_)) => Some("f64".to_string()),
                // A `bool` from the interpreter is a `bool` below, whether it
                // was written `true` or came out of a call.
                Some(build_time::Value::Bool(_)) => Some("bool".to_string()),
                Some(build_time::Value::Int(value)) => Some(match i32::try_from(*value) {
                    Ok(_) => "i32".to_string(),
                    Err(_) => "i64".to_string(),
                }),
                // **An array with nothing declaring its type.** The element
                // type is the **checker's** where it has one - a body declared
                // `-> Vec[i64]` says `i64`, and reading it off the values
                // instead would make `[0, 1, 4]` an `[i32; 3]` and every later
                // `i64` arithmetic on it `rustc`'s complaint about a file
                // nobody wrote. Part I 2.4's widest-holder rule is the fallback
                // for the case nothing declared anything.
                Some(build_time::Value::List(items)) => element_below(&found)
                    .or_else(|| rust_array_type(items))
                    .map(|ty| format!("[{ty}; {}]", items.len())),
                // **Text crosses as a view, and `&str` is the one a `const`
                // holds** ([ADR-079](../../docs/specification/adr/adr-079.md)
                // D1). A `String` allocates and `const X: String` is not
                // something the language below has; `const X: &str` is, and its
                // lifetime is `'static` by the elision a `const` already makes.
                Some(build_time::Value::Text(_)) => Some("&str".to_string()),
                // A pair with nothing declaring what it is for says nothing:
                // `Fixed[K, V]` is the declaration that makes a list of them a
                // table, and without it there is no type to write.
                Some(build_time::Value::Tuple(_)) => None,
                // **A struct is its own name below**, and the fields carry
                // themselves — `const P: Point = Point { x: 1, y: 2 };` is
                // Rust, and a struct whose fields own nothing is already the
                // view form ([ADR-079](../../docs/specification/adr/adr-079.md)
                // D1's *a number is already its own view*, one shape out).
                Some(build_time::Value::Struct { name, .. }) => Some(name.clone()),
                // …and a variant is its `enum`'s name, one shape over.
                Some(build_time::Value::Variant { ty, .. }) => Some(ty.clone()),
                None => None,
            },
        };

        // **A map the build can see** ([ADR-176](../../docs/specification/adr/adr-176.md)
        // D1): a list of pairs, and the **declared type** is what says it is a
        // table rather than a list — the rule
        // [ADR-152](../../docs/specification/adr/adr-152.md) D4 already makes
        // for `Array[T, N]`, one shape out. Both halves at once, because the
        // type below and the value below are one answer here: `Fixed<i64>` and
        // a `Fixed::new(…)` are written from the same table.
        let (crossed, said_a_table) = match (&want, &evaluated) {
            (Some(want), Some(build_time::Value::List(pairs))) if is_fixed(want) => {
                self.a_table_that_crosses(&bound, want, pairs, span)
            }
            _ => (None, false),
        };

        // **The value, spelled below, read against the declaration**
        // ([ADR-179](../../docs/specification/adr/adr-179.md) D2). Every shape
        // but one spells itself — an integer is its digits wherever it stands
        // — and a **list** is the one that does not: the same value is
        // `[1, 2, 3]` against an `Array[i64, 3]` and `&[1, 2, 3]` against a
        // `&[i64]`, so the type is walked beside it.
        //
        // A pair has no `const` form of its own, and a list of them has one
        // only where a declaration says it is a table — which `crossed` below
        // is.
        let written = match &evaluated {
            Some(value) => self.written_below(value, want.as_ref()),
            None => None,
        };
        let (below, written) = match crossed {
            Some((below, written)) => (Some(below), Some(written)),
            None => (below, written),
        };
        // **A part of the value the language below cannot write.**
        //
        // Asked of the **value** rather than of the declared type, because an
        // `enum` is not answerable from its type: `Shape::Empty` is a `const`
        // and `Shape::Many([1, 2])` is not, and the two are the same `Shape`.
        // Without this the second one lowered — `const M: Shape =
        // Shape::Many([1, 2]);` against a variant that declares a `Vec` — and
        // `rustc` answered about the generated file, which is [Part III
        // C.1](../../docs/specification/30-nikaia-tooling.md)'s class.
        let said_a_part = match &evaluated {
            Some(value) => match self.unwritable_in(value) {
                Some((where_it_is, held)) => {
                    self.a_field_that_owns_memory(&bound, &where_it_is, &held, span);
                    true
                }
                None => false,
            },
            None => false,
        };
        match (&below, &written) {
            // **Nothing is recorded where a part of it cannot be written**,
            // or the emitter would write the `const` the refusal just said is
            // not one.
            (Some(below), Some(written)) if !said_a_part => {
                self.checked
                    .comptime_values
                    .insert(span.start, (below.clone(), written.clone()));
            }
            // …and nothing at all where the refusal has already been made by
            // name: `NK1152` and `NK1165` each say what `NK1127` would, with
            // the part that matters in it.
            _ if said || said_a_type || said_a_table || said_a_part => {}
            _ => self.checked.findings.push(Finding {
                code: "NK1127",
                severity: Severity::Error,
                span: span.clone(),
                message: format!("this compiler cannot evaluate `{bound}` while it builds"),
                notes: vec![
                    "a `comptime` is a `let` that *must* fold, so one that cannot is \
                         refused rather than computed while the program runs (Part II, 10.2)"
                        .to_string(),
                    "what it evaluates today is an integer, a **float**, a `bool`, \
                         **text**, a **list**, a `struct` and an `enum` variant - a \
                         literal, `f\"… {n} …\"`, arithmetic and \
                         comparisons over literals and over other constants, `+`, `==` and \
                         `.len()` over text, an `if`, a **call** to a function or a **method** \
                         this **program** declares, whose body is made of those, a `for` over a range or a `while` \
                         inside such a body, and `xs[i]`, `xs[i] = …`, `xs.push(…)` and \
                         `xs.len()` over a list it holds (ADR-073 D5's second stage). \
                         What a build-time value owns, the program gets a view of: a \
                         list crosses as an `Array[T, N]` and text as a `&str`, because \
                         a `const` below holds neither a `Vec` nor a `String` (ADR-079 \
                         D1)"
                    .to_string(),
                ],
                help: Some(format!(
                    "write `let {bound} = …` if it is meant to be computed while the \
                         program runs"
                )),
            }),
        }

        let held = want.unwrap_or(found);
        // **What the name is worth is what was *evaluated*, not what folded.**
        // They were the same thing while the fold was the whole evaluator; with
        // a call in it they are not, and binding the fold left
        // `comptime ANSWER = double(21)` visible as a name with no value — so
        // the next constant that read it was `NK1127` although the one before
        // it had just been computed.
        self.bind_local(Local {
            literal: None,
            name: bound,
            ty: held,
            lent: false,
            changing: false,
            // The fold's number, which is what `constant_of` reads one
            // `comptime` later and what `ADR-043` D5's overflow check needs.
            constant: match &evaluated {
                Some(build_time::Value::Int(value)) => Some(*value),
                _ => None,
            },
            // …and the whole value, which is what a *call* one `comptime`
            // later needs: `ORIGIN.scaled(10)` wants a `Point`.
            built: evaluated,
            immutable: None,
            empty_list: None,
        });
        Ty::Tuple(Vec::new())
    }

    /// `NK1133`: a statement after a `break` or a `continue`, in the same block.
    ///
    /// **The shape this is really about is `break i`**
    /// ([ADR-084](../../../docs/specification/adr/adr-084.md) D3). A `break` in
    /// Rust carries a value out of a `loop`; here a loop is a statement and
    /// hands back nothing ([ADR-070](../../../docs/specification/adr/adr-070.md)
    /// D2), so the word takes no value - and a value written after it parses as
    /// a statement of its own. Without this the program compiles, the value is
    /// dropped, and nothing says so.
    ///
    /// It is stated as what it is rather than as a guess at intent: the
    /// statement is not reached. That covers `break i`, `continue x` and the
    /// line somebody left below a `break` while editing, in one sentence and
    /// with one caret.
    fn nothing_follows_a_jump(&mut self, jump: &Stmt, span: &Span) {
        let word = match jump {
            Stmt::Break => "break",
            _ => "continue",
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1133",
            message: format!("nothing after a `{word}` in the same block is reached"),
            notes: vec![format!(
                "`{word}` takes no value: a loop is a statement here and hands back nothing, \
                 so `{word} x` is two statements rather than one (Part I, 3.3)"
            )],
            // **The two shapes that carry a value out of a loop**
            // ([ADR-151](../../docs/specification/adr/adr-151.md) D2), and the
            // `return` one is second because that is what a search loop is
            // usually written as. It said *bind it before the `break`* alone,
            // which is one of the two and not the one a reader wants.
            help: Some(format!(
                "delete it - or, where a value was meant: a `let` before the loop that \
                 the loop assigns, or a `return`, where the function has nothing left to \
                 do after the `{word}`"
            )),
        });
    }

    /// **What a `return` hands back, asked once**
    /// ([ADR-138](../../docs/specification/adr/adr-138.md) D2): the statement
    /// and the expression are the same `return`, so they ask the same
    /// questions in the same order rather than in two places that drift.
    fn returns(&mut self, value: Option<&Expr>, span: &Span) {
        // **The `&` at a `return`** ([ADR-094](../../docs/specification/adr/adr-094.md)
        // D1's third position), asked before the refusal it takes the place of:
        // a place inside a borrowed subject, handed back where the function
        // declares a view, is a view *of* the subject and not a move out of it.
        let lending = value.is_some_and(|value| self.hands_back_a_view_of_the_subject(value));
        if lending {
            self.checked.lent_returns.insert(span.start);
        }
        if let Some(value) = value
            && !lending
        {
            self.a_field_of_a_borrowed_subject(value, span, "handed back");
        }
        let found = match value {
            Some(value) => self.expr(value, span),
            None => Ty::Tuple(Vec::new()),
        };
        let Some(expected) = self.expected.clone() else {
            return;
        };
        // The line hands back the **view**, so that is what answers to the
        // declared type. `view_of` and not a `&` pasted on the front, for the
        // reason the argument position has it: a view of a `String` is a `ref
        // String` and the one rewrite Part I 6.5 makes.
        let found = match lending {
            true => view_of(&found),
            false => found,
        };
        let mut found = found;
        if let Some(value) = value {
            self.constant_fits(value, Some(&expected), span);
            self.wraps_into_nullable(&found, &expected, value, span);
            // **The declared result is a use** (ADR-152 D4).
            found = self
                .literal_by_use(&found, &expected, value, span)
                .unwrap_or(found);
        }
        self.expect_kept(
            &found,
            &expected,
            value,
            "the function hands it to its caller, who keeps it after this call has ended",
            span.clone(),
            "returns",
            |found, want| format!("this returns `{found}`, and the function declares `{want}`"),
        );
    }

    /// `NK1132`: a `break` or a `continue` with no loop to act on.
    ///
    /// Two shapes and one code, because they are one mistake reached from two
    /// sides - there is no loop, or there is one and a function boundary
    /// stands between. The second is the one worth the code: it is a program
    /// that *looks* right, and without this it would be refused by `rustc`
    /// about the emitted file (Part III, C.1).
    fn a_jump_with_nowhere_to_go(&mut self, word: &str, span: &Span) {
        let does = match word {
            "break" => "leaves the innermost loop around it",
            _ => "starts the innermost loop's next turn",
        };
        let (message, note, help) = match self.barrier {
            Some(what) => (
                format!("`{word}` {does}, and the nearest loop is outside this {what}"),
                format!(
                    "{} is a function of its own in the language below, and a jump does \
                     not leave a function",
                    an(what)
                ),
                format!(
                    "decide inside the {what} and act on the answer outside it - a `bool` it \
                     hands back, tested by the loop"
                ),
            ),
            None => (
                format!("`{word}` {does}, and this is not in a loop"),
                "a `while` or a `for` is what it acts on, and there is none here (Part I, 3.3)"
                    .to_string(),
                match word {
                    "break" => "to leave the function rather than a loop, write `return`",
                    _ => "to leave the function rather than start a turn, write `return`",
                }
                .to_string(),
            ),
        };
        self.checked.findings.push(Finding {
            severity: Severity::Error,
            span: span.clone(),
            code: "NK1132",
            message,
            notes: vec![note],
            help: Some(help),
        });
    }

    /// Note where a method call in the function being walked went (ADR-028).
    ///
    /// `None` is "I could not find out", and it is recorded rather than
    /// dropped: an analysis that claims a property has to be able to tell that
    /// apart from a body that called nothing.
    fn reached_method(&mut self, key: Option<&str>) {
        // **Before the `current` gate**, which is about which function's set an
        // answer lands in and not about whether there is one: a method nothing
        // describes says nothing about failing, wherever it stands
        // ([ADR-091](../../../docs/specification/adr/adr-091.md)).
        if key.is_none() {
            self.guard_has_no_answer();
        }
        for frame in &mut self.stored_frames {
            match key {
                Some(key) => {
                    frame.resolved.insert(key.to_string());
                }
                None => frame.unresolved = true,
            }
        }
        let Some(current) = &self.current else {
            return;
        };
        // **Which side of a `spawn` this call is on** (ADR-039 D3). The stack
        // is the one the task walk already keeps, so "inside a task" is
        // exactly what it says.
        let inside_a_task = !self.task_bindings.is_empty();
        let entry = self.checked.methods.entry(current.clone()).or_default();
        match key {
            Some(key) => {
                entry.resolved.insert(key.to_string());
                if inside_a_task {
                    entry.in_a_task.resolved.insert(key.to_string());
                }
            }
            None => {
                entry.unresolved = true;
                entry.unanswered += 1;
                entry.in_a_task.unresolved |= inside_a_task;
            }
        }
    }

    /// **Something the guarded expression of a `catch` holds and this compiler
    /// cannot see the end of** ([ADR-091](../../../docs/specification/adr/adr-091.md)).
    ///
    /// Called where a call could not be resolved and where a `catch` stands
    /// inside another one's guarded expression. A no-op outside a guarded
    /// expression, which is most of a program.
    fn guard_has_no_answer(&mut self) {
        if let Some(guarded) = &mut self.guarded {
            guarded.unanswered = true;
        }
    }

    /// A method on a type, by the name `Type::method` the ledger records it
    /// under. A library writes the module in front of it (`fs::Mapped::deref`)
    /// and the receiver's type may or may not carry one, so both directions are
    /// tried.
    ///
    /// **The receiver's own module is dropped on the second try**, since
    /// [ADR-154](../../docs/specification/adr/adr-154.md) D3 put a type in one:
    /// a value of `collections::HashMap` has its methods keyed `HashMap::len`,
    /// because the module is where the **type** lives and the method belongs to
    /// the type. This is not the bare-name resolution that record took away —
    /// there the question was *which entry does this word mean*, and here the
    /// receiver's type is in hand and its last segment is that type's name.
    fn method(&self, key: &str) -> Option<(String, &'a FnContract)> {
        if let Some(contract) = self.own.functions.get(key) {
            return Some((key.to_string(), contract));
        }
        let suffix = format!("::{key}");
        let found = self
            .library
            .functions
            .iter()
            .find(|(name, _)| *name == key || name.ends_with(&suffix))
            .map(|(name, contract)| (name.clone(), contract));
        if found.is_some() {
            return found;
        }
        // `collections::HashMap::len` was asked for; `HashMap::len` is the key.
        let (_, without_the_module) = key.split_once("::")?;
        if !without_the_module.contains("::") {
            return None;
        }
        self.own
            .functions
            .get(without_the_module)
            .map(|contract| (without_the_module.to_string(), contract))
            .or_else(|| {
                self.library
                    .functions
                    .get(without_the_module)
                    .map(|contract| (without_the_module.to_string(), contract))
            })
    }

    /// The fields of a type, when something knows them.
    /// What a value's own type arguments bind its declaration's parameters to.
    ///
    /// `Pair[i64]` against `struct Pair[T]` gives `T = i64`, positionally,
    /// because a type argument's position is the only thing that says which
    /// parameter it fills. Empty for every type that declares none, which is
    /// every type written today.
    fn arguments_of(&self, on: &Ty) -> BTreeMap<String, Ty> {
        let Ty::Named { name, args, .. } = on else {
            return BTreeMap::new();
        };
        let Some(order) = self.struct_parameters.get(name) else {
            return BTreeMap::new();
        };
        order
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .filter(|(_, arg)| !arg.is_unknown())
            .collect()
    }

    fn fields_of(&self, name: &str) -> Option<Vec<FieldContract>> {
        if let Some(fields) = self.structs.get(name) {
            return Some(fields.clone()).filter(|f: &Vec<_>| !f.is_empty());
        }
        // A type of **another file of this program**, by the qualified name a
        // caller writes: `pool::Conn`. By the exact key and never by suffix,
        // which is the difference from the library below - two modules may each
        // declare a `Conn`, and a suffix match would answer with whichever came
        // first. `std` has no such ambiguity and a value's type does not carry
        // the module a library writes in front of it, which is why that one is
        // matched the other way (ADR-011 D2).
        if let Some(contract) = self.own.types.get(name) {
            return Some(contract.fields.clone()).filter(|f: &Vec<_>| !f.is_empty());
        }
        let suffix = format!("::{name}");
        self.library
            .types
            .iter()
            .find(|(key, _)| *key == name || key.ends_with(&suffix))
            .map(|(_, contract)| contract.fields.clone())
            .filter(|f| !f.is_empty())
    }

    /// Whether a step of this type can fail, as a ledger records it
    /// (ADR-025 D6). Matched by suffix, because a library writes the module in
    /// front of a type's name and a value's type does not carry one.
    fn iterates_fallibly(&self, name: &str) -> bool {
        let suffix = format!("::{name}");
        [self.own, self.library].iter().any(|ledger| {
            ledger.types.iter().any(|(key, contract)| {
                (key == name || key.ends_with(&suffix)) && contract.iterates_fallibly
            })
        })
    }

    fn is_variant(&self, ty: &str, variant: &str) -> bool {
        self.enums
            .get(ty)
            .is_some_and(|variants| variants.contains(variant))
    }

    /// **Whether a `match` is over a place this function owns**: a name, or a
    /// field of one, that is not itself a view. Only there can a part be lent
    /// and the whole stay: a temporary is gone after the `match` anyway, and a
    /// view's parts are views already.
    fn an_owned_place(&self, value: &Expr, on: &Ty) -> bool {
        if on.is_a_view() || !matches!(on, Ty::Named { .. }) {
            return false;
        }
        let mut root = value;
        while let Expr::Field { base, .. } = root {
            root = base;
        }
        let Expr::Variable(name) = root else {
            return false;
        };
        let name = self.parsed.text(*name);
        self.binding(name)
            .is_some_and(|local| !local.lent && !local.ty.is_a_view())
            && !self.lent_here(name)
    }

    /// **The types of the names an arm binds**, where the variant says them:
    /// a positional variant's parts in order, a variant with named fields by
    /// name. Nested patterns and or-patterns are left untyped, which leaves
    /// their names bound as they always were.
    fn pattern_parts(&self, pattern: &MatchPattern, on: &Ty) -> BTreeMap<String, Ty> {
        let Ty::Named { name: owner, .. } = on else {
            return BTreeMap::new();
        };
        let variant = |path: &[Ident]| -> String {
            let written: Vec<&str> = path.iter().map(|s| self.parsed.text(*s)).collect();
            match written.as_slice() {
                [one] => format!("{owner}::{one}"),
                _ => written.join("::"),
            }
        };
        let mut out = BTreeMap::new();
        match pattern {
            MatchPattern::Tuple { path, parts } if !path.is_empty() => {
                let key = variant(path);
                let types: Vec<Ty> = match self.variant_parts.get(&key) {
                    Some(types) => types.clone(),
                    None => self
                        .structs
                        .get(&key)
                        .map(|fields| fields.iter().map(|f| f.ty.clone()).collect())
                        .unwrap_or_default(),
                };
                for (part, ty) in parts.iter().zip(types) {
                    if let MatchPattern::Path(one) = part
                        && let [name] = one.as_slice()
                    {
                        out.insert(self.parsed.text(*name).to_string(), ty);
                    }
                }
            }
            MatchPattern::Named { path, bindings, .. } => {
                let fields = self.structs.get(&variant(path));
                for binding in bindings {
                    let name = self.parsed.text(*binding);
                    if let Some(field) = fields.and_then(|f| f.iter().find(|f| f.name == name)) {
                        out.insert(name.to_string(), field.ty.clone());
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// **The parts an arm only reads are lent** (ADR-242): a part
    /// that does not copy, which nothing in the arm took, returned or handed
    /// back as the arm's value, is bound as a view, and the scrutinee stays
    /// whole for what comes after the `match`.
    fn a_part_lent_where_it_is_not_kept(
        &mut self,
        pattern: &MatchPattern,
        body: &Expr,
        parts: &BTreeMap<String, Ty>,
        (_, from): (usize, usize),
        changed_from: usize,
    ) {
        let lent: BTreeSet<String> = parts
            .iter()
            .filter(|(name, ty)| {
                let taken = self.handed[from..].iter().any(|taken| {
                    taken.path == **name || taken.path.starts_with(&format!("{name}."))
                }) || self.changed[changed_from..].iter().any(|n| n == *name);
                moves_away(ty) && !taken && !gives_back(body, name, self.parsed)
            })
            .map(|(name, _)| name.clone())
            .collect();
        if !lent.is_empty() {
            let at = pattern as *const MatchPattern as usize;
            self.checked.lent_bindings.insert(at, lent);
        }
    }

    /// The names a `match` arm brings into scope, all of them unknown: what a
    /// variant carries is not in the ledger yet.
    fn pattern_bindings(&self, pattern: &MatchPattern) -> Vec<Local> {
        self.pattern_names(pattern)
            .into_iter()
            .map(|name| Local::free(name, Ty::Unknown))
            .collect()
    }

    /// The names one pattern binds, in the order it writes them.
    ///
    /// **Recursive, because a pattern nests**
    /// ([ADR-137](../../docs/specification/adr/adr-137.md) D1): a tuple's parts
    /// are patterns, so `Event::Click(Point { x, .. })` binds what the part
    /// inside it binds.
    ///
    /// **An or-pattern hands back its first alternative's names**, which is the
    /// answer that is right *once `NK1155` has run*: every alternative binds the
    /// same set, so any of them will do — and where they do not, the refusal is
    /// the finding rather than a scope this guessed at.
    fn pattern_names(&self, pattern: &MatchPattern) -> Vec<String> {
        match pattern {
            MatchPattern::Otherwise | MatchPattern::Literal(_) | MatchPattern::Range { .. } => {
                Vec::new()
            }
            // A single segment binds; `Op::Times` names a variant.
            MatchPattern::Path(segments) if segments.len() == 1 => {
                vec![self.parsed.text(segments[0]).to_string()]
            }
            MatchPattern::Path(_) => Vec::new(),
            MatchPattern::Tuple { parts, .. } => {
                parts.iter().flat_map(|p| self.pattern_names(p)).collect()
            }
            MatchPattern::Named { bindings, .. } => bindings
                .iter()
                .map(|b| self.parsed.text(*b).to_string())
                .collect(),
            MatchPattern::Or(alternatives) => alternatives
                .first()
                .map(|first| self.pattern_names(first))
                .unwrap_or_default(),
        }
    }
}

/// What one turn of a `for` binds, when the collection's element type is
/// written down.
///
/// Only a list with one element type and one binding: `for (k, v) in map`
/// takes apart a pair whose shape Stage 0 has no signature for, and a view of
/// a collection yields views whose spelling the language below chooses.
/// Whether a method takes its receiver **by value**, which for a produced
/// sequence is what consumes it ([ADR-105](../../docs/specification/adr/adr-105.md) D2).
///
/// Read off the signature rather than off a list of names: every `Seq` entry
/// writes `(Seq[$T], …)` and a container's writes `(&Vec[$T], …)`, so the file
/// that describes the method is what says whether the walk keeps it.
/// The ledger entries whose argument at a position is a count the language
/// below takes in `usize` ([ADR-212](../../docs/specification/adr/adr-212.md)
/// D5). The entry and not the method's name, which is what `emit::is_count`
/// has to go by.
const COUNTS: &[(&str, usize)] = &[
    ("Seq::take", 0),
    ("Seq::skip", 0),
    ("Seq::step_by", 0),
    ("Seq::nth", 0),
    ("Vec::chunks", 0),
    ("Vec::windows", 0),
];

/// **What a sequence entry's result is as a whole**
/// ([ADR-212](../../docs/specification/adr/adr-212.md) D2): a word the entry
/// writes on its result, where every sequence handed in has it too.
///
/// A `map` walked from the back is its input walked from the back, so `map`
/// writes `ends sized` and a `filter` over `io::lines()` still cannot be. Where
/// the entry says `ends_by_length`, walking the result from the back also needs
/// every input's length - `take`, `zip`, `skip`, `step_by`.
///
/// **A result never replays** unless the entry says so: a range replays, and a
/// `rev` of one is an ordinary sequence, walked once.
fn shape_through(contract: &FnContract, receiver: &Ty, found: &[Ty], result: Ty) -> Ty {
    let Ty::Seq {
        item,
        is_sync,
        pauses,
        throws,
        parallel,
        shape: written,
    } = result
    else {
        return result;
    };
    let mut inputs: Vec<ty::Shape> = Vec::new();
    if let Some(signature) = &contract.signature {
        let shape_of = |actual: &Ty| match actual {
            Ty::Seq { shape, .. } => *shape,
            _ => ty::Shape::default(),
        };
        if let Some((name, Ty::Seq { .. })) = signature.params.first()
            && name == "self"
        {
            inputs.push(shape_of(receiver));
        }
        for ((_, pattern), actual) in signature.arguments().iter().zip(found) {
            if matches!(pattern, Ty::Seq { .. }) {
                inputs.push(shape_of(actual));
            }
        }
    }
    let sized = inputs.iter().all(|s| s.sized);
    let ends = inputs.iter().all(|s| s.ends) && (!contract.ends_by_length || sized);
    Ty::Seq {
        item,
        is_sync,
        pauses,
        throws,
        parallel,
        shape: ty::Shape {
            ends: written.ends && ends,
            sized: written.sized && sized,
            replays: written.replays,
        },
    }
}

/// **A lazy walk of a sequence whose step pauses**
/// ([ADR-233](../../docs/specification/adr/adr-233.md) D1,
/// [ADR-234](../../docs/specification/adr/adr-234.md) D1): one this compiler
/// made pause, by handing a pausing lambda to a `map` or a `filter`, or
/// `io::lines()`, whose step can fail as well. Both have every lazy walk `Seq`
/// has but `rev`, as adapters of their own.
fn a_paused_chain(key: &str, on: &Ty) -> bool {
    matches!(
        key,
        "Seq::map" | "Seq::filter" | "Seq::take" | "Seq::skip" | "Seq::step_by" | "Seq::zip"
    ) && matches!(on, Ty::Seq { pauses: true, .. })
}

/// What a `map` or a `filter` hands back where its step pauses
/// ([ADR-233](../../docs/specification/adr/adr-233.md) D1): a sequence whose
/// step pauses, walked from the front only and with no length until the end.
///
/// **And one whose step can fail where what it walks can**
/// ([ADR-234](../../docs/specification/adr/adr-234.md) D1): a `map` of
/// `io::lines()` has produced nothing yet, so its failures are still ahead of
/// whatever walks it, and the `?` goes there.
fn a_pausing_sequence(result: Ty, on: &Ty) -> Ty {
    let fails = matches!(on, Ty::Seq { throws: true, .. });
    match result {
        Ty::Seq {
            item,
            throws,
            parallel,
            shape,
            ..
        } => Ty::Seq {
            item,
            is_sync: false,
            pauses: true,
            throws: throws || fails,
            parallel,
            shape: ty::Shape {
                ends: false,
                sized: false,
                ..shape
            },
        },
        other => other,
    }
}

fn walks_by_value(contract: &FnContract) -> bool {
    let Some(signature) = &contract.signature else {
        return false;
    };
    matches!(
        signature.params.first().map(|(_, ty)| ty),
        Some(Ty::Seq { .. })
    )
}

/// The sentence a word that used to be reserved gets instead
/// ([ADR-117](../../docs/specification/adr/adr-117.md) D2).
///
/// `None` for every other name, which keeps the general help exactly as it was:
/// this adds a sentence where the word is one of the four and changes nothing
/// anywhere else. Where the word **is** declared — a local, a parameter, a field
/// — the caller never gets here, which is D2's *nothing is said*.
fn a_word_that_was_reserved(name: &str) -> Option<&'static str> {
    match name {
        "loop" => Some(
            "there is no unconditional loop keyword: write `while true { … }` (Part I, 3.3). \
             `loop` is an ordinary name otherwise, so `let loop = 3` is a program",
        ),
        "const" => Some(
            "a value the compiler must work out while it builds is `comptime X = …` \
             (Part II, 10.2) - the word says *time* rather than *mutability*, which is what \
             `const` means in the languages it comes from. `const` is an ordinary name \
             otherwise",
        ),
        "macro" | "quote" => Some(
            "Nikaia has no macros: generating code from a type's shape is `comptime` and a \
             bound (Part II, 10.3), and a grammar is how text becomes a program \
             (Part II, 10.1). Both words are ordinary names otherwise",
        ),
        _ => None,
    }
}

/// **Whether this file declares a type by this name**
/// ([ADR-183](../../docs/specification/adr/adr-183.md) D1).
///
/// For the package's *other* files, which share one namespace with this one
/// (Part I 9.1). It reads the item tree rather than a ledger because a ledger
/// records what a type **promises**, and what is asked here is only that the
/// word was written down: a private `enum` a neighbouring file declares is a
/// head, whatever its contract says.
/// **The names a body gives text of its own by growing them**: `s = s + "…"`,
/// `s += "…"` or `s = f"…"`, anywhere in it (0.0.232). Syntactic, because it is
/// asked at the `let`, before the assignment has been walked; and narrow for
/// the same reason - `+` and an f-string can only hand back owned text, where a
/// name or a call may hand back a view.
fn grown_text(parsed: &Parsed, body: &Block) -> BTreeSet<String> {
    fn walk(parsed: &Parsed, block: &Block, out: &mut BTreeSet<String>) {
        for stmt in &block.stmts {
            if let Stmt::Assign {
                target: Expr::Variable(name),
                op,
                value,
            } = &stmt.node
            {
                let grows = matches!(op, Some(BinaryOp::Add))
                    || (op.is_none()
                        && matches!(
                            value,
                            Expr::Binary {
                                op: BinaryOp::Add,
                                ..
                            } | Expr::LitInterpolated(_)
                        ));
                if grows {
                    out.insert(parsed.text(*name).to_string());
                }
            }
            crate::contracts::sync::visit_stmt_blocks(&stmt.node, &mut |inner| {
                walk(parsed, inner, out)
            });
        }
    }
    let mut out = BTreeSet::new();
    walk(parsed, body, &mut out);
    out
}

fn declares_a_type(parsed: &Parsed, name: &str) -> bool {
    parsed.program.items.iter().any(|item| {
        let written = match &item.node {
            Item::Struct { name, .. } | Item::Enum { name, .. } => Some(*name),
            Item::Grammar(def) => Some(def.name),
            Item::Trait { name, .. } => Some(*name),
            _ => None,
        };
        written.is_some_and(|written| parsed.text(written) == name)
    })
}

fn element_of(over: &Ty, bindings: usize) -> Ty {
    match over {
        // **A list lent to the function is walked the same way** (0.0.238):
        // `for c in long` over a `long: ref Vec[char]` binds each `char`, and
        // the binding is a view below either way (`Local::lent`). Answering
        // `Unknown` for the view left every such binding unchecked, and a
        // comparison of it with a value reached `rustc` as `char == &char`.
        Ty::Named { name, args, .. } if bindings == 1 && args.len() == 1 => match name.as_str() {
            "Vec" | "List" => args[0].clone(),
            _ => Ty::Unknown,
        },
        // **An array's element is its first argument**
        // ([ADR-152](../../docs/specification/adr/adr-152.md) D4), and the
        // second is the length, which is why the arm above does not reach it:
        // `Array[i64, 2]` has two arguments where a `Vec[i64]` has one.
        //
        // Without this a `for` over an array bound a name of **unknown** type,
        // and everything downstream of it went quiet — including
        // [ADR-043](../../docs/specification/adr/adr-043.md) D4's abort, so
        // `for n in NS { n as u8 }` over an `Array[i64, 2]` **truncated
        // silently**, which is the one thing that record exists to stop.
        Ty::Named { name, args, view }
            if bindings == 1 && !view && name == ty::ARRAY && !args.is_empty() =>
        {
            args[0].clone()
        }
        // …and a `&[T]` carries its `T` the same way
        // ([ADR-179](../../docs/specification/adr/adr-179.md) D1). Both are a
        // run; which of the two carries the length is not what a walk asks.
        Ty::Pointed { .. } if bindings == 1 => match slice_element(over) {
            Some(item) => item.clone(),
            None => Ty::Unknown,
        },
        // **A produced sequence is what a `for` walks**
        // ([ADR-105](../../docs/specification/adr/adr-105.md) D1), and its item
        // is the binding's type: `for c in text.chars()` binds a `char`. A
        // container is walked *as* a `Seq` by a `for` and is not one, which is
        // why the arm above stays where it is.
        //
        // One binding only, as above: `for (k, v) in map.drain()` takes the
        // pair apart, and which half is which is the tuple arm's question
        // rather than this one's.
        Ty::Seq { item, .. } if bindings == 1 => (**item).clone(),
        _ => Ty::Unknown,
    }
}

/// `&x`, as far as Stage 0 can say.
///
/// A view of a `String` is a `&str`, because that is what the language below
/// does at a call and what a Nikaia programmer means by writing it. A view of
/// anything with type arguments is not stated: `&Vec[T]` and `&[T]` are the
/// same expression there, and picking one would report an error that is not
/// there.
fn view_of(inner: &Ty) -> Ty {
    match inner {
        // A view of a view is the view: `&&str` is not a type this language has.
        Ty::Named { view: true, .. } => inner.clone(),
        // `&String` is the one rewrite, because a text view is spelled `&str`
        // and `&String` is not a type a signature may ask for (6.5).
        Ty::Named { name, args, .. } if args.is_empty() && name == "String" => Ty::view("str"),
        // **Arguments are kept.** `&Vec[i64]` is a view of a `Vec[i64]`, and
        // answering `Unknown` here left every view of a generic type
        // *unchecked* - so a mismatch was reported by `rustc`, about the
        // generated file, which is the one thing Part III C.1 forbids.
        Ty::Named { name, args, .. } => Ty::Named {
            name: name.clone(),
            args: args.clone(),
            view: true,
        },
        // **A view of a `T?` is a nullable view**, which is the one shape
        // Part I 2.3 already writes: `&str?` is a view that may be absent, and
        // `is_view` and `is_nullable` are independent flags on a written type
        // for exactly that reason.
        //
        // Answering `Unknown` here left `&m[k]` unchecked the day a map read
        // became a `T?` ([ADR-114](../../docs/specification/adr/adr-114.md)
        // D1): `let s = &m[k]` followed by `s.min` reached no `NK1125` and was
        // refused by `rustc`, about the generated file
        // ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
        Ty::Nullable(inner) => Ty::Nullable(Box::new(view_of(inner))),
        // A tuple of views is not a view of a tuple, and nothing writes down
        // what a view of a lambda would be.
        _ => Ty::Unknown,
    }
}

/// What a `&x` was a view **of**, as far as [`view_of`] runs backwards.
///
/// It does not run backwards cleanly — a `&str` is a view of a `String` and
/// also a `&str` standing on its own — so this answers the owned types that
/// would have produced the view, and the caller asks the fit of each.
///
/// The one place that needs it is [`Checker::the_compiler_writes_the_reference`],
/// deciding whether a `&` the source wrote is the one the compiler would have
/// written anyway. There, a list that is too short costs a refusal that is not
/// raised and a list that is too long costs a correct program refused, so it is
/// the short one: only the views this compiler itself makes.
fn viewed_from(view: &Ty) -> Vec<Ty> {
    match view {
        Ty::Named {
            name,
            args,
            view: true,
        } if args.is_empty() && name == "str" => vec![Ty::named("String")],
        Ty::Named {
            name,
            args,
            view: true,
        } => vec![Ty::Named {
            name: name.clone(),
            args: args.clone(),
            view: false,
        }],
        _ => Vec::new(),
    }
}

/// The type several parts of a program own at once (Part I 6.2).
const SHARED: &str = "Shared";
/// Part I 6.3's lock, which shares the `Shared` hull's construction line
/// ([ADR-057](../../../docs/specification/adr/adr-057.md)).
const LOCKED: &str = "Locked";

/// The shared mutable type: a value several parts own at once and any of them may
/// change ([ADR-039](../../../docs/specification/adr/adr-039.md) D9).
///
/// It is a **name this module carries whole**. Only the emitter expands it to the
/// count around the lock, which is what keeps a spelling nobody wrote out of every
/// message about the language's most common type
/// ([ADR-064](../../../docs/specification/adr/adr-064.md) D1).
const SHARED_MUT: &str = "SharedMut";

/// The three types whose hull a program writes by calling their name
/// ([ADR-064](../../../docs/specification/adr/adr-064.md) D2).
///
/// **A hull you cannot observe, the compiler writes; a hull you can observe, you
/// write.** A `T?` costs nothing and hides nothing - the same value, possibly
/// absent - so its `Some(…)` is written for you (ADR-052). These three change
/// **when a value is cleaned up**, which Part I 6.2 says a program can see, so the
/// word stands where it happens.
fn is_hull(name: &str) -> bool {
    HULLS.contains(&name)
}

/// The same three, as a list something outside this module can read
/// ([ADR-167](../../docs/specification/adr/adr-167.md) D1).
///
/// **They are reached with no `use`, so they are on Part I 1.3's list**, and a
/// list that says *there are no others* needs the compiler's own answer to
/// compare against. [ADR-162](../../docs/specification/adr/adr-162.md) D3 built
/// that comparison for every **function** `std` keys bare and a type escaped it,
/// because a type is keyed under its own name rather than under nothing.
///
/// This is the answer from the compiler's side rather than the ledger's, and it
/// is the right side: what makes these three names a program writes with no
/// import is that this file knows them by name.
pub const HULLS: [&str; 3] = [SHARED, SHARED_MUT, LOCKED];

/// The two doors that take **several** locks at once
/// ([ADR-065](../../../docs/specification/adr/adr-065.md)).
///
/// They are not ledger entries and not grammar: the parser already takes
/// `access_all(a, b) fn(x, y) { … }` as an ordinary call with a trailing lambda
/// (Part I 5.3), and what is missing is only somebody to type it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiLock {
    /// `access_all` - every lock read in place, none of them changed.
    Reading,
    /// `update_all` - every lock written, one new value each.
    Writing,
}

impl MultiLock {
    /// The name a program writes, or `None` for anything else.
    pub fn named(name: &str) -> Option<MultiLock> {
        match name {
            "access_all" => Some(MultiLock::Reading),
            "update_all" => Some(MultiLock::Writing),
            _ => None,
        }
    }

    pub fn written(self) -> &'static str {
        match self {
            MultiLock::Reading => "access_all",
            MultiLock::Writing => "update_all",
        }
    }

    fn shape(self) -> &'static str {
        match self {
            MultiLock::Reading => "access_all(a, b) fn(x, y) { … }",
            MultiLock::Writing => "update_all(a, b) fn(x, y) { … }",
        }
    }
}

/// What a lock holds, where the type says it is one.
///
/// `SharedMut[T]` is a count around a lock and `Locked[T]` is the lock on its
/// own; both hold a `T` and a door over several takes either.
fn locked_content_of(ty: &Ty) -> Option<Ty> {
    let Ty::Named { name, args, .. } = ty else {
        return None;
    };
    if name != SHARED_MUT && name != LOCKED {
        return None;
    }
    args.first().cloned()
}

/// Whether a value **is** a handle on a shared one, held by value.
///
/// `&Shared[T]` is not: lending the inner value out hands no handle on, so
/// nothing is duplicated ([ADR-040](../../../docs/specification/adr/adr-040.md)
/// D1's correction). A type that merely *holds* one is not either - a struct is
/// carried by whatever holds it.
fn is_a_handle(ty: &Ty) -> bool {
    matches!(ty, Ty::Named { name, view: false, .. } if name == SHARED || name == SHARED_MUT)
}

/// Whether a plain value stands where a **hull** is wanted - the shape whose
/// message names the constructor.
///
/// `want` is `Shared[T]`, `SharedMut[T]` or `Locked[T]` and `found` is the `T` it
/// would hold. It used to *permit* that at two positions, because the annotation
/// was the constructor ([ADR-040](../../../docs/specification/adr/adr-040.md) §3).
/// It permits nothing now
/// ([ADR-064](../../../docs/specification/adr/adr-064.md) D2): a hull is made by a
/// call, wherever a call may stand, and what this answers is only *which sentence
/// to write* when one is missing.
///
/// That is the whole of why the positions stopped being a list. A literal could
/// not be handled at all while the annotation was the constructor - it has no
/// type of its own, so nothing knew the hull was wanted - and `SharedMut(0)`
/// needs nobody to know.
fn becomes_shared(found: &Ty, want: &Ty) -> bool {
    let Ty::Named { name, args, view } = want else {
        return false;
    };
    if !is_hull(name) || *view {
        return false;
    }
    match args.as_slice() {
        [held] => {
            if found.is_unknown() {
                return false;
            }
            // A `SharedMut[T]` beside a `T` is the same shape as a `Shared[T]`
            // beside one: the value that would go in. Asked in this order so a
            // value that is *already* what the hull holds matches at the first
            // level ([ADR-064](../../docs/specification/adr/adr-064.md) D2).
            found.fits(held) || found.fits(&locked_content(held))
        }
        _ => false,
    }
}

/// What a `Locked[T]` holds, or the type itself where it is not one.
fn locked_content(ty: &Ty) -> Ty {
    match ty {
        Ty::Named { name, args, view } if name == LOCKED && !*view => match args.as_slice() {
            [held] => held.clone(),
            _ => ty.clone(),
        },
        _ => ty.clone(),
    }
}

/// Part III C.2: every diagnostic names a concrete way out.
fn convert(found: &Ty, want: &Ty) -> String {
    // **A shared value is wanted and a plain one is here**, and the place this
    // comes up is a `return` into a `-> Shared[T]` signature. Sharing may only
    // begin on a line that writes the type (Part I 6.2), and a signature line is
    // not that line - so the generic "make it a `Shared[T]`" below would name a
    // destination without a road, which is what made this look like a dead end.
    // It is not: there are two roads and this says both.
    if becomes_shared(found, want)
        && let Ty::Named { name, .. } = want
    {
        return format!("write `{name}(…)` around it - a hull you can see is one you write");
    }
    // **A slice where a list is declared** (ADR-215 D3): the parameter is
    // what changes, to the type that takes a run of any list - a slice and a
    // whole list alike - with the element the declaration already names.
    if let (
        Ty::Named {
            name: slice,
            args: run,
            view: true,
        },
        Ty::Named {
            name: list,
            args: elements,
            ..
        },
    ) = (found, want)
        && slice == ty::ARRAY
        && run.len() == 1
        && ty::base(list) == "Vec"
    {
        let element = elements
            .first()
            .map(Ty::text)
            .unwrap_or_else(|| "?".to_string());
        return format!(
            "this is a slice of a list; declare the parameter `ref Array[{element}]`, \
                 which takes a slice and a whole list alike"
        );
    }
    let (found, want) = (found.text(), want.text());
    match (found.as_str(), want.as_str()) {
        // **The spelling this checker prints is `ref String`**, and these arms
        // used to match `&str` - which nothing prints any more, so the one
        // mismatch every newcomer meets fell through to *make it a `String`*
        // and never said how. A literal no longer reaches here in a position
        // that wants text of its own (ADR-207 D2); what does is a view of text
        // the program **has**, and a copy of that is written, never inserted
        // (ADR-107 D3).
        ("ref String", "String") => {
            "write `.clone()` to make text of its own from this view - a copy is \
             written where it happens, never inserted (ADR-107 D3)"
                .to_string()
        }
        ("String", "ref String") => "write `ref` in front of it to take a view of it".to_string(),
        _ if is_number(&found) && is_number(&want) => {
            format!("write `as {want}` - Nikaia converts where you say so, never quietly")
        }
        _ => format!("make it a `{want}`, or change what is declared to `{found}`"),
    }
}

/// The name of the integer type this is, among the three Part I 2.2 offers.
///
/// A view or a type with arguments is neither: `&i32` is a reference and
/// `Vec[i32]` is a list, and a number does not stand beside either of them.
fn integer_named(ty: &Ty) -> Option<String> {
    let Ty::Named { name, args, view } = ty else {
        return None;
    };
    if !args.is_empty() || *view {
        return None;
    }
    matches!(name.as_str(), "i32" | "i64" | "u8").then(|| name.clone())
}

/// The value a block ends in: its last statement, where that is an expression.
fn tail_of(block: &Block) -> Option<&Expr> {
    match block.stmts.last().map(|s| &s.node) {
        Some(Stmt::Expr(value)) => Some(value),
        _ => None,
    }
}

/// Whether every way out of an expression is a text literal
/// ([ADR-207](../../docs/specification/adr/adr-207.md) D2): the literal itself,
/// or an `if`, a `match` or a block whose every arm ends in one.
fn ends_in_text(expr: &Expr) -> bool {
    match expr {
        Expr::LitStr { .. } => true,
        Expr::If {
            then_branch,
            else_branch: Some(else_branch),
            ..
        } => [tail_of(then_branch), tail_of(else_branch)]
            .iter()
            .all(|arm| arm.is_some_and(ends_in_text)),
        Expr::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|arm| ends_in_text(&arm.body))
        }
        Expr::Block(block) => tail_of(block).is_some_and(ends_in_text),
        _ => false,
    }
}

/// Whether an expression is a literal — a value written in the source rather
/// than computed.
///
/// **What it is for:** a literal is never a `T?`, whatever this checker did or
/// did not work out about its type. `null` is deliberately absent, because it
/// *is* one.
fn is_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::LitInt(_)
            | Expr::LitFloat(_)
            | Expr::LitStr { .. }
            | Expr::LitInterpolated(_)
            | Expr::LitChar(_)
            | Expr::LitBool(_)
    )
}

/// **Whether an arm hands a name back by value**: as its own value, or through
/// a `return`. Either needs the part itself and not a view of it.
fn gives_back(body: &Expr, name: &str, parsed: &Parsed) -> bool {
    fn is_name(expr: &Expr, name: &str, parsed: &Parsed) -> bool {
        matches!(expr, Expr::Variable(n) if parsed.text(*n) == name)
    }
    fn tail(expr: &Expr, name: &str, parsed: &Parsed) -> bool {
        match expr {
            Expr::Block(block) => block_tail(block, name, parsed),
            Expr::If {
                then_branch,
                else_branch,
                ..
            } => {
                block_tail(then_branch, name, parsed)
                    || else_branch
                        .as_ref()
                        .is_some_and(|b| block_tail(b, name, parsed))
            }
            Expr::Match { arms, .. } => arms.iter().any(|arm| tail(&arm.body, name, parsed)),
            other => is_name(other, name, parsed),
        }
    }
    fn block_tail(block: &Block, name: &str, parsed: &Parsed) -> bool {
        matches!(block.stmts.last().map(|s| &s.node), Some(Stmt::Expr(e)) if tail(e, name, parsed))
    }
    fn block_returns(block: &Block, name: &str, parsed: &Parsed) -> bool {
        block.stmts.iter().any(|stmt| match &stmt.node {
            Stmt::Return(Some(value)) => {
                is_name(value, name, parsed) || returns(value, name, parsed)
            }
            Stmt::For { iter, body, .. } => {
                returns(iter, name, parsed) || block_returns(body, name, parsed)
            }
            Stmt::While { cond, body } => {
                returns(cond, name, parsed) || block_returns(body, name, parsed)
            }
            Stmt::Let { value, .. } | Stmt::Comptime { value, .. } => returns(value, name, parsed),
            Stmt::Assign { value, .. } => returns(value, name, parsed),
            Stmt::Expr(value) => returns(value, name, parsed),
            _ => false,
        })
    }
    fn returns(expr: &Expr, name: &str, parsed: &Parsed) -> bool {
        let mut found = false;
        crate::emit::visit_expr(expr, &mut |inner| match inner {
            Expr::Return(Some(value)) if is_name(value, name, parsed) => found = true,
            Expr::Block(block) | Expr::Unsafe(block) => {
                found |= block_returns(block, name, parsed);
            }
            Expr::If {
                then_branch,
                else_branch,
                ..
            } => {
                found |= block_returns(then_branch, name, parsed)
                    || else_branch
                        .as_ref()
                        .is_some_and(|b| block_returns(b, name, parsed));
            }
            _ => {}
        });
        found
    }
    tail(body, name, parsed) || returns(body, name, parsed)
}

/// Whether taking a value of this type takes it **away** (`NK2101`).
///
/// Three answers and only one of them is a yes, which is the shape Part III C.4
/// asks for: a refusal on a guess is worse than no refusal.
///
///   * **No, it is copied.** A number, a `bool`, a `char` and a **view** all go
///     into a task and stay here too - Rust copies them, so `let n = 7` and a
///     `spawn` that prints `n` and a `println` that prints it again is a correct
///     program. A view is in this list because `&str` is `Copy`, which is why
///     `let message = "Hello"` is *not* the case Part I 8.3 is about and
///     `"Hello".to_string()` is.
///   * **No, it is duplicated.** A handle on a `Shared[T]` keeps working outside
///     the task ([ADR-040](../../docs/specification/adr/adr-040.md) D1, D5), and
///     Part I 8.3 says `NK2101` belongs to the data case only.
///   * **Nothing is claimed**, for a type nothing describes or a nullable of
///     one: `Unknown` is the absence of an answer and not a licence to refuse.
fn moves_away(ty: &Ty) -> bool {
    match ty {
        // **An array copies as its elements do**
        // ([ADR-152](../../docs/specification/adr/adr-152.md) D2): `N` elements
        // inline and nothing allocated, so an `Array[i64, 3]` takes as little
        // away as an `i64` does and an `Array[String, 3]` takes as much as a
        // `String`. The count among the arguments answers `false` on its own.
        Ty::Named { name, args, .. } if name == ty::ARRAY => args.iter().any(moves_away),
        Ty::Named { name, view, .. } => {
            !view
                && !is_number(name)
                && !matches!(name.as_str(), "bool" | "char")
                && name != "Shared"
                && name != "SharedMut"
        }
        // A nullable of data is still data: `Option<String>` moves.
        Ty::Nullable(inner) => moves_away(inner),
        // A tuple of copied parts is copied; one with anything else in it is
        // not. `Unknown`, a function type and a type variable claim nothing.
        Ty::Tuple(parts) => parts.iter().any(moves_away),
        _ => false,
    }
}

/// A relayed diagnostic, moved under the note that introduces it.
///
/// **Relayed whole and not re-worded**: the parser's message is in the
/// grammar's own vocabulary and counts its line and column against the bytes
/// that were parsed, which is the one place they mean anything. What this does
/// is put it where the eye is already looking.
fn indented(text: &str) -> String {
    text.trim_end()
        .lines()
        .map(|line| format!("       {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a name is one of the types **Part I 2.2** offers, which no
/// declaration in any program makes a `struct` or an `enum`.
///
/// This is the one half of a shape bound this compiler can refuse with
/// certainty: everything else it has not read a declaration for fails open.
fn is_one_of_part_one_2_2(name: &str) -> bool {
    is_number(name) || matches!(name, "bool" | "char" | "String" | "str")
}

/// A view of a value that copies - a number, a truth value, a character - and
/// nothing else (ADR-231 D1).
fn copies_as_a_view(ty: &Ty) -> bool {
    matches!(ty, Ty::Named { name, args, view: true }
        if args.is_empty() && (is_number(name) || name == "bool" || name == "char"))
}

/// `ref String` as the language below has it, a `&String`: what a `$T` bound
/// to `String` becomes under `ref $T`, which is an element of `words.iter()`.
fn is_a_view_of_a_string(ty: &Ty) -> bool {
    matches!(ty, Ty::Named { name, args, view: true } if args.is_empty() && ty::base(name) == "String")
}

/// A view of a value that copies read as the value, and anything else as it is
/// ([ADR-233](../../docs/specification/adr/adr-233.md) D4).
fn value_of_a_copy(ty: Ty) -> Ty {
    match copies_as_a_view(&ty) {
        true => unviewed(&ty),
        false => ty,
    }
}

/// The value a view is of.
fn unviewed(ty: &Ty) -> Ty {
    match ty {
        Ty::Named { name, args, .. } => Ty::Named {
            name: name.clone(),
            args: args.clone(),
            view: false,
        },
        other => other.clone(),
    }
}

/// **`--asserts`** ([ADR-245](../../docs/specification/adr/adr-245.md) D6):
/// every `assert` of one file, by line, and how the compiler holds it. Today
/// every one is held at run time (D4): there is no prover yet, and the report
/// exists so that the day there is one, its progress can be read here.
pub fn claims_report(
    parsed: &Parsed,
    source: &str,
    path: &str,
    own: &Ledger,
    library: &Ledger,
) -> String {
    let checked = check(parsed, own, library);
    let mut rows: Vec<(usize, &Claim)> = checked
        .claims
        .iter()
        .map(|((start, _), claim)| {
            let line = source[..(*start).min(source.len())].matches('\n').count() + 1;
            (line, claim)
        })
        .collect();
    rows.sort_by_key(|(line, _)| *line);
    let mut out = format!(
        "asserts in {path}: {} - proved 0, refuted 0, at run time {}\n",
        rows.len(),
        rows.len()
    );
    for (line, claim) in rows {
        out.push_str(&format!(
            "  {path}:{line}  {}  run time: nothing proves it yet\n",
            claim.written
        ));
    }
    out
}

/// Whether an expression is a literal of a kind other than `kind` (`"bool"`
/// or `"text"`): the one answer a literal whose type is settled below gives
/// here.
fn is_a_literal_other_than(expr: &Expr, kind: &str) -> bool {
    match expr {
        Expr::LitBool(_) => kind != "bool",
        Expr::LitStr { .. } | Expr::LitInterpolated(_) => kind != "text",
        Expr::LitInt(_) | Expr::LitFloat(_) | Expr::LitChar(_) | Expr::LitNull => true,
        _ => false,
    }
}

/// The prelude's claim ([ADR-245](../../docs/specification/adr/adr-245.md) D2).
pub const ASSERT: &str = "assert";

/// Why a call cannot stand in an `assert`'s condition, or nothing where it can
/// ([ADR-245](../../docs/specification/adr/adr-245.md) D3): it may pause, may
/// fail, touches something or changes a value. A `touches` nobody wrote is not
/// an answer, and counts as touching.
fn not_pure_because(contract: &FnContract) -> Option<&'static str> {
    if !contract.sync.is_sync() {
        return Some("can pause");
    }
    if !contract.throws.is_empty() {
        return Some("can fail");
    }
    if contract.mutates {
        return Some("changes what it is called on");
    }
    if !contract.touches_known || !contract.touches.is_empty() {
        return Some("touches something outside the program's values");
    }
    if contract.touches_a_lock.holds() {
        return Some("takes a lock");
    }
    None
}

/// **An expression as it was written**, near enough, rewritten from the tree:
/// the parser keeps no text, and an `assert`'s failure names its condition
/// ([ADR-245](../../docs/specification/adr/adr-245.md) D2). What this does not
/// spell out it writes as `…`, which a reader recognises and nothing parses.
pub fn written(parsed: &Parsed, expr: &Expr) -> String {
    let w = |e: &Expr| written(parsed, e);
    let list = |items: &[Expr]| {
        items
            .iter()
            .map(|e| written(parsed, e))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let options = |config: &[ast::ConfigArg]| {
        config
            .iter()
            .map(|c| format!("{}: {}", parsed.text(c.name), written(parsed, &c.value)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let call = |args: &[Expr], config: &[ast::ConfigArg]| match config.is_empty() {
        true => format!("({})", list(args)),
        false if args.is_empty() => format!("(; {})", options(config)),
        false => format!("({}; {})", list(args), options(config)),
    };
    // A side that binds looser than the operator it stands under is written
    // in the parentheses it needed.
    let operand = |e: &Expr| match e {
        Expr::Binary { .. } | Expr::Coalesce { .. } | Expr::Cast { .. } => format!("({})", w(e)),
        _ => w(e),
    };
    match expr {
        Expr::LitInt(v) => v.to_string(),
        Expr::LitFloat(v) => v.clone(),
        Expr::LitBool(b) => b.to_string(),
        Expr::LitChar(c) => format!("'{c}'"),
        Expr::LitStr { text, .. } => format!("\"{text}\""),
        Expr::LitInterpolated(text) => format!("f\"{text}\""),
        Expr::LitNull => "null".to_string(),
        Expr::Variable(name) => parsed.text(*name).to_string(),
        Expr::Path(segments) => segments
            .iter()
            .map(|s| parsed.text(*s))
            .collect::<Vec<_>>()
            .join("::"),
        Expr::Field { base, name } => format!("{}.{}", operand(base), parsed.text(*name)),
        Expr::SafeField { base, name } => format!("{}?.{}", operand(base), parsed.text(*name)),
        Expr::Index { base, index } => format!("{}[{}]", operand(base), w(index)),
        Expr::Call { func, args, config } => format!("{}{}", w(func), call(args, config)),
        Expr::MethodCall {
            receiver,
            method,
            args,
            config,
        } => format!(
            "{}.{}{}",
            operand(receiver),
            parsed.text(*method),
            call(args, config)
        ),
        Expr::SafeMethod {
            receiver,
            method,
            args,
            config,
        } => format!(
            "{}?.{}{}",
            operand(receiver),
            parsed.text(*method),
            call(args, config)
        ),
        Expr::Unary { op, expr } => {
            let op = match op {
                ast::UnaryOp::Neg => "-",
                ast::UnaryOp::Not => "!",
                ast::UnaryOp::Ref => "&",
            };
            format!("{op}{}", operand(expr))
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            // Parentheses where the tree needs them and nowhere else, so the
            // claim reads as it was written: a side that binds looser than
            // this operator, or as loosely on the right.
            let binds = |op: &BinaryOp| match op {
                BinaryOp::Or => 1,
                BinaryOp::And => 2,
                BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge => 3,
                BinaryOp::Add | BinaryOp::Sub => 4,
                BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => 5,
            };
            let here = binds(op);
            let side = |e: &Expr, right: bool| match e {
                Expr::Binary { op: inner, .. }
                    if binds(inner) < here || (right && binds(inner) == here) =>
                {
                    format!("({})", w(e))
                }
                Expr::Binary { .. } => w(e),
                _ => operand(e),
            };
            let (lhs, rhs) = (side(lhs, false), side(rhs, true));
            let op = match op {
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
            };
            format!("{lhs} {op} {rhs}")
        }
        Expr::Coalesce { value, fallback } => {
            format!("{} ?? {}", operand(value), operand(fallback))
        }
        Expr::Tuple(items) => format!("({})", list(items)),
        Expr::ListLit { items, .. } => format!("[{}]", list(items)),
        Expr::Range {
            start,
            end,
            inclusive,
        } => format!(
            "{}{}{}",
            operand(start),
            if *inclusive { ".." } else { "..<" },
            operand(end)
        ),
        _ => "…".to_string(),
    }
}

fn is_number(name: &str) -> bool {
    matches!(
        name,
        "i8" | "i16"
            | "i32"
            | "i64"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "usize"
            | "f32"
            | "f64"
    )
}

/// A float, written as a literal `rustc` reads — or nothing, where it cannot be.
///
/// **`{:?}` and not `{}`**, because Rust's `Debug` for a float is the shortest
/// text that reads back as the same bits, and `Display` is not: `1.0` prints as
/// `1` under the second, and `const X: f64 = 1;` is not Rust.
///
/// **A value that is not finite has no literal in either language.** An
/// infinity and a `NaN` reach a program through a name — `f64::INFINITY` — and
/// writing one into a `const` would put a name in the generated file that the
/// `.nika` line never mentioned. So it is `None` here and `NK1127` at the
/// caller, which is the honest answer: this is a thing the crossing does not
/// do.
fn float_literal(value: f64) -> Option<String> {
    value.is_finite().then(|| format!("{value:?}"))
}

/// **`std` modules a page or a record names and this compiler does not describe
/// yet** ([Part III C.4](../../docs/specification/30-nikaia-tooling.md)).
///
/// `std.contracts` is the list of what `std` *offers*, and it is the right list
/// for every other question asked of `std`. It is the wrong one for a **refusal**:
/// a module the specification promises and the compiler has not built is a
/// correct program refused. `use std::db` is how
/// [ADR-143](../../docs/specification/adr/adr-143.md)'s driver is reached, and
/// the day it exists nothing about that line changes.
///
/// So the refusal stands on the join of two lists, and this half is **written
/// down rather than derived**, because there is nothing to derive it from: each
/// name is one a page or a record writes after `use std::`, and a name on
/// neither list is one nobody has written down anywhere. Where each comes from,
/// in the order they appear below: `use std::backend::x86`
/// ([ADR-007](../../docs/specification/adr/adr-007.md), Part III 16); the build
/// script's own API (Part III 13.4);
/// [ADR-143](../../docs/specification/adr/adr-143.md)'s driver protocol; Part III
/// 17.1's *other key modules*; Part I 2.6's panic hook, `panic::on_panic`
/// ([ADR-141](../../docs/specification/adr/adr-141.md)); Part III 17.2's table,
/// which says what a target withholds; Part II 12.7's `task::scope`; and that
/// table again.
///
/// **The provenance is here and not beside each name** because `cargo fmt`
/// reflows a trailing comment onto the line above it, and a list where every
/// reason has slid one entry along is worse than one with no reasons in it.
const PROMISED: &[&str] = &[
    "backend", "build", "db", "json", "panic", "process", "task", "thread",
];

/// **Prefixes `std`'s ledger keys that are not modules a program imports.**
///
/// The ledger files a method under the thing it is called on, so `str::len`,
/// `i64::to_string` and `list::ListExt::map` sit beside `fs::read` and look the
/// same from the outside. They are not the same: a primitive and a trait are
/// reached without a line (Part I 1.3, 2.2), and `use std::str` is as wrong as
/// `use std::nosuchthing`.
///
/// **Subtracted rather than listed**, which is the direction that needs no
/// upkeep: a module `std` gains is a module a program may import the day it
/// lands, and what this file has to know is only which prefixes are *not* one.
/// A derivation was tried and does not hold — a module's function has a named
/// first parameter where a method has a receiver, except that every entry of
/// `collections` is a method on `HashMap` and `collections` is imported by name.
const NOT_A_MODULE: &[&str] = &["f64", "i32", "i64", "list", "str"];

/// **A module of `nikaia-std`'s crate that is deliberately not part of `std`.**
///
/// One entry, and it earns its own list because it earns its own sentence:
/// `crates/nikaia-std/src/tools/` holds
/// [ADR-196](../../docs/specification/adr/adr-196.md)'s Rust-signature grammar,
/// which the *compiler* calls and a program may not. Left to the list above it
/// would be told *nobody has written that down*, which is false and sends the
/// reader looking for a typo.
const NOT_STD: &[(&str, &str)] = &[(
    "tools",
    "`tools` is the toolchain's own, not `std`'s: it holds the compiler's \
     Rust-signature grammar (ADR-196 D2), which `nikaia describe` calls and a \
     program cannot reach",
)];

/// The closest field name, when one is close enough to be worth suggesting.
///
/// The distance is `std`'s, written in Nikaia (`tools/spelling.nika`, 0.0.238):
/// an edit is a character put in, taken out or changed, and two neighbours
/// that swapped count as one.
fn nearest<'n>(name: &str, among: &[&'n str]) -> Option<&'n str> {
    among
        .iter()
        .map(|candidate| {
            let d = nikaia_std::tools::spelling::distance(name, candidate);
            (usize::try_from(d).unwrap_or(usize::MAX), *candidate)
        })
        .filter(|(d, _)| *d * 3 <= name.len().max(1))
        .min_by_key(|(d, _)| *d)
        .map(|(_, candidate)| candidate)
}

/// `a` or `an` in front of a word, which may be spelled in backticks.
///
/// Small and here because the alternative is worse: the three boundaries a
/// jump may not cross are named in one place, and carrying the article beside
/// each of them means the next one is added in two places or in one.
fn an(what: &str) -> String {
    let first = what.chars().find(|c| c.is_alphanumeric()).unwrap_or(' ');
    match first.to_ascii_lowercase() {
        'a' | 'e' | 'i' | 'o' | 'u' => format!("an {what}"),
        _ => format!("a {what}"),
    }
}

/// **The two forms of a root**, where that is the argument a call left out
/// ([ADR-108](../../docs/specification/adr/adr-108.md) D1: *a call that leaves
/// it out is `NK1101`, and the message names the two forms of D2*).
///
/// A sentence about one `std` type, written here because the ledger has no
/// column for an `enum`'s variants: what `it takes `root: ref Root`` leaves a
/// reader with is a type name and a question. The two lines beside `PROMISED`
/// and `NOT_A_MODULE` in this file are there for the same reason — a thing the
/// compiler has to know and the ledger does not carry.
///
/// **Empty for every other missing argument**, which is what keeps this from
/// being a habit: the parameter has to be named `root` *and* typed `Root`.
fn a_root_is_one_of_two(wanted: &[(String, Ty)]) -> String {
    let is_a_root = wanted
        .iter()
        .any(|(name, ty)| name == "root" && ty.text().trim_start_matches("ref ") == "Root");
    match is_a_root {
        false => String::new(),
        true => ". The root is `fs::Root::Dir(store)`, under which the name is resolved and \
                 may not leave, or `fs::Root::Anywhere`, which performs no check and says so \
                 in the word a review looks for"
            .to_string(),
    }
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

fn list(names: &[&str]) -> String {
    match names {
        [] => "no fields".to_string(),
        [one] => format!("`{one}`"),
        [rest @ .., last] => format!(
            "{} and `{last}`",
            rest.iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// What the receiver's actual type binds this signature's variables to.
///
/// The receiver is the signature's first parameter where there is one, so this
/// is one `bind` against one pattern - the narrowness is ADR-031's decision
/// rather than a gap. A signature with no variables produces an empty map and
/// every substitution below is the identity.
fn bindings(contract: &FnContract, receiver: &Ty) -> BTreeMap<String, Ty> {
    let mut bound = BTreeMap::new();
    let Some(signature) = &contract.signature else {
        return bound;
    };
    if let Some((name, pattern)) = signature.params.first()
        && name == "self"
    {
        ty::bind(pattern, receiver, &mut bound);
    }
    bound
}

/// What the **arguments** bind this signature's variables to.
///
/// ADR-031 bound from the receiver only, and said so on purpose: binding from
/// arguments is where a signature language grows into a unification algorithm.
/// [ADR-074](../../../docs/specification/adr/adr-074.md) D2 takes that step and
/// keeps it one step. This is the same `ty::bind` against one pattern per
/// argument - a structural walk, no queue, no fixpoint, no occurs check -
/// because a type parameter is **written** rather than inferred, so there is
/// never a variable on the right-hand side to solve for.
///
/// A mismatch binds nothing, exactly as the receiver's does: the question is
/// what a caller can *tell* the signature, and `substitute` turns what it could
/// not tell into `?`.
fn from_arguments(contract: &FnContract, found: &[Ty]) -> BTreeMap<String, Ty> {
    let mut bound = BTreeMap::new();
    let Some(signature) = &contract.signature else {
        return bound;
    };
    for ((_, pattern), actual) in signature.arguments().iter().zip(found) {
        ty::bind(pattern, actual, &mut bound);
    }
    bound
}

/// The types a callee's parameters expect, as a call site sees them.
///
/// A method's receiver is the first parameter, so the arguments a *call* writes
/// are the ones after it - `Signature::arguments` already draws that line.
fn expected_arguments(contract: &FnContract) -> Vec<Ty> {
    contract
        .signature
        .as_ref()
        .map(|signature| {
            signature
                .arguments()
                .iter()
                .map(|(_, ty)| ty.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether an `Array[T, N]` is anywhere inside a type.
///
/// What it guards is the walk **through** a container in
/// [`Checker::array_literal`]: a `Vec[T]` is opened only where an array is what
/// it holds, so a list of anything else reaches that rule and leaves it
/// untouched ([ADR-152](../../docs/specification/adr/adr-152.md) D4).
fn holds_an_array(ty: &Ty) -> bool {
    match ty {
        Ty::Named { name, args, .. } => name == ty::ARRAY || args.iter().any(holds_an_array),
        Ty::Tuple(parts) => parts.iter().any(holds_an_array),
        Ty::Nullable(inner) => holds_an_array(inner),
        _ => false,
    }
}

/// Whether handing a value of this type out takes nothing away.
///
/// Part I 2.2's numbers, `bool` and `char`, and any **view**: all of them are
/// copied rather than moved in the language below, so a field of one may leave a
/// borrowed subject freely. Everything else - a `String`, a `Vec`, a struct this
/// program declares - is moved, which is what `NK1131` is about.
///
/// A short list on purpose, and the polarity is the usual one: a type not on it
/// **Whether two values of this type may be compared**
/// ([ADR-204](../../docs/specification/adr/adr-204.md) D1).
///
/// `==` on a declared type had no lowering at all: `rustc` answered *binary
/// operation `==` cannot be applied to type `P`* about a file nobody wrote, with
/// *consider annotating `P` with `#[derive(PartialEq)]`* as the help — [Part III
/// C.1 and C.2](../../docs/specification/30-nikaia-tooling.md) at once. So the
/// question has to be answerable here, and the answer decides both the derive
/// the emitter writes and the refusal this file raises.
///
/// **Three sources and one rule.** The language's own types answer from the list
/// below; a **declared** type answers from its parts, structurally, which is what
/// cannot be wrong; and a type whose parts are Rust answers from the ledger's
/// `compares` column, whose absence is *no* ([ADR-010](../../docs/specification/adr/adr-010.md)
/// D1).
///
/// **`Ty::Unknown` compares.** Nothing is known, so nothing is refused — [Part
/// III C.4](../../docs/specification/30-nikaia-tooling.md)'s direction, and the
/// same answer every other question here gives an absent claim.
struct Comparable<'a> {
    structs: &'a BTreeMap<String, Vec<FieldContract>>,
    enums: &'a BTreeMap<String, BTreeSet<String>>,
    /// What a declared `enum`'s variants hold ([`Checker::enum_payloads`]).
    payloads: &'a BTreeMap<String, Vec<Ty>>,
    own: &'a Ledger,
    library: &'a Ledger,
}

/// Which of the two questions [`Comparable`] is being asked.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Strength {
    /// `==` works: Rust's `PartialEq`.
    Compares,
    /// …and equality is an equivalence: Rust's `Eq`, which a float is not and
    /// which a type holding one cannot be.
    Totally,
}

impl Comparable<'_> {
    /// The primitives and the containers, which are the language's rather than a
    /// module's and so are a list here rather than a column anywhere.
    ///
    /// A container compares exactly when what it holds does, which is Rust's own
    /// rule for each of them and is why they are one arm.
    fn of(&self, ty: &Ty) -> bool {
        self.asking(ty, Strength::Compares, &mut BTreeSet::new())
    }

    /// The same question about **`Eq`**, which is not the same walk one step
    /// deeper: a `struct` holding a type that compares and is not an equivalence
    /// is the same, and the float may be several declarations away.
    fn totally(&self, ty: &Ty) -> bool {
        self.asking(ty, Strength::Totally, &mut BTreeSet::new())
    }

    /// `seen` is the types already being asked about, so a `struct` that holds
    /// itself through a `Vec` answers once rather than forever.
    fn asking(&self, ty: &Ty, how: Strength, seen: &mut BTreeSet<String>) -> bool {
        match ty {
            // An absent claim is not a refusal (C.4). A type variable is one:
            // a library's `$V` is substituted away at a call, and one that is
            // not is `Unknown` by the same rule.
            Ty::Unknown | Ty::Var { .. } | Ty::Count(_) => true,
            Ty::Tuple(parts) => parts.iter().all(|part| self.asking(part, how, seen)),
            Ty::Nullable(inner) => self.asking(inner, how, seen),
            // **A produced sequence is not a value this language holds**: it is
            // walked, and what it hands over is the item.
            Ty::Seq { .. } => false,
            // **What the C boundary lends is an address**, and an address is not
            // a value this language compares
            // ([ADR-147](../../docs/specification/adr/adr-147.md) D1).
            Ty::Pointed { .. } => false,
            Ty::Fn { .. } => false,
            Ty::Named { name, args, .. } => self.named(name, args, how, seen),
        }
    }

    fn named(&self, name: &str, args: &[Ty], how: Strength, seen: &mut BTreeSet<String>) -> bool {
        // **A view compares as what it views does**: `&str` is `String`'s
        // comparison and `&T` is `T`'s, which is Rust's rule too.
        let holds =
            |seen: &mut BTreeSet<String>| args.iter().all(|arg| self.asking(arg, how, seen));
        match name {
            // **A float compares and is not an equivalence**, because
            // `NaN != NaN`. It is the one primitive the two questions differ on,
            // and every type holding one differs with it however far down.
            "f64" => how == Strength::Compares,
            "i32" | "i64" | "u8" | "bool" | "char" | "String" | "str" | "Bytes" => true,
            // A container compares when what it holds does.
            "Vec"
            | "List"
            | ty::ARRAY
            | "Option"
            | "Set"
            | "collections::HashMap"
            | "collections::HashSet"
            | "collections::BTreeMap"
            | "collections::BTreeSet"
            | "HashMap"
            | "HashSet"
            | "BTreeMap"
            | "BTreeSet"
            | "Shared" => holds(seen),
            _ => {
                if !seen.insert(name.to_string()) {
                    // Already being asked about, one level up: a type is not
                    // *un*comparable for holding itself.
                    return true;
                }
                if let Some(fields) = self.structs.get(name) {
                    return fields.iter().all(|f| self.asking(&f.ty, how, seen));
                }
                if self.enums.contains_key(name) {
                    return self
                        .payloads(name)
                        .iter()
                        .all(|ty| self.asking(ty, how, &mut seen.clone()));
                }
                // **A type whose parts are Rust**: the column, whose absence is
                // no. And it answers the `==` question only — a library type is
                // never taken for an *equivalence*, because whether it is one is
                // a second claim nobody has had a use for. So a `struct` holding
                // one compares and is not a map key yet, which is the
                // under-approximation that cannot be wrong.
                how == Strength::Compares
                    && [self.own, self.library]
                        .into_iter()
                        .filter_map(|ledger| ledger.types.get(name))
                        .any(|contract| contract.compares)
            }
        }
    }

    /// Every type a declared `enum`'s variants hold, **or a library one's**.
    ///
    /// A declared `enum` answers from [`Checker::enum_payloads`]; one that
    /// arrived in a ledger answers from its `variants` column, which carries the
    /// payload types for the same reason.
    fn payloads(&self, name: &str) -> Vec<Ty> {
        if let Some(held) = self.payloads.get(name) {
            return held.clone();
        }
        [self.own, self.library]
            .into_iter()
            .filter_map(|ledger| ledger.types.get(name))
            .flat_map(|contract| {
                contract
                    .variants
                    .iter()
                    .flat_map(|variant| variant.holds.iter().map(|f| f.ty.clone()))
            })
            .collect()
    }
}

/// is one this says nothing about only when it is also unknown.
fn copies(ty: &Ty) -> bool {
    match ty {
        // `moves_away`'s other half, and ADR-152 D2's same sentence.
        Ty::Named { name, args, .. } if name == ty::ARRAY => args.iter().all(copies),
        Ty::Named { name, view, .. } => {
            *view
                || matches!(
                    name.as_str(),
                    "i32" | "i64" | "u8" | "f64" | "bool" | "char"
                )
        }
        _ => false,
    }
}

/// The declared types `ty` holds **inline** - itself, nullable or not, a
/// tuple's parts and an array's element - and nothing it holds through a view,
/// a function, a list or anything else ([`Checker::types_that_contain_themselves`]).
fn held_inline(parsed: &Parsed, ty: &ast::Type, out: &mut BTreeSet<String>) {
    if ty.is_view || ty.code.is_some() {
        return;
    }
    if ty.is_tuple {
        for part in &ty.generics {
            held_inline(parsed, part, out);
        }
        return;
    }
    let name = parsed.text(ty.name);
    if name == "Array" {
        if let Some(element) = ty.generics.first() {
            held_inline(parsed, element, out);
        }
        return;
    }
    if ty.generics.is_empty() {
        out.insert(name.to_string());
    }
}

/// Whether `from` holds `to` inline, through any number of types between: a
/// depth-first walk over what each type holds ([`Checker::types_that_contain_themselves`]).
fn reaches(from: &str, to: &str, holds: &BTreeMap<String, BTreeSet<String>>) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from.to_string()];
    while let Some(at) = stack.pop() {
        for next in holds.get(&at).into_iter().flatten() {
            if next == to {
                return true;
            }
            if seen.insert(next.clone()) {
                stack.push(next.clone());
            }
        }
    }
    false
}

/// How a member that may be boxed is named, the same in the checker and the
/// emitter ([ADR-246](../../docs/specification/adr/adr-246.md)): a struct's
/// field by its name, a variant's part as `Variant#0`, and a variant's named
/// field as `Variant.name`.
pub fn boxed_member(owner: &str, at: Option<usize>, field: Option<&str>) -> String {
    match (at, field) {
        (Some(at), _) => format!("{owner}#{at}"),
        (None, Some(field)) => format!("{owner}.{field}"),
        (None, None) => owner.to_string(),
    }
}

/// **A `std` lookup whose key the language below takes by reference**
/// (0.0.235): the map or set is asked about the key and keeps nothing, so the
/// key is lent ([`Checker::a_lookup_key_lent`]) and not handed over.
fn is_a_lookup(key: &str) -> bool {
    const LOOKUPS: &[&str] = &[
        "HashMap::get",
        "HashMap::contains_key",
        "HashSet::contains",
        "HashSet::remove",
        "BTreeMap::get",
        "BTreeMap::contains_key",
        "BTreeSet::contains",
    ];
    LOOKUPS.contains(&key.strip_prefix("collections::").unwrap_or(key))
}

/// A list: what `[…]` makes, and what `Vec()` builds.
fn is_a_list(ty: &Ty) -> bool {
    matches!(ty.unseen(), Ty::Named { name, .. } if matches!(name.as_str(), "Vec" | "List" | "Array"))
}

/// A list, a map or a set - the types Part I gives no operator.
fn is_a_collection(ty: &Ty) -> bool {
    is_a_list(ty)
        || matches!(ty.unseen(), Ty::Named { name, .. }
        if matches!(
            name.rsplit("::").next().unwrap_or(name.as_str()),
            "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "TrustedMap" | "TrustedSet"
        ))
}

/// Whether a type is one this language calls text.
///
/// `String` and `&str`, and nothing else. A `T?` is deliberately **not** text:
/// `maybe + "x"` is a member reached off a nullable, which Part I 2.3 answers
/// and this must not quietly paper over.
fn is_text(ty: &Ty) -> bool {
    matches!(ty, Ty::Named { name, args, .. } if args.is_empty() && (name == "String" || name == "str"))
}

/// Every `{…}` group in a string, by the text between the braces.
///
/// An escape is skipped whole, so the `{` of a `\u{0041}` does not start one -
/// the same rule the emitter's `interpolation` follows, and for the same
/// reason: a string keeps its escapes as written, so a scanner that does not
/// know that reads `"\u{0041}"` as a hole named `0041`.
fn brace_groups(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if chars.next() == Some('u') && chars.peek() == Some(&'{') {
                    for c in chars.by_ref() {
                        if c == '}' {
                            break;
                        }
                    }
                }
            }
            '{' => {
                let mut group = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        found.push(group);
                        break;
                    }
                    group.push(c);
                }
            }
            _ => {}
        }
    }
    found.retain(|g| !g.trim().is_empty());
    found
}

/// A pause the checker was told of, in the words its refusals use: what
/// pauses, and the way out of a walk and of a door.
struct Pause {
    what: String,
    before_walk: String,
    before_door: String,
}
