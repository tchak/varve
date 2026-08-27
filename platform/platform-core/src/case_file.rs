//! The case-file catalog row and its participants (P.4 *Case-file
//! catalog and participants*, graphql.md G.13): the platform half of
//! a case file — the row an applicant creates on a published
//! procedure, and who is on it. The kernel record log (cells,
//! checkpoints, `submitCaseFile`) joins the row with `varve-store`'s
//! record-log persistence; until checkpoints exist the state column
//! is the only authority, and only `Draft` exists.
//!
//! [`CaseFileParticipant`] is a plain account⟷case-file join row
//! **with no role column** — the [`crate::organization`] membership
//! shape: participation *is* the right (seeing and later filling the
//! applicant surface). The creator is the first participant, written
//! in one transaction with the row and the `created` event
//! ([`crate::case_file_event`]); creator provenance lives there, not
//! in a column. Roles are the anticipated extension if DN's invités
//! carry fewer rights (P.9 Q16).

use toasty::Deferred;

use crate::account::Account;
use crate::case_file_event::{CaseFileEvent, CaseFileEventKind, append_case_file_event};
use crate::organization::Member;
use crate::procedure::{Procedure, current_state};
use crate::procedure_state::{CorruptState, ProcedureState};

/// A case file's catalog entry.
#[derive(Debug, toasty::Model)]
pub struct CaseFile {
    /// UUID v7 (time-ordered), generated on insert — also the
    /// listings' creation order and their cursor (G.13).
    #[key]
    #[auto]
    pub id: uuid::Uuid,

    /// The procedure this case file was created on.
    #[index]
    pub procedure_id: uuid::Uuid,

    /// The procedure (relation).
    #[belongs_to]
    pub procedure: Deferred<Procedure>,

    /// The lifecycle discriminant. Only [`CaseFileStateValue::Draft`]
    /// exists until the checkpoint machine lands (P1); the kernel
    /// record log then becomes authoritative and this column stays a
    /// read model maintained only by the use-case services (P.9 Q3).
    #[index]
    #[default(CaseFileStateValue::Draft)]
    pub state: CaseFileStateValue,

    /// When the current state was entered; `None` iff `state` is
    /// `Draft` (`created_at` already carries that fact) — the
    /// [`crate::procedure_state`] column pairing.
    pub state_since: Option<jiff::Timestamp>,

    /// Set on insert.
    #[auto]
    pub created_at: jiff::Timestamp,

    /// Set on insert and on every update.
    #[auto]
    pub updated_at: jiff::Timestamp,

    /// The join rows; mutate these to add or remove participants.
    #[has_many]
    pub participations: Deferred<Vec<CaseFileParticipant>>,

    /// Accounts participating — read-only derived relation.
    #[has_many(via = participations.account)]
    pub participants: Deferred<Vec<Account>>,

    /// The platform event log ([`crate::case_file_event`]), oldest
    /// first by id.
    #[has_many]
    pub events: Deferred<Vec<CaseFileEvent>>,

    /// Optimistic concurrency (toasty-managed), for the write paths
    /// that arrive with cells and checkpoints.
    #[version]
    pub version: u64,
}

/// A case file's lifecycle state with its fact. One state today; the
/// checkpoint machine (P.4: `DRAFT` → `SUBMITTED` → …) grows here in
/// the [`crate::procedure_state`] shape when the kernel record log
/// lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseFileState {
    /// Being filled, never submitted. No fact of its own —
    /// `created_at` already lives on the row.
    Draft,
}

/// The bare discriminant, as stored on the catalog row and mirrored
/// by the API's filtering enum (G.2 rule 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, toasty::Embed)]
pub enum CaseFileStateValue {
    Draft,
}

impl CaseFileState {
    /// Rebuilds the state from its row columns. The pairing
    /// (`since` is `None` iff `Draft`) is a storage invariant every
    /// write in this crate keeps; a mismatch is [`CorruptCaseFileState`]
    /// — corruption to surface, never to repair silently.
    pub fn from_columns(
        value: CaseFileStateValue,
        since: Option<jiff::Timestamp>,
    ) -> Result<Self, CorruptCaseFileState> {
        match (value, since) {
            (CaseFileStateValue::Draft, None) => Ok(Self::Draft),
            (value, since) => Err(CorruptCaseFileState { value, since }),
        }
    }

