//! The linter itself: a pure function from a graph to a list of findings.
//!
//! No I/O, no registry lookup, no execution. Everything it needs arrives in
//! [`LintContext`], which keeps this layer free of infrastructure and makes
//! every check trivially testable.

use super::catalog::{
    is_placeholder_key, NodeCatalog, NodeCatalogEntry, ToolActivation, UndeclaredKeyPolicy,
};
use super::diagnostic::{Diagnostic, DiagnosticCode, LintReport, Severity};
use crate::dag_engine::domain::graph::{Graph, NodeConfig};
use crate::dag_engine::domain::tool_configuration::{
    parse_node_schema, NodeSchema, NodeSchemaField,
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

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

    /// What the linter is entitled to conclude from a node type it has not seen.
    pub known_node_types: KnownNodeTypes<'a>,
}

/// Where the linter's idea of "a real node type" comes from.
///
/// The variant matters because it decides what the linter may *assert*. Saying
/// "this engine cannot run that node type" is only true when the set came from
/// the engine's own registry; said on the strength of the catalog alone it is a
/// confident guess, and wrong for a node that is registered but not yet
/// documented.
pub enum KnownNodeTypes<'a> {
    /// The engine's registry. Absence proves the engine cannot run the type.
    Registry(&'a BTreeSet<String>),

    /// Only the catalog's documented types. Absence proves nothing about the
    /// engine — only that the linter has no entry to check the node against.
    CatalogOnly,

    /// Do not draw conclusions about node types at all.
    Unchecked,
}

impl<'a> LintContext<'a> {
    /// A context backed by the embedded catalog that draws no conclusions about
    /// node types.
    ///
    /// Prefer [`Self::from_catalog`] unless you have a reason to suppress that
    /// check entirely.
    pub fn with_embedded_catalog() -> LintContext<'static> {
        LintContext {
            catalog: NodeCatalog::embedded(),
            known_node_types: KnownNodeTypes::Unchecked,
        }
    }

    /// A context that judges node types against the catalog alone.
    ///
    /// Costs nothing to build — no engine, no database — which is what lets the
    /// `lint` subcommand check a JSON file. What it gives up is the authority to
    /// say a type is unrunnable: for a type it has never seen it reports "I have
    /// no entry for this", and only calls it a mistake when a documented type is
    /// a near-miss away.
    pub fn from_catalog() -> LintContext<'static> {
        LintContext {
            catalog: NodeCatalog::embedded(),
            known_node_types: KnownNodeTypes::CatalogOnly,
        }
    }

    /// A context that judges node types against the engine's own registry.
    ///
    /// Use this when a registry is at hand: absence from it is proof, so an
    /// unknown type is reported as an outright error.
    pub fn from_registry(
        catalog: &'a NodeCatalog,
        registered: &'a BTreeSet<String>,
    ) -> LintContext<'a> {
        LintContext {
            catalog,
            known_node_types: KnownNodeTypes::Registry(registered),
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
    lint_raw_malformed_tool_entries(document, &mut report);
    lint_raw_tool_configurations(document, &mut report);
    lint_raw_tool_fields(document, ctx, &mut report);
    lint_raw_for_each_targets(document, ctx, &mut report);
    report.sort();
    Ok(report)
}

