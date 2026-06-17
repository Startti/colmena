export { LlmError, DagError } from "./errors";
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
/** Multi-provider LLM client. Loads API keys from the environment on construction. */
export declare class ColmenaLlm {
    private inner;
    call(messages: NodeLlmMessage[], provider: string, options?: NodeLlmConfigOptions): Promise<string>;
    healthCheck(provider: string): Promise<boolean>;
    getProviders(): string[];
}
/** Run a DAG graph to completion; resolves to the final output value. */
export declare function runDag(filePath: string, resumeId?: string | null, resumeAnswer?: string | null, injectPayload?: unknown, includeExtraInfo?: boolean | null): Promise<unknown>;
/** Serve a graph's webhook triggers as a (blocking) HTTP API. */
export declare function serveDag(filePath: string, host?: string | null, port?: number | null): Promise<void>;
