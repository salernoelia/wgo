use crate::audio_recorder::AudioRecorder;
use crate::config::AppConfig;
use crate::shortcut_detector::{is_accessibility_trusted, HotkeyBindings, HotkeyCommand, HotkeyRuntime};
use crate::transcription_history::{TranscriptionHistory, TranscriptionRecord};
use chrono::Local;
use eframe::egui;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

const MIC_GRAPH_HISTORY_LEN: usize = 120;
const MIC_GRAPH_SAMPLE_INTERVAL: Duration = Duration::from_millis(16);
const WINDOW_NORMAL_SIZE: [f32; 2] = [620.0, 480.0];
const WINDOW_RECORDING_SIZE: [f32; 2] = [420.0, 130.0];
const WINDOW_RECORDING_MARGIN: f32 = 16.0;

pub struct WgoApp {
    recorder: Arc<Mutex<AudioRecorder>>,
    config: AppConfig,
    hotkey_runtime: HotkeyRuntime,
    microphones: Vec<String>,
    selected_microphone: Option<String>,
    pending_toggle_shortcut: String,
    pending_show_shortcut: String,
    pending_hold_key: String,
    recording_target: Option<ShortcutTarget>,
    recording_by_hold: bool,
    hotkey_rx: Receiver<HotkeyCommand>,
    ui_event_rx: Receiver<UiEvent>,
    ui_event_tx: mpsc::Sender<UiEvent>,
    active_tab: AppTab,
    mic_level_history: Vec<f32>,
    last_mic_graph_sample: Instant,
    window_restore_outer_pos: Option<egui::Pos2>,
    window_restore_inner_size: Option<egui::Vec2>,
    status_line: String,
    last_transcription: String,
    last_failed_audio_path: Option<String>,
    update_state: UpdateState,
    history: TranscriptionHistory,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppTab {
    Recorder,
    Settings,
}

#[derive(Clone, Copy)]
enum ShortcutTarget {
    Toggle,
    ShowWindow,
    HoldKey,
}

enum UiEvent {
    TranscriptionReady {
        audio_path: String,
        text: String,
        markdown_path: Option<PathBuf>,
    },
    TranscriptionFailed {
        audio_path: String,
        error: String,
    },
    UpdateAvailable {
        version: String,
        html_url: String,
        download_url: String,
    },
    UpToDate,
    UpdateCheckFailed(String),
    UpdateApplied,
    UpdateInstallFailed(String),
}

#[derive(Default)]
enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpdateAvailable {
        version: String,
        html_url: String,
        download_url: String,
    },
    UpToDate,
    Updating,
    UpdateDone,
    Failed(String),
}

impl WgoApp {
    pub fn new(hotkey_rx: Receiver<HotkeyCommand>, hotkey_runtime: HotkeyRuntime) -> Self {
        let config = AppConfig::load();
        let recorder = Arc::new(Mutex::new(AudioRecorder::new()));

        if let Ok(mut rec) = recorder.lock() {
            rec.set_device_name(config.microphone_name.clone());
        }

        let microphones = AudioRecorder::list_input_devices().unwrap_or_default();

        let (ui_event_tx, ui_event_rx) = mpsc::channel();

        let history = TranscriptionHistory::load();

        spawn_update_check(ui_event_tx.clone());

        Self {
            recorder,
            hotkey_runtime,
            selected_microphone: config.microphone_name.clone(),
            pending_toggle_shortcut: config.toggle_shortcut.clone(),
            pending_show_shortcut: config.show_window_shortcut.clone(),
            pending_hold_key: config.hold_to_record_key.clone().unwrap_or_default(),
            recording_target: None,
            recording_by_hold: false,
            config,
            microphones,
            hotkey_rx,
            ui_event_rx,
            ui_event_tx,
            active_tab: AppTab::Recorder,
            mic_level_history: vec![0.0; MIC_GRAPH_HISTORY_LEN],
            last_mic_graph_sample: Instant::now(),
            window_restore_outer_pos: None,
            window_restore_inner_size: None,
            status_line: "Ready".to_string(),
            last_transcription: String::new(),
            last_failed_audio_path: None,
            update_state: UpdateState::Checking,
            history,
        }
    }

    fn start_transcription_job(&self, audio_path: String) {
        let cfg = self.config.clone();
        let ui_tx = self.ui_event_tx.clone();

        std::thread::spawn(
            move || match crate::groq_request::transcribe_audio(&audio_path) {
                Ok(text) => {
                    crate::utils::copy_to_clipboard(&text);
                    AudioRecorder::save_transcription(&audio_path, &text);

                    let md_path = match save_transcription_markdown(&cfg, &audio_path, &text) {
                        Ok(path) => Some(path),
                        Err(err) => {
                            let _ = ui_tx.send(UiEvent::TranscriptionFailed {
                                audio_path: audio_path.clone(),
                                error: format!(
                                    "Transcription succeeded but markdown save failed: {err}"
                                ),
                            });
                            None
                        }
                    };

                    let _ = ui_tx.send(UiEvent::TranscriptionReady {
                        audio_path,
                        text,
                        markdown_path: md_path,
                    });
                }
                Err(err) => {
                    let _ = ui_tx.send(UiEvent::TranscriptionFailed {
                        audio_path,
                        error: format!("Transcription error: {err}"),
                    });
                }
            },
        );
    }

