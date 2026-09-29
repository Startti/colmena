//! DAG pre-flight: validate the API keys of every LLM provider a graph will
//! statically use, before the run starts executing nodes.
//!
//! Root cause this closes: a graph with a revoked/invalid key currently fails
//! deep inside node execution (often after several turns / partial side
//! effects). Pre-flight catches it up front, cached per `(provider, key)`
//! with a short TTL so repeated ADP turns against the same conversation don't
//! re-hit the provider on every run.
//!
//! Coverage is intentionally partial — only statically-enumerable providers
//! are checked. See `enumerate_requirements` for the documented gaps.

use std::collections::BTreeSet;
use std::str::FromStr;
use std::time::Duration;

use serde_json::Value;

use crate::dag_engine::application::preflight_cache::shared_preflight_cache;
use crate::dag_engine::domain::child_graph_source::{CHILD_GRAPH_REF, CHILD_GRAPH_SOURCE_KEYS};
use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::graph::Graph;
use crate::llm::domain::{LlmError, ProviderKind};
use crate::llm::infrastructure::transient::backoff_with_jitter;
use crate::llm::infrastructure::LlmProviderFactory;

/// Node types that carry a top-level `provider` + `api_key` pair directly in
/// their `config`.
const TOP_LEVEL_PROVIDER_NODE_TYPES: &[&str] = &[
    "llm_call",
    "planner",
    "critic",
    "reactor",
    "information_extraction",
    "router",
    "image_generation",
    "image_edit",
    "tts",
];

/// Nested config keys inside an `orchestrator` node, each holding its own
/// `provider` / `api_key` pair.
const ORCHESTRATOR_ROLE_KEYS: &[&str] = &["planner", "critic", "phase_reactor", "final_reactor"];

/// How deep this pre-flight walk descends into nested `child_graph_inline`
/// definitions while enumerating provider keys.
///
/// NOT an execution limit — runtime subgraph nesting is unbounded (see
/// `SubGraphNode::depth_ceiling`). This is a stack guard for a pure JSON walk,
/// so it is set far above any plausible authored nesting: stopping early only
/// costs pre-flight coverage (the skipped branch is reported in `skipped` and
/// its provider keys go unvalidated until the run actually reaches them), and
/// with the old value of 5 an eight-deep composition silently lost that
/// validation. An inline child graph is literal JSON and therefore acyclic, so
/// the only thing this really guards against is pathologically deep input.
const MAX_SUBGRAPH_DEPTH: usize = 64;

/// One statically-enumerated `(provider, resolved api key)` pair a graph will
/// need at run time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderKeyRequirement {
    pub provider: ProviderKind,
    pub api_key: String,
}

/// Resolves a `${ENV_VAR}` placeholder in an api_key string. Local to the
/// pre-flight enumerator — mirrors the per-node `resolve_env_var` helpers
/// duplicated across `llm.rs` / `orchestrator.rs` / `planner.rs` / etc.
/// (deduplicating those copies is a separate finding, out of scope here).
fn resolve_env_var(value: &str) -> Result<String, String> {
    match value.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        Some(var_name) => std::env::var(var_name)
            .map_err(|_| format!("Environment variable {} not found", var_name)),
        None => Ok(value.to_string()),
    }
}

/// Reads a `{provider, api_key}` pair out of a JSON config object, resolving
/// `${ENV_VAR}` placeholders. Returns `None` when the pair can't be
/// statically resolved (missing fields, unknown provider string, unresolved
/// env var, empty key) — those cases degrade to normal runtime failure.
fn extract_requirement(config: &Value) -> Option<ProviderKeyRequirement> {
    let provider_str = config.get("provider")?.as_str()?;
    let api_key_raw = config.get("api_key")?.as_str()?;

    let provider = ProviderKind::from_str(provider_str).ok()?;
    let api_key = resolve_env_var(api_key_raw).ok()?;
    if api_key.trim().is_empty() {
        return None;
    }
    Some(ProviderKeyRequirement { provider, api_key })
}

fn push_unique(reqs: &mut Vec<ProviderKeyRequirement>, req: ProviderKeyRequirement) {
    if !reqs.contains(&req) {
        reqs.push(req);
    }
}

