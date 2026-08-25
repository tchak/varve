//! `ProcedureEvent`: the audit trail (platform P.4 *Event logs*,
//! G.9), an **interface** since G.11 — the schema's first — so the
//! `published` event can carry its facts (`publication`, `base`) and
//! answer `report`, the diff that publication made, recomputed at
//! read time from the content-addressed schemas. The actor resolves
//! to an [`AccountRef`] — `null` for a system event or an account
//! since deleted; the log entry itself outlives both.

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::{Context, ID, Interface, Object};

use crate::error::internal;
use crate::member::AccountRef;

/// The trail's shared row: every interface member wraps one. The
/// actor is shared across entries (`Arc`): one administrator usually
/// accounts for the whole trail.
struct EventRow {
    event: platform_core::ProcedureEvent,
    actor: Option<Arc<platform_core::Account>>,
}

/// One entry in a procedure's audit trail (G.11.2): the G.9 row as
/// the shared shape, one member per kind — facts live where they are
/// meaningful, so only the published event carries any.
#[derive(Interface)]
#[graphql(
    field(
        name = "id",
        ty = "ID",
        desc = "Time-ordered (UUID v7): the log's sequence as well as its id."
    ),
    field(name = "kind", ty = "ProcedureEventKind", desc = "What happened."),
    field(
        name = "actor",
        ty = "Option<AccountRef<'_>>",
        desc = "Who acted; `null` for a system event — or an account since deleted."
    ),
    field(
        name = "created_at",
        ty = "jiff::Timestamp",
        desc = "When it happened."
    )
)]
pub enum ProcedureEvent {
    Created(ProcedureCreatedEvent),
    Published(ProcedurePublishedEvent),
    Closed(ProcedureClosedEvent),
    Reopened(ProcedureReopenedEvent),
}

/// The interface members whose kind carries no facts: the shared row
/// and nothing else.
macro_rules! bare_event {
    ($(#[doc = $doc:literal] $name:ident)*) => {$(
        #[doc = $doc]
        pub struct $name(EventRow);

        #[Object]
        impl $name {
            /// Time-ordered (UUID v7): the log's sequence as well as
            /// its id.
            async fn id(&self) -> ID {
                ID::from(self.0.event.id)
            }

            /// What happened.
            async fn kind(&self) -> ProcedureEventKind {
                self.0.event.kind.into()
            }

            /// Who acted; `null` for a system event — or an account
            /// since deleted.
            async fn actor(&self) -> Option<AccountRef<'_>> {
                self.0.actor.as_deref().map(AccountRef)
            }

            /// When it happened.
            async fn created_at(&self) -> jiff::Timestamp {
                self.0.event.created_at
            }
        }
    )*};
}

bare_event! {
    #[doc = "The procedure was created."]
    ProcedureCreatedEvent
    #[doc = "The procedure was closed to new submissions."]
    ProcedureClosedEvent
    #[doc = "The procedure was reopened on its last published revision."]
    ProcedureReopenedEvent
}

/// A revision was published. The only member with facts (G.10.4):
/// which publication (§2.13 decision 9: its id commits to the
/// revision *and* the surface set), forked from which base.
pub struct ProcedurePublishedEvent {
    row: EventRow,
    facts: platform_core::PublishedFacts,
}

#[Object]
impl ProcedurePublishedEvent {
    /// Time-ordered (UUID v7): the log's sequence as well as its id.
    async fn id(&self) -> ID {
        ID::from(self.row.event.id)
    }

    /// What happened.
    async fn kind(&self) -> ProcedureEventKind {
        self.row.event.kind.into()
    }

