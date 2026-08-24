//! `…/schema/elements/{eid}`: the POST routes on one element of the
//! revision draft — any of the four kinds (column, group, section,
//! note) — [`element::update`] (the detail form),
//! [`element::relocate`] (move up / down / to), [`element::remove`]
//! — and [`apply_update`], the one field application both the form
//! and the autosave procedure go through.
//! The segment is the element id as the API spells it (opaque text,
//! `path_param!` on `String`); an id the draft does not hold is
//! `INVALID_EDIT` from the schema, shown as the editor's alert.

use cynic::MutationBuilder;
use platform_client::Error;
use platform_client::revision_draft::{
    AttachmentType, Audience, Cardinality, Column, ColumnType, ColumnTypeInput, Element,
    EnumOptionInput, MoveElement, MoveElementInput, MoveElementVariables, PlacementInput,
    ProcedureRevisionDraft, RegexFormatInput, RemoveElement, RemoveElementInput,
    RemoveElementVariables, TextFormatInput, UpdateColumn, UpdateColumnInput,
    UpdateColumnVariables, UpdateGroup, UpdateGroupInput, UpdateGroupVariables, UpdateNote,
    UpdateNoteInput, UpdateNoteVariables, UpdateSection, UpdateSectionInput,
    UpdateSectionVariables,
};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, page, path_param},
};

use crate::{client, i18n::t};

use super::super::procedure_draft;
use super::element::{
    id_of, kind_of, label_of, multiple_of, parent_of, text_format_of, unit_from_name, unit_of,
};
use super::{Notice, NoticeKind, back_to_editor, done, done_with, refused};

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
pub(super) fn kind_input(kind: &str) -> ColumnTypeInput {
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
/// label and arity untouched). `Err` is the notice to show.
pub(super) async fn set_options(
    cx: &Cx,
    client: &platform_graphql::InProcess,
    procedure: &ProcedureRevisionDraft,
    element_id: &str,
    options: Vec<EnumOptionInput>,
) -> Result<std::result::Result<(), Notice>> {
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
        Ok(_) => Ok(Ok(())),
        Err(error) => Ok(Err(refused(cx, error).await?)),
    }
}

