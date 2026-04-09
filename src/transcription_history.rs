use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionRecord {
    pub filename: String,
    pub transcription: String,
    pub timestamp: u64,
    #[serde(default)]
    pub audio_path: Option<String>,
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

    pub fn has_history(&self) -> bool {
        !self.records.is_empty()
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

    pub fn latest(&self) -> Option<&TranscriptionRecord> {
        self.records.last()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    // Serialize all tests that touch XDG_DATA_HOME (global env var).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn make_record(text: &str) -> TranscriptionRecord {
        TranscriptionRecord {
            filename: format!("/tmp/{text}.md"),
            transcription: text.to_string(),
            timestamp: 1_700_000_000,
            audio_path: None,
        }
    }

    /// Write JSON directly to the history file inside `dir` and return a
    /// `TranscriptionHistory` loaded from that exact path (bypasses env var).
    fn load_from_dir(dir: &std::path::Path) -> TranscriptionHistory {
        let path = dir.join("wgo").join("transcription_history.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| TranscriptionHistory::new()),
            Err(_) => TranscriptionHistory::new(),
        }
    }

    fn save_to_dir(h: &TranscriptionHistory, dir: &std::path::Path) {
        let d = dir.join("wgo");
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("transcription_history.json");
        std::fs::write(path, serde_json::to_string_pretty(h).unwrap()).unwrap();
    }

    // ── new() / has_history ─────────────────────────────────────────────────

    #[test]
    fn new_history_is_empty() {
        let h = TranscriptionHistory::new();
        assert!(!h.has_history());
        assert!(h.records.is_empty());
    }

    #[test]
    fn has_history_is_true_after_adding_record() {
        let mut h = TranscriptionHistory::new();
        h.records.push(make_record("test"));
        assert!(h.has_history());
    }

    // ── latest ──────────────────────────────────────────────────────────────

    #[test]
    fn latest_returns_none_when_empty() {
        assert!(TranscriptionHistory::new().latest().is_none());
    }

    #[test]
    fn latest_returns_last_added_record() {
        let mut h = TranscriptionHistory::new();
        h.records.push(make_record("first"));
        h.records.push(make_record("second"));
        assert_eq!(h.latest().unwrap().transcription, "second");
    }

    // ── save / load round-trip (via direct file path, no env var) ────────────

    #[test]
    fn save_then_load_round_trips_records() {
        let tmp = tempdir().expect("tempdir");
        let mut h = TranscriptionHistory::new();
        h.records.push(make_record("hello"));
        h.records.push(make_record("world"));
        save_to_dir(&h, tmp.path());

        let loaded = load_from_dir(tmp.path());
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.records[0].transcription, "hello");
        assert_eq!(loaded.records[1].transcription, "world");
    }

    #[test]
    fn load_returns_empty_history_when_file_missing() {
        let tmp = tempdir().expect("tempdir");
        let loaded = load_from_dir(tmp.path());
        assert!(!loaded.has_history());
    }

    #[test]
    fn add_record_persists_audio_path() {
        let tmp = tempdir().expect("tempdir");
        let mut h = TranscriptionHistory::new();
        h.records.push(TranscriptionRecord {
            filename: "/tmp/note.md".to_string(),
            transcription: "spoken words".to_string(),
            timestamp: 1_700_000_000,
            audio_path: Some("/tmp/recording.m4a".to_string()),
        });
        save_to_dir(&h, tmp.path());

        let loaded = load_from_dir(tmp.path());
        assert_eq!(
            loaded.records[0].audio_path.as_deref(),
            Some("/tmp/recording.m4a")
        );
    }

    // ── backward compatibility: old records without audio_path ───────────────

    #[test]
    fn load_tolerates_records_without_audio_path_field() {
        let tmp = tempdir().expect("tempdir");
        let d = tmp.path().join("wgo");
        std::fs::create_dir_all(&d).unwrap();
        // JSON without "audio_path" key — old format written before the field existed.
        std::fs::write(
            d.join("transcription_history.json"),
            r#"{"records":[{"filename":"/tmp/old.md","transcription":"old text","timestamp":1000}]}"#,
        )
        .unwrap();

        let loaded = load_from_dir(tmp.path());
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.records[0].transcription, "old text");
        assert!(loaded.records[0].audio_path.is_none());
    }

    // ── corrupt JSON falls back to empty ────────────────────────────────────

    #[test]
    fn load_returns_empty_history_on_corrupt_json() {
        let tmp = tempdir().expect("tempdir");
        let d = tmp.path().join("wgo");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("transcription_history.json"), b"not json").unwrap();

        let loaded = load_from_dir(tmp.path());
        assert!(!loaded.has_history());
    }

    // ── XDG_DATA_HOME round-trip (serialised to avoid env var races) ─────────

    #[test]
    fn save_and_load_via_env_var_round_trips() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempdir().expect("tempdir");
        std::env::set_var("XDG_DATA_HOME", tmp.path());

        let mut h = TranscriptionHistory::new();
        h.add_record(make_record("env-test"));
        let loaded = TranscriptionHistory::load();
        assert!(loaded.records.iter().any(|r| r.transcription == "env-test"));

        std::env::remove_var("XDG_DATA_HOME");
    }
}
