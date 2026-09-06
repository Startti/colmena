//! Tool configuration types for exposing DAG nodes as LLM-callable tools.
//!
//! ## Three approaches (in priority order)
//!
//! 1. **`node_schema`** (RECOMMENDED) — Unified approach via [`NodeSchema`]. A flat map where
//!    each key is a node field (e.g. `base_url`, `query_params`, `body`). Values can be:
//!    - `fixed`: hidden from the LLM, always applied as-is.
//!    - LLM-visible: typed, optionally required, with description and pattern constraints.
//!      Container fields (e.g. `body`, `query_params`) support nested `properties`, allowing
//!      mixed fixed/dynamic sub-fields. Use this for all non-trivial tool configurations.
//!
//! 2. **`$DYNAMIC` placeholders** — Simpler alternative. Use `fixed_config` with specific
//!    values set to the string literal `"$DYNAMIC"` (see [`DYNAMIC_PLACEHOLDER`]).
//!    The executor auto-exposes those fields as required `string` parameters to the LLM.
//!    **Limitation:** only works one level deep inside a container object
//!    (e.g. `body.title` works; `body.metadata.author.name` does NOT).
//!    Use only for simple cases with a few flat dynamic fields.
//!
//! 3. **Deprecated fallback** — `field_mapping` + `mergeable_fields` + `exposed_inputs`.
//!    Still executed for backward compatibility but must not be used in new configurations.
//!    All deprecated fields carry `#[deprecated(since = "0.3.0")]`.
//!
//! The execution priority in `DagToolExecutor` is: `node_schema` → `$DYNAMIC` → deprecated.

use crate::llm::domain::mcp::McpTransport;
use crate::llm::domain::ParameterProperty;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// Marker string used as a placeholder value in `fixed_config` to indicate that a field
/// should be provided by the LLM at call time.
///
/// ## Usage
/// Set any string value inside `fixed_config` to exactly `"$DYNAMIC"` (case-sensitive):
///
/// ```json
/// "fixed_config": {
///   "base_url": "https://api.example.com",
///   "body": { "author": "fixed-author", "title": "$DYNAMIC" }
/// }
/// ```
///
/// The executor detects these markers and automatically creates a required `string` parameter
/// for each one, named after the field. At execution time, the LLM-provided value replaces
/// the `"$DYNAMIC"` string in the final request.
///
/// ## Limitations
/// - All inferred parameters are typed as `string` and marked `required`. There is no way
///   to declare optional or non-string `$DYNAMIC` fields.
/// - Only works **one level deep** inside a container object. For example, `body.title`
///   is detected, but `body.metadata.author.name` is NOT — use `node_schema` instead
///   for deep nesting or complex type requirements.
pub const DYNAMIC_PLACEHOLDER: &str = "$DYNAMIC";

/// Selector for which sub-tools of a toolkit node to expose to the LLM.
///
/// Accepts either the string keyword `"all"` (expose everything the node declares)
/// or an explicit allow-list of sub-tool names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SubToolFilter {
    /// An explicit allow-list of sub-tool names (without the `toolkit_alias__` prefix).
    List(Vec<String>),
    /// String `"all"` — expose every sub-tool the node declares.
    Keyword(SubToolKeyword),
}

/// Enum-wrapped keyword used inside `SubToolFilter::Keyword` so serde can
/// distinguish the string `"all"` from an arbitrary bare string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubToolKeyword {
    #[serde(rename = "all")]
    All,
}

impl SubToolFilter {
    pub fn all() -> Self {
        Self::Keyword(SubToolKeyword::All)
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Self::Keyword(SubToolKeyword::All))
    }

    /// Return `true` if the given sub-tool should be exposed.
    pub fn includes(&self, sub_tool: &str) -> bool {
        match self {
            Self::Keyword(SubToolKeyword::All) => true,
            Self::List(v) => v.iter().any(|s| s == sub_tool),
        }
    }
}

/// How a memory-bearing tool (a sub-agent) keys its conversational memory across calls.
///
/// Conversation memory is keyed on `(agent_session_id | session_id, node_id)`, where
/// `node_id` is the tool's path qualifier. Changing that qualifier is the single lever
/// that decides whether a sub-agent invoked as a tool remembers previous calls.
///
/// Only meaningful for memory-bearing node types (see [`MEMORY_CAPABLE_NODE_TYPES`]).
/// Absent in JSON → [`MemoryMode::Stateless`], preserving today's behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMode {
    /// Every call is an isolated conversation. `node_id = tool/<tool_call_id>`.
    /// The default, and the only mode active in the current build.
    #[default]
    Stateless,
    /// One persistent conversation shared by every call to this tool.
    /// `node_id = tool/<tool_name>`.
    Persistent,
    /// The model names the thread per call via a required `thread_id` parameter.
    /// `node_id = tool/<tool_name>/<thread_id>`.
    Dynamic,
}

impl std::fmt::Display for MemoryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MemoryMode::Stateless => "stateless",
            MemoryMode::Persistent => "persistent",
            MemoryMode::Dynamic => "dynamic",
        })
    }
}

/// Node types for which [`ToolConfiguration::memory_mode`] is meaningful — the tool
/// carries or propagates conversational memory.
///
/// `orchestrator` is intentionally NOT here. Its memory propagation would actually work
/// (it dispatches sub-agents through `SubGraphNode` with a clone of its inputs, so
/// `__colmena_node_id_path` flows through), but the orchestrator node reads its entire
/// configuration (`agents`, `planner`, …) from the `config` argument via
/// `config.get("agents")` with no `inputs` fallback — and a tool dispatch passes
/// `config = {}` (everything arrives in `inputs`). So an orchestrator-as-tool runs with
/// zero agents today; it is not tool-ready at all, independent of memory. Making it
/// tool-ready first (an `inputs`-fallback like `subgraph`'s `resolve_child_graph_source`)
/// is a prerequisite before `memory_mode` on it is meaningful.
/// `planner`/`critic`/`reactor` are internal orchestrator sub-nodes, never
/// `tool_configurations` entries, so they inherit the path from their parent and are
/// not listed. The allowlist gates only the top-level tool node type.
pub const MEMORY_CAPABLE_NODE_TYPES: &[&str] = &["llm_call", "subgraph"];

/// Whether [`ToolConfiguration::memory_mode`] is meaningful for the given node type.
pub fn is_memory_capable(node_type: &str) -> bool {
    MEMORY_CAPABLE_NODE_TYPES.contains(&node_type)
}

/// Fail-closed validation of a `(node_type, memory_mode)` pair. Shared by
/// [`ToolConfiguration::validate_memory_config`] (struct path) and `Graph::validate`
/// (raw-JSON path, so a bad graph is rejected at load without a full struct decode).
///
/// Returns `Err` with an actionable message when the mode cannot be honored:
/// 1. `stateless` — always valid (nothing to persist; today's behavior).
/// 2. A non-stateless mode is only valid on [`MEMORY_CAPABLE_NODE_TYPES`].
///
/// This checks the `(node_type, mode)` pair only. The `connection_url` backend
/// requirement for a memory-bearing mode is checked separately by
/// [`memory_backend_missing_reason`], which needs the raw tool config.
pub fn validate_memory_mode(node_type: &str, mode: MemoryMode) -> Result<(), String> {
    if mode == MemoryMode::Stateless {
        return Ok(());
    }
    if !is_memory_capable(node_type) {
        return Err(format!(
            "memory_mode is only valid on memory-bearing tools ({}); \
             this tool is node_type '{}'",
            MEMORY_CAPABLE_NODE_TYPES.join(" | "),
            node_type
        ));
    }
    Ok(())
}

/// Whether a memory-bearing `llm_call` tool config carries a `connection_url`
/// (in `node_schema.<field>.fixed` or in `fixed_config`). Without one, conversation
/// memory falls back to an in-process store and does NOT survive across runs.
fn llm_call_has_connection_url(tool_cfg: &Value) -> bool {
    let in_fixed_config = tool_cfg
        .get("fixed_config")
        .and_then(|c| c.get("connection_url"))
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    let in_node_schema = tool_cfg
        .get("node_schema")
        .and_then(|s| s.get("connection_url"))
        .and_then(|f| f.get("fixed"))
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    in_fixed_config || in_node_schema
}

