//! Static-dispatch seam that collapses the per-`ArtifactKind` op-application
//! duplication in `apply_patch.rs` into a single generic path.
//!
//! Behavior-preserving refactor (code-audit finding #39): the op-application
//! loop, version-bump, and per-kind IR-parse error wording are byte-identical
//! to the pre-refactor inline per-kind match arms — only the duplication of
//! that loop across the three `ArtifactKind` variants is removed. See the
//! `documents-artifact-dispatch-dedup` design (finding #39) for the full
//! rationale.

use crate::documents::application::apply_excel_ops::ExcelOpApplier;
use crate::documents::application::apply_html_ops::HtmlOpApplier;
use crate::documents::application::apply_word_ops::WordOpApplier;
use crate::documents::domain::artifact::{OpOutcome, PatchSummary};
use crate::documents::domain::ids::ArtifactKind;
use crate::documents::domain::ir::html::HtmlIR;
use crate::documents::domain::ir::{ExcelIR, WordIR};
use crate::documents::domain::patch::PatchOp;
use crate::documents::domain::{DocumentError, IdGenerator};
use serde::Serialize;
use serde_json::Value;

/// Per-`ArtifactKind` behavior needed by the generic op-application loop in
/// [`run_ops`]. Each impl wraps one of the existing `*OpApplier` structs
/// verbatim — op semantics and error text are unchanged by this seam.
pub(crate) trait OpApplier {
    type Ir: Serialize;

    /// Parse the current version's IR JSON into the typed IR. Preserves the
    /// exact `DocumentError::IRValidationFailed` reason text used before
    /// extraction (each kind has its own wording).
    fn parse(v: &Value) -> Result<Self::Ir, DocumentError>;

    /// Apply one op to the typed IR.
    fn apply(&self, ir: &mut Self::Ir, op: &PatchOp) -> Result<OpOutcome, DocumentError>;

    /// Stamp the new version id onto the typed IR before serializing back.
    fn set_version(ir: &mut Self::Ir, new_version: &str);
}

impl OpApplier for ExcelOpApplier<'_> {
    type Ir = ExcelIR;

    fn parse(v: &Value) -> Result<Self::Ir, DocumentError> {
        serde_json::from_value(v.clone()).map_err(|e| DocumentError::IRValidationFailed {
            path: "/".into(),
            reason: format!("parse current IR: {e}"),
        })
    }

    fn apply(&self, ir: &mut Self::Ir, op: &PatchOp) -> Result<OpOutcome, DocumentError> {
        ExcelOpApplier::apply(self, ir, op)
    }

    fn set_version(ir: &mut Self::Ir, new_version: &str) {
        ir.version_id = new_version.to_string();
    }
}

impl OpApplier for WordOpApplier<'_> {
    type Ir = WordIR;

    fn parse(v: &Value) -> Result<Self::Ir, DocumentError> {
        serde_json::from_value(v.clone()).map_err(|e| DocumentError::IRValidationFailed {
            path: "/".into(),
            reason: format!("parse current Word IR: {e}"),
        })
    }

    fn apply(&self, ir: &mut Self::Ir, op: &PatchOp) -> Result<OpOutcome, DocumentError> {
        WordOpApplier::apply(self, ir, op)
    }

    fn set_version(ir: &mut Self::Ir, new_version: &str) {
        ir.version_id = new_version.to_string();
    }
}

impl OpApplier for HtmlOpApplier<'_> {
    type Ir = HtmlIR;

    fn parse(v: &Value) -> Result<Self::Ir, DocumentError> {
        serde_json::from_value(v.clone()).map_err(|e| DocumentError::IRValidationFailed {
            path: "/".into(),
            reason: format!("parse current HTML IR: {e}"),
        })
    }

    fn apply(&self, ir: &mut Self::Ir, op: &PatchOp) -> Result<OpOutcome, DocumentError> {
        HtmlOpApplier::apply(self, ir, op)
    }

    fn set_version(ir: &mut Self::Ir, new_version: &str) {
        ir.version_id = new_version.to_string();
    }
}

