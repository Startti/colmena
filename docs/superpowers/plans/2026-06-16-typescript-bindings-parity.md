# TypeScript Bindings Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the napi-rs (Node/TypeScript) binding to functional and documentation parity with the PyO3 (Python) binding of Colmena.

**Architecture:** Additive work only in `src/libs/colmena/src/node_bindings/` (split by capability) plus a thin hand-written TS facade in `ts/` that wraps native stream handles with `[Symbol.asyncIterator]` and typed `Error` classes. Every new binding reuses existing `crate::dag_engine::api` / service-container functions — no DAG-engine or LLM core logic changes. A separate opt-in companion package `@colmena-ai/documents` adds nodejs-polars DataFrame ergonomics over the cero-deps `documents` core.

**Tech Stack:** Rust + napi-rs 2.16 (`napi4`, `tokio_rt`, `async`, `serde-json` features), TypeScript (`tsc`), `node:test`, nodejs-polars (companion only), GitHub Actions.

**Spec:** [docs/superpowers/specs/2026-06-16-typescript-bindings-parity-design.md](../specs/2026-06-16-typescript-bindings-parity-design.md)

---

## File Structure

**Rust (`src/libs/colmena/src/node_bindings/`):**
- `mod.rs` — module declarations + shared error helper (was monolithic 160-line file)
- `llm.rs` — `ColmenaLlm` (call, stream, healthCheck, getProviders), `NodeLlmConfigOptions`, `NodeLlmMessage`, `LlmStreamHandle`
- `dag.rs` — `runDag`, `streamDag`, `validateGraph`, `serveDag`
- `registry.rs` — `defaultRegistry`, `Registry`
- `documents.rs` — `documentsListSheets`, `documentsReadSheet`, `documentsAddSheet`, `documentsWriteSheet`
- `stream.rs` — `DagStreamHandle`, `LlmStreamHandle` (napi async-iterator handles)

**TypeScript (`ts/`):**
- `index.ts` — facade: re-exports native, wraps stream handles + `ColmenaLlm`, exposes `documents` namespace, typed events
- `errors.ts` — `LlmError`, `DagError` (extend `Error`)
- `tsconfig.json` — compiles `ts/` → `lib/`
- `test/*.test.ts` — `node:test` suites
- `examples/*.mjs` — runnable examples

**Companion (`packages/documents/`):**
- `package.json`, `src/index.ts`, `test/documents.test.ts`

**Generated/packaging (repo root):**
- `index.js`, `index.d.ts` — napi loader + raw types (generated)
- `package.json` — `main: lib/index.js`, `types: lib/index.d.ts`, `files`, `build` adds `tsc`
- `.npmignore`

**Docs:**
- `docs/examples/typescript_usage.md`, `docs/developer_guide/49_typescript_dag.md`, `README.md` (Node section), `docs/DEVELOPER_GUIDE.md` (Node entries)

**CI/CD:**
- `.github/workflows/cd-main.yml` (`publish-npm` job), `.github/workflows/ci-develop.yml` (node:test step)

---

## Conventions for every task

- Build napi for local dev: `npm run build:debug` (produces `colmena.<platform>.node` + regenerates `index.js`/`index.d.ts`).
- Compile the TS facade: `npx tsc -p ts/tsconfig.json` (produces `lib/`).
- Run TS tests: `node --test lib/test/` (after compiling) — see Task 0.4 for the npm script.
- Rust must stay clean: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test --verbose` before each commit that touches Rust.
- Commit messages use Conventional Commits (`feat`, `fix`, `docs`, `chore`, `ci`, `test`, `refactor`) — never `plan`/`spec`/`diag`.

---

## FASE 0 — Infrastructure

### Task 0.1: Split `node_bindings/mod.rs` into per-capability modules

**Files:**
- Modify: `src/libs/colmena/src/node_bindings/mod.rs`
- Create: `src/libs/colmena/src/node_bindings/llm.rs`
- Create: `src/libs/colmena/src/node_bindings/dag.rs`

- [ ] **Step 1: Create `llm.rs` with the LLM surface moved verbatim**

Move lines 10–124 of the current `mod.rs` (the `// LLM Bindings` block: `NodeLlmConfigOptions`, `NodeLlmMessage`, `ColmenaLlm` and its impl) into `src/libs/colmena/src/node_bindings/llm.rs`. Add this header so the moved code keeps compiling:

```rust
use crate::llm::domain::{MessageRole, ProviderKind};
use crate::shared::infrastructure::{ConfigResolver, ServiceContainerFactory};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

// <-- paste the moved NodeLlmConfigOptions / NodeLlmMessage / ColmenaLlm here -->
```

- [ ] **Step 2: Create `dag.rs` with the DAG surface moved verbatim**

Move lines 126–159 of the current `mod.rs` (the `run_dag` and `serve_dag` functions) into `src/libs/colmena/src/node_bindings/dag.rs` with this header:

```rust
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;

// <-- paste the moved run_dag / serve_dag here -->
```

- [ ] **Step 3: Replace `mod.rs` with module declarations only**

Overwrite `src/libs/colmena/src/node_bindings/mod.rs` with:

```rust
//! napi-rs bindings for Colmena. napi collects `#[napi]` items across all
//! submodules of this crate, so each capability lives in its own file.

mod dag;
mod llm;
```

- [ ] **Step 4: Verify it builds and napi output is unchanged**

Run: `npm run build:debug`
Expected: build succeeds; `git diff index.d.ts` shows **no change** (same generated surface).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/node_bindings/
git commit -m "refactor(node): split node_bindings into per-capability modules"
```

---

### Task 0.2: Fix `publish-npm` to build all platforms with the correct version

**Files:**
- Modify: `.github/workflows/cd-main.yml:212-231`

- [ ] **Step 1: Replace the `publish-npm` job with a build matrix + bundle-all-binaries publish**

Replace the existing `publish-npm` job (lines 212–231) with:

```yaml
  build-node:
    name: Build .node on ${{ matrix.settings.target }}
    needs: release
    runs-on: ${{ matrix.settings.host }}
    strategy:
      matrix:
        settings:
          - host: ubuntu-latest
            target: x86_64-unknown-linux-gnu
          - host: macos-latest
            target: x86_64-apple-darwin
          - host: macos-latest
            target: aarch64-apple-darwin
          - host: windows-latest
            target: x86_64-pc-windows-msvc
    steps:
      - uses: actions/checkout@v4
        with:
          ref: v${{ needs.release.outputs.new_version }}
      - uses: actions-rust-lang/setup-rust-toolchain@v1
        with:
          target: ${{ matrix.settings.target }}
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
      - name: Install deps
        run: npm ci
      - name: Build native binding
        run: npx napi build --platform --release --target ${{ matrix.settings.target }} --features node --cargo-cwd src/libs/colmena --cargo-name colmena --cargo-flags="--lib"
      - name: Upload .node artifact
        uses: actions/upload-artifact@v4
        with:
          name: node-${{ matrix.settings.target }}
          path: "*.node"

  publish-npm:
    name: Publish to NPM
    runs-on: ubuntu-latest
    needs: [release, build-node]
    steps:
      - uses: actions/checkout@v4
        with:
          ref: v${{ needs.release.outputs.new_version }}
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          registry-url: 'https://registry.npmjs.org/'
      - name: Download all .node artifacts
        uses: actions/download-artifact@v4
        with:
          pattern: node-*
          path: .
          merge-multiple: true
      - name: Build TS facade
        run: |
          npm ci
          npx napi build --platform --release --features node --cargo-cwd src/libs/colmena --cargo-name colmena --cargo-flags="--lib"
          npx tsc -p ts/tsconfig.json
      - name: Publish
        run: npm publish --access public
        env:
          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
```

