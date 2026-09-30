use nikaia::ast::{Expr, Item, Stmt};
use nikaia::parser::parse_to_ast;

#[test]
fn test_advanced_hello_world_compilation() {
    // Hello World with an async spawn
    let source_code = r#"
        fn main() {
            println("Hello Nikaia");
            spawn fn {
                log("Async World")
            }
        }
    "#;

    let parsed = parse_to_ast(source_code).expect("parse failed");
    let program = &parsed.program;

    // 2. Execute Verification (Inspect AST)
    assert_eq!(program.items.len(), 1, "Should have 1 main function");

    if let Item::Fn { name, body, .. } = &program.items[0].node {
        assert_eq!(parsed.text(name.expect("a named function")), "main");
        assert_eq!(body.stmts.len(), 2, "Main should have 2 statements");

        // Verify println("Hello Nikaia")
        match &body.stmts[0].node {
            Stmt::Expr(Expr::Call { func, args, .. }) => {
                if let Expr::Variable(fname) = &**func {
                    assert_eq!(parsed.text(*fname), "println");
                } else {
                    panic!("Expected function name");
                }

                assert_eq!(args.len(), 1);
                if let Expr::LitStr { text: s, .. } = &args[0] {
                    assert_eq!(s, "Hello Nikaia");
                } else {
                    panic!("Expected string literal");
                }
            }
            _ => panic!("First statement should be a call"),
        }

        // Verify spawn fn { ... }
        match &body.stmts[1].node {
            Stmt::Expr(Expr::Spawn {
                body: spawn_body,
                is_move,
            }) => {
                assert!(!is_move, "Should not be move by default");
                // The body is the lambda of Part I 8.2 - one form, and the
                // trailing lambda of 5.3 is it.
                if let Expr::Closure {
                    body: inner_block,
                    params,
                    ..
                } = &**spawn_body
                {
                    assert!(params.is_empty(), "a task is handed nothing");
                    assert_eq!(inner_block.stmts.len(), 1);
                } else {
                    panic!("Spawn body should be a block");
                }
            }
            _ => panic!("Second statement should be spawn"),
        }
    } else {
        panic!("Top level item is not a function");
    }
}

/// **A word keyword does not match the beginning of a longer word.**
///
/// The grammar is scannerless (ADR-001 D2): there is no lexer deciding where a
/// word ends, so `"as"` matched the first two characters of `assert`. Measured
/// before the `KW_*` boundary rules, and every one of them silent:
///
/// ```text
/// true asfoo      ->  `true as foo`
/// returnx         ->  `return x`
/// forx in 0..3    ->  `for x in 0..3`
/// ```
///
/// The last is the one that matters most, because it is a **valid program with a
/// different meaning**: it binds `x` where the source says `forx`.
#[test]
fn a_keyword_does_not_swallow_the_start_of_a_longer_word() {
    // `asfoo` is a name, so there is no cast and no type called `foo`.
    let parsed = parse_to_ast("fn main() {\n    let x = true asfoo\n}").expect("it parses");
    let Item::Fn { body, .. } = &parsed.program.items[0].node else {
        panic!("a function");
    };
    assert!(
        matches!(&body.stmts[1].node, Stmt::Expr(Expr::Variable(name))
            if parsed.text(*name) == "asfoo"),
        "`asfoo` is one name: {:?}",
        body.stmts[1].node
    );

    // `returnx` is a name and not a `return`.
    let parsed = parse_to_ast("fn f() -> i64 {\n    returnx\n}").expect("it parses");
    let Item::Fn { body, .. } = &parsed.program.items[0].node else {
        panic!("a function");
    };
    assert!(
        matches!(&body.stmts[0].node, Stmt::Expr(Expr::Variable(name))
            if parsed.text(*name) == "returnx"),
        "`returnx` is one name: {:?}",
        body.stmts[0].node
    );

    // `forx` does not begin a loop, so nothing binds `x`.
    //
    // The probe used to be `forx in 0..3 { … }`, and the reserved-word list
    // (ADR-051) makes that program refused rather than
    // misread - `in` is not a name any more, so the three statements it used
    // to be read as cannot be read. Which is the same defect this test is
    // about, one level up; the boundary is what is under test here, so the
    // probe is a program that stays valid.
    let parsed = parse_to_ast("fn main() {\n    let forx = 3\n}").expect("it parses");
    let Item::Fn { body, .. } = &parsed.program.items[0].node else {
        panic!("a function");
    };
    assert!(
        matches!(&body.stmts[0].node, Stmt::Let { names, .. }
            if names.len() == 1 && parsed.text(names[0]) == "forx"),
        "`forx` is one name and does not begin a `for`: {:?}",
        body.stmts[0].node
    );
}

/// …and the keywords themselves still work, which is the half a boundary could
/// break: a lowercase boundary rule would have let the implicit whitespace in
/// between, and `as i32` would then be refused for having a space in it.
#[test]
fn the_keywords_themselves_are_untouched() {
    for source in [
        "fn main() {\n    let x = 3 as i64\n}",
        "fn f() -> i64 {\n    return 7\n}",
        "fn main() {\n    for n in 0..<3 { println(f\"{n}\") }\n}",
        "fn main() {\n    let mut n = 1\n    n = 2\n}",
        "pub fn f() sync {\n    return\n}",
        "fn f() throws {\n    throw oops()\n}",
        "fn main() {\n    if true { } else { }\n}",
        "fn main() {\n    let b = false\n    let c = true\n}",
        "struct S { id: i64 }\nenum E { A }\nimpl S {\n    fn id(ref self) -> i64 { return self.id }\n}",
        "use utils\n\nfn main() {\n    utils::f()\n}",
        "fn main() {\n    let s = seq { println(\"a\") }\n}",
        "fn main() {\n    spawn fn { println(\"a\") }\n}",
        "fn main() {\n    match 1 {\n        else => println(\"x\")\n    }\n}",
        "fn main() {\n    while true { }\n}",
    ] {
        parse_to_ast(source).unwrap_or_else(|e| panic!("{source}\n{e:#}"));
    }
}