/// Visits every `tool_configurations` entry in the document.
///
/// Only `llm_call` reads the block, but this walker does not filter by node
/// type: the findings it reports are about the tool entry's own shape, and an
/// entry written under the wrong node type is already reported as an unknown
/// field on that node.
///
/// Emits in whatever order the document holds. Sorting here would be dead
/// weight: [`LintReport::sort`] orders the finished report by severity, node id,
/// field and code, and every finding below puts the tool name in its `field`, so
/// the output order is already settled by the time a caller sees it.
///
/// Yields `(node_id, tool_name, entry)` for each entry that is a JSON object.
fn tool_configuration_entries(document: &Value) -> Vec<(&String, &String, &Value)> {
    let Some(nodes) = document.get("nodes").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    for (node_id, node) in nodes {
        let Some(tools) = node
            .get("config")
            .and_then(|c| c.get("tool_configurations"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (tool_name, entry) in tools {
            if entry.is_object() {
                entries.push((node_id, tool_name, entry));
            }
        }
    }
    entries
}

/// Why the engine will refuse this tool entry at load, if it will.
///
/// Reproduces `Graph::validate`'s `node_schema` check by calling the very same
/// domain functions it calls — `NodeSchema` deserialization, then
/// [`parse_node_schema`]. Reimplementing the rule here would let the two drift;
/// sharing the code makes drift impossible by construction, and the linter is
/// in the same layer, so reaching for them breaks no boundary.
///
/// The deserialization error itself is deliberately NOT forwarded — see
/// [`unreadable_schema_shape`] for why.
///
/// `None` when the entry has no `node_schema` at all: absent is valid.
fn node_schema_rejection(entry: &Value) -> Option<String> {
    let schema_value = entry.get("node_schema")?;
    match serde_json::from_value::<NodeSchema>(schema_value.clone()) {
        Err(_) => Some(unreadable_schema_shape(schema_value)),
        Ok(schema) => first_parse_rejection(&schema),
    }
}

/// The first reason `parse_node_schema` refuses a schema, chosen deterministically.
///
/// Asking it about the whole schema at once would be the obvious call, and it
/// is what `Graph::validate` does. But `NodeSchema` is a `HashMap` and
/// `parse_node_schema` returns on the FIRST field it dislikes, so with two bad
/// fields the reason it names depends on this process's hash seed — two `lint`
/// runs over the same bytes would print different sentences into a report meant
/// to be diffed. That is tolerable for a load-time crash and not for a linter.
///
/// So it is asked one field at a time, in sorted key order, and the first
/// complaint wins. Same function, same verdict, stable sentence.
///
/// The whole schema is passed at the end as a DRIFT GUARD, not because it
/// catches anything today: every error `parse_node_schema` can return lives in
/// its per-field loop, and its second pass — the one that renames colliding
/// container children — has no error path at all, so a schema whose fields all
/// probe clean cannot fail as a whole. Verified against the binary with two
/// containers sharing a child key: no finding. The call costs one pass and
/// means that the day that function grows a genuinely cross-field error, the
/// linter reports it instead of silently passing the entry.
fn first_parse_rejection(schema: &NodeSchema) -> Option<String> {
    let mut keys: Vec<&String> = schema.keys().collect();
    keys.sort();
    for key in keys {
        let one: NodeSchema = std::iter::once((key.clone(), schema[key].clone())).collect();
        if let Some(reason) = parse_node_schema(&one).err() {
            return Some(reason);
        }
    }
    parse_node_schema(schema).err()
}

/// Says why a `node_schema` could not be read, naming shapes and keys only.
///
/// The obvious implementation is to forward serde's own error, and the first
/// version of this rule did. But serde renders `Unexpected::Str` with the
/// literal string it found, so `"node_schema": {"api_key": "sk-live-…"}` — a
/// common slip, and exactly the shape this rule exists to catch — copied the
/// secret to stdout and into the `--format json` report, which is read in CI
/// logs where the graph body itself is not printed.
///
/// One diagnostic in this linter does print a value — `INVALID_FIELD_VALUE`
/// echoes what it found next to the accepted list — but only for a field the
/// catalog declares with `valid_values`, a closed enum like `method`. No
/// credential lands there. A `node_schema` key is the opposite: a free-form
/// slot the author names, and the wrong value in it is exactly what this rule
/// catches. So this one names shapes and keys only.
///
/// Offenders are sorted for a canonical order — the sentence then does not
/// depend on the order the author happened to write the keys in. (It was never
/// unstable across runs: `serde_json::Map` is ordered. The run-to-run
/// instability lives in the other branch, and [`first_parse_rejection`] is what
/// answers it.) `parse_node_schema`'s own errors are forwarded unchanged: they
/// name the field label only, never its value.
fn unreadable_schema_shape(schema: &Value) -> String {
    let Some(fields) = schema.as_object() else {
        return format!("the whole block is {}", json_shape(schema));
    };
    let mut offenders: Vec<String> = fields
        .iter()
        .filter(|(_, value)| serde_json::from_value::<NodeSchemaField>((*value).clone()).is_err())
        .map(|(key, value)| {
            if value.is_object() {
                format!("`{key}` is an object but not a valid field definition")
            } else {
                format!("`{key}` is {}", json_shape(value))
            }
        })
        .collect();
    offenders.sort();
    if offenders.is_empty() {
        return "one of its entries is not a valid field definition".into();
    }
    offenders.join(", ")
}

/// Names a JSON value's shape without ever revealing what it holds.
fn json_shape(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Reports a tool entry the engine will refuse before running anything.
///
/// `Graph::validate` rejects the whole graph when a tool's `node_schema` cannot
/// be read, and since §18 that validation runs on every engine entry, not just
/// the CLI. So the diagnosis already existed; what did not was saying it
/// *before* the run, which is the linter's entire reason to exist.
///
/// Two families of rejection, and both are needed. The block may fail to
/// deserialize into `NodeSchema` — `null`, a string, a number, an array, or an
/// object one of whose entries is not a field definition — or it may
/// deserialize cleanly and still be refused by `parse_node_schema`: an
/// LLM-visible field with no `type`, or an `array` field with no `items` /
/// `items.type`. The second family is not a corner: `{"body": {"required":
/// true}}` is a perfectly well-formed `NodeSchemaField`, so the first check
/// waves it through and the engine still refuses it.
///
/// Reporting this suppresses the PRECEDENCE rule for that entry, and only that
/// one: `DEAD_FIXED_CONFIG` would tell the author to move keys into the very
/// `node_schema` that is broken, which fixes nothing. The field rules keep
/// firing — see the note at the suppression site.
fn lint_raw_malformed_tool_entries(document: &Value, report: &mut LintReport) {
    for (node_id, tool_name, entry) in tool_configuration_entries(document) {
        let Some(reason) = node_schema_rejection(entry) else {
            continue;
        };
        report.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: DiagnosticCode::MalformedToolEntry,
            node_id: Some(node_id.clone()),
            field: Some(format!("tool_configurations.{tool_name}.node_schema")),
            message: format!(
                "tool \"{tool_name}\" has a node_schema the engine refuses at load: \
                 {reason}"
            ),
            suggestion: Some(
                "every entry in `node_schema` must be an object; an LLM-visible field needs \
                 a `type`, and an `array` one also needs `items` with its own `type`; a \
                 field with `fixed` may omit `type`"
                    .into(),
            ),
        });
    }
}

/// Reports a `fixed_config` that the executor will never read.
///
/// `DagToolExecutor` builds a tool call's arguments in one `if`/`else if`:
/// `node_schema` is PATH 0 and `fixed_config` is only reached when it is
/// absent. So an entry carrying both silently discards the whole
/// `fixed_config` block — every key in it, not just the colliding ones.
///
/// "Absent" means the JSON key is missing or `null`. An empty object still
/// counts as present, because the executor branches on `Option::is_some` and
/// never inspects the map.
///
/// This is not a hypothetical. Three example graphs in this repo declared
/// `url`, `method`, `headers` and `allow_http_urls` in `fixed_config` next to a
/// `node_schema` for `body`; the plumbing vanished and every run failed with
/// `Invalid URL '': relative URL without a base`. A lint over the node's own
/// config could not see it, because the mistake was one level down.
///
/// An empty `fixed_config` is not reported: discarding nothing costs nothing,
/// and the author has no bug to fix.
fn lint_raw_tool_configurations(document: &Value, report: &mut LintReport) {
    for (node_id, tool_name, entry) in tool_configuration_entries(document) {
        // An entry the engine will refuse has no meaningful precedence problem,
        // and this rule's advice — move the keys into `node_schema` — would fix
        // nothing, because that schema is the thing that is broken.
        // `MALFORMED_TOOL_ENTRY` already named the real cause.
        //
        // Note this suppression is deliberately narrow. The field rules are NOT
        // suppressed: "this key is not a field of that node type" stays true
        // and actionable whatever shape the `node_schema` is in, and hiding it
        // would cost the author a second round trip for a defect that has
        // nothing to do with the first. A review caught an earlier version of
        // this change suppressing both.
        if node_schema_rejection(entry).is_some() {
            continue;
        }
        // Presence, not contents. The executor's branch is
        // `if let Some(schema) = …node_schema.as_ref()`, and `NodeSchema` is a
        // `HashMap`, so `"node_schema": {}` deserializes to `Some(empty)` and
        // still takes PATH 0 — discarding the `fixed_config` just the same.
        // Requiring a non-empty schema here would miss that graph entirely.
        // Only an ABSENT key leaves `fixed_config` alive. An explicit `null`
        // does not: `Graph::validate` deserializes this value into `NodeSchema`,
        // a bare `HashMap` rather than an `Option`, so `null` — like any
        // string, number, array or bool — is rejected at load and the graph
        // never runs. Being silent on those shapes is therefore free: they
        // cannot reach the executor either way.
        //
        // The two shapes this rule actually earns its keep on are the ones
        // `validate` lets through: an empty object and a populated one. Both
        // take PATH 0 and discard the `fixed_config`, which is the defect this
        // rule exists to name.
        let has_schema = entry.get("node_schema").is_some_and(Value::is_object);
        let fixed_keys: Vec<&str> = entry
            .get("fixed_config")
            .and_then(Value::as_object)
            .map(|f| f.keys().map(String::as_str).collect())
            .unwrap_or_default();

        if !has_schema || fixed_keys.is_empty() {
            continue;
        }

        report.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: DiagnosticCode::DeadFixedConfig,
            node_id: Some(node_id.clone()),
            field: Some(format!("tool_configurations.{tool_name}.fixed_config")),
            message: format!(
                "tool \"{tool_name}\" declares both \"node_schema\" and \"fixed_config\"; \
                 the executor reads \"node_schema\" and discards \"fixed_config\" entirely, \
                 so {} never {} the node",
                render_key_list(&fixed_keys),
                if fixed_keys.len() == 1 {
                    "reaches"
                } else {
                    "reach"
                }
            ),
            suggestion: Some(format!(
                "move {} into \"node_schema\" as fixed fields, e.g. \
                 \"{}\": {{ \"fixed\": … }}",
                if fixed_keys.len() == 1 {
                    "it".to_string()
                } else {
                    "each of them".to_string()
                },
                fixed_keys[0]
            )),
        });
    }
}