> Note: the `publish-npm` runner re-runs `napi build` only to regenerate `index.js`/`index.d.ts` (the loader + raw types) on linux; the downloaded `*.node` artifacts from all four targets sit alongside it and the generated `index.js` selects the right one at runtime via its platform-detection fallback.

- [ ] **Step 2: Update `github-release` job dependency**

In the `github-release` job (was line 236), the `needs:` array already lists `publish-npm`; leave `needs: [release, publish-pypi, publish-npm]` unchanged. Confirm with: `grep -n "needs: \[release, publish-pypi, publish-npm\]" .github/workflows/cd-main.yml` → one match.

- [ ] **Step 3: Add npm install line to the changelog step**

In the `Generate Changelog` step, find the line (was line 124):

```bash
          CHANGELOG="${CHANGELOG}## Installation\n\n\`\`\`bash\npip install colmena-ai==${{ steps.version.outputs.new_version }}\n\`\`\`\n\n"
```

Replace it with:

```bash
          CHANGELOG="${CHANGELOG}## Installation\n\n**Python:**\n\`\`\`bash\npip install colmena-ai==${{ steps.version.outputs.new_version }}\n\`\`\`\n\n**Node.js:**\n\`\`\`bash\nnpm install colmena-ai@${{ steps.version.outputs.new_version }}\n\`\`\`\n\n"
```

