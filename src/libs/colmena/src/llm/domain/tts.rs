//! TTS (text-to-speech) value objects shared across providers. See
//! [`super::tts_repository::TtsRepository`] for the port.

use serde::{Deserialize, Serialize};

/// Output audio container format. Each adapter maps this to the
/// provider-specific token (e.g. OpenAI uses "mp3" / "wav" / "opus" / "pcm";
/// ElevenLabs uses "mp3_44100_128" etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    #[default]
    Mp3,
    Wav,
    Opus,
    Pcm,
}

impl AudioFormat {
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
            Self::Opus => "audio/ogg",
            Self::Pcm => "audio/L16",
        }
    }

    pub fn file_extension(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Opus => "opus",
            Self::Pcm => "pcm",
        }
    }
}

impl std::str::FromStr for AudioFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "mp3" | "mpeg" => Ok(Self::Mp3),
            "wav" => Ok(Self::Wav),
            "opus" | "ogg" => Ok(Self::Opus),
            "pcm" => Ok(Self::Pcm),
            other => Err(format!(
                "unknown audio format '{other}' (expected mp3|wav|opus|pcm)"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TtsRequest {
    pub text: String,
    /// Provider-specific voice identifier. OpenAI: "alloy"/"echo"/.../.
    /// ElevenLabs: voice_id string. Google Gemini TTS: prebuilt voice name
    /// (e.g. "Kore", "Puck").
    pub voice: String,
    pub format: AudioFormat,
    /// Playback speed multiplier. Range and support varies by provider —
    /// OpenAI 0.25-4.0, Google speakingRate 0.25-4.0, ElevenLabs ignores this.
    pub speed: Option<f32>,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct TtsResponse {
    pub audio_bytes: Vec<u8>,
    pub mime_type: String,
    /// Best-effort estimate; many providers don't return duration metadata so
    /// the adapter leaves this `None` and the consumer can compute it on the
    /// fly from the bytes if needed.
    pub duration_estimate_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn audio_format_round_trip_str() {
        for f in [
            AudioFormat::Mp3,
            AudioFormat::Wav,
            AudioFormat::Opus,
            AudioFormat::Pcm,
        ] {
            let s = f.file_extension();
            assert_eq!(AudioFormat::from_str(s).unwrap(), f);
        }
    }

    #[test]
    fn audio_format_accepts_aliases() {
        assert_eq!(AudioFormat::from_str("mpeg").unwrap(), AudioFormat::Mp3);
        assert_eq!(AudioFormat::from_str("ogg").unwrap(), AudioFormat::Opus);
        assert_eq!(AudioFormat::from_str("WAV").unwrap(), AudioFormat::Wav);
    }

    #[test]
    fn audio_format_rejects_unknown() {
        assert!(AudioFormat::from_str("flac").is_err());
    }

    #[test]
    fn default_is_mp3() {
        assert_eq!(AudioFormat::default(), AudioFormat::Mp3);
    }
}
