import { test } from "node:test";
import assert from "node:assert/strict";
import { readSheetAsDataFrame } from "../src/index";

test("cell map converts to a DataFrame", () => {
  const cells = { A1: "name", B1: "age", A2: "Alice", B2: 30, A3: "Bob", B3: 25 };
  const df = readSheetAsDataFrame(cells);
  assert.deepEqual(df.columns, ["name", "age"]);
  assert.equal(df.shape.height, 2);
  assert.deepEqual(df.getColumn("name").toArray(), ["Alice", "Bob"]);
});
