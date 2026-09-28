//! [`DecisionModelRepository`] adapter for TypeSafe Jev (`POST /v1/systemone`).
//!
//! Follows `web/infrastructure/tavily_adapter.rs`: one client from
//! `shared::http_client::builder()`, a 10 s timeout, and no retry, so a vendor
//! error fails the call. Vendor limits (255 choice options, 10 score levels) are
//! checked before any request.
//!
//! Wire shape, verified against the live API:
//! - request `{"state", "model", "questions": {id: {"type", "instructions"?, "criteria"?}}}`;
//!   noul `criteria` is `{"true"?, "false"?}`, choice `criteria` maps each option key
//!   to its description (or `null`), score `criteria` is the ordered level list;
//! - response `{"model", "answers": {id: {"type", ...}}, "usage"?}`.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::llm::domain::decision_model::{
    Answer, DecisionRequest, DecisionResponse, DecisionUsage, Question, QuestionKind,
};
use crate::llm::domain::decision_model_repository::{DecisionModelError, DecisionModelRepository};

const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CHOICE_OPTIONS: usize = 255;
const MAX_SCORE_LEVELS: usize = 10;
const MAX_ERROR_BODY_CHARS: usize = 512;

pub struct TypesafeJevAdapter {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl TypesafeJevAdapter {
    /// Fails with [`DecisionModelError::Configuration`], naming
    /// `TYPESAFE_API_KEY`, when `api_key` is empty or blank.
    pub fn new(api_key: impl Into<String>) -> Result<Self, DecisionModelError> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(DecisionModelError::Configuration(
                "TypeSafe Jev needs a non-empty api_key (e.g. \"${TYPESAFE_API_KEY}\")".to_string(),
            ));
        }
        Ok(Self {
            client: client(REQUEST_TIMEOUT)?,
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
        })
    }

    #[cfg(test)]
    fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.to_string();
        self
    }

    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.client = client(timeout).expect("test client");
        self
    }
}

fn client(timeout: Duration) -> Result<reqwest::Client, DecisionModelError> {
    crate::shared::http_client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| DecisionModelError::Configuration(format!("http client: {e}")))
}

fn check_vendor_limits(request: &DecisionRequest) -> Result<(), DecisionModelError> {
    for (id, question) in &request.questions {
        let (count, max, what) = match &question.kind {
            QuestionKind::Choice { options } => (options.len(), MAX_CHOICE_OPTIONS, "options"),
            QuestionKind::Score { levels } => (levels.len(), MAX_SCORE_LEVELS, "levels"),
            QuestionKind::Noul { .. } => continue,
        };
        if count > max {
            return Err(DecisionModelError::InvalidInput(format!(
                "question '{id}' has {count} {what}; TypeSafe Jev allows at most {max}"
            )));
        }
    }
    Ok(())
}

fn request_body(request: &DecisionRequest) -> Value {
    let questions: Map<String, Value> = request
        .questions
        .iter()
        .map(|(id, q)| (id.clone(), question_body(q)))
        .collect();
    json!({ "state": request.state, "model": request.model, "questions": questions })
}

fn question_body(question: &Question) -> Value {
    let (kind, criteria) = match &question.kind {
        QuestionKind::Noul { criteria } => (
            "noul",
            criteria.as_ref().map(|c| {
                let mut sides = Map::new();
                if let Some(t) = &c.when_true {
                    sides.insert("true".to_string(), json!(t));
                }
                if let Some(f) = &c.when_false {
                    sides.insert("false".to_string(), json!(f));
                }
                Value::Object(sides)
            }),
        ),
        QuestionKind::Choice { options } => (
            "choice",
            Some(Value::Object(
                options
                    .iter()
                    .map(|o| (o.key.clone(), json!(o.criteria)))
                    .collect(),
            )),
        ),
        QuestionKind::Score { levels } => ("score", Some(json!(levels))),
    };
    let mut body = Map::new();
    body.insert("type".to_string(), json!(kind));
    if let Some(instructions) = &question.instructions {
        body.insert("instructions".to_string(), json!(instructions));
    }
    if let Some(criteria) = criteria {
        body.insert("criteria".to_string(), criteria);
    }
    Value::Object(body)
}

