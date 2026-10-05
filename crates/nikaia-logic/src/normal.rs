// crates/nikaia-logic/src/normal.rs
//
// **A query in normal form, and an answer as text** (ADR-270 D19): what a
// `nikaia.proofs` entry is keyed by and what it holds.
//
// The key has to be the same for the same question, wherever it was asked
// and whatever the program called its values, so the query is rewritten
// before it is hashed: `>=` and `>` turned round into `<=` and `<`, the facts
// in a canonical order with duplicates dropped, and the variables renamed
// `v0`, `v1`, … by first occurrence. **The certificate is about the normal
// form** (D19): the solver is asked the normal query, and a recorded entry is
// checked against it (D20), so the order the frontend wrote its facts in is
// never part of what is trusted.
//
// The order of facts is by their shape, the fact written with every variable
// as `?`. Two facts of one shape keep the order they were asked in, so a
// query whose only difference from another is the order of two facts of one
// shape gets a key of its own. That costs one search, never a wrong answer.

use std::collections::BTreeMap;

use crate::{Arena, Certificate, Model, Query, Refutation, Step, Term, TermId};

/// A query rewritten into normal form, with the way back to its names.
#[derive(Debug, Clone)]
pub struct Normal {
    arena: Arena,
    facts: Vec<TermId>,
    goal: TermId,
    /// The program's name of `v{i}`, at `i`.
    names: Vec<String>,
}

impl Normal {
    /// `query` in normal form.
    pub fn of(query: &Query<'_>) -> Normal {
        // Turned round first, so that the shape a fact is sorted by is the
        // shape it is written in.
        let mut turned = query.arena.clone();
        let facts: Vec<TermId> = query
            .facts
            .iter()
            .map(|fact| turn(&mut turned, *fact))
            .collect();
        let goal = turn(&mut turned, query.goal);

        let mut ordered: Vec<(String, String, TermId)> = Vec::with_capacity(facts.len());
        for fact in facts {
            let shape = written(&turned, fact, &mut |_| "?".to_string());
            let named = written(&turned, fact, &mut |name| name.to_string());
            if ordered.iter().all(|(_, seen, _)| *seen != named) {
                ordered.push((shape, named, fact));
            }
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));

