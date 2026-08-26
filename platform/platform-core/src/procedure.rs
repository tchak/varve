//! The procedure catalog row (P.4): title, description, owning
//! organization — and the **revision draft**, the authored tree being
//! edited before it is published (P.4, *The authored tree is the
//! draft's single source*) — and the **lifecycle** (P.4 *Procedure
//! lifecycle*): the [`crate::procedure_state`] machine persisted as
//! two row columns, its transitions applied here in one transaction
//! with their [`crate::procedure_event`] log entry. The revision
//! DAG, publication and rules still arrive with the kernel edge
//! (`varve-service`) — [`close_procedure`] and [`reopen_procedure`]
//! exist now, publication does not yet.
//!
//! The draft is a nullable embedded object on the row, deferred out
//! of the catalog `SELECT`: it stores the authored tree
//! ([`crate::tree::Tree`]) as platform-owned JSON — the tree carries
//! audiences and presentation nodes, which no kernel object holds, so
//! publication *derives* the kernel schema from it
//! ([`crate::tree::Tree::schema`]) rather than the store holding
//! kernel bytes. Edits go through [`edit_revision_draft`], which
//! composes the pure operations of [`crate::tree_edit`] with a load
//! and a version-checked store.

use toasty::Deferred;

use crate::organization::Organization;
use crate::procedure_event::{ProcedureEvent, ProcedureEventKind, append_procedure_event};
use crate::procedure_state::{CorruptState, ProcedureState, ProcedureStateValue, TransitionError};
use crate::tree::{Tree, TreeDecodeError};
use crate::tree_edit::EditError;

/// A procedure's catalog entry.
#[derive(Debug, toasty::Model)]
pub struct Procedure {
    /// UUID v7 (time-ordered), generated on insert.
    #[key]
    #[auto]
    pub id: uuid::Uuid,

    /// The owning organization; its members administer this
    /// procedure (P.4).
    #[index]
    pub organization_id: uuid::Uuid,

    /// The owning organization (relation).
    #[belongs_to]
    pub organization: Deferred<Organization>,

    /// Title shown to applicants and reviewers.
    pub title: String,

    /// Free-text description; empty when none.
    pub description: String,

    /// The lifecycle discriminant (P.4 *Procedure lifecycle*):
    /// `Draft` until the first publication. The machine lives in
    /// [`crate::procedure_state`]; transitions go through
    /// [`close_procedure`] / [`reopen_procedure`] (publication
    /// arrives with the kernel edge), never through a bare update.
    #[index]
    #[default(ProcedureStateValue::Draft)]
    pub state: ProcedureStateValue,

    /// When the current state was entered; `None` iff `state` is
    /// `Draft`. *Since when the procedure is open* (or closed) —
    /// not a publication date: reopening resets it with no
    /// publication happening.
    pub state_since: Option<jiff::Timestamp>,

    /// The lineage head — the head **publication's** content address
    /// (§2.13 decision 9), a read model maintained only by
    /// publication ([`crate::publish`], the P.9 Q3 pattern): a new
    /// draft forks from it, and publication refuses a draft whose
    /// base no longer equals it. `None` until the first publication.
    pub latest_publication: Option<String>,

    /// Set on insert.
    #[auto]
    pub created_at: jiff::Timestamp,

    /// Set on insert and on every update.
    #[auto]
    pub updated_at: jiff::Timestamp,

    /// The unpublished authored tree being edited; `None` when nothing
    /// is in progress. Deferred: loaded only by
    /// [`find_procedure_with_revision_draft`].
    pub revision_draft: Deferred<Option<RevisionDraft>>,

    /// The authored tree of [`Self::latest_publication`], kept because
    /// the next draft forks from its base's *tree* (P.4: audiences
    /// and presentation nodes exist nowhere kernel-side), set only by
    /// publication. Deferred with the draft.
    pub published_tree: Deferred<Option<TreeBytes>>,

    /// The fillable preview's value bag (P.4 *The fillable preview*,
    /// graphql.md G.12): scratch record values, `None` until filled.
    /// Beside the draft, never inside it — filling must not fork the
    /// virtual draft, and `working_tree`'s `in_progress` stays a
    /// statement about the tree. Cleared by
    /// [`discard_revision_draft`] and by publication. Deferred with
    /// the draft.
    pub preview: Deferred<Option<crate::preview::PreviewBytes>>,

    /// The audit trail ([`crate::procedure_event`]), oldest first by
    /// id.
    #[has_many]
    pub events: Deferred<Vec<ProcedureEvent>>,

    /// Optimistic concurrency (toasty-managed): two editors saving the
    /// draft from the same loaded row cannot silently overwrite each
    /// other — the second save fails with a condition error and must
    /// reload.
    #[version]
    pub version: u64,
}

/// The draft of a procedure's next **revision** — named for what it
/// publishes as, since "draft" alone will also be a case-file state.
/// Holds the authored tree; publication derives the schema and
/// compiles the surfaces from it (P.4).
#[derive(Debug, Clone, PartialEq, toasty::Embed)]
pub struct RevisionDraft {
    /// The authored tree under edit, as its stored JSON bytes.
    pub tree: TreeBytes,

