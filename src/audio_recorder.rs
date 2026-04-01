use crate::transcription_history::{TranscriptionHistory, TranscriptionRecord};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{SampleFormat, WavSpec, WavWriter};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

fn normalize_input_error(context: &str, err: impl std::fmt::Display) -> String {
    let details = err.to_string();
    let lower = details.to_ascii_lowercase();

    let permission_related = [
        "permission",
        "not authorized",
        "not permitted",
        "access denied",
        "operation not permitted",
        "unauthorized",
        "forbidden",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    if permission_related {
        return format!(
            "Microphone access is blocked. Enable microphone access for this app in system privacy settings, then retry. {context}. Details: {details}"
        );
    }

    format!("{context}. Details: {details}")
}

pub struct AudioRecorder {
    stream: Option<cpal::Stream>,
    is_recording: Arc<AtomicBool>,
    is_monitoring: Arc<AtomicBool>,
    is_paused: Arc<AtomicBool>,
    level_milli: Arc<AtomicU32>,
    writer: Option<Arc<Mutex<WavWriter<std::io::BufWriter<std::fs::File>>>>>,
    current_filename: Option<String>,
    device_name: Option<String>,
    recordings_dir: Option<PathBuf>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            stream: None,
            is_recording: Arc::new(AtomicBool::new(false)),
            is_monitoring: Arc::new(AtomicBool::new(false)),
            is_paused: Arc::new(AtomicBool::new(false)),
            level_milli: Arc::new(AtomicU32::new(0)),
            writer: None,
            current_filename: None,
            device_name: None,
            recordings_dir: None,
        }
    }

    pub fn set_recordings_dir(&mut self, dir: Option<PathBuf>) {
        self.recordings_dir = dir;
    }

    pub fn list_input_devices() -> Result<Vec<String>, String> {
        let host = cpal::default_host();
        let devices: Vec<_> = host
            .input_devices()
            .map_err(|e| normalize_input_error("Failed to enumerate input devices", e))?
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

    fn get_recordings_dir(&self) -> PathBuf {
        let recordings_dir = self
            .recordings_dir
            .clone()
            .unwrap_or_else(|| crate::config::AppConfig::app_data_dir().join("recordings"));

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

    pub fn is_monitoring(&self) -> bool {
        self.is_monitoring.load(Ordering::SeqCst)
    }

    pub fn input_level(&self) -> f32 {
        self.level_milli.load(Ordering::SeqCst) as f32 / 1000.0
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

    fn get_input_device(&self) -> Result<cpal::Device, String> {
        let host = cpal::default_host();

        if let Some(ref device_name) = self.device_name {
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
                    Ok(device)
                }
                None => {
                    eprintln!("Selected device '{}' not found, using default", device_name);
                    host.default_input_device()
                        .ok_or_else(|| "No input device available".to_string())
                }
            }
        } else {
            match host.default_input_device() {
                Some(device) => {
                    if let Ok(name) = device.name() {
                        println!("Using default input device: {}", name);
                    }
                    Ok(device)
                }
                None => Err("No input device available".to_string()),
            }
        }
    }

    pub fn start_recording(&mut self) -> Result<String, String> {
        if self.is_recording.load(Ordering::SeqCst) {
            return Err("Recording is already running".to_string());
        }

        if self.is_monitoring() {
            self.stop_monitoring()?;
        }

        let device = self.get_input_device()?;

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
                return Err(normalize_input_error(
                    "Failed to get default microphone configuration",
                    err,
                ));
            }
        };

        // Try to get a 16kHz config — Whisper works best at 16kHz and it keeps files small.
        // If the device doesn't support it, fall back to the default rate.
        const TARGET_RATE: cpal::SampleRate = cpal::SampleRate(8000);
        let stream_config = {
            let found_16k = device.supported_input_configs().ok().and_then(|mut cfgs| {
                cfgs.find(|c| {
                    c.min_sample_rate() <= TARGET_RATE && c.max_sample_rate() >= TARGET_RATE
                })
                .map(|c| c.with_sample_rate(TARGET_RATE).config())
            });
            found_16k.unwrap_or_else(|| supported.config())
        };
        let sample_format = supported.sample_format();
        let num_input_channels = stream_config.channels as usize;

        // Always write mono — halves (or more) the file size vs stereo.
        let spec = WavSpec {
            channels: 1,
            sample_rate: stream_config.sample_rate.0,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };

        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let filename = format!("recording_{}.wav", timestamp);
        let file_path = self.get_recordings_dir().join(&filename);
        self.current_filename = Some(file_path.to_string_lossy().to_string());

        let writer = match WavWriter::create(&file_path, spec) {
            Ok(writer) => Arc::new(Mutex::new(writer)),
            Err(err) => {
                return Err(format!("Error creating wav file: {err}"));
            }
        };

        self.writer = Some(writer.clone());
        let is_recording = self.is_recording.clone();
        let is_monitoring = self.is_monitoring.clone();
        let is_paused = self.is_paused.clone();
        let level_milli = self.level_milli.clone();

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let writer_clone = writer.clone();
                let is_recording_clone = is_recording.clone();
                let is_paused_clone = is_paused.clone();
                let level_milli_clone = level_milli.clone();
                device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[f32], _| {
                            let recording = is_recording_clone.load(Ordering::SeqCst);
                            let monitoring = is_monitoring.load(Ordering::SeqCst);
                            if (!recording && !monitoring)
                                || (recording && is_paused_clone.load(Ordering::SeqCst))
                            {
                                return;
                            }
                            if let Ok(mut writer) = writer_clone.lock() {
                                let mut peak = 0.0f32;
                                for frame in data.chunks(num_input_channels) {
                                    let mono = frame.iter().sum::<f32>() / frame.len() as f32;
                                    peak = peak.max(mono.abs());
                                    let _ = writer.write_sample(Self::i16_from_f32(mono));
                                }
                                let scaled = (peak.clamp(0.0, 1.0) * 1000.0).round() as u32;
                                level_milli_clone.store(scaled, Ordering::SeqCst);
                            }
                        },
                        move |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                    .map_err(|e| {
                        normalize_input_error("Failed to open microphone input stream", e)
                    })?
            }
            cpal::SampleFormat::I16 => {
                let writer_clone = writer.clone();
                let is_recording_clone = is_recording.clone();
                let is_paused_clone = is_paused.clone();
                let level_milli_clone = level_milli.clone();
                device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[i16], _| {
                            let recording = is_recording_clone.load(Ordering::SeqCst);
                            let monitoring = is_monitoring.load(Ordering::SeqCst);
                            if (!recording && !monitoring)
                                || (recording && is_paused_clone.load(Ordering::SeqCst))
                            {
                                return;
                            }
                            if let Ok(mut writer) = writer_clone.lock() {
                                let mut peak = 0.0f32;
                                for frame in data.chunks(num_input_channels) {
                                    let mono = frame.iter().map(|&s| s as f32).sum::<f32>()
                                        / frame.len() as f32;
                                    peak = peak.max((mono / i16::MAX as f32).abs());
                                    let _ = writer.write_sample(mono as i16);
                                }
                                let scaled = (peak.clamp(0.0, 1.0) * 1000.0).round() as u32;
                                level_milli_clone.store(scaled, Ordering::SeqCst);
                            }
                        },
                        move |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                    .map_err(|e| {
                        normalize_input_error("Failed to open microphone input stream", e)
                    })?
            }
            cpal::SampleFormat::U16 => {
                let writer_clone = writer.clone();
                let is_recording_clone = is_recording.clone();
                let is_paused_clone = is_paused.clone();
                let level_milli_clone = level_milli.clone();
                device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[u16], _| {
                            let recording = is_recording_clone.load(Ordering::SeqCst);
                            let monitoring = is_monitoring.load(Ordering::SeqCst);
                            if (!recording && !monitoring)
                                || (recording && is_paused_clone.load(Ordering::SeqCst))
                            {
                                return;
                            }
                            if let Ok(mut writer) = writer_clone.lock() {
                                let mut peak = 0.0f32;
                                for frame in data.chunks(num_input_channels) {
                                    let mono = frame
                                        .iter()
                                        .map(|&s| Self::i16_from_u16(s) as f32)
                                        .sum::<f32>()
                                        / frame.len() as f32;
                                    peak = peak.max((mono / i16::MAX as f32).abs());
                                    let _ = writer.write_sample(mono as i16);
                                }
                                let scaled = (peak.clamp(0.0, 1.0) * 1000.0).round() as u32;
                                level_milli_clone.store(scaled, Ordering::SeqCst);
                            }
                        },
                        move |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                    .map_err(|e| {
                        normalize_input_error("Failed to open microphone input stream", e)
                    })?
            }
            other => return Err(format!("Unsupported sample format: {other:?}")),
        };

        if let Err(err) = stream.play() {
            return Err(normalize_input_error(
                "Failed to start microphone stream",
                err,
            ));
        }

        self.is_recording.store(true, Ordering::SeqCst);
        self.is_monitoring.store(false, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        self.stream = Some(stream);

        println!("Recording started: {}", filename);
        println!("Speak now...");
        Ok(file_path.to_string_lossy().to_string())
    }

    pub fn start_monitoring(&mut self) -> Result<(), String> {
        if self.is_recording() {
            return Err("Cannot start microphone test while recording".to_string());
        }

        if self.is_monitoring() {
            return Ok(());
        }

        let device = self.get_input_device()?;

        let supported = device.default_input_config().map_err(|err| {
            normalize_input_error("Failed to get default microphone configuration", err)
        })?;
        let stream_config = supported.config();
        let sample_format = supported.sample_format();

        let is_recording = self.is_recording.clone();
        let is_monitoring = self.is_monitoring.clone();
        let is_paused = self.is_paused.clone();
        let level_milli = self.level_milli.clone();

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let is_recording_clone = is_recording.clone();
                let is_monitoring_clone = is_monitoring.clone();
                let is_paused_clone = is_paused.clone();
                let level_milli_clone = level_milli.clone();
                device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[f32], _| {
                            let recording = is_recording_clone.load(Ordering::SeqCst);
                            let monitoring = is_monitoring_clone.load(Ordering::SeqCst);
                            if (!recording && !monitoring)
                                || (recording && is_paused_clone.load(Ordering::SeqCst))
                            {
                                return;
                            }

                            let mut peak = 0.0f32;
                            for &sample in data {
                                peak = peak.max(sample.abs());
                            }
                            let scaled = (peak.clamp(0.0, 1.0) * 1000.0).round() as u32;
                            level_milli_clone.store(scaled, Ordering::SeqCst);
                        },
                        move |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                    .map_err(|e| {
                        normalize_input_error("Failed to open microphone input stream", e)
                    })?
            }
            cpal::SampleFormat::I16 => {
                let is_recording_clone = is_recording.clone();
                let is_monitoring_clone = is_monitoring.clone();
                let is_paused_clone = is_paused.clone();
                let level_milli_clone = level_milli.clone();
                device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[i16], _| {
                            let recording = is_recording_clone.load(Ordering::SeqCst);
                            let monitoring = is_monitoring_clone.load(Ordering::SeqCst);
                            if (!recording && !monitoring)
                                || (recording && is_paused_clone.load(Ordering::SeqCst))
                            {
                                return;
                            }

                            let mut peak = 0.0f32;
                            for &sample in data {
                                peak = peak.max((sample as f32 / i16::MAX as f32).abs());
                            }
                            let scaled = (peak.clamp(0.0, 1.0) * 1000.0).round() as u32;
                            level_milli_clone.store(scaled, Ordering::SeqCst);
                        },
                        move |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                    .map_err(|e| {
                        normalize_input_error("Failed to open microphone input stream", e)
                    })?
            }
            cpal::SampleFormat::U16 => {
                let is_recording_clone = is_recording.clone();
                let is_monitoring_clone = is_monitoring.clone();
                let is_paused_clone = is_paused.clone();
                let level_milli_clone = level_milli.clone();
                device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[u16], _| {
                            let recording = is_recording_clone.load(Ordering::SeqCst);
                            let monitoring = is_monitoring_clone.load(Ordering::SeqCst);
                            if (!recording && !monitoring)
                                || (recording && is_paused_clone.load(Ordering::SeqCst))
                            {
                                return;
                            }

                            let mut peak = 0.0f32;
                            for &sample in data {
                                let i16_sample = Self::i16_from_u16(sample);
                                peak = peak.max((i16_sample as f32 / i16::MAX as f32).abs());
                            }
                            let scaled = (peak.clamp(0.0, 1.0) * 1000.0).round() as u32;
                            level_milli_clone.store(scaled, Ordering::SeqCst);
                        },
                        move |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                    .map_err(|e| {
                        normalize_input_error("Failed to open microphone input stream", e)
                    })?
            }
            other => return Err(format!("Unsupported sample format: {other:?}")),
        };

        stream
            .play()
            .map_err(|err| normalize_input_error("Failed to start microphone stream", err))?;

        self.stream = Some(stream);
        self.writer = None;
        self.current_filename = None;
        self.is_paused.store(false, Ordering::SeqCst);
        self.is_recording.store(false, Ordering::SeqCst);
        self.is_monitoring.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn stop_monitoring(&mut self) -> Result<(), String> {
        if !self.is_monitoring() {
            return Ok(());
        }

        self.is_monitoring.store(false, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        self.level_milli.store(0, Ordering::SeqCst);

        std::thread::sleep(std::time::Duration::from_millis(50));

        self.stream = None;
        self.writer = None;
        self.current_filename = None;
        Ok(())
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
        self.is_monitoring.store(false, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        self.level_milli.store(0, Ordering::SeqCst);

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

        // Convert WAV to M4A (AAC) to drastically reduce file size.
        // afconvert is built into macOS; Groq accepts m4a.
        if let Some(ref wav_path) = completed_filename {
            let m4a_path = wav_path.replace(".wav", ".m4a");
            let status = std::process::Command::new("afconvert")
                .args([
                    "-f", "m4af",
                    "-d", "aac",
                    "-b", "32000",
                    "-c", "1",
                    wav_path,
                    &m4a_path,
                ])
                .status();
            match status {
                Ok(s) if s.success() => {
                    let _ = std::fs::remove_file(wav_path);
                    println!("Converted to M4A: {}", m4a_path);
                    return Ok(Some(m4a_path));
                }
                Ok(s) => eprintln!("afconvert failed with status: {}", s),
                Err(e) => eprintln!("afconvert not available: {}", e),
            }
        }

        Ok(completed_filename)
    }
}
