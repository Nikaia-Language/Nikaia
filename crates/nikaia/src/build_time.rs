//! Running Nikaia while the program is built
//! ([ADR-287](../../../docs/specification/adr/adr-287.md) D6's second stage).
//!
//! `comptime` has had an evaluator since the word existed, and what it knew was
//! an integer literal, a name whose value already folded, a negation and
//! `+ - * / %` — [`fold`](crate::fold), 124 lines with no call in it. That is
//! D5's **first** stage, and the second one was written down as *when Q4 is
//! answered*: a call, and with it the file reading
//! [ADR-310](../../../docs/specification/adr/adr-310.md) waits behind.
//!
//! Q4 **is** answered — [ADR-287](../../../docs/specification/adr/adr-287.md)
//! D1 and D2 say what a build-time body may do — so this is the call, and with
//! it the **loop**: a `for` over a range, a `while`, `break`, `continue` and an
//! assignment, because a loop that cannot change anything is not one.
//!
//! **What a block means had to grow a third answer.** It used to be *a value or
//! an error*; a loop's body is neither — it runs to its end and produces
//! nothing, which is ordinary — so [`Flow`] names the four ways out and the
//! frame is shared rather than owned, since a `for` has to see what its body
//! assigned on the last turn.
//!
//! **What bounds it is a ledger column and not a list of allowed functions.** A
//! callee must be `sync` (D1) and its touch set must be empty or exactly the
//! build's own parameters (D2), and both are answers the compiler already
//! derives for every function it sees. Nothing here decides what is safe; it
//! reads what was decided.
//!
//! **There is no step budget** (D4), deliberately and with the cost written
//! down: a body that does not terminate hangs the build. What *is* bounded is
//! the **call depth**, and that is a different thing — an unbounded recursion
//! would overflow this compiler's own stack, and a compiler that falls over is
//! not the hang D4 accepted. The limit is high enough that no terminating
//! program meets it and the message says which call path found it.

use std::cell::RefCell;

use nikaia_std::tools::build_eval::{
    BtCallee, BtContract, BtHost, BtRan, BtRead, BtRefusal, bt_evaluate,
};

use crate::assets::{Denied, Reads};
use crate::ast::{Expr, Item};
use crate::contracts::ty::TyOps;
use crate::contracts::{Ledger, touch};
use crate::parser::Parsed;

/// The stack [`BuildTime::evaluate`] runs on: room for `BT_DEEPEST` calls
/// with a body nested deeply inside each, in a debug build, many times over.
/// Reserved, not touched - the pages a shallow evaluation never reaches cost
/// nothing.
const EVALUATION_STACK: usize = 256 * 1024 * 1024;

/// **What a build-time expression came to**, in Nikaia
/// (`tools/build_values.nika`, ADR-294, #125): an integer as a magnitude and
/// a sign, a float as the machine carries one, a truth value, decoded text, a
/// fixed-length list, a pair, a variant or a struct of the program's own -
/// each a kind a `const` can carry (ADR-287 D6).
pub use nikaia_std::tools::build_values::BuildValue as Value;

