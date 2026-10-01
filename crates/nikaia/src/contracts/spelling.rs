// crates/nikaia/src/contracts/spelling.rs
//
// **A signature in the ledger is written the way Nikaia writes one**
// ([ADR-251](../../../../docs/specification/adr/adr-251.md) D4).
//
// The compiler keeps a signature as it always has - the receiver as its type,
// a type variable as `$T`, what the result points into as a list beside it -
// and every reader of `Signature` reads that. What changes is the file: there
// the receiver is `ref self` or `ref mut self`, a type variable is declared in
// brackets before the parameters, and the result says where it points with
// `ref(a | b)`. This module writes the file's spelling; reading it back is
// `tools/ledger.nika`'s `unspell` (0.0.292, ADR-257 step (c)), beside the rest
// of the reader.
//
//     (ref Scan, c: char)                       mutates = true
//     (ref mut self, c: char)
//
//     (ref HashMap[$K, $V], key: ?) -> Entry[$V]
//     [K, V](ref self: HashMap[K, V], key: ?) -> Entry[V]
//
//     (ref Position) -> ref String              returns = "borrows(self)"
//     (ref self) -> ref(self) String

use super::ty::split_args;

/// A signature in the file's spelling, and which of the two facts beside it the
/// spelling could carry: a result with no `ref` in it has no place for where it
/// points, and a function with no receiver none for whether the call changes it.
pub struct Spelled {
    pub text: String,
    pub borrows_said: bool,
    pub mutates_said: bool,
}

/// The compiler's signature text, `key`'s entry, into the file's spelling.
pub fn spell(text: &str, key: &str, borrows: &[String], mutates: bool) -> Spelled {
    let Some(parts) = Parts::of(text) else {
        return Spelled {
            text: text.to_string(),
            borrows_said: false,
            mutates_said: false,
        };
    };
    let owner = owner_of(key);

    // **The receiver as the source writes it.** `ref self` where the type is
    // the owner's own, and the type written out where it is not - a type with
    // arguments, a trait's `?`.
    let (mut positional, options) = parts.positional_and_options();
    let mut mutates_said = false;
    if let Some(first) = positional.first_mut()
        && !is_named(first)
    {
        let written = first.clone();
        *first = match (written.strip_prefix("ref "), owner) {
            (Some(ty), Some(owner)) if ty == owner => match mutates {
                true => "ref mut self".to_string(),
                false => "ref self".to_string(),
            },
            (None, Some(owner)) if written == owner && !mutates => "self".to_string(),
            _ => match mutates {
                true => format!("mut self: {written}"),
                false => format!("self: {written}"),
            },
        };
        mutates_said = mutates;
    }

    // **Where the result points, in the result** - every `ref` in it, because
    // a view the result holds may point into any view it was given.
    let mut result = parts.result.clone();
    let mut borrows_said = false;
    if !borrows.is_empty() && has_word(&result, "ref") {
        result = replace_ref(&result, &format!("ref({})", borrows.join(" | ")));
        borrows_said = true;
    }

    // **Type variables declared in brackets**, the bounded ones first and in
    // their order, then every other in the order it first appears.
    let mut declared: Vec<String> = parts.bracket.clone();
    let mut seen: Vec<String> = declared
        .iter()
        .map(|d| d.split(':').next().unwrap_or(d).trim().to_string())
        .collect();
    let mut body = positional.join(", ");
    if let Some(options) = &options {
        body = format!("{body}; {options}");
    }
    for variable in variables(&format!("{body}{result}")) {
        if !seen.contains(&variable) {
            seen.push(variable.clone());
            declared.push(variable);
        }
    }
    let before = match declared.is_empty() {
        true => String::new(),
        false => format!("[{}]", declared.join(", ")),
    };
    Spelled {
        text: format!("{before}({body}){result}").replace('$', ""),
        borrows_said,
        mutates_said,
    }
}

/// A signature taken apart: the bracket list's entries, what is inside the
/// parameter list, and everything after it (` -> T`, or nothing).
struct Parts {
    bracket: Vec<String>,
    inside: String,
    result: String,
}

impl Parts {
    fn of(text: &str) -> Option<Parts> {
        let text = text.trim();
        let (bracket, rest) = match text.strip_prefix('[') {
            Some(rest) => {
                let close = closing(rest, '[', ']')?;
                (
                    split_args(&rest[..close])
                        .into_iter()
                        .map(|d| d.trim().to_string())
                        .filter(|d| !d.is_empty())
                        .collect(),
                    rest[close + 1..].trim_start(),
                )
            }
            None => (Vec::new(), text),
        };
        let inside = rest.strip_prefix('(')?;
        let close = closing(inside, '(', ')')?;
        Some(Parts {
            bracket,
            inside: inside[..close].to_string(),
            result: inside[close + 1..].to_string(),
        })
    }

    /// The positional parameters, each trimmed, and the options after a `;`.
    fn positional_and_options(&self) -> (Vec<String>, Option<String>) {
        let (positional, options) = match top_level_semicolon(&self.inside) {
            Some(at) => (
                &self.inside[..at],
                Some(self.inside[at + 1..].trim().to_string()),
            ),
            None => (self.inside.as_str(), None),
        };
        let parts = split_args(positional)
            .into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        (parts, options)
    }
}

/// The type an entry's receiver is when it is written `self`: what stands
/// before the last `::` of its key.
fn owner_of(key: &str) -> Option<&str> {
    key.rsplit_once("::").map(|(owner, _)| owner)
}

/// `name: T` or `mut name: T`, and not a bare type - which may hold a `::`.
fn is_named(part: &str) -> bool {
    let part = part.trim().trim_start_matches("mut ");
    let name: String = part
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') {
        return false;
    }
    let rest = part[name.len()..].trim_start();
    rest.starts_with(':') && !rest.starts_with("::")
}

/// Where the bracket opened just before `text` closes.
fn closing(text: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (at, c) in text.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                return Some(at);
            }
            depth -= 1;
        }
    }
    None
}

fn top_level_semicolon(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, c) in text.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => return Some(at),
            _ => {}
        }
    }
    None
}

/// Whether `word` stands in `text` as a word of its own, followed by a blank.
fn has_word(text: &str, word: &str) -> bool {
    word_starts(text, word).next().is_some()
}

fn word_starts<'a>(text: &'a str, word: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(word)
        .map(|(at, _)| at)
        .filter(move |&at| {
            let before = text[..at].chars().next_back();
            let after = text[at + word.len()..].chars().next();
            !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
                && after == Some(' ')
        })
}

/// Every `ref ` in `text`, written `with ` instead.
fn replace_ref(text: &str, with: &str) -> String {
    let starts: Vec<usize> = word_starts(text, "ref").collect();
    let mut out = String::new();
    let mut from = 0;
    for at in starts {
        out.push_str(&text[from..at]);
        out.push_str(with);
        from = at + "ref".len();
    }
    out.push_str(&text[from..]);
    out
}

/// Every `$T` in `text`, by name, in the order each first appears.
fn variables(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (at, c) in text.char_indices() {
        if c != '$' {
            continue;
        }
        let name: String = text[at + 1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}
