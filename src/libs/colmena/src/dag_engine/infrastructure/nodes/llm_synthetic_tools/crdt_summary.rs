//! Builds the "Recent changes since your last turn" block that
//! llm.rs prepends to the system_message when a `crdt_documents`
//! context is configured.
//!
//! Also exports `CRDT_SPREADSHEET_PROTOCOL_PRELUDE` — a short operating
//! manual auto-injected into the system_message whenever crdt_documents
//! is configured. It teaches the agent how to behave with naive,
//! non-technical user prompts (when to discover, when to load skills,
//! when to clarify, where to persist results) without requiring the
//! user to know any of the tool names.

use crate::crdt_documents::change_tracker_store::StoredEvent;
use std::collections::HashMap;

use super::crdt_doc_context::CrdtDocsContext;

/// Operating manual injected into the system_message whenever a
/// `crdt_documents` config block is present. Designed so a user who
/// only says "compará los dos Q y mostrame diferencias" — no tool
/// names, no `write_to_sheet`, no "patrón B" — still gets the right
/// behavior. Skills are loaded lazily by reference to avoid bloating
/// every turn with full pattern catalogs.
pub const CRDT_SPREADSHEET_PROTOCOL_PRELUDE: &str = "## CRDT Spreadsheet Protocol\n\
You have collaborative-spreadsheet tools (`crdt_doc_*`). The user is non-technical \
and will describe what they want in natural language. They do NOT know tool names, \
sheet ids, or which \"pattern\" to apply — translate their words to tool calls yourself.\n\n\
**Always**:\n\
1. **DISCOVER first.** On the first turn that touches a workbook, call \
`crdt_doc_list_sheets` (current artifact) and `crdt_doc_list_my_artifacts` \
(other workbooks in this session). If the user mentions other workbooks \
(\"Q4\", \"el catálogo\", \"el reporte anterior\"), call \
`crdt_doc_list_sheets_of` on each to peek before deciding.\n\
2. **LOAD skills lazily.** Before writing any pandas code, \
`load_skill('crdt-doc-run-python')`. For compare/join/enrich across sheets, \
`load_skill('crdt-doc-cross-sheet-analysis')`. Each skill's index lists \
specific references — load ONLY the reference you need \
(e.g. `reference='pattern-b-row-diff'` or `reference='dataframe-shape'`). \
This saves tokens.\n\
3. **CLARIFY before acting on ambiguity.** Ask the user once if anything matters \
for correctness: the key column (Producto / SKU / ID?), the output destination \
(new sheet vs chat answer), the scope (which rows / which dates). Don't ask about \
internals (sheet ids, tool names) — figure those out yourself.\n\
4. **PERSIST derived results as new sheets** via `write_to_sheet`. Short summaries \
(\"se encontraron N items\") go in chat. Anything tabular goes to a sheet so the \
user keeps it.\n\
5. **NAME results in the user's language.** \"Resumen Q3\", \"Diferencias Q3 vs Q4\", \
not \"Output 1\" or \"diff_result\".\n\
6. **CROSS-ARTIFACT pattern**: when the user wants something that involves another \
workbook, the canonical flow is `list_sheets_of` → `import_sheet` (clones the sheet \
into the current workbook) → `run_python` with both `sheet_ids`. You don't need \
to ask the user permission to import — that's the only way to combine data, just \
do it and tell them what you did.\n\n\
The user's words → your job is the translation, not a syntax check.";

const MAX_SHEETS_IN_SUMMARY: usize = 10;
const MAX_EVENTS_TO_FETCH: u32 = 200;

/// Returns the block text or `None` if no events to surface.
pub async fn build_recent_changes_block(ctx: &CrdtDocsContext) -> Option<String> {
    let session_id = ctx.session_id()?;
    let cursor = ctx
        .backend()
        .cursor_for(session_id, ctx.artifact_id())
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    let own_origin = format!("agent:{session_id}");
    let events = ctx
        .backend()
        .events_since(
            ctx.artifact_id(),
            cursor,
            None,
            Some(&own_origin),
            MAX_EVENTS_TO_FETCH,
        )
        .await
        .ok()?;
    if events.is_empty() {
        return None;
    }
    Some(format_block(&events))
}

