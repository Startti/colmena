# create_contact

**Tool:** `create_contact`
**HTTP:** `POST /crm/v3/objects/contacts`

## What you provide (LLM-visible parameters)

- `properties` (object, required) — the contact fields. See the table below.

The tool builds the request body as `{ "properties": { ... } }` automatically.

## Properties

| Internal name | Type | Required | Example | Notes |
|---|---|---|---|---|
| `email` | string | yes | `"jane@acme.test"` | Used by HubSpot for dedup. Triggers `CONFLICT` if exists. |
| `firstname` | string | no | `"Jane"` | |
| `lastname` | string | no | `"Doe"` | |
| `phone` | string | no | `"+57 300 1234567"` | Free-form string; E.164 recommended. |
| `company` | string | no | `"Acme Corp"` | Plain text only. To link the contact to a company record use `associate_contact_company` after creating both. |
| `jobtitle` | string | no | `"VP of Engineering"` | |
| `website` | string | no | `"https://acme.test"` | |
| `lifecyclestage` | string | no | `"customer"` | One of: `subscriber`, `lead`, `marketingqualifiedlead`, `salesqualifiedlead`, `opportunity`, `customer`, `evangelist`, `other`. |

## Request body example

```json
{
  "properties": {
    "email": "jane@acme.test",
    "firstname": "Jane",
    "lastname": "Doe",
    "phone": "+57 300 1234567",
    "lifecyclestage": "lead"
  }
}
```

## Response shape (HTTP 201)

The `body` returned by the tool:

```json
{
  "id": "12345",
  "properties": { "email": "jane@acme.test", "firstname": "Jane" },
  "createdAt": "2026-05-08T10:00:00.000Z",
  "updatedAt": "2026-05-08T10:00:00.000Z",
  "archived": false
}
```

**Save `body.id`** — you need it for `update_contact` and `associate_contact_company`.

## Errors

- `409 CONFLICT` `Contact already exists` — the email is taken. Call `search_contacts` with `propertyName: "email", operator: "EQ", value: "<email>"` to find the existing id, then either `update_contact` or stop and ask the user.
- `400 VALIDATION_ERROR` — re-read the property table; check for typos in the internal name (e.g. `firstName` is invalid; `firstname` is correct).

## When NOT to use

- Do not call `create_contact` again to "update" a contact you already created — use `update_contact` with the saved `id`.
- Do not invent properties not in the table. Custom properties exist but are out of scope.
