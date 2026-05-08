# associate_deal

**Tool:** `associate_deal`
**HTTP:** `PUT /crm/v4/objects/deal/{dealId}/associations/default/{contact|company}/{toId}`

Links a deal to a contact OR a company using HubSpot's default (primary) association type. **One call per direction**: if the deal needs to be linked to BOTH a contact and a company, call this tool twice (once per object type).

## What you provide (LLM-visible parameters)

- `endpoint` (string, required) — full path with both real ids substituted and the right object type. **Both ids must come from prior `search_*` / `create_*` / `list_*` results — never invent.** The body is empty (the tool fixes it to `{}`).

## Endpoint construction

| Segment | Value |
|---|---|
| Object type from | `deal` (literal — the v4 endpoint expects the lowercase name) |
| `{dealId}` | id of the deal |
| Object type to | `contact` OR `company` |
| `{toId}` | id of the contact or company |

Examples:

- Link deal to a contact: `/crm/v4/objects/deal/98765/associations/default/contact/220522880665`
- Link deal to a company: `/crm/v4/objects/deal/98765/associations/default/company/54878892173`

## Response shape (HTTP 200)

```json
{
  "completedAt": "2026-05-08T17:33:40.144Z",
  "status": "COMPLETE",
  "startedAt": "2026-05-08T17:33:40.061Z",
  "results": [
    { "from": { "id": "<companyOrContactId>" }, "to": { "id": "<dealId>" }, "associationSpec": { "associationCategory": "HUBSPOT_DEFINED", "associationTypeId": <int> } },
    { "from": { "id": "<dealId>" }, "to": { "id": "<companyOrContactId>" }, "associationSpec": { "associationCategory": "HUBSPOT_DEFINED", "associationTypeId": <int> } }
  ]
}
```

Both directions are created automatically by HubSpot — you do NOT need to call this tool again with swapped arguments.

## Errors

- `404 OBJECT_NOT_FOUND` — one of the ids does not exist. Re-search.
- `400` — malformed path. Check the segment order: `deal/{dealId}/associations/default/{contact|company}/{toId}`. The `default` segment is literal; do not replace it with anything.

## When NOT to use

- Do not pass a body. The default association does not accept type id in the body.
- Labeled (custom-typed) associations are out of scope.
- To associate a contact with a company (no deal involved), use `associate_contact_company`. This tool is only for deal↔contact and deal↔company.
