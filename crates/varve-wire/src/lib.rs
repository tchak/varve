//! Tier 4 (§7): tagged JSONL — the interchange format that is also the
//! change format (§5). One serializer: every emitted line is the JCS
//! canonical bytes of a `CanonicalValue`, so byte-stability (M3) is a
//! property of the canonical form, not of this crate.
//!
//! Two export modes, one op set (§5): **history** (`entry` lines —
//! lossless for the log, chain-preserving; the migration format) and
//! **snapshot** (`record`/`item` cell lines through a reading lens).
//! They never mix in one stream. Import modes are stream kinds, never
//! flags (§5). Not yet on the wire (§10 Q14): resolution instances,
//! checkpoints, payload snapshots and the bundled blob sidecar.
//!
//! The reader takes a whole buffer; a streaming reader over the same
//! line grammar is Tier 5 work.

#![forbid(unsafe_code)]

mod import;
mod line;
mod read;
mod write;

#[cfg(feature = "test-util")]
pub use import::test_salts;
pub use import::{
    ImportError, ImportOutcome, SnapshotImportRequest, adopt_history, import_snapshot,
};
pub use line::{Intent, ItemLine, Line, Manifest, Mode, RecordLine, SnapshotRecord};
pub use read::{ReadError, Stream, read_stream, snapshot_records};
pub use write::{WriteError, write_history, write_lines, write_snapshot};

/// Format version carried on line 1 (§5: fail fast on line 1).
pub const FORMAT_VERSION: u32 = 1;
