# HubSpot CRM Agent — Design

**Date:** 2026-05-08
**Owner:** Daniel Garcia
**Status:** Approved (brainstorming → ready for implementation plan)

## Goal

Build a conversational agent graph that creates and updates HubSpot CRM contacts and companies, and associates them, using a single `llm_call` node with `tool_configurations` (no orchestrator). Knowledge about each HubSpot endpoint lives in an on-demand colmena skill (`hubspot-crm`) loaded by the LLM via `load_skill`, mirroring the `adp-node-catalog` skill pattern in ADP.

## Non-goals

- OAuth refresh-token flow (Private App Access Token is sufficient — single tenant, no auth-fetch node).
- Orchestrator / planner / critic loops.
- Webhook trigger for production exposure (CLI conversational pattern only — `input` node + re-invoke with `--agent-session-id`).
- Custom HubSpot objects, deals, tickets, engagements, batch endpoints. Only contacts, companies, and contact↔company associations.

## Auth

- Single env var `HUBSPOT_PRIVATE_APP_TOKEN` in repo `.env`.
- Each `http_request` tool sets `bearer_token: "${HUBSPOT_PRIVATE_APP_TOKEN}"`. The LLM never sees or receives the token.
- Required Private App scopes: `crm.objects.contacts.read`, `crm.objects.contacts.write`, `crm.objects.companies.read`, `crm.objects.companies.write`, `crm.schemas.contacts.read`, `crm.schemas.companies.read`.

## Tools (7)

All tools use `node_type: "http_request"` configured via `node_schema`. The shared base — `base_url`, `method`, `bearer_token`, `headers` — is `fixed` (the LLM never sees it). What varies per tool is `endpoint` (literal vs. LLM-controlled string) and `body` (which sub-fields the LLM fills).

### Verified engine constraint (drives the body modeling)

`node_schema` containers (`body`, `query_params`, `headers`) support **two levels of nesting**, but at depth 2 only `fixed` sub-fields are merged into the base; LLM-visible grandchildren are **not** exposed as individual tool parameters — the LLM provides the whole child object as a single param. (See `parse_node_schema` in `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs`.)

HubSpot wraps every create/update body in `{ "properties": { … } }`. We model `body` as a container with one child `properties` declared as a **leaf object** the LLM fills. Field-by-field guidance (which properties exist, which are required, formats) lives in the corresponding skill reference, not in the tool schema. This honors both the engine's depth limit and the design split (tool = thin pointer, skill = full schema).

### Shared `node_schema` base (every tool)

```jsonc
"node_schema": {
  "base_url":     { "type": "string", "fixed": "https://api.hubapi.com" },
  "method":       { "type": "string", "fixed": "<POST|PATCH|PUT>" },        // per tool
  "bearer_token": { "type": "string", "fixed": "${HUBSPOT_PRIVATE_APP_TOKEN}" },
  "headers":      { "type": "object", "fixed": { "Content-Type": "application/json" } }
  // endpoint and body — defined per tool below
}
```

### 1. `create_contact` — POST `/crm/v3/objects/contacts`

```jsonc
"endpoint": { "type": "string", "fixed": "/crm/v3/objects/contacts" },
"body": {
  "type": "object",
  "properties": {
    "properties": {
      "type": "object", "required": true,
      "description": "Contact fields. Required: email. Common: firstname, lastname, phone, company, lifecyclestage. See hubspot-crm/create_contact for the full property list."
    }
  }
}
```

**Tool description:** *"Create a HubSpot contact. Provide a `properties` object with at least `email`. See `hubspot-crm/create_contact` for valid property names and dedup behavior."*

**Response shape (HTTP 201):** `{ id: string, properties: {...echo of fields with HubSpot defaults filled in...}, createdAt, updatedAt, archived: false }`. Engine wraps as `{ status: 201, body: {…above…} }`. Downstream interest: `body.id`.

**Failure modes:** `409 CONFLICT` with `category: "CONFLICT"` when email already exists; the reference instructs the LLM to fall back to `search_contacts` then `update_contact`.

### 2. `update_contact` — PATCH `/crm/v3/objects/contacts/{contactId}`

```jsonc
"endpoint": {
  "type": "string", "required": true,
  "description": "Full path of the form /crm/v3/objects/contacts/{contactId} where {contactId} is the real id from a prior search_contacts or create_contact response. Never invent an id."
},
"body": {
  "type": "object",
  "properties": {
    "properties": {
      "type": "object", "required": true,
      "description": "Properties to update (partial). Same names as create_contact. See hubspot-crm/update_contact."
    }
  }
}
```

