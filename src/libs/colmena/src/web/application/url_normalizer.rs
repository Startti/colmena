//! Normalize Git-forge "blob" URLs to their raw-content equivalents.
//!
//! LLMs paste whatever URL the user gave them. For the top three public
//! forges we can rewrite the rendered-HTML URL to a URL that actually
//! serves the raw file. Unknown hosts and URLs that already point to raw
//! content pass through unchanged.

/// Result of normalizing a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedUrl {
    /// The URL that the adapter should actually GET.
    pub resolved: String,
    /// `true` if the input looked like a recognized forge and was rewritten.
    pub rewritten: bool,
}

/// Normalize well-known Git-forge "blob" URLs.
///
/// Rules:
/// - `github.com/{owner}/{repo}/blob/{ref}/{path}` → `raw.githubusercontent.com/{owner}/{repo}/{ref}/{path}`
/// - `github.com/{owner}/{repo}/tree/{ref}/{path}` → same as above (rarer; a tree URL that points at a file).
/// - `gitlab.com/{owner}/{repo}/-/blob/{ref}/{path}` → `gitlab.com/{owner}/{repo}/-/raw/{ref}/{path}`
/// - `bitbucket.org/{owner}/{repo}/src/{ref}/{path}` → `bitbucket.org/{owner}/{repo}/raw/{ref}/{path}`
///
/// Query strings and fragments are preserved.
pub fn normalize_forge_url(input: &str) -> NormalizedUrl {
    // GitHub blob / tree
    if let Some(rest) = input.strip_prefix("https://github.com/") {
        if let Some(rewritten) = rewrite_github(rest) {
            return NormalizedUrl { resolved: rewritten, rewritten: true };
        }
    }
    // GitLab -/blob
    if let Some(rest) = input.strip_prefix("https://gitlab.com/") {
        if let Some(rewritten) = rewrite_gitlab(rest) {
            return NormalizedUrl { resolved: rewritten, rewritten: true };
        }
    }
    // Bitbucket src
    if let Some(rest) = input.strip_prefix("https://bitbucket.org/") {
        if let Some(rewritten) = rewrite_bitbucket(rest) {
            return NormalizedUrl { resolved: rewritten, rewritten: true };
        }
    }
    NormalizedUrl { resolved: input.to_string(), rewritten: false }
}

fn rewrite_github(rest: &str) -> Option<String> {
    // {owner}/{repo}/{blob|tree}/{ref}/{path...}
    let (owner, rest) = rest.split_once('/')?;
    let (repo, rest) = rest.split_once('/')?;
    let (kind, rest) = rest.split_once('/')?;
    if kind != "blob" && kind != "tree" {
        return None;
    }
    let (git_ref, path) = rest.split_once('/')?;
    Some(format!(
        "https://raw.githubusercontent.com/{owner}/{repo}/{git_ref}/{path}"
    ))
}

fn rewrite_gitlab(rest: &str) -> Option<String> {
    // {owner}/{repo}/-/blob/{ref}/{path...}
    let (owner, rest) = rest.split_once('/')?;
    let (repo, rest) = rest.split_once('/')?;
    let rest = rest.strip_prefix("-/")?;
    let rest = rest.strip_prefix("blob/")?;
    let (git_ref, path) = rest.split_once('/')?;
    Some(format!(
        "https://gitlab.com/{owner}/{repo}/-/raw/{git_ref}/{path}"
    ))
}

fn rewrite_bitbucket(rest: &str) -> Option<String> {
    // {owner}/{repo}/src/{ref}/{path...}
    let (owner, rest) = rest.split_once('/')?;
    let (repo, rest) = rest.split_once('/')?;
    let rest = rest.strip_prefix("src/")?;
    let (git_ref, path) = rest.split_once('/')?;
    Some(format!(
        "https://bitbucket.org/{owner}/{repo}/raw/{git_ref}/{path}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_blob_url_is_rewritten_to_raw() {
        let n = normalize_forge_url(
            "https://github.com/OAI/OpenAPI-Specification/blob/main/examples/v3.0/petstore.yaml",
        );
        assert!(n.rewritten);
        assert_eq!(
            n.resolved,
            "https://raw.githubusercontent.com/OAI/OpenAPI-Specification/main/examples/v3.0/petstore.yaml"
        );
    }

    #[test]
    fn github_tree_url_is_rewritten_to_raw() {
        let n = normalize_forge_url(
            "https://github.com/amadeus4dev/amadeus-code-examples/tree/master/self-service/reference-data/airlines/get_airlines.yaml",
        );
        assert!(n.rewritten);
        assert!(n.resolved.starts_with("https://raw.githubusercontent.com/"));
    }

    #[test]
    fn gitlab_blob_url_is_rewritten_to_raw() {
        let n = normalize_forge_url(
            "https://gitlab.com/some/repo/-/blob/main/spec/openapi.yaml",
        );
        assert!(n.rewritten);
        assert_eq!(
            n.resolved,
            "https://gitlab.com/some/repo/-/raw/main/spec/openapi.yaml"
        );
    }

    #[test]
    fn bitbucket_src_url_is_rewritten_to_raw() {
        let n = normalize_forge_url(
            "https://bitbucket.org/team/repo/src/main/openapi.yaml",
        );
        assert!(n.rewritten);
        assert_eq!(
            n.resolved,
            "https://bitbucket.org/team/repo/raw/main/openapi.yaml"
        );
    }

    #[test]
    fn raw_url_passes_through() {
        let url = "https://raw.githubusercontent.com/foo/bar/main/openapi.yaml";
        let n = normalize_forge_url(url);
        assert!(!n.rewritten);
        assert_eq!(n.resolved, url);
    }

    #[test]
    fn non_forge_url_passes_through() {
        let url = "https://petstore3.swagger.io/api/v3/openapi.json";
        let n = normalize_forge_url(url);
        assert!(!n.rewritten);
        assert_eq!(n.resolved, url);
    }

    #[test]
    fn private_gitlab_host_passes_through() {
        // Only public gitlab.com is rewritten; self-hosted instances must be raw already.
        let url = "https://git.internal.example.com/foo/bar/-/blob/main/openapi.yaml";
        let n = normalize_forge_url(url);
        assert!(!n.rewritten);
        assert_eq!(n.resolved, url);
    }

    #[test]
    fn github_non_blob_url_passes_through() {
        // e.g. a releases page — we don't try to rewrite.
        let url = "https://github.com/OAI/OpenAPI-Specification/releases/tag/3.0.3";
        let n = normalize_forge_url(url);
        assert!(!n.rewritten);
        assert_eq!(n.resolved, url);
    }

    #[test]
    fn github_url_with_query_is_preserved_verbatim() {
        // Query strings are not stripped — URLs we rewrite include the full tail.
        let n = normalize_forge_url(
            "https://github.com/foo/bar/blob/main/openapi.yaml?raw=1",
        );
        assert!(n.rewritten);
        assert_eq!(
            n.resolved,
            "https://raw.githubusercontent.com/foo/bar/main/openapi.yaml?raw=1"
        );
    }

    #[test]
    fn github_url_without_file_path_passes_through() {
        // Missing a path after the ref — can't rewrite.
        let n = normalize_forge_url("https://github.com/foo/bar/blob/main");
        assert!(!n.rewritten);
    }
}
