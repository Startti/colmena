# create_ticket

**Tool:** `create_ticket`
**HTTP:** `POST /crm/v3/objects/tickets`

Creates a support ticket — used to record a customer-reported issue, request, or message that needs the team's attention. Tickets live in their own pipeline (separate from deal pipelines).

## What you provide (LLM-visible parameters)

- `body` (object, required) — the full JSON body. Shape:
  - `properties` (object, required) — see table below.
  - `associations` (array, recommended) — link to the contact and/or company who reported the issue.

## Prerequisites

Call **`list_ticket_pipelines`** first to get valid `hs_pipeline` and `hs_pipeline_stage` IDs — ticket pipelines are NOT the same as deal pipelines, and the stage IDs are different. Never invent them.

## Properties

| Internal name | Type | Required | Example | Notes |
|---|---|---|---|---|
| `subject` | string | yes | `"Cannot log in to the portal"` | Short title of the ticket. |
| `content` | string | no | `"Customer says they get a 500 error after entering correct credentials..."` | Detailed description. HTML allowed. |
| `hs_pipeline` | string | recommended | `"0"` | Ticket pipeline id from `list_ticket_pipelines.results[].id`. If omitted, HubSpot uses the portal's default. |
| `hs_pipeline_stage` | string | **yes** | `"1"` | **Required by HubSpot.** Stage id from `list_ticket_pipelines.results[].stages[].id`. **Must belong to the chosen pipeline.** Always call `list_ticket_pipelines` first. |
| `hs_ticket_priority` | string | no | `"HIGH"` | One of: `LOW`, `MEDIUM`, `HIGH`. |
| `hs_ticket_category` | string | no | `"BILLING_ISSUE"` | Common values: `GENERAL_INQUIRY`, `BILLING_ISSUE`, `FEATURE_REQUEST`, `PRODUCT_ISSUE`. Portal-customizable. |
| `source_type` | string | no | `"CHAT"` | Channel where the issue arrived: `CHAT`, `EMAIL`, `FORM`, `PHONE`. |
| `hubspot_owner_id` | string | no | `"82854250"` | Who handles it. Look up via `list_owners`. |

## Associations array

| Linking the ticket to | associationTypeId |
|---|---|
| Contact | 16 |
| Company | 26 |
| Deal | 28 |

## Request body example — high-priority issue from a known contact and company

```json
{
  "properties": {
    "subject": "No puedo iniciar sesión al portal",
    "content": "El cliente reporta error 500 al ingresar credenciales correctas. Pasa desde esta mañana.",
    "hs_pipeline": "0",
    "hs_pipeline_stage": "1",
    "hs_ticket_priority": "HIGH",
    "source_type": "CHAT"
  },
  "associations": [
    {
      "to": { "id": "220522880665" },
      "types": [{ "associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 16 }]
    },
    {
      "to": { "id": "54878892173" },
      "types": [{ "associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 26 }]
    }
  ]
}
```

## Response shape (HTTP 201)

```json
{
  "id": "12345",
  "properties": { "subject": "...", "content": "...", "hs_pipeline": "0", "hs_pipeline_stage": "1", "hs_ticket_priority": "HIGH", "createdate": "...", "hs_lastmodifieddate": "..." },
  "createdAt": "...",
  "updatedAt": "...",
  "archived": false
}
```

Save `body.id` if you intend to update the ticket later.

## Errors

- `400 VALIDATION_ERROR` on `hs_pipeline_stage` — the stage does not belong to the chosen `hs_pipeline`. Re-run `list_ticket_pipelines`.
- `400 VALIDATION_ERROR` `subject is required` — set a title.
- `400` on `associationTypeId` — wrong id for the object type. Re-check the table.

## When NOT to use

- Don't use `create_ticket` for internal tasks — those use `create_task`.
- Don't use it to record general activity — `add_note` is the right tool for that.
- Don't omit associations: a ticket without a contact or company is hard to act on later.
