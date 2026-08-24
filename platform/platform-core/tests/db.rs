//! DB-backed integration tests, gated on `VARVE_TEST_DATABASE_URL`
//! (the settled P.3 convention: `cargo test --workspace` stays green
//! without Postgres; CI provides a service container). Run for real
//! with e.g.:
//!
//! ```text
//! VARVE_TEST_DATABASE_URL=postgres://localhost/varve_platform_test \
//!   cargo test -p platform-core
//! ```
//!
//! Tests share one database and run in parallel, so every test mints
//! unique emails/token hashes and never asserts on global counts.

use jiff::{SignedDuration, Timestamp};
use platform_core::{
    Audience, ColumnPatch, Placement, RevisionDraftError, TreeColumn, TreeElement, add_element,
    update_column,
};
use platform_core::{
    CreateApiTokenError, CreateOrganizationError, DEFAULT_SESSION_TTL, MAX_USER_AGENT_CHARS,
    RegisterError, add_organization_member, add_team_member, connect, create_api_token,
    create_organization, create_organization_for, create_procedure, create_session, create_team,
    delete_account_sessions, delete_session, destroy_api_token, destroy_session,
    discard_revision_draft, edit_revision_draft, find_live_api_token, find_live_session,
    find_organization_by_slug, find_procedure, find_procedure_with_revision_draft,
    is_organization_member, list_account_organizations, list_account_teams, list_live_api_tokens,
    list_live_sessions, list_organization_procedures, list_organization_teams, new_column_id,
    register, remove_organization_member, remove_team_member, revision_draft_tree, sweep_expired,
    sweep_expired_api_tokens, update_profile, verify_credentials,
};
use platform_core::{
    LifecycleError, ProcedureEventKind, ProcedureStateValue, PublishProcedureError,
    PublishProcedureOutcome, TransitionError, close_procedure, current_state,
    list_procedure_events, publish_procedure, reopen_procedure,
};
use varve_schema::{Arity, ScalarType};

/// Connects to the test database, applying migrations; `None` (after
/// printing why) when `VARVE_TEST_DATABASE_URL` is unset so the test
/// passes vacuously.
/// Concurrent connects (parallel tests, nextest's one process per
/// test) are safe: `connect` serializes migration application with a
/// database-side advisory lock.
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

fn unique_email(tag: &str) -> String {
    format!("{tag}+{}@example.test", uuid::Uuid::new_v4())
}

fn unique_hash(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4())
}

#[tokio::test]
async fn register_then_verify() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let email = unique_email("verify");

    let account = register(&mut db, &email, "s3cret", "Alice", None)
        .await
        .expect("register");
    assert_eq!(account.email, email);
    assert_eq!(account.name, "Alice");
    assert!(account.password_hash.starts_with("$argon2id$"));
    assert_eq!(account.locale, None);

    // Right password — and a case/whitespace-variant email must
    // normalize to the same account.
    let found = verify_credentials(&mut db, &format!("  {}  ", email.to_uppercase()), "s3cret")
        .await
        .expect("verify");
    assert_eq!(found.map(|a| a.id), Some(account.id));

    // Wrong password and unknown email are the same `None`.
    assert!(
        verify_credentials(&mut db, &email, "wrong")
            .await
            .expect("verify")
            .is_none()
    );
    assert!(
        verify_credentials(&mut db, &unique_email("ghost"), "s3cret")
            .await
            .expect("verify")
            .is_none()
    );
}

#[tokio::test]
async fn duplicate_email_is_a_typed_error() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let email = unique_email("dup");

    register(&mut db, &email, "first", "First", None)
        .await
        .expect("register");
    // Same email modulo normalization: still taken.
    let err = register(&mut db, &email.to_uppercase(), "second", "Second", None)
        .await
        .expect_err("duplicate must fail");
    assert!(matches!(err, RegisterError::EmailTaken), "got: {err:?}");
}

#[tokio::test]
async fn session_lifecycle() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let email = unique_email("session");
    let account = register(&mut db, &email, "pw", "Sess", None)
        .await
        .expect("register");

    let now = Timestamp::now();
    let hash = unique_hash("lifecycle");
    let session = create_session(
        &mut db,
        account.id,
        &hash,
        now,
        DEFAULT_SESSION_TTL,
        None,
        None,
    )
    .await
    .expect("create_session");
    assert_eq!(session.account_id, account.id);
    assert_eq!(session.created_at, now);
    assert_eq!(session.expires_at, now + DEFAULT_SESSION_TTL);

    // Live at `now`, live just before expiry, absent at expiry
    // (strict comparison) and after.
    for (probe, live) in [
        (now, true),
        (
            now + DEFAULT_SESSION_TTL - SignedDuration::from_secs(1),
            true,
        ),
        (now + DEFAULT_SESSION_TTL, false),
        (
            now + DEFAULT_SESSION_TTL + SignedDuration::from_secs(1),
            false,
        ),
    ] {
        let found = find_live_session(&mut db, &hash, probe)
            .await
            .expect("find");
        assert_eq!(found.is_some(), live, "probe at {probe}");
    }

    delete_session(&mut db, &hash).await.expect("delete");
    assert!(
        find_live_session(&mut db, &hash, now)
            .await
            .expect("find")
            .is_none()
    );
    // Idempotent.
    delete_session(&mut db, &hash).await.expect("delete twice");
}

