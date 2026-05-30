use crate::documents::application::apply_excel_ops::ExcelOpApplier;
use crate::documents::domain::artifact::{PatchApplied, PatchSummary, VersionData};
use crate::documents::domain::ids::{ArtifactId, ArtifactKind, VersionId};
use crate::documents::domain::ir::ExcelIR;
use crate::documents::domain::patch::Patch;
use crate::documents::domain::{
    ArtifactStore, DocumentError, IRRenderer, IRValidator, IdGenerator,
};
use chrono::Utc;
use std::sync::Arc;

pub struct ApplyPatchInput {
    pub patch: Patch,
}

#[derive(Debug)]
pub struct ApplyPatchOutput {
    pub version_id: VersionId,
    pub summary: PatchSummary,
}

pub struct ApplyPatchUseCase {
    pub store: Arc<dyn ArtifactStore>,
    pub excel_renderer: Arc<dyn IRRenderer>,
    pub excel_validator: Arc<dyn IRValidator>,
    pub word_renderer: Arc<dyn IRRenderer>,
    pub word_validator: Arc<dyn IRValidator>,
    pub ids: Arc<dyn IdGenerator>,
}

impl ApplyPatchUseCase {
    pub async fn execute(&self, input: ApplyPatchInput) -> Result<ApplyPatchOutput, DocumentError> {
        let artifact_id = ArtifactId::new(input.patch.artifact_id.clone());
        let meta = self.store.read_meta(&artifact_id).await?;
        let current = meta.current_version.clone();

        if input.patch.base_version != current.0 {
            return Err(DocumentError::VersionConflict {
                artifact: artifact_id.clone(),
                base: VersionId::new(input.patch.base_version.clone()),
                current: current.clone(),
                conflicts: vec![],
            });
        }

        let current_data = self.store.read_version(&artifact_id, &current).await?;

        match meta.kind {
            ArtifactKind::Excel => {
                let mut ir: ExcelIR =
                    serde_json::from_value(current_data.ir.clone()).map_err(|e| {
                        DocumentError::IRValidationFailed {
                            path: "/".into(),
                            reason: format!("parse current IR: {e}"),
                        }
                    })?;
                let applier = ExcelOpApplier {
                    ids: self.ids.as_ref(),
                };
                let mut structured = Vec::with_capacity(input.patch.ops.len());
                let mut natural_language = Vec::with_capacity(input.patch.ops.len());
                for (i, op) in input.patch.ops.iter().enumerate() {
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
                let new_version = current.next();
                ir.version_id = new_version.0.clone();
                let ir_value = serde_json::to_value(&ir).unwrap();
                self.excel_validator.validate(&ir_value)?;
                let rendered = self.excel_renderer.render(&ir_value).await?;

                let patch_applied = PatchApplied {
                    patch: serde_json::to_value(&input.patch).unwrap(),
                    applied_at: Utc::now(),
                    resulted_in: new_version.clone(),
                    summary: summary.clone(),
                };
                let version_data = VersionData {
                    ir: ir_value,
                    rendered_binary: rendered,
                    rendered_extension: "xlsx",
                    patch_applied,
                    blobs: vec![],
                };

                self.store
                    .write_version(&artifact_id, &new_version, &version_data)
                    .await?;
                self.store
                    .set_head(&artifact_id, Some(&current), &new_version)
                    .await?;

                let mut new_meta = meta.clone();
                new_meta.current_version = new_version.clone();
                new_meta.updated_at = Utc::now();
                self.store.update_meta(&artifact_id, &new_meta).await?;

                Ok(ApplyPatchOutput {
                    version_id: new_version,
                    summary,
                })
            }
            ArtifactKind::Word => {
                use crate::documents::application::apply_word_ops::WordOpApplier;
                use crate::documents::domain::ir::WordIR;

                let mut ir: WordIR =
                    serde_json::from_value(current_data.ir.clone()).map_err(|e| {
                        DocumentError::IRValidationFailed {
                            path: "/".into(),
                            reason: format!("parse current Word IR: {e}"),
                        }
                    })?;
                let applier = WordOpApplier {
                    ids: self.ids.as_ref(),
                };
                let mut structured = Vec::with_capacity(input.patch.ops.len());
                let mut natural_language = Vec::with_capacity(input.patch.ops.len());
                for (i, op) in input.patch.ops.iter().enumerate() {
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
                let new_version = current.next();
                ir.version_id = new_version.0.clone();
                let ir_value = serde_json::to_value(&ir).unwrap();
                self.word_validator.validate(&ir_value)?;
                let rendered = self.word_renderer.render(&ir_value).await?;

                let patch_applied = PatchApplied {
                    patch: serde_json::to_value(&input.patch).unwrap(),
                    applied_at: Utc::now(),
                    resulted_in: new_version.clone(),
                    summary: summary.clone(),
                };
                let version_data = VersionData {
                    ir: ir_value,
                    rendered_binary: rendered,
                    rendered_extension: "docx",
                    patch_applied,
                    blobs: vec![],
                };
                self.store
                    .write_version(&artifact_id, &new_version, &version_data)
                    .await?;
                self.store
                    .set_head(&artifact_id, Some(&current), &new_version)
                    .await?;

                let mut new_meta = meta.clone();
                new_meta.current_version = new_version.clone();
                new_meta.updated_at = Utc::now();
                self.store.update_meta(&artifact_id, &new_meta).await?;

                Ok(ApplyPatchOutput {
                    version_id: new_version,
                    summary,
                })
            }
            ArtifactKind::Html => {
                unimplemented!("Html patch application not yet implemented")
            }
        }
    }
}

fn op_outcome_entry(
    op_index: usize,
    op: &crate::documents::domain::patch::PatchOp,
    outcome: &crate::documents::domain::artifact::OpOutcome,
) -> serde_json::Value {
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

fn describe_op(
    op: &crate::documents::domain::patch::PatchOp,
    outcome: &crate::documents::domain::artifact::OpOutcome,
) -> String {
    use crate::documents::domain::patch::PatchOp::*;
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
    }
}

fn fmt_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("'{}'", truncate(s, 40)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::application::create_document::{
        CreateDocumentInput, CreateDocumentUseCase,
    };
    use crate::documents::domain::ids::SessionId;
    use crate::documents::domain::patch::{PatchOp, PatchSource};
    use crate::documents::infrastructure::ids::CountingIdGenerator;
    use crate::documents::infrastructure::render::ExcelRenderer;
    use crate::documents::infrastructure::storage::LocalFsStore;
    use crate::documents::infrastructure::validation::ExcelValidator;
    use async_trait::async_trait;
    use tempfile::tempdir;

    struct NoopR;
    #[async_trait]
    impl IRRenderer for NoopR {
        async fn render(
            &self,
            _ir: &serde_json::Value,
        ) -> Result<Vec<u8>, crate::documents::domain::RenderError> {
            Ok(vec![])
        }
        fn target_extension(&self) -> &'static str {
            "docx"
        }
        fn target_mime(&self) -> &'static str {
            "x"
        }
    }
    struct NoopV;
    impl IRValidator for NoopV {
        fn validate(&self, _ir: &serde_json::Value) -> Result<(), DocumentError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn apply_set_cell_creates_v2() {
        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let ids = Arc::new(CountingIdGenerator::default());
        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Excel,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: Some(serde_json::json!({
                    "kind": "excel",
                    "artifact_id": "x", "version_id": "v1",
                    "schema_version": "1.0.0",
                    "workbook": { "sheets": [
                        {"id": "s1", "name": "Hoja1", "order": 0, "columns": [], "cells": {}, "tables": []}
                    ], "named_styles": {} }
                })),
                source: PatchSource::Agent,
            })
            .await
            .unwrap();