    /// The row columns: the discriminant plus `state_since` (`None`
    /// iff `Draft`).
    pub fn columns(&self) -> (CaseFileStateValue, Option<jiff::Timestamp>) {
        match *self {
            Self::Draft => (CaseFileStateValue::Draft, None),
        }
    }
}

/// A stored state whose columns disagree — see
/// [`CaseFileState::from_columns`].
#[derive(Debug, thiserror::Error)]
#[error("corrupt case-file state: {value:?} with state_since {since:?}")]
pub struct CorruptCaseFileState {
    /// The stored discriminant.
    pub value: CaseFileStateValue,
    /// The stored fact.
    pub since: Option<jiff::Timestamp>,
}

/// The current state of a loaded row — the read every service uses,
/// so the column pairing is checked in exactly one place.
pub fn current_case_file_state(
    case_file: &CaseFile,
) -> Result<CaseFileState, CorruptCaseFileState> {
    CaseFileState::from_columns(case_file.state, case_file.state_since)
}

/// One account's participation in one case file. The composite key
/// makes the link unique; there is deliberately nothing else on the
/// row (no role — P.9 Q16 holds that question).
#[derive(Debug, toasty::Model)]
#[key(case_file_id, account_id)]
pub struct CaseFileParticipant {
    /// The case file. The leading key column needs no separate
    /// index: the composite primary key already serves that prefix.
    pub case_file_id: uuid::Uuid,

    /// The case file (relation).
    #[belongs_to]
    pub case_file: Deferred<CaseFile>,

    /// The participating account.
    #[index]
    pub account_id: uuid::Uuid,

    /// The participating account (relation).
    #[belongs_to]
    pub account: Deferred<Account>,

    /// Set on insert.
    #[auto]
    pub created_at: jiff::Timestamp,
}

