//! Schema-level tests of the P0 slice (`design/graphql.md` G.6),
//! executed directly through `execute` with hand-built principals —
//! no transport. DB-backed, gated on `VARVE_TEST_DATABASE_URL` like
//! `platform-core/tests/db.rs`; unset, every test passes vacuously.

use cynic::{MutationBuilder, QueryBuilder};
use platform_client::organization::{OrganizationQuery, OrganizationVariables};
use platform_client::procedure::{
    ChangeClass, CloseProcedure, CloseProcedureInput, CloseProcedureVariables, ProcedureEventKind,
    ProcedureLifecycleQuery, ProcedureLifecycleVariables, ProcedureState, PublishRevision,
    PublishRevisionInput, PublishRevisionVariables, ReopenProcedure, ReopenProcedureInput,
    ReopenProcedureVariables,
};
use platform_client::revision_draft::{
    AddColumn, AddColumnInput, AddColumnVariables, ColumnType, ColumnTypeInput, Element,
    ProcedureRevisionDraftQuery, ProcedureRevisionDraftVariables, Unit,
};
use platform_client::viewer::ViewerQuery;
use platform_core::{Principal, register};
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
    // `connect_with`: the kernel tables ride the same database
    // (P.3, *migrations and registration*) — `publishRevision`
    // executes against them.
    Some(
        platform_core::connect_with(
            &url,
            platform_store::models(),
            &[&platform_store::MIGRATIONS],
        )
        .await
        .expect("connect to test database"),
    )
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
        "union ProcedureState = ProcedureDraftState | ProcedurePublishedState | ProcedureClosedState",
        "state: ProcedureStateValue!",
        "events: [ProcedureEvent!]!",
        // G.11: the trail is an interface; the published member
        // carries the facts and the read-time diff.
        "interface ProcedureEvent",
        "event(id: ID!): ProcedureEvent",
        "type ProcedurePublishedEvent implements ProcedureEvent",
        "report: ImpactReport!",
        "label: String!",
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

/// The executor's complexity cap (G.1, platform P.9 Q5): depth is
/// bounded structurally, so the cap is aimed at *alias* amplification
/// — one document repeating a costly field hundreds of times. No
/// principal, no db: validation refuses the document before any
/// resolver could ask for either.
#[tokio::test]
async fn a_document_past_the_complexity_limit_is_refused_before_execution() {
    // Each alias selects two fields; one alias past the limit.
    let fields: String = (0..=platform_graphql::COMPLEXITY_LIMIT / 2)
        .map(|i| format!("v{i}: viewer {{ accountId }} "))
        .collect();
    let response = schema()
        .execute(async_graphql::Request::new(format!("{{ {fields} }}")))
        .await;
    assert_eq!(response.data, async_graphql::Value::Null);
    assert!(
        response
            .errors
            .iter()
            .any(|e| e.message.to_lowercase().contains("complex")),
        "{:?}",
        response.errors
    );
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

/// The draft selection every draft mutation and the read share.
const DRAFT: &str = "revisionDraft { base inProgress elements {
    __typename
    ... on Column { id parentId label required audience type { __typename
        ... on TextType { format { __typename ... on RegexFormat { pattern } } }
        ... on IntegerType { unit } ... on DecimalType { unit }
        ... on EnumType { multiple options { id label } }
        ... on AttachmentType { multiple accept maxBytes }
        ... on GeometryType { multiple } } }
    ... on Group { id parentId label cardinality audience }
    ... on Section { id parentId title help audience }
    ... on Note { id parentId title body audience }
} }";

fn draft_mutation(field: &str, input_type: &str) -> String {
    format!("mutation($input: {input_type}!) {{ {field}(input: $input) {{ id {DRAFT} }} }}")
}

/// `(typename, id, parentId, text)` of every element, document order
/// — the text is a column or group's label, a section's title, a
/// note's body.
fn outline(procedure: &Value) -> Vec<(String, String, Option<String>, String)> {
    procedure["revisionDraft"]["elements"]
        .as_array()
        .expect("elements")
        .iter()
        .map(|e| {
            (
                e["__typename"].as_str().unwrap().to_owned(),
                id_of(e),
                e["parentId"].as_str().map(str::to_owned),
                e["label"]
                    .as_str()
                    .or(e["title"].as_str())
                    .or(e["body"].as_str())
                    .unwrap()
                    .to_owned(),
            )
        })
        .collect()
}

fn labels(procedure: &Value) -> Vec<String> {
    outline(procedure).into_iter().map(|e| e.3).collect()
}

impl Api {
    async fn edit(&self, who: &Principal, field: &str, input_type: &str, input: Value) -> Value {
        let mut data = self
            .data(
                who,
                &draft_mutation(field, input_type),
                json!({ "input": input }),
            )
            .await;
        data[field].take()
    }

    async fn edit_error(
        &self,
        who: &Principal,
        field: &str,
        input_type: &str,
        input: Value,
    ) -> String {
        self.error_code(
            who,
            &draft_mutation(field, input_type),
            json!({ "input": input }),
        )
        .await
    }
}

