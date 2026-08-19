//! The §5 seam, exercised end to end: a surface encoded by this crate's
//! codec rides a `surface` wire line as an opaque body and comes back
//! identical, byte-stably; block defaults likewise, with the line's
//! hash being the defaults' content address. `varve-wire` is a
//! dev-dependency here — the reverse edge stays absent (§7: nothing
//! depends on `varve-surface`), which is exactly what the opaque-body
//! decision preserves.

use varve_core::{BlockId, ColumnId, GroupId, RevisionId, SurfaceId};
use varve_logic::{Atom, ColumnRef, Expr};
use varve_schema::BlockRef;
use varve_surface::{
    BlockDefaults, ColumnNode, GroupNode, Ineligibility, Node, Section, Surface, WritePolicy,
    block_defaults_canonical, block_defaults_from, surface_canonical, surface_from,
};
use varve_wire::{Intent, Line, Manifest, Mode, read_stream, write_lines};

fn sample_surface(revision: &RevisionId) -> Surface {
    let is_filled = |c: &str| {
        Expr::Atom(Atom::IsFilled {
            source: ColumnRef {
                column: ColumnId::new(c),
                field: None,
            },
        })
    };
    Surface {
        id: SurfaceId::new("depot"),
        revision: revision.clone(),
        nodes: vec![
            Node::Section(Section {
                title: "Identité".into(),
                help: None,
                visibility: None,
                children: vec![Node::Column(ColumnNode {
                    column: ColumnId::new("name"),
                    prompt: Some("Votre nom".into()),
                    help: None,
                    visibility: None,
                    required: Some(Expr::And(vec![])),
                    write: WritePolicy {
                        writable: true,
                        override_derived: false,
                    },
                    format: None,
                })],
            }),
            Node::Group(GroupNode {
                group: GroupId::new("contacts"),
                prompt: None,
                visibility: Some(is_filled("name")),
                children: vec![],
            }),
        ],
        ineligibility: Some(Ineligibility {
            rule: is_filled("name"),
            message: "non éligible".into(),
        }),
    }
}

fn defaults() -> BlockDefaults {
    BlockDefaults {
        block: BlockRef {
            id: BlockId::new("rib"),
            version: 2,
        },
        node: GroupNode {
            group: GroupId::new("rib"),
            prompt: Some("Coordonnées bancaires".into()),
            visibility: None,
            children: vec![],
        },
    }
}

#[test]
fn surfaces_ride_the_wire_as_opaque_bodies() {
    let schema = varve_schema::Schema::default();
    let revision = varve_schema::revision_id(&schema);
    let surface = sample_surface(&revision);
    let d = defaults();
    let lines = vec![
        Line::Header(Manifest {
            format_version: varve_wire::FORMAT_VERSION,
            source_instance: "seam".into(),
            mode: Mode::Snapshot,
            intent: Intent::Upsert,
            revisions: vec![revision.clone()],
            record_count: 0,
            blobs_bundled: false,
        }),
        Line::Revision {
            id: revision.clone(),
            schema,
        },
        Line::Surface {
            id: surface.id.clone(),
            revision: revision.clone(),
            body: surface_canonical(&surface),
        },
        Line::BlockDefaults {
            block: d.block.id.clone(),
            version: d.block.version,
            hash: d.content_hash(),
            body: block_defaults_canonical(&d),
        },
    ];
    let bytes = write_lines(&lines).unwrap();
    let stream = read_stream(&bytes).unwrap();
    assert_eq!(write_lines(&stream.lines).unwrap(), bytes, "byte-stable");

    // The Tier 5 importer's half: decode the bodies with this crate.
    let mut surfaces = Vec::new();
    let mut all_defaults = Vec::new();
    for line in &stream.lines {
        match line {
            Line::Surface { id, revision, body } => {
                let decoded = surface_from(body).unwrap();
                assert_eq!((&decoded.id, &decoded.revision), (id, revision));
                surfaces.push(decoded);
            }
            Line::BlockDefaults { hash, body, .. } => {
                let decoded = block_defaults_from(body).unwrap();
                assert_eq!(decoded.content_hash(), *hash);
                all_defaults.push(decoded);
            }
            _ => {}
        }
    }
    assert_eq!(surfaces, vec![surface]);
    assert_eq!(all_defaults, vec![d]);
}