/// Resolve a `subgraph` tool's inline child graph from either `node_schema`
/// (`child_graph_inline.fixed`) or `fixed_config.child_graph_inline`. `None` when the
/// child is external (`child_graph_path`) or absent — those cannot be inspected here.
fn subgraph_inline_child(tool_cfg: &Value) -> Option<&Value> {
    tool_cfg
        .get("node_schema")
        .and_then(|s| s.get("child_graph_inline"))
        .and_then(|f| f.get("fixed"))
        .or_else(|| {
            tool_cfg
                .get("fixed_config")
                .and_then(|c| c.get("child_graph_inline"))
        })
}

/// Whether an inline child graph contains at least one `llm_call` node with a
/// non-empty `connection_url` — i.e. a node that can actually persist memory.
fn inline_child_has_memory_llm(inline: &Value) -> bool {
    let Some(nodes) = inline.get("nodes").and_then(|n| n.as_object()) else {
        return false;
    };
    nodes.values().any(|node| {
        let is_llm = node.get("type").and_then(|v| v.as_str()) == Some("llm_call");
        let has_url = node
            .get("config")
            .and_then(|c| c.get("connection_url"))
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());
        is_llm && has_url
    })
}

/// Reason a memory-bearing `memory_mode` cannot persist, or `None` when the backend
/// is present (or cannot be proven absent). Complements [`validate_memory_mode`]: this
/// one needs the raw tool config to look for a `connection_url`.
///
/// - `stateless` → always `None` (nothing to persist).
/// - `llm_call` → requires a `connection_url` in `node_schema`/`fixed_config`.
/// - `subgraph` with an inline child → requires an `llm_call` in it carrying a
///   `connection_url`; with an external `child_graph_path` the child cannot be
///   inspected here, so this does not block (documented caveat).
pub fn memory_backend_missing_reason(
    node_type: &str,
    mode: MemoryMode,
    tool_cfg: &Value,
) -> Option<String> {
    if mode == MemoryMode::Stateless {
        return None;
    }
    let present = match node_type {
        "llm_call" => llm_call_has_connection_url(tool_cfg),
        "subgraph" => match subgraph_inline_child(tool_cfg) {
            Some(inline) => inline_child_has_memory_llm(inline),
            // External child_graph_path — not inspectable here; don't block.
            None => true,
        },
        // Non-capable node types are rejected by validate_memory_mode already.
        _ => true,
    };
    if present {
        None
    } else {
        Some(format!(
            "memory_mode '{mode}' needs a connection_url so conversational memory persists \
             across runs; this tool has none (without it memory is in-process only and is \
             lost between runs)"
        ))
    }
}

// ---------------------------------------------------------------------------
// MCP remote servers (R3.1/R3.2)
// ---------------------------------------------------------------------------

/// `node_type` marking a `tool_configurations` entry as backed by a remote MCP
/// server rather than by a registered `ExecutableNode`.
pub const MCP_NODE_TYPE: &str = "mcp";

/// Per-call deadline when the operator does not set one.
pub const DEFAULT_MCP_TIMEOUT_SECONDS: u64 = 30;

/// How long a server's `tools/list` result stays reusable when the operator
/// does not set one. Follows the existing per-node-config `cache_ttl_seconds`
/// convention rather than being promoted onto [`ToolConfiguration`].
pub const DEFAULT_MCP_CACHE_TTL_SECONDS: u64 = 300;

/// Operator-declared connection to one remote MCP server.
///
/// `headers` holds values exactly as written in the graph JSON — normally
/// unresolved secure-value / `$DYNAMIC` references, resolved only at connect
/// time and never stored on the connection's cache key (design §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerSpec {
    pub url: String,
    #[serde(default)]
    pub transport: McpTransport,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default = "default_mcp_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_mcp_cache_ttl_seconds")]
    pub cache_ttl_seconds: u64,
}

fn default_mcp_timeout_seconds() -> u64 {
    DEFAULT_MCP_TIMEOUT_SECONDS
}

fn default_mcp_cache_ttl_seconds() -> u64 {
    DEFAULT_MCP_CACHE_TTL_SECONDS
}

/// Whether a URL's scheme is `https`, compared case-insensitively because URL
/// schemes are case-insensitive per RFC 3986 — refusing `HTTPS://` would be a
/// false rejection of a valid config.
fn is_https_url(url: &str) -> bool {
    url.split_once("://")
        .is_some_and(|(scheme, rest)| scheme.eq_ignore_ascii_case("https") && !rest.is_empty())
}

