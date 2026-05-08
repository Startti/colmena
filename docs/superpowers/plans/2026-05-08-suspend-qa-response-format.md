# Suspend Nodes — Q/A Response Format Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the divergent answer-parsing logic in `suspend` and `secure_suspend` with a single shared **ID-keyed** `Q[<id>]:/A[<id>]:` parser. Make `suspend.config.id` required (no default). Treat `options` on choice questions as UX suggestions, not a whitelist.

**Architecture:** A new shared module `qa_response_parser.rs` exposes `parse_qa_response(answer, expected_ids: &[&str]) -> Result<HashMap<String,String>, QaParseError>`. Both `suspend.rs` and `secure_suspend.rs` import it. `suspend` requires `config.id` and parses a 1-id response; `secure_suspend` builds the expected-id set from `secrets[].name`. Order-independent. No choice validation against `options` — accepts any text. No backward compatibility — old payload formats fail loudly.

**Tech Stack:** Rust 1.95.0, `serde_json`, `async_trait`, existing `ExecutableNode` trait. No new crates.

**Spec:** [`docs/superpowers/specs/2026-05-08-suspend-qa-response-format-design.md`](../specs/2026-05-08-suspend-qa-response-format-design.md)

---

## File Structure

| File | Role |
|------|------|
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/qa_response_parser.rs` | NEW — shared parser, `QaParseError` enum, unit tests |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs` | Add `pub mod qa_response_parser;` |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs` | Resume path: call parser, validate choice answer against `options` |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs` | Remove `parse_answers`; call shared parser instead |
| `src/libs/colmena/tests/secure_suspend_integration.rs` | Update `--answer` payload in test driver |
| `tests/graphs/basic/secure_suspend_smoke.json` | No graph change; doc comment update if any |
| `docs/node_configurations.json` | Update suspend and secure_suspend resume-answer description fields |
| `docs/agent_context/node_ports_reference.md` | Update §"suspend" and §"secure_suspend" example payloads |
| `docs/developer_guide/13_security_strategy.md` | Strategy 6 examples: switch to Q/A |
| `docs/AGENT_FEATURES_INDEX.md` | Add Q/A note to `secure_suspend` entry |
| `~/.claude/projects/-home-daniel-garcia4-startti-colmena/memory/feedback_test_graphs_with_agent_session_id.md` | Update `--answer` example payload |

---

## Task 1 — Shared parser module + tests

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/qa_response_parser.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

- [ ] **Step 1: Write failing tests for the parser**

Create `qa_response_parser.rs` with skeleton + tests first:

