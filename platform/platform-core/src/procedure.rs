//! The procedure catalog row (P.4): title, description, owning
//! organization. **Shell only for now** — the revision DAG, surfaces,
//! rules, and the open/closed lifecycle arrive with the kernel edge
//! (P1, `varve-service`); this row exists so organizations own
//! something and the ownership relation is real.

use toasty::Deferred;

use crate::organization::Organization;

/// A procedure's catalog entry.
#[derive(Debug, toasty::Model)]
pub struct Procedure {
    /// UUID v7 (time-ordered), generated on insert.
    #[key]
    #[auto]
    pub id: uuid::Uuid,

    /// The owning organization; its members administer this
    /// procedure (P.4).
    #[index]
    pub organization_id: uuid::Uuid,

    /// The owning organization (relation).
    #[belongs_to(key = organization_id, references = id)]
    pub organization: Deferred<Organization>,

    /// Title shown to applicants and reviewers.
    pub title: String,

    /// Free-text description; empty when none.
    pub description: String,

    /// Set on insert.
    #[auto]
    pub created_at: jiff::Timestamp,

    /// Set on insert and on every update.
    #[auto]
    pub updated_at: jiff::Timestamp,
}

/// Creates a procedure catalog row owned by `organization_id`.
/// Title and description are stored trimmed.
pub async fn create_procedure(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
    title: &str,
    description: &str,
) -> toasty::Result<Procedure> {
    Procedure::create()
        .organization_id(organization_id)
        .title(title.trim())
        .description(description.trim())
        .exec(db)
        .await
}

/// The procedures an organization owns, oldest first.
pub async fn list_organization_procedures(
    db: &mut toasty::Db,
    organization_id: uuid::Uuid,
) -> toasty::Result<Vec<Procedure>> {
    Procedure::filter_by_organization_id(organization_id)
        .order_by(Procedure::fields().created_at().asc())
        .exec(db)
        .await
}
