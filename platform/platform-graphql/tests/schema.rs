//! Schema-level tests of the P0 slice (`design/graphql.md` G.6),
//! executed directly through `execute` with hand-built principals —
//! no transport. DB-backed, gated on `VARVE_TEST_DATABASE_URL` like
//! `platform-core/tests/db.rs`; unset, every test passes vacuously.

use cynic::QueryBuilder;
use platform_client::organization::{OrganizationQuery, OrganizationVariables};
use platform_client::viewer::ViewerQuery;
use platform_core::{Principal, connect, register};
use platform_graphql::{InProcess, PlatformSchema, execute, schema};
use serde_json::{Value, json};

async fn test_db() -> Option<toasty::Db> {
    let url = match std::env::var("VARVE_TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("skipped: VARVE_TEST_DATABASE_URL not set");
            return None;
        }
    };
    Some(connect(&url).await.expect("connect to test database"))
}

/// A fresh account as a principal.
async fn account(db: &mut toasty::Db, tag: &str) -> Principal {
    let email = format!("{tag}+{}@example.test", uuid::Uuid::new_v4());
    let account = register(db, &email, "s3cret-enough", tag, None)
        .await
        .expect("register");
    Principal::from_account(&account)
}

fn unique_slug(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4())
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().expect("id").to_owned()
}

struct Api {
    schema: PlatformSchema,
    db: toasty::Db,
}

impl Api {
    /// Executes `query` with `variables` as `who`, returning the
    /// whole JSON response (`data` and `errors`).
    async fn run(&self, who: &Principal, query: &str, variables: Value) -> Value {
        let request = async_graphql::Request::new(query)
            .variables(async_graphql::Variables::from_json(variables));
        let response = execute(&self.schema, request, who.clone(), self.db.clone()).await;
        serde_json::to_value(response).unwrap()
    }

    /// Like [`Self::run`], asserting no errors and returning `data`.
    async fn data(&self, who: &Principal, query: &str, variables: Value) -> Value {
        let mut response = self.run(who, query, variables).await;
        assert!(response.get("errors").is_none(), "{response}");
        response["data"].take()
    }

    /// Like [`Self::run`], asserting exactly one error and returning
    /// its `extensions.code`.
    async fn error_code(&self, who: &Principal, query: &str, variables: Value) -> String {
        let response = self.run(who, query, variables).await;
        let errors = response["errors"].as_array().expect("errors");
        assert_eq!(errors.len(), 1, "{response}");
        errors[0]["extensions"]["code"]
            .as_str()
            .expect("code")
            .to_owned()
    }

    async fn create_organization(&self, who: &Principal, slug: &str, name: &str) -> Value {
        self.data(
            who,
            "mutation($input: CreateOrganizationInput!) {
                createOrganization(input: $input) {
                    id slug name
                    members { account { id email } joinedAt }
                    counts { procedures teams members }
                }
            }",
            json!({ "input": { "slug": slug, "name": name } }),
        )
        .await["createOrganization"]
            .take()
    }

    async fn create_team(&self, who: &Principal, organization_id: &str, name: &str) -> Value {
        self.data(
            who,
            "mutation($input: CreateTeamInput!) {
                createTeam(input: $input) { id name organization { id slug } members { account { id } } }
            }",
            json!({ "input": { "organizationId": organization_id, "name": name } }),
        )
        .await["createTeam"]
            .take()
    }

    async fn create_procedure(&self, who: &Principal, organization_id: &str, title: &str) -> Value {
        self.data(
            who,
            "mutation($input: CreateProcedureInput!) {
                createProcedure(input: $input) { id title description organization { id } }
            }",
            json!({ "input": { "organizationId": organization_id, "title": title } }),
        )
        .await["createProcedure"]
            .take()
    }
}

impl Api {
    /// The typed client, executing in-process as `who`.
    fn client(&self, who: &Principal) -> InProcess {
        InProcess::new(self.schema.clone(), self.db.clone(), who.clone())
    }
}

