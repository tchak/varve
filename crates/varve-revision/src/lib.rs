//! Tier 3 (§7): the revision DAG, publication, nomenclature
//! publication (with the §2.11 append-only rule the cast table leans
//! on), block publication (§2.1), three-way schema merge, and aggregate
//! revision construction (§5.5).

#![forbid(unsafe_code)]

mod aggregate;
mod blocks;
mod merge;
mod nomenclatures;

pub use aggregate::{
    AggregateColumn, AggregatePolicy, AggregateReport, AggregateRevision, aggregate,
};
pub use blocks::{BlockRegistry, PublishBlockError};
pub use merge::{MergeConflict, merge};
pub use nomenclatures::{NomenclatureRegistry, PublishNomenclatureError};

use std::collections::BTreeMap;

use varve_core::canonical::{CanonicalValue, ContentHash, hash_plain};
use varve_core::{PublicationId, RevisionId, SurfaceId};
use varve_schema::{Schema, revision_id};

/// The revision DAG of one schema lineage (§2.1): **objects** —
/// immutable, content-addressed revisions — plus a **publication log**
/// of events. The object is identity; the log is history: publishing a
/// schema whose object already exists (a revert to an earlier revision)
/// adds no object but records the event and moves `latest`. The events
/// are themselves content-addressed (§2.13 decision 9): a publication
/// commits to its revision, its surface set, and its parent
/// publications. Records are never "on" a revision (§2.9) — entries are
/// authored against one, and lookups here serve reading lenses,
/// projection, and impact.
#[derive(Debug, Clone, Default)]
pub struct RevisionDag {
    revisions: BTreeMap<RevisionId, PublishedRevision>,
    /// Publication events, oldest first — the aggregate's input order.
    /// A revision id may recur (revert); a publication id cannot (its
    /// content includes its parents).
    log: Vec<(PublicationId, Publication)>,
}

#[derive(Debug, Clone)]
pub struct PublishedRevision {
    pub schema: Schema,
    /// Parent *revisions* at first publication — the object's place in
    /// the DAG (the revisions of the publishing event's parents).
    pub parents: Vec<RevisionId>,
}

/// One publication event (§2.13 decision 9): which object became
/// current, with which surfaces, following which publications. A
/// revert is an event whose object predates its parents. Identity is
/// [`Publication::id`] — the plain hash of the canonical form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub revision: RevisionId,
    pub parents: Vec<PublicationId>,
    /// The surface set published with the revision: surface id →
    /// content hash (`varve_surface::Surface::content_hash`'s regime,
    /// held here as the hash only — §7: nothing depends on
    /// `varve-surface`).
    pub surfaces: BTreeMap<SurfaceId, ContentHash>,
}

impl Publication {
    /// The publication's content address (§2.13 decision 9): plain
    /// hash of [`publication_canonical`].
    pub fn id(&self) -> PublicationId {
        let hash = hash_plain(&publication_canonical(self)).expect("publications carry no floats");
        PublicationId::new(hash.to_string())
    }
}

/// Canonical form of a publication — the preimage of
/// [`Publication::id`] and the shape stores persist (§2.13: canonical
/// shapes live in code with test vectors). Empty `parents` and
/// `surfaces` are omitted: one canonical form per state.
pub fn publication_canonical(p: &Publication) -> CanonicalValue {
    let mut map = BTreeMap::new();
    map.insert(
        "revision".to_string(),
        CanonicalValue::String(p.revision.to_string()),
    );
    if !p.parents.is_empty() {
        map.insert(
            "parents".to_string(),
            CanonicalValue::Array(
                p.parents
                    .iter()
                    .map(|id| CanonicalValue::String(id.to_string()))
                    .collect(),
            ),
        );
    }
    if !p.surfaces.is_empty() {
        map.insert(
            "surfaces".to_string(),
            CanonicalValue::Object(
                p.surfaces
                    .iter()
                    .map(|(id, hash)| (id.to_string(), CanonicalValue::String(hash.to_string())))
                    .collect(),
            ),
        );
    }
    CanonicalValue::Object(map)
}

/// Corruption in a stored publication body — surfaced, never repaired
/// (the store loader's charter, §13.2).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed publication data: {0}")]
pub struct PublicationDecodeError(pub String);

