# update_company

**Tool:** `update_company`
**HTTP:** `PATCH /crm/v3/objects/companies/{companyId}`

## What you provide (LLM-visible parameters)

- `endpoint` (string, required) — full path with the real company id, e.g. `/crm/v3/objects/companies/67890`. Get the id from a prior `create_company` or `search_companies`. **Never invent an id.**
- `properties` (object, required) — the fields to update (partial; only included fields change).

## Properties

Same names as `create_company` (see that reference for the full table). Any subset is valid:

| Internal name | Notes when updating |
|---|---|
| `name`, `domain`, `industry`, `phone`, `city`, `country`, `numberofemployees`, `description`, `website` | Free updates. |

## Request body example

To set the company's industry and number of employees:

```json
{
  "properties": {
    "industry": "FINANCIAL_SERVICES",
    "numberofemployees": "120"
  }
}
```

`endpoint` for the same call: `/crm/v3/objects/companies/67890`.

## Response shape (HTTP 200)

Same shape as `create_company` — `id`, full `properties`, `createdAt`, `updatedAt`.

## Errors

- `404 OBJECT_NOT_FOUND` — id is wrong or company was deleted. Run `search_companies` to refresh.
- `400 VALIDATION_ERROR` — typo in property name.

## When NOT to use

- Do not use `update_company` to attach a contact. Use `associate_contact_company`.
- Do not call without a verified id.
