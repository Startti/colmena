//! `files[]` entries that carry only a `storage_key`: with the large tabular
//! switch on and a CSV/xlsx strictly above 50 MiB they become
//! `FileSource::StorageRef`; in every other case they are skipped exactly as
//! today (silently, not rejected). The entries themselves are pinned by the
//! C0 suite in `characterisation.rs`, which stays untouched.

use super::{parse_file_entries, parse_file_entries_with};
use crate::llm::domain::{FileSource, LlmError};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

const MIB: u64 = 1024 * 1024;
const LIMIT: u64 = 50 * MIB;
const CSV: &str = "text/csv";
const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

fn entries(v: Value) -> Vec<Value> {
    v.as_array().cloned().expect("an array of entries")
}

fn key_only(id: &str, mime: &str, size: Option<u64>) -> Value {
    let mut entry = json!({
        "id": id,
        "mime_type": mime,
        "filename": "big.file",
        "storage_key": format!("chat-attachments/u/s/{id}"),
    });
    if let Some(size) = size {
        entry["size_bytes"] = json!(size);
    }
    entry
}

#[test]
fn a_large_key_only_entry_becomes_a_storage_ref_when_the_switch_is_on() {
    let arr = entries(json!([key_only("doc-1", CSV, Some(LIMIT + 1))]));
    let (files, kept) = parse_file_entries_with(&arr, false, true).unwrap();
    assert_eq!(kept, [0]);
    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert_eq!(file.document_id.as_deref(), Some("doc-1"));
    assert_eq!(file.mime_type, CSV);
    assert_eq!(file.filename, "big.file");
    assert_eq!(file.size_hint, Some(LIMIT + 1));
    assert!(file.retained_inline_bytes.is_none());
    match &file.source {
        FileSource::StorageRef(key) => assert_eq!(key, "chat-attachments/u/s/doc-1"),
        other => panic!("expected StorageRef, got {other:?}"),
    }
}

#[test]
fn an_xlsx_key_only_entry_is_large_too() {
    let arr = entries(json!([key_only("doc-x", XLSX, Some(LIMIT + 1))]));
    let (files, _) = parse_file_entries_with(&arr, false, true).unwrap();
    assert!(matches!(files[0].source, FileSource::StorageRef(_)));
}

#[test]
fn the_boundary_is_strict_and_the_switch_off_skips_every_size() {
    let sizes = [LIMIT - 1, LIMIT, LIMIT + 1, 400 * MIB];
    for size in sizes {
        let arr = entries(json!([key_only("doc-1", CSV, Some(size))]));

        let (files, kept) = parse_file_entries_with(&arr, false, true).unwrap();
        let large = size > LIMIT;
        assert_eq!(files.len(), usize::from(large), "switch on, size {size}");
        assert_eq!(kept.len(), usize::from(large), "switch on, size {size}");

        let (files, kept) = parse_file_entries_with(&arr, false, false).unwrap();
        assert!(
            files.is_empty() && kept.is_empty(),
            "switch off, size {size}"
        );
    }
}

#[test]
fn the_original_entry_point_is_the_switch_off_behaviour() {
    let arr = entries(json!([key_only("doc-1", CSV, Some(400 * MIB))]));
    let (files, kept) = parse_file_entries(&arr, false).unwrap();
    assert!(files.is_empty() && kept.is_empty());
}

#[test]
fn a_missing_size_or_another_type_is_skipped_not_rejected() {
    let arr = entries(json!([
        key_only("no-size", CSV, None),
        key_only("pdf", "application/pdf", Some(90 * MIB)),
        key_only("xls", "application/vnd.ms-excel", Some(90 * MIB)),
    ]));
    let (files, kept) = parse_file_entries_with(&arr, false, true).unwrap();
    assert!(files.is_empty() && kept.is_empty());
}

#[test]
fn an_empty_storage_key_is_skipped() {
    let mut entry = key_only("doc-1", CSV, Some(LIMIT + 1));
    entry["storage_key"] = json!("");
    let (files, _) = parse_file_entries_with(&entries(json!([entry])), false, true).unwrap();
    assert!(files.is_empty());
}

#[test]
fn skipped_entries_do_not_shift_the_kept_indices() {
    let data = STANDARD.encode(b"a,b\n1,2\n");
    let arr = entries(json!([
        key_only("small-key", CSV, Some(LIMIT)),
        key_only("large-key", CSV, Some(LIMIT + 1)),
        {"id": "doc-data", "mime_type": CSV, "filename": "s.csv", "data": data},
        key_only("pdf-key", "application/pdf", Some(90 * MIB)),
    ]));
    let (files, kept) = parse_file_entries_with(&arr, false, true).unwrap();
    assert_eq!(kept, [1, 2]);
    assert_eq!(files[0].document_id.as_deref(), Some("large-key"));
    assert_eq!(files[1].document_id.as_deref(), Some("doc-data"));
}

#[test]
fn data_url_and_path_keep_their_priority_over_the_key() {
    let size = LIMIT + 1;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("s.csv");
    std::fs::write(&file, b"a\n1\n").unwrap();

    // `data` wins over the key and keeps today's 30 MiB check on the hint, so
    // a hint this large fails exactly as it does with the switch off.
    let mut with_data = key_only("d", CSV, Some(size));
    with_data["data"] = json!(STANDARD.encode(b"a\n"));
    for switch in [false, true] {
        assert!(matches!(
            parse_file_entries_with(&entries(json!([with_data.clone()])), false, switch),
            Err(LlmError::DataFieldTooLarge { size: s }) if s == size
        ));
    }

    let mut with_url = key_only("u", CSV, Some(size));
    with_url["url"] = json!("https://storage.example/x?sig=y");
    let (files, _) = parse_file_entries_with(&entries(json!([with_url])), false, true).unwrap();
    assert!(matches!(files[0].source, FileSource::SignedUrl(_)));

    let mut with_path = key_only("p", CSV, Some(size));
    with_path["path"] = json!(file.to_str().unwrap());
    let (files, _) = parse_file_entries_with(&entries(json!([with_path])), true, true).unwrap();
    assert!(matches!(files[0].source, FileSource::InlineBytes { .. }));
}

#[test]
fn a_path_still_needs_local_mode_with_the_switch_on() {
    let mut entry = key_only("p", CSV, Some(LIMIT + 1));
    entry["path"] = json!("/tmp/whatever.csv");
    assert!(matches!(
        parse_file_entries_with(&entries(json!([entry])), false, true),
        Err(LlmError::PathFieldNotAllowed)
    ));
}

#[test]
fn a_large_entry_without_an_id_is_still_a_storage_ref() {
    let mut entry = key_only("ignored", CSV, Some(LIMIT + 1));
    entry.as_object_mut().unwrap().remove("id");
    let (files, _) = parse_file_entries_with(&entries(json!([entry])), false, true).unwrap();
    assert_eq!(files[0].document_id, None);
    assert!(matches!(files[0].source, FileSource::StorageRef(_)));
}

#[test]
fn a_storage_ref_serialises_with_its_own_kind() {
    let value = serde_json::to_value(FileSource::StorageRef("k".into())).unwrap();
    assert_eq!(value, json!({"kind": "storage_ref", "data": "k"}));
}
