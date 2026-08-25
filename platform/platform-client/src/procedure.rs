//! Procedures of an organization: the procedures page's read and
//! `createProcedure`.

use crate::revision_draft::DraftOrganization;
use crate::{Slug, schema};

/// Variables of [`OrganizationProceduresQuery`].
#[derive(cynic::QueryVariables, Debug)]
pub struct OrganizationProceduresVariables {
    pub id: cynic::Id,
}

/// `query($id: ID!) { organization(id: $id) { id slug name procedures { … } } }`;
/// `None` for an absent or invisible organization (G.6).
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "OrganizationProceduresVariables")]
pub struct OrganizationProceduresQuery {
    #[arguments(id: $id)]
    pub organization: Option<OrganizationProcedures>,
}

/// An organization reduced to its identity and its procedures.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "Organization")]
pub struct OrganizationProcedures {
    pub id: cynic::Id,
    pub slug: Slug,
    pub name: String,
    pub procedures: Vec<Procedure>,
}

/// A procedure as the procedures page lists it.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ProcedureRef")]
pub struct Procedure {
    pub id: cynic::Id,
    pub title: String,
}

/// `createProcedure` input; `description` defaults to empty on the
/// server.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct CreateProcedureInput {
    pub organization_id: cynic::Id,
    pub title: String,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Variables of [`CreateProcedure`].
#[derive(cynic::QueryVariables, Debug)]
pub struct CreateProcedureVariables {
    pub input: CreateProcedureInput,
}

/// `mutation($input: CreateProcedureInput!) { createProcedure(input: $input) { … } }`.
/// `FORBIDDEN` when the viewer is not a member of the organization
/// (absent or not, G.6); `INVALID_INPUT` for a blank title.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CreateProcedureVariables")]
pub struct CreateProcedure {
    #[arguments(input: $input)]
    pub create_procedure: CreatedProcedure,
}

/// What a creation answers with.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "Procedure")]
pub struct CreatedProcedure {
    pub id: cynic::Id,
    pub title: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cynic::{MutationBuilder, QueryBuilder};

    #[test]
    fn builds_the_procedures_read_and_the_creation() {
        let read = OrganizationProceduresQuery::build(OrganizationProceduresVariables {
            id: cynic::Id::new("abc"),
        });
        let document = serde_json::to_value(&read).unwrap();
        let query = document["query"].as_str().unwrap();
        assert!(query.contains("organization(id: $id)"), "{query}");
        assert!(query.contains("procedures"), "{query}");
        assert!(!query.contains("members"), "{query}");

        let create = CreateProcedure::build(CreateProcedureVariables {
            input: CreateProcedureInput {
                organization_id: cynic::Id::new("abc"),
                title: "Permit".into(),
                description: None,
            },
        });
        let document = serde_json::to_value(&create).unwrap();
        assert!(
            document["query"]
                .as_str()
                .unwrap()
                .contains("createProcedure(input: $input)")
        );
        assert_eq!(document["variables"]["input"]["title"], "Permit");
        // Absent, so the server's default applies.
        assert!(document["variables"]["input"].get("description").is_none());
    }

    #[test]
    fn builds_the_lifecycle_read_and_the_transitions() {
        let read = ProcedureLifecycleQuery::build(ProcedureLifecycleVariables {
            id: cynic::Id::new("abc"),
        });
        let query = serde_json::to_value(&read).unwrap()["query"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(query.contains("procedure(id: $id)"), "{query}");
        // The union queries every member inline.
        for member in [
            "ProcedureDraftState",
            "ProcedurePublishedState",
            "ProcedureClosedState",
        ] {
            assert!(query.contains(&format!("... on {member}")), "{query}");
        }
        assert!(query.contains("events"), "{query}");

        let close = CloseProcedure::build(CloseProcedureVariables {
            input: CloseProcedureInput {
                procedure_id: cynic::Id::new("abc"),
            },
        });
        let document = serde_json::to_value(&close).unwrap();
        assert!(
            document["query"]
                .as_str()
                .unwrap()
                .contains("closeProcedure(input: $input)")
        );
        assert_eq!(document["variables"]["input"]["procedureId"], "abc");

        let reopen = ReopenProcedure::build(ReopenProcedureVariables {
            input: ReopenProcedureInput {
                procedure_id: cynic::Id::new("abc"),
            },
        });
        assert!(
            serde_json::to_value(&reopen).unwrap()["query"]
                .as_str()
                .unwrap()
                .contains("reopenProcedure(input: $input)")
        );

        let publish = PublishRevision::build(PublishRevisionVariables {
            input: PublishRevisionInput {
                procedure_id: cynic::Id::new("abc"),
                confirm: false,
            },
        });
        let document = serde_json::to_value(&publish).unwrap();
        let query = document["query"].as_str().unwrap();
        assert!(query.contains("publishRevision(input: $input)"), "{query}");
        assert!(query.contains("report"), "{query}");
        assert_eq!(document["variables"]["input"]["confirm"], false);
    }
}

/// Variables of [`ProcedureLifecycleQuery`].
#[derive(cynic::QueryVariables, Debug)]
pub struct ProcedureLifecycleVariables {
    pub id: cynic::Id,
}

/// `query($id: ID!) { procedure(id: $id) { id state { … } events { … } } }`;
/// `None` for an absent or invisible procedure (G.6).
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "ProcedureLifecycleVariables")]
pub struct ProcedureLifecycleQuery {
    #[arguments(id: $id)]
    pub procedure: Option<ProcedureLifecycle>,
}