/// Walk one node's `config` and fold any statically-resolvable provider
/// requirement(s) into `reqs`. Records coverage notes into `covered`/`skipped`
/// for the diagnostic debug line emitted by `enumerate_requirements`.
fn collect_from_node(
    node_type: &str,
    config: &Value,
    depth: usize,
    reqs: &mut Vec<ProviderKeyRequirement>,
    covered: &mut BTreeSet<String>,
    skipped: &mut Vec<String>,
) {
    if TOP_LEVEL_PROVIDER_NODE_TYPES.contains(&node_type) {
        match extract_requirement(config) {
            Some(req) => {
                covered.insert(format!("{node_type}:{}", req.provider));
                push_unique(reqs, req);
            }
            None if config.get("provider").is_some() => {
                // A `provider` literal is present in config but couldn't be
                // statically resolved (unknown kind, unresolved ${ENV_VAR},
                // empty key). Not an error here — degrades to the node's own
                // runtime validation.
                skipped.push(format!(
                    "{node_type}: config.provider present but not statically resolvable"
                ));
            }
            None => {
                // No `config.provider` at all — most likely edge-fed
                // (`inputs.get("provider")`), which is only known once the
                // graph actually runs. Intentionally not covered.
                skipped.push(format!(
                    "{node_type}: provider not in static config (edge-fed?)"
                ));
            }
        }
        return;
    }

    if node_type == "orchestrator" {
        if let Some(obj) = config.as_object() {
            for role in ORCHESTRATOR_ROLE_KEYS {
                let Some(role_cfg) = obj.get(*role) else {
                    continue;
                };
                match extract_requirement(role_cfg) {
                    Some(req) => {
                        covered.insert(format!("orchestrator.{role}:{}", req.provider));
                        push_unique(reqs, req);
                    }
                    None if role_cfg.get("provider").is_some() => {
                        skipped.push(format!(
                            "orchestrator.{role}: provider present but not statically resolvable"
                        ));
                    }
                    None => {}
                }
            }
        }
        return;
    }

    if node_type == "subgraph" {
        if !CHILD_GRAPH_SOURCE_KEYS
            .iter()
            .any(|k| config.get(*k).is_some())
        {
            // No static field set — most likely subgraph-as-tool, where the
            // LLM supplies the child graph source at call time. Dynamic,
            // unknowable pre-run — intentionally not covered.
            skipped.push("subgraph: no static child graph source (subgraph-as-tool?)".to_string());
            return;
        }

        if config.get(CHILD_GRAPH_REF).is_some() {
            // Resolved through `ChildGraphResolverPort` at run time — the
            // referenced graph isn't known statically, so its providers can't
            // be enumerated here either.
            skipped.push(
                "subgraph.child_graph_ref: resolved at run time — checked when the subgraph actually runs"
                    .to_string(),
            );
            return;
        }

        if let Some(path_val) = config.get("child_graph_path") {
            skipped.push(format!(
                "subgraph.child_graph_path ({path_val}): file recursion deferred — checked when the subgraph actually runs"
            ));
        }

        let Some(inline) = config.get("child_graph_inline") else {
            return;
        };

        if depth >= MAX_SUBGRAPH_DEPTH {
            skipped.push(format!(
                "subgraph.child_graph_inline: max recursion depth {MAX_SUBGRAPH_DEPTH} reached, not descending further"
            ));
            return;
        }

        match serde_json::from_value::<Graph>(inline.clone()) {
            Ok(child_graph) => {
                collect_from_graph(&child_graph, depth + 1, reqs, covered, skipped);
            }
            Err(_) => {
                skipped.push(
                    "subgraph.child_graph_inline: not a well-formed embedded graph, skipped"
                        .to_string(),
                );
            }
        }
    }
}

fn collect_from_graph(
    graph: &Graph,
    depth: usize,
    reqs: &mut Vec<ProviderKeyRequirement>,
    covered: &mut BTreeSet<String>,
    skipped: &mut Vec<String>,
) {
    for node in graph.nodes.values() {
        collect_from_node(&node.node_type, &node.config, depth, reqs, covered, skipped);
    }
}