```rust
//! Shared parser for the canonical ID-keyed Q/A resume-answer format used by
//! all suspend-flavored nodes (`suspend`, `secure_suspend`).
//!
//! Format: `Q[<id>]: <text>\nA[<id>]: <text>` repeated, line-start anchored,
//! order-independent, ID character set `[A-Za-z0-9_-]{1,64}`.

use std::collections::HashMap;
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum QaParseError {
    InvalidIdSyntax { token: String },
    UnknownId { id: String },
    DuplicateId { id: String },
    MissingId { id: String },
    EmptyAnswer { id: String },
    OrphanQuestion { id: String },
}

impl fmt::Display for QaParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdSyntax { token } => {
                write!(f, "qa_response: invalid id syntax in '{token}'")
            }
            Self::UnknownId { id } => {
                write!(f, "qa_response: A[{id}] is not in the expected id set")
            }
            Self::DuplicateId { id } => {
                write!(f, "qa_response: A[{id}] appears more than once")
            }
            Self::MissingId { id } => {
                write!(f, "qa_response: missing answer for id '{id}'")
            }
            Self::EmptyAnswer { id } => {
                write!(f, "qa_response: empty answer for A[{id}]")
            }
            Self::OrphanQuestion { id } => {
                write!(f, "qa_response: Q[{id}] has no matching A[{id}]")
            }
        }
    }
}

impl std::error::Error for QaParseError {}

pub fn parse_qa_response(
    _answer: &str,
    _expected_ids: &[&str],
) -> Result<HashMap<String, String>, QaParseError> {
    todo!("implemented in step 3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_id_pair() {
        let input = "Q[confirm]: Confirm?\nA[confirm]: yes";
        let out = parse_qa_response(input, &["confirm"]).unwrap();
        assert_eq!(out.get("confirm"), Some(&"yes".to_string()));
    }

    #[test]
    fn parses_multiple_ids_in_declared_order() {
        let input = "Q[user]: User?\nA[user]: alice\nQ[pass]: Pass?\nA[pass]: hunter2";
        let out = parse_qa_response(input, &["user", "pass"]).unwrap();
        assert_eq!(out.get("user"), Some(&"alice".to_string()));
        assert_eq!(out.get("pass"), Some(&"hunter2".to_string()));
    }

    #[test]
    fn parses_multiple_ids_in_reversed_order() {
        // Order-independence: declared [user, pass] but answered pass-first.
        let input = "Q[pass]: P?\nA[pass]: hunter2\nQ[user]: U?\nA[user]: alice";
        let out = parse_qa_response(input, &["user", "pass"]).unwrap();
        assert_eq!(out.get("user"), Some(&"alice".to_string()));
        assert_eq!(out.get("pass"), Some(&"hunter2".to_string()));
    }

    #[test]
    fn preserves_internal_newlines_in_answer() {
        let input = "Q[k]: PEM?\nA[k]: -----BEGIN-----\nMIIEvQ\n-----END-----\nQ[fp]: FP?\nA[fp]: ab:cd";
        let out = parse_qa_response(input, &["k", "fp"]).unwrap();
        assert_eq!(out.get("k").unwrap(), "-----BEGIN-----\nMIIEvQ\n-----END-----");
        assert_eq!(out.get("fp").unwrap(), "ab:cd");
    }

    #[test]
    fn tolerates_no_space_after_colon() {
        let input = "Q[x]:Confirm?\nA[x]:yes";
        let out = parse_qa_response(input, &["x"]).unwrap();
        assert_eq!(out.get("x"), Some(&"yes".to_string()));
    }

    #[test]
    fn does_not_validate_question_text_matches() {
        let input = "Q[x]: anything goes here\nA[x]: payload";
        let out = parse_qa_response(input, &["x"]).unwrap();
        assert_eq!(out.get("x"), Some(&"payload".to_string()));
    }

    #[test]
    fn errors_on_invalid_id_syntax() {
        let input = "Q[bad space]: hi\nA[bad space]: x";
        let err = parse_qa_response(input, &["bad space"]).unwrap_err();
        assert!(matches!(err, QaParseError::InvalidIdSyntax { .. }));
    }

    #[test]
    fn errors_on_unknown_id() {
        let input = "Q[wrong]: hi\nA[wrong]: x";
        let err = parse_qa_response(input, &["right"]).unwrap_err();
        assert!(matches!(err, QaParseError::UnknownId { .. }));
    }

    #[test]
    fn errors_on_duplicate_id() {
        let input = "Q[x]: hi\nA[x]: one\nQ[x]: hi\nA[x]: two";
        let err = parse_qa_response(input, &["x"]).unwrap_err();
        assert!(matches!(err, QaParseError::DuplicateId { .. }));
    }

    #[test]
    fn errors_on_missing_id() {
        let input = "Q[a]: hi\nA[a]: x";
        let err = parse_qa_response(input, &["a", "b"]).unwrap_err();
        match err {
            QaParseError::MissingId { id } => assert_eq!(id, "b"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn errors_on_empty_answer() {
        let input = "Q[x]: hi\nA[x]: \nQ[y]: hi\nA[y]: ok";
        let err = parse_qa_response(input, &["x", "y"]).unwrap_err();
        assert!(matches!(err, QaParseError::EmptyAnswer { .. }));
    }

    #[test]
    fn errors_on_orphan_q_without_a() {
        let input = "Q[x]: hi";
        let err = parse_qa_response(input, &["x"]).unwrap_err();
        assert!(matches!(
            err,
            QaParseError::OrphanQuestion { .. } | QaParseError::MissingId { .. }
        ));
    }
}
```

- [ ] **Step 2: Verify tests fail**

Add to `mod.rs`:

```rust
pub mod qa_response_parser;
```

Run: `cargo test -p colmena_dag_engine --lib qa_response_parser`
Expected: all 12 tests fail with `not yet implemented` panic from `todo!`.

- [ ] **Step 3: Implement the parser**

Replace the `todo!` body with:

