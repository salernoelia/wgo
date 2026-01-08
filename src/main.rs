use arboard::Clipboard;
use rdev::{listen, Event};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

mod audio_recorder;
mod groq_request;
mod menu;
mod shortcut_detector;
mod transcription_history;

use audio_recorder::AudioRecorder;
use groq_request::transcribe_audio;
use serde_json::json;
use shortcut_detector::{ShortcutDetector, ShortcutType};
use std::fs;
use std::io::{self, Write};
use transcription_history::{TranscriptionHistory, TranscriptionRecord};

fn main() {
    ensure_config_exists();
    let detector = Arc::new(ShortcutDetector::new());
    let recorder = Arc::new(Mutex::new(AudioRecorder::new()));

    println!("Listening for key events:");
    println!("  Alt+Space: Toggle recording");
    println!("  Alt+H: Show menu");
    println!("Press Ctrl+C to exit");

    if let Err(error) = listen(create_callback(detector, recorder)) {
        println!("Error: {:?}", error);
    }
}

fn ensure_config_exists() {
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

fn create_callback(
    detector: Arc<ShortcutDetector>,
    recorder: Arc<Mutex<AudioRecorder>>,
) -> impl Fn(Event) {
    move |event: Event| match detector.handle_event(event) {
        Some(ShortcutType::ToggleRecording) => {
            println!("Alt+Space detected: toggling recording");
            handle_recording_toggle(&recorder);
        }
        Some(ShortcutType::ShowMenu) => {
            println!("Alt+H detected: showing menu");
            menu::show_menu(&recorder);
            println!("Listening for key events again...");
        }
        None => {}
    }
}

fn handle_recording_toggle(recorder: &Arc<Mutex<AudioRecorder>>) {
    let mut recorder = match recorder.lock() {
        Ok(recorder) => recorder,
        Err(_) => return,
    };

    let filename = match recorder.toggle_recording() {
        Some(filename) => filename,
        None => return,
    };

    println!("Transcribing audio file: {}", filename);
    process_transcription(&filename);
}

fn process_transcription(filename: &str) {
    let transcription = match transcribe_audio(filename) {
        Ok(transcription) => transcription,
        Err(e) => {
            eprintln!("Transcription error: {}", e);
            return;
        }
    };

    println!("Transcription: {}", transcription);
    copy_to_clipboard(&transcription);
    save_transcription(filename, &transcription);
}

fn copy_to_clipboard(text: &str) {
    let mut clipboard = match Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(e) => {
            eprintln!("Failed to access clipboard: {}", e);
            return;
        }
    };

    match clipboard.set_text(text) {
        Ok(_) => println!("Transcription copied to clipboard!"),
        Err(e) => eprintln!("Failed to copy to clipboard: {}", e),
    }
}

fn save_transcription(filename: &str, transcription: &str) {
    let mut history = TranscriptionHistory::load();
    let record = TranscriptionRecord {
        filename: filename.to_string(),
        transcription: transcription.to_string(),
        timestamp: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    history.add_record(record);
}
