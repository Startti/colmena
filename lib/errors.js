"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.DagError = exports.LlmError = void 0;
/** Raised by ColmenaLlm operations (call / stream / healthCheck). */
class LlmError extends Error {
    constructor(message) {
        super(message);
        this.name = "LlmError";
    }
}
exports.LlmError = LlmError;
/** Raised by DAG operations (runDag / streamDag / validateGraph / serveDag). */
class DagError extends Error {
    constructor(message) {
        super(message);
        this.name = "DagError";
    }
}
exports.DagError = DagError;
