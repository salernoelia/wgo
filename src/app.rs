use crate::audio_recorder::AudioRecorder;
use crate::config::AppConfig;
use crate::shortcut_detector::{HotkeyBindings, HotkeyCommand, HotkeyRuntime};
use chrono::Local;
use eframe::egui;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    recording_target: Option<ShortcutTarget>,
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
    microphone_access_checked: bool,
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

        Self {
            recorder,
            hotkey_runtime,
            selected_microphone: config.microphone_name.clone(),
            pending_toggle_shortcut: config.toggle_shortcut.clone(),
            pending_show_shortcut: config.show_window_shortcut.clone(),
            recording_target: None,
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
            microphone_access_checked: false,
        }
    }

    fn ensure_microphone_access(&mut self) -> Result<(), String> {
        if self.microphone_access_checked {
            return Ok(());
        }

        let preferred = self.selected_microphone.as_deref();
        crate::microphone_permission::request_microphone_access(preferred)?;
        self.microphone_access_checked = true;
        Ok(())
    }

    fn request_microphone_access_again(&mut self) {
        self.microphone_access_checked = false;
        match self.ensure_microphone_access() {
            Ok(()) => {
                self.status_line = "Microphone permission check completed".to_string();
            }
            Err(err) => {
                self.status_line = err;
            }
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
            }
            self.recording_target = None;
            self.status_line = "Shortcut captured. Save settings to apply globally.".to_string();
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
            }
        }
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
        let previous_microphone = self.config.microphone_name.clone();
        self.config.microphone_name = self.selected_microphone.clone();
        self.config.toggle_shortcut = self.pending_toggle_shortcut.clone();
        self.config.show_window_shortcut = self.pending_show_shortcut.clone();
        if let Ok(mut rec) = self.recorder.lock() {
            rec.set_device_name(self.config.microphone_name.clone());
        }

        if previous_microphone != self.config.microphone_name {
            self.microphone_access_checked = false;
        }

        match self.config.save() {
            Ok(()) => {
                self.hotkey_runtime.update_bindings(HotkeyBindings::new(
                    self.config.toggle_shortcut.clone(),
                    self.config.show_window_shortcut.clone(),
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

        if let Err(err) = self.ensure_microphone_access() {
            self.status_line = err;
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
        if !self.is_monitoring() {
            if let Err(err) = self.ensure_microphone_access() {
                self.status_line = err;
                return;
            }
        }

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
        if ui.button("Request microphone permission again").clicked() {
            self.request_microphone_access_again();
        }

        ui.add_space(8.0);
        ui.label("Markdown output folder");
        ui.add(
            egui::TextEdit::singleline(&mut self.config.markdown_dir)
                .hint_text("/path/to/transcriptions"),
        );

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
            };
            ui.small(label);
        }

        ui.add_space(12.0);
        if ui.button("Save settings").clicked() {
            self.save_settings();
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
        });

        if !compact {
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
        });
        ui.separator();
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
