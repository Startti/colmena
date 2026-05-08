# list_ticket_pipelines

**Tool:** `list_ticket_pipelines`
**HTTP:** `GET /crm/v3/pipelines/tickets`

Lists the ticket pipelines for the portal and the stages inside each. **Call this before any `create_ticket`** that sets `hs_pipeline` or `hs_pipeline_stage` — those values must be real ids from this response. Ticket pipelines are completely separate from deal pipelines.

## What you provide (LLM-visible parameters)

None. The tool calls the endpoint as-is.

## Response shape (HTTP 200)

```json
{
  "results": [
    {
      "id": "0",
      "label": "Support Pipeline",
      "displayOrder": 0,
      "stages": [
        { "id": "1", "label": "New",                 "displayOrder": 0, "metadata": { "ticketState": "OPEN" } },
        { "id": "2", "label": "Waiting on contact",  "displayOrder": 1, "metadata": { "ticketState": "OPEN" } },
        { "id": "3", "label": "Waiting on us",       "displayOrder": 2, "metadata": { "ticketState": "OPEN" } },
        { "id": "4", "label": "Closed",              "displayOrder": 3, "metadata": { "ticketState": "CLOSED" } }
      ],
      "createdAt": "...",
      "updatedAt": "...",
      "archived": false
    }
  ]
}
```

## How to use the response

- `results[].id` → put into `properties.hs_pipeline` on `create_ticket`.
- `results[].stages[].id` → put into `properties.hs_pipeline_stage`. The stage MUST belong to the chosen pipeline.
- `metadata.ticketState: "CLOSED"` marks terminal stages — use only when the ticket is actually resolved.

## Errors

- `403` if the Private App is missing the right scope (very unlikely — the standard CRM read scopes cover this).

## When NOT to use

- Don't call `list_ticket_pipelines` repeatedly within the same conversation — cache the result for the session.
- This is for **ticket** pipelines only. For deal pipelines use `list_pipelines`.
