//! Which parameters a body **keeps**
//! ([ADR-094](../../../docs/specification/adr/adr-094.md) D2), the first of
//! that record's five steps.
//!
//! The column alone: inferred, recorded, and read by nothing. Every call site
//! in the repository still writes its own `&`, and the point of landing this
//! half on its own is that the answer can be diffed against the corpus before
//! one of them changes.
//!
//! **Half of these tests are about the claim being withheld**, because the
//! polarity is the whole design. `keeps` is a restriction, so doubt adds it —
//! the opposite of `sync`, which is a promise and is taken away on doubt. An
//! analysis that answered *keeps* everywhere would be safe and useless, and
//! nothing in the column itself says which of the two it is; only a test that a
//! read stays a read does.

use nikaia::contracts::Ledger;
use nikaia::parser::parse_to_ast;

/// What the ledger says one function keeps, after the inference.
fn keeps(source: &str, name: &str) -> Vec<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let ledger = Ledger::infer(&parsed);
    ledger.functions[name].keeps.clone()
}

/// **A parameter that is only looked at is lent**, which is the answer the
/// whole record exists for: `page(entries)` rather than `page(&entries)`.
#[test]
fn a_parameter_that_is_only_read_is_not_kept() {
    assert!(
        keeps(
            "fn width(text: String) -> i64 { return text.len() as i64 }",
            "width"
        )
        .is_empty()
    );

    // A field read is a read of the field and not of the parameter — **where
    // the field copies**. An `i64` handed back takes nothing away, and owning
    // the parameter for it would take the value from a caller that still wants
    // it. A field that *moves* is the test at the bottom of this file.
    assert!(
        keeps(
            "struct Row { total: i64 }\n\
         fn of(row: Row) -> i64 { return row.total }",
            "of"
        )
        .is_empty()
    );
}

/// **Handed back by value, it is kept** — the value outlives the call by
/// definition.
#[test]
fn a_parameter_returned_by_value_is_kept() {
    assert_eq!(
        keeps("fn label(text: String) -> String { return text }", "label"),
        ["text"]
    );
}

/// **And handed back as a view, it is not.** `-> &str` points into the
/// argument rather than moving it, which is what `returns = "borrows(…)"`
/// already records; `-> String` is the one that takes it away.
#[test]
fn a_parameter_returned_as_a_view_is_not_kept() {
    let source = "fn same(text: ref String) -> ref String { return text }";
    assert!(keeps(source, "same").is_empty());

    let parsed = parse_to_ast(source).expect("the source parses");
    let ledger = Ledger::infer(&parsed);
    assert_eq!(
        ledger.functions["same"].borrows,
        ["text"],
        "the view is recorded where views are recorded"
    );
}

/// **Put into a struct, it is kept.** Where that struct then goes is not this
/// expression's question — it outlives the call either way.
#[test]
fn a_parameter_put_into_a_struct_is_kept() {
    assert_eq!(
        keeps(
            "struct Row { name: String }\n\
             fn wrap(name: String) -> Row { return Row { name: name } }",
            "wrap"
        ),
        ["name"]
    );

    // `Row { name }` is the field and the name in one, and the name may be a
    // parameter — the shorthand must not be a hole in the walk.
    assert_eq!(
        keeps(
            "struct Row { name: String }\n\
             fn wrap(name: String) -> Row { return Row { name } }",
            "wrap"
        ),
        ["name"]
    );
}

/// **Assigned into a place, it is kept**, whatever the place is.
#[test]
fn a_parameter_assigned_into_a_place_is_kept() {
    assert_eq!(
        keeps(
            "struct Stats { min: i64 }\n\
             impl Stats {\n\
             \x20   fn add(ref mut self, temp: i64) { self.min = temp }\n\
             }",
            "Stats::add"
        ),
        ["temp"]
    );
}

/// **And it travels up the call graph**, which is what makes the column an
/// answer about a *program* rather than about one body: `shout` keeps `text`
/// only because `label` does.
#[test]
fn keeping_travels_up_the_call_graph() {
    let source = "fn label(text: String) -> String { return text }\n\
                  fn shout(text: String) -> String { return label(text) }\n\
                  fn width(text: String) -> i64 { return text.len() as i64 }\n\
                  fn measure(text: String) -> i64 { return width(text) }";

    assert_eq!(keeps(source, "shout"), ["text"]);
    // And a callee that only reads leaves its caller lending, which is the half
    // that says the walk is not simply answering yes.
    assert!(keeps(source, "measure").is_empty());
}