/// The blocks of a tool entry whose keys name fields of the target node.
///
/// `node_schema` and `fixed_config` are the two ways to configure a tool;
/// `node_config` is the toolkit equivalent, used by every `expose_sub_tools`
/// entry in this repo's corpus and by no other shape. All three carry node
/// field names, so all three are checked the same way.
const TOOL_FIELD_BLOCKS: [&str; 3] = ["node_schema", "fixed_config", "node_config"];

/// Reports a tool field the target node type does not read.
///
/// This is the node-level `UNKNOWN_FIELD` check, one level down. A tool entry
/// names a `node_type` and then configures it, and until now nothing checked
/// those keys against that type — which is how three graphs in this repo came
/// to declare `url` on `http_request`, whose fields are `base_url` and
/// `endpoint`.
///
/// The key set is `config_fields` + `input_ports` + `reserved_input_keys`, not
/// `config_fields` alone: a node dispatched as a tool receives its configured
/// keys as *inputs*. Measured on this repo's corpus, checking `config_fields`
/// alone reports 16 working graphs as broken.
///
/// Severity follows what the node type actually does with an undeclared key,
/// which the catalog states through its placeholder keys:
///
/// - accepts anything (`<any_key>`, `<any_text>`) — silent, the key is the
///   intended way to use it;
/// - repurposes it (`<extra_keys>`, today only `http_request`, which turns any
///   non-reserved input into a query parameter) — a warning, because the engine
///   will not complain and the graph will quietly do something else;
/// - ignores it — an error, the ordinary invented field.
fn lint_raw_tool_fields(document: &Value, ctx: &LintContext<'_>, report: &mut LintReport) {
    for (node_id, tool_name, entry) in tool_configuration_entries(document) {
        let Some(node_type) = entry.get("node_type").and_then(Value::as_str) else {
            // No `node_type` is a malformed entry the engine rejects at load;
            // guessing which node's fields to check against would be worse
            // than saying nothing.
            continue;
        };

        // A synthetic tool or an MCP alias is a real thing the catalog documents
        // separately, because `node_types` is closed against the engine's
        // registry and these are not registered nodes. Reporting missing
        // coverage for them was noise on eleven correct graphs here; skipping
        // them is also what makes that same report meaningful for a typo.
        if let Some(tool_only) = ctx.catalog.tool_only_type(node_type) {
            if tool_only.activated_by == ToolActivation::MapKey && tool_name != node_type {
                report
                    .diagnostics
                    .push(never_exposed(node_id, tool_name, node_type));
            }
            continue;
        }

        if ctx.catalog.entry(node_type).is_none() {
            if TOOL_FIELD_BLOCKS
                .iter()
                .any(|b| entry.get(b).and_then(Value::as_object).is_some())
            {
                report.diagnostics.push(Diagnostic {
                    severity: Severity::Info,
                    code: DiagnosticCode::NoCatalogCoverage,
                    node_id: Some(node_id.clone()),
                    field: Some(format!("tool_configurations.{tool_name}")),
                    message: format!(
                        "tool \"{tool_name}\" targets \"{node_type}\", which has no entry in \
                         the node catalog, so its configuration was not checked"
                    ),
                    // Now that the catalog names the synthetic tools, a near
                    // miss of one is worth saying out loud: before this the
                    // same info fired for `data_run_python` and for
                    // `data_run_pythonn`, and telling the author of a typo to
                    // "add a catalog entry" sends them the wrong way.
                    suggestion: suggest(
                        node_type,
                        ctx.catalog
                            .covered_node_types()
                            .chain(ctx.catalog.tool_only_type_names()),
                    )
                    .map(|s| format!("did you mean \"{s}\"?"))
                    .or_else(|| {
                        Some(
                            "add an entry to docs/node_configurations.json to enable checking"
                                .into(),
                        )
                    }),
                });
            }
            continue;
        }

        for block in TOOL_FIELD_BLOCKS {
            let Some(fields) = entry.get(block).and_then(Value::as_object) else {
                continue;
            };
            check_configured_keys(
                node_id,
                &format!("tool_configurations.{tool_name}.{block}"),
                fields,
                node_type,
                ctx,
                report,
            );
        }
    }
}

