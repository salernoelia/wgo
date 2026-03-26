use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub groq_api_key: String,
    pub microphone_name: Option<String>,
    pub markdown_dir: String,
    pub markdown_pattern: String,
    pub toggle_shortcut: String,
    pub show_window_shortcut: String,
    pub minimize_on_stop: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        let default_md_dir = dirs::document_dir()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            .join("wgo-transcriptions")
            .to_string_lossy()
            .to_string();

        Self {
            groq_api_key: String::new(),
            microphone_name: None,
            markdown_dir: default_md_dir,
            markdown_pattern: "transcription_{date}_{time}.md".to_string(),
            toggle_shortcut: "Alt+Space".to_string(),
            show_window_shortcut: "Alt+H".to_string(),
            minimize_on_stop: false,
        }
    }
}

impl AppConfig {
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
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<Self>(&content) {
                let mut merged = Self::default();
                merged.groq_api_key = cfg.groq_api_key;
                merged.microphone_name = cfg.microphone_name;
                merged.markdown_dir = cfg.markdown_dir;
                merged.markdown_pattern = cfg.markdown_pattern;
                if !cfg.toggle_shortcut.trim().is_empty() {
                    merged.toggle_shortcut = cfg.toggle_shortcut;
                }
                if !cfg.show_window_shortcut.trim().is_empty() {
                    merged.show_window_shortcut = cfg.show_window_shortcut;
                }
                merged.minimize_on_stop = cfg.minimize_on_stop;
                return merged;
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
