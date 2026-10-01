//! Running Nikaia while the program is built
//! ([ADR-073](../../../docs/specification/adr/adr-073.md) D5's second stage).
//!
//! `comptime` has had an evaluator since the word existed, and what it knew was
//! an integer literal, a name whose value already folded, a negation and
//! `+ - * / %` — [`fold`](crate::fold), 124 lines with no call in it. That is
//! D5's **first** stage, and the second one was written down as *when Q4 is
//! answered*: a call, and with it the file reading
//! [ADR-072](../../../docs/specification/adr/adr-072.md) waits behind.
//!
//! Q4 **is** answered — [ADR-075](../../../docs/specification/adr/adr-075.md)
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

use std::collections::BTreeMap;

use crate::assets::{ASSET, Denied, Reads};
use crate::ast::{BinaryOp, Block, Expr, Item, Stmt, UnaryOp};
use crate::contracts::ty::TyOps;
use crate::contracts::{Ledger, touch};
use crate::parser::Parsed;

/// How deep a build-time call may go before this stops
/// ([ADR-075](../../../docs/specification/adr/adr-075.md) D4's neighbour).
///
/// **Not a step budget.** A body that loops forever still hangs the build, which
/// is what that record accepted; what this prevents is a *recursion* that takes
/// the compiler's stack down with it, which it did not.
const DEEPEST: usize = 128;

/// The stack [`BuildTime::evaluate`] runs on: room for [`DEEPEST`] calls
/// with a body nested deeply inside each, in a debug build, many times over.
/// Reserved, not touched - the pages a shallow evaluation never reaches cost
/// nothing.
const EVALUATION_STACK: usize = 256 * 1024 * 1024;

/// How a block ended.
///
/// Four ways, and the evaluator needed all four the moment it gained a loop: a
/// block that **falls through** has no value and is not an error — it is a
/// loop's body between turns — where before this a block with no value was the
/// only thing `Unevaluable` could mean.
#[derive(Debug, Clone)]
enum Flow {
    /// A `return`, or a last statement that is a value.
    Value(Value),
    /// Ran to the end and produced nothing.
    Fell,
    Broke,
    Continued,
}

/// What a build-time expression came to.
///
/// Four kinds, which is what the declaration can carry: Rust's `const` needs a
/// type this compiler can spell
/// ([ADR-073](../../../docs/specification/adr/adr-073.md) D5) — an integer, a
/// `bool`, a **list** of them, and **text**.
///
/// **The array is the aggregate issue #174 was about**, and it is an
/// array rather than a `Vec` for the reason that entry gives from the other
/// side: a `Vec` allocates and a `const` cannot hold one, where `[T; N]` is
/// exactly what one holds ([ADR-152](../../../docs/specification/adr/adr-152.md)).
/// So a build-time table is written at its length and filled by index, which is
/// the shape the type system already had — measured: `.push` on a list hands
/// back a `Vec[?]`, and `NK1104` refuses it against an `Array[i64, 5]` before
/// this evaluator is ever reached.
/// **`PartialEq` and no longer `Eq`**, since a float joined. Equality over a
/// float is not an equivalence — `NaN` is equal to nothing, itself included —
/// and keeping the derive by pretending otherwise would be a lie this compiler
/// then reasons with. Nothing here needs a total order or a hash.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// In an `i128`, so a sum that cannot fit an `i64` is a number the caller
    /// can name rather than one that wrapped — [`fold`](crate::fold)'s reason,
    /// one level up.
    Int(i128),
    /// **A number with a point in it**, carried as the machine carries one.
    ///
    /// It arrives as a literal or out of a grammar, and it crosses as the
    /// `f64` or `f32` a declaration asks for. What it does **not** do is take
    /// part in [`fold`](crate::fold)'s arithmetic, which is integers and stays
    /// integers: a build that folded `0.1 + 0.2` would have to answer for the
    /// answer, and nothing has asked it to yet.
    ///
    /// **A value that is not finite is refused where it crosses**, not here:
    /// there is no literal for an infinity or a `NaN` in either language, so a
    /// `const` holding one cannot be written — and a compiler that wrote
    /// `f64::INFINITY` would be putting a name into the program that the
    /// `.nika` line never mentioned.
    Float(f64),
    Bool(bool),
    /// A fixed-length list, every element already a value.
    List(Vec<Value>),
    /// Text, **decoded** — the value, not the spelling.
    ///
    /// 0.0.112 held the written form instead, escapes and all, because the
    /// parser keeps them and the emitter hands them to `rustc` verbatim. That
    /// agreed with the lowering and cost every question about the *value*:
    /// `"\u{0041}"` and `"A"` are one value and two spellings, so `.len()` and
    /// `==` were refused rather than answered wrongly.
    ///
    /// **The refusal was a representation showing through, and the fix is a
    /// decoder.** Which escapes exist is not a question this language has left
    /// open, though no page states it: the parser takes `\` and any character
    /// and hands the literal to the backend unchanged, so `rustc` is what
    /// accepts or rejects it — measured, `"a\qb"` is *unknown character
    /// escape* on the `.nika` line. This language's escapes **are** that one's,
    /// so [`decoded`] is a faithful reading rather than an invention, and
    /// [`written`] puts it back.
    Text(String),
    /// **A variant of an `enum` this program declares**, with what it carries.
    ///
    /// `Shade::Odd` and `Json::Number(1.5)` are both this; the payload is empty
    /// for the first. It lands as a `const` the way a struct does — `const C:
    /// Shade = Shade::Odd;` is Rust — and the **payloads** are what decide
    /// whether it can: a variant carrying a `Vec` has no `const` form for the
    /// same reason a field that is one does not.
    ///
    /// **The name is carried and not resolved here.** Which `enum` a variant
    /// belongs to is the checker's answer and the emitter writes what it is
    /// told ([ADR-011](../../../docs/specification/adr/adr-011.md) D2), so this
    /// holds the pair rather than a reference to a declaration.
    Variant {
        ty: String,
        variant: String,
        payload: Vec<Value>,
    },
    /// A tuple, which is what a **pair** is
    /// ([ADR-176](../../../docs/specification/adr/adr-176.md) D1): a map the
    /// build can see is written `[("get", 1), ("post", 2)]`, and that needed no
    /// new literal — only a value for the one this language already has.
    Tuple(Vec<Value>),
    /// A value of a `struct` this program declares, by field name.
    ///
    /// **What it is for is the method.** A `sync` method of the program's own
    /// looked exactly like something a `comptime` should be able to call — the
    /// body is right there and `sync` says it may run
    /// ([ADR-075](../../../docs/specification/adr/adr-075.md) D1) — and it
    /// could not, because a method needs a **value** to be called on and this
    /// evaluator had none to make.
    ///
    /// It lands as a `const` like anything else: `const P: Point = Point { x: 1, y: 2 };`
    /// is Rust, and a struct whose fields own nothing is already its own view —
    /// [ADR-079](../../../docs/specification/adr/adr-079.md) D1's *a number is
    /// already its own view*, read one shape out.
    Struct {
        name: String,
        fields: BTreeMap<String, Value>,
    },
}