async fn api() -> Option<Api> {
    let db = test_db().await?;
    Some(Api {
        schema: schema(),
        db,
    })
}

const ORGANIZATION: &str = "query($id: ID!) {
    organization(id: $id) {
        id slug name createdAt updatedAt
        teams { id name organization { id } }
        procedures { id title organization { id } }
        members { account { id name email } joinedAt }
        counts { procedures teams members }
    }
}";

const TEAM: &str = "query($id: ID!) {
    team(id: $id) {
        id name createdAt
        organization { id slug name }
        members { account { id } joinedAt }
    }
}";

const PROCEDURE: &str = "query($id: ID!) {
    procedure(id: $id) { id title description createdAt organization { id slug } }
}";

const NIL: &str = "00000000-0000-0000-0000-000000000000";

#[tokio::test]
async fn create_then_read_the_whole_graph() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let slug = unique_slug("ville");

    // createOrganization: the creator is the first member; the slug
    // is normalized; counts start at 0/0/1.
    let org = api
        .create_organization(&alice, &format!("  {}  ", slug.to_uppercase()), " Ville ")
        .await;
    assert_eq!(org["slug"], slug);
    assert_eq!(org["name"], "Ville");
    assert_eq!(org["members"].as_array().unwrap().len(), 1);
    assert_eq!(
        org["members"][0]["account"]["id"],
        alice.account_id.to_string()
    );
    assert_eq!(
        org["counts"],
        json!({ "procedures": 0, "teams": 0, "members": 1 })
    );
    let org_id = id_of(&org);

    let team = api.create_team(&alice, &org_id, " Guichet ").await;
    assert_eq!(team["name"], "Guichet");
    assert_eq!(team["organization"]["id"], org_id);
    assert_eq!(team["members"], json!([]));
    let team_id = id_of(&team);

    let procedure = api
        .create_procedure(&alice, &org_id, "Demande de place")
        .await;
    assert_eq!(procedure["title"], "Demande de place");
    assert_eq!(procedure["description"], "");
    assert_eq!(procedure["organization"]["id"], org_id);
    let procedure_id = id_of(&procedure);

    // The full organization, with Refs for its children.
    let data = api
        .data(&alice, ORGANIZATION, json!({ "id": org_id }))
        .await;
    let org = &data["organization"];
    assert_eq!(org["slug"], slug);
    assert!(org["createdAt"].as_str().unwrap().ends_with('Z'));
    assert_eq!(
        org["teams"],
        json!([{ "id": team_id, "name": "Guichet", "organization": { "id": org_id } }])
    );
    assert_eq!(
        org["procedures"],
        json!([{ "id": procedure_id, "title": "Demande de place", "organization": { "id": org_id } }])
    );
    assert_eq!(org["members"][0]["account"]["email"], alice.email);
    assert_eq!(
        org["counts"],
        json!({ "procedures": 1, "teams": 1, "members": 1 })
    );

    // Root lookups of the children.
    let data = api.data(&alice, TEAM, json!({ "id": team_id })).await;
    assert_eq!(data["team"]["organization"]["slug"], slug);
    let data = api
        .data(&alice, PROCEDURE, json!({ "id": procedure_id }))
        .await;
    assert_eq!(data["procedure"]["organization"]["id"], org_id);

    // Root lists are the viewer's.
    let data = api
        .data(
            &alice,
            "{ organizations { id slug } procedures { id organization { slug } } }",
            json!({}),
        )
        .await;
    assert!(
        data["organizations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["id"] == org_id)
    );
    let mine: Vec<&Value> = data["procedures"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["id"] == procedure_id)
        .collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["organization"]["slug"], slug);
}

