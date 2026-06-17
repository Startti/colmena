"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ColmenaLlm = exports.DagError = exports.LlmError = void 0;
exports.runDag = runDag;
exports.serveDag = serveDag;
// The napi loader at the repo root. Built by `napi build`.
// eslint-disable-next-line @typescript-eslint/no-var-requires
const native = require("../index.js");
var errors_1 = require("./errors");
Object.defineProperty(exports, "LlmError", { enumerable: true, get: function () { return errors_1.LlmError; } });
Object.defineProperty(exports, "DagError", { enumerable: true, get: function () { return errors_1.DagError; } });
/** Multi-provider LLM client. Loads API keys from the environment on construction. */
class ColmenaLlm {
    inner = new native.ColmenaLlm();
    call(messages, provider, options) {
        return this.inner.call(messages, provider, options);
    }
    healthCheck(provider) {
        return this.inner.healthCheck(provider);
    }
    getProviders() {
        return this.inner.getProviders();
    }
}
exports.ColmenaLlm = ColmenaLlm;
/** Run a DAG graph to completion; resolves to the final output value. */
function runDag(filePath, resumeId, resumeAnswer, injectPayload, includeExtraInfo) {
    return native.runDag(filePath, resumeId, resumeAnswer, injectPayload, includeExtraInfo);
}
/** Serve a graph's webhook triggers as a (blocking) HTTP API. */
function serveDag(filePath, host, port) {
    return native.serveDag(filePath, host, port);
}
