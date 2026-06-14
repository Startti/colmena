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
**loop** (the agent calling the *same* tool with the *same* args **in a row**, not
learning). Distinct calls — even many — are progress and should be free, and a
*streak* of repeats resets the moment the model does something different. A
separate, high, background turn ceiling remains as the pure cost/termination
backstop.

---

## 2. Semantics remap (no new public knob)

| Concept | Before | After |
|---|---|---|
| Public key `max_iterations` (graph/ADP) | hard cap on LLM **turns**, default 10 | **loop budget**: max *consecutive* repeats of one `(name+args)` signature, default **3** (→ `AgentRunParams.max_tool_repeats`) |
| Hard turn ceiling | == `max_iterations` (configurable) | per-run `AgentRunParams.max_turns`; default from **env `COLMENA_HARD_TURN_CAP` (fallback 50)**; not graph-configurable |
| Exhaustion outcome | `Err(MaxIterationsReached)` | **forced final synthesis** → `Ok(response)` |

- The **same** JSON key `max_iterations` keeps flowing from ADP; only its *effect*
  changes (now controls repeats, not turns). ADP touches nothing.
- A legacy graph with `max_iterations: 10` now allows 10 consecutive repeats of a
  signature *plus* the 50-turn ceiling → strictly more permissive, never dies early.
- **No explicit `max_tool_repeats` key** in the graph (decided). The internal
  param is named `max_tool_repeats`; it is fed solely from `max_iterations`.

### 2.1 Two knobs, six callers

`AgentService.run` is used by **six** nodes, not just `llm_call`: `planner`,
`reactor`, `critic`, `orchestrator`, and `extract_with_schema` all run the loop
too — and all five pass `max_iterations: Some(1)` today, meaning **single-shot**
(exactly one turn, no tool loop). The redesign must preserve that.

So `AgentRunParams` carries **two** independent knobs:

| Field | Meaning | Default | Who sets it |
|---|---|---|---|
| `max_tool_repeats: Option<usize>` | loop-guard streak budget | 3 | `llm_call` (from public `max_iterations`) |
| `max_turns: Option<usize>` | hard turn ceiling for this run | env `COLMENA_HARD_TURN_CAP` / 50 | the 5 single-shot nodes set `Some(1)`; `llm_call` leaves `None` |