```rust
const ID_MAX_LEN: usize = 64;

fn is_valid_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn validate_id(id: &str) -> Result<(), QaParseError> {
    if id.is_empty() || id.len() > ID_MAX_LEN || !id.chars().all(is_valid_id_char) {
        return Err(QaParseError::InvalidIdSyntax {
            token: id.to_string(),
        });
    }
    Ok(())
}

/// Returns Some((kind, id, after_colon_offset)) if the line at `offset`
/// starts with `Q[<id>]:` or `A[<id>]:`. `kind` is 'Q' or 'A'.
fn parse_prefix_at(answer: &str, offset: usize) -> Option<(char, String, usize)> {
    let bytes = answer.as_bytes();
    if offset >= bytes.len() {
        return None;
    }
    let kind = bytes[offset] as char;
    if kind != 'Q' && kind != 'A' {
        return None;
    }
    if bytes.get(offset + 1) != Some(&b'[') {
        return None;
    }
    let close = answer[offset + 2..].find(']')?;
    let id_end = offset + 2 + close;
    if bytes.get(id_end + 1) != Some(&b':') {
        return None;
    }
    let id = &answer[offset + 2..id_end];
    Some((kind, id.to_string(), id_end + 2))
}

pub fn parse_qa_response(
    answer: &str,
    expected_ids: &[&str],
) -> Result<HashMap<String, String>, QaParseError> {
    // Walk line by line, finding A[<id>]: anchors. For each, read body until
    // the next Q[…]: or A[…]: at line start (or EOF). Collect into a map.
    let mut answers: HashMap<String, String> = HashMap::new();
    let mut q_seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let bytes = answer.as_bytes();
    let mut line_start = 0usize;
    while line_start < bytes.len() {
        let line_end = answer[line_start..]
            .find('\n')
            .map(|n| line_start + n)
            .unwrap_or(bytes.len());

        if let Some((kind, id, after_colon)) = parse_prefix_at(answer, line_start) {
            validate_id(&id)?;

            if kind == 'Q' {
                q_seen.insert(id);
                line_start = line_end + 1;
                continue;
            }

            // kind == 'A': read body until next Q[…] or A[…] at line start.
            // Skip a single leading space after the colon if present.
            let mut body_start = after_colon;
            if bytes.get(body_start) == Some(&b' ') {
                body_start += 1;
            }

            let mut scan = line_end;
            let body_end = loop {
                if scan >= bytes.len() {
                    break bytes.len();
                }
                let s = scan + 1;
                if s >= bytes.len() {
                    break bytes.len();
                }
                if matches!(bytes[s] as char, 'Q' | 'A') && bytes.get(s + 1) == Some(&b'[')
                {
                    break s;
                }
                scan = answer[s..]
                    .find('\n')
                    .map(|n| s + n)
                    .unwrap_or(bytes.len());
            };

            let raw = &answer[body_start..body_end];
            let trimmed = raw.trim_end_matches('\n');
            if trimmed.trim().is_empty() {
                return Err(QaParseError::EmptyAnswer { id });
            }

            if !expected_ids.iter().any(|e| *e == id) {
                return Err(QaParseError::UnknownId { id });
            }
            if answers.contains_key(&id) {
                return Err(QaParseError::DuplicateId { id });
            }
            answers.insert(id, trimmed.to_string());

            line_start = body_end;
            continue;
        }

        line_start = line_end + 1;
    }

    // Orphan Q[<id>] (Q present, A missing) — count those declared but
    // without a corresponding answer.
    for id in &q_seen {
        if !answers.contains_key(id) && expected_ids.iter().any(|e| *e == id.as_str()) {
            return Err(QaParseError::OrphanQuestion { id: id.clone() });
        }
    }

    for expected in expected_ids {
        if !answers.contains_key(*expected) {
            return Err(QaParseError::MissingId {
                id: (*expected).to_string(),
            });
        }
    }

    Ok(answers)
}
```

(The implementer should refine the body-scanning loop if any test fails — the algorithm is straightforward but subtle edge cases around line-start anchoring may need adjustment.)

- [ ] **Step 4: Verify tests pass**

Run: `cargo test -p colmena_dag_engine --lib qa_response_parser`
Expected: all 9 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/qa_response_parser.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "$(cat <<'EOF'
feat(suspend): shared Q/A response parser

Adds qa_response_parser module shared by all suspend-flavored nodes.
Single canonical format `Q1:/A1:/Q2:/A2:...` with line-start anchoring
and multi-line answer support.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2 — `secure_suspend` adopts shared parser

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs:78-129` (delete `parse_answers`)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs:205` (call site)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs:530-595` (rewrite parser tests)

- [ ] **Step 1: Update existing parser tests to ID-keyed Q/A format**

In `secure_suspend.rs` test module, REPLACE the four `parser_*` tests (around lines 530-595) with:

```rust
#[test]
fn parser_extracts_two_values_by_id() {
    let secrets = vec![
        Secret { question: "Q1?".into(), name: "n1".into() },
        Secret { question: "Q2?".into(), name: "n2".into() },
    ];
    let answer = "Q[n1]: Q1?\nA[n1]: val-one\nQ[n2]: Q2?\nA[n2]: val-two";
    let values = call_parser(answer, &secrets).unwrap();
    assert_eq!(values.get("n1").unwrap(), "val-one");
    assert_eq!(values.get("n2").unwrap(), "val-two");
}