fn format_block(events: &[StoredEvent]) -> String {
    let mut buckets: HashMap<(Option<String>, String), u32> = HashMap::new();
    let peers: std::collections::HashSet<String> =
        events.iter().map(|e| e.origin.clone()).collect();
    for e in events {
        *buckets
            .entry((e.sheet_id.clone(), e.origin.clone()))
            .or_insert(0) += 1;
    }
    let mut lines: Vec<(String, String, u32)> = buckets
        .into_iter()
        .map(|((sheet, origin), n)| {
            let label = match sheet {
                Some(s) => s,
                None => "Workbook (sheet unknown)".to_string(),
            };
            (label, origin, n)
        })
        .collect();
    lines.sort_by_key(|l| std::cmp::Reverse(l.2)); // descending by count
    let total_lines = lines.len();
    let mut out = String::new();
    out.push_str("\n---\n");
    out.push_str(&format!(
        "Workbook changes since your last turn ({} events, {} peer{}):\n",
        events.len(),
        peers.len(),
        if peers.len() == 1 { "" } else { "s" }
    ));
    for (label, origin, n) in lines.iter().take(MAX_SHEETS_IN_SUMMARY) {
        out.push_str(&format!(
            "- {label}: {n} change{} by {origin}\n",
            if *n == 1 { "" } else { "s" }
        ));
    }
    if total_lines > MAX_SHEETS_IN_SUMMARY {
        out.push_str(&format!(
            "- ...and {} more sheet/peer groups changed\n",
            total_lines - MAX_SHEETS_IN_SUMMARY
        ));
    }
    out.push_str("Use `crdt_doc_get_recent_changes(sheet_id?)` for cell-level detail.\n---\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: u64, sheet: Option<&str>, origin: &str, summary: &str) -> StoredEvent {
        StoredEvent {
            id,
            artifact_id: "art_x".into(),
            sheet_id: sheet.map(String::from),
            origin: origin.to_string(),
            summary: summary.to_string(),
            created_at: "now".into(),
        }
    }

    #[test]
    fn format_block_with_empty_events_shows_zero() {
        let block = format_block(&[]);
        assert!(block.contains("0 events"));
    }

    #[test]
    fn single_sheet_single_peer() {
        let evs = vec![
            ev(1, Some("Inventory"), "peer:browser", "x"),
            ev(2, Some("Inventory"), "peer:browser", "y"),
            ev(3, Some("Inventory"), "peer:browser", "z"),
        ];
        let block = format_block(&evs);
        assert!(block.contains("3 events, 1 peer"));
        assert!(block.contains("Inventory: 3 changes by peer:browser"));
    }

    #[test]
    fn two_sheets_two_peers() {
        let evs = vec![
            ev(1, Some("Inventory"), "peer:browser", "x"),
            ev(2, Some("Inventory"), "peer:browser", "y"),
            ev(3, Some("Inventory"), "peer:browser", "z"),
            ev(4, Some("Pricing"), "agent:orchestrator", "a"),
            ev(5, Some("Pricing"), "agent:orchestrator", "b"),
        ];
        let block = format_block(&evs);
        assert!(block.contains("5 events, 2 peers"));
        assert!(block.contains("Inventory: 3 changes by peer:browser"));
        assert!(block.contains("Pricing: 2 changes by agent:orchestrator"));
    }

    #[test]
    fn workbook_level_when_sheet_unknown() {
        let evs = vec![
            ev(1, None, "peer:browser", "coarse"),
            ev(2, None, "peer:browser", "coarse"),
        ];
        let block = format_block(&evs);
        assert!(block.contains("Workbook (sheet unknown): 2 changes by peer:browser"));
    }

    #[test]
    fn caps_at_max_sheets_with_overflow_marker() {
        let evs: Vec<StoredEvent> = (0..15)
            .map(|i| ev(i, Some(&format!("Sheet{i}")), "peer:browser", "x"))
            .collect();
        let block = format_block(&evs);
        assert!(block.contains("...and 5 more sheet/peer groups changed"));
    }
}
