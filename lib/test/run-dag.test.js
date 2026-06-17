"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const node_test_1 = require("node:test");
const strict_1 = __importDefault(require("node:assert/strict"));
const index_1 = require("../index");
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
(0, node_test_1.test)("runDag accepts an in-memory graph object", async () => {
    const result = await (0, index_1.runDag)(GRAPH);
    strict_1.default.ok(result, "expected a result value");
});
(0, node_test_1.test)("runDag still accepts a file path", async () => {
    const result = await (0, index_1.runDag)("tests/graphs/basic/power.json");
    strict_1.default.ok(result);
});
