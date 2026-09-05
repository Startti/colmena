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

    /// Mark the field as required only under a condition the catalog states in
    /// prose rather than formalises — e.g. `router.schema` is `"mode B only"`.
    ///
    /// The linter never reports such a field as missing: it cannot evaluate the
    /// condition, and guessing produces errors on correct graphs.
    pub fn conditional(mut self, condition: impl Into<Value>) -> Self {
        self.required = Requiredness::Conditional(condition.into());
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

    /// An entry for a node that treats its whole `config` as free-form data, so
    /// no key can be "invented" on it.
    ///
    /// `input` and `mock_input` emit their configuration as the payload for
    /// downstream nodes. The catalog says this with the [`ANY_FIELD_KEY`]
    /// placeholder, and so does this constructor — a node can still add the
    /// specific keys it also gives meaning to, as `input` does with `data`.
    pub fn open_config() -> Self {
        NodeCatalogEntry::no_config().with_field(ANY_FIELD_KEY, FieldSpec::of_type("any"))
    }

    /// Add a documented field, builder-style.
    pub fn with_field(mut self, name: impl Into<String>, spec: FieldSpec) -> Self {
        self.config_fields.insert(name.into(), spec);
        self
    }

    /// Declare input keys the engine reserves for itself on this node type.
    ///
    /// Author-supplied config must not collide with these; only a handful of
    /// nodes have them (today, `http_request`).
    pub fn with_reserved_input_keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.reserved_input_keys
            .extend(keys.into_iter().map(Into::into));
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

/// The placeholder key the catalog uses for a node whose whole `config` is data.
///
/// Named rather than spelled out at each use so the code a node writes and the
/// document it is checked against cannot drift on the spelling alone.
pub const ANY_FIELD_KEY: &str = "<any_key>";

/// Whether a catalog key stands for "a name the author chooses" rather than a
/// literal field, e.g. `<any_key>`.
pub fn is_placeholder_key(key: &str) -> bool {
    key.starts_with('<') && key.ends_with('>')
}

/// What a node type does with a key it does not declare.
///
/// The distinction is the whole point of this enum: "not declared" is not the
/// same verdict everywhere. Most node types ignore the key, a few treat any key
/// as data, and one repurposes it into something else entirely — which is worse
/// than ignoring it, because the graph then does something the author never
/// wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndeclaredKeyPolicy {
    /// Every key is meaningful to this node type (`<any_key>`, `<any_text>`).
    /// Author-supplied keys are the intended way to use it, not mistakes.
    AcceptsAnything,

    /// The node accepts the key but gives it a different job — `http_request`
    /// turns any non-reserved input into a query parameter. Worth a warning:
    /// the engine will not complain, and the graph will not do what it says.
    Repurposes,

    /// The node reads a closed set of fields and ignores the rest.
    Ignores,
}

/// What one placeholder key means, or `None` when this linter has not been
/// taught that one.
///
/// The single source of truth for which placeholders are understood. Both
/// [`NodeCatalog::undeclared_key_policy`] and the guard test that keeps the
/// shipped catalog free of uninterpretable placeholders read it, so deleting a
/// case here fails that test instead of silently switching the tool-field rule
/// off for every node type that uses it.
fn placeholder_policy(placeholder: &str) -> Option<UndeclaredKeyPolicy> {
    match placeholder {
        "<any_key>" | "<any_text>" => Some(UndeclaredKeyPolicy::AcceptsAnything),
        "<extra_keys>" => Some(UndeclaredKeyPolicy::Repurposes),
        _ => None,
    }
}

/// How a tool-only type is turned on inside `tool_configurations`.
///
/// The distinction is not cosmetic: for a [`Self::MapKey`] type the entry's own
/// `node_type` field is never read, so naming the entry anything else exposes
/// nothing at all — silently, with no error, warning or log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolActivation {
    /// Exposed when the `tool_configurations` MAP KEY equals the type's name.
    MapKey,

    /// Selected by the entry's `node_type` field, the ordinary way.
    NodeType,
}

/// A name valid only as a `tool_configurations.<tool>.node_type`.
#[derive(Debug, Clone)]
pub struct ToolOnlyType {
    /// How the entry is turned on. See [`ToolActivation`].
    pub activated_by: ToolActivation,
}

