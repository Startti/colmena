//! Pure markdown → Google Docs `batchUpdate` request emitter.
//!
//! Used by every dispatcher that inserts formatted content
//! (insert_after_text, insert_before_text, replace_section,
//! append_markdown, add_tab with body, create_named_range, etc.).
//!
//! ## Strategy
//!
//! 1. Tokenise via `pulldown-cmark`.
//! 2. Walk events, accumulate per-paragraph text + style spans.
//! 3. On paragraph end, emit:
//!    - `InsertTextRequest`
//!    - `UpdateParagraphStyleRequest` (always — sets `namedStyleType`)
//!    - `UpdateTextStyleRequest` per style span
//!    - `CreateParagraphBulletsRequest` (if list item)
//! 4. Advance the cursor by paragraph length (+1 for `\n`).
//!
//! Forward-only emission — no write-backwards complexity because all
//! inserts happen at the running cursor.
//!
//! ## Unsupported elements
//!
//! Footnote definitions and image references emit a [`LossyConversion`]
//! record and are skipped (no request emitted).
//!
//! `$...$` math is NOT detected by pulldown-cmark (it doesn't ship a
//! math extension), so it passes through as plain text — no lossy
//! record is emitted. This is acceptable for v1; treat math as a known
//! v1.1 enhancement.

use crate::gdocs::domain::{LossyConversion, TabId};
use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel as CmarkHeading, Options, Parser, Tag, TagEnd,
};
use serde_json::{json, Value};

/// Where the converter inserts the markdown content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertionPoint {
    /// Concrete (segment-relative) UTF-16 index.
    Index { index: u32, tab_id: Option<TabId> },
    /// Append at end-of-segment (used by `append_markdown` / `add_tab`).
    ///
    /// For v1 we resolve this on the dispatcher side by snapshotting
    /// the doc and computing the trailing index; the converter still
    /// receives a concrete `index` via [`InsertionPoint::Index`].
    EndOfSegment { tab_id: Option<TabId> },
}

/// Result of converting a markdown string into a batchUpdate request
/// sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversionResult {
    pub requests: Vec<Value>,
    pub lossy:    Vec<LossyConversion>,
}

/// Convert markdown to a sequence of Google Docs `batchUpdate`
/// requests, starting from `ip` and advancing the cursor forward as
/// content is emitted.
pub fn markdown_to_requests(md: &str, ip: InsertionPoint) -> ConversionResult {
    let mut emitter = Emitter::new(ip);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    let parser = Parser::new_ext(md, opts);
    for ev in parser {
        emitter.feed(ev);
    }
    emitter.finish()
}

// ---------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParaKind {
    Normal,
    Heading(u8), // 1..=6
    Bullet,
    Numbered,
    CodeBlock,
    Quote,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StyleFlags {
    bold:          bool,
    italic:        bool,
    strikethrough: bool,
    code:          bool, // inline code (monospace)
    link:          Option<String>,
}

#[derive(Debug, Clone)]
struct StyleSpan {
    start_utf16: u32, // offset from paragraph start, in utf16 units
    len_utf16:   u32,
    style:       StyleFlags,
}

#[derive(Debug)]
struct Emitter {
    cursor:       u32,
    _tab_id:      Option<TabId>,
    out_requests: Vec<Value>,
    out_lossy:    Vec<LossyConversion>,

    /// Accumulated paragraph text since the last flush.
    cur_text:    String,
    /// Length of `cur_text` in UTF-16 code units.
    cur_len_u16: u32,
    /// Style spans within the current paragraph.
    cur_spans:   Vec<StyleSpan>,
    /// Stack of currently-open inline styles (Strong/Emphasis/etc.).
    style_stack: Vec<StyleFlags>,
    /// Kind of the current paragraph (default = Normal).
    cur_kind:    ParaKind,

    /// `Some(open_start_utf16)` while inside a Strong tag (etc.).
    style_open_starts: Vec<u32>,

