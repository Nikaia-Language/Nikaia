//! Part I 4.7's `trait` declaration, and the bound it makes possible
//! ([ADR-078](../../../docs/specification/adr/adr-078.md)).
//!
//! **The section's own example did not parse.** `trait Summarize { … }` was a
//! parse error at the keyword, so a trait could be implemented and not declared
//! — and that is what [ADR-074](../../../docs/specification/adr/adr-074.md) §4
//! named as the prerequisite for a bound: `[T: Summarize]` has to name
//! something. Until it did, a generic body could move and pass its value and
//! nothing else, which `NK1126` said to the user in so many words.

mod common;

use nikaia::check;
use nikaia::contracts::{Ledger, STD, Sync};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

/// **`check_program` and not `check`**, because the trait rules read the
/// ledger's *finished* `sync` column and only the program-level entry point runs
/// after `sync::infer` has filled it in
/// ([ADR-080](../../../docs/specification/adr/adr-080.md) §2). A test on the
/// inner entry point would have found `NK1129` silent and concluded it was not
/// built.
fn findings(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new()).findings
}

fn lowered(purpose: &str, source: &str) -> String {
    let found = findings(source);
    assert!(
        found.is_empty(),
        "{purpose} is a correct program and the checker says otherwise: {found:#?}"
    );
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// Compile the lowering as a binary, run it, hand back what it printed.
fn ran(purpose: &str, source: &str) -> String {
    let rust = lowered(purpose, source);
    let dir = common::scratch_dir(purpose);
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "the lowering of {purpose} does not compile:\n{}\n--- the Rust ---\n{rust}",
        String::from_utf8_lossy(&out.stderr),
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("the program runs");
    let printed = String::from_utf8_lossy(&ran.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    printed
}

/// Part I 4.7's own program, declared and bound and run.
///
/// The body interpolates where the page writes `"User: " + self.username`, and
/// that is **not** a tidy-up: `&str + String` is accepted here and refused by
/// the language below (`E0369`), which `open-work.md` now carries with its
/// reproduction. Found by writing this fixture, which is the third time this
/// session that running one of the specification's own programs turned one up.
const SUMMARIZE: &str = r#"
trait Summarize {
    fn summary(ref self) -> String
}

struct User {
    name: String,
}

impl Summarize for User {
    fn summary(ref self) -> String {
        return f"User: {self.name}"
    }
}

fn shout[T: Summarize](x: T) -> String {
    return x.summary()
}

fn main() {
    let u = User { name: "Ada".to_string() }
    println(shout(u))
}
"#;

/// D1 and D3 together: the declaration parses, the bound resolves the call, and
/// the program runs.
#[test]
fn a_trait_is_declared_bound_and_run() {
    assert_eq!(ran("a trait and a bound", SUMMARIZE).trim(), "User: Ada");
}

/// D1: the method is a **signature**, so the lowering writes a `;` where an
/// `impl`'s writes a body.
///
/// **And the form is the return-position one**
/// ([ADR-109](../../../docs/specification/adr/adr-109.md) D3), because
/// `Summarize` does not say `sync` and so may pause. The sugar `async fn` is
/// not written: `async_fn_in_trait` warns on a public trait whose future
/// carries no `Send` bound, and a warning about the generated file is
/// [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class.
#[test]
fn a_trait_method_is_a_signature() {
    let rust = lowered("a trait declaration", SUMMARIZE);
    assert!(
        rust.contains("trait Summarize {"),
        "the trait reaches the generated file:\n{rust}"
    );
    assert!(
        rust.contains("fn summary(&self) -> impl std::future::Future<Output = String>"),
        "its method is a signature and not a body, in the return-position form:\n{rust}"
    );
    assert!(
        !rust.contains("async fn summary(&self) -> String;"),
        "and the sugar `async fn` is not written in the trait:\n{rust}"
    );
}

/// **A declaration that says `sync` is a plain `fn` below**, which is the other
/// half of D3 and the one every trait had before ADR-109.
#[test]
fn a_sync_declaration_is_a_plain_signature() {
    let rust = lowered(
        "a sync trait",
        r#"
trait Named {
    fn name(ref self) -> String sync
}

struct User { username: String }

impl Named for User {
    fn name(ref self) -> String sync { return self.username.clone() }
}

fn main() { println(User { username: "Ada".to_string() }.name()) }
"#,
    );
    assert!(
        rust.contains("fn name(&self) -> String;"),
        "no future where nothing may pause:\n{rust}"
    );
}

/// D2: the bound is written where Rust writes one.
#[test]
fn a_bound_reaches_the_generated_file() {
    let rust = lowered("a bound", SUMMARIZE);
    assert!(
        rust.contains("fn shout<T: Summarize>(x: T)"),
        "the parameter carries its bound:\n{rust}"
    );
}

/// Several bounds, `[T: A + B]`, which is why the AST holds a list.
#[test]
fn several_bounds_are_written_with_a_plus() {
    let rust = lowered(
        "two bounds",
        r#"
trait Named {
    fn name_of(ref self) -> String
}

trait Aged {
    fn age_of(ref self) -> i64
}

struct User {
    name: String,
    age: i64,
}

impl Named for User {
    fn name_of(ref self) -> String {
        // `.clone()` and not `return self.name`, which `NK1131` refuses
        // ([ADR-083](../../../docs/specification/adr/adr-083.md)) - and it
        // caught this fixture, which had been writing a program that never
        // compiled. A test that rests on a shape the language does not have is
        // a test that measures the wrong thing.
        return self.name.clone()
    }
}

impl Aged for User {
    fn age_of(ref self) -> i64 {
        return self.age
    }
}

fn both[T: Named + Aged](x: T) -> String {
    return x.name_of()
}

fn main() {
    let u = User { name: "Ada".to_string(), age: 36 }
    println(both(u))
}
"#,
    );
    assert!(
        rust.contains("fn both<T: Named + Aged>(x: T)"),
        "both bounds, in the order written:\n{rust}"
    );
}

/// D3: the bound answers the **whole** call and not only whether it exists. An
/// argument the trait's signature does not take is `NK1101`, exactly as it would
/// be on a receiver whose type was written down.
#[test]
fn a_bound_answers_the_arity_too() {
    let found = findings(
        r#"
trait Summarize {
    fn summary(ref self) -> String
}

fn shout[T: Summarize](x: T) -> String {
    return x.summary(1)
}

fn main() {
    println("x")
}
"#,
    );
    assert!(
        found.iter().any(|f| f.code == "NK1101"),
        "the declaration says how many arguments it takes: {found:#?}"
    );
}

/// And the result type, which is what makes a bound worth more than a permission
/// slip: `summary` hands back a `String`, so a `let` that says `i64` is refused.
#[test]
fn a_bound_answers_the_result_type() {
    let found = findings(
        r#"
trait Summarize {
    fn summary(ref self) -> String
}

fn shout[T: Summarize](x: T) -> i64 {
    return x.summary()
}

fn main() {
    println("x")
}
"#,
    );
    assert!(
        found.iter().any(|f| f.code == "NK1104"),
        "a `String` is not an `i64`, whatever the caller picks: {found:#?}"
    );
}

/// `NK1126` is unchanged where the bound does **not** declare the member, which
/// is the half that keeps the refusal honest: a bound is not a blanket licence.
#[test]
fn a_member_no_bound_declares_is_still_refused() {
    let found = findings(
        r#"
trait Summarize {
    fn summary(ref self) -> String
}

fn shout[T: Summarize](x: T) -> String {
    return x.to_uppercase()
}

fn main() {
    println("x")
}
"#,
    );
    assert!(
        found.iter().any(|f| f.code == "NK1126"),
        "`Summarize` says nothing about `to_uppercase`: {found:#?}"
    );
}

/// **A trait method's `sync` is the declaration's own word**
/// ([ADR-109](../../../docs/specification/adr/adr-109.md) D1): it reads like a
/// function type, so without the word it **may pause**.
///
/// **It used to be asserted whatever the declaration said**
/// ([ADR-078](../../../docs/specification/adr/adr-078.md) D4), and had to be: a
/// plain `fn` was the only thing the emitter could write in a trait, so a
/// pausing declaration had no lowering and `No` would have made every call
/// through a bound an `.await` on a `String`. ADR-109 D3 takes the cause away
/// with the return-position form.
#[test]
fn a_trait_method_carries_the_word_it_was_written_with() {
    let parsed = parse_to_ast(SUMMARIZE).expect("the source parses");
    let own = Ledger::infer(&parsed);
    assert_eq!(
        own.functions
            .get("Summarize::summary")
            .expect("the declaration is in the ledger")
            .sync,
        Sync::No,
        "`Summarize` does not say `sync`, so its method may pause",
    );
    assert!(
        !own.functions
            .get("shout")
            .expect("the bounded function is in the ledger")
            .sync
            .is_sync(),
        "and a body that calls it through a bound pauses with it",
    );

    // The word, where it is written.
    let with = parse_to_ast(
        "trait Named {\n\
         \x20   fn name(ref self) -> String sync\n\
         }\n",
    )
    .expect("the source parses");
    assert_eq!(
        Ledger::infer(&with).functions["Named::name"].sync,
        Sync::Asserted,
    );
}

/// `trait` is a reserved word now, which it was not: it was found as a *name*
/// by [ADR-076](../../../docs/specification/adr/adr-076.md)'s sweep.
#[test]
fn trait_is_a_reserved_word() {
    assert!(
        nikaia::parser::RESERVED_WORDS.contains(&"trait"),
        "the construct exists, so the word is not a name"
    );
    assert!(
        parse_to_ast("fn main() {\n    let trait = 3\n}\n").is_err(),
        "`let trait = 3` does not parse"
    );
}

/// `NK1129` ([ADR-109](../../../docs/specification/adr/adr-109.md) D2): the
/// implementation pauses and the declaration **says `sync`**.
///
/// **It used to refuse every pausing implementation**
/// ([ADR-080](../../../docs/specification/adr/adr-080.md) D1), because a trait
/// method had no way to say it may pause: the trait lowered to `fn load(&self)
/// -> Result<…>;` and the `impl` to `async fn load(&self) -> Result<…>`, and
/// the language below answered `E0053` about a file nobody wrote. ADR-109 D3's
/// return-position form takes that away, so what is left is a **comparison** —
/// which is `NK2202` asked of somebody else's signature.
#[test]
fn an_implementation_that_pauses_is_refused_where_the_trait_says_sync() {
    let found = findings(
        r#"
use std::fs

trait Loader {
    fn load(ref self) -> String sync throws
}

struct File {
    path: String,
}

impl Loader for File {
    fn load(ref self) -> String throws {
        return fs::read_to_string(self.path.clone(), fs::Root::Anywhere)
    }
}

fn main() {
    println("x")
}
"#,
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1129")
        .unwrap_or_else(|| panic!("a pausing implementation is refused: {found:#?}"));
    assert!(
        refusal.message.contains("File::load") && refusal.message.contains("Loader"),
        "it names the method and the trait: {}",
        refusal.message
    );
}

/// And a body that does **not** pause is not refused, which is every trait
/// anybody has written so far.
#[test]
fn an_implementation_that_cannot_pause_is_left_alone() {
    assert!(
        findings(SUMMARIZE).is_empty(),
        "the ordinary case stays ordinary"
    );
}

/// `NK1130`, the direction that was `rustc`'s `E0046`: the `impl` leaves out a
/// method the trait declares.
#[test]
fn an_impl_that_leaves_a_method_out_is_refused() {
    let found = findings(
        r#"
trait Summarize {
    fn summary(ref self) -> String
    fn title(ref self) -> String
}

struct User {
    name: String,
}

impl Summarize for User {
    fn summary(ref self) -> String {
        return f"User: {self.name}"
    }
}

fn main() {
    println("x")
}
"#,
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1130")
        .unwrap_or_else(|| panic!("an incomplete impl is refused: {found:#?}"));
    assert!(
        refusal.message.contains("title"),
        "it names what is missing: {}",
        refusal.message
    );
}

/// The other direction, which was `E0407`: a method the trait does not declare.
#[test]
fn a_method_the_trait_does_not_declare_is_refused() {
    let found = findings(
        r#"
trait Summarize {
    fn summary(ref self) -> String
}

struct User {
    name: String,
}

impl Summarize for User {
    fn summary(ref self) -> String {
        return f"User: {self.name}"
    }

    fn shout(ref self) -> String {
        return f"USER"
    }
}

fn main() {
    println("x")
}
"#,
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1130")
        .unwrap_or_else(|| panic!("a method outside the trait is refused: {found:#?}"));
    assert!(
        refusal.message.contains("shout"),
        "it names the method: {}",
        refusal.message
    );
    assert!(
        refusal
            .help
            .as_deref()
            .is_some_and(|h| h.contains("impl User")),
        "and says where it belongs: {:?}",
        refusal.help
    );
}

/// **A trait this unit does not declare is not checked against**, and that is
/// the rule rather than a gap: `impl Error for ConfigError` names the one trait
/// the compiler reads rather than one a `.nika` file wrote
/// ([ADR-023](../../../docs/specification/adr/adr-023.md) D3), and a trait a
/// package publishes cannot be reached at all yet. Silence is the only correct
/// answer about a declaration that is not here.
#[test]
fn an_impl_of_a_trait_declared_elsewhere_is_left_alone() {
    let found = findings(
        r#"
struct ConfigError {
    path: String,
}

impl Error for ConfigError {
    fn message(ref self) -> String {
        return f"no config at {self.path}"
    }
}

fn main() {
    println("x")
}
"#,
    );
    assert!(
        !found.iter().any(|f| f.code == "NK1130"),
        "nothing here declares `Error`, so nothing here can check against it: {found:#?}"
    );
}

/// **`NK1140`: the implementation can fail and the declaration has no
/// `throws`** ([ADR-109](../../../docs/specification/adr/adr-109.md) D2) —
/// `NK1129`'s twin one column over, and the same `NK2202` asked of somebody
/// else's signature.
#[test]
fn an_implementation_that_fails_is_refused_where_the_trait_says_it_cannot() {
    let found = findings(
        r#"
use std::fs

trait Loader {
    fn load(ref self) -> String
}

struct File {
    path: String,
}

impl Loader for File {
    fn load(ref self) -> String throws {
        return fs::read_to_string(self.path.clone(), fs::Root::Anywhere)
    }
}

fn main() {
    println("x")
}
"#,
    );
    assert!(
        found.iter().any(|f| f.code == "NK1140"),
        "a failing implementation under a declaration without `throws`: {found:#?}"
    );
}

/// **The other direction fits and says nothing.** A declaration is the wider
/// claim: a body that never pauses under one that may, or one that cannot fail
/// under `throws`, is correct ([ADR-109](../../../docs/specification/adr/adr-109.md)
/// D2).
///
/// It is the half that says these two are comparisons rather than a demand that
/// the words match.
#[test]
fn a_body_that_does_less_than_the_declaration_allows_is_a_program() {
    let found = findings(
        r#"
trait Loader {
    fn load(ref self) -> String throws
}

struct Fixed {
    text: String,
}

impl Loader for Fixed {
    fn load(ref self) -> String sync {
        return self.text.clone()
    }
}

fn main() {
    println("x")
}
"#,
    );
    assert!(
        !found
            .iter()
            .any(|f| f.code == "NK1129" || f.code == "NK1140"),
        "a narrower body honours a wider declaration: {found:#?}"
    );
}

/// **`docs/language-review.md` §1.3's probe**, which is what the work entry
/// named as the evidence: a trait over a file read, refused as `NK1129` before
/// ADR-109 and a program now — compiled and run.
#[test]
fn a_trait_over_a_file_read_is_a_program() {
    let printed = ran(
        "a pausing trait",
        r#"
trait Source {
    fn load(ref self) -> String throws
    fn name(ref self) -> String sync
}

struct Fixed {
    text: String,
}

impl Source for Fixed {
    fn load(ref self) -> String throws {
        return self.text.clone()
    }
    fn name(ref self) -> String sync { return "fixed".to_string() }
}

fn main() throws {
    let s = Fixed { text: "loaded".to_string() }
    let text = s.load() catch { "".to_string() }
    println(f"{s.name()}: {text}")
}
"#,
    );
    assert_eq!(printed.trim(), "fixed: loaded");
}

/// **`+ Send` follows the executor, not the type**
/// ([ADR-109](../../../docs/specification/adr/adr-109.md) D3).
///
/// At `user_parallelism = yes` a task crosses threads and a spawn needs a
/// `Send` future; at `no` nothing crosses, the counts are plain
/// ([ADR-037](../../../docs/specification/adr/adr-037.md) D7) and a `Send`
/// demand would refuse them. It is a requirement of the executor this program
/// is built for — not a claim about a type, which is `contracts::send`'s and is
/// the same at both settings.
#[test]
fn the_send_bound_follows_the_setting() {
    use nikaia::emit::{Build, UserParallelism, emit_program};

    let parsed = parse_to_ast(SUMMARIZE).expect("the source parses");
    for (setting, expected) in [(UserParallelism::Yes, true), (UserParallelism::No, false)] {
        let rust = emit_program(
            &parsed,
            Build {
                user_parallelism: setting,
                ..Build::default()
            },
        )
        .expect("it lowers")
        .rust;
        assert_eq!(
            rust.contains("Output = String> + Send"),
            expected,
            "at {setting:?}:\n{rust}"
        );
    }
}

/// **The floor is written where Cargo reads it**
/// ([ADR-109](../../../docs/specification/adr/adr-109.md) D4), from one
/// constant in the emitter.
///
/// The floor is measured ([ADR-219](../../../docs/specification/adr/adr-219.md)).
/// The build compares `rustc --version` against it before handing anything to
/// Cargo, because Cargo's own *package requires rustc 1.88 or newer* names a
/// package the author never wrote — [Part III
/// C.1](../../../docs/specification/30-nikaia-tooling.md)'s class.
///
/// **A version this cannot read is not a refusal**: a `rustc --version` that
/// fails to run or prints something unparsable says nothing, because refusing
/// on a reading failure would refuse a correct toolchain (C.4).
#[test]
fn the_rust_floor_is_one_constant_and_this_toolchain_clears_it() {
    assert_eq!(nikaia::emit::RUST_FLOOR, "1.88");
    assert!(
        orchestrator::project::toolchain_is_new_enough(nikaia::emit::RUST_FLOOR).is_ok(),
        "the toolchain the tests run on clears the floor the emitter writes"
    );
    // A floor nothing could satisfy is refused, which is the half that says the
    // comparison happens at all.
    assert!(orchestrator::project::toolchain_is_new_enough("999.0").is_err());
    // And one this cannot read says nothing.
    assert!(orchestrator::project::toolchain_is_new_enough("stable").is_ok());
}

/// **`NK1164`: the bound was declared and not enforced**
/// ([ADR-174](../../../docs/specification/adr/adr-174.md) D2).
///
/// `[T: Speaks]` put `say` in reach of the body from the day ADR-078 landed —
/// that half is `NK1126` and is tested above. The other half, *may this type
/// stand here*, was asked by nobody: the call was accepted and `rustc` answered
/// *the trait bound `Rock: Speaks` is not satisfied* about a file nobody wrote.
#[test]
fn a_type_that_implements_nothing_does_not_meet_a_bound() {
    let found = findings(
        r#"
trait Speaks {
    fn say(ref self) -> String sync
}

struct Dog { name: String }

impl Speaks for Dog {
    fn say(ref self) -> String sync {
        return "woof".clone()
    }
}

struct Rock { weight: i64 }

fn tell[T: Speaks](x: T) -> String sync {
    return x.say()
}

fn main() {
    let r = Rock { weight: 3 }
    println(tell(r))
}
"#,
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1164")
        .unwrap_or_else(|| panic!("NK1164: {found:#?}"));
    assert!(
        refusal.message.contains("`Speaks`") && refusal.message.contains("`Rock`"),
        "it names the bound and the type that does not meet it: {}",
        refusal.message
    );
    assert!(
        refusal
            .help
            .as_deref()
            .is_some_and(|h| h.contains("impl Speaks for Rock")),
        "the way out is the line that would make it true: {:?}",
        refusal.help
    );
}

/// And the type that **does** implement it is not refused — which is the half
/// that matters, because a check that reads an `impl` table could as easily
/// refuse every call through a bound. Run rather than accepted: a bound that
/// type-checks here and not below would be worth nothing.
#[test]
fn the_type_that_implements_it_passes_and_runs() {
    let printed = ran(
        "a call through a bound",
        r#"
trait Speaks {
    fn say(ref self) -> String sync
}

struct Dog { name: String }

impl Speaks for Dog {
    fn say(ref self) -> String sync {
        return "woof".clone()
    }
}

fn tell[T: Speaks](x: T) -> String sync {
    return x.say()
}

fn main() {
    let d = Dog { name: "rex".clone() }
    println(tell(d))
}
"#,
    );
    assert_eq!(printed.trim(), "woof");
}

/// `[T: A + B]`: **each** bound is a question of its own, and the one the type
/// misses is the one named.
#[test]
fn every_bound_in_a_list_is_asked() {
    let found = findings(
        r#"
trait Speaks {
    fn say(ref self) -> String sync
}

trait Weighs {
    fn weight(ref self) -> i64 sync
}

struct Dog { name: String }

impl Speaks for Dog {
    fn say(ref self) -> String sync {
        return "woof".clone()
    }
}

fn both[T: Speaks + Weighs](x: T) -> String sync {
    return x.say()
}

fn main() {
    let d = Dog { name: "rex".clone() }
    println(both(d))
}
"#,
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1164")
        .unwrap_or_else(|| panic!("NK1164 for the bound it misses: {found:#?}"));
    assert!(
        refusal.message.contains("`Weighs`") && !refusal.message.contains("`Speaks`"),
        "the one it misses and not the one it has: {}",
        refusal.message
    );
}

/// **A parameter of the caller's own is a different sentence** (D2). `V` is a
/// name a *further* caller fills in, so `impl Speaks for V` is not a line
/// anybody can write and the way out is the caller's own bound list — which is
/// [Part III C.2](../../../docs/specification/30-nikaia-tooling.md)'s *a way
/// out*, and a way out that cannot be taken is not one.
#[test]
fn a_parameter_is_told_to_widen_its_own_bound() {
    let found = findings(
        r#"
trait Speaks {
    fn say(ref self) -> String sync
}

trait Weighs {
    fn weight(ref self) -> i64 sync
}

struct Dog { name: String }

impl Speaks for Dog {
    fn say(ref self) -> String sync {
        return "woof".clone()
    }
}

impl Weighs for Dog {
    fn weight(ref self) -> i64 sync {
        return 12
    }
}

fn tell[T: Speaks](x: T) -> String sync {
    return x.say()
}

fn passes_on[V: Weighs](z: V) -> String sync {
    return tell(z)
}

fn main() {
    let d = Dog { name: "rex".clone() }
    println(passes_on(d))
}
"#,
    );
    let refusal = found
        .iter()
        .find(|f| f.code == "NK1164")
        .unwrap_or_else(|| panic!("NK1164: {found:#?}"));
    assert!(
        refusal
            .help
            .as_deref()
            .is_some_and(|h| h.contains("[V: … + Speaks]")),
        "the way out is the bound list and not an `impl`: {:?}",
        refusal.help
    );
}

/// The same parameter, **carrying the bound it is asked for**, is not refused.
/// Without this the check would make a generic function uncallable from another
/// generic function, which is most of what a bound is for.
#[test]
fn a_parameter_that_carries_the_bound_passes_it_on() {
    let found = findings(
        r#"
trait Speaks {
    fn say(ref self) -> String sync
}

struct Dog { name: String }

impl Speaks for Dog {
    fn say(ref self) -> String sync {
        return "woof".clone()
    }
}

fn tell[T: Speaks](x: T) -> String sync {
    return x.say()
}

fn passes_on[U: Speaks](y: U) -> String sync {
    return tell(y)
}

fn main() {
    let d = Dog { name: "rex".clone() }
    println(passes_on(d))
}
"#,
    );
    assert!(
        found.is_empty(),
        "a parameter with the bound is a type that meets it: {found:#?}"
    );
}

/// **A trait this unit does not declare says nothing** — `traits::check` draws
/// the same line one rule over, and for the same reason: `impl Error for E`
/// names a trait the compiler reads rather than one a `.nika` file wrote
/// ([ADR-023](../../../docs/specification/adr/adr-023.md) D3), and a refusal
/// resting on a table that could not contain the answer is C.4's correct
/// program refused.
#[test]
fn a_bound_on_a_trait_nothing_here_declares_is_not_refused() {
    let found = findings(
        r#"
struct Rock { weight: i64 }

fn tell[T: Error](x: T) -> i64 sync {
    return 1
}

fn main() {
    let r = Rock { weight: 3 }
    println(f"{tell(r)}")
}
"#,
    );
    assert!(
        !found.iter().any(|f| f.code == "NK1164"),
        "nothing here can answer about `Error`: {found:#?}"
    );
}

/// **An `impl` of a trait nothing declares is refused here** (0.0.231), and
/// not by `rustc` about a file nobody wrote
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// `impl db::Connection for Fake` lowered as written and the language below
/// said *unresolved import `db`*; `impl Connection for Fake` said *cannot find
/// trait*. A bound may fail open on a trait it does not know, because it only
/// asks; an `impl` writes the name into the program.
#[test]
fn an_impl_of_a_trait_nothing_declares_is_refused() {
    let program = |trait_name: &str| {
        format!(
            "struct Fake {{ n: i64 }}\n\
             impl {trait_name} for Fake {{\n\
             \x20   fn run(ref self) -> i64 {{ return self.n }}\n\
             }}\n\
             fn main() {{ println(\"x\") }}\n"
        )
    };

    let bare = findings(&program("Connection"));
    assert_eq!(bare.len(), 1, "{bare:#?}");
    assert_eq!(bare[0].code, "NK1117", "{bare:#?}");
    assert_eq!(
        bare[0].message,
        "nothing declares a trait called `Connection`"
    );

    // A path is asked about its head, which is `NK1181`'s question.
    let path = findings(&program("db::Connection"));
    assert_eq!(path.len(), 1, "{path:#?}");
    assert_eq!(path[0].code, "NK1181", "{path:#?}");
    assert!(path[0].message.contains("`db`"), "{path:#?}");

    // A shape bound is answered by the declaration, so no `impl` says it.
    let shape = findings(&program("Struct"));
    assert_eq!(shape.len(), 1, "{shape:#?}");
    assert_eq!(shape[0].code, "NK1117", "{shape:#?}");
    assert_eq!(shape[0].message, "nothing declares a trait called `Struct`");
    assert!(shape[0].notes[0].contains("ADR-088"), "{shape:#?}");
}

/// **What declares a trait an `impl` may name**: the program's own `trait`,
/// and the three the language names itself. Each of these is a correct
/// program and the check must say nothing about it.
#[test]
fn an_impl_of_a_declared_trait_is_silent() {
    let own = "trait Speaks {\n\
               \x20   fn speak(ref self) -> String\n\
               }\n\
               struct Dog { name: String }\n\
               impl Speaks for Dog {\n\
               \x20   fn speak(ref self) -> String { return \"woof\" }\n\
               }\n\
               fn main() { println(Dog { name: \"rex\" }.speak()) }\n";
    assert!(findings(own).is_empty(), "{:#?}", findings(own));

    let dropped = "struct Handle { name: String }\n\
                   impl Drop for Handle {\n\
                   \x20   fn drop(ref mut self) { println(f\"closing {self.name}\") }\n\
                   }\n\
                   fn main() { let h = Handle { name: \"a\" } }\n";
    assert!(findings(dropped).is_empty(), "{:#?}", findings(dropped));
}
