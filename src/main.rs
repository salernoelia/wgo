mod audio_recorder;
mod config;
mod groq_request;
mod menu;
mod shortcut_detector;
mod transcription_history;
mod utils;

use audio_recorder::AudioRecorder;

use rdev::listen;
use shortcut_detector::ShortcutDetector;
use std::sync::{Arc, Mutex};
use transcription_history::{TranscriptionHistory, TranscriptionRecord};

fn main() {
    config::ensure_config_exists();
    let detector = Arc::new(ShortcutDetector::new());
    let recorder = Arc::new(Mutex::new(AudioRecorder::new()));

    println!("Listening for key events:");
    println!("  Alt+Space: Toggle recording");
    println!("  Alt+H: Show menu");
    println!("Press Ctrl+C to exit");

    if let Err(error) = listen(detector.create_callback(recorder)) {
        println!("Error: {:?}", error);
    }
}