/// Why a build-time expression did not come to a value.
#[derive(Debug, Clone)]
pub enum Refusal {
    /// This evaluator does not know the shape. **Not an error by itself**: it is
    /// what `NK1127` reports, and what the staging above expects to keep
    /// reporting until every stage lands.
    Unevaluable,
    /// The callee may not be run while the program is built
    /// ([ADR-287](../../../docs/specification/adr/adr-287.md) D13, D14). A
    /// different claim from the one above and it gets a different code: the
    /// shape is understood and the rule says no.
    NotAllowed { callee: String, because: String },
    /// The call depth above.
    TooDeep { callee: String },
    /// **Understood, and not something this evaluator can do here.**
    ///
    /// A third thing, between the two above: `NotAllowed` is *the rule says
    /// no*, `Unevaluable` is *this shape is not read*, and this is *the shape
    /// is read and the thing it needs is somewhere this walk cannot reach*.
    /// It exists because the three want different sentences — a reader who
    /// calls `"a".to_uppercase()` is owed *its body is Rust* rather than a
    /// catalogue of what does work, since no amount of rewriting the line will
    /// help.
    NotHere {
        what: String,
        why: String,
        /// **The way out belongs to the wall.** *Put the work in a function of
        /// this file* is right for a callee in another file and is a trap for
        /// `"a".to_uppercase()`: the method would be just as unreadable one
        /// function further in. [Part III C.2](../../../docs/specification/30-nikaia-tooling.md)
        /// asks for a way out, and one that cannot be taken is not one.
        way_out: String,
    },
    /// A constant worked out from itself.
    ///
    /// Not the call depth above, which is a *recursion* that would terminate if
    /// the stack were deeper. This one never does: `comptime A = B` beside
    /// `comptime B = A` has no base case to reach, and the message names the
    /// ring rather than the limit it hit.
    Circular { ring: Vec<String> },
    /// **A file this build may not read**
    /// ([ADR-310](../../../docs/specification/adr/adr-310.md) D4 to D5).
    ///
    /// Understood and refused, which is why it is not `Unevaluable`: the shape
    /// is read, the path is in hand, and the answer is no — with a reason that
    /// differs per shape, so the sentence is the checker's to write.
    MayNotRead { path: String, why: Denied },
    /// **A grammar this compiler could not run**
    /// (issue #178).
    ///
    /// Six reasons and one variant, because the sentence is the checker's to
    /// write and they share nothing but the code: a parser that did not compile
    /// is this compiler's fault, and input the parser refused is the input's.
    GrammarWall {
        grammar: String,
        rule: String,
        why: crate::grammar_run::Wall,
    },
    /// **A path that is not a literal** (D4).
    ///
    /// Its own variant because it is answered *before* the argument is
    /// evaluated: a name that folds to `"config.json"` would otherwise be read
    /// as one, and then *named in the code* would stop being decidable by
    /// looking at the line, which is the whole of what the three namings buy.
    PathIsComputed,
    /// An index this array does not have. **Understood and wrong**, like
    /// `NotAllowed` and unlike `Unevaluable`: the program says `xs[7]` of five
    /// elements, and a build that answered *cannot evaluate* would send the
    /// reader looking for a missing feature instead of at the line
    /// ([Part III C.2](../../../docs/specification/30-nikaia-tooling.md)).
    /// Running it would abort at run time ([ADR-285](../../../docs/specification/adr/adr-285.md)
    /// D1); at build time there is no run to abort.
    OutOfBounds { at: Integer, len: usize },
}

/// What a name outside a build-time body is worth: a `comptime` already
/// evaluated, or a `let` whose value folded.
/// **An integer while the program is built**: a magnitude and a sign, the
/// shape the tree gives a literal (ADR-294 D11), with what the evaluator does
/// with two of them written in Nikaia (`tools/integers.nika`).
pub use nikaia_std::tools::integers::Integer;

pub type Known<'a> = &'a (dyn Fn(&str) -> Option<Value> + Sync);

/// The evaluator, over one unit's items. The evaluator itself is
/// `tools/build_eval.nika` (ADR-294, #125); this holds what it asks for - the
/// program's files, the ledger, what a build may read, and the grammar workshop.
pub struct BuildTime<'a> {
    parsed: &'a Parsed,
    /// **Every file of this program**, because a body is an AST and an AST
    /// belongs to the file that was parsed into it.
    beside: &'a [&'a Parsed],
    own: &'a Ledger,
    /// **What this build may read while it builds**
    /// ([ADR-310](../../../docs/specification/adr/adr-310.md)). A caller that
    /// passes [`Reads::none`] gets D1: the whole class is off, which is what
    /// every test and every build that did not ask for it gets.
    reads: &'a Reads,
    known: Known<'a>,
}

impl<'a> BuildTime<'a> {
    pub fn new(
        parsed: &'a Parsed,
        beside: &'a [&'a Parsed],
        own: &'a Ledger,
        reads: &'a Reads,
        known: Known<'a>,
    ) -> Self {
        Self {
            parsed,
            beside,
            own,
            reads,
            known,
        }
    }

    /// What an initialiser comes to.
    ///
    /// **On a thread of its own, with a stack of its own size**
    /// ([`EVALUATION_STACK`]). `BT_DEEPEST` (`tools/build_eval.nika`) counts calls, and what a call costs
    /// on the stack differs by build profile and by how deeply the body nests.
    /// Whether the refusal happens may not depend on which thread asked.
    pub fn evaluate(&mut self, expr: &Expr) -> Result<Value, Refusal> {
        let this = &*self;
        let run = move || this.run(expr);
        std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("nikaia-build-time".to_string())
                .stack_size(EVALUATION_STACK)
                .spawn_scoped(scope, run)
                .map(|thread| thread.join())
        })
        .unwrap_or_else(|_| {
            // No thread to be had: the caller's own stack, as before.
            Ok(self.run(expr))
        })
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }

    fn run(&self, expr: &Expr) -> Result<Value, Refusal> {
        let mut units: Vec<&Parsed> = vec![self.parsed];
        units.extend(self.beside.iter().copied());
        let host = Host {
            units,
            own: self.own,
            reads: self.reads,
            known: self.known,
            kept: RefCell::new(Vec::new()),
        };
        bt_evaluate(expr, &host).map_err(|thrown| host.refusal(thrown.split().0))
    }
}