#[tokio::test]
async fn non_members_see_null_and_cannot_create() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let mallory = account(&mut db, "mallory").await;

    let org_id = id_of(
        &api.create_organization(&alice, &unique_slug("org"), "Org")
            .await,
    );
    let team_id = id_of(&api.create_team(&alice, &org_id, "Team").await);
    let procedure_id = id_of(&api.create_procedure(&alice, &org_id, "Proc").await);

    // Every root lookup is null for a non-member — and for an id
    // that does not exist, indistinguishably.
    for (query, id) in [
        (ORGANIZATION, org_id.as_str()),
        (TEAM, team_id.as_str()),
        (PROCEDURE, procedure_id.as_str()),
        (ORGANIZATION, NIL),
        (TEAM, NIL),
        (PROCEDURE, NIL),
    ] {
        let data = api.data(&mallory, query, json!({ "id": id })).await;
        let field = data.as_object().unwrap().values().next().unwrap();
        assert_eq!(field, &Value::Null, "{query} {id}");
    }
    let data = api
        .data(
            &mallory,
            "{ organizations { id } procedures { id } }",
            json!({}),
        )
        .await;
    assert!(
        !data["organizations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["id"] == org_id)
    );
    assert!(
        !data["procedures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == procedure_id)
    );

    // Mutations on a foreign organization, or a missing one, are the
    // same FORBIDDEN.
    for organization_id in [org_id.as_str(), NIL] {
        let code = api
            .error_code(
                &mallory,
                "mutation($input: CreateTeamInput!) { createTeam(input: $input) { id } }",
                json!({ "input": { "organizationId": organization_id, "name": "T" } }),
            )
            .await;
        assert_eq!(code, "FORBIDDEN");
        let code = api
            .error_code(
                &mallory,
                "mutation($input: CreateProcedureInput!) { createProcedure(input: $input) { id } }",
                json!({ "input": { "organizationId": organization_id, "title": "P" } }),
            )
            .await;
        assert_eq!(code, "FORBIDDEN");
    }
}

#[tokio::test]
async fn a_reviewer_sees_the_team_and_its_organization_ref_only() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let bob = account(&mut db, "bob").await;

    let org_id = id_of(
        &api.create_organization(&alice, &unique_slug("org"), "Org")
            .await,
    );
    let team_id = id_of(&api.create_team(&alice, &org_id, "Team").await);
    platform_core::add_team_member(&mut db, team_id.parse().unwrap(), bob.account_id)
        .await
        .expect("add");

    let data = api.data(&bob, TEAM, json!({ "id": team_id })).await;
    assert_eq!(data["team"]["organization"]["id"], org_id);
    assert_eq!(
        data["team"]["members"][0]["account"]["id"],
        bob.account_id.to_string()
    );
    // …but not the full organization, nor its procedures.
    let data = api.data(&bob, ORGANIZATION, json!({ "id": org_id })).await;
    assert_eq!(data["organization"], Value::Null);
    let data = api.data(&bob, "{ organizations { id } }", json!({})).await;
    assert_eq!(data["organizations"], json!([]));
}

#[tokio::test]
async fn input_errors_are_structured() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let slug = unique_slug("dup");
    api.create_organization(&alice, &slug, "First").await;

    const CREATE: &str =
        "mutation($input: CreateOrganizationInput!) { createOrganization(input: $input) { id } }";
    let code = api
        .error_code(
            &alice,
            CREATE,
            json!({ "input": { "slug": slug.to_uppercase(), "name": "Again" } }),
        )
        .await;
    assert_eq!(code, "SLUG_TAKEN");
    for (slug, name) in [("", "x"), ("Not Valid!", "x"), ("fine-slug", "   ")] {
        let code = api
            .error_code(
                &alice,
                CREATE,
                json!({ "input": { "slug": slug, "name": name } }),
            )
            .await;
        assert_eq!(code, "INVALID_INPUT", "{slug:?} {name:?}");
    }

    let code = api
        .error_code(&alice, ORGANIZATION, json!({ "id": "not-a-uuid" }))
        .await;
    assert_eq!(code, "INVALID_INPUT");
}

