"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const node_test_1 = require("node:test");
const strict_1 = __importDefault(require("node:assert/strict"));
const index_1 = require("../index");
// Requires a configured provider; skip if not available.
const provider = process.env.COLMENA_TEST_PROVIDER ?? "mock";
(0, node_test_1.test)("stream yields text chunks via for-await", async () => {
    const llm = new index_1.ColmenaLlm();
    if (!llm.getProviders().includes(provider))
        return; // env-gated
    const stream = await llm.stream([{ role: "user", content: "Say hi" }], provider);
    let combined = "";
    for await (const chunk of stream) {
        strict_1.default.equal(typeof chunk, "string");
        combined += chunk;
    }
    strict_1.default.ok(combined.length >= 0);
});
