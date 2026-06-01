# Gemini Scalar Tool Response Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop Gemini agents from dying silently when a tool returns a non-object JSON value (number, string, array, bool). Wrap any non-object tool result in `{result: <value>}` before injecting it into `Content.parts[].functionResponse.response`, which Gemini's API requires to be a `google.protobuf.Struct` (object).

**Architecture:** Tiny, surgical fix in a single function in one adapter file. The Gemini wire format already requires the field to be an object — Colmena currently only enforces this when the raw `LlmMessage::Tool` content fails to parse as JSON. The fix extends the same envelope (`{result: …}`) to also cover the case "parsed successfully, but isn't an object". Anthropic/OpenAI adapters do NOT have this bug (verified — they pass tool content as opaque strings, never as raw JSON values).

**Tech Stack:** Rust 1.95.0, `serde_json`, `mockall` (not needed for these tests — pure converter, no HTTP). Tests inline in `#[cfg(test)] mod tests` per project convention.

**Pre-flight context for the engineer:**

- **The bug** is documented in detail at the top of this conversation; one-line summary: `gemini_adapter.rs:173-175` only wraps tool content in `{result: …}` when JSON parsing **fails**, so a tool that returns `5040` ends up with `response: 5040` in the wire payload, which Gemini rejects with HTTP 400 `INVALID_ARGUMENT`. Result for the end user: Gemini agent dies silently after one turn — SSE shows `result: ""`, `completion_tokens: 0`, no error event.
- **Verification done before this plan:** ran 3 graphs (one per provider) with `python_script` returning a scalar — OpenAI ✅, Anthropic ✅, Gemini ❌ (silent death, `result: ""`). Also hit Gemini API directly via curl with the buggy payload → 400 INVALID_ARGUMENT reproduced character-for-character. Wrapping in `{result: 5040}` → Gemini returns `"7! is 5,040."` ✅.
- **Why only Gemini:** Anthropic encodes `tool_result.content` as a string opaque blob; OpenAI puts it under `"content"` as a string too. Gemini is the only adapter that parses the string back into JSON and injects the raw `Value` into a field typed as `Struct`.
- **NO ADP sweep needed.** This is an internal wire-format change between Colmena and the Gemini REST API. ADP only consumes Colmena's SSE events (`tool-output-available`, etc.) which keep the raw scalar value — those are not affected. Confirmed: the field that changes (`functionResponse.response`) never crosses the colmena↔ADP boundary.
- **Repro graphs already exist** at `/tmp/colmena_e2e/graphs/scalar_tool_{openai,anthropic,gemini}.json` (created during verification). They are the regression test bed for Task 7. Do NOT move them into `tests/graphs/` — they hit live LLM APIs and would slow down CI; keep them as a manual smoke test under `/tmp`.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs` | Modify lines 173-175 + add 6 unit tests in existing `mod tests` (starts line 847) | The single source of truth for the bug and its fix |
| `CLAUDE.md` | Modify "Current Status" section | Add a one-line entry documenting the fix and date |

No new files. No new modules. The fix is 4 lines of Rust + 6 unit tests in the same file.

---

## Task 1: Add the six failing unit tests

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs` (append tests inside the existing `#[cfg(test)] mod tests` block — find its closing brace and insert before it)

- [ ] **Step 1: Open the file and locate the closing brace of `mod tests`**

The module starts at line 847 (`#[cfg(test)] mod tests {`). Find the corresponding closing `}` (currently the last `}` in the file, since `mod tests` is the last item). Insert the new tests immediately before that closing brace.

- [ ] **Step 2: Write the six failing tests**

Add this code block inside `mod tests`, before its closing `}`:

```rust
    // ----------------------------------------------------------------------
    // Regression tests for the "scalar tool response" bug.
    //
    // Gemini's `Content.parts[].functionResponse.response` field is typed as
    // `google.protobuf.Struct` and ONLY accepts JSON objects. Scalars, arrays,
    // booleans, and null are rejected with HTTP 400 INVALID_ARGUMENT.
    //
    // `LlmMessage::Tool` content is an arbitrary JSON-encoded string. The
    // adapter must wrap any non-object value in `{ "result": <value> }` so
    // Gemini accepts the round-trip. Objects must pass through unchanged so
    // callers that already return dicts keep their keys.
    //
    // See: docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md
    // ----------------------------------------------------------------------

    fn build_request_with_tool_response(content: &str) -> crate::llm::domain::LlmRequest {
        use crate::llm::domain::{
            FunctionCall, LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind, ToolCall,
        };
        let provider =
            LlmProvider::new(ProviderKind::Google, "test_key".to_string(), None).unwrap();
        let config = LlmConfig::new(provider);
        let tool_call = ToolCall::new(
            "call_1".to_string(),
            FunctionCall::new("runCode".to_string(), "{}".to_string()),
        );
        let messages = vec![
            LlmMessage::user("compute 7!".to_string()).unwrap(),
            LlmMessage::assistant_with_tool_calls("".to_string(), vec![tool_call]).unwrap(),
            LlmMessage::tool("call_1".to_string(), content.to_string()).unwrap(),
        ];
        LlmRequest::new(messages, config, false).unwrap()
    }

    fn extract_function_response(contents: &[GeminiContent]) -> serde_json::Value {
        let function_msg = contents.iter().find(|c| c.role == "function").unwrap();
        let part = function_msg.parts.as_ref().unwrap().first().unwrap();
        part.function_response.clone().unwrap()
    }

    #[test]
    fn tool_response_scalar_number_is_wrapped() {
        let req = build_request_with_tool_response("5040");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert!(fr["response"].is_object(), "response must be an object, got {fr:?}");
        assert_eq!(fr["response"]["result"], 5040);
    }

    #[test]
    fn tool_response_array_is_wrapped() {
        let req = build_request_with_tool_response("[1, 2, 3]");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert!(fr["response"].is_object(), "response must be an object, got {fr:?}");
        assert_eq!(fr["response"]["result"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn tool_response_null_is_wrapped() {
        let req = build_request_with_tool_response("null");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert!(fr["response"].is_object(), "response must be an object, got {fr:?}");
        assert!(fr["response"]["result"].is_null());
    }

    #[test]
    fn tool_response_string_is_wrapped() {
        let req = build_request_with_tool_response("\"hello\"");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert!(fr["response"].is_object(), "response must be an object, got {fr:?}");
        assert_eq!(fr["response"]["result"], "hello");
    }

    #[test]
    fn tool_response_object_passes_through_unchanged() {
        let req = build_request_with_tool_response("{\"answer\": 42, \"unit\": \"jiffies\"}");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert_eq!(fr["response"]["answer"], 42);
        assert_eq!(fr["response"]["unit"], "jiffies");
        assert!(
            fr["response"].get("result").is_none(),
            "object must NOT be double-wrapped, got {fr:?}"
        );
    }

    #[test]
    fn tool_response_non_json_content_is_wrapped_as_string() {
        let req = build_request_with_tool_response("plain error text");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert_eq!(fr["response"]["result"], "plain error text");
    }
```

- [ ] **Step 3: Commit the tests as-is (red — to lock in TDD discipline)**

```bash
git add src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs
git commit -m "test(gemini): add regression tests for scalar function_response

Six unit tests covering the Gemini adapter contract that
function_response.response must always be a JSON object (google.protobuf.Struct).

Five of these currently FAIL — they document the bug where scalars/arrays/null/
strings get injected raw into the wire payload, triggering 400 INVALID_ARGUMENT
from Gemini.

The sixth (object passes through) currently passes and guards against accidental
double-wrapping when the fix lands.

Refs: docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md"
```

---

## Task 2: Run the new tests to confirm they fail in the expected way

**Files:**
- None — runs only.

- [ ] **Step 1: Run the six new tests**

```bash
cargo test --lib --package colmena_dag_engine \
  'llm::infrastructure::gemini_adapter::tests::tool_response_' \
  -- --nocapture 2>&1 | tail -60
```

Expected: **5 failures, 1 pass.**
- ❌ `tool_response_scalar_number_is_wrapped` — fails on `assert!(fr["response"].is_object())` (currently `fr["response"]` is `5040`, a `Number`)
- ❌ `tool_response_array_is_wrapped` — fails the same way (currently an `Array`)
- ❌ `tool_response_null_is_wrapped` — fails (currently `Null`)
- ❌ `tool_response_string_is_wrapped` — fails (currently `String`)
- ✅ `tool_response_object_passes_through_unchanged` — passes (the existing code already passes objects through correctly)
- ❌ `tool_response_non_json_content_is_wrapped_as_string` — actually this one **already passes** under the current code (the `unwrap_or_else` branch handles it). So expect **4 fails, 2 pass**.