#[tokio::test]
async fn revision_draft_editing_journey() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("draft"), "Org")
        .await;
    let procedure = api.create_procedure(&alice, &id_of(&org), "Bourse").await;
    let pid = id_of(&procedure);

    // Nothing in progress yet: the virtual draft (G.7) is the empty
    // tree, pristine.
    let read = format!("query($id: ID!) {{ procedure(id: $id) {{ id {DRAFT} }} }}");
    let data = api.data(&alice, &read, json!({ "id": pid })).await;
    let draft = &data["procedure"]["revisionDraft"];
    assert_eq!(draft["inProgress"], false, "{data}");
    assert!(draft["base"].is_null(), "{data}");
    assert_eq!(draft["elements"].as_array().unwrap().len(), 0, "{data}");

    // addColumn at the root starts the draft; base is null (no DAG yet).
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": " Nom ", "type": { "text": {} } }),
        )
        .await;
    assert!(p["revisionDraft"]["base"].is_null());
    assert_eq!(labels(&p), ["Nom"]);
    let nom = id_of(&p["revisionDraft"]["elements"][0]);
    assert_eq!(
        p["revisionDraft"]["elements"][0]["type"]["__typename"],
        "TextType"
    );

    // addGroup (MANY), then columns inside it: append, then before.
    let p = api
        .edit(
            &alice,
            "addGroup",
            "AddGroupInput",
            json!({ "procedureId": pid, "label": "Adresses", "cardinality": "MANY" }),
        )
        .await;
    let adresses = id_of(&p["revisionDraft"]["elements"][1]);
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Rue", "type": { "text": {} },
                    "placement": { "parentId": adresses } }),
        )
        .await;
    let rue = id_of(&p["revisionDraft"]["elements"][2]);
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Ville", "type": { "text": {} },
                    "placement": { "parentId": adresses, "beforeId": rue } }),
        )
        .await;
    assert_eq!(labels(&p), ["Nom", "Adresses", "Ville", "Rue"]);
    let ville = id_of(&p["revisionDraft"]["elements"][2]);
    assert_eq!(outline(&p)[2].2.as_deref(), Some(adresses.as_str()));
    assert_eq!(outline(&p)[1].2, None);

    // moveElement across levels, anchored on a sibling.
    let p = api
        .edit(
            &alice,
            "moveElement",
            "MoveElementInput",
            json!({ "procedureId": pid, "id": nom,
                    "placement": { "parentId": adresses, "beforeId": rue } }),
        )
        .await;
    assert_eq!(labels(&p), ["Adresses", "Ville", "Nom", "Rue"]);
    assert_eq!(outline(&p)[2].2.as_deref(), Some(adresses.as_str()));

    // updateColumn: type with a unit, label trimmed, id unchanged.
    let p = api
        .edit(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": nom, "label": " Surface ",
                    "type": { "decimal": { "unit": "SQUARE_METRE" } } }),
        )
        .await;
    let surface = &p["revisionDraft"]["elements"][2];
    assert_eq!(id_of(surface), nom);
    assert_eq!(surface["label"], "Surface");
    // Many values ride the type: only choices, attachments and
    // geometries carry `multiple` (G.7); the others have no such fact.
    assert!(surface["type"].get("multiple").is_none());
    let p = api
        .edit(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": nom, "type": { "attachment": { "multiple": true } } }),
        )
        .await;
    assert_eq!(p["revisionDraft"]["elements"][2]["type"]["multiple"], true);
    let p = api
        .edit(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": nom, "type": { "geometry": {} } }),
        )
        .await;
    assert_eq!(p["revisionDraft"]["elements"][2]["type"]["multiple"], false);
    // A text format rides the TEXT constructor (§2.6): a built-in,
    // then a custom pattern; a backtracking pattern is refused with
    // the draft unchanged; a type sent without a format clears it.
    let p = api
        .edit(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": nom,
                    "type": { "text": { "format": { "email": true } } } }),
        )
        .await;
    let ty = &p["revisionDraft"]["elements"][2]["type"];
    assert_eq!(ty["__typename"], "TextType");
    assert_eq!(ty["format"]["__typename"], "EmailFormat");
    let p = api
        .edit(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": nom,
                    "type": { "text": { "format": { "regex": { "pattern": "[0-9]{5}" } } } } }),
        )
        .await;
    assert_eq!(
        p["revisionDraft"]["elements"][2]["type"]["format"]["pattern"],
        "[0-9]{5}"
    );
    let code = api
        .edit_error(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": nom,
                    "type": { "text": { "format": { "regex": { "pattern": "(?=x)" } } } } }),
        )
        .await;
    assert_eq!(code, "INVALID_EDIT");
    let p = api
        .edit(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": nom, "type": { "text": {} } }),
        )
        .await;
    assert!(
        p["revisionDraft"]["elements"][2]["type"]["format"].is_null(),
        "{p}"
    );

    // Back to a decimal: a type without the fact.
    let p = api
        .edit(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": nom,
                    "type": { "decimal": { "unit": "SQUARE_METRE" } } }),
        )
        .await;
    let surface = &p["revisionDraft"]["elements"][2];
    assert!(surface["type"].get("multiple").is_none());
    assert_eq!(surface["type"]["__typename"], "DecimalType");
    assert_eq!(surface["type"]["unit"], "SQUARE_METRE");

    // An inline enum. An empty choice is accepted in the draft
    // (publication refuses it); then option ids are minted when
    // omitted and kept when given.
    let p = api
        .edit(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": ville, "type": { "enum": { "options": [] } } }),
        )
        .await;
    assert_eq!(
        p["revisionDraft"]["elements"][1]["type"]["options"],
        json!([])
    );
    let p = api
        .edit(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": ville,
                    "type": { "enum": { "options": [
                        { "label": "Paris" }, { "id": "lyon", "label": "Lyon" } ] } } }),
        )
        .await;
    let options = &p["revisionDraft"]["elements"][1]["type"]["options"];
    assert_eq!(options[0]["label"], "Paris");
    assert!(!options[0]["id"].as_str().unwrap().is_empty());
    assert_eq!(options[1], json!({ "id": "lyon", "label": "Lyon" }));

    // updateGroup, then removeElement of a column.
    let p = api
        .edit(
            &alice,
            "updateGroup",
            "UpdateGroupInput",
            json!({ "procedureId": pid, "id": adresses, "label": "Adresse", "cardinality": "ONE" }),
        )
        .await;
    assert_eq!(p["revisionDraft"]["elements"][0]["label"], "Adresse");
    assert_eq!(p["revisionDraft"]["elements"][0]["cardinality"], "ONE");
    let p = api
        .edit(
            &alice,
            "removeElement",
            "RemoveElementInput",
            json!({ "procedureId": pid, "id": rue }),
        )
        .await;
    assert_eq!(labels(&p), ["Adresse", "Ville", "Surface"]);

    // The read sees what the mutations answered with.
    let data = api.data(&alice, &read, json!({ "id": pid })).await;
    assert_eq!(labels(&data["procedure"]), ["Adresse", "Ville", "Surface"]);

    // Removing the group takes its subtree; discard empties the draft.
    let p = api
        .edit(
            &alice,
            "removeElement",
            "RemoveElementInput",
            json!({ "procedureId": pid, "id": adresses }),
        )
        .await;
    assert!(labels(&p).is_empty());
    let p = api
        .edit(
            &alice,
            "discardRevisionDraft",
            "DiscardRevisionDraftInput",
            json!({ "procedureId": pid }),
        )
        .await;
    // Discarded: back to the pristine virtual draft (the empty tree
    // here — never published).
    assert_eq!(p["revisionDraft"]["inProgress"], false, "{p}");
    assert_eq!(p["revisionDraft"]["elements"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn sections_notes_and_audiences_journey() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("tree"), "Org")
        .await;
    let procedure = api.create_procedure(&alice, &id_of(&org), "Aide").await;
    let pid = id_of(&procedure);

    // addSection: title trimmed, blank help collapses to null.
    let p = api
        .edit(
            &alice,
            "addSection",
            "AddSectionInput",
            json!({ "procedureId": pid, "title": " Identité ", "help": "   " }),
        )
        .await;
    let section = &p["revisionDraft"]["elements"][0];
    assert_eq!(section["__typename"], "Section");
    assert_eq!(section["title"], "Identité");
    assert!(section["help"].is_null());
    assert_eq!(section["audience"], "ALL");
    let sid = id_of(section);

    // A reviewer-only note inside the section: guidance DN's
    // annotations privées never had.
    let p = api
        .edit(
            &alice,
            "addNote",
            "AddNoteInput",
            json!({ "procedureId": pid, "body": "Vérifier la pièce.", "audience": "REVIEWER",
                    "placement": { "parentId": sid } }),
        )
        .await;
    let note = &p["revisionDraft"]["elements"][1];
    assert_eq!(note["__typename"], "Note");
    assert_eq!(note["parentId"], json!(sid));
    assert!(note["title"].is_null());
    assert_eq!(note["audience"], "REVIEWER");
    let nid = id_of(note);

    // A column inside the section, default audience.
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Nom", "type": { "text": {} },
                    "placement": { "parentId": sid } }),
        )
        .await;
    let nom = id_of(&p["revisionDraft"]["elements"][2]);
    assert_eq!(p["revisionDraft"]["elements"][2]["audience"], "ALL");
    // Effectively public at creation: required by default (G.7).
    assert_eq!(p["revisionDraft"]["elements"][2]["required"], true);

    // Narrow the whole section to reviewers…
    let p = api
        .edit(
            &alice,
            "updateSection",
            "UpdateSectionInput",
            json!({ "procedureId": pid, "id": sid, "audience": "REVIEWER", "help": "Interne" }),
        )
        .await;
    assert_eq!(p["revisionDraft"]["elements"][0]["audience"], "REVIEWER");
    assert_eq!(p["revisionDraft"]["elements"][0]["help"], "Interne");

    // …then explicitly widening a child is the refused contradiction
    // (P.4), while adding with the default clamps silently.
    let code = api
        .edit_error(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": nom, "audience": "ALL" }),
        )
        .await;
    assert_eq!(code, "INVALID_EDIT");
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Interne", "type": { "text": {} },
                    "placement": { "parentId": sid } }),
        )
        .await;
    assert_eq!(p["revisionDraft"]["elements"][3]["audience"], "REVIEWER");
    // Clamped reviewer-only at creation: optional by default (G.7).
    assert_eq!(p["revisionDraft"]["elements"][3]["required"], false);

    // The switchable half: always required, or not required.
    let p = api
        .edit(
            &alice,
            "updateColumn",
            "UpdateColumnInput",
            json!({ "procedureId": pid, "id": nom, "required": false }),
        )
        .await;
    assert_eq!(p["revisionDraft"]["elements"][2]["required"], false);

    // updateNote sets a title; an explicit null clears the section's
    // help (omitted leaves it — MaybeUndefined).
    let p = api
        .edit(
            &alice,
            "updateNote",
            "UpdateNoteInput",
            json!({ "procedureId": pid, "id": nid, "title": "Attention" }),
        )
        .await;
    assert_eq!(p["revisionDraft"]["elements"][1]["title"], "Attention");
    let p = api
        .edit(
            &alice,
            "updateSection",
            "UpdateSectionInput",
            json!({ "procedureId": pid, "id": sid, "help": null }),
        )
        .await;
    assert!(p["revisionDraft"]["elements"][0]["help"].is_null());

    // Moving out of the reviewer-only section restores the marker's
    // effect: the column authored ALL is ALL again at the root.
    let p = api
        .edit(
            &alice,
            "moveElement",
            "MoveElementInput",
            json!({ "procedureId": pid, "id": nom, "placement": {} }),
        )
        .await;
    let moved = p["revisionDraft"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| id_of(e) == nom)
        .unwrap()
        .clone();
    assert!(moved["parentId"].is_null());
    assert_eq!(moved["audience"], "ALL");

    // Removing the section takes its subtree.
    let p = api
        .edit(
            &alice,
            "removeElement",
            "RemoveElementInput",
            json!({ "procedureId": pid, "id": sid }),
        )
        .await;
    assert_eq!(labels(&p), ["Nom"]);
}