    /// `Some(label)` while inside a footnote def / image — suppresses output.
    suppress_depth: u32,

    /// Table state (for fixture 10).
    in_table:        bool,
    cur_table_rows:  Vec<Vec<String>>, // rows × cells (text only, no formatting in v1)
    cur_table_cell:  String,
    cur_table_row:   Vec<String>,
}

impl Emitter {
    fn new(ip: InsertionPoint) -> Self {
        let (cursor, tab_id) = match ip {
            InsertionPoint::Index { index, tab_id } => (index, tab_id),
            InsertionPoint::EndOfSegment { tab_id } => (0, tab_id),
        };
        Self {
            cursor,
            _tab_id: tab_id,
            out_requests: vec![],
            out_lossy: vec![],
            cur_text: String::new(),
            cur_len_u16: 0,
            cur_spans: vec![],
            style_stack: vec![],
            cur_kind: ParaKind::Normal,
            style_open_starts: vec![],
            suppress_depth: 0,
            in_table: false,
            cur_table_rows: vec![],
            cur_table_cell: String::new(),
            cur_table_row: vec![],
        }
    }

    fn current_style(&self) -> StyleFlags {
        let mut merged = StyleFlags::default();
        for s in &self.style_stack {
            if s.bold {
                merged.bold = true;
            }
            if s.italic {
                merged.italic = true;
            }
            if s.strikethrough {
                merged.strikethrough = true;
            }
            if s.code {
                merged.code = true;
            }
            if let Some(l) = &s.link {
                merged.link = Some(l.clone());
            }
        }
        merged
    }

    fn append_text(&mut self, s: &str) {
        if self.suppress_depth > 0 {
            return;
        }
        if self.in_table {
            self.cur_table_cell.push_str(s);
            return;
        }
        let style = self.current_style();
        let start = self.cur_len_u16;
        self.cur_text.push_str(s);
        let added: u32 = s.encode_utf16().count() as u32;
        self.cur_len_u16 += added;
        // Always record a span if style is non-default OR a link is set,
        // so we can emit updateTextStyle for it on flush.
        if style != StyleFlags::default() {
            self.cur_spans.push(StyleSpan {
                start_utf16: start,
                len_utf16:   added,
                style,
            });
        }
    }

