use crate::audio_recorder::AudioRecorder;
use crate::config::AppConfig;
use crate::shortcut_detector::HotkeyCommand;
use chrono::Local;
use eframe::egui;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

pub struct WgoApp {
    recorder: Arc<Mutex<AudioRecorder>>,
    config: AppConfig,
    microphones: Vec<String>,
    selected_microphone: Option<String>,
    hotkey_rx: Receiver<HotkeyCommand>,
    ui_event_rx: Receiver<UiEvent>,
    ui_event_tx: mpsc::Sender<UiEvent>,
    status_line: String,
    last_transcription: String,
}

enum UiEvent {
    TranscriptionReady {
        audio_path: String,
        text: String,
        markdown_path: Option<PathBuf>,
    },
    Error(String),
}

impl WgoApp {
    pub fn new(hotkey_rx: Receiver<HotkeyCommand>) -> Self {
        let config = AppConfig::load();
        let recorder = Arc::new(Mutex::new(AudioRecorder::new()));

        if let Ok(mut rec) = recorder.lock() {
            rec.set_device_name(config.microphone_name.clone());
        }

        let microphones = AudioRecorder::list_input_devices().unwrap_or_default();

        let (ui_event_tx, ui_event_rx) = mpsc::channel();

        Self {
            recorder,
            selected_microphone: config.microphone_name.clone(),
            config,
            microphones,
            hotkey_rx,
            ui_event_rx,
            ui_event_tx,
            status_line: "Ready".to_string(),
            last_transcription: String::new(),
        }
    }

    fn apply_hotkeys(&mut self, ctx: &egui::Context) {
        while let Ok(cmd) = self.hotkey_rx.try_recv() {
            match cmd {
                HotkeyCommand::ToggleRecording => {
                    self.bring_to_front(ctx);
                    if self.is_recording() {
                        self.stop_recording();
                    } else {
                        self.start_recording();
                    }
                }
                HotkeyCommand::ShowWindow => self.bring_to_front(ctx),
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
                    self.status_line = match markdown_path {
                        Some(path) => format!(
                            "Transcribed {} and saved markdown to {}",
                            audio_path,
                            path.to_string_lossy()
                        ),
                        None => format!("Transcribed {}", audio_path),
                    };
                }
                UiEvent::Error(err) => self.status_line = err,
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

    fn is_recording(&self) -> bool {
        self.recorder
            .lock()
            .map(|r| r.is_recording())
            .unwrap_or(false)
    }

    fn is_paused(&self) -> bool {
        self.recorder.lock().map(|r| r.is_paused()).unwrap_or(false)
    }

    fn refresh_microphones(&mut self) {
        self.microphones = AudioRecorder::list_input_devices().unwrap_or_default();
    }

    fn save_settings(&mut self) {
        self.config.microphone_name = self.selected_microphone.clone();
        if let Ok(mut rec) = self.recorder.lock() {
            rec.set_device_name(self.config.microphone_name.clone());
        }

        match self.config.save() {
            Ok(()) => self.status_line = "Settings saved".to_string(),
            Err(err) => self.status_line = err,
        }
    }

    fn start_recording(&mut self) {
        match self.recorder.lock() {
            Ok(mut recorder) => match recorder.start_recording() {
                Ok(path) => {
                    self.status_line = format!("Recording started: {path}");
                }
                Err(err) => {
                    self.status_line = err;
                }
            },
            Err(_) => self.status_line = "Failed to lock recorder".to_string(),
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

    fn stop_recording(&mut self) {
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

        self.status_line = format!("Recording stopped. Transcribing {}...", filename);
        let cfg = self.config.clone();
        let ui_tx = self.ui_event_tx.clone();

        std::thread::spawn(move || {
            match crate::groq_request::transcribe_audio(&filename) {
                Ok(text) => {
                    crate::utils::copy_to_clipboard(&text);
                    AudioRecorder::save_transcription(&filename, &text);

                    let md_path = match save_transcription_markdown(&cfg, &filename, &text) {
                        Ok(path) => Some(path),
                        Err(err) => {
                            let _ = ui_tx.send(UiEvent::Error(format!(
                                "Transcription succeeded but markdown save failed: {err}"
                            )));
                            None
                        }
                    };

                    let _ = ui_tx.send(UiEvent::TranscriptionReady {
                        audio_path: filename,
                        text,
                        markdown_path: md_path,
                    });
                }
                Err(err) => {
                    let _ = ui_tx.send(UiEvent::Error(format!("Transcription error: {err}")));
                }
            }
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.separator();

        ui.label("Groq API key");
        ui.add(
            egui::TextEdit::singleline(&mut self.config.groq_api_key)
                .password(true)
                .hint_text("Enter your Groq API key"),
        );

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

        ui.add_space(12.0);
        if ui.button("Save settings").clicked() {
            self.save_settings();
        }
    }

    fn controls_ui(&mut self, ui: &mut egui::Ui) {
        let is_recording = self.is_recording();
        let is_paused = self.is_paused();

        ui.heading("Recorder");
        ui.separator();

        ui.horizontal(|ui| {
            if ui
                .add_enabled(!is_recording, egui::Button::new("Start"))
                .clicked()
            {
                self.start_recording();
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
                self.stop_recording();
            }
        });

        ui.add_space(8.0);
        ui.label(format!(
            "Global shortcuts: Alt/Meta+Space = start/stop, Alt/Meta+H = show window"
        ));
    }
}

impl eframe::App for WgoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_hotkeys(ctx);
        self.apply_ui_events();

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.heading("wgo");
            ui.label(&self.status_line);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |columns| {
                self.controls_ui(&mut columns[0]);
                self.settings_ui(&mut columns[1]);
            });

            ui.separator();
            ui.heading("Last transcription");
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.last_transcription.is_empty() {
                    ui.label("No transcription yet.");
                } else {
                    ui.label(&self.last_transcription);
                }
            });
        });
    }
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
        "# Transcription\n\n- Date: {}\n- Audio file: {}\n\n## Text\n\n{}\n",
        now.to_rfc3339(),
        audio_path,
        transcription
    );

    fs::write(&path, body).map_err(|e| format!("Failed to write markdown file: {e}"))?;
    Ok(path)
}
