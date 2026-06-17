"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const node_test_1 = require("node:test");
const strict_1 = __importDefault(require("node:assert/strict"));
const index_1 = require("../index");
(0, node_test_1.test)("facade exports are wired", () => {
    strict_1.default.equal(typeof index_1.runDag, "function");
    strict_1.default.equal(typeof index_1.ColmenaLlm, "function");
    strict_1.default.ok(new index_1.LlmError("x") instanceof Error);
    strict_1.default.ok(new index_1.DagError("x") instanceof Error);
});
(0, node_test_1.test)("getProviders returns the configured providers", () => {
    const llm = new index_1.ColmenaLlm();
    const providers = llm.getProviders();
    strict_1.default.ok(Array.isArray(providers));
});
