import { test } from "node:test";
import assert from "node:assert/strict";
import { ColmenaLlm, runDag, LlmError, DagError } from "../index";

test("call to an unknown provider throws LlmError", async () => {
  const llm = new ColmenaLlm();
  await assert.rejects(
    () => llm.call([{ role: "user", content: "hi" }], "does-not-exist"),
    (err: unknown) => err instanceof LlmError && /(not found|not supported)/i.test((err as Error).message),
  );
});

test("runDag on a missing file throws DagError", async () => {
  await assert.rejects(
    () => runDag("/nonexistent/graph.json"),
    (err: unknown) => err instanceof DagError,
  );
});
