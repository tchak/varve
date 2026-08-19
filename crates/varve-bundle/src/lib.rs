//! Tier 5 (§7, §13.6): the **export bundle** — the exporter/importer
//! joining the halves the deterministic crates deliberately keep apart
//! (§13.6: "neither crate grows the other's half"; this crate is the
//! join).
//!
//! Two joins:
//!
//! - **Stream ⊕ sidecar** (§2.15): `varve-wire` describes blobs
//!   (`attachment`/`snapshot` lines, `Stream::described_blobs`),
//!   `varve-files` streams bytes; [`write_sidecar`]/[`import_sidecar`]
//!   assemble and take apart the plain hash-keyed tar between them,
//!   exact-set complete, every entry self-verifying through `put`.
//! - **Surface ⊕ envelope** (§5): `varve-surface` owns the body codec,
//!   `varve-wire` the line envelope; [`surface_line`]/[`surfaces`] and
//!   [`block_defaults_line`]/[`block_defaults`] convert between the
//!   typed objects and the opaque-body lines.
//!
//! Confidentiality is not this crate's business: a bundle travels
//! plaintext, and encrypting the pair (stream *and* sidecar) to a
//! receiving instance's recipient is a platform export option over age
//! (P.10) — outside, uniform, optional.

#![forbid(unsafe_code)]

mod sidecar;

pub use sidecar::{import_sidecar, write_sidecar};

use varve_core::canonical::ContentHash;
use varve_surface::{
    BlockDefaults, Surface, SurfaceDecodeError, block_defaults_canonical, block_defaults_from,
    surface_canonical, surface_from,
};
use varve_wire::{Line, Stream};

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// The stream's manifest says `referenced` — it declares no sidecar
    /// (§2.15: the manifest is authoritative, a sidecar it does not
    /// declare is refused, an export it declares needs one).
    #[error("the manifest declares blobs as referenced, not bundled")]
    NotBundled,
    /// An entry's size differs from the stream's description of it.
    #[error("blob {hash}: described as {described} bytes, entry has {actual}")]
    SizeMismatch {
        hash: ContentHash,
        described: u64,
        actual: u64,
    },
    /// The store's computed hash is not the entry's name: corrupt or
    /// substituted bytes (§13.6: claim verification is the contract).
    #[error("entry claims {claimed}, bytes hash to {actual}")]
    HashMismatch {
        claimed: ContentHash,
        actual: ContentHash,
    },
    /// An archive entry the stream does not describe — an undescribed
    /// blob is a smuggling vector, not a convenience (§2.15).
    #[error("archive entry '{0}' is not described by the stream")]
    UnknownEntry(String),
    #[error("archive carries blob {0} twice")]
    DuplicateEntry(ContentHash),
    /// Blobs the stream describes that the archive does not contain.
    #[error("archive is missing {} described blob(s)", .0.len())]
    MissingEntries(Vec<ContentHash>),
    #[error("malformed archive: {0}")]
    MalformedArchive(String),
    /// A `surface`/`block_defaults` body that does not decode — the
    /// wire carries bodies opaquely (§5); this crate is where they meet
    /// their codec.
    #[error(transparent)]
    Surface(#[from] SurfaceDecodeError),
    #[error(transparent)]
    Files(#[from] varve_files::FilesError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A surface as a wire line (§5): body from `varve-surface`'s codec,
/// envelope from the surface's own identity.
pub fn surface_line(surface: &Surface) -> Line {
    Line::Surface {
        id: surface.id.clone(),
        revision: surface.revision.clone(),
        body: surface_canonical(surface),
    }
}

/// Block defaults as a wire line (§5): the line's hash is the defaults'
/// content address — `hash_plain(body)`, which the wire reader verifies
/// without interpreting the body.
pub fn block_defaults_line(defaults: &BlockDefaults) -> Line {
    Line::BlockDefaults {
        block: defaults.block.id.clone(),
        version: defaults.block.version,
        hash: defaults.content_hash(),
        body: block_defaults_canonical(defaults),
    }
}

/// Decode every `surface` line of a read stream. The reader already
/// checked envelope/body agreement structurally; this is where the body
/// meets its codec. Validation against the carried revision
/// (`varve_surface::validate`) is the importer's next step — it needs
/// the schema, not the wire.
pub fn surfaces(stream: &Stream) -> Result<Vec<Surface>, BundleError> {
    stream
        .lines
        .iter()
        .filter_map(|line| match line {
            Line::Surface { body, .. } => Some(surface_from(body).map_err(BundleError::from)),
            _ => None,
        })
        .collect()
}

/// Decode every `block_defaults` line of a read stream. The reader
/// already verified each line's hash against its body.
pub fn block_defaults(stream: &Stream) -> Result<Vec<BlockDefaults>, BundleError> {
    stream
        .lines
        .iter()
        .filter_map(|line| match line {
            Line::BlockDefaults { body, .. } => {
                Some(block_defaults_from(body).map_err(BundleError::from))
            }
            _ => None,
        })
        .collect()
}