    fn sample_mic_graph_if_due(&mut self) {
        let now = Instant::now();
        let mut updates = 0usize;

        while now.duration_since(self.last_mic_graph_sample) >= MIC_GRAPH_SAMPLE_INTERVAL {
            self.last_mic_graph_sample += MIC_GRAPH_SAMPLE_INTERVAL;
            self.push_audio_level_sample();
            updates += 1;

            if updates >= 8 {
                self.last_mic_graph_sample = now;
                break;
            }
        }
    }

    fn push_audio_level_sample(&mut self) {
        let level = self
            .recorder
            .lock()
            .map(|r| r.input_level())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);

        if !self.mic_level_history.is_empty() {
            self.mic_level_history.remove(0);
        }
        self.mic_level_history.push(level);
    }

    fn apply_hotkeys(&mut self, ctx: &egui::Context) {
        while let Ok(cmd) = self.hotkey_rx.try_recv() {
            match cmd {
                HotkeyCommand::ToggleRecording => {
                    if self.is_recording() {
                        self.stop_recording(ctx);
                    } else {
                        self.start_recording(ctx);
                    }
                }
                HotkeyCommand::ShowWindow => self.bring_to_front(ctx),
                HotkeyCommand::StartHoldRecording => {
                    if !self.is_recording() {
                        self.recording_by_hold = true;
                        self.start_recording(ctx);
                    }
                }
                HotkeyCommand::StopHoldRecording => {
                    if self.is_recording() && self.recording_by_hold {
                        self.recording_by_hold = false;
                        self.stop_recording(ctx);
                    }
                }
                HotkeyCommand::HoldKeyCaptured(key_name) => {
                    self.pending_hold_key = key_name;
                    self.recording_target = None;
                    self.status_line = "Hold key captured. Save settings to apply.".to_string();
                }
                HotkeyCommand::AccessibilityRequired => {
                    self.status_line =
                        "Hold-to-record needs Accessibility permission. See Settings.".to_string();
                    self.active_tab = AppTab::Settings;
                    self.bring_to_front(ctx);
                }
            }
        }
    }

    fn apply_shortcut_recording(&mut self, ctx: &egui::Context) {
        let Some(target) = self.recording_target else {
            return;
        };

        let mut captured = None;
        ctx.input(|input| {
            for event in &input.events {
                if let egui::Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } = event
                {
                    if !pressed {
                        continue;
                    }

                    if let Some(shortcut) = key_event_to_shortcut(*key, *modifiers) {
                        captured = Some(shortcut);
                        break;
                    }
                }
            }
        });

        if let Some(shortcut) = captured {
            match target {
                ShortcutTarget::Toggle => self.pending_toggle_shortcut = shortcut,
                ShortcutTarget::ShowWindow => self.pending_show_shortcut = shortcut,
                ShortcutTarget::HoldKey => {} // handled via rdev callback
            }
            if !matches!(target, ShortcutTarget::HoldKey) {
                self.recording_target = None;
                self.status_line =
                    "Shortcut captured. Save settings to apply globally.".to_string();
            }
        }
    }

    fn apply_ui_events(&mut self) {
        while let Ok(event) = self.ui_event_rx.try_recv() {
            match event {
                UiEvent::TranscriptionReady {
                    audio_path,
                    text,
                    markdown_path,
                } => {
                    self.last_transcription = text.clone();
                    self.last_failed_audio_path = None;

                    if let Some(path) = &markdown_path {
                        let timestamp = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        self.history.add_record(TranscriptionRecord {
                            filename: path.to_string_lossy().to_string(),
                            transcription: text,
                            timestamp,
                        });
                    }

                    self.status_line = match markdown_path {
                        Some(path) => format!(
                            "Audio: {} \nMarkdown:{}",
                            audio_path,
                            path.to_string_lossy()
                        ),
                        None => format!("Transcribed {}", audio_path),
                    };
                }
                UiEvent::TranscriptionFailed { audio_path, error } => {
                    self.last_failed_audio_path = Some(audio_path.clone());
                    self.status_line = format!("{error} | You can retry for {audio_path}");
                }
                UiEvent::UpdateAvailable {
                    version,
                    html_url,
                    download_url,
                } => {
                    self.update_state = UpdateState::UpdateAvailable {
                        version,
                        html_url,
                        download_url,
                    };
                }
                UiEvent::UpToDate => {
                    self.update_state = UpdateState::UpToDate;
                }
                UiEvent::UpdateCheckFailed(err) => {
                    self.update_state = UpdateState::Failed(err);
                }
                UiEvent::UpdateApplied => {
                    self.update_state = UpdateState::UpdateDone;
                    self.status_line =
                        "Update installed. Restart wgo to use the new version.".to_string();
                }
                UiEvent::UpdateInstallFailed(err) => {
                    self.update_state = UpdateState::Failed(err.clone());
                    self.status_line = format!("Update failed: {err}");
                }
            }
        }
    }

    fn check_for_updates(&mut self) {
        self.update_state = UpdateState::Checking;
        spawn_update_check(self.ui_event_tx.clone());
    }

    fn perform_self_update(&mut self, download_url: String) {
        self.update_state = UpdateState::Updating;
        self.status_line = "Downloading update...".to_string();
        let ui_tx = self.ui_event_tx.clone();

        std::thread::spawn(move || {
            match do_self_update(&download_url) {
                Ok(()) => {
                    let _ = ui_tx.send(UiEvent::UpdateApplied);
                }
                Err(e) => {
                    let _ = ui_tx.send(UiEvent::UpdateInstallFailed(e));
                }
            }
        });
    }

    fn bring_to_front(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
            egui::UserAttentionType::Informational,
        ));
    }

    fn enter_recording_mode(&mut self, ctx: &egui::Context) {
        let viewport_state = ctx.input(|i| i.viewport().clone());
        self.window_restore_outer_pos = viewport_state.outer_rect.map(|rect| rect.min);
        self.window_restore_inner_size = viewport_state.inner_rect.map(|rect| rect.size());

        self.bring_to_front(ctx);

        let x = ctx
            .input(|i| i.viewport().monitor_size)
            .map(|size| (size.x - WINDOW_RECORDING_SIZE[0] - WINDOW_RECORDING_MARGIN).max(0.0))
            .unwrap_or(WINDOW_RECORDING_MARGIN);

        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            WINDOW_RECORDING_SIZE[0],
            WINDOW_RECORDING_SIZE[1],
        )));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::Normal,
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::AlwaysOnTop,
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
            x,
            WINDOW_RECORDING_MARGIN,
        )));
    }

    fn exit_recording_mode(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::Normal,
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
            self.window_restore_inner_size
                .unwrap_or(egui::vec2(WINDOW_NORMAL_SIZE[0], WINDOW_NORMAL_SIZE[1])),
        ));

        if let Some(pos) = self.window_restore_outer_pos {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
    }

    fn is_recording(&self) -> bool {
        self.recorder
            .lock()
            .map(|r| r.is_recording())
            .unwrap_or(false)
    }

    fn is_paused(&self) -> bool {
        self.recorder.lock().map(|r| r.is_paused()).unwrap_or(false)
    }

    fn is_monitoring(&self) -> bool {
        self.recorder
            .lock()
            .map(|r| r.is_monitoring())
            .unwrap_or(false)
    }

    fn refresh_microphones(&mut self) {
        self.microphones = AudioRecorder::list_input_devices().unwrap_or_default();
    }

    fn save_settings(&mut self) {
        self.config.microphone_name = self.selected_microphone.clone();
        self.config.toggle_shortcut = self.pending_toggle_shortcut.clone();
        self.config.show_window_shortcut = self.pending_show_shortcut.clone();
        self.config.hold_to_record_key = if self.pending_hold_key.trim().is_empty() {
            None
        } else {
            Some(self.pending_hold_key.trim().to_string())
        };
        if let Ok(mut rec) = self.recorder.lock() {
            rec.set_device_name(self.config.microphone_name.clone());
        }

        match self.config.save() {
            Ok(()) => {
                self.hotkey_runtime.update_bindings(HotkeyBindings::new(
                    self.config.toggle_shortcut.clone(),
                    self.config.show_window_shortcut.clone(),
                    self.config.hold_to_record_key.clone(),
                ));
                self.status_line = "Settings saved and hotkeys updated".to_string();
            }
            Err(err) => self.status_line = err,
        }
    }

    fn start_recording(&mut self, ctx: &egui::Context) {
        if !has_non_empty_api_key(&self.config) {
            self.active_tab = AppTab::Settings;
            self.status_line =
                "Set a Groq API key in Settings before starting a recording.".to_string();
            self.bring_to_front(ctx);
            return;
        }

        let start_result = match self.recorder.lock() {
            Ok(mut recorder) => recorder.start_recording(),
            Err(_) => {
                self.status_line = "Failed to lock recorder".to_string();
                return;
            }
        };

        match start_result {
            Ok(path) => {
                self.status_line = format!("Recording started: {path}");
                self.enter_recording_mode(ctx);
            }
            Err(err) => {
                self.status_line = err;
            }
        }
    }

    fn toggle_microphone_test(&mut self) {
        let result = match self.recorder.lock() {
            Ok(mut recorder) => {
                if recorder.is_monitoring() {
                    recorder.stop_monitoring().map(|_| false)
                } else {
                    recorder.start_monitoring().map(|_| true)
                }
            }
            Err(_) => {
                self.status_line = "Failed to lock recorder".to_string();
                return;
            }
        };

        match result {
            Ok(true) => self.status_line = "Microphone test started".to_string(),
            Ok(false) => self.status_line = "Microphone test stopped".to_string(),
            Err(err) => self.status_line = err,
        }
    }

    fn request_microphone_permission(&mut self) {
        if self.is_recording() {
            self.status_line =
                "Stop the active recording before requesting microphone permissions.".to_string();
            return;
        }

        let result = match self.recorder.lock() {
            Ok(mut recorder) => match recorder.start_monitoring() {
                Ok(()) => recorder.stop_monitoring(),
                Err(err) => Err(err),
            },
            Err(_) => {
                self.status_line = "Failed to lock recorder".to_string();
                return;
            }
        };

        match result {
            Ok(()) => {
                self.status_line = "Microphone permission request was triggered. If no prompt appeared, open system microphone settings and grant access for this app.".to_string();
            }
            Err(err) => {
                self.status_line =
                    format!("{err} | Use 'Open microphone settings' to grant permission manually.");
            }
        }
    }

    fn open_microphone_settings(&mut self) {
        match crate::utils::open_microphone_permissions_settings() {
            Ok(()) => {
                self.status_line =
                    "Opened system microphone settings. Grant access for this app, then retry."
                        .to_string();
            }
            Err(err) => self.status_line = err,
        }
    }

    fn pause_or_resume_recording(&mut self) {
        match self.recorder.lock() {
            Ok(mut recorder) => {
                let result = if recorder.is_paused() {
                    recorder.resume_recording()
                } else {
                    recorder.pause_recording()
                };
                match result {
                    Ok(()) => {
                        self.status_line = if recorder.is_paused() {
                            "Recording paused".to_string()
                        } else {
                            "Recording resumed".to_string()
                        }
                    }
                    Err(err) => self.status_line = err,
                }
            }
            Err(_) => self.status_line = "Failed to lock recorder".to_string(),
        }
    }

    fn stop_recording(&mut self, ctx: &egui::Context) {
        let filename = match self.recorder.lock() {
            Ok(mut recorder) => match recorder.stop_recording() {
                Ok(Some(path)) => path,
                Ok(None) => {
                    self.status_line = "No active recording".to_string();
                    return;
                }
                Err(err) => {
                    self.status_line = err;
                    return;
                }
            },
            Err(_) => {
                self.status_line = "Failed to lock recorder".to_string();
                return;
            }
        };

        self.exit_recording_mode(ctx);
        if self.config.minimize_on_stop {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        self.status_line = format!("Recording stopped. Transcribing {}...", filename);
        self.start_transcription_job(filename);
    }

    fn cancel_recording(&mut self, ctx: &egui::Context) {
        let filename = match self.recorder.lock() {
            Ok(mut recorder) => match recorder.stop_recording() {
                Ok(Some(path)) => path,
                Ok(None) => {
                    self.status_line = "No active recording".to_string();
                    return;
                }
                Err(err) => {
                    self.status_line = err;
                    return;
                }
            },
            Err(_) => {
                self.status_line = "Failed to lock recorder".to_string();
                return;
            }
        };

        self.exit_recording_mode(ctx);

        match std::fs::remove_file(&filename) {
            Ok(()) => self.status_line = "Recording cancelled and discarded.".to_string(),
            Err(err) => {
                self.status_line = format!("Recording cancelled, but could not delete file: {err}")
            }
        }
    }

    fn retry_last_transcription(&mut self) {
        let Some(audio_path) = self.last_failed_audio_path.clone() else {
            self.status_line = "No failed transcription to retry.".to_string();
            return;
        };

        if !has_non_empty_api_key(&self.config) {
            self.active_tab = AppTab::Settings;
            self.status_line =
                "Cannot retry without a Groq API key. Add one in Settings.".to_string();
            return;
        }

        self.status_line = format!("Retrying transcription for {audio_path}...");
        self.start_transcription_job(audio_path);
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        egui::Area::new(egui::Id::new("save_btn"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-10.0, 84.0))
            .show(ui.ctx(), |ui| {
                let btn =
                    egui::Button::new("Save settings").fill(egui::Color32::from_rgb(120, 217, 120));
                if ui.add(btn).clicked() {
                    self.save_settings();
                }
            });
        ui.label("Groq API key");
        ui.add(
            egui::TextEdit::singleline(&mut self.config.groq_api_key)
                .password(true)
                .hint_text("Enter your Groq API key"),
        );
        if !has_non_empty_api_key(&self.config) {
            ui.small(
                egui::RichText::new("API key is required before you can start recording.")
                    .color(ui.visuals().warn_fg_color),
            );
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Microphone");
            if ui.button("Refresh").clicked() {
                self.refresh_microphones();
            }
        });

        egui::ComboBox::from_id_salt("microphone_combo")
            .selected_text(
                self.selected_microphone
                    .clone()
                    .unwrap_or_else(|| "Default device".to_string()),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.selected_microphone, None, "Default device");
                for mic in &self.microphones {
                    ui.selectable_value(&mut self.selected_microphone, Some(mic.clone()), mic);
                }
            });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Request microphone permission").clicked() {
                self.request_microphone_permission();
            }
            if ui.button("Open microphone settings").clicked() {
                self.open_microphone_settings();
            }
        });

        ui.add_space(8.0);

        ui.label("Markdown output folder");

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.config.markdown_dir)
                    .hint_text("/path/to/transcriptions"),
            );
            if ui.button("Open Directory").clicked() {
                match &self.config.markdown_dir {
                    path if path.trim().is_empty() => {
                        self.status_line = "Markdown directory is not set.".to_string();
                    }
                    path => {
                        let result = crate::utils::open_folder_in_file_explorer(path);
                        if let Err(err) = result {
                            self.status_line = format!("Failed to open directory: {err}");
                        }
                    }
                }
            }
        });

        ui.label("Audio Recordings output folder");

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.config.recordings_dir)
                    .hint_text("/path/to/recordings"),
            );
            if ui.button("Open Directory").clicked() {
                match &self.config.recordings_dir {
                    path if path.trim().is_empty() => {
                        self.status_line = "Recordings directory is not set.".to_string();
                    }
                    path => {
                        if let Err(err) = fs::create_dir_all(path) {
                            self.status_line =
                                format!("Failed to create recordings directory: {err}");
                        } else {
                            let result = crate::utils::open_folder_in_file_explorer(path);
                            if let Err(err) = result {
                                self.status_line = format!("Failed to open directory: {err}");
                            }
                        }
                    }
                }
            }
        });

        ui.add_space(8.0);
        ui.label("Markdown file name pattern");

        ui.add(
            egui::TextEdit::singleline(&mut self.config.markdown_pattern)
                .hint_text("transcription_{date}_{time}.md"),
        );

        ui.small("Tokens: {date}, {time}, {timestamp}");

        ui.add_space(8.0);
        ui.checkbox(
            &mut self.config.minimize_on_stop,
            "Minimize window when stopping recording",
        );

        ui.add_space(12.0);
        ui.label("Toggle recording shortcut");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.pending_toggle_shortcut)
                    .hint_text("Alt+Space"),
            );
            if ui.button("Record").clicked() {
                self.recording_target = Some(ShortcutTarget::Toggle);
                self.status_line =
                    "Press a shortcut (must include at least one modifier).".to_string();
            }
        });

        ui.add_space(8.0);
        ui.label("Show window shortcut");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.pending_show_shortcut).hint_text("Alt+H"));
            if ui.button("Record").clicked() {
                self.recording_target = Some(ShortcutTarget::ShowWindow);
                self.status_line =
                    "Press a shortcut (must include at least one modifier).".to_string();
            }
        });

        if let Some(target) = self.recording_target {
            let label = match target {
                ShortcutTarget::Toggle => "Listening for toggle shortcut...",
                ShortcutTarget::ShowWindow => "Listening for show-window shortcut...",
                ShortcutTarget::HoldKey => "Listening for hold key...",
            };
            ui.small(label);
        }

        ui.add_space(8.0);
        ui.label("Hold-to-record key");
        ui.small("Hold this key to record; release to stop. Works with single keys like AltGr, F13, ControlRight.");

        // macOS: show Accessibility permission status — required for CGEventTap.
        #[cfg(target_os = "macos")]
        {
            let trusted = is_accessibility_trusted();
            ui.horizontal(|ui| {
                if trusted {
                    ui.label(
                        egui::RichText::new("✓ Accessibility permission granted")
                            .color(egui::Color32::from_rgb(80, 180, 80))
                            .small(),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("✗ Accessibility permission required")
                            .color(egui::Color32::from_rgb(220, 80, 80))
                            .small(),
                    );
                    if ui.small_button("Open System Settings").clicked() {
                        let _ = std::process::Command::new("open")
                            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
                            .spawn();
                    }
                }
            });
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.pending_hold_key)
                    .hint_text("e.g. AltGr, F1, ControlRight"),
            );
            if ui.button("Record").clicked() {
                self.recording_target = Some(ShortcutTarget::HoldKey);
                self.hotkey_runtime.start_capture_hold_key();
                self.status_line = "Press any key to set as hold-to-record key...".to_string();
            }
            if ui.button("Clear").clicked() {
                self.pending_hold_key = String::new();
                self.recording_target = None;
            }
        });
        if matches!(self.recording_target, Some(ShortcutTarget::HoldKey)) {
            ui.small("Listening for hold key...");
        }

        ui.separator();
        ui.add_space(8.0);

        // Collect update state data before drawing to avoid borrow conflicts.
        enum UpdateAction {
            None,
            CheckUpdates,
            PerformUpdate(String),
        }
        let busy = matches!(
            self.update_state,
            UpdateState::Checking | UpdateState::Updating
        );
        let update_info: Option<(String, String, String)> =
            if let UpdateState::UpdateAvailable {
                version,
                html_url,
                download_url,
            } = &self.update_state
            {
                Some((version.clone(), html_url.clone(), download_url.clone()))
            } else {
                None
            };
        let update_err: Option<String> = if let UpdateState::Failed(e) = &self.update_state {
            Some(e.clone())
        } else {
            None
        };

        let mut action = UpdateAction::None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Check for updates"))
                .clicked()
            {
                action = UpdateAction::CheckUpdates;
            }
            match &self.update_state {
                UpdateState::Idle => {}
                UpdateState::Checking => {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new("Checking...")
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                UpdateState::UpToDate => {
                    ui.label(
                        egui::RichText::new(format!(
                            "v{} is up to date",
                            env!("CARGO_PKG_VERSION")
                        ))
                        .color(ui.visuals().weak_text_color()),
                    );
                }
                UpdateState::UpdateAvailable { .. } => {
                    if let Some((version, html_url, download_url)) = &update_info {
                        ui.label(
                            egui::RichText::new(format!("v{version} available!"))
                                .color(egui::Color32::from_rgb(240, 180, 60)),
                        );
                        ui.hyperlink_to("Release notes", html_url);
                        if !download_url.is_empty()
                            && ui
                                .button(
                                    egui::RichText::new("Update now")
                                        .color(egui::Color32::from_rgb(120, 217, 120)),
                                )
                                .clicked()
                        {
                            action = UpdateAction::PerformUpdate(download_url.clone());
                        }
                    }
                }
                UpdateState::Updating => {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new("Installing update...")
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                UpdateState::UpdateDone => {
                    ui.label(
                        egui::RichText::new("Restart to apply update")
                            .color(egui::Color32::from_rgb(120, 217, 120)),
                    );
                }
                UpdateState::Failed(_) => {
                    if let Some(err) = &update_err {
                        ui.label(
                            egui::RichText::new(format!("Update failed: {err}"))
                                .color(ui.visuals().error_fg_color),
                        );
                    }
                }
            }
        });
        match action {
            UpdateAction::CheckUpdates => self.check_for_updates(),
            UpdateAction::PerformUpdate(url) => self.perform_self_update(url),
            UpdateAction::None => {}
        }
    }

    fn controls_ui(&mut self, ui: &mut egui::Ui, compact: bool) {
        let is_recording = self.is_recording();
        let is_paused = self.is_paused();
        let has_api_key = has_non_empty_api_key(&self.config);
        let can_start = !is_recording && has_api_key;

        ui.horizontal(|ui| {
            let mut start_response = ui.add_enabled(can_start, egui::Button::new("Start"));
            if !has_api_key {
                start_response = start_response
                    .on_disabled_hover_text("Set your Groq API key in Settings first.");
            }
            if start_response.clicked() {
                self.start_recording(ui.ctx());
            }

            let pause_label = if is_paused { "Resume" } else { "Pause" };
            if ui
                .add_enabled(is_recording, egui::Button::new(pause_label))
                .clicked()
            {
                self.pause_or_resume_recording();
            }

            if ui
                .add_enabled(is_recording, egui::Button::new("Stop"))
                .clicked()
            {
                self.stop_recording(ui.ctx());
            }

            if ui
                .add_enabled(is_recording, egui::Button::new("Cancel"))
                .clicked()
            {
                self.cancel_recording(ui.ctx());
            }
        });

        if !compact {
            let update_banner = if let UpdateState::UpdateAvailable {
                version,
                download_url,
                ..
            } = &self.update_state
            {
                Some((version.clone(), download_url.clone()))
            } else {
                None
            };
            if let Some((version, download_url)) = update_banner {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("v{version} available"))
                            .color(egui::Color32::from_rgb(240, 180, 60)),
                    );
                    if !download_url.is_empty() {
                        if ui
                            .button(
                                egui::RichText::new("Update now")
                                    .color(egui::Color32::from_rgb(120, 217, 120)),
                            )
                            .clicked()
                        {
                            self.perform_self_update(download_url);
                        }
                    } else if ui.small_button("See release").clicked() {
                        self.active_tab = AppTab::Settings;
                    }
                });
            }

            ui.add_space(8.0);
            ui.label(format!(
                "Global shortcuts: {} = start/stop, {} = show window",
                self.config.toggle_shortcut, self.config.show_window_shortcut
            ));

            if let Some(audio_path) = self.last_failed_audio_path.clone() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(format!("Last transcription failed: {audio_path}"));
                    if ui.button("Retry transcription").clicked() {
                        self.retry_last_transcription();
                    }
                });
            }

            let mic_test_active = self.is_monitoring();
            let mic_test_label = if mic_test_active {
                "Stop microphone test"
            } else {
                "Start microphone test"
            };
            if ui
                .add_enabled(!is_recording, egui::Button::new(mic_test_label))
                .clicked()
            {
                self.toggle_microphone_test();
            }
        }
    }

    fn latest_transcription_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Last transcription");
            if ui
                .add_enabled(
                    self.last_failed_audio_path.is_some(),
                    egui::Button::new("Retry failed"),
                )
                .clicked()
            {
                self.retry_last_transcription();
            }
            let can_copy = !self.last_transcription.is_empty();
            if ui
                .add_enabled(can_copy, egui::Button::new("Copy"))
                .clicked()
            {
                crate::utils::copy_to_clipboard(&self.last_transcription);
                self.status_line = "Copied latest transcription to clipboard".to_string();
            }
        });

        if self.last_transcription.is_empty() {
            ui.label("No transcription yet.");
        } else {
            ui.add(egui::Label::new(&self.last_transcription).wrap());
        }
    }

    fn tabs_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.active_tab, AppTab::Recorder, "Recorder");
            ui.selectable_value(&mut self.active_tab, AppTab::Settings, "Settings");

            let open_last = ui.add_enabled(
                self.history.has_history(),
                egui::Button::new("Open last transcription"),
            );
            if open_last.clicked() {
                if let Some(record) = self.history.latest() {
                    if let Err(err) = crate::utils::open_markdown_in_editor(&record.filename) {
                        self.status_line = format!("Failed to open transcription: {err}");
                    }
                }
            }
        });
        ui.separator();
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            let Some(path) = file.path else { continue };
            let path_string = path.to_string_lossy().to_string();

            if !crate::groq_request::is_supported_media_file(&path_string) {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
                    .unwrap_or_default();
                self.status_line = format!(
                    "Unsupported file type: .{ext}. Supported: mp3, wav, m4a, ogg, flac, webm, mp4, mov, m4v, mkv, avi, mpg"
                );
                continue;
            }

            if !has_non_empty_api_key(&self.config) {
                self.active_tab = AppTab::Settings;
                self.status_line =
                    "Cannot transcribe without a Groq API key. Add one in Settings.".to_string();
                return;
            }

            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string());
            self.status_line = if crate::groq_request::is_video_file(&path_string) {
                format!("Extracting audio from video: {name}...")
            } else {
                format!("Transcribing {name}...")
            };
            self.start_transcription_job(path_string);
        }
    }

    fn drop_overlay_ui(&self, ctx: &egui::Context) {
        let is_hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !is_hovering {
            return;
        }

        let screen = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop_overlay"),
        ));

        painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));

        let center = screen.center();
        let label = egui::RichText::new("Drop audio or video file to transcribe")
            .size(22.0)
            .color(egui::Color32::WHITE);
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            label.text(),
            egui::FontId::proportional(22.0),
            egui::Color32::WHITE,
        );
    }

    fn mic_graph_ui(&self, ui: &mut egui::Ui) {
        let desired_size = egui::vec2(ui.available_width(), 72.0);
        let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let is_recording = self.is_recording();

        let bg = ui.visuals().extreme_bg_color;
        painter.rect_filled(rect, 6.0, bg);
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.0, ui.visuals().window_stroke().color),
            egui::StrokeKind::Outside,
        );

        let wave_color = if is_recording {
            egui::Color32::from_rgb(240, 150, 60)
        } else {
            egui::Color32::from_rgb(80, 210, 140)
        };

        let center_y = rect.center().y;
        painter.line_segment(
            [
                egui::pos2(rect.left(), center_y),
                egui::pos2(rect.right(), center_y),
            ],
            egui::Stroke::new(1.0, wave_color.gamma_multiply(0.35)),
        );

        let len = self.mic_level_history.len().max(2);
        let step = if len > 1 {
            rect.width() / (len - 1) as f32
        } else {
            rect.width()
        };

        let mut points = Vec::with_capacity(len);
        for (idx, value) in self.mic_level_history.iter().enumerate() {
            let x = rect.left() + idx as f32 * step;
            let boosted = (value.clamp(0.0, 1.0) * 4.0).clamp(0.0, 1.0);
            let polarity = if idx % 2 == 0 { 1.0 } else { -1.0 };
            let amp = boosted * rect.height() * 0.42;
            let y = center_y - (polarity * amp);
            points.push(egui::pos2(x, y));
        }

        if points.len() >= 2 {
            let stroke_width = 1.2;
            let first = points.first().copied();
            let last = points.last().copied();
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(stroke_width, wave_color),
            ));

            // Rounded caps to keep the waveform visually soft.
            if let Some(p) = first {
                painter.circle_filled(p, stroke_width * 0.5, wave_color);
            }
            if let Some(p) = last {
                painter.circle_filled(p, stroke_width * 0.5, wave_color);
            }
        }
    }
}

