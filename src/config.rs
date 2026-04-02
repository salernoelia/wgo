use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::audio_recorder::AudioSource;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub groq_api_key: String,
    pub microphone_name: Option<String>,
    #[serde(default)]
    pub desktop_device_name: Option<String>,
    #[serde(default)]
    pub audio_source: AudioSource,
    pub markdown_dir: String,
    pub recordings_dir: String,
    pub markdown_pattern: String,
    pub toggle_shortcut: String,
    pub show_window_shortcut: String,
    pub minimize_on_stop: bool,
    pub hold_to_record_key: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let default_md_dir = dirs::document_dir()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            .join("wgo-transcriptions")
            .to_string_lossy()
            .to_string();

        let default_recordings_dir = Self::default_recordings_dir().to_string_lossy().to_string();

        Self {
            groq_api_key: String::new(),
            microphone_name: None,
            desktop_device_name: None,
            audio_source: AudioSource::default(),
            markdown_dir: default_md_dir,
            recordings_dir: default_recordings_dir,
            markdown_pattern: "transcription_{date}_{time}.md".to_string(),
            toggle_shortcut: "Alt+Space".to_string(),
            show_window_shortcut: "Alt+H".to_string(),
            minimize_on_stop: false,
            hold_to_record_key: Some("ControlLeft".to_string()),
        }
    }
}

impl AppConfig {
    fn merged_from(cfg: Self) -> Self {
        let mut merged = Self::default();
        merged.groq_api_key = cfg.groq_api_key;
        merged.microphone_name = cfg.microphone_name;
        merged.desktop_device_name = cfg.desktop_device_name;
        merged.audio_source = cfg.audio_source;
        merged.markdown_dir = cfg.markdown_dir;
        merged.recordings_dir = cfg.recordings_dir;
        merged.markdown_pattern = cfg.markdown_pattern;
        if !cfg.toggle_shortcut.trim().is_empty() {
            merged.toggle_shortcut = cfg.toggle_shortcut;
        }
        if !cfg.show_window_shortcut.trim().is_empty() {
            merged.show_window_shortcut = cfg.show_window_shortcut;
        }
        merged.minimize_on_stop = cfg.minimize_on_stop;
        merged.hold_to_record_key = cfg.hold_to_record_key;
        merged
    }

    fn read_config(path: &Path) -> Option<Self> {
        let content = fs::read_to_string(path).ok()?;
        let cfg = serde_json::from_str::<Self>(&content).ok()?;
        Some(Self::merged_from(cfg))
    }

    fn legacy_config_paths(primary: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();

        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let candidate = exe_dir.join("config.json");
                if candidate != primary {
                    paths.push(candidate);
                }
            }
        }

        if let Ok(cwd) = std::env::current_dir() {
            let candidate = cwd.join("config.json");
            if candidate != primary && !paths.contains(&candidate) {
                paths.push(candidate);
            }
        }

        paths
    }

    pub fn default_recordings_dir() -> PathBuf {
        dirs::document_dir()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            .join("wgo-recordings")
    }

    pub fn recordings_dir_path(&self) -> PathBuf {
        let trimmed = self.recordings_dir.trim();
        if trimmed.is_empty() {
            return Self::default_recordings_dir();
        }

        PathBuf::from(trimmed)
    }

    pub fn ensure_recordings_dir(&self) -> Result<PathBuf, String> {
        let dir = self.recordings_dir_path();
        fs::create_dir_all(&dir).map_err(|e| {
            format!(
                "Failed to create recordings directory '{}': {e}",
                dir.display()
            )
        })?;
        Ok(dir)
    }

    pub fn has_api_key(&self) -> bool {
        !self.groq_api_key.trim().is_empty()
    }

    pub fn app_data_dir() -> PathBuf {
        let base = dirs::data_local_dir()
            .or_else(dirs::data_dir)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        base.join("wgo")
    }

    pub fn config_path() -> PathBuf {
        Self::app_data_dir().join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Some(cfg) = Self::read_config(&path) {
            return cfg;
        }

        for legacy_path in Self::legacy_config_paths(&path) {
            if let Some(cfg) = Self::read_config(&legacy_path) {
                let _ = cfg.save();
                return cfg;
            }
        }

        let cfg = Self::default();
        let _ = cfg.save();
        cfg
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {e}"))?;
        }

        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {e}"))?;

        fs::write(path, content).map_err(|e| format!("Failed to write config.json: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;
    use std::path::PathBuf;

    #[test]
    fn has_api_key_is_false_for_blank_and_whitespace() {
        let mut cfg = AppConfig::default();
        cfg.groq_api_key = String::new();
        assert!(!cfg.has_api_key());

        cfg.groq_api_key = "   \n\t ".to_string();
        assert!(!cfg.has_api_key());
    }

    #[test]
    fn has_api_key_is_true_for_non_whitespace() {
        let mut cfg = AppConfig::default();
        cfg.groq_api_key = "  abc123  ".to_string();
        assert!(cfg.has_api_key());
    }

    #[test]
    fn recordings_dir_path_uses_fallback_when_config_value_is_blank() {
        let mut cfg = AppConfig::default();
        cfg.recordings_dir = "   ".to_string();

        assert_eq!(
            cfg.recordings_dir_path(),
            AppConfig::default_recordings_dir()
        );
    }

    #[test]
    fn recordings_dir_path_uses_configured_value_when_present() {
        let mut cfg = AppConfig::default();
        cfg.recordings_dir = "/tmp/wgo-recordings-test".to_string();

        assert_eq!(
            cfg.recordings_dir_path(),
            PathBuf::from("/tmp/wgo-recordings-test")
        );
    }
}
