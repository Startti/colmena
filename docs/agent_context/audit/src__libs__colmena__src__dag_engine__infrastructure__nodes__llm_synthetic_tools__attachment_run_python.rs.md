# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python.rs

**Layer:** infrastructure  
**Purpose:** Implements `attachment_run_python` synthetic tool — executes user-provided pandas code against CSV/XLSX attachments in a sandboxed environment, supporting both inline and signed-URL sources, ~600× cheaper per query than `load_attachment`.

## Symbols

- `ATTACHMENT_RUN_PYTHON_TOOL_NAME` (const, pub) — Tool name constant "attachment_run_python" surfaced to LLM
- `CODE_TIMEOUT_SECS` (const, private) — 30-second wall-clock execution timeout, mirrors `gsheets_run_python`
- `OUTPUT_BYTE_CAP` (const, private) — 50 KB truncation cap for stdout and error strings
- `AttachmentRunPythonArgs` (struct, pub) — LLM tool arguments: `attachment_id`, `code` (required); `delimiter`, `sheet_name`, `header_row` (optional)
- `AttachmentRunPythonResponse` (struct, pub) — Tool response shape: `stdout`, `result`, `duration_ms`, `row_count`, `columns`, optional `sheet_name` and `error`
- `build_attachment_run_python_tool_definition` (fn, pub) — Constructs `ToolDefinition` for LLM from args schema and text registry
- `wrap_user_code` (fn, private) — Wraps user code with pandas/numpy imports, DataFrame preload, serialization postlude; user assigns `result` global which is serialised via `__col_serialise`
- `truncate` (fn, private) — Truncates string to byte cap preserving char boundaries, appends `[truncated]` if cut
- `dispatch_attachment_run_python_via_executor` (async fn, pub) — Main entry point: parses args, fetches attachment bytes, parses CSV/XLSX to records, wraps code, runs in `spawn_blocking` with timeout, extracts stdout + `result` global
- `success_envelope` (fn, private) — Wraps `AttachmentRunPythonResponse` in `ToolResult::success`
- `err_envelope` (fn, private) — Wraps error message in `ToolResult::success` with `{ "error": msg, "source": "execution" }` JSON shape
- `_hashmap_keep_alive` (fn, private) — Dead-code placeholder to prevent unused import warnings during refactors [FLAG: dead_candidate — explicitly marked allow(dead_code) with comment "Removed at any cleanup pass"; imports HashMap which is never used]
- `tests` (mod, cfg(test)) — Test module with 5 unit tests covering arg deserialization, code wrapping, and string truncation

## File-level notes

- Reuses `execute_sandboxed_helper` from `python_node` in `restricted` mode with the same sandbox (pandas, numpy, scipy.stats, math, datetime, decimal, json, re, statistics, string, collections, functools, itertools allowed; os, sys, subprocess, socket, urllib, requests, importlib, builtins, ctypes blocked)
- Attachment resolution is uniform — `DagToolExecutor::fetch_attachment_bytes` and `lookup_attachment_meta` handle both inline `data:` base64 and signed-URL sources without dispatcher awareness (commit `479c321`)
- Output convention: user assigns to `result` global (serialised via postlude's `__col_serialise` helper) or prints via `print()` (captured to stdout); both truncated at 50 KB before returning to LLM
- Dispatcher uses triple-nested Result from `tokio::task::spawn_blocking` → `timeout` → `execute_sandboxed_helper` (line 276: `Ok(Ok(Ok(r)))` pattern)
- No breaking changes or external dependencies beyond existing infrastructure (serde, schemars, tokio)
