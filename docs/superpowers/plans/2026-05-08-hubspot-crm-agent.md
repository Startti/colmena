# HubSpot CRM Agent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a conversational HubSpot CRM agent (one `llm_call` graph + one on-demand markdown skill with 7 endpoint references) that creates and updates contacts and companies and associates them, authenticated via a Private App Access Token.

**Architecture:** Single `llm_call` node configured with 7 `http_request` tools (no orchestrator). Each tool's `node_schema` fixes the auth/transport (`base_url`, `method`, `bearer_token`, `headers`) and exposes a minimal LLM-visible body. Per-endpoint property tables, request/response shapes, and error remedies live in a colmena skill `hubspot-crm` loaded on demand via `load_skill`. Conversational loop is the project's standard pattern: an `input` node whose `prompt` is edited between turns and re-invoked with `--agent-session-id`.

**Tech Stack:** colmena DAG engine (`http_request`, `llm_call`, `input`, `log` nodes), colmena skills (markdown frontmatter + `references/`), Gemini 2.5 Flash, HubSpot CRM v3/v4 REST API.

**Spec:** [docs/superpowers/specs/2026-05-08-hubspot-crm-agent-design.md](../specs/2026-05-08-hubspot-crm-agent-design.md)

---

## File structure

| Path | Responsibility |
|---|---|
| `tests/graphs/advanced/hubspot/agent.json` | The graph: input → llm_call (with 7 tools + skill) → log |
| `tests/graphs/advanced/hubspot/skills/hubspot-crm/SKILL.md` | Always-visible catalog: auth model, error envelope, property naming, references list |
| `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/create_contact.md` | POST /crm/v3/objects/contacts — properties, dedup |
| `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/update_contact.md` | PATCH /crm/v3/objects/contacts/{id} — partial updates |
| `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/search_contacts.md` | POST .../contacts/search — filterGroups, operators |
| `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/create_company.md` | POST /crm/v3/objects/companies — properties |
| `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/update_company.md` | PATCH /crm/v3/objects/companies/{id} — partial updates |
| `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/search_companies.md` | POST .../companies/search — filterGroups, operators |
| `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/associate_contact_company.md` | PUT /crm/v4/.../associations/default/... — primary association |
| `.env` (repo root, user-managed) | Add `HUBSPOT_PRIVATE_APP_TOKEN=...` |

No Rust source code changes. The `http_request` node and the skills system already cover everything needed.

---

### Task 1: Scaffold directory and confirm env var

**Files:**
- Create: `tests/graphs/advanced/hubspot/`
- Create: `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/`
- Modify (user-managed): `.env` — add `HUBSPOT_PRIVATE_APP_TOKEN=...`

- [ ] **Step 1: Create the directory tree**

```bash
mkdir -p tests/graphs/advanced/hubspot/skills/hubspot-crm/references
```

- [ ] **Step 2: Verify `.env` contains the token (do NOT print its value)**

```bash
grep -c '^HUBSPOT_PRIVATE_APP_TOKEN=' .env
```

Expected: `1`. If `0`, ask the user to add `HUBSPOT_PRIVATE_APP_TOKEN=pat-...` to `.env`. Never echo, cat, or commit the value. The `.env` file is gitignored.

- [ ] **Step 3: Verify the skills allow-dir rule will pass**

Skills under the graph directory itself are always allowed; no `COLMENA_SKILLS_ALLOWED_DIRS` env var is required. Confirm the layout:

```bash
test -d tests/graphs/advanced/hubspot/skills/hubspot-crm/references && echo OK
```

Expected: `OK`.

- [ ] **Step 4: Commit the empty scaffold**

```bash
git add tests/graphs/advanced/hubspot/
git commit -m "chore(hubspot): scaffold graph + skill directories"
```

---

### Task 2: Write `SKILL.md` (always-visible catalog)

**Files:**
- Create: `tests/graphs/advanced/hubspot/skills/hubspot-crm/SKILL.md`

- [ ] **Step 1: Write the file with frontmatter + general body**

```markdown
---
name: hubspot-crm
description: Use when calling HubSpot CRM tools — create/update/search contacts and companies, or associate a contact with a company. Load the reference for the specific operation you are about to call to get its property table, request/response shape, and error remedies.
references:
  - name: create_contact
    description: POST /crm/v3/objects/contacts — required/optional properties, dedup behavior on email.
  - name: update_contact
    description: PATCH /crm/v3/objects/contacts/{id} — partial updates, requires id from search/create.
  - name: search_contacts
    description: POST /crm/v3/objects/contacts/search — filterGroups, common operators, pagination.
  - name: create_company
    description: POST /crm/v3/objects/companies — required/optional properties, dedup notes.
  - name: update_company
    description: PATCH /crm/v3/objects/companies/{id} — partial updates, requires id from search/create.
  - name: search_companies
    description: POST /crm/v3/objects/companies/search — filterGroups, operators, pagination.
  - name: associate_contact_company
    description: PUT /crm/v4/.../associations/default/... — primary association, empty body, when NOT to use labeled associations.
---

# HubSpot CRM

You are calling HubSpot's CRM API v3/v4. The agent has 7 tools wired to `http_request`. Auth, base URL, method, and headers are already fixed in the tool config — you never set them.

## Base URL and authentication

- Base URL: `https://api.hubapi.com`
- Authentication: a `Bearer` token from a HubSpot Private App is injected automatically. Do not include it in any tool argument.

## Body envelope

Every create and update body is wrapped as:

