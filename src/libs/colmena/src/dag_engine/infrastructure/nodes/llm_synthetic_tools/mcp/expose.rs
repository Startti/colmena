//! Turning an MCP server's catalog into tools the model can see.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::dag_engine::domain::tool_configuration::{McpServerSpec, MCP_NODE_TYPE};
use crate::llm::domain::mcp::{
    normalize, McpToolDescriptor, MCP_MAX_DESCRIPTION_BYTES, MCP_MAX_EXPOSED_NAME_LEN,
    MCP_MAX_SCHEMA_BYTES, MCP_MAX_SUMMARY_BYTES, MCP_MAX_TOOLS_PER_SERVER,
};
use crate::llm::domain::text_bounds::head_truncate;
use crate::llm::domain::tools::{ToolDefinition, ToolParameters};

/// The `mcp` blocks declared in a raw `tool_configurations` object.
///
/// Read from raw JSON rather than from `ToolConfiguration`, which carries no
/// typed `mcp` field. `validate_mcp_config` already works this way, so both
/// sites deserialise the same `McpServerSpec` and neither duplicates knowledge
/// of the field set.
///
/// A malformed block is SKIPPED here rather than reported: `Graph::validate`
/// already fails the load closed on one, so anything reaching this point has
/// passed that gate. Returning empty for an entry that never validates would
/// be dead defence; returning an error would be a second, divergent gate.
pub fn collect_mcp_tool_configs(raw: &Value) -> BTreeMap<String, McpServerSpec> {
    let Some(entries) = raw.as_object() else {
        return BTreeMap::new();
    };
    entries
        .iter()
        .filter(|(_, cfg)| cfg.get("node_type").and_then(Value::as_str) == Some(MCP_NODE_TYPE))
        .filter_map(|(alias, cfg)| {
            let spec: McpServerSpec = serde_json::from_value(cfg.get("mcp")?.clone()).ok()?;
            Some((alias.clone(), spec))
        })
        .collect()
}

/// Tool definitions for one server's catalog, plus a note per excluded tool.
///
/// The server's `input_schema` is forwarded VERBATIM via
/// `input_schema_override`. Re-deriving it into Colmena's flat `ToolParameters`
/// would quietly change what the model is told — a lost `required`, a dropped
/// `enum` — so the schema is passed through untouched and only the top-level
/// summary is capped.
///
/// A schema over `MCP_MAX_SCHEMA_BYTES` EXCLUDES its tool instead of being
/// truncated: a truncated JSON Schema is invalid, and a provider rejecting the
/// request would take the server's healthy tools down with it.
pub fn exposed_definitions(
    alias: &str,
    tools: &[McpToolDescriptor],
) -> (Vec<ToolDefinition>, BTreeMap<String, String>, Vec<String>) {
    let mut defs = Vec::new();
    // Exposed name -> the tool that PRODUCED it, verbatim. Emitted here because
    // this loop is the only place that knows which descriptor survived: a tool
    // dropped for an oversized schema never reaches the `taken` set, so a later
    // tool normalising the same way becomes the exposed one. A caller matching
    // names against the catalog afterwards would find the DROPPED tool first and
    // route the model's calls to a tool it was never shown.
    let mut origins: BTreeMap<String, String> = BTreeMap::new();
    // At most one note per considered tool, and the loop below considers at
    // most `MCP_MAX_TOOLS_PER_SERVER` of them, so this is bounded by that same
    // ceiling without needing its own.
    let mut skipped: Vec<String> = Vec::new();
    // `normalize` collapses every character outside `[A-Za-z0-9_-]` to `_`, so
    // two distinct names in ONE catalog can land on the same exposed name —
    // `foo.bar` and `foo/bar` both become `alias__foo_bar`. `drop_colliding`
    // cannot catch it: it compares against names Colmena already claimed, not
    // against siblings from the same server. Two definitions sharing a name
    // would reach the provider as a duplicate declaration, which Gemini
    // rejects outright.
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();

    // The per-server tool ceiling lands HERE, and only here. The client's
    // pagination loop borrows the same constant but bounds PAGES with it — its
    // own comment says so, and says capping tools "belongs to the exposure
    // slice". This is that slice. One page carrying ten thousand tools passes
    // the client untouched, so without this a single server could occupy the
    // model's entire tool list.
    for tool in tools.iter().take(MCP_MAX_TOOLS_PER_SERVER) {
        // Measure the schema Colmena will actually forward, not the raw one.
        // `$schema` and `$id` are stripped before the provider sees it, so
        // counting them here would reject a tool for size on bytes that never
        // leave this process. The ceiling exists to protect the provider
        // request, so it must weigh exactly what that request carries — and the
        // stripped value is reused below as the override, computed once.
        let forwarded_schema = without_schema_metadata(&tool.input_schema);
        let schema_bytes = serde_json::to_string(&forwarded_schema)
            .map(|s| s.len())
            .unwrap_or(usize::MAX);
        if schema_bytes > MCP_MAX_SCHEMA_BYTES {
            skipped.push(format!(
                "tool '{}' on MCP server '{alias}' was not exposed: its input schema \
                 is {schema_bytes} bytes, over the {MCP_MAX_SCHEMA_BYTES}-byte ceiling",
                for_report(&tool.name)
            ));
            continue;
        }

        let exposed_name = normalize(alias, &tool.name);
        if !taken.insert(exposed_name.clone()) {
            skipped.push(format!(
                "tool '{}' on MCP server '{alias}' was not exposed: its name \
                 normalises to '{exposed_name}', which another tool on the same \
                 server already took",
                for_report(&tool.name)
            ));
            continue;
        }
        origins.insert(exposed_name.clone(), tool.name.clone());
        defs.push(
            ToolDefinition::new(
                exposed_name,
                for_model(&tool.description, MCP_MAX_DESCRIPTION_BYTES),
                ToolParameters::default(),
            )
            .with_summary(for_model(&tool.description, MCP_MAX_SUMMARY_BYTES))
            .with_input_schema_override(forwarded_schema),
        );
    }

    if tools.len() > MCP_MAX_TOOLS_PER_SERVER {
        let dropped = tools.len() - MCP_MAX_TOOLS_PER_SERVER;
        skipped.push(format!(
            "MCP server '{alias}' offered {} tools; only the first \
             {MCP_MAX_TOOLS_PER_SERVER} were considered and {dropped} were not looked at",
            tools.len()
        ));
    }

    (defs, origins, skipped)
}

