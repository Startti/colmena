# src/libs/colmena/src/documents/application/get_head.rs

**Layer:** application  
**Purpose:** Implements the GetHead use case for retrieving artifact metadata and version history. Queries current version state, extracts source attribution, and optionally compiles a summary of user-edits since a specified version.

## Symbols

- `GetHeadOutput` (struct, pub) — Result data carrier: artifact_id, current_version, updated_at timestamp, last_source (agent/user), summary_since (formatted version change lines), versions_in_window (version list since filter).
- `GetHeadInput` (struct, pub) — Input carrier: artifact_id to query, optional since_version filter for change summary.
- `GetHeadUseCase` (struct, pub) — Application use case orchestrator holding Arc<dyn ArtifactStore> dependency.
- `GetHeadUseCase::execute` (async fn, pub) — Main use case: reads artifact metadata and current version, extracts last_source from patch metadata, optionally compiles version summary (user edits only) and version window if since_version provided, returns GetHeadOutput or DocumentError.

## File-level notes

- **Duplication (improvement):** Source extraction logic (patch.get("source").and_then(|s| s.as_str()).unwrap_or("agent").to_string()) appears identically in lines 33–39 and 49–55; could be extracted to a helper method to reduce repetition and clarify intent.
- **Magic strings (improvement):** Hardcoded fallback "agent" (lines 38, 54) and comparison "user" (line 56) are semantically significant but not named constants; could improve maintainability and discoverability.
- **Vector iteration (note):** Loop at lines 46–68 reads each version's full data (await? at line 48) to extract source and summary; no partial-success or logging on individual failures — any read error aborts the entire operation. Consistent with the Result return type, but worth noting for performance/observability in high-version-count scenarios.
