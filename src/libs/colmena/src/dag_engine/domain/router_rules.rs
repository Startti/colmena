//! Pure rules for the router's `decision_model` mode.
//!
//! Two independent rules live here, both pure (no I/O), so multiple callers
//! can share one implementation instead of drifting apart:
//!
//! - [`decision_model_rejection`]: the `fallback_branch` / reserved-name /
//!   `min_confidence` config rule. The router's runtime parser,
//!   `Graph::validate` and the linter all call this one function, so the
//!   three checks cannot drift apart.
//! - [`gate`]: the confidence gate that decides whether a decision-model
//!   answer stands or falls back.

use serde_json::Value;

/// The branch name injected into every `decision_model` choice question.
/// Reserved: no declared branch may use it (checked by
/// [`decision_model_rejection`]).
pub const NONE_OF_THESE: &str = "none_of_these";

/// Checks the `decision_model`-specific router config rules against a raw
/// node config. Returns `Some(message)` describing the first violation, or
/// `None` when the config is fine. Never reads the network or the
/// filesystem — this is checked purely from the JSON the author wrote.
pub fn decision_model_rejection(config: &Value) -> Option<String> {
    let is_decision_model = config.get("mode").and_then(|v| v.as_str()) == Some("decision_model");

    if !is_decision_model {
        if config.get("fallback_branch").is_some() {
            return Some(
                "RouterConfigError: 'fallback_branch' is only allowed in decision_model mode"
                    .to_string(),
            );
        }
        if config.get("min_confidence").is_some() {
            return Some(
                "RouterConfigError: 'min_confidence' is only allowed in decision_model mode"
                    .to_string(),
            );
        }
        return None;
    }

    let branch_names: Vec<&str> = config
        .get("branches")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|b| b.get("name").and_then(|n| n.as_str()))
                .collect()
        })
        .unwrap_or_default();

    if branch_names.contains(&NONE_OF_THESE) {
        return Some(format!(
            "RouterConfigError: branch name '{NONE_OF_THESE}' is reserved for the injected \
             none-of-these option in decision_model mode"
        ));
    }

    match config.get("fallback_branch").and_then(|v| v.as_str()) {
        None => {
            return Some(
                "RouterConfigError: decision_model mode requires 'fallback_branch'".to_string(),
            )
        }
        Some(name) if !branch_names.contains(&name) => {
            return Some(format!(
                "RouterConfigError: fallback_branch '{name}' does not name a declared branch"
            ))
        }
        _ => {}
    }

    if let Some(mc) = config.get("min_confidence") {
        let in_range = mc.as_f64().is_some_and(|v| v > 0.0 && v <= 1.0);
        if !in_range {
            return Some(
                "RouterConfigError: 'min_confidence' must be a number in (0, 1]".to_string(),
            );
        }
    }

    None
}

/// The outcome of gating a `decision_model` choice against `min_confidence`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GateOutcome {
    /// The model's own pick stands.
    Picked,
    /// Route to `fallback_branch` instead, carrying the `__decision.reason`.
    Fallback(&'static str),
}

/// Gates a decision-model `choice` against `min_confidence`.
///
/// `choice == "none_of_these"` always falls back (reason `"none_of_these"`);
/// otherwise a confidence strictly below `min_confidence` falls back (reason
/// `"low_confidence"`); exactly at the threshold passes.
pub fn gate(choice: &str, confidence: f64, min_confidence: f64) -> GateOutcome {
    if choice == NONE_OF_THESE {
        GateOutcome::Fallback("none_of_these")
    } else if confidence < min_confidence {
        GateOutcome::Fallback("low_confidence")
    } else {
        GateOutcome::Picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decision_model_rejection_table() {
        let cases: Vec<(&str, Value, Option<&str>)> = vec![
            (
                "decision_model with a valid fallback passes",
                json!({
                    "mode": "decision_model",
                    "fallback_branch": "human",
                    "branches": [{"name": "refund"}, {"name": "human"}]
                }),
                None,
            ),
            (
                "decision_model missing fallback_branch",
                json!({
                    "mode": "decision_model",
                    "branches": [{"name": "refund"}]
                }),
                Some("requires 'fallback_branch'"),
            ),
            (
                "decision_model fallback_branch not declared",
                json!({
                    "mode": "decision_model",
                    "fallback_branch": "missing",
                    "branches": [{"name": "refund"}]
                }),
                Some("does not name a declared branch"),
            ),
            (
                "decision_model reserved branch name",
                json!({
                    "mode": "decision_model",
                    "fallback_branch": "refund",
                    "branches": [{"name": "refund"}, {"name": "none_of_these"}]
                }),
                Some("reserved"),
            ),
            (
                "decision_model min_confidence too high",
                json!({
                    "mode": "decision_model",
                    "fallback_branch": "refund",
                    "min_confidence": 1.5,
                    "branches": [{"name": "refund"}]
                }),
                Some("(0, 1]"),
            ),
            (
                "decision_model min_confidence zero is rejected",
                json!({
                    "mode": "decision_model",
                    "fallback_branch": "refund",
                    "min_confidence": 0.0,
                    "branches": [{"name": "refund"}]
                }),
                Some("(0, 1]"),
            ),
            (
                "decision_model min_confidence at the upper boundary passes",
                json!({
                    "mode": "decision_model",
                    "fallback_branch": "refund",
                    "min_confidence": 1.0,
                    "branches": [{"name": "refund"}]
                }),
                None,
            ),
            (
                "llm_direct rejects fallback_branch",
                json!({
                    "mode": "llm_direct",
                    "fallback_branch": "x",
                    "branches": [{"name": "a"}]
                }),
                Some("only allowed in decision_model"),
            ),
            (
                "extract_and_route rejects min_confidence",
                json!({
                    "mode": "extract_and_route",
                    "min_confidence": 0.5,
                    "branches": [{"name": "a"}]
                }),
                Some("only allowed in decision_model"),
            ),
            (
                "llm_direct with neither field is untouched",
                json!({
                    "mode": "llm_direct",
                    "branches": [{"name": "a"}]
                }),
                None,
            ),
        ];

        for (name, config, expected) in cases {
            let result = decision_model_rejection(&config);
            match expected {
                None => assert!(
                    result.is_none(),
                    "{name}: expected no rejection, got {result:?}"
                ),
                Some(fragment) => {
                    let msg =
                        result.unwrap_or_else(|| panic!("{name}: expected a rejection message"));
                    assert!(
                        msg.contains(fragment),
                        "{name}: '{msg}' missing '{fragment}'"
                    );
                }
            }
        }
    }

    #[test]
    fn gate_table() {
        let cases = [
            (
                "confident pick routes directly",
                "refund",
                0.9,
                0.7,
                GateOutcome::Picked,
            ),
            (
                "none_of_these always falls back",
                NONE_OF_THESE,
                0.99,
                0.5,
                GateOutcome::Fallback("none_of_these"),
            ),
            (
                "low confidence falls back",
                "refund",
                0.5,
                0.7,
                GateOutcome::Fallback("low_confidence"),
            ),
            (
                "exact threshold passes",
                "refund",
                0.7,
                0.7,
                GateOutcome::Picked,
            ),
        ];
        for (name, choice, confidence, min_confidence, expected) in cases {
            assert_eq!(gate(choice, confidence, min_confidence), expected, "{name}");
        }
    }
}
