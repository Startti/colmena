"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const node_test_1 = require("node:test");
const strict_1 = __importDefault(require("node:assert/strict"));
const index_1 = require("../src/index");
(0, node_test_1.test)("cell map converts to a DataFrame", () => {
    const cells = { A1: "name", B1: "age", A2: "Alice", B2: 30, A3: "Bob", B3: 25 };
    const df = (0, index_1.readSheetAsDataFrame)(cells);
    strict_1.default.deepEqual(df.columns, ["name", "age"]);
    strict_1.default.equal(df.shape.height, 2);
    strict_1.default.deepEqual(df.getColumn("name").toArray(), ["Alice", "Bob"]);
});