/// Apply every op in `ops` to the IR parsed from `current_ir`, in order,
/// building the natural-language + structured summary exactly as the
/// pre-refactor per-kind arms did, then stamp `new_version` onto the IR and
/// serialize it back to JSON.
fn run_ops<A: OpApplier>(
    applier: &A,
    current_ir: &Value,
    ops: &[PatchOp],
    new_version: &str,
) -> Result<(Value, PatchSummary), DocumentError> {
    let mut ir = A::parse(current_ir)?;

    let mut structured = Vec::with_capacity(ops.len());
    let mut natural_language = Vec::with_capacity(ops.len());
    for (i, op) in ops.iter().enumerate() {
        let outcome = applier.apply(&mut ir, op)?;
        natural_language.push(describe_op(op, &outcome));
        if !outcome.assigned_ids.is_empty() {
            structured.push(op_outcome_entry(i, op, &outcome));
        }
    }
    let summary = PatchSummary {
        natural_language,
        structured,
    };

    A::set_version(&mut ir, new_version);
    let ir_value = serde_json::to_value(&ir).unwrap();
    Ok((ir_value, summary))
}

/// Apply `ops` to `current_ir` for the given `kind`, returning the updated IR
/// JSON and the patch summary. This is the single per-`ArtifactKind` match in
/// the whole op-application path — everything else ([`run_ops`]) is generic
/// over [`OpApplier`].
pub(crate) fn apply_ops_for_kind(
    kind: ArtifactKind,
    current_ir: &Value,
    ops: &[PatchOp],
    new_version: &str,
    ids: &dyn IdGenerator,
) -> Result<(Value, PatchSummary), DocumentError> {
    match kind {
        ArtifactKind::Excel => run_ops(&ExcelOpApplier { ids }, current_ir, ops, new_version),
        ArtifactKind::Word => run_ops(&WordOpApplier { ids }, current_ir, ops, new_version),
        ArtifactKind::Html => run_ops(&HtmlOpApplier { ids }, current_ir, ops, new_version),
    }
}

// ---- Shared op summary helpers ----
// Moved verbatim from apply_patch.rs — same behavior, now shared by the
// generic `run_ops` loop instead of being duplicated per `ArtifactKind` arm.

pub(crate) fn op_outcome_entry(op_index: usize, op: &PatchOp, outcome: &OpOutcome) -> Value {
    let op_tag = serde_json::to_value(op)
        .ok()
        .and_then(|v| v.get("op").and_then(|s| s.as_str()).map(|s| s.to_string()))
        .unwrap_or_default();
    serde_json::json!({
        "op_index": op_index,
        "op": op_tag,
        "assigned_ids": outcome.assigned_ids,
    })
}

