# src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs

**Layer:** infrastructure  
**Purpose:** Implements a Human-in-the-Loop suspend node that pauses graph execution to ask users questions. Handles both suspend path (building canonical Q/A format) and resume path (parsing ID-keyed answers via `cfg_or_input` to support dual edge/tool usage).

## Symbols

- `SuspendNode` (struct, pub) — Zero-sized marker struct for the HITL pause node type; implements ExecutableNode.
- `cfg_or_input` (fn, private) — Resolves config field from config first (edge usage), then inputs (tool usage where executor merges fixed_config/node_schema into inputs).
- `ExecutableNode::execute` (async fn, impl, pub) — Dual-path executor: resume path parses __colmena_resume_answer and returns user answer; suspend path builds canonical question object with id/question/type/options and emits __colmena_status: SUSPENDED.
- `ExecutableNode::default_input` (fn, impl, pub) — Returns "question" as default input port name.
- `ExecutableNode::default_output` (fn, impl, pub) — Returns "answer_received" as default output port name.
- `ExecutableNode::schema` (fn, impl, pub) — Returns empty JSON object (no schema validation).
- `empty_observer` (fn, private, test) — Test helper returning None for observer.
- `empty_inputs` (fn, private, test) — Test helper returning empty HashMap.
- `suspend_emits_open_by_default` (async test fn) — Verifies suspend emits "open" question type by default.
- `suspend_emits_choice_with_options` (async test fn) — Verifies suspend emits "choice" type with option list.
- `suspend_uses_explicit_id` (async test fn) — Verifies explicit config.id is used instead of fallback.
- `suspend_preserves_legacy_question_field` (async test fn) — Verifies both legacy "question" field and canonical "questions" array are present.
- `config_without_id_is_rejected_at_execute` (async test fn) — Verifies missing id raises config.id-required error.
- `resume_with_qa_open_yields_answer` (async test fn) — Verifies resume path parses Q[id]/A[id] format and extracts answer.
- `resume_with_qa_choice_accepts_option_value` (async test fn) — Verifies resume accepts values matching suggested options.
- `resume_with_qa_choice_accepts_free_text` (async test fn) — Verifies options are UX suggestions only; free-text is accepted.
- `suspend_reads_id_and_question_from_inputs_when_config_empty` (async test fn) — Verifies cfg_or_input fallback resolves id/question from inputs when config is empty (tool-path usage).
- `config_id_takes_precedence_over_inputs_id` (async test fn) — Verifies config field precedence over inputs fallback.
- `resume_path_propagates_parser_errors` (async test fn) — Verifies Q/A parsing errors are propagated on resume.

## File-level notes

- **Dual-usage pattern**: `cfg_or_input` enables suspend-as-tool by supporting both node config path (edge, static config) and tool path (executor merges fixed_config/node_schema into inputs, config is empty). See CLAUDE.md "Tool Config Standard" and docs/developer_guide/19_nested_agents_and_subgraphs.md.
- **ID validation**: `is_valid_qa_id()` enforces [A-Za-z0-9_-]{1,64} format for stable Q/A mapping.
- **Legacy + canonical output**: Emits both legacy `question` string and canonical `questions` array (object with id/question/type/options) for backwards compatibility and structured parsing.
- **Schema is empty**: `schema()` returns `{}` intentionally — no per-field validation; config/inputs are flexible and trusted at the node level.
- **Comprehensive test coverage**: 12 tests cover suspend (defaults, choice, legacy field), resume (Q/A parsing, free-text), tool-path fallback, config precedence, and error paths.
- **Parser dependency**: Uses `parse_qa_response()` from `qa_response_parser` module to handle ID-keyed format; errors bubble up with suspend: prefix.
