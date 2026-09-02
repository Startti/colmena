//! The linter itself: a pure function from a graph to a list of findings.
//!
//! No I/O, no registry lookup, no execution. Everything it needs arrives in
//! [`LintContext`], which keeps this layer free of infrastructure and makes
//! every check trivially testable.

use super::catalog::{is_placeholder_key, NodeCatalog, NodeCatalogEntry};
use super::diagnostic::{Diagnostic, DiagnosticCode, LintReport, Severity};
use crate::dag_engine::domain::graph::{Graph, NodeConfig};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// Keys the engine injects into a node's config or inputs at runtime.
///
/// They are never author-written, so flagging them as invented would be noise.
/// The `__` prefix is the engine-wide convention for this.
const RESERVED_KEY_PREFIX: &str = "__";

/// Whether a key is a human annotation rather than a setting.
///
/// Authors annotate graphs in place, and the engine ignores these keys by
/// design. Reporting them is pedantry that costs the linter its credibility:
/// this repo's own graphs carry over 260 of them. The linter's question is
/// "did you invent a *setting*", and a comment is not one.
fn is_annotation_key(key: &str) -> bool {
    matches!(key, "comment" | "_comment" | "$comment" | "description")
        || key.starts_with('_')
        || key.starts_with('$')
}

/// What the linter is allowed to assume about the world.
pub struct LintContext<'a> {
    /// The field-level source of truth.
    pub catalog: &'a NodeCatalog,

    /// The node types the engine can actually run.
    ///
    /// `None` when the caller has no registry at hand. The unknown-node-type
    /// check is then skipped rather than guessed: the catalog is documentation
    /// and is not authoritative about what is registered, so using it as a
    /// substitute would produce confident, wrong errors.
    pub registered_node_types: Option<&'a BTreeSet<String>>,
}

impl<'a> LintContext<'a> {
    /// A context backed by the embedded catalog and no registry.
    ///
    /// Node types are not checked. Use [`Self::from_catalog`] unless you have a
    /// specific reason to suppress that check.
    pub fn with_embedded_catalog() -> LintContext<'static> {
        LintContext {
            catalog: NodeCatalog::embedded(),
            registered_node_types: None,
        }
    }

    /// A context that also treats the catalog's documented types as the set the
    /// engine can run.
    ///
    /// This is sound because two tests in the registry enforce that the catalog
    /// and the registry describe the same set of node types, in both
    /// directions. It lets a caller check node types without paying to build an
    /// engine, which needs database connections.
    ///
    /// One documented gap: four node types register only when an optional
    /// dependency is wired — `secure_suspend` needs a `SecureValueService`, and
    /// `image_generation` / `image_edit` / `tts` need a storage adapter. This
    /// context treats them as available regardless, because it checks a graph
    /// against what the engine *can* run, not against one deployment's wiring.
    pub fn from_catalog() -> LintContext<'static> {
        static TYPES: OnceLock<BTreeSet<String>> = OnceLock::new();
        let types = TYPES.get_or_init(|| {
            NodeCatalog::embedded()
                .covered_node_types()
                .map(str::to_string)
                .collect()
        });
        LintContext {
            catalog: NodeCatalog::embedded(),
            registered_node_types: Some(types),
        }
    }
}

/// Analyses a graph from its raw JSON, and returns every finding.
///
/// Prefer this over [`lint_graph`] whenever the original document is available.
/// [`Graph`] deserialization silently discards any key it does not declare, so
/// a node carrying `"default_input_port"` — a real invention found in this
/// repo's own example graphs — is *gone* by the time a `Graph` exists. Only the
/// raw document can report it.
///
/// Returns `Err` when the document is not a graph at all; that is a parse
/// failure the caller should surface directly, not a lint finding.
pub fn lint_graph_json(
    document: &Value,
    ctx: &LintContext<'_>,
) -> Result<LintReport, serde_json::Error> {
    let graph: Graph = serde_json::from_value(document.clone())?;
    let mut report = lint_graph(&graph, ctx);
    lint_raw_node_properties(document, ctx, &mut report);
    report.sort();
    Ok(report)
}