/// The parsed catalog.
#[derive(Debug, Clone)]
pub struct NodeCatalog {
    node_types: BTreeMap<String, NodeCatalogEntry>,
    node_level_properties: BTreeSet<String>,
    declared_node_types: BTreeSet<String>,
    common_config_fields: BTreeMap<String, FieldSpec>,

    /// Input port names per node type, kept OUTSIDE [`NodeCatalogEntry`] on
    /// purpose.
    ///
    /// A node used as an LLM tool receives its `node_schema` / `fixed_config`
    /// keys as *inputs*, so judging those keys against `config_fields` alone
    /// reports working graphs as broken — measured on this repo's corpus, that
    /// mistake produces 16 false positives (`task` on `subgraph`, `rows` and
    /// `user` on `python_script`).
    ///
    /// It stays out of the entry because phase 2's cross-check compares a
    /// node's `config_schema()` against the whole entry, and the agreed scope
    /// of that declaration is the node's *config*. Putting input ports inside
    /// would force all 37 nodes to declare a second axis for no gain at the
    /// node level.
    input_ports: BTreeMap<String, BTreeSet<String>>,

    /// Names valid only inside `tool_configurations`, never as a graph node's
    /// `type`.
    ///
    /// They are absent from `node_types` by necessity, not by omission: that
    /// map is closed in both directions against the engine's registry, and
    /// these are synthetic tools `llm_call` assembles itself (or, for `mcp`, a
    /// remote server alias). Without this list the linter cannot tell
    /// `data_run_python` — correct, and used by eleven graphs here — from
    /// `data_run_pythonn`, which exposes nothing.
    tool_only_types: BTreeMap<String, ToolOnlyType>,
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
    #[serde(default)]
    tool_only_node_types: RawToolOnlyNodeTypes,
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

/// A second, narrower view of the same document, read only for `input_ports`.
///
/// Deserializing the catalog twice is cheaper than the alternative: folding
/// `input_ports` into [`NodeCatalogEntry`] would change what phase 2's
/// cross-check compares, and it happens once per process behind a `OnceLock`.
#[derive(Debug, Deserialize)]
struct RawInputPortCatalog {
    node_types: BTreeMap<String, RawInputPorts>,
}

#[derive(Debug, Deserialize)]
struct RawInputPorts {
    #[serde(default)]
    input_ports: BTreeMap<String, Value>,
}

/// The `tool_only_node_types` section, read from the same document.
#[derive(Debug, Default, Deserialize)]
struct RawToolOnlyNodeTypes {
    #[serde(default)]
    types: BTreeMap<String, RawToolOnlyType>,
}

#[derive(Debug, Deserialize)]
struct RawToolOnlyType {
    activated_by: String,
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

        let ports: RawInputPortCatalog = serde_json::from_str(json)?;
        let input_ports = ports
            .node_types
            .into_iter()
            .map(|(node_type, p)| (node_type, p.input_ports.into_keys().collect()))
            .collect();

        Ok(NodeCatalog {
            node_types: raw.node_types,
            node_level_properties,
            declared_node_types,
            common_config_fields: raw.common_config_fields.fields,
            input_ports,
            tool_only_types: raw
                .tool_only_node_types
                .types
                .into_iter()
                .map(|(name, raw)| {
                    let activated_by = match raw.activated_by.as_str() {
                        "node_type" => ToolActivation::NodeType,
                        // `map_key` is the stricter reading, and the one that
                        // makes the linter speak up. An unrecognised value falls
                        // here on purpose: a catalog that says something this
                        // code has not been taught should not silently become
                        // the permissive case. A test names any such value.
                        _ => ToolActivation::MapKey,
                    };
                    (name, ToolOnlyType { activated_by })
                })
                .collect(),
        })
    }

    /// The catalog entry for `node_type`, or `None` when the type has no
    /// coverage.
    ///
    /// `None` means "cannot check", never "nothing is allowed".
    pub fn entry(&self, node_type: &str) -> Option<&NodeCatalogEntry> {
        self.node_types.get(node_type)
    }

    /// The tool-only type named `name`, or `None` when it is not one.
    ///
    /// `Some` means "this is a real thing, just not a graph node type" — which
    /// is what lets the linter stop reporting missing catalog coverage for the
    /// five synthetic names while still reporting a typo of one.
    pub fn tool_only_type(&self, name: &str) -> Option<&ToolOnlyType> {
        self.tool_only_types.get(name)
    }

    /// Every tool-only type name, for a "did you mean" suggestion.
    pub fn tool_only_type_names(&self) -> impl Iterator<Item = &str> {
        self.tool_only_types.keys().map(String::as_str)
    }

