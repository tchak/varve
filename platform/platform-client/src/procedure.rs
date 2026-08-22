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
}