/// Checks the keys of one block that names fields of `node_type`.
///
/// Shared by the two places a node is configured from outside its own `config`:
/// a tool entry's blocks, and the `target` a `for_each` dispatches per row. The
/// question is identical in both — "does that node type read this key?" — so
/// the answer, its severity and its wording must be too.
///
/// Silent when the node type accepts any key: on those, an author-supplied key
/// is the intended way to use the node, not a mistake.
fn check_configured_keys(
    node_id: &str,
    location: &str,
    fields: &Map<String, Value>,
    node_type: &str,
    ctx: &LintContext<'_>,
    report: &mut LintReport,
) {
    let policy = ctx
        .catalog
        .undeclared_key_policy(node_type)
        .unwrap_or(UndeclaredKeyPolicy::AcceptsAnything);
    if policy == UndeclaredKeyPolicy::AcceptsAnything {
        return;
    }
    for key in fields.keys() {
        if key.starts_with(RESERVED_KEY_PREFIX)
            || is_annotation_key(key)
            || ctx
                .catalog
                .declares_tool_key(node_type, key)
                .unwrap_or(true)
        {
            continue;
        }
        report.diagnostics.push(undeclared_tool_field(
            node_id, location, key, node_type, policy, ctx,
        ));
    }
}

/// The node type whose `config.target` embeds another node's configuration.
const FOR_EACH_NODE_TYPE: &str = "for_each";