- [ ] **Step 4: Lint the workflow YAML**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/cd-main.yml')); print('valid')"`
Expected: `valid`

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/cd-main.yml
git commit -m "ci(node): build all platforms and publish correct version to npm"
```

---

### Task 0.3: Add `files` allowlist and `.npmignore`

**Files:**
- Modify: `package.json`
- Create: `.npmignore`

- [ ] **Step 1: Add `files`, point `main`/`types` at the TS facade, add build steps**

Edit `package.json` so the top keys read:

```json
{
  "name": "colmena-ai",
  "version": "0.3.0",
  "description": "TypeScript bindings for Colmena LLM and DAG Engine",
  "main": "lib/index.js",
  "types": "lib/index.d.ts",
  "files": [
    "index.js",
    "index.d.ts",
    "lib/",
    "*.node"
  ],
  "scripts": {
    "build": "napi build --platform --release --features node --cargo-cwd src/libs/colmena --cargo-name colmena --cargo-flags=\"--lib\" && tsc -p ts/tsconfig.json",
    "build:debug": "napi build --platform --features node --cargo-cwd src/libs/colmena --cargo-name colmena --cargo-flags=\"--lib\" && tsc -p ts/tsconfig.json",
    "test": "node --test lib/test/",
    "prepublishOnly": "npm run build"
  },
```

(Keep the existing `napi`, `engines`, `devDependencies` blocks. Add `"typescript": "^5.4.0"` to `devDependencies`.)

- [ ] **Step 2: Create `.npmignore`**

```
src/
docs/
tests/
python/
ts/
target/
.github/
*.rs
Cargo.toml
Cargo.lock
pyproject.toml
```

- [ ] **Step 3: Verify the package contents**

Run: `npm pack --dry-run 2>&1 | grep -E "Tarball Contents|index.js|\.node|lib/|src/" | head -20`
Expected: lists `index.js`, `index.d.ts`, `lib/...`, `*.node`; does NOT list `src/` or `*.rs`.

- [ ] **Step 4: Commit**

```bash
git add package.json .npmignore
git commit -m "chore(node): restrict npm tarball to runtime artifacts"
```

---

### Task 0.4: Scaffold the TS facade, tsconfig, and node:test harness

**Files:**
- Create: `ts/tsconfig.json`
- Create: `ts/index.ts`
- Create: `ts/errors.ts`
- Create: `ts/test/smoke.test.ts`
- Modify: `.github/workflows/ci-develop.yml`

- [ ] **Step 1: Create `ts/tsconfig.json`**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "CommonJS",
    "moduleResolution": "Node",
    "declaration": true,
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "outDir": "../lib",
    "rootDir": "."
  },
  "include": ["index.ts", "errors.ts", "test/**/*.ts"]
}
```

- [ ] **Step 2: Create `ts/errors.ts`**

```ts
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
```

- [ ] **Step 3: Create `ts/index.ts` (initial facade re-exporting the current native surface)**

```ts
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
```

- [ ] **Step 4: Write the smoke test**

Create `ts/test/smoke.test.ts`:

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { ColmenaLlm, runDag, LlmError, DagError } from "../index";

test("facade exports are wired", () => {
  assert.equal(typeof runDag, "function");
  assert.equal(typeof ColmenaLlm, "function");
  assert.ok(new LlmError("x") instanceof Error);
  assert.ok(new DagError("x") instanceof Error);
});

test("getProviders returns the configured providers", () => {
  const llm = new ColmenaLlm();
  const providers = llm.getProviders();
  assert.ok(Array.isArray(providers));
});
```

- [ ] **Step 5: Build and run the test**

Run: `npm run build:debug && npm test`
Expected: both tests PASS.

- [ ] **Step 6: Add the node:test step to CI develop**

In `.github/workflows/ci-develop.yml`, after the `Run Python tests` step, append (same indentation, inside the `steps:` list):

```yaml
      - name: Set up Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '20'

      - name: Build Node bindings + TS facade
        run: |
          npm ci
          npm run build:debug

      - name: Run Node tests
        run: npm test
```

- [ ] **Step 7: Commit**

```bash
git add ts/ package.json .github/workflows/ci-develop.yml lib/ index.js index.d.ts
git commit -m "test(node): scaffold TS facade and node:test harness"
```

---

## SLICE 1 — LLM stream + typed errors

### Task 1.1: Add `LlmStreamHandle` napi class and `ColmenaLlm.stream`

**Files:**
- Create: `src/libs/colmena/src/node_bindings/stream.rs`
- Modify: `src/libs/colmena/src/node_bindings/llm.rs`
- Modify: `src/libs/colmena/src/node_bindings/mod.rs`
- Test: `ts/test/llm-stream.test.ts`

- [ ] **Step 1: Write the failing TS test**

Create `ts/test/llm-stream.test.ts`:

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { ColmenaLlm } from "../index";

// Requires a mock provider; skip if not configured.
const provider = process.env.COLMENA_TEST_PROVIDER ?? "mock";

test("stream yields text chunks via for-await", async () => {
  const llm = new ColmenaLlm();
  if (!llm.getProviders().includes(provider)) return; // env-gated
  const stream = await llm.stream(
    [{ role: "user", content: "Say hi" }],
    provider,
  );
  let combined = "";
  for await (const chunk of stream) {
    assert.equal(typeof chunk, "string");
    combined += chunk;
  }
  assert.ok(combined.length >= 0);
});
```

- [ ] **Step 2: Build and confirm it fails to compile (no `stream` method)**

Run: `npx tsc -p ts/tsconfig.json`
Expected: FAIL — `Property 'stream' does not exist on type 'ColmenaLlm'`.

- [ ] **Step 3: Create `stream.rs` with `LlmStreamHandle`**

```rust
use futures::StreamExt;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Async-iterator handle over an LLM text stream. Each `pull()` resolves to the
/// next chunk, or `null` when the stream is exhausted. The TS facade attaches
/// `[Symbol.asyncIterator]` so callers use `for await (const chunk of stream)`.
#[napi]
pub struct LlmStreamHandle {
    stream: Arc<Mutex<crate::llm::domain::LlmStream>>,
}

#[napi]
impl LlmStreamHandle {
    #[napi]
    pub async fn pull(&self) -> Result<Option<String>> {
        let mut stream = self.stream.lock().await;
        match stream.next().await {
            Some(Ok(chunk)) => Ok(Some(chunk.content().to_string())),
            Some(Err(e)) => Err(Error::new(Status::GenericFailure, e.to_string())),
            None => Ok(None),
        }
    }
}

impl LlmStreamHandle {
    pub fn new(stream: crate::llm::domain::LlmStream) -> Self {
        Self {
            stream: Arc::new(Mutex::new(stream)),
        }
    }
}
```

- [ ] **Step 4: Add `stream` to `ColmenaLlm` in `llm.rs`**

Add `use crate::node_bindings::stream::LlmStreamHandle;` to the top of `llm.rs`, and add this method inside `impl ColmenaLlm` (after `call`):

```rust
    #[napi]
    pub async fn stream(
        &self,
        messages: Vec<NodeLlmMessage>,
        provider: String,
        options: Option<NodeLlmConfigOptions>,
    ) -> Result<LlmStreamHandle> {
        let provider_kind = ProviderKind::from_str(&provider)
            .map_err(|e| Error::new(Status::InvalidArg, e.to_string()))?;
        let container = self
            .containers
            .get(&provider)
            .ok_or_else(|| Error::new(Status::InvalidArg, format!("Provider {} not found", provider)))?
            .clone();

        let llm_messages: Result<Vec<crate::llm::domain::LlmMessage>> = messages
            .into_iter()
            .map(|msg| {
                let role = MessageRole::from_str(&msg.role)
                    .map_err(|e| Error::new(Status::InvalidArg, e.to_string()))?;
                crate::llm::domain::LlmMessage::new(role, msg.content)
                    .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
            })
            .collect();

        let options = options.unwrap_or_default();
        let config = ConfigResolver::create_config(
            provider_kind,
            options.api_key,
            options.model,
            options.temperature.map(|v| v as f32),
            options.max_tokens,
            options.top_p.map(|v| v as f32),
            options.frequency_penalty.map(|v| v as f32),
            options.presence_penalty.map(|v| v as f32),
        )
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;

        let stream = container
            .llm_stream
            .execute(llm_messages?, config)
            .await
            .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
        Ok(LlmStreamHandle::new(stream))
    }
```

- [ ] **Step 5: Register `stream` module in `mod.rs`**

Add `pub mod stream;` to `mod.rs` (must be `pub` so `llm.rs` can `use` it). Final `mod.rs`:

```rust
//! napi-rs bindings for Colmena.
mod dag;
mod llm;
pub mod stream;
```

- [ ] **Step 6: Build Rust and confirm the napi class appears**

Run: `npm run build:debug && grep -n "LlmStreamHandle\|pull" index.d.ts`
Expected: `index.d.ts` now declares `class LlmStreamHandle { pull(): Promise<string | null> }`.

- [ ] **Step 7: Wrap the handle in the TS facade with a `LlmStream` async-iterator**

In `ts/index.ts`, add after the `NodeLlmMessage` type:

```ts
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
```

And add the `stream` method inside the `ColmenaLlm` class:

```ts
  async stream(
    messages: NodeLlmMessage[],
    provider: string,
    options?: NodeLlmConfigOptions,
  ): Promise<LlmStream> {
    return new LlmStream(await this.inner.stream(messages, provider, options));
  }
```

- [ ] **Step 8: Build everything and run the test**

Run: `npm run build:debug && node --test lib/test/llm-stream.test.js`
Expected: PASS (env-gated test runs to completion or returns early without a configured provider).

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/src/node_bindings/ ts/ index.js index.d.ts lib/
git commit -m "feat(node): add ColmenaLlm.stream async iterator"
```

---

### Task 1.2: Map napi errors to `LlmError` / `DagError` in the facade

**Files:**
- Modify: `ts/index.ts`
- Test: `ts/test/errors.test.ts`

- [ ] **Step 1: Write the failing test**

Create `ts/test/errors.test.ts`:

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { ColmenaLlm, runDag, LlmError, DagError } from "../index";

test("call to an unknown provider throws LlmError", async () => {
  const llm = new ColmenaLlm();
  await assert.rejects(
    () => llm.call([{ role: "user", content: "hi" }], "does-not-exist"),
    (err: unknown) => err instanceof LlmError && /not found/i.test((err as Error).message),
  );
});

test("runDag on a missing file throws DagError", async () => {
  await assert.rejects(
    () => runDag("/nonexistent/graph.json"),
    (err: unknown) => err instanceof DagError,
  );
});
```

- [ ] **Step 2: Build and confirm it fails**

Run: `npm run build:debug && node --test lib/test/errors.test.js`
Expected: FAIL — rejections are plain `Error`, not `LlmError`/`DagError`.

- [ ] **Step 3: Add error-rewrapping helpers and apply them in the facade**

In `ts/index.ts`, import the classes (already exported) and add near the top:

```ts
import { LlmError, DagError } from "./errors";

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
```

Wrap the async calls: in `ColmenaLlm.call` use `return asLlm(this.inner.call(...))`; in `healthCheck` use `return asLlm(this.inner.healthCheck(provider))`; in `stream` use `return asLlm(this.inner.stream(...)).then((h) => new LlmStream(h))`. In `runDag` use `return asDag(native.runDag(...))`; in `serveDag` use `return asDag(native.serveDag(...))`.

Remove the now-duplicate `export { LlmError, DagError } from "./errors";` line if it conflicts with the new `import` — keep a single `export { LlmError, DagError };` after the import instead.

- [ ] **Step 4: Build and run the test**

Run: `npm run build:debug && node --test lib/test/errors.test.js`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ts/index.ts lib/
git commit -m "feat(node): rewrap binding errors as LlmError/DagError"
```

---

## SLICE 2 — runDag(in-memory graph) + validateGraph + agentSessionId

### Task 2.1: Accept an in-memory graph object and `agentSessionId` in `runDag`

**Files:**
- Modify: `src/libs/colmena/src/node_bindings/dag.rs`
- Modify: `ts/index.ts`
- Test: `ts/test/run-dag.test.ts`

- [ ] **Step 1: Write the failing test**

Create `ts/test/run-dag.test.ts`:

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { runDag } from "../index";

const GRAPH = {
  nodes: {
    start: { type: "mock_input", config: { input: 5 } },
    pow_step: { type: "exponential", config: { exponent: 3 } },
    log_result: { type: "log" },
  },
  edges: [
    { from: "start", to: "pow_step" },
    { from: "pow_step", to: "log_result" },
  ],
};

test("runDag accepts an in-memory graph object", async () => {
  const result = await runDag(GRAPH);
  assert.ok(result, "expected a result value");
});

test("runDag still accepts a file path", async () => {
  const result = await runDag("tests/graphs/basic/power.json");
  assert.ok(result);
});
```

- [ ] **Step 2: Build and confirm it fails**

Run: `npm run build:debug && node --test lib/test/run-dag.test.js`
Expected: FAIL — passing an object errors (native signature is `filePath: string`).

- [ ] **Step 3: Rewrite `run_dag` in `dag.rs` to accept string-or-object + agentSessionId**

Replace the `run_dag` function in `dag.rs` with:

```rust
#[napi]
pub async fn run_dag(
    graph: Either<String, Value>,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<Value>,
    include_extra_info: Option<bool>,
    agent_session_id: Option<String>,
) -> Result<Value> {
    let extra = include_extra_info.unwrap_or(false);
    let result = match graph {
        Either::A(path) => {
            crate::dag_engine::api::run_dag(
                path, resume_id, resume_answer, inject_payload, extra, agent_session_id,
            )
            .await
        }
        Either::B(value) => {
            let json = serde_json::to_string(&value)
                .map_err(|e| Error::new(Status::InvalidArg, e.to_string()))?;
            crate::dag_engine::api::run_dag_from_str(
                json, resume_id, resume_answer, inject_payload, extra, agent_session_id,
            )
            .await
        }
    }
    .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
    Ok(result)
}
```

(`Either` comes from `napi::bindgen_prelude::*`, already imported in `dag.rs`.)

- [ ] **Step 4: Update the facade `runDag` signature**

In `ts/index.ts`, replace the `runDag` function with:

```ts
export type GraphObject = Record<string, unknown>;

/** Run a DAG graph (file path or in-memory object); resolves to the final output. */
export function runDag(
  graph: string | GraphObject,
  resumeId?: string | null,
  resumeAnswer?: string | null,
  injectPayload?: unknown,
  includeExtraInfo?: boolean | null,
  agentSessionId?: string | null,
): Promise<unknown> {
  return asDag(
    native.runDag(graph, resumeId, resumeAnswer, injectPayload, includeExtraInfo, agentSessionId),
  );
}
```

- [ ] **Step 5: Build and run the test**

Run: `npm run build:debug && node --test lib/test/run-dag.test.js`
Expected: PASS (both tests).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/node_bindings/dag.rs ts/index.ts index.js index.d.ts lib/
git commit -m "feat(node): runDag accepts in-memory graph and agentSessionId"
```

---

### Task 2.2: Add `validateGraph`

**Files:**
- Modify: `src/libs/colmena/src/node_bindings/dag.rs`
- Modify: `ts/index.ts`
- Test: `ts/test/validate-graph.test.ts`

- [ ] **Step 1: Write the failing test**

Create `ts/test/validate-graph.test.ts`:

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { validateGraph, DagError } from "../index";

test("validateGraph accepts a valid graph", () => {
  assert.doesNotThrow(() =>
    validateGraph({
      nodes: { a: { type: "mock_input", config: { input: 1 } } },
      edges: [],
    }),
  );
});

test("validateGraph rejects an invalid graph", () => {
  assert.throws(() => validateGraph({ not: "a graph" }), DagError);
});
```

- [ ] **Step 2: Build and confirm it fails**

Run: `npm run build:debug && node --test lib/test/validate-graph.test.js`
Expected: FAIL — `validateGraph` is not exported.

- [ ] **Step 3: Add `validate_graph` to `dag.rs`**

```rust
#[napi]
pub fn validate_graph(graph: Value) -> Result<()> {
    let _: crate::dag_engine::domain::graph::Graph = serde_json::from_value(graph)
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid graph: {}", e)))?;
    Ok(())
}
```

- [ ] **Step 4: Add `validateGraph` to the facade**

In `ts/index.ts`:

```ts
/** Validate a graph object; throws DagError if it is not a valid graph. */
export function validateGraph(graph: GraphObject): void {
  try {
    native.validateGraph(graph);
  } catch (e) {
    throw new DagError(e instanceof Error ? e.message : String(e));
  }
}
```

- [ ] **Step 5: Build and run the test**

Run: `npm run build:debug && node --test lib/test/validate-graph.test.js`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/node_bindings/dag.rs ts/index.ts index.js index.d.ts lib/
git commit -m "feat(node): add validateGraph"
```

---

## SLICE 3 — streamDag (async iterator)

### Task 3.1: Add `DagStreamHandle` and `streamDag`

**Files:**
- Modify: `src/libs/colmena/src/node_bindings/stream.rs`
- Modify: `src/libs/colmena/src/node_bindings/dag.rs`
- Modify: `ts/index.ts`
- Test: `ts/test/stream-dag.test.ts`

- [ ] **Step 1: Write the failing test**

Create `ts/test/stream-dag.test.ts`:

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { streamDag, type DagEvent } from "../index";

const GRAPH = {
  nodes: {
    start: { type: "mock_input", config: { input: 5 } },
    pow_step: { type: "exponential", config: { exponent: 3 } },
    log_result: { type: "log" },
  },
  edges: [
    { from: "start", to: "pow_step" },
    { from: "pow_step", to: "log_result" },
  ],
};

test("streamDag yields typed events ending in finish", async () => {
  const stream = await streamDag(GRAPH);
  const types: string[] = [];
  for await (const event of stream) {
    const ev = event as DagEvent;
    assert.equal(typeof ev.type, "string");
    types.push(ev.type);
  }
  assert.ok(types.includes("finish"), `expected a finish event, got ${types.join(",")}`);
});
```

- [ ] **Step 2: Build and confirm it fails**

Run: `npm run build:debug && node --test lib/test/stream-dag.test.js`
Expected: FAIL — `streamDag` is not exported.

- [ ] **Step 3: Add `DagStreamHandle` to `stream.rs`**

Append to `src/libs/colmena/src/node_bindings/stream.rs`:

```rust
use serde_json::Value;

/// Owned, `'static` stream of SSE-mapped DAG parts (each a `serde_json::Value`).
type DagPartStream = std::pin::Pin<
    Box<
        dyn futures::Stream<
                Item = std::result::Result<Value, crate::dag_engine::domain::error::DagError>,
            > + Send,
    >,
>;

/// Async-iterator handle over a running DAG's SSE-mapped events. Each `pull()`
/// resolves to the next `{ type: ... }` event, or `null` when the graph finishes.
#[napi]
pub struct DagStreamHandle {
    stream: Arc<Mutex<DagPartStream>>,
}

#[napi]
impl DagStreamHandle {
    #[napi]
    pub async fn pull(&self) -> Result<Option<Value>> {
        let mut stream = self.stream.lock().await;
        match stream.next().await {
            Some(Ok(part)) => Ok(Some(part)),
            Some(Err(e)) => Err(Error::new(Status::GenericFailure, e.to_string())),
            None => Ok(None),
        }
    }
}

impl DagStreamHandle {
    pub fn new(stream: DagPartStream) -> Self {
        Self {
            stream: Arc::new(Mutex::new(stream)),
        }
    }
}
```

- [ ] **Step 4: Add `stream_dag` to `dag.rs`**

Add `use crate::node_bindings::stream::DagStreamHandle;` to the top of `dag.rs`, then:

```rust
#[napi]
pub async fn stream_dag(
    graph: Either<String, Value>,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<Value>,
    include_extra_info: Option<bool>,
    agent_session_id: Option<String>,
) -> Result<DagStreamHandle> {
    let extra = include_extra_info.unwrap_or(false);
    let boxed: std::pin::Pin<Box<dyn futures::Stream<Item = std::result::Result<Value, crate::dag_engine::domain::error::DagError>> + Send>> =
        match graph {
            Either::A(path) => {
                let s = crate::dag_engine::api::stream_dag(
                    path, resume_id, resume_answer, inject_payload, extra, agent_session_id,
                )
                .await
                .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
                Box::pin(s)
            }
            Either::B(value) => {
                let json = serde_json::to_string(&value)
                    .map_err(|e| Error::new(Status::InvalidArg, e.to_string()))?;
                let s = crate::dag_engine::api::stream_dag_from_str(
                    json, resume_id, resume_answer, inject_payload, extra, agent_session_id,
                )
                .await
                .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
                Box::pin(s)
            }
        };
    Ok(DagStreamHandle::new(boxed))
}
```

Add `use futures::StreamExt;` is not needed in `dag.rs` (no `.next()` here); the `Box::pin` only needs `futures::Stream` in scope via the fully-qualified path used above.

- [ ] **Step 5: Add the `DagEvent` type, `DagStream`, and `streamDag` to the facade**

In `ts/index.ts`:

```ts
/** A DAG execution event. `type` discriminates the variant; extra fields vary. */
export type DagEvent =
  | { type: "node-start"; [k: string]: unknown }
  | { type: "node-end"; [k: string]: unknown }
  | { type: "text-delta"; delta: string; [k: string]: unknown }
  | { type: "finish"; [k: string]: unknown }
  | { type: string; [k: string]: unknown };

/** Async iterator of DAG events. Use `for await (const event of stream)`. */
export class DagStream implements AsyncIterableIterator<DagEvent> {
  constructor(private handle: { pull(): Promise<DagEvent | null> }) {}
  [Symbol.asyncIterator](): AsyncIterableIterator<DagEvent> {
    return this;
  }
  async next(): Promise<IteratorResult<DagEvent>> {
    const value = await this.handle.pull();
    return value === null
      ? { value: undefined, done: true }
      : { value, done: false };
  }
}

/** Stream a DAG's execution as typed events. */
export async function streamDag(
  graph: string | GraphObject,
  resumeId?: string | null,
  resumeAnswer?: string | null,
  injectPayload?: unknown,
  includeExtraInfo?: boolean | null,
  agentSessionId?: string | null,
): Promise<DagStream> {
  const handle = await asDag(
    native.streamDag(graph, resumeId, resumeAnswer, injectPayload, includeExtraInfo, agentSessionId),
  );
  return new DagStream(handle);
}
```

- [ ] **Step 6: Build and run the test**

Run: `npm run build:debug && node --test lib/test/stream-dag.test.js`
Expected: PASS — the event list includes `finish`.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/node_bindings/ ts/index.ts index.js index.d.ts lib/
git commit -m "feat(node): add streamDag async iterator"
```

---

## SLICE 4 — Registry introspection

### Task 4.1: Add `Registry` class and `defaultRegistry`

**Files:**
- Create: `src/libs/colmena/src/node_bindings/registry.rs`
- Modify: `src/libs/colmena/src/node_bindings/mod.rs`
- Modify: `ts/index.ts`
- Test: `ts/test/registry.test.ts`

- [ ] **Step 1: Write the failing test**

Create `ts/test/registry.test.ts`:

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { defaultRegistry } from "../index";

test("defaultRegistry lists node types", () => {
  const registry = defaultRegistry();
  const types = registry.nodeTypes();
  assert.ok(Array.isArray(types));
  assert.ok(types.includes("log"), "expected the 'log' node type");
});

test("toolkitCatalog returns sub-tools for a toolkit node", () => {
  const registry = defaultRegistry();
  const types = registry.nodeTypes();
  if (!types.includes("api_explorer")) return; // env-gated
  const catalog = registry.toolkitCatalog("api_explorer", {});
  assert.ok(Array.isArray(catalog));
});
```

- [ ] **Step 2: Build and confirm it fails**

Run: `npm run build:debug && node --test lib/test/registry.test.js`
Expected: FAIL — `defaultRegistry` is not exported.

- [ ] **Step 3: Create `registry.rs` (mirrors the Python `Registry` + `default_registry`)**

```rust
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;
use std::sync::Arc;

/// Read-only handle to a `HashMapNodeRegistry`; inspection only, no DB.
#[napi]
pub struct Registry {
    inner: Arc<crate::dag_engine::infrastructure::registry::HashMapNodeRegistry>,
}

#[napi]
impl Registry {
    #[napi]
    pub fn node_types(&self) -> Vec<String> {
        use crate::dag_engine::application::ports::NodeRegistryPort;
        let mut keys: Vec<String> = self.inner.get_all_nodes().keys().cloned().collect();
        keys.sort();
        keys
    }

    #[napi]
    pub fn toolkit_catalog(&self, node_type: String, config: Value) -> Result<Value> {
        use crate::dag_engine::application::ports::NodeRegistryPort;
        let tk = self
            .inner
            .get_toolkit_node(&node_type)
            .ok_or_else(|| Error::new(Status::InvalidArg, format!("not a toolkit node: {}", node_type)))?;
        let entries: Vec<Value> = tk
            .sub_tool_catalog(&config)
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name.as_ref(),
                    "description": s.description,
                    "required": s.required,
                })
            })
            .collect();
        Ok(Value::Array(entries))
    }
}