/// Why a build-time expression did not come to a value.
#[derive(Debug, Clone)]
pub enum Refusal {
    /// This evaluator does not know the shape. **Not an error by itself**: it is
    /// what `NK1127` reports, and what the staging above expects to keep
    /// reporting until every stage lands.
    Unevaluable,
    /// The callee may not be run while the program is built
    /// ([ADR-075](../../../docs/specification/adr/adr-075.md) D1, D2). A
    /// different claim from the one above and it gets a different code: the
    /// shape is understood and the rule says no.
    NotAllowed {
        callee: String,
        because: &'static str,
    },
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
        why: &'static str,
        /// **The way out belongs to the wall.** *Put the work in a function of
        /// this file* is right for a callee in another file and is a trap for
        /// `"a".to_uppercase()`: the method would be just as unreadable one
        /// function further in. [Part III C.2](../../../docs/specification/30-nikaia-tooling.md)
        /// asks for a way out, and one that cannot be taken is not one.
        way_out: &'static str,
    },
    /// A constant worked out from itself.
    ///
    /// Not the call depth above, which is a *recursion* that would terminate if
    /// the stack were deeper. This one never does: `comptime A = B` beside
    /// `comptime B = A` has no base case to reach, and the message names the
    /// ring rather than the limit it hit.
    Circular { ring: Vec<String> },
    /// **A file this build may not read**
    /// ([ADR-072](../../../docs/specification/adr/adr-072.md) D1 to D5).
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
    /// Running it would abort at run time ([ADR-048](../../../docs/specification/adr/adr-048.md)
    /// D1); at build time there is no run to abort.
    OutOfBounds { at: i128, len: usize },
}

/// What a name outside a build-time body is worth: a `comptime` already
/// evaluated, or a `let` whose value folded.
pub type Known<'a> = &'a (dyn Fn(&str) -> Option<Value> + Sync);

