mod app;
mod audio_recorder;
mod config;
mod groq_request;
mod shortcut_detector;
mod transcription_history;
mod utils;

use app::WgoApp;
use config::AppConfig;
use eframe::egui;
use shortcut_detector::{start_global_hotkeys, HotkeyBindings};
use std::sync::mpsc;

fn main() {
    let config = AppConfig::load();
    let (hotkey_tx, hotkey_rx) = mpsc::channel();
    let hotkey_runtime = start_global_hotkeys(
        hotkey_tx,
        HotkeyBindings::new(
            config.toggle_shortcut.clone(),
            config.show_window_shortcut.clone(),
        ),
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("wgo")
            .with_inner_size([960.0, 580.0])
            .with_min_inner_size([760.0, 420.0]),
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