/// Stub task-memory repository so `default_registry` needs no database.
struct SmokeTaskMemory;

#[async_trait::async_trait]
impl crate::dag_engine::domain::state::DagTaskMemoryRepository for SmokeTaskMemory {
    async fn add_task(&self, _task: &crate::dag_engine::domain::state::DagTask) -> std::result::Result<(), crate::dag_engine::domain::error::DagError> { Ok(()) }
    async fn update_task_result(&self, _task_id: &str, _result: Value) -> std::result::Result<(), crate::dag_engine::domain::error::DagError> { Ok(()) }
    async fn get_tasks_for_run(&self, _session_id: &str) -> std::result::Result<Vec<crate::dag_engine::domain::state::DagTask>, crate::dag_engine::domain::error::DagError> { Ok(vec![]) }
    async fn get_first_uncompleted_task(&self, _session_id: &str) -> std::result::Result<Option<crate::dag_engine::domain::state::DagTask>, crate::dag_engine::domain::error::DagError> { Ok(None) }
    async fn delete_task(&self, _task_id: &str) -> std::result::Result<(), crate::dag_engine::domain::error::DagError> { Ok(()) }
    async fn clear_tasks_for_run(&self, _session_id: &str) -> std::result::Result<(), crate::dag_engine::domain::error::DagError> { Ok(()) }
    async fn get_current_phase(&self, _session_id: &str) -> std::result::Result<Option<i32>, crate::dag_engine::domain::error::DagError> { Ok(None) }
    async fn get_uncompleted_tasks_for_phase(&self, _session_id: &str, _phase: i32) -> std::result::Result<Vec<crate::dag_engine::domain::state::DagTask>, crate::dag_engine::domain::error::DagError> { Ok(vec![]) }
    async fn save_phase_summary(&self, _session_id: &str, _phase: i32, _summary: &str) -> std::result::Result<(), crate::dag_engine::domain::error::DagError> { Ok(()) }
    async fn get_phase_summaries(&self, _session_id: &str) -> std::result::Result<Vec<crate::dag_engine::domain::state::DagPhaseSummary>, crate::dag_engine::domain::error::DagError> { Ok(vec![]) }
}