/// A procedure reduced to its lifecycle: the state union and the
/// audit trail (G.9).
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "Procedure")]
pub struct ProcedureLifecycle {
    pub id: cynic::Id,
    pub state: ProcedureState,
    pub events: Vec<ProcedureEvent>,
}

/// The state union (G.2 rule 5): one variant per state-specific
/// object, facts where they are meaningful.
#[derive(cynic::InlineFragments, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ProcedureState")]
pub enum ProcedureState {
    Draft(ProcedureDraftState),
    Published(ProcedurePublishedState),
    Closed(ProcedureClosedState),
    /// A state this client predates.
    #[cynic(fallback)]
    Unknown,
}

/// Never published.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ProcedureDraftState")]
pub struct ProcedureDraftState {
    pub created_at: jiff::Timestamp,
}

/// Open for submissions; `since` is reset by reopening — not a
/// publication date.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ProcedurePublishedState")]
pub struct ProcedurePublishedState {
    pub since: jiff::Timestamp,
}

/// Closed to new submissions.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ProcedureClosedState")]
pub struct ProcedureClosedState {
    pub since: jiff::Timestamp,
}

/// One audit-trail entry (G.11): the published member with its
/// facts, every other kind through the shared interface row.
#[derive(cynic::InlineFragments, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ProcedureEvent")]
pub enum ProcedureEvent {
    Published(PublishedEvent),
    #[cynic(fallback)]
    Other(EventRow),
}

impl ProcedureEvent {
    /// Time-ordered (UUID v7): the log's sequence as well as its id.
    pub fn id(&self) -> &cynic::Id {
        match self {
            Self::Published(e) => &e.id,
            Self::Other(e) => &e.id,
        }
    }

    /// What happened.
    pub fn kind(&self) -> ProcedureEventKind {
        match self {
            Self::Published(_) => ProcedureEventKind::Published,
            Self::Other(e) => e.kind,
        }
    }

    /// Who acted; `None` for a system event or an account since
    /// deleted.
    pub fn actor(&self) -> Option<&Actor> {
        match self {
            Self::Published(e) => e.actor.as_ref(),
            Self::Other(e) => e.actor.as_ref(),
        }
    }

    /// When it happened.
    pub fn created_at(&self) -> jiff::Timestamp {
        match self {
            Self::Published(e) => e.created_at,
            Self::Other(e) => e.created_at,
        }
    }
}

/// The trail's shared shape (the G.11 interface fields).
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ProcedureEvent")]
pub struct EventRow {
    pub id: cynic::Id,
    pub kind: ProcedureEventKind,
    pub actor: Option<Actor>,
    pub created_at: jiff::Timestamp,
}

/// A publication, with its facts (G.11.2): which publication (§2.13
/// decision 9 — its content address commits to the revision *and*
/// the surface set), forked from which base (`None` on a first
/// publication).
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ProcedurePublishedEvent")]
pub struct PublishedEvent {
    pub id: cynic::Id,
    pub actor: Option<Actor>,
    pub created_at: jiff::Timestamp,
    pub publication: cynic::Id,
    pub base: Option<cynic::Id>,
}

/// The event alphabet.
#[derive(cynic::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[cynic(graphql_type = "ProcedureEventKind")]
pub enum ProcedureEventKind {
    Created,
    Published,
    Closed,
    Reopened,
}

/// The acting account, as the log names it.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "AccountRef")]
pub struct Actor {
    pub id: cynic::Id,
    pub name: String,
}

/// `closeProcedure` input.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct CloseProcedureInput {
    pub procedure_id: cynic::Id,
}

/// Variables of [`CloseProcedure`].
#[derive(cynic::QueryVariables, Debug)]
pub struct CloseProcedureVariables {
    pub input: CloseProcedureInput,
}

/// `mutation($input: CloseProcedureInput!) { closeProcedure(input: $input) { … } }`.
/// `INVALID_TRANSITION` unless the procedure is published;
/// `FORBIDDEN` when the viewer does not administer it.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CloseProcedureVariables")]
pub struct CloseProcedure {
    #[arguments(input: $input)]
    pub close_procedure: ProcedureLifecycle,
}