    /// Who acted; `null` for a system event — or an account since
    /// deleted.
    async fn actor(&self) -> Option<AccountRef<'_>> {
        self.row.actor.as_deref().map(AccountRef)
    }

    /// When it happened.
    async fn created_at(&self) -> jiff::Timestamp {
        self.row.event.created_at
    }

    /// The publication's content address (§2.13 decision 9).
    async fn publication(&self) -> ID {
        ID::from(self.facts.publication.as_str())
    }

    /// The fork point this publication was authored against; `null`
    /// on a first publication.
    async fn base(&self) -> Option<ID> {
        self.facts.base.as_deref().map(ID::from)
    }

    /// The diff this publication made (G.11.4): the exact
    /// classification `publishRevision` gated on, recomputed at read
    /// time from the two content-addressed schemas. A `null` base
    /// classifies against the empty schema — a first publication
    /// reads as the initial column list, no special case.
    async fn report(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<crate::impact::ImpactReport> {
        use varve_store::RevisionStore;
        let (_, mut db) = crate::session(ctx)?;
        let shared: platform_core::SharedExecutor =
            tokio::sync::Mutex::new(&mut db as &mut dyn toasty::Executor);
        let store = platform_store::PlatformStore::new(&shared);
        // Publication ids resolve through the lineage's event log
        // (§2.13 decision 9) — each publication arrives with its
        // schema.
        let lineage = varve_store::LineageId::new(self.row.event.procedure_id.to_string());
        let publications = store.publications(&lineage).await.map_err(internal)?;
        let schema_of = |id: &str| {
            publications
                .iter()
                .find(|(publication, _)| publication.id().as_str() == id)
                .map(|(_, schema)| schema.clone())
        };
        let base = match &self.facts.base {
            Some(id) => Some(
                schema_of(id).ok_or_else(|| internal("publication's base is not in the store"))?,
            ),
            None => None,
        };
        let next = schema_of(&self.facts.publication)
            .ok_or_else(|| internal("published event's publication is not in the store"))?;
        let report = platform_core::draft_report(base.as_ref(), &next).map_err(internal)?;
        let labels = platform_core::ColumnLabels::resolve(base.as_ref(), &next);
        Ok(crate::impact::ImpactReport::labeled(&report, &labels))
    }
}

/// The event alphabet (platform P.4), generated from the
/// platform-core enum.
#[derive(async_graphql::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[graphql(remote = "platform_core::ProcedureEventKind")]
pub enum ProcedureEventKind {
    Created,
    Published,
    Closed,
    Reopened,
}

/// One row as its interface member. A `published` row without
/// decodable facts is surfaced, never repaired (the platform-core
/// contract on stored facts).
fn event_member(
    event: platform_core::ProcedureEvent,
    actor: Option<Arc<platform_core::Account>>,
) -> async_graphql::Result<ProcedureEvent> {
    let kind = event.kind;
    let row = EventRow { event, actor };
    Ok(match kind {
        platform_core::ProcedureEventKind::Created => {
            ProcedureEvent::Created(ProcedureCreatedEvent(row))
        }
        platform_core::ProcedureEventKind::Published => {
            let facts = row
                .event
                .facts
                .as_ref()
                .ok_or_else(|| internal("published event without facts"))?
                .decode()
                .map_err(internal)?;
            ProcedureEvent::Published(ProcedurePublishedEvent { row, facts })
        }
        platform_core::ProcedureEventKind::Closed => {
            ProcedureEvent::Closed(ProcedureClosedEvent(row))
        }
        platform_core::ProcedureEventKind::Reopened => {
            ProcedureEvent::Reopened(ProcedureReopenedEvent(row))
        }
    })
}

/// A procedure's events, oldest first, actors resolved. One account
/// fetch per **distinct** actor — the list is bounded by design
/// (lifecycle transitions only), so no dataloader yet (P.6 owns that
/// story).
pub(crate) async fn procedure_events(
    db: &mut toasty::Db,
    procedure_id: uuid::Uuid,
) -> async_graphql::Result<Vec<ProcedureEvent>> {
    let events = platform_core::list_procedure_events(db, procedure_id)
        .await
        .map_err(internal)?;
    let mut actors: HashMap<uuid::Uuid, Option<Arc<platform_core::Account>>> = HashMap::new();
    for event in &events {
        if let Some(id) = event.actor_account_id
            && let std::collections::hash_map::Entry::Vacant(slot) = actors.entry(id)
        {
            let account = platform_core::find_account(db, id)
                .await
                .map_err(internal)?;
            slot.insert(account.map(Arc::new));
        }
    }
    events
        .into_iter()
        .map(|event| {
            let actor = event
                .actor_account_id
                .and_then(|id| actors.get(&id).cloned().flatten());
            event_member(event, actor)
        })
        .collect()
}

/// One trail entry by id (G.11.6): the diff page's point lookup. The
/// trail is bounded by design, so this filters the list; `None` for
/// an id that is not this procedure's.
pub(crate) async fn procedure_event(
    db: &mut toasty::Db,
    procedure_id: uuid::Uuid,
    id: uuid::Uuid,
) -> async_graphql::Result<Option<ProcedureEvent>> {
    let Some(event) = platform_core::list_procedure_events(db, procedure_id)
        .await
        .map_err(internal)?
        .into_iter()
        .find(|event| event.id == id)
    else {
        return Ok(None);
    };
    let actor = match event.actor_account_id {
        Some(id) => platform_core::find_account(db, id)
            .await
            .map_err(internal)?
            .map(Arc::new),
        None => None,
    };
    event_member(event, actor).map(Some)
}