/// Reports keys on a node object that the engine does not read.
///
/// Graph-root keys are deliberately *not* checked. Across this repo's 301
/// example graphs, the undeclared root keys are all annotations — `comment`,
/// `metadata`, `_comment`, `$comment`, `description` — used over 260 times and
/// understood by everyone to be inert. Flagging them would bury the findings
/// that matter. Node-level keys are the opposite: the only undeclared ones in
/// the entire corpus are `inputs`, `default_input_port` and
/// `default_output_port`, and every one of them is a genuine mistake.
fn lint_raw_node_properties(document: &Value, ctx: &LintContext<'_>, report: &mut LintReport) {
    let Some(nodes) = document.get("nodes").and_then(Value::as_object) else {
        return;
    };
    let allowed: BTreeSet<&str> = ctx.catalog.node_level_properties().collect();

    let mut ids: Vec<&String> = nodes.keys().collect();
    ids.sort();

    for node_id in ids {
        let Some(node) = nodes[node_id].as_object() else {
            continue;
        };
        for key in node.keys() {
            if key == "config"
                || key.starts_with(RESERVED_KEY_PREFIX)
                || is_annotation_key(key)
                || allowed.contains(key.as_str())
            {
                continue;
            }
            report.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: DiagnosticCode::UnknownNodeProperty,
                node_id: Some(node_id.clone()),
                field: Some(key.clone()),
                message: format!(
                    "\"{key}\" is not a property of a node; the engine discards it when \
                     loading the graph"
                ),
                suggestion: suggest(key, allowed.iter().copied())
                    .map(|s| format!("did you mean \"{s}\"?"))
                    .or_else(|| Some("move it into \"config\" if the node reads it there".into())),
            });
        }
    }
}

/// Analyses `graph` and returns every finding, most severe first.
pub fn lint_graph(graph: &Graph, ctx: &LintContext<'_>) -> LintReport {
    let mut report = LintReport::default();

    lint_edges(graph, &mut report);

    // Iterating a HashMap yields an arbitrary order; collecting and sorting the
    // ids first makes the report deterministic before the final sort, which
    // matters for readable diffs in tests and CI logs.
    let mut node_ids: Vec<&String> = graph.nodes.keys().collect();
    node_ids.sort();

    for node_id in node_ids {
        let node = &graph.nodes[node_id];
        lint_node(node_id, node, graph, ctx, &mut report);
    }

    report.sort();
    report
}

/// An edge endpoint that names no node is silently resolved to JSON `null` at
/// run time, so the downstream node receives nothing and the graph appears to
/// "work". Naming it here is the whole point.
fn lint_edges(graph: &Graph, report: &mut LintReport) {
    for edge in &graph.edges {
        for (role, endpoint) in [("from", &edge.from), ("to", &edge.to)] {
            // An edge endpoint may address a port on a node — `node.field` —
            // so only the node part is matched against the graph's nodes.
            let node_part = endpoint.split('.').next().unwrap_or(endpoint);
            if node_part.is_empty() || graph.nodes.contains_key(node_part) {
                continue;
            }
            report.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: DiagnosticCode::EdgeUnknownNode,
                node_id: None,
                field: None,
                message: format!(
                    "edge {}=\"{}\" names a node that this graph does not define",
                    role, endpoint
                ),
                // `graph.nodes` is a HashMap; `suggest` breaks ties on the
                // candidate name so the answer does not follow hash order.
                suggestion: suggest(node_part, graph.nodes.keys().map(String::as_str))
                    .map(|s| format!("did you mean \"{s}\"?")),
            });
        }
    }
}