/// What the evaluator asks the compiler: the files by number (0 is the one
/// being checked), the ledger, what may be read, and the grammar workshop.
struct Host<'h> {
    units: Vec<&'h Parsed>,
    own: &'h Ledger,
    reads: &'h Reads,
    known: Known<'h>,
    /// The refusals the evaluator carries by their place here: a grammar that
    /// would not run says why in a type of this compiler's.
    kept: RefCell<Vec<Refusal>>,
}

impl Host<'_> {
    fn refusal(&self, refused: BtRefusal) -> Refusal {
        match refused {
            BtRefusal::Unevaluable => Refusal::Unevaluable,
            BtRefusal::NotAllowed { callee, because } => Refusal::NotAllowed { callee, because },
            BtRefusal::TooDeep { callee } => Refusal::TooDeep { callee },
            BtRefusal::NotHere { what, why, way_out } => Refusal::NotHere { what, why, way_out },
            BtRefusal::Circular { ring } => Refusal::Circular { ring },
            BtRefusal::MayNotRead { path, why } => Refusal::MayNotRead { path, why },
            BtRefusal::PathIsComputed => Refusal::PathIsComputed,
            BtRefusal::OutOfBounds { at, len } => Refusal::OutOfBounds {
                at,
                len: len as usize,
            },
            BtRefusal::Host(slot) => self.kept.borrow()[slot as usize].clone(),
        }
    }

    /// The files in the order a name is looked for: the one the body being read
    /// came from, and then every file beside the one being checked.
    fn in_reach(&self, unit: i64) -> impl Iterator<Item = (usize, &Parsed)> {
        std::iter::once(unit as usize)
            .chain(1..self.units.len())
            .map(|at| (at, self.units[at]))
    }

    fn in_file(&self, parsed: &Parsed, at: usize, name: &str) -> Option<BtCallee> {
        // `Tag::doubled` is a method's key, and it is the ledger's own - so the
        // split here is the same one `contracts` makes when it writes the
        // entry, and the two cannot drift about which name a call resolves to.
        match name.split_once("::") {
            Some((target, method)) => method_of(parsed, at, target, method),
            None => free_body_of(parsed, at, name),
        }
    }
}

