import { test } from "node:test";
import assert from "node:assert/strict";
import { documents } from "../index";

// ArtifactId format: "art_" + 26-char ULID (Crockford base32)
const ARTIFACT = "art_00000000000000000000000000";

test("add, write, read, list roundtrip", async () => {
  const sheetId = await documents.addSheet(ARTIFACT, "Data");
  assert.equal(typeof sheetId, "string");

  await documents.writeSheet(
    ARTIFACT,
    sheetId,
    ["name", "age"],
    [
      ["Alice", 30],
      ["Bob", 25],
    ],
    "replace",
  );

  const cells = await documents.readSheet(ARTIFACT, sheetId);
  assert.equal(cells["A1"], "name");
  assert.equal(cells["B1"], "age");
  assert.equal(cells["A2"], "Alice");

  const sheets = await documents.listSheets(ARTIFACT);
  assert.ok(sheets.some((s) => s.sheetId === sheetId));
});
