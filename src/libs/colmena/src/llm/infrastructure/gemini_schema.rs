//! Conforming a foreign JSON Schema to the dialect Gemini actually accepts.
//!
//! Gemini's `function_declarations[].parameters` is not JSON Schema. It is a
//! protobuf message with a fixed field set, and a protobuf rejects every name it
//! does not declare. A single unrecognised key fails the request with HTTP 400
//! — and it fails the WHOLE request, not the offending tool: every built-in the
//! agent had goes down with one stray keyword from one server.
//!
//! That is survivable while Colmena authors every schema it sends. It stops
//! being survivable once an MCP server publishes its own: the schema is written
//! by a third party, can change between turns, and is forwarded verbatim.
//!
//! # Why an allowlist
//!
//! The two sanitisers that predate this module are denylists — they remove the
//! keys that were observed to break. Measured against the live API
//! (`gemini-2.5-flash`, 2026-09-07), **14 of 32 common JSON Schema keywords are
//! rejected**, and those denylists between them covered three. A denylist over a
//! protobuf can only ever enumerate yesterday's failures.
//!
//! An allowlist is safe here because of one measured asymmetry: a property whose
//! schema is `{}`, or which carries no `type` at all, is **accepted** (HTTP 200).
//! So dropping a key we should have kept costs a validation constraint, while
//! keeping a key we should have dropped costs the entire turn. The failure modes
//! are not comparable, and the allowlist takes the survivable one.
//!
//! # What this does NOT do
//!
//! It does not translate. A rejected keyword is dropped, never rewritten into an
//! accepted equivalent — `const: "x"` does not become `enum: ["x"]`, however
//! tempting. Rewriting means deciding that our reading of a third party's schema
//! is better than what they wrote, and a wrong guess silently changes which
//! arguments the model believes are valid. Dropping is honest: the model loses a
//! hint and the server still validates its own input.
//!
//! It also does not yet resolve `$ref`. That drop is the one that is NOT
//! graceful — see [`conform`].

use serde_json::{Map, Value};

/// Keys Gemini's `Schema` accepts, verified by probing the live API one keyword
/// at a time rather than by reading the reference.
///
/// Anything absent is dropped by [`conform`]. Notably absent, and each one a
/// confirmed 400: `$schema`, `$id`, `$ref`, `$defs`, `definitions`,
/// `additionalProperties`, `examples` (the plural — singular `example` is fine),
/// `const`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`, `uniqueItems`,
/// `deprecated`, `readOnly`, `writeOnly`, and every `x-` vendor extension.
const GEMINI_SCHEMA_KEYS: &[&str] = &[
    "type",
    "format",
    "title",
    "description",
    "nullable",
    "enum",
    "properties",
    "required",
    "items",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "minProperties",
    "maxProperties",
    "pattern",
    "default",
    "example",
    "anyOf",
    "oneOf",
    "allOf",
    "not",
    "propertyOrdering",
];

/// Positions whose value is a map of NAME to schema, so the walk has to descend
/// into the values rather than treat the map itself as a schema.
const SCHEMA_MAPS: &[&str] = &["properties"];

/// Positions whose value is a list of schemas.
const SCHEMA_LISTS: &[&str] = &["anyOf", "oneOf", "allOf"];

/// Positions whose value is one nested schema.
const SCHEMA_VALUES: &[&str] = &["items", "not"];

/// Bound on how deep the walk will go.
///
/// Not a performance guard — a defence against a hostile or merely broken remote
/// schema. Nothing stops an MCP server publishing a schema nested ten thousand
/// levels deep, and this function recurses. Real schemas are shallow; the 16
/// tools of GitHub's live catalog reach depth 3.
const MAX_DEPTH: usize = 64;

/// The same schema, carrying only what Gemini's protobuf will accept.
///
/// # Known gap: `$ref`
///
/// `$ref` is rejected by Gemini like any other unaccepted key, so it is dropped
/// — but this drop is the one that is not graceful. Every other drop costs a
/// constraint; dropping a `$ref` costs the property's ENTIRE definition, leaving
/// `{}`: a parameter the model is told nothing about, with no error anywhere to
/// say so.
///
/// That is still strictly better than today, where the same schema fails the
/// whole request, and it is deliberately not fixed here: resolving refs means
/// carrying `$defs`/`definitions` bodies to their reference sites, which is its
/// own change with its own failure modes (cycles, external URLs, missing
/// definitions). Until then, a server that publishes `$ref` gets a working turn
/// with a vaguer schema rather than no turn at all.
///
/// ```
/// use colmena::llm::infrastructure::gemini_schema::conform;
/// use serde_json::json;
///
/// let raw = json!({
///     "type": "object",
///     "properties": {
///         "owner": { "type": "string", "x-mcp-header": "X-Owner" }
///     }
/// });
///
/// let sent = conform(&raw);
/// assert!(sent["properties"]["owner"].get("x-mcp-header").is_none());
/// assert_eq!(sent["properties"]["owner"]["type"], "string");
/// ```
pub fn conform(schema: &Value) -> Value {
    filter(schema, 0)
}

