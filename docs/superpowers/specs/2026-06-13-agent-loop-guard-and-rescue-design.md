# Design — Agent loop: per-signature loop guard + graceful rescue

**Date:** 2026-06-13
**Status:** Approved (brainstorm) — pending implementation plan
**Scope:** Colmena LLM agent loop (`agent_service.rs`) + its node wiring
(`llm.rs`) + LLM-facing text registry. No new public config keys. ADP unchanged
(same `max_iterations` field, new effect).

---

## 1. Problem & goal

Today the agent ReAct loop is bounded by `max_iterations` counted **once per LLM
turn** (`for _iteration in 0..max_iter`, default 10). Two problems:

1. **Legitimate multi-step work dies prematurely.** A productive agent that
   reads four sheets and then iterates pandas (the FRIKO comparison) burns
   through 10 turns and dies with `Err(MaxIterationsReached)` — even though every
   turn was forward progress (distinct tool calls). The per-turn counter cannot
   tell "making progress" from "stuck".

2. **Hitting the limit yields nothing useful.** On exhaustion the loop returns a
   hard error; the user gets no answer, just a failure.

**Goal.** Make the user-facing limit measure the thing that actually matters —
**spinning on the same call** — instead of turns, and never kill the run: when a
limit is reached, force a final answer ("rescue") instead of erroring.

**Key realization.** The danger `max_iterations` really guards against is a
**loop** (the agent calling the *same* tool with the *same* args, not learning).
Distinct calls — even many — are progress and should be free. A separate, high,
background turn ceiling remains as the pure cost/termination backstop.

---

## 2. Semantics remap (no new public knob)

| Concept | Before | After |
|---|---|---|
| Public key `max_iterations` (graph/ADP) | hard cap on LLM **turns**, default 10 | **loop budget**: max repeats of one `(name+args)` signature, default **3** |
| Hard turn ceiling | == `max_iterations` (configurable) | **internal constant `HARD_TURN_CAP = 50`** (background, not configurable) |
| Exhaustion outcome | `Err(MaxIterationsReached)` | **forced final synthesis** → `Ok(response)` |

- The **same** JSON key `max_iterations` keeps flowing from ADP; only its *effect*
  changes (now controls repeats, not turns). ADP touches nothing.
- A legacy graph with `max_iterations: 10` now allows 10 repeats of a signature
  *plus* the 50-turn ceiling → strictly more permissive, never dies early.
- **No explicit `max_tool_repeats` key** in the graph (decided). The internal
  param is named `max_tool_repeats`; it is fed solely from `max_iterations`.

---

## 3. Loop detector (per-signature)

### 3.1 Signature

`Signature = canonical_string(tool_name, arguments)` — arguments serialized with
**recursively sorted object keys** so semantically-identical calls collapse to one
key regardless of field order. Helper: `tool_call_signature(name, &args) -> String`.

### 3.2 State

In the loop, a map keyed by signature:

```rust
struct SigEntry { count: u32, first_result: serde_json::Value }
let mut seen: HashMap<String, SigEntry> = HashMap::new();
```

`count` increments each time the LLM **emits** that signature. `first_result`
stores the result of the one real execution, so a nudge can echo it.

### 3.3 Per tool-call decision (default `max_tool_repeats = 3`)

When the LLM emits a tool call with signature `S`:

| Occurrence | `count` after inc | Action |
|---|---|---|
| 1st | 1 | **Execute** the tool, store `first_result`, push real tool result. |
| 2nd | 2 | `2 < 3` → **nudge**: do NOT execute; push a tool result = `first_result` + redirect text. |
| 3rd | 3 | `3 >= 3` → flag **rescue**. |

Rule: `count >= max_tool_repeats` → rescue; otherwise if `count > 1` → nudge;
else execute. So with the default a signature gets exactly **one real call + one
nudge, then rescue** ("nudge en la 2ª, rescate en la 3ª").

### 3.4 Nudge mechanics

- The duplicate is **not re-executed** (saves the call / side effects).
- The OpenAI/Anthropic contract requires a tool response for every
  `tool_call_id`, so we still push a tool-result message for that id. Its content
  is the **prior result** plus a redirect line drawn from the text registry, e.g.:
  *"Ya llamaste esta tool con estos argumentos; el resultado está arriba. Usalo o
  probá algo distinto — no repitas la misma llamada."*
- The turn still counts toward `HARD_TURN_CAP` (the LLM consumed a turn).

### 3.5 Multiple tool calls in one turn

Each tool call in the assistant message is evaluated independently (some may
execute, some nudge). If any one reaches the rescue threshold, we finish pushing
tool responses for **all** ids in that turn first (history must stay valid), then
break to synthesis.

---

## 4. Rescue (forced final synthesis)

Two triggers, one unified outcome:

1. A signature reaches `max_tool_repeats` (loop guard), **or**
2. The turn loop reaches `HARD_TURN_CAP` (50).

On either, instead of `Err`:

- Make **one terminal LLM call** built from the full `messages` history but with
  **tools removed from the request** (not merely `tool_choice: none`), plus a
  final instruction message from the text registry: *"Llegaste al límite. Dá tu
  mejor respuesta final con lo que ya tenés y aclará qué quedó incompleto."*
