//! M0 expressibility harness (§8 of DESIGN.md).
//!
//! Reads the DN corpus (data.gouv.fr "descriptif des démarches publiées")
//! and attempts to express every procedure in the proposed kernel model.
//! The residue — what cannot be expressed, and why — is the deliverable.
//!
//! Desugarings applied per the M0 residue resolutions
//! (`corpus/M0-type-frequency.md`):
//! - `otherOption`  → enum + companion text column (visibility rule is
//!   surface-side, not modeled at M0)
//! - LinkedDropDownList → primary enum + one secondary enum per primary
//! - DossierLink → text holding an opaque id (record refs are on ice, §6)

#![forbid(unsafe_code)]
//! - Header/Explication → surface nodes, no column
//!
//! Resolver-fed champs become block-shaped groups + synthesized resolver
//! declarations. The mappings are institutional knowledge (DN fixes them
//! per champ type in code); they are NOT in the public dataset.

use std::collections::BTreeMap;

use serde_json::Value;
use varve_core::{ColumnId, GroupId, NomenclatureId, OptionId, ResolverId};
use varve_schema::{
    Arity, Cardinality, Column, DepthPolicy, Element, Group, Mapping, NomenclatureRef, OptionRow,
    ResolverDeclaration, ResultField, ScalarType, Schema, validate,
};

/// Fetched by `scripts/fetch-corpus.sh` (gitignored). Manifest-relative
/// so `cargo run -p m0` works from any directory.
const DEFAULT_CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/data/demarches.json"
);

#[derive(Default)]
struct Stats {
    procedures: u64,
    schemas_valid: u64,
    columns_emitted: u64,
    groups_emitted: u64,
    resolver_declarations: u64,
    surface_only_dropped: u64,
    desugar_other_option: u64,
    desugar_linked_dropdown: u64,
    desugar_dossier_link: u64,
    desugar_pre_rempli: u64,
    empty_enums: u64,
    linked_orphan_secondaries: u64,
    by_typename: BTreeMap<String, u64>,
    validation_errors: BTreeMap<String, u64>,
    /// (procedure number, typename, label) of anything inexpressible.
    residue: Vec<(i64, String, String)>,
}

/// Per-schema converter state: synthesizes stable-within-schema ids.
struct Converter<'a> {
    next_column: u32,
    next_group: u32,
    procedure: i64,
    stats: &'a mut Stats,
    resolvers: Vec<ResolverDeclaration>,
}

impl<'a> Converter<'a> {
    fn column_id(&mut self) -> ColumnId {
        self.next_column += 1;
        ColumnId::new(format!("c{}", self.next_column))
    }

    fn group_id(&mut self) -> GroupId {
        self.next_group += 1;
        GroupId::new(format!("g{}", self.next_group))
    }

    fn column(&mut self, label: &str, ty: ScalarType, arity: Arity) -> Element {
        self.stats.columns_emitted += 1;
        Element::Column(Column {
            id: self.column_id(),
            label: label.to_string(),
            ty,
            arity,
        })
    }

    fn inline_enum(&mut self, options: &[String]) -> ScalarType {
        if options.is_empty() {
            self.stats.empty_enums += 1;
        }
        let rows = options
            .iter()
            .enumerate()
            .map(|(i, label)| OptionRow {
                id: OptionId::new(format!("o{}", i + 1)),
                label: label.clone(),
                fields: Vec::new(),
            })
            .collect();
        ScalarType::Enum(NomenclatureRef::Inline(rows))
    }

    fn published_enum(&self, id: &str) -> ScalarType {
        ScalarType::Enum(NomenclatureRef::Published {
            id: NomenclatureId::new(id),
            version: 1,
        })
    }

