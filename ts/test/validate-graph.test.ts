import { test } from "node:test";
import assert from "node:assert/strict";
import { validateGraph, DagError } from "../index";

test("validateGraph accepts a valid graph", () => {
  assert.doesNotThrow(() =>
    validateGraph({
      nodes: { a: { type: "mock_input", config: { input: 1 } } },
      edges: [],
    }),
  );
});

test("validateGraph rejects an invalid graph", () => {
  assert.throws(() => validateGraph({ not: "a graph" }), DagError);
});
