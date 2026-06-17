// The napi loader at the repo root. Built by `napi build`.
// eslint-disable-next-line @typescript-eslint/no-var-requires
const native = require("../index.js");

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

export type NodeLlmMessage = { role: string; content: string };

/** Multi-provider LLM client. Loads API keys from the environment on construction. */
export class ColmenaLlm {
  private inner = new native.ColmenaLlm();

  call(
    messages: NodeLlmMessage[],
    provider: string,
    options?: NodeLlmConfigOptions,
  ): Promise<string> {
    return this.inner.call(messages, provider, options);
  }

  healthCheck(provider: string): Promise<boolean> {
    return this.inner.healthCheck(provider);
  }

  getProviders(): string[] {
    return this.inner.getProviders();
  }
}

/** Run a DAG graph to completion; resolves to the final output value. */
export function runDag(
  filePath: string,
  resumeId?: string | null,
  resumeAnswer?: string | null,
  injectPayload?: unknown,
  includeExtraInfo?: boolean | null,
): Promise<unknown> {
  return native.runDag(filePath, resumeId, resumeAnswer, injectPayload, includeExtraInfo);
}

/** Serve a graph's webhook triggers as a (blocking) HTTP API. */
export function serveDag(
  filePath: string,
  host?: string | null,
  port?: number | null,
): Promise<void> {
  return native.serveDag(filePath, host, port);
}
