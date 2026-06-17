"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const node_test_1 = require("node:test");
const strict_1 = __importDefault(require("node:assert/strict"));
const index_1 = require("../index");
(0, node_test_1.test)("call to an unknown provider throws LlmError", async () => {
    const llm = new index_1.ColmenaLlm();
    await strict_1.default.rejects(() => llm.call([{ role: "user", content: "hi" }], "does-not-exist"), (err) => err instanceof index_1.LlmError && /(not found|not supported)/i.test(err.message));
});
(0, node_test_1.test)("runDag on a missing file throws DagError", async () => {
    await strict_1.default.rejects(() => (0, index_1.runDag)("/nonexistent/graph.json"), (err) => err instanceof index_1.DagError);
});