    /// The publication this draft forks from (its content address,
    /// §2.13 decision 9) — the next publication's parent and the
    /// stale-fork anchor. `None` until the procedure has a first
    /// publication to fork from.
    pub base: Option<String>,
}

/// An authored [`Tree`] as its stored JSON bytes (one `BYTEA`
/// column). Constructed from a `Tree` only, so the column never holds
/// anything [`Tree::from_bytes`] would refuse — short of corruption,
/// which [`RevisionDraftError::Corrupt`] reports.
#[derive(Debug, Clone, PartialEq, Eq, toasty::Embed)]
pub struct TreeBytes(Vec<u8>);

impl TreeBytes {
    pub fn encode(tree: &Tree) -> Self {
        Self(tree.to_bytes())
    }

    pub fn decode(&self) -> Result<Tree, TreeDecodeError> {
        Tree::from_bytes(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RevisionDraftError {
    #[error(transparent)]
    Edit(#[from] EditError),
    /// The stored draft no longer decodes — never produced by this
    /// crate's writes; a database-level corruption to surface, not
    /// silently replace.
    #[error("stored draft is unreadable: {0}")]
    Corrupt(#[from] TreeDecodeError),
    /// Database failure, including the optimistic-concurrency conflict
    /// (`condition_failed`) when the row changed since it was loaded.
    #[error(transparent)]
    Db(#[from] toasty::Error),
}

/// Creates a procedure catalog row owned by `organization_id`, in
/// `Draft`, logging the `created` event in the same transaction.
/// Title and description are stored trimmed.
pub async fn create_procedure(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
    actor_account_id: uuid::Uuid,
    title: &str,
    description: &str,
) -> toasty::Result<Procedure> {
    let mut tx = db.transaction().await?;
    let procedure = Procedure::create()
        .organization_id(organization_id)
        .title(title.trim())
        .description(description.trim())
        .exec(&mut tx)
        .await?;
    append_procedure_event(
        &mut tx,
        procedure.id,
        Some(actor_account_id),
        ProcedureEventKind::Created,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(procedure)
}

/// Looks a procedure up by id.
pub async fn find_procedure(
    db: &mut toasty::Db,
    id: uuid::Uuid,
) -> toasty::Result<Option<Procedure>> {
    Procedure::filter_by_id(id).first().exec(db).await
}

/// Looks a procedure up by id with its draft loaded.
pub async fn find_procedure_with_revision_draft(
    db: &mut toasty::Db,
    id: uuid::Uuid,
) -> toasty::Result<Option<Procedure>> {
    Procedure::filter_by_id(id)
        .include(Procedure::fields().revision_draft())
        .include(Procedure::fields().published_tree())
        .include(Procedure::fields().preview())
        .first()
        .exec(db)
        .await
}

/// The tree the next edit operates on — *head until touched* (G.7
/// virtual draft): the stored working buffer when one is in
/// progress; otherwise the published head's authored tree with the
/// head as `base`; otherwise the empty tree. Nothing is stored for
/// the two virtual cases; [`edit_revision_draft`] materializes the
/// fork on the first edit from exactly this shape.
pub struct WorkingTree {
    pub tree: Tree,
    /// The publication a fork records as its parent (the stale-fork
    /// check's anchor): the stored draft's `base`, or the head.
    pub base: Option<String>,
    /// Whether a stored draft exists — the one fact the projection
    /// would otherwise erase (an empty impact report cannot stand in
    /// for it: edits that never touch the derived schema still store
    /// a draft).
    pub in_progress: bool,
}

/// The [`WorkingTree`] of a procedure loaded by
/// [`find_procedure_with_revision_draft`].
///
/// # Panics
///
/// If the draft was not loaded (the catalog lookups defer it).
pub fn working_tree(procedure: &Procedure) -> Result<WorkingTree, RevisionDraftError> {
    Ok(match procedure.revision_draft.get() {
        Some(draft) => WorkingTree {
            tree: draft.tree.decode()?,
            base: draft.base.clone(),
            in_progress: true,
        },
        None => WorkingTree {
            tree: match procedure.published_tree.get() {
                Some(bytes) => bytes.decode()?,
                None => Tree::default(),
            },
            base: procedure.latest_publication.clone(),
            in_progress: false,
        },
    })
}

/// Applies `edit` to the procedure's [`working_tree`] and stores the
/// result, returning the tree as stored — with no draft in progress,
/// this is the fork: the published head's tree (or the empty tree)
/// becomes the working buffer, `base` recording the head (P.4
/// *Publication*). The procedure must come from
/// [`find_procedure_with_revision_draft`]; on success it is updated in
/// place (draft, `version`, `updated_at`).
///
/// Atomic: `edit` errors (a rejected operation) store nothing, and a
/// concurrent change to the row since it was loaded fails the store
/// ([`RevisionDraftError::Db`], `condition_failed`) instead of overwriting.
pub async fn edit_revision_draft(
    db: &mut toasty::Db,
    procedure: &mut Procedure,
    edit: impl FnOnce(&mut Tree) -> Result<(), EditError>,
) -> Result<Tree, RevisionDraftError> {
    let WorkingTree { mut tree, base, .. } = working_tree(procedure)?;
    edit(&mut tree)?;
    procedure
        .update()
        .revision_draft(Some(RevisionDraft {
            tree: TreeBytes::encode(&tree),
            base,
        }))
        .exec(db)
        .await?;
    Ok(tree)
}

/// Drops the procedure's draft, if any — and the preview bag with it
/// (G.12: the preview is scoped to a draft cycle). Not an event (P.4:
/// the draft is a working buffer — discarding it is authoring
/// workflow, the same altitude as the autosaves the log deliberately
/// omits). Same loading and concurrency contract as
/// [`edit_revision_draft`].
pub async fn discard_revision_draft(
    db: &mut toasty::Db,
    procedure: &mut Procedure,
) -> toasty::Result<()> {
    procedure
        .update()
        .revision_draft(None)
        .preview(None)
        .exec(db)
        .await
}

/// Errors of the persisted lifecycle transitions.
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    /// The machine refuses the transition from the current state.
    #[error(transparent)]
    Transition(#[from] TransitionError),
    /// The stored `(state, state_since)` pair is corrupt — surfaced,
    /// never repaired silently.
    #[error(transparent)]
    Corrupt(#[from] CorruptState),
    /// Database failure, including the optimistic-concurrency
    /// conflict (`condition_failed`) when the row changed since it
    /// was loaded.
    #[error(transparent)]
    Db(#[from] toasty::Error),
}

/// The clock as a stored column will hold it: Postgres keeps
/// timestamps at microsecond precision, so a nanosecond `now` would
/// make the in-place-updated row disagree with its own fresh read.
/// Every timestamp minted for a row goes through this.
pub fn stored_now() -> jiff::Timestamp {
    jiff::Timestamp::now()
        .round(jiff::Unit::Microsecond)
        .expect("rounding a real clock reading to microseconds cannot overflow")
}

/// Closes a published procedure to new submissions
/// ([`ProcedureState::close`]), logging the `closed` event. On
/// success the row is updated in place.
pub async fn close_procedure(
    db: &mut toasty::Db,
    procedure: &mut Procedure,
    actor_account_id: uuid::Uuid,
) -> Result<(), LifecycleError> {
    let state = current_state(procedure)?.close(stored_now())?;
    apply_transition(
        db,
        procedure,
        actor_account_id,
        state,
        ProcedureEventKind::Closed,
    )
    .await
}

/// Reopens a closed procedure on its last published revision
/// ([`ProcedureState::reopen`] — no publication involved), logging
/// the `reopened` event. On success the row is updated in place.
pub async fn reopen_procedure(
    db: &mut toasty::Db,
    procedure: &mut Procedure,
    actor_account_id: uuid::Uuid,
) -> Result<(), LifecycleError> {
    let state = current_state(procedure)?.reopen(stored_now())?;
    apply_transition(
        db,
        procedure,
        actor_account_id,
        state,
        ProcedureEventKind::Reopened,
    )
    .await
}

/// The row's lifecycle state, decoded ([`ProcedureState::from_columns`]).
pub fn current_state(procedure: &Procedure) -> Result<ProcedureState, CorruptState> {
    ProcedureState::from_columns(procedure.state, procedure.state_since)
}

/// Persists an already-taken transition: the state columns and the
/// event land in one transaction, under the row's `#[version]`
/// guard, so column and log cannot disagree.
async fn apply_transition(
    db: &mut toasty::Db,
    procedure: &mut Procedure,
    actor_account_id: uuid::Uuid,
    state: ProcedureState,
    kind: ProcedureEventKind,
) -> Result<(), LifecycleError> {
    let (value, since) = state.columns();
    let mut tx = db.transaction().await?;
    procedure
        .update()
        .state(value)
        .state_since(since)
        .exec(&mut tx)
        .await?;
    append_procedure_event(&mut tx, procedure.id, Some(actor_account_id), kind, None).await?;
    tx.commit().await?;
    Ok(())
}

/// The procedures `account_id` administers — those owned by any
/// organization the account is a member of — oldest first.
pub async fn list_account_procedures(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
) -> toasty::Result<Vec<Procedure>> {
    Procedure::filter(
        Procedure::fields().organization().memberships().any(
            crate::organization::OrganizationMembership::fields()
                .account_id()
                .eq(account_id),
        ),
    )
    .order_by(Procedure::fields().created_at().asc())
    .exec(db)
    .await
}

/// The procedures an organization owns, oldest first.
pub async fn list_organization_procedures(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
) -> toasty::Result<Vec<Procedure>> {
    Procedure::filter_by_organization_id(organization_id)
        .order_by(Procedure::fields().created_at().asc())
        .exec(db)
        .await
}