impl BtHost for Host<'_> {
    fn text(&self, unit: i64, symbol: winnow_grammar::Symbol) -> String {
        self.units[unit as usize].text(symbol).to_string()
    }

    fn char_of(&self, n: i64) -> Option<char> {
        char_of(n)
    }

    fn known(&self, name: &str) -> Option<Value> {
        (self.known)(name)
    }

    fn contract(&self, name: &str) -> BtContract {
        match self.own.functions.get(name) {
            None => BtContract {
                declared: false,
                reason: String::new(),
            },
            Some(contract) => BtContract {
                declared: true,
                reason: may_not_run(contract).unwrap_or_default().to_string(),
            },
        }
    }

    fn callee(&self, unit: i64, name: &str) -> Option<BtCallee> {
        self.in_reach(unit)
            .find_map(|(at, parsed)| self.in_file(parsed, at, name))
    }

    fn constant(&self, unit: i64, name: &str) -> Option<Expr> {
        let parsed = self.units[unit as usize];
        parsed
            .program
            .items
            .iter()
            .find_map(|item| match &item.node {
                Item::Comptime {
                    name: declared,
                    value,
                    ..
                } if parsed.text(*declared) == name => Some(value.clone()),
                _ => None,
            })
    }

    fn variant_of(&self, unit: i64, ty: &str, variant: &str) -> bool {
        self.in_reach(unit).any(|(_, parsed)| {
            parsed.program.items.iter().any(|item| match &item.node {
                Item::Enum { name, variants, .. } if parsed.text(*name) == ty => variants
                    .iter()
                    .any(|held| parsed.text(held.name) == variant),
                _ => false,
            })
        })
    }

    fn run_grammar(&self, unit: i64, which: &str, rule: &str, input: &str) -> BtRan {
        let Some((parsed, result)) = self.rule_of(unit, which, rule) else {
            return BtRan {
                found: false,
                value: None,
                slot: -1,
            };
        };
        let beside: Vec<&Parsed> = self.units[1..].to_vec();
        let ask = crate::grammar_run::Ask {
            grammar: which,
            rule,
            input,
            result: &result,
            units: &beside,
        };
        let wall = |why| Refusal::GrammarWall {
            grammar: which.to_string(),
            rule: rule.to_string(),
            why,
        };
        let outcome = match self.reads.workshop().run(parsed, &ask) {
            Ok(dump) => crate::grammar_run::decode(&dump).map_err(wall),
            Err(why) => Err(wall(why)),
        };
        match outcome {
            Ok(value) => BtRan {
                found: true,
                value: Some(value),
                slot: -1,
            },
            Err(refusal) => {
                let mut kept = self.kept.borrow_mut();
                kept.push(refusal);
                BtRan {
                    found: true,
                    value: None,
                    slot: kept.len() as i64 - 1,
                }
            }
        }
    }

    fn read(&self, path: &str) -> BtRead {
        match self.reads.read(path) {
            Ok(text) => BtRead {
                text: Some(text),
                denied: None,
            },
            Err(why) => BtRead {
                text: None,
                denied: Some(why),
            },
        }
    }
}

impl Host<'_> {
    /// The file a grammar was declared in, and what its `entry` rule hands back.
    fn rule_of(
        &self,
        unit: i64,
        grammar: &str,
        rule: &str,
    ) -> Option<(&Parsed, crate::contracts::ty::Ty)> {
        self.in_reach(unit).find_map(|(_, parsed)| {
            let found = parsed
                .program
                .items
                .iter()
                .find_map(|item| match &item.node {
                    Item::Grammar(def) if parsed.text(def.name) == grammar => def
                        .rules
                        .iter()
                        .find(|r| r.is_entry && parsed.text(r.name) == rule),
                    _ => None,
                })?;
            let ty = found.ret_type.as_ref()?;
            Some((parsed, crate::contracts::ty::Ty::from_ast(parsed, ty)))
        })
    }
}

fn parameters(parsed: &Parsed, args: &[crate::ast::FnArg]) -> Vec<String> {
    args.iter()
        .map(|arg| parsed.text(arg.name).to_string())
        .collect()
}

fn options_of(parsed: &Parsed, config: &[crate::ast::ConfigParam]) -> Vec<(String, Expr)> {
    config
        .iter()
        .map(|option| (parsed.text(option.name).to_string(), option.default.clone()))
        .collect()
}

fn free_body_of(parsed: &Parsed, owner: usize, name: &str) -> Option<BtCallee> {
    parsed.program.items.iter().find_map(|item| {
        let Item::Fn {
            name: declared,
            args,
            receiver,
            config,
            body,
            ..
        } = &item.node
        else {
            return None;
        };
        if receiver.is_some() || parsed.text((*declared)?) != name {
            return None;
        }
        Some(BtCallee {
            args: parameters(parsed, args),
            options: options_of(parsed, config),
            body: body.clone(),
            owner: owner as i64,
        })
    })
}

/// A method of a `struct` this file declares, by the key a call resolves to.
///
/// **`self` is the first parameter**, which is the shape the call site builds:
/// the receiver is evaluated before the arguments, because it is what says
/// *which* method. A method with **no receiver** is Kap 4.2's constructor and
/// is reached by its own name (`Stats::new`), so it takes no `self`.
fn method_of(parsed: &Parsed, owner: usize, target: &str, method: &str) -> Option<BtCallee> {
    parsed.program.items.iter().find_map(|item| {
        let Item::Impl {
            target: on,
            methods,
            ..
        } = &item.node
        else {
            return None;
        };
        if parsed.text(on.name) != target {
            return None;
        }
        methods.iter().find_map(|declared| {
            let Item::Fn {
                name,
                args,
                receiver,
                config,
                body,
                ..
            } = &declared.node
            else {
                return None;
            };
            if parsed.text((*name)?) != method {
                return None;
            }
            let mut names = match receiver {
                Some(_) => vec!["self".to_string()],
                None => Vec::new(),
            };
            names.extend(parameters(parsed, args));
            Some(BtCallee {
                args: names,
                options: options_of(parsed, config),
                body: body.clone(),
                owner: owner as i64,
            })
        })
    })
}

