# src/libs/colmena/src/web/application/url_normalizer.rs

**Layer:** application  **Purpose:** Converts Git-forge "blob" URLs (GitHub, GitLab, Bitbucket) from their rendered-HTML form to raw-content URLs that can be directly downloaded. Unknown hosts and already-raw URLs pass through unchanged.

## Symbols

- `NormalizedUrl` (struct, pub) — Result wrapper with `resolved: String` (the normalized URL) and `rewritten: bool` (whether the input was a recognized forge)
- `normalize_forge_url(input: &str) -> NormalizedUrl` (fn, pub) — Main entry point; routes to forge-specific rewriters or returns input unchanged
- `rewrite_github(rest: &str) -> Option<String>` (fn, private) — Rewrites `github.com/{owner}/{repo}/{blob|tree}/{ref}/{path}` to `raw.githubusercontent.com/{owner}/{repo}/{ref}/{path}`
- `rewrite_gitlab(rest: &str) -> Option<String>` (fn, private) — Rewrites `gitlab.com/{owner}/{repo}/-/blob/{ref}/{path}` to `gitlab.com/{owner}/{repo}/-/raw/{ref}/{path}`
- `rewrite_bitbucket(rest: &str) -> Option<String>` (fn, private) — Rewrites `bitbucket.org/{owner}/{repo}/src/{ref}/{path}` to `bitbucket.org/{owner}/{repo}/raw/{ref}/{path}`
- `tests` (mod, private) — Test module with 9 test cases covering GitHub blob/tree URLs, GitLab URLs, Bitbucket URLs, raw URLs, non-forge URLs, private hosts, non-blob URLs, query string preservation, and malformed URLs

## File-level notes

- **Coverage**: Nine test cases exercise the happy path (rewrite for each forge), raw URLs passing through, non-forge URLs, private hosts, and edge cases (missing path segments, query strings).
- **Query string handling**: Doc comment claims query strings are "preserved" — they are, but implicitly as part of the trailing `path` variable in each rewriter. No explicit query-string-aware logic; the design works because `split_once('/')` captures everything after the git reference in the third parameter.
- **No dead code**: All private rewriters are used in the main function; all test functions are valid.
- **No error handling boundary issues**: Rewriters return `Option`, callers check `if let Some(...)`. Falls through to pass-through on `None`.
