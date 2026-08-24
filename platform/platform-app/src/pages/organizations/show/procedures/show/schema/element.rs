//! The **element helpers**: the small total functions every part of
//! the editor reads a `platform_client` [`Element`] through — its
//! id, parent, row text and kind, a column's type spelled as the
//! form values the editor posts, and the audience rule the tree
//! resolves against (P.4 inheritance).
//!
//! They live apart from the views and the routes because all three
//! sides need them — the editor's structure panel, the preview, and
//! the POST routes that apply an edit — and none of them owns the
//! mapping. Every function here is pure and total: `Element::Unknown`
//! (a kind this client does not know) answers with the empty or
//! widest value rather than panicking.

use platform_client::revision_draft::{Audience, ColumnType, Element, TextFormat, Unit};
use topcoat::icon::{IconData, iconify::iconify_icon};

pub(in crate::pages) fn id_of(element: &Element) -> &str {
    match element {
        Element::Column(c) => c.id.inner(),
        Element::Group(g) => g.id.inner(),
        Element::Section(s) => s.id.inner(),
        Element::Note(n) => n.id.inner(),
        Element::Unknown => "",
    }
}

pub(in crate::pages) fn parent_of(element: &Element) -> Option<String> {
    match element {
        Element::Column(c) => c.parent_id.as_ref().map(|p| p.inner().to_owned()),
        Element::Group(g) => g.parent_id.as_ref().map(|p| p.inner().to_owned()),
        Element::Section(s) => s.parent_id.as_ref().map(|p| p.inner().to_owned()),
        Element::Note(n) => n.parent_id.as_ref().map(|p| p.inner().to_owned()),
        Element::Unknown => None,
    }
}

/// The row text: a column or group's label, a section's title, a
/// note's title or its text.
pub(in crate::pages) fn label_of(element: &Element) -> &str {
    match element {
        Element::Column(c) => &c.label,
        Element::Group(g) => &g.label,
        Element::Section(s) => &s.title,
        Element::Note(n) => n.title.as_deref().unwrap_or(&n.body),
        Element::Unknown => "",
    }
}

/// A text column's format as the editor's form values: the select's
/// choice and the custom pattern ("" where absent).
pub(in crate::pages) fn text_format_of(ty: &ColumnType) -> (&'static str, &str) {
    match ty {
        ColumnType::Text(text) => match &text.format {
            Some(TextFormat::Email(_)) => ("EMAIL", ""),
            Some(TextFormat::Phone(_)) => ("PHONE", ""),
            Some(TextFormat::Iban(_)) => ("IBAN", ""),
            Some(TextFormat::Regex(regex)) => ("REGEX", regex.pattern.as_str()),
            Some(TextFormat::Unknown) | None => ("", ""),
        },
        _ => ("", ""),
    }
}

/// The row's `data-element-kind`.
pub(in crate::pages) fn element_kind(element: &Element) -> &'static str {
    match element {
        Element::Column(_) => "column",
        Element::Group(_) => "group",
        Element::Section(_) => "section",
        Element::Note(_) => "note",
        Element::Unknown => "",
    }
}

/// The row's leading icon: a column's by its type, the other kinds
/// by what they are. Purely decorative — the kind badge carries the
/// words — so no `label`: the icon component hides unlabelled icons
/// from assistive tech. Ids resolve against the staged feather set
/// at compile time (`build.rs`); a mistyped id fails the build.
pub(in crate::pages) fn element_icon(element: &Element) -> IconData {
    match element {
        Element::Column(c) => match kind_of(&c.ty) {
            "BOOLEAN" => iconify_icon!("feather:check-square"),
            "INTEGER" => iconify_icon!("feather:hash"),
            "DECIMAL" => iconify_icon!("feather:percent"),
            "DATE" => iconify_icon!("feather:calendar"),
            "DATETIME" => iconify_icon!("feather:clock"),
            "ENUM" => iconify_icon!("feather:list"),
            "ATTACHMENT" => iconify_icon!("feather:paperclip"),
            "GEOMETRY" => iconify_icon!("feather:map-pin"),
            _ => iconify_icon!("feather:type"),
        },
        Element::Group(_) => iconify_icon!("feather:folder"),
        Element::Section(_) => iconify_icon!("feather:bookmark"),
        Element::Note(_) => iconify_icon!("feather:info"),
        Element::Unknown => iconify_icon!("feather:circle"),
    }
}

/// The element's own audience marker (its *effective* audience is
/// [`Tree::reviewer_only`]'s business — inheritance, platform P.4).
/// The element's *effective* audience is reviewer-only: its own
/// marker, or any ancestor's (platform P.4 — inheritance).
pub(in crate::pages) fn effectively_reviewer(elements: &[Element], id: &str) -> bool {
    let mut current = Some(id.to_owned());
    while let Some(current_id) = current {
        let Some(element) = elements.iter().find(|e| id_of(e) == current_id) else {
            return false;
        };
        if audience_of(element) == Audience::Reviewer {
            return true;
        }
        current = parent_of(element);
    }
    false
}

