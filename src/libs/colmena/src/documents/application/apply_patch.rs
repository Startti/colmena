use crate::documents::application::artifact_ops::{self, apply_ops_for_kind};
use crate::documents::domain::artifact::{PatchApplied, PatchSummary, VersionData};
use crate::documents::domain::ids::{ArtifactId, VersionId};
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
    pub html_renderer: Arc<dyn IRRenderer>,
    pub html_validator: Arc<dyn IRValidator>,
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
        let new_version = current.next();

        let (ir_value, summary) = apply_ops_for_kind(
            meta.kind,
            &current_data.ir,
            &input.patch.ops,
            &new_version.0,
            self.ids.as_ref(),
        )?;

        let (validator, renderer) = artifact_ops::render_pair(meta.kind, self);
        validator.validate(&ir_value)?;
        let rendered = renderer.render(&ir_value).await?;

        let patch_applied = PatchApplied {
            patch: serde_json::to_value(&input.patch).unwrap(),
            applied_at: Utc::now(),
            resulted_in: new_version.clone(),
            summary: summary.clone(),
        };
        let version_data = VersionData {
            ir: ir_value,
            rendered_binary: rendered,
            rendered_extension: meta.kind.extension(),
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
}

impl artifact_ops::ArtifactRenderers for ApplyPatchUseCase {
    fn excel_pair(&self) -> (&Arc<dyn IRValidator>, &Arc<dyn IRRenderer>) {
        (&self.excel_validator, &self.excel_renderer)
    }
    fn word_pair(&self) -> (&Arc<dyn IRValidator>, &Arc<dyn IRRenderer>) {
        (&self.word_validator, &self.word_renderer)
    }
    fn html_pair(&self) -> (&Arc<dyn IRValidator>, &Arc<dyn IRRenderer>) {
        (&self.html_validator, &self.html_renderer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::application::artifact_ops::describe_op;
    use crate::documents::application::create_document::{
        CreateDocumentInput, CreateDocumentUseCase,
    };
    use crate::documents::domain::ids::{ArtifactKind, SessionId};
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
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
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
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
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
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
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
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
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

    #[tokio::test]
    async fn apply_set_theme_on_html_advances_version() {
        use crate::documents::domain::ir::html::Theme;
        use crate::documents::domain::patch::PatchOp;
        use crate::documents::domain::ports::AssetStore;
        use crate::documents::infrastructure::render::HtmlRenderer;
        use crate::documents::infrastructure::storage::LocalFsAssetStore;
        use crate::documents::infrastructure::validation::HtmlValidator;

        let tmp_a = tempdir().unwrap();
        let tmp_b = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp_a.path()));
        let asset_store: Arc<dyn AssetStore> = Arc::new(LocalFsAssetStore::new(tmp_b.path()));
        let ids = Arc::new(CountingIdGenerator::default());

        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            html_renderer: Arc::new(HtmlRenderer::new(asset_store.clone())),
            html_validator: Arc::new(HtmlValidator),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Html,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: None,
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
            html_renderer: Arc::new(HtmlRenderer::new(asset_store)),
            html_validator: Arc::new(HtmlValidator),
            ids: ids.clone(),
        };
        let res = apply
            .execute(ApplyPatchInput {
                patch: Patch {
                    artifact_id: out.artifact_id.0.clone(),
                    base_version: "v1".into(),
                    source: PatchSource::Agent,
                    ops: vec![PatchOp::SetTheme {
                        theme: Theme::Vibrant,
                    }],
                },
            })
            .await
            .unwrap();
        assert_eq!(res.version_id.0, "v2");
    }

    #[test]
    fn describe_op_add_slide_includes_layout_and_title() {
        use crate::documents::domain::artifact::OpOutcome;
        use crate::documents::domain::ir::html::SlideLayout;
        let op = PatchOp::AddSlide {
            layout: SlideLayout::Title,
            at_index: None,
            title: Some("Welcome".into()),
            subtitle: None,
        };
        let outcome = OpOutcome::default();
        let s = describe_op(&op, &outcome);
        assert!(s.contains("Welcome"));
        assert!(s.to_lowercase().contains("title"));
    }

    #[test]
    fn describe_op_set_theme_mentions_theme() {
        use crate::documents::domain::artifact::OpOutcome;
        use crate::documents::domain::ir::html::Theme;
        let op = PatchOp::SetTheme { theme: Theme::Dark };
        let s = describe_op(&op, &OpOutcome::default());
        assert!(s.to_lowercase().contains("theme"));
        assert!(s.to_lowercase().contains("dark"));
    }

    // ---- Phase 0 parity net (finding #39) ----
    // Characterization tests pinning current behavior BEFORE the artifact_ops
    // dispatch refactor. These must pass unmodified against both pre- and
    // post-refactor code.

    #[tokio::test]
    async fn apply_insert_block_on_word_advances_version() {
        use crate::documents::infrastructure::render::WordRenderer;
        use crate::documents::infrastructure::validation::WordValidator;

        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let ids = Arc::new(CountingIdGenerator::default());
        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(WordRenderer),
            word_validator: Arc::new(WordValidator),
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Word,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: None,
                source: PatchSource::Agent,
            })
            .await
            .unwrap();

        let apply = ApplyPatchUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(WordRenderer),
            word_validator: Arc::new(WordValidator),
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
        };
        let res = apply
            .execute(ApplyPatchInput {
                patch: Patch {
                    artifact_id: out.artifact_id.0.clone(),
                    base_version: "v1".into(),
                    source: PatchSource::Agent,
                    ops: vec![PatchOp::InsertBlock {
                        before: None,
                        after: None,
                        block: serde_json::json!({
                            "type": "paragraph",
                            "runs": [{"text": "hello world"}]
                        }),
                    }],
                },
            })
            .await
            .unwrap();

        assert_eq!(res.version_id.0, "v2");
        assert_eq!(res.summary.natural_language.len(), 1);
        assert!(res.summary.natural_language[0].starts_with("Inserted paragraph"));

        let stored = store.read_current(&out.artifact_id).await.unwrap();
        assert_eq!(stored.rendered_extension, "docx");
        assert!(stored.rendered_binary.len() > 4);
        assert_eq!(&stored.rendered_binary[0..4], &[0x50, 0x4B, 0x03, 0x04]);
    }

    #[tokio::test]
    async fn apply_invalid_op_on_excel_returns_exact_reason() {
        use crate::documents::domain::ir::html::SlideLayout;

        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let ids = Arc::new(CountingIdGenerator::default());
        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Excel,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: None,
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
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
        };
        let err = apply
            .execute(ApplyPatchInput {
                patch: Patch {
                    artifact_id: out.artifact_id.0.clone(),
                    base_version: "v1".into(),
                    source: PatchSource::Agent,
                    ops: vec![PatchOp::AddSlide {
                        layout: SlideLayout::Blank,
                        at_index: None,
                        title: None,
                        subtitle: None,
                    }],
                },
            })
            .await
            .unwrap_err();

        match err {
            DocumentError::InvalidPatchOp { reason, .. } => {
                assert_eq!(reason, "Word/HTML op not applicable to Excel artifact");
            }
            other => panic!("expected InvalidPatchOp, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_invalid_op_on_word_returns_exact_reason() {
        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let ids = Arc::new(CountingIdGenerator::default());
        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Word,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: None,
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
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
        };
        let err = apply
            .execute(ApplyPatchInput {
                patch: Patch {
                    artifact_id: out.artifact_id.0.clone(),
                    base_version: "v1".into(),
                    source: PatchSource::Agent,
                    ops: vec![PatchOp::SetCell {
                        sheet_id: "s1".into(),
                        address: "A1".into(),
                        value: serde_json::json!(1),
                        value_type: None,
                        format: None,
                        style_ref: None,
                    }],
                },
            })
            .await
            .unwrap_err();

        match err {
            DocumentError::InvalidPatchOp { reason, .. } => {
                assert_eq!(reason, "not a Word op");
            }
            other => panic!("expected InvalidPatchOp, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_invalid_op_on_html_returns_exact_reason() {
        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let ids = Arc::new(CountingIdGenerator::default());
        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Html,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: None,
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
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
        };
        let err = apply
            .execute(ApplyPatchInput {
                patch: Patch {
                    artifact_id: out.artifact_id.0.clone(),
                    base_version: "v1".into(),
                    source: PatchSource::Agent,
                    ops: vec![PatchOp::SetCell {
                        sheet_id: "s1".into(),
                        address: "A1".into(),
                        value: serde_json::json!(1),
                        value_type: None,
                        format: None,
                        style_ref: None,
                    }],
                },
            })
            .await
            .unwrap_err();

        match err {
            DocumentError::InvalidPatchOp { reason, .. } => {
                assert_eq!(reason, "op not applicable to HTML kind");
            }
            other => panic!("expected InvalidPatchOp, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_parse_error_reason_excel_is_exact() {
        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let ids = Arc::new(CountingIdGenerator::default());
        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Excel,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: None,
                source: PatchSource::Agent,
            })
            .await
            .unwrap();

        let mut corrupted = store.read_current(&out.artifact_id).await.unwrap();
        corrupted.ir = serde_json::json!({"totally": "wrong"});
        store
            .write_version(&out.artifact_id, &VersionId::initial(), &corrupted)
            .await
            .unwrap();

        let apply = ApplyPatchUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
        };
        let err = apply
            .execute(ApplyPatchInput {
                patch: Patch {
                    artifact_id: out.artifact_id.0.clone(),
                    base_version: "v1".into(),
                    source: PatchSource::Agent,
                    ops: vec![],
                },
            })
            .await
            .unwrap_err();

        match err {
            DocumentError::IRValidationFailed { path, reason } => {
                assert_eq!(path, "/");
                assert!(reason.starts_with("parse current IR: "));
            }
            other => panic!("expected IRValidationFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_parse_error_reason_word_is_exact() {
        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let ids = Arc::new(CountingIdGenerator::default());
        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Word,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: None,
                source: PatchSource::Agent,
            })
            .await
            .unwrap();

        let mut corrupted = store.read_current(&out.artifact_id).await.unwrap();
        corrupted.ir = serde_json::json!({"totally": "wrong"});
        store
            .write_version(&out.artifact_id, &VersionId::initial(), &corrupted)
            .await
            .unwrap();

        let apply = ApplyPatchUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
        };
        let err = apply
            .execute(ApplyPatchInput {
                patch: Patch {
                    artifact_id: out.artifact_id.0.clone(),
                    base_version: "v1".into(),
                    source: PatchSource::Agent,
                    ops: vec![],
                },
            })
            .await
            .unwrap_err();

        match err {
            DocumentError::IRValidationFailed { path, reason } => {
                assert_eq!(path, "/");
                assert!(reason.starts_with("parse current Word IR: "));
            }
            other => panic!("expected IRValidationFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_parse_error_reason_html_is_exact() {
        let tmp = tempdir().unwrap();
        let store: Arc<dyn ArtifactStore> = Arc::new(LocalFsStore::new(tmp.path()));
        let ids = Arc::new(CountingIdGenerator::default());
        let create = CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
            default_retention: 10,
        };
        let out = create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Html,
                session_id: SessionId::new("s"),
                label: None,
                retention_limit: None,
                initial_ir: None,
                source: PatchSource::Agent,
            })
            .await
            .unwrap();

        let mut corrupted = store.read_current(&out.artifact_id).await.unwrap();
        corrupted.ir = serde_json::json!({"totally": "wrong"});
        store
            .write_version(&out.artifact_id, &VersionId::initial(), &corrupted)
            .await
            .unwrap();

        let apply = ApplyPatchUseCase {
            store: store.clone(),
            excel_renderer: Arc::new(ExcelRenderer),
            excel_validator: Arc::new(ExcelValidator),
            word_renderer: Arc::new(NoopR),
            word_validator: Arc::new(NoopV),
            html_renderer: Arc::new(NoopR),
            html_validator: Arc::new(NoopV),
            ids: ids.clone(),
        };
        let err = apply
            .execute(ApplyPatchInput {
                patch: Patch {
                    artifact_id: out.artifact_id.0.clone(),
                    base_version: "v1".into(),
                    source: PatchSource::Agent,
                    ops: vec![],
                },
            })
            .await
            .unwrap_err();

        match err {
            DocumentError::IRValidationFailed { path, reason } => {
                assert_eq!(path, "/");
                assert!(reason.starts_with("parse current HTML IR: "));
            }
            other => panic!("expected IRValidationFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn excel_renderer_output_is_deterministic_for_same_ir() {
        let ir_value = serde_json::json!({
            "kind": "excel",
            "artifact_id": "x", "version_id": "v1",
            "schema_version": "1.0.0",
            "workbook": { "sheets": [
                {"id": "s1", "name": "Hoja1", "order": 0, "columns": [], "cells": {}, "tables": []}
            ], "named_styles": {} }
        });
        let renderer = ExcelRenderer;
        let a = renderer.render(&ir_value).await.unwrap();
        let b = renderer.render(&ir_value).await.unwrap();
        assert!(!a.is_empty());
        assert_eq!(a, b);
    }

    /// Regression test for the one-second-resolution `creation_time` bug in
    /// `rust_xlsxwriter`: `Workbook::new()` without `set_properties` defaults
    /// `docProps/core.xml`'s `dcterms:created`/`dcterms:modified` to
    /// `ExcelDateTime::utc_now()`, which truncates to whole seconds. Two
    /// renders of the SAME IR that straddle a second boundary therefore
    /// produce DIFFERENT bytes, even though nothing about the content
    /// changed. This is why `excel_renderer_output_is_deterministic_for_same_ir`
    /// above is flaky (~1/many runs, but reliably reproducible with a
    /// deliberate delay) rather than reliably green.
    #[tokio::test]
    async fn excel_renderer_output_is_deterministic_across_a_second_boundary() {
        let ir_value = serde_json::json!({
            "kind": "excel",
            "artifact_id": "x", "version_id": "v1",
            "schema_version": "1.0.0",
            "workbook": { "sheets": [
                {"id": "s1", "name": "Hoja1", "order": 0, "columns": [], "cells": {}, "tables": []}
            ], "named_styles": {} }
        });
        let renderer = ExcelRenderer;
        let a = renderer.render(&ir_value).await.unwrap();
        // Force the two renders to straddle a wall-clock second boundary so
        // an `utc_now()`-derived timestamp is guaranteed to differ.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let b = renderer.render(&ir_value).await.unwrap();
        assert_eq!(
            a, b,
            "same IR must render to identical bytes regardless of wall-clock \
             time — content-addressability requires no wall-clock timestamp \
             to leak into the output"
        );
    }
}
