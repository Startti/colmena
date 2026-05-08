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

```json
{ "properties": { "<internal_name>": "<value>", ... } }
```

The `properties` object is what the LLM provides. The wrapper is built by the tool config. Keys inside `properties` use HubSpot's internal property names (lowercase, no spaces).

Common contact internal names: `email`, `firstname`, `lastname`, `phone`, `company`, `lifecyclestage`, `jobtitle`, `website`.
Common company internal names: `name`, `domain`, `industry`, `phone`, `city`, `country`, `numberofemployees`, `description`.

## Error envelope

HubSpot 4xx/5xx responses have this shape:

```json
{
  "status": "error",
  "message": "Property values were not valid: ...",
  "category": "VALIDATION_ERROR",
  "errors": [ { "message": "...", "in": "properties.email" } ]
}
```

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
