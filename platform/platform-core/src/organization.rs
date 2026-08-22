//! Organizations and organization membership (P.4, settled
//! 2026-08-21).
//!
//! An [`Organization`] is the owning administration: it owns
//! procedures and teams. [`OrganizationMembership`] is a plain
//! account⟷organization join row **with no role column**: membership
//! *is* the right — a member administers every procedure the
//! organization owns (the procedure-administrator principal, held at
//! the organization rather than per procedure as DN did). Team
//! membership ([`crate::team`]) is independent: a reviewer is
//! commonly not an organization member.
//!
//! **Slug normalization happens here, in the service functions**, on
//! the same discipline as [`crate::account`]'s email: [`create_organization`]
//! trims and lowercases the slug before storage, so the plain
//! `#[unique]` index is effectively case-insensitive as long as every
//! write path goes through this module (P.3).

use toasty::Deferred;

use crate::account::Account;
use crate::procedure::Procedure;
use crate::team::Team;

/// An administration that owns procedures and teams.
#[derive(Debug, toasty::Model)]
pub struct Organization {
    /// UUID v7 (time-ordered), generated on insert.
    #[key]
    #[auto]
    pub id: uuid::Uuid,

    /// Normalized (trimmed, lowercased) URL/API handle, unique across
    /// the platform — see the module docs.
    #[unique]
    pub slug: String,

    /// Display name.
    pub name: String,

    /// Set on insert.
    #[auto]
    pub created_at: jiff::Timestamp,

    /// Set on insert and on every update.
    #[auto]
    pub updated_at: jiff::Timestamp,

    /// The join rows; mutate these to add or remove members.
    #[has_many]
    pub memberships: Deferred<Vec<OrganizationMembership>>,

    /// Accounts holding membership — read-only derived relation.
    #[has_many(via = memberships.account)]
    pub members: Deferred<Vec<Account>>,

    /// Teams of this organization (P.4: teams are organization-level,
    /// not per procedure).
    #[has_many]
    pub teams: Deferred<Vec<Team>>,

    /// Procedures this organization owns.
    #[has_many]
    pub procedures: Deferred<Vec<Procedure>>,
}

/// One account's membership in one organization. The composite key
/// makes the link unique; there is deliberately nothing else on the
/// row (no role — P.4).
#[derive(Debug, toasty::Model)]
#[key(organization_id, account_id)]
pub struct OrganizationMembership {
    /// The organization. The leading key column needs no separate
    /// index: the composite primary key already serves that prefix.
    pub organization_id: uuid::Uuid,