/// Fail-closed validation of a tool configuration's MCP block, on the raw-JSON
/// path so a bad graph is rejected at load without a full struct decode —
/// mirroring [`validate_memory_mode`].
///
/// Four rejections, each for a config that would otherwise fail silently:
/// 1. `node_type: "mcp"` with no reachable address. An MCP tool with no `url`
///    exposes nothing, which reads to the operator as "the model ignored my
///    server" rather than as a broken config.
/// 2. A non-HTTPS URL (R3.2). These connections carry credential headers;
///    plaintext is refused at load rather than at connect.
/// 3. An `mcp` block on a tool that is not an MCP tool — dead config the
///    operator believes is live. Not named in the design, but it is the same
///    failure class as a misplaced `memory_mode` and gets the same treatment.
/// 4. A block that parses as JSON but not as an [`McpServerSpec`] — a typo'd
///    `transport`, a `headers` that is not a string map. Checking only the url
///    let these load cleanly and then contribute nothing, which is the same
///    silent failure the other three exist to prevent.
///
/// The returned message does not repeat the tool alias: the caller wraps it in
/// [`DagError::InvalidToolSchema`], whose `Display` already names both the tool
/// and the node.
///
/// [`DagError::InvalidToolSchema`]: crate::dag_engine::domain::DagError
pub fn validate_mcp_config(node_type: &str, tool_cfg: &Value) -> Result<(), String> {
    let block = tool_cfg.get("mcp");

    if node_type != MCP_NODE_TYPE {
        return match block {
            Some(_) => Err(format!(
                "an 'mcp' block is only valid on a tool with node_type '{MCP_NODE_TYPE}'; \
                 this tool is node_type '{node_type}' and the block would be ignored"
            )),
            None => Ok(()),
        };
    }

    let url = block
        .and_then(|b| b.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("");

    if url.is_empty() {
        return Err(format!(
            "a tool with node_type '{MCP_NODE_TYPE}' needs 'mcp.url' pointing at the \
             remote MCP server; this tool has none"
        ));
    }

    if !is_https_url(url) {
        return Err(format!(
            "MCP server URL must be HTTPS, got '{url}' (these connections carry \
             credential headers, so plaintext transport is refused)"
        ));
    }

    // Parse the WHOLE block, not just the url. Checking only the url leaves a
    // graph with a typo'd `transport` or a non-object `headers` loading
    // cleanly, and then the server contributes nothing — which reads to the
    // operator as "the model ignored my server", not as "my config is
    // malformed". Failing closed here is the only place that distinction can
    // still be made.
    if let Some(block) = block {
        if let Err(e) = serde_json::from_value::<McpServerSpec>(block.clone()) {
            return Err(format!(
                "the 'mcp' block on this tool is malformed: {e}. Valid fields are \
                 url, transport (streamable_http | sse), headers (string map), \
                 timeout_seconds and cache_ttl_seconds"
            ));
        }
    }

    Ok(())
}

/// Configuration for exposing a DAG node as an LLM-callable tool.
///
/// Defined inside `tool_configurations` of an `llm_call` node. The executor uses this
/// struct to generate the tool definition sent to the LLM and to execute the node when
/// the LLM invokes the tool. See module-level docs for the three configuration approaches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfiguration {
    /// Name of the tool shown to the LLM. Optional in the JSON config: when
    /// absent it falls back to the `tool_configurations` MAP KEY, which
    /// `DagToolExecutor::generate_tool_definition` already does. It is only
    /// needed when the key is not the name you want the model to see — the
    /// frontend uses UUID keys, for instance.
    ///
    /// Made optional because an `mcp` entry has no single tool name to give: the
    /// server publishes many, and the key is the ALIAS that prefixes all of
    /// them. Requiring it there forced the alias to be repeated for nothing, and
    /// a graph that omitted it failed to load with `missing field 'name'` —
    /// which is how the first live MCP run failed, against a configuration
    /// written exactly as the canonical reference documented it.
    #[serde(default)]
    pub name: String,

    /// Human-readable description for the LLM. Optional in the JSON config: when
    /// absent or empty, the engine auto-fills a canonical description for nodes
    /// that ship one (e.g. `secure_suspend`). Otherwise it stays empty.
    #[serde(default)]
    pub description: String,

    /// Node type to execute
    pub node_type: String,

    /// Static configuration values never exposed to the LLM.
    ///
    /// When used with [`DYNAMIC_PLACEHOLDER`] values (e.g. `"title": "$DYNAMIC"`),
    /// the executor auto-exposes those fields to the LLM as required `string` parameters
    /// and replaces them at call time. This is the `$DYNAMIC` approach — simpler than
    /// `node_schema` but limited to flat, `string`-typed dynamic fields.
    ///
    /// For full control (types, optional fields, nested structures), use `node_schema` instead.
    #[serde(default)]
    pub fixed_config: HashMap<String, Value>,

    /// Which input parameters to expose to the LLM
    /// If None, expose all inputs not in fixed_config
    /// **DEPRECATED**: Use `node_schema` instead
    #[serde(skip_serializing_if = "Option::is_none")]
    #[deprecated(since = "0.3.0", note = "Use node_schema instead")]
    pub exposed_inputs: Option<Vec<String>>,

    /// Optional JSON Schema for parameters to override node schema
    /// **DEPRECATED**: Use `node_schema` instead
    #[serde(skip_serializing_if = "Option::is_none")]
    #[deprecated(since = "0.3.0", note = "Use node_schema instead")]
    pub parameters: Option<Value>,

    /// Fields where fixed + dynamic values should be merged (not overridden).
    /// Example: ["headers", "query_params", "body"]
    /// When merging a field listed here, the fixed object is the base
    /// and the dynamic (LLM-provided) object overlays it.
    /// **DEPRECATED**: Use `node_schema` instead
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[deprecated(since = "0.3.0", note = "Use node_schema instead")]
    pub mergeable_fields: Option<Vec<String>>,

    /// Maps each LLM parameter to its destination container field.
    /// The parameter value is moved into that container under its own key.
    /// Example: {"title" → "body", "x_request_id" → "headers"}
    /// Parameters not listed in this map are kept at the top level.
    /// **DEPRECATED**: Use `node_schema` instead
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[deprecated(since = "0.3.0", note = "Use node_schema instead")]
    pub field_mapping: Option<HashMap<String, String>>,

    /// Unified schema defining all node fields in one place. **This is the recommended approach.**
    ///
    /// A flat map where each key is a node field (e.g. `base_url`, `query_params`, `body`).
    /// Each entry is a [`NodeSchemaField`] that can be:
    /// - **Fixed** (`fixed` present): hidden from LLM, always applied.
    /// - **LLM-visible** (`fixed` absent): exposed to the LLM with type, description, and optional constraints.
    /// - **Container** (`properties` present): a nested object where children can be individually
    ///   fixed or LLM-visible. The fixed children are merged as base values; the LLM fills the rest.
    ///
    /// Takes priority over `fixed_config` + `$DYNAMIC` if both are present (though mixing is not recommended).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub node_schema: Option<NodeSchema>,

    /// Per-toolkit static node configuration passed to the toolkit node at runtime.
    /// Only meaningful for toolkit entries (where `expose_sub_tools` is set).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub node_config: Option<Value>,

    /// Which sub-tools of this toolkit to expose to the LLM. When present, the entry
    /// is treated as a toolkit entry and the generator expands it into N ToolDefinitions.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub expose_sub_tools: Option<SubToolFilter>,

    /// Optional short catalog entry shown when this tool is exposed via the
    /// lazy-loading catalog. ≤ 200 chars; longer values are truncated with a warning.
    /// Ignored when `lazy_tool_loading` is disabled.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub summary: Option<String>,

    /// When `lazy_tool_loading` is enabled on the parent llm_call, an `eager: true`
    /// tool is registered in every request with its full schema and does NOT appear
    /// in the `describe_tool` catalog. No effect when lazy_tool_loading is disabled.
    #[serde(default)]
    pub eager: bool,

    /// How this tool's conversational memory is keyed across calls. Only meaningful
    /// for memory-bearing node types ([`MEMORY_CAPABLE_NODE_TYPES`]). Absent → `stateless`
    /// (current behavior). Validated by [`ToolConfiguration::validate_memory_config`].
    #[serde(default)]
    pub memory_mode: MemoryMode,
}

impl ToolConfiguration {
    /// Whether this configuration represents a **toolkit** entry (a node that
    /// exposes multiple sub-tools to the LLM) rather than a legacy single-tool
    /// configuration.
    pub fn is_toolkit(&self) -> bool {
        self.expose_sub_tools.is_some()
    }

    /// The name this entry is actually known by: the `name` field when it is
    /// non-empty, otherwise the `tool_configurations` MAP KEY.
    ///
    /// `name` is optional, so every consumer that shows or matches a tool name
    /// must apply this same fallback or it will render a nameless entry. The
    /// dispatch path in `dag_tool_executor` has always done this; the lazy
    /// catalog must too.
    pub fn effective_name<'a>(&'a self, map_key: &'a str) -> &'a str {
        if self.name.is_empty() {
            map_key
        } else {
            &self.name
        }
    }

    /// Whether this entry contributes its own line to the lazy `describe_tool`
    /// catalog.
    ///
    /// Two kinds of entry do not. An `eager` tool always ships its full schema
    /// up front and never enters the catalog. An `mcp` entry is a **server**,
    /// not a tool: it publishes many and its map key is the alias that prefixes
    /// them, so listing the entry itself would show the model a line it cannot
    /// act on — and since `name` is optional on an `mcp` entry, that line would
    /// carry no name at all.
    ///
    /// Nor do the tools that server exposes: they stay in `tools`, always
    /// present with the server's own schema. `describe_tool` resolves against
    /// `ToolConfiguration`s and an MCP tool has none, so cataloguing one would
    /// hide it behind a discovery that can never happen.
    pub fn enters_lazy_catalog(&self) -> bool {
        !self.eager && self.node_type != MCP_NODE_TYPE
    }

    /// Fail-closed guard for [`ToolConfiguration::memory_mode`]. Returns `Err`
    /// describing the misconfiguration so a bad graph fails at tool-build time rather
    /// than silently doing nothing.
    ///
    /// Delegates to [`validate_memory_mode`] with this config's node type and mode.
    pub fn validate_memory_config(&self) -> Result<(), String> {
        validate_memory_mode(&self.node_type, self.memory_mode)
    }
}

/// A single field entry within a node_schema object or nested properties map.
/// Handles both leaf fields (with `fixed` or `required`) and container fields (with `properties`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSchemaField {
    /// JSON Schema type: "string", "number", "boolean", "object", "array".
    /// **Required when the field is LLM-visible** (no `fixed`, no `properties`).
    /// **Optional when `fixed` is present** — the LLM never sees the field, so
    /// the type is irrelevant. Container fields (with `properties`) default to
    /// `"object"` if omitted.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub field_type: Option<String>,

    /// If present, this field is hidden from the LLM and always set to this value.
    /// Supports runtime template syntax like "${context.foo}" (resolved elsewhere).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed: Option<Value>,

    /// Whether the LLM must supply this field (only meaningful when `fixed` is absent).
    /// If absent or false, the field is optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// Human-readable description passed to the LLM in the tool definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Regex pattern constraint (e.g., "^\\d{4}-\\d{2}-\\d{2}$"). Passed through to ParameterProperty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,

    /// Nested properties — makes this field a container (type = "object").
    /// The executor collects LLM params from children and merges them into this container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, NodeSchemaField>>,

    /// Item schema for array types. **Required** when `field_type` is `"array"` —
    /// `parse_node_schema` returns an error if missing. Describes the element type
    /// the LLM is expected to put in the array (e.g. `{ "type": "object" }` for
    /// lists of dicts, `{ "type": "string" }` for tag lists).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<NodeSchemaField>>,
}