#[test]
fn parser_order_independent() {
    // Secrets declared as [n1, n2] but answered in reverse order.
    let secrets = vec![
        Secret { question: "Q1?".into(), name: "n1".into() },
        Secret { question: "Q2?".into(), name: "n2".into() },
    ];
    let answer = "Q[n2]: Q2?\nA[n2]: val-two\nQ[n1]: Q1?\nA[n1]: val-one";
    let values = call_parser(answer, &secrets).unwrap();
    assert_eq!(values.get("n1").unwrap(), "val-one");
    assert_eq!(values.get("n2").unwrap(), "val-two");
}

#[test]
fn parser_preserves_internal_newlines_in_value() {
    let secrets = vec![
        Secret { question: "PEM?".into(), name: "key".into() },
        Secret { question: "FP?".into(), name: "fp".into() },
    ];
    let answer = "Q[key]: PEM?\nA[key]: line-1\nline-2\nline-3\nQ[fp]: FP?\nA[fp]: ab:cd";
    let values = call_parser(answer, &secrets).unwrap();
    assert_eq!(values.get("key").unwrap(), "line-1\nline-2\nline-3");
    assert_eq!(values.get("fp").unwrap(), "ab:cd");
}

#[test]
fn parser_errors_on_missing_id() {
    let secrets = vec![
        Secret { question: "Q1?".into(), name: "n1".into() },
        Secret { question: "Q2?".into(), name: "n2".into() },
    ];
    let answer = "Q[n1]: Q1?\nA[n1]: only";
    let err = call_parser(answer, &secrets).unwrap_err();
    assert!(err.contains("missing answer for id 'n2'"), "got: {err}");
}

#[test]
fn parser_errors_on_empty_value() {
    let secrets = vec![
        Secret { question: "Q1?".into(), name: "n1".into() },
        Secret { question: "Q2?".into(), name: "n2".into() },
    ];
    let answer = "Q[n1]: Q1?\nA[n1]: \nQ[n2]: Q2?\nA[n2]: v2";
    let err = call_parser(answer, &secrets).unwrap_err();
    assert!(err.contains("empty answer for A[n1]"), "got: {err}");
}

// Adapter that calls the shared parser with the secrets' names as the
// expected id set.
fn call_parser(
    answer: &str,
    secrets: &[Secret],
) -> Result<std::collections::HashMap<String, String>, String> {
    use crate::dag_engine::infrastructure::nodes::qa_response_parser::parse_qa_response;
    let ids: Vec<&str> = secrets.iter().map(|s| s.name.as_str()).collect();
    parse_qa_response(answer, &ids).map_err(|e| e.to_string())
}
```

- [ ] **Step 2: Verify tests fail**

Run: `cargo test -p colmena_dag_engine --lib secure_suspend::tests::parser_`
Expected: tests fail (the local `parse_answers` still exists; `call_parser` references the new module that exists, but the *node* still uses old parser, so behavior tests in next step will need it).

- [ ] **Step 3: Delete `parse_answers` and switch call site**

In `secure_suspend.rs`:

(a) Delete the `parse_answers` function (lines ~78-129) AND its doc comment.

(b) At the top of the file, add the import:

```rust
use crate::dag_engine::infrastructure::nodes::qa_response_parser::parse_qa_response;
```

(c) Replace the call site (around line 205). Currently:

```rust
let values = parse_answers(answer, &secrets)
    .map_err(Box::<dyn Error + Send + Sync>::from)?;
```

Replace with code that calls the new ID-keyed parser and reorders the answers to match the declared `secrets` order (downstream code zips secrets ↔ values):

```rust
let id_refs: Vec<&str> = secrets.iter().map(|s| s.name.as_str()).collect();
let answer_map = parse_qa_response(answer, &id_refs)
    .map_err(|e| Box::<dyn Error + Send + Sync>::from(format!("secure_suspend: {e}")))?;
let values: Vec<String> = secrets
    .iter()
    .map(|s| {
        answer_map
            .get(&s.name)
            .cloned()
            .expect("parser guarantees all expected ids are present")
    })
    .collect();
