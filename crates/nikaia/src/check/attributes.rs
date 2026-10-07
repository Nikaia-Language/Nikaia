//! **Attributes** ([ADR-331](../../../docs/specification/adr/adr-331.md)
//! D1-D4, Part II 10.3): `@json::Name("createdAt")` before a declaration is a
//! value of the struct the path names. A struct is an attribute where
//! `@meta::Attribute(…)` marks it, naming the places it may stand; anywhere
//! else it is `NK1235`, and a second one where the mark does not say
//! `repeatable: true` is `NK1236`. The arguments are checked as a call's are,
//! against the struct's fields: those without a default by position, those
//! with one by name after the `;`.

use super::*;

/// The mark itself, which `std` declares and nothing else may be.
pub(super) const MARK: &str = "meta::Attribute";

/// Where an attribute may stand, as the mark's arguments spell them (D2).
pub(super) const PLACES: [&str; 8] = [
    "field",
    "struct",
    "enum",
    "variant",
    "fn",
    "parameter",
    "trait",
    "impl",
];

/// What a struct's `@meta::Attribute(…)` says: the places, and whether it may
/// stand twice.
#[derive(Clone, Debug, Default)]
pub(super) struct Mark {
    pub places: BTreeSet<String>,
    pub repeatable: bool,
}