#[tokio::test]
async fn revision_draft_errors_are_structured() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let bob = account(&mut db, "bob").await;
    let org = api
        .create_organization(&alice, &unique_slug("draft-err"), "Org")
        .await;
    let procedure = api.create_procedure(&alice, &id_of(&org), "Permis").await;
    let pid = id_of(&procedure);
    let text = json!({ "text": {} });

    // FORBIDDEN: a non-member, a missing procedure — the same answer.
    for who in [&bob, &alice] {
        let target = if who.account_id == bob.account_id {
            pid.clone()
        } else {
            NIL.to_owned()
        };
        let code = api
            .edit_error(
                who,
                "addColumn",
                "AddColumnInput",
                json!({ "procedureId": target, "label": "x", "type": text }),
            )
            .await;
        assert_eq!(code, "FORBIDDEN");
    }

    // INVALID_INPUT: malformed id, blank label, a false marker, a blank
    // option label. (Two members, or none, fail `@oneOf` validation
    // before any resolver — no code, a plain validation error.)
    for input in [
        json!({ "procedureId": "nope", "label": "x", "type": text }),
        json!({ "procedureId": pid, "label": "  ", "type": text }),
        json!({ "procedureId": pid, "label": "x", "type": { "date": false } }),
        json!({ "procedureId": pid, "label": "x", "type": { "enum": { "options": [{ "label": " " }] } } }),
    ] {
        let code = api
            .edit_error(&alice, "addColumn", "AddColumnInput", input)
            .await;
        assert_eq!(code, "INVALID_INPUT");
    }

    // INVALID_EDIT: the draft or the kernel refuses — and the draft is
    // unchanged afterwards.
    let p = api
        .edit(
            &alice,
            "addGroup",
            "AddGroupInput",
            json!({ "procedureId": pid, "label": "Rows", "cardinality": "MANY" }),
        )
        .await;
    let rows = id_of(&p["revisionDraft"]["elements"][0]);
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Cell", "type": text,
                    "placement": { "parentId": rows } }),
        )
        .await;
    let cell = id_of(&p["revisionDraft"]["elements"][1]);
    for (field, input_type, input) in [
        // many inside many: depth policy
        (
            "addGroup",
            "AddGroupInput",
            json!({ "procedureId": pid, "label": "Deep", "cardinality": "MANY",
                    "placement": { "parentId": rows } }),
        ),
        // unknown parent
        (
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "x", "type": text,
                    "placement": { "parentId": NIL } }),
        ),
        // anchor outside its parent (cell is in rows, not at the root)
        (
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "x", "type": text,
                    "placement": { "beforeId": cell } }),
        ),
        // unknown element
        (
            "removeElement",
            "RemoveElementInput",
            json!({ "procedureId": pid, "id": NIL }),
        ),
        // a column updated as a group
        (
            "updateGroup",
            "UpdateGroupInput",
            json!({ "procedureId": pid, "id": cell, "label": "x" }),
        ),
        // a group moved into itself
        (
            "moveElement",
            "MoveElementInput",
            json!({ "procedureId": pid, "id": rows, "placement": { "parentId": rows } }),
        ),
    ] {
        let code = api.edit_error(&alice, field, input_type, input).await;
        assert_eq!(code, "INVALID_EDIT", "{field} {input_type}");
    }
    let read = format!("query($id: ID!) {{ procedure(id: $id) {{ id {DRAFT} }} }}");
    let data = api.data(&alice, &read, json!({ "id": pid })).await;
    assert_eq!(labels(&data["procedure"]), ["Rows", "Cell"]);

    // A non-member reads null, never the draft (G.6).
    let data = api.data(&bob, &read, json!({ "id": pid })).await;
    assert!(data["procedure"].is_null());
}

