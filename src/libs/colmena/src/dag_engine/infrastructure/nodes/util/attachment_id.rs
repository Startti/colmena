//! Plan A: shared helpers for deriving stable `document_id` values from
//! provider-emitted artifacts. Used by `image_generation`, `image_edit`,
//! and `tts` nodes to keep document_id formatting consistent across artifact
//! producers.

use uuid::Uuid;

/// Build a human-friendly, collision-resistant `document_id` from the
/// artifact's filename, mime type, and storage_key.
///
/// Strategy: strip the known extension, sanitize the stem to `[A-Za-z0-9_-]`,
/// append the first 8 hex chars of a fresh UUID v4 to guarantee uniqueness
/// even across simultaneous calls with identical filename + storage_key.
/// Falls back to `<prefix>_<uuid8>` when the sanitized stem is empty.
///
/// `prefix` distinguishes producers: `"img"` for images, `"audio"` for tts.
///
/// **History (2026-06-07 fix):** the previous implementation used the last
/// 6 alphanumeric chars of `storage_key` as the suffix. When storage_keys
/// ended with the file extension (e.g. `chat-attachments/.../image_0.png`),
/// the last 6 alphanumeric chars were dominated by `image0png` → suffix
/// became `ge0png` regardless of the unique prefix. Two providers in
/// quick succession (Vertex Imagen + OpenAI gpt-image-1) produced
/// identical `document_id` values and one row silently overwrote the
/// other in `conversation_attachments`. Replaced with `Uuid::new_v4()`
/// for true uniqueness. See colmena BACKLOG entry "document_id collision
/// entre image_generation providers" for full diagnosis.
pub fn build_document_id(
    filename: &str,
    mime_type: &str,
    _storage_key: &str,
    prefix: &str,
) -> String {
    let stem = filename
        .strip_suffix(&format!(".{}", file_ext(mime_type)))
        .unwrap_or(filename);
    let sanitized = sanitize(stem);
    let suffix = short_uuid8();
    if sanitized.is_empty() {
        format!("{}_{}", prefix, suffix)
    } else {
        format!("{}_{}_{}", prefix, sanitized, suffix)
    }
}

/// First 8 hex chars of a fresh UUID v4. Used by `build_document_id` to
/// guarantee uniqueness across same-filename, same-storage-key calls.
/// 8 hex chars = 32 bits of randomness ≈ 4.3 × 10⁹ values → collision
/// probability is negligible for artifact volumes a single agent_session
/// will ever produce.
fn short_uuid8() -> String {
    let s = Uuid::new_v4().simple().to_string();
    s[..8].to_string()
}

/// Map MIME type to a short extension. Covers both image and audio types
/// used by Plan A's three producer nodes. Unknown mimes map to `"bin"`.
pub fn file_ext(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/m4a" => "m4a",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/L16" | "audio/l16" => "pcm",
        _ => "bin",
    }
}

/// Lower-case alphanumeric + `_` + `-`. Trims leading/trailing underscores
/// so `"Revenue Chart!.jpg"` → `"Revenue_Chart"` (not `"Revenue_Chart_"`).
pub fn sanitize(s: &str) -> String {
    let raw: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    raw.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_known_extensions_and_appends_uuid_suffix() {
        let key = "sk-abc123def";
        let img = build_document_id("image_0.png", "image/png", key, "img");
        let audio = build_document_id("sound.wav", "audio/wav", key, "audio");
        assert!(img.starts_with("img_image_0_"));
        assert!(audio.starts_with("audio_sound_"));
        // Suffix is 8 hex chars after the last underscore.
        let img_suffix = img.rsplit('_').next().unwrap();
        let audio_suffix = audio.rsplit('_').next().unwrap();
        assert_eq!(
            img_suffix.len(),
            8,
            "expected 8-char uuid suffix, got {img_suffix:?}"
        );
        assert_eq!(audio_suffix.len(), 8);
        assert!(img_suffix.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(audio_suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn falls_back_when_stem_empty() {
        let id = build_document_id(".png", "image/png", "sk-1", "img");
        assert!(id.starts_with("img_"));
        let suffix = id.trim_start_matches("img_");
        assert_eq!(suffix.len(), 8);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));

        let id2 = build_document_id(".wav", "audio/wav", "sk-2", "audio");
        assert!(id2.starts_with("audio_"));
        let suffix2 = id2.trim_start_matches("audio_");
        assert_eq!(suffix2.len(), 8);
        assert!(suffix2.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn avoids_collision_between_same_filename_different_keys() {
        // Pre-2026-06-07 behavior — distinct storage_keys produced distinct IDs.
        // Still holds with the new uuid-based suffix.
        let a = build_document_id("image_0.png", "image/png", "sk-abc123", "img");
        let b = build_document_id("image_0.png", "image/png", "sk-xyz789", "img");
        assert_ne!(a, b);
        assert!(a.starts_with("img_image_0_"));
        assert!(b.starts_with("img_image_0_"));
    }

    #[test]
    fn avoids_collision_between_same_filename_and_same_storage_key() {
        // REGRESSION TEST (2026-06-07): the previous implementation used
        // `last 6 alphanumeric chars of storage_key` as the suffix. When
        // storage_keys ended with the file extension (e.g. `image_0.png`),
        // the suffix degenerated to a function of the FILENAME (`ge0png`)
        // regardless of the unique prefix portion. Two providers in quick
        // succession (Vertex Imagen + OpenAI gpt-image-1) produced
        // identical document_ids — the second silently overwrote the
        // first in `conversation_attachments`.
        //
        // The new uuid-v4-based suffix guarantees two calls with IDENTICAL
        // arguments produce distinct IDs. This protects against the
        // worst-case scenario the original implementation missed.
        let a = build_document_id(
            "image_0.png",
            "image/png",
            "chat-attachments/sess_xyz/image_0.png",
            "img",
        );
        let b = build_document_id(
            "image_0.png",
            "image/png",
            "chat-attachments/sess_xyz/image_0.png",
            "img",
        );
        assert_ne!(
            a, b,
            "two calls with identical args must produce distinct IDs"
        );
        assert!(a.starts_with("img_image_0_"));
        assert!(b.starts_with("img_image_0_"));
    }

    #[test]
    fn avoids_collision_across_many_rapid_calls() {
        // Stronger uniqueness test — generate 1000 IDs with identical args
        // and verify all are distinct. The 8-char hex suffix has 2^32 ≈
        // 4.3 × 10⁹ values; collision probability over 1000 IDs is
        // negligible (birthday bound ~10⁻⁴).
        use std::collections::HashSet;
        let mut seen = HashSet::with_capacity(1000);
        for _ in 0..1000 {
            let id = build_document_id("image_0.png", "image/png", "same-key", "img");
            assert!(seen.insert(id.clone()), "collision on {id}");
        }
    }

    #[test]
    fn sanitize_trims_trailing_underscores() {
        assert_eq!(sanitize("Revenue Chart!"), "Revenue_Chart");
        assert_eq!(sanitize("__weird__"), "weird");
        assert_eq!(sanitize(""), "");
    }

    #[test]
    fn file_ext_handles_image_and_audio() {
        assert_eq!(file_ext("image/png"), "png");
        assert_eq!(file_ext("audio/wav"), "wav");
        assert_eq!(file_ext("audio/L16"), "pcm");
        assert_eq!(file_ext("application/pdf"), "bin");
    }
}
