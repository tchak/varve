//! `…/schema/elements/{eid}`: the POST routes on one element of the
//! revision draft — [`element::update`] (the detail form),
//! [`element::relocate`] (move up / down / to), [`element::remove`]
//! — and [`apply_update`], the one field application both the form
//! and the autosave procedure go through.
//! The segment is the element id as the API spells it (opaque text,
//! `path_param!` on `String`); an id the draft does not hold is
//! `INVALID_EDIT` from the schema, shown as the editor's alert.

use cynic::MutationBuilder;
use platform_client::Error;
use platform_client::revision_draft::{
    Arity, AttachmentType, Cardinality, ColumnType, ColumnTypeInput, EnumOptionInput, MoveElement,
    MoveElementInput, MoveElementVariables, PlacementInput, ProcedureRevisionDraft, RemoveElement,
    RemoveElementInput, RemoveElementVariables, SchemaColumn, SchemaElement, UpdateColumn,
    UpdateColumnInput, UpdateColumnVariables, UpdateGroup, UpdateGroupInput, UpdateGroupVariables,
};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, page, path_param},
};

use crate::{client, i18n::t};

use super::super::procedure_draft;
use super::{
    Notice, NoticeKind, back_to_editor, done, id_of, kind_of, parent_of, refused, unit_from_name,
    unit_of,
};

/// A submitted form as ordered pairs — repeated names (the enum
/// option rows) keep their order, which a map would lose.
pub(super) struct Fields(Vec<(String, String)>);

impl Fields {
    pub(super) fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        Self(pairs)
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn all(&self, name: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

/// Applies `fields` to the element `element_id` of `procedure`'s
/// draft: the current element supplies every fact the form did not
/// send, so one changed field never resets the others. `Err` is the
/// notice to show (a refused edit, a bad value); the outer error is
/// the request's.
pub(super) async fn apply_update(
    cx: &Cx,
    client: &platform_graphql::InProcess,
    procedure: &ProcedureRevisionDraft,
    element_id: &str,
    fields: &Fields,
) -> Result<std::result::Result<(), Notice>> {
    let elements = procedure
        .revision_draft
        .as_ref()
        .map(|d| d.schema.elements.as_slice())
        .unwrap_or_default();
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
    let result = match element {
        SchemaElement::Group(_) => {
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
                    },
                }),
            )
            .await
            .map(|_| ())
        }
        SchemaElement::Column(column) => {
            let ty = match column_type_input(cx, column, fields).await? {
                Ok(ty) => ty,
                Err(notice) => return Ok(Err(notice)),
            };
            let arity = match fields.get("arity") {
                Some("MANY") => Some(Arity::Many),
                Some(_) => Some(Arity::One),
                None => None,
            };
            platform_client::run(
                client,
                UpdateColumn::build(UpdateColumnVariables {
                    input: UpdateColumnInput {
                        procedure_id,
                        id,
                        label,
                        ty,
                        arity,
                    },
                }),
            )
            .await
            .map(|_| ())
        }
        SchemaElement::Unknown => Ok(()),
    };
    match result {
        Ok(()) => Ok(Ok(())),
        Err(error) => Ok(Err(refused(cx, error).await?)),
    }
}

/// The column's type after `fields`: the submitted kind (or the
/// current one) with the facts the form sent, the current facts
/// otherwise. `None` when nothing type-related was sent.
async fn column_type_input(
    cx: &Cx,
    column: &SchemaColumn,
    fields: &Fields,
) -> Result<std::result::Result<Option<ColumnTypeInput>, Notice>> {
    let touched = ["kind", "unit", "option_label", "accept", "max_bytes"]
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
    let input = match kind {
        "BOOLEAN" => ColumnTypeInput::boolean(),
        "INTEGER" => ColumnTypeInput::integer(unit),
        "DECIMAL" => ColumnTypeInput::decimal(unit),
        "DATE" => ColumnTypeInput::date(),
        "DATETIME" => ColumnTypeInput::datetime(),
        "GEOMETRY" => ColumnTypeInput::geometry(),
        "ENUM" => {
            let options = if fields.get("option_label").is_some() {
                // Rows in order: a blank label drops its row; a row
                // with an id keeps its identity, a new row is minted.
                let ids = fields.all("option_id");
                fields
                    .all("option_label")
                    .into_iter()
                    .enumerate()
                    .filter(|(_, label)| !label.trim().is_empty())
                    .map(|(row, label)| EnumOptionInput {
                        id: ids
                            .get(row)
                            .filter(|id| !id.is_empty())
                            .map(|id| cynic::Id::new(*id)),
                        label: label.trim().to_owned(),
                    })
                    .collect()
            } else {
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
            };
            if options.is_empty() {
                return Ok(Err(Notice {
                    kind: NoticeKind::Alert,
                    text: t(cx, "schema.error.options-required").await?,
                }));
            }
            ColumnTypeInput::enumeration(options)
        }
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
            ColumnTypeInput::attachment(accept, max_bytes)
        }
        _ => ColumnTypeInput::text(),
    };
    Ok(Ok(Some(input)))
}

/// A move: `direction` (`up` / `down`) among siblings, or `parent`
/// (a group id, empty for the top level) to move into, appended.
#[derive(Deserialize)]
pub(super) struct Relocation {
    #[serde(default)]
    direction: String,
    parent: Option<String>,
}