\`\`\`json
{ "properties": { "<internal_name>": "<value>", ... } }
\`\`\`

The `properties` object is what the LLM provides. The wrapper is built by the tool config. Keys inside `properties` use HubSpot's internal property names (lowercase, no spaces).

Common contact internal names: `email`, `firstname`, `lastname`, `phone`, `company`, `lifecyclestage`, `jobtitle`, `website`.
Common company internal names: `name`, `domain`, `industry`, `phone`, `city`, `country`, `numberofemployees`, `description`.

## Error envelope

HubSpot 4xx/5xx responses have this shape:

\`\`\`json
{
  "status": "error",
  "message": "Property values were not valid: ...",
  "category": "VALIDATION_ERROR",
  "errors": [ { "message": "...", "in": "properties.email" } ]
}
\`\`\`

The `body` field returned by the `http_request` tool contains this exact JSON.

| `category` | What it means | What to do |
|---|---|---|
| `VALIDATION_ERROR` | A property name or value is invalid | Re-read the reference's property table and fix; do not retry blindly. |
| `OBJECT_NOT_FOUND` | The id in the path doesn't exist | The id is stale or wrong. Call `search_*` to find the correct id. Do not invent ids. |
| `CONFLICT` | A unique property collides (e.g. duplicate email) | Call `search_*` by that property and decide between `update_*` and abort. Ask the user if ambiguous. |
| `RATE_LIMITS` | Too many requests | Stop. Inform the user. Do not loop retries. |

## When to search before acting

- **Update / associate** require an `id`. **Never invent an id.** If you do not have one from a prior `create_*` or `search_*` in this conversation, call the matching `search_*` first.
- For associate, you need both ids — search the contact and the company independently if you do not have them.

## Association type IDs

The `associate_contact_company` tool uses the URL segment `/associations/default/`, which means HubSpot's primary association type. The body is empty. There is no need to pass a type id. Labeled associations (custom association types) are out of scope.

## Tool naming and reference loading

Each tool name maps 1:1 to a reference in this skill (e.g. tool `create_contact` ↔ reference `create_contact`). Load the reference whenever you need the full property table, valid filter operators, or detailed error remedies. Simple, well-known calls (e.g. creating a contact with just an email) often do not need a reference load.
```

- [ ] **Step 2: Sanity-check frontmatter parses**

Run a quick load of the graph (still empty) is not yet possible — but the SKILL.md frontmatter parses if it has the trailing `---` and a `name` and `description`. Just `cat` the file to make sure both are present:

```bash
head -25 tests/graphs/advanced/hubspot/skills/hubspot-crm/SKILL.md
```

Expected: see the frontmatter block ending with `---` followed by the body.

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/advanced/hubspot/skills/hubspot-crm/SKILL.md
git commit -m "docs(hubspot-crm): SKILL.md catalog with auth + error envelope"
```

---

### Task 3: Write `references/create_contact.md`

**Files:**
- Create: `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/create_contact.md`

- [ ] **Step 1: Write the file**

```markdown
# create_contact

**Tool:** `create_contact`
**HTTP:** `POST /crm/v3/objects/contacts`

## What you provide (LLM-visible parameters)

- `properties` (object, required) — the contact fields. See the table below.

The tool builds the request body as `{ "properties": { ... } }` automatically.

## Properties

| Internal name | Type | Required | Example | Notes |
|---|---|---|---|---|
| `email` | string | yes | `"jane@acme.test"` | Used by HubSpot for dedup. Triggers `CONFLICT` if exists. |
| `firstname` | string | no | `"Jane"` | |
| `lastname` | string | no | `"Doe"` | |
| `phone` | string | no | `"+57 300 1234567"` | Free-form string; E.164 recommended. |
| `company` | string | no | `"Acme Corp"` | Plain text only. To link the contact to a company record use `associate_contact_company` after creating both. |
| `jobtitle` | string | no | `"VP of Engineering"` | |
| `website` | string | no | `"https://acme.test"` | |
| `lifecyclestage` | string | no | `"customer"` | One of: `subscriber`, `lead`, `marketingqualifiedlead`, `salesqualifiedlead`, `opportunity`, `customer`, `evangelist`, `other`. |

## Request body example

\`\`\`json
{
  "properties": {
    "email": "jane@acme.test",
    "firstname": "Jane",
    "lastname": "Doe",
    "phone": "+57 300 1234567",
    "lifecyclestage": "lead"
  }
}
\`\`\`

## Response shape (HTTP 201)

The `body` returned by the tool:

\`\`\`json
{
  "id": "12345",
  "properties": { "email": "jane@acme.test", "firstname": "Jane", ... },
  "createdAt": "2026-05-08T10:00:00.000Z",
  "updatedAt": "2026-05-08T10:00:00.000Z",
  "archived": false
}
\`\`\`

**Save `body.id`** — you need it for `update_contact` and `associate_contact_company`.

## Errors

- `409 CONFLICT` `Contact already exists` — the email is taken. Call `search_contacts` with `propertyName: "email", operator: "EQ", value: "<email>"` to find the existing id, then either `update_contact` or stop and ask the user.
- `400 VALIDATION_ERROR` — re-read the property table; check for typos in the internal name (e.g. `firstName` is invalid; `firstname` is correct).

## When NOT to use

- Do not call `create_contact` again to "update" a contact you already created — use `update_contact` with the saved `id`.
- Do not invent properties not in the table. Custom properties exist but are out of scope.
```

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/advanced/hubspot/skills/hubspot-crm/references/create_contact.md
git commit -m "docs(hubspot-crm): reference create_contact"
```

---

### Task 4: Write `references/update_contact.md`

**Files:**
- Create: `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/update_contact.md`

- [ ] **Step 1: Write the file**

```markdown
# update_contact