If you see anything other than "scalars/arrays/null/string fail because of `is_object()` assertion + the two object-shaped tests pass" — STOP and re-read the test code; do not proceed to the fix until the failures match this profile.

- [ ] **Step 2: Do not commit anything yet — proceed to Task 3.**

---

## Task 3: Apply the fix

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs:173-175`

- [ ] **Step 1: Replace the buggy block**

Find this exact block (currently at lines 173-175):

```rust
                    let parsed_content =
                        serde_json::from_str::<serde_json::Value>(message.content())
                            .unwrap_or_else(|_| serde_json::json!({ "result": message.content() }));
```

Replace it with:

```rust
                    // Gemini's `functionResponse.response` is typed as
                    // `google.protobuf.Struct` and only accepts JSON objects.
                    // Wrap non-object values (scalars, arrays, null) in
                    // `{ "result": <value> }`. Objects pass through unchanged
                    // so callers that already return dicts keep their keys.
                    // Non-JSON content (free-form error strings) is wrapped
                    // as a string under the same key.
                    //
                    // See: docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md
                    let parsed_content = match serde_json::from_str::<serde_json::Value>(
                        message.content(),
                    ) {
                        Ok(v) if v.is_object() => v,
                        Ok(v) => serde_json::json!({ "result": v }),
                        Err(_) => serde_json::json!({ "result": message.content() }),
                    };
```

The rest of the `MessageRole::Tool` arm (lines 176 onward — `contents.push(GeminiContent { … function_response: Some(json!({ "name": tool_name, "response": parsed_content })) … })`) stays exactly as-is.

---

## Task 4: Run the tests and confirm all six pass

**Files:**
- None — runs only.

- [ ] **Step 1: Run the six new tests again**

```bash
cargo test --lib --package colmena_dag_engine \
  'llm::infrastructure::gemini_adapter::tests::tool_response_' \
  -- --nocapture 2>&1 | tail -30
```

Expected: `test result: ok. 6 passed; 0 failed`.

If any test still fails, STOP and inspect — the fix block above is the canonical form, do not improvise variants.

---

## Task 5: Run the full LLM module test suite to confirm no regressions

**Files:**
- None — runs only.

- [ ] **Step 1: Run the whole `llm` module test suite**

```bash
cargo test --lib --package colmena_dag_engine 'llm::' 2>&1 | tail -20
```

Expected: all tests pass. The Gemini adapter has ~3 existing tests (file handling) and other adapters have their own — none should be affected.

- [ ] **Step 2: Run the integration test pass too (catches doctests + cross-module breakage)**

```bash
cargo test --verbose --package colmena_dag_engine 2>&1 | tail -30
```

Expected: all tests pass. Per CLAUDE.md, CI uses `cargo test --verbose` (unit + integration + doctests), so this is the canonical pre-push check. Ignored tests (requiring `DATABASE_URL`, `TAVILY_API_KEY`, etc.) stay ignored — that is correct.

---

## Task 6: Lint, format, and confirm the deny-warnings build is clean

**Files:**
- None — runs only.

- [ ] **Step 1: Format**

```bash
cargo fmt
```

Expected: no diff (the inserted code follows rustfmt defaults — if you see a diff, accept the formatter's choice; do not re-edit by hand).

- [ ] **Step 2: Clippy on the touched package**

```bash
cargo clippy --package colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail -20
```

Expected: zero warnings. (Per CLAUDE.md, the crate has `[lints.rust] warnings = "deny"`.)

- [ ] **Step 3: Build the binary so subsequent E2E tasks have a fresh artifact**

```bash
cargo build --bin dag_engine 2>&1 | tail -5
```

Expected: `Finished dev profile`.

---

## Task 7: End-to-end smoke test — run the three repro graphs and verify Gemini now works

**Files:**
- None — runs only. The graphs were created during verification and live at `/tmp/colmena_e2e/graphs/scalar_tool_{openai,anthropic,gemini}.json`.

If `/tmp/colmena_e2e/graphs/` is missing (e.g. machine was rebooted, /tmp got cleaned), recreate the three files from their canonical content captured in the SSE event dumps still under `/tmp/colmena_e2e/{openai,anthropic,gemini}.sse` (the `node-start` event echoes the whole graph config). If those are also gone, copy the exact JSON from this plan's git history or rebuild a minimal equivalent — a single `llm_call` node with `python_script` exposed as a tool, with `prompt = "What is 7 factorial? Use runCode and assign the integer answer directly to output."` and the system message instructing the LLM NOT to wrap the result in a dict.

- [ ] **Step 1: Load env and run the Gemini graph (the bug case)**

```bash
mkdir -p /tmp/colmena_e2e
set -a && source .env && set +a
cargo run --quiet --bin dag_engine -- run /tmp/colmena_e2e/graphs/scalar_tool_gemini.json \
  --agent-session-id verify_gemini_fix > /tmp/colmena_e2e/gemini_after_fix.sse 2>&1
