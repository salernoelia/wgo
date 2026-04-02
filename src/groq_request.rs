use crate::config::AppConfig;
use reqwest::blocking::multipart::{Form, Part};
use reqwest::blocking::Client;
use rust_ffmpeg::{Codec, FFmpegBuilder, Output};
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const SUPPORTED_AUDIO_EXTS: [&str; 6] = ["mp3", "wav", "m4a", "ogg", "flac", "webm"];
const SUPPORTED_VIDEO_EXTS: [&str; 7] = ["mp4", "mov", "m4v", "mkv", "avi", "webm", "mpg"];

fn mime_for_ext(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "webm" => "audio/webm",
        _ => "application/octet-stream",
    }
}

fn is_video_ext(ext: &str) -> bool {
    SUPPORTED_VIDEO_EXTS
        .iter()
        .any(|candidate| candidate == &ext)
}

pub fn is_video_file(file_path: &str) -> bool {
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    is_video_ext(&ext)
}

pub fn is_supported_media_file(file_path: &str) -> bool {
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    SUPPORTED_AUDIO_EXTS
        .iter()
        .any(|candidate| candidate == &ext)
        || SUPPORTED_VIDEO_EXTS
            .iter()
            .any(|candidate| candidate == &ext)
}

fn extract_audio_from_video(input_path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("video");
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let pid = std::process::id();
    let output_path = std::env::temp_dir().join(format!("wgo_extract_{stem}_{pid}_{ts}.wav"));

    let ffmpeg_args = FFmpegBuilder::new()
        .map_err(|e| {
            format!(
                "Failed to initialize ffmpeg (via rust_ffmpeg). Install ffmpeg and try again. Details: {e}"
            )
        })?
        .input_path(input_path.to_string_lossy().to_string())
        .output(
            Output::new(output_path.to_string_lossy().to_string())
                .no_video()
                .audio_codec(Codec::new("pcm_s16le"))
                .option("ac", "1")
                .option("ar", "16000"),
        )
        .overwrite()
        .build_args()
        .map_err(|e| format!("Failed to build ffmpeg extraction command: {e}"))?;

    let output = Command::new("ffmpeg")
        .args(ffmpeg_args)
        .output()
        .map_err(|e| {
            format!(
                "Failed to run ffmpeg for video transcription. Install ffmpeg and try again. Details: {e}"
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let details = stderr.trim();
        return Err(format!(
            "Failed to extract audio from video. ffmpeg exited with status {}. {}",
            output.status,
            if details.is_empty() {
                "No additional error output.".to_string()
            } else {
                format!("Details: {details}")
            }
        )
        .into());
    }

    Ok(output_path)
}

fn prepare_media_for_transcription(
    file_path: &Path,
) -> Result<(PathBuf, Option<PathBuf>), Box<dyn std::error::Error>> {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    if is_video_ext(&ext) {
        let extracted = extract_audio_from_video(file_path)?;
        return Ok((extracted.clone(), Some(extracted)));
    }

    Ok((file_path.to_path_buf(), None))
}

fn get_exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn get_recordings_dir() -> PathBuf {
    AppConfig::app_data_dir().join("recordings")
}

fn resolve_audio_file_path(file_path: &str) -> PathBuf {
    resolve_audio_file_path_with(file_path, &get_recordings_dir(), &get_exe_dir())
}

fn resolve_audio_file_path_with(file_path: &str, recordings_dir: &Path, exe_dir: &Path) -> PathBuf {
    if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else if file_path.contains('/') || file_path.contains('\\') {
        PathBuf::from(file_path)
    } else {
        let recordings_path = recordings_dir.join(file_path);
        if recordings_path.exists() {
            recordings_path
        } else {
            exe_dir.join(file_path)
        }
    }
}

pub fn transcribe_audio(file_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let config = AppConfig::load();
    let api_key = config.groq_api_key.trim();

    if api_key.is_empty() {
        return Err("Groq API key is empty. Set it in the app settings.".into());
    }

    let url = "https://api.groq.com/openai/v1/audio/transcriptions";

    let media_file_path = resolve_audio_file_path(file_path);

    let (audio_file_path, cleanup_path) = prepare_media_for_transcription(&media_file_path)?;

    let mut file = File::open(&audio_file_path)
        .map_err(|e| format!("Failed to open audio file at {:?}: {}", audio_file_path, e))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let file_name = audio_file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_owned());

    let mime = audio_file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(mime_for_ext)
        .unwrap_or("application/octet-stream");

    let file_part = Part::bytes(buffer).file_name(file_name).mime_str(mime)?;

    let form = Form::new()
        .part("file", file_part)
        .text("model", "whisper-large-v3");

    let client = Client::new();
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()?;

    let result = if response.status().is_success() {
        let json: Value = response.json()?;

        if let Some(text) = json.get("text").and_then(Value::as_str) {
            Ok(text.to_owned())
        } else {
            Ok("No transcription text found in response".to_owned())
        }
    } else {
        let status = response.status();
        let error_body = response
            .text()
            .unwrap_or_else(|_| "Unable to read response body".to_string());
        Err(format!("API request failed with status {}: {}", status, error_body).into())
    };

    if let Some(path) = cleanup_path {
        let _ = std::fs::remove_file(path);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolve_audio_file_path_prefers_recordings_for_plain_filename_when_present() {
        let recordings = tempdir().expect("recordings dir");
        let exe = tempdir().expect("exe dir");
        let filename = "x.wav";
        std::fs::write(recordings.path().join(filename), "audio").expect("seed file");

        let resolved = resolve_audio_file_path_with(filename, recordings.path(), exe.path());
        assert_eq!(resolved, recordings.path().join(filename));
    }

    #[test]
    fn resolve_audio_file_path_falls_back_to_exe_for_plain_filename_when_missing() {
        let recordings = tempdir().expect("recordings dir");
        let exe = tempdir().expect("exe dir");
        let filename = "missing.wav";

        let resolved = resolve_audio_file_path_with(filename, recordings.path(), exe.path());
        assert_eq!(resolved, exe.path().join(filename));
    }

    #[test]
    fn resolve_audio_file_path_keeps_relative_paths_with_separator() {
        let recordings = tempdir().expect("recordings dir");
        let exe = tempdir().expect("exe dir");
        let relative = "nested/file.wav";

        let resolved = resolve_audio_file_path_with(relative, recordings.path(), exe.path());
        assert_eq!(resolved, PathBuf::from(relative));
    }

    #[test]
    fn support_check_allows_audio_and_video_extensions() {
        assert!(is_supported_media_file("clip.wav"));
        assert!(is_supported_media_file("movie.mp4"));
        assert!(is_supported_media_file("movie.MOV"));
        assert!(!is_supported_media_file("notes.txt"));
    }

    #[test]
    fn is_video_file_detects_video_extensions() {
        assert!(is_video_file("movie.mp4"));
        assert!(is_video_file("movie.mkv"));
        assert!(!is_video_file("audio.m4a"));
        assert!(!is_video_file("audio.wav"));
    }

    #[test]
    fn extract_audio_from_real_test_material_videos() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_material");
        assert!(
            base.exists(),
            "Missing test_material directory at {}",
            base.display()
        );

        let samples = ["test_video.mp4", "test_video.mov", "test_video.mkv"];

        for file in samples {
            let input = base.join(file);
            assert!(input.exists(), "Missing test sample: {}", input.display());

            let extracted = extract_audio_from_video(&input)
                .unwrap_or_else(|e| panic!("Failed to extract from {}: {e}", input.display()));

            let metadata = std::fs::metadata(&extracted)
                .unwrap_or_else(|e| panic!("No extracted file {}: {e}", extracted.display()));
            assert!(
                metadata.len() > 44,
                "Extracted WAV seems empty for {}",
                input.display()
            );

            let bytes = std::fs::read(&extracted)
                .unwrap_or_else(|e| panic!("Failed to read {}: {e}", extracted.display()));
            assert!(bytes.starts_with(b"RIFF"), "Output is not a RIFF WAV file");

            let _ = std::fs::remove_file(extracted);
        }
    }
}
