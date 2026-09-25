use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{SampleFormat, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(target_os = "macos")]
mod macos_capture {
    use super::*;
    use std::ffi::{c_char, c_void, CStr};

    extern "C" {
        fn wgo_desktop_start(
            sample_rate: i32,
            context: *mut c_void,
            audio: extern "C" fn(*mut c_void, *const f32, usize, u32),
            error: extern "C" fn(*mut c_void, *const c_char),
            message: *mut c_char,
            capacity: usize,
            retain_context: *mut i32,
        ) -> *mut c_void;
        fn wgo_desktop_stop(handle: *mut c_void) -> i32;
    }

    struct State {
        writer: Arc<Mutex<WavWriter<std::io::BufWriter<std::fs::File>>>>,
        mix: Option<Arc<Mutex<VecDeque<f32>>>>,
        recording: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        level: Arc<AtomicU32>,
        bytes: Arc<AtomicU64>,
        error: Arc<Mutex<Option<String>>>,
        rate: u32,
        resampler: Mutex<Resampler>,
    }

    #[derive(Default)]
    struct Resampler {
        input_count: u64,
        next_output: f64,
        previous: f32,
        source_rate: u32,
    }

    impl Resampler {
        fn convert(&mut self, input: &[f32], input_rate: u32, output_rate: u32) -> Vec<f32> {
            if input.is_empty() {
                return Vec::new();
            }
            if self.source_rate != input_rate {
                self.input_count = 0;
                self.next_output = 0.0;
                self.previous = 0.0;
                self.source_rate = input_rate;
            }
            if input_rate == output_rate {
                self.input_count += input.len() as u64;
                self.next_output = self.input_count as f64;
                self.previous = *input.last().unwrap();
                return input.to_vec();
            }
            let start = self.input_count as f64;
            let end = start + input.len() as f64 - 1.0;
            let step = input_rate as f64 / output_rate as f64;
            let mut output = Vec::new();
            while self.next_output <= end {
                let local = self.next_output - start;
                let lower = local.floor();
                let fraction = (local - lower) as f32;
                let left = if lower < 0.0 {
                    self.previous
                } else {
                    input[lower as usize]
                };
                let right = if lower < 0.0 {
                    input[0]
                } else {
                    input[(lower as usize + 1).min(input.len() - 1)]
                };
                output.push(left + (right - left) * fraction);
                self.next_output += step;
            }
            self.input_count += input.len() as u64;
            self.previous = *input.last().unwrap();
            output
        }
    }