#[tokio::test]
async fn sign_out_everywhere() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let account = register(&mut db, &unique_email("everywhere"), "pw", "Multi", None)
        .await
        .expect("register");

    let now = Timestamp::now();
    let hashes: Vec<String> = (0..3).map(|i| unique_hash(&format!("multi{i}"))).collect();
    for hash in &hashes {
        create_session(
            &mut db,
            account.id,
            hash,
            now,
            DEFAULT_SESSION_TTL,
            None,
            None,
        )
        .await
        .expect("create");
    }

    delete_account_sessions(&mut db, account.id)
        .await
        .expect("delete all");
    for hash in &hashes {
        assert!(
            find_live_session(&mut db, hash, now)
                .await
                .expect("find")
                .is_none()
        );
    }
}

#[tokio::test]
async fn sweep_collects_only_expired() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let account = register(&mut db, &unique_email("sweep"), "pw", "Sweep", None)
        .await
        .expect("register");

    let now = Timestamp::now();
    let expired = unique_hash("expired");
    let live = unique_hash("live");
    create_session(
        &mut db,
        account.id,
        &expired,
        now - SignedDuration::from_hours(2),
        SignedDuration::from_hours(1),
        None,
        None,
    )
    .await
    .expect("create expired");
    create_session(
        &mut db,
        account.id,
        &live,
        now,
        DEFAULT_SESSION_TTL,
        None,
        None,
    )
    .await
    .expect("create live");

    sweep_expired(&mut db, now).await.expect("sweep");

    // The expired row is gone outright (absent even for a probe
    // instant at which it was live), the live one untouched.
    assert!(
        find_live_session(&mut db, &expired, now - SignedDuration::from_mins(90))
            .await
            .expect("find")
            .is_none()
    );
    assert!(
        find_live_session(&mut db, &live, now)
            .await
            .expect("find")
            .is_some()
    );
}

#[tokio::test]
async fn session_metadata_is_stored_and_user_agent_truncated() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let account = register(&mut db, &unique_email("metadata"), "pw", "Meta", None)
        .await
        .expect("register");

    let now = Timestamp::now();
    // A multibyte character straddling the limit must not split: the
    // truncation counts characters, not bytes.
    let long_agent = "é".repeat(MAX_USER_AGENT_CHARS + 100);
    let session = create_session(
        &mut db,
        account.id,
        &unique_hash("metadata"),
        now,
        DEFAULT_SESSION_TTL,
        Some(&long_agent),
        Some("203.0.113.7"),
    )
    .await
    .expect("create");
    assert_eq!(
        session.user_agent.as_deref(),
        Some("é".repeat(MAX_USER_AGENT_CHARS).as_str())
    );
    assert_eq!(session.ip.as_deref(), Some("203.0.113.7"));

    // A short agent is stored verbatim, and both columns stay `None`
    // when nothing was presented.
    let short = create_session(
        &mut db,
        account.id,
        &unique_hash("metadata-short"),
        now,
        DEFAULT_SESSION_TTL,
        Some("TestBrowser/1.0"),
        None,
    )
    .await
    .expect("create short");
    assert_eq!(short.user_agent.as_deref(), Some("TestBrowser/1.0"));
    assert_eq!(short.ip, None);
}

#[tokio::test]
async fn list_live_sessions_is_scoped_live_only_and_newest_first() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let account = register(&mut db, &unique_email("list"), "pw", "List", None)
        .await
        .expect("register");
    let other = register(&mut db, &unique_email("list-other"), "pw", "Other", None)
        .await
        .expect("register other");

    let now = Timestamp::now();
    // Three sessions for the account — one expired — and one for
    // another account that must never appear.
    let oldest = create_session(
        &mut db,
        account.id,
        &unique_hash("list-oldest"),
        now - SignedDuration::from_hours(2),
        DEFAULT_SESSION_TTL,
        None,
        None,
    )
    .await
    .expect("create oldest");
    let newest = create_session(
        &mut db,
        account.id,
        &unique_hash("list-newest"),
        now,
        DEFAULT_SESSION_TTL,
        None,
        None,
    )
    .await
    .expect("create newest");
    create_session(
        &mut db,
        account.id,
        &unique_hash("list-expired"),
        now - SignedDuration::from_hours(2),
        SignedDuration::from_hours(1),
        None,
        None,
    )
    .await
    .expect("create expired");
    create_session(
        &mut db,
        other.id,
        &unique_hash("list-foreign"),
        now,
        DEFAULT_SESSION_TTL,
        None,
        None,
    )
    .await
    .expect("create foreign");

    let sessions = list_live_sessions(&mut db, account.id, now)
        .await
        .expect("list");
    assert_eq!(
        sessions.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![newest.id, oldest.id],
        "live sessions of the account only, newest first"
    );
}

