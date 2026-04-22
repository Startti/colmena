//! End-to-end integration test for the documents module:
//! create → apply_patch → read → list_versions → rollback → get_head,
//! backed by LocalFsStore.

use colmena::documents::application::apply_patch::{ApplyPatchInput, ApplyPatchUseCase};
use colmena::documents::application::create_document::{
    CreateDocumentInput, CreateDocumentUseCase,
};
use colmena::documents::application::get_head::{GetHeadInput, GetHeadUseCase};
use colmena::documents::application::list_versions::ListVersionsUseCase;
use colmena::documents::application::read_document::{ReadDocumentInput, ReadDocumentUseCase};
use colmena::documents::application::rollback::{RollbackInput, RollbackUseCase};
use colmena::documents::domain::ids::{ArtifactKind, SessionId, VersionId};
use colmena::documents::domain::patch::{Patch, PatchOp, PatchSource};
use colmena::documents::domain::{ArtifactStore, IRRenderer, IRValidator};
use colmena::documents::infrastructure::ids::CountingIdGenerator;
use colmena::documents::infrastructure::render::ExcelRenderer;
use colmena::documents::infrastructure::storage::LocalFsStore;
use colmena::documents::infrastructure::validation::ExcelValidator;
use std::sync::Arc;
use tempfile::tempdir;

struct NoopR;
#[async_trait::async_trait]
impl IRRenderer for NoopR {
    async fn render(
        &self,
        _ir: &serde_json::Value,
    ) -> Result<Vec<u8>, colmena::documents::domain::RenderError> {
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
    fn validate(
        &self,
        _ir: &serde_json::Value,
    ) -> Result<(), colmena::documents::domain::DocumentError> {
        Ok(())
    }
}

#[tokio::test]
async fn full_cycle_excel() {
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
    let apply = ApplyPatchUseCase {
        store: store.clone(),
        excel_renderer: Arc::new(ExcelRenderer),
        excel_validator: Arc::new(ExcelValidator),
        word_renderer: Arc::new(NoopR),
        word_validator: Arc::new(NoopV),
        ids: ids.clone(),
    };
    let read = ReadDocumentUseCase {
        store: store.clone(),
    };
    let list = ListVersionsUseCase {
        store: store.clone(),
    };
    let rollback = RollbackUseCase {
        store: store.clone(),
    };
    let head = GetHeadUseCase {
        store: store.clone(),
    };

    let created = create
        .execute(CreateDocumentInput {
            kind: ArtifactKind::Excel,
            session_id: SessionId::new("s"),
            label: Some("Report".into()),
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

    apply
        .execute(ApplyPatchInput {
            patch: Patch {
                artifact_id: created.artifact_id.0.clone(),
                base_version: "v1".into(),
                source: PatchSource::Agent,
                ops: vec![PatchOp::SetCell {
                    sheet_id: "s1".into(),
                    address: "A1".into(),
                    value: serde_json::json!("hello"),
                    value_type: None,
                    format: None,
                    style_ref: None,
                }],
            },
        })
        .await
        .unwrap();

    let r = read
        .execute(ReadDocumentInput {
            artifact_id: created.artifact_id.clone(),
            version: None,
        })
        .await
        .unwrap();
    assert_eq!(r.version.0, "v2");
    assert_eq!(
        r.ir["workbook"]["sheets"][0]["cells"]["A1"]["value"],
        "hello"
    );

    let versions = list.execute(&created.artifact_id, None).await.unwrap();
    assert_eq!(versions.len(), 2);

    let rb = rollback
        .execute(RollbackInput {
            artifact_id: created.artifact_id.clone(),
            to_version: VersionId::new("v1"),
        })
        .await
        .unwrap();
    assert_eq!(rb.new_version_id.0, "v3");

    let h = head
        .execute(GetHeadInput {
            artifact_id: created.artifact_id.clone(),
            since_version: None,
        })
        .await
        .unwrap();
    assert_eq!(h.current_version.0, "v3");
}