/// Every `for_each` target in the document, with the dotted path that reaches it.
///
/// A `for_each` embeds `{node_type, node_schema}` — the same shape as a tool
/// entry — and dispatches it once per row. That block therefore names fields of
/// another node type, and until now nothing checked them: a `url` on
/// `http_request`, the exact defect of section 20, went unreported here while
/// being caught one container over.
///
/// The node resolves its `target` through `cfg_or_input`, so the block arrives
/// by one of three doors and all three are walked: the node's own `config`, and
/// — when the `for_each` is itself dispatched as an LLM tool — the entry's
/// `node_schema.target.fixed` or its `fixed_config.target`. Anything the model
/// is left to choose at run time is not here to be read.
///
/// Yields `(node_id, location, target)` for each target that is a JSON object.
fn for_each_targets(document: &Value) -> Vec<(&String, String, &Value)> {
    let Some(nodes) = document.get("nodes").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut targets = Vec::new();
    for (node_id, node) in nodes {
        let Some(config) = node.get("config") else {
            continue;
        };

        // Door 1: the node is a `for_each` in the graph.
        if node.get("type").and_then(Value::as_str) == Some(FOR_EACH_NODE_TYPE) {
            if let Some(target) = config.get("target").filter(|t| t.is_object()) {
                targets.push((node_id, "target".to_string(), target));
            }
        }

        // Doors 2 and 3: a `for_each` exposed as a tool carries its target
        // inside the entry, fixed — the model must not choose what to run.
        let Some(tools) = config.get("tool_configurations").and_then(Value::as_object) else {
            continue;
        };
        for (tool_name, entry) in tools {
            if entry.get("node_type").and_then(Value::as_str) != Some(FOR_EACH_NODE_TYPE) {
                continue;
            }
            for (path, target) in [
                (
                    format!("tool_configurations.{tool_name}.node_schema.target.fixed"),
                    entry
                        .get("node_schema")
                        .and_then(|s| s.get("target"))
                        .and_then(|t| t.get("fixed")),
                ),
                (
                    format!("tool_configurations.{tool_name}.fixed_config.target"),
                    entry.get("fixed_config").and_then(|f| f.get("target")),
                ),
            ] {
                if let Some(target) = target.filter(|t| t.is_object()) {
                    targets.push((node_id, path, target));
                }
            }
        }
    }
    targets
}