#[tokio::test]
async fn destroy_session_is_scoped_to_the_account() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let owner = register(&mut db, &unique_email("destroy"), "pw", "Owner", None)
        .await
        .expect("register");
    let attacker = register(
        &mut db,
        &unique_email("destroy-attacker"),
        "pw",
        "Attacker",
        None,
    )
    .await
    .expect("register attacker");

    let now = Timestamp::now();
    let hash = unique_hash("destroy");
    let session = create_session(
        &mut db,
        owner.id,
        &hash,
        now,
        DEFAULT_SESSION_TTL,
        None,
        None,
    )
    .await
    .expect("create");

    // The authorization boundary: another account's id cannot destroy
    // the session — quiet `false`, row intact.
    assert!(
        !destroy_session(&mut db, attacker.id, session.id)
            .await
            .expect("scoped destroy")
    );
    assert!(
        find_live_session(&mut db, &hash, now)
            .await
            .expect("find")
            .is_some(),
        "the row survives a foreign destroy attempt"
    );

    // The owner destroys it; a second attempt reports nothing to do.
    assert!(
        destroy_session(&mut db, owner.id, session.id)
            .await
            .expect("destroy")
    );
    assert!(
        find_live_session(&mut db, &hash, now)
            .await
            .expect("find")
            .is_none()
    );
    assert!(
        !destroy_session(&mut db, owner.id, session.id)
            .await
            .expect("destroy twice")
    );
}

#[tokio::test]
async fn register_trims_the_name_and_rejects_empty_fields() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let account = register(&mut db, &unique_email("trim"), "pw", "  Élodie \n", None)
        .await
        .expect("register");
    assert_eq!(account.name, "Élodie");

    // Empty after normalization/trimming, or an empty password: a
    // typed error, no row.
    for (email, password, name) in [
        ("   ", "pw", "Name"),
        (&unique_email("noname"), "pw", "  "),
        (&unique_email("nopw"), "", "Name"),
    ] {
        let err = register(&mut db, email, password, name, None)
            .await
            .expect_err("empty field must fail");
        assert!(matches!(err, RegisterError::EmptyField), "got: {err:?}");
    }
    assert!(
        verify_credentials(&mut db, "", "pw")
            .await
            .expect("verify")
            .is_none()
    );
}

#[tokio::test]
async fn register_stores_the_locale() {
    let Some(mut db) = test_db().await else {
        return;
    };
    // The value is opaque to platform-core: whatever the app passes
    // is what comes back.
    let account = register(&mut db, &unique_email("locale"), "pw", "Loc", Some("fr"))
        .await
        .expect("register");
    assert_eq!(account.locale.as_deref(), Some("fr"));

    let found = verify_credentials(&mut db, &account.email, "pw")
        .await
        .expect("verify")
        .expect("the account exists");
    assert_eq!(found.locale.as_deref(), Some("fr"));
}

#[tokio::test]
async fn update_profile_trims_persists_and_returns_the_row() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let account = register(&mut db, &unique_email("profile"), "pw", "Before", None)
        .await
        .expect("register");

    let updated = update_profile(&mut db, account.id, "  Après  ", Some("fr"))
        .await
        .expect("update_profile");
    assert_eq!(updated.id, account.id);
    assert_eq!(updated.name, "Après", "the name is stored trimmed");
    assert_eq!(updated.locale.as_deref(), Some("fr"));
    assert!(
        updated.updated_at > account.updated_at,
        "the returned row is the updated one"
    );

    // Persisted, not just reloaded in memory.
    let found = verify_credentials(&mut db, &account.email, "pw")
        .await
        .expect("verify")
        .expect("the account exists");
    assert_eq!(found.name, "Après");
    assert_eq!(found.locale.as_deref(), Some("fr"));

    // `None` leaves the stored locale untouched.
    let updated = update_profile(&mut db, account.id, "Nom Final", None)
        .await
        .expect("update_profile without locale");
    assert_eq!(updated.name, "Nom Final");
    assert_eq!(updated.locale.as_deref(), Some("fr"));
}

fn unique_slug(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4())
}

#[tokio::test]
async fn organization_slug_is_normalized_and_unique() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let slug = unique_slug("org");

    let org = create_organization(&mut db, &format!("  {}  ", slug.to_uppercase()), " DGFiP ")
        .await
        .expect("create");
    assert_eq!(org.slug, slug);
    assert_eq!(org.name, "DGFiP");

    let err = create_organization(&mut db, &slug, "Again")
        .await
        .expect_err("duplicate slug must fail");
    assert!(
        matches!(err, CreateOrganizationError::SlugTaken),
        "got: {err:?}"
    );

    let found = find_organization_by_slug(&mut db, &slug.to_uppercase())
        .await
        .expect("find");
    assert_eq!(found.map(|o| o.id), Some(org.id));
    assert!(
        find_organization_by_slug(&mut db, &unique_slug("ghost"))
            .await
            .expect("find")
            .is_none()
    );
}