/// The evaluator, over one unit's items.
pub struct BuildTime<'a> {
    parsed: &'a Parsed,
    /// **Every file of this program**, because a body is an AST and an AST
    /// belongs to the file that was parsed into it.
    ///
    /// The *permission* to run a callee has been program-wide from the start —
    /// it is two ledger columns ([ADR-075](../../../docs/specification/adr/adr-075.md)
    /// D1, D2) and a program's ledger is absorbed from its units'. What was not
    /// was the **body**, and no column could carry one: a ledger records what a
    /// caller has to know about a function it *cannot see the body of*, which
    /// is the opposite of what this needs.
    beside: &'a [&'a Parsed],
    own: &'a Ledger,
    /// **What this build may read while it builds**
    /// ([ADR-072](../../../docs/specification/adr/adr-072.md)). A caller that
    /// passes [`Reads::none`] gets D1: the whole class is off, which is what
    /// every test and every build that did not ask for it gets.
    reads: &'a Reads,
    known: Known<'a>,
    depth: usize,
    /// Whether the body being read came from a file other than the one being
    /// checked — see [`BuildTime::call`] for what it costs.
    foreign: bool,
    /// The item-level constants being worked out, innermost last.
    ///
    /// **A constant is an item, so it is visible wherever its file is** — which
    /// makes `comptime A = B * 2` above `comptime B = 21` a program, and makes
    /// `comptime A = B` beside `comptime B = A` one this has to refuse rather
    /// than run forever. The stack is what tells the two apart, and it names
    /// the ring.
    resolving: Vec<String>,
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
            depth: 0,
            foreign: false,
            resolving: Vec::new(),
        }
    }

    /// What an initialiser comes to.
    ///
    /// **On a thread of its own, with a stack of its own size**
    /// ([`EVALUATION_STACK`]). [`DEEPEST`] counts calls, and what a call costs
    /// on the stack is this evaluator's frames, which differ by build profile
    /// and by how deeply the body nests: measured, 128 calls of a one-line
    /// recursion took more than the 2 MiB a test thread has in a debug build,
    /// and `an_unbounded_recursion_is_refused_rather_than_crashing` overflowed
    /// instead of reading `NK1152`. Whether the refusal happens may not depend
    /// on which thread asked.
    pub fn evaluate(&mut self, expr: &Expr) -> Result<Value, Refusal> {
        std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("nikaia-build-time".to_string())
                .stack_size(EVALUATION_STACK)
                .spawn_scoped(scope, || self.expr(expr, &BTreeMap::new()))
                .map(|thread| thread.join())
        })
        .unwrap_or_else(|_| {
            // No thread to be had: the caller's own stack, as before.
            Ok(self.expr(expr, &BTreeMap::new()))
        })
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }

    fn expr(&mut self, expr: &Expr, frame: &BTreeMap<String, Value>) -> Result<Value, Refusal> {
        match expr {
            Expr::LitInt { value, negative } => {
                Ok(Value::Int(crate::ast::int_value(*value, *negative)))
            }
            Expr::LitFloat(written) => written
                .parse()
                .map(Value::Float)
                .map_err(|_| Refusal::Unevaluable),
            Expr::LitBool(value) => Ok(Value::Bool(*value)),
            Expr::LitStr { text, .. } => decoded(text).map(Value::Text).ok_or(Refusal::Unevaluable),
            Expr::Tuple(parts) => {
                let mut held = Vec::with_capacity(parts.len());
                for part in parts {
                    held.push(self.expr(part, frame)?);
                }
                Ok(Value::Tuple(held))
            }
            // **`f"…"` is text with code in it** (ADR-035), and the code is
            // Nikaia, so this evaluator can read it — which is what makes text
            // at build time worth having at all. A literal alone would be a
            // value somebody could have written down.
            Expr::LitInterpolated(literal) => self.interpolated(literal, frame),
            // **A variant that carries nothing**: `Shade::Odd`, which is a
            // value and not a call. The path arm below a call, one shape out.
            Expr::Path(path) => {
                let names: Vec<&str> = path.iter().map(|s| self.parsed.text(*s)).collect();
                let [ty, variant] = names.as_slice() else {
                    return Err(Refusal::Unevaluable);
                };
                match self.variant_of(ty, variant) {
                    Some(()) => Ok(Value::Variant {
                        ty: ty.to_string(),
                        variant: variant.to_string(),
                        payload: Vec::new(),
                    }),
                    None => Err(Refusal::Unevaluable),
                }
            }
            Expr::Variable(name) => {
                let name = self.parsed.text(*name).to_string();
                if let Some(held) = frame.get(&name) {
                    return Ok(held.clone());
                }
                // **The checking file's scope**, which holds what the walk
                // above this one has already worked out. Asked only while the
                // body is that file's: a body read elsewhere names its own
                // file's constants, and this scope is not that file's.
                if !self.foreign
                    && let Some(known) = (self.known)(&name)
                {
                    return Ok(known);
                }
                // …and otherwise the **item**, read from the file that wrote
                // it. A constant is an item, so it is visible wherever its file
                // is — which is what makes a forward reference a program, and
                // what lets a body in another file name its own constants
                // without this one guessing from the wrong scope.
                self.constant_item(&name)
            }
            Expr::Unary { op, expr } => {
                let inner = self.expr(expr, frame)?;
                match (op, inner) {
                    (UnaryOp::Neg, Value::Int(v)) => {
                        Ok(Value::Int(v.checked_neg().ok_or(Refusal::Unevaluable)?))
                    }
                    (UnaryOp::Not, Value::Bool(v)) => Ok(Value::Bool(!v)),
                    _ => Err(Refusal::Unevaluable),
                }
            }
            Expr::Binary { op, lhs, rhs, .. } => self.binary(*op, lhs, rhs, frame),
            // **An `if` is an expression always** (Part I 3.1), so it is one
            // here too — and it is what makes a body worth calling at all.
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let taken = match self.expr(cond, frame)? {
                    Value::Bool(true) => Some(then_branch),
                    Value::Bool(false) => else_branch.as_ref(),
                    _ => return Err(Refusal::Unevaluable),
                };
                // **The branch gets a frame of its own**, because an `if` in
                // value position is not a place a name is assigned from: what
                // the branch writes is the branch's, and what it hands back is
                // the value.
                let Some(block) = taken else {
                    return Err(Refusal::Unevaluable);
                };
                let mut inner = frame.clone();
                match self.block(block, &mut inner)? {
                    Flow::Value(value) => Ok(value),
                    Flow::Fell | Flow::Broke | Flow::Continued => Err(Refusal::Unevaluable),
                }
            }
            // **A list literal is the aggregate's only constructor here**
            // ([ADR-135](../../../docs/specification/adr/adr-135.md)). There is
            // no `push`: what `.push` hands back is a `Vec[?]`, and `NK1104`
            // refuses one against the `Array[T, N]` a `const` can hold long
            // before this evaluator sees it. So a table is written at its
            // length and filled by index, below.
            Expr::ListLit { items, .. } => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.expr(item, frame)?);
                }
                Ok(Value::List(values))
            }
            Expr::Index { base, index } => {
                let on = self.expr(base, frame)?;
                let at = self.expr(index, frame)?;
                element(&on, &at).cloned()
            }
            // `xs.len()`, which is what a loop over a table is written with —
            // `for i in 0..<xs.len()`. One method and no others: the length of
            // a list this evaluator already holds is a fact it has, where
            // anything else would be a body somewhere it cannot read.
            Expr::MethodCall {
                receiver,
                method,
                args,
                config,
            } if args.is_empty() && config.is_empty() && self.parsed.text(*method) == "len" => {
                match self.expr(receiver, frame)? {
                    Value::List(items) => Ok(Value::Int(items.len() as i128)),
                    // **Bytes, which is what `String::len` says it is.** Text
                    // is UTF-8 and a character outside ASCII is more than one
                    // byte; `chars().count()` is the other question and `std`
                    // spells it out. The value is decoded, so this is the
                    // number the program would have counted itself.
                    Value::Text(text) => Ok(Value::Int(text.len() as i128)),
                    _ => Err(Refusal::Unevaluable),
                }
            }
            // **A method is not a shape this evaluator reads**, and the two it
            // does read above - `len` and `push` over a list it holds - are
            // forms it knows itself rather than entries it resolved. Said out
            // loud, because a `sync` method of this program's own looks exactly
            // like something that should work: `sync` is the **permission**
            // ([ADR-075](../../../docs/specification/adr/adr-075.md) D1) and a
            // body this walk can read is the **ability**, and they are two
            // different things.
            // Kap 4.2's literal, and the shorthand with it: `Point { x, y }`
            // is `Point { x: x, y: y }`, which the parser leaves as a field
            // with no value of its own.
            Expr::StructLit { name, fields } => {
                let name = self.parsed.text(*name).to_string();
                let mut held = BTreeMap::new();
                for field in fields {
                    let written = self.parsed.text(field.name).to_string();
                    let value = match &field.value {
                        Some(value) => self.expr(value, frame)?,
                        None => frame.get(&written).cloned().ok_or(Refusal::Unevaluable)?,
                    };
                    held.insert(written, value);
                }
                Ok(Value::Struct { name, fields: held })
            }
            Expr::Field { base, name } => {
                let on = self.expr(base, frame)?;
                let field = self.parsed.text(*name);
                match on {
                    Value::Struct { mut fields, .. } => {
                        fields.remove(field).ok_or(Refusal::Unevaluable)
                    }
                    _ => Err(Refusal::Unevaluable),
                }
            }
            // **A method of this program, on a value this evaluator made.**
            // The receiver is evaluated first because it is what says *which*
            // method: a `Point`'s `length` and a `Line`'s are two entries, and
            // the ledger keys them apart by the type.
            Expr::MethodCall {
                receiver,
                method,
                args,
                config,
            } if config.is_empty() => {
                let on = self.expr(receiver, frame)?;
                let Value::Struct { name, .. } = &on else {
                    return Err(self.no_method_here(*method));
                };
                let key = format!("{name}::{}", self.parsed.text(*method));
                let mut given = vec![on.clone()];
                for arg in args {
                    given.push(self.expr(arg, frame)?);
                }
                self.call(&key, &given)
            }
            Expr::MethodCall { method, .. } => Err(self.no_method_here(*method)),
            // **A grammar's entry, run by compiling the parser it generates**
            // (issue #178, Part II
            // 10.2 A). `Json::value(asset("config.json"))` says *when* with the
            // `comptime` around it, *where the bytes come from* with `asset`,
            // and *what is done with them* here.
            Expr::Call { func, args, config }
                if config.is_empty() && matches!(func.as_ref(), Expr::Path(_)) =>
            {
                let Expr::Path(path) = func.as_ref() else {
                    return Err(Refusal::Unevaluable);
                };
                let names: Vec<&str> = path.iter().map(|s| self.parsed.text(*s)).collect();
                let [grammar, rule] = names.as_slice() else {
                    return Err(Refusal::Unevaluable);
                };
                let (grammar, rule) = (grammar.to_string(), rule.to_string());
                // **A variant with a payload is written the same way a grammar
                // rule is called** — `Json::Number(1.5)` beside
                // `Cfg::file(text)` — so the declaration decides which it is.
                // An `enum` this program declares wins, because a grammar and
                // an `enum` under one name is a name declared twice and
                // `NK1148`'s to refuse, not this walk's.
                if self.variant_of(&grammar, &rule).is_some() {
                    let mut payload = Vec::with_capacity(args.len());
                    for arg in args {
                        payload.push(self.expr(arg, frame)?);
                    }
                    return Ok(Value::Variant {
                        ty: grammar,
                        variant: rule,
                        payload,
                    });
                }
                let [input] = args.as_slice() else {
                    return Err(Refusal::Unevaluable);
                };
                let Value::Text(input) = self.expr(input, frame)? else {
                    return Err(Refusal::Unevaluable);
                };
                self.grammar(&grammar, &rule, &input)
            }
            Expr::Call { func, args, config } if config.is_empty() => {
                let Expr::Variable(name) = func.as_ref() else {
                    return Err(Refusal::Unevaluable);
                };
                let name = self.parsed.text(*name).to_string();
                // **`asset("…")` is answered before the arguments are**
                // ([ADR-116](../../../docs/specification/adr/adr-116.md) D2,
                // [ADR-072](../../../docs/specification/adr/adr-072.md) D4).
                // A name that folds to `"config.json"` is a path that was
                // *computed*, and reading it would make *named in the code*
                // something a reader cannot decide by looking at the line.
                if name == ASSET {
                    return self.asset(args);
                }
                let mut given = Vec::new();
                for arg in args {
                    given.push(self.expr(arg, frame)?);
                }
                self.call(&name, &given)
            }
            _ => Err(Refusal::Unevaluable),
        }
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        frame: &BTreeMap<String, Value>,
    ) -> Result<Value, Refusal> {
        // **`&&` and `||` short-circuit**, which is not an optimisation here: a
        // body may guard a call with one, and evaluating the far side of a
        // guard is how a build-time evaluator reaches what the guard was
        // keeping it away from.
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            let Value::Bool(left) = self.expr(lhs, frame)? else {
                return Err(Refusal::Unevaluable);
            };
            return match (op, left) {
                (BinaryOp::And, false) => Ok(Value::Bool(false)),
                (BinaryOp::Or, true) => Ok(Value::Bool(true)),
                _ => match self.expr(rhs, frame)? {
                    Value::Bool(right) => Ok(Value::Bool(right)),
                    _ => Err(Refusal::Unevaluable),
                },
            };
        }
        let left = self.expr(lhs, frame)?;
        let right = self.expr(rhs, frame)?;
        self.operate(op, left, right)
    }

    /// An operator on two values already in hand.
    ///
    /// **Split out of [`binary`](Self::binary) for `+=`**, whose left side is a
    /// name that is already bound: re-evaluating it as an expression would read
    /// it a second time, which is the same answer here and would stop being one
    /// the moment a left side can call anything.
    fn operate(&self, op: BinaryOp, left: Value, right: Value) -> Result<Value, Refusal> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => match op {
                BinaryOp::Add => a.checked_add(b).map(Value::Int),
                BinaryOp::Sub => a.checked_sub(b).map(Value::Int),
                BinaryOp::Mul => a.checked_mul(b).map(Value::Int),
                BinaryOp::Div => a.checked_div(b).map(Value::Int),
                BinaryOp::Rem => a.checked_rem(b).map(Value::Int),
                BinaryOp::Eq => Some(Value::Bool(a == b)),
                BinaryOp::Ne => Some(Value::Bool(a != b)),
                BinaryOp::Lt => Some(Value::Bool(a < b)),
                BinaryOp::Le => Some(Value::Bool(a <= b)),
                BinaryOp::Gt => Some(Value::Bool(a > b)),
                BinaryOp::Ge => Some(Value::Bool(a >= b)),
                BinaryOp::And | BinaryOp::Or => None,
                // **The bit operators, on the value as it is held**
                // ([ADR-248](../../docs/specification/adr/adr-248.md) D3): an
                // `i128` holds every `u64` and `i64` with the bits they have,
                // and whether the result fits the type it goes into is
                // `NK1116`'s, as for a sum. A count outside the width is not
                // evaluated (D5).
                BinaryOp::BitAnd => Some(Value::Int(a & b)),
                BinaryOp::BitOr => Some(Value::Int(a | b)),
                BinaryOp::BitXor => Some(Value::Int(a ^ b)),
                BinaryOp::Shl => u32::try_from(b)
                    .ok()
                    .and_then(|b| a.checked_shl(b))
                    .map(Value::Int),
                BinaryOp::Shr => u32::try_from(b)
                    .ok()
                    .and_then(|b| a.checked_shr(b))
                    .map(Value::Int),
            }
            .ok_or(Refusal::Unevaluable),
            // **Arithmetic over floats**, which has no `checked_` half: a
            // float does not wrap, it reaches infinity or `NaN` — and what
            // catches that is the spelling, because `float_literal` writes only
            // a finite one. So `1.0 / 0.0` is not silently a `const` of
            // something the source never named; it is a `comptime` this
            // compiler says it cannot evaluate, and a `let` computes it while
            // the program runs.
            (Value::Float(a), Value::Float(b)) => match op {
                BinaryOp::Add => Some(Value::Float(a + b)),
                BinaryOp::Sub => Some(Value::Float(a - b)),
                BinaryOp::Mul => Some(Value::Float(a * b)),
                BinaryOp::Div => Some(Value::Float(a / b)),
                BinaryOp::Rem => Some(Value::Float(a % b)),
                BinaryOp::Eq => Some(Value::Bool(a == b)),
                BinaryOp::Ne => Some(Value::Bool(a != b)),
                BinaryOp::Lt => Some(Value::Bool(a < b)),
                BinaryOp::Le => Some(Value::Bool(a <= b)),
                BinaryOp::Gt => Some(Value::Bool(a > b)),
                BinaryOp::Ge => Some(Value::Bool(a >= b)),
                _ => None,
            }
            .ok_or(Refusal::Unevaluable),
            // **Part I 4.7's `+` over text**, and only that one. A comparison
            // would be a question about the *value* where this holds the
            // written form — see [`Value::Text`].
            (Value::Text(a), Value::Text(b)) => match op {
                BinaryOp::Add => Ok(Value::Text(format!("{a}{b}"))),
                // A comparison of **values**, which is what a decoded text
                // makes answerable: `"\u{0041}" == "A"` is `true` here and at
                // run time, and was refused while this held spellings.
                BinaryOp::Eq => Ok(Value::Bool(a == b)),
                BinaryOp::Ne => Ok(Value::Bool(a != b)),
                _ => Err(Refusal::Unevaluable),
            },
            (Value::Bool(a), Value::Bool(b)) => match op {
                BinaryOp::Eq => Ok(Value::Bool(a == b)),
                BinaryOp::Ne => Ok(Value::Bool(a != b)),
                BinaryOp::And => Ok(Value::Bool(a && b)),
                BinaryOp::Or => Ok(Value::Bool(a || b)),
                _ => Err(Refusal::Unevaluable),
            },
            _ => Err(Refusal::Unevaluable),
        }
    }

    /// **`f"…"` while the program is built** (ADR-035, [ADR-032](../../../docs/specification/adr/adr-032.md)
    /// D3 — a hole is code and every analysis sees it, this one included).
    ///
    /// The holes are split out by the same function the lowering uses, so the
    /// two cannot disagree about where one begins. What is rebuilt is the
    /// **written** form: a number contributes its digits, a `bool` its word,
    /// and text its own written form, which is why nothing has to be escaped
    /// on the way in or out.
    ///
    /// **A format spec is not read.** `f"{n:>8}"` asks for a width, and what
    /// that means is `std::fmt`'s rather than this language's — so it is a
    /// shape this evaluator does not know, not a rule it breaks.
    fn interpolated(
        &mut self,
        literal: &str,
        frame: &BTreeMap<String, Value>,
    ) -> Result<Value, Refusal> {
        let (format, holes) = crate::emit::interpolation(literal).map_err(|_| {
            // A malformed literal is the lowering's refusal and it names the
            // line; saying anything else here would be a second sentence about
            // one mistake.
            Refusal::Unevaluable
        })?;
        let mut values = Vec::with_capacity(holes.len());
        for hole in &holes {
            let expr = crate::parser::parse_expression(&self.parsed.interner, hole)
                .map_err(|_| Refusal::Unevaluable)?;
            values.push(self.expr(&expr, frame)?);
        }

        // **The literal parts are still *written*.** `interpolation` splits
        // the text and does not decode it — a `\` and the character after it
        // are copied through, `\u{…}` braces included — so a chunk is read by
        // [`decoded`] exactly as a plain literal is, and a hole's value is
        // already decoded.
        let mut out = String::new();
        let mut chunk = String::new();
        let mut taken = values.into_iter();
        let mut chars = format.chars().peekable();
        let flush = |chunk: &mut String, out: &mut String| -> Result<(), Refusal> {
            if !chunk.is_empty() {
                out.push_str(&decoded(chunk).ok_or(Refusal::Unevaluable)?);
                chunk.clear();
            }
            Ok(())
        };
        while let Some(c) = chars.next() {
            match c {
                // An escape is two characters and the chunk keeps both, so
                // that `decoded` sees what the source wrote.
                '\\' => {
                    chunk.push('\\');
                    match chars.next() {
                        Some(escape) => {
                            chunk.push(escape);
                            // `\u{…}` carries its braces, and they are not the
                            // format's — `interpolation` copies them through
                            // for exactly this reason.
                            if escape == 'u' && chars.peek() == Some(&'{') {
                                for c in chars.by_ref() {
                                    chunk.push(c);
                                    if c == '}' {
                                        break;
                                    }
                                }
                            }
                        }
                        None => return Err(Refusal::Unevaluable),
                    }
                }
                // `{{` and `}}` are how a format string spells one brace, and
                // text spells it with one.
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    flush(&mut chunk, &mut out)?;
                    out.push('{');
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    flush(&mut chunk, &mut out)?;
                    out.push('}');
                }
                '{' => {
                    // `{}` and nothing else: anything between the braces is a
                    // spec, which this does not read.
                    if chars.next() != Some('}') {
                        return Err(Refusal::Unevaluable);
                    }
                    flush(&mut chunk, &mut out)?;
                    match taken.next() {
                        Some(Value::Int(n)) => out.push_str(&n.to_string()),
                        Some(Value::Bool(yes)) => out.push_str(&yes.to_string()),
                        Some(Value::Text(text)) => out.push_str(&text),
                        _ => return Err(Refusal::Unevaluable),
                    }
                }
                c => chunk.push(c),
            }
        }
        flush(&mut chunk, &mut out)?;
        Ok(Value::Text(out))
    }

    /// **A constant of this file, worked out on demand.**
    ///
    /// The one lookup that makes a constant behave like the item it is. A
    /// function declared below its caller has always been callable — items are
    /// order-independent — and a constant was not, because the walk that binds
    /// them goes down the file. So `comptime A = B * 2` above `comptime B = 21`
    /// was *nothing declares `B`*, which was a **correct program refused** with
    /// a sentence that was not true: the next line declares it.
    ///
    /// **And the ring is refused by name.** `comptime A = B` beside
    /// `comptime B = A` has no base case to reach, so it is not the call depth
    /// that catches it — the stack of names being worked out is, and it can say
    /// which ones.
    fn constant_item(&mut self, name: &str) -> Result<Value, Refusal> {
        if self.resolving.iter().any(|held| held == name) {
            let mut ring = self.resolving.clone();
            ring.push(name.to_string());
            return Err(Refusal::Circular { ring });
        }
        let value = self
            .parsed
            .program
            .items
            .iter()
            .find_map(|item| match &item.node {
                Item::Comptime {
                    name: declared,
                    value,
                    ..
                } if self.parsed.text(*declared) == name => Some(value.clone()),
                _ => None,
            })
            .ok_or(Refusal::Unevaluable)?;

        self.resolving.push(name.to_string());
        let out = self.expr(&value, &BTreeMap::new());
        self.resolving.pop();
        out
    }

    /// **A method this evaluator has no receiver for.**
    ///
    /// Two of them reach a value it holds — a struct's own method, and `len`
    /// and `push` over a list — and everything else is `std`'s or a package's,
    /// whose body is Rust rather than something this reads.
    fn no_method_here(&self, method: winnow_grammar::Symbol) -> Refusal {
        Refusal::NotHere {
            what: format!(".{}()", self.parsed.text(method)),
            why: "at build time the compiler can only call methods your program \
                  declares, and `len` and `push` on a list. Methods from `std` or a \
                  package are compiled Rust, which it can't run",
            // **Not *move it into a function***, which is the trap: the method
            // would be just as unreadable one function further in.
            way_out: "write what it does with arithmetic, `if`, `for` and calls to \
                      functions in this file",
        }
    }
    /// **A grammar, run while the program is built**
    /// (issue #178).
    ///
    /// Not interpreted: the generated parser is compiled and run, so there is
    /// one implementation of the grammar language and Part II 10.2's *the same
    /// syntax and the same meaning* is a tautology rather than a claim.
    /// [`crate::grammar_run`] says why at length.
    ///
    /// **The grammar may be in another file**, which is the same rule a call
    /// across a file boundary already follows: a program's files share one
    /// namespace (Part I 9.1), and each owns the interner its symbols resolve
    /// in — so the parser is compiled from the file that *declared* it.
    fn grammar(&mut self, grammar: &str, rule: &str, input: &str) -> Result<Value, Refusal> {
        let Some((parsed, result)) = self.rule_of(grammar, rule) else {
            return Err(Refusal::Unevaluable);
        };
        let ask = crate::grammar_run::Ask {
            grammar,
            rule,
            input,
            result: &result,
        };
        match self.reads.workshop().run(parsed, &ask) {
            Ok(dump) => crate::grammar_run::decode(&dump).map_err(|why| Refusal::GrammarWall {
                grammar: grammar.to_string(),
                rule: rule.to_string(),
                why,
            }),
            Err(why) => Err(Refusal::GrammarWall {
                grammar: grammar.to_string(),
                rule: rule.to_string(),
                why,
            }),
        }
    }

    /// Whether `ty::variant` names a variant of an `enum` this **program**
    /// declares — this file or one beside it, which is the same reach a call
    /// across a file boundary has (Part I 9.1).
    fn variant_of(&self, ty: &str, variant: &str) -> Option<()> {
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
                            .any(|held| parsed.text(held.name) == variant)
                            .then_some(()),
                        _ => None,
                    })
            })
    }

    /// The file a grammar was declared in, and what its `pub` rule hands back.
    fn rule_of(&self, grammar: &str, rule: &str) -> Option<(&'a Parsed, crate::contracts::ty::Ty)> {
        std::iter::once(self.parsed)
            .chain(self.beside.iter().copied())
            .find_map(|parsed| {
                let found = parsed
                    .program
                    .items
                    .iter()
                    .find_map(|item| match &item.node {
                        Item::Grammar(def) if parsed.text(def.name) == grammar => def
                            .rules
                            .iter()
                            .find(|r| r.is_public && parsed.text(r.name) == rule),
                        _ => None,
                    })?;
                let ty = found.ret_type.as_ref()?;
                Some((parsed, crate::contracts::ty::Ty::from_ast(parsed, ty)))
            })
    }

    /// **The file a build reads**
    /// ([ADR-116](../../../docs/specification/adr/adr-116.md) D2).
    ///
    /// It is the compiler's and not `std`'s, so it is read here rather than
    /// resolved through a ledger: there is no body to describe. What comes back
    /// is the file's **text**, which is what crosses to the program as a `&str`
    /// ([ADR-079](../../../docs/specification/adr/adr-079.md) D1) and what a
    /// grammar's entry takes.
    ///
    /// Every rule [ADR-072](../../../docs/specification/adr/adr-072.md) states
    /// holds unchanged under the call rather than under the keyword it was
    /// written for, and the first of them is the one that costs nothing to
    /// keep: a build given no list reads nothing.
    fn asset(&mut self, args: &[Expr]) -> Result<Value, Refusal> {
        // **A string literal and nothing else** (D4). Not "an expression that
        // evaluates to text": a name that folds to one is exactly what this
        // refuses, and the refusal has to happen before the fold.
        let [Expr::LitStr { text: written, .. }] = args else {
            return Err(Refusal::PathIsComputed);
        };
        let Some(path) = decoded(written) else {
            return Err(Refusal::PathIsComputed);
        };
        match self.reads.read(&path) {
            Ok(text) => Ok(Value::Text(text)),
            Err(why) => Err(Refusal::MayNotRead { path, why }),
        }
    }

    /// A call to a function this unit declares.
    ///
    /// **The permission is read off the ledger**
    /// ([ADR-075](../../../docs/specification/adr/adr-075.md) D1, D2) and not
    /// off a list kept here: `sync` says the body never pauses, and a touch set
    /// that is empty or exactly the build's own parameters says it reaches
    /// nothing else. Both are derived for every function already.
    fn call(&mut self, name: &str, given: &[Value]) -> Result<Value, Refusal> {
        if self.depth >= DEEPEST {
            return Err(Refusal::TooDeep {
                callee: name.to_string(),
            });
        }
        let Some(contract) = self.own.functions.get(name) else {
            // A callee this program does not declare — `std`, a package. Not
            // *you may not*, which would be a claim about somebody else's
            // body; **there is no body here to run**, and for `std` there
            // never will be, because half of it is Rust
            // ([ADR-014](../../../docs/specification/adr/adr-014.md)).
            //
            // Reimplementing one here is the thing issue #178 argues
            // against one construct over: two implementations of one meaning
            // is a promise that becomes a hope.
            return Err(Refusal::NotHere {
                what: name.to_string(),
                why: "it comes from `std` or a package, which is compiled Rust, and at \
                      build time the compiler can only run code written in your program",
                way_out: "write the work in Nikaia, in this file, and call that",
            });
        };
        if !contract.sync.is_sync() {
            return Err(Refusal::NotAllowed {
                callee: name.to_string(),
                because: "It can pause, and nothing run at build time may pause.",
            });
        }
        if !contract.touches_known {
            return Err(Refusal::NotAllowed {
                callee: name.to_string(),
                because: "Nothing says what it touches outside the program.",
            });
        }
        if !contract.touches.iter().all(is_the_builds_own) {
            return Err(Refusal::NotAllowed {
                callee: name.to_string(),
                because: "It touches the world outside the program.",
            });
        }
        let Some((args, body, owner)) = self.body_of(name) else {
            // **No file of this program declares it**, which for a name the
            // ledger describes means a `.contracts` a package shipped: its
            // body was compiled beside this build rather than parsed into it.
            return Err(Refusal::NotHere {
                what: name.to_string(),
                why: "it isn't declared in your program: it comes from a package, which \
                      is compiled Rust, and at build time the compiler can only run code \
                      written in your program",
                way_out: "write the work in Nikaia, in this program, and call that",
            });
        };
        if args.len() != given.len() {
            return Err(Refusal::Unevaluable);
        }
        let mut frame: BTreeMap<String, Value> = args
            .iter()
            .map(|name| name.to_string())
            .zip(given.iter().cloned())
            .collect();
        // **The body is read with the file it came from.** Every
        // `parse_to_ast` builds its own interner, so a symbol from another file
        // resolves to nothing — or to the wrong text — when read with this
        // one's. Swapped for the length of the call and put back after, which
        // is what makes a nested call across two more files right as well.
        let outer = std::mem::replace(&mut self.parsed, owner);
        let elsewhere = self.foreign || !std::ptr::eq(owner, outer);
        let was_foreign = std::mem::replace(&mut self.foreign, elsewhere);
        self.depth += 1;
        let out = self.block(&body, &mut frame);
        self.depth -= 1;
        self.parsed = outer;
        self.foreign = was_foreign;
        // A body that fell off its end has no value, and a `break` or a
        // `continue` outside a loop is not a body this evaluator reads — the
        // checker refuses both long before here, and neither is a value.
        match out? {
            Flow::Value(value) => Ok(value),
            Flow::Fell | Flow::Broke | Flow::Continued => Err(Refusal::Unevaluable),
        }
    }

    /// The parameters and the body of a function this unit declares.
    ///
    /// A **receiver** stops it: a method needs a value to be called on, and a
    /// build-time call by name has none.
    /// The parameters, the body, and **the file the body came from**.
    ///
    /// The third is what makes a call across a file boundary sound: every
    /// `parse_to_ast` builds its own interner, so a symbol from another file
    /// resolves to nothing — or, worse, to the wrong text — when read with this
    /// one's. So the body travels with its `Parsed` and [`Self::call`] reads it
    /// with that.
    ///
    /// **This file first**, which is not an optimisation: the files of a
    /// package share one namespace (Part I 9.1) and the checker has already
    /// refused a duplicate, so the order settles nothing — it just means the
    /// common case never looks further.
    fn body_of(&self, name: &str) -> Option<(Vec<String>, Block, &'a Parsed)> {
        let here = self.parsed;
        self.in_file(here, name)
            .map(|(args, body)| (args, body, here))
            .or_else(|| {
                self.beside.iter().find_map(|parsed| {
                    self.in_file(parsed, name)
                        .map(|(args, body)| (args, body, *parsed))
                })
            })
    }

    fn in_file(&self, parsed: &Parsed, name: &str) -> Option<(Vec<String>, Block)> {
        // `Tag::doubled` is a method's key, and it is the ledger's own — so the
        // split here is the same one `contracts` makes when it writes the
        // entry, and the two cannot drift about which name a call resolves to.
        match name.split_once("::") {
            Some((target, method)) => self.method_of(parsed, target, method),
            None => self.free_body_of(parsed, name),
        }
    }

    fn free_body_of(&self, parsed: &Parsed, name: &str) -> Option<(Vec<String>, Block)> {
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
            if receiver.is_some() || !config.is_empty() {
                return None;
            }
            if parsed.text((*declared)?) != name {
                return None;
            }
            Some((self.parameters(parsed, args), body.clone()))
        })
    }

    /// A method of a `struct` this file declares, by the key a call resolves to.
    ///
    /// **`self` is the first parameter**, which is the shape the call site
    /// builds: the receiver is evaluated before the arguments, because it is
    /// what says *which* method — a `Point`'s `length` and a `Line`'s are two
    /// entries the ledger keys apart by the type.
    ///
    /// A method with **no receiver** is Kap 4.2's constructor and is reached by
    /// its own name (`Stats::new`), so it takes no `self` and is left as the
    /// declaration wrote it.
    fn method_of(
        &self,
        parsed: &Parsed,
        target: &str,
        method: &str,
    ) -> Option<(Vec<String>, Block)> {
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
                if !config.is_empty() || parsed.text((*name)?) != method {
                    return None;
                }
                let mut parameters = match receiver {
                    Some(_) => vec!["self".to_string()],
                    None => Vec::new(),
                };
                parameters.extend(self.parameters(parsed, args));
                Some((parameters, body.clone()))
            })
        })
    }

    fn parameters(&self, parsed: &Parsed, args: &[crate::ast::FnArg]) -> Vec<String> {
        args.iter()
            .map(|arg| parsed.text(arg.name).to_string())
            .collect()
    }

    /// A body: `let`s, an early `return`, a loop, and a last statement that is
    /// the value — which is Part I 3.1's rule for every block, read here.
    ///
    /// **The frame is by reference now, and a loop is why.** A body used to get
    /// a fresh map it owned; a `for` has to see what its body assigned on the
    /// last turn, and a `while` has to see what its condition reads.
    fn block(
        &mut self,
        block: &Block,
        frame: &mut BTreeMap<String, Value>,
    ) -> Result<Flow, Refusal> {
        let last = block.stmts.len().saturating_sub(1);
        for (at, stmt) in block.stmts.iter().enumerate() {
            match &stmt.node {
                Stmt::Let { names, value, .. } => {
                    let [name] = names.as_slice() else {
                        return Err(Refusal::Unevaluable);
                    };
                    let value = self.expr(value, frame)?;
                    frame.insert(self.parsed.text(*name).to_string(), value);
                }
                // A `comptime` inside a body is a `let` that must fold
                // ([ADR-073](../../../docs/specification/adr/adr-073.md) D2),
                // and inside a build-time body everything must, so the two are
                // the same statement here.
                Stmt::Comptime { name, value, .. } => {
                    let value = self.expr(value, frame)?;
                    frame.insert(self.parsed.text(*name).to_string(), value);
                }
                // **`xs[i] = …`, which is how a build-time table is filled.**
                // Before the name, because an index is a target this evaluator
                // reads and `Expr::Variable` is not what it looks like.
                Stmt::Assign {
                    target: Expr::Index { base, index },
                    op,
                    value,
                } => {
                    let Expr::Variable(name) = base.as_ref() else {
                        return Err(Refusal::Unevaluable);
                    };
                    let name = self.parsed.text(*name).to_string();
                    let at = self.expr(index, frame)?;
                    let given = self.expr(value, frame)?;
                    let held = frame.get(&name).ok_or(Refusal::Unevaluable)?;
                    let next = match op {
                        None => given,
                        Some(op) => {
                            let before = element(held, &at)?.clone();
                            self.operate(*op, before, given)?
                        }
                    };
                    let Value::List(items) = frame.get_mut(&name).ok_or(Refusal::Unevaluable)?
                    else {
                        return Err(Refusal::Unevaluable);
                    };
                    let Value::Int(at) = at else {
                        return Err(Refusal::Unevaluable);
                    };
                    let len = items.len();
                    let slot = usize::try_from(at)
                        .ok()
                        .and_then(|at| items.get_mut(at))
                        .ok_or(Refusal::OutOfBounds { at, len })?;
                    *slot = next;
                }
                Stmt::Assign { target, op, value } => {
                    let Expr::Variable(name) = target else {
                        return Err(Refusal::Unevaluable);
                    };
                    let name = self.parsed.text(*name).to_string();
                    let given = self.expr(value, frame)?;
                    let next = match op {
                        None => given,
                        Some(op) => {
                            let held = frame.get(&name).cloned().ok_or(Refusal::Unevaluable)?;
                            self.operate(*op, held, given)?
                        }
                    };
                    if !frame.contains_key(&name) {
                        return Err(Refusal::Unevaluable);
                    }
                    frame.insert(name, next);
                }
                Stmt::Return(Some(value)) => return Ok(Flow::Value(self.expr(value, frame)?)),
                Stmt::Break => return Ok(Flow::Broke),
                Stmt::Continue => return Ok(Flow::Continued),
                Stmt::For {
                    bindings,
                    iter,
                    body,
                } => {
                    let flow = self.walk(bindings, iter, body, frame)?;
                    if let Flow::Value(_) = flow {
                        return Ok(flow);
                    }
                }
                Stmt::While { cond, body } => {
                    // **No step budget**
                    // ([ADR-075](../../../docs/specification/adr/adr-075.md)
                    // D4), deliberately and with the cost written down: a
                    // `while` that does not end hangs the build. The call-depth
                    // limit above is not this and does not become it.
                    loop {
                        match self.expr(cond, frame)? {
                            Value::Bool(true) => {}
                            Value::Bool(false) => break,
                            _ => return Err(Refusal::Unevaluable),
                        }
                        match self.block(body, frame)? {
                            Flow::Value(value) => return Ok(Flow::Value(value)),
                            Flow::Broke => break,
                            Flow::Fell | Flow::Continued => {}
                        }
                    }
                }
                // **An `if` is read as a statement here whatever its
                // position**, which is what a loop's body needs: `if i > n
                // { break }` is the last statement of its block and hands back
                // no value at all. A branch that falls through carries on; one
                // that returns, breaks or continues ends the block.
                Stmt::Expr(Expr::If {
                    cond,
                    then_branch,
                    else_branch,
                }) => {
                    let taken = match self.expr(cond, frame)? {
                        Value::Bool(true) => Some(then_branch),
                        Value::Bool(false) => else_branch.as_ref(),
                        _ => return Err(Refusal::Unevaluable),
                    };
                    if let Some(block) = taken {
                        match self.block(block, frame)? {
                            Flow::Fell => {}
                            other => return Ok(other),
                        }
                    }
                }
                // **`xs.push(v)`, which is how a table is *grown* rather than
                // filled** — [ADR-079](../../../docs/specification/adr/adr-079.md)'s
                // own title, *growable going in, fixed coming out*. The body
                // works with a list that does not know its length yet; what
                // crosses into the program is fixed, and the declaration is
                // where it becomes so.
                //
                // A statement and not an expression, because that is what it
                // is: `Vec::push` hands back nothing, and a body that reads
                // its result is not one this evaluator sees.
                Stmt::Expr(Expr::MethodCall {
                    receiver,
                    method,
                    args,
                    config,
                }) if self.parsed.text(*method) == "push"
                    && args.len() == 1
                    && config.is_empty() =>
                {
                    let Expr::Variable(name) = receiver.as_ref() else {
                        return Err(Refusal::Unevaluable);
                    };
                    let name = self.parsed.text(*name).to_string();
                    let given = self.expr(&args[0], frame)?;
                    let Some(Value::List(items)) = frame.get_mut(&name) else {
                        return Err(Refusal::Unevaluable);
                    };
                    items.push(given);
                }
                Stmt::Expr(expr) if at == last => return Ok(Flow::Value(self.expr(expr, frame)?)),
                _ => return Err(Refusal::Unevaluable),
            }
        }
        Ok(Flow::Fell)
    }

    /// A `for` over a range, which is the one shape a build-time loop has: a
    /// list needs a value this evaluator does not carry yet, and that is
    /// issue #178's next step rather than this one's.
    fn walk(
        &mut self,
        bindings: &[winnow_grammar::Symbol],
        iter: &Expr,
        body: &Block,
        frame: &mut BTreeMap<String, Value>,
    ) -> Result<Flow, Refusal> {
        let [binding] = bindings else {
            return Err(Refusal::Unevaluable);
        };
        let Expr::Range {
            start,
            end,
            inclusive,
        } = iter
        else {
            return Err(Refusal::Unevaluable);
        };
        let (Value::Int(start), Value::Int(end)) =
            (self.expr(start, frame)?, self.expr(end, frame)?)
        else {
            return Err(Refusal::Unevaluable);
        };
        let name = self.parsed.text(*binding).to_string();
        let mut at = start;
        while if *inclusive { at <= end } else { at < end } {
            frame.insert(name.clone(), Value::Int(at));
            match self.block(body, frame)? {
                Flow::Value(value) => return Ok(Flow::Value(value)),
                Flow::Broke => break,
                Flow::Fell | Flow::Continued => {}
            }
            at = at.checked_add(1).ok_or(Refusal::Unevaluable)?;
        }
        // **The binding does not outlive the loop**, which is Part I 3.3's rule
        // and matters here because the frame is shared: a name the loop bound
        // must not be readable after it.
        frame.remove(&name);
        Ok(Flow::Fell)
    }
}

