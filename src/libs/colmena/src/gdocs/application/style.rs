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
        let para_tab = lookup_tab_id(snap, *n);
        if !text_fields.is_empty() {
            requests.push(serde_json::json!({
                "updateTextStyle": {
                    "range": crate::gdocs::application::util::range(start, end, &para_tab),
                    "textStyle": text_style_json.clone(),
                    "fields": text_fields.clone(),
                }
            }));
        }
        if !paragraph_fields.is_empty() {
            // Paragraph-level style applies to the WHOLE containing paragraph.
            requests.push(serde_json::json!({
                "updateParagraphStyle": {
                    "range": crate::gdocs::application::util::range(
                        p.start_index, p.end_index, &para_tab,
                    ),
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
    // CRITICAL: store the snapshot's revision_id (documents.get format)
    // — not batch_update's writeControl id — so the next guard's
    // revisionId comparison succeeds. See replace_text.rs for the full
    // explanation.
    ctx.cache.invalidate(ctx.session_id, doc_id);
    let fresh = ctx.client.get(doc_id).await?;
    ctx.revisions
        .put(ctx.session_id, doc_id, &fresh.revision_id)
        .await?;
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

#[cfg(test)]
mod app_tests {
    use super::*;
    use crate::gdocs::application::_test_helpers::*;
    use crate::gdocs::application::co_edit_guard::GuardContext;
    use crate::gdocs::domain::{BatchUpdateResult, HeadingLevel, ParagraphKind, RevisionId};
    use std::sync::{Arc, Mutex};

    fn expect_get_sequence(
        client: &mut crate::gdocs::domain::traits::MockDocsClient,
        snaps: Vec<DocumentSnapshot>,
    ) {
        let queue = Arc::new(Mutex::new(snaps));
        client.expect_get().returning(move |_| {
            let mut q = queue.lock().unwrap();
            let s = if q.len() > 1 {
                q.remove(0)
            } else {
                q[0].clone()
            };
            Ok(s)
        });
    }

    fn make_batch_update_capture(
        client: &mut crate::gdocs::domain::traits::MockDocsClient,
        rev_after: &'static str,
    ) -> Arc<Mutex<Vec<serde_json::Value>>> {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let c = captured.clone();
        client.expect_batch_update().returning(move |_, reqs, _| {
            *c.lock().unwrap() = reqs.clone();
            Ok(BatchUpdateResult {
                revision_id_after: RevisionId(rev_after.into()),
                replies: vec![],
            })
        });
        captured
    }

    fn default_input() -> StyleTextInput {
        StyleTextInput {
            find: "negrita".into(),
            style: StylePatch::default(),
            scope: Scope::All,
            case_sensitive: false,
            whole_word: false,
            occurrence: None,
            anchor: None,
            dry_run: false,
        }
    }

    #[tokio::test]
    async fn style_bold_emits_update_text_style() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Texto negrita texto", 1, 22)],
        );
        let s2 = snap(
            "r2",
            vec![(1, ParagraphKind::Paragraph, "Texto negrita texto", 1, 22)],
        );
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        let captured = make_batch_update_capture(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = StyleTextInput {
            style: StylePatch {
                bold: Some(true),
                ..Default::default()
            },
            ..default_input()
        };
        let result = super::run(&ctx, &doc_id(), input).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, ChangeKind::Style);
        let reqs = captured.lock().unwrap();
        assert!(reqs.iter().any(|r| r.get("updateTextStyle").is_some()));
    }

    #[tokio::test]
    async fn style_heading_level_emits_update_paragraph_style() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Mi título", 1, 11)],
        );
        let s2 = snap("r2", vec![(1, ParagraphKind::Heading1, "Mi título", 1, 11)]);
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        let captured = make_batch_update_capture(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = StyleTextInput {
            find: "Mi título".into(),
            style: StylePatch {
                heading_level: Some(HeadingLevel::H1),
                ..Default::default()
            },
            ..default_input()
        };
        super::run(&ctx, &doc_id(), input).await.unwrap();
        let reqs = captured.lock().unwrap();
        assert!(reqs.iter().any(|r| r.get("updateParagraphStyle").is_some()));
    }

    #[tokio::test]
    async fn style_link_sets_url() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "click negrita aquí", 1, 21)],
        );
        let s2 = snap(
            "r2",
            vec![(1, ParagraphKind::Paragraph, "click negrita aquí", 1, 21)],
        );
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        let captured = make_batch_update_capture(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = StyleTextInput {
            style: StylePatch {
                link: Some("https://example.com".into()),
                ..Default::default()
            },
            ..default_input()
        };
        super::run(&ctx, &doc_id(), input).await.unwrap();
        let reqs = captured.lock().unwrap();
        let has_link = reqs.iter().any(|r| {
            r.get("updateTextStyle")
                .and_then(|u| u.get("textStyle"))
                .and_then(|t| t.get("link"))
                .and_then(|l| l.get("url"))
                .map(|u| u.as_str() == Some("https://example.com"))
                .unwrap_or(false)
        });
        assert!(has_link);
    }

    #[tokio::test]
    async fn style_dry_run_skips_batch_update() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Texto negrita texto", 1, 22)],
        );
        expect_get_sequence(&mut rig.client, vec![s]);
        // No expect_batch_update.
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = StyleTextInput {
            style: StylePatch {
                bold: Some(true),
                ..Default::default()
            },
            dry_run: true,
            ..default_input()
        };
        let result = super::run(&ctx, &doc_id(), input).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.revision_id_after.0, "r1");
    }

    #[tokio::test]
    async fn style_text_not_found() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Solo texto", 1, 12)],
        );
        expect_get_sequence(&mut rig.client, vec![s]);
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = StyleTextInput {
            style: StylePatch {
                bold: Some(true),
                ..Default::default()
            },
            ..default_input()
        };
        let err = super::run(&ctx, &doc_id(), input).await.unwrap_err();
        assert!(matches!(err, DocsError::TextNotFound { .. }));
    }
}
