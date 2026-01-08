use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionRecord {
    pub filename: String,
    pub transcription: String,
    pub timestamp: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TranscriptionHistory {
    pub records: Vec<TranscriptionRecord>,
}

impl TranscriptionHistory {
    const HISTORY_FILE: &'static str = "transcription_history.json";

    fn get_data_dir() -> PathBuf {
        if cfg!(target_os = "windows") {
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
        }
    }

    fn get_history_path() -> PathBuf {
        let data_dir = Self::get_data_dir();
        // Ensure directory exists
        let _ = std::fs::create_dir_all(&data_dir);
        data_dir.join(Self::HISTORY_FILE)
    }

    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    pub fn load() -> Self {
        let history_path = Self::get_history_path();
        if history_path.exists() {
            match fs::read_to_string(&history_path) {
                Ok(content) => match serde_json::from_str(&content) {
                    Ok(history) => history,
                    Err(_) => Self::new(),
                },
                Err(_) => Self::new(),
            }
        } else {
            Self::new()
        }
    }

    pub fn save(&self) {
        let history_path = Self::get_history_path();
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = fs::write(history_path, content);
        }
    }

    pub fn add_record(&mut self, record: TranscriptionRecord) {
        self.records.push(record);
        self.save();
    }

    pub fn list_records(&self) -> &[TranscriptionRecord] {
        &self.records
    }
}