/// Statically enumerate the distinct `(provider, resolved api key)` pairs a
/// graph will use, by walking every node's `config` (recursing into
/// `subgraph` → `child_graph_inline`, depth-bounded).
///
/// **Documented, intentional gaps** (degrade to normal runtime failure —
/// safe, never silent, always logged):
/// - Providers wired via an incoming edge (`inputs.get("provider")`) —
///   dynamic, unknowable before the graph runs.
/// - `subgraph` used **as a tool**, where the LLM supplies
///   `child_graph_inline`/`path` at call time — same reason.
/// - `child_graph_path` (file) recursion — deferred, to avoid per-run file
///   I/O at depth up to 5; those providers are checked when the subgraph
///   actually executes.
///
/// Emits a `tracing::debug!` listing exactly what was covered vs. skipped, so
/// this is never mistaken for full coverage.
pub fn enumerate_requirements(graph: &Graph) -> Vec<ProviderKeyRequirement> {
    let mut reqs = Vec::new();
    let mut covered = BTreeSet::new();
    let mut skipped = Vec::new();
    collect_from_graph(graph, 0, &mut reqs, &mut covered, &mut skipped);

    tracing::debug!(
        target: "colmena::preflight",
        covered = ?covered,
        skipped = ?skipped,
        "pre-flight provider enumeration (partial coverage by design — edge-fed providers, \
         subgraph-as-tool dynamic children, and child_graph_path file recursion are not \
         statically enumerable and degrade to normal runtime node failure)"
    );

    reqs
}

/// How many times pre-flight asks a provider about a key before it gives up
/// on a verdict. Only a transient answer (see `LlmError::is_transient`) is
/// asked again.
const CHECK_ATTEMPTS: u32 = 3;
const CHECK_BACKOFF_BASE: Duration = Duration::from_millis(300);
const CHECK_BACKOFF_CAP: Duration = Duration::from_secs(2);

/// What one key check found.
#[derive(Debug, PartialEq)]
enum KeyCheck {
    Valid,
    /// The provider refused the key: the run must not start.
    Rejected(String),
    /// The provider could not answer (a 408, 429 or 5xx, or the network). This
    /// says nothing about the key: the run goes on, and the node reports what
    /// the provider says when it calls it.
    Inconclusive(String),
}

fn classify(result: Result<(), LlmError>) -> KeyCheck {
    match result {
        Ok(()) => KeyCheck::Valid,
        Err(e) if e.is_transient() => KeyCheck::Inconclusive(e.to_string()),
        Err(e) => KeyCheck::Rejected(e.to_string()),
    }
}

/// Asks the provider about one key, and again (exponential backoff with full
/// jitter) while the answer is transient, up to `CHECK_ATTEMPTS` times.
async fn check_key(req: &ProviderKeyRequirement) -> KeyCheck {
    let repo = LlmProviderFactory::create(req.provider.clone());
    let mut attempt = 1;
    loop {
        let check = classify(repo.validate_credentials(&req.api_key).await);
        if !matches!(check, KeyCheck::Inconclusive(_)) || attempt >= CHECK_ATTEMPTS {
            return check;
        }
        tokio::time::sleep(backoff_with_jitter(
            attempt,
            CHECK_BACKOFF_BASE,
            CHECK_BACKOFF_CAP,
        ))
        .await;
        attempt += 1;
    }
}