#[tokio::test]
async fn organization_membership_is_idempotent_and_independent_of_teams() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let admin = register(&mut db, &unique_email("admin"), "pw", "Admin", None)
        .await
        .expect("register");
    let reviewer = register(&mut db, &unique_email("reviewer"), "pw", "Reviewer", None)
        .await
        .expect("register");
    let org = create_organization(&mut db, &unique_slug("org"), "Org")
        .await
        .expect("create org");

    // Adding twice keeps one membership.
    add_organization_member(&mut db, org.id, admin.id)
        .await
        .expect("add");
    add_organization_member(&mut db, org.id, admin.id)
        .await
        .expect("add again");
    assert!(
        is_organization_member(&mut db, org.id, admin.id)
            .await
            .unwrap()
    );
    assert!(
        !is_organization_member(&mut db, org.id, reviewer.id)
            .await
            .unwrap()
    );

    let orgs = list_account_organizations(&mut db, admin.id).await.unwrap();
    assert_eq!(orgs.iter().map(|o| o.id).collect::<Vec<_>>(), vec![org.id]);

    // A reviewer in one of the org's teams is not thereby an org member.
    let team = create_team(&mut db, org.id, " Guichet ")
        .await
        .expect("team");
    assert_eq!(team.name, "Guichet");
    assert_eq!(team.organization_id, org.id);
    add_team_member(&mut db, team.id, reviewer.id)
        .await
        .expect("add");
    add_team_member(&mut db, team.id, reviewer.id)
        .await
        .expect("add again");
    assert!(
        !is_organization_member(&mut db, org.id, reviewer.id)
            .await
            .unwrap()
    );
    assert!(
        list_account_organizations(&mut db, reviewer.id)
            .await
            .unwrap()
            .is_empty()
    );

    let teams = list_account_teams(&mut db, reviewer.id).await.unwrap();
    assert_eq!(
        teams.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![team.id]
    );
    assert!(
        list_account_teams(&mut db, admin.id)
            .await
            .unwrap()
            .is_empty()
    );
    let org_teams = list_organization_teams(&mut db, org.id).await.unwrap();
    assert_eq!(
        org_teams.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![team.id]
    );

    // The derived `via` relations agree with the service queries.
    let members = org.members().exec(&mut db).await.unwrap();
    assert_eq!(
        members.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![admin.id]
    );
    let reviewers = team.members().exec(&mut db).await.unwrap();
    assert_eq!(
        reviewers.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![reviewer.id]
    );

    // Removal: a no-op for non-members, effective for members.
    remove_organization_member(&mut db, org.id, reviewer.id)
        .await
        .expect("noop");
    remove_organization_member(&mut db, org.id, admin.id)
        .await
        .expect("remove");
    assert!(
        !is_organization_member(&mut db, org.id, admin.id)
            .await
            .unwrap()
    );
    remove_team_member(&mut db, team.id, admin.id)
        .await
        .expect("noop");
    remove_team_member(&mut db, team.id, reviewer.id)
        .await
        .expect("remove");
    assert!(
        list_account_teams(&mut db, reviewer.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn organization_owns_procedures() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let org = create_organization(&mut db, &unique_slug("org"), "Org")
        .await
        .expect("create org");
    let other = create_organization(&mut db, &unique_slug("other"), "Other")
        .await
        .expect("create org");
    let admin = register(&mut db, &unique_email("admin"), "s3cret", "Admin", None)
        .await
        .expect("register");

    let first = create_procedure(&mut db, org.id, admin.id, " Demande de bourse ", "")
        .await
        .expect("procedure");
    assert_eq!(first.title, "Demande de bourse");
    assert_eq!(first.description, "");
    let second = create_procedure(&mut db, org.id, admin.id, "Permis", " Desc ")
        .await
        .expect("procedure");
    assert_eq!(second.description, "Desc");
    create_procedure(&mut db, other.id, admin.id, "Elsewhere", "")
        .await
        .expect("procedure");

    let listed = list_organization_procedures(&mut db, org.id).await.unwrap();
    assert_eq!(
        listed.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
    let via = org.procedures().exec(&mut db).await.unwrap();
    assert_eq!(via.len(), 2);
    assert_eq!(
        second.organization().exec(&mut db).await.unwrap().id,
        org.id
    );
}

#[tokio::test]
async fn api_token_lifecycle() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let account = register(&mut db, &unique_email("token"), "pw", "Tok", None)
        .await
        .expect("register");
    let now = jiff::Timestamp::now();

    let issued = create_api_token(&mut db, account.id, "  CI deploy ", now)
        .await
        .expect("create");
    assert_eq!(issued.token.name, "CI deploy");
    assert!(issued.secret.starts_with("varve_"), "{}", issued.secret);
    assert!(issued.token.prefix.len() < issued.secret.len());
    assert!(issued.secret.starts_with(&issued.token.prefix));
    // Nothing recoverable is stored: the row holds the hash only.
    assert_ne!(issued.token.token_hash, issued.secret);
    assert_eq!(issued.token.created_at, now);
    assert_eq!(
        issued.token.expires_at,
        now.to_zoned(jiff::tz::TimeZone::UTC)
            .checked_add(jiff::Span::new().months(6))
            .unwrap()
            .timestamp()
    );

    // The secret resolves while live, not once expired, and a wrong
    // secret resolves to nothing.
    let found = find_live_api_token(&mut db, &issued.secret, now)
        .await
        .expect("find")
        .expect("live token resolves");
    assert_eq!(found.id, issued.token.id);
    assert_eq!(found.account_id, account.id);
    assert!(
        find_live_api_token(&mut db, &issued.secret, issued.token.expires_at)
            .await
            .expect("find")
            .is_none()
    );
    assert!(
        find_live_api_token(&mut db, "varve_not-a-real-secret", now)
            .await
            .expect("find")
            .is_none()
    );

    // Listing is newest first and live-only.
    let later = now.checked_add(jiff::SignedDuration::from_secs(1)).unwrap();
    let second = create_api_token(&mut db, account.id, "Second", later)
        .await
        .expect("create");
    let ids: Vec<_> = list_live_api_tokens(&mut db, account.id, later)
        .await
        .expect("list")
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ids, vec![second.token.id, issued.token.id]);
    assert!(
        list_live_api_tokens(&mut db, account.id, issued.token.expires_at)
            .await
            .expect("list")
            .iter()
            .all(|t| t.id == second.token.id)
    );

    // Revocation is scoped to the owning account.
    let other = register(&mut db, &unique_email("token-other"), "pw", "Other", None)
        .await
        .expect("register");
    assert!(
        !destroy_api_token(&mut db, other.id, issued.token.id)
            .await
            .expect("destroy")
    );
    assert!(
        destroy_api_token(&mut db, account.id, issued.token.id)
            .await
            .expect("destroy")
    );
    assert!(
        find_live_api_token(&mut db, &issued.secret, now)
            .await
            .expect("find")
            .is_none()
    );

    // The sweep collects only expired rows.
    sweep_expired_api_tokens(&mut db, second.token.expires_at)
        .await
        .expect("sweep");
    assert!(
        list_live_api_tokens(&mut db, account.id, later)
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn api_token_name_is_validated() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let account = register(&mut db, &unique_email("token-name"), "pw", "Tok", None)
        .await
        .expect("register");
    let now = jiff::Timestamp::now();
    let err = create_api_token(&mut db, account.id, "   ", now)
        .await
        .expect_err("empty name");
    assert!(matches!(err, CreateApiTokenError::EmptyName), "{err:?}");
    let err = create_api_token(&mut db, account.id, &"x".repeat(101), now)
        .await
        .expect_err("long name");
    assert!(matches!(err, CreateApiTokenError::NameTooLong), "{err:?}");
    assert!(
        list_live_api_tokens(&mut db, account.id, now)
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn create_organization_for_is_one_transaction() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let creator = register(
        &mut db,
        &unique_email("creator"),
        "s3cret-enough",
        "Creator",
        None,
    )
    .await
    .expect("register");

    // The creator is the first member.
    let slug = unique_slug("org");
    let org = create_organization_for(&mut db, &slug, "Org", creator.id)
        .await
        .expect("create");
    assert!(
        is_organization_member(&mut db, org.id, creator.id)
            .await
            .expect("member")
    );

    // A failing second write rolls the first back: an unknown creator
    // violates the membership's account foreign key (migration 0005),
    // and the organization row must not survive it — its slug stays
    // free.
    let slug = unique_slug("orphan");
    let err = create_organization_for(&mut db, &slug, "Orphan", uuid::Uuid::new_v4())
        .await
        .expect_err("unknown creator must fail");
    assert!(
        matches!(err, CreateOrganizationError::Db(_)),
        "got: {err:?}"
    );
    assert!(
        find_organization_by_slug(&mut db, &slug)
            .await
            .expect("find")
            .is_none(),
        "organization row survived a rolled-back transaction"
    );

    // A duplicate slug is still the typed error, and adds no membership.
    let err = create_organization_for(&mut db, &org.slug, "Again", creator.id)
        .await
        .expect_err("duplicate slug must fail");
    assert!(
        matches!(err, CreateOrganizationError::SlugTaken),
        "got: {err:?}"
    );
}

#[tokio::test]
async fn procedure_draft_round_trips_through_edits() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let org = create_organization(&mut db, &unique_slug("draft"), "Org")
        .await
        .expect("create org");
    let admin = register(&mut db, &unique_email("draft"), "s3cret", "Admin", None)
        .await
        .expect("register");
    let created = create_procedure(&mut db, org.id, admin.id, "Bourse", "")
        .await
        .expect("procedure");

    // A fresh procedure has no draft; the first edit starts one from
    // the empty schema.
    let mut procedure = find_procedure_with_revision_draft(&mut db, created.id)
        .await
        .unwrap()
        .expect("exists");
    assert_eq!(revision_draft_tree(&procedure).unwrap(), None);
    let id = new_column_id();
    let stored = edit_revision_draft(&mut db, &mut procedure, |tree| {
        add_element(
            tree,
            &Placement::root(),
            TreeElement::Column(TreeColumn {
                id: id.clone(),
                label: "Nom".into(),
                ty: ScalarType::Text,
                arity: Arity::One,
                format: None,
                required: true,
                audience: Audience::All,
            }),
        )
    })
    .await
    .expect("first edit");
    assert_eq!(stored.elements.len(), 1);

    // The stored bytes decode to the same value on a fresh load, and
    // the next edit builds on them; the catalog lookup never loads it.
    let mut reloaded = find_procedure_with_revision_draft(&mut db, created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        revision_draft_tree(&reloaded).unwrap(),
        Some(stored.clone())
    );
    assert!(
        find_procedure(&mut db, created.id)
            .await
            .unwrap()
            .unwrap()
            .revision_draft
            .is_unloaded()
    );
    let stored = edit_revision_draft(&mut db, &mut reloaded, |tree| {
        update_column(
            tree,
            &id,
            ColumnPatch {
                label: Some("Nom de famille".into()),
                ..Default::default()
            },
        )
    })
    .await
    .expect("second edit");
    match &stored.elements[0] {
        TreeElement::Column(c) => assert_eq!(c.label, "Nom de famille"),
        _ => panic!(),
    }

    // A rejected operation stores nothing.
    let err = edit_revision_draft(&mut db, &mut reloaded, |tree| {
        update_column(tree, &new_column_id(), ColumnPatch::default())
    })
    .await
    .unwrap_err();
    assert!(matches!(err, RevisionDraftError::Edit(_)));
    let again = find_procedure_with_revision_draft(&mut db, created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(revision_draft_tree(&again).unwrap(), Some(stored));

    // `procedure` is the stale copy from before the second edit:
    // optimistic concurrency refuses its save rather than overwriting.
    let conflict = edit_revision_draft(&mut db, &mut procedure, |_| Ok(()))
        .await
        .unwrap_err();
    assert!(matches!(conflict, RevisionDraftError::Db(_)), "{conflict}");

    let mut fresh = find_procedure_with_revision_draft(&mut db, created.id)
        .await
        .unwrap()
        .unwrap();
    discard_revision_draft(&mut db, &mut fresh)
        .await
        .expect("discard");
    assert_eq!(revision_draft_tree(&fresh).unwrap(), None);
    let after = find_procedure_with_revision_draft(&mut db, created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(revision_draft_tree(&after).unwrap(), None);
}

/// Seeds an organization, an acting account and a procedure for the
/// lifecycle tests.
async fn lifecycle_fixture(
    db: &mut toasty::Db,
    tag: &str,
) -> (platform_core::Account, platform_core::Procedure) {
    let org = create_organization(db, &unique_slug(tag), "Org")
        .await
        .expect("create org");
    let admin = register(db, &unique_email(tag), "s3cret", "Admin", None)
        .await
        .expect("register");
    let procedure = create_procedure(db, org.id, admin.id, "Bourse", "")
        .await
        .expect("procedure");
    (admin, procedure)
}

#[tokio::test]
async fn a_new_procedure_is_a_draft_and_logs_created() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let (admin, procedure) = lifecycle_fixture(&mut db, "lifecycle-new").await;

    assert_eq!(procedure.state, ProcedureStateValue::Draft);
    assert_eq!(procedure.state_since, None);
    assert_eq!(
        current_state(&procedure).unwrap(),
        platform_core::ProcedureState::Draft
    );

    let events = list_procedure_events(&mut db, procedure.id).await.unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| (e.kind, e.actor_account_id))
            .collect::<Vec<_>>(),
        vec![(ProcedureEventKind::Created, Some(admin.id))]
    );
}