fn lint_node(
    node_id: &str,
    node: &NodeConfig,
    graph: &Graph,
    ctx: &LintContext<'_>,
    report: &mut LintReport,
) {
    if let Some(registered) = ctx.registered_node_types {
        if !registered.contains(&node.node_type) {
            report.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: DiagnosticCode::UnknownNodeType,
                node_id: Some(node_id.to_string()),
                field: None,
                message: format!(
                    "\"{}\" is not a node type this engine can run",
                    node.node_type
                ),
                suggestion: suggest(&node.node_type, registered.iter().map(String::as_str))
                    .map(|s| format!("did you mean \"{s}\"?")),
            });
            // Without a real node type there is nothing to check the config
            // against; every field would be reported as invented.
            return;
        }
    }

    let Some(entry) = ctx.catalog.entry(&node.node_type) else {
        report.diagnostics.push(Diagnostic {
            severity: Severity::Info,
            code: DiagnosticCode::NoCatalogCoverage,
            node_id: Some(node_id.to_string()),
            field: None,
            message: format!(
                "\"{}\" has no entry in the node catalog, so this node's \
                 configuration was not checked",
                node.node_type
            ),
            suggestion: Some(
                "add an entry to docs/node_configurations.json to enable checking".into(),
            ),
        });
        return;
    };

    let Some(config) = node.config.as_object() else {
        // Absent config is normal — many nodes take everything from edges.
        // A non-object config is not, but the engine tolerates it and every
        // field lookup simply misses, so this is a warning rather than an error.
        if !node.config.is_null() {
            report.diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                code: DiagnosticCode::FieldTypeMismatch,
                node_id: Some(node_id.to_string()),
                field: None,
                message: "\"config\" should be a JSON object; every field lookup \
                          on this node will miss"
                    .into(),
                suggestion: None,
            });
        }
        report_missing_required(node_id, entry, &Map::new(), graph, report);
        return;
    };

    for (field, value) in config {
        lint_field(
            node_id,
            &node.node_type,
            entry,
            ctx.catalog,
            field,
            value,
            report,
        );
    }

    report_missing_required(node_id, entry, config, graph, report);
}

fn lint_field(
    node_id: &str,
    node_type: &str,
    entry: &NodeCatalogEntry,
    catalog: &NodeCatalog,
    field: &str,
    value: &Value,
    report: &mut LintReport,
) {
    if field.starts_with(RESERVED_KEY_PREFIX) {
        return;
    }
    // An annotation is only ever inert; but a node type that genuinely documents
    // a field by that name still gets it checked.
    if is_annotation_key(field) && !entry.knows_field(field) {
        return;
    }

    let Some(spec) = entry
        .config_fields
        .get(field)
        // Some config keys are read by the engine off any node, whatever its
        // type, so they belong to no node's `config_fields`.
        .or_else(|| catalog.common_config_field(field))
    else {
        // This node type treats its config as free-form data, so there is no
        // such thing as an invented key on it.
        if entry.accepts_any_field() {
            return;
        }
        report.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: DiagnosticCode::UnknownField,
            node_id: Some(node_id.to_string()),
            field: Some(field.to_string()),
            message: format!("\"{field}\" is not a configuration field of {node_type}"),
            suggestion: suggest(
                field,
                entry
                    .field_names()
                    .chain(catalog.common_config_field_names()),
            )
            .map(|s| format!("did you mean \"{s}\"?"))
            .or_else(|| Some("the engine ignores it silently".into())),
        });
        return;
    };

    // A field the catalog marks engine-populated cannot be set by the author.
    // Silently accepting it is precisely the dead configuration this tool
    // exists to surface — `router.temperature` is documented "NOT
    // configurable" and hardcoded to 0.1 in both of the router's modes.
    if spec.read_only {
        report.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: DiagnosticCode::UnknownField,
            node_id: Some(node_id.to_string()),
            field: Some(field.to_string()),
            message: format!(
                "\"{field}\" is populated by the engine on {node_type} and cannot be set here"
            ),
            suggestion: Some("remove it; the value written here has no effect".into()),
        });
        return;
    }

    if let Some(accepted) = &spec.valid_values {
        if !accepted.is_empty() && !accepted.contains(value) && !is_placeholder(value) {
            report.diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                code: DiagnosticCode::InvalidFieldValue,
                node_id: Some(node_id.to_string()),
                field: Some(field.to_string()),
                message: format!(
                    "{} is not one of the documented values for \"{field}\"",
                    compact(value)
                ),
                suggestion: Some(format!(
                    "accepted: {}",
                    accepted.iter().map(compact).collect::<Vec<_>>().join(", ")
                )),
            });
        }
    }

    if let Some(expected) = type_mismatch(&spec.field_type, value) {
        report.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            code: DiagnosticCode::FieldTypeMismatch,
            node_id: Some(node_id.to_string()),
            field: Some(field.to_string()),
            message: format!(
                "\"{field}\" is documented as {expected} but the value is {}",
                json_type_name(value)
            ),
            suggestion: None,
        });
    }
}

