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
fn normalize_slug(slug: &str) -> String {
    slug.trim().to_lowercase()
}

/// Creates an organization. Duplicate slugs are a typed error: the
/// insert is insert-or-ignore against the `slug` unique index, so two
/// concurrent creations cannot both succeed.
pub async fn create_organization(
    db: &mut toasty::Db,
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

/// Adds an account to an organization. Idempotent: an existing
/// membership is left as is (insert-or-ignore on the composite key).
pub async fn add_organization_member(
    db: &mut toasty::Db,
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
