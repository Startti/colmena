//! End-to-end integration test for Word artifacts:
//! create → apply_patch (replace_run_text) → read, backed by LocalFsStore.

use colmena::documents::application::apply_patch::{ApplyPatchInput, ApplyPatchUseCase};
use colmena::documents::application::create_document::{
    CreateDocumentInput, CreateDocumentUseCase,
};
use colmena::documents::application::read_document::{ReadDocumentInput, ReadDocumentUseCase};
use colmena::documents::domain::ids::{ArtifactKind, SessionId};
use colmena::documents::domain::patch::{Patch, PatchOp, PatchSource};
use colmena::documents::domain::{ArtifactStore, IRRenderer, IRValidator, RenderError};
use colmena::documents::infrastructure::ids::CountingIdGenerator;
use colmena::documents::infrastructure::render::{ExcelRenderer, WordRenderer};
use colmena::documents::infrastructure::storage::LocalFsStore;
use colmena::documents::infrastructure::validation::{ExcelValidator, WordValidator};
use std::sync::Arc;
use tempfile::tempdir;

struct NoopR;
#[async_trait::async_trait]
impl IRRenderer for NoopR {
    async fn render(
        &self,
        _ir: &serde_json::Value,
    ) -> Result<Vec<u8>, RenderError> {
        Ok(vec![])
    }
    fn target_extension(&self) -> &'static str {
        "html"
    }
    fn target_mime(&self) -> &'static str {
        "text/html"
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
async fn word_create_replace_run_text() {
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
    let read = ReadDocumentUseCase {
        store: store.clone(),
    };

    let created = create
        .execute(CreateDocumentInput {
            kind: ArtifactKind::Word,
            session_id: SessionId::new("s"),
            label: Some("Report".into()),
            retention_limit: None,
            initial_ir: Some(serde_json::json!({
                "kind": "word",
                "artifact_id": "x", "version_id": "v1",
                "schema_version": "1.0.0",
                "document": {
                    "blocks": [
                        {"type": "paragraph", "id": "b1", "runs": [{"id": "r1", "text": "old"}]}
                    ],
                    "named_styles": {}
                }
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
                ops: vec![PatchOp::ReplaceRunText {
                    block_id: "b1".into(),
                    run_id: "r1".into(),
                    new_text: "new".into(),
                }],
            },
        })
        .await
        .unwrap();

    let r = read
        .execute(ReadDocumentInput {
            artifact_id: created.artifact_id,
            version: None,
        })
        .await
        .unwrap();
    assert_eq!(r.ir["document"]["blocks"][0]["runs"][0]["text"], "new");
}