#[tokio::test]
async fn the_typed_client_edits_and_reads_the_draft() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("typed-draft"), "Org")
        .await;
    let procedure = api.create_procedure(&alice, &id_of(&org), "Typed").await;
    let client = api.client(&alice);

    let added = platform_client::run(
        &client,
        AddColumn::build(AddColumnVariables {
            input: AddColumnInput {
                procedure_id: cynic::Id::new(id_of(&procedure)),
                placement: None,
                label: "Surface".into(),
                ty: ColumnTypeInput::integer(Some(Unit::SquareMetre)),
                required: None,
                audience: None,
            },
        }),
    )
    .await
    .expect("addColumn")
    .add_column;
    let draft = added.revision_draft;
    assert!(draft.in_progress);
    assert!(draft.base.is_none());
    let Element::Column(column) = draft.elements[0].clone() else {
        panic!("{:?}", draft.elements);
    };
    assert_eq!(column.label, "Surface");
    assert!(column.parent_id.is_none());
    assert!(matches!(&column.ty, ColumnType::Integer(t) if t.unit == Some(Unit::SquareMetre)));

    let read = platform_client::run(
        &client,
        ProcedureRevisionDraftQuery::build(ProcedureRevisionDraftVariables {
            id: cynic::Id::new(id_of(&procedure)),
        }),
    )
    .await
    .expect("read")
    .procedure
    .expect("visible");
    assert_eq!(read.revision_draft, draft);

    // A refused edit arrives typed.
    let error = platform_client::run(
        &client,
        AddColumn::build(AddColumnVariables {
            input: AddColumnInput {
                procedure_id: cynic::Id::new(id_of(&procedure)),
                placement: Some(platform_client::revision_draft::PlacementInput {
                    parent_id: Some(column.id.clone()),
                    before_id: None,
                }),
                label: "Inside a column".into(),
                ty: ColumnTypeInput::text(),
                required: None,
                audience: None,
            },
        }),
    )
    .await
    .expect_err("a column is not a parent");
    assert_eq!(error.code(), Some(platform_client::Code::InvalidEdit));
}

#[test]
fn sdl_has_the_revision_draft_slice_and_no_recursive_type() {
    let sdl = schema().sdl();
    for needle in [
        "revisionDraft: RevisionDraft",
        "elements: [Element!]!",
        "union Element = Column | Group | Section | Note",
        "union ColumnType = TextType | BooleanType | IntegerType | DecimalType | DateType | DatetimeType | EnumType | AttachmentType | GeometryType",
        "enum Audience",
        "audience: Audience!",
        "required: Boolean!",
        "union TextFormat = EmailFormat | PhoneFormat | IbanFormat | RegexFormat",
        "format: TextFormat",
        "pattern: String!",
        "addColumn(input: AddColumnInput!): Procedure!",
        "addGroup(input: AddGroupInput!): Procedure!",
        "addSection(input: AddSectionInput!): Procedure!",
        "addNote(input: AddNoteInput!): Procedure!",
        "updateColumn(input: UpdateColumnInput!): Procedure!",
        "updateGroup(input: UpdateGroupInput!): Procedure!",
        "updateSection(input: UpdateSectionInput!): Procedure!",
        "updateNote(input: UpdateNoteInput!): Procedure!",
        "moveElement(input: MoveElementInput!): Procedure!",
        "removeElement(input: RemoveElementInput!): Procedure!",
        "discardRevisionDraft(input: DiscardRevisionDraftInput!): Procedure!",
        "beforeId: ID",
    ] {
        assert!(sdl.contains(needle), "missing {needle:?} in\n{sdl}");
    }
    // G.2 / G.5 Q1: the tree is flat — containers name their parent
    // and carry no children.
    for container in ["Group", "Section"] {
        let start = sdl.find(&format!("type {container} {{")).expect(container);
        let body = &sdl[start..start + sdl[start..].find('}').unwrap()];
        assert!(body.contains("parentId: ID"), "{body}");
        assert!(!body.contains('['), "{container} has a list field:\n{body}");
    }
}

/// The lifecycle read every test below shares: the state union with
/// every member inline, and the audit trail.
const PROCEDURE_LIFECYCLE: &str = "query($id: ID!) {
    procedure(id: $id) {
        id
        state {
            __typename
            ... on ProcedureDraftState { createdAt }
            ... on ProcedurePublishedState { since }
            ... on ProcedureClosedState { since }
        }
        events { kind actor { name } createdAt }
    }
}";

/// Publication does not exist yet (it arrives with the kernel edge),
/// so lifecycle tests force the row into `Published` directly.
async fn force_published(db: &mut toasty::Db, procedure_id: &str) {
    let id: uuid::Uuid = procedure_id.parse().expect("uuid");
    let mut procedure = platform_core::find_procedure(db, id)
        .await
        .expect("find")
        .expect("exists");
    procedure
        .update()
        .state(platform_core::ProcedureStateValue::Published)
        .state_since(Some(jiff::Timestamp::now()))
        .exec(db)
        .await
        .expect("force published");
}

#[tokio::test]
async fn procedure_lifecycle_over_the_schema() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("lifecycle"), "Org")
        .await;
    let org_id = id_of(&org);
    let procedure_id = id_of(&api.create_procedure(&alice, &org_id, "Bourse").await);

    // A new procedure is a draft: the union answers the draft member
    // with its one fact, the trail holds `created` with its actor.
    let read = api
        .data(&alice, PROCEDURE_LIFECYCLE, json!({ "id": &procedure_id }))
        .await["procedure"]
        .take();
    assert_eq!(read["state"]["__typename"], "ProcedureDraftState");
    assert!(read["state"]["createdAt"].is_string(), "{read}");
    assert_eq!(read["events"][0]["kind"], "CREATED");
    assert_eq!(read["events"][0]["actor"]["name"], "alice");

    // The Ref carries the bare enum (G.2 rule 5's parallel enum).
    let listed = api
        .data(
            &alice,
            "query($id: ID!) { organization(id: $id) { procedures { id state } } }",
            json!({ "id": &org_id }),
        )
        .await;
    assert_eq!(listed["organization"]["procedures"][0]["state"], "DRAFT");

    // The machine's refusals arrive as `INVALID_TRANSITION`, and a
    // non-member gets the uninformative `FORBIDDEN`.
    const CLOSE: &str = "mutation($input: CloseProcedureInput!) {
        closeProcedure(input: $input) { id state { __typename } }
    }";
    const REOPEN: &str = "mutation($input: ReopenProcedureInput!) {
        reopenProcedure(input: $input) { id state { __typename } }
    }";
    let input = json!({ "input": { "procedureId": &procedure_id } });
    assert_eq!(
        api.error_code(&alice, CLOSE, input.clone()).await,
        "INVALID_TRANSITION"
    );
    assert_eq!(
        api.error_code(&alice, REOPEN, input.clone()).await,
        "INVALID_TRANSITION"
    );
    let bob = account(&mut db, "bob").await;
    assert_eq!(
        api.error_code(&bob, CLOSE, input.clone()).await,
        "FORBIDDEN"
    );

    // Published (forced — publication waits on the kernel edge) →
    // closed → reopened, each answering with the new state member.
    force_published(&mut db, &procedure_id).await;
    let closed = api.data(&alice, CLOSE, input.clone()).await;
    assert_eq!(
        closed["closeProcedure"]["state"]["__typename"],
        "ProcedureClosedState"
    );
    let reopened = api.data(&alice, REOPEN, input.clone()).await;
    assert_eq!(
        reopened["reopenProcedure"]["state"]["__typename"],
        "ProcedurePublishedState"
    );

    // The trail tells the whole story, oldest first.
    let read = api
        .data(&alice, PROCEDURE_LIFECYCLE, json!({ "id": &procedure_id }))
        .await["procedure"]
        .take();
    let kinds: Vec<&str> = read["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["CREATED", "CLOSED", "REOPENED"]);
    assert!(read["state"]["since"].is_string(), "{read}");
}

