"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ColmenaLlm = exports.LlmStream = exports.DagError = exports.LlmError = void 0;
exports.runDag = runDag;
exports.serveDag = serveDag;
exports.validateGraph = validateGraph;
// The napi loader at the repo root. Built by `napi build`.
// eslint-disable-next-line @typescript-eslint/no-var-requires
const native = require("../index.js");
const errors_1 = require("./errors");
Object.defineProperty(exports, "LlmError", { enumerable: true, get: function () { return errors_1.LlmError; } });
Object.defineProperty(exports, "DagError", { enumerable: true, get: function () { return errors_1.DagError; } });
function asLlm(p) {
    return p.catch((e) => {
        throw new errors_1.LlmError(e instanceof Error ? e.message : String(e));
    });
}
function asDag(p) {
    return p.catch((e) => {
        throw new errors_1.DagError(e instanceof Error ? e.message : String(e));
    });
}
/** Async iterator of text chunks. Use `for await (const chunk of stream)`. */
class LlmStream {
    handle;
    constructor(handle) {
        this.handle = handle;
    }
    [Symbol.asyncIterator]() {
        return this;
    }
    async next() {
        const value = await this.handle.pull();
        return value === null
            ? { value: undefined, done: true }
            : { value, done: false };
    }
}
exports.LlmStream = LlmStream;
/** Multi-provider LLM client. Loads API keys from the environment on construction. */
class ColmenaLlm {
    inner = new native.ColmenaLlm();
    call(messages, provider, options) {
        return asLlm(this.inner.call(messages, provider, options));
    }
    async stream(messages, provider, options) {
        const handle = await asLlm(this.inner.stream(messages, provider, options));
        return new LlmStream(handle);
    }
    healthCheck(provider) {
        return asLlm(this.inner.healthCheck(provider));
    }
    getProviders() {
        return this.inner.getProviders();
    }
}
exports.ColmenaLlm = ColmenaLlm;
/** Run a DAG graph (file path or in-memory object); resolves to the final output. */
function runDag(graph, resumeId, resumeAnswer, injectPayload, includeExtraInfo, agentSessionId) {
    if (typeof graph === "string") {
        return asDag(native.runDag(graph, resumeId, resumeAnswer, injectPayload, includeExtraInfo, agentSessionId));
    }
    return asDag(native.runDagFromJson(JSON.stringify(graph), resumeId, resumeAnswer, injectPayload, includeExtraInfo, agentSessionId));
}
/** Serve a graph's webhook triggers as a (blocking) HTTP API. */
function serveDag(filePath, host, port) {
    return asDag(native.serveDag(filePath, host, port));
}
/** Validate a graph object; throws DagError if it is not a valid graph. */
function validateGraph(graph) {
    try {
        native.validateGraph(graph);
    }
    catch (e) {
        throw new errors_1.DagError(e instanceof Error ? e.message : String(e));
    }
}