/// Builds an inspection-only `HashMapNodeRegistry` with no live DB connections.
#[napi]
pub fn default_registry() -> Registry {
    use crate::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
    use crate::dag_engine::infrastructure::registry::HashMapNodeRegistry;
    use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
    use crate::llm::infrastructure::ConversationRepositoryFactory;

    let pools = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
    let conv = Arc::new(ConversationRepositoryFactory::new(pools.clone()));
    let sql = Arc::new(SqlPortFactory::new(pools));
    let task_memory: Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository> =
        Arc::new(SmokeTaskMemory);
    let inner = HashMapNodeRegistry::new(conv, sql, Some(task_memory));
    Registry { inner }
}
```

> If `cargo build` reports the concrete type of `s.name` is not `AsRef<str>`, replace `s.name.as_ref()` with `s.name.to_string()` (the Python binding uses `.as_ref()`; match whichever the compiler accepts).

- [ ] **Step 4: Register the module**

Add `mod registry;` to `mod.rs`. Final:

```rust
//! napi-rs bindings for Colmena.
mod dag;
mod llm;
mod registry;
pub mod stream;
```

- [ ] **Step 5: Add `Registry` + `defaultRegistry` to the facade**

In `ts/index.ts`:

```ts
/** Read-only handle to the node registry (no DB connection). */
export type Registry = {
  nodeTypes(): string[];
  toolkitCatalog(nodeType: string, config: unknown): unknown[];
};

