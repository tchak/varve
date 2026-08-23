//! The procedure catalog row (P.4): title, description, owning
//! organization — and the **revision draft**, the schema being
//! edited before it is published (P.4, *Procedure drafts*). The
//! revision DAG, surfaces, rules, and the open/closed lifecycle still
//! arrive with the kernel edge (`varve-service`); until then the
//! draft is the one kernel value the platform holds.
//!
//! The draft is a nullable embedded object on the row, deferred out
//! of the catalog `SELECT`: its schema travels as the kernel's own
//! wire-canonical bytes ([`varve_wire::schema_bytes`]), so what is
//! stored is exactly the value that will be published — no platform
//! mirror of the schema type. Edits go through [`edit_revision_draft`], which
//! composes the pure operations of [`crate::schema_edit`] with a load
//! and a version-checked store.

use toasty::Deferred;
use varve_schema::Schema;
use varve_wire::{ReadError, schema_bytes, schema_from_bytes};

use crate::organization::Organization;
use crate::schema_edit::EditError;

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
    #[belongs_to]
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

    /// The unpublished schema being edited; `None` when nothing is in
    /// progress. Deferred: loaded only by [`find_procedure_with_revision_draft`].
    pub revision_draft: Deferred<Option<RevisionDraft>>,

    /// Optimistic concurrency (toasty-managed): two editors saving the
    /// draft from the same loaded row cannot silently overwrite each
    /// other — the second save fails with a condition error and must
    /// reload.
    #[version]
    pub version: u64,
}

/// The draft of a procedure's next **revision** — named for what it
/// publishes as, since "draft" alone will also be a case-file state.
/// Holds the schema today; the surface draft joins it when the editor
/// grows a surface side, and both publish together as one revision.
#[derive(Debug, Clone, PartialEq, Eq, toasty::Embed)]
pub struct RevisionDraft {
    /// The schema under edit, wire-canonical.
    pub schema: SchemaBytes,

    /// The published revision this draft forks from (its id) — the
    /// publication's parent. `None` until the procedure has a first
    /// revision to fork from, which is every draft until the revision
    /// DAG lands.
    pub base: Option<String>,
}

/// A kernel [`Schema`] as its wire-canonical bytes (one `BYTEA`
/// column). Constructed from a `Schema` only, so the column never
/// holds anything [`schema_from_bytes`] would refuse — short of
/// corruption, which [`DraftError::Corrupt`] reports.
#[derive(Debug, Clone, PartialEq, Eq, toasty::Embed)]
pub struct SchemaBytes(Vec<u8>);

impl SchemaBytes {
    pub fn encode(schema: &Schema) -> Self {
        Self(schema_bytes(schema))
    }

    pub fn decode(&self) -> Result<Schema, ReadError> {
        schema_from_bytes(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    #[error(transparent)]
    Edit(#[from] EditError),
    /// The stored draft no longer decodes — never produced by this
    /// crate's writes; a database-level corruption to surface, not
    /// silently replace.
    #[error("stored draft is unreadable: {0}")]
    Corrupt(#[from] ReadError),
    /// Database failure, including the optimistic-concurrency conflict
    /// (`condition_failed`) when the row changed since it was loaded.
    #[error(transparent)]
    Db(#[from] toasty::Error),
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

/// Looks a procedure up by id.
pub async fn find_procedure(
    db: &mut toasty::Db,
    id: uuid::Uuid,
) -> toasty::Result<Option<Procedure>> {
    Procedure::filter_by_id(id).first().exec(db).await
}

/// Looks a procedure up by id with its draft loaded.
pub async fn find_procedure_with_revision_draft(
    db: &mut toasty::Db,
    id: uuid::Uuid,
) -> toasty::Result<Option<Procedure>> {
    Procedure::filter_by_id(id)
        .include(Procedure::fields().revision_draft())
        .first()
        .exec(db)
        .await
}

/// The draft schema of a procedure loaded by
/// [`find_procedure_with_revision_draft`]: `None` when no draft is in progress.
///
/// # Panics
///
/// If the draft was not loaded (the catalog lookups defer it).
pub fn draft_schema(procedure: &Procedure) -> Result<Option<Schema>, DraftError> {
    Ok(match procedure.revision_draft.get() {
        Some(draft) => Some(draft.schema.decode()?),
        None => None,
    })
}

/// Applies `edit` to the procedure's draft schema and stores the
/// result, returning the schema as stored. With no draft in progress
/// the edit starts one from the empty schema. The procedure must come
/// from [`find_procedure_with_revision_draft`]; on success it is updated in
/// place (draft, `version`, `updated_at`).
///
/// Atomic: `edit` errors (a rejected operation) store nothing, and a
/// concurrent change to the row since it was loaded fails the store
/// ([`DraftError::Db`], `condition_failed`) instead of overwriting.
pub async fn edit_draft(
    db: &mut toasty::Db,
    procedure: &mut Procedure,
    edit: impl FnOnce(&mut Schema) -> Result<(), EditError>,
) -> Result<Schema, DraftError> {
    let (mut schema, base) = match procedure.revision_draft.get() {
        Some(draft) => (draft.schema.decode()?, draft.base.clone()),
        None => (Schema::default(), None),
    };
    edit(&mut schema)?;
    procedure
        .update()
        .revision_draft(Some(RevisionDraft {
            schema: SchemaBytes::encode(&schema),
            base,
        }))
        .exec(db)
        .await?;
    Ok(schema)
}

/// Drops the procedure's draft, if any. Same loading and concurrency
/// contract as [`edit_revision_draft`].
pub async fn discard_draft(db: &mut toasty::Db, procedure: &mut Procedure) -> toasty::Result<()> {
    procedure.update().revision_draft(None).exec(db).await
}

/// The procedures `account_id` administers — those owned by any
/// organization the account is a member of — oldest first.
pub async fn list_account_procedures(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
) -> toasty::Result<Vec<Procedure>> {
    Procedure::filter(
        Procedure::fields().organization().memberships().any(
            crate::organization::OrganizationMembership::fields()
                .account_id()
                .eq(account_id),
        ),
    )
    .order_by(Procedure::fields().created_at().asc())
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
