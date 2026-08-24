//! `ProcedureEvent`: one audit-trail row (platform P.4 *Event
//! logs*, G.9). The actor resolves to an [`AccountRef`] — `null`
//! for a system event or an account since deleted; the log entry
//! itself outlives both.

use async_graphql::{ID, Object};

use crate::error::internal;
use crate::member::AccountRef;

/// One entry in a procedure's audit trail.
pub struct ProcedureEvent {
    event: platform_core::ProcedureEvent,
    actor: Option<platform_core::Account>,
}

#[Object]
impl ProcedureEvent {
    /// Time-ordered (UUID v7): the log's sequence as well as its id.
    async fn id(&self) -> ID {
        ID::from(self.event.id)
    }

    /// What happened.
    async fn kind(&self) -> ProcedureEventKind {
        self.event.kind.into()
    }

    /// Who acted; `null` for a system event — or an account since
    /// deleted.
    async fn actor(&self) -> Option<AccountRef<'_>> {
        self.actor.as_ref().map(AccountRef)
    }

    /// When it happened.
    async fn created_at(&self) -> jiff::Timestamp {
        self.event.created_at
    }
}

/// The event alphabet (platform P.4), generated from the
/// platform-core enum. `PUBLISHED` has no writer until the kernel
/// edge lands.
#[derive(async_graphql::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[graphql(remote = "platform_core::ProcedureEventKind")]
pub enum ProcedureEventKind {
    Created,
    Published,
    Closed,
    Reopened,
}

/// A procedure's events, oldest first, actors resolved. One account
/// fetch per event — the list is bounded by design (transitions and
/// draft discards), so no dataloader yet (P.6 owns that story).
pub(crate) async fn procedure_events(
    db: &mut toasty::Db,
    procedure_id: uuid::Uuid,
) -> async_graphql::Result<Vec<ProcedureEvent>> {
    let events = platform_core::list_procedure_events(db, procedure_id)
        .await
        .map_err(internal)?;
    let mut resolved = Vec::with_capacity(events.len());
    for event in events {
        let actor = match event.actor_account_id {
            Some(id) => platform_core::find_account(db, id)
                .await
                .map_err(internal)?,
            None => None,
        };
        resolved.push(ProcedureEvent { event, actor });
    }
    Ok(resolved)
}