#[derive(Deserialize)]
struct WireResponse {
    model: String,
    answers: BTreeMap<String, WireAnswer>,
    usage: Option<DecisionUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WireAnswer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

fn parse_response(
    request: &DecisionRequest,
    body: &str,
) -> Result<DecisionResponse, DecisionModelError> {
    let malformed = DecisionModelError::MalformedResponse;
    let mut wire: WireResponse = serde_json::from_str(body)
        .map_err(|e| malformed(format!("unexpected response body: {e}")))?;
    let mut answers = BTreeMap::new();
    for (id, question) in &request.questions {
        let answer = match wire.answers.remove(id) {
            None => return Err(malformed(format!("no answer for question '{id}'"))),
            Some(WireAnswer::Noul { noul }) => Answer::Noul { probability: noul },
            Some(WireAnswer::Choice {
                choice,
                probabilities,
                confidence,
            }) => {
                let offered = matches!(&question.kind,
                    QuestionKind::Choice { options } if options.iter().any(|o| o.key == choice));
                if !offered {
                    return Err(malformed(format!(
                        "answer '{id}' picked '{choice}', which was not offered"
                    )));
                }
                Answer::Choice {
                    choice,
                    probabilities,
                    confidence,
                }
            }
            Some(WireAnswer::Score {
                score,
                probabilities,
                confidence,
            }) => Answer::Score {
                score,
                probabilities,
                confidence,
            },
        };
        answers.insert(id.clone(), answer);
    }
    Ok(DecisionResponse {
        model: wire.model,
        answers,
        usage: wire.usage,
    })
}

/// Maps a non-2xx response. 400/422 bodies come in three shapes:
/// `{detail: {error_type, message}}`, `{detail: "text"}` and
/// `{detail: [{loc, msg}, ...]}`.
fn map_error(status: u16, body: &str) -> DecisionModelError {
    let truncated: String = body.chars().take(MAX_ERROR_BODY_CHARS).collect();
    match status {
        401 | 403 => DecisionModelError::Auth(truncated),
        429 => DecisionModelError::RateLimited,
        400 | 422 => {
            let detail = serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|v| v.get("detail").cloned());
            let (error_type, message) = match detail {
                Some(Value::Object(d)) => (
                    d.get("error_type")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    d.get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ),
                Some(Value::String(text)) => (None, text),
                Some(Value::Array(items)) => (
                    None,
                    items
                        .iter()
                        .map(validation_entry)
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
                _ => (None, truncated),
            };
            DecisionModelError::InvalidRequest {
                status,
                error_type,
                message,
            }
        }
        _ => DecisionModelError::Upstream {
            status,
            body: truncated,
        },
    }
}

fn validation_entry(item: &Value) -> String {
    let loc = item
        .get("loc")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .map(|p| {
                    p.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| p.to_string())
                })
                .collect::<Vec<_>>()
                .join(".")
        })
        .unwrap_or_default();
    let msg = item.get("msg").and_then(Value::as_str).unwrap_or_default();
    format!("{loc}: {msg}")
}

fn transport_error(err: reqwest::Error) -> DecisionModelError {
    if err.is_timeout() {
        DecisionModelError::Timeout
    } else {
        DecisionModelError::Transport(err.to_string())
    }
}

#[async_trait]
impl DecisionModelRepository for TypesafeJevAdapter {
    async fn decide(
        &self,
        request: DecisionRequest,
    ) -> Result<DecisionResponse, DecisionModelError> {
        request.validate()?;
        check_vendor_limits(&request)?;
        let response = self
            .client
            .post(format!("{}/v1/systemone", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&request_body(&request))
            .send()
            .await
            .map_err(transport_error)?;
        let status = response.status().as_u16();
        let body = response.text().await.map_err(transport_error)?;
        if !(200..300).contains(&status) {
            return Err(map_error(status, &body));
        }
        parse_response(&request, &body)
    }

    fn provider_name(&self) -> &'static str {
        "typesafe"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::decision_model::{ChoiceOption, NoulCriteria};
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn question(kind: QuestionKind) -> Question {
        Question {
            instructions: Some("Classify the intent".to_string()),
            kind,
        }
    }

    fn choice(keys: &[&str]) -> QuestionKind {
        let options = keys
            .iter()
            .map(|k| ChoiceOption {
                key: k.to_string(),
                criteria: None,
            })
            .collect();
        QuestionKind::Choice { options }
    }

    fn request(questions: Vec<(&str, QuestionKind)>) -> DecisionRequest {
        DecisionRequest {
            model: "jev-1.13.0".to_string(),
            state: json!("Me cobraron dos veces"),
            questions: questions
                .into_iter()
                .map(|(id, k)| (id.to_string(), question(k)))
                .collect(),
        }
    }

    async fn serve(status: u16, body: &str) -> (MockServer, TypesafeJevAdapter) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;
        let adapter = TypesafeJevAdapter::new("k")
            .unwrap()
            .with_base_url(&server.uri());
        (server, adapter)
    }

