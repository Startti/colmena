# src/libs/colmena/src/documents/domain/ids.rs

**Layer:** domain  
**Purpose:** Strongly-typed ID wrappers and artifact metadata for document operations. Defines four newtype ID identifiers (ArtifactId, VersionId, SessionId, AssetId) and an ArtifactKind enum for file metadata.

## Symbols

- `ArtifactId` (struct, pub) — newtype wrapper around String for artifact identifiers
- `ArtifactId::new` (pub fn) — constructs an ArtifactId from any stringifiable type
- `ArtifactId::as_str` (pub fn) — borrows the inner string as &str
- `impl Display for ArtifactId` (impl, pub) — Display trait to render artifact ID as string
- `VersionId` (struct, pub) — newtype wrapper around String for version identifiers (e.g., "v1", "v2")
- `VersionId::new` (pub fn) — constructs a VersionId from any stringifiable type
- `VersionId::initial` (pub fn) — returns initial version "v1"
- `VersionId::next` (pub fn) — increments version number; parses "v{n}" to "v{n+1}", defaults to "v1" on parse failure
- `VersionId::as_str` (pub fn) — borrows the inner string as &str
- `VersionId::number` (pub fn) — extracts numeric part after "v" prefix, returns None if parse fails
- `impl Display for VersionId` (impl, pub) — Display trait to render version ID as string
- `SessionId` (struct, pub) — newtype wrapper around String for session identifiers
- `SessionId::new` (pub fn) — constructs a SessionId from any stringifiable type
- `SessionId::as_str` (pub fn) — borrows the inner string as &str
- `impl Display for SessionId` (impl, pub) — Display trait to render session ID as string
- `AssetId` (struct, pub) — newtype wrapper around String for asset identifiers
- `AssetId::new` (pub fn) — constructs an AssetId from any stringifiable type
- `AssetId::as_str` (pub fn) — borrows the inner string as &str
- `impl Display for AssetId` (impl, pub) — Display trait to render asset ID as string
- `ArtifactKind` (enum, pub) — enum with three variants (Excel, Word, Html) representing document file types
- `ArtifactKind::extension` (pub fn) — returns file extension for artifact kind ("xlsx", "docx", "html")
- `ArtifactKind::mime` (pub fn) — returns MIME type string for artifact kind
- `tests` (mod, private) — comprehensive unit test suite covering version increment, parsing, artifact kind metadata, and serde roundtrips

## File-level notes

- All derives are standard and necessary (Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize for ID types; Copy, Serialize, Deserialize for enum).
- Four ID newtype structs follow an identical pattern intentionally for compile-time type safety (no cross-ID confusion).
- `VersionId::next()` uses `unwrap_or(0)` fallback on parse failure, treating malformed IDs defensively (e.g., "vfoo" → "v1"); this is intentional but undocumented.
- All public methods include `.as_str()` accessor for zero-copy string borrowing.
- Test coverage is thorough: version increment, number extraction, artifact kind metadata, Display/serde round-trips.
- No infrastructure dependencies; purely domain layer value objects.
