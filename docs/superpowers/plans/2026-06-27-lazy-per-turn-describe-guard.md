# Implementation Plan: Per-turn describe-before-use guard for lazy tool loading

## Summary
Make `lazy_tool_loading` sound by enforcing **describe-before-use per user-turn**:
(1) scope the `discovered_set` to the current turn so each turn re-forces discovery,
and (2) add a dispatch guard that, when the model calls a tool not discovered this
turn, returns that tool's **schema** (a redirect) instead of executing it blind.
Plus make the lazy-mode prompts explicit so the model understands the workflow and
never mistakes the guard's schema-return for the real tool result. Mirrors the
gsheets inspect-before-code guard (per-turn, re-fires each turn).

## Motivation
Live E2E (gemini-2.5-pro, `lazy_tool_loading: true`, prod's mode) writing a sheet
formula: the model called `gsheets_run_python` **without** `describe_tool` first, so
it never loaded the schema/`{{Column}}` guidance → hand-computed A1 (`=F5*V5`, wrong)
→ `invalid_args` → only self-corrected on attempt #2, leaving contaminated narration.

Two root causes:
1. **Blind-call hole.** The per-request rebuild correctly excludes undiscovered tools
   from `tools[]`, but the executor dispatches by name regardless — gemini hallucinates
   the call and it runs blind. The lazy gating is bypassable.
2. **History-wide stickiness.** `reconstruct_discovered_set` scans ALL history, so a
   tool described once stays "discovered" forever. Across turns, compaction can drop
   the actual guidance from context while the set still marks it discovered → the model
   acts in turn 2 without the guidance. (User's insight: enforcement must be per-turn —
   "el turno 2 debe obligar nuevamente", exactly like the gsheets inspect guard, which
   re-inspects cross-turn "por si algo se modificó".)

## Architectural Impact
- **Layers affected**: infrastructure (`dag_engine/infrastructure/nodes/`) + application
  (`llm/application/agent_service.rs`).
- **New traits/ports**: none. **New adapters**: none.
- **Public API / bindings**: no change to `EngineConfig`/`ColmenaEngine`/exported traits.
  One additive optional field on the internal `AgentRunParams`. → ADP worker unaffected
  (no code change; behavior change only for agents already using `lazy_tool_loading`).
- **Scope**: active **only when `lazy_tool_loading: true`**. Eager agents (default
  `false`, the majority) see zero change — no new flag, no breaking change.

## Design

### Piece 1 — `discovered_set` per turn
`reconstruct_discovered_set` (lazy_tools_catalog.rs:30) currently does `for msg in
messages` over the whole history. Scope it to the **current user-turn** = messages from
the last `MessageRole::User` onward.
- Add a small helper `current_turn_slice(messages) -> &[LlmMessage]` that finds the last
  `User` index and returns the tail (whole slice if none — turn 1 / seeded history).
- `reconstruct_discovered_set` stays pure; the caller (llm.rs tools_provider closure,
  line ~3086) passes `current_turn_slice(messages)` instead of `messages`.
- Result: turn 2's fresh `user` message resets the set → cataloged tools drop out of
  `tools[]` → the model must re-`describe_tool`. Within a turn's ReAct loop, this-turn
  describes stay in scope (they're after the last user message).
- **Resume edge case**: suspend/resume injects an answer, not always a fresh `user`
  message. Confirm the resume window still counts the pre-suspend describes as
  "this turn" (the resumed ReAct loop is logically the same turn). If the last `User`
  predates the suspend, the slice naturally includes the describes → correct. Add a
  test for the suspend/resume path.

### Piece 2 — dispatch guard (redirect through describe_tool)
At the dispatch point (agent_service.rs:422, `tool_executor.execute(tool_call)`), the
loop already has `iteration_tools` (line 231) = the per-request `tools[]` from
`tools_provider`. With Piece 1, that set reflects per-turn discovery.

Guard: **before executing**, if `tool_call.name` is NOT in `iteration_tools` AND it is a
known cataloged tool → do NOT execute. Instead, transparently run `describe_tool(name)`
(the executor already renders the curated schema markdown) and return THAT as the tool
result, wrapped with an explicit redirect note (see Prompts below). This:
- reuses the existing `describe_tool` dispatch/rendering — no new schema-render code;
- adds the describe to history → the tool becomes discovered **this turn** → it appears
  in `iteration_tools` next round-trip → the model re-calls it correctly;
- closes the blind-call hole (a hallucinated undiscovered call can never execute blind).

**Knowing it's "cataloged":** pass the catalog tool-names into `AgentRunParams` as an
optional `lazy_catalog_names: Option<HashSet<String>>` (llm.rs already has `catalog`).
The guard fires only when this is `Some` (i.e. lazy on). Alternative (no new param):
read the `describe_tool` definition's `name` enum from `iteration_tools` — rejected as
too implicit; prefer the explicit param.

Why **schema-only, not auto-retry**: the model's args were guessed blind (e.g.
`bindings={"df":"'Hoja 16'"}`); re-running them automatically still fails. Returning the
schema lets the model re-formulate. (Contrast with gsheets Option A, which executes +
attaches the preview because the *code* can still be valid — here the *args* are
unreliable without the schema.)

### Prompts — make the lazy workflow explicit ("diciente")
Three LLM-facing texts, so the model understands the steps and never reads the guard's
output as the real result:

1. **`describe_tool` description** (lazy_tools_catalog.rs:106). Add:
   > "ALWAYS call describe_tool for a tool BEFORE you invoke it. If you call a tool
   > without describing it first this turn, you will NOT get its result — you'll get its
   > schema back as a redirect, and must call the tool again with correct arguments.
   > Discovery resets each turn, so re-describe a tool the first time you use it in a
   > new turn."

2. **Lazy system block** (llm.rs ~2978, the `## Tools` section). When lazy is on, replace
   the generic line with a short workflow: "These tools load lazily. Step 1: call
   `describe_tool(name)` to load a tool's schema. Step 2: call the tool. Skipping step 1
   returns the schema (not a result) — read it and retry. Discovery is per-turn."

3. **Guard redirect result wording** (the wrapper around the schema). Must be
   unmistakably an instruction, not data:
   > "⚠️ NOT A RESULT. The tool `X` was not loaded this turn, so it was not executed.
   > Below is its schema. Call `X` again now with arguments that match it."
   > followed by the rendered schema.

## Detailed Steps
1. `lazy_tools_catalog.rs`: add `current_turn_slice`; keep `reconstruct_discovered_set`
   pure; update `build_describe_tool_definition` description (Prompt 1).
2. `llm.rs`: pass `current_turn_slice(messages)` into `reconstruct_discovered_set` in the
   tools_provider closure; populate `AgentRunParams.lazy_catalog_names` from `catalog`
   when lazy; update the `## Tools` system block (Prompt 2).
3. `agent_service.rs`: add the pre-dispatch guard at ~422 using `iteration_tools` +
   `lazy_catalog_names`; on miss, run `describe_tool(name)` and return the wrapped
   redirect (Prompt 3). Add the optional field to `AgentRunParams`.
4. `text/tools/*.yaml`: enrich the lazy `summary` of multi-capability write tools
   (start with `gsheets_run_python`) so the model knows which tool to describe.
5. Docs: update `29_lazy_tool_loading.md` (per-turn discovery + dispatch guard +
   prompt workflow); CHANGELOG entry.

## Testing Strategy
- **Unit**: `current_turn_slice` (no user msg → whole; multiple turns → tail);
  `reconstruct_discovered_set` over a per-turn slice ignores prior-turn describes;
  guard predicate (cataloged + not in iteration_tools → redirect).
- **Integration**: agent_service guard — a tool_call for an undiscovered cataloged tool
  returns the schema redirect, not execution; a discovered-this-turn tool executes.
- **Live E2E (lazy, real Google)**: re-run the formula prompt with `lazy_tool_loading:
  true`. Expect: model calls run_python → guard returns schema → model re-calls with
  `{{Column}}` correctly on the FIRST real attempt → sheet `=S5*U5`, clean narration.
  Then a 2-turn run: turn 2 must re-describe (verify a fresh describe/redirect fires).
- Full: `cargo test --verbose`, clippy, fmt.

## Documentation Updates
- `docs/developer_guide/29_lazy_tool_loading.md`
- `docs/CHANGELOG_2026-06.md`
- `text/tools/gsheets.yaml` (summary)

## Risks & Mitigations
| Risk | Impact | Mitigation |
|------|--------|------------|
| Per-turn re-describe re-pays the round-trip every turn | More tokens/latency per turn | Accepted (correctness > tokens, Eje 2); same trade as gsheets inspect-guard; only for tools actually used |
| Guard fires for a tool that SHOULD be eager | Spurious redirect | Guard only checks `lazy_catalog_names` (cataloged, non-eager). Eager + registry tools never in the set |
| Resume/suspend window misclassifies the turn | Lost discovery on resume | `current_turn_slice` from last `User` includes pre-suspend describes; explicit resume test |
| Model loops describe↔call without converging | Wasted turns | `max_tool_repeats` already bounds it; redirect wording says "call again now with correct args" |
| Provider emits describe + call same turn | Second call rejected | Existing doc-29 edge case; redirect wording reinforces "next turn / now" |

## Open Questions
- None blocking. (Eager-selective for synthetic toolkits — the Eje 3 knob — is deferred;
  this guard makes lazy sound without it.)

## Execution
Implement with `/rust_dev`. Branch from `develop`. Additive, lazy-only → no ADP code
change; ADP picks it up on the next worker colmena bump.
