# src/libs/colmena/src/crdt_documents/artifact_id.rs

**Layer:** domain  **Purpose:** Defines ArtifactId, a stable ULID-based opaque identifier for CRDT documents. Provides generation, parsing, and serialization for use in URLs and client-facing APIs.

## Symbols

- `ArtifactId` (struct, pub) — String-backed newtype wrapper for "art_" prefixed 26-character ULID identifiers
- `ArtifactId::new()` (fn, pub) — Generates a new ArtifactId by creating a fresh ULID and prefixing with "art_"
- `ArtifactId::as_str()` (fn, pub) — Returns the ID string as a borrowed &str reference
- `Default for ArtifactId` (impl, pub) — Provides default ID generation by delegating to `new()`
- `Display for ArtifactId` (impl, pub) — Implements fmt::Display to render the ID as a string
- `FromStr for ArtifactId` (impl, pub) — Parses "art_XXXXX" format strings with prefix and length validation
- `ArtifactIdError` (enum, pub) — Enumerated error type for ID parsing and validation failures
- `ArtifactIdError::BadPrefix` (variant) — Variant for IDs that do not start with "art_" prefix
- `ArtifactIdError::BadLength` (variant) — Variant for IDs that are not exactly 30 characters (4 + 26)
- `tests` (mod, test) — Test module with four unit tests validating generation, round-trip parsing, prefix rejection, and length rejection

## File-level notes

- **Improvement candidate:** The `FromStr` implementation validates prefix and length but does not validate that the 26 characters after "art_" conform to valid ULID characters (Crockford base32: [0-7A-Z]). This allows strings like "art_ZZZZZZZZZZZZZZZZZZZZZZZZ" to parse successfully despite being invalid ULIDs. Consider adding ULID character-set validation to `FromStr`.
- No derives marked for skipping; all derives are standard and justified (Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize).
- Test coverage is complete: generation, round-trip, bad prefix, bad length.