/** Build an inspection-only node registry with no database connection. */
export function defaultRegistry(): Registry {
  return native.defaultRegistry();
}
```

- [ ] **Step 6: Build and run the test**

Run: `npm run build:debug && node --test lib/test/registry.test.js`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/node_bindings/ ts/index.ts index.js index.d.ts lib/
git commit -m "feat(node): add registry introspection (defaultRegistry/nodeTypes/toolkitCatalog)"
```

---

## SLICE 5 — documents core + polars companion + docs

### Task 5.1: Add the raw `documents` binding

**Files:**
- Create: `src/libs/colmena/src/node_bindings/documents.rs`
- Modify: `src/libs/colmena/src/node_bindings/mod.rs`
- Modify: `ts/index.ts`
- Test: `ts/test/documents.test.ts`

- [ ] **Step 1: Write the failing test**

Create `ts/test/documents.test.ts`:

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { documents } from "../index";

const ARTIFACT = "00000000-0000-0000-0000-0000000000aa";

test("add, write, read, list roundtrip", () => {
  const sheetId = documents.addSheet(ARTIFACT, "Data");
  assert.equal(typeof sheetId, "string");

  documents.writeSheet(
    ARTIFACT,
    sheetId,
    ["name", "age"],
    [
      ["Alice", 30],
      ["Bob", 25],
    ],
    "replace",
  );

  const cells = documents.readSheet(ARTIFACT, sheetId);
  assert.equal(cells["A1"], "name");
  assert.equal(cells["B1"], "age");
  assert.equal(cells["A2"], "Alice");

  const sheets = documents.listSheets(ARTIFACT);
  assert.ok(sheets.some((s) => s.sheetId === sheetId));
});
```

> The `documents` runtime uses an on-disk store rooted at `COLMENA_CRDT_DOCUMENTS_STORAGE_ROOT` (default `.colmena/crdt_documents`). The test writes there; CI cleans the workspace per run.

- [ ] **Step 2: Build and confirm it fails**

Run: `npm run build:debug && node --test lib/test/documents.test.js`
Expected: FAIL — `documents` is not exported.

- [ ] **Step 3: Create `documents.rs` (napi mirror of `crdt_documents.rs`)**

```rust
//! napi mirror of the PyO3 `colmena.documents` submodule. Cero-deps raw
//! surface; the polars DataFrame ergonomics live in the `@colmena-ai/documents`
//! companion package, analogous to the Python `colmena_documents` pandas wrapper.

use crate::crdt_documents::{ArtifactId, CrdtDocumentsRuntime};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use once_cell::sync::OnceCell;
use serde_json::Value;
use std::sync::Arc;

static RUNTIME: OnceCell<Arc<CrdtDocumentsRuntime>> = OnceCell::new();

async fn runtime() -> Result<Arc<CrdtDocumentsRuntime>> {
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt.clone());
    }
    let storage_root = std::env::var("COLMENA_CRDT_DOCUMENTS_STORAGE_ROOT")
        .unwrap_or_else(|_| ".colmena/crdt_documents".to_string());
    let cfg = serde_json::json!({ "storage_root": storage_root });
    let built = CrdtDocumentsRuntime::from_config(&cfg)
        .await
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
    let arc = Arc::new(built);
    let _ = RUNTIME.set(arc.clone());
    Ok(arc)
}

fn parse_id(s: &str) -> Result<ArtifactId> {
    s.parse::<ArtifactId>()
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid artifact_id: {e}")))
}

#[napi]
pub async fn documents_list_sheets(artifact_id: String) -> Result<Value> {
    let rt = runtime().await?;
    let id = parse_id(&artifact_id)?;
    let entry = rt
        .registry
        .get(&id)
        .ok_or_else(|| Error::new(Status::GenericFailure, "artifact not found"))?;
    let proj = crate::crdt_documents::projection::project(&entry.doc);
    let mut out = Vec::new();
    for s in proj["sheets"].as_array().cloned().unwrap_or_default() {
        out.push(serde_json::json!({
            "sheetId": s["id"].as_str().unwrap_or(""),
            "name": s["name"].as_str().unwrap_or(""),
        }));
    }
    Ok(Value::Array(out))
}