**Tool:** `update_contact`
**HTTP:** `PATCH /crm/v3/objects/contacts/{contactId}`

## What you provide (LLM-visible parameters)

- `endpoint` (string, required) — the full path with the real id substituted, e.g. `/crm/v3/objects/contacts/12345`. Get the id from a prior `create_contact` response or `search_contacts`. **Never invent an id.**
- `properties` (object, required) — the fields to update. Partial updates are allowed; only the fields you include are changed.

## Properties

Same property names as `create_contact` (see that reference for the table). Any subset is valid:

| Internal name | Notes when updating |
|---|---|
| `email` | Allowed but rare — changing email may collide with another contact (`CONFLICT`). |
| `firstname`, `lastname`, `phone`, `jobtitle`, `website` | Free-form updates. |
| `lifecyclestage` | One of the values listed in `create_contact`. HubSpot prevents moving "backwards" in some portals — a `VALIDATION_ERROR` here means the move is not allowed. |

## Request body example

To set Jane's lifecycle stage to customer:

\`\`\`json
{
  "properties": {
    "lifecyclestage": "customer"
  }
}
\`\`\`

`endpoint` for the same call: `/crm/v3/objects/contacts/12345`.

## Response shape (HTTP 200)

Same shape as `create_contact` — full updated record with `id`, `properties`, `createdAt`, `updatedAt`. The properties echoed are HubSpot's view, which may include defaults.

## Errors

- `404 OBJECT_NOT_FOUND` — the id in the path is wrong or the contact was deleted. Re-run `search_contacts` to get a fresh id.
- `400 VALIDATION_ERROR` — wrong property name (e.g. `Phone` instead of `phone`), invalid enum value (e.g. `"VIP"` for `lifecyclestage`).
- `409 CONFLICT` — only when changing `email` to one that already exists.

## When NOT to use

- Do not call `update_contact` with an id you "remember from earlier" if you are unsure — call `search_contacts` first to verify it still exists.
- Do not use `update_contact` to associate a contact with a company. Use `associate_contact_company`.
```

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/advanced/hubspot/skills/hubspot-crm/references/update_contact.md
git commit -m "docs(hubspot-crm): reference update_contact"
```

---

### Task 5: Write `references/search_contacts.md`

**Files:**
- Create: `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/search_contacts.md`

- [ ] **Step 1: Write the file**

```markdown
# search_contacts

**Tool:** `search_contacts`
**HTTP:** `POST /crm/v3/objects/contacts/search`

## What you provide (LLM-visible parameters)

- `filterGroups` (array, required) — see structure and operators below.
- `properties` (array, optional) — names of the properties you want returned, e.g. `["email","firstname","lastname","phone"]`. Default minimal set if omitted.
- `limit` (number, optional) — max results. Default 10, max 100.

## filterGroups structure

\`\`\`json
[
  {
    "filters": [
      { "propertyName": "<internal name>", "operator": "<OP>", "value": "<value>" }
    ]
  }
]
\`\`\`

- Filters within a single group are AND'd.
- Filter groups (top-level array) are OR'd.

## Operators

| Operator | Use when | Value field |
|---|---|---|
| `EQ` | Exact match (most common) | string |
| `NEQ` | Not equal | string |
| `CONTAINS_TOKEN` | Substring tokens (whitespace-tokenized) | string |
| `HAS_PROPERTY` | Property is set (any value) | omit `value` |
| `NOT_HAS_PROPERTY` | Property is empty | omit `value` |
| `GT` / `GTE` / `LT` / `LTE` | Numeric or date comparisons | string-encoded number/timestamp |
| `BETWEEN` | Range | use `value` AND `highValue` |
| `IN` | Match any of a list | use `values` (array) instead of `value` |

## Request body example — find a contact by email

\`\`\`json
{
  "filterGroups": [
    {
      "filters": [
        { "propertyName": "email", "operator": "EQ", "value": "jane@acme.test" }
      ]
    }
  ],
  "properties": ["email", "firstname", "lastname", "phone"],
  "limit": 1
}
\`\`\`

## Request body example — find contacts associated with a company

\`\`\`json
{
  "filterGroups": [
    {
      "filters": [
        { "propertyName": "associatedcompanyid", "operator": "EQ", "value": "<companyId>" }
      ]
    }
  ],
  "properties": ["email", "firstname", "lastname"],
  "limit": 50
}
\`\`\`

## Response shape (HTTP 200)

\`\`\`json
{
  "total": 1,
  "results": [
    {
      "id": "12345",
      "properties": { "email": "jane@acme.test", "firstname": "Jane", ... },
      "createdAt": "...",
      "updatedAt": "...",
      "archived": false
    }
  ],
  "paging": { "next": { "after": "10", "link": "..." } }
}
\`\`\`

**Save `body.results[].id`** for follow-up `update_*` or `associate_*` calls.

## Errors

- `400 VALIDATION_ERROR` — wrong `propertyName` or operator. Re-read the operator table.
- Empty `results` and `total: 0` are not errors — they mean "no match".

## When NOT to use

- Do not chain multiple `search_contacts` calls when one filter group with multiple AND'd filters works.
- Do not use this for analytics/list-everything — `limit` is capped at 100. For larger sets you would page via `paging.next.after`, but that is rarely needed in conversational flows.
```

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/advanced/hubspot/skills/hubspot-crm/references/search_contacts.md
git commit -m "docs(hubspot-crm): reference search_contacts"
```

---

### Task 6: Write `references/create_company.md`

**Files:**
- Create: `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/create_company.md`

- [ ] **Step 1: Write the file**

```markdown
# create_company

