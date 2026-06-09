//! `style_text` use case — content-addressed inline style update.
//!
//! Locates a substring inside the requested scope, then emits
//! `updateTextStyle` (character-level) and/or `updateParagraphStyle`
//! (paragraph-level for `heading_level`) requests.
//!
//! Style updates don't shift indices, so we don't need to sort hits.

use crate::gdocs::application::co_edit_guard::{run_guard, GuardContext};
use crate::gdocs::domain::{
    ChangeKind, ChangeRecord, DocsError, DocumentId, DocumentSnapshot, EditResult, OutlineEntry,
    ParagraphSnapshot, RgbColor, Scope, StylePatch, TabId,
};
use regex::RegexBuilder;

#[derive(Debug, Clone)]
pub struct StyleTextInput {
    pub find: String,
    pub style: StylePatch,
    pub scope: Scope,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub occurrence: Option<u32>,
    pub anchor: Option<String>,
    pub dry_run: bool,
}

pub async fn run(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: StyleTextInput,
) -> Result<EditResult, DocsError> {
    let guard = run_guard(ctx, doc_id, &input.scope).await?;
    let snap = &guard.snapshot;
    let scope = &guard.resolved_scope;

    let pattern = if input.whole_word {
        format!(r"\b{}\b", regex::escape(&input.find))
    } else {
        regex::escape(&input.find)
    };
    let re = RegexBuilder::new(&pattern)
        .case_insensitive(!input.case_sensitive)
        .build()
        .map_err(|e| DocsError::InvalidArgs(format!("regex: {e}")))?;

    let mut hits: Vec<(u32, usize, usize)> = Vec::new();
    for tab in &snap.tabs {
        if let Some(t) = &scope.tab_id {
            if tab.tab_id.as_ref() != Some(t) {
                continue;
            }
        }
        for p in &tab.paragraphs {
            if p.n < scope.paragraph_start || p.n > scope.paragraph_end {
                continue;
            }
            if let Some(a) = &input.anchor {
                if !p.text.contains(a) {
                    continue;
                }
            }
            for m in re.find_iter(&p.text) {
                hits.push((p.n, m.start(), m.end() - m.start()));
            }
        }
    }

    if let Some(n) = input.occurrence {
        if n == 0 || (n as usize) > hits.len() {
            return Err(DocsError::TextNotFound {
                find: input.find,
                fuzzy_suggestions: vec![],
            });
        }
        hits = vec![hits[(n - 1) as usize]];
    }
    if hits.is_empty() {
        return Err(DocsError::TextNotFound {
            find: input.find,
            fuzzy_suggestions: vec![],
        });
    }

    let (text_style_json, paragraph_style_json, text_fields, paragraph_fields) =
        build_style_payload(&input.style);

    let mut requests: Vec<serde_json::Value> = Vec::new();
    let mut changes: Vec<ChangeRecord> = Vec::new();
    for (n, off, len) in &hits {
        let p = lookup_para(snap, *n);
        let prefix_utf16 = utf16_len(&p.text[..*off]);
        let match_utf16 = utf16_len(&p.text[*off..*off + *len]);
        let start = p.start_index + prefix_utf16;
        let end = start + match_utf16;
        if !text_fields.is_empty() {
            requests.push(serde_json::json!({
                "updateTextStyle": {
                    "range": {"startIndex": start, "endIndex": end},
                    "textStyle": text_style_json.clone(),
                    "fields": text_fields.clone(),
                }
            }));
        }
        if !paragraph_fields.is_empty() {
            // Paragraph-level style applies to the WHOLE containing paragraph.
            requests.push(serde_json::json!({
                "updateParagraphStyle": {
                    "range": {"startIndex": p.start_index, "endIndex": p.end_index},
                    "paragraphStyle": paragraph_style_json.clone(),
                    "fields": paragraph_fields.clone(),
                }
            }));
        }
        changes.push(ChangeRecord {
            kind: ChangeKind::Style,
            paragraph: *n,
            before: Some(p.text[*off..*off + *len].to_string()),
            after: None,
            tab_id: lookup_tab_id(snap, *n),
        });
    }

    if input.dry_run {
        return Ok(EditResult {
            changes,
            revision_id_after: snap.revision_id.clone(),
            outline_snapshot: vec![],
            lossy_conversions: vec![],
            pending_human_changes_outside_scope: guard.soft_warnings,
        });
    }

    let result = ctx
        .client
        .batch_update(doc_id, requests, Some(&snap.revision_id))
        .await?;
    ctx.cache.invalidate(ctx.session_id, doc_id);
    ctx.revisions
        .put(ctx.session_id, doc_id, &result.revision_id_after)
        .await?;
    let fresh = ctx.client.get(doc_id).await?;
    ctx.cache.put(ctx.session_id, doc_id, fresh.clone());

    Ok(EditResult {
        changes,
        revision_id_after: result.revision_id_after,
        outline_snapshot: build_outline(&fresh),
        lossy_conversions: vec![],
        pending_human_changes_outside_scope: guard.soft_warnings,
    })
}

