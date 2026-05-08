# list_owners

**Tool:** `list_owners`
**HTTP:** `GET /crm/v3/owners`

Lists the workspace users (HubSpot "owners") who can be assigned as `hubspot_owner_id` on contacts, companies, deals, and tickets. **Call this before assigning an owner** — owner ids must be real.

## What you provide (LLM-visible parameters)

All optional. The tool sends them as URL query parameters.

| Param | Type | Notes |
|---|---|---|
| `limit` | number | Page size, 1–100. Default 10. |
| `after` | string | Pagination cursor from `paging.next.after`. Omit on first page. |
| `email` | string | Filter to a specific user's email — exact match. Useful when the user gives you a name like "Maria" and you want to confirm her HubSpot id. |
| `archived` | boolean | If `true`, include deactivated users. Default `false`. |

## Request examples

All active owners:

```
GET /crm/v3/owners?limit=50
```

Find one user by email:

```
GET /crm/v3/owners?email=daniel@startti.co
```

## Response shape (HTTP 200)

```json
{
  "results": [
    {
      "id": "82854250",
      "email": "daniel@startti.co",
      "firstName": "Daniel",
      "lastName": "Garcia",
      "userId": 38980805,
      "userIdIncludingInactive": 38980805,
      "createdAt": "...",
      "updatedAt": "...",
      "archived": false
    }
  ],
  "paging": { "next": { "after": "..." } }
}
```

The id you want is `results[].id` (a string, e.g. `"82854250"`). Use it as `hubspot_owner_id` on create/update calls.

## Errors

- `400 VALIDATION_ERROR` — usually a malformed `email` filter.
- Empty `results` when filtering by email means there is no user with that email; do NOT invent an id.

## When NOT to use

- Do not call `list_owners` repeatedly within the same conversation — cache the id for the duration of the session.
- The id from `list_owners` is the **owner id** (`results[].id`), not the `userId` field. They are different. Always pass the `id`.