#[tokio::test]
async fn close_and_reopen_transition_and_log() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let (admin, mut procedure) = lifecycle_fixture(&mut db, "lifecycle-close").await;

    // A draft cannot close: the machine refuses before anything is
    // written.
    let refused = close_procedure(&mut db, &mut procedure, admin.id)
        .await
        .unwrap_err();
    assert!(
        matches!(
            refused,
            LifecycleError::Transition(TransitionError::NotPublished(ProcedureStateValue::Draft))
        ),
        "{refused}"
    );

    // Publication does not exist yet (it arrives with the kernel
    // edge), so the test forces the row into `Published` directly to
    // exercise the closed half of the machine.
    let opened = jiff::Timestamp::now();
    procedure
        .update()
        .state(ProcedureStateValue::Published)
        .state_since(Some(opened))
        .exec(&mut db)
        .await
        .expect("force published");

    close_procedure(&mut db, &mut procedure, admin.id)
        .await
        .expect("close");
    assert_eq!(procedure.state, ProcedureStateValue::Closed);
    let closed_since = procedure.state_since.expect("closed since");
    assert!(closed_since >= opened);

    // Reopen resets `since` — the reason the column is not a
    // publication date.
    reopen_procedure(&mut db, &mut procedure, admin.id)
        .await
        .expect("reopen");
    assert_eq!(procedure.state, ProcedureStateValue::Published);
    assert!(procedure.state_since.expect("reopened since") >= closed_since);

    let reopened = reopen_procedure(&mut db, &mut procedure, admin.id)
        .await
        .unwrap_err();
    assert!(
        matches!(
            reopened,
            LifecycleError::Transition(TransitionError::NotClosed(ProcedureStateValue::Published))
        ),
        "{reopened}"
    );

    // The log carries the whole story, oldest first, with the actor
    // on every row — and the refused transitions left no entries.
    let events = list_procedure_events(&mut db, procedure.id).await.unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| (e.kind, e.actor_account_id))
            .collect::<Vec<_>>(),
        vec![
            (ProcedureEventKind::Created, Some(admin.id)),
            (ProcedureEventKind::Closed, Some(admin.id)),
            (ProcedureEventKind::Reopened, Some(admin.id)),
        ]
    );

    // The columns a fresh read sees match the in-place row.
    let fetched = find_procedure(&mut db, procedure.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.state, procedure.state);
    assert_eq!(fetched.state_since, procedure.state_since);
}