```

- [ ] **Step 4: Update existing resume-path integration tests in this same file**

Find the tests that pass `__colmena_resume_answer` (around lines 600-820) and update each `--answer` payload to ID-keyed format. Pattern:

OLD: `"Q1?\nval-one\nQ2?\nval-two"` (with secret names `n1`, `n2`)
NEW: `"Q[n1]: Q1?\nA[n1]: val-one\nQ[n2]: Q2?\nA[n2]: val-two"`

Apply this rewrite to every test that has a multi-line resume answer string. Search the file for `__colmena_resume_answer` occurrences and update each. The literal `Q[…]:` text doesn't have to match the secret's `question` — only the bracketed id matters.

- [ ] **Step 5: Run all secure_suspend tests**

Run: `cargo test -p colmena_dag_engine --lib secure_suspend`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs
git commit -m "$(cat <<'EOF'
refactor(secure_suspend): use shared Q/A parser

Removes the anchored question-text parser. secure_suspend now consumes
the canonical Q1:/A1:/Q2:/A2: format via the shared qa_response_parser.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3 — `suspend` (classic) adopts ID-keyed Q/A format + required `id`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs:38-82` (suspend path: require `id`)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs:28-34` (resume path)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs:97-211` (tests)

- [ ] **Step 1: Add tests for ID requirement + ID-keyed resume**

At the bottom of the existing `tests` module in `suspend.rs`, add:

```rust
#[tokio::test]
async fn config_without_id_is_rejected_at_execute() {
    let node = SuspendNode;
    let inputs: NodeInputs = HashMap::new(); // no __node_id, no inputs
    let cfg = json!({ "question": "Confirm?" }); // no id
    let mut state = Value::Null;
    let err = node
        .execute(&inputs, &cfg, &mut state, empty_observer())
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("config.id is required"));
}

#[tokio::test]
async fn resume_with_qa_open_yields_answer() {
    let node = SuspendNode;
    let mut inputs: NodeInputs = HashMap::new();
    inputs.insert(
        "__colmena_resume_answer".to_string(),
        Value::String("Q[confirm]: Confirm?\nA[confirm]: yes".to_string()),
    );
    let cfg = json!({ "id": "confirm", "question": "Confirm?" });
    let mut state = Value::Null;
    let out = node
        .execute(&inputs, &cfg, &mut state, empty_observer())
        .await
        .unwrap();
    assert_eq!(out["status"], "resumed");
    assert_eq!(out["answer_received"], "yes");
}

#[tokio::test]
async fn resume_with_qa_choice_accepts_option_value() {
    let node = SuspendNode;
    let mut inputs: NodeInputs = HashMap::new();
    inputs.insert(
        "__colmena_resume_answer".to_string(),
        Value::String("Q[env]: Pick env\nA[env]: production".to_string()),
    );
    let cfg = json!({
        "id": "env",
        "question": "Pick env",
        "question_type": "choice",
        "options": ["production", "staging"]
    });
    let mut state = Value::Null;
    let out = node
        .execute(&inputs, &cfg, &mut state, empty_observer())
        .await
        .unwrap();
    assert_eq!(out["answer_received"], "production");
}

#[tokio::test]
async fn resume_with_qa_choice_accepts_free_text() {
    // options are suggestions, not a whitelist — free-text is also accepted.
    let node = SuspendNode;
    let mut inputs: NodeInputs = HashMap::new();
    inputs.insert(
        "__colmena_resume_answer".to_string(),
        Value::String("Q[env]: Pick env\nA[env]: review-app-123".to_string()),
    );
    let cfg = json!({
        "id": "env",
        "question": "Pick env",
        "question_type": "choice",
        "options": ["production", "staging"]
    });
    let mut state = Value::Null;
    let out = node
        .execute(&inputs, &cfg, &mut state, empty_observer())
        .await
        .unwrap();
    assert_eq!(out["answer_received"], "review-app-123");
}

#[tokio::test]
async fn resume_path_propagates_parser_errors() {
    let node = SuspendNode;
    let mut inputs: NodeInputs = HashMap::new();
    inputs.insert(
        "__colmena_resume_answer".to_string(),
        Value::String("just a raw string".to_string()),
    );
    let cfg = json!({ "id": "x", "question": "Confirm?" });
    let mut state = Value::Null;
    let err = node
        .execute(&inputs, &cfg, &mut state, empty_observer())
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("missing answer"));
}
```

ALSO — DELETE the existing `resume_path_unchanged` test (around lines 195-211, asserts the old pass-through behavior). Update `suspend_uses_local_node_id_as_default_id` and `suspend_uses_explicit_id_when_set` so both pass `id` explicitly in the cfg (the default-from-node_id behavior is going away).