impl eframe::App for WgoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(50));
        self.apply_hotkeys(ctx);
        self.apply_ui_events();
        self.apply_shortcut_recording(ctx);
        self.sample_mic_graph_if_due();
        self.handle_dropped_files(ctx);
        self.drop_overlay_ui(ctx);
        let is_recording = self.is_recording();

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            self.mic_graph_ui(ui);
        });

        if !is_recording {
            egui::TopBottomPanel::bottom("status_bar")
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(&self.status_line);
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if is_recording {
                self.controls_ui(ui, true);
            } else {
                self.tabs_ui(ui);

                match self.active_tab {
                    AppTab::Recorder => {
                        egui::ScrollArea::vertical()
                            .id_salt("recorder_tab_scroll")
                            .show(ui, |ui| {
                                self.controls_ui(ui, false);
                                ui.separator();
                                self.latest_transcription_ui(ui);
                            });
                    }
                    AppTab::Settings => {
                        egui::ScrollArea::vertical()
                            .id_salt("settings_tab_scroll")
                            .show(ui, |ui| {
                                self.settings_ui(ui);
                                ui.separator();
                                self.latest_transcription_ui(ui);
                            });
                    }
                }
            }
        });
    }
}

fn platform_asset_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "wgo-macos.app.tar.gz"
    }
    #[cfg(target_os = "linux")]
    {
        "wgo-linux"
    }
    #[cfg(target_os = "windows")]
    {
        "wgo-windows.exe"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        ""
    }
}