#[tokio::test]
async fn the_typed_client_walks_the_lifecycle() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("typed-lc"), "Org")
        .await;
    let procedure_id = id_of(&api.create_procedure(&alice, &id_of(&org), "Aide").await);
    let client = api.client(&alice);

    let lifecycle = platform_client::run(
        &client,
        ProcedureLifecycleQuery::build(ProcedureLifecycleVariables {
            id: cynic::Id::new(&procedure_id),
        }),
    )
    .await
    .expect("lifecycle")
    .procedure
    .expect("visible to its administrator");
    assert!(matches!(lifecycle.state, ProcedureState::Draft(_)));
    assert_eq!(lifecycle.events[0].kind(), ProcedureEventKind::Created);
    assert_eq!(lifecycle.events[0].actor().expect("actor").name, "alice");

    // A refused transition arrives as the typed code.
    let refused = platform_client::run(
        &client,
        CloseProcedure::build(CloseProcedureVariables {
            input: CloseProcedureInput {
                procedure_id: cynic::Id::new(&procedure_id),
            },
        }),
    )
    .await
    .expect_err("closing a draft");
    assert_eq!(
        refused.code(),
        Some(platform_client::Code::InvalidTransition)
    );

    force_published(&mut db, &procedure_id).await;
    let closed = platform_client::run(
        &client,
        CloseProcedure::build(CloseProcedureVariables {
            input: CloseProcedureInput {
                procedure_id: cynic::Id::new(&procedure_id),
            },
        }),
    )
    .await
    .expect("close")
    .close_procedure;
    assert!(matches!(closed.state, ProcedureState::Closed(_)));

    let reopened = platform_client::run(
        &client,
        ReopenProcedure::build(ReopenProcedureVariables {
            input: ReopenProcedureInput {
                procedure_id: cynic::Id::new(&procedure_id),
            },
        }),
    )
    .await
    .expect("reopen")
    .reopen_procedure;
    let ProcedureState::Published(published) = &reopened.state else {
        panic!("expected published, got {:?}", reopened.state);
    };
    // Reopening reset `since` to the reopen, after the close.
    let ProcedureState::Closed(closed_state) = &closed.state else {
        unreachable!()
    };
    assert!(published.since >= closed_state.since);
    assert_eq!(
        reopened.events.iter().map(|e| e.kind()).collect::<Vec<_>>(),
        [
            ProcedureEventKind::Created,
            ProcedureEventKind::Closed,
            ProcedureEventKind::Reopened,
        ]
    );
}

const PUBLISH: &str = "mutation($input: PublishRevisionInput!) {
    publishRevision(input: $input) {
        published
        report {
            worst
            columns { columnId class change }
            surfaces { surface class change label }
        }
        procedure {
            id
            state { __typename ... on ProcedurePublishedState { since } }
            revisionDraft { base inProgress elements { ... on Column { label } } }
            events { kind }
        }
    }
}";

#[tokio::test]
async fn publish_revision_over_the_schema() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("publish"), "Org")
        .await;
    let pid = id_of(&api.create_procedure(&alice, &id_of(&org), "Bourse").await);
    let input = json!({ "input": { "procedureId": pid } });

    // Nothing in progress: INVALID_DRAFT.
    assert_eq!(
        api.error_code(&alice, PUBLISH, input.clone()).await,
        "INVALID_DRAFT"
    );

    // A draft with a column publishes free on first publication.
    api.edit(
        &alice,
        "addColumn",
        "AddColumnInput",
        json!({ "procedureId": pid, "label": "Nom", "type": { "text": {} } }),
    )
    .await;
    let data = api.data(&alice, PUBLISH, input.clone()).await;
    let result = &data["publishRevision"];
    assert_eq!(result["published"], true);
    assert_eq!(result["report"]["worst"], "SAFE");
    assert_eq!(
        result["procedure"]["state"]["__typename"],
        "ProcedurePublishedState"
    );
    // The draft is consumed: what remains is the pristine virtual
    // draft — the published head, base naming it (G.7).
    let draft = &result["procedure"]["revisionDraft"];
    assert_eq!(draft["inProgress"], false, "{result}");
    assert!(!draft["base"].is_null(), "{result}");
    assert_eq!(draft["elements"][0]["label"], "Nom", "{result}");
    let kinds: Vec<&str> = result["procedure"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["CREATED", "PUBLISHED"]);

    // The next draft forks from the head: base is the revision, and
    // the published tree seeds it (the column is already there).
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Ville", "type": { "text": {} } }),
        )
        .await;
    assert!(!p["revisionDraft"]["base"].is_null(), "{p}");
    let labels: Vec<&str> = p["revisionDraft"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["Nom", "Ville"]);

    // A non-member cannot publish.
    let bob = account(&mut db, "bob").await;
    assert_eq!(
        api.error_code(&bob, PUBLISH, input.clone()).await,
        "FORBIDDEN"
    );

    // An empty choice is refused at publication (G.7).
    api.edit(
        &alice,
        "addColumn",
        "AddColumnInput",
        json!({ "procedureId": pid, "label": "Choix", "type": { "enum": { "options": [] } } }),
    )
    .await;
    assert_eq!(
        api.error_code(&alice, PUBLISH, input.clone()).await,
        "INVALID_DRAFT"
    );
}

