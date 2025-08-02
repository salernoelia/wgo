use dotenv::dotenv;
use reqwest::blocking::Client;
use reqwest::blocking::multipart::{Form, Part};
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

pub fn transcribe_audio(file_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    dotenv().ok();

    let config_path = get_exe_dir().join("config.json");
    let config_file = File::open(&config_path)?;
    let config: Value = serde_json::from_reader(config_file)?;
    let api_key = config["groq_api_key"]
        .as_str()
        .ok_or("groq_api_key not found or not a string in config.json")?;

    if api_key.is_empty() {
        return Err("groq_api_key is empty in config.json".into());
    }
    let url = "https://api.groq.com/openai/v1/audio/transcriptions";

    // Handle file path - if it's just a filename, make it relative to exe dir
    let audio_file_path = if Path::new(file_path).is_absolute()
        || file_path.contains('/')
        || file_path.contains('\\')
    {
        PathBuf::from(file_path)
    } else {
        get_exe_dir().join(file_path)
    };

    let mut file = File::open(&audio_file_path)?;
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
