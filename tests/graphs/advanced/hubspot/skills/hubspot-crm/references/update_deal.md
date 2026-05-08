# update_deal

**Tool:** `update_deal`
**HTTP:** `PATCH /crm/v3/objects/deals/{dealId}`

## What you provide (LLM-visible parameters)

- `endpoint` (string, required) — the full path with the real id substituted, e.g. `/crm/v3/objects/deals/98765`. Get the id from a prior `create_deal`, `search_deals`, or `list_deals`. **Never invent an id.**
- `properties` (object, required) — the fields to update (partial; only included fields change).

## Properties

Same names as `create_deal` (see that reference for the table). Common updates:

| Internal name | Notes when updating |
|---|---|
| `dealstage` | Must belong to the deal's current pipeline. Re-run `list_pipelines` if unsure. Moving forward (e.g. to `closedwon`) is fine; moving "backwards" depends on portal config. |
| `amount` | Free update (string). |
| `closedate` | ISO 8601 timestamp. |
| `hubspot_owner_id` | Must be a valid owner id from `list_owners`. |
| `dealname` | Free update. |
| `description` | Free update. |

## Request body example

To move the deal to closed-won and lock in the final amount:

```json
{
  "properties": {
    "dealstage": "closedwon",
    "amount": "7500"
  }
}
```

`endpoint` for the same call: `/crm/v3/objects/deals/98765`.

## Response shape (HTTP 200)

Same shape as `create_deal` — `id`, full updated `properties`, `createdAt`, `updatedAt`.

## Errors

- `404 OBJECT_NOT_FOUND` — id is wrong or deal was deleted. Run `search_deals` or `list_deals` to refresh.
- `400 VALIDATION_ERROR` on `dealstage` — the stage does not belong to this deal's pipeline. Look up valid stages with `list_pipelines`.

## When NOT to use

- Do not use `update_deal` to change the pipeline of a deal that has already moved through stages — HubSpot generally rejects pipeline changes once the deal has progressed. Verify by reading the response body.
- Do not use `update_deal` to associate a contact or company — use `associate_deal`.
