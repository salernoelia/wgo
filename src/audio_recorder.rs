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
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            stream: None,
            is_recording: Arc::new(AtomicBool::new(false)),
            writer: None,
            file_name_history: vec![],
            current_filename: None,
        }
    }

    fn get_exe_dir() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
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
        let device = match host.default_input_device() {
            Some(device) => device,
            None => {
                eprintln!("No input device available");
                return;
            }
        };

        let config = match device.default_input_config() {
            Ok(config) => config,
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
        let file_path = Self::get_exe_dir().join(&filename);
        self.current_filename = Some(filename.clone());

        let writer = match WavWriter::create(&file_path, spec) {
            Ok(writer) => Arc::new(Mutex::new(writer)),
            Err(err) => {
                eprintln!("Error creating wav file: {}", err);
                return;
            }
        };

        self.writer = Some(writer.clone());
        let is_recording = self.is_recording.clone();

        let stream = match device.build_input_stream(
            &config.into(),
            move |data: &[f32], _| {
                if !is_recording.load(Ordering::SeqCst) {
                    return;
                }

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
        self.file_name_history.push(filename.to_string());
    }

    fn stop_recording(&mut self) -> Option<String> {
        if !self.is_recording.load(Ordering::SeqCst) {
            return None;
        }

        self.is_recording.store(false, Ordering::SeqCst);

        self.stream = None;

        let completed_filename = self.current_filename.take();

        if let Some(writer_arc) = self.writer.take() {
            if let Ok(writer) = Arc::try_unwrap(writer_arc) {
                if let Ok(writer) = writer.into_inner() {
                    if let Err(err) = writer.finalize() {
                        eprintln!("Error finalizing WAV file: {}", err);
                        return None;
                    } else {
                        println!("Recording saved successfully");
                    }
                }
            }
        }

        completed_filename
    }
}
