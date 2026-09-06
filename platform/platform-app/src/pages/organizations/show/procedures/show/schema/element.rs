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
use topcoat::{
    Result,
    icon::{IconData, icon, iconify::iconify_icon},
    view::{View, attributes, component, view},
};

use crate::components::badge::{BadgeVariant, badge};

/// The **audience marker**: the badge a reviewer-only element wears,
/// in the editor's tree and on the preview's captions alike.
///
/// It must not read as one more type badge, so it takes an amber
/// tint — a colour outside the theme tokens, with dark values of its
/// own, recorded in platform.md P.4 as the a11y contract asks — and
/// an eye-off glyph. The text is what carries the meaning; the
/// colour never does.
#[component]
pub(in crate::pages) async fn reviewer_badge(text: String) -> Result<impl View> {
    Ok(view! {
        badge(
            variant: BadgeVariant::Outline,
            attrs: attributes! {
                class="border-transparent bg-amber-100 text-amber-900 \
                    dark:bg-amber-500/15 dark:text-amber-300"
            },
            icon(
                data: iconify_icon!("lucide:eye-off"),
                attrs: attributes! { class="size-3" }
            )
            (text.as_str())
        )
    })
}

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
/// from assistive tech. Ids resolve against the staged lucide set
/// at compile time (`build.rs`); a mistyped id fails the build.
pub(in crate::pages) fn element_icon(element: &Element) -> IconData {
    match element {
        Element::Column(c) => match kind_of(&c.ty) {
            "BOOLEAN" => iconify_icon!("lucide:square-check"),
            "INTEGER" => iconify_icon!("lucide:hash"),
            "DECIMAL" => iconify_icon!("lucide:percent"),
            "DATE" => iconify_icon!("lucide:calendar"),
            "DATETIME" => iconify_icon!("lucide:clock"),
            "ENUM" => iconify_icon!("lucide:list"),
            "ATTACHMENT" => iconify_icon!("lucide:paperclip"),
            "GEOMETRY" => iconify_icon!("lucide:map-pin"),
            _ => iconify_icon!("lucide:type"),
        },
        Element::Group(_) => iconify_icon!("lucide:folder"),
        Element::Section(_) => iconify_icon!("lucide:bookmark"),
        Element::Note(_) => iconify_icon!("lucide:info"),
        Element::Unknown => iconify_icon!("lucide:circle"),
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