#[tokio::test]
async fn discarding_a_draft_is_not_an_event() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let (_, procedure) = lifecycle_fixture(&mut db, "lifecycle-discard").await;

    let mut loaded = find_procedure_with_revision_draft(&mut db, procedure.id)
        .await
        .unwrap()
        .unwrap();
    edit_revision_draft(&mut db, &mut loaded, |tree| {
        add_element(
            tree,
            &Placement::root(),
            draft_column("Nom", ScalarType::Text),
        )
    })
    .await
    .expect("start a draft");
    discard_revision_draft(&mut db, &mut loaded)
        .await
        .expect("discard");
    assert_eq!(revision_draft_tree(&loaded).unwrap(), None);

    // The trail records lifecycle facts only: discarding the working
    // buffer is authoring workflow, absent by design (P.4).
    let events = list_procedure_events(&mut db, procedure.id).await.unwrap();
    assert_eq!(
        events.iter().map(|e| e.kind).collect::<Vec<_>>(),
        vec![ProcedureEventKind::Created]
    );
}

/// Runs the publish use case the way production does — platform
/// writes on a real transaction — but with `MemoryStore` as the
/// kernel store (the §13.2 oracle; `platform-store`'s own tests own
/// the Postgres store contract).
async fn publish(
    db: &mut toasty::Db,
    store: &varve_store::MemoryStore,
    procedure_id: uuid::Uuid,
    actor: uuid::Uuid,
    confirm: bool,
) -> Result<(PublishProcedureOutcome, platform_core::Procedure), PublishProcedureError> {
    let mut procedure = find_procedure_with_revision_draft(db, procedure_id)
        .await?
        .expect("procedure exists");
    let mut tx = db.transaction().await?;
    let shared: platform_core::SharedExecutor =
        tokio::sync::Mutex::new(&mut tx as &mut dyn toasty::Executor);
    let outcome = publish_procedure(&shared, store, &mut procedure, actor, confirm).await?;
    drop(shared);
    tx.commit().await?;
    Ok((outcome, procedure))
}