        let mut normal = Normal {
            arena: Arena::new(),
            facts: Vec::with_capacity(ordered.len()),
            goal: TermId::default(),
            names: Vec::new(),
        };
        let mut renamed: BTreeMap<String, String> = BTreeMap::new();
        for (_, _, fact) in &ordered {
            let copied = normal.copy(&turned, *fact, &mut renamed);
            normal.facts.push(copied);
        }
        normal.goal = normal.copy(&turned, goal, &mut renamed);
        normal
    }

    /// The normal form as a query, to ask a solver and to check an answer
    /// against.
    pub fn query(&self) -> Query<'_> {
        Query {
            arena: &self.arena,
            facts: &self.facts,
            goal: self.goal,
        }
    }

    /// The normal form written out: what the key is the hash of.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for fact in &self.facts {
            out.push_str(&written(&self.arena, *fact, &mut |name| name.to_string()));
            out.push('\n');
        }
        out.push_str("|- ");
        out.push_str(&written(&self.arena, self.goal, &mut |name| {
            name.to_string()
        }));
        out
    }

    /// A model of the normal form, in the program's names.
    pub fn named(&self, model: &Model) -> Model {
        Model {
            values: model
                .values
                .iter()
                .map(|(name, value)| {
                    let original = name
                        .strip_prefix('v')
                        .and_then(|n| n.parse::<usize>().ok())
                        .and_then(|n| self.names.get(n))
                        .cloned()
                        .unwrap_or_else(|| name.clone());
                    (original, *value)
                })
                .collect(),
        }
    }

    fn copy(&mut self, from: &Arena, id: TermId, renamed: &mut BTreeMap<String, String>) -> TermId {
        let term = from.get(id).clone();
        let mut c = |a: TermId, normal: &mut Normal| normal.copy(from, a, renamed);
        match term {
            Term::Bool(b) => self.arena.bool(b),
            Term::Int(n) => self.arena.int(n),
            Term::Var(name) => {
                let next = format!("v{}", renamed.len());
                let fresh = renamed.entry(name.clone()).or_insert_with(|| {
                    self.names.push(name.clone());
                    next
                });
                let fresh = fresh.clone();
                self.arena.var(&fresh)
            }
            Term::Neg(a) => {
                let a = c(a, self);
                self.arena.neg(a)
            }
            Term::Not(a) => {
                let a = c(a, self);
                self.arena.not(a)
            }
            Term::Add(a, b) => {
                let (a, b) = (c(a, self), c(b, self));
                self.arena.add(a, b)
            }
            Term::Sub(a, b) => {
                let (a, b) = (c(a, self), c(b, self));
                self.arena.sub(a, b)
            }
            Term::Mul(a, b) => {
                let (a, b) = (c(a, self), c(b, self));
                self.arena.mul(a, b)
            }
            Term::Le(a, b) => {
                let (a, b) = (c(a, self), c(b, self));
                self.arena.le(a, b)
            }
            Term::Lt(a, b) => {
                let (a, b) = (c(a, self), c(b, self));
                self.arena.lt(a, b)
            }
            Term::Ge(a, b) => {
                let (a, b) = (c(a, self), c(b, self));
                self.arena.ge(a, b)
            }
            Term::Gt(a, b) => {
                let (a, b) = (c(a, self), c(b, self));
                self.arena.gt(a, b)
            }
            Term::Eq(a, b) => {
                let (a, b) = (c(a, self), c(b, self));
                self.arena.eq(a, b)
            }
            Term::Ne(a, b) => {
                let (a, b) = (c(a, self), c(b, self));
                self.arena.ne(a, b)
            }
            Term::And(parts) => {
                let parts = parts.into_iter().map(|p| c(p, self)).collect();
                self.arena.and(parts)
            }
            Term::Or(parts) => {
                let parts = parts.into_iter().map(|p| c(p, self)).collect();
                self.arena.or(parts)
            }
        }
    }
}

/// `a >= b` as `b <= a` and `a > b` as `b < a`, all the way down: one way of
/// writing each comparison, so that the two spellings share a key.
fn turn(arena: &mut Arena, id: TermId) -> TermId {
    match arena.get(id).clone() {
        Term::Ge(a, b) => arena.le(b, a),
        Term::Gt(a, b) => arena.lt(b, a),
        Term::Not(a) => {
            let a = turn(arena, a);
            arena.not(a)
        }
        Term::And(parts) => {
            let parts = parts.into_iter().map(|p| turn(arena, p)).collect();
            arena.and(parts)
        }
        Term::Or(parts) => {
            let parts = parts.into_iter().map(|p| turn(arena, p)).collect();
            arena.or(parts)
        }
        _ => id,
    }
}

/// A term in prefix form, each variable written by `var`.
fn written(arena: &Arena, id: TermId, var: &mut impl FnMut(&str) -> String) -> String {
    let mut w = |a: &TermId| written(arena, *a, &mut *var);
    match arena.get(id) {
        Term::Bool(b) => b.to_string(),
        Term::Int(n) => n.to_string(),
        Term::Var(name) => var(name),
        Term::Neg(a) => format!("(- {})", w(a)),
        Term::Not(a) => format!("(not {})", w(a)),
        Term::Add(a, b) => format!("(+ {} {})", w(a), w(b)),
        Term::Sub(a, b) => format!("(- {} {})", w(a), w(b)),
        Term::Mul(a, b) => format!("(* {} {})", w(a), w(b)),
        Term::Le(a, b) => format!("(<= {} {})", w(a), w(b)),
        Term::Lt(a, b) => format!("(< {} {})", w(a), w(b)),
        Term::Ge(a, b) => format!("(>= {} {})", w(a), w(b)),
        Term::Gt(a, b) => format!("(> {} {})", w(a), w(b)),
        Term::Eq(a, b) => format!("(= {} {})", w(a), w(b)),
        Term::Ne(a, b) => format!("(distinct {} {})", w(a), w(b)),
        Term::And(parts) => {
            let parts: Vec<String> = parts.iter().map(&mut w).collect();
            format!("(and {})", parts.join(" "))
        }
        Term::Or(parts) => {
            let parts: Vec<String> = parts.iter().map(&mut w).collect();
            format!("(or {})", parts.join(" "))
        }
    }
}

