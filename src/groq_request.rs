use dotenv::dotenv;
use reqwest::blocking::multipart::{Form, Part};
use reqwest::blocking::Client;
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

fn get_exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn get_recordings_dir() -> PathBuf {
    let data_dir = if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            .join("wgo")
    } else {
        // Linux/macOS: use XDG_DATA_HOME or ~/.local/share
        std::env::var("XDG_DATA_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".local").join("share"))
            })
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            .join("wgo")
    };

    data_dir.join("recordings")
}

pub fn transcribe_audio(file_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    dotenv().ok();

    let config_path = get_exe_dir().join("config.json");
    let config_file = File::open(&config_path)
        .map_err(|e| format!("Failed to open config.json at {:?}: {}", config_path, e))?;
    let config: Value = serde_json::from_reader(config_file)?;
    let api_key = config["groq_api_key"]
        .as_str()
        .ok_or("groq_api_key not found or not a string in config.json")?;

    if api_key.is_empty() {
        return Err("groq_api_key is empty in config.json".into());
    }
    let url = "https://api.groq.com/openai/v1/audio/transcriptions";

    let audio_file_path = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else if file_path.contains('/') || file_path.contains('\\') {
        PathBuf::from(file_path)
    } else {
        let recordings_path = get_recordings_dir().join(file_path);
        if recordings_path.exists() {
            recordings_path
        } else {
            get_exe_dir().join(file_path)
        }
    };

    let mut file = File::open(&audio_file_path)
        .map_err(|e| format!("Failed to open audio file at {:?}: {}", audio_file_path, e))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let file_part = Part::bytes(buffer)
        .file_name(file_path.to_owned())
        .mime_str("audio/wav")?;

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