**Tool:** `create_company`
**HTTP:** `POST /crm/v3/objects/companies`

## What you provide (LLM-visible parameters)

- `properties` (object, required) — the company fields. See the table below.

The tool builds the request body as `{ "properties": { ... } }` automatically.

## Properties

| Internal name | Type | Required | Example | Notes |
|---|---|---|---|---|
| `name` | string | yes | `"Acme Corp"` | Display name. |
| `domain` | string | no | `"acme.test"` | Bare domain (no scheme, no path). HubSpot uses domain to suggest matches; not strictly unique unless the portal enables domain-based dedup. |
| `industry` | string | no | `"COMPUTER_SOFTWARE"` | Free string; HubSpot has a recommended enum but accepts arbitrary strings. |
| `phone` | string | no | `"+1 415 5550100"` | |
| `city` | string | no | `"Bogota"` | |
| `country` | string | no | `"Colombia"` | |
| `numberofemployees` | string | no | `"50"` | Numeric content but stored as string. |
| `description` | string | no | `"B2B logistics platform."` | |
| `website` | string | no | `"https://acme.test"` | |

## Request body example

\`\`\`json
{
  "properties": {
    "name": "Acme Corp",
    "domain": "acme.test",
    "industry": "COMPUTER_SOFTWARE",
    "city": "Bogota",
    "country": "Colombia"
  }
}
\`\`\`

## Response shape (HTTP 201)

\`\`\`json
{
  "id": "67890",
  "properties": { "name": "Acme Corp", "domain": "acme.test", ... },
  "createdAt": "2026-05-08T10:01:00.000Z",
  "updatedAt": "2026-05-08T10:01:00.000Z",
  "archived": false
}
\`\`\`

**Save `body.id`** — required for `update_company` and `associate_contact_company`.

## Errors

- `400 VALIDATION_ERROR` — typically a typo in property name (e.g. `Name` vs `name`).
- HubSpot does NOT auto-error on duplicate domain by default — duplicates can be created. To avoid this, call `search_companies` by `domain EQ "<domain>"` first.

## When NOT to use

- Do not call `create_company` repeatedly for the same domain — search first when there is risk of duplicate.
- Do not use it to associate the company with a contact. Use `associate_contact_company`.
```

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/advanced/hubspot/skills/hubspot-crm/references/create_company.md
git commit -m "docs(hubspot-crm): reference create_company"
```

---

### Task 7: Write `references/update_company.md`

**Files:**
- Create: `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/update_company.md`

- [ ] **Step 1: Write the file**

```markdown
# update_company

**Tool:** `update_company`
**HTTP:** `PATCH /crm/v3/objects/companies/{companyId}`

## What you provide (LLM-visible parameters)

- `endpoint` (string, required) — full path with the real company id, e.g. `/crm/v3/objects/companies/67890`. Get the id from a prior `create_company` or `search_companies`. **Never invent an id.**
- `properties` (object, required) — the fields to update (partial; only included fields change).

## Properties

Same names as `create_company` (see that reference for the full table). Any subset is valid:

| Internal name | Notes when updating |
|---|---|
| `name`, `domain`, `industry`, `phone`, `city`, `country`, `numberofemployees`, `description`, `website` | Free updates. |

## Request body example

To set the company's industry and number of employees:

\`\`\`json
{
  "properties": {
    "industry": "FINANCIAL_SERVICES",
    "numberofemployees": "120"
  }
}
\`\`\`

`endpoint` for the same call: `/crm/v3/objects/companies/67890`.

## Response shape (HTTP 200)

Same shape as `create_company` — `id`, full `properties`, `createdAt`, `updatedAt`.

## Errors

- `404 OBJECT_NOT_FOUND` — id is wrong or company was deleted. Run `search_companies` to refresh.
- `400 VALIDATION_ERROR` — typo in property name.

## When NOT to use

- Do not use `update_company` to attach a contact. Use `associate_contact_company`.
- Do not call without a verified id.
```

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/advanced/hubspot/skills/hubspot-crm/references/update_company.md
git commit -m "docs(hubspot-crm): reference update_company"
```

---

### Task 8: Write `references/search_companies.md`

**Files:**
- Create: `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/search_companies.md`

- [ ] **Step 1: Write the file**

```markdown
# search_companies

**Tool:** `search_companies`
**HTTP:** `POST /crm/v3/objects/companies/search`

## What you provide (LLM-visible parameters)

- `filterGroups` (array, required) — same structure as `search_contacts`.
- `properties` (array, optional) — property names to return, e.g. `["name","domain","industry"]`.
- `limit` (number, optional) — default 10, max 100.

## filterGroups structure

