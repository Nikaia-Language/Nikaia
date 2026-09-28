// crates/nikaia/src/emit/template.rs
//
// The `html` template DSL (ADR-017), compiled where the template is written.
//
// **The splitting is Nikaia** since 0.0.252: `nikaia-std/src/tools/template.nika`
// holds the HTML scan that decides where a hole sits and whether escaping can
// make it safe, lowered by `nikaia lower-std` and reached here as
// `nikaia_std::tools::template` (ADR-196's route, ADR-250's road). What stays
// in Rust is the one conversion a Nikaia module cannot write for this crate:
// its thrown `Refused` into this compiler's own refusal, and a length into the
// `usize` the emitter reserves with.

use anyhow::Result;

use crate::refused;

pub use nikaia_std::tools::template::{Position, Segment, illegal, illegal_message};

/// The literal bytes a template writes, whatever its holes turn out to be - a
/// floor on the rendered length (ADR-178 §1), counted in Nikaia.
pub fn literal_length(segments: &[Segment]) -> usize {
    usize::try_from(nikaia_std::tools::template::literal_length(segments)).unwrap_or(0)
}

/// Split a template body into text and holes, deciding each hole's position.
pub fn split(body: &str) -> Result<Vec<Segment>> {
    nikaia_std::tools::template::split(body).map_err(|refusal| refused!("{refusal}"))
}
