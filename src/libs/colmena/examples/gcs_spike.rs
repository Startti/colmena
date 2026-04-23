//! GCS Spike — validates the 5 behaviors we need from `google-cloud-storage`
//! before wiring it into the documents feature:
//!
//!   1. Create-only with `set_if_generation_match(0)` succeeds on a fresh key.
//!   2. Repeating the create-only call fails with HTTP 412 (object exists).
//!   3. CAS update: `set_if_generation_match(G_current)` succeeds; stale gen → 412.
//!   4. Binary roundtrip: OOXML bytes come back identical and `content_type`
//!      survives on the metadata.
//!   5. Confirms auth via ADC (no config needed beyond env).
//!
//! Check 5 re. `openssl-sys` is enforced externally:
//!   `cargo tree -e normal --features gcs | grep -i openssl`   # expect empty
//!
//! Requirements:
//!   - Set `DOCUMENTS_STORAGE_BUCKET` to a bucket you own.
//!   - ADC configured (`gcloud auth application-default login`
//!     --impersonate-service-account=...).
//!
//! Run:  `cargo run --example gcs_spike --features gcs`

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use google_cloud_storage::client::Storage;

/// GCS resource path format required by the SDK.
fn bucket_path(name: &str) -> String {
    format!("projects/_/buckets/{name}")
}

#[tokio::main]
async fn main() -> Result<()> {
    let bucket = std::env::var("DOCUMENTS_STORAGE_BUCKET")
        .context("DOCUMENTS_STORAGE_BUCKET must be set (bucket name only, no gs:// prefix)")?;
    let bucket_ref = bucket_path(&bucket);

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let prefix = format!("spike/rust-{ts}");
    let head_key = format!("{prefix}/head.json");
    let blob_key = format!("{prefix}/artifact.xlsx");

    println!("── GCS spike ───────────────────────────────────────────────");
    println!("bucket : {bucket}");
    println!("prefix : gs://{bucket}/{prefix}/");
    println!();

    let storage = Storage::builder()
        .build()
        .await
        .context("building Storage client (ADC should auto-resolve)")?;

    check_1_create_only(&storage, &bucket_ref, &head_key).await?;
    let g1 = check_1_generation(&storage, &bucket_ref, &head_key).await?;
    check_2_create_only_conflict(&storage, &bucket_ref, &head_key).await?;
    let g2 = check_3_cas_update(&storage, &bucket_ref, &head_key, g1).await?;
    check_3_cas_stale(&storage, &bucket_ref, &head_key, g1, g2).await?;
    check_4_binary_roundtrip(&storage, &bucket_ref, &blob_key).await?;

    println!();
    println!("── All 5 runtime checks passed ─────────────────────────────");
    println!("Leftover objects under gs://{bucket}/{prefix}/ — remove with:");
    println!("  gcloud storage rm -r gs://{bucket}/{prefix}/");
    println!();
    println!("Next: verify no openssl-sys is linked:");
    println!("  cargo tree -e normal --features gcs | grep -i openssl  # expect empty");
    Ok(())
}