    #[test]
    fn empty_api_key_is_a_configuration_error_naming_the_env_var() {
        match TypesafeJevAdapter::new("  ") {
            Err(DecisionModelError::Configuration(msg)) => {
                assert!(msg.contains("TYPESAFE_API_KEY"))
            }
            Err(other) => panic!("expected Configuration, got {other:?}"),
            Ok(_) => panic!("a blank key must be rejected"),
        }
    }

    #[test]
    fn request_body_follows_the_wire_contract() {
        let when_true = Some("about money".to_string());
        let mut req = request(vec![
            (
                "billing",
                QuestionKind::Noul {
                    criteria: Some(NoulCriteria {
                        when_true,
                        when_false: None,
                    }),
                },
            ),
            ("plain", QuestionKind::Noul { criteria: None }),
            (
                "intent",
                QuestionKind::Choice {
                    options: vec![
                        ChoiceOption {
                            key: "refund".into(),
                            criteria: Some("Wants money back".into()),
                        },
                        ChoiceOption {
                            key: "other".into(),
                            criteria: None,
                        },
                    ],
                },
            ),
            (
                "urgency",
                QuestionKind::Score {
                    levels: vec!["low".into(), "high".into()],
                },
            ),
        ]);
        req.questions[1].1.instructions = None;
        let i = "Classify the intent";
        assert_eq!(
            request_body(&req),
            json!({
                "state": "Me cobraron dos veces",
                "model": "jev-1.13.0",
                "questions": {
                    "billing": {"type": "noul", "instructions": i, "criteria": {"true": "about money"}},
                    "plain": {"type": "noul"},
                    "intent": {"type": "choice", "instructions": i,
                               "criteria": {"refund": "Wants money back", "other": null}},
                    "urgency": {"type": "score", "instructions": i, "criteria": ["low", "high"]}
                }
            })
        );
    }

