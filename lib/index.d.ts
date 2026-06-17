import { LlmError, DagError } from "./errors";
export { LlmError, DagError };
export type NodeLlmConfigOptions = {
    apiKey?: string;
    model?: string;
    temperature?: number;
    maxTokens?: number;
    topP?: number;
    frequencyPenalty?: number;
    presencePenalty?: number;
};
export type NodeLlmMessage = {
    role: string;
    content: string;
};
/** Async iterator of text chunks. Use `for await (const chunk of stream)`. */
export declare class LlmStream implements AsyncIterableIterator<string> {
    private handle;
    constructor(handle: {
        pull(): Promise<string | null>;
    });
    [Symbol.asyncIterator](): AsyncIterableIterator<string>;
    next(): Promise<IteratorResult<string>>;
}
/** Multi-provider LLM client. Loads API keys from the environment on construction. */
export declare class ColmenaLlm {
    private inner;
    call(messages: NodeLlmMessage[], provider: string, options?: NodeLlmConfigOptions): Promise<string>;
    stream(messages: NodeLlmMessage[], provider: string, options?: NodeLlmConfigOptions): Promise<LlmStream>;
    healthCheck(provider: string): Promise<boolean>;
    getProviders(): string[];
}
export type GraphObject = Record<string, unknown>;
/** Run a DAG graph (file path or in-memory object); resolves to the final output. */
export declare function runDag(graph: string | GraphObject, resumeId?: string | null, resumeAnswer?: string | null, injectPayload?: unknown, includeExtraInfo?: boolean | null, agentSessionId?: string | null): Promise<unknown>;
/** Serve a graph's webhook triggers as a (blocking) HTTP API. */
export declare function serveDag(filePath: string, host?: string | null, port?: number | null): Promise<void>;
/** Validate a graph object; throws DagError if it is not a valid graph. */
export declare function validateGraph(graph: GraphObject): void;