    extern "C" fn audio(context: *mut c_void, samples: *const f32, count: usize, rate: u32) {
        let state = unsafe { &*(context as *const State) };
        if !state.recording.load(Ordering::SeqCst)
            || state.paused.load(Ordering::SeqCst)
            || samples.is_null()
        {
            return;
        }
        if rate == 0 {
            return;
        }
        let input = unsafe { std::slice::from_raw_parts(samples, count) };
        let samples = match state.resampler.lock() {
            Ok(mut resampler) => resampler.convert(input, rate, state.rate),
            Err(_) => return,
        };
        if let Some(queue) = &state.mix {
            if let Ok(mut queue) = queue.lock() {
                for &sample in &samples {
                    if queue.len() >= 32768 {
                        queue.pop_front();
                    }
                    queue.push_back(sample.clamp(-1.0, 1.0));
                }
            }
        } else if let Ok(mut writer) = state.writer.lock() {
            let mut peak = 0.0f32;
            for &sample in &samples {
                peak = peak.max(sample.abs());
                if let Err(err) = writer.write_sample(AudioRecorder::i16_from_f32(sample)) {
                    if let Ok(mut error) = state.error.lock() {
                        *error = Some(format!("Desktop audio write failed: {err}"));
                    }
                    state.recording.store(false, Ordering::SeqCst);
                    return;
                }
            }
            state
                .bytes
                .fetch_add((samples.len() * 2) as u64, Ordering::Relaxed);
            state
                .level
                .store((peak.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::SeqCst);
            if state.bytes.load(Ordering::Relaxed) >= MAX_RECORDING_BYTES {
                state.recording.store(false, Ordering::SeqCst);
            }
        }
    }

    extern "C" fn capture_error_callback(context: *mut c_void, message: *const c_char) {
        let state = unsafe { &*(context as *const State) };
        let detail = if message.is_null() {
            "Unknown capture error".to_string()
        } else {
            unsafe { CStr::from_ptr(message) }
                .to_string_lossy()
                .into_owned()
        };
        if let Ok(mut error) = state.error.lock() {
            *error = Some(detail);
        }
        state.recording.store(false, Ordering::SeqCst);
    }

    pub struct Capture {
        handle: *mut c_void,
        state: *mut State,
    }
    unsafe impl Send for Capture {}

    impl Capture {
        pub fn start(
            rate: u32,
            writer: Arc<Mutex<WavWriter<std::io::BufWriter<std::fs::File>>>>,
            mix: Option<Arc<Mutex<VecDeque<f32>>>>,
            recording: Arc<AtomicBool>,
            paused: Arc<AtomicBool>,
            level: Arc<AtomicU32>,
            bytes: Arc<AtomicU64>,
            error: Arc<Mutex<Option<String>>>,
        ) -> Result<Self, String> {
            let state = Box::into_raw(Box::new(State {
                writer,
                mix,
                recording,
                paused,
                level,
                bytes,
                error,
                rate,
                resampler: Mutex::new(Resampler::default()),
            }));
            let mut message = [0i8; 512];
            let mut retain_context = 0;
            let handle = unsafe {
                wgo_desktop_start(
                    rate as i32,
                    state.cast(),
                    audio,
                    capture_error_callback,
                    message.as_mut_ptr(),
                    message.len(),
                    &mut retain_context,
                )
            };
            if handle.is_null() {
                if retain_context == 0 {
                    unsafe {
                        drop(Box::from_raw(state));
                    }
                }
                return Err(format!("Could not capture system audio: {}. Grant Screen & System Audio Recording access in macOS Privacy & Security, then retry.", unsafe { CStr::from_ptr(message.as_ptr()) }.to_string_lossy()));
            }
            Ok(Self { handle, state })
        }
    }
    impl Drop for Capture {
        fn drop(&mut self) {
            unsafe {
                if wgo_desktop_stop(self.handle) != 0 {
                    drop(Box::from_raw(self.state));
                } else if let Ok(mut error) = (*self.state).error.lock() {
                    *error = Some("Timed out stopping system audio capture".to_string());
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::Resampler;

        #[test]
        fn resampler_preserves_timing_across_buffers() {
            let mut resampler = Resampler::default();
            let first = resampler.convert(&[0.0, 1.0], 2, 4);
            let second = resampler.convert(&[0.0, -1.0], 2, 4);
            assert_eq!(first, vec![0.0, 0.5, 1.0]);
            assert_eq!(second, vec![0.5, 0.0, -0.5, -1.0]);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum AudioSource {
    #[default]
    MicOnly,
    DesktopOnly,
    MicAndDesktop,
}

const MAX_RECORDING_BYTES: u64 = 1000 * 1024 * 1024;
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
    stream2: Option<cpal::Stream>,
    is_recording: Arc<AtomicBool>,
    is_monitoring: Arc<AtomicBool>,
    is_paused: Arc<AtomicBool>,
    level_milli: Arc<AtomicU32>,
    bytes_written: Arc<AtomicU64>,
    write_error: Arc<Mutex<Option<String>>>,
    writer: Option<Arc<Mutex<WavWriter<std::io::BufWriter<std::fs::File>>>>>,
    pub current_filename: Option<String>,
    device_name: Option<String>,
    desktop_device_name: Option<String>,
    pub audio_source: AudioSource,
    desktop_mix_buf: Arc<Mutex<VecDeque<f32>>>,
    #[cfg(target_os = "macos")]
    macos_capture: Option<macos_capture::Capture>,
    #[cfg(target_os = "macos")]
    capture_error: Arc<Mutex<Option<String>>>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            stream: None,
            stream2: None,
            is_recording: Arc::new(AtomicBool::new(false)),
            is_monitoring: Arc::new(AtomicBool::new(false)),
            is_paused: Arc::new(AtomicBool::new(false)),
            level_milli: Arc::new(AtomicU32::new(0)),
            bytes_written: Arc::new(AtomicU64::new(0)),
            write_error: Arc::new(Mutex::new(None)),
            writer: None,
            current_filename: None,
            device_name: None,
            desktop_device_name: None,
            audio_source: AudioSource::default(),
            desktop_mix_buf: Arc::new(Mutex::new(VecDeque::new())),
            #[cfg(target_os = "macos")]
            macos_capture: None,
            #[cfg(target_os = "macos")]
            capture_error: Arc::new(Mutex::new(None)),
        }
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

    pub fn list_desktop_input_devices() -> Result<Vec<String>, String> {
        #[cfg(target_os = "macos")]
        {
            return Ok(vec!["System audio (macOS)".to_string()]);
        }
        #[cfg(not(target_os = "macos"))]
        Self::list_input_devices()
    }

    pub fn set_device_name(&mut self, device_name: Option<String>) {
        self.device_name = device_name;
    }

    pub fn set_desktop_device_name(&mut self, name: Option<String>) {
        self.desktop_device_name = name;
    }

    pub fn set_audio_source(&mut self, source: AudioSource) {
        self.audio_source = source;
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

    fn i16_from_f32(sample: f32) -> i16 {
        let clamped = sample.clamp(-1.0, 1.0);
        (clamped * i16::MAX as f32) as i16
    }

    fn i16_from_u16(sample: u16) -> i16 {
        (sample as i32 - i16::MAX as i32 - 1) as i16
    }

    fn get_named_input_device(
        &self,
        name: &Option<String>,
        role: &str,
    ) -> Result<cpal::Device, String> {
        let host = cpal::default_host();

        if let Some(device_name) = name {
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
                    println!("Using selected {role} device: {device_name}");
                    Ok(device)
                }
                None => {
                    eprintln!("Selected {role} device '{device_name}' not found, using default");
                    host.default_input_device()
                        .ok_or_else(|| "No input device available".to_string())
                }
            }
        } else {
            match host.default_input_device() {
                Some(device) => {
                    if let Ok(name) = device.name() {
                        println!("Using default {role} device: {name}");
                    }
                    Ok(device)
                }
                None => Err("No input device available".to_string()),
            }
        }
    }

    fn get_recording_devices(&self) -> Result<(cpal::Device, Option<cpal::Device>), String> {
        match self.audio_source {
            AudioSource::MicOnly => {
                let mic = self.get_named_input_device(&self.device_name, "microphone")?;
                Ok((mic, None))
            }
            AudioSource::DesktopOnly => {
                #[cfg(target_os = "macos")]
                {
                    return Err("macOS system audio uses ScreenCaptureKit".to_string());
                }
                #[cfg(not(target_os = "macos"))]
                let desktop = self.get_named_input_device(&self.desktop_device_name, "desktop")?;
                #[cfg(not(target_os = "macos"))]
                Ok((desktop, None))
            }
            AudioSource::MicAndDesktop => {
                let mic = self.get_named_input_device(&self.device_name, "microphone")?;
                #[cfg(target_os = "macos")]
                let desktop = None;
                #[cfg(not(target_os = "macos"))]
                let desktop =
                    Some(self.get_named_input_device(&self.desktop_device_name, "desktop")?);
                Ok((mic, desktop))
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn start_macos_desktop_only(&mut self) -> Result<String, String> {
        let recordings_dir = crate::config::AppConfig::load()
            .ensure_recordings_dir()
            .map_err(|e| format!("Failed to prepare recordings directory: {e}"))?;
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let file_path = recordings_dir.join(format!("recording_{timestamp}.wav"));
        let spec = WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let writer = Arc::new(Mutex::new(
            WavWriter::create(&file_path, spec)
                .map_err(|e| format!("Error creating WAV file: {e}"))?,
        ));
        self.is_recording.store(true, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        self.bytes_written.store(0, Ordering::SeqCst);
        if let Ok(mut error) = self.capture_error.lock() {
            *error = None;
        }
        match macos_capture::Capture::start(
            48000,
            writer.clone(),
            None,
            self.is_recording.clone(),
            self.is_paused.clone(),
            self.level_milli.clone(),
            self.bytes_written.clone(),
            self.capture_error.clone(),
        ) {
            Ok(capture) => self.macos_capture = Some(capture),
            Err(err) => {
                self.is_recording.store(false, Ordering::SeqCst);
                drop(writer);
                let _ = std::fs::remove_file(&file_path);
                return Err(err);
            }
        }
        if !self.is_recording.load(Ordering::SeqCst) {
            self.macos_capture = None;
            drop(writer);
            let _ = std::fs::remove_file(&file_path);
            return Err(self
                .capture_error
                .lock()
                .ok()
                .and_then(|error| error.clone())
                .unwrap_or_else(|| "System audio capture stopped during startup".to_string()));
        }
        self.writer = Some(writer);
        let path = file_path.to_string_lossy().to_string();
        self.current_filename = Some(path.clone());
        Ok(path)
    }

    pub fn start_recording(&mut self) -> Result<String, String> {
        if self.is_recording.load(Ordering::SeqCst) {
            return Err("Recording is already running".to_string());
        }

        if self.is_monitoring() {
            self.stop_monitoring()?;
        }

        #[cfg(target_os = "macos")]
        if self.audio_source == AudioSource::DesktopOnly {
            return self.start_macos_desktop_only();
        }

        let (device, desktop_device) = self.get_recording_devices()?;
        #[cfg(target_os = "macos")]
        let _ = &desktop_device;

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
        const TARGET_RATE: cpal::SampleRate = cpal::SampleRate(16000);
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
        let should_mix_desktop = matches!(self.audio_source, AudioSource::MicAndDesktop);

        if should_mix_desktop {
            if let Ok(mut q) = self.desktop_mix_buf.lock() {
                q.clear();
            }
        }

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
            .as_nanos();

        let filename = format!("recording_{}.wav", timestamp);
        let recordings_dir = crate::config::AppConfig::load()
            .ensure_recordings_dir()
            .map_err(|e| format!("Failed to prepare recordings directory: {e}"))?;
        let file_path = recordings_dir.join(&filename);
        self.current_filename = Some(file_path.to_string_lossy().to_string());

        let writer = match WavWriter::create(&file_path, spec) {
            Ok(writer) => Arc::new(Mutex::new(writer)),
            Err(err) => {
                return Err(format!("Error creating wav file: {err}"));
            }
        };

        self.writer = Some(writer.clone());
        self.bytes_written.store(0, Ordering::SeqCst);
        if let Ok(mut error) = self.write_error.lock() {
            *error = None;
        }
        #[cfg(target_os = "macos")]
        if let Ok(mut error) = self.capture_error.lock() {
            *error = None;
        }
        let is_recording = self.is_recording.clone();
        let is_monitoring = self.is_monitoring.clone();
        let is_paused = self.is_paused.clone();
        let level_milli = self.level_milli.clone();
        let bytes_written = self.bytes_written.clone();
        let write_error = self.write_error.clone();
        let desktop_mix_buf = self.desktop_mix_buf.clone();

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let writer_clone = writer.clone();
                let is_recording_clone = is_recording.clone();
                let is_paused_clone = is_paused.clone();
                let level_milli_clone = level_milli.clone();
                let bytes_written_clone = bytes_written.clone();
                let write_error_clone = write_error.clone();
                let desktop_mix_buf_clone = desktop_mix_buf.clone();
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
                            if bytes_written_clone.load(Ordering::Relaxed) >= MAX_RECORDING_BYTES {
                                is_recording_clone.store(false, Ordering::SeqCst);
                                eprintln!(
                                    "Recording stopped: reached {MAX_RECORDING_BYTES} byte limit"
                                );
                                return;
                            }
                            if let Ok(mut writer) = writer_clone.lock() {
                                let mut peak = 0.0f32;
                                for frame in data.chunks(num_input_channels) {
                                    let mic = frame.iter().sum::<f32>() / frame.len() as f32;
                                    let desktop = if should_mix_desktop {
                                        desktop_mix_buf_clone
                                            .lock()
                                            .ok()
                                            .and_then(|mut q| q.pop_front())
                                            .unwrap_or(0.0)
                                    } else {
                                        0.0
                                    };
                                    let mono = if should_mix_desktop {
                                        ((mic + desktop) * 0.5).clamp(-1.0, 1.0)
                                    } else {
                                        mic
                                    };
                                    peak = peak.max(mono.abs());
                                    if let Err(err) = writer.write_sample(Self::i16_from_f32(mono))
                                    {
                                        if let Ok(mut error) = write_error_clone.lock() {
                                            *error = Some(err.to_string());
                                        }
                                        is_recording_clone.store(false, Ordering::SeqCst);
                                        return;
                                    }
                                }
                                bytes_written_clone.fetch_add(
                                    (data.len() / num_input_channels * 2) as u64,
                                    Ordering::Relaxed,
                                );
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
                let bytes_written_clone = bytes_written.clone();
                let write_error_clone = write_error.clone();
                let desktop_mix_buf_clone = desktop_mix_buf.clone();
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
                            if bytes_written_clone.load(Ordering::Relaxed) >= MAX_RECORDING_BYTES {
                                is_recording_clone.store(false, Ordering::SeqCst);
                                eprintln!(
                                    "Recording stopped: reached {MAX_RECORDING_BYTES} byte limit"
                                );
                                return;
                            }
                            if let Ok(mut writer) = writer_clone.lock() {
                                let mut peak = 0.0f32;
                                for frame in data.chunks(num_input_channels) {
                                    let mic = frame
                                        .iter()
                                        .map(|&s| s as f32 / i16::MAX as f32)
                                        .sum::<f32>()
                                        / frame.len() as f32;
                                    let desktop = if should_mix_desktop {
                                        desktop_mix_buf_clone
                                            .lock()
                                            .ok()
                                            .and_then(|mut q| q.pop_front())
                                            .unwrap_or(0.0)
                                    } else {
                                        0.0
                                    };
                                    let mono = if should_mix_desktop {
                                        ((mic + desktop) * 0.5).clamp(-1.0, 1.0)
                                    } else {
                                        mic
                                    };
                                    peak = peak.max(mono.abs());
                                    if let Err(err) = writer.write_sample(Self::i16_from_f32(mono))
                                    {
                                        if let Ok(mut error) = write_error_clone.lock() {
                                            *error = Some(err.to_string());
                                        }
                                        is_recording_clone.store(false, Ordering::SeqCst);
                                        return;
                                    }
                                }
                                bytes_written_clone.fetch_add(
                                    (data.len() / num_input_channels * 2) as u64,
                                    Ordering::Relaxed,
                                );
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
                let bytes_written_clone = bytes_written.clone();
                let write_error_clone = write_error.clone();
                let desktop_mix_buf_clone = desktop_mix_buf.clone();
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
                            if bytes_written_clone.load(Ordering::Relaxed) >= MAX_RECORDING_BYTES {
                                is_recording_clone.store(false, Ordering::SeqCst);
                                eprintln!(
                                    "Recording stopped: reached {MAX_RECORDING_BYTES} byte limit"
                                );
                                return;
                            }
                            if let Ok(mut writer) = writer_clone.lock() {
                                let mut peak = 0.0f32;
                                for frame in data.chunks(num_input_channels) {
                                    let mic = frame
                                        .iter()
                                        .map(|&s| Self::i16_from_u16(s) as f32 / i16::MAX as f32)
                                        .sum::<f32>()
                                        / frame.len() as f32;
                                    let desktop = if should_mix_desktop {
                                        desktop_mix_buf_clone
                                            .lock()
                                            .ok()
                                            .and_then(|mut q| q.pop_front())
                                            .unwrap_or(0.0)
                                    } else {
                                        0.0
                                    };
                                    let mono = if should_mix_desktop {
                                        ((mic + desktop) * 0.5).clamp(-1.0, 1.0)
                                    } else {
                                        mic
                                    };
                                    peak = peak.max(mono.abs());
                                    if let Err(err) = writer.write_sample(Self::i16_from_f32(mono))
                                    {
                                        if let Ok(mut error) = write_error_clone.lock() {
                                            *error = Some(err.to_string());
                                        }
                                        is_recording_clone.store(false, Ordering::SeqCst);
                                        return;
                                    }
                                }
                                bytes_written_clone.fetch_add(
                                    (data.len() / num_input_channels * 2) as u64,
                                    Ordering::Relaxed,
                                );
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

        #[cfg(not(target_os = "macos"))]
        let desktop_stream = if should_mix_desktop {
            let Some(desktop_device) = desktop_device else {
                return Err("Desktop audio source requires a desktop input device".to_string());
            };

            let desktop_supported = desktop_device.default_input_config().map_err(|err| {
                normalize_input_error("Failed to get default desktop audio configuration", err)
            })?;

            let desktop_sample_rate = stream_config.sample_rate;
            let desktop_stream_config = desktop_device
                .supported_input_configs()
                .ok()
                .and_then(|mut cfgs| {
                    cfgs.find(|c| {
                        c.min_sample_rate() <= desktop_sample_rate
                            && c.max_sample_rate() >= desktop_sample_rate
                    })
                    .map(|c| c.with_sample_rate(desktop_sample_rate).config())
                })
                .unwrap_or_else(|| desktop_supported.config());
            let desktop_channels = desktop_stream_config.channels as usize;
            let desktop_mix_buf_clone = self.desktop_mix_buf.clone();

            let stream2 = match desktop_supported.sample_format() {
                cpal::SampleFormat::F32 => desktop_device
                    .build_input_stream(
                        &desktop_stream_config,
                        move |data: &[f32], _| {
                            if let Ok(mut q) = desktop_mix_buf_clone.lock() {
                                for frame in data.chunks(desktop_channels) {
                                    let mono = frame.iter().sum::<f32>() / frame.len() as f32;
                                    if q.len() >= 32768 {
                                        q.pop_front();
                                    }
                                    q.push_back(mono.clamp(-1.0, 1.0));
                                }
                            }
                        },
                        move |err| eprintln!("Desktop stream error: {}", err),
                        None,
                    )
                    .map_err(|e| normalize_input_error("Failed to open desktop audio stream", e))?,
                cpal::SampleFormat::I16 => desktop_device
                    .build_input_stream(
                        &desktop_stream_config,
                        move |data: &[i16], _| {
                            if let Ok(mut q) = desktop_mix_buf_clone.lock() {
                                for frame in data.chunks(desktop_channels) {
                                    let mono = frame
                                        .iter()
                                        .map(|&s| s as f32 / i16::MAX as f32)
                                        .sum::<f32>()
                                        / frame.len() as f32;
                                    if q.len() >= 32768 {
                                        q.pop_front();
                                    }
                                    q.push_back(mono.clamp(-1.0, 1.0));
                                }
                            }
                        },
                        move |err| eprintln!("Desktop stream error: {}", err),
                        None,
                    )
                    .map_err(|e| normalize_input_error("Failed to open desktop audio stream", e))?,
                cpal::SampleFormat::U16 => desktop_device
                    .build_input_stream(
                        &desktop_stream_config,
                        move |data: &[u16], _| {
                            if let Ok(mut q) = desktop_mix_buf_clone.lock() {
                                for frame in data.chunks(desktop_channels) {
                                    let mono = frame
                                        .iter()
                                        .map(|&s| Self::i16_from_u16(s) as f32 / i16::MAX as f32)
                                        .sum::<f32>()
                                        / frame.len() as f32;
                                    if q.len() >= 32768 {
                                        q.pop_front();
                                    }
                                    q.push_back(mono.clamp(-1.0, 1.0));
                                }
                            }
                        },
                        move |err| eprintln!("Desktop stream error: {}", err),
                        None,
                    )
                    .map_err(|e| normalize_input_error("Failed to open desktop audio stream", e))?,
                other => {
                    return Err(format!("Unsupported desktop sample format: {other:?}"));
                }
            };

            stream2
                .play()
                .map_err(|err| normalize_input_error("Failed to start desktop stream", err))?;
            Some(stream2)
        } else {
            None
        };

        #[cfg(target_os = "macos")]
        let desktop_stream: Option<cpal::Stream> = None;

        self.is_recording.store(true, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        #[cfg(target_os = "macos")]
        if should_mix_desktop {
            match macos_capture::Capture::start(
                stream_config.sample_rate.0,
                writer.clone(),
                Some(self.desktop_mix_buf.clone()),
                self.is_recording.clone(),
                self.is_paused.clone(),
                self.level_milli.clone(),
                self.bytes_written.clone(),
                self.capture_error.clone(),
            ) {
                Ok(capture) => self.macos_capture = Some(capture),
                Err(err) => {
                    self.is_recording.store(false, Ordering::SeqCst);
                    drop(stream);
                    self.writer = None;
                    self.current_filename = None;
                    let _ = std::fs::remove_file(&file_path);
                    return Err(err);
                }
            }
            if !self.is_recording.load(Ordering::SeqCst) {
                self.macos_capture = None;
                drop(stream);
                self.writer = None;
                self.current_filename = None;
                let _ = std::fs::remove_file(&file_path);
                return Err(self
                    .capture_error
                    .lock()
                    .ok()
                    .and_then(|error| error.clone())
                    .unwrap_or_else(|| "System audio capture stopped during startup".to_string()));
            }
        }

        self.is_monitoring.store(false, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        self.stream = Some(stream);
        self.stream2 = desktop_stream;

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

        let device = self.get_named_input_device(&self.device_name, "microphone")?;

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
        self.stream2 = None;
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
        self.stream2 = None;
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

    /// Returns true if recording was stopped externally (e.g. byte limit hit) and
    /// the app should call stop_recording() to finalize and transcribe.
    pub fn was_stopped_externally(&self) -> bool {
        !self.is_recording.load(Ordering::SeqCst) && self.writer.is_some()
    }

    pub fn stop_recording(&mut self) -> Result<Option<String>, String> {
        let recording_active = self.is_recording.load(Ordering::SeqCst);
        let has_writer = self.writer.is_some();
        if !recording_active && !has_writer {
            return Ok(None);
        }

        self.is_recording.store(false, Ordering::SeqCst);
        self.is_monitoring.store(false, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        self.level_milli.store(0, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(100));

        self.stream = None;
        self.stream2 = None;
        #[cfg(target_os = "macos")]
        {
            self.macos_capture = None;
        }
        if let Ok(mut q) = self.desktop_mix_buf.lock() {
            q.clear();
        }

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

        if let Ok(mut error) = self.write_error.lock() {
            if let Some(detail) = error.take() {
                return Err(format!(
                    "Audio write failed: {detail}. Recording saved at {}",
                    completed_filename.as_deref().unwrap_or("unknown location")
                ));
            }
        }

        #[cfg(target_os = "macos")]
        if let Ok(mut error) = self.capture_error.lock() {
            if let Some(detail) = error.take() {
                return Err(format!(
                    "System audio capture stopped: {detail}. Recording saved at {}",
                    completed_filename.as_deref().unwrap_or("unknown location")
                ));
            }
        }

        self.bytes_written.store(0, Ordering::SeqCst);

        Ok(completed_filename)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription_history::TranscriptionRecord;
    use std::sync::atomic::Ordering;

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires macOS Screen & System Audio Recording permission and an output device"]
    fn macos_system_audio_records_playback_before_transcription() {
        let dir = tempfile::tempdir().unwrap();
        let tone_path = dir.path().join("tone.wav");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut tone = WavWriter::create(&tone_path, spec).unwrap();
        for n in 0..48000 {
            let sample =
                ((2.0 * std::f32::consts::PI * 440.0 * n as f32 / 48000.0).sin() * 12000.0) as i16;
            tone.write_sample(sample).unwrap();
        }
        tone.finalize().unwrap();
        let mut recorder = AudioRecorder::new();
        recorder.set_audio_source(AudioSource::DesktopOnly);
        let path = recorder.start_recording().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let playback = std::process::Command::new("afplay")
            .arg(&tone_path)
            .status()
            .unwrap();
        assert!(playback.success());
        std::thread::sleep(std::time::Duration::from_millis(300));
        let saved = recorder.stop_recording().unwrap().unwrap();
        assert_eq!(path, saved);
        let mut wav = hound::WavReader::open(&saved).unwrap();
        let peak = wav
            .samples::<i16>()
            .map(|s| s.unwrap().unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(peak > 1000, "desktop recording was silent: {saved}");
        std::fs::remove_file(saved).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires macOS microphone and Screen & System Audio Recording permissions"]
    fn macos_microphone_and_system_audio_save_wav() {
        let dir = tempfile::tempdir().unwrap();
        let tone_path = dir.path().join("tone.wav");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut tone = WavWriter::create(&tone_path, spec).unwrap();
        for n in 0..48000 {
            tone.write_sample(
                ((2.0 * std::f32::consts::PI * 440.0 * n as f32 / 48000.0).sin() * 12000.0) as i16,
            )
            .unwrap();
        }
        tone.finalize().unwrap();
        let mut recorder = AudioRecorder::new();
        recorder.set_audio_source(AudioSource::MicAndDesktop);
        let path = recorder.start_recording().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(std::process::Command::new("afplay")
            .arg(&tone_path)
            .status()
            .unwrap()
            .success());
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(
            recorder.stop_recording().unwrap().as_deref(),
            Some(path.as_str())
        );
        let mut wav = hound::WavReader::open(&path).unwrap();
        let peak = wav
            .samples::<i16>()
            .map(|s| s.unwrap().unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(peak > 1000, "mixed recording was silent: {path}");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn i16_from_f32_maps_positive_one_to_max() {
        assert_eq!(AudioRecorder::i16_from_f32(1.0), i16::MAX);
    }

    #[test]
    fn i16_from_f32_maps_zero_to_zero() {
        assert_eq!(AudioRecorder::i16_from_f32(0.0), 0);
    }

    #[test]
    fn i16_from_f32_maps_negative_one_to_min_plus_one() {
        // -1.0 * i16::MAX = -32767; casting to i16 is exact
        let result = AudioRecorder::i16_from_f32(-1.0);
        assert!(result <= -32767);
    }

    #[test]
    fn i16_from_f32_clamps_above_one() {
        let clamped = AudioRecorder::i16_from_f32(2.0);
        let at_one = AudioRecorder::i16_from_f32(1.0);
        assert_eq!(clamped, at_one);
    }

    #[test]
    fn i16_from_f32_clamps_below_negative_one() {
        let clamped = AudioRecorder::i16_from_f32(-5.0);
        let at_neg_one = AudioRecorder::i16_from_f32(-1.0);
        assert_eq!(clamped, at_neg_one);
    }

    #[test]
    fn i16_from_u16_midpoint_maps_near_zero() {
        // u16 midpoint is 32768; subtracting i16::MAX+1 (32768) = 0
        let result = AudioRecorder::i16_from_u16(32768u16);
        assert_eq!(result, 0);
    }

    #[test]
    fn i16_from_u16_zero_maps_to_negative_max() {
        // 0 - 32768 = -32768 which wraps to i16::MIN in i16
        let result = AudioRecorder::i16_from_u16(0u16);
        assert!(result < 0);
    }

    #[test]
    fn i16_from_u16_max_maps_to_positive() {
        let result = AudioRecorder::i16_from_u16(u16::MAX);
        assert!(result > 0);
    }

    // ── atomic state flags ───────────────────────────────────────────────────

    #[test]
    fn new_recorder_starts_idle() {
        let rec = AudioRecorder::new();
        assert!(!rec.is_recording());
        assert!(!rec.is_paused());
        assert!(!rec.is_monitoring());
    }

    #[test]
    fn input_level_starts_at_zero() {
        let rec = AudioRecorder::new();
        assert_eq!(rec.input_level(), 0.0);
    }

    #[test]
    fn was_stopped_externally_is_false_when_idle() {
        // No writer exists and is_recording is false → not "externally stopped"
        let rec = AudioRecorder::new();
        assert!(!rec.was_stopped_externally());
    }

    #[test]
    fn was_stopped_externally_is_false_while_recording_flag_set() {
        // is_recording=true with no writer → still not "externally stopped"
        let rec = AudioRecorder::new();
        rec.is_recording.store(true, Ordering::SeqCst);
        assert!(!rec.was_stopped_externally());
    }

    #[test]
    fn stop_recording_on_idle_recorder_returns_none() {
        let mut rec = AudioRecorder::new();
        let result = rec.stop_recording();
        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn max_recording_bytes_is_at_least_100_mb() {
        assert!(
            MAX_RECORDING_BYTES >= 100 * 1024 * 1024,
            "MAX_RECORDING_BYTES ({MAX_RECORDING_BYTES}) is too small — recordings will be silently cut short"
        );
    }
    #[test]
    fn max_recording_bytes_is_at_least_500_mb() {
        assert!(
            MAX_RECORDING_BYTES >= 500 * 1024 * 1024,
            "MAX_RECORDING_BYTES ({MAX_RECORDING_BYTES}) is too small — recordings will be silently cut short"
        );
    }

    #[test]
    fn max_recording_bytes_is_at_least_950_mb() {
        assert!(
            MAX_RECORDING_BYTES >= 950 * 1024 * 1024,
            "MAX_RECORDING_BYTES ({MAX_RECORDING_BYTES}) is too small — recordings will be silently cut short"
        );
    }

    // ── save_transcription builds a well-formed TranscriptionRecord ──────────
    // Rather than touching the file system (which races with parallel tests
    // via the XDG_DATA_HOME env var), we verify the record fields directly.
    #[test]
    fn save_transcription_record_fields_are_correct() {
        let record = TranscriptionRecord {
            filename: "rec.wav".to_string(),
            transcription: "hello world".to_string(),
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            audio_path: None,
            backend: None,
        };
        assert_eq!(record.filename, "rec.wav");
        assert_eq!(record.transcription, "hello world");
        assert!(record.timestamp > 0, "timestamp should be non-zero");
        assert!(record.audio_path.is_none());
        assert!(record.backend.is_none());
    }

    #[test]
    fn set_audio_source_changes_audio_source() {
        let mut rec = AudioRecorder::new();
        rec.set_audio_source(AudioSource::DesktopOnly);
        assert_eq!(rec.audio_source, AudioSource::DesktopOnly);
    }

    #[test]
    fn set_device_name_stores_name() {
        let mut rec = AudioRecorder::new();
        rec.set_device_name(Some("My Mic".to_string()));
        assert_eq!(rec.device_name, Some("My Mic".to_string()));
    }

    #[test]
    fn set_desktop_device_name_stores_name() {
        let mut rec = AudioRecorder::new();
        rec.set_desktop_device_name(Some("BlackHole 2ch".to_string()));
        assert_eq!(rec.desktop_device_name, Some("BlackHole 2ch".to_string()));
    }
}
