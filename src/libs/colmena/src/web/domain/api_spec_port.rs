//! `ApiSpecPort` — fetch-and-parse contract for OpenAPI 3.x / Swagger 2.0 specs.
//!
//! The port returns a format-normalized `ParsedSpec` domain value (the
//! adapter converts Swagger 2.0 internally). Domain code never sees raw
//! `oas3` or `serde_yaml` types.

use crate::web::domain::errors::WebDomainError;
use async_trait::async_trait;
use std::collections::HashMap;

#[async_trait]
pub trait ApiSpecPort: Send + Sync {
    /// Fetch the spec at `url`, revalidating against the optional cached
    /// ETag (or Last-Modified, if the adapter stored one) using a
    /// conditional GET. Returns `NotModified` when the server answered 304.
    async fn fetch_and_parse(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<SpecFetchResult, WebDomainError>;
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum SpecFetchResult {
    Fresh {
        spec: ParsedSpec,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    NotModified,
}

#[derive(Debug, Clone)]
pub struct ParsedSpec {
    /// URL actually fetched (post-normalization).
    pub resolved_url: String,
    /// URL as given by the agent (pre-normalization) — useful for error reporting.
    pub input_url: String,
    pub original_format: SpecFormat,
    pub internal_format: String, // always "openapi-3.x.y"
    pub title: String,
    pub version: String,
    pub description: Option<String>,
    pub servers: Vec<String>,
    pub endpoints: Vec<Endpoint>,
    pub security_schemes: HashMap<String, SecurityScheme>,
    pub tags: Vec<String>,
    /// Verbatim copy of `components.schemas` from the spec (or the
    /// equivalent `definitions` block for Swagger 2.0). Used by
    /// `get_endpoint_details` to inline `$ref` references — Gemini's
    /// strict tool-response validator rejects strings that look like
    /// `#/components/schemas/X`, so we resolve them before they reach
    /// the model.
    pub components_schemas: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecFormat {
    OpenApi3x,
    Swagger20,
}

impl SpecFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenApi3x => "openapi-3.x",
            Self::Swagger20 => "swagger-2.0",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Copy)]
pub enum HttpMethod {
    Get,
    Put,
    Post,
    Delete,
    Options,
    Head,
    Patch,
    Trace,
}

impl HttpMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Post => "POST",
            Self::Delete => "DELETE",
            Self::Options => "OPTIONS",
            Self::Head => "HEAD",
            Self::Patch => "PATCH",
            Self::Trace => "TRACE",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "PUT" => Some(Self::Put),
            "POST" => Some(Self::Post),
            "DELETE" => Some(Self::Delete),
            "OPTIONS" => Some(Self::Options),
            "HEAD" => Some(Self::Head),
            "PATCH" => Some(Self::Patch),
            "TRACE" => Some(Self::Trace),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub operation_id: String,
    pub method: HttpMethod,
    pub path: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub path_params: Vec<ParameterSpec>,
    pub query_params: Vec<ParameterSpec>,
    pub header_params: Vec<ParameterSpec>,
    pub request_body: Option<RequestBodySpec>,
    pub responses: HashMap<String, ResponseSpec>,
    pub security: Vec<SecurityRequirement>,
}

#[derive(Debug, Clone)]
pub struct ParameterSpec {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
    pub param_type: ParamType,
    /// e.g. `form`, `spaceDelimited`, `pipeDelimited`. None for non-array params.
    pub style: Option<String>,
    pub explode: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    Number,
    Boolean,
    Array(Box<ParamType>),
    /// Shape may be opaque (`$ref` not inlined). We accept it and leave
    /// validation of structure to the user / server.
    Object,
    /// Unknown / unrecognized — treated as opaque string for HTTP purposes.
    Unknown,
}

#[derive(Debug, Clone)]
pub struct RequestBodySpec {
    /// Media type to use when encoding the body.
    pub content_type: String,
    pub required: bool,
    /// Raw JSON-Schema-ish object describing the body; preserved verbatim
    /// and exposed to the LLM so it can see `properties` / `required`.
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ResponseSpec {
    pub description: Option<String>,
    /// Map content_type → schema (verbatim JSON). Empty if no schema declared.
    pub content: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyLocation {
    Header,
    Query,
    Cookie,
}

#[derive(Debug, Clone)]
pub enum SecurityScheme {
    /// `type: http` — `scheme: bearer | basic | ...`
    Http {
        scheme: String,
        bearer_format: Option<String>,
    },
    ApiKey {
        name: String,
        location: ApiKeyLocation,
    },
    /// OAuth2 — Colmena expects a pre-obtained token supplied via a
    /// Secure Value reference; the scheme metadata is preserved for the
    /// LLM's benefit.
    OAuth2 {
        /// Flow name → metadata (auth/token URLs + scopes). Verbatim from spec.
        flows: serde_json::Value,
    },
    /// OpenID Connect — same contract as OAuth2 from the build-request POV.
    OpenIdConnect {
        openid_connect_url: String,
    },
}

#[derive(Debug, Clone)]
pub struct SecurityRequirement {
    pub scheme: String,
    pub scopes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn http_method_roundtrips_all_verbs() {
        for m in [
            HttpMethod::Get,
            HttpMethod::Put,
            HttpMethod::Post,
            HttpMethod::Delete,
            HttpMethod::Options,
            HttpMethod::Head,
            HttpMethod::Patch,
            HttpMethod::Trace,
        ] {
            assert_eq!(HttpMethod::parse(m.as_str()), Some(m));
        }
    }

    #[test]
    fn http_method_parse_is_case_insensitive() {
        assert_eq!(HttpMethod::parse("get"), Some(HttpMethod::Get));
        assert_eq!(HttpMethod::parse("Post"), Some(HttpMethod::Post));
    }

    #[test]
    fn http_method_parse_rejects_garbage() {
        assert!(HttpMethod::parse("FOO").is_none());
    }

    #[test]
    fn spec_format_string_is_stable() {
        assert_eq!(SpecFormat::OpenApi3x.as_str(), "openapi-3.x");
        assert_eq!(SpecFormat::Swagger20.as_str(), "swagger-2.0");
    }

    #[test]
    fn parsed_spec_clone_is_deep() {
        let p = ParsedSpec {
            resolved_url: "a".into(),
            input_url: "b".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "T".into(),
            version: "1".into(),
            description: None,
            servers: vec!["https://x".into()],
            endpoints: Vec::new(),
            security_schemes: HashMap::new(),
            tags: Vec::new(),
            components_schemas: std::collections::HashMap::new(),
        };
        let c = p.clone();
        assert_eq!(c.title, "T");
        assert_eq!(c.servers, vec!["https://x".to_string()]);
    }
}
