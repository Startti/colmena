import { test } from "node:test";
import assert from "node:assert/strict";
import { ColmenaLlm, runDag, LlmError, DagError } from "../index";

test("facade exports are wired", () => {
  assert.equal(typeof runDag, "function");
  assert.equal(typeof ColmenaLlm, "function");
  assert.ok(new LlmError("x") instanceof Error);
  assert.ok(new DagError("x") instanceof Error);
});

test("getProviders returns the configured providers", () => {
  const llm = new ColmenaLlm();
  const providers = llm.getProviders();
  assert.ok(Array.isArray(providers));
});