/// **Two functions that pass a parameter neither stores keep neither.**
///
/// The least fixpoint, in the one shape that tells it from a greatest one.
/// `sync` starts from *everything is sync* and takes the claim away, so mutual
/// recursion keeps it; this starts from *nothing is kept* and adds, so mutual
/// recursion keeps nothing. Both are right, and they are right for opposite
/// reasons.
#[test]
fn mutual_recursion_that_only_reads_keeps_nothing() {
    let source = "fn ping(text: String) -> i64 {\n\
                  \x20   if text.len() == 0 { return 0 }\n\
                  \x20   return pong(text)\n\
                  }\n\
                  fn pong(text: String) -> i64 { return ping(text) }";

    assert!(
        keeps(source, "ping").is_empty(),
        "{:?}",
        keeps(source, "ping")
    );
    assert!(keeps(source, "pong").is_empty());
}

/// **A task keeps what it names** ([ADR-040](../../../docs/specification/adr/adr-040.md)
/// D1): its body may outlive the statement, so it takes what it names by value.
#[test]
fn a_parameter_a_task_names_is_kept() {
    assert_eq!(
        keeps(
            "fn start(text: String, n: i64) -> i64 {\n\
             \x20   let h = spawn fn { println(f\"{text}\") }\n\
             \x20   return n + 1\n\
             }",
            "start"
        ),
        ["text"],
        "the one the task names, and not the one beside it"
    );

    // **And `return n` would keep `n`**, which is not a quirk of copy types
    // being missed — it is the rule being applied: the value leaves the call.
    // What a *copy* type costs when it is kept is the emitter's question and is
    // step 3's; this column says what the body does.
    assert_eq!(
        keeps(
            "fn start(text: String, n: i64) -> i64 {\n\
             \x20   let h = spawn fn { println(f\"{text}\") }\n\
             \x20   return n\n\
             }",
            "start"
        ),
        ["n", "text"]
    );
}

/// **A callee nothing describes keeps.** D2's fail-closed case, and the reason
/// the polarity is written down: the wrong answer this way costs a caller an
/// owned argument, which is where every caller already is, and the wrong answer
/// the other way is a `&T` parameter whose body moves the value — `rustc`'s
/// error about a file nobody wrote.
#[test]
fn a_call_nothing_describes_keeps_what_it_is_given() {
    assert_eq!(
        keeps(
            "struct Sink { n: i64 }\n\
             fn hand(text: String, sink: Sink) { sink.swallow(text) }",
            "hand"
        ),
        ["sink", "text"],
        "a method no ledger has an entry for is not one to assume about — and \
         that goes for the receiver as much as for the argument"
    );
}

/// **A receiver a method takes by value is moved out of**, so a parameter
/// standing there is kept.
///
/// And which entry the call goes to is the type checker's answer
/// ([ADR-028](../../../docs/specification/adr/adr-028.md)), so where **any**
/// method call in a body went to an entry no ledger has, the candidate list is
/// not the whole list and may not be believed. `crates/nikaia/tests/lambdas.rs`
/// is where that was met: its `Account::access` is a Rust stand-in taking
/// `self`, and the two `::access` entries `std` carries both take `&self`.
#[test]
fn a_receiver_is_kept_where_the_method_might_consume_it() {
    // Every method here resolves, and every one of them reads its receiver.
    assert!(
        keeps(
            "fn width(text: String) -> i64 { return text.len() as i64 }",
            "width"
        )
        .is_empty()
    );

    // One unresolvable call in the body, and every receiver in it is kept —
    // including the one whose own method resolves perfectly well.
    assert_eq!(
        keeps(
            "struct Sink { n: i64 }\n\
             fn hand(text: String, sink: Sink) -> i64 {\n\
             \x20   sink.swallow()\n\
             \x20   return text.len() as i64\n\
             }",
            "hand"
        ),
        ["sink", "text"]
    );
}