    /// The organization (relation).
    #[belongs_to]
    pub organization: Deferred<Organization>,

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

/// Failure modes of [`create_organization`].
#[derive(Debug, thiserror::Error)]
pub enum CreateOrganizationError {
    /// An organization with this (normalized) slug already exists —
    /// settled race-free by the unique index, like
    /// [`crate::RegisterError::EmailTaken`].
    #[error("an organization with this slug already exists")]
    SlugTaken,
    /// The underlying store failed.
    #[error("database error: {0}")]
    Db(#[from] toasty::Error),
}

/// Normalizes a slug for storage and lookup: trim, lowercase. What
/// characters a slug may contain is the caller's validation (P.3);
/// this crate only guarantees the stored form is canonical.
pub fn normalize_slug(slug: &str) -> String {
    slug.trim().to_lowercase()
}

/// Creates an organization. Duplicate slugs are a typed error: the
/// insert is insert-or-ignore against the `slug` unique index, so two
/// concurrent creations cannot both succeed. Generic over the
/// executor so it runs inside a caller's transaction
/// ([`create_organization_for`]).
pub async fn create_organization(
    db: &mut impl toasty::Executor,
    slug: &str,
    name: &str,
) -> Result<Organization, CreateOrganizationError> {
    let slug = normalize_slug(slug);
    let created = Organization::upsert_by_slug(&slug)
        .name(name.trim())
        .or_ignore()
        .exec(db)
        .await?;
    created.ok_or(CreateOrganizationError::SlugTaken)
}

/// Looks an organization up by id.
pub async fn find_organization(
    db: &mut toasty::Db,
    id: uuid::Uuid,
) -> toasty::Result<Option<Organization>> {
    Organization::filter_by_id(id).first().exec(db).await
}

/// Looks an organization up by its (normalized) slug.
pub async fn find_organization_by_slug(
    db: &mut toasty::Db,
    slug: &str,
) -> toasty::Result<Option<Organization>> {
    Organization::filter_by_slug(normalize_slug(slug))
        .first()
        .exec(db)
        .await
}

/// Creates an organization **and makes `creator_id` its first
/// member** — the use case behind "create organization": without
/// that second write the organization is reachable by nobody
/// (membership is the only right, P.4). One transaction (the shape
/// P.9 Q10 settled): either both rows exist or neither, so an
/// organization whose slug is taken but that nobody can reach cannot
/// come out of a crash. An early return (duplicate slug, store error)
/// drops the transaction, which rolls it back.
pub async fn create_organization_for(
    db: &mut toasty::Db,
    slug: &str,
    name: &str,
    creator_id: uuid::Uuid,
) -> Result<Organization, CreateOrganizationError> {
    let mut tx = db.transaction().await?;
    let organization = create_organization(&mut tx, slug, name).await?;
    add_organization_member(&mut tx, organization.id, creator_id).await?;
    tx.commit().await?;
    Ok(organization)
}

/// Adds an account to an organization. Idempotent: an existing
/// membership is left as is (insert-or-ignore on the composite key).
/// Generic over the executor like [`create_organization`].
pub async fn add_organization_member(
    db: &mut impl toasty::Executor,
    organization_id: uuid::Uuid,
    account_id: uuid::Uuid,
) -> toasty::Result<()> {
    OrganizationMembership::upsert_by_organization_id_and_account_id(organization_id, account_id)
        .or_ignore()
        .exec(db)
        .await?;
    Ok(())
}

/// Removes an account from an organization. Removing a non-member is
/// a no-op, not an error.
pub async fn remove_organization_member(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
    account_id: uuid::Uuid,
) -> toasty::Result<()> {
    OrganizationMembership::filter_by_organization_id_and_account_id(organization_id, account_id)
        .delete()
        .exec(db)
        .await
}

/// Whether `account_id` is a member of `organization_id` — i.e.
/// administers its procedures.
pub async fn is_organization_member(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
    account_id: uuid::Uuid,
) -> toasty::Result<bool> {
    Ok(
        OrganizationMembership::filter_by_organization_id_and_account_id(
            organization_id,
            account_id,
        )
        .first()
        .exec(db)
        .await?
        .is_some(),
    )
}

/// The members of an organization, oldest membership first, each
/// with the instant the membership was created. Two queries (the
/// join rows, then the accounts in one `IN` list), never one per
/// member.
pub async fn list_organization_members(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
) -> toasty::Result<Vec<Member>> {
    let memberships = OrganizationMembership::filter_by_organization_id(organization_id)
        .order_by(OrganizationMembership::fields().created_at().asc())
        .exec(db)
        .await?;
    let joined: Vec<(uuid::Uuid, jiff::Timestamp)> = memberships
        .into_iter()
        .map(|m| (m.account_id, m.created_at))
        .collect();
    crate::account::members_of(db, joined).await
}

/// How many procedures the organization owns.
pub async fn count_organization_procedures(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
) -> toasty::Result<u64> {
    Procedure::filter_by_organization_id(organization_id)
        .count()
        .exec(db)
        .await
}

/// How many teams the organization has.
pub async fn count_organization_teams(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
) -> toasty::Result<u64> {
    Team::filter_by_organization_id(organization_id)
        .count()
        .exec(db)
        .await
}

/// How many members the organization has.
pub async fn count_organization_members(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
) -> toasty::Result<u64> {
    OrganizationMembership::filter_by_organization_id(organization_id)
        .count()
        .exec(db)
        .await
}

/// One account's membership in a container (organization or team),
/// as read APIs expose it: the account plus when it joined.
#[derive(Debug)]
pub struct Member {
    /// The member account.
    pub account: Account,
    /// When the membership was created.
    pub joined_at: jiff::Timestamp,
}

/// The organizations `account_id` is a member of, oldest first.
pub async fn list_account_organizations(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
) -> toasty::Result<Vec<Organization>> {
    Organization::filter(
        Organization::fields()
            .memberships()
            .any(OrganizationMembership::fields().account_id().eq(account_id)),
    )
    .order_by(Organization::fields().created_at().asc())
    .exec(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_normalization() {
        assert_eq!(normalize_slug("  Ville-De-Paris \n"), "ville-de-paris");
        assert_eq!(normalize_slug("dgfip"), "dgfip");
    }
}