/// Reports fields the catalog marks required that the config does not set.
///
/// A required field is not necessarily a *config* field: several nodes resolve
/// one from an incoming edge instead (the `cfg_or_input` pattern). Whether a
/// given edge carries a given field is not knowable from the graph, so when the
/// node has any incoming edge this is a warning that says so, and only a node
/// with no incoming edge at all can be called an outright error.
fn report_missing_required(
    node_id: &str,
    entry: &NodeCatalogEntry,
    config: &Map<String, Value>,
    graph: &Graph,
    report: &mut LintReport,
) {
    // An edge writes into a named port: `"to": "node.query"` states exactly
    // which field it supplies. Ignoring that name and asking only "does this
    // node have any incoming edge?" throws away the answer the graph already
    // gave, and reports a field that is demonstrably provided.
    let mut supplied_ports: BTreeSet<&str> = BTreeSet::new();
    let mut has_unnamed_incoming_edge = false;
    for edge in &graph.edges {
        let mut parts = edge.to.splitn(2, '.');
        if parts.next() != Some(node_id) {
            continue;
        }
        match parts.next() {
            Some(port) => {
                supplied_ports.insert(port);
            }
            // A bare `"to": "node"` targets the node's default input port,
            // whose name lives in the node implementation rather than the
            // graph, so it could still be carrying this field.
            None => has_unnamed_incoming_edge = true,
        }
    }

    for (field, spec) in &entry.config_fields {
        // A placeholder is not a field anyone can set.
        if is_placeholder_key(field) {
            continue;
        }
        // A conditional requirement states a condition the catalog does not
        // formalise. Guessing it produces errors on correct graphs.
        if !spec.required.is_unconditional() || config.contains_key(field) {
            continue;
        }
        if spec.read_only {
            continue;
        }
        // An edge names this exact field: it is supplied, not missing.
        if supplied_ports.contains(field.as_str()) {
            continue;
        }

        let (severity, message, suggestion) = if has_unnamed_incoming_edge {
            (
                Severity::Warning,
                format!("required field \"{field}\" is not set in config"),
                Some(
                    "this node has an incoming edge with no port name, so the value may \
                     arrive through its default input port instead"
                        .to_string(),
                ),
            )
        } else {
            (
                Severity::Error,
                format!("required field \"{field}\" is not set, and no incoming edge supplies it"),
                None,
            )
        };

        report.diagnostics.push(Diagnostic {
            severity,
            code: DiagnosticCode::MissingRequiredField,
            node_id: Some(node_id.to_string()),
            field: Some(field.clone()),
            message,
            suggestion,
        });
    }
}

/// Whether a value is a placeholder the engine resolves before the node sees it.
///
/// `${ENV_VAR}`, `$DYNAMIC…` and `$ref…` all stand in for a value that does not
/// exist yet, so checking them against a documented value set is meaningless.
fn is_placeholder(value: &Value) -> bool {
    matches!(value, Value::String(s) if s.starts_with("${") || s.starts_with('$'))
}

/// The documented type name when `value` does not match `documented`, else `None`.
///
/// Deliberately permissive. The catalog's type vocabulary is prose-adjacent —
/// it includes `"any"` and unions like `"string|array"` — and a linter that
/// guesses wrong about a type is worse than one that stays quiet, so anything
/// not clearly understood is accepted.
fn type_mismatch<'a>(documented: &'a str, value: &Value) -> Option<&'a str> {
    // Placeholders stand in for the eventual value, whose type we cannot see.
    if is_placeholder(value) {
        return None;
    }
    // `null` means "unset" in every node that reads config, not a typed value.
    if value.is_null() {
        return None;
    }

    let accepted = documented.split('|').any(|t| match t.trim() {
        "any" => true,
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        // An unrecognised vocabulary word means the catalog knows something
        // this function does not. Accept rather than invent a mismatch.
        _ => true,
    });

    (!accepted).then_some(documented)
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(n) if n.is_f64() => "a number",
        Value::Number(_) => "an integer",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// A short rendering of a value for a message, so a huge object cannot flood
/// the report.
fn compact(value: &Value) -> String {
    let s = match value {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    };
    if s.chars().count() > 60 {
        let truncated: String = s.chars().take(57).collect();
        format!("{truncated}...")
    } else {
        s
    }
}