/// One element of a list value, by an index that is a value.
///
/// **An index this array does not have is a refusal and not an
/// `Unevaluable`** — see [`Refusal::OutOfBounds`]. A receiver that is not a
/// list, or an index that is not a number, is a shape this evaluator does not
/// read, which is the other thing entirely.
fn element<'v>(on: &'v Value, at: &Value) -> Result<&'v Value, Refusal> {
    let (Value::List(items), Value::Int(at)) = (on, at) else {
        return Err(Refusal::Unevaluable);
    };
    usize::try_from(*at)
        .ok()
        .and_then(|index| items.get(index))
        .ok_or(Refusal::OutOfBounds {
            at: *at,
            len: items.len(),
        })
}

/// One escape, read off the characters after the `\`, or what is wrong with it.
///
/// **The one table, with two readers**
/// ([ADR-188](../../docs/specification/adr/adr-188.md) D2): [`decoded`] turns a
/// literal into a value at build time, and `Checker::an_escape_nothing_names`
/// refuses a literal the set does not cover. Two walks would be two sets, and
/// the second would drift into refusing something the first accepts - which is
/// a correct program refused, the one thing this compiler may never do
/// ([Part III C.4](../../docs/specification/30-nikaia-tooling.md)).
///
/// The set is **Rust's**, and that is describing rather than deciding
/// ([ADR-188](../../docs/specification/adr/adr-188.md) D1): a `.nika` literal
/// is written into the generated file verbatim, so what a `\` means is already
/// the language below's answer. This reads it back out so that a refusal can be
/// in this compiler's words.
fn one_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<char, Refused> {
    let mut written = String::from("\\");
    let Some(c) = chars.next() else {
        return Err(Refused {
            written,
            why: "a `\\` at the end of a literal escapes nothing",
        });
    };
    written.push(c);
    match c {
        'n' => Ok('\n'),
        'r' => Ok('\r'),
        't' => Ok('\t'),
        '0' => Ok('\0'),
        '\\' => Ok('\\'),
        '\'' => Ok('\''),
        '"' => Ok('"'),
        // `\x41`, which Rust limits to the ASCII range inside a string.
        'x' => {
            for _ in 0..2 {
                match chars.next() {
                    Some(digit) => written.push(digit),
                    None => {
                        return Err(Refused {
                            written,
                            why: "`\\x` takes exactly two hexadecimal digits",
                        });
                    }
                }
            }
            let digits = &written[2..];
            match u8::from_str_radix(digits, 16).ok().filter(u8::is_ascii) {
                Some(byte) => Ok(char::from(byte)),
                None => Err(Refused {
                    written,
                    why: "`\\x` takes two hexadecimal digits naming a byte below `\\x80`",
                }),
            }
        }
        // `\u{…}`, up to six digits.
        'u' => {
            if chars.peek() != Some(&'{') {
                return Err(Refused {
                    written,
                    why: "`\\u` takes its digits in braces, as `\\u{1F600}`",
                });
            }
            written.push(chars.next().expect("peeked"));
            loop {
                match chars.next() {
                    Some('}') => {
                        written.push('}');
                        break;
                    }
                    Some(digit) => written.push(digit),
                    None => {
                        return Err(Refused {
                            written,
                            why: "`\\u{` is never closed",
                        });
                    }
                }
            }
            let digits = &written[3..written.len() - 1];
            match u32::from_str_radix(digits, 16)
                .ok()
                .and_then(char::from_u32)
            {
                Some(c) => Ok(c),
                None => Err(Refused {
                    written,
                    why: "`\\u{…}` takes up to six hexadecimal digits naming a character",
                }),
            }
        }
        _ => Err(Refused {
            written,
            why: "no escape starts with that character",
        }),
    }
}