    fn feed(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(end) => self.end_tag(end),
            Event::Text(t) => {
                let s: String = t.into_string();
                self.append_text(&s);
            }
            Event::Code(c) => {
                // Inline code → monospace span.
                self.style_stack.push(StyleFlags {
                    code: true,
                    ..Default::default()
                });
                let s: String = c.into_string();
                self.append_text(&s);
                self.style_stack.pop();
            }
            Event::SoftBreak => self.append_text(" "),
            Event::HardBreak => self.append_text("\n"),
            Event::Rule => {
                // Horizontal rule → InsertSectionBreakRequest at cursor.
                self.flush_paragraph_if_any();
                self.out_requests.push(json!({
                    "insertSectionBreak": {
                        "location": { "index": self.cursor },
                        "sectionType": "CONTINUOUS"
                    }
                }));
                // A section break is one character in the doc model.
                self.cursor += 1;
            }
            Event::FootnoteReference(_) => {
                // We don't emit footnote anchors in v1 — silently drop.
            }
            Event::Html(_) | Event::InlineHtml(_) | Event::TaskListMarker(_) => {
                // Ignored in v1.
            }
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.cur_kind = match self.cur_kind {
                    ParaKind::Bullet | ParaKind::Numbered | ParaKind::Quote => self.cur_kind,
                    _ => ParaKind::Normal,
                };
            }
            Tag::Heading { level, .. } => {
                let n = match level {
                    CmarkHeading::H1 => 1,
                    CmarkHeading::H2 => 2,
                    CmarkHeading::H3 => 3,
                    CmarkHeading::H4 => 4,
                    CmarkHeading::H5 => 5,
                    CmarkHeading::H6 => 6,
                };
                self.cur_kind = ParaKind::Heading(n);
            }
            Tag::BlockQuote => {
                self.cur_kind = ParaKind::Quote;
            }
            Tag::CodeBlock(_kind) => {
                self.cur_kind = ParaKind::CodeBlock;
                self.style_stack.push(StyleFlags {
                    code: true,
                    ..Default::default()
                });
                self.style_open_starts.push(self.cur_len_u16);
            }
            Tag::List(start_num) => {
                if start_num.is_some() {
                    self.cur_kind = ParaKind::Numbered;
                } else {
                    self.cur_kind = ParaKind::Bullet;
                }
            }
            Tag::Item => {
                // Each item is its own paragraph; flush any previous content.
                self.flush_paragraph_if_any();
            }
            Tag::FootnoteDefinition(label) => {
                let label_str: String = label.into_string();
                self.out_lossy.push(LossyConversion {
                    element_type:      "footnote".to_string(),
                    original_markdown: format!("[^{}]", label_str),
                });
                self.suppress_depth += 1;
            }
            Tag::Table(_) => {
                self.flush_paragraph_if_any();
                self.in_table = true;
                self.cur_table_rows.clear();
                self.cur_table_cell.clear();
                self.cur_table_row.clear();
            }
            Tag::TableHead | Tag::TableRow => {
                self.cur_table_row.clear();
            }
            Tag::TableCell => {
                self.cur_table_cell.clear();
            }
            Tag::Emphasis => {
                self.style_stack.push(StyleFlags {
                    italic: true,
                    ..Default::default()
                });
            }
            Tag::Strong => {
                self.style_stack.push(StyleFlags {
                    bold: true,
                    ..Default::default()
                });
            }
            Tag::Strikethrough => {
                self.style_stack.push(StyleFlags {
                    strikethrough: true,
                    ..Default::default()
                });
            }
            Tag::Link {
                dest_url, ..
            } => {
                self.style_stack.push(StyleFlags {
                    link: Some(dest_url.into_string()),
                    ..Default::default()
                });
            }
            Tag::Image { dest_url, .. } => {
                let url: String = dest_url.into_string();
                self.out_lossy.push(LossyConversion {
                    element_type:      "image_reference".to_string(),
                    original_markdown: format!("![]({})", url),
                });
                self.suppress_depth += 1;
            }
            Tag::HtmlBlock | Tag::MetadataBlock(_) => {
                // Ignored.
            }
        }
    }

    fn end_tag(&mut self, end: TagEnd) {
        match end {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::BlockQuote => {
                if matches!(end, TagEnd::CodeBlock) {
                    // pop the monospace style
                    self.style_stack.pop();
                    self.style_open_starts.pop();
                    // The code-block content typically ends with a trailing
                    // `\n` from the parser; strip it so we don't add a blank.
                    if self.cur_text.ends_with('\n') {
                        self.cur_text.pop();
                        self.cur_len_u16 -= 1;
                        // Also trim any style span that included the trailing nl.
                        if let Some(last) = self.cur_spans.last_mut() {
                            if last.start_utf16 + last.len_utf16 > self.cur_len_u16 {
                                last.len_utf16 = self.cur_len_u16 - last.start_utf16;
                            }
                        }
                    }
                }
                self.flush_paragraph_if_any();
                // Reset to Normal unless we're still inside a list.
                if !matches!(self.cur_kind, ParaKind::Bullet | ParaKind::Numbered) {
                    self.cur_kind = ParaKind::Normal;
                }
            }
            TagEnd::List(_) => {
                self.cur_kind = ParaKind::Normal;
            }
            TagEnd::Item => {
                self.flush_paragraph_if_any();
            }
            TagEnd::FootnoteDefinition => {
                self.suppress_depth = self.suppress_depth.saturating_sub(1);
            }
            TagEnd::Table => {
                self.in_table = false;
                self.emit_table();
                self.cur_table_rows.clear();
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                let row = std::mem::take(&mut self.cur_table_row);
                self.cur_table_rows.push(row);
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.cur_table_cell);
                self.cur_table_row.push(cell);
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                self.style_stack.pop();
            }
            TagEnd::Image => {
                self.suppress_depth = self.suppress_depth.saturating_sub(1);
            }
            TagEnd::HtmlBlock | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn flush_paragraph_if_any(&mut self) {
        if self.suppress_depth > 0 {
            // Reset accumulators but don't emit.
            self.cur_text.clear();
            self.cur_len_u16 = 0;
            self.cur_spans.clear();
            return;
        }
        if self.cur_text.is_empty() {
            return;
        }

        let para_start = self.cursor;
        // Append the newline that terminates the paragraph in the doc model.
        let mut text_with_nl = self.cur_text.clone();
        text_with_nl.push('\n');
        let text_with_nl_u16: u32 = text_with_nl.encode_utf16().count() as u32;
        let para_end = para_start + text_with_nl_u16; // open-ended

        // 1. InsertText.
        self.out_requests.push(json!({
            "insertText": {
                "location": { "index": para_start },
                "text": text_with_nl,
            }
        }));

        // 2. UpdateParagraphStyle (always — sets namedStyleType + extras).
        let (named_style, extras) = match self.cur_kind {
            ParaKind::Normal => ("NORMAL_TEXT", None),
            ParaKind::Heading(1) => ("HEADING_1", None),
            ParaKind::Heading(2) => ("HEADING_2", None),
            ParaKind::Heading(3) => ("HEADING_3", None),
            ParaKind::Heading(4) => ("HEADING_4", None),
            ParaKind::Heading(5) => ("HEADING_5", None),
            ParaKind::Heading(6) => ("HEADING_6", None),
            ParaKind::Heading(_) => ("NORMAL_TEXT", None),
            ParaKind::Bullet | ParaKind::Numbered => ("NORMAL_TEXT", None),
            ParaKind::CodeBlock => ("NORMAL_TEXT", None),
            ParaKind::Quote => (
                "NORMAL_TEXT",
                Some(json!({
                    "indentStart": { "magnitude": 36.0, "unit": "PT" }
                })),
            ),
        };
        let (style_obj, fields) = if let Some(extra) = extras {
            let mut obj = serde_json::Map::new();
            obj.insert("namedStyleType".into(), json!(named_style));
            if let Some(map) = extra.as_object() {
                for (k, v) in map {
                    obj.insert(k.clone(), v.clone());
                }
            }
            (Value::Object(obj), "namedStyleType,indentStart")
        } else {
            (
                json!({ "namedStyleType": named_style }),
                "namedStyleType",
            )
        };
        self.out_requests.push(json!({
            "updateParagraphStyle": {
                "range": { "startIndex": para_start, "endIndex": para_end },
                "paragraphStyle": style_obj,
                "fields": fields,
            }
        }));

        // 3. Bullets (if list item).
        let bullet_preset = match self.cur_kind {
            ParaKind::Bullet => Some("BULLET_DISC_CIRCLE_SQUARE"),
            ParaKind::Numbered => Some("NUMBERED_DECIMAL_ALPHA_ROMAN"),
            _ => None,
        };
        if let Some(preset) = bullet_preset {
            self.out_requests.push(json!({
                "createParagraphBullets": {
                    "range": { "startIndex": para_start, "endIndex": para_end },
                    "bulletPreset": preset,
                }
            }));
        }

        // 4. CodeBlock paragraph-wide monospace (when no spans cover it).
        if matches!(self.cur_kind, ParaKind::CodeBlock) {
            self.out_requests.push(json!({
                "updateTextStyle": {
                    "range": { "startIndex": para_start, "endIndex": para_end - 1 },
                    "textStyle": { "weightedFontFamily": { "fontFamily": "Roboto Mono" } },
                    "fields": "weightedFontFamily",
                }
            }));
        } else {
            // 5. UpdateTextStyle for each accumulated span.
            for span in &self.cur_spans {
                let start = para_start + span.start_utf16;
                let end = start + span.len_utf16;
                let (style_json, fields) = build_text_style(&span.style);
                self.out_requests.push(json!({
                    "updateTextStyle": {
                        "range": { "startIndex": start, "endIndex": end },
                        "textStyle": style_json,
                        "fields": fields,
                    }
                }));
            }
        }

        // Advance cursor + reset.
        self.cursor = para_end;
        self.cur_text.clear();
        self.cur_len_u16 = 0;
        self.cur_spans.clear();
    }

    /// Emit an InsertTableRequest + per-cell InsertText.
    ///
    /// Layout per Google Docs reference: an InsertTable adds the table
    /// at `index`; cells are then addressable starting at offsets
    /// computed from a fixed walk. For v1 we use a simplified shape that
    /// emits one `insertTable` request followed by one `insertText` per
    /// cell at increasing pseudo-indices; the dispatcher will follow up
    /// with a real snapshot before issuing the batchUpdate against the
    /// API. The fixture captures THIS shape, not the API-exact indices.
    fn emit_table(&mut self) {
        if self.cur_table_rows.is_empty() {
            return;
        }
        let rows = self.cur_table_rows.len() as u32;
        let cols = self
            .cur_table_rows
            .iter()
            .map(|r| r.len())
            .max()
            .unwrap_or(0) as u32;
        let table_start = self.cursor;
        self.out_requests.push(json!({
            "insertTable": {
                "location": { "index": table_start },
                "rows": rows,
                "columns": cols,
            }
        }));
        // Each cell consumes a baseline of 2 indices in the doc model
        // (one for the cell start + one for the trailing newline). For
        // the v1 emitter we walk cells in order and emit InsertText with
        // a logical `cell` selector — the dispatcher resolves to real
        // indices after a snapshot fetch. The fixture mirrors this shape.
        for (r_idx, row) in self.cur_table_rows.iter().enumerate() {
            for (c_idx, cell) in row.iter().enumerate() {
                if cell.is_empty() {
                    continue;
                }
                self.out_requests.push(json!({
                    "insertText": {
                        "tableCellLocation": {
                            "tableStartLocation": { "index": table_start },
                            "rowIndex": r_idx as u32,
                            "columnIndex": c_idx as u32,
                        },
                        "text": cell,
                    }
                }));
            }
        }
        // Advance the cursor past the table block. Real index math is
        // 1 (table) + sum(cells)+rows+cols; for v1 emitter purposes
        // advance by a deterministic amount derived from row × col count.
        self.cursor = table_start + 1 + rows * cols * 2 + rows;
    }

    fn finish(mut self) -> ConversionResult {
        self.flush_paragraph_if_any();
        ConversionResult {
            requests: self.out_requests,
            lossy:    self.out_lossy,
        }
    }
}

