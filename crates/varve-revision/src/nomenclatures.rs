//! Nomenclature publication (§2.12) with the **append-only rule** the
//! §5.5 join relies on ("same-nomenclature enum joins take the higher
//! version"): a new version's id set must contain the previous one's.
//! Removal is deprecation elsewhere (§2.11) — ids are never deleted.
//! Labels and extra fields may change freely (renames are the point).

use std::collections::{BTreeMap, BTreeSet};

use varve_core::{NomenclatureId, OptionId};
use varve_schema::{NomenclatureTable, OptionRow};

#[derive(Debug, Clone, Default)]
pub struct NomenclatureRegistry {
    /// Versions per nomenclature, version 1 first.
    versions: BTreeMap<NomenclatureId, Vec<Vec<OptionRow>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublishNomenclatureError {
    #[error(
        "nomenclature '{id}': version {version} removes ids {removed:?} — versions are append-only (§2.11)"
    )]
    RemovesIds {
        id: NomenclatureId,
        version: u32,
        removed: Vec<OptionId>,
    },
    #[error("nomenclature '{id}': duplicate option id '{option}'")]
    DuplicateId {
        id: NomenclatureId,
        option: OptionId,
    },
}

impl NomenclatureRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish the next version; returns the version number (1-based).
    pub fn publish(
        &mut self,
        id: NomenclatureId,
        rows: Vec<OptionRow>,
    ) -> Result<u32, PublishNomenclatureError> {
        let mut seen = BTreeSet::new();
        for row in &rows {
            if !seen.insert(row.id.clone()) {
                return Err(PublishNomenclatureError::DuplicateId {
                    id,
                    option: row.id.clone(),
                });
            }
        }
        let versions = self.versions.entry(id.clone()).or_default();
        if let Some(previous) = versions.last() {
            let removed: Vec<OptionId> = previous
                .iter()
                .map(|r| r.id.clone())
                .filter(|option| !seen.contains(option))
                .collect();
            if !removed.is_empty() {
                return Err(PublishNomenclatureError::RemovesIds {
                    id,
                    version: versions.len() as u32 + 1,
                    removed,
                });
            }
        }
        versions.push(rows);
        Ok(versions.len() as u32)
    }

    pub fn rows(&self, id: &NomenclatureId, version: u32) -> Option<&[OptionRow]> {
        self.versions
            .get(id)?
            .get(version.checked_sub(1)? as usize)
            .map(Vec::as_slice)
    }

    /// The lookup table consumers pass around (conformance, casts,
    /// logic): every version of every nomenclature, so a column bound to
    /// `N@v` resolves against exactly `v` (§2.12).
    pub fn table(&self) -> NomenclatureTable {
        let mut table = NomenclatureTable::new();
        for (id, versions) in &self.versions {
            for (i, rows) in versions.iter().enumerate() {
                table.insert(id.clone(), i as u32 + 1, rows.clone());
            }
        }
        table
    }
}
