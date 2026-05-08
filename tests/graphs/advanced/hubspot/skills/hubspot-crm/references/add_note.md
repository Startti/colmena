# add_note

**Tool:** `add_note`
**HTTP:** `POST /crm/v3/objects/notes`

Logs a note on the timeline of a contact, company, deal, or ticket. Use this to leave a footprint of important context — what the agent did, what the user said, or anything that the sales/support team should see when they open the record.

## What you provide (LLM-visible parameters)

- `body` (object, required) — the full JSON body. Shape:
  - `properties` (object, required) — the note fields (see table below).
  - `associations` (array, recommended) — inline links to one or more contacts/companies/deals. Without this, the note is orphaned and won't appear on any timeline.

## Properties

| Internal name | Type | Required | Example | Notes |
|---|---|---|---|---|
| `hs_note_body` | string | yes | `"Customer asked for a 10% discount on renewal."` | The text of the note. HTML allowed (e.g. `<p>...</p>`, `<a href>`). |
| `hs_timestamp` | string | yes | `"2026-05-08T19:30:00Z"` | ISO 8601 timestamp of when the note happened. Required by HubSpot. |

## Associations array

Each entry: `{ to: { id }, types: [{ associationCategory, associationTypeId }] }`. Use `associationCategory: "HUBSPOT_DEFINED"` and these IDs:

| Linking the note to | associationTypeId |
|---|---|
| Contact | 202 |
| Company | 190 |
| Deal | 214 |
| Ticket | 228 |

You can include multiple entries to attach the note to several objects at once.

## Request body example — note attached to one contact and one deal

```json
{
  "properties": {
    "hs_note_body": "Cliente pidió 10% de descuento en la renovación; le ofrecí 5%. Pendiente respuesta.",
    "hs_timestamp": "2026-05-08T19:30:00Z"
  },
  "associations": [
    {
      "to": { "id": "220522880665" },
      "types": [{ "associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 202 }]
    },
    {
      "to": { "id": "60140413055" },
      "types": [{ "associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 214 }]
    }
  ]
}
```

## Response shape (HTTP 201)

```json
{
  "id": "12345",
  "properties": { "hs_note_body": "...", "hs_timestamp": "...", "hs_createdate": "...", "hubspot_owner_id": "..." },
  "createdAt": "...",
  "updatedAt": "...",
  "archived": false
}
```

The note id (`body.id`) is rarely needed downstream — notes are usually fire-and-forget.

## Errors

- `400 VALIDATION_ERROR` `hs_timestamp is required` — you forgot the timestamp.
- `400` on `associationTypeId` — wrong id for the object type. Re-check the table.

## When NOT to use

- Don't use `add_note` to create tasks — tasks have their own object (`create_task`) with a due date and assignee.
- Don't use it to send messages to the customer — notes are internal only.
- Don't store secrets, passwords, or PII you wouldn't want the whole sales team to read.
