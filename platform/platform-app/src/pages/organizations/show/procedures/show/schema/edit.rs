//! The **edit application**: one place where a submitted (or
//! autosaved) set of fields becomes a mutation on one element of the
//! revision draft.
//!
//! The rule the whole module exists to hold: **the stored element
//! supplies every fact the submission did not send**, so changing one
//! field never resets the others. The detail form posts all of its
//! fields at once; the runtime posts one at a time
//! ([`super::save_field`]); both arrive here as [`Fields`] and take
//! the same path. Where a value cannot be derived from one field
//! alone — a text column's format, which is the *pair* (choice,
//! pattern) — the caller must send the pair, and
//! [`super::save_text_format`] is the procedure that does.
//!
//! This is deliberately not in [`super::elements`]: that module is
//! the POST routes, and the routes are a caller of this logic like
//! the procedures are, not its owner.

use platform_client::revision_draft::{
    AttachmentType, Audience, Cardinality, Column, ColumnType, ColumnTypeInput, Element,
    EnumOptionInput, ProcedureRevisionDraft, RegexFormatInput, TextFormatInput, UpdateColumn,
    UpdateColumnInput, UpdateColumnVariables, UpdateGroup, UpdateGroupInput, UpdateGroupVariables,
    UpdateNote, UpdateNoteInput, UpdateNoteVariables, UpdateSection, UpdateSectionInput,
    UpdateSectionVariables,
};

use cynic::MutationBuilder;
use topcoat::{Result, context::Cx};

use crate::i18n::t;

use super::element::{id_of, kind_of, multiple_of, text_format_of, unit_from_name, unit_of};
use super::{Notice, NoticeKind, refused};

/// A submitted form as ordered pairs — repeated names (the enum
/// option rows) keep their order, which a map would lose.
pub(in crate::pages) struct Fields(Vec<(String, String)>);

