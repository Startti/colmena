# associate_contact_company

**Tool:** `associate_contact_company`
**HTTP:** `PUT /crm/v4/objects/contact/{contactId}/associations/default/company/{companyId}`

## What you provide (LLM-visible parameters)

- `endpoint` (string, required) — full path with both real ids substituted. Example: `/crm/v4/objects/contact/12345/associations/default/company/67890`. **Both ids must come from prior `search_*` or `create_*` results — never invent.**

The body is empty (the tool fixes it to `{}`). The `default` URL segment selects HubSpot's primary association type, so no type id needs to be passed.

## Endpoint construction

| Segment | Value |
|---|---|
| Object type from | `contact` (literal — the v4 endpoint expects the lowercase name) |
| `{contactId}` | id of the contact |
| Object type to | `company` |
| `{companyId}` | id of the company |

So with `contactId = 12345` and `companyId = 67890`:

```
/crm/v4/objects/contact/12345/associations/default/company/67890
```

## Response shape (HTTP 200)

```json
{
  "fromObjectTypeId": "0-1",
  "fromObjectId": "12345",
  "toObjectTypeId": "0-2",
  "toObjectId": "67890",
  "labels": []
}
```

`labels` is empty for the default (primary) association.

## Errors

- `404 OBJECT_NOT_FOUND` — one of the ids does not exist. Re-search.
- `400` — malformed path. Re-check the segment order: it is `contact/{id}/associations/default/company/{id}`, not the reverse.

## When NOT to use

- Do not pass a body. The default association does not accept type id in the body.
- Labeled (custom-typed) associations are out of scope. They use a different endpoint with a body containing `associationCategory` and `associationTypeId` — not supported by this tool.
- The reverse direction (company-to-contact) is created automatically by HubSpot when you create the contact-to-company default association. Do not call this tool again with swapped arguments.