- [ ] **Step 2: Run tests to verify the new ones fail**

Run: `cargo test -p colmena_dag_engine --lib suspend::tests`
Expected: new tests fail; the suspend path doesn't yet require `id` and resume path still passes through verbatim.

- [ ] **Step 3: Update `suspend.rs`**

At the top of the file, add:

```rust
use crate::dag_engine::infrastructure::nodes::qa_response_parser::parse_qa_response;
```

(a) **Suspend path** — replace the existing `id` resolution block (lines ~44-54):

```rust
let id = config
    .get("id")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string())
    .or_else(|| {
        inputs
            .get("__node_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    })
    .unwrap_or_else(|| "suspend".to_string());
```

with:

```rust
let id = config
    .get("id")
    .and_then(|v| v.as_str())
    .ok_or_else(|| {
        Box::<dyn Error + Send + Sync>::from(
            "suspend: config.id is required (must be [A-Za-z0-9_-]{1,64})",
        )
    })?
    .to_string();

// Validate the id charset.
if id.is_empty()
    || id.len() > 64
    || !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
{
    return Err(Box::<dyn Error + Send + Sync>::from(format!(
        "suspend: invalid config.id '{id}' (must match [A-Za-z0-9_-]{{1,64}})"
    )));
}
```

(b) **Resume path** — replace lines 28-34:

```rust
if let Some(answer) = inputs.get("__colmena_resume_answer") {
    return Ok(json!({
        "status": "resumed",
        "answer_received": answer
    }));
}
```

with:

```rust
if let Some(answer_val) = inputs.get("__colmena_resume_answer") {
    let raw = answer_val.as_str().ok_or_else(|| {
        Box::<dyn Error + Send + Sync>::from(
            "suspend: __colmena_resume_answer must be a string",
        )
    })?;
    let id = config
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            Box::<dyn Error + Send + Sync>::from(
                "suspend: config.id is required on resume",
            )
        })?;
    let mut parsed = parse_qa_response(raw, &[id])
        .map_err(|e| Box::<dyn Error + Send + Sync>::from(format!("suspend: {e}")))?;
    let answer = parsed.remove(id).expect("parser guarantees the id is present");

    return Ok(json!({
        "status": "resumed",
        "answer_received": answer
    }));
}
```

NOTE: choice questions have NO validation against `options` — `options` is a UX suggestion list, not a whitelist (see spec).

- [ ] **Step 4: Run all suspend tests**

Run: `cargo test -p colmena_dag_engine --lib suspend`
Expected: all suspend tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs
git commit -m "$(cat <<'EOF'
refactor(suspend): require config.id and adopt ID-keyed Q/A format

- config.id is now required (no fallback to __node_id)
- Resume path parses --answer with shared qa_response_parser keyed by id
- Choice answers are not validated against options (options are suggestions)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4 — Integration test fixture update

**Files:**
- Modify: `src/libs/colmena/tests/secure_suspend_integration.rs` (search for `--answer` / `__colmena_resume_answer` payloads)
- Modify: `src/libs/colmena/tests/secure_values_cross_session_integration.rs` (likely has resume payloads)
- Modify: any graph fixtures under `tests/graphs/advanced/secure_suspend_*.json` whose embedded operator answers reference the old format

- [ ] **Step 1: Read each integration test driver**

Run:
```bash
grep -rn "__colmena_resume_answer\|resume_answer\|--answer" src/libs/colmena/tests/ tests/graphs/
```

Identify each test/fixture that constructs a resume-answer payload.

- [ ] **Step 2: Update each payload to ID-keyed format**

For every multi-secret payload like `"What is your username?\nalice\nWhat is your password?\nhunter2"` (where the secrets were `[{name:"username"},{name:"password"}]`), rewrite to:

```
"Q[username]: What is your username?\nA[username]: alice\nQ[password]: What is your password?\nA[password]: hunter2"
```

For `suspend` classic payloads (single question), wrap with the configured `id`:

```
"Q[<id>]: <echo>\nA[<id>]: <value>"
```

The text after `Q[<id>]:` is not validated — any human-readable echo works.

- [ ] **Step 3: Run the integration test**

Run: `source .env && cargo test -p colmena_dag_engine --test secure_suspend_integration -- --ignored`
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/tests/secure_suspend_integration.rs
git commit -m "$(cat <<'EOF'
test(secure_suspend): update integration payloads to Q/A format

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5 — Documentation updates

