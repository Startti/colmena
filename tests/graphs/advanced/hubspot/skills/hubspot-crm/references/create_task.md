# create_task

**Tool:** `create_task`
**HTTP:** `POST /crm/v3/objects/tasks`

Creates a to-do for a HubSpot owner (a workspace user). Tasks appear on the assignee's HubSpot home and on the timeline of any associated contact/company/deal.

## What you provide (LLM-visible parameters)

- `body` (object, required) — the full JSON body. Shape:
  - `properties` (object, required) — see table below.
  - `associations` (array, recommended) — inline links to contacts/companies/deals.

## Properties

| Internal name | Type | Required | Example | Notes |
|---|---|---|---|---|
| `hs_task_subject` | string | yes | `"Llamar a Juan para cerrar contrato"` | Short title. |
| `hs_task_body` | string | no | `"Revisar términos del descuento antes de la llamada."` | Longer description (HTML OK). |
| `hs_task_status` | string | no | `"NOT_STARTED"` | One of: `NOT_STARTED`, `IN_PROGRESS`, `WAITING`, `COMPLETED`, `DEFERRED`. Default `NOT_STARTED`. |
| `hs_task_priority` | string | no | `"HIGH"` | One of: `LOW`, `MEDIUM`, `HIGH`. Default `MEDIUM`. |
| `hs_task_type` | string | no | `"CALL"` | One of: `TODO`, `CALL`, `EMAIL`. Default `TODO`. |
| `hs_timestamp` | string | yes | `"2026-05-15T14:00:00Z"` | **Due date** in ISO 8601. Required. |
| `hubspot_owner_id` | string | recommended | `"82854250"` | Who is assigned. Look up via `list_owners`. If omitted, HubSpot may leave the task unassigned. |

## Associations array

| Linking the task to | associationTypeId |
|---|---|
| Contact | 204 |
| Company | 192 |
| Deal | 216 |
| Ticket | 230 |

## Request body example — call task assigned to an owner, linked to a deal and a contact

```json
{
  "properties": {
    "hs_task_subject": "Llamar a Juan para cerrar el deal de POC",
    "hs_task_body": "Confirmar fecha de firma y enviar contrato.",
    "hs_task_type": "CALL",
    "hs_task_priority": "HIGH",
    "hs_task_status": "NOT_STARTED",
    "hs_timestamp": "2026-05-15T14:00:00Z",
    "hubspot_owner_id": "82854250"
  },
  "associations": [
    {
      "to": { "id": "60140413055" },
      "types": [{ "associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 216 }]
    },
    {
      "to": { "id": "220522880665" },
      "types": [{ "associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 204 }]
    }
  ]
}
```

## Response shape (HTTP 201)

```json
{
  "id": "67890",
  "properties": { "hs_task_subject": "...", "hs_task_body": "...", "hs_task_status": "NOT_STARTED", "hs_timestamp": "1747576800000", "hubspot_owner_id": "82854250", ... },
  "createdAt": "...",
  "updatedAt": "...",
  "archived": false
}
```

## Errors

- `400 VALIDATION_ERROR` `hs_timestamp is required` — set the due date.
- `400` on `hs_task_status` / `hs_task_priority` / `hs_task_type` — value must be one of the enums above.
- `400` on `hubspot_owner_id` — id does not exist; re-run `list_owners`.

## When NOT to use

- Don't use `create_task` for things the agent itself just did — those go in `add_note`.
- Don't use it for customer-facing tickets — those use `create_ticket`.
- Don't create tasks without a due date (`hs_timestamp` is required, and a missing date defeats the purpose of a to-do anyway).