    /// Block-shaped group + synthesized resolver declaration.
    /// `key` feeds the resolver; `fields` are the mapped target columns.
    fn resolver_block(
        &mut self,
        label: &str,
        resolver: &str,
        key: (&str, ScalarType),
        fields: &[(&str, ScalarType)],
        out: &mut Vec<Element>,
    ) {
        let key_id = self.column_id();
        self.stats.columns_emitted += 1;
        let mut children = vec![Element::Column(Column {
            id: key_id.clone(),
            label: key.0.to_string(),
            ty: key.1.clone(),
            arity: Arity::One,
        })];
        let mut mapping = Vec::new();
        let mut result_type = Vec::new();
        for (name, ty) in fields {
            let id = self.column_id();
            self.stats.columns_emitted += 1;
            children.push(Element::Column(Column {
                id: id.clone(),
                label: name.to_string(),
                ty: ty.clone(),
                arity: Arity::One,
            }));
            result_type.push(ResultField {
                name: name.to_string(),
                ty: ty.clone(),
            });
            mapping.push(Mapping {
                result_field: name.to_string(),
                target: id,
            });
        }
        let group_id = self.group_id();
        // §10 Q17: the declaration is anchored to the block-shaped group
        // it feeds — two SIRET blocks are two declarations.
        self.resolvers.push(ResolverDeclaration {
            id: ResolverId::new(resolver),
            version: 1,
            anchor: group_id.clone(),
            input: vec![(key_id, key.1)],
            result_type,
            mapping,
        });
        self.stats.resolver_declarations += 1;
        self.stats.groups_emitted += 1;
        out.push(Element::Group(Group {
            included_from: None,
            id: group_id,
            label: label.to_string(),
            cardinality: Cardinality::One,
            children,
        }));
    }

