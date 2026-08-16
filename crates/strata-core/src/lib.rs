//! Tier 0 (§7 of the handoff): identifiers, row paths, scalar primitives,
//! canonical serialization and content hashing. Depends on nothing.
//!
//! Deterministic by construction: no IO, no clock, no async.

#![forbid(unsafe_code)]

use std::fmt;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(
    /// Stable identity of a typed field (§2.1). Cells are addressed by
    /// `(column_id, row_path)`; stability across revisions is what makes
    /// cells revision-agnostic (§3).
    ColumnId
);
id_type!(
    /// Identity of an ordered container of columns (§2.1).
    GroupId
);
id_type!(
    /// Identity of a record — a long-lived case file (§2.9).
    RecordId
);
id_type!(
    /// Identity of one instance of a `many` group (§2.1).
    ItemId
);
id_type!(
    /// Content-address of an immutable published schema version (§2.1).
    RevisionId
);
id_type!(
    /// Identity of a published, reusable group definition (§2.1).
    BlockId
);
id_type!(
    /// Identity of a published nomenclature (§2.12). Inline nomenclatures
    /// have no identity — they version with their containing revision.
    NomenclatureId
);
id_type!(
    /// Identity of a resolver declaration (§2.7).
    ResolverId
);
id_type!(
    /// Identity of an enum option within a nomenclature (§2.11). Cells
    /// store option ids; labels live in the revision.
    OptionId
);

/// One segment of a row path: which item of which `many` group.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PathSeg {
    pub group: GroupId,
    pub item: ItemId,
}

/// A possibly-empty sequence of segments (§2.3).
///
/// Storage and addressing work at depth N; `depth <= 1` is a schema
/// validation *policy* (`strata-schema`), deliberately never encoded in
/// this type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct RowPath(Vec<PathSeg>);

impl RowPath {
    /// The empty path: root scope.
    pub fn root() -> Self {
        Self::default()
    }

    pub fn depth(&self) -> usize {
        self.0.len()
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn child(&self, seg: PathSeg) -> Self {
        let mut segs = self.0.clone();
        segs.push(seg);
        Self(segs)
    }

    pub fn segments(&self) -> &[PathSeg] {
        &self.0
    }
}

pub mod canonical {
    //! Canonical serialization and content hashing — deliberately empty.
    //!
    //! The canonical encoding is constrained by §2.10 before it exists:
    //! hashes must commit to salted or encrypted value encodings, never
    //! plaintext (erasure tolerance). Not needed by the M0 expressibility
    //! harness; must be designed before anything record-shaped is hashed.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_path_depth() {
        let root = RowPath::root();
        assert!(root.is_root());
        assert_eq!(root.depth(), 0);

        let item = root.child(PathSeg {
            group: GroupId::new("g1"),
            item: ItemId::new("i1"),
        });
        assert_eq!(item.depth(), 1);
        assert!(!item.is_root());
        assert!(root.is_root(), "child must not mutate the parent");
    }
}
