//! The blob sidecar (§2.15, settled 2026-08-19): a **plain POSIX tar**
//! keyed by hash beside the JSONL stream — entries `sha256/<hex>` in
//! hash order with fixed header fields, plaintext, no compression, and
//! **exact-set complete** against the stream's `attachment` and
//! `snapshot` lines.
//!
//! The writer is deterministic byte for byte: the same blob set yields
//! the same archive (M3's byte stability, extended to the sidecar).
//! The reader is strict on what matters — well-formed hash names, the
//! exact described set, no duplicates, sizes as described, and every
//! entry self-verifying through `BlobStore::put` (§13.6: the store
//! computes the hash) — and tolerant of header metadata (mode, mtime,
//! owner) so a sidecar re-packed with standard tools still imports:
//! entries verify by content, so metadata proves nothing either way.
//!
//! Import is one pass and fail-safe by existing machinery (§2.15): put
//! the blobs, *then* adopt the stream; if adoption fails the blobs are
//! merely unreferenced and the §13.6 grace window collects them.

use std::collections::BTreeMap;
use std::time::SystemTime;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use varve_core::canonical::ContentHash;
use varve_files::{BlobStore, PutMeta};
use varve_wire::Stream;

use crate::BundleError;

const BLOCK: usize = 512;
/// Largest size the ustar 12-byte octal field carries; larger entries
/// get a pax `size` record (§2.15: "pax when an entry needs it").
const USTAR_MAX: u64 = 0o77777777777;

// ------------------------------------------------------------- writing

fn octal(field: &mut [u8], value: u64) {
    // Zero-padded octal, NUL-terminated: one spelling per value.
    let s = format!("{value:0>width$o}\0", width = field.len() - 1);
    field.copy_from_slice(s.as_bytes());
}

fn header(name: &str, size: u64, typeflag: u8) -> [u8; BLOCK] {
    let mut h = [0u8; BLOCK];
    h[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut h[100..108], 0o644); // mode
    octal(&mut h[108..116], 0); // uid
    octal(&mut h[116..124], 0); // gid
    octal(&mut h[124..136], size.min(USTAR_MAX));
    octal(&mut h[136..148], 0); // mtime: epoch — determinism
    h[156] = typeflag;
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    // uname/gname/devmajor/devminor stay NUL. Checksum last: computed
    // with the checksum field as spaces, rendered 6-digit octal + NUL
    // + space (the historical format every reader accepts).
    h[148..156].copy_from_slice(b"        ");
    let sum: u64 = h.iter().map(|b| u64::from(*b)).sum();
    let chk = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(chk.as_bytes());
    h
}

fn entry_name(hash: &ContentHash) -> String {
    // `sha256:<hex>` → `sha256/<hex>`: the algorithm tag as a directory.
    hash.to_string().replacen(':', "/", 1)
}

async fn write_padding(out: &mut (impl AsyncWrite + Unpin), size: u64) -> std::io::Result<()> {
    let rem = (size % BLOCK as u64) as usize;
    if rem != 0 {
        out.write_all(&[0u8; BLOCK][..BLOCK - rem]).await?;
    }
    Ok(())
}

/// Write the sidecar for `stream` to `out`: every blob its `attachment`
/// and `snapshot` lines describe, from `store`, as a deterministic tar.
/// Refuses a stream whose manifest says `referenced` (a sidecar it does
/// not declare), and a blob whose stored size differs from its
/// description (the entry header would lie).
pub async fn write_sidecar<S: BlobStore>(
    stream: &Stream,
    store: &S,
    mut out: impl AsyncWrite + Unpin,
) -> Result<(), BundleError> {
    if !stream.manifest.blobs_bundled {
        return Err(BundleError::NotBundled);
    }
    // `described_blobs` is already in hash order and duplicate-free
    // (enforced by the reader; the writer sorts).
    for (hash, byte_size, _content_type) in stream.described_blobs() {
        let name = entry_name(&hash);
        if byte_size > USTAR_MAX {
            // Pax extended header carrying the real size; the ustar
            // field is clamped. Record: "<len> size=<v>\n", len = the
            // record's own total length.
            let record = pax_record("size", &byte_size.to_string());
            out.write_all(&header(
                &format!("PaxHeaders/{}", &name[7..]),
                record.len() as u64,
                b'x',
            ))
            .await?;
            out.write_all(record.as_bytes()).await?;
            write_padding(&mut out, record.len() as u64).await?;
        }
        out.write_all(&header(&name, byte_size, b'0')).await?;
        let mut reader = store.get(&hash).await?;
        let copied = tokio::io::copy(&mut reader, &mut out).await?;
        if copied != byte_size {
            return Err(BundleError::SizeMismatch {
                hash,
                described: byte_size,
                actual: copied,
            });
        }
        write_padding(&mut out, byte_size).await?;
    }
    // End of archive: two zero blocks.
    out.write_all(&[0u8; 2 * BLOCK]).await?;
    out.flush().await?;
    Ok(())
}

