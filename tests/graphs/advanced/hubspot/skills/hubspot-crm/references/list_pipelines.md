# list_pipelines

**Tool:** `list_pipelines`
**HTTP:** `GET /crm/v3/pipelines/deals`

Lists the deal pipelines for the portal and the stages inside each. **Call this before any `create_deal` or `update_deal` that sets `pipeline` or `dealstage`** — those values must be real ids from this response.

## What you provide (LLM-visible parameters)

None. The tool calls the endpoint as-is.

## Response shape (HTTP 200)

```json
{
  "results": [
    {
      "id": "default",
      "label": "Sales Pipeline",
      "displayOrder": 0,
      "stages": [
        { "id": "appointmentscheduled", "label": "Appointment Scheduled", "displayOrder": 0, "metadata": { "isClosed": "false", "probability": "0.2" } },
        { "id": "qualifiedtobuy",       "label": "Qualified To Buy",       "displayOrder": 1, "metadata": { "isClosed": "false", "probability": "0.4" } },
        { "id": "presentationscheduled","label": "Presentation Scheduled", "displayOrder": 2, "metadata": { "isClosed": "false", "probability": "0.6" } },
        { "id": "decisionmakerboughtin","label": "Decision Maker Bought-In","displayOrder": 3, "metadata": { "isClosed": "false", "probability": "0.8" } },
        { "id": "contractsent",         "label": "Contract Sent",           "displayOrder": 4, "metadata": { "isClosed": "false", "probability": "0.9" } },
        { "id": "closedwon",            "label": "Closed Won",              "displayOrder": 5, "metadata": { "isClosed": "true",  "probability": "1.0" } },
        { "id": "closedlost",           "label": "Closed Lost",             "displayOrder": 6, "metadata": { "isClosed": "true",  "probability": "0.0" } }
      ],
      "createdAt": "...",
      "updatedAt": "...",
      "archived": false
    }
  ]
}
```

## How to use the response

- The `results[].id` value is what you put into `properties.pipeline` on `create_deal` / `update_deal`.
- The `results[].stages[].id` is what you put into `properties.dealstage`. The stage MUST belong to the chosen pipeline — use the same pipeline's `stages` array.
- `metadata.isClosed: "true"` marks terminal stages (`closedwon`, `closedlost`); use these only when the deal is actually finished.
- `metadata.probability` is HubSpot's default close probability — informational only.

## Errors

- `403` if the Private App is missing `crm.pipelines.read` scope (HubSpot bundles this with the standard CRM read scopes; should not normally happen).

## When NOT to use

- Do not call `list_pipelines` repeatedly within the same conversation — once you have the pipeline/stage map, reuse it.
- This tool is for **deal** pipelines only. Ticket pipelines use a different endpoint (`/crm/v3/pipelines/tickets`) and are out of scope.