#[tokio::test]
async fn a_breaking_publication_gates_through_the_api() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("publish-gate"), "Org")
        .await;
    let pid = id_of(&api.create_procedure(&alice, &id_of(&org), "Aide").await);
    let input = json!({ "input": { "procedureId": pid } });

    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Champ", "type": { "text": {} } }),
        )
        .await;
    let column_id = p["revisionDraft"]["elements"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    api.data(&alice, PUBLISH, input.clone()).await;

    // Retype to a castless type: the report gates, nothing changes.
    api.edit(
        &alice,
        "updateColumn",
        "UpdateColumnInput",
        json!({ "procedureId": pid, "id": column_id, "type": { "geometry": {} } }),
    )
    .await;
    let data = api.data(&alice, PUBLISH, input.clone()).await;
    let result = &data["publishRevision"];
    assert_eq!(result["published"], false);
    assert_eq!(result["report"]["worst"], "BREAKING");
    assert_eq!(result["report"]["columns"][0]["columnId"], column_id);
    assert_eq!(result["report"]["columns"][0]["change"], "FORBIDDEN");
    // The procedure still shows the draft: nothing was consumed.
    assert_eq!(
        result["procedure"]["revisionDraft"]["inProgress"], true,
        "{result}"
    );

    // Confirmed through the typed client: published, report intact.
    let client = api.client(&alice);
    let confirmed = platform_client::run(
        &client,
        PublishRevision::build(PublishRevisionVariables {
            input: PublishRevisionInput {
                procedure_id: cynic::Id::new(&pid),
                confirm: true,
            },
        }),
    )
    .await
    .expect("confirmed publish")
    .publish_revision;
    assert!(confirmed.published);
    assert_eq!(confirmed.report.worst, ChangeClass::Breaking);
    assert!(matches!(
        confirmed.procedure.state,
        ProcedureState::Published(_)
    ));
    assert_eq!(
        confirmed
            .procedure
            .events
            .iter()
            .filter(|e| e.kind() == ProcedureEventKind::Published)
            .count(),
        2
    );
}

const HISTORY: &str = "query($id: ID!) {
    procedure(id: $id) {
        events {
            id kind actor { name } createdAt
            ... on ProcedurePublishedEvent { publication base }
        }
    }
}";

const EVENT_DIFF: &str = "query($id: ID!, $event: ID!) {
    procedure(id: $id) {
        event(id: $event) {
            kind
            ... on ProcedurePublishedEvent {
                publication
                base
                report { worst columns { columnId label class change } }
            }
        }
    }
}";

#[tokio::test]
async fn history_reads_each_publication_and_its_diff() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("history"), "Org")
        .await;
    let pid = id_of(&api.create_procedure(&alice, &id_of(&org), "Aide").await);
    let input = json!({ "input": { "procedureId": pid } });

    // First publication: one column.
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Nom", "type": { "text": {} } }),
        )
        .await;
    let nom_id = p["revisionDraft"]["elements"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    api.data(&alice, PUBLISH, input.clone()).await;

    // Second publication: one added, the first removed (SAFE —
    // hidden never deletes).
    api.edit(
        &alice,
        "addColumn",
        "AddColumnInput",
        json!({ "procedureId": pid, "label": "Ville", "type": { "text": {} } }),
    )
    .await;
    api.edit(
        &alice,
        "removeElement",
        "RemoveElementInput",
        json!({ "procedureId": pid, "id": nom_id }),
    )
    .await;
    // §3.1: the added column is required (the public default), so
    // the *surface* half of the report tightens admissibility while
    // every schema entry stays SAFE — the composed verdict is
    // CHECKED, the publication gates, and the surface section names
    // the tightening on both surfaces.
    let gated = api.data(&alice, PUBLISH, input.clone()).await;
    assert_eq!(gated["publishRevision"]["published"], false, "{gated}");
    let report = &gated["publishRevision"]["report"];
    assert_eq!(report["worst"], "CHECKED", "{report}");
    assert!(
        report["columns"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["class"] == "SAFE"),
        "{report}"
    );
    let presented: Vec<&Value> = report["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["change"] == "COLUMN_PRESENTED")
        .collect();
    assert_eq!(presented.len(), 2, "{report}");
    for entry in presented {
        assert_eq!(entry["class"], "CHECKED");
        assert_eq!(entry["label"], "Ville");
    }
    let confirmed = api
        .data(
            &alice,
            PUBLISH,
            json!({ "input": { "procedureId": pid, "confirm": true } }),
        )
        .await;
    assert_eq!(
        confirmed["publishRevision"]["published"], true,
        "{confirmed}"
    );

    // The trail carries the publication facts through the interface.
    let history = api
        .data(&alice, HISTORY, json!({ "id": pid.clone() }))
        .await;
    let events = history["procedure"]["events"].as_array().unwrap();
    let published: Vec<&Value> = events.iter().filter(|e| e["kind"] == "PUBLISHED").collect();
    assert_eq!(published.len(), 2, "{history}");
    assert!(published[0]["base"].is_null(), "{history}");
    assert!(!published[0]["publication"].is_null(), "{history}");
    assert_eq!(published[1]["base"], published[0]["publication"]);
    assert_eq!(published[0]["actor"]["name"], "alice");
    // A bare row carries no facts fields.
    assert!(events[0]["publication"].is_null(), "{history}");

    // The first publication's diff is the initial column list.
    let first = api
        .data(
            &alice,
            EVENT_DIFF,
            json!({ "id": pid.clone(), "event": published[0]["id"] }),
        )
        .await;
    let report = &first["procedure"]["event"]["report"];
    assert_eq!(report["worst"], "SAFE");
    assert_eq!(report["columns"][0]["label"], "Nom");
    assert_eq!(report["columns"][0]["change"], "ADDED");

    // The second names both sides: the addition from the next
    // schema, the removal by the base's label (G.11.5).
    let second = api
        .data(
            &alice,
            EVENT_DIFF,
            json!({ "id": pid.clone(), "event": published[1]["id"] }),
        )
        .await;
    let report = &second["procedure"]["event"]["report"];
    // The recomputed diff composes both halves, exactly as the gate
    // did (§3.1): the required addition keeps reading CHECKED.
    assert_eq!(report["worst"], "CHECKED");
    let mut named: Vec<(&str, &str)> = report["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["label"].as_str().unwrap(), c["change"].as_str().unwrap()))
        .collect();
    named.sort();
    assert_eq!(named, [("Nom", "REMOVED"), ("Ville", "ADDED")]);

    // An id that is not this procedure's answers null.
    let missing = api
        .data(
            &alice,
            EVENT_DIFF,
            json!({ "id": pid, "event": uuid::Uuid::now_v7().to_string() }),
        )
        .await;
    assert!(missing["procedure"]["event"].is_null(), "{missing}");
}

#[tokio::test]
async fn the_draft_report_reads_live() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("draft-report"), "Org")
        .await;
    let pid = id_of(&api.create_procedure(&alice, &id_of(&org), "Bourse").await);
    const REPORT: &str = "query($id: ID!) {
        procedure(id: $id) {
            revisionDraft { report { worst columns { columnId class change } } }
        }
    }";

    // A base-less draft classifies against the empty schema: ADDED,
    // SAFE.
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Champ", "type": { "text": {} } }),
        )
        .await;
    let column_id = p["revisionDraft"]["elements"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let data = api.data(&alice, REPORT, json!({ "id": pid })).await;
    let report = &data["procedure"]["revisionDraft"]["report"];
    assert_eq!(report["worst"], "SAFE");
    assert_eq!(report["columns"][0]["change"], "ADDED");

    // Published, then retyped: the report shows the breaking change
    // while editing — nothing was published to see it.
    api.data(
        &alice,
        "mutation($input: PublishRevisionInput!) {
            publishRevision(input: $input) { published }
        }",
        json!({ "input": { "procedureId": pid } }),
    )
    .await;
    api.edit(
        &alice,
        "updateColumn",
        "UpdateColumnInput",
        json!({ "procedureId": pid, "id": column_id, "type": { "geometry": {} } }),
    )
    .await;
    let data = api.data(&alice, REPORT, json!({ "id": pid })).await;
    let report = &data["procedure"]["revisionDraft"]["report"];
    assert_eq!(report["worst"], "BREAKING");
    assert_eq!(report["columns"][0]["columnId"], column_id);
    assert_eq!(report["columns"][0]["change"], "FORBIDDEN");
    // Still one publication: reading the report wrote nothing.
    let lifecycle = api
        .data(&alice, PROCEDURE_LIFECYCLE, json!({ "id": pid }))
        .await;
    let kinds: Vec<&str> = lifecycle["procedure"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["CREATED", "PUBLISHED"]);
}

