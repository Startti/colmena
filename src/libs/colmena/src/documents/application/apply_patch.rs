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
    pub async fn execute(
        &self,
        input: ApplyPatchInput,
    ) -> Result<ApplyPatchOutput, DocumentError> {
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
                let mut ir: ExcelIR = serde_json::from_value(current_data.ir.clone()).map_err(
                    |e| DocumentError::IRValidationFailed {
                        path: "/".into(),
                        reason: format!("parse current IR: {e}"),
                    },
                )?;
                let applier = ExcelOpApplier {
                    ids: self.ids.as_ref(),
                };
                for op in &input.patch.ops {
                    applier.apply(&mut ir, op)?;
                }
                let new_version = current.next();
                ir.version_id = new_version.0.clone();
                let ir_value = serde_json::to_value(&ir).unwrap();
                self.excel_validator.validate(&ir_value)?;
                let rendered = self.excel_renderer.render(&ir_value).await?;

                let patch_applied = PatchApplied {
                    patch: serde_json::to_value(&input.patch).unwrap(),
                    applied_at: Utc::now(),
                    resulted_in: new_version.clone(),
                    summary: PatchSummary::default(),
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
                    summary: PatchSummary::default(),
                })
            }
            ArtifactKind::Word => Err(DocumentError::InvalidPatchOp {
                reason: "Word patches not yet implemented (Phase B)".into(),
                op: serde_json::Value::Null,
            }),
        }
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