/// An escape the set does not name, as it is written, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// The escape exactly as the literal writes it, `\` and all.
    pub written: String,
    /// One clause, said in this compiler's words rather than the backend's.
    pub why: &'static str,
}

/// The escapes this language has, as a reader of a diagnostic wants them
/// ([Part I 2.5](../../docs/specification/10-nikaia-light.md)).
///
/// A list and not a sentence, because the message prints it and the page prints
/// it, and a set written twice is a set that disagrees with itself.
pub const ESCAPES: &str = "\\n \\r \\t \\0 \\\\ \\' \\\" \\xNN \\u{…}";

/// The first escape in a literal that this language's set does not name.
///
/// **Reads the same table [`decoded`] does**, which is what makes a refusal
/// here safe: everything this returns `Some` for is something `decoded`
/// returns `None` for, and `rustc` refuses in its own words.
pub fn an_escape_nothing_names(literal: &str) -> Option<Refused> {
    let mut chars = literal.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            continue;
        }
        if let Err(refused) = one_escape(&mut chars) {
            return Some(refused);
        }
    }
    None
}

/// **What a written string literal means** — the value behind the spelling.
///
/// The parser keeps a literal's escapes (`STR_CHAR` takes `\` and any
/// character) and the emitter hands the text to `rustc` unchanged, so **this
/// language's escapes are Rust's**, decided by what that compiler accepts
/// rather than by a page here. Measured: `println("a\qb")` is *unknown
/// character escape: `q`* — on the `.nika` line, which is
/// [ADR-012](../../../docs/specification/adr/adr-012.md)'s source map working,
/// and in `rustc`'s vocabulary, which is issue #197's.
///
/// So this is a **faithful reading** and not a second definition. `None` where
/// the escape is one `rustc` would reject: that program does not compile either
/// way, and the evaluator says *cannot evaluate* rather than inventing a
/// meaning for it.
///
/// [`written`] is the inverse, and `crates/nikaia/tests/build_time.rs` holds
/// the pair to the only standard that settles it: the same literal, read at
/// build time and at run time, printing the same bytes.
pub fn decoded(literal: &str) -> Option<String> {
    let mut out = String::with_capacity(literal.len());
    let mut chars = literal.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        // **The same walk the refusal reads** (ADR-188 D2). This used to hold
        // its own copy of the table, and a second copy is a second set: the
        // `None` here and the refusal there would drift, and the one that
        // drifted open would refuse a literal this one decodes.
        out.push(one_escape(&mut chars).ok()?);
    }
    Some(out)
}

/// The inverse of [`decoded`]: a value, spelled as a literal `rustc` reads.
///
/// **Total rather than minimal.** Every control character is written as an
/// escape rather than passed through, because a literal is going into a
/// generated file that a person may open: a bell in the middle of a `const` is
/// the sort of thing that makes a reader doubt the file rather than the value.
pub fn written(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Whether a touch is the build's own parameters
/// ([ADR-075](../../../docs/specification/adr/adr-075.md) D2).
///
/// **`cli::args` passes, deliberately**: same parameters, same code. A build's
/// own arguments are an input like its source is, and a body that shapes a
/// table differently for two declared parameters is doing the thing `comptime`
/// is for.
fn is_the_builds_own(touch: &touch::Touch) -> bool {
    touch.text() == "args read"
}
