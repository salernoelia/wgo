mod app;
mod audio_recorder;
mod config;
mod groq_request;
mod icon;
mod local_whisper;
mod shortcut_detector;
mod transcriber;
mod transcription_history;
mod utils;

use app::WgoApp;
use config::AppConfig;
use eframe::egui;
use icon::load_app_icon;
use shortcut_detector::{start_global_hotkeys, HotkeyBindings};
use std::sync::mpsc;

fn main() {
    // Run the same native engines without starting the GUI or global hotkeys.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--transcribe-local") {
        let result = local_whisper::transcribe_cli(&args[2..]);
        match result {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }

    let config = AppConfig::load();
    let (hotkey_tx, hotkey_rx) = mpsc::channel();
    let hotkey_runtime = start_global_hotkeys(
        hotkey_tx,
        HotkeyBindings::new(
            config.toggle_shortcut.clone(),
            config.show_window_shortcut.clone(),
            config.hold_to_record_key.clone(),
        ),
    );

    let initial_title = match config.transcription_provider {
        config::TranscriptionProvider::Local => "wgo [Local]",
        config::TranscriptionProvider::Groq => "wgo [Cloud]",
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(initial_title)
        .with_inner_size([620.0, 480.0])
        // Keep the minimum small from startup so Wayland compositors that
        // ignore lowering min-size at runtime can still shrink for recording mode.
        .with_min_inner_size([420.0, 105.0]);

    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let run_result = eframe::run_native(
        "wgo",
        options,
        Box::new(move |_cc| Ok(Box::new(WgoApp::new(hotkey_rx, hotkey_runtime)))),
    );

    if let Err(err) = run_result {
        eprintln!("Failed to start GUI: {err}");
    }
}
