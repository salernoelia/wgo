use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{SampleFormat, WavSpec, WavWriter};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

pub struct AudioRecorder {
    stream: Option<cpal::Stream>,
    is_recording: Arc<AtomicBool>,
    writer: Option<Arc<Mutex<WavWriter<std::io::BufWriter<std::fs::File>>>>>,
    file_name_history: Vec<String>,
    current_filename: Option<String>,
    device_name: Option<String>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            stream: None,
            is_recording: Arc::new(AtomicBool::new(false)),
            writer: None,
            file_name_history: vec![],
            current_filename: None,
            device_name: None,
        }
    }

    pub fn select_device(&mut self) -> Result<(), String> {
        let host = cpal::default_host();

        let devices: Vec<_> = host
            .input_devices()
            .map_err(|e| format!("Failed to get input devices: {}", e))?
            .collect();

        if devices.is_empty() {
            return Err("No input devices available".to_string());
        }

        println!("\nAvailable audio input devices:");
        for (idx, device) in devices.iter().enumerate() {
            if let Ok(name) = device.name() {
                println!("  {}. {}", idx + 1, name);
            }
        }

        print!("\nSelect device number (or press Enter for default): ");
        std::io::Write::flush(&mut std::io::stdout()).ok();

        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return Err("Failed to read input".to_string());
        }

        let input = input.trim();
        if input.is_empty() {
            self.device_name = None;
            println!("Using default device");
            return Ok(());
        }

        match input.parse::<usize>() {
            Ok(num) if num > 0 && num <= devices.len() => {
                if let Ok(name) = devices[num - 1].name() {
                    self.device_name = Some(name.clone());
                    println!("Selected: {}", name);
                    Ok(())
                } else {
                    Err("Failed to get device name".to_string())
                }
            }
            _ => Err("Invalid selection".to_string()),
        }
    }

    fn get_recordings_dir() -> PathBuf {
        let data_dir = if cfg!(target_os = "windows") {
            std::env::var("APPDATA")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
                .join("wgo")
        } else {
            std::env::var("XDG_DATA_HOME")
                .ok()
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var("HOME")
                        .ok()
                        .map(|h| PathBuf::from(h).join(".local").join("share"))
                })
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
                .join("wgo")
        };

        let recordings_dir = data_dir.join("recordings");

        if let Err(e) = std::fs::create_dir_all(&recordings_dir) {
            eprintln!("Warning: Failed to create recordings directory: {}", e);
            return std::env::current_dir().unwrap_or_default();
        }

        recordings_dir
    }

    pub fn print_history(&self) {
        println!("Recording history:");
        for (i, filename) in self.file_name_history.iter().enumerate() {
            println!("  {}. {}", i + 1, filename);
        }
    }

    pub fn toggle_recording(&mut self) -> Option<String> {
        if self.is_recording.load(Ordering::SeqCst) {
            let filename = self.stop_recording();
            self.print_history();
            filename
        } else {
            self.start_recording();
            None
        }
    }

    fn start_recording(&mut self) {
        if self.is_recording.load(Ordering::SeqCst) {
            return;
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
                            eprintln!("No input device available");
                            return;
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
                    eprintln!("No input device available");
                    return;
                }
            }
        };

        let config = match device.default_input_config() {
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
                eprintln!("Error getting default input config: {}", err);
                return;
            }
        };

        let spec = WavSpec {
            channels: config.channels(),
            sample_rate: config.sample_rate().0,
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
                eprintln!("Error creating wav file: {}", err);
                return;
            }
        };

        self.writer = Some(writer.clone());
        let is_recording = self.is_recording.clone();
        let sample_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sample_count_clone = sample_count.clone();

        let stream = match device.build_input_stream(
            &config.into(),
            move |data: &[f32], _| {
                if !is_recording.load(Ordering::SeqCst) {
                    return;
                }

                sample_count_clone.fetch_add(data.len(), Ordering::SeqCst);

                if let Ok(mut writer) = writer.lock() {
                    for &sample in data {
                        let amplitude = (sample * i16::MAX as f32) as i16;
                        let _ = writer.write_sample(amplitude);
                    }
                }
            },
            move |err| {
                eprintln!("Stream error: {}", err);
            },
            None,
        ) {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("Error building input stream: {}", err);
                return;
            }
        };

        if let Err(err) = stream.play() {
            eprintln!("Error starting stream: {}", err);
            return;
        }

        self.stream = Some(stream);
        self.is_recording.store(true, Ordering::SeqCst);
        println!("Recording started: {}", filename);
        println!("Speak now...");
        self.file_name_history.push(filename.clone());
    }

    fn stop_recording(&mut self) -> Option<String> {
        if !self.is_recording.load(Ordering::SeqCst) {
            return None;
        }

        self.is_recording.store(false, Ordering::SeqCst);

        std::thread::sleep(std::time::Duration::from_millis(100));

        self.stream = None;

        let completed_filename = self.current_filename.take();

        if let Some(writer_arc) = self.writer.take() {
            match Arc::try_unwrap(writer_arc) {
                Ok(writer_mutex) => match writer_mutex.into_inner() {
                    Ok(writer) => {
                        if let Err(err) = writer.finalize() {
                            eprintln!("Error finalizing WAV file: {}", err);
                            return None;
                        } else {
                            println!("Recording saved successfully");
                        }
                    }
                    Err(err) => {
                        eprintln!("Error getting writer mutex: {}", err);
                        return None;
                    }
                },
                Err(_) => {
                    eprintln!(
                        "Error: Multiple references to writer exist, cannot finalize WAV file."
                    );
                    return None;
                }
            }
        }

        completed_filename
    }
}