/// A certificate as one word of text: `R(h0,h2,c0:1:1:2,t3)` for a
/// refutation, `S1[…|…]` for a split of the second disjunction.
pub fn certificate_text(certificate: &Certificate) -> String {
    match certificate {
        Certificate::Refuted(refutation) => {
            let steps: Vec<String> = refutation
                .steps
                .iter()
                .map(|step| match step {
                    Step::Hypothesis(n) => format!("h{n}"),
                    Step::Tighten(n) => format!("t{n}"),
                    Step::Combine {
                        left,
                        by_left,
                        right,
                        by_right,
                    } => format!("c{left}:{by_left}:{right}:{by_right}"),
                })
                .collect();
            format!("R({})", steps.join(","))
        }
        Certificate::Split { disjunction, cases } => {
            let cases: Vec<String> = cases.iter().map(certificate_text).collect();
            format!("S{disjunction}[{}]", cases.join("|"))
        }
    }
}

/// [`certificate_text`] read back, or nothing for text it did not write.
pub fn certificate_of(text: &str) -> Option<Certificate> {
    let (certificate, rest) = certificate_from(text)?;
    rest.is_empty().then_some(certificate)
}

fn certificate_from(text: &str) -> Option<(Certificate, &str)> {
    if let Some(rest) = text.strip_prefix("R(") {
        let end = rest.find(')')?;
        let mut steps = Vec::new();
        for word in rest[..end].split(',').filter(|w| !w.is_empty()) {
            steps.push(step_of(word)?);
        }
        return Some((Certificate::Refuted(Refutation { steps }), &rest[end + 1..]));
    }
    let rest = text.strip_prefix('S')?;
    let open = rest.find('[')?;
    let disjunction = rest[..open].parse().ok()?;
    let mut rest = &rest[open + 1..];
    let mut cases = Vec::new();
    loop {
        let (case, after) = certificate_from(rest)?;
        cases.push(case);
        if let Some(after) = after.strip_prefix('|') {
            rest = after;
        } else {
            let after = after.strip_prefix(']')?;
            return Some((Certificate::Split { disjunction, cases }, after));
        }
    }
}

fn step_of(word: &str) -> Option<Step> {
    if let Some(n) = word.strip_prefix('h') {
        return Some(Step::Hypothesis(n.parse().ok()?));
    }
    if let Some(n) = word.strip_prefix('t') {
        return Some(Step::Tighten(n.parse().ok()?));
    }
    let parts: Vec<&str> = word.strip_prefix('c')?.split(':').collect();
    let [left, by_left, right, by_right] = parts.as_slice() else {
        return None;
    };
    Some(Step::Combine {
        left: left.parse().ok()?,
        by_left: by_left.parse().ok()?,
        right: right.parse().ok()?,
        by_right: by_right.parse().ok()?,
    })
}

/// A model as one word of text, `v0=3,v1=-2`, or `-` for one with no
/// variables.
pub fn model_text(model: &Model) -> String {
    if model.values.is_empty() {
        return "-".to_string();
    }
    let values: Vec<String> = model
        .values
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    values.join(",")
}

/// [`model_text`] read back.
pub fn model_of(text: &str) -> Option<Model> {
    let mut values = BTreeMap::new();
    if text != "-" {
        for pair in text.split(',') {
            let (name, value) = pair.split_once('=')?;
            if name.is_empty() || name.contains(char::is_whitespace) {
                return None;
            }
            values.insert(name.to_string(), value.parse().ok()?);
        }
    }
    Some(Model { values })
}