/// The preview selection every preview write and read shares (G.12).
const PREVIEW: &str = "revisionDraft { inProgress preview {
    cells { __typename
        ... on TextCell { columnId path { groupId itemId } value }
        ... on BooleanCell { columnId value }
        ... on IntegerCell { columnId value }
        ... on EnumCell { columnId optionIds }
        ... on EmptyCell { columnId path { groupId itemId } }
    }
    items { groupId parent { groupId itemId } itemIds }
    findings { __typename
        ... on MissingRequiredFinding { surface columnId path { groupId itemId } }
        ... on FormatViolationFinding { surface columnId path { groupId itemId } }
    }
} }";

impl Api {
    async fn write_preview(&self, who: &Principal, procedure_id: &str, writes: Value) -> Value {
        let mut data = self
            .data(
                who,
                &format!(
                    "mutation($input: UpdatePreviewInput!) {{
                        updatePreview(input: $input) {{ id {PREVIEW} }}
                    }}"
                ),
                json!({ "input": { "procedureId": procedure_id, "writes": writes } }),
            )
            .await;
        data["updatePreview"].take()
    }

    async fn write_preview_error(
        &self,
        who: &Principal,
        procedure_id: &str,
        writes: Value,
    ) -> String {
        self.error_code(
            who,
            &format!(
                "mutation($input: UpdatePreviewInput!) {{
                    updatePreview(input: $input) {{ id {PREVIEW} }}
                }}"
            ),
            json!({ "input": { "procedureId": procedure_id, "writes": writes } }),
        )
        .await
    }

    async fn read_preview(&self, who: &Principal, procedure_id: &str) -> Value {
        let mut data = self
            .data(
                who,
                &format!("query($id: ID!) {{ procedure(id: $id) {{ id {PREVIEW} }} }}"),
                json!({ "id": procedure_id }),
            )
            .await;
        data["procedure"].take()
    }
}

/// `(surface, columnId)` of every finding, sorted.
fn finding_index(procedure: &Value) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = procedure["revisionDraft"]["preview"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .map(|f| {
            (
                f["surface"].as_str().expect("surface").to_owned(),
                f["columnId"].as_str().expect("columnId").to_owned(),
            )
        })
        .collect();
    found.sort();
    found
}

fn preview_cells(procedure: &Value) -> &Vec<Value> {
    procedure["revisionDraft"]["preview"]["cells"]
        .as_array()
        .expect("cells")
}

#[tokio::test]
async fn the_fillable_preview_journey() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("preview"), "Org")
        .await;
    let pid = id_of(&api.create_procedure(&alice, &id_of(&org), "Bourse").await);

    // A draft: one required public column, one `many` group with a
    // required column inside.
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Nom", "type": { "text": {} } }),
        )
        .await;
    let nom = id_of(&p["revisionDraft"]["elements"][0]);
    let p = api
        .edit(
            &alice,
            "addGroup",
            "AddGroupInput",
            json!({ "procedureId": pid, "label": "Enfants", "cardinality": "MANY" }),
        )
        .await;
    let enfants = id_of(&p["revisionDraft"]["elements"][1]);
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Prénom", "type": { "text": {} },
                    "placement": { "parentId": enfants } }),
        )
        .await;
    let prenom = id_of(&p["revisionDraft"]["elements"][2]);

    // The empty bag: no cells, no items — and the required column is
    // missing on both compiled surfaces. The group's column has no
    // rows yet, so it is not reachable and not missing (§2.6).
    let read = api.read_preview(&alice, &pid).await;
    assert_eq!(preview_cells(&read).len(), 0, "{read}");
    assert_eq!(
        finding_index(&read),
        [
            ("applicant".to_owned(), nom.clone()),
            ("reviewer".to_owned(), nom.clone())
        ],
        "{read}"
    );

    // Filling the required cell clears its findings; the cell reads
    // back typed.
    let p = api
        .write_preview(
            &alice,
            &pid,
            json!([{ "set": { "columnId": nom, "state": { "text": "Ada" } } }]),
        )
        .await;
    assert_eq!(finding_index(&p), [] as [(String, String); 0], "{p}");
    assert_eq!(preview_cells(&p)[0]["__typename"], "TextCell", "{p}");
    assert_eq!(preview_cells(&p)[0]["value"], "Ada", "{p}");

    // An item joins the `many` group: its server-minted id comes back
    // in `items`, and the required column inside is now missing on
    // that row, on both surfaces.
    let p = api
        .write_preview(&alice, &pid, json!([{ "addItem": { "groupId": enfants } }]))
        .await;
    let items = &p["revisionDraft"]["preview"]["items"];
    assert_eq!(items[0]["groupId"], enfants.as_str(), "{p}");
    let item = items[0]["itemIds"][0]
        .as_str()
        .expect("minted id")
        .to_owned();
    assert_eq!(
        finding_index(&p),
        [
            ("applicant".to_owned(), prenom.clone()),
            ("reviewer".to_owned(), prenom.clone())
        ],
        "{p}"
    );

    // Writing into the row through its path clears the finding.
    let p = api
        .write_preview(
            &alice,
            &pid,
            json!([{ "set": { "columnId": prenom,
                "path": [{ "groupId": enfants, "itemId": item }],
                "state": { "text": "Sam" } } }]),
        )
        .await;
    assert_eq!(finding_index(&p), [] as [(String, String); 0], "{p}");
    assert_eq!(preview_cells(&p).len(), 2, "{p}");

    // Filling never forks: this whole time, the working buffer is the
    // one the tree edits stored — the preview writes left `inProgress`
    // exactly as the tree left it.
    assert_eq!(p["revisionDraft"]["inProgress"], true, "{p}");

    // A refused batch stores nothing: unknown column, then a kind
    // mismatch on a known one — both `INVALID_WRITE`.
    assert_eq!(
        api.write_preview_error(
            &alice,
            &pid,
            json!([{ "set": { "columnId": "missing", "state": { "text": "x" } } }]),
        )
        .await,
        "INVALID_WRITE"
    );
    assert_eq!(
        api.write_preview_error(
            &alice,
            &pid,
            json!([{ "set": { "columnId": nom, "state": { "boolean": true } } }]),
        )
        .await,
        "INVALID_WRITE"
    );
    // A malformed scalar literal fails input decoding, before any use
    // case runs.
    assert_eq!(
        api.write_preview_error(
            &alice,
            &pid,
            json!([{ "set": { "columnId": nom, "state": { "empty": false } } }]),
        )
        .await,
        "INVALID_INPUT"
    );
    let read = api.read_preview(&alice, &pid).await;
    assert_eq!(preview_cells(&read).len(), 2, "{read}");

    // Removing the item cascades: its cells go with it.
    let p = api
        .write_preview(
            &alice,
            &pid,
            json!([{ "removeItem": { "groupId": enfants, "itemId": item } }]),
        )
        .await;
    assert_eq!(preview_cells(&p).len(), 1, "{p}");
    assert_eq!(p["revisionDraft"]["preview"]["items"], json!([]), "{p}");

    // Stale cells are inert (G.12): the column leaves the tree and
    // its cell disappears from the read — no error, nothing refused.
    api.edit(
        &alice,
        "removeElement",
        "RemoveElementInput",
        json!({ "procedureId": pid, "id": nom }),
    )
    .await;
    let read = api.read_preview(&alice, &pid).await;
    assert_eq!(preview_cells(&read).len(), 0, "{read}");
    assert_eq!(finding_index(&read), [] as [(String, String); 0], "{read}");
}

