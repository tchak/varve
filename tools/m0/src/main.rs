//! M0 expressibility harness (§8 of the handoff).
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
//! - Header/Explication → surface nodes, no column
//!
//! Resolver-fed champs become block-shaped groups + synthesized resolver
//! declarations. The mappings are institutional knowledge (DN fixes them
//! per champ type in code); they are NOT in the public dataset.

use std::collections::BTreeMap;

use serde_json::Value;
use varve_core::{ColumnId, GroupId, NomenclatureId, OptionId, ResolverId};
use varve_schema::{
    Arity, Cardinality, Column, DepthPolicy, Element, Group, Mapping,
    NomenclatureRef, OptionRow, ResolverDeclaration, ResultField, ScalarType,
    Schema, validate,
};

const DEFAULT_CORPUS: &str = concat!(
    "/private/tmp/claude-501/-Users-tchak-dev-github-tchak-strata/",
    "8ac7c870-a9ee-4f13-9575-d2de4b8d9c87/scratchpad/demarches.json"
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
        self.resolvers.push(ResolverDeclaration {
            id: ResolverId::new(resolver),
            version: 1,
            input: vec![(key_id, key.1)],
            result_type,
            mapping,
        });
        self.stats.resolver_declarations += 1;
        self.stats.groups_emitted += 1;
        out.push(Element::Group(Group {
            id: self.group_id(),
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
                out.push(self.column(label, ScalarType::Integer, Arity::One));
            }
            // "Number" is DN's legacy numeric champ: decimal-valued.
            "DecimalNumberChampDescriptor" | "NumberChampDescriptor" => {
                out.push(self.column(label, ScalarType::Decimal, Arity::One));
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
                    if trimmed.len() > 4
                        && trimmed.starts_with("--")
                        && trimmed.ends_with("--")
                    {
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
                out.push(self.column(label, ScalarType::Attachment, Arity::Many));
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
                    id: self.group_id(),
                    label: label.to_string(),
                    cardinality: Cardinality::Many,
                    children,
                }));
            }

            // Anything else is residue: the reason M0 exists.
            other => {
                self.stats.residue.push((
                    self.procedure,
                    other.to_string(),
                    label.to_string(),
                ));
            }
        }
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_CORPUS.to_string());
    eprintln!("reading {path}…");
    let bytes = std::fs::read(&path).expect("cannot read corpus file");
    let procedures: Vec<Value> =
        serde_json::from_slice(&bytes).expect("cannot parse corpus JSON");
    drop(bytes);
    eprintln!("parsed {} procedures", procedures.len());

    let mut stats = Stats::default();
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
        if let Some(champs) = procedure["revision"]["champDescriptors"].as_array()
        {
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
    println!("  otherOption → enum + text: {:>7}", stats.desugar_other_option);
    println!("  linked dropdown → enums:   {:>7}", stats.desugar_linked_dropdown);
    println!("  dossier link → text:       {:>7}", stats.desugar_dossier_link);
    println!("  pre-rempli → surface r/o:  {:>7}", stats.desugar_pre_rempli);
    println!();
    println!("warnings:");
    println!("  empty enums:               {:>7}", stats.empty_enums);
    println!("  linked orphan secondaries: {:>7}", stats.linked_orphan_secondaries);
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
