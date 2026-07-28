# src/libs/colmena/src/crdt_documents/doc_registry.rs

**Layer:** infrastructure  **Purpose:** In-memory registry of live Yrs documents keyed by ArtifactId; manages artifact lifecycle (load, create, snapshot writer coordination, shutdown) and persistence via abstract ArtifactStorage port.

## Symbols

- `RegisteredArtifact` (struct, pub) — Wrapper holding Arc<Doc>, dirty flag, notify channel, snapshot handle, metadata, recalc observer subscription, and meta-save task handle; keeps artifacts alive and tracks mutations.
- `RegisteredArtifact::mark_dirty` (fn, pub) — Marks artifact dirty via atomic flag and notifies snapshot writer task.
- `DocRegistry` (struct, pub) — Main registry mapping ArtifactId → RegisteredArtifact; coordinates artifact lifecycle and storage.
- `DocRegistry::new` (fn, pub) — Constructor taking ArtifactStorage; initializes empty registry.
- `DocRegistry::load_from_disk` (fn, pub async) — Reloads all known artifacts from storage at startup; decodes Yrs updates, attaches observers, spawns snapshot writers; returns count of loaded artifacts.
- `DocRegistry::get_or_create` (fn, pub) — Gets existing artifact or creates new one; spawns detached meta.json save task captured in meta_save for shutdown coordination.
- `DocRegistry::get` (fn, pub) — Retrieves artifact by ID; returns Option.
- `DocRegistry::list` (fn, pub) — Returns Vec of all artifact metadata.
- `DocRegistry::shutdown_all` (fn, pub async) — Drains all snapshot writers and awaits final flush; drains meta_save tasks first to guarantee disk persistence before runtime drop.
- `DocRegistry::delete` (fn, pub async) — Removes artifact from registry, drains its meta_save and snapshot tasks, invokes storage deletion.
- `DocRegistry::len` (fn, pub) — Returns artifact count.
- `DocRegistry::is_empty` (fn, pub) — Returns true if no artifacts registered.
- `temp_storage` (fn, private) — Test helper; creates temporary storage directory with random ulid suffix.
- `get_or_create_returns_same_arc_on_repeat` (test) — Verifies get_or_create with same ID returns identical Arc pointer.
- `different_ids_get_different_entries` (test) — Verifies different IDs produce different registry entries.

## File-level notes

- **Observer attachment duplication** — The recalc observer attachment pattern (attach, log warning on error, store Some/None) appears in both `load_from_disk` (lines 78–88) and `get_or_create` (lines 116–126). Could be extracted to a private helper function to reduce duplication and centralize error handling.
- **meta_save detached task coordination** — Uses tokio::spawn to fire off a background meta.json write, then captures its JoinHandle in RegisteredArtifact::meta_save; shutdown_all and delete drain it before registry teardown. This is intentional (documented in comments) to avoid race where phase-2 load_from_disk sees missing meta.json on slow filesystems (ext4 CI issue). Pattern is sound but moderately complex; comments explain the motivation well.
- **No error handling in get_or_create observer attach** — Errors attaching the recalc observer are logged but don't abort artifact creation; Option allows graceful degradation. This is documented and intentional.
- **Snapshot writer lifecycle invariant** — The snapshot writer (spawned via spawn_writer) is always wrapped in a Mutex and drained before registry drop; the registry enforces this invariant via shutdown_all. No risk of detached writer panicking after runtime drop.
