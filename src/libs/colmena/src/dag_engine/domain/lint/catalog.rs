//! The node catalog: the set of configuration fields each `node_type` accepts.
//!
//! The catalog is the linter's source of truth for answering "is this field
//! real, or did the author invent it?". It is parsed from
//! `docs/node_configurations.json`, embedded at compile time.
//!
//! # Why embed the doc instead of owning a copy
//!
//! `docs/node_configurations.json` is already the canonical reference read by
//! humans and by agents. Embedding that exact file — rather than maintaining a
//! second copy inside the crate — means the document people read and the data
//! the linter enforces can never disagree, because they are the same bytes.
//! The crate already sets this precedent in
//! [`crate::dag_engine::log_policy`], which embeds a developer guide to assert
//! that documented log targets stay in sync with the constants.
//!
//! Caveat: `docs/` lives outside this crate's package root, so `cargo package`
//! would not be able to resolve the include. That is not a constraint today —
//! the crate is consumed as a git dependency, never from a registry — but it is
//! the reason to reach for a generated, in-crate artifact if that ever changes.
//!
//! # What the catalog does not know
//!
//! Coverage is per `node_type`. A type absent from the catalog is *unknown*,
//! not *invalid*: the linter must report that it cannot check that node rather
//! than flag every one of its fields as invented. [`NodeCatalog::entry`]
//! returns `None` for those, and callers are expected to treat `None` as
//! "no coverage", never as "no fields allowed".

use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// The raw `docs/node_configurations.json` bytes, embedded at compile time.
const CATALOG_JSON: &str = include_str!("../../../../../../../docs/node_configurations.json");

/// Whether a documented field has to be set.
///
/// Most entries are a plain boolean, but the catalog also expresses
/// *conditional* requirement in prose or as a list of dependent keys — e.g.
/// `router.schema` is required only in the node's mode B, and
/// `llm_call.crdt_documents` documents `["artifact_id"]` as the sub-key its
/// entries need. Those cannot be reduced to a boolean without lying in one
/// direction or the other.
///
/// Modelling them explicitly keeps the type a faithful mirror of the document
/// and, more importantly, lets the linter stay silent about them: a condition
/// it cannot evaluate must not become a "missing required field" error on a
/// perfectly valid graph.
#[derive(Debug, Clone, PartialEq)]
pub enum Requiredness {
    /// Always required.
    Always,
    /// Never required.
    Never,
    /// Required only under a condition the catalog states but does not
    /// formalise. Carries the raw value so tooling can surface it verbatim.
    Conditional(Value),
}

impl Requiredness {
    /// Whether absence of this field is unconditionally an error.
    ///
    /// [`Requiredness::Conditional`] answers `false`: the condition is not
    /// machine-checkable, and guessing produces false positives.
    pub fn is_unconditional(&self) -> bool {
        matches!(self, Requiredness::Always)
    }
}

impl<'de> Deserialize<'de> for Requiredness {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match Value::deserialize(d)? {
            Value::Bool(true) => Requiredness::Always,
            Value::Bool(false) => Requiredness::Never,
            other => Requiredness::Conditional(other),
        })
    }
}

/// What the catalog knows about one configuration field of one node type.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FieldSpec {
    /// The documented type, verbatim from the catalog.
    ///
    /// Not a JSON Schema type: the catalog also uses `"any"` (the field accepts
    /// anything) and unions such as `"string|array"`. Interpretation is left to
    /// the linter so this type stays a faithful mirror of the document.
    #[serde(rename = "type")]
    pub field_type: String,

    /// Whether the field must be present.
    ///
    /// Note this is *not* the same as "must appear in the node's `config`
    /// object": several nodes resolve a required field from an incoming edge
    /// instead. Callers must account for that before reporting an error.
    pub required: Requiredness,

    /// The closed set of accepted values, when the field has one.
    #[serde(default)]
    pub valid_values: Option<Vec<Value>>,

    /// Set when the field is engine-populated and not author-settable.
    #[serde(default)]
    pub read_only: bool,
}

/// What the catalog knows about one `node_type`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NodeCatalogEntry {
    /// The configuration fields this node type accepts, keyed by field name.
    #[serde(default)]
    pub config_fields: BTreeMap<String, FieldSpec>,

    /// Input keys the engine reserves for itself on this node type.
    ///
    /// Author-supplied config must not collide with these; they are documented
    /// only for the handful of nodes that have them (today, `http_request`).
    #[serde(default)]
    pub reserved_input_keys: BTreeSet<String>,
}