pub(in crate::pages) fn audience_of(element: &Element) -> Audience {
    match element {
        Element::Column(c) => c.audience,
        Element::Group(g) => g.audience,
        Element::Section(s) => s.audience,
        Element::Note(n) => n.audience,
        Element::Unknown => Audience::All,
    }
}

/// The kinds the editor offers, as the API spells them.
pub(in crate::pages) const KINDS: &[&str] = &[
    "TEXT",
    "BOOLEAN",
    "INTEGER",
    "DECIMAL",
    "DATE",
    "DATETIME",
    "ENUM",
    "ATTACHMENT",
    "GEOMETRY",
];

/// Whether a column holds many values — `Some` for the types that
/// carry the fact (choice, attachment, geometry), `None` otherwise.
pub(in crate::pages) fn multiple_of_type(ty: &ColumnType) -> Option<bool> {
    match ty {
        ColumnType::Enum(e) => Some(e.multiple),
        ColumnType::Attachment(a) => Some(a.multiple),
        ColumnType::Geometry(g) => Some(g.multiple),
        _ => None,
    }
}

/// [`multiple_of_type`], `false` where the fact does not apply.
pub(in crate::pages) fn multiple_of(ty: &ColumnType) -> bool {
    multiple_of_type(ty).unwrap_or(false)
}

/// A number column's unit; `None` for any other column.
pub(in crate::pages) fn unit_of(ty: &ColumnType) -> Option<Unit> {
    match ty {
        ColumnType::Integer(n) => n.unit,
        ColumnType::Decimal(n) => n.unit,
        _ => None,
    }
}

pub(in crate::pages) fn kind_of(ty: &ColumnType) -> &'static str {
    match ty {
        ColumnType::Text(_) => "TEXT",
        ColumnType::Boolean(_) => "BOOLEAN",
        ColumnType::Integer(_) => "INTEGER",
        ColumnType::Decimal(_) => "DECIMAL",
        ColumnType::Date(_) => "DATE",
        ColumnType::Datetime(_) => "DATETIME",
        ColumnType::Enum(_) => "ENUM",
        ColumnType::Attachment(_) => "ATTACHMENT",
        ColumnType::Geometry(_) => "GEOMETRY",
        ColumnType::Unknown => "TEXT",
    }
}

pub(in crate::pages) fn kind_message_id(ty: &ColumnType) -> &'static str {
    kind_message_id_of(kind_of(ty))
}

pub(in crate::pages) fn kind_message_id_of(kind: &str) -> &'static str {
    match kind {
        "BOOLEAN" => "schema.kind.boolean",
        "INTEGER" => "schema.kind.integer",
        "DECIMAL" => "schema.kind.decimal",
        "DATE" => "schema.kind.date",
        "DATETIME" => "schema.kind.datetime",
        "ENUM" => "schema.kind.enum",
        "ATTACHMENT" => "schema.kind.attachment",
        "GEOMETRY" => "schema.kind.geometry",
        _ => "schema.kind.text",
    }
}

/// Every unit, in the API's order.
pub(in crate::pages) const UNITS: &[Unit] = &[
    Unit::Millimetre,
    Unit::Centimetre,
    Unit::Metre,
    Unit::Kilometre,
    Unit::Gram,
    Unit::Kilogram,
    Unit::Tonne,
    Unit::Minute,
    Unit::Hour,
    Unit::Day,
    Unit::Week,
    Unit::Month,
    Unit::Year,
    Unit::SquareMetre,
    Unit::Hectare,
    Unit::SquareKilometre,
    Unit::Litre,
    Unit::CubicMetre,
    Unit::Percent,
];

/// The unit's symbol, also its form value (the kernel's spelling).
pub(in crate::pages) fn unit_name(unit: Unit) -> &'static str {
    match unit {
        Unit::Millimetre => "mm",
        Unit::Centimetre => "cm",
        Unit::Metre => "m",
        Unit::Kilometre => "km",
        Unit::Gram => "g",
        Unit::Kilogram => "kg",
        Unit::Tonne => "t",
        Unit::Minute => "minute",
        Unit::Hour => "hour",
        Unit::Day => "day",
        Unit::Week => "week",
        Unit::Month => "month",
        Unit::Year => "year",
        Unit::SquareMetre => "m2",
        Unit::Hectare => "ha",
        Unit::SquareKilometre => "km2",
        Unit::Litre => "L",
        Unit::CubicMetre => "m3",
        Unit::Percent => "percent",
    }
}

pub(in crate::pages) fn unit_from_name(name: &str) -> Option<Unit> {
    UNITS.iter().copied().find(|u| unit_name(*u) == name)
}