fn pax_record(key: &str, value: &str) -> String {
    // "<len> <key>=<value>\n" where len counts the whole record
    // including itself — the POSIX fixed point.
    let base = key.len() + value.len() + 3; // ' ' '=' '\n'
    let mut len = base + base.to_string().len();
    if len.to_string().len() + base != len {
        len = base + len.to_string().len();
    }
    format!("{len} {key}={value}\n")
}

// ------------------------------------------------------------- reading

fn parse_octal(field: &[u8]) -> Result<u64, BundleError> {
    // Leading spaces tolerated: historic writers space-pad octal
    // fields where ours zero-pads — metadata proves nothing either
    // way (module docs), so read both spellings.
    let text = field
        .iter()
        .skip_while(|b| **b == b' ')
        .take_while(|b| **b != 0 && **b != b' ')
        .map(|b| *b as char)
        .collect::<String>();
    u64::from_str_radix(&text, 8)
        .map_err(|_| BundleError::MalformedArchive("bad octal field".into()))
}

fn parse_name(h: &[u8; BLOCK]) -> Result<String, BundleError> {
    let name = h[..100]
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as char)
        .collect::<String>();
    if name.is_empty() {
        return Err(BundleError::MalformedArchive("empty entry name".into()));
    }
    Ok(name)
}

async fn read_block(
    reader: &mut (impl AsyncRead + Unpin),
    block: &mut [u8; BLOCK],
) -> Result<(), BundleError> {
    reader
        .read_exact(block)
        .await
        .map_err(|_| BundleError::MalformedArchive("truncated archive".into()))?;
    Ok(())
}

