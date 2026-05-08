# Suspend Nodes — Q/A Response Format

**Date:** 2026-05-08
**Status:** Design
**Scope:** `suspend`, `secure_suspend`

## Problem

The current response parsing for suspend-style nodes diverges by node:

- **`suspend`** (classic) does not parse the `--answer` payload at all — it passes the raw string through as `answer_received`. Operators must know whether the downstream consumer expects raw text, JSON, or a specific format.
- **`secure_suspend`** uses an **anchored parser**: each `secret.question` literal must appear in the answer in order, and the value is whatever sits between question N and question N+1. This forced operators to copy-paste the EXACT question text emitted by the LLM (including punctuation, whitespace, multilingual phrasing). One real test failure in the canvas-builder e2e validation: the operator wrote `"usuario"` but the LLM emitted `"Por favor, introduce tu nombre de usuario"`, so parsing rejected the run.

Two suspend nodes, two contracts, both fragile.

## Solution: ID-keyed Q/A Format

All suspend-flavored nodes accept a **single canonical response format** keyed by **stable per-question IDs**, not position:

```
Q[<id>]: <question text>
A[<id>]: <answer text>
Q[<id2>]: <question text>
A[<id2>]: <answer text>
...
```

- `Q[<id>]:` and `A[<id>]:` are **literal prefixes** at the start of a line. `<id>` is the question's stable identifier:
  - For `suspend`: the value of `config.id` (now **required** — no fallback).
  - For `secure_suspend`: the value of `secrets[i].name`.
- ID character set: `[A-Za-z0-9_-]{1,64}` (already what `name` and `id` accept today).
- Order-independent: the operator can answer in any order; the parser binds by ID.
- Answers may span multiple lines; an answer ends when the parser sees the next `Q[…]:` prefix at the start of a line, or end of input.
- Each expected ID must appear in the response **exactly once** as `A[<id>]:`. Duplicates → error. Missing IDs → error. Unknown IDs (not in the expected set) → error.
- Empty answers (`A[<id>]:` followed by nothing or only whitespace before the next prefix or EOF) → error.
- The text after `Q[<id>]:` is **echoed for human readability and round-trip diffing only — the parser does not validate that it matches the original question text**. Rationale: ID binding makes question text irrelevant; the LLM can rephrase, translate, or reformat freely without breaking parsing.

### Single-question example (`suspend` classic)

Graph defines `suspend` with `config.id: "confirm_transfer"`. Operator runs:
```bash
cargo run -- run graph.json --agent-session-id agent_x --answer "Q[confirm_transfer]: Confirm transfer?
A[confirm_transfer]: yes"
```

Parser yields `{"confirm_transfer": "yes"}`. Node outputs `answer_received: "yes"`.

### Multi-question example (`secure_suspend`)

LLM tool emits two secrets named `username` and `password`; operator answers:
```bash
cargo run -- run graph.json --agent-session-id agent_x --answer "Q[username]: API key
A[username]: sk-live-abc123
Q[password]: API secret
A[password]: shh-secret-def456"
```

Or in any order:
```bash
--answer "Q[password]: API secret
A[password]: shh-secret-def456
Q[username]: API key
A[username]: sk-live-abc123"
```

Both yield `{"username": "sk-live-abc123", "password": "shh-secret-def456"}` and the node persists each as a secure value under its name.

### Multi-line answer example

```
Q[private_key]: Paste your private key
A[private_key]: -----BEGIN PRIVATE KEY-----
MIIEvQIBADAN...
-----END PRIVATE KEY-----
Q[fingerprint]: Confirm fingerprint
A[fingerprint]: ab:cd:ef
```

Parser yields the multi-line PEM block as `A[private_key]`, the fingerprint as `A[fingerprint]`.

### Choice-question support (`suspend` only)

When `question_type: "choice"`:
- The configured `options` array is **a suggestion list** for the UX (clients may render them as quick-pick buttons), **not a whitelist**.
- The parser does **not** validate the answer against `options`. The operator may pick one of the suggested options OR write any free-text answer.
- Example: `Q[env]: Pick env\nA[env]: production` is valid; `A[env]: review-app-123` is also valid even if it's not in `options`.

Rationale: real-world choice questions almost always have an "other / specify" escape hatch. Treating `options` as suggestions keeps the format simple (single `A[<id>]`) and avoids forcing the caller to model an "Other (specify)" companion question.

`secure_suspend` does NOT support choice questions in this iteration (open only). Adding choice support is a separate spec.

### `suspend.config.id` is now required

Previously `id` defaulted to `__node_id`. With ID-keyed parsing, an explicit ID makes the contract unambiguous and survives copy-paste / refactors of the graph (renaming a node no longer changes the resume payload schema).

A `suspend` node config without `id` is rejected at execute time with `suspend: config.id is required`.

## Backward Compatibility

**None.** This is a hard cutover.

