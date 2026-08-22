//! Teams of an organization: the teams page's read and `createTeam`.

use crate::{Slug, schema};

/// Variables of [`OrganizationTeamsQuery`].
#[derive(cynic::QueryVariables, Debug)]
pub struct OrganizationTeamsVariables {
    pub id: cynic::Id,
}

/// `query($id: ID!) { organization(id: $id) { id slug name teams { … } } }`:
/// what a teams page needs and nothing else; `None` for an absent or
/// invisible organization (G.6).
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "OrganizationTeamsVariables")]
pub struct OrganizationTeamsQuery {
    #[arguments(id: $id)]
    pub organization: Option<OrganizationTeams>,
}

/// An organization reduced to its identity and its teams.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "Organization")]
pub struct OrganizationTeams {
    pub id: cynic::Id,
    pub slug: Slug,
    pub name: String,
    pub teams: Vec<Team>,
}

/// A team as the teams page lists it.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "TeamRef")]
pub struct Team {
    pub id: cynic::Id,
    pub name: String,
}

/// `createTeam` input.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct CreateTeamInput {
    pub organization_id: cynic::Id,
    pub name: String,
}

/// Variables of [`CreateTeam`].
#[derive(cynic::QueryVariables, Debug)]
pub struct CreateTeamVariables {
    pub input: CreateTeamInput,
}

/// `mutation($input: CreateTeamInput!) { createTeam(input: $input) { … } }`.
/// Fails with `FORBIDDEN` when the viewer is not a member of the
/// organization (absent or not — the same answer, G.6) and
/// `INVALID_INPUT` for a blank name.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CreateTeamVariables")]
pub struct CreateTeam {
    #[arguments(input: $input)]
    pub create_team: CreatedTeam,
}

/// What a creation answers with.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "Team")]
pub struct CreatedTeam {
    pub id: cynic::Id,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cynic::{MutationBuilder, QueryBuilder};

    #[test]
    fn builds_the_teams_read_and_the_creation() {
        let read = OrganizationTeamsQuery::build(OrganizationTeamsVariables {
            id: cynic::Id::new("abc"),
        });
        let document = serde_json::to_value(&read).unwrap();
        let query = document["query"].as_str().unwrap();
        assert!(query.contains("organization(id: $id)"), "{query}");
        assert!(query.contains("teams"), "{query}");
        assert!(!query.contains("members"), "{query}");

        let create = CreateTeam::build(CreateTeamVariables {
            input: CreateTeamInput {
                organization_id: cynic::Id::new("abc"),
                name: "Reviewers".into(),
            },
        });
        let document = serde_json::to_value(&create).unwrap();
        assert!(
            document["query"]
                .as_str()
                .unwrap()
                .contains("createTeam(input: $input)")
        );
        assert_eq!(document["variables"]["input"]["organizationId"], "abc");
    }
}
