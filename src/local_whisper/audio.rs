use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Prepares 16 kHz mono float32 PCM samples from any supported audio/video file.
/// PCM/float WAV decoding, downmixing and resampling are native Rust.
/// Other media formats use the existing ffmpeg import path.
pub fn load_audio_samples_for_whisper(
    input_path: &Path,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if !input_path.exists() {
        return Err(format!("Audio file does not exist: {}", input_path.display()).into());
    }

    if let Ok(mut reader) = hound::WavReader::open(input_path) {
        let spec = reader.spec();
        if spec.channels == 0 || spec.sample_rate == 0 {
            return Err("Invalid WAV channel count or sample rate".into());
        }
        let samples = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<Vec<_>, _>>()?,
            hound::SampleFormat::Int => {
                let scale = 2_f32.powi(spec.bits_per_sample as i32 - 1);
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|v| v as f32 / scale))
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        if samples.iter().any(|s| !s.is_finite()) {
            return Err("Audio contains non-finite samples".into());
        }
        let mono: Vec<f32> = samples
            .chunks_exact(spec.channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / spec.channels as f32)
            .collect();
        return resample_mono(mono, spec.sample_rate);
    }

    // Resample using ffmpeg
    let ffmpeg_path = crate::groq_request::find_ffmpeg().ok_or_else(|| {
        "ffmpeg not found. Please install ffmpeg to process audio files for local transcription.".to_string()
    })?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let temp_wav = std::env::temp_dir().join(format!("wgo_local_prep_{pid}_{ts}.wav"));

    let output = Command::new(&ffmpeg_path)
        .args([
            "-y",
            "-i",
            &input_path.to_string_lossy(),
            "-vn",
            "-acodec",
            "pcm_s16le",
            "-ac",
            "1",
            "-ar",
            "16000",
            &temp_wav.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("Failed to run ffmpeg: {e}"))?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&temp_wav);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg audio preparation failed: {}", stderr.trim()).into());
    }

    let result = (|| -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        let mut reader = hound::WavReader::open(&temp_wav)?;
        let samples: Result<Vec<f32>, _> = reader
            .samples::<i16>()
            .map(|s| s.map(|val| val as f32 / 32768.0))
            .collect();
        Ok(samples?)
    })();

    let _ = std::fs::remove_file(&temp_wav);
    result
}

fn resample_mono(
    samples: Vec<f32>,
    sample_rate: u32,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    use rubato::{FftFixedInOut, Resampler};
    if sample_rate == 16_000 || samples.is_empty() {
        return Ok(samples);
    }
    let mut resampler = FftFixedInOut::<f32>::new(sample_rate as usize, 16_000, 1024, 1)?;
    let delay = resampler.output_delay();
    let wanted = (samples.len() as u64 * 16_000 / sample_rate as u64) as usize;
    let mut output = Vec::with_capacity(wanted + delay + resampler.output_frames_max());
    for chunk in samples.chunks(resampler.input_frames_next()) {
        output.extend_from_slice(&resampler.process_partial(Some(&[chunk]), None)?[0]);
    }
    while output.len() < wanted + delay {
        output.extend_from_slice(&resampler.process_partial::<&[f32]>(None, None)?[0]);
    }
    Ok(output[delay..delay + wanted].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wav_resampling_preserves_duration_and_pitch() {
        for rate in [8000, 16000, 44100, 48000] {
            let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::create(file.path(), spec).unwrap();
            for i in 0..rate {
                let sample = (0.5
                    * 32767.0
                    * (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin())
                    as i16;
                writer.write_sample(sample).unwrap();
                writer.write_sample(sample).unwrap();
            }
            writer.finalize().unwrap();
            let pcm = load_audio_samples_for_whisper(file.path()).unwrap();
            assert_eq!(pcm.len(), 16000);
            // FFT resampling has a fractional-sample phase delay for some ratios;
            // verify the tone's amplitude/frequency, independently of that phase.
            let mut sin_gain = 0.0;
            let mut cos_gain = 0.0;
            for (i, sample) in pcm.iter().enumerate().take(15000).skip(1000) {
                let phase = std::f32::consts::TAU * 440.0 * i as f32 / 16000.0;
                sin_gain += sample * phase.sin() * 2.0 / 14000.0;
                cos_gain += sample * phase.cos() * 2.0 / 14000.0;
            }
            let amplitude = sin_gain.hypot(cos_gain);
            assert!(
                (amplitude - 0.5).abs() < 0.005,
                "sample rate {rate}, amplitude {amplitude}"
            );
            assert!(
                cos_gain.atan2(sin_gain).abs() < 0.18,
                "delay exceeds one output sample"
            );
        }
    }
    #[test]
    fn resampling_keeps_short_audio_and_silence() {
        assert_eq!(resample_mono(vec![0.0; 80], 8000).unwrap(), vec![0.0; 160]);
        assert!(resample_mono(Vec::new(), 48000).unwrap().is_empty());
    }
}
