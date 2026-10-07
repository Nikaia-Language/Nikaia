//! **How far a compiler written in Nikaia gets**, kept as a test.
//!
//! A miniature compiler - a grammar that parses `let` lines of arithmetic, a
//! syntax tree that holds itself, a check for names nothing declares, and Rust
//! written out as text - was written at 0.0.231 to measure what stood between
//! this language and a compiler written in it. It needed three workarounds
//! then: its tree held its children in a list, its output text had to be
//! annotated, and it read a number literal with a loop over `digit_value`.
//! Each was a gap, and each is closed: a type holds itself (ADR-246), a `let mut`
//! of a literal the body grows owns its text (0.0.232), and `std` reads a number
//! (`text::parse_i64`, 0.0.237).
//!
//! **What this is for is the next gap.** The program, now in
//! `tests/language/src/self_hosting.nika` with the other behaviour tests, is
//! written the way a compiler would be, without a workaround in it; a change
//! that makes it stop building or compute something else is a step back on
//! that road, and a new construct a compiler needs goes in there first. Here
//! stay the pieces of the compiler that are Nikaia, called from Rust.

/// **The first piece of the compiler written in Nikaia** (0.0.238):
/// `std/tools/spelling.nika`, lowered ahead of time and called from the
/// checker's and `dsl`'s *did you mean* as ordinary Rust. The Rust it
/// replaced is gone, so these cases are what hold the answers where they were.
#[test]
fn the_compilers_spelling_is_nikaia() {
    use nikaia_std::tools::spelling::{distance, one_edit_apart};
    let d = |a: &str, b: &str| distance(a, b);
    assert_eq!(d("name", "name"), 0);
    assert_eq!(d("", "abc"), 3);
    assert_eq!(d("abc", ""), 3);
    assert_eq!(d("nmae", "name"), 1, "two neighbours swapped are one edit");
    assert_eq!(d("kitten", "sitting"), 3);
    assert_eq!(d("lenght", "length"), 1);
    assert_eq!(d("prnitln", "println"), 1);
    let one = |a: &str, b: &str| one_edit_apart(a, b);
    assert!(one("id", "ids"), "one put in");
    assert!(one("ids", "id"), "one taken out");
    assert!(one("name", "nome"), "one changed");
    assert!(!one("name", "name"), "the same word is not a near miss");
    assert!(!one("nmae", "name"), "a swap is two edits here");
    assert!(!one("id", "idss"));
    assert!(!one("abc", "xbz"));
}

/// **The second piece, a `dsl` body's holes** (0.0.248), held to what the Rust
/// it replaced answered: a path and a time are not holes, a name is taken once
/// and in the order the body names it, and a body without one has none.
#[test]
fn the_compilers_dsl_holes_are_nikaia() {
    use nikaia_std::tools::dsl::parameters;
    assert_eq!(
        parameters("SELECT * FROM users WHERE id = :id AND name = :name"),
        vec!["id", "name"]
    );
    assert_eq!(
        parameters("a::b and ::c"),
        Vec::<String>::new(),
        "a path is not a hole"
    );
    assert_eq!(
        parameters("at 12:30"),
        Vec::<String>::new(),
        "a time is not a hole"
    );
    assert_eq!(
        parameters(":a :b :a"),
        vec!["a", "b"],
        "each once, first first"
    );
    assert_eq!(parameters(":user_id2."), vec!["user_id2"]);
    assert_eq!(parameters("no holes here"), Vec::<String>::new());
    assert_eq!(parameters(":"), Vec::<String>::new());
    assert_eq!(
        parameters("x = :ä"),
        Vec::<String>::new(),
        "a name is ASCII"
    );
}