    /// Whether `node_type` declares `key` as something it reads.
    ///
    /// The union of `config_fields`, `input_ports` and `reserved_input_keys`,
    /// because a node used as an LLM tool receives its configured keys as
    /// inputs. Judging against `config_fields` alone reports working graphs as
    /// broken.
    ///
    /// Returns `None` when the catalog has no entry for `node_type`: "cannot
    /// check", never "nothing is allowed".
    pub fn declares_tool_key(&self, node_type: &str, key: &str) -> Option<bool> {
        let entry = self.node_types.get(node_type)?;
        if entry.config_fields.contains_key(key) || entry.reserved_input_keys.contains(key) {
            return Some(true);
        }
        Some(
            self.input_ports
                .get(node_type)
                .is_some_and(|ports| ports.contains(key)),
        )
    }

    /// Every key `node_type` declares, for a "did you mean" suggestion.
    ///
    /// Placeholders are excluded: `<any_key>` is not a name anyone meant to
    /// type.
    pub fn tool_key_names<'a>(&'a self, node_type: &str) -> Vec<&'a str> {
        let Some(entry) = self.node_types.get(node_type) else {
            return Vec::new();
        };
        entry
            .config_fields
            .keys()
            .map(String::as_str)
            .chain(entry.reserved_input_keys.iter().map(String::as_str))
            .chain(
                self.input_ports
                    .get(node_type)
                    .into_iter()
                    .flatten()
                    .map(String::as_str),
            )
            .filter(|k| !is_placeholder_key(k))
            .collect()
    }

    /// What `node_type` does with a key it does not declare.
    ///
    /// Read from the placeholder keys the catalog uses to describe open-ended
    /// behavior, so a new node type gets the right verdict by documenting
    /// itself rather than by editing the linter.
    ///
    /// Returns `None` when the catalog has no entry for `node_type`.
    pub fn undeclared_key_policy(&self, node_type: &str) -> Option<UndeclaredKeyPolicy> {
        let entry = self.node_types.get(node_type)?;
        let placeholders = entry
            .config_fields
            .keys()
            .map(String::as_str)
            .chain(
                self.input_ports
                    .get(node_type)
                    .into_iter()
                    .flatten()
                    .map(String::as_str),
            )
            .filter(|k| is_placeholder_key(k));

        let mut policy = UndeclaredKeyPolicy::Ignores;
        for placeholder in placeholders {
            match placeholder_policy(placeholder) {
                // The node's own data. Every key is intended.
                Some(UndeclaredKeyPolicy::AcceptsAnything) => {
                    return Some(UndeclaredKeyPolicy::AcceptsAnything)
                }
                // The node keeps the key but changes its job. Weaker than
                // `AcceptsAnything`, so it does not win over it.
                Some(other) => policy = other,
                // An unrecognised placeholder means the catalog describes
                // something this linter has not been taught. Staying at
                // `Ignores` would assert more than we know, so treat it as
                // open: silence is the safe verdict. A test guards the cost of
                // that silence.
                None => return Some(UndeclaredKeyPolicy::AcceptsAnything),
            }
        }
        Some(policy)
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

    /// Every placeholder in the blocks the tool-field rule reads, that
    /// [`NodeCatalog::undeclared_key_policy`] does not understand.
    ///
    /// Parameterised by catalog so the guard below can assert the real document
    /// is clean AND a second test can prove the scan actually detects — a guard
    /// that only ever runs against a passing input proves nothing about its own
    /// detection power.
    fn unknown_placeholders(catalog: &NodeCatalog) -> Vec<String> {
        let mut unknown: Vec<String> = Vec::new();
        for node_type in catalog.covered_node_types() {
            let entry = catalog.entry(node_type).expect("covered type has an entry");
            let placeholders = entry
                .config_fields
                .keys()
                .map(String::as_str)
                .chain(
                    catalog
                        .input_ports
                        .get(node_type)
                        .into_iter()
                        .flatten()
                        .map(String::as_str),
                )
                .filter(|k| is_placeholder_key(k));
            for placeholder in placeholders {
                if placeholder_policy(placeholder).is_none() {
                    unknown.push(format!("{node_type}.{placeholder}"));
                }
            }
        }
        unknown.sort();
        unknown
    }

    /// The guard the reliability lens asked for: the shipped catalog must not
    /// contain a placeholder the rule cannot interpret.
    ///
    /// [`NodeCatalog::undeclared_key_policy`] answers a placeholder it does not
    /// recognise with `AcceptsAnything`, because asserting less would report
    /// working graphs as broken. The cost of that choice is invisible: ONE new
    /// placeholder in a node's `config_fields` or `input_ports` silently
    /// disables the tool-field check for that whole node type, and nothing
    /// would say so.
    ///
    /// This turns that silence into a failing test. It is not hypothetical
    /// drift: the catalog already uses five other placeholder names
    /// (`<branch_name>`, `<child_output>`, `<raw>`, `<raw_config>`,
    /// `<schema_fields>`), all confined today to `output_ports`, which this
    /// rule does not read. Any of them moving would trip this.
    #[test]
    fn every_placeholder_the_tool_field_rule_can_meet_is_one_it_understands() {
        let unknown = unknown_placeholders(NodeCatalog::embedded());
        assert!(
            unknown.is_empty(),
            "these placeholders appear where the tool-field rule reads, and it \
             does not know them: {unknown:?}. Until `undeclared_key_policy` is \
             taught what they mean, that rule is silently OFF for those node \
             types. Add an arm for each, or move the placeholder to a block the \
             rule does not read."
        );
    }

    /// The guard above passes for free today, because the shipped catalog is
    /// clean — so on its own it says nothing about whether the scan can detect.
    /// This proves it can, without needing anyone to edit the real document by
    /// hand and remember to put it back.
    #[test]
    fn the_placeholder_guard_detects_a_placeholder_the_rule_cannot_interpret() {
        let drifted = NodeCatalog::parse(
            r#"{
                "common_node_properties": { "type": { "valid_values": ["add"] } },
                "node_types": {
                    "add": {
                        "config_fields": {},
                        "input_ports": {
                            "a": { "type": "number", "required": true },
                            "<inventado_nuevo>": { "type": "any", "required": false }
                        }
                    }
                }
            }"#,
        )
        .expect("test catalog must parse");

        assert_eq!(
            unknown_placeholders(&drifted),
            vec!["add.<inventado_nuevo>".to_string()],
            "the guard must name the offending node type and placeholder"
        );
    }

    /// The fallback itself, pinned so it is a decision rather than an accident.
    ///
    /// Silence is the safe answer when the catalog describes behavior this
    /// linter has not been taught: the alternative, treating the node as a
    /// closed contract, reports every configured key on it as invented.
    #[test]
    fn an_unrecognised_placeholder_makes_the_rule_silent_rather_than_wrong() {
        let catalog = NodeCatalog::parse(
            r#"{
                "common_node_properties": { "type": { "valid_values": ["future_node"] } },
                "node_types": {
                    "future_node": {
                        "config_fields": { "known": { "type": "string", "required": false } },
                        "input_ports": { "<something_new>": { "type": "any" } }
                    }
                }
            }"#,
        )
        .expect("test catalog must parse");

        assert_eq!(
            catalog.undeclared_key_policy("future_node"),
            Some(UndeclaredKeyPolicy::AcceptsAnything),
            "an unknown placeholder must silence the rule, not make it guess"
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

    /// The three declaration shapes the catalog uses that a plain
    /// `of_type(..).required()` cannot express. Each is checked against the real
    /// document rather than against itself, so a mismatch in spelling or
    /// semantics fails here rather than in a node's migration.
    #[test]
    fn open_config_builder_matches_the_documented_placeholder() {
        let built = NodeCatalogEntry::open_config();
        assert!(built.accepts_any_field());

        let documented = NodeCatalog::embedded()
            .entry("mock_input")
            .expect("mock_input is documented");
        assert_eq!(
            &built, documented,
            "open_config() must reproduce the catalog entry"
        );
    }

    #[test]
    fn conditional_builder_reproduces_a_prose_condition() {
        let spec = FieldSpec::of_type("object").conditional("mode B only");
        assert!(!spec.required.is_unconditional());

        let documented = &NodeCatalog::embedded()
            .entry("router")
            .expect("router is documented")
            .config_fields["schema"];
        assert_eq!(&spec, documented);
    }

    #[test]
    fn reserved_input_keys_builder_round_trips() {
        let built = NodeCatalogEntry::no_config().with_reserved_input_keys(["a", "b"]);
        assert_eq!(
            built
                .reserved_input_keys
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["a", "b"]
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
