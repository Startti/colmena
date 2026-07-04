# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- feat(dag_engine): mid-run liveness — `Progress` heartbeat event while an in-flight
  node is silent (default 20s, `COLMENA_HEARTBEAT_INTERVAL_SECS`, 0=off) and idle
  watchdog that aborts a node after silence (default 300s, `COLMENA_IDLE_TIMEOUT_SECS`,
  0=off) with a descriptive `node '<id>' produced no events for Ns` failure. SseMapper
  emits `{"type":"status","stage":"running","node_id",...}` for heartbeats. Real events
  (including subgraph-as-tool inner events) reset both clocks; heartbeats never do.

## [0.3.0] - 2026-03-18

### Added
- Complete architecture migration to Workspace-based layout in `src/libs/colmena`.
- Node.js/TypeScript bindings via `napi-rs`.
- Unified `run_dag` signature (5 arguments) across Rust, Python, and Node.js.
- Fully resolved all clippy warnings and modernised PyO3 integration (0.21.x).
- Corrected CI/CD workflows for workspace member builds.


### Added
- Initial project setup with Hexagonal Architecture
- Multi-provider LLM support (OpenAI, Gemini, Anthropic)
- Synchronous and streaming API calls
- Native Python bindings via PyO3
- Comprehensive test suite
- CI/CD workflows for develop, staging, and main branches
- Conventional commits validation with git hooks
- Automatic semantic versioning
- PyPI and TestPyPI publishing

### Documentation
- Developer guide with architecture details
- CI/CD guide with branch protection rules
- Git hooks and conventional commits guide
- PyPI project description

---

*This changelog is automatically updated on each release.*