\`\`\`json
[
  { "filters": [ { "propertyName": "<internal name>", "operator": "<OP>", "value": "<value>" } ] }
]
\`\`\`

Filters within a group are AND'd; groups are OR'd.

## Operators

Same as `search_contacts`: `EQ`, `NEQ`, `CONTAINS_TOKEN`, `HAS_PROPERTY`, `NOT_HAS_PROPERTY`, `GT`, `GTE`, `LT`, `LTE`, `BETWEEN`, `IN`.

## Request body example — find by domain

\`\`\`json
{
  "filterGroups": [
    {
      "filters": [
        { "propertyName": "domain", "operator": "EQ", "value": "acme.test" }
      ]
    }
  ],
  "properties": ["name", "domain", "industry"],
  "limit": 1
}
\`\`\`

## Request body example — find by name fragment

\`\`\`json
{
  "filterGroups": [
    {
      "filters": [
        { "propertyName": "name", "operator": "CONTAINS_TOKEN", "value": "Acme" }
      ]
    }
  ],
  "properties": ["name", "domain"],
  "limit": 5
}
\`\`\`

## Response shape (HTTP 200)

\`\`\`json
{
  "total": 1,
  "results": [
    { "id": "67890", "properties": { "name": "Acme Corp", "domain": "acme.test", ... }, "createdAt": "...", "updatedAt": "...", "archived": false }
  ],
  "paging": { "next": { "after": "10" } }
}
\`\`\`

**Save `body.results[].id`** for follow-up calls.

## Errors

- `400 VALIDATION_ERROR` — wrong `propertyName` or operator.
- Empty `results` is not an error.

## When NOT to use

- Do not call `search_companies` when you already have the id from a prior `create_company` in this conversation — reuse the id.
- Do not use it for analytics — `limit` is capped at 100.
```

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/advanced/hubspot/skills/hubspot-crm/references/search_companies.md
git commit -m "docs(hubspot-crm): reference search_companies"
```

---

### Task 9: Write `references/associate_contact_company.md`

**Files:**
- Create: `tests/graphs/advanced/hubspot/skills/hubspot-crm/references/associate_contact_company.md`

- [ ] **Step 1: Write the file**

```markdown
# associate_contact_company

**Tool:** `associate_contact_company`
**HTTP:** `PUT /crm/v4/objects/contact/{contactId}/associations/default/company/{companyId}`

## What you provide (LLM-visible parameters)

- `endpoint` (string, required) — full path with both real ids substituted. Example: `/crm/v4/objects/contact/12345/associations/default/company/67890`. **Both ids must come from prior `search_*` or `create_*` results — never invent.**

The body is empty (the tool fixes it to `{}`). The `default` URL segment selects HubSpot's primary association type, so no type id needs to be passed.

## Endpoint construction

| Segment | Value |
|---|---|
| Object type from | `contact` (literal — the v4 endpoint expects the lowercase name) |
| `{contactId}` | id of the contact |
| Object type to | `company` |
| `{companyId}` | id of the company |

So with `contactId = 12345` and `companyId = 67890`:

