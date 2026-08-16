//! Element-level change description for `many` cells (§2.4): the ops stay
//! cell-atomic, but value-internal identity lets a diff *report* say
//! "file 2 replaced" instead of "cell changed".

use std::collections::BTreeMap;

use crate::{CellValue, Scalar};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElementChanges {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Same identity, different content (e.g. a re-uploaded file keeping
    /// its element id).
    pub changed: Vec<String>,
    /// True when some element carries no identity — the report degrades
    /// to "cell changed" for those.
    pub unidentified: bool,
}

/// Describe how a `many` cell changed, element by element. Returns `None`
/// for `one` cells — there is nothing finer than the cell itself.
pub fn cell_delta(old: &CellValue, new: &CellValue) -> Option<ElementChanges> {
    let (CellValue::Many(old), CellValue::Many(new)) = (old, new) else {
        return None;
    };
    let mut changes = ElementChanges::default();
    let mut old_by_id: BTreeMap<&str, &Scalar> = BTreeMap::new();
    for scalar in old {
        match scalar.element_id() {
            Some(id) => {
                old_by_id.insert(id, scalar);
            }
            None => changes.unidentified = true,
        }
    }
    let mut seen = Vec::new();
    for scalar in new {
        match scalar.element_id() {
            Some(id) => {
                seen.push(id);
                match old_by_id.get(id) {
                    None => changes.added.push(id.to_string()),
                    Some(previous) if *previous != scalar => {
                        changes.changed.push(id.to_string());
                    }
                    Some(_) => {}
                }
            }
            None => changes.unidentified = true,
        }
    }
    for id in old_by_id.keys() {
        if !seen.contains(id) {
            changes.removed.push((*id).to_string());
        }
    }
    Some(changes)
}
