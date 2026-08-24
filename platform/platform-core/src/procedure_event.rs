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
//! autosaves are deliberately not events (P.4); per-kind facts ride
//! as platform-owned JSON bytes — today only `published` carries any
//! ([`PublishedFacts`]).

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

    /// Per-kind facts as platform-owned JSON bytes; `None` when the
    /// kind carries none. Today only `published` does
    /// ([`PublishedFacts`]).
    pub facts: Option<FactsBytes>,

    /// Set on insert.
    #[auto]
    pub created_at: jiff::Timestamp,
}

/// The `published` event's facts (P.4 *Event logs*): which revision,
/// forked from which base — the platform mirror of the kernel
/// publication event, timestamped by the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedFacts {
    /// The published revision's content address.
    pub revision: String,
    /// The draft's fork point; `None` on a first publication.
    pub base: Option<String>,
}

/// Facts as stored JSON bytes (one `BYTEA` column — the
/// [`crate::procedure::TreeBytes`] pattern). Constructed from typed
/// facts only; a stored value that no longer decodes is surfaced,
/// never repaired.
#[derive(Debug, Clone, PartialEq, Eq, toasty::Embed)]
pub struct FactsBytes(Vec<u8>);

impl FactsBytes {
    fn encode(facts: &PublishedFacts) -> Self {
        Self(
            serde_json::to_vec(&serde_json::json!({
                "revision": facts.revision,
                "base": facts.base,
            }))
            .expect("json! values serialize"),
        )
    }

    /// Decodes stored facts; `Err` carries the reason.
    pub fn decode(&self) -> Result<PublishedFacts, String> {
        let value: serde_json::Value =
            serde_json::from_slice(&self.0).map_err(|e| e.to_string())?;
        let revision = value["revision"]
            .as_str()
            .ok_or("'revision' must be a string")?
            .to_owned();
        let base = match &value["base"] {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.clone()),
            _ => return Err("'base' must be a string or null".into()),
        };
        Ok(PublishedFacts { revision, base })
    }
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
    facts: Option<&PublishedFacts>,
) -> toasty::Result<ProcedureEvent> {
    ProcedureEvent::create()
        .procedure_id(procedure_id)
        .actor_account_id(actor_account_id)
        .kind(kind)
        .facts(facts.map(FactsBytes::encode))
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