// ── Helpers ────────────────────────────────────────────────────────

/// Convert a `StylePatch` into
/// `(textStyle json, paragraphStyle json, textFields csv, paragraphFields csv)`.
fn build_style_payload(s: &StylePatch) -> (serde_json::Value, serde_json::Value, String, String) {
    let mut text = serde_json::Map::new();
    let mut para = serde_json::Map::new();
    let mut text_fields: Vec<&'static str> = Vec::new();
    let mut para_fields: Vec<&'static str> = Vec::new();

    if let Some(b) = s.bold {
        text.insert("bold".into(), b.into());
        text_fields.push("bold");
    }
    if let Some(i) = s.italic {
        text.insert("italic".into(), i.into());
        text_fields.push("italic");
    }
    if let Some(u) = s.underline {
        text.insert("underline".into(), u.into());
        text_fields.push("underline");
    }
    if let Some(s2) = s.strikethrough {
        text.insert("strikethrough".into(), s2.into());
        text_fields.push("strikethrough");
    }
    if let Some(sz) = s.font_size_pt {
        text.insert(
            "fontSize".into(),
            serde_json::json!({"magnitude": sz, "unit": "PT"}),
        );
        text_fields.push("fontSize");
    }
    if let Some(c) = s.foreground {
        text.insert("foregroundColor".into(), rgb_to_color(c));
        text_fields.push("foregroundColor");
    }
    if let Some(c) = s.background {
        text.insert("backgroundColor".into(), rgb_to_color(c));
        text_fields.push("backgroundColor");
    }
    if let Some(url) = &s.link {
        text.insert("link".into(), serde_json::json!({"url": url}));
        text_fields.push("link");
    }
    if let Some(h) = s.heading_level {
        para.insert(
            "namedStyleType".into(),
            serde_json::Value::String(h.as_api_str().to_string()),
        );
        para_fields.push("namedStyleType");
    }

    (
        serde_json::Value::Object(text),
        serde_json::Value::Object(para),
        text_fields.join(","),
        para_fields.join(","),
    )
}

fn rgb_to_color(c: RgbColor) -> serde_json::Value {
    serde_json::json!({
        "color": {"rgbColor": {"red": c.r, "green": c.g, "blue": c.b}}
    })
}

fn lookup_para(s: &DocumentSnapshot, n: u32) -> &ParagraphSnapshot {
    s.tabs
        .iter()
        .flat_map(|t| t.paragraphs.iter())
        .find(|p| p.n == n)
        .expect("paragraph exists")
}

fn lookup_tab_id(s: &DocumentSnapshot, n: u32) -> Option<TabId> {
    s.tabs
        .iter()
        .find(|t| t.paragraphs.iter().any(|p| p.n == n))
        .and_then(|t| t.tab_id.clone())
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

fn build_outline(s: &DocumentSnapshot) -> Vec<OutlineEntry> {
    s.tabs
        .iter()
        .flat_map(|t| {
            t.paragraphs.iter().map(|p| OutlineEntry {
                paragraph: p.n,
                tab_id: t.tab_id.clone(),
                kind: p.kind,
                text_preview: p.text.chars().take(80).collect(),
            })
        })
        .collect()
}