fn draft_column(label: &str, ty: varve_schema::ScalarType) -> TreeElement {
    TreeElement::Column(TreeColumn {
        id: new_column_id(),
        label: label.into(),
        ty,
        arity: Arity::One,
        format: None,
        required: true,
        audience: Audience::All,
    })
}

#[tokio::test]
async fn publish_walks_the_whole_lifecycle() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let store = varve_store::MemoryStore::default();
    let (admin, procedure) = lifecycle_fixture(&mut db, "publish").await;

    // Nothing to publish yet.
    let err = publish(&mut db, &store, procedure.id, admin.id, false)
        .await
        .expect_err("no draft");
    assert!(matches!(err, PublishProcedureError::NoDraft), "{err}");

    // Author a draft and publish it: first publication is free.
    let mut loaded = find_procedure_with_revision_draft(&mut db, procedure.id)
        .await
        .unwrap()
        .unwrap();
    edit_revision_draft(&mut db, &mut loaded, |tree| {
        add_element(
            tree,
            &Placement::root(),
            draft_column("Nom", ScalarType::Text),
        )
    })
    .await
    .expect("draft");
    let (outcome, published) = publish(&mut db, &store, procedure.id, admin.id, false)
        .await
        .expect("publish");
    let PublishProcedureOutcome::Published { revision, report } = outcome else {
        panic!("first publication must not gate");
    };
    assert_eq!(report.worst(), varve_impact::ChangeClass::Safe);
    assert_eq!(published.state, ProcedureStateValue::Published);
    assert_eq!(
        published.latest_revision.as_deref(),
        Some(revision.as_str())
    );
    assert_eq!(published.revision_draft.get(), &None);

    // The kernel store holds the pair, compiled against the revision.
    use varve_store::SurfaceStore;
    let surfaces = store.surfaces(&revision).await.expect("surfaces");
    assert_eq!(
        surfaces.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["applicant", "reviewer"]
    );

    // The event carries its facts.
    let events = list_procedure_events(&mut db, procedure.id).await.unwrap();
    let published_event = events
        .iter()
        .find(|e| e.kind == ProcedureEventKind::Published)
        .expect("published event");
    let facts = published_event
        .facts
        .as_ref()
        .expect("facts")
        .decode()
        .expect("decode");
    assert_eq!(facts.revision, revision.as_str());
    assert_eq!(facts.base, None);

    // The next draft forks from the head.
    let mut loaded = find_procedure_with_revision_draft(&mut db, procedure.id)
        .await
        .unwrap()
        .unwrap();
    edit_revision_draft(&mut db, &mut loaded, |tree| {
        add_element(
            tree,
            &Placement::root(),
            draft_column("Ville", ScalarType::Text),
        )
    })
    .await
    .expect("second draft");
    let draft_base = loaded
        .revision_draft
        .get()
        .as_ref()
        .expect("draft")
        .base
        .clone();
    assert_eq!(draft_base.as_deref(), Some(revision.as_str()));

    // Adding a column is free: publishes without confirmation, facts
    // carry the base, and `since` is untouched (the procedure never
    // closed).
    let opened_since = published.state_since;
    let (outcome, republished) = publish(&mut db, &store, procedure.id, admin.id, false)
        .await
        .expect("second publish");
    let PublishProcedureOutcome::Published { revision: r2, .. } = outcome else {
        panic!("a free change publishes");
    };
    assert_ne!(r2, revision);
    assert_eq!(republished.state_since, opened_since);
    let events = list_procedure_events(&mut db, procedure.id).await.unwrap();
    let last_facts = events
        .iter()
        .rev()
        .find(|e| e.kind == ProcedureEventKind::Published)
        .unwrap()
        .facts
        .as_ref()
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(last_facts.base.as_deref(), Some(revision.as_str()));

    // Close, draft a change while closed, publish: the reopen.
    let mut row = find_procedure(&mut db, procedure.id)
        .await
        .unwrap()
        .unwrap();
    close_procedure(&mut db, &mut row, admin.id)
        .await
        .expect("close");
    let mut loaded = find_procedure_with_revision_draft(&mut db, procedure.id)
        .await
        .unwrap()
        .unwrap();
    edit_revision_draft(&mut db, &mut loaded, |tree| {
        add_element(
            tree,
            &Placement::root(),
            draft_column("Pays", ScalarType::Text),
        )
    })
    .await
    .expect("closed draft");
    let (outcome, reopened) = publish(&mut db, &store, procedure.id, admin.id, false)
        .await
        .expect("publish from closed");
    assert!(matches!(outcome, PublishProcedureOutcome::Published { .. }));
    assert_eq!(reopened.state, ProcedureStateValue::Published);
}