The single-shot nodes set `max_turns: Some(1)` and leave `max_tool_repeats` at
default (irrelevant — one turn can't repeat). With no tools they return on turn 1
exactly as before; the loop guard never engages for them.

---

## 3. Loop detector (per-signature **streak**)

### 3.1 Signature

`Signature = canonical_string(tool_name, arguments)` — arguments serialized with
**recursively sorted object keys** so semantically-identical calls collapse to one
key regardless of field order. Helper: `tool_call_signature(name, &args) -> String`.

### 3.2 State (consecutive streak, resets on change)

The guard counts **consecutive** repeats of the current signature. State is a
single streak, **not** a lifetime map:

```rust
let mut streak_sig: Option<String> = None;  // signature of the current streak
let mut streak_count: u32 = 0;               // how many times in a row
let mut streak_first: String = String::new();// raw output of this streak's 1st exec
```

Per processed tool call with signature `S`: if `S == streak_sig` → `streak_count
+= 1`; else → **reset** (`streak_sig = S`, `streak_count = 1`, clear
`streak_first`). So emitting *any* different signature resets the counter — that
is the "reset when the model changes what it's doing" behavior. Example
`A,B,A,B,B,C,B,C`: B peaks at a streak of **2** (one nudge), then `C` resets it —
never accumulates to 4.

### 3.3 Per tool-call decision (default `max_tool_repeats = 3`)

After updating the streak for signature `S`:

| `streak_count` | Action |
|---|---|
| 1 | **Execute** the tool, store `streak_first`, push real tool result. |
| 2 | `2 < 3` → **nudge**: do NOT execute; push `streak_first` + redirect text. |
| 3 | `3 >= 3` → flag **rescue**. |

Rule: `streak_count >= max_tool_repeats` → rescue; else if `streak_count >= 2` →
nudge; else execute. Default ⇒ **one real call + one nudge, then rescue** ("nudge
en la 2ª, rescate en la 3ª"). A pure 2-cycle (`A,B,A,B,…`) never trips the guard
(each streak is 1) — the `max_turns` ceiling catches it with a graceful synthesis.

### 3.4 Nudge mechanics

- The duplicate is **not re-executed** (saves the call / side effects).
- The OpenAI/Anthropic contract requires a tool response for every
  `tool_call_id`, so we still push a tool-result message for that id. Its content
  is the **prior result** plus a redirect line drawn from the text registry, e.g.:
  *"Ya llamaste esta tool con estos argumentos; el resultado está arriba. Usalo o
  probá algo distinto — no repitas la misma llamada."*
- The turn still counts toward `max_turns` (the LLM consumed a turn).

### 3.5 Multiple tool calls in one turn

Each tool call in the assistant message is evaluated independently (some may
execute, some nudge). If any one reaches the rescue threshold, we finish pushing
tool responses for **all** ids in that turn first (history must stay valid), then
break to synthesis.

---

## 4. Rescue (forced final synthesis)

Two triggers, one unified outcome:

1. A signature streak reaches `max_tool_repeats` (loop guard), **or**
2. The turn loop reaches `max_turns` (env `COLMENA_HARD_TURN_CAP` / 50).

On either, instead of `Err`:

- Make **one terminal LLM call** built from the full `messages` history but with
  **tools removed from the request** (not merely `tool_choice: none`), plus a
  final instruction message from the text registry: *"Llegaste al límite. Dá tu
  mejor respuesta final con lo que ya tenés y aclará qué quedó incompleto."*
- This terminal call **does not count** toward `max_turns`.
- If `on_token` is set, the synthesis **streams** like a normal final turn.
- Persist the synthesis message to `conversation_repository`; return it as
  `Ok(response)` (with the accumulated `all_tool_calls_executed` attached as today).

`MaxIterationsReached` stays in the `LlmError` enum (compat) but is **no longer
returned** by the normal path. If the synthesis call itself fails, that provider
error propagates as usual.

---

## 5. Config wiring

- `AgentRunParams.max_iterations` field is **renamed** to `max_tool_repeats:
  Option<usize>` (default in the loop: `unwrap_or(3)`), and a **new** field
  `max_turns: Option<usize>` is added (default resolved from env).
- `llm.rs` stops treating `max_iterations` as a turn cap. It reads the same JSON
  key (`inputs["max_iterations"]` → `config["max_iterations"]`, default 3) and
  assigns it to `params.max_tool_repeats`, leaving `max_turns: None` (→ env/50).
  Both its `AgentRunParams` construction sites updated; the
  `llm_call_max_iterations_resolved` log relabels.
- The **5 single-shot callers** (`planner`, `reactor`, `critic`, `orchestrator`,
  `extract_with_schema`) change `max_iterations: Some(1)` →
  `max_turns: Some(1), max_tool_repeats: None`, preserving one-turn behavior.
- The turn loop becomes `for _iteration in 0..max_turns` where
  `let max_turns = params.max_turns.unwrap_or_else(default_hard_turn_cap);` and
  `fn default_hard_turn_cap()` reads `COLMENA_HARD_TURN_CAP` (positive usize),
  falling back to `50`.

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
- **Streak reset:** `A,A,B,A` → the second `A`-run starts fresh (the `B` resets
  the streak); never rescues.
- `max_turns` reached with all-distinct calls → forced synthesis (`Ok`).
- Synthesis call does not count toward the ceiling; streams when `on_token` set.
- `max_tool_repeats` wiring: value flows from node `config.max_iterations`;
  absent → default 3. `max_turns: Some(1)` → single-shot (one turn) behavior.
- `default_hard_turn_cap()` honors `COLMENA_HARD_TURN_CAP` and falls back to 50.
- Rewrite `test_agent_service_max_iterations` to the new `Ok`-synthesis contract.

**E2E (real, save SSE to `/tmp/colmena_e2e/`):**

- Reliable deterministic repro via a MockAdapter graph that always emits one
  fixed tool call → assert nudge then synthesis (no error).
- Best-effort real-LLM FRIKO repro with `gemini-2.5-flash`: confirm the agent
  that used to spin and die now gets nudged off the loop and returns a final
  answer instead of `MaxIterationsReached`.

---

## 9. Files (anticipated)

- `src/libs/colmena/src/llm/application/agent_service.rs` — `default_hard_turn_cap`,
  `tool_call_signature`, streak state + nudge/rescue logic, forced synthesis,
  `AgentRunParams` field rename + new `max_turns`, updated tests.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — read
  `max_iterations` → `max_tool_repeats` (default 3), `max_turns: None`, update
  both `AgentRunParams` sites + the resolved-value log.
- `src/.../nodes/{planner,reactor,critic,orchestrator}.rs` and
  `nodes/util/extract_with_schema.rs` — `max_iterations: Some(1)` →
  `max_turns: Some(1), max_tool_repeats: None` (preserve single-shot).
- `text/prompts/agent_loop/repeat_nudge.*`, `rescue_synthesis.*` — new LLM text.
- `docs/developer_guide/14_llm_deep_dive.md` — document the new `max_iterations`
  semantics (loop budget) + the env-backed turn ceiling + rescue behavior.

---

## 10. Out of scope

- A **graph/ADP-configurable** turn ceiling — the ceiling is env-only
  (`COLMENA_HARD_TURN_CAP`) plus the internal `max_turns: Some(1)` single-shot
  override; it is intentionally not exposed in graph JSON.
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