    #[tokio::test]
    async fn vendor_limits_fail_before_any_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let adapter = TypesafeJevAdapter::new("k")
            .unwrap()
            .with_base_url(&server.uri());
        let keys: Vec<String> = (0..256).map(|i| format!("o{i}")).collect();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let levels = (0..11).map(|i| i.to_string()).collect();
        for req in [
            request(vec![("c", choice(&keys))]),
            request(vec![("s", QuestionKind::Score { levels })]),
        ] {
            let err = adapter.decide(req).await.unwrap_err();
            assert!(
                matches!(&err, DecisionModelError::InvalidInput(m) if m.contains("at most")),
                "{err}"
            );
        }
    }

    #[tokio::test]
    async fn sends_bearer_key_and_parses_a_real_choice_response() {
        let server = MockServer::start().await;
        let req = request(vec![(
            "intent",
            choice(&["refund", "technical_support", "sales", "other"]),
        )]);
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .and(header("authorization", "Bearer k"))
            .and(body_json(request_body(&req)))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"model":"jev-1.13.0","answers":{"intent":{"type":"choice","choice":"refund","confidence":1.0,"probabilities":{"refund":1.0,"technical_support":0.0,"sales":0.0,"other":0.0}}},"usage":{"input_tokens":427,"output_tokens":80}}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;
        let adapter = TypesafeJevAdapter::new("k")
            .unwrap()
            .with_base_url(&server.uri());

        let response = adapter.decide(req).await.unwrap();
        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(
            response.usage,
            Some(DecisionUsage {
                input_tokens: 427,
                output_tokens: 80
            })
        );
        match &response.answers["intent"] {
            Answer::Choice {
                choice,
                confidence,
                probabilities,
            } => {
                assert_eq!((choice.as_str(), *confidence), ("refund", 1.0));
                assert_eq!(probabilities.len(), 4);
            }
            other => panic!("expected a choice, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn parses_real_noul_and_score_responses() {
        let body = r#"{"model":"jev-1.13.0","answers":{"b":{"type":"noul","noul":0.99},"u":{"type":"score","score":2.03,"confidence":0.93,"legend":{"0":"a","1":"b","2":"c","3":"d"},"probabilities":{"0":0.0,"1":0.02,"2":0.93,"3":0.05}}},"usage":{"input_tokens":427,"output_tokens":80}}"#;
        let (_server, adapter) = serve(200, body).await;
        let levels = ["a", "b", "c", "d"].iter().map(|l| l.to_string()).collect();
        let req = request(vec![
            ("b", QuestionKind::Noul { criteria: None }),
            ("u", QuestionKind::Score { levels }),
        ]);
        let response = adapter.decide(req).await.unwrap();
        assert_eq!(response.answers["b"], Answer::Noul { probability: 0.99 });
        match &response.answers["u"] {
            Answer::Score {
                score,
                confidence,
                probabilities,
            } => {
                assert_eq!(
                    (*score, *confidence, probabilities["2"]),
                    (2.03, 0.93, 0.93)
                )
            }
            other => panic!("expected a score, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_success_bodies_are_rejected() {
        let cases = [
            ("not json", "<html>oops</html>"),
            (
                "choice not offered",
                r#"{"model":"m","answers":{"q":{"type":"choice","choice":"zzz","confidence":1.0,"probabilities":{}}}}"#,
            ),
            ("missing answer", r#"{"model":"m","answers":{}}"#),
            (
                "unknown type",
                r#"{"model":"m","answers":{"q":{"type":"extract","value":"x"}}}"#,
            ),
        ];
        for (name, body) in cases {
            let (_server, adapter) = serve(200, body).await;
            let err = adapter
                .decide(request(vec![("q", choice(&["a", "b"]))]))
                .await
                .unwrap_err();
            assert!(
                matches!(err, DecisionModelError::MalformedResponse(_)),
                "{name}: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn vendor_errors_map_to_domain_errors() {
        // Error bodies captured from the live API.
        type Check = fn(&DecisionModelError) -> bool;
        let cases: Vec<(u16, &str, Check)> = vec![
            (
                401,
                r#"{"detail":{"error_type":"authentication_error","message":"Cannot authenticate with the server."}}"#,
                |e| matches!(e, DecisionModelError::Auth(_)),
            ),
            (
                400,
                r#"{"detail":{"error_type":"api_usage_error","message":"Unknown model: jev-1.12"}}"#,
                |e| {
                    matches!(e, DecisionModelError::InvalidRequest { status: 400, error_type: Some(t), message }
                    if t == "api_usage_error" && message == "Unknown model: jev-1.12")
                },
            ),
            (
                400,
                r#"{"detail":{"error_type":"max_tokens_exceeded"}}"#,
                |e| matches!(e, DecisionModelError::InvalidRequest { error_type: Some(t), .. } if t == "max_tokens_exceeded"),
            ),
            (
                400,
                r#"{"detail":"Too many choices. Must have at most 255 choices."}"#,
                |e| {
                    matches!(e, DecisionModelError::InvalidRequest { error_type: None, message, .. }
                    if message.starts_with("Too many choices"))
                },
            ),
            (
                422,
                r#"{"detail":[{"type":"missing","loc":["body","model"],"msg":"Field required","input":{}}]}"#,
                |e| {
                    matches!(e, DecisionModelError::InvalidRequest { status: 422, message, .. }
                    if message == "body.model: Field required")
                },
            ),
            (429, "rate limited", |e| {
                matches!(e, DecisionModelError::RateLimited)
            }),
            (529, "overloaded", |e| {
                matches!(e, DecisionModelError::Upstream { status: 529, .. })
            }),
            (503, "unavailable", |e| {
                matches!(e, DecisionModelError::Upstream { status: 503, .. })
            }),
        ];
        for (status, body, expected) in cases {
            let (_server, adapter) = serve(status, body).await;
            let err = adapter
                .decide(request(vec![("q", choice(&["a"]))]))
                .await
                .unwrap_err();
            assert!(expected(&err), "status {status}: got {err:?}");
        }
    }

    #[tokio::test]
    async fn a_slow_vendor_is_a_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
            .mount(&server)
            .await;
        let adapter = TypesafeJevAdapter::new("k")
            .unwrap()
            .with_base_url(&server.uri())
            .with_timeout(Duration::from_millis(50));
        let err = adapter
            .decide(request(vec![("q", choice(&["a"]))]))
            .await
            .unwrap_err();
        assert!(matches!(err, DecisionModelError::Timeout), "{err:?}");
    }

    #[tokio::test]
    #[ignore = "requires TYPESAFE_API_KEY — run with cargo test -- --ignored"]
    async fn live_choice_call_against_typesafe() {
        let key = std::env::var("TYPESAFE_API_KEY").expect("TYPESAFE_API_KEY");
        let adapter = TypesafeJevAdapter::new(key).unwrap();
        let options = ["refund", "technical_support", "sales", "none_of_these"];
        let response = adapter
            .decide(request(vec![("intent", choice(&options))]))
            .await
            .unwrap();
        match &response.answers["intent"] {
            Answer::Choice {
                choice, confidence, ..
            } => {
                assert_eq!(choice, "refund");
                assert!((0.0..=1.0).contains(confidence));
            }
            other => panic!("expected a choice, got {other:?}"),
        }
        assert!(response.usage.is_some_and(|u| u.input_tokens > 0));
    }
}