/// The enum column `element_id` of `procedure`'s draft and its
/// options; the conflict notice when it is not one.
pub(super) async fn enum_column<'a>(
    cx: &Cx,
    procedure: &'a ProcedureRevisionDraft,
    element_id: &str,
) -> Result<std::result::Result<(&'a Column, Vec<EnumOptionInput>), Notice>> {
    let column = procedure
        .revision_draft
        .as_ref()
        .and_then(|d| {
            d.elements.iter().find_map(|e| match e {
                Element::Column(c) if c.id.inner() == element_id => Some(c),
                _ => None,
            })
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

/// One option's change from the autosave or the row's form: `Err` is
/// the notice to show.
pub(super) async fn rename_option(
    cx: &Cx,
    client: &platform_graphql::InProcess,
    procedure: &ProcedureRevisionDraft,
    element_id: &str,
    option_id: &str,
    label: &str,
) -> Result<std::result::Result<(), Notice>> {
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
        .map(|d| d.elements.as_slice())
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
            .map(|_| ())
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
            .map(|_| ())
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
            .map(|_| ())
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
            .map(|_| ())
        }
        Element::Unknown => Ok(()),
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

/// A move: `direction` (`up` / `down` / `top` / `bottom`) among
/// siblings, `after` a sibling, or `parent` (a group id, empty for
/// the top level) to move into, appended.
#[derive(Deserialize)]
pub(super) struct Relocation {
    /// `up` / `down` / `top` / `bottom` among siblings.
    #[serde(default)]
    direction: String,
    /// A sibling to land right after.
    after: Option<String>,
    /// A group to move into (empty = top level), appended.
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
                .map(|d| d.elements.as_slice())
                .unwrap_or_default();
            let moved_label = elements
                .iter()
                .find(|e| id_of(e) == element_id.as_str())
                .map(|e| label_of(e).to_owned())
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
                    let siblings: Vec<&Element> =
                        elements.iter().filter(|e| parent_of(e) == parent).collect();
                    let index = siblings
                        .iter()
                        .position(|e| id_of(e) == element_id.as_str())
                        .unwrap_or(0);
                    let before = match (input.direction.as_str(), &input.after) {
                        // After a sibling = before the one that follows it
                        // (appended when it is the last).
                        (_, Some(after)) => {
                            let Some(at) = siblings.iter().position(|e| id_of(e) == after) else {
                                return back_to_editor(cx, Some(&element_id), None).await;
                            };
                            siblings
                                .iter()
                                .skip(at + 1)
                                .find(|e| id_of(e) != element_id.as_str())
                                .map(|e| id_of(e).to_owned())
                        }
                        ("up", _) => match index.checked_sub(1) {
                            Some(i) => Some(id_of(siblings[i]).to_owned()),
                            None => return back_to_editor(cx, Some(&element_id), None).await,
                        },
                        ("down", _) => {
                            if index + 1 >= siblings.len() {
                                return back_to_editor(cx, Some(&element_id), None).await;
                            }
                            siblings.get(index + 2).map(|e| id_of(e).to_owned())
                        }
                        ("top", _) => {
                            if index == 0 {
                                return back_to_editor(cx, Some(&element_id), None).await;
                            }
                            Some(id_of(siblings[0]).to_owned())
                        }
                        ("bottom", _) => {
                            if index + 1 >= siblings.len() {
                                return back_to_editor(cx, Some(&element_id), None).await;
                            }
                            None
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
                Ok(_) => done_with(cx, "schema.notice.moved", &moved_label).await?,
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
            let removed = procedure
                .revision_draft
                .as_ref()
                .and_then(|d| d.elements.iter().find(|e| id_of(e) == element_id.as_str()));
            let parent = removed.and_then(parent_of);
            let removed_label = removed.map(|e| label_of(e).to_owned()).unwrap_or_default();
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
                    let notice = done_with(cx, "schema.notice.removed", &removed_label).await?;
                    back_to_editor(cx, parent.as_deref(), Some(notice)).await
                }
                Err(error) => {
                    let notice = refused(cx, error).await?;
                    back_to_editor(cx, Some(&element_id), Some(notice)).await
                }
            }
        }
    }

    /// `…/elements/{eid}/options/{add,update,remove}`: the enum
    /// column's options, one POST each; the row's label input also
    /// autosaves through `save_option`.
    pub(in crate::pages) mod options {
        use super::*;

        /// An option to add: its label.
        #[derive(Deserialize)]
        pub(in crate::pages) struct Addition {
            label: String,
        }

        /// A row's change: the option and its new label.
        #[derive(Deserialize)]
        pub(in crate::pages) struct Change {
            option_id: String,
            label: String,
        }

        /// A removal: the option.
        #[derive(Deserialize)]
        pub(in crate::pages) struct Removal {
            option_id: String,
        }

        pub(in crate::pages) mod add {
            use super::*;

            /// Appends an option (a minted id).
            #[page(POST)]
            pub(in crate::pages) async fn submit(cx: &Cx, Form(input): Form<Addition>) -> Result {
                let client = client(cx).await?;
                let procedure = procedure_draft(cx).await?;
                let element_id = path_param::<ElementId>(cx).to_owned();
                let label = input.label.trim().to_owned();
                let notice = if label.is_empty() {
                    Notice {
                        kind: NoticeKind::Alert,
                        text: t(cx, "schema.error.label-required").await?,
                    }
                } else {
                    match enum_column(cx, &procedure, &element_id).await? {
                        Err(notice) => notice,
                        Ok((_, mut options)) => {
                            let added = label.clone();
                            options.push(EnumOptionInput { id: None, label });
                            match set_options(cx, &client, &procedure, &element_id, options).await?
                            {
                                Ok(()) => {
                                    done_with(cx, "schema.notice.option-added", &added).await?
                                }
                                Err(notice) => notice,
                            }
                        }
                    }
                };
                back_to_editor(cx, Some(&element_id), Some(notice)).await
            }
        }

        pub(in crate::pages) mod update {
            use super::*;

            /// Renames an option (the row's form; Enter without the
            /// script).
            #[page(POST)]
            pub(in crate::pages) async fn submit(cx: &Cx, Form(input): Form<Change>) -> Result {
                let client = client(cx).await?;
                let procedure = procedure_draft(cx).await?;
                let element_id = path_param::<ElementId>(cx).to_owned();
                let notice = match rename_option(
                    cx,
                    &client,
                    &procedure,
                    &element_id,
                    &input.option_id,
                    &input.label,
                )
                .await?
                {
                    Ok(()) => done(cx, "schema.notice.saved").await?,
                    Err(notice) => notice,
                };
                back_to_editor(cx, Some(&element_id), Some(notice)).await
            }
        }

        pub(in crate::pages) mod remove {
            use super::*;

            /// Removes an option (the last one too: an empty choice is
            /// a draft state, publication's to refuse).
            #[page(POST)]
            pub(in crate::pages) async fn submit(cx: &Cx, Form(input): Form<Removal>) -> Result {
                let client = client(cx).await?;
                let procedure = procedure_draft(cx).await?;
                let element_id = path_param::<ElementId>(cx).to_owned();
                let notice = match enum_column(cx, &procedure, &element_id).await? {
                    Err(notice) => notice,
                    Ok((_, options)) => {
                        let removed = options
                            .iter()
                            .find(|o| {
                                o.id.as_ref()
                                    .is_some_and(|id| id.inner() == input.option_id)
                            })
                            .map(|o| o.label.clone())
                            .unwrap_or_default();
                        let kept: Vec<EnumOptionInput> = options
                            .into_iter()
                            .filter(|o| {
                                o.id.as_ref().is_none_or(|id| id.inner() != input.option_id)
                            })
                            .collect();
                        match set_options(cx, &client, &procedure, &element_id, kept).await? {
                            Ok(()) => {
                                done_with(cx, "schema.notice.option-removed", &removed).await?
                            }
                            Err(notice) => notice,
                        }
                    }
                };
                back_to_editor(cx, Some(&element_id), Some(notice)).await
            }
        }
    }
}