- This terminal call **does not count** toward `HARD_TURN_CAP`.
- If `on_token` is set, the synthesis **streams** like a normal final turn.
- Persist the synthesis message to `conversation_repository`; return it as
  `Ok(response)` (with the accumulated `all_tool_calls_executed` attached as today).

`MaxIterationsReached` stays in the `LlmError` enum (compat) but is **no longer
returned** by the normal path. If the synthesis call itself fails, that provider
error propagates as usual.

---

## 5. Config wiring

- `AgentRunParams.max_iterations` field is **renamed** to `max_tool_repeats:
  Option<usize>` (internal name approved). Default applied in the loop:
  `unwrap_or(3)`.
- `llm.rs` stops treating `max_iterations` as a turn cap. It reads the same JSON
  key (`inputs["max_iterations"]` → `config["max_iterations"]`, default 3) and
  assigns it to `params.max_tool_repeats`. Both `AgentRunParams` construction
  sites updated. The `llm_call_max_iterations_resolved` log relabels to reflect
  the new meaning.
- The turn loop becomes `for _iteration in 0..HARD_TURN_CAP` with
  `const HARD_TURN_CAP: usize = 50;` in `agent_service.rs`. Not read from config.

---

## 6. Behavior change & ADP sweep

- **Exhaustion now succeeds** (`Ok` synthesis) where it used to fail (`Err`). The
  `llm_call` node previously propagated the error → node failed; now the node
  succeeds with a useful answer. Strictly better, but it is a behavior change.
- **Required sweep (breaking-change discipline):** check the ADP platform worker
  (`apps/service/ia/platform/{worker,api}/src/`) for any match on
  `MaxIterationsReached` / reliance on the agent erroring at the limit. Expected:
  it only propagates the `Result`, so receiving `Ok` is safe — but verify before
  pushing colmena develop.
- The unit test `test_agent_service_max_iterations` (asserts `Err`) is rewritten
  to assert the synthesized `Ok` response.

### Optional (deferred)

A `terminated_by: "loop_guard" | "max_iterations" | null` field on the response
so ADP/UI can show "cut short by limit". Cheap and low-risk, but **out of scope**
unless ADP asks for it — adding a public response field needs ADP confirmation.

---

## 7. LLM-facing text (registry)

Per repo convention, LLM-facing strings live in `text/`, not hardcoded:

- Nudge redirect line → `text/prompts/agent_loop/repeat_nudge.md` (or `.yaml`).
- Rescue/synthesis instruction → `text/prompts/agent_loop/rescue_synthesis.md`.

Both Spanish-first (matches agent usage), model-agnostic, concise.

---

## 8. Testing

**Unit / integration (`agent_service.rs`, MockAdapter):**

- `tool_call_signature`: key-order independence (`{a,b}` == `{b,a}`),
  name-sensitivity, args-sensitivity.
- Repeated identical signature → 2nd occurrence is **nudged** (tool **not**
  executed; pushed content echoes first result + redirect).
- Signature reaching 3 → **forced synthesis**, returns `Ok` (not `Err`), tools
  absent from the synthesis request.
- Distinct signatures (same tool, different args) → **never** nudged across many
  turns.
- `HARD_TURN_CAP` reached with all-distinct calls → forced synthesis (`Ok`).
- Synthesis call does not count toward the ceiling; streams when `on_token` set.
- `max_tool_repeats` wiring: value flows from node `config.max_iterations`;
  absent → default 3.
- Rewrite `test_agent_service_max_iterations` to the new `Ok`-synthesis contract.

**E2E (real, save SSE to `/tmp/colmena_e2e/`):**

- Reliable deterministic repro via a MockAdapter graph that always emits one
  fixed tool call → assert nudge then synthesis (no error).
- Best-effort real-LLM FRIKO repro with `gemini-2.5-flash`: confirm the agent
  that used to spin and die now gets nudged off the loop and returns a final
  answer instead of `MaxIterationsReached`.

---

## 9. Files (anticipated)

- `src/libs/colmena/src/llm/application/agent_service.rs` — `HARD_TURN_CAP`,
  `tool_call_signature`, `seen` map + nudge/rescue logic, forced synthesis,
  `AgentRunParams` field rename, updated tests.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — read
  `max_iterations` → `max_tool_repeats` (default 3), drop turn-cap usage, update
  both `AgentRunParams` sites + the resolved-value log.
- `text/prompts/agent_loop/repeat_nudge.*`, `rescue_synthesis.*` — new LLM text.
- `docs/developer_guide/14_llm_deep_dive.md` — document the new `max_iterations`
  semantics (loop budget) + the 50-turn background ceiling + rescue behavior.

---

## 10. Out of scope

- A configurable hard turn ceiling — intentionally a fixed background constant.
- `terminated_by` response metadata — deferred (see §6) unless ADP requests it.
- Detecting "same call, different wording" (semantic dedup) — signature is exact
  `(name+args)`; fuzzy matching is explicitly not attempted.
- Caching/replaying tool results beyond the single `first_result` echoed in a
  nudge.
- Any change to the suspend / load_attachment early-exit sentinels (untouched).

---

## 11. Open items for the plan

- Exact canonicalization of args for the signature (recursive key sort vs. a hash)
  — pick the simplest stable form in the plan.
- Whether the nudge echoes the **full** first result or a truncated form when the
  result is large (decision likely: echo as-is; the result is already in history).
- Wording of the two registry strings (nudge, rescue) — finalize in the plan.