\`\`\`
/crm/v4/objects/contact/12345/associations/default/company/67890
\`\`\`

## Response shape (HTTP 200)

\`\`\`json
{
  "fromObjectTypeId": "0-1",
  "fromObjectId": "12345",
  "toObjectTypeId": "0-2",
  "toObjectId": "67890",
  "labels": []
}
\`\`\`

`labels` is empty for the default (primary) association.

## Errors

- `404 OBJECT_NOT_FOUND` — one of the ids does not exist. Re-search.
- `400` — malformed path. Re-check the segment order: it is `contact/{id}/associations/default/company/{id}`, not the reverse.

## When NOT to use

- Do not pass a body. The default association does not accept type id in the body.
- Labeled (custom-typed) associations are out of scope. They use a different endpoint with a body containing `associationCategory` and `associationTypeId` — not supported by this tool.
- The reverse direction (company-to-contact) is created automatically by HubSpot when you create the contact-to-company default association. Do not call this tool again with swapped arguments.
```

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/advanced/hubspot/skills/hubspot-crm/references/associate_contact_company.md
git commit -m "docs(hubspot-crm): reference associate_contact_company"
```

---

### Task 10: Write `agent.json` skeleton (no tools yet) and verify it loads

**Files:**
- Create: `tests/graphs/advanced/hubspot/agent.json`

The goal of this task is to confirm the input → llm_call → log shape parses, runs, and that the skill catalog appears in the LLM tool list (visible via the `load_skill` synthetic tool). Tools are added in Task 11 to keep this isolation.

- [ ] **Step 1: Write the skeleton graph**

```json
{
  "nodes": {
    "input": {
      "type": "input",
      "config": {
        "prompt": "Hi — just say hello back, no tools needed."
      }
    },
    "agent_llm": {
      "type": "llm_call",
      "config": {
        "provider": "gemini",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "system_message": "You manage HubSpot CRM contacts and companies. When you need details about a specific operation, load the matching reference from the hubspot-crm skill.",
        "temperature": 0.2,
        "connection_url": "${DATABASE_URL}",
        "skills": {
          "paths": ["./skills/hubspot-crm"]
        }
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input", "to": "agent_llm" },
    { "from": "agent_llm", "to": "log" }
  ]
}
```

- [ ] **Step 2: Verify graph loads (skill validation runs at graph load — broken frontmatter or missing reference files will fail here)**

```bash
source .env && cargo run --bin dag_engine -- run tests/graphs/advanced/hubspot/agent.json --agent-session-id hubspot_demo_001
```

Expected:
- Graph loads with no error.
- A "hello" reply from the LLM is logged.
- The run summary's `skills_used` field is **absent** (no skill actually loaded — the message did not require it).

If you see `skill validation failed` or `reference 'X' not found in skills/`, re-check that all 7 reference files exist with the exact names `create_contact.md`, `update_contact.md`, etc.

- [ ] **Step 3: Optional — confirm the LLM sees the skill catalog**

The catalog appears inside the `load_skill` tool description, which is part of the tool list sent to the model. There is no CLI flag to dump the tool list directly, so a quick way to verify is to set `prompt` to: `"List the references available to you in the hubspot-crm skill, by name only."` and re-run. The LLM should enumerate the 7 references. Revert `prompt` afterwards.

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/advanced/hubspot/agent.json
git commit -m "feat(hubspot): agent.json skeleton with hubspot-crm skill"
```

---

### Task 11: Add the 7 `tool_configurations` entries to `agent.json`

**Files:**
- Modify: `tests/graphs/advanced/hubspot/agent.json` — add `tool_configurations` block inside `agent_llm.config`

- [ ] **Step 1: Replace the file with the full version including all 7 tools**

Overwrite `tests/graphs/advanced/hubspot/agent.json` with:

```json
{
  "nodes": {
    "input": {
      "type": "input",
      "config": {
        "prompt": "Create a contact: Jane Doe, jane@acme.test, phone +57 300 1234567."
      }
    },
    "agent_llm": {
      "type": "llm_call",
      "config": {
        "provider": "gemini",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "system_message": "You manage HubSpot CRM contacts and companies. When you need details about a specific operation, load the matching reference from the hubspot-crm skill.",
        "temperature": 0.2,
        "connection_url": "${DATABASE_URL}",
        "skills": {
          "paths": ["./skills/hubspot-crm"]
        },
        "tool_configurations": {
          "create_contact": {
            "name": "create_contact",
            "node_type": "http_request",
            "description": "Create a HubSpot contact. Provide a `properties` object with at least `email`. See `hubspot-crm/create_contact` for valid property names and dedup behavior.",
            "node_schema": {
              "base_url":     { "type": "string", "fixed": "https://api.hubapi.com" },
              "endpoint":     { "type": "string", "fixed": "/crm/v3/objects/contacts" },
              "method":       { "type": "string", "fixed": "POST" },
              "bearer_token": { "type": "string", "fixed": "${HUBSPOT_PRIVATE_APP_TOKEN}" },
              "headers":      { "type": "object", "fixed": { "Content-Type": "application/json" } },
              "body": {
                "type": "object",
                "properties": {
                  "properties": {
                    "type": "object",
                    "required": true,
                    "description": "Contact fields. Required: email. Common: firstname, lastname, phone, company, lifecyclestage. See hubspot-crm/create_contact for the full property list."
                  }
                }
              }
            }
          },
          "update_contact": {
            "name": "update_contact",
            "node_type": "http_request",
            "description": "Update a HubSpot contact's properties. Requires the contact id in the endpoint path. See `hubspot-crm/update_contact`.",
            "node_schema": {
              "base_url":     { "type": "string", "fixed": "https://api.hubapi.com" },
              "method":       { "type": "string", "fixed": "PATCH" },
              "bearer_token": { "type": "string", "fixed": "${HUBSPOT_PRIVATE_APP_TOKEN}" },
              "headers":      { "type": "object", "fixed": { "Content-Type": "application/json" } },
              "endpoint": {
                "type": "string",
                "required": true,
                "description": "Full path of the form /crm/v3/objects/contacts/{contactId} where {contactId} is the real id from a prior search_contacts or create_contact response. Never invent an id."
              },
              "body": {
                "type": "object",
                "properties": {
                  "properties": {
                    "type": "object",
                    "required": true,
                    "description": "Properties to update (partial). Same names as create_contact. See hubspot-crm/update_contact."
                  }
                }
              }
            }
          },
          "search_contacts": {
            "name": "search_contacts",
            "node_type": "http_request",
            "description": "Search HubSpot contacts using filterGroups. Use this before update or associate when you only have an email/name. See `hubspot-crm/search_contacts` for operators.",
            "node_schema": {
              "base_url":     { "type": "string", "fixed": "https://api.hubapi.com" },
              "endpoint":     { "type": "string", "fixed": "/crm/v3/objects/contacts/search" },
              "method":       { "type": "string", "fixed": "POST" },
              "bearer_token": { "type": "string", "fixed": "${HUBSPOT_PRIVATE_APP_TOKEN}" },
              "headers":      { "type": "object", "fixed": { "Content-Type": "application/json" } },
              "body": {
                "type": "object",
                "properties": {
                  "filterGroups": {
                    "type": "array",
                    "required": true,
                    "description": "Array of filter groups (OR'd together). Each group: { filters: [ { propertyName, operator, value } ] } (filters within a group are AND'd). Operators: EQ, NEQ, CONTAINS_TOKEN, HAS_PROPERTY, NOT_HAS_PROPERTY, GT, LT, BETWEEN, IN. See hubspot-crm/search_contacts.",
                    "items": { "type": "object" }
                  },
                  "properties": {
                    "type": "array",
                    "description": "Property names to return on each result, e.g. ['email','firstname','lastname']. Default minimal set if omitted.",
                    "items": { "type": "string" }
                  },
                  "limit": {
                    "type": "number",
                    "description": "Max results (default 10, max 100)."
                  }
                }
              }
            }
          },
          "create_company": {
            "name": "create_company",
            "node_type": "http_request",
            "description": "Create a HubSpot company. Provide a `properties` object with at least `name`. See `hubspot-crm/create_company` for valid property names and dedup notes.",
            "node_schema": {
              "base_url":     { "type": "string", "fixed": "https://api.hubapi.com" },
              "endpoint":     { "type": "string", "fixed": "/crm/v3/objects/companies" },
              "method":       { "type": "string", "fixed": "POST" },
              "bearer_token": { "type": "string", "fixed": "${HUBSPOT_PRIVATE_APP_TOKEN}" },
              "headers":      { "type": "object", "fixed": { "Content-Type": "application/json" } },
              "body": {
                "type": "object",
                "properties": {
                  "properties": {
                    "type": "object",
                    "required": true,
                    "description": "Company fields. Required: name. Common: domain, industry, phone, city, country, numberofemployees. See hubspot-crm/create_company."
                  }
                }
              }
            }
          },
          "update_company": {
            "name": "update_company",
            "node_type": "http_request",
            "description": "Update a HubSpot company's properties. Requires the company id in the endpoint path. See `hubspot-crm/update_company`.",
            "node_schema": {
              "base_url":     { "type": "string", "fixed": "https://api.hubapi.com" },
              "method":       { "type": "string", "fixed": "PATCH" },
              "bearer_token": { "type": "string", "fixed": "${HUBSPOT_PRIVATE_APP_TOKEN}" },
              "headers":      { "type": "object", "fixed": { "Content-Type": "application/json" } },
              "endpoint": {
                "type": "string",
                "required": true,
                "description": "Full path of the form /crm/v3/objects/companies/{companyId} where {companyId} is the real id from a prior search_companies or create_company response. Never invent an id."
              },
              "body": {
                "type": "object",
                "properties": {
                  "properties": {
                    "type": "object",
                    "required": true,
                    "description": "Properties to update (partial). Same names as create_company. See hubspot-crm/update_company."
                  }
                }
              }
            }
          },
          "search_companies": {
            "name": "search_companies",
            "node_type": "http_request",
            "description": "Search HubSpot companies using filterGroups. Use this before update or associate when you only have a domain/name. See `hubspot-crm/search_companies` for operators.",
            "node_schema": {
              "base_url":     { "type": "string", "fixed": "https://api.hubapi.com" },
              "endpoint":     { "type": "string", "fixed": "/crm/v3/objects/companies/search" },
              "method":       { "type": "string", "fixed": "POST" },
              "bearer_token": { "type": "string", "fixed": "${HUBSPOT_PRIVATE_APP_TOKEN}" },
              "headers":      { "type": "object", "fixed": { "Content-Type": "application/json" } },
              "body": {
                "type": "object",
                "properties": {
                  "filterGroups": {
                    "type": "array",
                    "required": true,
                    "description": "Array of filter groups (OR'd). Each group: { filters: [ { propertyName, operator, value } ] }. Operators: EQ, NEQ, CONTAINS_TOKEN, HAS_PROPERTY, NOT_HAS_PROPERTY, GT, LT, BETWEEN, IN. See hubspot-crm/search_companies.",
                    "items": { "type": "object" }
                  },
                  "properties": {
                    "type": "array",
                    "description": "Property names to return, e.g. ['name','domain','industry'].",
                    "items": { "type": "string" }
                  },
                  "limit": {
                    "type": "number",
                    "description": "Max results (default 10, max 100)."
                  }
                }
              }
            }
          },
          "associate_contact_company": {
            "name": "associate_contact_company",
            "node_type": "http_request",
            "description": "Associate a HubSpot contact with a HubSpot company using the default (primary) association type. Both ids must come from prior search/create — never invent. See `hubspot-crm/associate_contact_company`.",
            "node_schema": {
              "base_url":     { "type": "string", "fixed": "https://api.hubapi.com" },
              "method":       { "type": "string", "fixed": "PUT" },
              "bearer_token": { "type": "string", "fixed": "${HUBSPOT_PRIVATE_APP_TOKEN}" },
              "headers":      { "type": "object", "fixed": { "Content-Type": "application/json" } },
              "endpoint": {
                "type": "string",
                "required": true,
                "description": "Full path of the form /crm/v4/objects/contact/{contactId}/associations/default/company/{companyId}. Both ids required. The 'default' segment means HubSpot's primary association type; body is empty."
              },
              "body": { "type": "object", "fixed": {} }
            }
          }
        }
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input", "to": "agent_llm" },
    { "from": "agent_llm", "to": "log" }
  ]
}
```

- [ ] **Step 2: Verify the JSON parses**

```bash
python3 -c "import json,sys; json.load(open('tests/graphs/advanced/hubspot/agent.json')); print('JSON OK')"
```

Expected: `JSON OK`. If it fails, the error message points at the line with the syntax issue.

- [ ] **Step 3: Verify the graph loads (this validates `node_schema` parsing too)**

```bash
source .env && cargo run --bin dag_engine -- run tests/graphs/advanced/hubspot/agent.json --agent-session-id hubspot_smoke_$(date +%s)
```

Use a fresh `--agent-session-id` per try while iterating so you don't pollute the smoke test session. Expected:
- Graph loads with no error.
- The LLM calls `create_contact` with a `properties` object containing at least `email` (since the `prompt` in `input` asks for it).
- The HTTP node returns `status: 201` and a `body.id`.
- The run completes; the log node prints the LLM reply.

If the schema does not parse, the engine will print `parse_node_schema` errors at load time pointing to the offending tool name.

If the LLM sends only `properties: {...}` but the engine raises a "missing required field" error for the body wrapper, that means the engine did not auto-merge the LLM-supplied `properties` into `body.properties`. Inspect the run logs around `[HttpNode] Sending`. The expected behavior (per `parse_node_schema` in `tool_configuration.rs`) is that the LLM-visible `properties` param gets routed under the container `body`, so the final body is `{"properties": {...}}`.

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/advanced/hubspot/agent.json
git commit -m "feat(hubspot): wire 7 tool_configurations (create/update/search × contact|company + associate)"
```

---

### Task 12: Five-turn smoke test against real HubSpot

**Files:**
- Modify between turns: `tests/graphs/advanced/hubspot/agent.json` — only `nodes.input.config.prompt`

This task verifies the end-to-end flow on a live HubSpot Private App. Use a stable agent session id across all 5 turns so memory and skill state carry across runs.

- [ ] **Step 1: Pick a stable agent_session_id and clean any previous state**

```bash
export HSAS=hubspot_demo_001
```

If you have run prior tests with this id and want a clean slate, delete the conversation memory rows for it:

```bash
psql "$DATABASE_URL" -c "DELETE FROM llm_node_history WHERE agent_session_id = '$HSAS';"
```

(Skip this if you do not have direct DB access — running against the existing memory is fine, but the LLM may recall earlier ids.)

- [ ] **Step 2: Turn 1 — create contact**

Edit `tests/graphs/advanced/hubspot/agent.json`, set `nodes.input.config.prompt` to:

```
Create a contact: Jane Doe, email jane+test@acme.test, phone +57 300 1234567, lifecyclestage lead.
```

Run:

```bash
source .env && cargo run --bin dag_engine -- run tests/graphs/advanced/hubspot/agent.json --agent-session-id "$HSAS"
```

Expected:
- Tool call: `create_contact` with `properties: { email, firstname, lastname, phone, lifecyclestage }`.
- HTTP status `201`, `body.id` is a numeric string. Note this id (it should also persist in conversation memory).
- LLM reply confirms creation and includes the id.

- [ ] **Step 3: Turn 2 — create company**

Edit `prompt` to:

```
Create a company: Acme Corp, domain acme.test, industry COMPUTER_SOFTWARE, city Bogota, country Colombia.
```

Run the same command as Turn 1 (same `--agent-session-id`).

Expected:
- Tool call: `create_company` with `properties: { name, domain, industry, city, country }`.
- HTTP status `201`, `body.id` is a numeric string.
- LLM reply confirms creation and includes the id.

- [ ] **Step 4: Turn 3 — associate**

Edit `prompt` to:

```
Associate Jane (from earlier) with Acme Corp.
```

Run.

Expected:
- The LLM either reuses the ids from memory or calls `search_contacts` and/or `search_companies` first.
- If the LLM hesitates on the URL shape, it should call `load_skill name=hubspot-crm reference=associate_contact_company` — the run summary's `skills_used` should mention `associate_contact_company`.
- Tool call: `associate_contact_company` with `endpoint: "/crm/v4/objects/contact/<contactId>/associations/default/company/<companyId>"`.
- HTTP status `200`, response shows `fromObjectId` and `toObjectId`.

- [ ] **Step 5: Turn 4 — update contact**

Edit `prompt` to:

```
Set Jane's lifecycle stage to customer.
```

Run.

Expected:
- Tool call: `update_contact` with `endpoint: "/crm/v3/objects/contacts/<contactId>"` and `properties: { lifecyclestage: "customer" }`.
- HTTP status `200`.
- LLM reply confirms the update.

- [ ] **Step 6: Turn 5 — search by company**

Edit `prompt` to:

```
List the contacts associated with Acme Corp (just emails).
```

Run.

Expected:
- Tool call: `search_contacts` with a filter on `associatedcompanyid` equal to the company id from Turn 2 (or a fallback that searches by name pattern then narrows).
- HTTP status `200`. `body.results` includes Jane.
- LLM reply lists the email(s).

- [ ] **Step 7: Acceptance check**

For the run sequence to be considered green:
- All five HTTP calls return 2xx.
- At least one of the five turns shows `skills_used` in its run summary (this confirms the skill was actually loaded at least once during the demo).
- No `secure_values` warnings in any run output.
- No `body.errors` field on any HTTP response (i.e. no HubSpot validation errors).

If a turn fails, fix forward by inspecting the actual HTTP response body — the failure mode pages in the references explain how to recover. Do not silently skip.

- [ ] **Step 8: Final commit (only if you edited prompts you want to keep as defaults)**

If you want to leave the `prompt` set to a useful seed for future runs, leave it as the Turn 1 message. Otherwise revert to a generic placeholder. Then commit:

```bash
git add tests/graphs/advanced/hubspot/agent.json
git commit -m "test(hubspot): smoke-test 5-turn run against real HubSpot Private App"
```

---

## Self-review notes

**Spec coverage:** Every section of the spec maps to a task — auth (Task 1 step 2), 7 tools (Task 11 with one tool config block per spec entry), skill catalog (Task 2), 7 references with the spec's prescribed structure (Tasks 3–9), graph layout (Task 10–11), 5-turn testing plan (Task 12 mirrors the spec's testing plan).

**Placeholders:** None. Every code/markdown step shows complete content. Every command has its expected output stated.

**Type consistency:** Tool names in `agent.json` (`create_contact`, `update_contact`, `search_contacts`, `create_company`, `update_company`, `search_companies`, `associate_contact_company`) match reference filenames (`create_contact.md`, etc.) and SKILL.md catalog entries (`name: create_contact`, etc.) byte-for-byte. The body wrapper uses the same field name (`properties`) at every layer (HubSpot API field, tool LLM-visible param, HubSpot reference table headers).

**One known item to watch at run time:** the engine's behavior when a depth-2 LLM-visible `properties` param is merged under the `body` container. If the executor's `param_to_container` routing does not fold a top-level LLM param of name `properties` under `body.properties`, the request body would come out as `{ "<properties contents>" }` instead of `{ "properties": { "<properties contents>" } }`. Task 11 step 3 calls this out and tells the implementer to inspect logs to confirm the merge. If the merge is not what we need, the simplest fix is to declare body itself as a leaf object the LLM fills entirely (drop the container wrapper) — but only do that if the run actually exhibits the bug.