/// The top-level node_schema value: a flat map of field name → NodeSchemaField.
/// Example top-level keys: "base_url", "bearer_token", "query_params".
pub type NodeSchema = HashMap<String, NodeSchemaField>;

/// Output of parsing a NodeSchema for use by the executor.
#[derive(Debug)]
pub struct ParsedNodeSchema {
    /// Values that are always fixed (LLM never sees them).
    /// Key = top-level field name. Value = the fixed Value (string, number, object, etc.).
    pub fixed_values: HashMap<String, Value>,

    /// LLM-visible parameter name → ParameterProperty (for ToolDefinition generation).
    pub llm_properties: HashMap<String, ParameterProperty>,

    /// Required parameter names (subset of llm_properties keys).
    pub required_params: Vec<String>,

    /// Maps each LLM param name → the container field it should be merged into.
    /// None means it goes to the top level of inputs.
    /// This replaces field_mapping for node_schema configs.
    pub param_to_container: HashMap<String, String>,
}

/// Populate `prop.items` from `field.items` when `field_type` is `"array"`.
///
/// Both OpenAI and Gemini's strict tool-schema validators reject array
/// parameters that lack an `items` clause, so arrays MUST declare it. Used by
/// BOTH the top-level and container-child branches of [`parse_node_schema`] so
/// nested arrays (e.g. `body.attachments` on an `http_request` tool) get the
/// same treatment as top-level ones. `field_label` is the field's path, used in
/// error messages (e.g. `"rows"` or `"body.attachments"`). No-op for non-arrays.
fn apply_array_items(
    prop: &mut ParameterProperty,
    field: &NodeSchemaField,
    field_type: &str,
    field_label: &str,
) -> Result<(), String> {
    if field_type != "array" {
        return Ok(());
    }
    let items_field = field.items.as_ref().ok_or_else(|| {
        format!(
            "node_schema field '{}' has type 'array' but no 'items' was specified. \
             Add e.g. \"items\": {{ \"type\": \"object\" }} for lists of objects, \
             or \"items\": {{ \"type\": \"string\" }} for lists of strings.",
            field_label
        )
    })?;
    // `items` describes what each element looks like to the LLM — so its `type`
    // is mandatory for the same reason as the field's own `type`.
    let items_type = items_field.field_type.as_ref().ok_or_else(|| {
        format!(
            "node_schema field '{}' has type 'array' but `items.type` is missing. \
             Add e.g. \"items\": {{ \"type\": \"string\" }}.",
            field_label
        )
    })?;
    let mut items_prop = ParameterProperty::new(
        items_type.clone(),
        items_field.description.clone().unwrap_or_default(),
    );
    if let Some(pattern) = &items_field.pattern {
        items_prop = items_prop.with_pattern(pattern.clone());
    }
    prop.items = Some(Box::new(items_prop));
    Ok(())
}

/// Parse a [`NodeSchema`] into the components needed by `generate_tool_definition()` and `execute()`.
///
/// Iterates over each top-level entry and handles three cases:
/// - **Fixed top-level field** (`fixed` present): added directly to `fixed_values`.
/// - **Container field** (`properties` present): child fields with `fixed` are collected into a
///   base object stored in `fixed_values`; LLM-visible children go into `llm_properties` and
///   `param_to_container` (mapped to this container key).
/// - **LLM-visible top-level field** (no `fixed`, no `properties`): added to `llm_properties`
///   at the top level (no container mapping).
pub fn parse_node_schema(schema: &NodeSchema) -> Result<ParsedNodeSchema, String> {
    let mut fixed_values: HashMap<String, Value> = HashMap::new();
    let mut llm_properties: HashMap<String, ParameterProperty> = HashMap::new();
    let mut required_params: Vec<String> = Vec::new();
    let mut param_to_container: HashMap<String, String> = HashMap::new();

    // Collected LLM-visible children from containers (for two-pass collision detection).
    // Each entry: (child_key, container_key, ParameterProperty, is_required)
    let mut container_children: Vec<(String, String, ParameterProperty, bool)> = Vec::new();

    for (top_key, top_field) in schema {
        // Case 1: Top-level field with fixed value
        if let Some(fixed_val) = &top_field.fixed {
            fixed_values.insert(top_key.clone(), fixed_val.clone());
        }
        // Case 2: Container field (has properties)
        else if let Some(properties) = &top_field.properties {
            let mut container_fixed: serde_json::Map<String, Value> = serde_json::Map::new();

            for (child_key, child_field) in properties {
                if let Some(fixed_val) = &child_field.fixed {
                    // Child has fixed value
                    container_fixed.insert(child_key.clone(), fixed_val.clone());
                } else if let Some(nested_properties) = &child_field.properties {
                    // Child is a nested container (e.g., "edge" inside "payload").
                    // Collect its fixed sub-properties into a fixed sub-object so the
                    // executor can deep-merge them with the LLM-provided object.
                    let mut nested_fixed: serde_json::Map<String, Value> = serde_json::Map::new();
                    for (nested_key, nested_field) in nested_properties {
                        if let Some(fixed_val) = &nested_field.fixed {
                            nested_fixed.insert(nested_key.clone(), fixed_val.clone());
                        }
                        // LLM-visible nested sub-properties are not individually exposed;
                        // the LLM provides them as part of the child object.
                    }
                    if !nested_fixed.is_empty() {
                        container_fixed.insert(child_key.clone(), Value::Object(nested_fixed));
                    }

                    // Container fields default to "object" when `type` is omitted —
                    // the presence of `properties` already implies object semantics.
                    let nested_type = child_field
                        .field_type
                        .clone()
                        .unwrap_or_else(|| "object".to_string());
                    let mut prop = ParameterProperty::new(
                        nested_type,
                        child_field.description.clone().unwrap_or_default(),
                    );
                    if let Some(pattern) = &child_field.pattern {
                        prop = prop.with_pattern(pattern.clone());
                    }
                    container_children.push((
                        child_key.clone(),
                        top_key.clone(),
                        prop,
                        child_field.required == Some(true),
                    ));
                } else {
                    // Child is LLM-visible (deferred to pass 2 for collision detection).
                    // `type` is required for LLM-visible fields — without it the LLM
                    // has no idea what shape of value to emit.
                    let child_type = child_field.field_type.as_ref().ok_or_else(|| {
                        format!(
                            "node_schema field '{}.{}' is LLM-visible but missing `type`. \
                             Add e.g. \"type\": \"string\" — required because the LLM needs \
                             to know what to generate. (Fields with `fixed` may omit `type`.)",
                            top_key, child_key
                        )
                    })?;
                    let mut prop = ParameterProperty::new(
                        child_type.clone(),
                        child_field.description.clone().unwrap_or_default(),
                    );

                    if let Some(pattern) = &child_field.pattern {
                        prop = prop.with_pattern(pattern.clone());
                    }

                    // Nested arrays need `items` too — same rule as top-level.
                    // (Previously omitted here, which dropped `items` for array
                    // fields inside a container and broke Gemini/OpenAI tools.)
                    apply_array_items(
                        &mut prop,
                        child_field,
                        child_type,
                        &format!("{}.{}", top_key, child_key),
                    )?;

                    container_children.push((
                        child_key.clone(),
                        top_key.clone(),
                        prop,
                        child_field.required == Some(true),
                    ));
                }
            }

            // If container has fixed children, store them as a base object
            if !container_fixed.is_empty() {
                fixed_values.insert(top_key.clone(), Value::Object(container_fixed));
            }
        }
        // Case 3: Top-level LLM-visible field (no fixed, no properties).
        // `type` is mandatory here because the LLM needs to know what to emit.
        else {
            let top_type = top_field.field_type.as_ref().ok_or_else(|| {
                format!(
                    "node_schema field '{}' is LLM-visible but missing `type`. \
                     Add e.g. \"type\": \"string\" — required because the LLM needs \
                     to know what to generate. (Fields with `fixed` may omit `type`.)",
                    top_key
                )
            })?;
            let mut prop = ParameterProperty::new(
                top_type.clone(),
                top_field.description.clone().unwrap_or_default(),
            );

            if let Some(pattern) = &top_field.pattern {
                prop = prop.with_pattern(pattern.clone());
            }

            // Array fields MUST declare `items` (OpenAI/Gemini strict
            // validators reject arrays without it). Shared with the
            // container-child branch so both paths stay consistent.
            apply_array_items(&mut prop, top_field, top_type, top_key)?;

            llm_properties.insert(top_key.clone(), prop);

            // Check if required
            if top_field.required == Some(true) {
                required_params.push(top_key.clone());
            }
        }
    }

    // Pass 2: Detect collisions and insert container children with conditional dot-prefix.
    // Count how many containers each child_key appears in.
    let mut key_count: HashMap<String, usize> = HashMap::new();
    for (child_key, _, _, _) in &container_children {
        *key_count.entry(child_key.clone()).or_insert(0) += 1;
    }

    for (child_key, container_key, prop, is_required) in container_children {
        let effective_key = if key_count.get(&child_key).copied().unwrap_or(0) > 1 {
            format!("{}.{}", container_key, child_key)
        } else {
            child_key
        };

        llm_properties.insert(effective_key.clone(), prop);
        if is_required {
            required_params.push(effective_key.clone());
        }
        param_to_container.insert(effective_key, container_key);
    }

    Ok(ParsedNodeSchema {
        fixed_values,
        llm_properties,
        required_params,
        param_to_container,
    })
}