/// The closest candidate to `input`, when one is close enough to be worth
/// suggesting.
///
/// The threshold scales with the word's length: a two-character name has no
/// near-misses worth guessing at, while a long one tolerates a couple of typos.
///
/// Ties break on the candidate's own name, so the answer never depends on the
/// order candidates arrive in. That matters because one caller draws them from
/// a `HashMap`: without a total order, two equally-close names would be
/// separated by hash order and the message would change between runs.
fn suggest<'a, I: Iterator<Item = &'a str>>(input: &str, candidates: I) -> Option<String> {
    let max_distance = match input.chars().count() {
        0..=3 => 1,
        4..=8 => 2,
        _ => 3,
    };

    candidates
        .map(|c| (levenshtein(input, c), c))
        .filter(|(d, _)| *d > 0 && *d <= max_distance)
        .min_by_key(|(d, c)| (*d, c.len(), *c))
        .map(|(_, c)| c.to_string())
}

/// Standard edit distance, two-row variant.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];

    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    //! Unit tests for this module's private helpers.
    //!
    //! Everything that exercises the public surface over a whole graph lives in
    //! `tests/graph_lint.rs` instead — those are integration tests by nature,
    //! and keeping them here made this file's test module four times the size
    //! of the code it covers.
    use super::*;

    #[test]
    fn a_union_type_accepts_either_member() {
        // `stream` and similar union-typed fields must not be flagged.
        for value in [serde_json::json!("a"), serde_json::json!(["a"])] {
            let mismatch = type_mismatch("string|array", &value);
            assert!(mismatch.is_none(), "union must accept {value}");
        }
        assert_eq!(
            type_mismatch("string|array", &serde_json::json!(1)),
            Some("string|array")
        );
    }
    #[test]
    fn an_unknown_type_word_is_accepted_rather_than_guessed() {
        assert!(type_mismatch("some-future-type", &serde_json::json!(1)).is_none());
        assert!(type_mismatch("any", &serde_json::json!({"a": 1})).is_none());
    }
    /// A dangling edge draws its suggestion from `graph.nodes`, a HashMap.
    /// When two candidates are equally close, `min_by_key` keeps whichever it
    /// saw first, so the answer follows iteration order.
    ///
    /// Note this cannot be caught by linting the same graph repeatedly:
    /// Rust seeds `RandomState` once per process, so every HashMap built in one
    /// test run iterates identically. The property to pin down is that
    /// `suggest` gives the same answer whatever order the candidates arrive in.
    #[test]
    fn suggest_breaks_ties_independently_of_candidate_order() {
        let tied = ["abd", "abc", "abf", "abe"];
        let expected = suggest("abx", tied.iter().copied());
        assert_eq!(
            expected.as_deref(),
            Some("abc"),
            "with every candidate one edit away, the tie must break on a stable rule"
        );

        // Every rotation and the reverse must agree.
        for start in 0..tied.len() {
            let rotated: Vec<&str> = tied[start..]
                .iter()
                .chain(&tied[..start])
                .copied()
                .collect();
            assert_eq!(suggest("abx", rotated.iter().copied()), expected);
            assert_eq!(
                suggest("abx", rotated.iter().rev().copied()),
                expected,
                "order must not decide the answer"
            );
        }
    }
    #[test]
    fn suggest_stays_quiet_when_nothing_is_close() {
        assert_eq!(
            suggest("completely_different", ["model", "provider"].into_iter()),
            None
        );
        assert_eq!(
            suggest("model", ["model"].into_iter()),
            None,
            "an exact match is not a suggestion"
        );
    }
    #[test]
    fn suggest_is_stricter_on_short_names() {
        // "n" vs "a": distance 1, allowed for a 1-char input.
        assert_eq!(suggest("n", ["a"].into_iter()).as_deref(), Some("a"));
        // Two edits away from a 3-char input is too far to guess.
        assert_eq!(suggest("abc", ["xyz"].into_iter()), None);
    }
    #[test]
    fn levenshtein_matches_known_distances() {
        assert_eq!(levenshtein("model", "modle"), 2);
        assert_eq!(levenshtein("model", "model"), 0);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }
    #[test]
    fn compact_truncates_a_long_value() {
        let long = Value::String("x".repeat(200));
        let rendered = compact(&long);
        assert!(
            rendered.chars().count() <= 60,
            "got {} chars",
            rendered.chars().count()
        );
        assert!(rendered.ends_with("..."));
    }
}