**Files:**
- Modify: `docs/node_configurations.json` — `suspend` and `secure_suspend` entries (resume-answer description)
- Modify: `docs/agent_context/node_ports_reference.md` — §"suspend" and §"secure_suspend"
- Modify: `docs/developer_guide/13_security_strategy.md` — Strategy 6 examples
- Modify: `docs/AGENT_FEATURES_INDEX.md` — `secure_suspend` entry: append one-line note

- [ ] **Step 1: Update node_configurations.json**

In the `suspend` entry, mark the `id` config field as **required** (whatever the schema convention is — likely a `"required": true` flag on the field). Then locate the description for the resume answer mechanism (around line 257). Add a `resume_answer_format` field documenting the ID-keyed format:

```json
"resume_answer_format": {
  "format": "Q[<id>]: <question echo>\nA[<id>]: <answer>",
  "description": "Operators (or LLMs) write the answer using Q[<id>]:/A[<id>]: prefixes anchored at line start. The <id> is the config.id of this suspend node. Multi-line answers are preserved between A[<id>]: and the next prefix or end of input. The question echo is for human readability; the parser does not validate it. For choice questions, options are UX suggestions — any free-text answer is accepted."
}
```

In the `secure_suspend` entry, add the same block with the multi-question + ordered-by-name example. Make sure the `secrets[].name` field is documented as "the id used in the Q[…]:/A[…]: resume payload":

```json
"resume_answer_format": {
  "format": "Q[<name1>]: <q1>\nA[<name1>]: <a1>\nQ[<name2>]: <q2>\nA[<name2>]: <a2>...",
  "description": "Resume answers are keyed by the secret's `name`. Order does not matter — the parser binds by id. Multi-line answers preserved."
}
```

- [ ] **Step 2: Update node_ports_reference.md**

Open `docs/agent_context/node_ports_reference.md`. Find the `suspend` In-Depth section (search for `--answer "Approved"`). Replace the example block:

```bash
--answer "Approved"
```

with:

```bash
--answer "Q[confirm_transfer]: Approve transfer?
A[confirm_transfer]: Approved"
```

Add a paragraph after explaining: ID-keyed, order-independent, `id` from `config.id`, link to `node_configurations.json` for the canonical schema, and noting that choice answers are not validated against `options`.

For the `secure_suspend` entry, find the existing `--answer` example and replace with:

```bash
--answer "Q[username]: API key
A[username]: sk-abc123
Q[password]: API secret
A[password]: shh-def456"
```

- [ ] **Step 3: Update 13_security_strategy.md**

Open `docs/developer_guide/13_security_strategy.md`. Find Strategy 6 (`secure_suspend`). Locate any example showing a `--answer` payload and replace with ID-keyed equivalents. Add a short subsection titled "Resume answer format" that:
- States the ID-keyed Q/A format is canonical and shared with the classic `suspend` node.
- Links to `node_configurations.json` for the schema.
- Notes the format is order-independent and the question echo is human-readable only.
- Notes that for `suspend` classic, `config.id` is now required.

- [ ] **Step 4: Update AGENT_FEATURES_INDEX.md**

Open `docs/AGENT_FEATURES_INDEX.md`. In the `secure_suspend` section ("Reanudación" subsection), replace:

```
CLI: `--answer "<pregunta_1>\n<valor_1>\n<pregunta_2>\n<valor_2>"`. Parser ancla en el texto literal de cada pregunta (preserva multilinea internos).
```

with:

```
CLI: `--answer "Q[<name1>]: <pregunta>\nA[<name1>]: <valor>\nQ[<name2>]: <pregunta>\nA[<name2>]: <valor>"`. Formato canónico compartido con el nodo `suspend` clásico — keyed por el `name` de cada secret (o `config.id` en suspend), orden-independiente, multilinea preservada. Para suspend clásico, `config.id` es **obligatorio**.
```

- [ ] **Step 5: Commit**

