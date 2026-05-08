# create_deal

**Tool:** `create_deal`
**HTTP:** `POST /crm/v3/objects/deals`

## What you provide (LLM-visible parameters)

- `properties` (object, required) — the deal fields. See the table below.

The tool builds the request body as `{ "properties": { ... } }` automatically.

## Prerequisites

Before calling `create_deal`, run **`list_pipelines`** so you know the valid `pipeline` and `dealstage` IDs for this portal — never invent them. If you also intend to set `hubspot_owner_id`, run **`list_owners`** first.

## Properties

| Internal name | Type | Required | Example | Notes |
|---|---|---|---|---|
| `dealname` | string | yes | `"Acme expansion Q3"` | Display name. |
| `pipeline` | string | no | `"default"` | Pipeline id from `list_pipelines`. If omitted, HubSpot uses the portal's default pipeline. |
| `dealstage` | string | no | `"qualifiedtobuy"` | Stage id from `list_pipelines.results[].stages[].id`. **Must belong to the chosen pipeline.** If omitted, HubSpot uses the first stage of the chosen pipeline. |
| `amount` | string | no | `"5000"` | Monetary amount as a string. Currency is the portal's default unless `deal_currency_code` is set. |
| `closedate` | string | no | `"2026-08-31T00:00:00Z"` | ISO 8601 timestamp of the expected close. |
| `hubspot_owner_id` | string | no | `"82854250"` | Owner id from `list_owners`. |
| `description` | string | no | `"Renewal + 3 new seats."` | Free text. |
| `dealtype` | string | no | `"newbusiness"` | Common values: `newbusiness`, `existingbusiness`. |

## Request body example

```json
{
  "properties": {
    "dealname": "Acme expansion Q3",
    "pipeline": "default",
    "dealstage": "qualifiedtobuy",
    "amount": "5000",
    "closedate": "2026-08-31T00:00:00Z",
    "hubspot_owner_id": "82854250"
  }
}
```

## Response shape (HTTP 201)

```json
{
  "id": "98765",
  "properties": { "dealname": "Acme expansion Q3", "amount": "5000", "dealstage": "qualifiedtobuy", "pipeline": "default", "createdate": "...", "hs_lastmodifieddate": "..." },
  "createdAt": "...",
  "updatedAt": "...",
  "archived": false
}
```

**Save `body.id`** — required for `associate_deal`, `update_deal`, and any reference to this deal.

## Errors

- `400 VALIDATION_ERROR` `Property values were not valid: ... pipeline stage ... is not valid` — the `dealstage` does not belong to the chosen `pipeline`. Re-run `list_pipelines` and pick a stage from the right pipeline's `stages` array.
- `400 VALIDATION_ERROR` on `pipeline` — pipeline id is wrong; re-check `list_pipelines.results[].id`.
- `400 VALIDATION_ERROR` on `hubspot_owner_id` — owner id does not exist; re-run `list_owners`.

## When NOT to use

- Do not use `create_deal` to update an existing deal — use `update_deal` with the saved id.
- Do not invent `dealstage` or `hubspot_owner_id` values; always look them up first.
- Inline associations via the body's top-level `associations` field are NOT supported by this tool — use `associate_deal` after creating the deal.