echo "EXIT=$?"
```

Expected exit code: 0.

- [ ] **Step 2: Inspect the Gemini SSE — look for the smoking-gun events that were missing before**

```bash
grep -E '"type":"(tool-input-available|tool-output-available|node-end)"' \
  /tmp/colmena_e2e/gemini_after_fix.sse
```

Expected: at least one `tool-input-available` (Gemini emitted the function call), one `tool-output-available` with `"output":5040` (the python_script executed and returned the scalar), and a `node-end` with a non-empty `"result"` field containing the number 5040 in some natural-language form (e.g. `"5040"` or `"is 5040"`).

Before the fix, the SSE had `node-end` with `"result":""` and `"completion_tokens":0` — no tool events at all. If you still see that profile, the fix did not take effect; rebuild and re-run.

- [ ] **Step 3: Run the OpenAI and Anthropic graphs to confirm no collateral damage**

These should already work (they did before the fix) — just make sure they still do.

```bash
cargo run --quiet --bin dag_engine -- run /tmp/colmena_e2e/graphs/scalar_tool_openai.json \
  --agent-session-id verify_openai_after_fix > /tmp/colmena_e2e/openai_after_fix.sse 2>&1
cargo run --quiet --bin dag_engine -- run /tmp/colmena_e2e/graphs/scalar_tool_anthropic.json \
  --agent-session-id verify_anthropic_after_fix > /tmp/colmena_e2e/anthropic_after_fix.sse 2>&1
grep -c '"type":"tool-output-available"' /tmp/colmena_e2e/{openai,anthropic}_after_fix.sse
```

Expected: each file has at least 1 `tool-output-available` event, exit codes 0 from both runs, and the OpenAI/Anthropic SSE results contain "5040" in the final node-end `result` text.

---

## Task 8: Defensive audit of the other adapters

**Files:**
- Read-only inspection: `src/libs/colmena/src/llm/infrastructure/openai_adapter.rs`, `src/libs/colmena/src/llm/infrastructure/anthropic_adapter.rs`

This is documented as a known-clean check in the bug report, but reproducing the proof here so the engineer doesn't have to take that on faith.

- [ ] **Step 1: Confirm OpenAI passes tool content as a plain string**

```bash
grep -n -B1 -A8 'MessageRole::Tool\|tool_call_id\|"content"' \
  src/libs/colmena/src/llm/infrastructure/openai_adapter.rs | head -40
```

Expected: see something like `message_json["content"] = json!(msg.content())` — content goes in as a string, never parsed back to a `Value`. No fix needed.

- [ ] **Step 2: Confirm Anthropic passes tool content as an opaque string**

```bash
grep -n -B1 -A10 'MessageRole::Tool' \
  src/libs/colmena/src/llm/infrastructure/anthropic_adapter.rs | head -25
```

Expected: see `AnthropicContentBlock::ToolResult { tool_use_id, content: message.content().to_string() }` — content stays a `String`. No fix needed.

- [ ] **Step 3: If either adapter shows a pattern like `from_str::<Value>(message.content()).unwrap_or_else(...)` injected raw into a structured field — STOP and flag it.** That would mean the bug exists in another adapter and this plan needs another task. (As of the verification run on 2026-06-01 on branch `feature/docs`, neither adapter has it — but verify, do not assume.)

---

## Task 9: Update `CLAUDE.md` "Current Status" with a one-line shipped note

**Files:**
- Modify: `CLAUDE.md` (the "Current Status" section near the bottom — preserve its existing bullet-list style, see how the most recent shipped items are documented, e.g. the "Layered tool context shipped" or "Attachment uniform resolution" bullets).

- [ ] **Step 1: Add the bullet**

Insert a new bullet at the **top** of the "Current Status" bullet list (most-recent-first ordering, matching how the existing bullets are arranged), right after the line that starts with `**Active development on \`develop\`**.`

