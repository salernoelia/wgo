use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Prepares 16 kHz mono float32 PCM samples from any supported audio/video file.
/// If the file is already a 16 kHz mono 16-bit PCM WAV, samples are read directly.
/// Otherwise, ffmpeg is used to resample it to a temporary WAV file.
pub fn load_audio_samples_for_whisper(
    input_path: &Path,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if !input_path.exists() {
        return Err(format!("Audio file does not exist: {}", input_path.display()).into());
    }

    // Fast path: try reading directly if it's already a 16kHz mono WAV
    if let Ok(mut reader) = hound::WavReader::open(input_path) {
        let spec = reader.spec();
        if spec.channels == 1 && spec.sample_rate == 16_000 && spec.bits_per_sample == 16 {
            let samples: Result<Vec<f32>, _> = reader
                .samples::<i16>()
                .map(|s| s.map(|val| val as f32 / 32768.0))
                .collect();
            if let Ok(s) = samples {
                return Ok(s);
            }
        }
    }

    // Resample using ffmpeg
    let ffmpeg_path = crate::groq_request::find_ffmpeg().ok_or_else(|| {
        "ffmpeg not found. Please install ffmpeg to process audio files for local transcription.".to_string()
    })?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let temp_wav = std::env::temp_dir().join(format!("wgo_local_prep_{pid}_{ts}.wav"));

    let output = Command::new(&ffmpeg_path)
        .args([
            "-y",
            "-i",
            &input_path.to_string_lossy(),
            "-vn",
            "-acodec",
            "pcm_s16le",
            "-ac",
            "1",
            "-ar",
            "16000",
            &temp_wav.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("Failed to run ffmpeg: {e}"))?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&temp_wav);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg audio preparation failed: {}", stderr.trim()).into());
    }

    let result = (|| -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        let mut reader = hound::WavReader::open(&temp_wav)?;
        let samples: Result<Vec<f32>, _> = reader
            .samples::<i16>()
            .map(|s| s.map(|val| val as f32 / 32768.0))
            .collect();
        Ok(samples?)
    })();

    let _ = std::fs::remove_file(&temp_wav);
    result
}
