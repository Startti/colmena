# search_contacts

**Tool:** `search_contacts`
**HTTP:** `POST /crm/v3/objects/contacts/search`

## What you provide (LLM-visible parameters)

- `body` (object, required) — the full JSON body for the search. The tool sends it verbatim. Shape:
  - `filterGroups` (array, required) — see structure and operators below.
  - `properties` (array, optional) — names of the properties you want returned, e.g. `["email","firstname","lastname","phone"]`. Default minimal set if omitted.
  - `limit` (number, optional) — max results. Default 10, max 100.

## filterGroups structure

```json
[
  {
    "filters": [
      { "propertyName": "<internal name>", "operator": "<OP>", "value": "<value>" }
    ]
  }
]
```

- Filters within a single group are AND'd.
- Filter groups (top-level array) are OR'd.

## Operators

| Operator | Use when | Value field |
|---|---|---|
| `EQ` | Exact match (most common) | string |
| `NEQ` | Not equal | string |
| `CONTAINS_TOKEN` | Substring tokens (whitespace-tokenized) | string |
| `HAS_PROPERTY` | Property is set (any value) | omit `value` |
| `NOT_HAS_PROPERTY` | Property is empty | omit `value` |
| `GT` / `GTE` / `LT` / `LTE` | Numeric or date comparisons | string-encoded number/timestamp |
| `BETWEEN` | Range | use `value` AND `highValue` |
| `IN` | Match any of a list | use `values` (array) instead of `value` |

## Request body example — find a contact by email

```json
{
  "filterGroups": [
    {
      "filters": [
        { "propertyName": "email", "operator": "EQ", "value": "jane@acme.test" }
      ]
    }
  ],
  "properties": ["email", "firstname", "lastname", "phone"],
  "limit": 1
}
```

## Request body example — find contacts associated with a company

```json
{
  "filterGroups": [
    {
      "filters": [
        { "propertyName": "associatedcompanyid", "operator": "EQ", "value": "<companyId>" }
      ]
    }
  ],
  "properties": ["email", "firstname", "lastname"],
  "limit": 50
}
```

## Response shape (HTTP 200)

```json
{
  "total": 1,
  "results": [
    {
      "id": "12345",
      "properties": { "email": "jane@acme.test", "firstname": "Jane" },
      "createdAt": "...",
      "updatedAt": "...",
      "archived": false
    }
  ],
  "paging": { "next": { "after": "10", "link": "..." } }
}
```

**Save `body.results[].id`** for follow-up `update_*` or `associate_*` calls.

## Errors

- `400 VALIDATION_ERROR` — wrong `propertyName` or operator. Re-read the operator table.
- Empty `results` and `total: 0` are not errors — they mean "no match".

## When NOT to use

- Do not chain multiple `search_contacts` calls when one filter group with multiple AND'd filters works.
- Do not use this for analytics/list-everything — `limit` is capped at 100. For larger sets you would page via `paging.next.after`, but that is rarely needed in conversational flows.
