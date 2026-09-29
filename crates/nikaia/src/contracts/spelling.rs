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
// `ref(a | b)`. This module is the one place that turns one into the other, in
// both directions, and nothing else knows the file's spelling.
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

/// A signature read back from the file's spelling into the compiler's.
pub struct Read {
    pub text: String,
    pub borrows: Vec<String>,
    pub mutates: bool,
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

/// The file's spelling of `key`'s signature back into the compiler's.
///
/// **A signature in the spelling before it reads unchanged**: no bracket list
/// of bare names, no `self`, no `ref(…)`, and `$T` already the compiler's.
pub fn unspell(text: &str, key: &str) -> Read {
    let Some(parts) = Parts::of(text) else {
        return Read {
            text: text.to_string(),
            borrows: Vec::new(),
            mutates: false,
        };
    };
    let names: Vec<String> = parts
        .bracket
        .iter()
        .map(|d| d.split(':').next().unwrap_or(d).trim().to_string())
        .collect();
    let bounded: Vec<String> = parts
        .bracket
        .iter()
        .filter(|d| d.contains(':'))
        .cloned()
        .collect();

    let (mut positional, options) = parts.positional_and_options();
    let mut mutates = false;
    let owner = owner_of(key).unwrap_or("?");
    if let Some(first) = positional.first_mut() {
        let written = first.trim().to_string();
        let as_type = match written.as_str() {
            "self" => Some(owner.to_string()),
            "ref self" => Some(format!("ref {owner}")),
            "ref mut self" => {
                mutates = true;
                Some(format!("ref {owner}"))
            }
            other => match (other.strip_prefix("mut self:"), other.strip_prefix("self:")) {
                (Some(ty), _) => {
                    mutates = true;
                    Some(ty.trim().to_string())
                }
                (None, Some(ty)) => Some(ty.trim().to_string()),
                (None, None) => None,
            },
        };
        if let Some(ty) = as_type {
            *first = ty;
        }
    }
    let takes_self = positional
        .first()
        .is_some_and(|_| receiver_written(&parts.positional_and_options().0));

    // Where the result points: every `ref(…)` in it, gathered in the order the
    // parameters are declared.
    let (result, pointed) = gather_refs(&parts.result);
    let mut order: Vec<String> = Vec::new();
    if takes_self {
        order.push("self".to_string());
    }
    for part in &positional {
        if let Some((name, _)) = part.split_once(':')
            && is_named(part)
        {
            order.push(name.trim().trim_start_matches("mut ").trim().to_string());
        }
    }
    let mut borrows: Vec<String> = order
        .iter()
        .filter(|p| pointed.contains(p))
        .cloned()
        .collect();
    for name in &pointed {
        if !borrows.contains(name) {
            borrows.push(name.clone());
        }
    }

    let mut body = positional.join(", ");
    if let Some(options) = &options {
        body = format!("{body}; {options}");
    }
    let before = match bounded.is_empty() {
        true => String::new(),
        false => format!("[{}]", bounded.join(", ")),
    };
    Read {
        text: format!(
            "{before}({}){}",
            variables_marked(&body, &names),
            variables_marked(&result, &names)
        ),
        borrows,
        mutates,
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

/// Whether the file's first parameter is a receiver, in either spelling.
fn receiver_written(positional: &[String]) -> bool {
    positional.first().is_some_and(|first| {
        let first = first.trim();
        matches!(first, "self" | "ref self" | "ref mut self")
            || first.starts_with("self:")
            || first.starts_with("mut self:")
            || !is_named(first)
    })
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

/// Every `ref(a | b)` in `text` written `ref`, and the names they held.
fn gather_refs(text: &str) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut names: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("ref(") {
        let before = rest[..at].chars().next_back();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            out.push_str(&rest[..at + 4]);
            rest = &rest[at + 4..];
            continue;
        }
        let Some(close) = rest[at + 4..].find(')') else {
            break;
        };
        for name in rest[at + 4..at + 4 + close].split('|') {
            let name = name.trim().to_string();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
        out.push_str(&rest[..at]);
        out.push_str("ref");
        rest = &rest[at + 4 + close + 1..];
    }
    out.push_str(rest);
    (out, names)
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

/// `text` with every word that is one of `names` written `$name` - not a
/// segment of a path (`a::T`), and not one already marked.
fn variables_marked(text: &str, names: &[String]) -> String {
    if names.is_empty() {
        return text.to_string();
    }
    let mut out = String::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut at = 0;
    while at < bytes.len() {
        let c = bytes[at];
        if c.is_ascii_alphabetic() || c == '_' {
            let start = at;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == '_') {
                at += 1;
            }
            let word: String = bytes[start..at].iter().collect();
            let before = start.checked_sub(1).map(|b| bytes[b]);
            let path_before = start >= 2 && bytes[start - 1] == ':' && bytes[start - 2] == ':';
            let path_after = at + 1 < bytes.len() && bytes[at] == ':' && bytes[at + 1] == ':';
            if names.contains(&word) && before != Some('$') && !path_before && !path_after {
                out.push('$');
            }
            out.push_str(&word);
            continue;
        }
        out.push(c);
        at += 1;
    }
    out
}
