import { test } from "node:test";
import assert from "node:assert/strict";
import { runDag } from "../index";

const GRAPH = {
  nodes: {
    start: { type: "mock_input", config: { input: 5 } },
    pow_step: { type: "exponential", config: { exponent: 3 } },
    log_result: { type: "log" },
  },
  edges: [
    { from: "start", to: "pow_step" },
    { from: "pow_step", to: "log_result" },
  ],
};

test("runDag accepts an in-memory graph object", async () => {
  const result = await runDag(GRAPH);
  assert.ok(result, "expected a result value");
});

test("runDag still accepts a file path", async () => {
  const result = await runDag("tests/graphs/basic/power.json");
  assert.ok(result);
});