impl FieldSpec {
    /// A field of `field_type`, not required, no closed value set, author-settable.
    ///
    /// Chain [`Self::required`], [`Self::valid_values`] and [`Self::read_only`]
    /// to add the rest. This is how a node declares its config in code (phase 2
    /// of the linter): a node's [`config_schema`] is checked field-for-field
    /// against the catalog, so the machine facts can no longer drift apart.
    ///
    /// [`config_schema`]: crate::dag_engine::domain::node::ExecutableNode::config_schema
    pub fn of_type(field_type: impl Into<String>) -> Self {
        FieldSpec {
            field_type: field_type.into(),
            required: Requiredness::Never,
            valid_values: None,
            read_only: false,
        }
    }

    /// Mark the field as unconditionally required.
    pub fn required(mut self) -> Self {
        self.required = Requiredness::Always;
        self
    }

    /// Mark the field as engine-populated and not author-settable.
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Restrict the field to a closed set of accepted values.
    pub fn valid_values(mut self, values: impl IntoIterator<Item = Value>) -> Self {
        self.valid_values = Some(values.into_iter().collect());
        self
    }
}

impl NodeCatalogEntry {
    /// An entry that accepts no configuration fields at all.
    ///
    /// A node returning this from `config_schema` is asserting it reads nothing
    /// from `config` — which the drift test then verifies against the catalog.
    pub fn no_config() -> Self {
        NodeCatalogEntry {
            config_fields: BTreeMap::new(),
            reserved_input_keys: BTreeSet::new(),
        }
    }

    /// Add a documented field, builder-style.
    pub fn with_field(mut self, name: impl Into<String>, spec: FieldSpec) -> Self {
        self.config_fields.insert(name.into(), spec);
        self
    }

    /// Whether `field` is a documented configuration field of this node type.
    pub fn knows_field(&self, field: &str) -> bool {
        self.config_fields.contains_key(field)
    }

    /// Whether this node type accepts any field name at all.
    ///
    /// Some nodes treat their whole `config` as free-form data rather than a
    /// set of settings — `mock_input` returns `config.clone()` as its output,
    /// and `input` emits its config as the payload for downstream nodes. For
    /// those, no key can be "invented", and the catalog says so with a
    /// placeholder key written in angle brackets, e.g. `<any_key>`.
    pub fn accepts_any_field(&self) -> bool {
        self.config_fields.keys().any(|k| is_placeholder_key(k))
    }

    /// The concrete documented field names — the candidate set a "did you mean"
    /// suggestion is drawn from. Placeholder keys are excluded: suggesting
    /// `<any_key>` would be nonsense.
    pub fn field_names(&self) -> impl Iterator<Item = &str> {
        self.config_fields
            .keys()
            .filter(|k| !is_placeholder_key(k))
            .map(String::as_str)
    }
}

/// Whether a catalog key stands for "a name the author chooses" rather than a
/// literal field, e.g. `<any_key>`.
pub fn is_placeholder_key(key: &str) -> bool {
    key.starts_with('<') && key.ends_with('>')
}

/// The parsed catalog.
#[derive(Debug, Clone)]
pub struct NodeCatalog {
    node_types: BTreeMap<String, NodeCatalogEntry>,
    node_level_properties: BTreeSet<String>,
    declared_node_types: BTreeSet<String>,
    common_config_fields: BTreeMap<String, FieldSpec>,
}

/// Mirrors only the parts of the catalog document this module consumes.
///
/// Deliberately tolerant of unknown keys: the document carries a good deal of
/// prose and per-node ad-hoc keys that are meaningful to human readers and
/// irrelevant here. Tightening this to `deny_unknown_fields` would make every
/// documentation improvement a compile error.
#[derive(Debug, Deserialize)]
struct RawCatalog {
    node_types: BTreeMap<String, NodeCatalogEntry>,
    common_node_properties: BTreeMap<String, RawCommonProperty>,
    #[serde(default)]
    common_config_fields: RawCommonConfigFields,
}

#[derive(Debug, Default, Deserialize)]
struct RawCommonConfigFields {
    #[serde(default)]
    fields: BTreeMap<String, FieldSpec>,
}

#[derive(Debug, Deserialize)]
struct RawCommonProperty {
    #[serde(default)]
    valid_values: Option<Vec<String>>,
}