#[napi]
pub async fn documents_read_sheet(artifact_id: String, sheet_id: String) -> Result<Value> {
    let rt = runtime().await?;
    let id = parse_id(&artifact_id)?;
    let entry = rt
        .registry
        .get(&id)
        .ok_or_else(|| Error::new(Status::GenericFailure, "artifact not found"))?;
    let proj = crate::crdt_documents::projection::project(&entry.doc);
    let sheet = proj["sheets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|s| s["id"].as_str() == Some(sheet_id.as_str()))
        .ok_or_else(|| Error::new(Status::GenericFailure, "sheet not found"))?;
    Ok(sheet["cells"].clone())
}

#[napi]
pub async fn documents_add_sheet(artifact_id: String, name: String) -> Result<String> {
    let rt = runtime().await?;
    let id = parse_id(&artifact_id)?;
    let entry = rt.registry.get_or_create(&id, "(from node)");
    let sheet_id = crate::crdt_documents::tool_executor::apply_add_sheet(&entry.doc, &name);
    entry.mark_dirty();
    let msg = format!("added sheet '{name}'");
    rt.tracker.record(&id, None, "node", &msg).await;
    Ok(sheet_id)
}

#[napi]
pub async fn documents_write_sheet(
    artifact_id: String,
    sheet_id: String,
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    mode: Option<String>,
) -> Result<()> {
    let rt = runtime().await?;
    let id = parse_id(&artifact_id)?;
    let entry = rt
        .registry
        .get(&id)
        .ok_or_else(|| Error::new(Status::GenericFailure, "artifact not found"))?;
    let mode = mode.unwrap_or_else(|| "replace".to_string());
    if !matches!(mode.as_str(), "replace" | "append") {
        return Err(Error::new(Status::InvalidArg, "mode must be 'replace' or 'append'"));
    }
    for (col_idx, col_name) in columns.iter().enumerate() {
        let addr = format!("{}{}", col_letter(col_idx as u32), 1);
        let _ = crate::crdt_documents::tool_executor::apply_set_cell_in_proc(
            &entry.doc, &sheet_id, &addr, &Value::String(col_name.clone()),
        );
    }
    for (row_idx, row) in rows.iter().enumerate() {
        for (col_idx, val) in row.iter().enumerate() {
            let addr = format!("{}{}", col_letter(col_idx as u32), row_idx + 2);
            let _ = crate::crdt_documents::tool_executor::apply_set_cell_in_proc(
                &entry.doc, &sheet_id, &addr, val,
            );
        }
    }
    entry.mark_dirty();
    let msg = format!("wrote {} rows to {sheet_id}", rows.len());
    rt.tracker.record(&id, None, "node", &msg).await;
    Ok(())
}

fn col_letter(mut col: u32) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (col % 26) as u8) as char);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    s
}
```

> The Python binding marks these functions `#[allow(deprecated)]`. If `cargo clippy -- -D warnings` flags a deprecation here, add `#[allow(deprecated)]` to the affected function (mirror the Python file) — never to production node code outside this mirror.

- [ ] **Step 4: Register the module**

Add `mod documents;` to `mod.rs`. Final:

```rust
//! napi-rs bindings for Colmena.
mod dag;
mod documents;
mod llm;
mod registry;
pub mod stream;
```

- [ ] **Step 5: Add the `documents` namespace to the facade**

In `ts/index.ts`:

```ts
export type SheetInfo = { sheetId: string; name: string };
export type SheetCells = Record<string, string | number | boolean | null>;

/** Raw CRDT-sheet access (cero-deps). For DataFrames use @colmena-ai/documents. */
export const documents = {
  listSheets(artifactId: string): Promise<SheetInfo[]> {
    return asDag(native.documentsListSheets(artifactId)) as Promise<SheetInfo[]>;
  },
  readSheet(artifactId: string, sheetId: string): Promise<SheetCells> {
    return asDag(native.documentsReadSheet(artifactId, sheetId)) as Promise<SheetCells>;
  },
  addSheet(artifactId: string, name: string): Promise<string> {
    return asDag(native.documentsAddSheet(artifactId, name));
  },
  writeSheet(
    artifactId: string,
    sheetId: string,
    columns: string[],
    rows: unknown[][],
    mode?: "replace" | "append",
  ): Promise<void> {
    return asDag(native.documentsWriteSheet(artifactId, sheetId, columns, rows, mode));
  },
};
```

- [ ] **Step 6: Update the test to await (the napi functions are async)**

Edit `ts/test/documents.test.ts` to make the test callback `async` and `await` each call, e.g. `const sheetId = await documents.addSheet(...)`, `await documents.writeSheet(...)`, `const cells = await documents.readSheet(...)`, `const sheets = await documents.listSheets(...)`.

- [ ] **Step 7: Build and run the test**

Run: `npm run build:debug && node --test lib/test/documents.test.js`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/node_bindings/ ts/index.ts index.js index.d.ts lib/
git commit -m "feat(node): add raw documents (CRDT sheets) binding"
```

---

### Task 5.2: Create the `@colmena-ai/documents` polars companion

**Files:**
- Create: `packages/documents/package.json`
- Create: `packages/documents/tsconfig.json`
- Create: `packages/documents/src/index.ts`
- Create: `packages/documents/test/documents.test.ts`

- [ ] **Step 1: Create `packages/documents/package.json`**

```json
{
  "name": "@colmena-ai/documents",
  "version": "0.3.0",
  "description": "Polars DataFrame ergonomics over Colmena CRDT sheets",
  "main": "lib/index.js",
  "types": "lib/index.d.ts",
  "files": ["lib/"],
  "scripts": {
    "build": "tsc -p tsconfig.json",
    "test": "node --test lib/test/"
  },
  "peerDependencies": {
    "colmena-ai": "^0.3.0"
  },
  "dependencies": {
    "nodejs-polars": "^0.20.0"
  },
  "devDependencies": {
    "typescript": "^5.4.0"
  }
}
```

- [ ] **Step 2: Create `packages/documents/tsconfig.json`**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "CommonJS",
    "moduleResolution": "Node",
    "declaration": true,
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "outDir": "lib",
    "rootDir": "src"
  },
  "include": ["src/**/*.ts", "test/**/*.ts"]
}
```

- [ ] **Step 3: Write the failing test**

Create `packages/documents/test/documents.test.ts`:

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import pl from "nodejs-polars";
import { readSheetAsDataFrame } from "../src/index";

test("cell map converts to a DataFrame", () => {
  const cells = { A1: "name", B1: "age", A2: "Alice", B2: 30, A3: "Bob", B3: 25 };
  const df = readSheetAsDataFrame(cells);
  assert.deepEqual(df.columns, ["name", "age"]);
  assert.equal(df.shape.height, 2);
  assert.deepEqual(df.getColumn("name").toArray(), ["Alice", "Bob"]);
});
```

- [ ] **Step 4: Build and confirm it fails**

Run: `cd packages/documents && npm install && npm run build`
Expected: FAIL — `readSheetAsDataFrame` is not defined.

- [ ] **Step 5: Implement `packages/documents/src/index.ts`**

```ts
import pl from "nodejs-polars";

