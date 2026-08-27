//! `CaseFile` (full, root only), `CaseFileRef`, the participant link
//! and the schema's first connection (G.13) — the platform catalog
//! half of a case file. Cells, checkpoints and `submitCaseFile`
//! arrive with the kernel edge; the state union carries its one
//! member until then.

use std::collections::HashMap;

use async_graphql::{Context, ID, Object};

use crate::error::{internal, invalid_input};
use crate::member::AccountRef;
use crate::organization::OrganizationRef;
use crate::procedure::ProcedureRef;
use crate::session;

/// The full case file. Visible to its participants and to the
/// owning organization's members (G.13) — the caller resolves that
/// before constructing one.
pub struct CaseFile {
    pub case_file: platform_core::CaseFile,
    pub procedure: ProcedureRef,
}

#[Object]
impl CaseFile {
    async fn id(&self) -> ID {
        ID::from(self.case_file.id)
    }

    async fn created_at(&self) -> jiff::Timestamp {
        self.case_file.created_at
    }

    async fn updated_at(&self) -> jiff::Timestamp {
        self.case_file.updated_at
    }

    /// The lifecycle state (G.2 rule 5): a union of state-specific
    /// objects — one member until the checkpoint machine lands.
    async fn state(&self) -> async_graphql::Result<CaseFileState> {
        case_file_state(&self.case_file)
    }

    /// The procedure this case file was created on.
    async fn procedure(&self) -> &ProcedureRef {
        &self.procedure
    }

    /// The participants, oldest first — every one may see the case
    /// file and later fill its applicant surface (bounded, G.2.4).
    async fn participants(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<CaseFileParticipant>> {
        let (_, mut db) = session(ctx)?;
        let participants = platform_core::list_case_file_participants(&mut db, self.case_file.id)
            .await
            .map_err(internal)?;
        Ok(participants.into_iter().map(CaseFileParticipant).collect())
    }
}

/// A participation link: the account plus when it joined. The case
/// file is the parent object — no back-pointer (G.2 rule 1).
pub struct CaseFileParticipant(pub platform_core::Member);

#[Object]
impl CaseFileParticipant {
    /// The participating account.
    async fn account(&self) -> AccountRef<'_> {
        AccountRef(&self.0.account)
    }

    /// When the participation was created.
    async fn joined_at(&self) -> jiff::Timestamp {
        self.0.joined_at
    }
}

/// A case file as lists name it: scalars plus its ancestor Ref
/// (G.2 rule 1). The state rides as the bare enum — the facts stay
/// on the full object's union.
pub struct CaseFileRef {
    id: uuid::Uuid,
    state: platform_core::CaseFileStateValue,
    created_at: jiff::Timestamp,
    procedure: ProcedureRef,
}

impl CaseFileRef {
    pub fn new(case_file: &platform_core::CaseFile, procedure: ProcedureRef) -> Self {
        Self {
            id: case_file.id,
            state: case_file.state,
            created_at: case_file.created_at,
            procedure,
        }
    }
}

#[Object]
impl CaseFileRef {
    async fn id(&self) -> ID {
        ID::from(self.id)
    }

    /// The bare lifecycle state.
    async fn state(&self) -> CaseFileStateValue {
        self.state.into()
    }

    async fn created_at(&self) -> jiff::Timestamp {
        self.created_at
    }

    async fn procedure(&self) -> &ProcedureRef {
        &self.procedure
    }
}

/// The lifecycle state (G.2 rule 5): a union of state-specific
/// objects, subject-prefixed. One member until the checkpoint
/// machine lands (P1).
#[derive(async_graphql::Union)]
pub enum CaseFileState {
    Draft(CaseFileDraftState),
}

/// Being filled, never submitted. Its only fact is the row's
/// creation time.
#[derive(async_graphql::SimpleObject)]
pub struct CaseFileDraftState {
    pub created_at: jiff::Timestamp,
}

/// The bare state, for list rows and filters (G.2 rule 5's parallel
/// enum, generated from the platform-core discriminant).
#[derive(async_graphql::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[graphql(remote = "platform_core::CaseFileStateValue")]
pub enum CaseFileStateValue {
    Draft,
}

/// The row's lifecycle columns as the union; a corrupt pair is an
/// internal error, never a guess.
pub(crate) fn case_file_state(
    case_file: &platform_core::CaseFile,
) -> async_graphql::Result<CaseFileState> {
    Ok(
        match platform_core::current_case_file_state(case_file).map_err(internal)? {
            platform_core::CaseFileState::Draft => CaseFileState::Draft(CaseFileDraftState {
                created_at: case_file.created_at,
            }),
        },
    )
}