/// Validate the API keys of every statically-enumerable provider this graph
/// will use. Blocking: aborts (returns `Err`) on the first key a provider
/// rejects; a provider that cannot answer does not block (see `check_key`).
/// Verdicts are cached per `(provider, key)` with a TTL, so repeated entries
/// into the same graph (fresh run, resume, subgraph re-entry) don't re-hit
/// providers.
///
/// Disabled entirely via `COLMENA_PREFLIGHT_HEALTH=off` (safety valve).
pub async fn validate_graph_providers(graph: &Graph) -> Result<(), DagError> {
    if std::env::var("COLMENA_PREFLIGHT_HEALTH").ok().as_deref() == Some("off") {
        tracing::debug!(
            target: "colmena::preflight",
            "pre-flight health check disabled via COLMENA_PREFLIGHT_HEALTH=off"
        );
        return Ok(());
    }

    let requirements = enumerate_requirements(graph);
    if requirements.is_empty() {
        return Ok(());
    }

    let cache = shared_preflight_cache().await;

    for req in requirements {
        let outcome = match cache.get_fresh(&req.provider, &req.api_key) {
            Some(cached) => cached,
            None => match check_key(&req).await {
                KeyCheck::Valid => {
                    cache.put(&req.provider, &req.api_key, Ok(()));
                    Ok(())
                }
                KeyCheck::Rejected(reason) => {
                    cache.put(&req.provider, &req.api_key, Err(reason.clone()));
                    Err(reason)
                }
                // Neither cached nor blocking: a provider that cannot answer
                // now may answer on the next run, and the node's own call
                // reports what it says.
                KeyCheck::Inconclusive(reason) => {
                    tracing::warn!(
                        target: "colmena::preflight",
                        provider = %req.provider,
                        reason = %reason,
                        "pre-flight got no verdict on the API key; the run goes on"
                    );
                    Ok(())
                }
            },
        };

        if let Err(reason) = outcome {
            return Err(DagError::NodeExecution(format!(
                "Pre-flight: provider {} rejected the API key: {}",
                req.provider, reason
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod enumerate_requirements_tests {
    use super::*;
    use serde_json::json;

    fn graph_from(json: Value) -> Graph {
        serde_json::from_value(json).expect("test fixture must parse as a Graph")
    }

    #[test]
    fn top_level_llm_call_is_covered() {
        let g = graph_from(json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": { "provider": "openai", "api_key": "sk-test-1" }
                }
            },
            "edges": []
        }));

        let reqs = enumerate_requirements(&g);
        assert_eq!(
            reqs,
            vec![ProviderKeyRequirement {
                provider: ProviderKind::OpenAi,
                api_key: "sk-test-1".to_string(),
            }]
        );
    }

    #[test]
    fn distinct_node_types_all_covered_and_deduped() {
        let g = graph_from(json!({
            "nodes": {
                "planner": { "type": "planner", "config": { "provider": "google", "api_key": "gk-1" } },
                "critic": { "type": "critic", "config": { "provider": "anthropic", "api_key": "ak-1" } },
                "reactor": { "type": "reactor", "config": { "provider": "google", "api_key": "gk-1" } },
                "extract": { "type": "information_extraction", "config": { "provider": "openai", "api_key": "sk-1" } },
                "route": { "type": "router", "config": { "provider": "google", "api_key": "gk-2" } },
                "img": { "type": "image_generation", "config": { "provider": "openai", "api_key": "sk-1" } }
            },
            "edges": []
        }));

        let reqs = enumerate_requirements(&g);
        // 4 distinct pairs: (google, gk-1) deduped from planner+reactor,
        // (anthropic, ak-1), (openai, sk-1) deduped from extract+img, (google, gk-2).
        assert_eq!(reqs.len(), 4);
        assert!(reqs.contains(&ProviderKeyRequirement {
            provider: ProviderKind::Google,
            api_key: "gk-1".to_string(),
        }));
        assert!(reqs.contains(&ProviderKeyRequirement {
            provider: ProviderKind::Anthropic,
            api_key: "ak-1".to_string(),
        }));
        assert!(reqs.contains(&ProviderKeyRequirement {
            provider: ProviderKind::OpenAi,
            api_key: "sk-1".to_string(),
        }));
        assert!(reqs.contains(&ProviderKeyRequirement {
            provider: ProviderKind::Google,
            api_key: "gk-2".to_string(),
        }));
    }

    #[test]
    fn orchestrator_nested_roles_are_covered() {
        let g = graph_from(json!({
            "nodes": {
                "orch": {
                    "type": "orchestrator",
                    "config": {
                        "planner": { "provider": "openai", "api_key": "sk-planner" },
                        "critic": { "provider": "anthropic", "api_key": "ak-critic" },
                        "phase_reactor": { "provider": "google", "api_key": "gk-phase" },
                        "final_reactor": { "provider": "google", "api_key": "gk-final" }
                    }
                }
            },
            "edges": []
        }));

        let reqs = enumerate_requirements(&g);
        assert_eq!(reqs.len(), 4);
        assert!(reqs.contains(&ProviderKeyRequirement {
            provider: ProviderKind::OpenAi,
            api_key: "sk-planner".to_string(),
        }));
        assert!(reqs.contains(&ProviderKeyRequirement {
            provider: ProviderKind::Anthropic,
            api_key: "ak-critic".to_string(),
        }));
        assert!(reqs.contains(&ProviderKeyRequirement {
            provider: ProviderKind::Google,
            api_key: "gk-phase".to_string(),
        }));
        assert!(reqs.contains(&ProviderKeyRequirement {
            provider: ProviderKind::Google,
            api_key: "gk-final".to_string(),
        }));
    }

    #[test]
    fn env_var_placeholder_is_resolved() {
        std::env::set_var("COLMENA_PREFLIGHT_TEST_KEY", "resolved-value");
        let g = graph_from(json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": { "provider": "openai", "api_key": "${COLMENA_PREFLIGHT_TEST_KEY}" }
                }
            },
            "edges": []
        }));

        let reqs = enumerate_requirements(&g);
        std::env::remove_var("COLMENA_PREFLIGHT_TEST_KEY");

        assert_eq!(
            reqs,
            vec![ProviderKeyRequirement {
                provider: ProviderKind::OpenAi,
                api_key: "resolved-value".to_string(),
            }]
        );
    }

    #[test]
    fn unresolved_env_var_is_skipped_not_errored() {
        // Ensure the var really is unset for this test.
        std::env::remove_var("COLMENA_PREFLIGHT_MISSING_VAR_XYZ");
        let g = graph_from(json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": { "provider": "openai", "api_key": "${COLMENA_PREFLIGHT_MISSING_VAR_XYZ}" }
                }
            },
            "edges": []
        }));

        let reqs = enumerate_requirements(&g);
        assert!(reqs.is_empty());
    }

    #[test]
    fn edge_fed_provider_is_not_covered() {
        // No `config.provider` at all — the node reads it from `inputs` at
        // runtime (an incoming edge). Must NOT be enumerated.
        let g = graph_from(json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": { "api_key": "sk-test" }
                }
            },
            "edges": []
        }));

        let reqs = enumerate_requirements(&g);
        assert!(reqs.is_empty());
    }

    #[test]
    fn unknown_provider_string_is_skipped() {
        let g = graph_from(json!({
            "nodes": {
                "tts_node": {
                    "type": "tts",
                    "config": { "provider": "elevenlabs", "api_key": "el-key" }
                }
            },
            "edges": []
        }));

        // "elevenlabs" is not a ProviderKind understood by LlmProviderFactory
        // (tts uses a separate TtsRepository dispatch) — must be skipped.
        let reqs = enumerate_requirements(&g);
        assert!(reqs.is_empty());
    }

    #[test]
    fn gemini_provider_string_is_rejected_not_normalized() {
        // ProviderKind::from_str intentionally rejects "gemini" (only
        // "google" is valid) — the enumerator must not silently normalize it.
        let g = graph_from(json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": { "provider": "gemini", "api_key": "sk-test" }
                }
            },
            "edges": []
        }));

        let reqs = enumerate_requirements(&g);
        assert!(reqs.is_empty());
    }

    #[test]
    fn subgraph_child_graph_inline_is_recursed() {
        let g = graph_from(json!({
            "nodes": {
                "sub": {
                    "type": "subgraph",
                    "config": {
                        "child_graph_inline": {
                            "nodes": {
                                "inner_agent": {
                                    "type": "llm_call",
                                    "config": { "provider": "anthropic", "api_key": "ak-inner" }
                                }
                            },
                            "edges": []
                        }
                    }
                }
            },
            "edges": []
        }));

        let reqs = enumerate_requirements(&g);
        assert_eq!(
            reqs,
            vec![ProviderKeyRequirement {
                provider: ProviderKind::Anthropic,
                api_key: "ak-inner".to_string(),
            }]
        );
    }

    #[test]
    fn subgraph_child_graph_path_is_not_covered() {
        // File recursion is deferred by design — must not attempt file I/O
        // and must not appear in the covered set.
        let g = graph_from(json!({
            "nodes": {
                "sub": {
                    "type": "subgraph",
                    "config": { "child_graph_path": "./nonexistent_child.json" }
                }
            },
            "edges": []
        }));

        let reqs = enumerate_requirements(&g);
        assert!(reqs.is_empty());
    }

    #[test]
    fn subgraph_as_tool_with_no_static_child_graph_is_not_covered() {
        // subgraph-as-tool: no child_graph_inline/path in static config at
        // all (the LLM supplies it at call time via tool_configurations).
        let g = graph_from(json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": {
                        "provider": "openai",
                        "api_key": "sk-outer",
                        "tool_configurations": {
                            "delegate": { "node_type": "subgraph" }
                        }
                    }
                }
            },
            "edges": []
        }));

        // Only the outer llm_call's own provider is covered; the nested
        // subgraph-as-tool config isn't walked as a node at all (it's a tool
        // definition, not a graph node) — confirms it can't leak in.
        let reqs = enumerate_requirements(&g);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].api_key, "sk-outer");
    }

    #[test]
    fn subgraph_recursion_depth_is_bounded() {
        // Build a chain of `MAX_SUBGRAPH_DEPTH + 2` nested subgraphs, each
        // wrapping the next via child_graph_inline, with a provider only at
        // the innermost (unreachable) level. The walk must stop at
        // MAX_SUBGRAPH_DEPTH and not recurse infinitely or panic.
        let mut innermost = json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": { "provider": "openai", "api_key": "sk-too-deep" }
                }
            },
            "edges": []
        });

        for _ in 0..(MAX_SUBGRAPH_DEPTH + 2) {
            innermost = json!({
                "nodes": {
                    "sub": {
                        "type": "subgraph",
                        "config": { "child_graph_inline": innermost }
                    }
                },
                "edges": []
            });
        }

        let g = graph_from(innermost);
        let reqs = enumerate_requirements(&g);
        assert!(
            reqs.is_empty(),
            "provider past the max recursion depth must not be enumerated"
        );
    }

    #[test]
    fn empty_graph_yields_no_requirements() {
        let g = graph_from(json!({ "nodes": {}, "edges": [] }));
        assert!(enumerate_requirements(&g).is_empty());
    }
}