    fn convert(&mut self, champ: &Value, out: &mut Vec<Element>) {
        let typename = champ["__typename"].as_str().unwrap_or("<missing>");
        let label = champ["label"].as_str().unwrap_or("");
        *self
            .stats
            .by_typename
            .entry(typename.to_string())
            .or_default() += 1;

        let options = || -> Vec<String> {
            champ["options"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };

        match typename {
            // Surface nodes: prompts and help, no data (§2.6).
            "HeaderSectionChampDescriptor" | "ExplicationChampDescriptor" => {
                self.stats.surface_only_dropped += 1;
            }

            // Plain text; textarea/formatted presentation and phone/email/
            // iban format checks are surface concerns (§2.6).
            "TextChampDescriptor"
            | "TextareaChampDescriptor"
            | "FormattedChampDescriptor"
            | "PhoneChampDescriptor"
            | "EmailChampDescriptor"
            | "IbanChampDescriptor" => {
                out.push(self.column(label, ScalarType::Text, Arity::One));
            }

            "YesNoChampDescriptor" | "CheckboxChampDescriptor" => {
                out.push(self.column(label, ScalarType::Boolean, Arity::One));
            }

            "IntegerNumberChampDescriptor" => {
                out.push(self.column(label, ScalarType::Integer(None), Arity::One));
            }
            // "Number" is DN's legacy numeric champ: decimal-valued.
            "DecimalNumberChampDescriptor" | "NumberChampDescriptor" => {
                out.push(self.column(label, ScalarType::Decimal(None), Arity::One));
            }

            "DateChampDescriptor" => {
                out.push(self.column(label, ScalarType::Date, Arity::One));
            }
            "DatetimeChampDescriptor" => {
                out.push(self.column(label, ScalarType::Datetime, Arity::One));
            }

            "DropDownListChampDescriptor" => {
                let ty = self.inline_enum(&options());
                out.push(self.column(label, ty, Arity::One));
                if champ["otherOption"].as_bool() == Some(true) {
                    // Desugared pre-conditional-logic artifact: companion
                    // text column; the visibility rule is surface-side.
                    self.stats.desugar_other_option += 1;
                    let other = format!("{label} (autre)");
                    out.push(self.column(&other, ScalarType::Text, Arity::One));
                }
            }

            "MultipleDropDownListChampDescriptor" => {
                let ty = self.inline_enum(&options());
                out.push(self.column(label, ty, Arity::Many));
            }

            "LinkedDropDownListChampDescriptor" => {
                // Desugared pre-conditional-logic artifact: primaries are
                // "--wrapped--" entries; each primary gets its own
                // secondary enum under a (surface-side) visibility rule.
                self.stats.desugar_linked_dropdown += 1;
                let mut primaries: Vec<(String, Vec<String>)> = Vec::new();
                for entry in options() {
                    let trimmed = entry.trim();
                    if trimmed.len() > 4 && trimmed.starts_with("--") && trimmed.ends_with("--") {
                        let name = trimmed[2..trimmed.len() - 2].to_string();
                        primaries.push((name, Vec::new()));
                    } else if let Some((_, secondaries)) = primaries.last_mut() {
                        secondaries.push(entry);
                    } else {
                        self.stats.linked_orphan_secondaries += 1;
                    }
                }
                let primary_labels: Vec<String> =
                    primaries.iter().map(|(p, _)| p.clone()).collect();
                let ty = self.inline_enum(&primary_labels);
                out.push(self.column(label, ty, Arity::One));
                for (primary, secondaries) in &primaries {
                    if secondaries.is_empty() {
                        continue;
                    }
                    let ty = self.inline_enum(secondaries);
                    let sub = format!("{label} — {primary}");
                    out.push(self.column(&sub, ty, Arity::One));
                }
            }

            "CiviliteChampDescriptor" => {
                let ty = self.inline_enum(&["M.".into(), "Mme".into()]);
                out.push(self.column(label, ty, Arity::One));
            }

            "PieceJustificativeChampDescriptor" => {
                out.push(self.column(
                    label,
                    ScalarType::Attachment(Default::default()),
                    Arity::Many,
                ));
            }

            "CarteChampDescriptor" => {
                out.push(self.column(label, ScalarType::Geometry, Arity::Many));
            }

            // Static referential sources: nomenclature-backed enums (§2.12).
            "PaysChampDescriptor" => {
                let ty = self.published_enum("insee-pays");
                out.push(self.column(label, ty, Arity::One));
            }
            "RegionChampDescriptor" => {
                let ty = self.published_enum("insee-cog-regions");
                out.push(self.column(label, ty, Arity::One));
            }
            "DepartementChampDescriptor" => {
                let ty = self.published_enum("insee-cog-departements");
                out.push(self.column(label, ty, Arity::One));
            }
            "CommuneChampDescriptor" => {
                let ty = self.published_enum("insee-cog-communes");
                out.push(self.column(label, ty, Arity::One));
            }
            "EpciChampDescriptor" => {
                let ty = self.published_enum("insee-cog-epci");
                out.push(self.column(label, ty, Arity::One));
            }

            // API-fed composites: block + resolver declaration (§2.7).
            // Mappings synthesized from DN's fixed per-type semantics.
            "SiretChampDescriptor" => self.resolver_block(
                label,
                "insee-sirene",
                ("siret", ScalarType::Text),
                &[
                    ("raison_sociale", ScalarType::Text),
                    ("code_naf", ScalarType::Text),
                    ("adresse", ScalarType::Text),
                    ("date_creation", ScalarType::Date),
                ],
                out,
            ),
            "RNAChampDescriptor" => self.resolver_block(
                label,
                "rna",
                ("rna", ScalarType::Text),
                &[("titre", ScalarType::Text), ("adresse", ScalarType::Text)],
                out,
            ),
            "RNFChampDescriptor" => self.resolver_block(
                label,
                "rnf",
                ("rnf", ScalarType::Text),
                &[("titre", ScalarType::Text), ("adresse", ScalarType::Text)],
                out,
            ),
            "AnnuaireEducationChampDescriptor" => self.resolver_block(
                label,
                "annuaire-education",
                ("uai", ScalarType::Text),
                &[
                    ("nom_etablissement", ScalarType::Text),
                    ("adresse", ScalarType::Text),
                ],
                out,
            ),
            "AddressChampDescriptor" => self.resolver_block(
                label,
                "ban-address",
                ("recherche", ScalarType::Text),
                &[
                    ("adresse", ScalarType::Text),
                    ("code_postal", ScalarType::Text),
                    ("commune", ScalarType::Text),
                ],
                out,
            ),
            // DN's own generic pluggable connector: the mapping is
            // instance-configured, unknown to the public dataset.
            "ReferentielChampDescriptor" => self.resolver_block(
                label,
                "referentiel-generique",
                ("cle", ScalarType::Text),
                &[],
                out,
            ),
            // Dead one-off (Paris 2024) — kept expressible as a plain
            // declaration precisely because resolvers are not types.
            "COJOChampDescriptor" => self.resolver_block(
                label,
                "cojo",
                ("numero_accreditation", ScalarType::Text),
                &[("statut", ScalarType::Text)],
                out,
            ),

            // Read-only, filled only through external data (institutional
            // memory): a plain column — read-only-ness is a surface write
            // policy (§2.9), the filling is prefill (§2.7). No type.
            "PreRempliChampDescriptor" => {
                self.stats.desugar_pre_rempli += 1;
                out.push(self.column(label, ScalarType::Text, Arity::One));
            }

            // Record refs are on ice (§6): opaque id in a text column.
            "DossierLinkChampDescriptor" => {
                self.stats.desugar_dossier_link += 1;
                out.push(self.column(label, ScalarType::Text, Arity::One));
            }

            "RepetitionChampDescriptor" => {
                let mut children = Vec::new();
                if let Some(kids) = champ["champDescriptors"].as_array() {
                    for kid in kids {
                        self.convert(kid, &mut children);
                    }
                }
                self.stats.groups_emitted += 1;
                out.push(Element::Group(Group {
                    included_from: None,
                    id: self.group_id(),
                    label: label.to_string(),
                    cardinality: Cardinality::Many,
                    children,
                }));
            }

            // Anything else is residue: the reason M0 exists.
            other => {
                self.stats
                    .residue
                    .push((self.procedure, other.to_string(), label.to_string()));
            }
        }
    }
}

fn main() {
    // Usage: m0 [PATH] [--wire] — flags and the path in any order.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let wire_mode = args.iter().any(|a| a == "--wire");
    if let Some(unknown) = args.iter().find(|a| a.starts_with("--") && *a != "--wire") {
        eprintln!("unknown flag {unknown}\nusage: m0 [corpus.json] [--wire]");
        std::process::exit(2);
    }
    let path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| DEFAULT_CORPUS.to_string());
    eprintln!("reading {path}…");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read corpus at {path}: {e}\nfetch it with scripts/fetch-corpus.sh");
            std::process::exit(1);
        }
    };
    let procedures: Vec<Value> = match serde_json::from_slice(&bytes) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cannot parse corpus JSON: {e}");
            std::process::exit(1);
        }
    };
    drop(bytes);
    eprintln!("parsed {} procedures", procedures.len());

    let mut stats = Stats::default();
    let mut schemas: Vec<Schema> = Vec::new();
    for procedure in &procedures {
        let number = procedure["number"].as_i64().unwrap_or(-1);
        stats.procedures += 1;
        let mut converter = Converter {
            next_column: 0,
            next_group: 0,
            procedure: number,
            stats: &mut stats,
            resolvers: Vec::new(),
        };
        let mut root = Vec::new();
        if let Some(champs) = procedure["revision"]["champDescriptors"].as_array() {
            for champ in champs {
                converter.convert(champ, &mut root);
            }
        }
        let resolvers = std::mem::take(&mut converter.resolvers);
        let schema = Schema { root, resolvers };
        let errors = validate(&schema, DepthPolicy::default());
        if errors.is_empty() {
            stats.schemas_valid += 1;
        } else {
            for error in errors {
                let key = format!("{error}");
                *stats.validation_errors.entry(key).or_default() += 1;
            }
        }
        if wire_mode {
            schemas.push(schema);
        }
    }

    if wire_mode {
        wire_round_trip(&schemas);
        return;
    }

    println!("# M0 expressibility run\n");
    println!("procedures:            {:>9}", stats.procedures);
    println!("schemas valid:         {:>9}", stats.schemas_valid);
    println!("columns emitted:       {:>9}", stats.columns_emitted);
    println!("groups emitted:        {:>9}", stats.groups_emitted);
    println!("resolver declarations: {:>9}", stats.resolver_declarations);
    println!("surface-only dropped:  {:>9}", stats.surface_only_dropped);
    println!();
    println!("desugarings:");
    println!(
        "  otherOption → enum + text: {:>7}",
        stats.desugar_other_option
    );
    println!(
        "  linked dropdown → enums:   {:>7}",
        stats.desugar_linked_dropdown
    );
    println!(
        "  dossier link → text:       {:>7}",
        stats.desugar_dossier_link
    );
    println!(
        "  pre-rempli → surface r/o:  {:>7}",
        stats.desugar_pre_rempli
    );
    println!();
    println!("warnings:");
    println!("  empty enums:               {:>7}", stats.empty_enums);
    println!(
        "  linked orphan secondaries: {:>7}",
        stats.linked_orphan_secondaries
    );
    println!();
    if stats.validation_errors.is_empty() {
        println!("validation errors: none");
    } else {
        println!("validation errors:");
        for (error, count) in &stats.validation_errors {
            println!("  {count:>7} × {error}");
        }
    }
    println!();
    if stats.residue.is_empty() {
        println!("residue: NONE — every procedure is expressible");
    } else {
        println!("residue ({} champs):", stats.residue.len());
        for (procedure, typename, label) in &stats.residue {
            println!("  procedure {procedure}: {typename} ({label:?})");
        }
    }
}