/// One page of case files (G.13): newest first, forward-only.
#[derive(async_graphql::SimpleObject)]
pub struct CaseFileConnection {
    pub edges: Vec<CaseFileEdge>,
    pub page_info: PageInfo,
}

/// One row of a connection page.
#[derive(async_graphql::SimpleObject)]
pub struct CaseFileEdge {
    /// Resume after this row: pass as `after`.
    pub cursor: String,
    pub node: CaseFileRef,
}

/// Forward-only paging facts (G.13); the backward half joins with
/// the reviewer table if it needs it.
#[derive(async_graphql::SimpleObject)]
pub struct PageInfo {
    /// Whether another page follows `endCursor`.
    pub has_next_page: bool,
    /// The last row's cursor; `null` on an empty page.
    pub end_cursor: Option<String>,
}

/// Default page size when `first` is omitted.
pub const DEFAULT_PAGE_SIZE: usize = 25;
/// Hard cap on `first` (G.13).
pub const MAX_PAGE_SIZE: usize = 100;

/// Validates the shared connection arguments: `first` in 1..=100
/// (default 25), `after` a cursor a previous page handed out.
pub(crate) fn page_arguments(
    first: Option<i32>,
    after: Option<String>,
) -> async_graphql::Result<(usize, Option<uuid::Uuid>)> {
    let limit = match first {
        None => DEFAULT_PAGE_SIZE,
        Some(first) if (1..=MAX_PAGE_SIZE as i32).contains(&first) => first as usize,
        Some(_) => {
            return Err(invalid_input(format!(
                "'first' must be between 1 and {MAX_PAGE_SIZE}"
            )));
        }
    };
    let after = after
        .map(|cursor| {
            cursor
                .parse::<uuid::Uuid>()
                .map_err(|_| invalid_input("'after' is not a cursor this list handed out"))
        })
        .transpose()?;
    Ok((limit, after))
}

/// Builds the connection over one core page, resolving each row's
/// `ProcedureRef` through `resolve` (the listings differ in how they
/// know the procedure: the root list batch-fetches, a parent
/// procedure already holds its own Ref).
pub(crate) fn connection_of(
    page: platform_core::CaseFilePage,
    mut resolve: impl FnMut(&platform_core::CaseFile) -> async_graphql::Result<ProcedureRef>,
) -> async_graphql::Result<CaseFileConnection> {
    let mut edges = Vec::with_capacity(page.items.len());
    for case_file in &page.items {
        edges.push(CaseFileEdge {
            cursor: case_file.id.to_string(),
            node: CaseFileRef::new(case_file, resolve(case_file)?),
        });
    }
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());
    Ok(CaseFileConnection {
        edges,
        page_info: PageInfo {
            has_next_page: page.has_next,
            end_cursor,
        },
    })
}

/// Resolves the `ProcedureRef` of every row on a page in two
/// `IN`-list queries (procedures, then their organizations) — the
/// root listing's batch shape (G.2 rule 1: one query per list
/// field), never one query per row.
pub(crate) async fn procedure_refs_of(
    db: &mut toasty::Db,
    page: &platform_core::CaseFilePage,
) -> async_graphql::Result<HashMap<uuid::Uuid, ProcedureRef>> {
    let mut procedure_ids: Vec<uuid::Uuid> = page.items.iter().map(|c| c.procedure_id).collect();
    procedure_ids.sort_unstable();
    procedure_ids.dedup();
    let procedures = platform_core::procedures_by_ids(&mut *db, procedure_ids)
        .await
        .map_err(internal)?;
    let mut organization_ids: Vec<uuid::Uuid> =
        procedures.iter().map(|p| p.organization_id).collect();
    organization_ids.sort_unstable();
    organization_ids.dedup();
    let organizations: HashMap<uuid::Uuid, OrganizationRef> =
        platform_core::organizations_by_ids(&mut *db, organization_ids)
            .await
            .map_err(internal)?
            .iter()
            .map(|o| (o.id, OrganizationRef::from(o)))
            .collect();
    procedures
        .into_iter()
        .map(|procedure| {
            // Foreign keys make a miss a wiring bug — surfaced,
            // never silently dropped.
            let organization = organizations
                .get(&procedure.organization_id)
                .cloned()
                .ok_or_else(|| {
                    internal(format!(
                        "procedure {} listed without its organization {}",
                        procedure.id, procedure.organization_id
                    ))
                })?;
            Ok((procedure.id, ProcedureRef::of(&procedure, organization)))
        })
        .collect()
}
