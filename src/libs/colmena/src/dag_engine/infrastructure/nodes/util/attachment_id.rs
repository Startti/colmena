//! Plan A: shared helpers for deriving stable `document_id` values from
//! provider-emitted artifacts. Used by `image_generation`, `image_edit`,
//! and `tts` nodes to keep document_id formatting consistent across artifact
//! producers.

/// Build a human-friendly, collision-resistant `document_id` from the
/// artifact's filename, mime type, and storage_key.
///
/// Strategy: strip the known extension, sanitize the stem to `[A-Za-z0-9_-]`,
/// append the last 6 alphanumeric chars of `storage_key` to guarantee
/// uniqueness across same-filename generations within a session. Falls back
/// to `<prefix>_<storage_key>` when the sanitized stem is empty.
///
/// `prefix` distinguishes producers: `"img"` for images, `"audio"` for tts.
pub fn build_document_id(
    filename: &str,
    mime_type: &str,
    storage_key: &str,
    prefix: &str,
) -> String {
    let stem = filename
        .strip_suffix(&format!(".{}", file_ext(mime_type)))
        .unwrap_or(filename);
    let sanitized = sanitize(stem);
    let suffix = storage_key_suffix(storage_key);
    if sanitized.is_empty() {
        format!("{}_{}", prefix, storage_key)
    } else {
        format!("{}_{}_{}", prefix, sanitized, suffix)
    }
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

/// Last 6 alphanumeric chars of the storage_key.
pub fn storage_key_suffix(storage_key: &str) -> String {
    let alphanumeric: String = storage_key
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    alphanumeric
        .chars()
        .rev()
        .take(6)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_known_extensions() {
        let key = "sk-abc123def";
        assert!(
            build_document_id("image_0.png", "image/png", key, "img").starts_with("img_image_0_")
        );
        assert!(
            build_document_id("sound.wav", "audio/wav", key, "audio").starts_with("audio_sound_")
        );
    }

    #[test]
    fn falls_back_to_storage_key_when_stem_empty() {
        assert_eq!(
            build_document_id(".png", "image/png", "sk-1", "img"),
            "img_sk-1"
        );
        assert_eq!(
            build_document_id(".wav", "audio/wav", "sk-2", "audio"),
            "audio_sk-2"
        );
    }

    #[test]
    fn avoids_collision_between_same_filename_different_keys() {
        let a = build_document_id("image_0.png", "image/png", "sk-abc123", "img");
        let b = build_document_id("image_0.png", "image/png", "sk-xyz789", "img");
        assert_ne!(a, b);
        assert!(a.starts_with("img_image_0_"));
        assert!(b.starts_with("img_image_0_"));
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
