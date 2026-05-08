# search_deals

**Tool:** `search_deals`
**HTTP:** `POST /crm/v3/objects/deals/search`

## What you provide (LLM-visible parameters)

- `body` (object, required) — the full JSON body. Shape:
  - `filterGroups` (array, required) — same structure as `search_contacts`.
  - `properties` (array, optional) — property names to return, e.g. `["dealname","amount","dealstage","hubspot_owner_id"]`.
  - `limit` (number, optional) — default 10, max 100.

## filterGroups structure

```json
[
  { "filters": [ { "propertyName": "<internal name>", "operator": "<OP>", "value": "<value>" } ] }
]
```

Filters within a group are AND'd; groups are OR'd.

## Operators

Same as `search_contacts`: `EQ`, `NEQ`, `CONTAINS_TOKEN`, `HAS_PROPERTY`, `NOT_HAS_PROPERTY`, `GT`, `GTE`, `LT`, `LTE`, `BETWEEN`, `IN`.

## Request body example — open deals owned by a user

```json
{
  "filterGroups": [
    {
      "filters": [
        { "propertyName": "hubspot_owner_id", "operator": "EQ", "value": "82854250" },
        { "propertyName": "dealstage", "operator": "NEQ", "value": "closedwon" },
        { "propertyName": "dealstage", "operator": "NEQ", "value": "closedlost" }
      ]
    }
  ],
  "properties": ["dealname", "amount", "dealstage", "closedate"],
  "limit": 50
}
```

## Request body example — deals over a threshold

```json
{
  "filterGroups": [
    { "filters": [ { "propertyName": "amount", "operator": "GT", "value": "10000" } ] }
  ],
  "properties": ["dealname", "amount", "hubspot_owner_id"],
  "limit": 20
}
```

## Response shape (HTTP 200)

```json
{
  "total": 1,
  "results": [
    { "id": "98765", "properties": { "dealname": "...", "amount": "5000", "dealstage": "qualifiedtobuy" }, "createdAt": "...", "updatedAt": "...", "archived": false }
  ],
  "paging": { "next": { "after": "..." } }
}
```

Save `results[].id` for follow-up calls.

## Errors

- `400 VALIDATION_ERROR` — wrong `propertyName` or operator.
- Empty `results` is not an error.

## When NOT to use

- Do not use `search_deals` when you already have the id from a prior `create_deal` or `list_deals` in this conversation.
