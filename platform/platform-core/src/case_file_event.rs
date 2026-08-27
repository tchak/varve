//! The case-file event log (P.4 *Event logs*): what the kernel
//! record log doesn't hold — today only `created`; team assignment,
//! messages and reviewer administrivia join it at P1. Its own table,
//! deliberately separate from [`crate::procedure_event`]: retention
//! and authority differ — these rows are personal-data-adjacent and
//! go with the case file under the DESIGN §2.10 erasure guarantees,
//! and once checkpoints exist the kernel record log, not this table,
//! is authoritative for lifecycle.
//!
//! Rows are append-only, written by the use-case services in
//! [`crate::case_file`] inside the same transaction as the row
//! change they record. **Payloads never hold cell values** (P.4) —
//! references and metadata only, or the log becomes an erasure leak;
//! no kind carries facts today, so the facts column waits for the
//! first kind that does (a nullable column is a trivial later
//! migration).

use toasty::Deferred;

use crate::case_file::CaseFile;

/// One entry in a case file's event log.
#[derive(Debug, toasty::Model)]
pub struct CaseFileEvent {
    /// UUID v7 (time-ordered), generated on insert — the log's
    /// sequence as well as its key.
    #[key]
    #[auto]
    pub id: uuid::Uuid,

    /// The case file this entry belongs to.
    #[index]
    pub case_file_id: uuid::Uuid,

    /// The case file (relation).
    #[belongs_to]
    pub case_file: Deferred<CaseFile>,

    /// The account that acted; `None` for system events.
    pub actor_account_id: Option<uuid::Uuid>,

    /// What happened.
    pub kind: CaseFileEventKind,

    /// Set on insert.
    #[auto]
    pub created_at: jiff::Timestamp,
}

/// The event alphabet (P.4, G.13): one word until the checkpoint
/// machine and the reviewer side land (P1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, toasty::Embed)]
pub enum CaseFileEventKind {
    /// The catalog row was created.
    Created,
}

/// Appends one entry. `pub(crate)`: only the use-case services
/// write events, inside the transaction that applies what the entry
/// records.
pub(crate) async fn append_case_file_event(
    exec: &mut dyn toasty::Executor,
    case_file_id: uuid::Uuid,
    actor_account_id: Option<uuid::Uuid>,
    kind: CaseFileEventKind,
) -> toasty::Result<CaseFileEvent> {
    CaseFileEvent::create()
        .case_file_id(case_file_id)
        .actor_account_id(actor_account_id)
        .kind(kind)
        .exec(exec)
        .await
}

/// A case file's events, oldest first (by id — UUID v7 is the
/// insertion order).
pub async fn list_case_file_events(
    db: &mut toasty::Db,
    case_file_id: uuid::Uuid,
) -> toasty::Result<Vec<CaseFileEvent>> {
    CaseFileEvent::filter_by_case_file_id(case_file_id)
        .order_by(CaseFileEvent::fields().id().asc())
        .exec(db)
        .await
}