impl NodeCatalog {
    /// The catalog embedded in this build, parsed once.
    ///
    /// # Panics
    ///
    /// Panics if the embedded document does not parse. That is deliberate: the
    /// document ships inside the binary, so a parse failure is a build-time
    /// defect present on every run, not a runtime input error a caller could
    /// handle. The unit tests in this module fail first.
    pub fn embedded() -> &'static NodeCatalog {
        static CATALOG: OnceLock<NodeCatalog> = OnceLock::new();
        CATALOG.get_or_init(|| {
            NodeCatalog::parse(CATALOG_JSON)
                .expect("embedded docs/node_configurations.json must parse as a node catalog")
        })
    }

    /// Parses a catalog document.
    pub fn parse(json: &str) -> Result<NodeCatalog, serde_json::Error> {
        let raw: RawCatalog = serde_json::from_str(json)?;

        // `common_node_properties` describes the node object itself (`type`,
        // `config`, `trigger_on`, …), not the contents of `config`. `config` is
        // excluded because it is the container being linted, not a sibling key.
        let node_level_properties = raw
            .common_node_properties
            .keys()
            .filter(|k| k.as_str() != "config")
            .cloned()
            .collect();

        let declared_node_types = raw
            .common_node_properties
            .get("type")
            .and_then(|p| p.valid_values.clone())
            .unwrap_or_default()
            .into_iter()
            .collect();

        Ok(NodeCatalog {
            node_types: raw.node_types,
            node_level_properties,
            declared_node_types,
            common_config_fields: raw.common_config_fields.fields,
        })
    }

    /// The catalog entry for `node_type`, or `None` when the type has no
    /// coverage.
    ///
    /// `None` means "cannot check", never "nothing is allowed".
    pub fn entry(&self, node_type: &str) -> Option<&NodeCatalogEntry> {
        self.node_types.get(node_type)
    }

    /// The node types the catalog documents in full.
    pub fn covered_node_types(&self) -> impl Iterator<Item = &str> {
        self.node_types.keys().map(String::as_str)
    }

    /// The node types the catalog *claims* exist, via
    /// `common_node_properties.type.valid_values`.
    ///
    /// Kept separate from [`Self::covered_node_types`] because the two have
    /// historically disagreed; see the consistency test below.
    pub fn declared_node_types(&self) -> impl Iterator<Item = &str> {
        self.declared_node_types.iter().map(String::as_str)
    }

    /// Keys allowed on the node object itself, alongside `config`.
    pub fn node_level_properties(&self) -> impl Iterator<Item = &str> {
        self.node_level_properties.iter().map(String::as_str)
    }

    /// Config keys the *engine* reads from any node, whatever its type.
    ///
    /// These belong to no node implementation, so they appear in no
    /// `config_fields`; treating them as unknown would report a real, working
    /// setting as invented. `include_extra_info` is the case that motivated
    /// this: `DagRunUseCase` reads it off every node when assembling the final
    /// output, and 24 example graphs in this repo set it.
    pub fn common_config_field(&self, field: &str) -> Option<&FieldSpec> {
        self.common_config_fields.get(field)
    }

    /// The names of those engine-wide config keys.
    pub fn common_config_field_names(&self) -> impl Iterator<Item = &str> {
        self.common_config_fields.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_parses() {
        let catalog = NodeCatalog::embedded();
        assert!(
            catalog.covered_node_types().count() > 30,
            "catalog should cover the engine's node types; got {}",
            catalog.covered_node_types().count()
        );
    }

    #[test]
    fn node_level_properties_exclude_the_config_container() {
        let catalog = NodeCatalog::embedded();
        let props: Vec<_> = catalog.node_level_properties().collect();
        assert!(props.contains(&"type"), "got {props:?}");
        assert!(props.contains(&"trigger_on"), "got {props:?}");
        assert!(
            !props.contains(&"config"),
            "`config` is the container being linted, not a sibling key; got {props:?}"
        );
    }

    /// The catalog declares its node types in two places. They drifted apart
    /// once already — five types were listed as valid but had no entry — and
    /// nothing detected it. A linter built on a catalog that disagrees with
    /// itself reports invented fields on real nodes, so this is checked.
    #[test]
    fn declared_node_types_all_have_an_entry() {
        let catalog = NodeCatalog::embedded();
        let covered: BTreeSet<&str> = catalog.covered_node_types().collect();
        let missing: Vec<&str> = catalog
            .declared_node_types()
            .filter(|t| !covered.contains(t))
            .collect();
        assert!(
            missing.is_empty(),
            "these node types are listed in common_node_properties.type.valid_values \
             but have no entry in node_types: {missing:?}. \
             Every declared type needs an entry, otherwise the linter cannot check it."
        );
    }

    #[test]
    fn every_entry_is_a_declared_node_type() {
        let catalog = NodeCatalog::embedded();
        let declared: BTreeSet<&str> = catalog.declared_node_types().collect();
        let undeclared: Vec<&str> = catalog
            .covered_node_types()
            .filter(|t| !declared.contains(t))
            .collect();
        assert!(
            undeclared.is_empty(),
            "these node types have an entry but are absent from \
             common_node_properties.type.valid_values: {undeclared:?}"
        );
    }

    #[test]
    fn a_known_entry_exposes_its_fields() {
        let catalog = NodeCatalog::embedded();
        let entry = catalog.entry("llm_call").expect("llm_call is documented");
        assert!(entry.knows_field("model"));
        assert!(entry.knows_field("provider"));
        assert!(
            !entry.knows_field("modle"),
            "typo must not be a known field"
        );
    }

    #[test]
    fn an_uncovered_type_reports_no_coverage_rather_than_no_fields() {
        let catalog = NodeCatalog::embedded();
        assert!(catalog.entry("definitely_not_a_node_type").is_none());
    }

    /// The catalog states conditional requirement in prose. A
    /// conditional field must never read as unconditionally required, or the
    /// linter reports a missing field on a graph that is correct.
    #[test]
    fn conditional_requirement_is_not_unconditional() {
        let catalog = NodeCatalog::embedded();

        let router = catalog.entry("router").expect("router is documented");
        let schema = &router.config_fields["schema"];
        assert!(
            matches!(schema.required, Requiredness::Conditional(_)),
            "router.schema is documented as required only in mode B; got {:?}",
            schema.required
        );
        assert!(!schema.required.is_unconditional());

        // A plain boolean still reads the obvious way.
        let llm_call = catalog.entry("llm_call").expect("llm_call is documented");
        assert!(llm_call.config_fields["provider"]
            .required
            .is_unconditional());
        assert!(!llm_call.config_fields["temperature"]
            .required
            .is_unconditional());
    }

    /// Nodes that emit their own config as data accept any key by definition;
    /// the catalog marks them with an angle-bracket placeholder.
    #[test]
    fn open_config_node_types_are_recognised() {
        let catalog = NodeCatalog::embedded();
        for open in ["mock_input", "input"] {
            let entry = catalog.entry(open).expect("documented");
            assert!(
                entry.accepts_any_field(),
                "{open} emits its config as data, so no field can be invented"
            );
            assert!(
                !entry.field_names().any(is_placeholder_key),
                "a placeholder must never be offered as a did-you-mean candidate"
            );
        }

        let closed = catalog.entry("llm_call").expect("documented");
        assert!(!closed.accepts_any_field());
    }

    /// `include_extra_info` is read by the engine off *any* node's config
    /// (`DagRunUseCase`, when it strips `extra_info` from the final output).
    /// It belongs to no node implementation, so without this section the linter
    /// reports a working setting as invented — and claims, falsely, that the
    /// engine ignores it.
    #[test]
    fn engine_wide_config_keys_are_known() {
        let catalog = NodeCatalog::embedded();
        let spec = catalog
            .common_config_field("include_extra_info")
            .expect("the engine reads this off every node");
        assert_eq!(spec.field_type, "boolean");
        assert!(!spec.required.is_unconditional());

        assert!(
            catalog.common_config_field("model").is_none(),
            "`model` is per-node-type, not engine-wide; it must stay in config_fields"
        );
    }

    #[test]
    fn parse_rejects_a_document_missing_required_sections() {
        assert!(NodeCatalog::parse("{}").is_err());
    }

    #[test]
    fn parse_tolerates_unknown_keys() {
        let json = r#"{
            "some_future_section": {"whatever": true},
            "common_node_properties": {"type": {"valid_values": ["add"]}, "config": {}},
            "node_types": {
                "add": {
                    "name": "Add",
                    "config_fields": {},
                    "some_ad_hoc_key": "prose"
                }
            }
        }"#;
        let catalog = NodeCatalog::parse(json).expect("unknown keys must not break parsing");
        assert_eq!(
            catalog.covered_node_types().collect::<Vec<_>>(),
            vec!["add"]
        );
    }
}
