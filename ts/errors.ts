/** Raised by ColmenaLlm operations (call / stream / healthCheck). */
export class LlmError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "LlmError";
  }
}

/** Raised by DAG operations (runDag / streamDag / validateGraph / serveDag). */
export class DagError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DagError";
  }
}
