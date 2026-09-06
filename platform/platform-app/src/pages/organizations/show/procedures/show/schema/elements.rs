//! `…/schema/elements/{eid}`: the POST routes on one element of the
//! revision draft — any of the four kinds (column, group, section,
//! note) — [`element::update`] (the detail form),
//! [`element::relocate`] (move up / down / to), [`element::remove`],
//! and the enum column's [`element::options`].
//! The segment is the element id as the API spells it (opaque text,
//! `path_param!` on `String`); an id the draft does not hold is
//! `INVALID_EDIT` from the schema, shown as the editor's alert.
//!
//! Routing only: what a submission *means* for an element lives in
//! [`super::edit`], which the autosave procedures call by the same
//! door these routes do.

use cynic::MutationBuilder;
use platform_client::Error;
use platform_client::revision_draft::{
    Element, EnumOptionInput, MoveElement, MoveElementInput, MoveElementVariables, PlacementInput,
    RemoveElement, RemoveElementInput, RemoveElementVariables,
};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, page, path_param},
    view::View,
};

use crate::{client, i18n::t};

use super::super::procedure_draft;
use super::edit::{Fields, apply_update, enum_column, rename_option, set_options};
use super::element::{id_of, label_of, parent_of};
use super::{Notice, NoticeKind, back_to_editor, done, done_with, refused};

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
        ) -> Result<impl View> {
            let client = client(cx).await?;
            let procedure = procedure_draft(cx).await?;
            let element_id = path_param::<ElementId>(cx).to_owned();
            let fields = Fields::from_pairs(pairs);
            let notice = match apply_update(cx, &client, &procedure, &element_id, &fields).await? {
                Ok(_) => done(cx, "schema.notice.saved").await?,
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
        pub(in crate::pages) async fn submit(
            cx: &Cx,
            Form(input): Form<Relocation>,
        ) -> Result<impl View> {
            let client = client(cx).await?;
            let procedure = procedure_draft(cx).await?;
            let element_id = path_param::<ElementId>(cx).to_owned();
            let elements = procedure.revision_draft.elements.as_slice();
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
        pub(in crate::pages) async fn submit(cx: &Cx) -> Result<impl View> {
            let client = client(cx).await?;
            let procedure = procedure_draft(cx).await?;
            let element_id = path_param::<ElementId>(cx).to_owned();
            let removed = procedure
                .revision_draft
                .elements
                .iter()
                .find(|e| id_of(e) == element_id.as_str());
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
            pub(in crate::pages) async fn submit(
                cx: &Cx,
                Form(input): Form<Addition>,
            ) -> Result<impl View> {
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
                                Ok(_) => {
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
            pub(in crate::pages) async fn submit(
                cx: &Cx,
                Form(input): Form<Change>,
            ) -> Result<impl View> {
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
                    Ok(_) => done(cx, "schema.notice.saved").await?,
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
            pub(in crate::pages) async fn submit(
                cx: &Cx,
                Form(input): Form<Removal>,
            ) -> Result<impl View> {
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
                            Ok(_) => {
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
