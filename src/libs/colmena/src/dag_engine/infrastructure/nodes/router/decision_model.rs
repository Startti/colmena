//! Branch selection for the router's `decision_model` mode: a decision model picks a branch
//! (or the injected `none_of_these` option) via the decision-model port,
//! gated by confidence. See design decision #6 for the wire shapes.

use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

use super::config::RouterConfig;
use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
use crate::dag_engine::domain::router_rules::{gate, GateOutcome, NONE_OF_THESE};
use crate::llm::domain::{
    Answer, ChoiceOption, DecisionModelRepository, DecisionRequest, Question, QuestionKind,
};

/// Injected `none_of_these` option criteria, shown to the model alongside
/// every declared branch's own `description`.
const NONE_CRITERIA: &str =
    include_str!("../../../../../text/prompts/router_decision_model_none.md");

const ROUTE_QUESTION_ID: &str = "route";

/// Default model; see [`crate::dag_engine::domain::router_rules::DEFAULT_DECISION_MODEL`].
pub const DEFAULT_MODEL: &str = crate::dag_engine::domain::router_rules::DEFAULT_DECISION_MODEL;

/// Default `min_confidence` when the config omits it.
pub const DEFAULT_MIN_CONFIDENCE: f64 = 0.7;

/// Builds one `DecisionRequest`, calls `repo.decide`, gates the answer, and
/// returns `(branch_index, __decision json)`. Takes the repository as a
/// trait object so tests inject `MockDecisionModelRepository` without a
/// `cfg(test)` field on `RouterNode` (design decision #8, adjusted: the
/// router builds a fresh repo per-call via `build_decision_model_repository`
/// and this function stays test-injectable on its own).
///
/// A repository error propagates as a node error — it is never routed to
/// `fallback_branch` (design decision #5): an outage must not masquerade as
/// a low-confidence decision.
pub async fn decide_branch(
    cfg: &RouterConfig,
    repo: &dyn DecisionModelRepository,
    model: Option<String>,
    fallback_branch: &str,
    min_confidence: f64,
    state: Value,
    observer: Option<Arc<dyn ExecutionObserver>>,
) -> Result<(usize, Value), Box<dyn Error + Send + Sync>> {
    let mut options: Vec<ChoiceOption> = cfg
        .branches
        .iter()
        .map(|b| ChoiceOption {
            key: b.name.clone(),
            criteria: b.description.clone(),
        })
        .collect();
    options.push(ChoiceOption {
        key: NONE_OF_THESE.to_string(),
        criteria: Some(NONE_CRITERIA.trim().to_string()),
    });

    let request = DecisionRequest {
        model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        // Structured input stays structured so the model can read its fields.
        // A bare number or boolean is not a state the vendor accepts: send it
        // as text.
        state: match state {
            Value::String(_) | Value::Object(_) | Value::Array(_) => state,
            other => Value::String(other.to_string()),
        },
        questions: vec![(
            ROUTE_QUESTION_ID.to_string(),
            Question {
                instructions: cfg.instructions.clone(),
                kind: QuestionKind::Choice { options },
            },
        )],
    };

    let response = repo.decide(request).await?;

    if let Some(obs) = &observer {
        if let Some(usage) = &response.usage {
            obs.on_event(NodeEvent::LlmUsage {
                prompt_tokens: usage.input_tokens,
                completion_tokens: usage.output_tokens,
                thinking_tokens: None,
                cache_read_tokens: None,
                cache_write_tokens: None,
            });
        }
    }

    let answer = response
        .answers
        .get(ROUTE_QUESTION_ID)
        .ok_or("Router: decision model response is missing the 'route' answer")?;
    let (choice, probabilities, confidence) = match answer {
        Answer::Choice {
            choice,
            probabilities,
            confidence,
        } => (choice.clone(), probabilities.clone(), *confidence),
        other => {
            return Err(format!(
                "Router: decision model returned an unexpected answer shape for 'route': {other:?}"
            )
            .into())
        }
    };

    let (selected_branch, reason) = match gate(&choice, confidence, min_confidence) {
        GateOutcome::Picked => (choice.clone(), "confident"),
        GateOutcome::Fallback(reason) => (fallback_branch.to_string(), reason),
    };

    let idx = cfg
        .branches
        .iter()
        .position(|b| b.name == selected_branch)
        .ok_or_else(|| {
            format!("Router: fallback_branch '{selected_branch}' does not name a declared branch")
        })?;

    let decision = json!({
        "selected_branch": selected_branch,
        "reason": reason,
        "extracted": Value::Null,
        "model_choice": choice,
        "confidence": confidence,
        "min_confidence": min_confidence,
        "probabilities": probabilities,
        "model": response.model,
    });

    Ok((idx, decision))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::nodes::router::config::BranchConfig;
    use crate::llm::domain::{
        DecisionModelError, DecisionResponse, DecisionUsage, MockDecisionModelRepository,
    };
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    fn branch(name: &str, description: &str) -> BranchConfig {
        BranchConfig {
            name: name.to_string(),
            description: Some(description.to_string()),
            when: None,
            subgraph: None,
        }
    }

    fn cfg() -> RouterConfig {
        RouterConfig {
            // decide_branch reads only the branches and the instructions.
            mode: crate::dag_engine::infrastructure::nodes::router::config::RouterMode::LlmDirect,
            branches: vec![
                branch("refund", "wants money back"),
                branch("sales", "wants to buy"),
                branch("human_review", "needs a person"),
            ],
            inline_schema: None,
            instructions: None,
        }
    }

    fn choice_response(
        choice: &str,
        confidence: f64,
        usage: Option<DecisionUsage>,
    ) -> DecisionResponse {
        DecisionResponse {
            model: "jev-1.13.0".to_string(),
            answers: BTreeMap::from([(
                "route".to_string(),
                Answer::Choice {
                    choice: choice.to_string(),
                    probabilities: BTreeMap::from([(choice.to_string(), confidence)]),
                    confidence,
                },
            )]),
            usage,
        }
    }

    #[derive(Default)]
    struct Capturing {
        events: Mutex<Vec<NodeEvent>>,
    }
    impl ExecutionObserver for Capturing {
        fn on_event(&self, event: NodeEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[tokio::test]
    async fn confident_pick_routes_directly_and_emits_usage() {
        let mut mock = MockDecisionModelRepository::new();
        mock.expect_decide().returning(|_| {
            Ok(choice_response(
                "refund",
                0.9,
                Some(DecisionUsage {
                    input_tokens: 100,
                    output_tokens: 20,
                }),
            ))
        });
        let capturing = Arc::new(Capturing::default());

        let (idx, decision) = decide_branch(
            &cfg(),
            &mock,
            None,
            "human_review",
            0.7,
            json!("me cobraron dos veces"),
            Some(capturing.clone()),
        )
        .await
        .unwrap();

        assert_eq!(idx, 0);
        assert_eq!(decision["selected_branch"], json!("refund"));
        assert_eq!(decision["reason"], json!("confident"));
        assert_eq!(decision["extracted"], Value::Null);

        let events = capturing.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            NodeEvent::LlmUsage {
                prompt_tokens,
                completion_tokens,
                ..
            } => {
                assert_eq!(*prompt_tokens, 100);
                assert_eq!(*completion_tokens, 20);
            }
            other => panic!("expected LlmUsage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fallback_table() {
        // (name, model's choice, confidence, expected __decision.reason)
        let cases = [
            (
                "none_of_these always falls back",
                NONE_OF_THESE,
                1.0,
                "none_of_these",
            ),
            ("low confidence falls back", "sales", 0.45, "low_confidence"),
        ];
        for (name, choice, confidence, expected_reason) in cases {
            let mut mock = MockDecisionModelRepository::new();
            mock.expect_decide()
                .returning(move |_| Ok(choice_response(choice, confidence, None)));

            let (idx, decision) =
                decide_branch(&cfg(), &mock, None, "human_review", 0.7, json!("x"), None)
                    .await
                    .unwrap_or_else(|e| panic!("{name}: {e}"));

            assert_eq!(idx, 2, "{name}");
            assert_eq!(decision["selected_branch"], json!("human_review"), "{name}");
            assert_eq!(decision["reason"], json!(expected_reason), "{name}");
            assert_eq!(decision["model_choice"], json!(choice), "{name}");
        }
    }

    #[tokio::test]
    async fn repository_error_fails_the_node_never_the_fallback() {
        let mut mock = MockDecisionModelRepository::new();
        mock.expect_decide()
            .returning(|_| Err(DecisionModelError::RateLimited));

        let err = decide_branch(&cfg(), &mock, None, "human_review", 0.7, json!("x"), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("rate limit"));
    }

    #[tokio::test]
    async fn request_carries_every_branch_plus_the_injected_none_option() {
        let captured: Arc<Mutex<Option<DecisionRequest>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();
        let mut mock = MockDecisionModelRepository::new();
        mock.expect_decide().returning(move |req| {
            *captured_clone.lock().unwrap() = Some(req);
            Ok(choice_response("refund", 0.9, None))
        });

        decide_branch(&cfg(), &mock, None, "human_review", 0.7, json!("x"), None)
            .await
            .unwrap();

        let req = captured.lock().unwrap().take().unwrap();
        assert_eq!(req.model, DEFAULT_MODEL);
        let QuestionKind::Choice { options } = &req.questions[0].1.kind else {
            panic!("expected a Choice question");
        };
        assert_eq!(options.len(), 4);
        assert!(options
            .iter()
            .any(|o| o.key == "refund" && o.criteria.as_deref() == Some("wants money back")));
        assert!(options
            .iter()
            .any(|o| o.key == NONE_OF_THESE && o.criteria.is_some()));
    }

    /// Structured input reaches the model as structured `state`, so it can read
    /// named fields; a bare number or boolean, which the vendor rejects, is sent
    /// as text.
    #[tokio::test]
    async fn the_input_reaches_the_model_as_json_and_scalars_as_text() {
        let cases = [
            (
                json!({ "ticket": "cobro doble", "plan": "pro" }),
                json!({ "ticket": "cobro doble", "plan": "pro" }),
            ),
            (
                json!(["hola", "me cobraron dos veces"]),
                json!(["hola", "me cobraron dos veces"]),
            ),
            (json!("texto"), json!("texto")),
            (json!(42), json!("42")),
            (json!(true), json!("true")),
        ];
        for (input, expected_state) in cases {
            let captured: Arc<Mutex<Option<DecisionRequest>>> = Arc::new(Mutex::new(None));
            let captured_clone = captured.clone();
            let mut mock = MockDecisionModelRepository::new();
            mock.expect_decide().returning(move |req| {
                *captured_clone.lock().unwrap() = Some(req);
                Ok(choice_response("refund", 0.9, None))
            });
            decide_branch(
                &cfg(),
                &mock,
                None,
                "human_review",
                0.7,
                input.clone(),
                None,
            )
            .await
            .unwrap();
            let req = captured.lock().unwrap().take().unwrap();
            assert_eq!(req.state, expected_state, "input {input}");
        }
    }
}