/// Keep the accepted keys, and recurse into the ones whose values are schemas.
///
/// A key that is accepted but whose value is NOT a schema — `enum`, `required`,
/// `default`, `example` — is copied through untouched. Recursing into `default`
/// would strip keys from a value the model is meant to receive literally.
fn filter(schema: &Value, depth: usize) -> Value {
    let Some(map) = schema.as_object() else {
        // A boolean schema (`true` / `false`) has no protobuf equivalent. `{}`
        // is the accepted way to say "any value"; `false` ("no value is valid")
        // has no expression here and degrades to the same thing.
        if schema.is_boolean() {
            return Value::Object(Map::new());
        }
        return schema.clone();
    };
    if depth > MAX_DEPTH {
        return Value::Object(Map::new());
    }

    let mut kept = Map::new();
    for (key, value) in map {
        if !GEMINI_SCHEMA_KEYS.contains(&key.as_str()) {
            continue;
        }
        let conformed = if SCHEMA_MAPS.contains(&key.as_str()) {
            match value.as_object() {
                Some(children) => Value::Object(
                    children
                        .iter()
                        .map(|(name, child)| (name.clone(), filter(child, depth + 1)))
                        .collect(),
                ),
                None => value.clone(),
            }
        } else if SCHEMA_LISTS.contains(&key.as_str()) {
            match value.as_array() {
                Some(children) => {
                    Value::Array(children.iter().map(|c| filter(c, depth + 1)).collect())
                }
                None => value.clone(),
            }
        } else if SCHEMA_VALUES.contains(&key.as_str()) {
            filter(value, depth + 1)
        } else {
            value.clone()
        };
        kept.insert(key.clone(), conformed);
    }
    Value::Object(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The exact keys the live API was measured rejecting. Each one alone is a
    /// 400 that takes down every tool in the request, so each one alone is worth
    /// a case.
    #[test]
    fn every_measured_rejection_is_dropped() {
        for (key, value) in [
            ("$schema", json!("http://json-schema.org/draft-07/schema#")),
            ("$id", json!("https://example.com/s.json")),
            ("additionalProperties", json!(false)),
            ("examples", json!(["a"])),
            ("const", json!("c")),
            ("exclusiveMinimum", json!(1)),
            ("exclusiveMaximum", json!(5)),
            ("multipleOf", json!(2)),
            ("uniqueItems", json!(true)),
            ("deprecated", json!(false)),
            ("readOnly", json!(false)),
            ("writeOnly", json!(false)),
            ("x-mcp-header", json!("X-Foo")),
        ] {
            let mut property = Map::new();
            property.insert("type".to_string(), json!("string"));
            property.insert(key.to_string(), value);
            let raw = json!({
                "type": "object",
                "properties": { "q": Value::Object(property) }
            });

            let sent = conform(&raw);

            assert!(
                sent["properties"]["q"].get(key).is_none(),
                "{key} survived and would 400 the whole request"
            );
            assert_eq!(
                sent["properties"]["q"]["type"], "string",
                "{key} was dropped but took the property's type with it"
            );
        }
    }

    /// The live shape that started this: GitHub publishes `x-mcp-header` inside
    /// every property, never at the root, so a top-level-only filter sees a
    /// clean schema and forwards the failure.
    #[test]
    fn refuses_the_nested_vendor_extension_a_top_level_filter_misses() {
        let raw = json!({
            "type": "object",
            "properties": {
                "owner": { "type": "string", "x-mcp-header": "X-Owner" },
                "repo": { "type": "string", "x-mcp-header": "X-Repo" }
            },
            "required": ["owner", "repo"]
        });

        let sent = conform(&raw);

        assert!(sent["properties"]["owner"].get("x-mcp-header").is_none());
        assert!(sent["properties"]["repo"].get("x-mcp-header").is_none());
        assert_eq!(sent["required"], json!(["owner", "repo"]));
    }

    /// Rejection recurses in the API — a `const` under `items` 400s exactly like
    /// one at the top — so the filter has to recurse too.
    #[test]
    fn descends_into_items_and_the_composition_keywords() {
        let raw = json!({
            "type": "object",
            "properties": {
                "tags": {
                    "type": "array",
                    "items": { "type": "string", "const": "x" }
                },
                "either": {
                    "anyOf": [
                        { "type": "string", "uniqueItems": true },
                        { "type": "number", "multipleOf": 2 }
                    ]
                }
            }
        });

        let sent = conform(&raw);

        assert!(sent["properties"]["tags"]["items"].get("const").is_none());
        assert_eq!(sent["properties"]["tags"]["items"]["type"], "string");
        assert!(sent["properties"]["either"]["anyOf"][0]
            .get("uniqueItems")
            .is_none());
        assert!(sent["properties"]["either"]["anyOf"][1]
            .get("multipleOf")
            .is_none());
    }

    /// The keys that survive are the ones worth carrying: they tell the model
    /// what a valid argument looks like. A filter that kept nothing would also
    /// pass every rejection test above.
    #[test]
    fn keeps_what_the_model_needs_to_call_the_tool() {
        let raw = json!({
            "type": "object",
            "properties": {
                "page": {
                    "type": "integer",
                    "description": "Page number",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 1
                },
                "sort": { "type": "string", "enum": ["created", "updated"] }
            },
            "required": ["page"]
        });

        let sent = conform(&raw);

        assert_eq!(sent["properties"]["page"]["description"], "Page number");
        assert_eq!(sent["properties"]["page"]["minimum"], 1);
        assert_eq!(sent["properties"]["page"]["maximum"], 100);
        assert_eq!(sent["properties"]["page"]["default"], 1);
        assert_eq!(
            sent["properties"]["sort"]["enum"],
            json!(["created", "updated"])
        );
        assert_eq!(sent["required"], json!(["page"]));
    }

    /// A schema nested past the bound is truncated to `{}`, not dropped and not
    /// followed. `{}` is accepted by the API, so a hostile depth costs a
    /// parameter's detail rather than the turn.
    #[test]
    fn truncates_past_the_depth_bound_into_an_accepted_shape() {
        let mut deep = json!({ "type": "string", "const": "x" });
        for _ in 0..(MAX_DEPTH + 10) {
            deep = json!({ "type": "array", "items": deep });
        }

        let sent = conform(&deep);
        let flat = serde_json::to_string(&sent).expect("serialisable");

        assert!(
            !flat.contains("const"),
            "a key past the bound reached the provider"
        );
    }

    /// `schemars` emits `true` for an opaque field type, and a boolean has no
    /// protobuf equivalent at all.
    #[test]
    fn replaces_a_boolean_schema_with_the_empty_object() {
        let raw = json!({
            "type": "object",
            "properties": { "anything": true, "nothing": false }
        });

        let sent = conform(&raw);

        assert_eq!(sent["properties"]["anything"], json!({}));
        assert_eq!(sent["properties"]["nothing"], json!({}));
    }

    /// `default` and `example` carry a VALUE, not a schema. Recursing into them
    /// would strip keys from data the model is meant to read literally.
    #[test]
    fn leaves_a_value_carrying_key_untouched() {
        let raw = json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "object",
                    "default": { "const": "not-a-schema", "x-thing": 1 }
                }
            }
        });

        let sent = conform(&raw);

        assert_eq!(
            sent["properties"]["filter"]["default"],
            json!({ "const": "not-a-schema", "x-thing": 1 }),
            "a default value was mistaken for a schema and filtered"
        );
    }

    /// The whole schema, not just its properties: a root-level rejection is the
    /// case the previous filter DID cover, and it must not regress.
    #[test]
    fn filters_the_root_as_well_as_the_properties() {
        let raw = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "$id": "https://example.com/s.json",
            "additionalProperties": false,
            "type": "object",
            "properties": { "q": { "type": "string" } }
        });

        let sent = conform(&raw);

        assert!(sent.get("$schema").is_none());
        assert!(sent.get("$id").is_none());
        assert!(sent.get("additionalProperties").is_none());
        assert_eq!(sent["type"], "object");
    }

    /// The documented gap, pinned so it is a decision and not a surprise: a
    /// `$ref` is dropped, and the property degrades to "anything" rather than
    /// failing the request. Inlining is a separate change.
    #[test]
    fn drops_a_ref_into_an_accepted_shape_rather_than_failing_the_request() {
        let raw = json!({
            "type": "object",
            "$defs": { "Sort": { "type": "string", "enum": ["asc", "desc"] } },
            "properties": { "order": { "$ref": "#/$defs/Sort" } }
        });

        let sent = conform(&raw);

        assert!(sent.get("$defs").is_none(), "$defs would 400");
        assert_eq!(
            sent["properties"]["order"],
            json!({}),
            "the property must degrade to the accepted empty schema"
        );
    }
}