impl Checker<'_> {
    /// **Every attribute of this file, where it stands** (ADR-331 D1-D4).
    pub(super) fn attributes(&mut self) {
        let marks = self.attribute_marks();
        let parsed = self.parsed;
        for item in &parsed.program.items {
            let place = match &item.node {
                Item::Struct { .. } => "struct",
                Item::Enum { .. } => "enum",
                Item::Fn { .. } => "fn",
                Item::Trait { .. } => "trait",
                Item::Impl { .. } => "impl",
                _ => "",
            };
            self.attributes_at(&item.attributes, place, &marks);
            match &item.node {
                Item::Struct { fields, .. } => {
                    for field in fields {
                        self.attributes_at(&field.attributes, "field", &marks);
                    }
                }
                Item::Enum { variants, .. } => {
                    for variant in variants {
                        self.attributes_at(&variant.attributes, "variant", &marks);
                        if let ast::VariantFields::Named(fields) = &variant.fields {
                            for field in fields {
                                self.attributes_at(&field.attributes, "field", &marks);
                            }
                        }
                    }
                }
                Item::Fn { args, .. } => self.parameter_attributes(args, &marks),
                Item::Impl { methods, .. } => {
                    for method in methods {
                        self.attributes_at(&method.attributes, "fn", &marks);
                        if let Item::Fn { args, .. } = &method.node {
                            self.parameter_attributes(args, &marks);
                        }
                    }
                }
                Item::Trait { methods, .. } => {
                    for method in methods {
                        self.attributes_at(&method.attributes, "fn", &marks);
                        self.parameter_attributes(&method.node.args, &marks);
                    }
                }
                _ => {}
            }
        }
    }

    fn parameter_attributes(&mut self, args: &[ast::FnArg], marks: &BTreeMap<String, Mark>) {
        for arg in args {
            self.attributes_at(&arg.attributes, "parameter", marks);
        }
    }

    /// **The marks of this package's structs**, read off every file of it:
    /// an attribute declared in one file is used in another.
    fn attribute_marks(&self) -> BTreeMap<String, Mark> {
        let mut marks = BTreeMap::new();
        let units = std::iter::once(self.parsed).chain(self.beside.iter().copied());
        for unit in units {
            for item in &unit.program.items {
                let Item::Struct { name, .. } = &item.node else {
                    continue;
                };
                let Some(mark) = item
                    .attributes
                    .iter()
                    .find(|a| unit.unaliased(unit.text(a.name)) == MARK)
                else {
                    continue;
                };
                marks.insert(unit.text(*name).to_string(), mark_of(unit, mark));
            }
        }
        marks
    }

    /// The attributes before one declaration, `place` being what it is; an
    /// empty `place` is a declaration no attribute may stand before.
    fn attributes_at(
        &mut self,
        attributes: &[ast::Attribute],
        place: &str,
        marks: &BTreeMap<String, Mark>,
    ) {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for attribute in attributes {
            let name = self.parsed.unaliased(self.parsed.text(attribute.name));
            if name == MARK {
                self.the_mark(attribute, place);
                continue;
            }
            let Some(fields) = self.fields_of(&name) else {
                self.an_attribute_nothing_declares(&name, &attribute.span);
                continue;
            };
            let mark = marks.get(&name);
            let allowed = mark.is_some_and(|m| m.places.contains(place));
            if !allowed {
                self.an_attribute_out_of_place(&name, place, mark, &attribute.span);
                continue;
            }
            let count = seen.entry(name.clone()).or_default();
            *count += 1;
            if *count == 2 && !mark.is_some_and(|m| m.repeatable) {
                self.an_attribute_written_twice(&name, &attribute.span);
            }
            self.attribute_arguments(&name, &fields, attribute);
        }
    }

    /// `@meta::Attribute(field, variant; repeatable: true)` itself: only on a
    /// struct, with places it knows and the one option.
    fn the_mark(&mut self, attribute: &ast::Attribute, place: &str) {
        if place != "struct" {
            self.an_attribute_out_of_place(MARK, place, None, &attribute.span);
            return;
        }
        if attribute.args.is_empty() {
            self.checked.findings.push(Finding {
                code: "NK1101",
                severity: Severity::Error,
                span: attribute.span,
                message: "`@meta::Attribute` names where the attribute may stand, and this \
                          names no place."
                    .to_string(),
                notes: vec![format!("The places are {}.", places_text())],
                help: Some("Write `@meta::Attribute(field)`.".to_string()),
                labels: Vec::new(),
            });
        }
        for arg in &attribute.args {
            let word = match arg {
                Expr::Variable(word) => self.parsed.text(*word).to_string(),
                Expr::Path(segments) => segments
                    .last()
                    .map(|s| self.parsed.text(*s).to_lowercase())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            if !PLACES.contains(&word.as_str()) {
                self.checked.findings.push(Finding {
                    code: "NK1102",
                    severity: Severity::Error,
                    span: attribute.span,
                    message: match word.is_empty() {
                        true => "`@meta::Attribute` takes places, and this is not one.".to_string(),
                        false => {
                            format!("`@meta::Attribute` takes places, and `{word}` is not one.")
                        }
                    },
                    notes: vec![format!("The places are {}.", places_text())],
                    help: Some("Name a place by its word alone: `field`.".to_string()),
                    labels: Vec::new(),
                });
            }
        }
        for option in &attribute.config {
            let option_name = self.parsed.text(option.name).to_string();
            if option_name != "repeatable" {
                self.checked.findings.push(Finding {
                    code: "NK1109",
                    severity: Severity::Error,
                    span: attribute.span,
                    message: format!("`@meta::Attribute` has no option called `{option_name}`."),
                    notes: vec!["Its one option is `repeatable: bool`.".to_string()],
                    help: Some("Write `repeatable: true`, or leave it out.".to_string()),
                    labels: Vec::new(),
                });
            } else if !matches!(option.value, Expr::LitBool(_)) {
                self.checked.findings.push(Finding {
                    code: "NK1102",
                    severity: Severity::Error,
                    span: attribute.span,
                    message: "`repeatable` is `true` or `false`.".to_string(),
                    notes: Vec::new(),
                    help: Some("Write `repeatable: true`.".to_string()),
                    labels: Vec::new(),
                });
            }
        }
    }

    /// **The arguments, against the fields** (D4): those without a default
    /// by position, in declaration order, those with one by name after the
    /// `;`; each a build-time value, and a variant of the field's `enum` by
    /// its name alone (D3).
    fn attribute_arguments(
        &mut self,
        name: &str,
        fields: &[FieldContract],
        attribute: &ast::Attribute,
    ) {
        let required: Vec<&FieldContract> =
            fields.iter().filter(|f| f.default.is_empty()).collect();
        let options: Vec<&FieldContract> =
            fields.iter().filter(|f| !f.default.is_empty()).collect();
        if attribute.args.len() != required.len() {
            let wanted = required
                .iter()
                .map(|f| format!("`{}`", f.name))
                .collect::<Vec<_>>()
                .join(", ");
            self.checked.findings.push(Finding {
                code: "NK1101",
                severity: Severity::Error,
                span: attribute.span,
                message: format!(
                    "`@{name}` takes {} argument{} before the `;`, and this gives {}.",
                    required.len(),
                    if required.len() == 1 { "" } else { "s" },
                    attribute.args.len()
                ),
                notes: vec![match required.is_empty() {
                    true => format!("Every field of `{name}` has a default, so each is named after the `;`."),
                    false => format!(
                        "They are the fields without a default, in the order `{name}` declares them: {wanted}."
                    ),
                }],
                help: Some("Give one value for each, by position.".to_string()),
                labels: Vec::new(),
            });
        }
        for (field, arg) in required.iter().zip(&attribute.args) {
            self.attribute_argument(name, field, arg, attribute.span);
        }
        for option in &attribute.config {
            let option_name = self.parsed.text(option.name).to_string();
            match options.iter().find(|f| f.name == option_name) {
                Some(field) => self.attribute_argument(name, field, &option.value, attribute.span),
                None => {
                    let known: Vec<String> =
                        options.iter().map(|f| format!("`{}`", f.name)).collect();
                    let positional = required.iter().any(|f| f.name == option_name);
                    self.checked.findings.push(Finding {
                        code: "NK1109",
                        severity: Severity::Error,
                        span: attribute.span,
                        message: format!("`@{name}` has no option called `{option_name}`."),
                        notes: vec![match (positional, known.is_empty()) {
                            (true, _) => format!(
                                "`{option_name}` has no default, so it is given by position, before the `;`."
                            ),
                            (false, true) => format!("No field of `{name}` has a default, so it takes no options."),
                            (false, false) => format!("Its options are {}.", known.join(", ")),
                        }],
                        help: Some(match positional {
                            true => format!("Move `{option_name}`'s value before the `;`."),
                            false => "Name one of its options, or leave it out.".to_string(),
                        }),
                        labels: Vec::new(),
                    });
                }
            }
        }
    }

    fn attribute_argument(&mut self, owner: &str, field: &FieldContract, arg: &Expr, span: Span) {
        // **A variant by its name alone** (D3): `snake` for a `Case`, read
        // off the field's type - only here.
        if let Expr::Variable(word) = arg
            && let Some(variants) = self.variants_of_type(&field.ty)
        {
            let word = self.parsed.text(*word);
            if variants.iter().any(|v| v.eq_ignore_ascii_case(word)) {
                return;
            }
        }
        let want = match &field.ty {
            Ty::Nullable(inner) if !matches!(arg, Expr::LitNull) => (**inner).clone(),
            other => other.clone(),
        };
        if matches!(arg, Expr::LitNull) && matches!(field.ty, Ty::Nullable(_)) {
            return;
        }
        if matches!(arg, Expr::LitStr { .. })
            && matches!(&want, Ty::Named { name, .. } if name == "String")
        {
            return;
        }
        if !crate::contracts::a_literal(arg) {
            let label = format!("{owner}.{}", field.name);
            self.computed_value(&label, &want, arg, span, true);
            return;
        }
        let found = self.expr(arg, &span);
        let (owner, field_name) = (owner.to_string(), field.name.clone());
        if let Some(kind) = literal_misfit(arg, &want) {
            self.checked.findings.push(Finding {
                code: "NK1102",
                severity: Severity::Error,
                span,
                message: format!(
                    "`@{owner}`'s `{field_name}` is `{}`, and this gives {kind}.",
                    want.text()
                ),
                notes: Vec::new(),
                help: Some(format!("Give `{field_name}` a `{}`.", want.text())),
                labels: Vec::new(),
            });
            return;
        }
        self.expect(&found, &want, span, "argument", move |found, want| {
            format!("`@{owner}`'s `{field_name}` is `{want}`, and this gives `{found}`.")
        });
    }

    /// The variants of an `enum` a field holds, written as the program
    /// writes them (`Camel`).
    fn variants_of_type(&self, ty: &Ty) -> Option<Vec<String>> {
        let Ty::Named { name, .. } = ty else {
            return None;
        };
        if let Some(variants) = self.enums.get(name) {
            return Some(variants.iter().cloned().collect());
        }
        let suffix = format!("::{name}");
        self.own
            .types
            .get(name)
            .or_else(|| {
                self.library
                    .types
                    .iter()
                    .find(|(key, _)| *key == name || key.ends_with(&suffix))
                    .map(|(_, c)| c)
            })
            .filter(|c| !c.variants.is_empty())
            .map(|c| c.variants.iter().map(|v| v.name.clone()).collect())
    }

    fn an_attribute_nothing_declares(&mut self, name: &str, span: &Span) {
        self.checked.findings.push(Finding {
            code: "NK1135",
            severity: Severity::Error,
            span: *span,
            message: format!("There's no struct called `{name}`."),
            notes: vec![
                "An attribute is a value of a struct that `@meta::Attribute` marks, declared \
                 here or in a package you use."
                    .to_string(),
            ],
            help: Some("Declare the struct, or fix the name.".to_string()),
            labels: Vec::new(),
        });
    }

    /// `NK1235`: a struct the mark does not make an attribute, or a place its
    /// mark does not name.
    fn an_attribute_out_of_place(
        &mut self,
        name: &str,
        place: &str,
        mark: Option<&Mark>,
        span: &Span,
    ) {
        let here = match place {
            "" => "this declaration".to_string(),
            place => format!("a `{place}`"),
        };
        let (message, note, help) = match (name == MARK, mark) {
            (true, _) => (
                format!("`@{MARK}` marks a struct, and this is {here}."),
                "It makes the struct after it an attribute.".to_string(),
                "Move it before a `struct`.".to_string(),
            ),
            (false, None) => (
                format!("`{name}` is not an attribute: `@{MARK}` does not mark it."),
                "An attribute is a struct marked with `@meta::Attribute(…)`, naming where it \
                 may stand."
                    .to_string(),
                format!(
                    "Write `@meta::Attribute({})` before `struct {name}`.",
                    place_or_field(place)
                ),
            ),
            (false, Some(mark)) => (
                format!("`@{name}` may not stand before {here}."),
                format!(
                    "Its mark names {}.",
                    mark.places
                        .iter()
                        .map(|p| format!("`{p}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                match place {
                    "" => "Remove it.".to_string(),
                    place => {
                        format!("Add `{place}` to its `@meta::Attribute(…)`, or remove it here.")
                    }
                },
            ),
        };
        self.checked.findings.push(Finding {
            code: "NK1235",
            severity: Severity::Error,
            span: *span,
            message,
            notes: vec![note],
            help: Some(help),
            labels: Vec::new(),
        });
    }

    /// `NK1236`: the same attribute twice before one declaration.
    fn an_attribute_written_twice(&mut self, name: &str, span: &Span) {
        self.checked.findings.push(Finding {
            code: "NK1236",
            severity: Severity::Error,
            span: *span,
            message: format!("`@{name}` stands twice before one declaration."),
            notes: vec![format!(
                "An attribute stands once unless its mark says `repeatable: true`, and \
                 `{name}`'s does not."
            )],
            help: Some(format!(
                "Remove one, or write `@meta::Attribute(…; repeatable: true)` before `struct {name}`."
            )),
            labels: Vec::new(),
        });
    }
}

/// The places a mark names, read off its arguments; a word that is not one is
/// refused where the mark is checked.
fn mark_of(unit: &Parsed, mark: &ast::Attribute) -> Mark {
    let places = mark
        .args
        .iter()
        .filter_map(|arg| match arg {
            Expr::Variable(word) => Some(unit.text(*word).to_string()),
            Expr::Path(segments) => segments.last().map(|s| unit.text(*s).to_lowercase()),
            _ => None,
        })
        .filter(|word| PLACES.contains(&word.as_str()))
        .collect();
    let repeatable = mark.config.iter().any(|option| {
        unit.text(option.name) == "repeatable" && matches!(option.value, Expr::LitBool(true))
    });
    Mark { places, repeatable }
}

fn places_text() -> String {
    PLACES
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn place_or_field(place: &str) -> &str {
    match place {
        "" => "field",
        place => place,
    }
}

/// **A literal that cannot be the type wanted**, said by its kind: a number
/// has no type of its own until a use gives it one, so the general check
/// passes it, and before a declaration there is no later use to ask.
pub(super) fn literal_misfit(literal: &Expr, want: &Ty) -> Option<&'static str> {
    const WHOLE: [&str; 10] = [
        "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64", "u128",
    ];
    let Ty::Named { name, args, .. } = want else {
        return None;
    };
    let known = WHOLE.contains(&name.as_str())
        || matches!(name.as_str(), "f32" | "f64" | "bool" | "String" | "str");
    if !known || !args.is_empty() {
        return None;
    }
    let number = WHOLE.contains(&name.as_str()) || matches!(name.as_str(), "f32" | "f64");
    let literal = match literal {
        Expr::Unary { expr, .. } => expr,
        other => other,
    };
    match literal {
        Expr::LitInt { .. } if !number => Some("a whole number"),
        Expr::LitFloat(_) if !matches!(name.as_str(), "f32" | "f64") => {
            Some("a number with a fraction")
        }
        Expr::LitBool(_) if name != "bool" => Some("a `bool`"),
        Expr::LitStr { .. } if !matches!(name.as_str(), "String" | "str") => Some("text"),
        _ => None,
    }
}