#[cfg(test)]
mod tests {
    /// The exact shape the canonical reference documents for an MCP server, run
    /// through the SAME typed parse `llm_call` uses.
    ///
    /// This is the test that was missing. The MCP unit tests all called
    /// `collect_mcp_tool_configs` on raw JSON, which skips this parse entirely —
    /// so a config that could never load passed every one of them, and the
    /// defect only surfaced on the first live run, with `missing field 'name'`.
    #[test]
    fn the_documented_mcp_shape_parses_as_a_tool_configuration() {
        let raw = serde_json::json!({
            "deepwiki": {
                "node_type": "mcp",
                "mcp": { "url": "https://mcp.deepwiki.com/mcp" }
            }
        });

        let parsed: HashMap<String, ToolConfiguration> =
            serde_json::from_value(raw).expect("the documented MCP shape must parse");

        let entry = parsed.get("deepwiki").expect("the entry survives");
        assert_eq!(entry.node_type, "mcp");
        assert!(
            entry.name.is_empty(),
            "name is omitted, and the executor falls back to the map key"
        );
    }

    /// An ordinary tool whose author simply forgot `name` must still be listed
    /// under its map key, not as a nameless line. `name` being optional is what
    /// makes this reachable for every node_type, not only `mcp`.
    #[test]
    fn an_entry_without_a_name_falls_back_to_its_map_key() {
        let raw = serde_json::json!({
            "buscar_precio": { "node_type": "http_request" },
            "6f1c-uuid-key": { "name": "buscador", "node_type": "http_request" }
        });

        let parsed: HashMap<String, ToolConfiguration> =
            serde_json::from_value(raw).expect("parses");

        assert_eq!(
            parsed["buscar_precio"].effective_name("buscar_precio"),
            "buscar_precio",
            "an omitted name must resolve to the map key, never to the empty string"
        );
        assert_eq!(
            parsed["6f1c-uuid-key"].effective_name("6f1c-uuid-key"),
            "buscador",
            "an explicit name still wins over the key"
        );
    }

    /// A name that IS given still wins — the frontend uses UUID map keys and
    /// carries the human name in this field.
    #[test]
    fn an_explicit_name_is_preserved() {
        let raw = serde_json::json!({
            "6f1c-uuid-key": { "name": "buscador", "node_type": "http_request" }
        });

        let parsed: HashMap<String, ToolConfiguration> =
            serde_json::from_value(raw).expect("parses");

        assert_eq!(parsed["6f1c-uuid-key"].name, "buscador");
    }

    use super::*;
    use serde_json::json;

    // --- MCP config surface (R3.1/R3.2) ---

    fn mcp_cfg(node_type: &str, mcp: Value) -> Value {
        json!({ "name": "t", "node_type": node_type, "mcp": mcp })
    }

    /// The defaults are a documented part of the config surface: an operator
    /// who writes only `url` must get a working, bounded connection.
    #[test]
    fn mcp_spec_defaults_match_the_documented_values() {
        let spec: McpServerSpec =
            serde_json::from_value(json!({ "url": "https://mcp.example.com/mcp" })).unwrap();
        assert_eq!(spec.transport, McpTransport::StreamableHttp);
        assert!(spec.headers.is_empty());
        assert_eq!(spec.timeout_seconds, DEFAULT_MCP_TIMEOUT_SECONDS);
        assert_eq!(spec.cache_ttl_seconds, DEFAULT_MCP_CACHE_TTL_SECONDS);
    }

    /// Fail-closed: an `mcp` tool with no reachable address is not a tool.
    /// Silently exposing nothing would read to the operator as "the model
    /// ignored my server".
    #[test]
    fn mcp_node_type_without_a_url_is_rejected() {
        let missing_block = json!({ "name": "t", "node_type": MCP_NODE_TYPE });
        let err = validate_mcp_config(MCP_NODE_TYPE, &missing_block).unwrap_err();
        assert!(
            err.contains("url"),
            "the message must name the missing field: {err}"
        );

        let empty_url = mcp_cfg(MCP_NODE_TYPE, json!({ "url": "" }));
        assert!(validate_mcp_config(MCP_NODE_TYPE, &empty_url).is_err());
    }

    /// R3.2 — plaintext transport is refused at load, not at connect. Headers
    /// on these connections carry credentials.
    #[test]
    fn mcp_url_must_be_https() {
        let http = mcp_cfg(
            MCP_NODE_TYPE,
            json!({ "url": "http://mcp.example.com/mcp" }),
        );
        let err = validate_mcp_config(MCP_NODE_TYPE, &http).unwrap_err();
        assert!(err.contains("HTTPS"), "got: {err}");
        assert!(
            err.contains("http://mcp.example.com/mcp"),
            "the message must quote the offending URL so it is fixable: {err}"
        );

        for scheme in ["ws://", "file://", "ftp://"] {
            let cfg = mcp_cfg(MCP_NODE_TYPE, json!({ "url": format!("{scheme}host/mcp") }));
            assert!(
                validate_mcp_config(MCP_NODE_TYPE, &cfg).is_err(),
                "{scheme} must be refused"
            );
        }
    }

    /// URL schemes are case-insensitive per RFC 3986. Refusing `HTTPS://`
    /// would be a false rejection of a perfectly valid config.
    #[test]
    fn mcp_url_scheme_check_is_case_insensitive() {
        let upper = mcp_cfg(
            MCP_NODE_TYPE,
            json!({ "url": "HTTPS://mcp.example.com/mcp" }),
        );
        assert!(validate_mcp_config(MCP_NODE_TYPE, &upper).is_ok());
    }

    /// A field the url check never looks at must still fail the load.
    ///
    /// Checking only `mcp.url` let a graph with a typo'd `transport` load
    /// cleanly and then contribute zero tools, which reads to the operator as
    /// "the model ignored my server" rather than "my config is malformed".
    /// This is the only place that distinction can still be drawn.
    #[test]
    fn a_malformed_mcp_field_beyond_the_url_fails_the_load() {
        let bad_transport = json!({
            "node_type": MCP_NODE_TYPE,
            "mcp": { "url": "https://mcp.example.com/mcp", "transport": "streamablehttp" }
        });
        let err = validate_mcp_config(MCP_NODE_TYPE, &bad_transport).unwrap_err();
        assert!(
            err.contains("malformed"),
            "the error must say the block is malformed, got: {err}"
        );
        assert!(
            err.contains("streamable_http"),
            "and name the valid values, got: {err}"
        );

        let bad_headers = json!({
            "node_type": MCP_NODE_TYPE,
            "mcp": { "url": "https://mcp.example.com/mcp", "headers": ["not", "a", "map"] }
        });
        assert!(
            validate_mcp_config(MCP_NODE_TYPE, &bad_headers).is_err(),
            "headers must be a string map"
        );

        // The well-formed case still passes, so the gate did not become a wall.
        let good = json!({
            "node_type": MCP_NODE_TYPE,
            "mcp": { "url": "https://mcp.example.com/mcp", "transport": "sse" }
        });
        assert!(validate_mcp_config(MCP_NODE_TYPE, &good).is_ok());
    }