/// Check 1: create-only on a fresh key.
async fn check_1_create_only(storage: &Storage, bucket: &str, key: &str) -> Result<()> {
    let payload = bytes::Bytes::from_static(br#"{"version":1,"kind":"head"}"#);
    let object = storage
        .write_object(bucket, key, payload)
        .set_if_generation_match(0_i64)
        .set_content_type("application/json")
        .send_buffered()
        .await
        .context("check 1: create-only write should succeed on fresh key")?;

    println!(
        "✔ check 1: created {} (generation = {})",
        key, object.generation
    );
    Ok(())
}

/// Fetch the current generation via a read.
async fn check_1_generation(storage: &Storage, bucket: &str, key: &str) -> Result<i64> {
    let mut reader = storage
        .read_object(bucket, key)
        .send()
        .await
        .context("reading head to get generation")?;
    let obj = reader.object().clone();
    // Drain body so the connection releases cleanly.
    while let Some(chunk) = reader.next().await.transpose()? {
        drop(chunk);
    }
    Ok(obj.generation)
}

/// Check 2: repeating the create-only call must 412.
async fn check_2_create_only_conflict(storage: &Storage, bucket: &str, key: &str) -> Result<()> {
    let payload = bytes::Bytes::from_static(br#"{"version":1,"kind":"head","dup":true}"#);
    let res = storage
        .write_object(bucket, key, payload)
        .set_if_generation_match(0_i64)
        .set_content_type("application/json")
        .send_buffered()
        .await;

    match res {
        Ok(obj) => Err(anyhow!(
            "check 2: create-only replay should have failed, got generation = {}",
            obj.generation
        )),
        Err(err) => {
            let code = err.http_status_code();
            if code == Some(412) {
                println!("✔ check 2: create-only replay → 412 Precondition Failed");
                Ok(())
            } else {
                Err(anyhow!(
                    "check 2: expected HTTP 412, got http_status = {code:?}, err = {err}"
                ))
            }
        }
    }
}

/// Check 3a: CAS update with the correct current generation.
async fn check_3_cas_update(
    storage: &Storage,
    bucket: &str,
    key: &str,
    expected_gen: i64,
) -> Result<i64> {
    let payload = bytes::Bytes::from_static(br#"{"version":2,"kind":"head"}"#);
    let object = storage
        .write_object(bucket, key, payload)
        .set_if_generation_match(expected_gen)
        .set_content_type("application/json")
        .send_buffered()
        .await
        .context("check 3a: CAS update with correct gen should succeed")?;

    if object.generation == expected_gen {
        return Err(anyhow!(
            "check 3a: generation did not change after successful update ({expected_gen})"
        ));
    }
    println!(
        "✔ check 3a: CAS update ok (gen {expected_gen} → {})",
        object.generation
    );
    Ok(object.generation)
}

/// Check 3b: CAS update with a stale generation must 412.
async fn check_3_cas_stale(
    storage: &Storage,
    bucket: &str,
    key: &str,
    stale_gen: i64,
    current_gen: i64,
) -> Result<()> {
    let payload = bytes::Bytes::from_static(br#"{"version":3,"kind":"head","stale":true}"#);
    let res = storage
        .write_object(bucket, key, payload)
        .set_if_generation_match(stale_gen)
        .set_content_type("application/json")
        .send_buffered()
        .await;

    match res {
        Ok(obj) => Err(anyhow!(
            "check 3b: stale-gen update should have failed, got generation = {} (current was {current_gen})",
            obj.generation
        )),
        Err(err) => {
            let code = err.http_status_code();
            if code == Some(412) {
                println!(
                    "✔ check 3b: stale-gen CAS ({stale_gen} vs current {current_gen}) → 412"
                );
                Ok(())
            } else {
                Err(anyhow!(
                    "check 3b: expected HTTP 412, got http_status = {code:?}, err = {err}"
                ))
            }
        }
    }
}

/// Check 4: binary bytes + content-type roundtrip.
async fn check_4_binary_roundtrip(storage: &Storage, bucket: &str, key: &str) -> Result<()> {
    // Synthetic binary payload — every byte from 0..=255 plus some high-entropy tail.
    let mut payload: Vec<u8> = (0u8..=255).collect();
    payload.extend_from_slice(b"\x00\xff\xfe\x01end-of-spike");
    let content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

    let written = storage
        .write_object(bucket, key, bytes::Bytes::from(payload.clone()))
        .set_if_generation_match(0_i64)
        .set_content_type(content_type)
        .send_buffered()
        .await
        .context("check 4: write binary artifact")?;

    if written.content_type != content_type {
        return Err(anyhow!(
            "check 4: content_type not preserved on write response (got `{}`)",
            written.content_type
        ));
    }

    let mut reader = storage
        .read_object(bucket, key)
        .send()
        .await
        .context("check 4: read binary artifact")?;
    let meta = reader.object().clone();
    let mut back: Vec<u8> = Vec::with_capacity(payload.len());
    while let Some(chunk) = reader.next().await.transpose()? {
        back.extend_from_slice(&chunk);
    }

    if back != payload {
        return Err(anyhow!(
            "check 4: binary mismatch — wrote {} bytes, read {} bytes",
            payload.len(),
            back.len()
        ));
    }
    if meta.content_type != content_type {
        return Err(anyhow!(
            "check 4: content_type on read metadata is `{}`, expected `{}`",
            meta.content_type,
            content_type
        ));
    }

    println!(
        "✔ check 4: binary roundtrip ok ({} bytes, content_type preserved)",
        payload.len()
    );
    Ok(())
}
