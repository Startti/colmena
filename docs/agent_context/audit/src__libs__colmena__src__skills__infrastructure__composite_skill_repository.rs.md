# src/libs/colmena/src/skills/infrastructure/composite_skill_repository.rs

**Layer:** infrastructure  
**Purpose:** Implements a composite repository that merges built-in and filesystem skill repositories, detects name collisions at construction, and routes queries to the appropriate underlying repository.

## Symbols

- `MAX_ACTIVE_SKILLS` (const, pub) — Upper bound (50) on total number of active skills per node
- `CompositeSkillRepository` (struct, pub) — Composite repository holding references to builtin and filesystem repositories with cached name sets for collision detection and dispatch
- `impl Debug for CompositeSkillRepository` (impl) — Custom Debug formatter that omits non-Debug trait objects via finish_non_exhaustive()
- `CompositeSkillRepository::new()` (fn, pub) — Constructor that extracts available names from both repositories, detects collisions, enforces MAX_ACTIVE_SKILLS limit, returns Result
- `SkillRepository::list_available()` (fn) — Combines available skill lists from both repositories
- `SkillRepository::load_skill()` (async fn) — Dispatches skill load request to builtin or filesystem repository based on cached name sets, returns SkillNotFound if name not in either set
- `SkillRepository::load_reference()` (async fn) — Dispatches reference load request to appropriate repository using same dispatch logic as load_skill
- `CompositeSkillRepository::source_of()` (fn, pub) — Returns SkillSource (Builtin or Path) for observability/debugging purposes, None if skill not found
- `FakeRepo` (struct, private, test) — Test fixture implementing SkillRepository for unit tests with fake catalog entries
- `entry()` (fn, private, test) — Helper creating test SkillCatalogEntry with name, description, and source
- `merges_disjoint_names()` (test) — Verifies composite list_available merges non-overlapping skill sets
- `detects_collision_between_builtin_and_path()` (test) — Verifies SkillNameCollision error when same name exists in both repositories
- `rejects_when_total_exceeds_50()` (test) — Verifies TooManySkills error when combined count exceeds MAX_ACTIVE_SKILLS
- `dispatches_load_to_correct_repo()` (test) — Verifies load_skill routes to correct repository and preserves source
- `load_unknown_skill_returns_not_found()` (test) — Verifies SkillNotFound error for names not in either repository
- `source_of_returns_correct_origin()` (test) — Verifies source_of correctly identifies builtin vs path skills and returns None for missing

## File-level notes

- Clean composite pattern implementation with O(1) dispatch via HashSet membership checks
- Collision detection happens at construction time, preventing runtime surprises
- Custom Debug impl necessary because Arc<dyn SkillRepository> doesn't implement Debug by default
- Dispatch logic in `load_skill()` and `load_reference()` follows identical if-else-if-else pattern; duplication is minimal and inherent to trait method boundaries
- Test coverage comprehensive: 6 test cases covering construction, merging, dispatch, limits, and error paths with good edge case coverage