**Tool description:** *"Update a HubSpot contact's properties. Requires the contact id in the endpoint path. See `hubspot-crm/update_contact`."*

**Response shape (HTTP 200):** same shape as create_contact (full updated record).

**Failure modes:** `404 OBJECT_NOT_FOUND` for stale/wrong id; `400 VALIDATION_ERROR` for unknown property names — the reference instructs the LLM to verify property names via the table.

### 3. `search_contacts` — POST `/crm/v3/objects/contacts/search`

```jsonc
"endpoint": { "type": "string", "fixed": "/crm/v3/objects/contacts/search" },
"body": {
  "type": "object",
  "properties": {
    "filterGroups": {
      "type": "array", "required": true,
      "description": "Array of filter groups (OR'd together). Each group is { filters: [ { propertyName, operator, value } ] } (filters within a group are AND'd). Operators: EQ, NEQ, CONTAINS_TOKEN, HAS_PROPERTY, NOT_HAS_PROPERTY, GT, LT, BETWEEN, IN. See hubspot-crm/search_contacts."
    },
    "properties": {
      "type": "array",
      "description": "Property names to return on each result, e.g. ['email','firstname','lastname','phone']. Default minimal set if omitted."
    },
    "limit": {
      "type": "number",
      "description": "Max results (default 10, max 100)."
    }
  }
}
```

**Tool description:** *"Search HubSpot contacts using filterGroups. Use this before update or associate when you only have an email/name. See `hubspot-crm/search_contacts` for operators."*

**Response shape (HTTP 200):** `{ total: number, results: [ { id, properties, createdAt, updatedAt, archived } ], paging?: { next: { after } } }`. Downstream interest: `body.results[].id`, `body.total`.

### 4. `create_company` — POST `/crm/v3/objects/companies`

```jsonc
"endpoint": { "type": "string", "fixed": "/crm/v3/objects/companies" },
"body": {
  "type": "object",
  "properties": {
    "properties": {
      "type": "object", "required": true,
      "description": "Company fields. Required: name. Common: domain, industry, phone, city, country, numberofemployees. See hubspot-crm/create_company."
    }
  }
}
```

**Tool description:** *"Create a HubSpot company. Provide a `properties` object with at least `name`. See `hubspot-crm/create_company` for valid property names and domain-based dedup."*

**Response shape (HTTP 201):** same shape as create_contact (id + properties echo).

**Failure modes:** HubSpot does not auto-dedup companies by domain unless the portal has the setting enabled — the reference instructs the LLM to call `search_companies` by domain first when there's risk of duplicate.

### 5. `update_company` — PATCH `/crm/v3/objects/companies/{companyId}`

Same shape as `update_contact` with `companyId` in the endpoint path. Tool description and response shape mirror update_contact.

### 6. `search_companies` — POST `/crm/v3/objects/companies/search`

Same shape as `search_contacts`. Common filters per the reference: `domain EQ "acme.test"`, `name CONTAINS_TOKEN "Acme"`. Response identical to search_contacts.

### 7. `associate_contact_company` — PUT `/crm/v4/objects/contact/{contactId}/associations/default/company/{companyId}`

```jsonc
"endpoint": {
  "type": "string", "required": true,
  "description": "Full path of the form /crm/v4/objects/contact/{contactId}/associations/default/company/{companyId}. Both ids must come from prior search/create — never invent. The 'default' segment means HubSpot's primary association type; body is empty."
},
"body": { "type": "object", "fixed": {} }
```

**Tool description:** *"Associate a HubSpot contact with a HubSpot company using the default (primary) association type. Both ids required. See `hubspot-crm/associate_contact_company`."*

**Response shape (HTTP 200):** `{ fromObjectTypeId, fromObjectId, toObjectTypeId, toObjectId, labels: [] }`.

**Failure modes:** `404 OBJECT_NOT_FOUND` if either id is wrong; `400` if the path is malformed. Out of scope: labeled associations (require body with type id) — referenced as out-of-scope in the skill.

### Summary — what the LLM sees per tool