/// Reports a `for_each` target key the node it dispatches does not read.
///
/// Same defect as on a tool entry, one container down, so it gets the same
/// wording from the same function. A malformed `target.node_schema` is a
/// separate finding and lands in its own change: leaving it out here makes
/// nothing quieter than it already is, since nothing reports it today.
fn lint_raw_for_each_targets(document: &Value, ctx: &LintContext<'_>, report: &mut LintReport) {
    for (node_id, location, target) in for_each_targets(document) {
        let Some(node_type) = target.get("node_type").and_then(Value::as_str) else {
            // Without a target type there is nothing to check the keys against.
            // The node itself fails the run with `target.node_type is required`.
            continue;
        };

        // The keys are worth checking whatever shape the block is in: an
        // invented field name is a defect of its own, and it stays true.
        if let Some(fields) = target.get("node_schema").and_then(Value::as_object) {
            check_configured_keys(
                node_id,
                &format!("{location}.node_schema"),
                fields,
                node_type,
                ctx,
                report,
            );
        }
    }
}

/// Reports a synthetic tool that will never be handed to the model.
///
/// These four are turned on by the `tool_configurations` MAP KEY, not by the
/// entry's `node_type` — `llm.rs` collects the keys into `configured_aliases`
/// and gates on `configured_aliases.contains(TOOL_DATA_RUN_PYTHON)`. The
/// `node_type` field of such an entry is read by nothing.
///
/// So an entry keyed `my_python` with `"node_type": "data_run_python"` exposes
/// no tool at all. Nothing says so: `available_tools` does look the name up in
/// the registry, gets `None`, and skips the entry with no `else` arm and no
/// log. The graph loads, validates, runs, and exits zero — the only symptom is
/// an agent that says it cannot do the task. Verified live on two graphs
/// identical but for the key: the correctly keyed one emitted tool frames and
/// the model called the tool; the other emitted none.
///
/// This rule is also why teaching the linter these five names cannot ship on
/// its own. Skipping them unconditionally would take this exact broken shape
/// from a weak `NO_CATALOG_COVERAGE` note to complete silence — a review caught
/// that regression, and the answer was to land the rule with the names rather
/// than after them.
///
/// Deliberately narrow: it fires only when the catalog says the type is
/// key-activated, so `mcp` — where `node_type` IS what selects the entry — is
/// untouched by construction rather than by a hand-written exception.
fn never_exposed(node_id: &str, tool_name: &str, node_type: &str) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: DiagnosticCode::ToolNeverExposed,
        node_id: Some(node_id.to_string()),
        field: Some(format!("tool_configurations.{tool_name}")),
        message: format!(
            "\"{node_type}\" is turned on by the entry's KEY, not by its \"node_type\"; \
             this entry is keyed \"{tool_name}\", so the tool is never handed to the \
             model and nothing reports it at run time"
        ),
        suggestion: Some(format!(
            "rename the entry key to \"{node_type}\", or drop the entry if the tool is \
             not wanted"
        )),
    }
}