- The anchored parser in `secure_suspend` is removed.
- `suspend`'s pass-through `answer_received` becomes a parsed string.
- No flag, no fallback. Old graphs with old-format `--answer` payloads will fail loudly (parser error).

Justification: the canvas-builder pair is the first real consumer of `secure_suspend`, and it isn't in production yet. `suspend` is more entrenched but its contract (`--answer "yes"`) is so simple that updating callers is trivial — any operator writing one-question suspend now writes `"Q1: ?\nA1: yes"`. The cutover is one short search-and-replace and the new format is unambiguous forever after.

## Parser Location

A single shared parser lives in a new module:

- `src/libs/colmena/src/dag_engine/infrastructure/nodes/qa_response_parser.rs`

Both `suspend.rs` and `secure_suspend.rs` import it. The parser API:

```rust
pub fn parse_qa_response(
    answer: &str,
    expected_ids: &[&str],
) -> Result<HashMap<String, String>, QaParseError>;

pub enum QaParseError {
    InvalidIdSyntax { token: String },         // bracket missing, charset violation
    UnknownId { id: String },                  // A[x] but x not in expected_ids
    DuplicateId { id: String },                // A[x] appears twice
    MissingId { id: String },                  // expected_ids[i] not present
    EmptyAnswer { id: String },
    OrphanQuestion { id: String },             // Q[x] without matching A[x]
}
```

The parser:
1. Walks the input line-by-line, finding `Q[<id>]:` and `A[<id>]:` line-start anchors.
2. Collects answer bodies between each `A[<id>]:` and the next `Q[…]:`/`A[…]:` start-of-line or EOF.
3. Validates ID set membership and uniqueness against `expected_ids`.
4. Returns a `HashMap<id, answer>`.

Note: `Q[<id>]:` lines are scanned only for syntax validation (orphan detection); their contents are not consumed.

## Affected Code (high level)

| File | Change |
|------|--------|
| `nodes/qa_response_parser.rs` | NEW: shared parser + error type + tests |
| `nodes/suspend.rs` | Resume path: parse `__colmena_resume_answer` with parser, expect 1 answer; output that as `answer_received`. Choice validation against `options`. |
| `nodes/secure_suspend.rs` | Remove anchored parser; call shared parser with `expected_count == secrets.len()` |
| `node_configurations.json` | Update `suspend` and `secure_suspend` resume-answer documentation to describe Q/A format |
| `agent_context/node_ports_reference.md` | §"suspend" + §"secure_suspend" — update example `--answer` payloads |
| `developer_guide/13_security_strategy.md` | Strategy 6 examples: update to Q/A |
| `tests/graphs/advanced/secure_suspend_login_e2e.json` (and similar fixtures) | Update any embedded operator answers if present |
| `MEMORY.md` index + memory files referencing `--answer` patterns | Update guidance |

## Tests

### Unit (parser)

- `parses_single_id_pair`
- `parses_multiple_ids_in_declared_order`
- `parses_multiple_ids_in_reversed_order` (proves order-independence)
- `preserves_internal_newlines_in_answer`
- `tolerates_no_space_after_colon` (`A[x]:value` and `A[x]: value` both work)
- `does_not_validate_question_text_matches`
- `errors_on_invalid_id_syntax` (e.g. `A[bad space]:` or `A[<>]:`)
- `errors_on_unknown_id`
- `errors_on_duplicate_id`
- `errors_on_missing_id`
- `errors_on_empty_answer`
- `errors_on_orphan_q_without_a`

### Unit (suspend.rs)

- `config_without_id_is_rejected_at_execute`
- `resume_with_qa_open_yields_answer`
- `resume_with_qa_choice_accepts_option_value` (option matches → ok)
- `resume_with_qa_choice_accepts_free_text` (option NOT in list → still ok; options are suggestions)
- `resume_path_propagates_parser_errors`

### Unit (secure_suspend.rs)

- Existing parser tests deleted.
- New tests:
  - 3 secrets, multi-line value in one of them, parser error propagation.
  - Order-independent: secrets declared `[a, b, c]`, answer in order `[c, a, b]`, all resolve correctly.

### Integration

- `tests/secure_suspend_integration.rs`: update existing smoke test to Q/A format.
- `tests/graphs/basic/secure_suspend_smoke.json`: no change to graph; the test caller updates the `--answer` payload it uses to drive the resume.

## Documentation

This spec must be reflected in:

- `13_security_strategy.md` — Strategy 6 examples
- `30_database_schema.md` — no change (schema unaffected)
- `node_configurations.json` — both suspend entries
- `node_ports_reference.md` — both suspend entries
- `AGENT_FEATURES_INDEX.md` — `secure_suspend` entry adds a one-line note about format

## Out of Scope

- Choice questions in `secure_suspend`. (Open-only for this iteration.)
- Validating that echoed `Q<N>:` text matches the original question. (Position-based binding only.)
- Localizing the `Q`/`A` prefix. (English literals.)
