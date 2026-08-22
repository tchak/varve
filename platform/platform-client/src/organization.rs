//! `organization(id)`: the full organization and the Refs it names
//! (G.2: full objects only at root, `*Ref` everywhere else).

// The derives resolve their markers through `schema` in scope.
use crate::{Slug, schema};

/// Variables of [`OrganizationQuery`].
#[derive(cynic::QueryVariables, Debug)]
pub struct OrganizationVariables {
    pub id: cynic::Id,
}

/// `query($id: ID!) { organization(id: $id) { … } }`; `None` for an
/// absent or invisible organization (G.6).
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "OrganizationVariables")]
pub struct OrganizationQuery {
    #[arguments(id: $id)]
    pub organization: Option<Organization>,
}

/// The full organization.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct Organization {
    pub id: cynic::Id,
    pub slug: Slug,
    pub name: String,
    pub created_at: jiff::Timestamp,
    pub updated_at: jiff::Timestamp,
    pub teams: Vec<TeamRef>,
    pub procedures: Vec<ProcedureRef>,
    pub members: Vec<Member>,
    pub counts: OrganizationCounts,
}

/// `organization.counts`.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct OrganizationCounts {
    pub procedures: i32,
    pub teams: i32,
    pub members: i32,
}

/// An organization as lists and parents name it.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct OrganizationRef {
    pub id: cynic::Id,
    pub slug: Slug,
    pub name: String,
}

/// A team as lists name it.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct TeamRef {
    pub id: cynic::Id,
    pub name: String,
    pub organization: OrganizationRef,
}

/// A procedure as lists name it.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct ProcedureRef {
    pub id: cynic::Id,
    pub title: String,
    pub organization: OrganizationRef,
}

/// The account ⟷ container link.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub account: AccountRef,
    pub joined_at: jiff::Timestamp,
}

/// An account as members name it.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct AccountRef {
    pub id: cynic::Id,
    pub name: String,
    pub email: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cynic::{MutationBuilder, QueryBuilder};

    #[test]
    fn builds_the_root_lookup_with_its_variable() {
        let operation = OrganizationQuery::build(OrganizationVariables {
            id: cynic::Id::new("abc"),
        });
        let document = serde_json::to_value(&operation).unwrap();
        let query = document["query"].as_str().unwrap();
        assert!(query.contains("organization(id: $id)"), "{query}");
        assert!(query.contains("counts"), "{query}");
        assert_eq!(document["variables"]["id"], "abc");
    }

    #[test]
    fn builds_the_creation_with_its_input() {
        let operation = CreateOrganization::build(CreateOrganizationVariables {
            input: CreateOrganizationInput {
                slug: Slug("acme".into()),
                name: "Acme".into(),
            },
        });
        let document = serde_json::to_value(&operation).unwrap();
        let query = document["query"].as_str().unwrap();
        assert!(query.starts_with("mutation"), "{query}");
        assert!(
            query.contains("createOrganization(input: $input)"),
            "{query}"
        );
        assert_eq!(document["variables"]["input"]["slug"], "acme");
        assert_eq!(document["variables"]["input"]["name"], "Acme");
    }
}

/// `query { organizations { … } }`: the viewer's organizations,
/// oldest first.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct OrganizationsQuery {
    pub organizations: Vec<OrganizationRef>,
}

/// `createOrganization` input: the slug is normalized and validated
/// by the server's scalar.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct CreateOrganizationInput {
    pub slug: Slug,
    pub name: String,
}

/// Variables of [`CreateOrganization`].
#[derive(cynic::QueryVariables, Debug)]
pub struct CreateOrganizationVariables {
    pub input: CreateOrganizationInput,
}

/// `mutation($input: CreateOrganizationInput!) { createOrganization(input: $input) { … } }`;
/// the viewer becomes the first member. Fails with `SLUG_TAKEN` or
/// `INVALID_INPUT` (G.2.7).
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CreateOrganizationVariables")]
pub struct CreateOrganization {
    #[arguments(input: $input)]
    pub create_organization: CreatedOrganization,
}

/// What a creation answers with: enough to navigate to the new
/// organization.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "Organization")]
pub struct CreatedOrganization {
    pub id: cynic::Id,
    pub slug: Slug,
    pub name: String,
}