/// Builds the finding for one undeclared key, worded for the policy in force.
fn undeclared_tool_field(
    node_id: &str,
    location: &str,
    key: &str,
    node_type: &str,
    policy: UndeclaredKeyPolicy,
    ctx: &LintContext<'_>,
) -> Diagnostic {
    let did_you_mean = suggest(key, ctx.catalog.tool_key_names(node_type).into_iter())
        .map(|s| format!("did you mean \"{s}\"?"));
    let (severity, code, message, fallback) = match policy {
        UndeclaredKeyPolicy::Repurposes => (
            Severity::Warning,
            DiagnosticCode::RepurposedToolField,
            format!(
                "\"{key}\" is not a field of \"{node_type}\"; that node type does not ignore an \
                 unknown key, it repurposes it — the value will be sent as a query parameter \
                 instead of configuring the node"
            ),
            "remove it, or use the field that does what you meant",
        ),
        _ => (
            Severity::Error,
            DiagnosticCode::UnknownField,
            format!("\"{key}\" is not a field of \"{node_type}\", so the node never reads it"),
            "remove it, or check docs/node_configurations.json for the field you meant",
        ),
    };
    Diagnostic {
        severity,
        code,
        node_id: Some(node_id.to_string()),
        field: Some(format!("{location}.{key}")),
        message,
        suggestion: did_you_mean.or_else(|| Some(fallback.into())),
    }
}

