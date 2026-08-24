//! Procedures of an organization: the procedures page's read and
//! `createProcedure`.

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

/// One audit-trail entry.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ProcedureEvent")]
pub struct ProcedureEvent {
    pub kind: ProcedureEventKind,
    pub actor: Option<Actor>,
    pub created_at: jiff::Timestamp,
}

/// The event alphabet; `PUBLISHED` has no writer until the kernel
/// edge lands.
#[derive(cynic::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[cynic(graphql_type = "ProcedureEventKind")]
pub enum ProcedureEventKind {
    Created,
    Published,
    Closed,
    Reopened,
    DraftDiscarded,
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

/// `mutation($input: ReopenProcedureInput!) { reopenProcedure(input: $input) { … } }`.
/// `INVALID_TRANSITION` unless the procedure is closed; `FORBIDDEN`
/// when the viewer does not administer it.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "ReopenProcedureVariables")]
pub struct ReopenProcedure {
    #[arguments(input: $input)]
    pub reopen_procedure: ProcedureLifecycle,
}