/// An escape the set does not name, as it is written, and why.
///
/// **The one table, with two readers**
/// ([ADR-188](../../docs/specification/adr/adr-188.md) D2): [`decoded`] turns a
/// literal into a value at build time, and `Checker::an_escape_nothing_names`
/// refuses a literal the set does not cover. Both read
/// `crates/nikaia-std/src/tools/escapes.nika`, where the table is Nikaia
/// ([ADR-294](../../docs/specification/adr/adr-294.md), #125).
pub use nikaia_std::tools::escapes::NotAnEscape as Refused;

/// The escapes this language has, as a reader of a diagnostic wants them
/// ([Part I 2.5](../../docs/specification/10-nikaia-light.md)).
///
/// A list and not a sentence, because the message prints it and the page prints
/// it, and a set written twice is a set that disagrees with itself.
pub const ESCAPES: &str = "\\n \\r \\t \\0 \\\\ \\' \\\" \\xNN \\u{…}";

/// Which numbers name a character: Unicode's table, handed to the Nikaia walk.
pub(crate) fn char_of(n: i64) -> Option<char> {
    u32::try_from(n).ok().and_then(char::from_u32)
}

/// The first escape in a literal that this language's set does not name.
///
/// **Reads the same table [`decoded`] does**, which is what makes a refusal
/// here safe: everything this returns `Some` for is something `decoded`
/// returns `None` for, and `rustc` refuses in its own words.
pub fn an_escape_nothing_names(literal: &str) -> Option<Refused> {
    nikaia_std::tools::escapes::an_escape_nothing_names(literal, &char_of)
}

/// **What a written string literal means** — the value behind the spelling.
///
/// The parser keeps a literal's escapes (`STR_CHAR` takes `\` and any
/// character) and the emitter hands the text to `rustc` unchanged, so **this
/// language's escapes are Rust's**, decided by what that compiler accepts
/// rather than by a page here. So this is a **faithful reading** and not a
/// second definition. `None` where the escape is one `rustc` would reject:
/// that program does not compile either way, and the evaluator says *cannot
/// evaluate* rather than inventing a meaning for it.
///
/// [`written`] is the inverse, and `crates/nikaia/tests/build_time.rs` holds
/// the pair to the only standard that settles it: the same literal, read at
/// build time and at run time, printing the same bytes.
pub fn decoded(literal: &str) -> Option<String> {
    nikaia_std::tools::escapes::decoded(literal, &char_of)
}

/// The inverse of [`decoded`]: a value, spelled as a literal `rustc` reads.
/// Every control character is written as an escape.
pub fn written(text: &str) -> String {
    nikaia_std::tools::escapes::as_a_literal(text)
}

/// **Why a function may not run while the program is built**, read off its
/// ledger entry ([ADR-287](../../../docs/specification/adr/adr-287.md) D13,
/// D14): it can pause, nothing says what it touches, or it touches more than
/// the build's own parameters. Nothing where it may run.
pub(crate) fn may_not_run(contract: &crate::contracts::FnContract) -> Option<&'static str> {
    if !contract.sync_claim.is_sync() {
        return Some("It can pause, and nothing run at build time may pause.");
    }
    if !contract.touches_known {
        return Some("Nothing says what it touches outside the program.");
    }
    if !contract.touches.iter().all(is_the_builds_own) {
        return Some("It touches the world outside the program.");
    }
    None
}

/// Whether a touch is the build's own parameters
/// ([ADR-287](../../../docs/specification/adr/adr-287.md) D14).
///
/// **`cli::args` passes, deliberately**: same parameters, same code. A build's
/// own arguments are an input like its source is, and a body that shapes a
/// table differently for two declared parameters is doing the thing `comptime`
/// is for.
fn is_the_builds_own(touch: &touch::Touch) -> bool {
    touch.text() == "args read"
}