/// `…/schema/elements/{eid}`: the element segment — this module keeps
/// the literal `elements` segment, and the parameter lives one level
/// below it, where `path_param!` replaces the module segment.
pub(super) mod element {
    use super::*;

    path_param!(pub(in crate::pages) element_id);

    /// `…/elements/{eid}/update`: [`update::submit`].
    pub(in crate::pages) mod update {
        use super::*;

        /// The detail form: every field present is applied; absent fields
        /// (the autosave sends one at a time) keep their value.
        #[page(POST)]
        pub(in crate::pages) async fn submit(
            cx: &Cx,
            Form(pairs): Form<Vec<(String, String)>>,
        ) -> Result {
            let client = client(cx).await?;
            let procedure = procedure_draft(cx).await?;
            let element_id = path_param::<ElementId>(cx).to_owned();
            let fields = Fields::from_pairs(pairs);
            let notice = match apply_update(cx, &client, &procedure, &element_id, &fields).await? {
                Ok(()) => done(cx, "schema.notice.saved").await?,
                Err(notice) => notice,
            };
            back_to_editor(cx, Some(&element_id), Some(notice)).await
        }
    }

    /// `…/elements/{eid}/relocate`: [`relocate::submit`].
    pub(in crate::pages) mod relocate {
        use super::*;

        /// `moveElement`. Up and down are computed here from the draft's
        /// document order into the API's sibling anchor: up = before the
        /// previous sibling; down = before the sibling after the next one,
        /// or appended when the next one is the last. At an edge the move is
        /// a no-op landing back on the element.
        #[page(POST)]
        pub(in crate::pages) async fn submit(cx: &Cx, Form(input): Form<Relocation>) -> Result {
            let client = client(cx).await?;
            let procedure = procedure_draft(cx).await?;
            let element_id = path_param::<ElementId>(cx).to_owned();
            let elements = procedure
                .revision_draft
                .as_ref()
                .map(|d| d.schema.elements.as_slice())
                .unwrap_or_default();
            let placement = match input.parent {
                Some(parent) => PlacementInput {
                    parent_id: (!parent.is_empty()).then(|| cynic::Id::new(parent)),
                    before_id: None,
                },
                None => {
                    let Some(element) = elements.iter().find(|e| id_of(e) == element_id.as_str())
                    else {
                        let notice = Notice {
                            kind: NoticeKind::Alert,
                            text: t(cx, "schema.error.conflict").await?,
                        };
                        return back_to_editor(cx, None, Some(notice)).await;
                    };
                    let parent = parent_of(element);
                    let siblings: Vec<&SchemaElement> =
                        elements.iter().filter(|e| parent_of(e) == parent).collect();
                    let index = siblings
                        .iter()
                        .position(|e| id_of(e) == element_id.as_str())
                        .unwrap_or(0);
                    let before = match input.direction.as_str() {
                        "up" => match index.checked_sub(1) {
                            Some(i) => Some(id_of(siblings[i]).to_owned()),
                            None => return back_to_editor(cx, Some(&element_id), None).await,
                        },
                        "down" => {
                            if index + 1 >= siblings.len() {
                                return back_to_editor(cx, Some(&element_id), None).await;
                            }
                            siblings.get(index + 2).map(|e| id_of(e).to_owned())
                        }
                        _ => return back_to_editor(cx, Some(&element_id), None).await,
                    };
                    PlacementInput {
                        parent_id: parent.map(cynic::Id::new),
                        before_id: before.map(cynic::Id::new),
                    }
                }
            };
            let result = platform_client::run(
                &client,
                MoveElement::build(MoveElementVariables {
                    input: MoveElementInput {
                        procedure_id: cynic::Id::new(procedure.id.inner()),
                        id: cynic::Id::new(element_id.as_str()),
                        placement,
                    },
                }),
            )
            .await;
            let notice = match result {
                Ok(_) => done(cx, "schema.notice.moved").await?,
                Err(error) => refused(cx, error).await?,
            };
            back_to_editor(cx, Some(&element_id), Some(notice)).await
        }
    }

    /// `…/elements/{eid}/remove`: [`remove::submit`].
    pub(in crate::pages) mod remove {
        use super::*;

        /// `removeElement`; the selection moves to the removed element's
        /// parent (or nothing).
        #[page(POST)]
        pub(in crate::pages) async fn submit(cx: &Cx) -> Result {
            let client = client(cx).await?;
            let procedure = procedure_draft(cx).await?;
            let element_id = path_param::<ElementId>(cx).to_owned();
            let parent = procedure.revision_draft.as_ref().and_then(|d| {
                d.schema
                    .elements
                    .iter()
                    .find(|e| id_of(e) == element_id.as_str())
                    .and_then(parent_of)
            });
            let result: std::result::Result<_, Error> = platform_client::run(
                &client,
                RemoveElement::build(RemoveElementVariables {
                    input: RemoveElementInput {
                        procedure_id: cynic::Id::new(procedure.id.inner()),
                        id: cynic::Id::new(element_id.as_str()),
                    },
                }),
            )
            .await;
            match result {
                Ok(_) => {
                    let notice = done(cx, "schema.notice.removed").await?;
                    back_to_editor(cx, parent.as_deref(), Some(notice)).await
                }
                Err(error) => {
                    let notice = refused(cx, error).await?;
                    back_to_editor(cx, Some(&element_id), Some(notice)).await
                }
            }
        }
    }
}
