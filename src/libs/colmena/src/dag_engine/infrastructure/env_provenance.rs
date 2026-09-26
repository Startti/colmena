//! Per-leaf provenance for `${VAR}` environment-variable expansion in
//! tool-dispatched node arguments.
//!
//! Env templates expand only in values the author wrote: a `${VAR}`
//! placeholder in a tool argument the model supplied is sent as written and
//! never resolved against the process environment. Operator-authored values
//! (`node_schema` `fixed`, `fixed_config`, the fixed portion of a `$DYNAMIC`
//! template) keep resolving.
//!
//! The dispatcher (`DagToolExecutor`) computes, once per tool call, the set
//! of JSON pointers into the merged tool arguments whose STRING value is
//! byte-identical to the operator-authored value at that same pointer. That
//! set travels with the call as `__colmena_env_trusted_paths`, which
//! `http_request` consults via [`EnvPolicy`]; `for_each` sends it for its
//! rows too. With no key at all — a graph edge, global state — nothing in
//! `inputs` is trusted: only a node's own `config` expands `${VAR}`.
//! Equality-at-pointer covers `node_schema`,
//! `$DYNAMIC`, and legacy `field_mapping` in one function, and naturally
//! excludes any leaf an LLM argument touched. Whole objects are never
//! trusted — only individual string leaves.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Tool-argument key carrying the trusted-pointer set for this dispatch.
/// Absent or malformed means fail closed (nothing in `inputs` is trusted).
pub const ENV_TRUSTED_PATHS_KEY: &str = "__colmena_env_trusted_paths";

/// Whether a given input pointer is allowed to expand `${VAR}` against the
/// process environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvPolicy {
    /// Only pointers in this set may expand. A missing key (graph mode: the
    /// value came over an edge or from global state) or a malformed one
    /// parses to `Restricted(HashSet::new())` — fail closed.
    Restricted(HashSet<String>),
}

impl EnvPolicy {
    /// Derive the policy from a node's raw inputs. No key, or a key that is
    /// not a JSON array of strings → `Restricted(empty)`: an input value only
    /// expands when a provenance-aware dispatcher vouched for its pointer.
    pub fn from_inputs(inputs: &crate::dag_engine::domain::node::NodeInputs) -> Self {
        let pointers = inputs
            .get(ENV_TRUSTED_PATHS_KEY)
            .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
            .unwrap_or_default();
        EnvPolicy::Restricted(pointers.into_iter().collect())
    }

    /// Whether the value at `pointer` may resolve `${VAR}` against the
    /// process environment.
    pub fn may_expand(&self, pointer: &str) -> bool {
        let EnvPolicy::Restricted(set) = self;
        set.contains(pointer)
    }
}

/// Escape one JSON Pointer (RFC 6901) reference-token segment: `~` → `~0`,
/// `/` → `~1`. Applied per path segment before joining with `/`.
/// `pub(crate)` so a node's own gating logic (e.g. `http.rs`) can build matching pointers.
pub(crate) fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// Compute the trusted pointer set: one RFC 6901 JSON pointer per STRING
/// leaf in `merged` that contains `${` and is identical to the
/// operator-authored value at the same pointer in `authored`. Recursion only
/// continues where `authored` has a value at that same key/index — an
/// LLM-introduced key has no pointer produced under it. A container is never
/// itself pushed as a trusted pointer — only its string leaves are.
pub fn trusted_pointers(
    authored: &HashMap<String, Value>,
    merged: &HashMap<String, Value>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (top_key, merged_val) in merged {
        let Some(authored_val) = authored.get(top_key) else {
            continue;
        };
        let mut pointer = format!("/{}", escape_pointer_segment(top_key));
        walk(merged_val, authored_val, &mut pointer, &mut out);
    }
    out.sort();
    out
}

