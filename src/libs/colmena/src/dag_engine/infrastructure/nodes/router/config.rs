use super::when_dsl::WhenRule;
use crate::dag_engine::domain::child_graph_source::CHILD_GRAPH_SOURCE_KEYS;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum RouterMode {
    LlmDirect,
    ExtractAndRoute,
}

#[derive(Debug)]
pub struct BranchConfig {
    pub name: String,
    pub description: Option<String>,
    pub when: Option<WhenRule>,
    pub subgraph: Option<Value>,
}

#[derive(Debug)]
pub struct RouterConfig {
    pub mode: RouterMode,
    pub branches: Vec<BranchConfig>,
    pub inline_schema: Option<Value>,
    pub instructions: Option<String>,
}

const NAME_RE: &str = r"^[a-z][a-z0-9_]{0,63}$";

pub fn parse_and_validate(config: &Value) -> Result<RouterConfig, String> {
    let mode_str = config
        .get("mode")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "RouterConfigError: 'mode' is required".to_string())?;
    let mode = match mode_str {
        "llm_direct" => RouterMode::LlmDirect,
        "extract_and_route" => RouterMode::ExtractAndRoute,
        other => return Err(format!("RouterConfigError: invalid mode '{}'", other)),
    };

    let branches_val = config
        .get("branches")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "RouterConfigError: 'branches' must be a non-empty array".to_string())?;
    if branches_val.is_empty() {
        return Err("RouterConfigError: at least one branch required".to_string());
    }

    let name_re = regex::Regex::new(NAME_RE).unwrap();
    let mut seen_names = std::collections::HashSet::new();
    let mut branches = Vec::with_capacity(branches_val.len());

    let inline_schema = match mode {
        RouterMode::LlmDirect => None,
        RouterMode::ExtractAndRoute => {
            let s = config
                .get("schema")
                .ok_or_else(|| "RouterConfigError: extract_and_route requires schema".to_string())?
                .clone();
            super::super::util::inline_schema::inline_to_json_schema(&s)
                .map_err(|e| format!("RouterConfigError: schema invalid — {}", e))?;
            Some(s)
        }
    };

    for (idx, b) in branches_val.iter().enumerate() {
        let name = b
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("RouterConfigError: branch #{} missing 'name'", idx))?
            .to_string();
        if !name_re.is_match(&name) {
            return Err(format!("RouterConfigError: invalid branch name '{}'", name));
        }
        if !seen_names.insert(name.clone()) {
            return Err(format!(
                "RouterConfigError: duplicate branch name '{}'",
                name
            ));
        }

        let description = b
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let when_val = b.get("when");
        let subgraph = b.get("subgraph").cloned();

        if let Some(sg) = &subgraph {
            let found: Vec<&str> = CHILD_GRAPH_SOURCE_KEYS
                .iter()
                .copied()
                .filter(|k| sg.get(*k).is_some())
                .collect();
            if found.len() > 1 {
                return Err(format!(
                    "RouterConfigError: branch '{}' subgraph declares {} — pick one",
                    name,
                    found.join(" and ")
                ));
            }
            if found.is_empty() {
                return Err(format!(
                    "RouterConfigError: branch '{}' subgraph requires child_graph_path, child_graph_inline or child_graph_ref",
                    name
                ));
            }
        }

        match mode {
            RouterMode::LlmDirect => {
                if when_val.is_some() {
                    return Err(format!(
                        "RouterConfigError: 'when' not allowed in llm_direct mode (branch '{}')",
                        name
                    ));
                }
                if description.is_none() {
                    return Err(format!(
                        "RouterConfigError: llm_direct requires description per branch (branch '{}')",
                        name
                    ));
                }
                branches.push(BranchConfig {
                    name,
                    description,
                    when: None,
                    subgraph,
                });
            }
            RouterMode::ExtractAndRoute => {
                let when_val = when_val.ok_or_else(|| {
                    format!(
                        "RouterConfigError: extract_and_route requires 'when' per branch (branch '{}')",
                        name
                    )
                })?;
                let when = WhenRule::parse(when_val, inline_schema.as_ref().unwrap())
                    .map_err(|e| format!("RouterConfigError: branch '{}' — {}", name, e))?;
                branches.push(BranchConfig {
                    name,
                    description,
                    when: Some(when),
                    subgraph,
                });
            }
        }
    }

    Ok(RouterConfig {
        mode,
        branches,
        inline_schema,
        instructions: config
            .get("instructions")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_invalid_mode() {
        let cfg = json!({ "mode": "weird", "branches": [] });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("invalid mode"));
    }

    #[test]
    fn rejects_empty_branches() {
        let cfg = json!({ "mode": "llm_direct", "branches": [] });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("at least one branch"));
    }

    #[test]
    fn rejects_duplicate_branch_names() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [
                { "name": "a", "description": "x" },
                { "name": "a", "description": "y" }
            ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("duplicate branch name 'a'"));
    }

    #[test]
    fn rejects_invalid_branch_name_regex() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ { "name": "BadName", "description": "x" } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("invalid branch name 'BadName'"));
    }

    #[test]
    fn llm_direct_rejects_branch_without_description() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ { "name": "sales" } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("requires description per branch"));
    }

    #[test]
    fn llm_direct_rejects_branch_with_when() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ { "name": "sales", "description": "x", "when": { "field": "y", "equals": "z" } } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("'when' not allowed in llm_direct"));
    }

    #[test]
    fn extract_and_route_requires_schema() {
        let cfg = json!({
            "mode": "extract_and_route",
            "branches": [ { "name": "a", "when": {} } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("requires schema"));
    }

    #[test]
    fn extract_and_route_requires_when() {
        let cfg = json!({
            "mode": "extract_and_route",
            "schema": { "intent": { "type": "string", "required": true } },
            "branches": [ { "name": "a" } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("requires 'when' per branch"));
    }

    #[test]
    fn subgraph_rejects_both_path_and_inline() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ {
                "name": "a",
                "description": "x",
                "subgraph": { "child_graph_path": "p.json", "child_graph_inline": {} }
            } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("pick one"));
    }

    #[test]
    fn subgraph_rejects_neither_path_nor_inline() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ { "name": "a", "description": "x", "subgraph": {} } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("requires child_graph_path, child_graph_inline or child_graph_ref"));
    }

    #[test]
    fn a_branch_subgraph_may_name_its_child_by_reference() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ {
                "name": "a",
                "description": "x",
                "subgraph": { "child_graph_ref": { "agent_id": "x" } }
            } ]
        });
        assert!(parse_and_validate(&cfg).is_ok());
    }

    #[test]
    fn a_branch_subgraph_with_two_sources_is_rejected() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ {
                "name": "a",
                "description": "x",
                "subgraph": {
                    "child_graph_ref": { "agent_id": "x" },
                    "child_graph_inline": { "nodes": {}, "edges": [] }
                }
            } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("pick one"));
    }

    #[test]
    fn happy_path_llm_direct_three_branches() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [
                { "name": "sales",   "description": "buy" },
                { "name": "support", "description": "help" },
                { "name": "billing", "description": "money" }
            ]
        });
        let cfg = parse_and_validate(&cfg).unwrap();
        assert_eq!(cfg.mode, RouterMode::LlmDirect);
        assert_eq!(cfg.branches.len(), 3);
    }
}
