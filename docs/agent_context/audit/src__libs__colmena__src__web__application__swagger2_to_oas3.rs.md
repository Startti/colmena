# src/libs/colmena/src/web/application/swagger2_to_oas3.rs

**Layer:** application  **Purpose:** Converts Swagger 2.0 JSON/YAML specifications to OpenAPI 3.0.3 format, handling structural and naming differences between the two standards and raising errors on unsupported features.

## Symbols

- `convert_swagger2_to_openapi3` (fn, pub) — Main entry point; converts Swagger 2.0 Value root to OpenAPI 3.0.3 Value, extracting and transforming info, servers, paths, components, security, and tags
- `build_servers` (fn, private) — Constructs OpenAPI servers array from Swagger host, basePath, and schemes; defaults to https if schemes absent; returns empty if host is empty
- `convert_security_definitions` (fn, private) — Transforms Swagger securityDefinitions to OpenAPI securitySchemes, mapping type variants (basic→http+scheme, apiKey→unchanged, oauth2→flows structure with flow-name remapping)
- `rewrite_refs_recursive` (fn, pub) — Recursively walks JSON tree and rewrites all `$ref` strings from Swagger 2.0 paths to OpenAPI 3.0 paths
- `rewrite_single_ref` (fn, private) — Rewrites a single `$ref` string by pattern matching on 2.0 prefixes (#/definitions/ → #/components/schemas/, etc.)
- `convert_operations` (fn, private) — Iterates all path items and their HTTP methods, delegating each operation to convert_single_operation
- `is_http_method` (fn, private) — Predicate checking if a string is a valid HTTP method name (get, post, put, delete, patch, options, head, trace)
- `convert_single_operation` (fn, private) — Converts a single operation: extracts global consumes/produces, splits parameters (body, formData, other), converts body to requestBody, converts formData to multipart/urlencoded requestBody, wraps response schemas in content blocks
- `pick_first_content_type` (fn, private) — Returns first string from a Value array with a given default fallback
- `pick_first_consume_for_form` (fn, private) — Scans consumes list for form-urlencoded or multipart; returns None if neither found
- `convert_param_collection_format` (fn, private) — Maps Swagger collectionFormat to OpenAPI style+explode (csv→form/explode=false, multi→form/explode=true, ssv→spaceDelimited, pipes→pipeDelimited); errors on tsv and unknown formats
- `tests_root` (mod, private) — 10 integration tests covering minimal conversion, server building, component migration, security schemes, global parameters, tags
- `tests_operations` (mod, private) — 15 integration tests covering body→requestBody, response schema wrapping, formData→urlencoded, formData with files→multipart, collectionFormat translation, operation-level overrides

## File-level notes

- No dead code; all functions are either pub or called within the conversion pipeline
- No unfinished work; no todo!(), unimplemented!(), unreachable!(), or FIXME comments
- Error handling is explicit and context-rich: unsupported features (collectionFormat: tsv, unknown oauth2 flow, invalid security scheme type) return WebDomainError with the unsupported feature name
- Comprehensive test coverage (25 tests) validates both happy-path transformations and error cases
- Lossy conversion by design: the module rejects features with no 3.0 equivalent rather than silently degrading (per module docstring)
- All public functions documented with `///` comments
- Consistent use of serde_json Value as the transport format (input likely came from YAML→JSON pre-conversion)
