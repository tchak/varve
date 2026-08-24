//! The kernel tables, as `pub(crate)` models: publication events by
//! `(lineage, index)`, content-addressed revision objects, surfaces
//! by `(revision, surface)`. Append-only or upsert per the trait
//! contracts; no timestamps — kernel time is an input, and these
//! rows carry none (§2.13). The platform's own audit of *when* lives
//! in its event log, not here.

/// One publication event (§2.1): which object became current in a
/// lineage, following which revisions.
#[derive(Debug, toasty::Model)]
#[table = "publications"]
#[key(partition = lineage, local = index)]
pub(crate) struct PublicationRow {
    /// The lineage — the platform keys it by procedure UUID (P.4).
    pub(crate) lineage: String,
    /// 0-based event index: the append order and the conflict guard.
    pub(crate) index: u64,
    /// The published revision's content address.
    pub(crate) revision: String,
    /// Parent revision ids at this event.
    pub(crate) parents: Vec<String>,
}

/// One content-addressed revision object (§2.13), shared across
/// lineages: identical schemas converge on one row.
#[derive(Debug, toasty::Model)]
#[table = "revisions"]
pub(crate) struct RevisionRow {
    /// The content address (`varve_schema::revision_id`).
    #[key]
    pub(crate) revision: String,
    /// `varve_wire::schema_bytes` — the `revision`-line body (§5).
    pub(crate) schema: Vec<u8>,
}

/// One surface (§2.6), keyed by the revision it compiles against and
/// its own id.
#[derive(Debug, toasty::Model)]
#[table = "surfaces"]
#[key(partition = revision, local = surface)]
pub(crate) struct SurfaceRow {
    /// The revision this surface compiles against.
    pub(crate) revision: String,
    /// The surface id (the platform's fixed pair: `applicant`,
    /// `reviewer` — P.4).
    pub(crate) surface: String,
    /// JCS bytes of `varve_surface::canon::surface_canonical` — the
    /// `surface`-line body (§5).
    pub(crate) body: Vec<u8>,
}