/// Import the sidecar `tar` against `stream` (§2.15): store every entry
/// through `put` — the store computes the hash, so each entry verifies
/// against its own name — and require **exactly** the described set: an
/// entry the stream does not describe is refused (an undescribed blob
/// is a smuggling vector), a described blob missing from the archive is
/// refused. Sizes must match the descriptions; only regular-file
/// entries (plus their pax size headers) are accepted — no directories,
/// no links. `now` stamps `created_at` (timestamps are inputs, §2.13).
///
/// Returns the stored hashes in archive order. Call this *before*
/// adopting the stream: a failed adoption leaves the blobs unreferenced
/// for the sweep, never half-adopted records.
pub async fn import_sidecar<S: BlobStore>(
    stream: &Stream,
    store: &S,
    mut tar: impl AsyncRead + Send + Unpin,
    now: SystemTime,
) -> Result<Vec<ContentHash>, BundleError> {
    if !stream.manifest.blobs_bundled {
        return Err(BundleError::NotBundled);
    }
    let mut expected: BTreeMap<ContentHash, (u64, String)> = stream
        .described_blobs()
        .into_iter()
        .map(|(hash, size, content_type)| (hash, (size, content_type.to_string())))
        .collect();

    let mut stored = Vec::new();
    let mut block = [0u8; BLOCK];
    let mut pax_size: Option<u64> = None;
    loop {
        read_block(&mut tar, &mut block).await?;
        if block.iter().all(|b| *b == 0) {
            // End of archive (first zero block; the second is padding —
            // read if present, tolerate its absence).
            let _ = tar.read_exact(&mut block).await;
            break;
        }
        let name = parse_name(&block)?;
        let header_size = parse_octal(&block[124..136])?;
        match block[156] {
            b'x' => {
                // Pax extended header: only `size` records matter here.
                let mut data = vec![0u8; header_size as usize];
                tar.read_exact(&mut data)
                    .await
                    .map_err(|_| BundleError::MalformedArchive("truncated pax header".into()))?;
                let mut skip = [0u8; BLOCK];
                let rem = (header_size % BLOCK as u64) as usize;
                if rem != 0 {
                    tar.read_exact(&mut skip[..BLOCK - rem])
                        .await
                        .map_err(|_| BundleError::MalformedArchive("truncated padding".into()))?;
                }
                let text = String::from_utf8(data)
                    .map_err(|_| BundleError::MalformedArchive("pax header not UTF-8".into()))?;
                for record in text.split_terminator('\n') {
                    if let Some((_, kv)) = record.split_once(' ')
                        && let Some((key, value)) = kv.split_once('=')
                        && key == "size"
                    {
                        pax_size =
                            Some(value.parse().map_err(|_| {
                                BundleError::MalformedArchive("bad pax size".into())
                            })?);
                    }
                }
            }
            b'0' | 0 => {
                let size = pax_size.take().unwrap_or(header_size);
                let claimed: ContentHash = name
                    .replacen('/', ":", 1)
                    .parse()
                    .map_err(|_| BundleError::UnknownEntry(name.clone()))?;
                let Some((described_size, content_type)) = expected.remove(&claimed) else {
                    // Described-and-already-seen (duplicate) or never
                    // described: both refused.
                    return Err(if stored.contains(&claimed) {
                        BundleError::DuplicateEntry(claimed)
                    } else {
                        BundleError::UnknownEntry(name)
                    });
                };
                if size != described_size {
                    return Err(BundleError::SizeMismatch {
                        hash: claimed,
                        described: described_size,
                        actual: size,
                    });
                }
                let actual = store
                    .put(
                        PutMeta {
                            content_type,
                            created_at: now,
                        },
                        (&mut tar).take(size),
                    )
                    .await?;
                if actual != claimed {
                    return Err(BundleError::HashMismatch { claimed, actual });
                }
                let mut skip = [0u8; BLOCK];
                let rem = (size % BLOCK as u64) as usize;
                if rem != 0 {
                    tar.read_exact(&mut skip[..BLOCK - rem])
                        .await
                        .map_err(|_| BundleError::MalformedArchive("truncated padding".into()))?;
                }
                stored.push(claimed);
            }
            other => {
                return Err(BundleError::MalformedArchive(format!(
                    "unsupported entry type '{}'",
                    other as char
                )));
            }
        }
    }
    if !expected.is_empty() {
        return Err(BundleError::MissingEntries(expected.into_keys().collect()));
    }
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The POSIX pax record length is a fixed point: the length field
    /// counts the whole record, itself included.
    #[test]
    fn pax_record_length_is_its_own_fixed_point() {
        for value in ["0", "8589934592", &"9".repeat(90)] {
            let record = pax_record("size", value);
            let (len, _) = record.split_once(' ').unwrap();
            assert_eq!(len.parse::<usize>().unwrap(), record.len(), "{record}");
        }
    }

    #[test]
    fn octal_fields_round_trip() {
        for value in [0u64, 1, 0o644, USTAR_MAX] {
            let mut field = [0u8; 12];
            octal(&mut field, value);
            assert_eq!(parse_octal(&field).unwrap(), value);
        }
        let mut field = [0u8; 8];
        octal(&mut field, 0o644);
        assert_eq!(parse_octal(&field).unwrap(), 0o644);
        // Space-padded octal (historic writers): tolerated on read.
        let mut field = [0u8; 12];
        field[..7].copy_from_slice(b"  644 \0");
        assert_eq!(parse_octal(&field).unwrap(), 0o644);
        assert!(parse_octal(b"        \0   ").is_err());
    }

    /// A pax `size` record overrides the ustar field. The writer only
    /// emits pax above `USTAR_MAX`, so the read side is pinned with a
    /// hand-built archive: ustar size zeroed, pax size correct — the
    /// import must read through the pax path to find the bytes.
    #[tokio::test]
    async fn pax_size_records_drive_the_read() {
        use std::time::{Duration, UNIX_EPOCH};

        use varve_files::{MemoryKeyring, ObjectBlobStore};
        use varve_wire::{Intent, Line, Manifest, Mode};

        let content = b"hello";
        let now = UNIX_EPOCH + Duration::from_secs(1_766_000_000);
        let meta = || PutMeta {
            content_type: "text/plain".into(),
            created_at: now,
        };
        // A scratch store computes the content address.
        let scratch = ObjectBlobStore::memory(MemoryKeyring::default());
        let hash = scratch.put(meta(), &content[..]).await.unwrap();

        let stream = Stream {
            manifest: Manifest {
                format_version: varve_wire::FORMAT_VERSION,
                source_instance: "s".into(),
                mode: Mode::History,
                intent: Intent::CreateOnly,
                revisions: vec![],
                record_count: 0,
                blobs_bundled: true,
            },
            lines: vec![Line::Attachment {
                hash,
                byte_size: content.len() as u64,
                content_type: "text/plain".into(),
            }],
        };

        let name = entry_name(&hash);
        let record = pax_record("size", &content.len().to_string());
        let mut tar = Vec::new();
        tar.extend_from_slice(&header(
            &format!("PaxHeaders/{}", &name[7..]),
            record.len() as u64,
            b'x',
        ));
        tar.extend_from_slice(record.as_bytes());
        tar.extend_from_slice(&vec![0u8; BLOCK - record.len() % BLOCK]);
        tar.extend_from_slice(&header(&name, 0, b'0')); // ustar size lies
        tar.extend_from_slice(content);
        tar.extend_from_slice(&vec![0u8; BLOCK - content.len()]);
        tar.extend_from_slice(&[0u8; 2 * BLOCK]);

        let store = ObjectBlobStore::memory(MemoryKeyring::default());
        let stored = import_sidecar(&stream, &store, tar.as_slice(), now)
            .await
            .unwrap();
        assert_eq!(stored, vec![hash]);
        let mut out = Vec::new();
        AsyncReadExt::read_to_end(&mut store.get(&hash).await.unwrap(), &mut out)
            .await
            .unwrap();
        assert_eq!(out, content);
    }

    /// The header checksum is the historical spaces-then-sum form that
    /// every tar reader verifies.
    #[test]
    fn header_checksum_verifies() {
        let h = header("sha256/aa", 5, b'0');
        let mut copy = h;
        copy[148..156].copy_from_slice(b"        ");
        let sum: u64 = copy.iter().map(|b| u64::from(*b)).sum();
        assert_eq!(parse_octal(&h[148..156]).unwrap(), sum);
        assert_eq!(parse_name(&h).unwrap(), "sha256/aa");
    }
}
