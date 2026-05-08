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

```json
{
  "properties": {
    "lifecyclestage": "customer"
  }
}
```

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
