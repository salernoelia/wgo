use serde_json::json;
use std::fs;
use std::io::{self, Write};

pub fn ensure_config_exists() {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let config_path = exe_dir.join("config.json");
    if !config_path.exists() {
        println!("No config.json found. Please enter your Groq API key:");
        print!("Groq API key: ");
        io::stdout().flush().unwrap();
        let mut api_key = String::new();
        if io::stdin().read_line(&mut api_key).is_ok() {
            let api_key = api_key.trim();
            if !api_key.is_empty() {
                let config = json!({"groq_api_key": api_key});
                if let Ok(content) = serde_json::to_string_pretty(&config) {
                    if let Err(e) = fs::write(&config_path, content) {
                        eprintln!("Failed to write config.json: {}", e);
                    } else {
                        println!("config.json created successfully.");
                    }
                }
            } else {
                eprintln!("API key cannot be empty. Please restart and provide a valid key.");
                std::process::exit(1);
            }
        } else {
            eprintln!("Failed to read input. Please restart and provide a valid key.");
            std::process::exit(1);
        }
    }
}
