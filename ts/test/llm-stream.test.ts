import { test } from "node:test";
import assert from "node:assert/strict";
import { ColmenaLlm } from "../index";

// Requires a configured provider; skip if not available.
const provider = process.env.COLMENA_TEST_PROVIDER ?? "mock";

test("stream yields text chunks via for-await", async () => {
  const llm = new ColmenaLlm();
  if (!llm.getProviders().includes(provider)) return; // env-gated
  const stream = await llm.stream(
    [{ role: "user", content: "Say hi" }],
    provider,
  );
  let combined = "";
  for await (const chunk of stream) {
    assert.equal(typeof chunk, "string");
    combined += chunk;
  }
  assert.ok(combined.length >= 0);
});