| Tool | LLM-visible parameters | Hidden (fixed) |
|---|---|---|
| create_contact | `properties` (object, req) | base_url, method, bearer_token, headers, endpoint, body wrapper |
| update_contact | `endpoint` (string, req), `properties` (object, req) | base_url, method, bearer_token, headers, body wrapper |
| search_contacts | `filterGroups` (array, req), `properties` (array), `limit` (number) | base_url, method, bearer_token, headers, endpoint, body wrapper |
| create_company | `properties` (object, req) | base_url, method, bearer_token, headers, endpoint, body wrapper |
| update_company | `endpoint` (string, req), `properties` (object, req) | base_url, method, bearer_token, headers, body wrapper |
| search_companies | `filterGroups` (array, req), `properties` (array), `limit` (number) | base_url, method, bearer_token, headers, endpoint, body wrapper |
| associate_contact_company | `endpoint` (string, req) | base_url, method, bearer_token, headers, body (fixed {}) |

## Skill: `hubspot-crm`

### Layout

```
tests/graphs/advanced/hubspot/
  agent.json
  skills/
    hubspot-crm/
      SKILL.md
      references/
        create_contact.md
        update_contact.md
        search_contacts.md
        create_company.md
        update_company.md
        search_companies.md
        associate_contact_company.md
```

Self-contained under the graph dir so colmena's default allowed-dir validation passes without setting `COLMENA_SKILLS_ALLOWED_DIRS`.

### `SKILL.md` (always visible to LLM via catalog)

Frontmatter:

```yaml
---
name: hubspot-crm
description: Use when calling HubSpot CRM tools (create/update/search contacts and companies, associate contact↔company). Load the reference for the specific operation.
references:
  - name: create_contact
    description: POST /crm/v3/objects/contacts — required/optional properties, dedup behavior on email.
  - name: update_contact
    description: PATCH /crm/v3/objects/contacts/{id} — partial updates, requires id from search/create.
  - name: search_contacts
    description: POST /crm/v3/objects/contacts/search — filterGroups, common operators, pagination.
  - name: create_company
    description: POST /crm/v3/objects/companies — required/optional properties, domain dedup.
  - name: update_company
    description: PATCH /crm/v3/objects/companies/{id} — partial updates, requires id from search/create.
  - name: search_companies
    description: POST /crm/v3/objects/companies/search — filterGroups, common operators, pagination.
  - name: associate_contact_company
    description: PUT /crm/v4/.../associations/default/... — default association type IDs and when to use this vs. labeled associations.
---
```

