//! Regenerates `tests/specification/EXPECTED.txt`.
//!
//! ```text
//! cargo run -p nikaia --example specification > tests/specification/EXPECTED.txt
//! ```
//!
//! The sibling of `errors.rs`, and the same bargain: what is checked in is how
//! far the compiler gets with every `nika` block in the specification **today**,
//! fragments and refusals included, so that any change is a diff somebody can
//! read. The walk itself is `nikaia::specbook`, because an example is a binary a
//! test cannot call into.

fn main() {
    print!(
        "{}",
        nikaia::specbook::report(
            &nikaia::specbook::specification_dir(),
            &nikaia::specbook::workshop(),
        )
    );
}
