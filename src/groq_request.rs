use crate::config::AppConfig;
use reqwest::blocking::multipart::{Form, Part};
use reqwest::blocking::Client;
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

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

fn get_exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn get_recordings_dir() -> PathBuf {
    AppConfig::load().recordings_dir_path()
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

    let audio_file_path = resolve_audio_file_path(file_path);

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

    if response.status().is_success() {
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
    }
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
}
