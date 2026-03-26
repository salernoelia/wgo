mod app;
mod audio_recorder;
mod config;
mod groq_request;
mod shortcut_detector;
mod transcription_history;
mod utils;

use app::WgoApp;
use eframe::egui;
use shortcut_detector::start_global_hotkeys;
use std::sync::mpsc;

fn main() {
    let (hotkey_tx, hotkey_rx) = mpsc::channel();
    let _hotkey_runtime = start_global_hotkeys(hotkey_tx);

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
        Box::new(move |_cc| Ok(Box::new(WgoApp::new(hotkey_rx)))),
    );

    if let Err(err) = run_result {
        eprintln!("Failed to start GUI: {err}");
    }
}