pub(crate) fn describe_op(op: &PatchOp, outcome: &OpOutcome) -> String {
    use PatchOp::*;
    let ids = &outcome.assigned_ids;
    match op {
        // ---- Excel ----
        SetCell {
            sheet_id,
            address,
            value,
            style_ref,
            ..
        } => {
            let mut s = format!(
                "Set cell {address} on sheet '{sheet_id}' to {}",
                fmt_value(value)
            );
            if let Some(sr) = style_ref {
                s.push_str(&format!(" (style: {sr})"));
            }
            s
        }
        SetRange {
            sheet_id,
            range,
            values,
            ..
        } => {
            let n: usize = values.iter().flatten().filter(|v| !v.is_null()).count();
            format!("Set {n} cells in range {range} on sheet '{sheet_id}'")
        }
        ClearRange { sheet_id, range } => {
            format!("Cleared cells in range {range} on sheet '{sheet_id}'")
        }
        InsertRow {
            sheet_id,
            before_row,
            ..
        } => format!("Inserted row before row {before_row} on sheet '{sheet_id}'"),
        DeleteRow {
            sheet_id,
            row_index,
        } => format!("Deleted row {row_index} on sheet '{sheet_id}'"),
        InsertColumn {
            sheet_id,
            before_col,
            ..
        } => format!("Inserted column before col {before_col} on sheet '{sheet_id}'"),
        DeleteColumn {
            sheet_id,
            col_index,
        } => format!("Deleted column {col_index} on sheet '{sheet_id}'"),
        AddSheet { name, .. } => match &ids.sheet {
            Some(sid) => format!("Added sheet '{name}' (id: {sid})"),
            None => format!("Added sheet '{name}'"),
        },
        RenameSheet { sheet_id, new_name } => {
            format!("Renamed sheet '{sheet_id}' to '{new_name}'")
        }
        DeleteSheet { sheet_id } => format!("Deleted sheet '{sheet_id}'"),
        ReorderSheets { order } => format!("Reordered sheets to [{}]", order.join(", ")),
        CreateTable {
            sheet_id,
            range,
            name,
            ..
        } => match &ids.table {
            Some(tid) => {
                format!("Created table '{name}' over {range} on sheet '{sheet_id}' (id: {tid})")
            }
            None => format!("Created table '{name}' over {range} on sheet '{sheet_id}'"),
        },
        ResizeTable {
            table_id,
            new_range,
        } => format!("Resized table {table_id} to {new_range}"),
        DeleteTable { table_id } => format!("Deleted table {table_id}"),
        SetColumnWidth {
            sheet_id,
            col,
            width,
        } => format!("Set column {col} width to {width} on sheet '{sheet_id}'"),
        DefineStyle { style_ref, .. } => format!("Defined style '{style_ref}'"),

        // ---- Word ----
        InsertBlock {
            before,
            after,
            block,
        } => {
            let btype = block
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("block");
            let pos = match (before.as_deref(), after.as_deref()) {
                (Some(b), _) => format!(" before {b}"),
                (_, Some(a)) => format!(" after {a}"),
                _ => " at end".to_string(),
            };
            let id_part = ids
                .block
                .as_ref()
                .map(|b| format!(" (id: {b})"))
                .unwrap_or_default();
            format!("Inserted {btype}{pos}{id_part}")
        }
        DeleteBlock { block_id } => format!("Deleted block {block_id}"),
        ReplaceBlock { block_id, .. } => format!("Replaced block {block_id}"),
        MoveBlock {
            block_id,
            after_block_id,
        } => format!("Moved block {block_id} after {after_block_id}"),
        SetHeadingLevel { block_id, level } => {
            format!("Set heading level of {block_id} to {level}")
        }
        ReplaceRunText {
            block_id,
            run_id,
            new_text,
        } => format!(
            "Replaced text in run {run_id} of {block_id} to '{}'",
            truncate(new_text, 60)
        ),
        SetRunStyle {
            block_id, run_id, ..
        } => format!("Updated style on run {run_id} of {block_id}"),
        InsertRun {
            block_id, at_index, ..
        } => {
            let id_part = ids
                .runs
                .first()
                .map(|r| format!(" (id: {r})"))
                .unwrap_or_default();
            format!("Inserted run in {block_id} at index {at_index}{id_part}")
        }
        DeleteRun { block_id, run_id } => format!("Deleted run {run_id} from {block_id}"),
        InsertListItem {
            list_block_id,
            at_index,
            ..
        } => {
            let id_part = ids
                .list_items
                .first()
                .map(|i| format!(" (id: {i})"))
                .unwrap_or_default();
            format!("Inserted list item in {list_block_id} at index {at_index}{id_part}")
        }
        ReplaceListItem {
            list_block_id,
            item_id,
            ..
        } => format!("Replaced list item {item_id} in {list_block_id}"),
        DeleteListItem {
            list_block_id,
            item_id,
        } => format!("Deleted list item {item_id} from {list_block_id}"),
        InsertTableRow {
            table_block_id,
            before,
            after,
            ..
        } => {
            let pos = match (before.as_deref(), after.as_deref()) {
                (Some(b), _) => format!(" before {b}"),
                (_, Some(a)) => format!(" after {a}"),
                _ => " at end".to_string(),
            };
            let id_part = ids
                .rows
                .first()
                .map(|r| format!(" (id: {r})"))
                .unwrap_or_default();
            format!("Inserted row in {table_block_id}{pos}{id_part}")
        }
        DeleteTableRow {
            table_block_id,
            row_id,
        } => format!("Deleted row {row_id} from {table_block_id}"),
        UpdateTableCell {
            table_block_id,
            row_id,
            col_index,
            ..
        } => format!("Updated cell in {table_block_id} at row {row_id}, col {col_index}"),

        // ---- HTML — slide level ----
        AddSlide { layout, title, .. } => {
            use crate::documents::domain::ir::html::SlideLayout;
            match (layout, title.as_deref()) {
                (SlideLayout::Title, Some(t)) => format!("Added title slide '{t}'"),
                (SlideLayout::SectionDivider, Some(t)) => {
                    format!("Added section divider '{t}'")
                }
                (l, Some(t)) => format!("Added {:?} slide '{t}'", l),
                (l, None) => format!("Added {:?} slide", l),
            }
        }
        DeleteSlide { slide_id } => format!("Deleted slide {slide_id}"),
        ReorderSlides { order } => format!("Reordered slides to [{}]", order.join(", ")),
        SetSlideLayout { slide_id, layout } => {
            format!("Set slide {slide_id} layout to {:?}", layout)
        }
        SetSlideTitle {
            slide_id, title, ..
        } => match title {
            Some(t) => format!("Set slide {slide_id} title to '{t}'"),
            None => format!("Cleared slide {slide_id} title"),
        },
        SetSlideNotes { slide_id, notes } => match notes {
            Some(_) => format!("Updated speaker notes on {slide_id}"),
            None => format!("Cleared speaker notes on {slide_id}"),
        },

        // ---- HTML — block level ----
        InsertHtmlBlock {
            slide_id, block, ..
        } => {
            let kind = block
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("block");
            let id_part = ids
                .block
                .as_ref()
                .map(|b| format!(" (id: {b})"))
                .unwrap_or_default();
            format!("Inserted {kind} block on slide {slide_id}{id_part}")
        }
        DeleteHtmlBlock { slide_id, block_id } => {
            format!("Deleted block {block_id} from slide {slide_id}")
        }
        ReplaceHtmlBlock { block_id, .. } => format!("Replaced block {block_id}"),
        MoveHtmlBlock {
            block_id,
            after_block_id,
            ..
        } => format!("Moved block {block_id} after {after_block_id}"),

        // ---- HTML — table ----
        InsertHtmlTableRow { table_block_id, .. } => {
            format!("Inserted row in table {table_block_id}")
        }
        DeleteHtmlTableRow {
            table_block_id,
            row_id,
            ..
        } => format!("Deleted row {row_id} from table {table_block_id}"),
        UpdateHtmlTableCell {
            table_block_id,
            row_id,
            col_index,
            ..
        } => format!("Updated cell ({row_id}, col {col_index}) of table {table_block_id}"),

        // ---- HTML — list ----
        InsertHtmlListItem {
            list_block_id,
            at_index,
            ..
        } => format!("Inserted list item at index {at_index} of {list_block_id}"),
        DeleteHtmlListItem {
            list_block_id,
            item_id,
            ..
        } => format!("Deleted list item {item_id} from {list_block_id}"),
        UpdateHtmlListItem {
            list_block_id,
            item_id,
            ..
        } => format!("Updated list item {item_id} in {list_block_id}"),

        // ---- HTML — document level ----
        SetTheme { theme } => {
            format!("Set theme to {}", format!("{:?}", theme).to_lowercase())
        }
        SetDocProps { title, .. } => match title {
            Some(t) => format!("Set document title to '{t}'"),
            None => "Updated document props".into(),
        },
        SetFooter { footer } => {
            if footer.enabled {
                format!(
                    "Enabled footer (page_numbers={}, custom='{}')",
                    footer.page_numbers,
                    footer.custom_text.as_deref().unwrap_or("")
                )
            } else {
                "Disabled footer".into()
            }
        }
    }
}

fn fmt_value(v: &Value) -> String {
    match v {
        Value::String(s) => format!("'{}'", truncate(s, 40)),
        _ => truncate(&v.to_string(), 40),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{kept}...")
    }
}

// NOTE: `describe_op`/`op_outcome_entry` behavioral coverage lives in
// `apply_patch.rs`'s test module (unchanged tests, now importing these
// functions from here) to keep the Phase 0 parity net test paths stable
// across the extraction.