#[test]
fn sdl_has_the_slice_and_refs_carry_no_child_lists() {
    let sdl = schema().sdl();
    for needle in [
        "organization(id: ID!): Organization",
        "organizations: [OrganizationRef!]!",
        "team(id: ID!): Team",
        "procedure(id: ID!): Procedure",
        "procedures: [ProcedureRef!]!",
        "createOrganization(input: CreateOrganizationInput!): Organization!",
        "createTeam(input: CreateTeamInput!): Team!",
        "createProcedure(input: CreateProcedureInput!): Procedure!",
        "scalar DateTime",
        "scalar Slug",
        "slug: Slug!",
        "members: [Member!]!",
    ] {
        assert!(sdl.contains(needle), "missing {needle:?} in\n{sdl}");
    }
    // G.2 rule 1: a Ref holds scalars and ancestor Refs only.
    for ref_type in ["OrganizationRef", "TeamRef", "ProcedureRef", "AccountRef"] {
        let start = sdl.find(&format!("type {ref_type} {{")).expect(ref_type);
        let body = &sdl[start..start + sdl[start..].find('}').unwrap()];
        assert!(!body.contains('['), "{ref_type} has a list field:\n{body}");
    }
}

/// The SDL `platform-client` builds against is this schema's, byte for
/// byte (G.3, platform P.9 Q2). `VARVE_UPDATE_SDL=1` rewrites the
/// artifact; cynic then recompiles every operation against it.
#[test]
fn sdl_artifact_is_current() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../platform-client/schema.graphql"
    );
    let sdl = schema().sdl();
    if std::env::var_os("VARVE_UPDATE_SDL").is_some() {
        std::fs::write(path, &sdl).expect("write schema.graphql");
    }
    let artifact = std::fs::read_to_string(path).expect("platform-client/schema.graphql");
    assert!(
        artifact == sdl,
        "platform-client/schema.graphql is stale; run \
         `VARVE_UPDATE_SDL=1 cargo test -p platform-graphql sdl_artifact_is_current`"
    );
}

#[tokio::test]
async fn the_typed_client_reads_what_the_schema_wrote() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let slug = unique_slug("typed");
    let created = api.create_organization(&alice, &slug, "Typed").await;
    let client = api.client(&alice);

    let viewer = platform_client::run(&client, ViewerQuery::build(()))
        .await
        .expect("viewer")
        .viewer;
    assert_eq!(viewer.account_id.inner(), alice.account_id.to_string());
    assert_eq!(viewer.email, alice.email);

    let organization = platform_client::run(
        &client,
        OrganizationQuery::build(OrganizationVariables {
            id: cynic::Id::new(id_of(&created)),
        }),
    )
    .await
    .expect("organization")
    .organization
    .expect("visible to its member");
    assert_eq!(organization.slug.0, slug);
    assert_eq!(organization.name, "Typed");
    assert_eq!(organization.counts.members, 1);
    assert_eq!(organization.members[0].account.email, alice.email);
    assert!(organization.teams.is_empty());

    // Absent-or-invisible is `None`, not an error (G.6).
    let bob = account(&mut db, "bob").await;
    let hidden = platform_client::run(
        &api.client(&bob),
        OrganizationQuery::build(OrganizationVariables {
            id: cynic::Id::new(id_of(&created)),
        }),
    )
    .await
    .expect("a query, not an error");
    assert!(hidden.organization.is_none());

    // A structured error arrives typed.
    let error = platform_client::run(
        &client,
        OrganizationQuery::build(OrganizationVariables {
            id: cynic::Id::new("not-a-uuid"),
        }),
    )
    .await
    .expect_err("malformed id");
    assert_eq!(error.code(), Some(platform_client::Code::InvalidInput));
}

#[test]
fn the_client_mirrors_every_error_code() {
    let server: Vec<&str> = platform_graphql::error::Code::ALL
        .iter()
        .map(|c| c.as_str())
        .collect();
    let client: Vec<&str> = platform_client::Code::ALL
        .iter()
        .map(|c| c.as_str())
        .collect();
    assert_eq!(server, client);
}