    /// An `mcp` block on a tool that is not an MCP tool is dead config the
    /// operator believes is live. Same failure class as a misplaced
    /// `memory_mode`, so it gets the same fail-closed treatment.
    #[test]
    fn mcp_block_on_a_non_mcp_node_type_is_rejected() {
        let cfg = mcp_cfg(
            "http_request",
            json!({ "url": "https://mcp.example.com/mcp" }),
        );
        let err = validate_mcp_config("http_request", &cfg).unwrap_err();
        assert!(
            err.contains("http_request"),
            "the message must name the node type: {err}"
        );
    }

    /// The overwhelmingly common case must stay untouched: no `mcp` key, no
    /// opinion.
    #[test]
    fn a_tool_without_an_mcp_block_is_unaffected() {
        let cfg = json!({ "name": "t", "node_type": "http_request" });
        assert!(validate_mcp_config("http_request", &cfg).is_ok());
    }

    fn cfg(node_type: &str, mode: Value) -> ToolConfiguration {
        serde_json::from_value(json!({
            "name": "t",
            "node_type": node_type,
            "memory_mode": mode,
        }))
        .expect("valid ToolConfiguration")
    }

    #[test]
    fn memory_mode_defaults_to_stateless_when_absent() {
        let c: ToolConfiguration =
            serde_json::from_value(json!({ "name": "t", "node_type": "llm_call" })).unwrap();
        assert_eq!(c.memory_mode, MemoryMode::Stateless);
    }

    #[test]
    fn memory_mode_parses_snake_case() {
        assert_eq!(
            cfg("llm_call", json!("persistent")).memory_mode,
            MemoryMode::Persistent
        );
        assert_eq!(
            cfg("subgraph", json!("dynamic")).memory_mode,
            MemoryMode::Dynamic
        );
    }

    #[test]
    fn is_memory_capable_allowlist() {
        assert!(is_memory_capable("llm_call"));
        assert!(is_memory_capable("subgraph"));
        assert!(!is_memory_capable("orchestrator")); // not tool-ready (reads config, not inputs)
        assert!(!is_memory_capable("http_request"));
        assert!(!is_memory_capable("critic")); // internal-only, never a tool entry
    }

    #[test]
    fn validate_stateless_is_always_ok_even_on_non_capable_type() {
        assert!(cfg("http_request", json!("stateless"))
            .validate_memory_config()
            .is_ok());
        assert!(cfg("llm_call", json!("stateless"))
            .validate_memory_config()
            .is_ok());
    }

    #[test]
    fn validate_rejects_memory_mode_on_non_capable_node_type() {
        let err = cfg("http_request", json!("persistent"))
            .validate_memory_config()
            .unwrap_err();
        assert!(
            err.contains("only valid on memory-bearing tools"),
            "got: {err}"
        );
        assert!(err.contains("http_request"), "got: {err}");
    }

    #[test]
    fn validate_accepts_persistent_and_dynamic_on_capable_type() {
        // The (node_type, mode) gate passes for both active memory modes. The
        // connection_url backend requirement is checked separately by
        // memory_backend_missing_reason.
        for mode in ["persistent", "dynamic"] {
            assert!(
                cfg("llm_call", json!(mode))
                    .validate_memory_config()
                    .is_ok(),
                "llm_call + {mode} should pass the mode gate"
            );
            assert!(
                cfg("subgraph", json!(mode))
                    .validate_memory_config()
                    .is_ok(),
                "subgraph + {mode} should pass the mode gate"
            );
        }
    }

    #[test]
    fn backend_check_applies_to_dynamic_too() {
        let without =
            json!({ "node_type": "llm_call", "node_schema": { "prompt": { "type": "string" } } });
        assert!(
            memory_backend_missing_reason("llm_call", MemoryMode::Dynamic, &without).is_some(),
            "dynamic without connection_url must be flagged"
        );
    }

    #[test]
    fn backend_ok_for_stateless_regardless_of_url() {
        let c = json!({ "node_type": "llm_call" });
        assert!(memory_backend_missing_reason("llm_call", MemoryMode::Stateless, &c).is_none());
    }

    #[test]
    fn backend_llm_call_requires_connection_url() {
        let without =
            json!({ "node_type": "llm_call", "node_schema": { "prompt": { "type": "string" } } });
        assert!(
            memory_backend_missing_reason("llm_call", MemoryMode::Persistent, &without).is_some(),
            "missing connection_url must be flagged"
        );

        let via_schema = json!({
            "node_type": "llm_call",
            "node_schema": { "connection_url": { "fixed": "${DATABASE_URL}" } }
        });
        assert!(
            memory_backend_missing_reason("llm_call", MemoryMode::Persistent, &via_schema)
                .is_none()
        );

        let via_fixed = json!({
            "node_type": "llm_call",
            "fixed_config": { "connection_url": "${DATABASE_URL}" }
        });
        assert!(
            memory_backend_missing_reason("llm_call", MemoryMode::Persistent, &via_fixed).is_none()
        );
    }

    #[test]
    fn backend_subgraph_inline_requires_llm_with_url() {
        let inline_no_url = json!({
            "node_type": "subgraph",
            "node_schema": { "child_graph_inline": { "fixed": {
                "nodes": { "keeper": { "type": "llm_call", "config": { "prompt": "{{task}}" } } },
                "edges": []
            } } }
        });
        assert!(
            memory_backend_missing_reason("subgraph", MemoryMode::Persistent, &inline_no_url)
                .is_some(),
            "inline child without connection_url must be flagged"
        );

        let inline_with_url = json!({
            "node_type": "subgraph",
            "fixed_config": { "child_graph_inline": {
                "nodes": { "keeper": { "type": "llm_call", "config": { "connection_url": "${DATABASE_URL}", "prompt": "{{task}}" } } },
                "edges": []
            } }
        });
        assert!(memory_backend_missing_reason(
            "subgraph",
            MemoryMode::Persistent,
            &inline_with_url
        )
        .is_none());
    }

    #[test]
    fn backend_subgraph_external_path_is_not_blocked() {
        // Cannot inspect an external child_graph_path here — do not block.
        let external = json!({
            "node_type": "subgraph",
            "fixed_config": { "child_graph_path": "./agents/keeper.json" }
        });
        assert!(
            memory_backend_missing_reason("subgraph", MemoryMode::Persistent, &external).is_none()
        );
    }

    #[test]
    fn test_parse_node_schema_fixed_only() {
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "base_url": { "type": "string", "fixed": "https://api.example.com" },
            "method": { "type": "string", "fixed": "GET" }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema).unwrap();

