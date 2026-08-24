//! The procedure event log (P.4 *Event logs*): the audit trail of
//! one procedure's lifecycle. For close/reopen this log is the
//! primary record (the kernel has no open/closed concept); for
//! publications it will mirror the kernel's own events
//! (`RevisionStore`). Its own table, deliberately separate from the
//! future case-file log: retention and authority differ — case-file
//! events are erased with the case file (DESIGN §2.10), this log is
//! the long-lived administrative audit.
//!
//! Rows are append-only, written by the use-case services in
//! [`crate::procedure`] inside the same transaction as the row
//! change they record, so column and log cannot disagree. Draft
//! autosaves are deliberately not events (P.4); the `published`
//! kind's facts (revision, base) join as a column when publication
//! lands with the kernel edge.

use toasty::Deferred;

use crate::procedure::Procedure;

/// One entry in a procedure's event log.
#[derive(Debug, toasty::Model)]
pub struct ProcedureEvent {
    /// UUID v7 (time-ordered), generated on insert — the log's
    /// sequence as well as its key.
    #[key]
    #[auto]
    pub id: uuid::Uuid,

    /// The procedure this entry belongs to.
    #[index]
    pub procedure_id: uuid::Uuid,

    /// The procedure (relation).
    #[belongs_to]
    pub procedure: Deferred<Procedure>,

    /// The account that acted; `None` for system events.
    pub actor_account_id: Option<uuid::Uuid>,

    /// What happened.
    pub kind: ProcedureEventKind,

    /// Set on insert.
    #[auto]
    pub created_at: jiff::Timestamp,
}

/// The event alphabet (P.4). No kind carries facts today; the
/// `published` revision and base arrive with the kernel edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, toasty::Embed)]
pub enum ProcedureEventKind {
    /// The catalog row was created.
    Created,
    /// A revision was published (mirrors the kernel publication
    /// event; from `Closed` this is also the reopen). No writer
    /// until the kernel edge lands.
    Published,
    /// The procedure closed to new submissions.
    Closed,
    /// The procedure reopened on its last published revision.
    Reopened,
    /// The revision draft in progress was discarded.
    DraftDiscarded,
}

/// Appends one entry. `pub(crate)`: only the use-case services
/// write events, inside the transaction that applies what the entry
/// records.
pub(crate) async fn append_procedure_event(
    exec: &mut dyn toasty::Executor,
    procedure_id: uuid::Uuid,
    actor_account_id: Option<uuid::Uuid>,
    kind: ProcedureEventKind,
) -> toasty::Result<ProcedureEvent> {
    ProcedureEvent::create()
        .procedure_id(procedure_id)
        .actor_account_id(actor_account_id)
        .kind(kind)
        .exec(exec)
        .await
}

/// A procedure's events, oldest first (by id — UUID v7 is the
/// insertion order).
pub async fn list_procedure_events(
    db: &mut toasty::Db,
    procedure_id: uuid::Uuid,
) -> toasty::Result<Vec<ProcedureEvent>> {
    ProcedureEvent::filter_by_procedure_id(procedure_id)
        .order_by(ProcedureEvent::fields().id().asc())
        .exec(db)
        .await
}