impl Fields {
    pub(in crate::pages) fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        Self(pairs)
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The last value under `name` — for controls that post a hidden
    /// fallback before the real input (the required switch: hidden
    /// `false`, then the checkbox's `true` when checked).
    fn last(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A fresh column's type from its kind alone (no unit, options, or
/// constraints yet — those come through the detail form). Unknown
/// kinds are text.
pub(in crate::pages) fn kind_input(kind: &str) -> ColumnTypeInput {
    match kind {
        "BOOLEAN" => ColumnTypeInput::boolean(),
        "INTEGER" => ColumnTypeInput::integer(None),
        "DECIMAL" => ColumnTypeInput::decimal(None),
        "DATE" => ColumnTypeInput::date(),
        "DATETIME" => ColumnTypeInput::datetime(),
        "ENUM" => ColumnTypeInput::enumeration(Vec::new(), false),
        "ATTACHMENT" => ColumnTypeInput::attachment(Vec::new(), None, false),
        "GEOMETRY" => ColumnTypeInput::geometry(false),
        _ => ColumnTypeInput::text(),
    }
}

/// A column's inline options as inputs that keep their ids.
fn current_options(column: &Column) -> Vec<EnumOptionInput> {
    match &column.ty {
        ColumnType::Enum(e) => e
            .options
            .iter()
            .map(|o| EnumOptionInput {
                id: Some(o.id.clone()),
                label: o.label.clone(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Stores `options` as the enum column `element_id`'s backing (its
/// label and arity untouched). `Ok` is the draft as it now stands;
/// `Err` is the notice to show.
pub(in crate::pages) async fn set_options(
    cx: &Cx,
    client: &platform_graphql::InProcess,
    procedure: &ProcedureRevisionDraft,
    element_id: &str,
    options: Vec<EnumOptionInput>,
) -> Result<std::result::Result<ProcedureRevisionDraft, Notice>> {
    let (column, _) = match enum_column(cx, procedure, element_id).await? {
        Ok(found) => found,
        Err(notice) => return Ok(Err(notice)),
    };
    let result = platform_client::run(
        client,
        UpdateColumn::build(UpdateColumnVariables {
            input: UpdateColumnInput {
                procedure_id: cynic::Id::new(procedure.id.inner()),
                id: cynic::Id::new(element_id),
                label: None,
                ty: Some(ColumnTypeInput::enumeration(
                    options,
                    multiple_of(&column.ty),
                )),
                required: None,
                audience: None,
            },
        }),
    )
    .await;
    match result {
        Ok(updated) => Ok(Ok(updated.update_column)),
        Err(error) => Ok(Err(refused(cx, error).await?)),
    }
}

/// The enum column `element_id` of `procedure`'s draft and its
/// options; the conflict notice when it is not one.
pub(in crate::pages) async fn enum_column<'a>(
    cx: &Cx,
    procedure: &'a ProcedureRevisionDraft,
    element_id: &str,
) -> Result<std::result::Result<(&'a Column, Vec<EnumOptionInput>), Notice>> {
    let column = procedure
        .revision_draft
        .elements
        .iter()
        .find_map(|e| match e {
            Element::Column(c) if c.id.inner() == element_id => Some(c),
            _ => None,
        })
        .filter(|c| matches!(c.ty, ColumnType::Enum(_)));
    match column {
        Some(column) => Ok(Ok((column, current_options(column)))),
        None => Ok(Err(Notice {
            kind: NoticeKind::Alert,
            text: t(cx, "schema.error.conflict").await?,
        })),
    }
}

/// One option's change from the autosave or the row's form: `Ok` is
/// the draft as it now stands, `Err` the notice to show.
pub(in crate::pages) async fn rename_option(
    cx: &Cx,
    client: &platform_graphql::InProcess,
    procedure: &ProcedureRevisionDraft,
    element_id: &str,
    option_id: &str,
    label: &str,
) -> Result<std::result::Result<ProcedureRevisionDraft, Notice>> {
    let label = label.trim();
    if label.is_empty() {
        return Ok(Err(Notice {
            kind: NoticeKind::Alert,
            text: t(cx, "schema.error.label-required").await?,
        }));
    }
    let (_, mut options) = match enum_column(cx, procedure, element_id).await? {
        Ok(found) => found,
        Err(notice) => return Ok(Err(notice)),
    };
    match options
        .iter_mut()
        .find(|o| o.id.as_ref().is_some_and(|id| id.inner() == option_id))
    {
        Some(option) => option.label = label.to_owned(),
        None => {
            return Ok(Err(Notice {
                kind: NoticeKind::Alert,
                text: t(cx, "schema.error.conflict").await?,
            }));
        }
    }
    set_options(cx, client, procedure, element_id, options).await
}

/// Applies `fields` to the element `element_id` of `procedure`'s
/// draft: the current element supplies every fact the form did not
/// send, so one changed field never resets the others. `Err` is the
/// notice to show (a refused edit, a bad value); the outer error is
/// the request's.
pub(in crate::pages) async fn apply_update(
    cx: &Cx,
    client: &platform_graphql::InProcess,
    procedure: &ProcedureRevisionDraft,
    element_id: &str,
    fields: &Fields,
) -> Result<std::result::Result<ProcedureRevisionDraft, Notice>> {
    let elements = procedure.revision_draft.elements.as_slice();
    let Some(element) = elements.iter().find(|e| id_of(e) == element_id) else {
        return Ok(Err(Notice {
            kind: NoticeKind::Alert,
            text: t(cx, "schema.error.conflict").await?,
        }));
    };
    let label = match fields.get("label") {
        Some(label) if label.trim().is_empty() => {
            return Ok(Err(Notice {
                kind: NoticeKind::Alert,
                text: t(cx, "schema.error.label-required").await?,
            }));
        }
        Some(label) => Some(label.trim().to_owned()),
        None => None,
    };
    let procedure_id = cynic::Id::new(procedure.id.inner());
    let id = cynic::Id::new(element_id);
    // The audience select posts `ALL` / `REVIEWER` on every kind.
    let audience = fields.get("audience").map(|value| match value {
        "REVIEWER" => Audience::Reviewer,
        _ => Audience::All,
    });
    let result = match element {
        Element::Group(_) => {
            let cardinality = match fields.get("cardinality") {
                Some("MANY") => Some(Cardinality::Many),
                Some(_) => Some(Cardinality::One),
                None => None,
            };
            platform_client::run(
                client,
                UpdateGroup::build(UpdateGroupVariables {
                    input: UpdateGroupInput {
                        procedure_id,
                        id,
                        label,
                        cardinality,
                        audience,
                    },
                }),
            )
            .await
            .map(|updated| updated.update_group)
        }
        Element::Column(column) => {
            let ty = match column_type_input(cx, column, fields).await? {
                Ok(ty) => ty,
                Err(notice) => return Ok(Err(notice)),
            };
            platform_client::run(
                client,
                UpdateColumn::build(UpdateColumnVariables {
                    input: UpdateColumnInput {
                        procedure_id,
                        id,
                        label,
                        ty,
                        required: fields.last("required").map(|value| value == "true"),
                        audience,
                    },
                }),
            )
            .await
            .map(|updated| updated.update_column)
        }
        Element::Section(_) => {
            let title = match fields.get("title") {
                Some(title) if title.trim().is_empty() => {
                    return Ok(Err(Notice {
                        kind: NoticeKind::Alert,
                        text: t(cx, "schema.error.title-required").await?,
                    }));
                }
                Some(title) => Some(title.trim().to_owned()),
                None => None,
            };
            // A blank help clears it (the server collapses blank to
            // null); absent leaves it.
            let help = fields.get("help").map(|help| help.trim().to_owned());
            platform_client::run(
                client,
                UpdateSection::build(UpdateSectionVariables {
                    input: UpdateSectionInput {
                        procedure_id,
                        id,
                        title,
                        help,
                        audience,
                    },
                }),
            )
            .await
            .map(|updated| updated.update_section)
        }
        Element::Note(_) => {
            let body = match fields.get("body") {
                Some(body) if body.trim().is_empty() => {
                    return Ok(Err(Notice {
                        kind: NoticeKind::Alert,
                        text: t(cx, "schema.error.body-required").await?,
                    }));
                }
                Some(body) => Some(body.trim().to_owned()),
                None => None,
            };
            // A blank title clears it; absent leaves it.
            let title = fields.get("title").map(|title| title.trim().to_owned());
            platform_client::run(
                client,
                UpdateNote::build(UpdateNoteVariables {
                    input: UpdateNoteInput {
                        procedure_id,
                        id,
                        title,
                        body,
                        audience,
                    },
                }),
            )
            .await
            .map(|updated| updated.update_note)
        }
        // Unreachable through the editor: `Element::Unknown` carries
        // no id, so the lookup above cannot land on it. Nothing was
        // sent, so nothing changed.
        Element::Unknown => Ok(procedure.clone()),
    };
    match result {
        Ok(updated) => Ok(Ok(updated)),
        Err(error) => Ok(Err(refused(cx, error).await?)),
    }
}

/// The column's type after `fields`: the submitted kind (or the
/// current one) with the facts the form sent, the current facts
/// otherwise. `None` when nothing type-related was sent.
async fn column_type_input(
    cx: &Cx,
    column: &Column,
    fields: &Fields,
) -> Result<std::result::Result<Option<ColumnTypeInput>, Notice>> {
    let touched = [
        "kind",
        "unit",
        "accept",
        "max_bytes",
        "arity",
        "format",
        "pattern",
    ]
    .iter()
    .any(|name| fields.get(name).is_some());
    if !touched {
        return Ok(Ok(None));
    }
    let kind = fields.get("kind").unwrap_or(kind_of(&column.ty));
    let current_unit = unit_of(&column.ty);
    let unit = match fields.get("unit") {
        Some("") => None,
        Some(name) => match unit_from_name(name) {
            Some(unit) => Some(unit),
            None => {
                return Ok(Err(Notice {
                    kind: NoticeKind::Alert,
                    text: t(cx, "schema.error.unit").await?,
                }));
            }
        },
        None => current_unit,
    };
    // Many values: the form's select when sent (it is posted with the
    // whole form even when hidden, so only a list-capable kind reads
    // it), else what the column already has.
    let multiple = match fields.get("arity") {
        Some(value) if matches!(kind, "ENUM" | "ATTACHMENT" | "GEOMETRY") => value == "MANY",
        Some(_) => false,
        None => multiple_of(&column.ty),
    };
    let input = match kind {
        "BOOLEAN" => ColumnTypeInput::boolean(),
        "INTEGER" => ColumnTypeInput::integer(unit),
        "DECIMAL" => ColumnTypeInput::decimal(unit),
        "DATE" => ColumnTypeInput::date(),
        "DATETIME" => ColumnTypeInput::datetime(),
        "GEOMETRY" => ColumnTypeInput::geometry(multiple),
        // Options are edited through `element::options`; a column
        // becoming an enum starts with none — an empty choice is a
        // draft state, refused at publication, not here.
        "ENUM" => ColumnTypeInput::enumeration(current_options(column), multiple),
        "ATTACHMENT" => {
            let (current_accept, current_max) = match &column.ty {
                ColumnType::Attachment(AttachmentType {
                    accept, max_bytes, ..
                }) => (accept.clone(), *max_bytes),
                _ => (Vec::new(), None),
            };
            let accept = match fields.get("accept") {
                Some(list) => list
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
                None => current_accept,
            };
            let max_bytes = match fields.get("max_bytes") {
                Some("") => None,
                Some(n) => match n.trim().parse::<i32>() {
                    Ok(n) if n > 0 => Some(n),
                    _ => {
                        return Ok(Err(Notice {
                            kind: NoticeKind::Alert,
                            text: t(cx, "schema.error.max-bytes").await?,
                        }));
                    }
                },
                None => current_max,
            };
            ColumnTypeInput::attachment(accept, max_bytes, multiple)
        }
        _ => {
            // TEXT: the format select posts "", EMAIL, PHONE, IBAN or
            // REGEX, the pattern input its text; absent fields keep
            // the column's current constraint (§2.6 — the server
            // verifies patterns on the linear-time engine).
            let (current_format, current_pattern) = text_format_of(&column.ty);
            let choice = fields.get("format").unwrap_or(current_format);
            let pattern = fields
                .get("pattern")
                .map(str::trim)
                .unwrap_or(current_pattern);
            let format = match choice {
                "EMAIL" => Some(TextFormatInput {
                    email: Some(true),
                    ..Default::default()
                }),
                "PHONE" => Some(TextFormatInput {
                    phone: Some(true),
                    ..Default::default()
                }),
                "IBAN" => Some(TextFormatInput {
                    iban: Some(true),
                    ..Default::default()
                }),
                "REGEX" => {
                    if pattern.is_empty() {
                        return Ok(Err(Notice {
                            kind: NoticeKind::Alert,
                            text: t(cx, "schema.error.pattern-required").await?,
                        }));
                    }
                    Some(TextFormatInput {
                        regex: Some(RegexFormatInput {
                            pattern: pattern.to_owned(),
                        }),
                        ..Default::default()
                    })
                }
                _ => None,
            };
            ColumnTypeInput::text_with_format(format)
        }
    };
    Ok(Ok(Some(input)))
}
