"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const node_test_1 = require("node:test");
const strict_1 = __importDefault(require("node:assert/strict"));
const index_1 = require("../index");
// ArtifactId format: "art_" + 26-char ULID (Crockford base32)
const ARTIFACT = "art_00000000000000000000000000";
(0, node_test_1.test)("add, write, read, list roundtrip", async () => {
    const sheetId = await index_1.documents.addSheet(ARTIFACT, "Data");
    strict_1.default.equal(typeof sheetId, "string");
    await index_1.documents.writeSheet(ARTIFACT, sheetId, ["name", "age"], [
        ["Alice", 30],
        ["Bob", 25],
    ], "replace");
    const cells = await index_1.documents.readSheet(ARTIFACT, sheetId);
    strict_1.default.equal(cells["A1"], "name");
    strict_1.default.equal(cells["B1"], "age");
    strict_1.default.equal(cells["A2"], "Alice");
    const sheets = await index_1.documents.listSheets(ARTIFACT);
    strict_1.default.ok(sheets.some((s) => s.sheetId === sheetId));
});
