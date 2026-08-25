//! The kernel tables, as `pub(crate)` models: publication events by
//! `(lineage, index)`, content-addressed revision objects, surfaces
//! by content hash (§2.13 decision 9). Append-only per the trait
//! contracts; no timestamps — kernel time is an input, and these
//! rows carry none (§2.13). The platform's own audit of *when* lives
//! in its event log, not here.

/// One publication event (§2.1, §2.13 decision 9): which object
/// became current in a lineage, with which surfaces, following which
/// publications.
#[derive(Debug, toasty::Model)]
#[table = "publications"]
#[key(partition = lineage, local = index)]
pub(crate) struct PublicationRow {
    /// The lineage — the platform keys it by procedure UUID (P.4).
    pub(crate) lineage: String,
    /// 0-based event index: the append order and the conflict guard.
    pub(crate) index: u64,
    /// The published revision's content address — also inside `body`;
    /// this column exists for the FK to `revisions`.
    pub(crate) revision: String,
    /// JCS bytes of `varve_revision::publication_canonical` — the
    /// event's full content (parents, surfaces): its id's preimage.
    pub(crate) body: Vec<u8>,
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

/// One surface (§2.6), content-addressed and immutable (§2.13
/// decision 9): keyed by `Surface::content_hash`, never replaced —
/// publications' surface maps are what name these rows.
#[derive(Debug, toasty::Model)]
#[table = "surfaces"]
pub(crate) struct SurfaceRow {
    /// The content address — `hash_plain` of `body`.
    #[key]
    pub(crate) hash: String,
    /// The revision this surface compiles against — also inside
    /// `body`; this column exists for the FK to `revisions`.
    pub(crate) revision: String,
    /// JCS bytes of `varve_surface::canon::surface_canonical` — the
    /// `surface`-line body (§5) and the hash's preimage.
    pub(crate) body: Vec<u8>,
}
