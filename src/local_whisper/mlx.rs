#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use vona_mlx::MlxSpeechModel;
#[cfg(target_os = "macos")]
use vona_mlx_whisper::{WhisperSpeechConfig, WhisperSpeechModel, DEFAULT_WHISPER_SAMPLE_RATE_HZ};

#[cfg(target_os = "macos")]
pub fn transcribe_pcm(
    model_dir: &Path,
    pcm_samples: &[f32],
) -> Result<String, Box<dyn std::error::Error>> {
    if !model_dir.exists() {
        return Err(format!("MLX model directory not found: {}", model_dir.display()).into());
    }

    let config = WhisperSpeechConfig::new(model_dir);
    let model = WhisperSpeechModel::load(config).map_err(|e| {
        format!(
            "Failed to load MLX Whisper model from {}: {e}",
            model_dir.display()
        )
    })?;

    let audio = mlx_rs::Array::from_slice(pcm_samples, &[pcm_samples.len() as i32]);
    let transcript = model
        .transcribe(&audio, DEFAULT_WHISPER_SAMPLE_RATE_HZ)
        .map_err(|e| format!("MLX Whisper transcription failed: {e}"))?;

    let cleaned = deduplicate_repeated_phrases(transcript.trim());
    Ok(cleaned)
}

/// Collapses hallucinated phrase loops that occur during silent audio tails in greedy decoding.
pub fn deduplicate_repeated_phrases(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= 4 {
        return text.trim().to_string();
    }

    let mut result_words = words;
    let max_phrase_len = (result_words.len() / 2).min(20);

    for phrase_len in (3..=max_phrase_len).rev() {
        let mut i = 0;
        let mut cleaned = Vec::new();
        while i < result_words.len() {
            if i + phrase_len <= result_words.len() {
                let phrase = &result_words[i..i + phrase_len];
                let mut next_i = i + phrase_len;
                let mut matches = 0;
                while next_i + phrase_len <= result_words.len()
                    && &result_words[next_i..next_i + phrase_len] == phrase
                {
                    matches += 1;
                    next_i += phrase_len;
                }
                if matches >= 1 {
                    for &w in phrase {
                        cleaned.push(w);
                    }
                    i = next_i;
                    continue;
                }
            }
            cleaned.push(result_words[i]);
            i += 1;
        }
        result_words = cleaned;
    }

    result_words.join(" ")
}

#[cfg(not(target_os = "macos"))]
pub fn transcribe_pcm(
    _model_dir: &std::path::Path,
    _pcm_samples: &[f32],
) -> Result<String, Box<dyn std::error::Error>> {
    Err("MLX backend is only supported on macOS (Apple Silicon).".into())
}