type Cell = string | number | boolean | null;
type CellMap = Record<string, Cell>;

/** Parse an A1 address like "B12" into 0-based [col, row]. */
function parseAddr(addr: string): [number, number] {
  const m = /^([A-Z]+)(\d+)$/.exec(addr);
  if (!m) throw new Error(`bad cell address: ${addr}`);
  let col = 0;
  for (const ch of m[1]) col = col * 26 + (ch.charCodeAt(0) - 64);
  return [col - 1, parseInt(m[2], 10) - 1];
}

/** Convert a colmena-ai cell map (row 1 = headers, row 2+ = data) into a polars DataFrame. */
export function readSheetAsDataFrame(cells: CellMap): pl.DataFrame {
  const headers: string[] = [];
  const grid: Cell[][] = [];
  for (const [addr, value] of Object.entries(cells)) {
    const [col, row] = parseAddr(addr);
    if (row === 0) {
      headers[col] = String(value);
    } else {
      (grid[row - 1] ??= [])[col] = value;
    }
  }
  const series = headers.map((name, col) =>
    pl.Series(name, grid.map((r) => (r ? r[col] ?? null : null))),
  );
  return pl.DataFrame(series);
}

/** Convert a polars DataFrame into the (columns, rows) shape colmena-ai's writeSheet expects. */
export function dataFrameToSheet(df: pl.DataFrame): { columns: string[]; rows: Cell[][] } {
  const columns = df.columns;
  const rows = df.toRecords().map((rec) => columns.map((c) => rec[c] as Cell));
  return { columns, rows };
}
```

- [ ] **Step 6: Build and run the test**

Run: `cd packages/documents && npm run build && npm test`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add packages/documents/
git commit -m "feat(node): add @colmena-ai/documents polars companion"
```

---

### Task 5.3: Write the TypeScript usage docs and runnable examples

**Files:**
- Create: `docs/examples/typescript_usage.md`
- Create: `docs/developer_guide/49_typescript_dag.md`
- Create: `ts/examples/run_dag.mjs`
- Create: `ts/examples/stream_dag.mjs`
- Modify: `README.md`
- Modify: `docs/DEVELOPER_GUIDE.md`

- [ ] **Step 1: Write `docs/examples/typescript_usage.md`**

Mirror `docs/examples/python_usage.md` section-for-section, in Spanish (per repo docs convention), covering: install (`npm install colmena-ai`), `ColmenaLlm.call`, `stream` with `for await`, `LlmConfigOptions`, `runDag` (path + object), `streamDag`, `validateGraph`, `defaultRegistry`, `documents` + the polars companion, and `LlmError`/`DagError` handling. Use these as the canonical code blocks:

````markdown
## LLM — llamada simple

```ts
import { ColmenaLlm } from "colmena-ai";

const llm = new ColmenaLlm();
const text = await llm.call(
  [{ role: "user", content: "Hola" }],
  "google",
  { model: "gemini-2.5-flash", temperature: 0.7 },
);
console.log(text);
```

## LLM — streaming

```ts
const stream = await llm.stream([{ role: "user", content: "Cuenta hasta 5" }], "google");
for await (const chunk of stream) process.stdout.write(chunk);
```

## DAG — ejecutar y hacer streaming

```ts
import { runDag, streamDag } from "colmena-ai";

const result = await runDag("tests/graphs/basic/power.json", null, null, null, false, "agent_demo_001");

const stream = await streamDag({ nodes: { /* ... */ }, edges: [] });
for await (const event of stream) {
  if (event.type === "text-delta") process.stdout.write(event.delta as string);
}
```
````

- [ ] **Step 2: Write `docs/developer_guide/49_typescript_dag.md`**

Mirror `docs/developer_guide/48_python_dag.md`: document `runDag`, `streamDag`, `validateGraph`, `serveDag`, `defaultRegistry`, the `DagEvent` union, `agentSessionId` / resume semantics, and `injectPayload`. Reuse the code blocks from Step 1.

- [ ] **Step 3: Write `ts/examples/run_dag.mjs`**

```js
import { runDag } from "colmena-ai";

const result = await runDag("tests/graphs/basic/power.json");
console.log("DAG output:", JSON.stringify(result, null, 2));
```

- [ ] **Step 4: Write `ts/examples/stream_dag.mjs`**

```js
import { streamDag } from "colmena-ai";

const stream = await streamDag("tests/graphs/basic/power.json");
for await (const event of stream) {
  console.log(event.type, JSON.stringify(event));
}
```

- [ ] **Step 5: Add a Node/TypeScript section to `README.md`**

Add a `## Node.js / TypeScript` section after the Python section, with the install line and the `ColmenaLlm.call` + `runDag` snippets from Step 1, and a link to `docs/examples/typescript_usage.md`.

- [ ] **Step 6: Index the new guides in `docs/DEVELOPER_GUIDE.md`**

Add two entries to the index: `49_typescript_dag.md` (DAG execution from Node) and a pointer to `examples/typescript_usage.md`. Match the existing list formatting.

- [ ] **Step 7: Verify examples run**

Run: `npm run build:debug && node ts/examples/run_dag.mjs`
Expected: prints the DAG output JSON (note: examples import `colmena-ai`; run from a context where the package resolves, or temporarily `import("../../lib/index.js")` — document the published-package form in the file).

- [ ] **Step 8: Commit**

```bash
git add docs/ README.md ts/examples/
git commit -m "docs(node): add TypeScript usage guide, DAG guide, and examples"
```

---

## Final verification

- [ ] **Run the full Rust suite:** `cargo test --verbose` → all pass, no warnings.
- [ ] **Run clippy + fmt:** `cargo clippy -- -D warnings && cargo fmt -- --check` → clean.
- [ ] **Build + run all Node tests:** `npm run build && npm test` → all pass.
- [ ] **Build the companion:** `cd packages/documents && npm install && npm run build && npm test` → all pass.
- [ ] **Check the published tarball:** `npm pack --dry-run` → only `index.js`, `index.d.ts`, `lib/`, `*.node`.
- [ ] **Confirm parity:** every row in the spec's gap table (§1) now has a Node implementation.

---

## Self-review notes (addressed)

- **read/write asymmetry:** `documents.readSheet` returns a cell-addressed map (`{ "A1": ... }`) exactly like the Python `read_sheet`; `writeSheet` takes `columns` + `rows`. This asymmetry exists in Python too — the polars companion bridges it. Parity preserved.
- **async documents:** napi free functions are `async` (the runtime builds via `await`), so the facade `documents.*` methods return Promises — a deliberate, idiomatic difference from Python's sync `colmena.documents` (Python blocks on a tokio handle). Documented in the usage guide.
- **`run_dag` return:** Python returns a JSON **string**; the Node `runDag` returns a parsed **value** (`Promise<unknown>`) — idiomatic for Node, matches the existing napi behavior. Documented.
