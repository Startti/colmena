"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const node_test_1 = require("node:test");
const strict_1 = __importDefault(require("node:assert/strict"));
const index_1 = require("../index");
(0, node_test_1.test)("validateGraph accepts a valid graph", () => {
    strict_1.default.doesNotThrow(() => (0, index_1.validateGraph)({
        nodes: { a: { type: "mock_input", config: { input: 1 } } },
        edges: [],
    }));
});
(0, node_test_1.test)("validateGraph rejects an invalid graph", () => {
    strict_1.default.throws(() => (0, index_1.validateGraph)({ not: "a graph" }), index_1.DagError);
});
