import { test } from "node:test";
import assert from "node:assert/strict";
import { streamDag, type DagEvent } from "../index";

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

test("streamDag yields typed events ending in finish", async () => {
  const stream = await streamDag(GRAPH);
  const types: string[] = [];
  for await (const event of stream) {
    const ev = event as DagEvent;
    assert.equal(typeof ev.type, "string");
    types.push(ev.type);
  }
  assert.ok(types.includes("finish"), `expected a finish event, got ${types.join(",")}`);
});
