//! Teams and team membership (P.4, settled 2026-08-21).
//!
//! A [`Team`] (groupe instructeur) belongs to an organization, not to
//! a procedure — unlike DN, where identical groups get re-created per
//! procedure. Its entire effect is surface assignment over a set of
//! case files; a procedure's routing rules select among the owning
//! organization's teams (P.9 Q13 holds the per-procedure routable
//! subset). [`TeamMembership`] is a plain account⟷team join row with
//! no role: being in a team *is* being a reviewer. It is independent
//! of organization membership ([`crate::organization`]).

use toasty::Deferred;

use crate::account::Account;
use crate::organization::Organization;

/// A reviewer team of an organization.
#[derive(Debug, toasty::Model)]
pub struct Team {
    /// UUID v7 (time-ordered), generated on insert.
    #[key]
    #[auto]
    pub id: uuid::Uuid,

    /// The owning organization.
    #[index]
    pub organization_id: uuid::Uuid,

    /// The owning organization (relation).
    #[belongs_to]
    pub organization: Deferred<Organization>,

    /// Display name, unique within the organization.
    pub name: String,

    /// Set on insert.
    #[auto]
    pub created_at: jiff::Timestamp,

    /// Set on insert and on every update.
    #[auto]
    pub updated_at: jiff::Timestamp,

    /// The join rows; mutate these to add or remove members.
    #[has_many]
    pub memberships: Deferred<Vec<TeamMembership>>,

    /// Accounts in the team (reviewers) — read-only derived relation.
    #[has_many(via = memberships.account)]
    pub members: Deferred<Vec<Account>>,
}

/// One account's membership in one team. No role column (P.4).
#[derive(Debug, toasty::Model)]
#[key(team_id, account_id)]
pub struct TeamMembership {
    /// The team. The leading key column needs no separate index: the
    /// composite primary key already serves that prefix.
    pub team_id: uuid::Uuid,

    /// The team (relation).
    #[belongs_to]
    pub team: Deferred<Team>,

    /// The member account.
    #[index]
    pub account_id: uuid::Uuid,

    /// The member account (relation).
    #[belongs_to]
    pub account: Deferred<Account>,

    /// Set on insert.
    #[auto]
    pub created_at: jiff::Timestamp,
}

/// Creates a team in an organization. The name is stored trimmed;
/// an empty trimmed name is the caller's validation (P.3).
pub async fn create_team(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
    name: &str,
) -> toasty::Result<Team> {
    Team::create()
        .organization_id(organization_id)
        .name(name.trim())
        .exec(db)
        .await
}

/// Adds an account to a team. Idempotent (insert-or-ignore on the
/// composite key).
pub async fn add_team_member(
    db: &mut toasty::Db,
    team_id: uuid::Uuid,
    account_id: uuid::Uuid,
) -> toasty::Result<()> {
    TeamMembership::upsert_by_team_id_and_account_id(team_id, account_id)
        .or_ignore()
        .exec(db)
        .await?;
    Ok(())
}

/// Removes an account from a team. Removing a non-member is a no-op.
pub async fn remove_team_member(
    db: &mut toasty::Db,
    team_id: uuid::Uuid,
    account_id: uuid::Uuid,
) -> toasty::Result<()> {
    TeamMembership::filter_by_team_id_and_account_id(team_id, account_id)
        .delete()
        .exec(db)
        .await
}

/// The teams of an organization, oldest first.
pub async fn list_organization_teams(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
) -> toasty::Result<Vec<Team>> {
    Team::filter_by_organization_id(organization_id)
        .order_by(Team::fields().created_at().asc())
        .exec(db)
        .await
}

/// The teams `account_id` reviews for, across organizations, oldest
/// first.
pub async fn list_account_teams(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
) -> toasty::Result<Vec<Team>> {
    Team::filter(
        Team::fields()
            .memberships()
            .any(TeamMembership::fields().account_id().eq(account_id)),
    )
    .order_by(Team::fields().created_at().asc())
    .exec(db)
    .await
}
