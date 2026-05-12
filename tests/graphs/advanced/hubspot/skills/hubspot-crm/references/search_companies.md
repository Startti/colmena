# search_companies

**Tool:** `search_companies`
**HTTP:** `POST /crm/v3/objects/companies/search`

## What you provide (LLM-visible parameters)

- `body` (object, required) — the full JSON body. Shape:
  - `filterGroups` (array, required) — same structure as `search_contacts`.
  - `properties` (array, optional) — property names to return, e.g. `["name","domain","industry"]`.
  - `limit` (number, optional) — default 10, max 100.

## filterGroups structure

```json
[
  { "filters": [ { "propertyName": "<internal name>", "operator": "<OP>", "value": "<value>" } ] }
]
```

Filters **within a group are AND'd**; **groups (top-level array) are OR'd**.

### ⚠️ Common mistake — multi-criteria search

When the user asks for "Acme Corp in software" (name AND industry), put BOTH filters inside ONE group, not two groups. Two groups would mean OR.

**✅ Correct — name CONTAINS Acme AND industry=COMPUTER_SOFTWARE:**
```json
"filterGroups": [
  { "filters": [
      { "propertyName": "name",     "operator": "CONTAINS_TOKEN", "value": "Acme" },
      { "propertyName": "industry", "operator": "EQ",             "value": "COMPUTER_SOFTWARE" }
  ]}
]
```

**❌ Wrong — name CONTAINS Acme OR industry=COMPUTER_SOFTWARE (way too many results):**
```json
"filterGroups": [
  { "filters": [{ "propertyName": "name",     "operator": "CONTAINS_TOKEN", "value": "Acme" }] },
  { "filters": [{ "propertyName": "industry", "operator": "EQ",             "value": "COMPUTER_SOFTWARE" }] }
]
```

Use multiple groups ONLY for genuine OR semantics.

## Operators

Same as `search_contacts`: `EQ`, `NEQ`, `CONTAINS_TOKEN`, `HAS_PROPERTY`, `NOT_HAS_PROPERTY`, `GT`, `GTE`, `LT`, `LTE`, `BETWEEN`, `IN`.

## Request body example — find by domain

```json
{
  "filterGroups": [
    {
      "filters": [
        { "propertyName": "domain", "operator": "EQ", "value": "acme.test" }
      ]
    }
  ],
  "properties": ["name", "domain", "industry"],
  "limit": 1
}
```

## Request body example — find by name fragment

```json
{
  "filterGroups": [
    {
      "filters": [
        { "propertyName": "name", "operator": "CONTAINS_TOKEN", "value": "Acme" }
      ]
    }
  ],
  "properties": ["name", "domain"],
  "limit": 5
}
```

## Response shape (HTTP 200)

```json
{
  "total": 1,
  "results": [
    { "id": "67890", "properties": { "name": "Acme Corp", "domain": "acme.test" }, "createdAt": "...", "updatedAt": "...", "archived": false }
  ],
  "paging": { "next": { "after": "10" } }
}
```

**Save `body.results[].id`** for follow-up calls.

## Errors

- `400 VALIDATION_ERROR` — wrong `propertyName` or operator.
- Empty `results` is not an error.

## When NOT to use

- Do not call `search_companies` when you already have the id from a prior `create_company` in this conversation — reuse the id.
- Do not use it for analytics — `limit` is capped at 100.
