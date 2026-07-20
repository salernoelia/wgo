use crate::config::AppConfig;
use reqwest::blocking::multipart::{Form, Part};
use reqwest::blocking::Client;
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const SUPPORTED_AUDIO_EXTS: [&str; 7] = ["mp3", "wav", "m4a", "ogg", "opus", "flac", "webm"];
const SUPPORTED_VIDEO_EXTS: [&str; 19] = [
    "mp4", "mov", "m4v", "mkv", "avi", "webm", "mpg", "mpeg",
    "f4v", "flv", "ts", "mts", "m2ts", "wmv", "3gp", "ogv",
    "rm", "rmvb", "vob",
];

fn mime_for_ext(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "webm" => "audio/webm",
        _ => "application/octet-stream",
    }
}

fn is_video_ext(ext: &str) -> bool {
    SUPPORTED_VIDEO_EXTS
        .iter()
        .any(|candidate| candidate == &ext)
}

pub fn is_video_file(file_path: &str) -> bool {
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    is_video_ext(&ext)
}

pub fn is_supported_media_file(file_path: &str) -> bool {
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    SUPPORTED_AUDIO_EXTS
        .iter()
        .any(|candidate| candidate == &ext)
        || SUPPORTED_VIDEO_EXTS
            .iter()
            .any(|candidate| candidate == &ext)
}