/// `reopenProcedure` input.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct ReopenProcedureInput {
    pub procedure_id: cynic::Id,
}

/// Variables of [`ReopenProcedure`].
#[derive(cynic::QueryVariables, Debug)]
pub struct ReopenProcedureVariables {
    pub input: ReopenProcedureInput,
}

/// `publishRevision` input; `confirm` accepts a report worse than
/// `SAFE` (G.10).
#[derive(cynic::InputObject, Debug, Clone)]
pub struct PublishRevisionInput {
    pub procedure_id: cynic::Id,
    pub confirm: bool,
}

/// Variables of [`PublishRevision`].
#[derive(cynic::QueryVariables, Debug)]
pub struct PublishRevisionVariables {
    pub input: PublishRevisionInput,
}

/// `mutation($input: PublishRevisionInput!) { publishRevision(input: $input) { … } }`.
/// `INVALID_DRAFT` when nothing is in progress or a choice has no
/// options; `CONFLICT` when the draft's base is no longer the head.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "PublishRevisionVariables")]
pub struct PublishRevision {
    #[arguments(input: $input)]
    pub publish_revision: PublishRevisionResult,
}

/// The report always, the procedure as it now stands, and whether
/// anything was written.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "PublishRevisionResult")]
pub struct PublishRevisionResult {
    pub report: ImpactReport,
    pub published: bool,
    pub procedure: ProcedureLifecycle,
}

/// What a publication does (or would do) to records.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ImpactReport")]
pub struct ImpactReport {
    pub worst: ChangeClass,
    pub columns: Vec<ColumnImpactEntry>,
    pub relabeled_groups: Vec<GroupRelabelEntry>,
}

/// A renamed group (§3.1): safe, reported.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "GroupRelabelEntry")]
pub struct GroupRelabelEntry {
    pub from: String,
    pub to: String,
}

/// One changed column of the report, named by its label (G.11.5 —
/// the base schema names a removal).
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ColumnImpactEntry")]
pub struct ColumnImpactEntry {
    pub column_id: cynic::Id,
    pub label: String,
    pub class: ChangeClass,
    pub change: ColumnChangeKind,
    pub removed_options: Vec<cynic::Id>,
    pub renamed_from: Option<String>,
}

/// §3's vocabulary.
#[derive(cynic::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[cynic(graphql_type = "ChangeClass")]
pub enum ChangeClass {
    Safe,
    Lossy,
    Checked,
    Breaking,
}

/// The change's shape.
#[derive(cynic::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[cynic(graphql_type = "ColumnChangeKind")]
pub enum ColumnChangeKind {
    Added,
    Removed,
    Cast,
    ScopeMoved,
    Forbidden,
    Relabeled,
}

/// `mutation($input: ReopenProcedureInput!) { reopenProcedure(input: $input) { … } }`.
/// `INVALID_TRANSITION` unless the procedure is closed; `FORBIDDEN`
/// when the viewer does not administer it.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "ReopenProcedureVariables")]
pub struct ReopenProcedure {
    #[arguments(input: $input)]
    pub reopen_procedure: ProcedureLifecycle,
}

/// Variables of [`ProcedureEventDiffQuery`].
#[derive(cynic::QueryVariables, Debug)]
pub struct ProcedureEventDiffVariables {
    pub id: cynic::Id,
    pub event: cynic::Id,
}

/// `query($id: ID!, $event: ID!) { procedure(id: $id) { … event(id: $event) { … } } }`
/// — the diff page's read (G.11.6): one trail entry, the report
/// selected on the published member only. `None` procedure for an
/// absent or invisible one; `None` event for an id that is not this
/// procedure's.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "ProcedureEventDiffVariables")]
pub struct ProcedureEventDiffQuery {
    #[arguments(id: $id)]
    pub procedure: Option<ProcedureEventDiff>,
}

/// The procedure naming the page, and the one entry.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Procedure", variables = "ProcedureEventDiffVariables")]
pub struct ProcedureEventDiff {
    pub id: cynic::Id,
    pub title: String,
    pub organization: DraftOrganization,
    #[arguments(id: $event)]
    pub event: Option<DiffEvent>,
}

/// The entry the diff page shows: only a publication carries a
/// report; any other kind falls back to the shared row.
#[derive(cynic::InlineFragments, Debug)]
#[cynic(graphql_type = "ProcedureEvent")]
pub enum DiffEvent {
    Published(PublishedEventDiff),
    #[cynic(fallback)]
    Other(EventRow),
}

/// A publication with the diff it made (G.11.4): the report,
/// recomputed at read time from the content-addressed schemas.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "ProcedurePublishedEvent")]
pub struct PublishedEventDiff {
    pub id: cynic::Id,
    pub actor: Option<Actor>,
    pub created_at: jiff::Timestamp,
    pub base: Option<cynic::Id>,
    pub report: ImpactReport,
}
