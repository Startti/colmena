/** Raised by ColmenaLlm operations (call / stream / healthCheck). */
export declare class LlmError extends Error {
    constructor(message: string);
}
/** Raised by DAG operations (runDag / streamDag / validateGraph / serveDag). */
export declare class DagError extends Error {
    constructor(message: string);
}
