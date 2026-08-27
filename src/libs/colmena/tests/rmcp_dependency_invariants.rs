//! Guards on what the `rmcp` dependency actually resolves to.
//!
//! Both guards read `Cargo.lock`, never `Cargo.toml`. That is deliberate.
//! What we care about is not how the dependency is *written* but what ends up
//! *compiled in*, and those differ: a feature can arrive through Cargo feature
//! unification with another workspace member, a workspace-level override, a
//! `[target.'cfg(…)']` block, or a transitive dependency enabling it on our
//! behalf. The lockfile is the resolved truth and is machine-generated, so it
//! is both more complete and safer to parse than a hand-edited manifest.
//!
//! An earlier version of these guards string-matched `Cargo.toml` instead. It
//! was reproduced giving a FALSE GREEN: wrapping the `features` array across
//! several lines — a routine reformat — moved the added feature outside what
//! the check inspected, so the tests passed while `process-wrap` genuinely
//! entered the lockfile and the stdio transport was really compiled in. A
//! guard that reports success while the invariant is broken is worse than no
//! guard, because it will be believed.
//!
//! `src/libs/colmena/Cargo.toml` states both constraints in a comment beside
//! the dependency, so a reader of the manifest sees them without needing a
//! test to tell them.

use std::path::PathBuf;

/// Locate the workspace `Cargo.lock` by walking up from this crate's manifest
/// directory, rather than assuming a fixed nesting depth — the crate can move
/// within the workspace without silently breaking these guards.
fn workspace_lockfile() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("Cargo.lock");
        if candidate.is_file() {
            return candidate;
        }
        if !dir.pop() {
            panic!(
                "no Cargo.lock found in any ancestor of {} — these guards assert on resolved \
                 dependencies and cannot run without one",
                env!("CARGO_MANIFEST_DIR")
            );
        }
    }
}

/// The crate names listed under `rmcp`'s own `dependencies` in `Cargo.lock`,
/// with any version suffix stripped (cargo writes `"http 1.4.0"` when several
/// versions of a crate coexist, and plain `"bytes"` when only one does).
///
/// Scoped to rmcp's own list on purpose: `schemars`, `uuid` and `tower` all
/// legitimately appear elsewhere in this workspace, so asserting their global
/// absence would be wrong. What matters is whether *rmcp* pulls them.
fn rmcp_resolved_dependencies() -> Vec<String> {
    let lock_path = workspace_lockfile();
    let lock = std::fs::read_to_string(&lock_path).unwrap_or_else(|e| {
        panic!(
            "Cargo.lock must be readable at {}: {e}",
            lock_path.display()
        )
    });

    let pkg_start = lock
        .find("\nname = \"rmcp\"\n")
        .expect("Cargo.lock must contain a package entry for `rmcp`");
    let block = &lock[pkg_start..];

    let deps_start = match block.find("\ndependencies = [") {
        Some(i) => i,
        // A package with no dependencies has no `dependencies` key at all.
        None => return Vec::new(),
    };
    // Guard against reading into a later package: the key must belong to rmcp.
    if let Some(next_pkg) = block.find("\n[[package]]") {
        assert!(
            deps_start < next_pkg,
            "the `dependencies` list found after `name = \"rmcp\"` belongs to a later package; \
             the Cargo.lock layout changed and these guards need updating"
        );
    }
    let list = &block[deps_start..];
    let end = list
        .find("\n]")
        .expect("rmcp's `dependencies` list must be closed");

    list[..end]
        .lines()
        .filter_map(|l| l.trim().strip_prefix('"')?.split('"').next())
        .map(|entry| entry.split_whitespace().next().unwrap_or(entry).to_string())
        .collect()
}

/// `transport-child-process` is rmcp's stdio transport: it spawns an arbitrary
/// local binary and talks to it over pipes. Remote-only is a deliberate product
/// decision — a worker that can spawn processes named by graph configuration is
/// a different security posture than one that cannot.
///
/// `process-wrap` is the crate that transport pulls in, so its presence in
/// rmcp's resolved dependencies means the transport is compiled in, however it
/// came to be enabled.
#[test]
fn rmcp_does_not_resolve_the_child_process_transport() {
    let deps = rmcp_resolved_dependencies();
    assert!(
        !deps.iter().any(|d| d == "process-wrap"),
        "rmcp resolves `process-wrap`, so its transport-child-process (the stdio transport that \
         spawns local binaries) is compiled in. This project is remote-only. Resolved \
         dependencies were: {deps:?}"
    );
}

/// rmcp's default features include `server`, which drags in the whole
/// server-side stack. Colmena is a pure MCP *client*, so taking the defaults
/// would cost binary size and dependency surface for nothing.
///
/// Asserted against rmcp's resolved dependencies rather than the manifest's
/// `default-features = false`, so it also catches the feature arriving through
/// Cargo feature unification from another workspace member.
#[test]
fn rmcp_does_not_resolve_the_server_stack() {
    let deps = rmcp_resolved_dependencies();
    let server_only = ["schemars", "uuid", "tower"];
    let found: Vec<_> = deps
        .iter()
        .filter(|d| server_only.contains(&d.as_str()))
        .collect();
    assert!(
        found.is_empty(),
        "rmcp resolves server-only crates {found:?}, so its `server` feature is enabled. Colmena \
         is a pure MCP client; check `default-features` and any workspace-level feature \
         unification. Resolved dependencies were: {deps:?}"
    );
}