#[tokio::test]
async fn the_preview_is_scoped_to_a_draft_cycle() {
    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("preview-cycle"), "Org")
        .await;
    let pid = id_of(&api.create_procedure(&alice, &id_of(&org), "Bourse").await);
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Nom", "type": { "text": {} } }),
        )
        .await;
    let nom = id_of(&p["revisionDraft"]["elements"][0]);
    api.data(
        &alice,
        PUBLISH,
        json!({ "input": { "procedureId": pid, "confirm": true } }),
    )
    .await;

    // Filling the pristine head's preview does not fork the virtual
    // draft (G.12): the values store, `inProgress` stays false.
    let p = api
        .write_preview(
            &alice,
            &pid,
            json!([{ "set": { "columnId": nom, "state": { "text": "Ada" } } }]),
        )
        .await;
    assert_eq!(p["revisionDraft"]["inProgress"], false, "{p}");
    assert_eq!(preview_cells(&p).len(), 1, "{p}");

    // Discard clears the bag (a draft-cycle scope) and stays a no-op
    // for the pristine tree.
    let mut data = api
        .data(
            &alice,
            &format!(
                "mutation($input: DiscardRevisionDraftInput!) {{
                    discardRevisionDraft(input: $input) {{ id {PREVIEW} }}
                }}"
            ),
            json!({ "input": { "procedureId": pid } }),
        )
        .await;
    let p = data["discardRevisionDraft"].take();
    assert_eq!(preview_cells(&p).len(), 0, "{p}");
    assert_eq!(p["revisionDraft"]["inProgress"], false, "{p}");

    // Publication clears it too: fill, edit the tree, publish — the
    // new head answers the empty bag.
    api.write_preview(
        &alice,
        &pid,
        json!([{ "set": { "columnId": nom, "state": { "text": "Ada" } } }]),
    )
    .await;
    api.edit(
        &alice,
        "addColumn",
        "AddColumnInput",
        json!({ "procedureId": pid, "label": "Ville", "type": { "text": {} },
                "required": false }),
    )
    .await;
    let data = api
        .data(
            &alice,
            PUBLISH,
            json!({ "input": { "procedureId": pid, "confirm": true } }),
        )
        .await;
    assert_eq!(data["publishRevision"]["published"], true, "{data}");
    let read = api.read_preview(&alice, &pid).await;
    assert_eq!(preview_cells(&read).len(), 0, "{read}");
}

#[tokio::test]
async fn the_typed_client_fills_the_preview() {
    use platform_client::preview::{
        AdmissibilityFinding, Cell, CellStateInput, CellWriteInput, ProcedurePreviewQuery,
        ProcedurePreviewVariables, UpdatePreview, UpdatePreviewInput, UpdatePreviewVariables,
    };

    let Some(api) = api().await else { return };
    let mut db = api.db.clone();
    let alice = account(&mut db, "alice").await;
    let org = api
        .create_organization(&alice, &unique_slug("typed-preview"), "Org")
        .await;
    let pid = id_of(&api.create_procedure(&alice, &id_of(&org), "Bourse").await);
    let p = api
        .edit(
            &alice,
            "addColumn",
            "AddColumnInput",
            json!({ "procedureId": pid, "label": "Nom", "type": { "text": {} } }),
        )
        .await;
    let nom = id_of(&p["revisionDraft"]["elements"][0]);

    let client = api.client(&alice);
    let written = platform_client::run(
        &client,
        UpdatePreview::build(UpdatePreviewVariables {
            input: UpdatePreviewInput {
                procedure_id: cynic::Id::new(&pid),
                writes: vec![CellWriteInput::set(
                    cynic::Id::new(&nom),
                    vec![],
                    CellStateInput::text("Ada"),
                )],
            },
        }),
    )
    .await
    .expect("updatePreview")
    .update_preview;
    let preview = &written.revision_draft.preview;
    assert_eq!(preview.findings, []);
    match &preview.cells[0] {
        Cell::Text(cell) => {
            assert_eq!(cell.column_id.inner(), nom);
            assert_eq!(cell.value, "Ada");
        }
        other => panic!("expected a text cell, got {other:?}"),
    }

    // Blank the cell: the finding returns, the cell reads back
    // written-blank — distinct from absent.
    let written = platform_client::run(
        &client,
        UpdatePreview::build(UpdatePreviewVariables {
            input: UpdatePreviewInput {
                procedure_id: cynic::Id::new(&pid),
                writes: vec![CellWriteInput::set(
                    cynic::Id::new(&nom),
                    vec![],
                    CellStateInput::empty(),
                )],
            },
        }),
    )
    .await
    .expect("updatePreview")
    .update_preview;
    let preview = &written.revision_draft.preview;
    assert!(matches!(preview.cells[0], Cell::Empty(_)), "{preview:?}");
    assert!(
        preview
            .findings
            .iter()
            .all(|f| matches!(f, AdmissibilityFinding::MissingRequired(_))),
        "{preview:?}"
    );
    assert_eq!(preview.findings.len(), 2, "{preview:?}");

    let read = platform_client::run(
        &client,
        ProcedurePreviewQuery::build(ProcedurePreviewVariables {
            id: cynic::Id::new(&pid),
        }),
    )
    .await
    .expect("read")
    .procedure
    .expect("visible");
    assert_eq!(read.revision_draft.preview, written.revision_draft.preview);
}
