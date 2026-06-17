// The napi loader at the repo root. Built by `napi build`.
// eslint-disable-next-line @typescript-eslint/no-var-requires
const native = require("../index.js");

import { LlmError, DagError } from "./errors";
export { LlmError, DagError };

function asLlm<T>(p: Promise<T>): Promise<T> {
  return p.catch((e: unknown) => {
    throw new LlmError(e instanceof Error ? e.message : String(e));
  });
}

function asDag<T>(p: Promise<T>): Promise<T> {
  return p.catch((e: unknown) => {
    throw new DagError(e instanceof Error ? e.message : String(e));
  });
}

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

/** Async iterator of text chunks. Use `for await (const chunk of stream)`. */
export class LlmStream implements AsyncIterableIterator<string> {
  constructor(private handle: { pull(): Promise<string | null> }) {}
  [Symbol.asyncIterator](): AsyncIterableIterator<string> {
    return this;
  }
  async next(): Promise<IteratorResult<string>> {
    const value = await this.handle.pull();
    return value === null
      ? { value: undefined, done: true }
      : { value, done: false };
  }
}

/** Multi-provider LLM client. Loads API keys from the environment on construction. */
export class ColmenaLlm {
  private inner = new native.ColmenaLlm();

  call(
    messages: NodeLlmMessage[],
    provider: string,
    options?: NodeLlmConfigOptions,
  ): Promise<string> {
    return asLlm(this.inner.call(messages, provider, options));
  }

  async stream(
    messages: NodeLlmMessage[],
    provider: string,
    options?: NodeLlmConfigOptions,
  ): Promise<LlmStream> {
    const handle = await asLlm(
      this.inner.stream(messages, provider, options) as Promise<{ pull(): Promise<string | null> }>,
    );
    return new LlmStream(handle);
  }

  healthCheck(provider: string): Promise<boolean> {
    return asLlm(this.inner.healthCheck(provider));
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
  return asDag(native.runDag(filePath, resumeId, resumeAnswer, injectPayload, includeExtraInfo));
}

/** Serve a graph's webhook triggers as a (blocking) HTTP API. */
export function serveDag(
  filePath: string,
  host?: string | null,
  port?: number | null,
): Promise<void> {
  return asDag(native.serveDag(filePath, host, port));
}