Use exactly this text (replace `<commit-sha>` with the actual short SHA of the commit you'll create in Task 10 — if you don't know it yet, write `pending` and update after committing):

```markdown
- **Gemini scalar tool response fix shipped 2026-06-01** — `gemini_adapter.rs` now wraps any non-object `LlmMessage::Tool` content (scalars, arrays, null, strings) in `{ "result": <value> }` before injecting into `functionResponse.response`. Fixes silent agent death (`completion_tokens: 0`, empty `result`) on Gemini agents whose tool returns a non-dict — e.g. a `python_script` that assigns `output = 5040`. OpenAI/Anthropic adapters audited clean. ADP unaffected (wire-format change only — never crosses the SSE boundary). See [`docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md`](docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md).
```

---

## Task 10: Final commit

**Files:**
- None — committing what's already staged from Task 3 and Task 9.

- [ ] **Step 1: Stage and review**

```bash
git status
git diff --staged
```

Expected: the staged diff contains exactly two files — `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs` (the 4-line fix; the tests were committed in Task 1) and `CLAUDE.md` (one new bullet).

- [ ] **Step 2: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs CLAUDE.md
git commit -m "$(cat <<'EOF'
fix(gemini): wrap non-object tool responses in {result: ...}

Gemini's functionResponse.response field is typed as google.protobuf.Struct,
which only accepts JSON objects. Scalars, arrays, booleans, and null are
rejected with 400 INVALID_ARGUMENT — observable as Gemini agents dying
silently after one turn (empty result, 0 completion tokens, no error event).

The adapter previously only wrapped tool content in {result: ...} when the
content failed to parse as JSON. Extend the same envelope to cover the case
"parsed successfully, but not an object" so any non-dict tool output
survives the round-trip.

Objects pass through unchanged — callers that already return dicts keep
their keys, no double-wrapping.

OpenAI and Anthropic adapters audited clean (they pass tool content as
opaque strings, never as raw JSON values).

ADP unaffected — wire-format change between Colmena and the Gemini REST
API only; never crosses the SSE boundary that ADP consumes.

Verified end-to-end with /tmp/colmena_e2e/graphs/scalar_tool_*.json against
all three providers; Gemini now returns the expected final response where
it previously went mute.

Refs: docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3: Update the `CLAUDE.md` bullet with the real commit SHA if you wrote `pending`**

```bash
SHA=$(git rev-parse --short HEAD)
# Edit CLAUDE.md to replace "pending" with $SHA in the bullet you just added,
# then amend the commit (this is one of the rare amend-justified cases — the
# bullet refers to its own commit, so the SHA can only be known after committing).
git add CLAUDE.md
git commit --amend --no-edit
```

- [ ] **Step 4: Verify**

```bash
git log -1 --stat
```

Expected: one commit touching `gemini_adapter.rs` (~12 line diff, the comment + the new match block) and `CLAUDE.md` (1 line added).

---

## Self-Review Notes

- ✅ **Spec coverage:** every section of the bug report (root cause, ubicación exacta, repro, fix propuesto, tests sugeridos, notas adicionales about other adapters and Python guide compat) maps to a task above. The "compat con Python Node Developer Guide" point doesn't require a code task — the fix makes the guide's `output = 52` example continue to work, which is a property the tests now verify (Task 1 covers exactly that case via `tool_response_scalar_number_is_wrapped`).
- ✅ **Placeholder scan:** no `TODO`/`TBD`/"appropriate"/"handle edge cases" — every step has either concrete code, a concrete command, or a concrete file diff.
- ✅ **Type consistency:** `ProviderKind::Google` (not `::Gemini` — the bug report's tests had this wrong, corrected in Task 1). `convert_messages` returns `(Option<String>, Vec<GeminiContent>)` — destructured as `(_, contents)`. `GeminiContent.parts` is `Option<Vec<GeminiPart>>`. `GeminiPart.function_response` is `Option<Value>`. All match the file as of `gemini_adapter.rs` on `feature/docs`.
- ✅ **Risk:** the change is isolated to one match arm; the test suite catches accidental double-wrapping of objects.