Body covers what applies to **every** endpoint (so it doesn't need to repeat in each reference):
- Base URL and auth model (token already injected — LLM ignores).
- HubSpot's standard error envelope: `{ status, message, category, errors[] }` with categories `VALIDATION_ERROR`, `OBJECT_NOT_FOUND`, `CONFLICT` (duplicate), `RATE_LIMITS`. How the LLM should react to each (retry vs. ask user vs. abort).
- Property naming convention (HubSpot internal names: `firstname`, `lastname`, `email`, `phone`, `lifecyclestage`, `name`, `domain`, `industry`, `city`, `country`).
- The `properties` envelope: every create/update body wraps fields in `{ "properties": { ... } }`.
- When to `search` before `update`/`associate`: the rule is "never invent an `id`; always obtain it from `create_*` response or `search_*`".
- Association type semantics: the v4 endpoint with the `/associations/default/` URL segment delegates to HubSpot's primary association type — the call body is empty and no type ID is passed. Listed once here so `associate_contact_company.md` can stay focused on the call shape and the alternate "labeled associations" form (type ID in body) is referenced as out-of-scope. The exact primary type IDs (e.g. contact→company) must be confirmed at implementation time against the live HubSpot API spec — do not hardcode IDs without verification.

### Each `references/<endpoint>.md` (loaded on demand)

Uniform structure (~1 page each):
1. **Tool name + HTTP method/path** (the entry must match the tool name in `tool_configurations` exactly).
2. **What the LLM provides** — restate the LLM-visible params from the Tools section so the reference is self-contained when loaded.
3. **Property table** (for create/update): internal name, type, required, example, notes (formats, valid enum values, common pitfalls).
4. **Filter operators table** (for search references): EQ, NEQ, CONTAINS_TOKEN, HAS_PROPERTY, GT, LT, BETWEEN, IN — with example values and when to use each.
5. **Request body example** — copy-pasteable JSON the LLM can adapt.
6. **Response shape** — the `body` field returned by the http_request node. Highlight `id` for create/update, `results[].id` and `total` for search, association ids for associate. The skill must teach the LLM how to parse outputs the engine will not pre-process.
7. **Common errors and remedies** — by `category` (`VALIDATION_ERROR`, `OBJECT_NOT_FOUND`, `CONFLICT`, `RATE_LIMITS`) with concrete next-step instructions.
8. **When NOT to use this tool** — e.g. *"do not call `update_contact` without an `id` — call `search_contacts` first"*.

## Graph: `agent.json`

Conversational pattern from project memory: one `input` node feeds one `llm_call`; multi-turn is achieved by editing `nodes.input.config.default` and re-invoking with the same `--agent-session-id`.

```
input → agent_llm → log
```

`agent_llm` (`llm_call`) configuration:
- `provider: "gemini"`, `model: "gemini-2.5-flash"` (project default).
- `connection_url: "${DATABASE_URL}"` for `llm_node_history`.
- `system_message`: ~1 line — agent role only. *"You manage HubSpot CRM contacts and companies. Load the matching reference from the `hubspot-crm` skill before any operation whose schema you are unsure of."*
- `skills: { "paths": ["./skills/hubspot-crm"] }`.
- `tool_configurations`: 7 entries as described above. No `enabled_tools` field (auto-enabled per project convention).

`input` node uses `default` for the prompt; turns are advanced by editing the JSON file (per project memory).

## Knowledge split (the design's key invariant)

| Surface | Visibility | Contents |
|---|---|---|
| `system_message` | every turn | Agent role only. |
| Tool `description` | every turn (native tool list) | 1–2 lines: what the tool does + pointer to the matching reference. |
| `SKILL.md` body | every turn (catalog in `load_skill` description) | Auth model (informational), error envelope, property naming, `search`-before-`update` rule, association type IDs. |
| `references/<endpoint>.md` | only after `load_skill` | Property tables, body examples, response shape, error-by-error remedies. |

Simple turns (`"crear contacto Juan juan@x.com"`) call the tool without loading any reference. Complex turns (`"asocia juan@acme.com con Acme"`) trigger `search_contacts` → `search_companies` → `associate_contact_company`, with `load_skill` calls interleaved as the LLM hits unfamiliar parameters.

## Testing plan

Manual via the DAG engine CLI. Each turn is a separate `cargo run` with `--agent-session-id hubspot_demo_001`:

1. **Turn 1 — create contact**: prompt *"Create contact Juan Perez, juan@acme.test, phone +57 300 1234567"*. Expect `create_contact` call, returned `id` saved in conversation memory.
2. **Turn 2 — create company**: prompt *"Create company Acme Corp, domain acme.test, industry SOFTWARE"*. Expect `create_company` call.
3. **Turn 3 — associate**: prompt *"Associate Juan with Acme Corp"*. Expect (a) maybe `search_contacts`/`search_companies` if the LLM did not retain ids, (b) `associate_contact_company`. The reference `associate_contact_company` is expected to be loaded here.
4. **Turn 4 — update**: prompt *"Set Juan's lifecycle stage to customer"*. Expect `update_contact` with the `id` recalled from memory and `lifecyclestage: "customer"`. May load `update_contact` reference for the property name.
5. **Turn 5 — search by company**: prompt *"List contacts of Acme Corp"*. Sanity check on `search_contacts` filtering by `associatedcompanyid`.

Acceptance: every HTTP call returns 2xx, the `skills_used` summary in the run output shows at least one reference loaded across the 5 turns, and the engine logs no `secure_values` warnings.

Optional clean-up: a sixth turn that deletes the test contact + company is *not* part of this scope (no delete tool).

## Out of scope / explicit deferrals

- Delete tools (DELETE on contacts/companies). Add later if tests need teardown automation.
- Batch endpoints (`/batch/create`, `/batch/update`).
- Custom properties (only standard HubSpot properties).
- ADP canvas-builder integration (this is a colmena-side test graph, not an ADP canvas).
- Migration to OAuth refresh-token flow — covered by the existing Amadeus pattern; if required later, swap `bearer_token` to `${context.hubspot_token}` and add a `get_hubspot_token` `http_request` node.

## Files to create / modify

- `tests/graphs/advanced/hubspot/agent.json` (new)
- `tests/graphs/advanced/hubspot/skills/hubspot-crm/SKILL.md` (new)
- `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/{create,update,search}_{contact,company}.md` (6 new)
- `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/associate_contact_company.md` (new)
- `.env` — add `HUBSPOT_PRIVATE_APP_TOKEN=...` (user-managed, not committed)
- No source-code changes (no new Rust nodes, no registry edits — `http_request` and the skills system already cover everything).