/// **`std` says which of its own keep**, and the answer is six entries out of
/// ninety-four.
///
/// That ratio is the column's whole value. An absent `keeps` on a *present*
/// entry means *keeps nothing* — `std.contracts`' own convention for `sync`,
/// said once more — so a caller that hands a name to `str::len` may lend it. A
/// file that recorded nothing and made every caller assume the worst would be
/// safe and would leave the language exactly where it is.
#[test]
fn std_says_which_of_its_own_functions_keep() {
    let source = "struct Row { n: i64 }\n\
                  fn collect(rows: Vec[Row], row: Row) -> Vec[Row] {\n\
                  \x20   rows.push(row)\n\
                  \x20   return rows\n\
                  }\n\
                  fn sized(rows: Vec[Row]) -> i64 { return rows.len() as i64 }";

    assert_eq!(keeps(source, "collect"), ["row", "rows"]);
    assert!(
        keeps(source, "sized").is_empty(),
        "reading a length is reading"
    );
}

// ---------------------------------------------------------------------------
// A grammar's entry ([ADR-186](../../../docs/specification/adr/adr-186.md))
// ---------------------------------------------------------------------------

/// The parse [ADR-186](../../../docs/specification/adr/adr-186.md) was written
/// about, and the caller whose columns it took away.
const STOCK: &str = "grammar Stock {\n\
                     \x20   rule FIELD -> ref String = s:until(\";\" | frame_end) { s }\n\
                     \x20   rule COUNT -> i64 = n:dec[i64](digit+) { n }\n\
                     \x20   @frame(boundary: \"\\n\")\n\
                     \x20   rule ENTRY -> Entry =\n\
                     \x20       category:FIELD \";\" => count:COUNT frame_end\n\
                     \x20       { Entry { category, count } }\n\
                     \x20   pub rule file -> Vec[Entry] = entries:ENTRY* { entries }\n\
                     }\n\
                     \n\
                     pub struct Entry { pub category: ref String, pub count: i64 }\n\
                     \n\
                     pub fn read(data: ref String) -> Vec[Entry] throws {\n\
                     \x20   return Stock::file(data)\n\
                     }\n";

/// **A parse keeps the text it hands back views into**, which is the answer
/// `keeps` never had for an entry
/// ([ADR-186](../../../docs/specification/adr/adr-186.md) D1).
#[test]
fn a_parse_keeps_the_text_its_record_views() {
    assert_eq!(keeps(STOCK, "Stock::file"), ["input"]);
}

/// **And the caller gets its column back.** This is
/// [ADR-186](../../../docs/specification/adr/adr-186.md) §1's own reproduction
/// read forwards: `read`'s body is one `Stock::file(data)`,
/// and before the entry answered, an absent `keeps` on a present entry read as
/// *keeps nothing* — so the one caller of the parse was told it could lend text
/// the parse holds views into.
#[test]
fn a_caller_of_a_parse_keeps_what_it_hands_over() {
    assert_eq!(keeps(STOCK, "read"), ["data"]);
}

/// **A parse that hands back a number keeps nothing.** The empty column means
/// *asked and no* now, where before it meant *nobody derived it*, and the two
/// were written the same way.
#[test]
fn a_parse_that_views_nothing_keeps_nothing() {
    let source = "grammar Calc {\n\
                  \x20   rule NUM -> i64 = n:dec[i64](digit+) { n }\n\
                  \x20   pub rule expr -> i64 = n:NUM { n }\n\
                  }\n";
    assert!(keeps(source, "Calc::expr").is_empty());
}

/// **A result this walk did not read a declaration for keeps the text**, which
/// is this file's polarity and not the tether's: a type it cannot see into is
/// one it cannot rule a view out of, and the cost of being wrong that way is a
/// caller that hands the text over where it could have lent it.
#[test]
fn a_parse_whose_record_this_walk_cannot_see_keeps_the_text() {
    let source = "grammar Wire {\n\
                  \x20   rule NUM -> i64 = n:dec[i64](digit+) { n }\n\
                  \x20   pub rule frame -> Vec[Packet] = n:NUM { [] }\n\
                  }\n";
    assert_eq!(keeps(source, "Wire::frame"), ["input"]);
}