/// Failure modes of [`create_case_file`].
#[derive(Debug, thiserror::Error)]
pub enum CreateCaseFileError {
    /// No procedure with this id.
    #[error("no such procedure")]
    ProcedureNotFound,
    /// The procedure has never been published — there is no revision
    /// a case file could read (G.13: `FORBIDDEN` at the API, the
    /// same answer as not existing).
    #[error("the procedure has never been published")]
    NeverPublished,
    /// The procedure is closed to new submissions since `since`
    /// (G.13: `INVALID_TRANSITION` at the API).
    #[error("the procedure is closed to new submissions")]
    Closed {
        /// Since when.
        since: jiff::Timestamp,
    },
    /// The procedure's stored state columns disagree.
    #[error(transparent)]
    Corrupt(#[from] CorruptState),
    /// The underlying store failed.
    #[error("database error: {0}")]
    Db(#[from] toasty::Error),
}

/// Creates a case file on a published procedure **with `creator_id`
/// as its first participant** — without that second write the case
/// file is reachable by nobody (participation is the only right).
/// One transaction: the row, the participant row, and the `created`
/// event ([`crate::case_file_event`]) exist together or not at all.
/// Any account may create on a `Published` procedure — applicants
/// need no prior relation to it, and nothing bounds how many case
/// files one account opens on one procedure (G.13).
pub async fn create_case_file(
    db: &mut toasty::Db,
    procedure_id: uuid::Uuid,
    creator_id: uuid::Uuid,
) -> Result<CaseFile, CreateCaseFileError> {
    let mut tx = db.transaction().await?;
    let Some(procedure) = Procedure::filter_by_id(procedure_id)
        .first()
        .exec(&mut tx)
        .await?
    else {
        return Err(CreateCaseFileError::ProcedureNotFound);
    };
    match current_state(&procedure)? {
        ProcedureState::Published { .. } => {}
        ProcedureState::Draft => return Err(CreateCaseFileError::NeverPublished),
        ProcedureState::Closed { since } => return Err(CreateCaseFileError::Closed { since }),
    }
    let case_file = CaseFile::create()
        .procedure_id(procedure_id)
        .exec(&mut tx)
        .await?;
    add_case_file_participant(&mut tx, case_file.id, creator_id).await?;
    append_case_file_event(
        &mut tx,
        case_file.id,
        Some(creator_id),
        CaseFileEventKind::Created,
    )
    .await?;
    tx.commit().await?;
    Ok(case_file)
}

/// Looks a case file up by id.
pub async fn find_case_file(
    db: &mut toasty::Db,
    id: uuid::Uuid,
) -> toasty::Result<Option<CaseFile>> {
    CaseFile::filter_by_id(id).first().exec(db).await
}

/// Adds an account to a case file. Idempotent: an existing
/// participation is left as is (insert-or-ignore on the composite
/// key). Generic over the executor so [`create_case_file`] runs it
/// inside its transaction.
pub async fn add_case_file_participant(
    db: &mut impl toasty::Executor,
    case_file_id: uuid::Uuid,
    account_id: uuid::Uuid,
) -> toasty::Result<()> {
    CaseFileParticipant::upsert_by_case_file_id_and_account_id(case_file_id, account_id)
        .or_ignore()
        .exec(db)
        .await?;
    Ok(())
}

/// Whether `account_id` participates in `case_file_id` — i.e. may
/// see it (and later fill its applicant surface).
pub async fn is_case_file_participant(
    db: &mut toasty::Db,
    case_file_id: uuid::Uuid,
    account_id: uuid::Uuid,
) -> toasty::Result<bool> {
    Ok(
        CaseFileParticipant::filter_by_case_file_id_and_account_id(case_file_id, account_id)
            .first()
            .exec(db)
            .await?
            .is_some(),
    )
}

/// The participants of a case file, oldest participation first,
/// each with the instant they joined. Two queries (the join rows,
/// then the accounts in one `IN` list), never one per participant.
pub async fn list_case_file_participants(
    db: &mut toasty::Db,
    case_file_id: uuid::Uuid,
) -> toasty::Result<Vec<Member>> {
    let participations = CaseFileParticipant::filter_by_case_file_id(case_file_id)
        .order_by(CaseFileParticipant::fields().created_at().asc())
        .exec(db)
        .await?;
    let joined: Vec<(uuid::Uuid, jiff::Timestamp)> = participations
        .into_iter()
        .map(|p| (p.account_id, p.created_at))
        .collect();
    crate::account::members_of(db, joined).await
}

/// One page of a case-file listing (G.13): newest first, forward
/// only. `has_next` comes from fetching one row beyond the page,
/// never from page size.
#[derive(Debug)]
pub struct CaseFilePage {
    /// The page's rows, newest first.
    pub items: Vec<CaseFile>,
    /// Whether another page follows the last row.
    pub has_next: bool,
}

/// The case files `account_id` participates in, newest first —
/// the viewer-scoped root listing (G.13: today, viewer-scoped *is*
/// participant-of). `after` is the previous page's last id.
pub async fn list_account_case_files(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
    after: Option<uuid::Uuid>,
    limit: usize,
) -> toasty::Result<CaseFilePage> {
    let mut filter = CaseFile::fields()
        .participations()
        .any(CaseFileParticipant::fields().account_id().eq(account_id));
    if let Some(after) = after {
        filter = filter.and(CaseFile::fields().id().lt(after));
    }
    page_of(db, filter, limit).await
}

/// All case files of a procedure, newest first — the administering
/// side's listing (G.13: gated by the caller through the procedure's
/// organization membership). `after` is the previous page's last id.
pub async fn list_procedure_case_files(
    db: &mut toasty::Db,
    procedure_id: uuid::Uuid,
    after: Option<uuid::Uuid>,
    limit: usize,
) -> toasty::Result<CaseFilePage> {
    let mut filter = CaseFile::fields().procedure_id().eq(procedure_id);
    if let Some(after) = after {
        filter = filter.and(CaseFile::fields().id().lt(after));
    }
    page_of(db, filter, limit).await
}

/// Runs a listing query as one page: newest first by id (UUID v7 is
/// creation order), one row past `limit` to learn `has_next`.
async fn page_of(
    db: &mut toasty::Db,
    filter: toasty::stmt::Expr<bool>,
    limit: usize,
) -> toasty::Result<CaseFilePage> {
    let mut items = CaseFile::filter(filter)
        .order_by(CaseFile::fields().id().desc())
        .limit(limit + 1)
        .exec(db)
        .await?;
    let has_next = items.len() > limit;
    items.truncate(limit);
    Ok(CaseFilePage { items, has_next })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_columns_round_trip() {
        let (value, since) = CaseFileState::Draft.columns();
        assert_eq!(
            CaseFileState::from_columns(value, since).unwrap(),
            CaseFileState::Draft
        );
    }

    #[test]
    fn corrupt_pairing_is_surfaced() {
        let err = CaseFileState::from_columns(
            CaseFileStateValue::Draft,
            Some(jiff::Timestamp::UNIX_EPOCH),
        )
        .unwrap_err();
        assert_eq!(err.value, CaseFileStateValue::Draft);
        assert!(err.since.is_some());
    }
}