/// Renders field names the way the message needs to read: `"a"`, `"a" and "b"`,
/// `"a", "b" and "c"`.
fn render_key_list(keys: &[&str]) -> String {
    let quoted: Vec<String> = keys.iter().map(|k| format!("\"{k}\"")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
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
    // Judge the node type only as strongly as the context allows. Saying "this
    // engine cannot run that type" on the strength of the catalog alone is
    // false for a node that is registered but not yet documented — and that is
    // the likeliest way for an unknown type to show up in practice.
    match &ctx.known_node_types {
        KnownNodeTypes::Registry(registered) if !registered.contains(&node.node_type) => {
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
        KnownNodeTypes::CatalogOnly if ctx.catalog.entry(&node.node_type).is_none() => {
            // A near-miss against a documented type is strong evidence of a
            // typo, and worth an error. Anything else is only a gap in our own
            // coverage, and must not be dressed up as a claim about the engine.
            // A tool-only type is not a near miss and not a coverage gap: it
            // is an exact name used one level too high. Saying "add a catalog
            // entry" would send its author somewhere they cannot go, since
            // `node_types` is closed in both directions against the engine
            // registry and that entry would fail the suite.
            if ctx.catalog.tool_only_type(&node.node_type).is_some() {
                report.diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: DiagnosticCode::UnknownNodeType,
                    node_id: Some(node_id.to_string()),
                    field: None,
                    message: format!(
                        "\"{}\" is not a node type; it is only valid as the \
                         `node_type` of an entry inside a tool's \
                         `tool_configurations`",
                        node.node_type
                    ),
                    suggestion: Some(
                        "move it into the `tool_configurations` of an `llm_call`, or use \
                         a real node type here"
                            .into(),
                    ),
                });
                return;
            }

            let near = suggest(&node.node_type, ctx.catalog.covered_node_types());
            report.diagnostics.push(match near {
                Some(near) => Diagnostic {
                    severity: Severity::Error,
                    code: DiagnosticCode::UnknownNodeType,
                    node_id: Some(node_id.to_string()),
                    field: None,
                    message: format!("\"{}\" is not a documented node type", node.node_type),
                    suggestion: Some(format!("did you mean \"{near}\"?")),
                },
                None => Diagnostic {
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
                        "if the engine registers it, add an entry to \
                         docs/node_configurations.json to enable checking"
                            .into(),
                    ),
                },
            });
            return;
        }
        _ => {}
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
            // A tool-only type here is not an uncovered node: it is a name
            // that belongs one level down, inside `tool_configurations`.
            // Telling its author to add a catalog entry sends them somewhere
            // they cannot go — `node_types` is closed in both directions
            // against the engine registry, so that entry would fail the suite.
            suggestion: Some(if ctx.catalog.tool_only_type(&node.node_type).is_some() {
                format!(
                    "\"{}\" is only valid inside a tool's `tool_configurations`, \
                     never as a node's `type`",
                    node.node_type
                )
            } else {
                "add an entry to docs/node_configurations.json to enable checking".into()
            }),
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
    // Truncate the string's *contents* and then quote, rather than cutting the
    // already-quoted rendering: slicing after the opening quote left the closing
    // one behind, so a long value came out as `"xxxxx...` and read as
    // unterminated.
    match value {
        Value::String(text) => {
            if text.chars().count() > 58 {
                let head: String = text.chars().take(55).collect();
                format!("\"{head}...\"")
            } else {
                format!("\"{text}\"")
            }
        }
        other => {
            let rendered = other.to_string();
            if rendered.chars().count() > 60 {
                let head: String = rendered.chars().take(57).collect();
                format!("{head}...")
            } else {
                rendered
            }
        }
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
    fn render_key_list_reads_as_a_sentence_at_every_length() {
        assert_eq!(render_key_list(&[]), "");
        assert_eq!(render_key_list(&["url"]), "\"url\"");
        assert_eq!(
            render_key_list(&["url", "method"]),
            "\"url\" and \"method\""
        );
        assert_eq!(
            render_key_list(&["url", "method", "headers"]),
            "\"url\", \"method\" and \"headers\""
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
    /// A truncated value must still read as a complete quoted string.
    /// Cutting the rendered form left the closing quote behind, so a long value
    /// came out as `"xxxxx...` — which reads as an unterminated string and made
    /// the message look like the linter had crashed mid-sentence.
    #[test]
    fn compact_keeps_a_truncated_string_balanced() {
        let rendered = compact(&Value::String("x".repeat(200)));
        assert!(
            rendered.starts_with('"') && rendered.ends_with('"'),
            "quotes must balance; got {rendered}"
        );
        assert!(
            rendered.contains("..."),
            "must mark the truncation: {rendered}"
        );
        assert!(
            rendered.chars().count() <= 60,
            "got {} chars",
            rendered.chars().count()
        );
    }

    #[test]
    fn compact_leaves_a_short_string_intact() {
        assert_eq!(compact(&Value::String("gpt-4o".into())), "\"gpt-4o\"");
    }

    #[test]
    fn compact_truncates_a_long_non_string_without_adding_quotes() {
        let long = Value::Array((0..100).map(Value::from).collect());
        let rendered = compact(&long);
        assert!(!rendered.starts_with('"'), "not a string: {rendered}");
        assert!(rendered.ends_with("..."));
        assert!(rendered.chars().count() <= 60);
    }
}