// --- the reserved words (ADR-051) --------------------------------------------

/// **Every word in `RESERVED_WORDS` is refused as a name**, and that is what
/// holds the const and the grammar's `RESERVED` rule together.
///
/// The two are separate lists - one in Rust for the note a parse error gets and
/// for the checker's `self` case, one in the grammar - so a word added to either
/// alone would drift. Asking the parser instead of comparing the texts is what
/// makes the test about the behaviour rather than about the spelling.
///
/// `self` is the exception the const documents: it is the one reserved word the
/// grammar must accept as a name, because `self.min` refers to it. Declaring it
/// is `NK1119`, which `typecheck.rs` covers.
#[test]
fn every_reserved_word_is_refused_as_a_name() {
    for word in nikaia::parser::RESERVED_WORDS {
        // `let mut …` rather than `let …`, so the word stands in the name
        // position for every member of the list: `let mut = 3` would put `mut`
        // itself in the marker's place and be refused for a different reason.
        let source = format!("fn main() {{\n    let mut {word} = 3\n}}\n");
        let parsed = parse_to_ast(&source);
        if word == "self" {
            assert!(
                parsed.is_ok(),
                "`self` is the one reserved word the grammar has to accept as a \
                 name - `self.min` refers to it - and its declaration is refused \
                 by the checker instead (NK1119)"
            );
            continue;
        }
        let err = parsed.err().unwrap_or_else(|| {
            panic!("`let {word} = 3` must not parse: a reserved word is not a name")
        });
        // The reason is a note under the headline, which is what the reader
        // sees rendered.
        let finding = nikaia::diagnostics::refused_finding(&err)
            .unwrap_or_else(|| panic!("a parse error carries its finding: {err:#}"));
        let said = nikaia::diagnostics::render_finding(finding, "app.nika", &source);
        assert!(
            said.contains(&format!(
                "= note: `{word}` is a reserved word, so it can't be used as a name here."
            )),
            "the parse error has to say why, not just that it failed: {said}"
        );
    }
}

/// **The reserved-word note only where a name was wanted**: `struct` inside an
/// `impl` and `ref` where a `:` is missing are other mistakes, and a note about
/// names would explain the wrong one.
#[test]
fn a_reserved_word_where_no_name_was_wanted_gets_no_note_about_names() {
    for source in ["impl P {\n    struct T {}\n}\n", "fn f(x ref i64) {\n}\n"] {
        let err = parse_to_ast(source).err().expect("it does not parse");
        let finding = nikaia::diagnostics::refused_finding(&err).expect("a finding");
        assert!(
            finding.notes.iter().all(|n| !n.contains("reserved word")),
            "{source}: {finding:#?}"
        );
    }
}

/// **And the grammar sublanguage's words are not reserved**, which is the other
/// half of the decision: they are keywords inside a `grammar` block and names
/// everywhere else.
#[test]
fn the_grammar_sublanguages_words_are_still_names() {
    for word in ["rule", "boundary", "fold", "par_fold", "unchecked", "eod"] {
        let source =
            format!("fn main() {{\n    let {word} = 3\n    println(f\"{{{word}}}\")\n}}\n");
        assert!(
            parse_to_ast(&source).is_ok(),
            "`{word}` belongs to the grammar sublanguage, so a program may use it \
             as a name"
        );
    }
}

/// **A segment after `::` may be a reserved word**, because no construct begins
/// in that position.
///
/// `Self::dsl` is the case that requires it: the shadow type of a
/// deferred-parameter DSL is spelled with the keyword (ADR-007 D5).
#[test]
fn a_path_segment_may_be_a_reserved_word() {
    let source = "\
struct S { n: i32 }
impl S {
    pub fn execute(ref self; ...args: Self::dsl) -> Self::dsl { return args }
}
";
    assert!(
        parse_to_ast(source).is_ok(),
        "`Self::dsl` has to parse: a segment follows a `::` and nothing can be \
         misread there"
    );
}

/// **The three silent misreadings the reserved list closed**, kept as a test so
/// they cannot come back.
///
/// Each was accepted with no diagnostic of any kind, because a keyword could be
/// a name: `if { }` was a variable `if` and a block, `let 5 = x` was a variable
/// `let` and then an assignment to `5`, and `if a = b { }` was three statements.
/// A program that means something other than what is written is the worst class
/// this project names (`docs/error-corpus.md`, rows B3, C4 and D2).
#[test]
fn a_keyword_standing_alone_no_longer_reads_as_a_variable() {
    for source in [
        "fn main() {\n    if { }\n}\n",
        "fn main() {\n    let 5 = x\n}\n",
        "fn main() {\n    if a = b { }\n}\n",
    ] {
        assert!(
            parse_to_ast(source).is_err(),
            "{source} used to parse as something else entirely"
        );
    }
}
