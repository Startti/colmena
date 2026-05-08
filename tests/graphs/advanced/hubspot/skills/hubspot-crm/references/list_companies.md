# list_companies

**Tool:** `list_companies`
**HTTP:** `GET /crm/v3/objects/companies`

Use this when the user wants to **browse** their companies ("show me my companies", "list the first 20"). For filtered queries (by domain, name, etc.) use `search_companies` instead.

## What you provide (LLM-visible parameters)

All optional. The tool sends them as URL query parameters.

| Param | Type | Notes |
|---|---|---|
| `limit` | number | Page size, 1–100. Default 10. |
| `after` | string | Pagination cursor. Pass the `paging.next.after` value from the previous response to fetch the next page. Omit on the first call. |
| `properties` | string | Comma-separated property names to include, e.g. `"name,domain,industry,city"`. Omit for HubSpot's default minimal set. |
| `archived` | boolean | If `true`, list archived (soft-deleted) companies. Default `false`. |

## Request examples

First page, default properties:

```
GET /crm/v3/objects/companies?limit=10
```

First page, specific properties:

```
GET /crm/v3/objects/companies?limit=20&properties=name,domain,industry
```

Next page (using a cursor from a previous response):

```
GET /crm/v3/objects/companies?limit=20&after=eyJpZCI6IjY3OSJ9
```

## Response shape (HTTP 200)

```json
{
  "results": [
    {
      "id": "67890",
      "properties": { "name": "Acme Corp", "domain": "acme.test", "industry": "COMPUTER_SOFTWARE" },
      "createdAt": "2026-05-08T10:01:00.000Z",
      "updatedAt": "2026-05-08T10:01:00.000Z",
      "archived": false
    }
  ],
  "paging": {
    "next": {
      "after": "eyJpZCI6IjY3OSJ9",
      "link": "https://api.hubapi.com/crm/v3/objects/companies?after=..."
    }
  }
}
```

- `results[]` may be empty if the portal has no companies.
- `paging.next` is **absent** on the last page — that's how you know to stop.
- Save `results[].id` for follow-up `update_company` or `associate_contact_company` calls.

## Errors

- `400 VALIDATION_ERROR` — usually a typo in `properties` (e.g. `Domain` instead of `domain`).
- `429 RATE_LIMITS` — stop and inform the user. Do not retry in a loop.

## When NOT to use

- Do not use `list_companies` to find a specific company by domain — `search_companies` with `domain EQ` is precise.
- Do not auto-paginate beyond what the user asked for.
