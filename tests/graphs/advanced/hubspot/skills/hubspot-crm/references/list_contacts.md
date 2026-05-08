# list_contacts

**Tool:** `list_contacts`
**HTTP:** `GET /crm/v3/objects/contacts`

Use this when the user wants to **browse** their contacts ("show me my contacts", "list the first 20"). For filtered queries (by email, name, etc.) use `search_contacts` instead.

## What you provide (LLM-visible parameters)

All optional. The tool sends them as URL query parameters.

| Param | Type | Notes |
|---|---|---|
| `limit` | number | Page size, 1–100. Default 10. |
| `after` | string | Pagination cursor. Pass the `paging.next.after` value from the previous response to fetch the next page. Omit on the first call. |
| `properties` | string | Comma-separated property names to include in each result, e.g. `"email,firstname,lastname,phone"`. Omit for HubSpot's default minimal set. |
| `archived` | boolean | If `true`, list archived (soft-deleted) contacts. Default `false`. |

## Request examples

First page, default properties:

```
GET /crm/v3/objects/contacts?limit=10
```

First page, specific properties:

```
GET /crm/v3/objects/contacts?limit=20&properties=email,firstname,lastname
```

Next page (using a cursor from a previous response):

```
GET /crm/v3/objects/contacts?limit=20&after=eyJpZCI6IjEyMyJ9
```

## Response shape (HTTP 200)

```json
{
  "results": [
    {
      "id": "12345",
      "properties": { "email": "jane@acme.test", "firstname": "Jane", "lastname": "Doe" },
      "createdAt": "2026-05-08T10:00:00.000Z",
      "updatedAt": "2026-05-08T10:00:00.000Z",
      "archived": false
    }
  ],
  "paging": {
    "next": {
      "after": "eyJpZCI6IjEyMyJ9",
      "link": "https://api.hubapi.com/crm/v3/objects/contacts?after=..."
    }
  }
}
```

- `results[]` may be empty if the portal has no contacts.
- `paging.next` is **absent** on the last page — that's how you know to stop.
- Save `results[].id` for follow-up `update_contact` or `associate_contact_company` calls.

## Errors

- `400 VALIDATION_ERROR` — usually a typo in `properties` (e.g. `firstName` instead of `firstname`).
- `429 RATE_LIMITS` — stop and inform the user. Do not retry in a loop.

## When NOT to use

- Do not use `list_contacts` to find a specific person — `search_contacts` with a filter is faster and more precise.
- Do not page beyond what the user actually asked for. If they say "show me 20 contacts", do not auto-paginate to fetch hundreds.