#[cfg(test)]
mod validate_graph_providers_tests {
    use super::*;
    use crate::llm::domain::{
        LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, MockLlmRepository,
    };
    use crate::llm::infrastructure::OverrideGuard;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;

    /// Stub `LlmRepository` that overrides only `validate_credentials`,
    /// installed via `LlmProviderFactory`'s process-global test override so
    /// `validate_graph_providers` exercises the real enumerate → cache →
    /// factory → validate → abort decision without hitting a real provider.
    struct StubRepo {
        accept: bool,
    }

    #[async_trait]
    impl LlmRepository for StubRepo {
        async fn call(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
            unimplemented!("not exercised by pre-flight tests")
        }

        async fn stream(&self, _request: LlmRequest) -> Result<LlmStream, LlmError> {
            unimplemented!("not exercised by pre-flight tests")
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }

        async fn validate_credentials(&self, _api_key: &str) -> Result<(), LlmError> {
            if self.accept {
                Ok(())
            } else {
                Err(LlmError::InvalidApiKey)
            }
        }

        fn provider_name(&self) -> &'static str {
            "stub"
        }
    }

    fn graph_with_llm_call(api_key: &str) -> Graph {
        let json = json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": { "provider": "openai", "api_key": api_key }
                }
            },
            "edges": []
        });
        serde_json::from_value(json).unwrap()
    }

    #[derive(Clone, Copy)]
    enum Answer {
        Valid,
        Invalid,
        Unavailable(u16),
    }

    /// A provider asked exactly `answers.len()` times, answering in order
    /// (mockall panics on an extra call and, when dropped, on a missing one).
    fn scripted(answers: &[Answer]) -> Arc<MockLlmRepository> {
        let mut mock = MockLlmRepository::new();
        let mut seq = mockall::Sequence::new();
        for answer in answers.iter().copied() {
            mock.expect_validate_credentials()
                .times(1)
                .in_sequence(&mut seq)
                .returning(move |_| match answer {
                    Answer::Valid => Ok(()),
                    Answer::Invalid => Err(LlmError::InvalidApiKey),
                    Answer::Unavailable(status) => Err(LlmError::provider_unavailable(status)),
                });
        }
        Arc::new(mock)
    }

    #[test]
    fn classify_splits_verdicts_from_transient_answers() {
        assert_eq!(classify(Ok(())), KeyCheck::Valid);
        let rejected = |e| matches!(classify(Err(e)), KeyCheck::Rejected(_));
        let inconclusive = |e| matches!(classify(Err(e)), KeyCheck::Inconclusive(_));
        assert!(rejected(LlmError::InvalidApiKey));
        assert!(rejected(LlmError::request_failed("status 400")));
        assert!(inconclusive(LlmError::provider_unavailable(503)));
        assert!(inconclusive(LlmError::network_error("reset")));
    }

    #[tokio::test(start_paused = true)]
    async fn a_transient_answer_is_asked_again_and_the_later_verdict_counts() {
        for status in [503, 429] {
            let _guard =
                OverrideGuard::install(scripted(&[Answer::Unavailable(status), Answer::Valid]));
            let graph = graph_with_llm_call(&format!("preflight-unit-test-{status}-then-valid"));
            assert!(validate_graph_providers(&graph).await.is_ok());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_provider_that_stays_unavailable_neither_blocks_the_run_nor_is_cached() {
        // Three attempts per run; nothing cached, so the second run asks again.
        let _guard = OverrideGuard::install(scripted(&[Answer::Unavailable(503); 6]));
        let graph = graph_with_llm_call("preflight-unit-test-503-always");
        assert!(validate_graph_providers(&graph).await.is_ok());
        assert!(validate_graph_providers(&graph).await.is_ok());
    }

    #[tokio::test]
    async fn a_rejected_key_is_not_asked_again_and_stays_cached() {
        let _guard = OverrideGuard::install(scripted(&[Answer::Invalid]));
        let graph = graph_with_llm_call("preflight-unit-test-invalid-once");
        assert!(validate_graph_providers(&graph).await.is_err());
        assert!(validate_graph_providers(&graph).await.is_err());
    }

    #[tokio::test]
    async fn valid_key_passes_preflight() {
        let _guard = OverrideGuard::install(Arc::new(StubRepo { accept: true }));
        let graph = graph_with_llm_call("preflight-unit-test-valid-key");
        let result = validate_graph_providers(&graph).await;
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
    }

    #[tokio::test]
    async fn invalid_key_aborts_preflight() {
        let _guard = OverrideGuard::install(Arc::new(StubRepo { accept: false }));
        let graph = graph_with_llm_call("preflight-unit-test-invalid-key");
        let err = validate_graph_providers(&graph).await.unwrap_err();
        match err {
            DagError::NodeExecution(msg) => {
                assert!(msg.contains("Pre-flight"), "got: {msg}");
                assert!(msg.contains("openai"), "got: {msg}");
            }
            other => panic!("expected DagError::NodeExecution, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn kill_switch_skips_validation_even_with_invalid_key() {
        let _guard = OverrideGuard::install(Arc::new(StubRepo { accept: false }));
        std::env::set_var("COLMENA_PREFLIGHT_HEALTH", "off");
        let graph = graph_with_llm_call("preflight-unit-test-killswitch-key");
        let result = validate_graph_providers(&graph).await;
        std::env::remove_var("COLMENA_PREFLIGHT_HEALTH");

        assert!(result.is_ok(), "expected Ok (skipped), got {:?}", result);
    }

    #[tokio::test]
    async fn graph_with_no_enumerable_providers_is_a_no_op() {
        let g: Graph = serde_json::from_value(json!({ "nodes": {}, "edges": [] })).unwrap();
        let result = validate_graph_providers(&g).await;
        assert!(result.is_ok());
    }
}