        assert_eq!(parsed.fixed_values.len(), 2);
        assert_eq!(parsed.llm_properties.len(), 0);
        assert_eq!(parsed.required_params.len(), 0);
        assert_eq!(
            parsed.fixed_values.get("base_url").unwrap().as_str(),
            Some("https://api.example.com")
        );
    }

    #[test]
    fn test_parse_node_schema_required_implicit_false() {
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "title": { "type": "string", "required": true, "description": "Required title" },
            "tags": { "type": "string", "description": "Optional tags" }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema).unwrap();

        assert_eq!(parsed.llm_properties.len(), 2);
        assert_eq!(parsed.required_params.len(), 1);
        assert!(parsed.required_params.contains(&"title".to_string()));
        assert!(!parsed.required_params.contains(&"tags".to_string()));
    }

    #[test]
    fn test_parse_node_schema_nested_container() {
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "base_url": { "type": "string", "fixed": "https://api.example.com" },
            "query_params": {
                "type": "object",
                "properties": {
                    "max": { "type": "string", "fixed": "5" },
                    "originLocationCode": { "type": "string", "required": true, "description": "Origin code" },
                    "destinationLocationCode": { "type": "string", "required": true, "description": "Destination code" },
                    "children": { "type": "string", "description": "Optional children count" }
                }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema).unwrap();

        // base_url is fixed at top level
        assert_eq!(parsed.fixed_values.len(), 2);
        assert!(parsed.fixed_values.contains_key("base_url"));
        assert!(parsed.fixed_values.contains_key("query_params"));

        // Check query_params fixed content
        let query_params = parsed.fixed_values.get("query_params").unwrap();
        assert!(query_params.is_object());
        assert_eq!(query_params.get("max").unwrap().as_str(), Some("5"));

        // LLM properties should include the 3 non-fixed children
        assert_eq!(parsed.llm_properties.len(), 3);
        assert!(parsed.llm_properties.contains_key("originLocationCode"));
        assert!(parsed
            .llm_properties
            .contains_key("destinationLocationCode"));
        assert!(parsed.llm_properties.contains_key("children"));

        // Required params check
        assert_eq!(parsed.required_params.len(), 2);
        assert!(parsed
            .required_params
            .contains(&"originLocationCode".to_string()));
        assert!(parsed
            .required_params
            .contains(&"destinationLocationCode".to_string()));

        // Param to container mapping
        assert_eq!(
            parsed.param_to_container.get("originLocationCode"),
            Some(&"query_params".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("children"),
            Some(&"query_params".to_string())
        );
    }

    #[test]
    fn test_parse_node_schema_body_container() {
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "method": { "type": "string", "fixed": "POST" },
            "body": {
                "type": "object",
                "properties": {
                    "userId": { "type": "string", "fixed": "1" },
                    "title": { "type": "string", "required": true, "description": "Post title" },
                    "content": { "type": "string", "required": true, "description": "Post content" },
                    "tags": { "type": "string", "description": "Optional tags" }
                }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema).unwrap();

        // body should be in fixed_values with userId
        assert!(parsed.fixed_values.contains_key("body"));
        let body = parsed.fixed_values.get("body").unwrap();
        assert_eq!(body.get("userId").unwrap().as_str(), Some("1"));

        // LLM properties: title, content, tags
        assert_eq!(parsed.llm_properties.len(), 3);
        assert_eq!(parsed.required_params.len(), 2); // title and content

        // All should map to body container
        assert_eq!(
            parsed.param_to_container.get("title"),
            Some(&"body".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("content"),
            Some(&"body".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("tags"),
            Some(&"body".to_string())
        );
    }

    #[test]
    fn test_parse_node_schema_array_requires_items() {
        // An array field declared without `items` must produce a parse error
        // with a message that points to the exact remedy.
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "rows": {
                "type": "array",
                "required": true,
                "description": "Lista de productos"
            }
        }))
        .unwrap();

        let err = parse_node_schema(&schema).expect_err("array without items must fail");
        assert!(
            err.contains("'rows'"),
            "error must name the field, got: {err}"
        );
        assert!(
            err.contains("'items'") || err.contains("items"),
            "error must mention items, got: {err}"
        );
        assert!(
            err.contains("\"type\": \"object\"") || err.contains("type\": \"string\""),
            "error must show example fix, got: {err}"
        );
    }

    #[test]
    fn test_parse_node_schema_array_with_items_object() {
        // Array of objects (the common case for HTTP/SQL result piping).
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "rows": {
                "type": "array",
                "required": true,
                "description": "Productos a procesar",
                "items": { "type": "object" }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema).unwrap();
        let prop = parsed.llm_properties.get("rows").unwrap();
        assert_eq!(prop.property_type, "array");
        let items = prop
            .items
            .as_ref()
            .expect("items must be set when declared in node_schema");
        assert_eq!(items.property_type, "object");
        assert!(parsed.required_params.contains(&"rows".to_string()));
    }

    #[test]
    fn test_parse_node_schema_array_with_items_string() {
        // Array of strings — verifies the items type is propagated, not silently
        // overridden to "object" the way the previous permissive default did.
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "tags": {
                "type": "array",
                "required": false,
                "description": "Etiquetas",
                "items": { "type": "string", "description": "Una etiqueta" }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema).unwrap();
        let prop = parsed.llm_properties.get("tags").unwrap();
        let items = prop.items.as_ref().unwrap();
        assert_eq!(items.property_type, "string");
        assert_eq!(items.description, "Una etiqueta");
    }

    #[test]
    fn test_parse_node_schema_container_array_propagates_items() {
        // Regression: an LLM-visible array field nested inside a CONTAINER
        // (e.g. `body.attachments` in an http_request tool) must propagate its
        // `items` to the exposed ParameterProperty — exactly like a top-level
        // array. Previously the container-child branch dropped `items`, so
        // Gemini/OpenAI rejected the tool with
        // `properties[<field>].items: missing field`.
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "body": {
                "type": "object",
                "properties": {
                    "attachments": {
                        "type": "array",
                        "required": false,
                        "description": "Files to send",
                        "items": { "type": "object", "description": "A file ref" }
                    }
                }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema).unwrap();
        // Unique child key → exposed flat as "attachments".
        let prop = parsed
            .llm_properties
            .get("attachments")
            .expect("nested array child must be exposed");
        assert_eq!(prop.property_type, "array");
        let items = prop
            .items
            .as_ref()
            .expect("items must be propagated for a container-nested array");
        assert_eq!(items.property_type, "object");
    }

    #[test]
    fn test_parse_node_schema_container_array_requires_items() {
        // Consistency with top-level arrays: a nested array WITHOUT `items`
        // fails fast at parse time (naming the dotted path) instead of silently
        // producing an invalid tool schema that the provider later rejects.
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "body": {
                "type": "object",
                "properties": {
                    "attachments": {
                        "type": "array",
                        "required": false,
                        "description": "Files to send"
                    }
                }
            }
        }))
        .unwrap();

        let err = parse_node_schema(&schema).expect_err("nested array without items must fail");
        assert!(
            err.contains("body.attachments"),
            "error must name the dotted path, got: {err}"
        );
        assert!(
            err.contains("items"),
            "error must mention items, got: {err}"
        );
    }

    #[test]
    fn test_parse_node_schema_pattern_passthrough() {
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "departureDate": {
                "type": "string",
                "required": true,
                "description": "Date in YYYY-MM-DD format",
                "pattern": "^\\d{4}-\\d{2}-\\d{2}$"
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema).unwrap();

        assert_eq!(parsed.llm_properties.len(), 1);
        let prop = parsed.llm_properties.get("departureDate").unwrap();
        assert_eq!(prop.pattern.as_deref(), Some("^\\d{4}-\\d{2}-\\d{2}$"));
    }

    #[test]
    fn test_parse_node_schema_deeply_nested_container() {
        // Simulates the create_edge payload structure:
        // payload.properties.environmentId (fixed) + payload.properties.edge (nested container
        // with its own fixed and LLM-visible sub-properties).
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "url": { "type": "string", "fixed": "https://api.example.com" },
            "payload": {
                "type": "object",
                "properties": {
                    "environmentId": { "type": "string", "fixed": "env-123" },
                    "edge": {
                        "type": "object",
                        "required": true,
                        "description": "Edge object",
                        "properties": {
                            "id": { "type": "string", "required": true, "description": "Edge ID" },
                            "source": { "type": "string", "required": true, "description": "Source node" },
                            "target": { "type": "string", "required": true, "description": "Target node" },
                            "type": { "type": "string", "fixed": "default" },
                            "animated": { "type": "boolean", "fixed": true },
                            "environmentId": { "type": "string", "fixed": "env-123" }
                        }
                    }
                }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema).unwrap();

        // url is fixed at top level
        assert!(parsed.fixed_values.contains_key("url"));

        // payload should contain fixed values for both environmentId and edge
        assert!(parsed.fixed_values.contains_key("payload"));
        let payload = parsed.fixed_values.get("payload").unwrap();
        assert_eq!(payload.get("environmentId").unwrap(), "env-123");

        // edge's fixed sub-properties should be collected
        let edge_fixed = payload.get("edge").unwrap();
        assert!(edge_fixed.is_object());
        assert_eq!(edge_fixed.get("type").unwrap(), "default");
        assert_eq!(edge_fixed.get("animated").unwrap(), true);
        assert_eq!(edge_fixed.get("environmentId").unwrap(), "env-123");

        // edge should be exposed as an LLM-visible object parameter mapped to payload
        assert!(parsed.llm_properties.contains_key("edge"));
        assert!(parsed.required_params.contains(&"edge".to_string()));
        assert_eq!(
            parsed.param_to_container.get("edge"),
            Some(&"payload".to_string())
        );

        // The LLM-visible sub-properties (id, source, target) should NOT be individually
        // exposed — the LLM provides them as part of the edge object
        assert!(!parsed.llm_properties.contains_key("id"));
        assert!(!parsed.llm_properties.contains_key("source"));
        assert!(!parsed.llm_properties.contains_key("target"));
    }

    #[test]
    fn test_parse_node_schema_collision_prefixed() {
        // Two containers with children that share the same key names ("name", "id").
        // The parser should prefix them as "source_params.name", "target_params.name", etc.
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "source_params": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "required": true, "description": "Source name" },
                    "id": { "type": "string", "required": true, "description": "Source ID" }
                }
            },
            "target_params": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "required": true, "description": "Target name" },
                    "id": { "type": "string", "description": "Target ID" }
                }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema).unwrap();

        // All 4 children should be present (no overwrites)
        assert_eq!(parsed.llm_properties.len(), 4);

        // Keys should be dot-prefixed
        assert!(parsed.llm_properties.contains_key("source_params.name"));
        assert!(parsed.llm_properties.contains_key("source_params.id"));
        assert!(parsed.llm_properties.contains_key("target_params.name"));
        assert!(parsed.llm_properties.contains_key("target_params.id"));

        // Original un-prefixed keys should NOT be present
        assert!(!parsed.llm_properties.contains_key("name"));
        assert!(!parsed.llm_properties.contains_key("id"));

        // param_to_container should map prefixed keys to the correct container
        assert_eq!(
            parsed.param_to_container.get("source_params.name"),
            Some(&"source_params".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("target_params.name"),
            Some(&"target_params".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("source_params.id"),
            Some(&"source_params".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("target_params.id"),
            Some(&"target_params".to_string())
        );

        // Required: source_params.name, source_params.id, target_params.name (3 total)
        assert_eq!(parsed.required_params.len(), 3);
        assert!(parsed
            .required_params
            .contains(&"source_params.name".to_string()));
        assert!(parsed
            .required_params
            .contains(&"source_params.id".to_string()));
        assert!(parsed
            .required_params
            .contains(&"target_params.name".to_string()));
        // target_params.id is NOT required
        assert!(!parsed
            .required_params
            .contains(&"target_params.id".to_string()));
    }

    #[test]
    fn deserialize_toolkit_config_all() {
        let json = serde_json::json!({
            "name": "web",
            "description": "Web search",
            "node_type": "tavily_client",
            "node_config": { "api_key": "${TAVILY_API_KEY}" },
            "expose_sub_tools": "all"
        });

        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.node_type, "tavily_client");
        assert!(cfg.is_toolkit());
        assert!(cfg.expose_sub_tools.as_ref().unwrap().is_all());
        assert_eq!(
            cfg.node_config
                .as_ref()
                .and_then(|v| v.get("api_key"))
                .and_then(|v| v.as_str()),
            Some("${TAVILY_API_KEY}")
        );
    }

    #[test]
    fn deserialize_toolkit_config_list() {
        let json = serde_json::json!({
            "name": "browser",
            "description": "",
            "node_type": "browser",
            "node_config": { "browserless_ws_url": "ws://localhost:3000" },
            "expose_sub_tools": ["navigate", "click"]
        });

        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert!(cfg.is_toolkit());
        let filter = cfg.expose_sub_tools.as_ref().unwrap();
        assert!(!filter.is_all());
        assert!(filter.includes("navigate"));
        assert!(filter.includes("click"));
        assert!(!filter.includes("fill"));
    }

    #[test]
    fn legacy_config_is_not_toolkit() {
        let json = serde_json::json!({
            "name": "fetch_users",
            "description": "List users",
            "node_type": "http_request",
            "fixed_config": { "base_url": "https://api.example.com" }
        });

        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert!(!cfg.is_toolkit());
        assert!(cfg.node_config.is_none());
        assert!(cfg.expose_sub_tools.is_none());
    }

    #[test]
    fn test_parse_node_schema_no_collision_no_prefix() {
        // Two containers with unique child names — no collision, no prefix needed.
        let schema = serde_json::from_value::<NodeSchema>(json!({
            "query_params": {
                "type": "object",
                "properties": {
                    "city": { "type": "string", "required": true, "description": "City name" },
                    "limit": { "type": "string", "description": "Result limit" }
                }
            },
            "headers": {
                "type": "object",
                "properties": {
                    "x_request_id": { "type": "string", "description": "Request ID" }
                }
            }
        }))
        .unwrap();

        let parsed = parse_node_schema(&schema).unwrap();

        // Keys should remain flat (no dot prefix)
        assert_eq!(parsed.llm_properties.len(), 3);
        assert!(parsed.llm_properties.contains_key("city"));
        assert!(parsed.llm_properties.contains_key("limit"));
        assert!(parsed.llm_properties.contains_key("x_request_id"));

        // No dotted keys should exist
        assert!(!parsed.llm_properties.contains_key("query_params.city"));
        assert!(!parsed.llm_properties.contains_key("headers.x_request_id"));

        // Container mappings
        assert_eq!(
            parsed.param_to_container.get("city"),
            Some(&"query_params".to_string())
        );
        assert_eq!(
            parsed.param_to_container.get("x_request_id"),
            Some(&"headers".to_string())
        );
    }

    #[test]
    fn deserializes_summary_and_eager_when_present() {
        let json = serde_json::json!({
            "name": "search_orders",
            "description": "Search the orders table",
            "node_type": "sql_query",
            "summary": "Find orders. Use when user asks about purchases.",
            "eager": true
        });
        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert_eq!(
            cfg.summary.as_deref(),
            Some("Find orders. Use when user asks about purchases.")
        );
        assert!(cfg.eager);
    }

    #[test]
    fn defaults_summary_to_none_and_eager_to_false() {
        let json = serde_json::json!({
            "name": "send_email",
            "description": "Send email",
            "node_type": "http_request"
        });
        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert!(cfg.summary.is_none());
        assert!(!cfg.eager);
    }

    /// Regression: `type` MUST stay optional in serde so authors can omit it
    /// on `fixed` fields. Before this change, every entry inside `node_schema`
    /// required `type` even when `fixed` was present — causing the silent
    /// parse failure that stripped all tools from agents (see media nodes
    /// debug session).
    #[test]
    fn fixed_field_parses_without_type() {
        let raw = serde_json::json!({
            "name": "generate_image",
            "node_type": "image_generation",
            "node_schema": {
                "provider": { "fixed": "openai" },
                "model":    { "fixed": "gpt-image-1" },
                "prompt":   { "type": "string", "required": true, "description": "p" }
            }
        });
        let cfg: ToolConfiguration =
            serde_json::from_value(raw).expect("fixed fields must parse without `type`");
        let schema = cfg.node_schema.expect("schema present");
        assert!(schema.get("provider").unwrap().field_type.is_none());
        assert_eq!(
            schema.get("prompt").unwrap().field_type.as_deref(),
            Some("string")
        );
        // Parsed schema produces exactly one LLM-visible param.
        let parsed = parse_node_schema(&schema).expect("parse ok");
        let llm_keys: Vec<&str> = parsed.llm_properties.keys().map(|s| s.as_str()).collect();
        assert_eq!(llm_keys, vec!["prompt"]);
        assert_eq!(parsed.required_params, vec!["prompt"]);
    }

    #[test]
    fn llm_visible_field_missing_type_errors_with_helpful_hint() {
        // Field has no `fixed` and no `type` → must error with a message
        // that points at the field name and suggests the fix.
        let raw = serde_json::json!({
            "broken": { "required": true, "description": "no type here" }
        });
        let schema: NodeSchema = serde_json::from_value(raw).unwrap();
        let err = parse_node_schema(&schema).unwrap_err();
        assert!(err.contains("'broken'"), "error must name the field: {err}");
        assert!(
            err.contains("LLM-visible") && err.contains("`type`"),
            "error must explain the missing type: {err}"
        );
    }

    #[test]
    fn array_items_still_require_type() {
        // Even for fixed-less arrays, items.type stays mandatory because
        // it determines the LLM-emitted element shape.
        let raw = serde_json::json!({
            "tags": {
                "type": "array",
                "required": true,
                "items": { "description": "missing type" }
            }
        });
        let schema: NodeSchema = serde_json::from_value(raw).unwrap();
        let err = parse_node_schema(&schema).unwrap_err();
        assert!(
            err.contains("items.type") || err.contains("`items"),
            "error must mention items: {err}"
        );
    }
}