fn walk(merged_val: &Value, authored_val: &Value, pointer: &mut String, out: &mut Vec<String>) {
    match merged_val {
        Value::String(s) if s.contains("${") && authored_val.as_str() == Some(s.as_str()) => {
            out.push(pointer.clone());
        }
        Value::Object(map) => {
            if let Value::Object(auth_map) = authored_val {
                for (k, v) in map {
                    if let Some(av) = auth_map.get(k) {
                        let base_len = pointer.len();
                        pointer.push('/');
                        pointer.push_str(&escape_pointer_segment(k));
                        walk(v, av, pointer, out);
                        pointer.truncate(base_len);
                    }
                }
            }
        }
        Value::Array(arr) => {
            if let Value::Array(auth_arr) = authored_val {
                for (i, v) in arr.iter().enumerate() {
                    if let Some(av) = auth_arr.get(i) {
                        let base_len = pointer.len();
                        pointer.push('/');
                        pointer.push_str(&i.to_string());
                        walk(v, av, pointer, out);
                        pointer.truncate(base_len);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Drop any pointer whose value changed between `before` and `after` — e.g.
/// secure-value injection replaced a `<value_N>` placeholder that happened
/// to sit at a trusted pointer. A decrypted secret must never be
/// re-interpreted as an env placeholder just because the placeholder it
/// replaced was operator-authored.
pub fn prune_after_secrets(trusted: Vec<String>, before: &Value, after: &Value) -> Vec<String> {
    trusted
        .into_iter()
        .filter(|p| before.pointer(p) == after.pointer(p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hm(v: Value) -> HashMap<String, Value> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn trusts_a_top_level_leaf_identical_to_authored() {
        let authored = hm(json!({ "connection_url": "${DATABASE_URL}" }));
        let merged = hm(json!({ "connection_url": "${DATABASE_URL}" }));
        let ptrs = trusted_pointers(&authored, &merged);
        assert_eq!(ptrs, vec!["/connection_url".to_string()]);
    }

    #[test]
    fn overridden_or_env_free_leaves_are_not_trusted() {
        // Overwritten (merge/templating changed it) or has no `${` at all.
        let authored = hm(json!({ "base_url": "${API_BASE}", "method": "GET" }));
        let merged = hm(json!({ "base_url": "https://evil.example", "method": "GET" }));
        assert!(trusted_pointers(&authored, &merged).is_empty());
    }

    #[test]
    fn object_is_never_trusted_as_a_whole_only_its_leaves() {
        let authored = hm(json!({
            "headers": { "Authorization": "${AMADEUS_TOKEN}", "Accept": "application/json" }
        }));
        let merged = hm(json!({
            "headers": { "Authorization": "${AMADEUS_TOKEN}", "Accept": "application/json", "X-Model": "injected" }
        }));
        let ptrs = trusted_pointers(&authored, &merged);
        // Only the `${...}` leaf identical to authored is trusted: "Accept"
        // has no `${` so it is never even a candidate, the container itself
        // is never trusted as a whole, and the LLM-introduced key is absent
        // from authored so no pointer is produced under it.
        assert_eq!(ptrs, vec!["/headers/Authorization".to_string()]);
    }

    #[test]
    fn templated_fixed_value_no_longer_equal_is_not_trusted() {
        // A fixed "/anything/${bearer_token}" templated against a declared
        // param BEFORE the merge may differ from its authored form here.
        let authored = hm(json!({ "path": "/anything/${bearer_token}" }));
        let merged = hm(json!({ "path": "/anything/secret-value-123" }));
        let ptrs = trusted_pointers(&authored, &merged);
        assert!(ptrs.is_empty());
    }

    #[test]
    fn no_key_fails_closed_and_expands_nothing() {
        // Graph mode: a value that arrived over an edge or from global state.
        let inputs: HashMap<String, Value> = hm(json!({ "q": "${DATABASE_URL}" }));
        let policy = EnvPolicy::from_inputs(&inputs);
        assert_eq!(policy, EnvPolicy::Restricted(HashSet::new()));
        assert!(!policy.may_expand("/q"));
    }

    #[test]
    fn malformed_key_fails_closed() {
        let inputs: HashMap<String, Value> = hm(json!({
            ENV_TRUSTED_PATHS_KEY: { "not": "an array of strings" }
        }));
        let policy = EnvPolicy::from_inputs(&inputs);
        assert_eq!(policy, EnvPolicy::Restricted(HashSet::new()));
        assert!(!policy.may_expand("/connection_url"));
    }

    #[test]
    fn well_formed_key_restricts_to_listed_pointers() {
        let inputs: HashMap<String, Value> = hm(json!({
            ENV_TRUSTED_PATHS_KEY: ["/connection_url"]
        }));
        let policy = EnvPolicy::from_inputs(&inputs);
        assert!(policy.may_expand("/connection_url"));
        assert!(!policy.may_expand("/q"));
    }

    #[test]
    fn prune_drops_changed_but_keeps_unchanged_pointers() {
        let before = json!({ "connection_url": "<value_3>", "other": "${DATABASE_URL}" });
        let after = json!({ "connection_url": "postgres://decrypted", "other": "${DATABASE_URL}" });
        let pruned = prune_after_secrets(
            vec!["/connection_url".to_string(), "/other".to_string()],
            &before,
            &after,
        );
        assert_eq!(pruned, vec!["/other".to_string()]);
    }
}
