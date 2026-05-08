---
name: hubspot-crm
description: Use when calling HubSpot CRM tools — create/update/search contacts and companies, or associate a contact with a company. Load the reference for the specific operation you are about to call to get its property table, request/response shape, and error remedies.
references:
  - name: list_contacts
    description: GET /crm/v3/objects/contacts — paginated list of all contacts, no filters. Use for "show me what I have".
  - name: create_contact
    description: POST /crm/v3/objects/contacts — required/optional properties, dedup behavior on email.
  - name: update_contact
    description: PATCH /crm/v3/objects/contacts/{id} — partial updates, requires id from search/create.
  - name: search_contacts
    description: POST /crm/v3/objects/contacts/search — filterGroups, common operators, pagination.
  - name: list_companies
    description: GET /crm/v3/objects/companies — paginated list of all companies, no filters. Use for "show me what I have".
  - name: create_company
    description: POST /crm/v3/objects/companies — required/optional properties, dedup notes.
  - name: update_company
    description: PATCH /crm/v3/objects/companies/{id} — partial updates, requires id from search/create.
  - name: search_companies
    description: POST /crm/v3/objects/companies/search — filterGroups, operators, pagination.
  - name: list_deals
    description: GET /crm/v3/objects/deals — paginated list of all deals, no filters.
  - name: create_deal
    description: POST /crm/v3/objects/deals — required dealname; pipeline/dealstage IDs come from list_pipelines.
  - name: update_deal
    description: PATCH /crm/v3/objects/deals/{id} — partial updates (move stage, change amount, reassign owner).
  - name: search_deals
    description: POST /crm/v3/objects/deals/search — filterGroups, operators, pagination.
  - name: list_pipelines
    description: GET /crm/v3/pipelines/deals — list deal pipelines and their stage IDs. Call BEFORE create_deal/update_deal so you do not invent stage values.
  - name: list_owners
    description: GET /crm/v3/owners — list workspace users (HubSpot owners). Call before assigning hubspot_owner_id.
  - name: associate_contact_company
    description: PUT /crm/v4/.../associations/default/... — primary association, empty body, when NOT to use labeled associations.
  - name: associate_deal
    description: PUT /crm/v4/objects/deal/{dealId}/associations/default/{contact|company}/{id} — link a deal to a contact OR company; one call per direction.
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

- **Update / associate** require an `id`. **Never invent an id.** If you do not have one from a prior `create_*`, `search_*`, or `list_*` in this conversation, call the matching `search_*` (when you have a filter) or `list_*` (when you want a browse) first.
- For associate, you need both ids — search the contact and the company independently if you do not have them.

## List vs search

- Use `list_*` when the user wants to **browse** ("show me my contacts", "what companies do I have?"). No filter logic needed; results are paginated.
- Use `search_*` when the user gives **filtering criteria** ("find Jane by email", "companies in the financial sector"). Search supports `filterGroups` with operators.
- Both return the same per-record shape (`{ id, properties, ... }`), so once you have results, follow-up `update_*` and `associate_*` work the same way.

## Association type IDs

The `associate_contact_company` and `associate_deal` tools use the URL segment `/associations/default/`, which means HubSpot's primary association type. The body is empty. There is no need to pass a type id. Labeled associations (custom association types) are out of scope.

## Working with deals

A deal is HubSpot's object for a sales opportunity (one potential transaction). The typical flow:

1. **`list_pipelines`** first — returns the deal pipelines and the stage IDs for each. Without this you would have to guess `dealstage` values, and HubSpot rejects unknown stage IDs.
2. **`list_owners`** if you intend to assign a `hubspot_owner_id` (a real user in the workspace). Without this you would invent owner ids.
3. **`create_deal`** with at minimum `dealname`, plus `pipeline` and `dealstage` from step 1, optionally `amount`, `closedate`, `hubspot_owner_id`, `description`.
4. **`associate_deal`** — call once per related object: one call to link the deal to a contact, another to link it to a company. Both directions get auto-created by HubSpot.
5. **`update_deal`** when the stage advances, the amount changes, or the owner is reassigned.

Common deal property internal names: `dealname`, `pipeline`, `dealstage`, `amount`, `closedate`, `hubspot_owner_id`, `description`, `dealtype`.

## Tool naming and reference loading

Each tool name maps 1:1 to a reference in this skill (e.g. tool `create_contact` ↔ reference `create_contact`). Load the reference whenever you need the full property table, valid filter operators, or detailed error remedies. Simple, well-known calls (e.g. creating a contact with just an email) often do not need a reference load.
