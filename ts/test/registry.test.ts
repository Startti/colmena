import { test } from "node:test";
import assert from "node:assert/strict";
import { defaultRegistry } from "../index";

test("defaultRegistry lists node types", () => {
  const registry = defaultRegistry();
  const types = registry.nodeTypes();
  assert.ok(Array.isArray(types));
  assert.ok(types.includes("log"), "expected the 'log' node type");
});

test("toolkitCatalog returns sub-tools for a toolkit node", () => {
  const registry = defaultRegistry();
  const types = registry.nodeTypes();
  if (!types.includes("api_explorer")) return; // env-gated
  const catalog = registry.toolkitCatalog("api_explorer", {});
  assert.ok(Array.isArray(catalog));
});
