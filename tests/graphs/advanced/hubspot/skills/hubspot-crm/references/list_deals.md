# list_deals

**Tool:** `list_deals`
**HTTP:** `GET /crm/v3/objects/deals`

Use this when the user wants to **browse** their deals ("show me my open deals", "list the first 20"). For filtered queries (by stage, amount, owner) use `search_deals` instead.

## What you provide (LLM-visible parameters)

All optional. The tool sends them as URL query parameters.

| Param | Type | Notes |
|---|---|---|
| `limit` | number | Page size, 1–100. Default 10. |
| `after` | string | Pagination cursor from the previous response's `paging.next.after`. Omit on the first call. |
| `properties` | string | Comma-separated property names to include, e.g. `"dealname,amount,dealstage,closedate,hubspot_owner_id"`. Omit for HubSpot's default minimal set. |
| `archived` | boolean | If `true`, list archived (soft-deleted) deals. Default `false`. |

## Request examples

```
GET /crm/v3/objects/deals?limit=20&properties=dealname,amount,dealstage,closedate
```

## Response shape (HTTP 200)

```json
{
  "results": [
    {
      "id": "12345",
      "properties": { "dealname": "Acme expansion Q3", "amount": "5000", "dealstage": "qualifiedtobuy", "closedate": "2026-08-31T00:00:00Z" },
      "createdAt": "...",
      "updatedAt": "...",
      "archived": false
    }
  ],
  "paging": { "next": { "after": "..." } }
}
```

`paging.next` is absent on the last page. Save `results[].id` for follow-up `update_deal` or `associate_deal` calls.

## Errors

- `400 VALIDATION_ERROR` — typo in `properties` (e.g. `Amount` vs `amount`).
- `429 RATE_LIMITS` — stop and inform the user.

## When NOT to use

- Do not use `list_deals` to find a specific deal — `search_deals` with a filter is precise.
- Do not auto-paginate beyond what the user asked for.