fn spawn_update_check(ui_tx: mpsc::Sender<UiEvent>) {
    std::thread::spawn(move || {
        let result = (|| -> Result<(String, String, String), String> {
            let client = reqwest::blocking::Client::builder()
                .user_agent("wgo-updater")
                .build()
                .map_err(|e| e.to_string())?;
            let resp: serde_json::Value = client
                .get("https://api.github.com/repos/salernoelia/wgo/releases/latest")
                .send()
                .map_err(|e| e.to_string())?
                .json()
                .map_err(|e| e.to_string())?;
            let tag = resp["tag_name"]
                .as_str()
                .ok_or("Missing tag_name")?
                .trim_start_matches('v')
                .to_string();
            let html_url = resp["html_url"]
                .as_str()
                .ok_or("Missing html_url")?
                .to_string();
            let asset_name = platform_asset_name();
            let download_url = resp["assets"]
                .as_array()
                .and_then(|assets| {
                    assets.iter().find(|a| {
                        a["name"].as_str().map(|n| n == asset_name).unwrap_or(false)
                    })
                })
                .and_then(|a| a["browser_download_url"].as_str())
                .unwrap_or("")
                .to_string();
            Ok((tag, html_url, download_url))
        })();

        match result {
            Ok((latest, html_url, download_url)) => {
                let current = env!("CARGO_PKG_VERSION");
                if latest != current {
                    let _ = ui_tx.send(UiEvent::UpdateAvailable {
                        version: latest,
                        html_url,
                        download_url,
                    });
                } else {
                    let _ = ui_tx.send(UiEvent::UpToDate);
                }
            }
            Err(e) => {
                let _ = ui_tx.send(UiEvent::UpdateCheckFailed(e));
            }
        }
    });
}