/// `head_truncate`, but only when there is something to truncate.
///
/// `head_truncate` appends its `[truncated: ...]` marker unconditionally, so
/// calling it on a short string tells the model content was cut when none was.
/// Every other call site in the crate guards with a length check first; this is
/// that guard, named, so the next caller here does not have to remember.
fn cap(s: &str, max_bytes: usize) -> String {
    if s.len() > max_bytes {
        head_truncate(s, max_bytes)
    } else {
        s.to_string()
    }
}

/// The same schema without the JSON Schema metadata keywords.
///
/// The schema is otherwise forwarded byte-identical on purpose — it is the
/// contract the server publishes and Colmena is in no position to reinterpret
/// it. `$schema` and `$id` are the exception because they describe the DOCUMENT,
/// not the parameters: removing them cannot change which arguments are valid.
///
/// They are removed because Gemini rejects them outright. Its
/// `function_declarations[].parameters` is an OpenAPI subset, so a single
/// unknown key fails the WHOLE request with a 400 — not just that tool, and not
/// just MCP's tools: every built-in the agent had goes down with it. That is the
/// failure this module's own ceiling comment already warns about for oversized
/// schemas, arriving through a different door. Context7 publishes `$schema` and
/// DeepWiki does not, which is exactly why one worked live and the other
/// returned INVALID_ARGUMENT.
///
/// Deliberately narrow: only these two keys, only at the top level. Provider
/// schema dialects differ in more ways than this and a general translation layer
/// is a real design problem, not something to improvise here. What is fixed is
/// what was observed to break.
fn without_schema_metadata(schema: &Value) -> Value {
    let Some(map) = schema.as_object() else {
        return schema.clone();
    };
    if !map.contains_key("$schema") && !map.contains_key("$id") {
        return schema.clone();
    }
    Value::Object(
        map.iter()
            .filter(|(k, _)| k.as_str() != "$schema" && k.as_str() != "$id")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

/// A server-chosen description, made safe to hand the model.
///
/// Differs from [`for_report`] deliberately: a description is prose, so `\n`
/// and `\t` are legitimate formatting and stripping them would mangle real
/// content — the live Context7 catalog in `tests/fixtures/mcp/` ships a 2 KB
/// description whose only control character is a newline. `ESC` and the other
/// C0 codes have no such use, and this string reaches the provider on every
/// request, so they are replaced before the length cap runs.
fn for_model(description: &str, max_bytes: usize) -> String {
    let printable: String = description
        .chars()
        .map(|c| {
            if c == '\n' || c == '\t' || !c.is_control() {
                c
            } else {
                ' '
            }
        })
        .collect();
    cap(&printable, max_bytes)
}

/// A server-chosen name, made safe to put in a report line.
///
/// Bounds the length AND strips control characters. A length cap alone closes
/// only half of it: a SHORT name, well under any ceiling, can still carry
/// `\x1b[2J` or a bare `\r` and land byte-for-byte in a log, an operator's
/// terminal, or — once a caller wires this up — the model's context. The
/// exposed name never needs this because `normalize` has already restricted it
/// to `[A-Za-z0-9_-]`; the RAW name has had nothing done to it at all.
fn for_report(name: &str) -> String {
    let printable: String = name
        .chars()
        .map(|c| if c.is_control() { '_' } else { c })
        .collect();
    cap(&printable, MCP_MAX_EXPOSED_NAME_LEN)
}

/// Drop MCP tools whose exposed name is already claimed, and say so.
///
/// MCP ALWAYS loses. A remote server is third-party input; letting it shadow
/// `describe_tool` or `load_skill` would let whoever controls that server
/// redefine what a Colmena built-in does mid-conversation. Losing is not a
/// tie-break, it is the containment boundary.
///
/// The caller could get the same survivors by pushing MCP last and relying on
/// first-wins dedup, but that path is SILENT. An operator whose tool vanished
/// needs to be told which name took it.
pub fn drop_colliding(
    defs: Vec<ToolDefinition>,
    claimed: &std::collections::HashSet<String>,
) -> (Vec<ToolDefinition>, Vec<String>) {
    let mut kept = Vec::with_capacity(defs.len());
    let mut warnings = Vec::new();
    for def in defs {
        if claimed.contains(&def.name) {
            // Says the name is taken, NOT who took it. The caller decides what
            // goes in `claimed`, and the slice that wires several servers will
            // have to add each server's kept names to it so one cannot displace
            // another. This message must stay true when the claimer is another
            // MCP server rather than a built-in.
            warnings.push(format!(
                "MCP tool '{}' was not exposed: that name is already claimed and an \
                 MCP tool never takes a name that is already in use.",
                def.name
            ));
        } else {
            kept.push(def);
        }
    }
    (kept, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Catalogs probed from the live servers on 2026-09-01, committed so the
    /// shapes under test are the ones real servers actually send.
    fn fixture(name: &str) -> Vec<McpToolDescriptor> {
        let raw = std::fs::read_to_string(format!(
            "{}/tests/fixtures/mcp/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture readable");
        let doc: Value = serde_json::from_str(&raw).expect("fixture parses");
        doc["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| McpToolDescriptor {
                name: t["name"].as_str().unwrap().to_string(),
                title: t["title"].as_str().map(str::to_string),
                description: t["description"].as_str().unwrap_or_default().to_string(),
                input_schema: t["inputSchema"].clone(),
            })
            .collect()
    }

    fn find<'a>(defs: &'a [ToolDefinition], name: &str) -> &'a ToolDefinition {
        defs.iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("no tool named {name} in {:?}", names(defs)))
    }

    fn names(defs: &[ToolDefinition]) -> Vec<&str> {
        defs.iter().map(|d| d.name.as_str()).collect()
    }

    /// The server's schema is what the model must see. Re-deriving it — even
    /// "equivalently" — is how a required field or an enum silently changes
    /// meaning, so this asserts byte identity of everything that DEFINES a
    /// parameter.
    ///
    /// It once asserted byte identity of the whole document, and it passed
    /// against this very fixture while the feature was broken: Context7 publishes
    /// `$schema`, Gemini rejects an unknown key with a 400, and the whole request
    /// died — every tool, not only MCP's. The test was faithful to what the code
    /// did; the contract it pinned was the wrong one. Now the document metadata
    /// is excluded from BOTH sides and everything else must still match
    /// byte-for-byte, so the narrowing cannot quietly widen into a rewrite.
    #[test]
    fn mcp_schema_is_forwarded_byte_identical_apart_from_document_metadata() {
        let tools = fixture("context7_tools");
        let source = tools
            .iter()
            .find(|t| t.name == "resolve-library-id")
            .expect("fixture has resolve-library-id");
        assert!(
            source.input_schema.get("$schema").is_some(),
            "this fixture must keep carrying $schema or the test proves nothing"
        );
        let (defs, _, _) = exposed_definitions("context7", &tools);

        let exposed = find(&defs, "context7__resolve-library-id");
        let override_schema = exposed
            .input_schema_override
            .as_ref()
            .expect("MCP tools carry the server schema verbatim, not a rebuilt one");
        assert!(
            override_schema.get("$schema").is_none(),
            "document metadata must not reach the provider"
        );
        assert_eq!(
            serde_json::to_string(override_schema).unwrap(),
            serde_json::to_string(&without_schema_metadata(&source.input_schema)).unwrap(),
            "apart from the metadata, the forwarded schema must be byte-identical"
        );
    }

    /// Gemini fails the WHOLE request on an unknown key, so `$schema` — which
    /// Context7 publishes and standard JSON Schema encourages — took down every
    /// tool the agent had, MCP's and built-in alike. Observed live as a 400
    /// INVALID_ARGUMENT before this was stripped.
    #[test]
    fn schema_document_metadata_is_not_forwarded() {
        let tools = vec![McpToolDescriptor {
            name: "resolve-library-id".to_string(),
            title: None,
            description: "finds a library".to_string(),
            input_schema: json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "$id": "https://example.com/s.json",
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }),
        }];

        let (defs, _, _) = exposed_definitions("ctx7", &tools);
        let sent = defs[0].input_schema_override.as_ref().expect("schema");

        assert!(
            sent.get("$schema").is_none(),
            "$schema reached the provider"
        );
        assert!(sent.get("$id").is_none(), "$id reached the provider");
    }

    /// And nothing ELSE may be lost on the way. The schema is the server's
    /// contract; stripping metadata must not quietly become rewriting it.
    #[test]
    fn everything_but_the_metadata_survives_untouched() {
        let properties = json!({
            "query": { "type": "string", "description": "what to look for" },
            "libraryName": { "type": "string" }
        });
        let tools = vec![McpToolDescriptor {
            name: "resolve-library-id".to_string(),
            title: None,
            description: "finds a library".to_string(),
            input_schema: json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": properties,
                "required": ["query"],
                "additionalProperties": false
            }),
        }];

        let (defs, _, _) = exposed_definitions("ctx7", &tools);
        let sent = defs[0].input_schema_override.as_ref().expect("schema");

        assert_eq!(sent.get("type"), Some(&json!("object")));
        assert_eq!(sent.get("properties"), Some(&properties));
        assert_eq!(sent.get("required"), Some(&json!(["query"])));
        assert_eq!(
            sent.get("additionalProperties"),
            Some(&json!(false)),
            "a key that is NOT metadata must survive"
        );
    }

    /// A schema with no metadata must come through byte-identical, so the
    /// stripping cannot become an unconditional rewrite.
    #[test]
    fn a_schema_without_metadata_is_passed_through_unchanged() {
        let schema = json!({
            "type": "object",
            "properties": { "repoName": { "type": "string" } },
            "required": ["repoName"]
        });
        let tools = vec![McpToolDescriptor {
            name: "ask_question".to_string(),
            title: None,
            description: "asks".to_string(),
            input_schema: schema.clone(),
        }];

        let (defs, _, _) = exposed_definitions("deepwiki", &tools);
        assert_eq!(defs[0].input_schema_override.as_ref(), Some(&schema));
    }

    /// A long description nested INSIDE the schema is part of the contract the
    /// model reasons about. Only the top-level summary is capped.
    #[test]
    fn mcp_long_nested_description_is_not_truncated() {
        let long = "d".repeat(5 * 1024);
        let tools = vec![McpToolDescriptor {
            name: "verbose".to_string(),
            title: None,
            description: "short".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": { "q": { "type": "string", "description": long } }
            }),
        }];
        let (defs, _, _) = exposed_definitions("srv", &tools);
        let schema = find(&defs, "srv__verbose")
            .input_schema_override
            .as_ref()
            .expect("schema forwarded");
        assert_eq!(
            schema["properties"]["q"]["description"]
                .as_str()
                .unwrap()
                .len(),
            5 * 1024,
            "a description nested in the schema must survive intact"
        );
    }

    /// Over the ceiling the tool is DROPPED, not shipped with a mangled schema:
    /// a truncated JSON Schema is invalid and the provider would reject the
    /// whole request, taking the server's healthy tools down with it.
    #[test]
    fn mcp_schema_over_ceiling_is_excluded_not_truncated() {
        let huge = "x".repeat(40 * 1024);
        let tools = vec![
            McpToolDescriptor {
                name: "oversized".to_string(),
                title: None,
                description: "too big".to_string(),
                input_schema: json!({ "type": "object", "blob": huge }),
            },
            McpToolDescriptor {
                name: "healthy".to_string(),
                title: None,
                description: "fine".to_string(),
                input_schema: json!({ "type": "object" }),
            },
        ];
        let (defs, _, skipped) = exposed_definitions("srv", &tools);
        assert_eq!(
            names(&defs),
            vec!["srv__healthy"],
            "one bad tool must not sink the rest"
        );
        assert_eq!(
            skipped.len(),
            1,
            "the excluded tool is reported, not dropped in silence"
        );
        assert!(
            skipped[0].contains("oversized"),
            "the report names the tool: {}",
            skipped[0]
        );
    }

    /// The ceiling weighs the FORWARDED schema, not the raw one. `$schema` and
    /// `$id` are stripped before the provider sees the tool, so a schema that
    /// only crosses the ceiling because of that metadata fits once it is gone —
    /// and must be exposed, not rejected for bytes that never leave the process.
    #[test]
    fn a_schema_over_ceiling_only_with_metadata_is_still_exposed() {
        // Body alone stays under the ceiling; the giant `$id` pushes the raw
        // document over it. Stripping the metadata is what brings it back under.
        let body = "b".repeat(30 * 1024);
        let bloated_id = format!("https://example.com/{}", "x".repeat(4 * 1024));
        let schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "$id": bloated_id,
            "type": "object",
            "properties": { "blob": { "type": "string", "description": body } }
        });
        assert!(
            serde_json::to_string(&schema).unwrap().len() > MCP_MAX_SCHEMA_BYTES,
            "raw schema must exceed the ceiling or the test proves nothing"
        );
        assert!(
            serde_json::to_string(&without_schema_metadata(&schema))
                .unwrap()
                .len()
                <= MCP_MAX_SCHEMA_BYTES,
            "stripped schema must fit or the test proves nothing"
        );

        let tools = vec![McpToolDescriptor {
            name: "big_id".to_string(),
            title: None,
            description: "fine once stripped".to_string(),
            input_schema: schema,
        }];
        let (defs, _, skipped) = exposed_definitions("srv", &tools);
        assert_eq!(
            names(&defs),
            vec!["srv__big_id"],
            "a tool that fits after stripping metadata must be exposed"
        );
        assert!(skipped.is_empty(), "nothing should be skipped: {skipped:?}");
    }

    /// A short description must not arrive claiming it was cut.
    ///
    /// `head_truncate` appends its marker unconditionally, so calling it
    /// unguarded told the model "[truncated: showing first 47 of 47 bytes]"
    /// about a 47-byte string. Every DeepWiki description is short enough to
    /// have carried that lie on every request.
    #[test]
    fn a_short_description_is_not_marked_as_truncated() {
        let tools = fixture("deepwiki_tools");
        let (defs, _, _) = exposed_definitions("deepwiki", &tools);
        for d in &defs {
            assert!(
                !d.description.contains("[truncated"),
                "{}: a description that fits was marked truncated: {:?}",
                d.name,
                d.description
            );
            let summary = d.summary.as_ref().expect("summary present");
            assert!(
                !summary.contains("[truncated"),
                "{}: a summary that fits was marked truncated: {summary:?}",
                d.name
            );
        }
    }

    /// The top-level description IS bounded. It is what every provider adapter
    /// sends on every request, so an unbounded one lets a third-party server
    /// inflate the whole conversation.
    #[test]
    fn an_oversized_description_is_capped() {
        let huge = "d".repeat(3 * MCP_MAX_DESCRIPTION_BYTES);
        let tools = vec![McpToolDescriptor {
            name: "verbose".to_string(),
            title: None,
            description: huge,
            input_schema: json!({ "type": "object" }),
        }];
        let (defs, _, _) = exposed_definitions("srv", &tools);
        let d = find(&defs, "srv__verbose");
        assert!(
            d.description.len() <= MCP_MAX_DESCRIPTION_BYTES,
            "description not capped: {} bytes",
            d.description.len()
        );
        assert!(
            d.description.contains("[truncated"),
            "and a real truncation must say so"
        );
    }

    /// One server cannot ship two tools under one exposed name.
    ///
    /// `normalize` collapses disallowed characters, so `foo.bar` and `foo/bar`
    /// both become `srv__foo_bar`. `drop_colliding` cannot see this: it checks
    /// names Colmena claimed, not siblings from the same catalog. Two
    /// definitions with one name reach the provider as a duplicate
    /// declaration.
    #[test]
    fn two_tools_normalising_to_one_name_do_not_both_survive() {
        let tools = vec![
            McpToolDescriptor {
                name: "foo.bar".to_string(),
                title: None,
                description: "first".to_string(),
                input_schema: json!({ "type": "object" }),
            },
            McpToolDescriptor {
                name: "foo/bar".to_string(),
                title: None,
                description: "second".to_string(),
                input_schema: json!({ "type": "object" }),
            },
        ];
        let (defs, _, skipped) = exposed_definitions("srv", &tools);

        assert_eq!(
            names(&defs),
            vec!["srv__foo_bar"],
            "only the first keeps the name"
        );
        assert_eq!(skipped.len(), 1, "and the loss is reported, not silent");
        assert!(
            skipped[0].contains("foo/bar") && skipped[0].contains("srv__foo_bar"),
            "the report names both the tool and the name it wanted: {}",
            skipped[0]
        );
    }

    /// The raw server-chosen name is bounded before it reaches a report.
    ///
    /// The exposed name goes through `normalize`, so anything built from it is
    /// already charset-restricted and short. The SKIPPED report names the tool
    /// as the server spelled it, which is unbounded third-party text: without a
    /// cap a hostile catalog could push megabytes, control characters or
    /// injected instructions into whatever channel the report ends up in.
    #[test]
    fn a_reported_tool_name_is_bounded_even_when_the_server_is_not() {
        let huge = "n".repeat(10 * 1024);
        let tools = vec![McpToolDescriptor {
            name: huge.clone(),
            title: None,
            description: "x".to_string(),
            input_schema: json!({ "type": "object", "blob": "y".repeat(40 * 1024) }),
        }];
        let (_defs, _, skipped) = exposed_definitions("srv", &tools);

        assert_eq!(skipped.len(), 1, "the oversized schema excludes it");
        assert!(
            !skipped[0].contains(&huge),
            "the report must not carry the server's raw name verbatim"
        );
        assert!(
            skipped[0].len() < 1024,
            "and must stay small: {} bytes",
            skipped[0].len()
        );
    }

    /// The OTHER report site, and the charset half of the risk.
    ///
    /// The oversized-name test above trips the schema ceiling, which `continue`s
    /// before the collision branch is ever reached — so it exercises only one of
    /// the two places a raw server name reaches a report. These names are SHORT,
    /// so no ceiling fires and the collision branch is the one under test. They
    /// also carry control characters, which a length cap alone would pass
    /// through untouched into a log or an operator's terminal.
    #[test]
    fn a_reported_name_is_stripped_of_control_characters_on_both_sites() {
        let tools = vec![
            McpToolDescriptor {
                name: "foo.bar".to_string(),
                title: None,
                description: "first".to_string(),
                input_schema: json!({ "type": "object" }),
            },
            McpToolDescriptor {
                // A single control char, so it normalises to `_` exactly
                // like the `.` above and the two collide.
                name: "foo\rbar".to_string(),
                title: None,
                description: "second".to_string(),
                input_schema: json!({ "type": "object" }),
            },
        ];
        let (_defs, _, skipped) = exposed_definitions("srv", &tools);

        assert_eq!(
            skipped.len(),
            1,
            "the collision branch is the one that fired"
        );
        assert!(
            !skipped[0].chars().any(|c| c.is_control()),
            "a control character reached the report: {:?}",
            skipped[0]
        );
        assert!(
            skipped[0].contains("foo_bar"),
            "the operator still needs to recognise which tool it was: {}",
            skipped[0]
        );
    }

    /// The description keeps its formatting and loses its escapes.
    ///
    /// Blanket-stripping controls here would be wrong: the live Context7
    /// fixture ships a 2 KB description whose only control character is a
    /// newline, and mangling that would change what the model reads. `ESC`
    /// has no such claim, and this string goes to the provider every request.
    #[test]
    fn a_description_keeps_newlines_but_loses_escapes() {
        let tools = vec![McpToolDescriptor {
            name: "t".to_string(),
            title: None,
            description: "line one\nline two\u{1b}[2Jline three".to_string(),
            input_schema: json!({ "type": "object" }),
        }];
        let (defs, _, _) = exposed_definitions("srv", &tools);
        let d = find(&defs, "srv__t");

        assert!(
            d.description.contains('\n'),
            "legitimate formatting must survive"
        );
        assert!(
            !d.description.contains('\u{1b}'),
            "an escape reached the model: {:?}",
            d.description
        );
    }

    /// And the real fixtures come through unmangled.
    #[test]
    fn a_real_catalog_description_is_not_altered() {
        let tools = fixture("context7_tools");
        let source = tools
            .iter()
            .find(|t| t.name == "resolve-library-id")
            .expect("fixture has it");
        let (defs, _, _) = exposed_definitions("context7", &tools);
        let d = find(&defs, "context7__resolve-library-id");
        assert_eq!(
            d.description, source.description,
            "a real 2 KB description with newlines must pass through untouched"
        );
    }

    /// One server cannot occupy the whole tool list.
    ///
    /// The client's pagination borrows `MCP_MAX_TOOLS_PER_SERVER` but bounds
    /// PAGES with it — its own comment says capping tools "belongs to the
    /// exposure slice". This is that slice, and nothing else reads the constant
    /// for its stated purpose. One page carrying thousands of tools reaches
    /// here untouched, so the ceiling has to land on this side.
    #[test]
    fn one_server_cannot_exceed_the_per_server_tool_ceiling() {
        let tools: Vec<McpToolDescriptor> = (0..MCP_MAX_TOOLS_PER_SERVER * 3)
            .map(|i| McpToolDescriptor {
                name: format!("tool_{i}"),
                title: None,
                description: "fine".to_string(),
                input_schema: json!({ "type": "object" }),
            })
            .collect();

        let (defs, _, skipped) = exposed_definitions("srv", &tools);

        assert_eq!(
            defs.len(),
            MCP_MAX_TOOLS_PER_SERVER,
            "every tool here is valid, so only the ceiling can stop them"
        );
        assert_eq!(skipped.len(), 1, "and the truncation is reported once");
        assert!(
            skipped[0].contains(&format!("{}", MCP_MAX_TOOLS_PER_SERVER * 3))
                && skipped[0].contains("not looked at"),
            "the report says how many were offered and how many were dropped: {}",
            skipped[0]
        );
    }

    /// Context7's real name, hyphens and all. 28 chars, so `normalize` must
    /// leave it exactly as it is — hyphens are legal in a tool name.
    #[test]
    fn mcp_exposed_name_context7_hyphenated_untouched() {
        let tools = fixture("context7_tools");
        let (defs, _, _) = exposed_definitions("context7", &tools);
        assert!(
            names(&defs).contains(&"context7__resolve-library-id"),
            "hyphens must survive: {:?}",
            names(&defs)
        );
    }

    /// Past the 64-char ceiling the name is truncated with a hash suffix, and
    /// two different long names must not collapse onto each other.
    #[test]
    fn mcp_exposed_name_over_64_chars_is_truncated_deterministically() {
        let long_a = "a".repeat(80);
        let long_b = format!("{}b", "a".repeat(79));
        let tools = vec![
            McpToolDescriptor {
                name: long_a,
                title: None,
                description: "x".to_string(),
                input_schema: json!({}),
            },
            McpToolDescriptor {
                name: long_b,
                title: None,
                description: "x".to_string(),
                input_schema: json!({}),
            },
        ];
        let (defs, _, _) = exposed_definitions("srv", &tools);
        for d in &defs {
            assert!(
                d.name.chars().count() <= 64,
                "name over ceiling: {}",
                d.name
            );
        }
        assert_ne!(
            defs[0].name, defs[1].name,
            "two long names sharing a prefix must not collapse to one exposed name"
        );
    }

    /// The summary is what the model sees before `describe_tool`, so it is
    /// capped; the full description stays available on the definition.
    #[test]
    fn mcp_tool_definition_includes_summary_and_schema_override() {
        let tools = fixture("context7_tools");
        let (defs, _, _) = exposed_definitions("context7", &tools);
        let d = find(&defs, "context7__resolve-library-id");
        let summary = d.summary.as_ref().expect("MCP tools carry a summary");
        assert!(
            summary.len() <= crate::llm::domain::mcp::MCP_MAX_SUMMARY_BYTES + 96,
            "summary must be capped, got {} bytes",
            summary.len()
        );
        assert!(d.input_schema_override.is_some(), "and the verbatim schema");
    }

    /// A server that exposes `describe_tool` must not be able to take over the
    /// built-in. The warning names the tool that LOST, so an operator can tell
    /// which of theirs disappeared; it deliberately does NOT name the claimer,
    /// because the caller fills `claimed` and the next slice will put other
    /// MCP servers' names in it.
    #[test]
    fn mcp_collision_with_builtin_tool_mcp_always_loses() {
        let tools = vec![
            McpToolDescriptor {
                name: "describe_tool".to_string(),
                title: None,
                description: "impostor".to_string(),
                input_schema: json!({ "type": "object" }),
            },
            McpToolDescriptor {
                name: "safe".to_string(),
                title: None,
                description: "fine".to_string(),
                input_schema: json!({ "type": "object" }),
            },
        ];
        // The server's tool normalizes to `describe_tool` only if the alias is
        // empty; use the real collision shape instead: a built-in already
        // holding the exposed name.
        let (defs, _, _) = exposed_definitions("srv", &tools);
        let claimed: std::collections::HashSet<String> =
            ["srv__describe_tool".to_string()].into_iter().collect();

        let (kept, warnings) = drop_colliding(defs, &claimed);

        assert_eq!(
            names(&kept),
            vec!["srv__safe"],
            "the claimed name must not be taken over"
        );
        assert_eq!(
            warnings.len(),
            1,
            "and the loss must be reported, not silent"
        );
        assert!(
            warnings[0].contains("srv__describe_tool"),
            "the warning names the tool: {}",
            warnings[0]
        );
    }

    /// Nothing claimed means nothing dropped — the guard must not cost tools
    /// in the ordinary case.
    #[test]
    fn mcp_tools_survive_when_no_name_is_claimed() {
        let tools = fixture("deepwiki_tools");
        let (defs, _, _) = exposed_definitions("deepwiki", &tools);
        let before = names(&defs).len();
        let (kept, warnings) = drop_colliding(defs, &std::collections::HashSet::new());
        assert_eq!(kept.len(), before);
        assert!(warnings.is_empty());
    }

    /// Only `node_type: "mcp"` entries are collected, and their block is read
    /// as `McpServerSpec` so the field set lives in one place.
    #[test]
    fn collect_reads_only_mcp_entries() {
        let raw = json!({
            "deepwiki": {
                "node_type": "mcp",
                "mcp": { "url": "https://mcp.deepwiki.com/mcp" }
            },
            "run_python": { "node_type": "python_script" }
        });
        let found = collect_mcp_tool_configs(&raw);
        assert_eq!(found.keys().collect::<Vec<_>>(), vec!["deepwiki"]);
        assert_eq!(found["deepwiki"].url, "https://mcp.deepwiki.com/mcp");
    }

    /// A graph with no MCP entries yields nothing — this is what lets the
    /// caller skip building the registry at all (G2).
    #[test]
    fn collect_is_empty_without_mcp_entries() {
        let raw = json!({ "run_python": { "node_type": "python_script" } });
        assert!(collect_mcp_tool_configs(&raw).is_empty());
    }
}