fn find_ffmpeg() -> Option<PathBuf> {
    // Check PATH first (works in terminal-launched contexts)
    if let Ok(p) = which::which("ffmpeg") {
        return Some(p);
    }
    // macOS GUI apps don't inherit shell PATH — probe common install locations
    let candidates = [
        "/opt/homebrew/bin/ffmpeg",   // Apple Silicon Homebrew
        "/usr/local/bin/ffmpeg",       // Intel Homebrew / manual installs
        "/usr/bin/ffmpeg",
        "/opt/local/bin/ffmpeg",       // MacPorts
    ];
    for path in candidates {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn extract_audio_from_video(input_path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let ffmpeg_path = find_ffmpeg().ok_or_else(|| {
        "ffmpeg not found. Install it with: brew install ffmpeg".to_string()
    })?;

    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("video");
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let pid = std::process::id();
    let output_path = std::env::temp_dir().join(format!("wgo_extract_{stem}_{pid}_{ts}.m4a"));

    // Invoke ffmpeg directly using the resolved path — avoids rust_ffmpeg's
    // internal which() call that fails in macOS GUI apps without full shell PATH.
    let output = Command::new(&ffmpeg_path)
        .args([
            "-y",
            "-i",
            &input_path.to_string_lossy(),
            "-vn",
            "-acodec", "aac",
            "-ac", "1",
            "-ar", "16000",
            "-b:a", "12k",
            &output_path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| {
            format!(
                "Failed to run ffmpeg for video transcription. Install ffmpeg and try again. Details: {e}"
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let details = stderr.trim();
        return Err(format!(
            "Failed to extract audio from video. ffmpeg exited with status {}. {}",
            output.status,
            if details.is_empty() {
                "No additional error output.".to_string()
            } else {
                format!("Details: {details}")
            }
        )
        .into());
    }

    Ok(output_path)
}

fn prepare_media_for_transcription(
    file_path: &Path,
) -> Result<(PathBuf, Option<PathBuf>), Box<dyn std::error::Error>> {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    if is_video_ext(&ext) {
        let extracted = extract_audio_from_video(file_path)?;
        return Ok((extracted.clone(), Some(extracted)));
    }

    Ok((file_path.to_path_buf(), None))
}

fn get_exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn get_recordings_dir() -> PathBuf {
    AppConfig::load().recordings_dir_path()
}

fn resolve_audio_file_path(file_path: &str) -> PathBuf {
    resolve_audio_file_path_with(file_path, &get_recordings_dir(), &get_exe_dir())
}

fn resolve_audio_file_path_with(file_path: &str, recordings_dir: &Path, exe_dir: &Path) -> PathBuf {
    if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else if file_path.contains('/') || file_path.contains('\\') {
        PathBuf::from(file_path)
    } else {
        let recordings_path = recordings_dir.join(file_path);
        if recordings_path.exists() {
            recordings_path
        } else {
            exe_dir.join(file_path)
        }
    }
}

fn send_transcription_request(
    api_key: &str,
    audio_file_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let url = "https://api.groq.com/openai/v1/audio/transcriptions";

    let mut file = File::open(audio_file_path)
        .map_err(|e| format!("Failed to open audio file at {:?}: {}", audio_file_path, e))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let file_name = audio_file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "audio.m4a".to_string());

    let mime = audio_file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(mime_for_ext)
        .unwrap_or("application/octet-stream");

    let file_part = Part::bytes(buffer).file_name(file_name).mime_str(mime)?;

    let form = Form::new()
        .part("file", file_part)
        .text("model", "whisper-large-v3");

    let client = Client::new();
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()?;

    if response.status().is_success() {
        let json: Value = response.json()?;

        if let Some(text) = json.get("text").and_then(Value::as_str) {
            Ok(text.to_owned())
        } else {
            Ok("No transcription text found in response".to_owned())
        }
    } else {
        let status = response.status();
        let error_body = response
            .text()
            .unwrap_or_else(|_| "Unable to read response body".to_string());
        Err(format!("API request failed with status {}: {}", status, error_body).into())
    }
}

fn chunk_audio_file(
    ffmpeg_path: &Path,
    input_path: &Path,
    temp_dir: &Path,
    segment_time_secs: u32,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(temp_dir)?;
    
    let output_pattern = temp_dir.join("chunk_%03d.m4a");
    
    let output = Command::new(ffmpeg_path)
        .args([
            "-y",
            "-i",
            &input_path.to_string_lossy(),
            "-f", "segment",
            "-segment_time", &segment_time_secs.to_string(),
            "-reset_timestamps", "1",
            "-c:a", "aac",
            "-ac", "1",
            "-ar", "16000",
            "-b:a", "64k",
            &output_pattern.to_string_lossy(),
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Failed to chunk audio file. ffmpeg exited with status {}. Details: {}",
            output.status,
            stderr.trim()
        )
        .into());
    }

    // Read the chunk files in alphabetical order
    let mut chunks = Vec::new();
    for entry in std::fs::read_dir(temp_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                if file_name.starts_with("chunk_") && file_name.ends_with(".m4a") {
                    chunks.push(path);
                }
            }
        }
    }
    
    // Sort alphabetically so chunk_000 is first, chunk_001 is second, etc.
    chunks.sort();
    
    Ok(chunks)
}

pub fn transcribe_audio(file_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let config = AppConfig::load();
    let api_key = config.groq_api_key.trim();

    if api_key.is_empty() {
        return Err("Groq API key is empty. Set it in the app settings.".into());
    }

    transcribe_audio_inner(file_path, api_key, send_transcription_request)
}

fn transcribe_audio_inner<F>(
    file_path: &str,
    api_key: &str,
    send_req: F,
) -> Result<String, Box<dyn std::error::Error>>
where
    F: Fn(&str, &Path) -> Result<String, Box<dyn std::error::Error>>,
{
    let media_file_path = resolve_audio_file_path(file_path);

    let (audio_file_path, cleanup_path) = prepare_media_for_transcription(&media_file_path)?;

    // Check size of the audio file to determine if we need to chunk it.
    let file_metadata = std::fs::metadata(&audio_file_path)
        .map_err(|e| format!("Failed to read metadata of audio file at {:?}: {}", audio_file_path, e))?;
    let file_size = file_metadata.len();

    let result = if file_size >= 20 * 1024 * 1024 {
        // Chunking path
        let ffmpeg_path = find_ffmpeg().ok_or_else(|| {
            "ffmpeg not found. Install ffmpeg to transcribe files larger than 20MB.".to_string()
        })?;

        let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let pid = std::process::id();
        let temp_dir = std::env::temp_dir().join(format!("wgo_chunks_{}_{}", pid, ts));

        let chunk_results = match chunk_audio_file(&ffmpeg_path, &audio_file_path, &temp_dir, 600) {
            Ok(chunks) => {
                let mut results = Vec::new();
                let mut err = None;
                for chunk in chunks {
                    match send_req(api_key, &chunk) {
                        Ok(text) => results.push(text),
                        Err(e) => {
                            err = Some(e);
                            break;
                        }
                    }
                }
                let _ = std::fs::remove_dir_all(&temp_dir);
                if let Some(e) = err {
                    return Err(e);
                }
                results
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&temp_dir);
                return Err(e);
            }
        };

        // Combine the results with spaces
        Ok(chunk_results.join(" "))
    } else {
        // Direct transcription path
        send_req(api_key, &audio_file_path)
    };

    if let Some(path) = cleanup_path {
        let _ = std::fs::remove_file(path);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolve_audio_file_path_prefers_recordings_for_plain_filename_when_present() {
        let recordings = tempdir().expect("recordings dir");
        let exe = tempdir().expect("exe dir");
        let filename = "x.wav";
        std::fs::write(recordings.path().join(filename), "audio").expect("seed file");

        let resolved = resolve_audio_file_path_with(filename, recordings.path(), exe.path());
        assert_eq!(resolved, recordings.path().join(filename));
    }

    #[test]
    fn resolve_audio_file_path_falls_back_to_exe_for_plain_filename_when_missing() {
        let recordings = tempdir().expect("recordings dir");
        let exe = tempdir().expect("exe dir");
        let filename = "missing.wav";

        let resolved = resolve_audio_file_path_with(filename, recordings.path(), exe.path());
        assert_eq!(resolved, exe.path().join(filename));
    }

    #[test]
    fn resolve_audio_file_path_keeps_relative_paths_with_separator() {
        let recordings = tempdir().expect("recordings dir");
        let exe = tempdir().expect("exe dir");
        let relative = "nested/file.wav";

        let resolved = resolve_audio_file_path_with(relative, recordings.path(), exe.path());
        assert_eq!(resolved, PathBuf::from(relative));
    }

    #[test]
    fn support_check_allows_audio_and_video_extensions() {
        assert!(is_supported_media_file("clip.wav"));
        assert!(is_supported_media_file("movie.mp4"));
        assert!(is_supported_media_file("movie.MOV"));
        assert!(!is_supported_media_file("notes.txt"));
    }

    #[test]
    fn is_video_file_detects_video_extensions() {
        assert!(is_video_file("movie.mp4"));
        assert!(is_video_file("movie.mkv"));
        assert!(!is_video_file("audio.m4a"));
        assert!(!is_video_file("audio.wav"));
    }

    #[test]
    fn extract_audio_from_real_test_material_videos() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_material");
        assert!(
            base.exists(),
            "Missing test_material directory at {}",
            base.display()
        );

        let samples = ["test_video.mp4", "test_video.mov", "test_video.mkv"];

        for file in samples {
            let input = base.join(file);
            assert!(input.exists(), "Missing test sample: {}", input.display());

            let extracted = extract_audio_from_video(&input)
                .unwrap_or_else(|e| panic!("Failed to extract from {}: {e}", input.display()));

            let metadata = std::fs::metadata(&extracted)
                .unwrap_or_else(|e| panic!("No extracted file {}: {e}", extracted.display()));
            assert!(
                metadata.len() > 44,
                "Extracted audio seems empty for {}",
                input.display()
            );

            let bytes = std::fs::read(&extracted)
                .unwrap_or_else(|e| panic!("Failed to read {}: {e}", extracted.display()));
            let has_ftyp = bytes.iter().take(20).copied().collect::<Vec<_>>();
            let is_m4a = has_ftyp.windows(4).any(|w| w == b"ftyp");
            assert!(is_m4a, "Output is not a valid M4A/MP4 file");

            let _ = std::fs::remove_file(extracted);
        }
    }

    #[test]
    fn test_chunk_audio_file_splitting() {
        let ffmpeg_path = find_ffmpeg().expect("ffmpeg not found");
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_material");
        let input_path = base.join("test_audio.mp3");
        assert!(input_path.exists());

        let temp_dir = tempdir().expect("temp dir");
        let chunks = chunk_audio_file(&ffmpeg_path, &input_path, temp_dir.path(), 1).expect("chunking failed");
        
        assert!(!chunks.is_empty(), "Should generate at least one chunk");
        for chunk in &chunks {
            assert!(chunk.exists());
            assert!(chunk.file_name().unwrap().to_str().unwrap().starts_with("chunk_"));
            assert!(chunk.file_name().unwrap().to_str().unwrap().ends_with(".m4a"));
        }
    }

    #[test]
    fn test_transcribe_audio_inner_under_limit() {
        let temp_dir = tempdir().expect("temp dir");
        let file_path = temp_dir.path().join("test_small.wav");
        std::fs::write(&file_path, b"dummy audio content").expect("write failed");

        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let result = transcribe_audio_inner(
            file_path.to_str().unwrap(),
            "dummy_api_key",
            move |key, path| {
                call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert_eq!(key, "dummy_api_key");
                assert!(path.exists());
                Ok("transcribed text".to_string())
            },
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "transcribed text");
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn test_transcribe_audio_inner_over_limit_chunking() {
        let ffmpeg_path = find_ffmpeg();
        if ffmpeg_path.is_none() {
            return;
        }

        let temp_dir = tempdir().expect("temp dir");
        let file_path = temp_dir.path().join("test_large.wav");
        
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        
        {
            let mut writer = hound::WavWriter::create(&file_path, spec).unwrap();
            let samples = vec![0i16; 10000];
            for _ in 0..1100 {
                for &sample in &samples {
                    writer.write_sample(sample).unwrap();
                }
            }
            writer.finalize().unwrap();
        }

        let file_size = std::fs::metadata(&file_path).unwrap().len();
        assert!(file_size >= 20 * 1024 * 1024, "File size must be >= 20MB, got {}", file_size);

        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let result = transcribe_audio_inner(
            file_path.to_str().unwrap(),
            "dummy_api_key",
            move |key, path| {
                let count = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert_eq!(key, "dummy_api_key");
                assert!(path.exists());
                Ok(format!("Chunk{}", count))
            },
        );

        assert!(result.is_ok());
        let final_text = result.unwrap();
        assert!(final_text.contains("Chunk0"));
        assert!(final_text.contains("Chunk1"));
        assert!(final_text.contains("Chunk2"));
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}
