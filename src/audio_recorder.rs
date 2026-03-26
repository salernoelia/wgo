use crate::transcription_history::{TranscriptionHistory, TranscriptionRecord};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{SampleFormat, WavSpec, WavWriter};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

pub struct AudioRecorder {
    stream: Option<cpal::Stream>,
    is_recording: Arc<AtomicBool>,
    is_paused: Arc<AtomicBool>,
    writer: Option<Arc<Mutex<WavWriter<std::io::BufWriter<std::fs::File>>>>>,
    current_filename: Option<String>,
    device_name: Option<String>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            stream: None,
            is_recording: Arc::new(AtomicBool::new(false)),
            is_paused: Arc::new(AtomicBool::new(false)),
            writer: None,
            current_filename: None,
            device_name: None,
        }
    }

    pub fn list_input_devices() -> Result<Vec<String>, String> {
        let host = cpal::default_host();
        let devices: Vec<_> = host
            .input_devices()
            .map_err(|e| format!("Failed to get input devices: {}", e))?
            .collect();

        if devices.is_empty() {
            return Err("No input devices available".to_string());
        }

        let names = devices.into_iter().filter_map(|d| d.name().ok()).collect();
        Ok(names)
    }

    pub fn set_device_name(&mut self, device_name: Option<String>) {
        self.device_name = device_name;
    }

    fn get_recordings_dir() -> PathBuf {
        let recordings_dir = crate::config::AppConfig::app_data_dir().join("recordings");

        if let Err(e) = std::fs::create_dir_all(&recordings_dir) {
            eprintln!("Warning: Failed to create recordings directory: {}", e);
            return std::env::current_dir().unwrap_or_default();
        }

        recordings_dir
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    pub fn is_paused(&self) -> bool {
        self.is_paused.load(Ordering::SeqCst)
    }

    pub fn save_transcription(filename: &str, transcription: &str) {
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

    fn i16_from_f32(sample: f32) -> i16 {
        let clamped = sample.clamp(-1.0, 1.0);
        (clamped * i16::MAX as f32) as i16
    }

    fn i16_from_u16(sample: u16) -> i16 {
        (sample as i32 - i16::MAX as i32 - 1) as i16
    }

    pub fn start_recording(&mut self) -> Result<String, String> {
        if self.is_recording.load(Ordering::SeqCst) {
            return Err("Recording is already running".to_string());
        }

        let host = cpal::default_host();

        let device = if let Some(ref device_name) = self.device_name {
            let found_device =
                host.input_devices().ok().and_then(|devices| {
                    devices
                        .filter_map(|d| {
                            d.name().ok().and_then(|name| {
                                if name == *device_name {
                                    Some(d)
                                } else {
                                    None
                                }
                            })
                        })
                        .next()
                });

            match found_device {
                Some(device) => {
                    println!("Using selected device: {}", device_name);
                    device
                }
                None => {
                    eprintln!("Selected device '{}' not found, using default", device_name);
                    match host.default_input_device() {
                        Some(d) => d,
                        None => {
                            return Err("No input device available".to_string());
                        }
                    }
                }
            }
        } else {
            match host.default_input_device() {
                Some(device) => {
                    if let Ok(name) = device.name() {
                        println!("Using default input device: {}", name);
                    }
                    device
                }
                None => {
                    return Err("No input device available".to_string());
                }
            }
        };

        let supported = match device.default_input_config() {
            Ok(config) => {
                println!(
                    "Audio config: {} channels, {} Hz, {:?}",
                    config.channels(),
                    config.sample_rate().0,
                    config.sample_format()
                );
                config
            }
            Err(err) => {
                return Err(format!("Error getting default input config: {err}"));
            }
        };

        let stream_config = supported.config();
        let sample_format = supported.sample_format();

        let spec = WavSpec {
            channels: stream_config.channels,
            sample_rate: stream_config.sample_rate.0,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };

        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let filename = format!("recording_{}.wav", timestamp);
        let file_path = Self::get_recordings_dir().join(&filename);
        self.current_filename = Some(file_path.to_string_lossy().to_string());

        let writer = match WavWriter::create(&file_path, spec) {
            Ok(writer) => Arc::new(Mutex::new(writer)),
            Err(err) => {
                return Err(format!("Error creating wav file: {err}"));
            }
        };

        self.writer = Some(writer.clone());
        let is_recording = self.is_recording.clone();
        let is_paused = self.is_paused.clone();

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let writer_clone = writer.clone();
                let is_recording_clone = is_recording.clone();
                let is_paused_clone = is_paused.clone();
                device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[f32], _| {
                            if !is_recording_clone.load(Ordering::SeqCst)
                                || is_paused_clone.load(Ordering::SeqCst)
                            {
                                return;
                            }
                            if let Ok(mut writer) = writer_clone.lock() {
                                for &sample in data {
                                    let _ = writer.write_sample(Self::i16_from_f32(sample));
                                }
                            }
                        },
                        move |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                    .map_err(|e| e.to_string())?
            }
            cpal::SampleFormat::I16 => {
                let writer_clone = writer.clone();
                let is_recording_clone = is_recording.clone();
                let is_paused_clone = is_paused.clone();
                device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[i16], _| {
                            if !is_recording_clone.load(Ordering::SeqCst)
                                || is_paused_clone.load(Ordering::SeqCst)
                            {
                                return;
                            }
                            if let Ok(mut writer) = writer_clone.lock() {
                                for &sample in data {
                                    let _ = writer.write_sample(sample);
                                }
                            }
                        },
                        move |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                    .map_err(|e| e.to_string())?
            }
            cpal::SampleFormat::U16 => {
                let writer_clone = writer.clone();
                let is_recording_clone = is_recording.clone();
                let is_paused_clone = is_paused.clone();
                device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[u16], _| {
                            if !is_recording_clone.load(Ordering::SeqCst)
                                || is_paused_clone.load(Ordering::SeqCst)
                            {
                                return;
                            }
                            if let Ok(mut writer) = writer_clone.lock() {
                                for &sample in data {
                                    let _ = writer.write_sample(Self::i16_from_u16(sample));
                                }
                            }
                        },
                        move |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                    .map_err(|e| e.to_string())?
            }
            other => return Err(format!("Unsupported sample format: {other:?}")),
        };

        if let Err(err) = stream.play() {
            return Err(format!("Error starting stream: {err}"));
        }

        self.is_recording.store(true, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        self.stream = Some(stream);

        println!("Recording started: {}", filename);
        println!("Speak now...");
        Ok(file_path.to_string_lossy().to_string())
    }

    pub fn pause_recording(&mut self) -> Result<(), String> {
        if !self.is_recording() {
            return Err("Cannot pause because recording is not running".to_string());
        }
        self.is_paused.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn resume_recording(&mut self) -> Result<(), String> {
        if !self.is_recording() {
            return Err("Cannot resume because recording is not running".to_string());
        }
        self.is_paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub fn stop_recording(&mut self) -> Result<Option<String>, String> {
        if !self.is_recording.load(Ordering::SeqCst) {
            return Ok(None);
        }

        self.is_recording.store(false, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);

        std::thread::sleep(std::time::Duration::from_millis(100));

        self.stream = None;

        let completed_filename = self.current_filename.take();

        if let Some(writer_arc) = self.writer.take() {
            match Arc::try_unwrap(writer_arc) {
                Ok(writer_mutex) => match writer_mutex.into_inner() {
                    Ok(writer) => {
                        if let Err(err) = writer.finalize() {
                            return Err(format!("Error finalizing WAV file: {err}"));
                        } else {
                            println!("Recording saved successfully");
                        }
                    }
                    Err(err) => {
                        return Err(format!("Error getting writer mutex: {err}"));
                    }
                },
                Err(_) => {
                    return Err(
                        "Error: Multiple references to writer exist, cannot finalize WAV file."
                            .to_string(),
                    );
                }
            }
        }

        Ok(completed_filename)
    }
}