/// The column survives the round trip, which `--locked` needs: it compares
/// bytes, so a column that rendered differently than it parsed would fail a
/// build that changed nothing.
#[test]
fn the_column_renders_and_parses_back() {
    let parsed = parse_to_ast(
        "struct Row { name: String }\n\
         fn wrap(name: String) -> Row { return Row { name: name } }",
    )
    .expect("the source parses");
    let ledger = Ledger::infer(&parsed);
    let rendered = ledger.render();
    assert!(rendered.contains("keeps = [\"name\"]"), "{rendered}");

    let read = Ledger::parse(&rendered).expect("its own output parses");
    assert_eq!(read.functions["wrap"].keeps, ["name"]);
    assert_eq!(read.render(), rendered);

    // And a function that keeps nothing writes no line, so no existing ledger
    // grows a column of falses.
    let plain = parse_to_ast("fn width(text: String) -> i64 { return text.len() as i64 }")
        .expect("the source parses");
    assert!(!Ledger::infer(&plain).render().contains("keeps"));
}

// --- A field handed out of a lent parameter ---

/// **`return answer.text` keeps `answer`**
/// ([ADR-094](../../../docs/specification/adr/adr-094.md) D1,
/// [`open-work.md`](../../../docs/open-work.md) §1.11).
///
/// It did not, and four lines were enough to see it: the parameter stayed
/// **lent**, the declaration was written `&Answer`, and the body took a piece
/// out of a loan — *cannot move out of `answer.text` which is behind a shared
/// reference*, about a file nobody wrote ([Part III
/// C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// `hand_over` read a bare name, so `return answer` kept and `return
/// answer.text` did not.
#[test]
fn a_field_that_moves_keeps_the_parameter_it_came_out_of() {
    let source = "struct Answer { text: String }\n\
                  fn say(answer: Answer) -> String { return answer.text }";
    assert_eq!(keeps(source, "say"), ["answer"]);
}

/// **And `self` is not this rule's**, deliberately: a `ref self` is a word the
/// author wrote, so what a method does with its subject may not silently turn it
/// into `fn(self)`. That shape is `NK1131`, which names `.clone()` and
/// `fn …(self)` as the two ways out — and this test is what would notice the
/// widening reaching it.
#[test]
fn a_field_of_a_borrowed_subject_does_not_keep_it() {
    let source = "struct Answer { text: String }\n\
                  impl Answer {\n\
                  \x20   fn say(ref self) -> String { return self.text }\n\
                  }";
    assert!(
        keeps(source, "Answer::say").is_empty(),
        "{:?}",
        keeps(source, "Answer::say")
    );
}

/// **A field of a parameter whose type this file does not declare keeps**, which
/// is the polarity the whole column has: `keeps_its` reads an unknown callee as
/// one that keeps, and an unknown field is the same question one level in. The
/// cost of guessing the other way is `rustc` about a file nobody wrote; the cost
/// of guessing this way is one value the caller has to `.clone()`.
#[test]
fn a_field_of_a_type_this_file_does_not_declare_keeps() {
    let source = "fn first(m: Mapped) -> String { return m.text }";
    assert_eq!(keeps(source, "first"), ["m"]);
}

/// **Every way a value leaves is a return** ([ADR-207](../../../docs/specification/adr/adr-207.md)
/// D4): the body's last expression, an `if`'s or a `match`'s arm, and a name
/// a `let` bound to the parameter. Each of these was read as lent, and each
/// came out as `rustc`'s *mismatched types* about a file nobody wrote.
#[test]
fn a_parameter_that_leaves_by_any_way_out_is_kept() {
    assert_eq!(
        keeps("fn same(name: String) -> String { name }", "same"),
        ["name"]
    );
    assert_eq!(
        keeps(
            "fn or(c: bool, name: String) -> String { if c { name } else { f\"x\" } }",
            "or"
        ),
        ["name"]
    );
    // `n` is the scrutinee, and what the column says of a number does not
    // matter: a copy is never lent (`keeps::lends` asks `moves` first).
    assert!(
        keeps(
            "fn pick(n: i64, name: String) -> String {\n\
             let s = match n { 0 => f\"zero\" else => name }\n\
             return s\n\
         }",
            "pick"
        )
        .contains(&"name".to_string())
    );
}