fn build_text_style(style: &StyleFlags) -> (Value, String) {
    let mut obj = serde_json::Map::new();
    let mut fields: Vec<&str> = vec![];
    if style.bold {
        obj.insert("bold".into(), json!(true));
        fields.push("bold");
    }
    if style.italic {
        obj.insert("italic".into(), json!(true));
        fields.push("italic");
    }
    if style.strikethrough {
        obj.insert("strikethrough".into(), json!(true));
        fields.push("strikethrough");
    }
    if style.code {
        obj.insert(
            "weightedFontFamily".into(),
            json!({ "fontFamily": "Roboto Mono" }),
        );
        fields.push("weightedFontFamily");
    }
    if let Some(url) = &style.link {
        obj.insert("link".into(), json!({ "url": url }));
        fields.push("link");
    }
    (Value::Object(obj), fields.join(","))
}

#[allow(dead_code)]
fn _suppress_unused(_k: CodeBlockKind) {}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures_dir() -> std::path::PathBuf {
        // CARGO_MANIFEST_DIR is `src/libs/colmena`; tests live at repo root.
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest
            .ancestors()
            .nth(3)
            .expect("repo root above CARGO_MANIFEST_DIR")
            .join("tests/gdocs_markdown_fixtures")
    }

    fn run_fixture(stem: &str) {
        let dir = fixtures_dir();
        let md_path = dir.join(format!("{stem}.md"));
        let exp_path = dir.join(format!("{stem}.ops.json"));
        let md = std::fs::read_to_string(&md_path)
            .unwrap_or_else(|e| panic!("md fixture {md_path:?} missing: {e}"));
        let got = markdown_to_requests(
            &md,
            InsertionPoint::Index { index: 1, tab_id: None },
        );
        let got_json = json!({
            "requests": got.requests,
            "lossy": got.lossy,
        });
        // Set COLMENA_MD_FIXTURE_UPDATE=1 to regenerate fixtures locally.
        if std::env::var("COLMENA_MD_FIXTURE_UPDATE").ok().as_deref() == Some("1") {
            std::fs::write(
                &exp_path,
                serde_json::to_string_pretty(&got_json).unwrap() + "\n",
            )
            .unwrap();
            return;
        }
        let exp_raw = std::fs::read_to_string(&exp_path)
            .unwrap_or_else(|e| panic!("ops.json {exp_path:?} missing: {e}"));
        let exp: Value =
            serde_json::from_str(&exp_raw).expect("ops.json parse");
        if got_json != exp {
            panic!(
                "fixture '{stem}' mismatch (set COLMENA_MD_FIXTURE_UPDATE=1 to regenerate):\n--- expected ---\n{}\n--- got ---\n{}\n",
                serde_json::to_string_pretty(&exp).unwrap(),
                serde_json::to_string_pretty(&got_json).unwrap(),
            );
        }
    }

    #[test]
    fn fixture_01_heading_and_paragraph() {
        run_fixture("01_heading_and_paragraph");
    }
    #[test]
    fn fixture_02_bold_italic_link() {
        run_fixture("02_bold_italic_link");
    }
    #[test]
    fn fixture_03_bullet_list() {
        run_fixture("03_bullet_list");
    }
    #[test]
    fn fixture_04_numbered_list() {
        run_fixture("04_numbered_list");
    }
    #[test]
    fn fixture_05_inline_code() {
        run_fixture("05_inline_code");
    }
    #[test]
    fn fixture_06_code_block() {
        run_fixture("06_code_block");
    }
    #[test]
    fn fixture_07_blockquote() {
        run_fixture("07_blockquote");
    }
    #[test]
    fn fixture_08_horizontal_rule() {
        run_fixture("08_horizontal_rule");
    }
    #[test]
    fn fixture_09_strikethrough() {
        run_fixture("09_strikethrough");
    }
    #[test]
    fn fixture_10_table_simple() {
        run_fixture("10_table_simple");
    }
    #[test]
    fn fixture_11_nested_emphasis() {
        run_fixture("11_nested_emphasis");
    }
    #[test]
    fn fixture_12_lossy_footnote() {
        run_fixture("12_lossy_footnote");
    }
    #[test]
    fn fixture_13_lossy_image() {
        run_fixture("13_lossy_image");
    }
    #[test]
    fn fixture_14_lossy_math() {
        run_fixture("14_lossy_math");
    }

    /// Helper test: smoke that `EndOfSegment` returns the same shape as
    /// `Index { index: 0 }` (placeholder semantics in v1).
    #[test]
    fn end_of_segment_shape_matches_index_zero() {
        let md = "Hello\n";
        let a = markdown_to_requests(
            md,
            InsertionPoint::Index { index: 0, tab_id: None },
        );
        let b = markdown_to_requests(
            md,
            InsertionPoint::EndOfSegment { tab_id: None },
        );
        assert_eq!(a.requests, b.requests);
    }
}