#[tokio::test]
async fn a_breaking_publication_gates_and_writes_nothing() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let store = varve_store::MemoryStore::default();
    let (admin, procedure) = lifecycle_fixture(&mut db, "publish-gate").await;

    let mut loaded = find_procedure_with_revision_draft(&mut db, procedure.id)
        .await
        .unwrap()
        .unwrap();
    let column_id = new_column_id();
    edit_revision_draft(&mut db, &mut loaded, |tree| {
        add_element(
            tree,
            &Placement::root(),
            TreeElement::Column(TreeColumn {
                id: column_id.clone(),
                label: "Champ".into(),
                ty: ScalarType::Text,
                arity: Arity::One,
                format: None,
                required: false,
                audience: Audience::All,
            }),
        )
    })
    .await
    .expect("draft");
    publish(&mut db, &store, procedure.id, admin.id, false)
        .await
        .expect("v1");

    // Retype the column to something with no cast: breaking.
    let mut loaded = find_procedure_with_revision_draft(&mut db, procedure.id)
        .await
        .unwrap()
        .unwrap();
    edit_revision_draft(&mut db, &mut loaded, |tree| {
        update_column(
            tree,
            &column_id,
            ColumnPatch {
                ty: Some(ScalarType::Geometry),
                ..ColumnPatch::default()
            },
        )
        .map(|_| ())
    })
    .await
    .expect("retype");

    let (outcome, unchanged) = publish(&mut db, &store, procedure.id, admin.id, false)
        .await
        .expect("gate");
    let PublishProcedureOutcome::RequiresConfirmation { report } = outcome else {
        panic!("a breaking change must gate");
    };
    assert_eq!(report.worst(), varve_impact::ChangeClass::Breaking);
    // Nothing moved: the draft is intact and no event was logged.
    assert!(unchanged.revision_draft.get().is_some());
    let events = list_procedure_events(&mut db, procedure.id).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == ProcedureEventKind::Published)
            .count(),
        1
    );

    // Confirmed: published.
    let (outcome, _) = publish(&mut db, &store, procedure.id, admin.id, true)
        .await
        .expect("confirmed");
    assert!(matches!(outcome, PublishProcedureOutcome::Published { .. }));
}

#[tokio::test]
async fn an_empty_choice_is_refused_at_publication() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let store = varve_store::MemoryStore::default();
    let (admin, procedure) = lifecycle_fixture(&mut db, "publish-enum").await;

    let mut loaded = find_procedure_with_revision_draft(&mut db, procedure.id)
        .await
        .unwrap()
        .unwrap();
    let column_id = new_column_id();
    edit_revision_draft(&mut db, &mut loaded, |tree| {
        add_element(
            tree,
            &Placement::root(),
            TreeElement::Column(TreeColumn {
                id: column_id.clone(),
                label: "Choix".into(),
                ty: ScalarType::Enum(varve_schema::NomenclatureRef::Inline(vec![])),
                arity: Arity::One,
                format: None,
                required: false,
                audience: Audience::All,
            }),
        )
    })
    .await
    .expect("draft");

    let err = publish(&mut db, &store, procedure.id, admin.id, false)
        .await
        .expect_err("refused");
    match err {
        PublishProcedureError::EmptyEnum(id) => assert_eq!(id, column_id),
        other => panic!("unexpected: {other}"),
    }
}