/// Decode of [`publication_canonical`]: strict and total — unknown
/// keys, wrong shapes, and non-canonical empties (a present-but-empty
/// `parents` or `surfaces`) all refuse.
pub fn publication_from(v: &CanonicalValue) -> Result<Publication, PublicationDecodeError> {
    let err = |m: &str| PublicationDecodeError(m.into());
    let map = match v {
        CanonicalValue::Object(m) => m,
        _ => return Err(err("publication must be an object")),
    };
    if let Some(extra) = map
        .keys()
        .find(|k| !["revision", "parents", "surfaces"].contains(&k.as_str()))
    {
        return Err(err(&format!("unexpected key '{extra}'")));
    }
    let revision = match map.get("revision") {
        Some(CanonicalValue::String(s)) => RevisionId::new(s),
        _ => return Err(err("'revision' must be a string")),
    };
    let parents: Vec<PublicationId> = match map.get("parents") {
        None => Vec::new(),
        Some(CanonicalValue::Array(items)) => {
            if items.is_empty() {
                return Err(err("an empty 'parents' must be omitted"));
            }
            items
                .iter()
                .map(|item| match item {
                    CanonicalValue::String(s) => Ok(PublicationId::new(s)),
                    _ => Err(err("parent ids must be strings")),
                })
                .collect::<Result<_, _>>()?
        }
        Some(_) => return Err(err("'parents' must be an array")),
    };
    let surfaces: BTreeMap<SurfaceId, ContentHash> = match map.get("surfaces") {
        None => BTreeMap::new(),
        Some(CanonicalValue::Object(entries)) => {
            if entries.is_empty() {
                return Err(err("an empty 'surfaces' must be omitted"));
            }
            entries
                .iter()
                .map(|(id, hash)| match hash {
                    CanonicalValue::String(s) => s
                        .parse::<ContentHash>()
                        .map(|h| (SurfaceId::new(id), h))
                        .map_err(|_| err("bad surface hash")),
                    _ => Err(err("surface hashes must be strings")),
                })
                .collect::<Result<_, _>>()?
        }
        Some(_) => return Err(err("'surfaces' must be an object")),
    };
    Ok(Publication {
        revision,
        parents,
        surfaces,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublishError {
    #[error("unknown parent publication '{0}'")]
    UnknownParent(PublicationId),
    /// §2.13 decision 9: same revision, same surface set as the head —
    /// an append-only history has no meaning for a node byte-identical
    /// to its parent.
    #[error("the publication is identical to the head")]
    IdenticalToHead,
}

impl RevisionDag {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a schema with its surface set: the revision id is the
    /// schema's canonical hash — identical schemas converge on
    /// identical ids on every instance (§2.13) — and the returned
    /// publication id is the event's own content address (decision 9).
    /// Publishing a schema whose revision already exists creates no new
    /// object but is still an event (a revert) — unless revision *and*
    /// surfaces equal the head's, which is a refused no-op.
    pub fn publish(
        &mut self,
        schema: Schema,
        surfaces: BTreeMap<SurfaceId, ContentHash>,
        parents: Vec<PublicationId>,
    ) -> Result<PublicationId, PublishError> {
        for parent in &parents {
            if !self.log.iter().any(|(id, _)| id == parent) {
                return Err(PublishError::UnknownParent(parent.clone()));
            }
        }
        let revision = revision_id(&schema);
        if let Some((_, head)) = self.log.last()
            && head.revision == revision
            && head.surfaces == surfaces
        {
            return Err(PublishError::IdenticalToHead);
        }
        let parent_revisions: Vec<RevisionId> = {
            let mut seen = std::collections::BTreeSet::new();
            parents
                .iter()
                .map(|p| self.publication(p).expect("checked above").revision.clone())
                .filter(|r| seen.insert(r.clone()))
                .collect()
        };
        self.revisions
            .entry(revision.clone())
            .or_insert_with(|| PublishedRevision {
                schema,
                parents: parent_revisions,
            });
        let publication = Publication {
            revision,
            parents,
            surfaces,
        };
        let id = publication.id();
        self.log.push((id.clone(), publication));
        Ok(id)
    }

    pub fn get(&self, id: &RevisionId) -> Option<&PublishedRevision> {
        self.revisions.get(id)
    }

    /// Point lookup of a publication event by its content address.
    pub fn publication(&self, id: &PublicationId) -> Option<&Publication> {
        self.log.iter().find(|(pid, _)| pid == id).map(|(_, p)| p)
    }

    /// The current head: the last publication (whose object may be an
    /// earlier revision, after a revert).
    pub fn head(&self) -> Option<(&PublicationId, &Publication)> {
        self.log.last().map(|(id, p)| (id, p))
    }

    /// The current publication id — the stale-fork check's anchor
    /// (§2.13 decision 9: publication ids, not revision ids, so two
    /// surface-only publications from one revision stay distinct).
    pub fn latest(&self) -> Option<&PublicationId> {
        self.log.last().map(|(id, _)| id)
    }

    /// The publication events, oldest first, each with its id.
    pub fn publications(&self) -> &[(PublicationId, Publication)] {
        &self.log
    }

    /// Oldest-first publication history — the §5.5 aggregate input. A
    /// revision id recurs when it was re-published; the aggregate is
    /// over objects, so callers that want distinct revisions dedup by
    /// id.
    pub fn history(&self) -> impl Iterator<Item = (&RevisionId, &Schema)> {
        self.log
            .iter()
            .map(|(_, p)| (&p.revision, &self.revisions[&p.revision].schema))
    }

    /// The §5.5 aggregate over this lineage's entire history: every
    /// distinct revision, in first-publication order — computed once per
    /// publication, cacheable by the caller.
    pub fn aggregate(
        &self,
        nomenclatures: &varve_schema::NomenclatureTable,
    ) -> Result<(AggregateRevision, AggregateReport), varve_schema::CastError> {
        let mut seen = std::collections::BTreeSet::new();
        let history: Vec<(RevisionId, &Schema)> = self
            .history()
            .filter(|(id, _)| seen.insert((*id).clone()))
            .map(|(id, schema)| (id.clone(), schema))
            .collect();
        aggregate(&history, nomenclatures)
    }
}
