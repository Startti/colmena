//! Vendor-neutral value objects for a typed decision port (noul / choice /
//! score questions), separate from the chat-shaped
//! [`super::llm_repository::LlmRepository`] port. The port itself is
//! [`super::decision_model_repository::DecisionModelRepository`]; the TypeSafe
//! Jev adapter lives in `crate::llm::infrastructure`.
//!
//! Zero infrastructure dependencies: only `std`, `serde` and `serde_json`.
//!
//! This module enforces vendor-neutral invariants only: a non-null `state`,
//! at least one question with unique ids, at least one option per `choice`
//! with unique keys, and at least one level per `score`. Vendor limits (for
//! TypeSafe Jev: at most 255 options, at most 10 levels) belong to the
//! adapter, which checks them before any network call.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::decision_model_repository::DecisionModelError;

/// One option of a `choice` question. `criteria` describes when the option
/// applies; `None` sends no description for that option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub key: String,
    pub criteria: Option<String>,
}

/// Optional guidance for a `noul` (yes/no) question: what counts as "yes"
/// and what counts as "no".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    pub when_true: Option<String>,
    pub when_false: Option<String>,
}

/// The shape of a single decision question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QuestionKind {
    /// Yes/no question; answered with the probability of "yes".
    Noul { criteria: Option<NoulCriteria> },
    /// Single-select question over a closed set of named options.
    Choice { options: Vec<ChoiceOption> },
    /// Ordered scale; `levels[0]` is the lowest level.
    Score { levels: Vec<String> },
}

impl QuestionKind {
    /// Checks the vendor-neutral invariants. Never makes a network call.
    pub fn validate(&self) -> Result<(), DecisionModelError> {
        match self {
            QuestionKind::Noul { .. } => Ok(()),
            QuestionKind::Choice { options } => {
                if options.is_empty() {
                    return Err(invalid("a choice question needs at least one option"));
                }
                let mut seen = HashSet::with_capacity(options.len());
                for option in options {
                    if !seen.insert(option.key.as_str()) {
                        return Err(invalid(&format!(
                            "a choice question has a duplicate option key '{}'",
                            option.key
                        )));
                    }
                }
                Ok(())
            }
            QuestionKind::Score { levels } => {
                if levels.is_empty() {
                    return Err(invalid("a score question needs at least one level"));
                }
                Ok(())
            }
        }
    }
}

/// A single question of a decision request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub instructions: Option<String>,
    pub kind: QuestionKind,
}

/// A decision request: the `state` the model reasons over plus named
/// questions. `questions` is an ordered list so the wire body is
/// deterministic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub model: String,
    /// A JSON string, object or array. Never `null`.
    pub state: Value,
    pub questions: Vec<(String, Question)>,
}

impl DecisionRequest {
    /// Checks the vendor-neutral invariants. Never makes a network call.
    pub fn validate(&self) -> Result<(), DecisionModelError> {
        if self.state.is_null() {
            return Err(invalid("state must not be null"));
        }
        if self.questions.is_empty() {
            return Err(invalid("a decision request needs at least one question"));
        }
        let mut ids = HashSet::with_capacity(self.questions.len());
        for (id, question) in &self.questions {
            if !ids.insert(id.as_str()) {
                return Err(invalid(&format!("duplicate question id '{id}'")));
            }
            question.kind.validate()?;
        }
        Ok(())
    }
}

/// The model's answer to a single question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Answer {
    /// Probability that the answer is "yes", in `[0, 1]`.
    Noul { probability: f64 },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    /// `score` is fractional (expected level); `probabilities` is keyed by
    /// the level index as a string (`"0"`, `"1"`, ...).
    Score {
        score: f64,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

/// Token usage of one decision call. Reported as `NodeEvent::LlmUsage`:
/// `input_tokens` as prompt tokens, `output_tokens` as completion tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// One answer per requested question, plus usage when reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    /// The model that actually answered (e.g. `jev-1.13.0`).
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    pub usage: Option<DecisionUsage>,
}

fn invalid(message: &str) -> DecisionModelError {
    DecisionModelError::InvalidInput(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn noul() -> Question {
        Question {
            instructions: Some("Is this about billing?".to_string()),
            kind: QuestionKind::Noul { criteria: None },
        }
    }

    fn choice(keys: &[&str]) -> Question {
        Question {
            instructions: None,
            kind: QuestionKind::Choice {
                options: keys
                    .iter()
                    .map(|k| ChoiceOption {
                        key: k.to_string(),
                        criteria: None,
                    })
                    .collect(),
            },
        }
    }

    fn score(levels: &[&str]) -> Question {
        Question {
            instructions: None,
            kind: QuestionKind::Score {
                levels: levels.iter().map(|l| l.to_string()).collect(),
            },
        }
    }

    fn request(state: Value, questions: Vec<(&str, Question)>) -> DecisionRequest {
        DecisionRequest {
            model: "jev-1.13.0".to_string(),
            state,
            questions: questions
                .into_iter()
                .map(|(id, q)| (id.to_string(), q))
                .collect(),
        }
    }

    #[test]
    fn valid_requests_pass() {
        let cases = vec![
            ("string state", request(json!("hola"), vec![("q", noul())])),
            (
                "object state",
                request(json!({"m": "hola"}), vec![("q", noul())]),
            ),
            (
                "array state",
                request(json!(["a", "b"]), vec![("q", noul())]),
            ),
            (
                "unique choice keys",
                request(json!("x"), vec![("q", choice(&["a", "b"]))]),
            ),
            (
                "single choice option",
                request(json!("x"), vec![("q", choice(&["only"]))]),
            ),
            (
                "one score level",
                request(json!("x"), vec![("q", score(&["only"]))]),
            ),
            (
                "mixed questions",
                request(
                    json!("x"),
                    vec![
                        ("a", noul()),
                        ("b", choice(&["x"])),
                        ("c", score(&["lo", "hi"])),
                    ],
                ),
            ),
        ];
        for (name, req) in cases {
            assert!(req.validate().is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn invalid_requests_are_rejected_before_any_call() {
        let cases = vec![
            (
                "null state",
                request(json!(null), vec![("q", noul())]),
                "null",
            ),
            (
                "no questions",
                request(json!("x"), vec![]),
                "at least one question",
            ),
            (
                "duplicate question ids",
                request(json!("x"), vec![("q", noul()), ("q", noul())]),
                "duplicate question id",
            ),
            (
                "empty choice",
                request(json!("x"), vec![("q", choice(&[]))]),
                "at least one option",
            ),
            (
                "duplicate option keys",
                request(json!("x"), vec![("q", choice(&["a", "a"]))]),
                "duplicate option key",
            ),
            (
                "empty score",
                request(json!("x"), vec![("q", score(&[]))]),
                "at least one level",
            ),
        ];
        for (name, req, expected) in cases {
            match req.validate() {
                Err(DecisionModelError::InvalidInput(msg)) => {
                    assert!(msg.contains(expected), "{name}: unexpected message '{msg}'")
                }
                other => panic!("{name}: expected InvalidInput, got {other:?}"),
            }
        }
    }
}