```bash
git add docs/node_configurations.json \
        docs/agent_context/node_ports_reference.md \
        docs/developer_guide/13_security_strategy.md \
        docs/AGENT_FEATURES_INDEX.md
git commit -m "$(cat <<'EOF'
docs(suspend): document Q/A resume answer format

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6 — Memory & CLAUDE.md updates

**Files:**
- Modify: `~/.claude/projects/-home-daniel-garcia4-startti-colmena/memory/feedback_test_graphs_with_agent_session_id.md`
- Modify: `~/.claude/projects/-home-daniel-garcia4-startti-colmena/memory/feedback_conversational_demo_pattern.md`
- Modify: `CLAUDE.md` (`Reanudación` example under suspend section if any)

- [ ] **Step 1: Update memory file `feedback_test_graphs_with_agent_session_id.md`**

Locate the `--answer` example. Replace with the ID-keyed format (assuming suspend's `config.id` is `"confirm"`):

```bash
--answer "Q[confirm]: ¿Confirmar transferencia?\nA[confirm]: sí"
```

Add a short paragraph noting that all suspend-flavored nodes share the ID-keyed Q/A format and that `suspend.config.id` is now required.

- [ ] **Step 2: (If applicable) Update `feedback_conversational_demo_pattern.md`**

Search the file for `--answer` examples. If the example is for an `input` node (NOT a suspend node), leave it — `input` nodes don't use the Q/A format. If the example is for a suspend node, update.

- [ ] **Step 3: Update CLAUDE.md if needed**

Search `CLAUDE.md` for `--answer` examples. If any reference suspend nodes, update to Q/A format.

- [ ] **Step 4: Add a new feedback memory for the Q/A rule**

Create `~/.claude/projects/-home-daniel-garcia4-startti-colmena/memory/feedback_suspend_qa_response_format.md`:

```markdown
---
name: Suspend nodes use ID-keyed Q/A response format
description: All suspend-flavored nodes parse --answer as Q[<id>]:/A[<id>]: pairs keyed by suspend.config.id (required) or secure_suspend.secrets[].name; order-independent
type: feedback
---

When writing or reading a `--answer` payload for any suspend-flavored node (`suspend`, `secure_suspend`), use the canonical ID-keyed format:

```
Q[<id>]: <question echo>
A[<id>]: <answer>
Q[<id2>]: <question echo>
A[<id2>]: <answer>
```

The `<id>` is:
- For `suspend`: the value of `config.id` — now **required**, no default.
- For `secure_suspend`: the value of `secrets[i].name`.

**Why:** Before 2026-05-08 each suspend node had its own contract. `secure_suspend` required the LLM-emitted question text verbatim (fragile — failed once in canvas-builder e2e because the operator wrote "usuario" instead of the literal "Por favor, introduce tu nombre de usuario"). `suspend` had no parser at all — pass-through. The unified ID-keyed format binds answers by stable id, is order-independent, and treats `Q[<id>]:` echoes as human-readable only — so question text mismatches never break parsing.

**How to apply:**
- One question (suspend classic): `--answer "Q[<id>]: <anything>\nA[<id>]: <value>"` where `<id>` matches `config.id`.
- N secrets (secure_suspend): `--answer "Q[<name1>]: ...\nA[<name1>]: ...\nQ[<name2>]: ...\nA[<name2>]: ..."` — order doesn't matter.
- Multi-line answers (e.g. PEM keys): everything between `A[<id>]:` and the next `Q[…]:`/`A[…]:` line-start or EOF is the value, internal newlines preserved.
- Choice questions on `suspend`: `options` is a UX suggestion list — any free-text answer is accepted; no whitelist enforcement.
- ID character set: `[A-Za-z0-9_-]{1,64}`. Same as the existing `name`/`id` constraints.
- The text after `Q[<id>]:` is NOT validated — only the bracketed id matters.
```

Then add an entry to `MEMORY.md`:

```markdown
- [Suspend Q/A response format](feedback_suspend_qa_response_format.md) — ID-keyed `Q[<id>]:/A[<id>]:` for all suspend nodes; `suspend.config.id` required; order-independent; `options` are suggestions
```

- [ ] **Step 5: No commit needed (memory files are not under git)**

(Memory directory is in `~/.claude/`, not the repo.)

---

## Task 7 — Final verification

- [ ] **Step 1: Full test suite**

Run: `cargo test -p colmena_dag_engine --lib`
Expected: all tests pass.

- [ ] **Step 2: Doctests + integration**

Run: `source .env && cargo test --verbose`
Expected: all pass (including ignored ones run with `-- --ignored` if locally available).

- [ ] **Step 3: Clippy + fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 4: Manual smoke test**

Run a real e2e suspend → resume cycle:

```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/advanced/secure_suspend_login_e2e.json \
  --agent-session-id agent_qa_smoke_001
# Note the suspended questions and the secret names emitted, then:
cargo run --bin dag_engine -- run tests/graphs/advanced/secure_suspend_login_e2e.json \
  --agent-session-id agent_qa_smoke_001 \
  --answer "Q[username]: <echoed question 1>
A[username]: alice
Q[password]: <echoed question 2>
A[password]: hunter2"
```

Expected: run completes, secure values resolved, downstream `http_request` (or whatever consumer) receives injected values.
