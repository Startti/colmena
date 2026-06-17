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
(0, node_test_1.test)("streamDag yields typed events ending in finish", async () => {
    const stream = await (0, index_1.streamDag)(GRAPH);
    const types = [];
    for await (const event of stream) {
        const ev = event;
        strict_1.default.equal(typeof ev.type, "string");
        types.push(ev.type);
    }
    strict_1.default.ok(types.includes("finish"), `expected a finish event, got ${types.join(",")}`);
});
