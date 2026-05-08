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

```json
{
  "properties": {
    "name": "Acme Corp",
    "domain": "acme.test",
    "industry": "COMPUTER_SOFTWARE",
    "city": "Bogota",
    "country": "Colombia"
  }
}
```

## Response shape (HTTP 201)

```json
{
  "id": "67890",
  "properties": { "name": "Acme Corp", "domain": "acme.test" },
  "createdAt": "2026-05-08T10:01:00.000Z",
  "updatedAt": "2026-05-08T10:01:00.000Z",
  "archived": false
}
```

**Save `body.id`** — required for `update_company` and `associate_contact_company`.

## Errors

- `400 VALIDATION_ERROR` — typically a typo in property name (e.g. `Name` vs `name`).
- HubSpot does NOT auto-error on duplicate domain by default — duplicates can be created. To avoid this, call `search_companies` by `domain EQ "<domain>"` first.

## When NOT to use

- Do not call `create_company` repeatedly for the same domain — search first when there is risk of duplicate.
- Do not use it to associate the company with a contact. Use `associate_contact_company`.