        let apply = ApplyPatchUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            ids: ids.clone(),
        };
        let res = apply
            .execute(ApplyPatchInput {
                patch: Patch {
                    artifact_id: out.artifact_id.0.clone(),
                    base_version: "v1".into(),
                    source: PatchSource::Agent,
                    ops: vec![PatchOp::SetCell {
                        sheet_id: "s1".into(),
                        address: "B5".into(),
                        value: serde_json::json!(42),
                        value_type: None,
                        format: None,
                        style_ref: None,
                    }],
                },
            })
            .await
            .unwrap();
        assert_eq!(res.version_id.0, "v2");
    }

    #[tokio::test]
    async fn apply_with_stale_base_returns_conflict() {
        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let ids = Arc::new(CountingIdGenerator::default());
        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Excel,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: Some(serde_json::json!({
                    "kind": "excel", "artifact_id": "x", "version_id": "v1",
                    "schema_version": "1.0.0",
                    "workbook": {"sheets": [{"id": "s1", "name": "H", "order": 0, "columns": [], "cells": {}, "tables": []}], "named_styles": {}}
                })),
                source: PatchSource::Agent,
            })
            .await
            .unwrap();

        let apply = ApplyPatchUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            ids: ids.clone(),
        };
        let err = apply
            .execute(ApplyPatchInput {
                patch: Patch {
                    artifact_id: out.artifact_id.0,
                    base_version: "v0".into(),
                    source: PatchSource::Agent,
                    ops: vec![],
                },
            })
            .await
            .unwrap_err();
        assert!(matches!(err, DocumentError::VersionConflict { .. }));
    }
}