fn do_self_update(download_url: &str) -> Result<(), String> {
    let tmp_dir = tempfile::tempdir().map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    {
        let archive_path = tmp_dir.path().join("wgo-update.tar.gz");
        let mut archive_file =
            std::fs::File::create(&archive_path).map_err(|e| e.to_string())?;
        let client = reqwest::blocking::Client::builder()
            .user_agent("wgo-updater")
            .build()
            .map_err(|e| e.to_string())?;
        let bytes = client
            .get(download_url)
            .send()
            .and_then(|r| r.bytes())
            .map_err(|e| e.to_string())?;
        std::io::copy(&mut bytes.as_ref(), &mut archive_file).map_err(|e| e.to_string())?;

        let file = std::fs::File::open(&archive_path).map_err(|e| e.to_string())?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz);

        let extract_to = tmp_dir.path().join("wgo_bin");
        for entry in archive.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let entry_path = entry.path().map_err(|e| e.to_string())?;
            let file_name = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if file_name == "wgo" {
                entry
                    .unpack(&extract_to)
                    .map_err(|e| e.to_string())?;
                break;
            }
        }

        if !extract_to.exists() {
            return Err("Could not find wgo binary inside update archive".to_string());
        }

        self_replace::self_replace(&extract_to).map_err(|e| e.to_string())?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        let bin_path = tmp_dir.path().join("wgo_new");
        let client = reqwest::blocking::Client::builder()
            .user_agent("wgo-updater")
            .build()
            .map_err(|e| e.to_string())?;
        let bytes = client
            .get(download_url)
            .send()
            .and_then(|r| r.bytes())
            .map_err(|e| e.to_string())?;
        std::fs::write(&bin_path, &bytes).map_err(|e| e.to_string())?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&bin_path)
                .map_err(|e| e.to_string())?
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&bin_path, perms).map_err(|e| e.to_string())?;
        }

        self_replace::self_replace(&bin_path).map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn key_event_to_shortcut(key: egui::Key, modifiers: egui::Modifiers) -> Option<String> {
    let key_text = match key {
        egui::Key::A => "A",
        egui::Key::B => "B",
        egui::Key::C => "C",
        egui::Key::D => "D",
        egui::Key::E => "E",
        egui::Key::F => "F",
        egui::Key::G => "G",
        egui::Key::H => "H",
        egui::Key::I => "I",
        egui::Key::J => "J",
        egui::Key::K => "K",
        egui::Key::L => "L",
        egui::Key::M => "M",
        egui::Key::N => "N",
        egui::Key::O => "O",
        egui::Key::P => "P",
        egui::Key::Q => "Q",
        egui::Key::R => "R",
        egui::Key::S => "S",
        egui::Key::T => "T",
        egui::Key::U => "U",
        egui::Key::V => "V",
        egui::Key::W => "W",
        egui::Key::X => "X",
        egui::Key::Y => "Y",
        egui::Key::Z => "Z",
        egui::Key::Space => "Space",
        _ => return None,
    };

    let mut parts: Vec<&str> = Vec::new();
    if modifiers.ctrl {
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    if modifiers.mac_cmd || modifiers.command {
        parts.push("Meta");
    }

    if parts.is_empty() {
        return None;
    }

    parts.push(key_text);
    Some(parts.join("+"))
}

fn save_transcription_markdown(
    config: &AppConfig,
    audio_path: &str,
    transcription: &str,
) -> Result<PathBuf, String> {
    let output_dir = Path::new(config.markdown_dir.trim());
    if output_dir.as_os_str().is_empty() {
        return Err("Markdown output folder cannot be empty".to_string());
    }

    fs::create_dir_all(output_dir)
        .map_err(|e| format!("Failed to create markdown output directory: {e}"))?;

    let now = Local::now();
    let mut file_name = config.markdown_pattern.clone();
    if file_name.trim().is_empty() {
        file_name = "transcription_{date}_{time}.md".to_string();
    }

    file_name = file_name
        .replace("{timestamp}", &now.timestamp().to_string())
        .replace("{date}", &now.format("%Y-%m-%d").to_string())
        .replace("{time}", &now.format("%H-%M-%S").to_string());

    let mut sanitized = file_name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>();

    if !sanitized.ends_with(".md") {
        sanitized.push_str(".md");
    }

    let mut path = output_dir.join(&sanitized);
    let mut idx = 1usize;
    while path.exists() {
        let stem = Path::new(&sanitized)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("transcription");
        path = output_dir.join(format!("{stem}_{idx}.md"));
        idx += 1;
    }

    let body = format!(
        "---\nDate: {}\nAudio file: {}\nTags: \n- transcription \n---\n{}\n",
        now.to_rfc3339(),
        audio_path,
        transcription
    );

    fs::write(&path, body).map_err(|e| format!("Failed to write markdown file: {e}"))?;
    Ok(path)
}

fn has_non_empty_api_key(config: &AppConfig) -> bool {
    config.has_api_key()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn key_event_to_shortcut_requires_modifier() {
        let no_mods = egui::Modifiers::default();
        assert_eq!(key_event_to_shortcut(egui::Key::A, no_mods), None);
    }

    #[test]
    fn key_event_to_shortcut_formats_modifiers_and_key() {
        let mods = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            key_event_to_shortcut(egui::Key::Space, mods),
            Some("Ctrl+Shift+Space".to_string())
        );
    }

    #[test]
    fn has_non_empty_api_key_trims_whitespace() {
        let mut cfg = AppConfig::default();
        cfg.groq_api_key = "   ".to_string();
        assert!(!has_non_empty_api_key(&cfg));

        cfg.groq_api_key = " key_123 ".to_string();
        assert!(has_non_empty_api_key(&cfg));
    }

    #[test]
    fn save_transcription_markdown_sanitizes_and_adds_extension() {
        let tmp = tempdir().expect("tempdir");
        let mut cfg = AppConfig::default();
        cfg.markdown_dir = tmp.path().to_string_lossy().to_string();
        cfg.markdown_pattern = "bad:name*pattern".to_string();

        let path = save_transcription_markdown(&cfg, "audio.wav", "hello").expect("save");
        let file_name = path.file_name().and_then(|n| n.to_str()).expect("filename");

        assert!(file_name.ends_with(".md"));
        assert!(!file_name.contains(':'));
        assert!(!file_name.contains('*'));
    }

    #[test]
    fn save_transcription_markdown_avoids_overwrite() {
        let tmp = tempdir().expect("tempdir");
        let mut cfg = AppConfig::default();
        cfg.markdown_dir = tmp.path().to_string_lossy().to_string();
        cfg.markdown_pattern = "fixed_name.md".to_string();

        let first = save_transcription_markdown(&cfg, "a.wav", "one").expect("first");
        let second = save_transcription_markdown(&cfg, "b.wav", "two").expect("second");

        assert_ne!(first, second);
        assert!(first.exists());
        assert!(second.exists());
    }
}