/// **And a `let` that only reads still keeps nothing**: the second name has to
/// leave for the first to be kept.
#[test]
fn a_parameter_bound_again_and_only_read_is_not_kept() {
    assert!(
        keeps(
            "fn width(name: String) -> i64 { let s = name\n return s.len() as i64 }",
            "width"
        )
        .is_empty()
    );
}

/// **A receiver the author lent is not kept** (`open-work.md` §1.29, found in
/// the ledger `tools/template.nika` lowers to). A `match` on it, a method that
/// changes it and a call of itself are what a `ref mut self` is written for;
/// each was recorded `keeps = ["self"]`, and a reader of the contract saw the
/// subject of a borrowing method taken.
#[test]
fn a_receiver_written_ref_is_not_kept() {
    let source = "enum Position { Text, Url }\n\
                  struct Scan { tail: Vec[char], at: i64 }\n\
                  impl Position {\n\
                  \x20   fn escapable(ref self) -> bool {\n\
                  \x20       return match self { Position::Text => true\n Position::Url => false }\n\
                  \x20   }\n\
                  }\n\
                  impl Scan {\n\
                  \x20   fn step(ref mut self) { self.at += 1 }\n\
                  \x20   fn walk(ref mut self, n: i64) {\n\
                  \x20       if n > 0 {\n self.step()\n self.walk(n - 1)\n }\n\
                  \x20   }\n\
                  }";
    for name in ["Position::escapable", "Scan::step", "Scan::walk"] {
        assert!(
            keeps(source, name).is_empty(),
            "{name}: {:?}",
            keeps(source, name)
        );
    }
}

/// **An argument the author lent is not kept by a `match` over it**: it is a
/// reference in the Rust whatever the column says, so the over-approximation
/// the `match` arm makes for an owned parameter buys nothing here.
#[test]
fn a_lent_argument_a_match_takes_apart_is_not_kept() {
    let source = "enum Position { Text, Url }\n\
                  fn escapable(p: ref Position) -> bool {\n\
                  \x20   return match p { Position::Text => true\n Position::Url => false }\n\
                  }";
    assert!(
        keeps(source, "escapable").is_empty(),
        "{:?}",
        keeps(source, "escapable")
    );
}

/// **A receiver taken by value that copies is not moved out of**:
/// `c.to_ascii_lowercase()` leaves the caller its `char`. Recorded as kept, it
/// said `Scan::feed(ref mut self, c: char)` took the character.
#[test]
fn a_copied_receiver_a_method_takes_by_value_is_not_kept() {
    let source = "fn lower(c: char) -> bool { return c.to_ascii_lowercase() == 'a' }";
    assert!(
        keeps(source, "lower").is_empty(),
        "{:?}",
        keeps(source, "lower")
    );
}

/// **A method call reaches methods only**: `text.lines()` is `str::lines`,
/// never `io::lines()`, which takes no `self`. Counted as a candidate, the free
/// function read as a method taking its receiver whole, and every
/// `text.lines()` over a lent parameter kept it (found at 0.0.258, moving the
/// ledger's reader to Nikaia).
#[test]
fn a_method_name_a_free_function_shares_does_not_keep_the_receiver() {
    let source = "pub fn count(text: ref String) -> i64 {\n\
                  \x20   let mut n = 0\n\
                  \x20   for line in text.lines() {\n n += line.len()\n }\n\
                  \x20   return n\n\
                  }";
    assert!(
        keeps(source, "count").is_empty(),
        "{:?}",
        keeps(source, "count")
    );
}

/// **A parameter written `mut` is changed in place and not kept**
/// ([ADR-094](../../../docs/specification/adr/adr-094.md) D3): it is a `&mut`
/// by the author's word, and `entries.push(…)` is what it is written for.
#[test]
fn a_parameter_written_mut_is_not_kept_for_being_changed() {
    let source = "fn take(entry: ref String, mut entries: Vec[String]) {\n\
                  \x20   entries.push(entry.clone())\n\
                  }";
    assert!(
        keeps(source, "take").is_empty(),
        "{:?}",
        keeps(source, "take")
    );
}