/// M3 (§8): the corpus in and out, byte-stable. Every schema becomes a
/// `revision` line in one history-mode stream; the stream is read back,
/// re-emitted, and compared byte for byte; every schema is compared
/// structurally; and every revision id is recomputed from the decoded
/// schema and compared to the id on the wire.
fn wire_round_trip(schemas: &[Schema]) {
    use varve_schema::revision_id;
    use varve_wire::{Intent, Line, Manifest, Mode, read_stream, write_lines};

    let mut lines = Vec::with_capacity(schemas.len() + 1);
    let mut ids = Vec::with_capacity(schemas.len());
    let mut distinct = std::collections::BTreeSet::new();
    for schema in schemas {
        let id = revision_id(schema);
        distinct.insert(id.clone());
        ids.push(id.clone());
        lines.push(Line::Revision {
            id,
            schema: schema.clone(),
        });
    }
    lines.insert(
        0,
        Line::Header(Manifest {
            format_version: varve_wire::FORMAT_VERSION,
            source_instance: "dn-corpus".into(),
            mode: Mode::History,
            intent: Intent::CreateOnly,
            revisions: ids.clone(),
            record_count: 0,
            attachments_bundled: false,
        }),
    );

    let bytes = write_lines(&lines).expect("corpus schemas are JCS-representable");
    let stream = read_stream(&bytes).expect("corpus stream must read back");
    let again = write_lines(&stream.lines).expect("corpus schemas are JCS-representable");

    let mut schema_mismatches = 0u64;
    let mut id_mismatches = 0u64;
    let decoded: Vec<&Line> = stream.lines.iter().skip(1).collect();
    for (line, original) in decoded.iter().zip(schemas) {
        let Line::Revision { id, schema } = line else {
            panic!("expected a revision line");
        };
        if schema != original {
            schema_mismatches += 1;
        }
        if revision_id(schema) != *id {
            id_mismatches += 1;
        }
    }

    println!("# M3 corpus round-trip run\n");
    println!("schemas emitted:        {:>9}", schemas.len());
    println!("distinct revision ids:  {:>9}", distinct.len());
    println!("stream bytes:           {:>9}", bytes.len());
    println!("lines read back:        {:>9}", stream.lines.len());
    println!(
        "byte-stable re-emit:    {:>9}",
        if again == bytes { "YES" } else { "NO" }
    );
    println!("schema mismatches:      {:>9}", schema_mismatches);
    println!("revision id mismatches: {:>9}", id_mismatches);
    let ok = again == bytes
        && schema_mismatches == 0
        && id_mismatches == 0
        && decoded.len() == schemas.len();
    println!();
    println!(
        "M3: {}",
        if ok {
            "PASS — the corpus round-trips byte-stably"
        } else {
            "FAIL"
        }
    );
    if !ok {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use varve_schema::DepthPolicy;

    fn convert_all(champs: Value) -> (Schema, Stats) {
        let mut stats = Stats::default();
        let mut root = Vec::new();
        let resolvers = {
            let mut converter = Converter {
                next_column: 0,
                next_group: 0,
                procedure: 1,
                stats: &mut stats,
                resolvers: Vec::new(),
            };
            for champ in champs.as_array().unwrap() {
                converter.convert(champ, &mut root);
            }
            converter.resolvers
        };
        (Schema { root, resolvers }, stats)
    }

    fn champ(typename: &str) -> Value {
        json!({"__typename": typename, "label": typename, "required": false})
    }

    #[test]
    fn every_known_champ_type_converts_and_validates() {
        let mut champs: Vec<Value> = [
            "HeaderSectionChampDescriptor",
            "ExplicationChampDescriptor",
            "TextChampDescriptor",
            "TextareaChampDescriptor",
            "FormattedChampDescriptor",
            "PhoneChampDescriptor",
            "EmailChampDescriptor",
            "IbanChampDescriptor",
            "YesNoChampDescriptor",
            "CheckboxChampDescriptor",
            "IntegerNumberChampDescriptor",
            "DecimalNumberChampDescriptor",
            "NumberChampDescriptor",
            "DateChampDescriptor",
            "DatetimeChampDescriptor",
            "CiviliteChampDescriptor",
            "PieceJustificativeChampDescriptor",
            "CarteChampDescriptor",
            "PaysChampDescriptor",
            "RegionChampDescriptor",
            "DepartementChampDescriptor",
            "CommuneChampDescriptor",
            "EpciChampDescriptor",
            "SiretChampDescriptor",
            "RNAChampDescriptor",
            "RNFChampDescriptor",
            "AnnuaireEducationChampDescriptor",
            "AddressChampDescriptor",
            "ReferentielChampDescriptor",
            "COJOChampDescriptor",
            "PreRempliChampDescriptor",
            "DossierLinkChampDescriptor",
        ]
        .into_iter()
        .map(champ)
        .collect();
        champs.push(json!({
            "__typename": "DropDownListChampDescriptor",
            "label": "dd", "options": ["A", "B"], "otherOption": true
        }));
        champs.push(json!({
            "__typename": "MultipleDropDownListChampDescriptor",
            "label": "mdd", "options": ["A", "B"]
        }));
        champs.push(json!({
            "__typename": "LinkedDropDownListChampDescriptor",
            "label": "ldd", "options": ["--P--", "p1", "p2"]
        }));
        champs.push(json!({
            "__typename": "RepetitionChampDescriptor",
            "label": "rep",
            "champDescriptors": [
                {"__typename": "TextChampDescriptor", "label": "inner"},
                {"__typename": "PieceJustificativeChampDescriptor", "label": "files"}
            ]
        }));

        let (schema, stats) = convert_all(Value::Array(champs));
        assert!(stats.residue.is_empty(), "residue: {:?}", stats.residue);
        assert_eq!(validate(&schema, DepthPolicy::default()), vec![]);
        assert_eq!(stats.surface_only_dropped, 2);
        assert_eq!(stats.resolver_declarations, 7);
        assert_eq!(stats.desugar_other_option, 1);
        assert_eq!(stats.desugar_linked_dropdown, 1);
        assert_eq!(stats.desugar_dossier_link, 1);
        assert_eq!(stats.desugar_pre_rempli, 1);
        // 7 resolver blocks + 1 repetition.
        assert_eq!(stats.groups_emitted, 8);
    }

    #[test]
    fn linked_dropdown_desugars_per_primary() {
        let (schema, stats) = convert_all(json!([{
            "__typename": "LinkedDropDownListChampDescriptor",
            "label": "pcs",
            "options": ["--A--", "a1", "a2", "--B--", "b1", "--C--"]
        }]));
        // Primary enum + secondaries for A and B; C has none.
        assert_eq!(schema.root.len(), 3);
        assert_eq!(stats.linked_orphan_secondaries, 0);

        let (_, stats) = convert_all(json!([{
            "__typename": "LinkedDropDownListChampDescriptor",
            "label": "bad",
            "options": ["orphan", "--A--", "a1"]
        }]));
        assert_eq!(stats.linked_orphan_secondaries, 1);
    }

    #[test]
    fn other_option_adds_companion_text_column() {
        let (schema, _) = convert_all(json!([{
            "__typename": "DropDownListChampDescriptor",
            "label": "choix", "options": ["A"], "otherOption": true
        }]));
        assert_eq!(schema.root.len(), 2);
        let Element::Column(companion) = &schema.root[1] else {
            panic!("expected column");
        };
        assert_eq!(companion.ty, ScalarType::Text);
        assert_eq!(companion.label, "choix (autre)");
    }

    #[test]
    fn empty_dropdown_is_a_warning_not_an_error() {
        let (schema, stats) = convert_all(json!([{
            "__typename": "DropDownListChampDescriptor",
            "label": "vide", "options": [], "otherOption": false
        }]));
        assert_eq!(stats.empty_enums, 1);
        assert_eq!(validate(&schema, DepthPolicy::default()), vec![]);
    }

    #[test]
    fn unknown_typename_is_residue() {
        let (_, stats) = convert_all(json!([
            {"__typename": "HologramChampDescriptor", "label": "future"}
        ]));
        assert_eq!(stats.residue.len(), 1);
        assert_eq!(stats.residue[0].1, "HologramChampDescriptor");
    }
}
