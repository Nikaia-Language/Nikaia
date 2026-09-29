//! Where a node stands: two `u32` byte offsets
//! ([ADR-252](../../../docs/specification/adr/adr-252.md) D2).

use nikaia::ast::{Item, LONGEST_SOURCE, Span};
use nikaia::parser::{fits, parse_to_ast};

/// Every node carries one, so its size is the tree's: two `u32`, where a
/// `Range<usize>` was two `usize`.
#[test]
fn a_span_is_eight_bytes_and_a_copy() {
    assert_eq!(std::mem::size_of::<Span>(), 8);
    let span = Span::new(3, 7);
    let copied = span;
    assert_eq!(span, copied, "a span is copied, not moved");
    assert_eq!(span.bytes(), 3..7);
    assert_eq!((span.at(), span.stop()), (3, 7));
    assert!(span.contains(3) && span.contains(6) && !span.contains(7));
}

/// A span still names the bytes the node was parsed from.
#[test]
fn a_span_slices_the_source_it_came_from() {
    let source = "fn first() { }\n\nfn second() -> i64 { return 2 }\n";
    let parsed = parse_to_ast(source).expect("parses");
    let second = &parsed.program.items[1];
    assert!(matches!(second.node, Item::Fn { .. }));
    assert!(
        source[second.span.bytes()].starts_with("fn second()"),
        "{:?}",
        &source[second.span.bytes()]
    );
}

/// A source longer than a `u32` can point into is refused before it is read,
/// rather than wrapping every offset past 4 GiB. Asked of the length, so the
/// test does not have to allocate one.
#[test]
fn a_source_over_four_gib_is_refused_with_the_reason() {
    assert!(fits(LONGEST_SOURCE).is_ok());
    let refused = fits(LONGEST_SOURCE + 1).expect_err("one byte over is refused");
    let said = format!("{refused:#}");
    assert!(said.contains("4 GiB"), "{said}");
    assert!(said.contains("ADR-252"), "{said}");
}
