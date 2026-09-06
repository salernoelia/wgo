pub mod audio;
pub mod downloader;
pub mod mlx;
pub mod whisper_cpp;

pub use self::downloader::{
    cancel_download, delete_model, get_download_progress, get_installed_model_path, is_downloading,
    is_model_installed, start_download, start_download_batch, uninstalled_model_kinds,
    LocalModelKind,
};
use crate::config::AppConfig;
use std::path::{Path, PathBuf};

pub fn models_root_dir() -> PathBuf {
    AppConfig::load()
        .ensure_models_dir()
        .unwrap_or_else(|_| AppConfig::app_data_dir().join("models"))
}

/// Checks if any local model is ready for transcription.
pub fn is_any_local_model_installed() -> bool {
    let root = models_root_dir();
    #[cfg(target_os = "macos")]
    {
        if is_model_installed(LocalModelKind::Mlx, &root) {
            return true;
        }
    }
    is_model_installed(LocalModelKind::WhisperCpp, &root)
}

#[allow(dead_code)]
pub fn is_default_local_model_installed() -> bool {
    is_any_local_model_installed()
}

/// Transcribes the given audio or video file locally using the preferred local engine.
pub fn transcribe_local(media_path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let root = models_root_dir();

    // On macOS, try native MLX if installed
    #[cfg(target_os = "macos")]
    {
        if is_model_installed(LocalModelKind::Mlx, &root) {
            if let Some(model_dir) = get_installed_model_path(LocalModelKind::Mlx, &root) {
                let pcm = audio::load_audio_samples_for_whisper(media_path)?;
                match mlx::transcribe_pcm(&model_dir, &pcm) {
                    Ok(text) if !text.trim().is_empty() => return Ok(text),
                    Ok(_) => {
                        eprintln!("MLX transcription produced empty output; attempting whisper.cpp fallback...");
                    }
                    Err(err) => {
                        eprintln!(
                            "MLX transcription failed ({err}); attempting whisper.cpp fallback..."
                        );
                    }
                }
            }
        }
    }

    // Standard local fallback / non-macOS default: whisper.cpp
    if is_model_installed(LocalModelKind::WhisperCpp, &root) {
        let model_path = get_installed_model_path(LocalModelKind::WhisperCpp, &root)
            .ok_or_else(|| "Whisper.cpp model file not found".to_string())?;
        let pcm = audio::load_audio_samples_for_whisper(media_path)?;
        return whisper_cpp::transcribe_pcm(&model_path, &pcm);
    }

    #[cfg(target_os = "macos")]
    {
        Err("No local Whisper model is downloaded. Please download either the MLX (Metal) or Whisper.cpp model in Settings.".into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("No local Whisper model is downloaded. Please download the Whisper.cpp model in Settings.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_downloader_model_specs() {
        let tmp = tempdir().unwrap();
        let (name, dir, files) =
            downloader::get_model_specs(LocalModelKind::WhisperCpp, tmp.path());
        assert!(name.contains("Whisper"));
        assert!(dir.starts_with(tmp.path()));
        assert!(!files.is_empty());
        assert!(files[0].url.contains("huggingface.co"));

        #[cfg(target_os = "macos")]
        {
            let (name_mlx, dir_mlx, files_mlx) =
                downloader::get_model_specs(LocalModelKind::Mlx, tmp.path());
            assert!(name_mlx.contains("MLX"));
            assert!(dir_mlx.starts_with(tmp.path()));
            assert_eq!(files_mlx.len(), 4);
        }
    }

    #[test]
    fn test_is_model_installed_checks_files() {
        let tmp = tempdir().unwrap();
        assert!(!is_model_installed(LocalModelKind::WhisperCpp, tmp.path()));

        let (_, dir, files) = downloader::get_model_specs(LocalModelKind::WhisperCpp, tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        for f in &files {
            std::fs::write(dir.join(&f.relative_path), b"mock model data").unwrap();
        }

        assert!(is_model_installed(LocalModelKind::WhisperCpp, tmp.path()));
        assert!(get_installed_model_path(LocalModelKind::WhisperCpp, tmp.path()).is_some());
    }

    #[test]
    fn test_uninstalled_model_kinds() {
        let tmp = tempdir().unwrap();
        let uninstalled = uninstalled_model_kinds(tmp.path());
        assert!(!uninstalled.is_empty());
        assert!(uninstalled.contains(&LocalModelKind::WhisperCpp));

        // Mock install WhisperCpp
        let (_, dir, files) = downloader::get_model_specs(LocalModelKind::WhisperCpp, tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        for f in &files {
            std::fs::write(dir.join(&f.relative_path), b"mock model data").unwrap();
        }

        let uninstalled_after = uninstalled_model_kinds(tmp.path());
        assert!(!uninstalled_after.contains(&LocalModelKind::WhisperCpp));
    }

    #[test]
    fn test_audio_resampling_from_real_test_material() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_material");
        let mp3_path = base.join("test_audio.mp3");
        if mp3_path.exists() {
            let samples = audio::load_audio_samples_for_whisper(&mp3_path).expect("load mp3");
            assert!(!samples.is_empty());
            for &s in samples.iter().take(100) {
                assert!(s >= -1.0 && s <= 1.0);
            }
        }

        let mp4_path = base.join("test_video.mp4");
        if mp4_path.exists() {
            let samples = audio::load_audio_samples_for_whisper(&mp4_path).expect("load mp4");
            assert!(!samples.is_empty());
            for &s in samples.iter().take(100) {
                assert!(s >= -1.0 && s <= 1.0);
            }
        }
    }

    #[test]
    fn test_phrase_deduplication() {
        #[cfg(target_os = "macos")]
        {
            let repeated = "test test hello hello world i am transcribing something locally locally hello hello world i am transcribing something locally locally hello hello world i am transcribing something locally locally";
            let deduped = mlx::deduplicate_repeated_phrases(repeated);
            assert_eq!(
                deduped,
                "test test hello hello world i am transcribing something locally locally"
            );
        }
    }

    #[test]
    fn test_real_recording_file() {
        let recording_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test_material")
            .join("recording_test.wav");
        let fallback_path =
            std::path::Path::new("/Users/elia/Documents/wgo-recordings/recording_1788696371.wav");

        let p = if recording_path.exists() {
            recording_path
        } else if fallback_path.exists() {
            fallback_path.to_path_buf()
        } else {
            return;
        };

        println!("Testing with {}", p.display());
        let result = crate::transcriber::transcribe(p.to_str().unwrap());
        match result {
            Ok(success) => {
                println!(
                    "SUCCESS: backend={:?}, text='{}', fallback_note={:?}",
                    success.backend_used, success.text, success.fallback_note
                );
                assert_eq!(success.backend_used, crate::transcriber::BackendUsed::Local);
                assert!(!success.text.is_empty());
                assert!(success.text.to_lowercase().contains("transcribing"));
            }
            Err(e) => {
                println!("FAILED: {e}");
                panic!("Transcription failed: {e}");
            }
        }
    }

    #[test]
    fn test_whisper_cpp_on_real_recording() {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test_material")
            .join("recording_test.wav");
        let root = models_root_dir();
        if p.exists() && is_model_installed(LocalModelKind::WhisperCpp, &root) {
            let model_path = get_installed_model_path(LocalModelKind::WhisperCpp, &root).unwrap();
            let pcm = audio::load_audio_samples_for_whisper(&p).expect("load audio");
            let text =
                whisper_cpp::transcribe_pcm(&model_path, &pcm).expect("whisper cpp transcribe");
            println!("WHISPER.CPP RESULT: '{text}'");
            assert!(text.to_lowercase().contains("transcribing"));
        }
    }

    #[test]
    fn test_mlx_on_real_recording() {
        #[cfg(target_os = "macos")]
        {
            let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("test_material")
                .join("recording_test.wav");
            let root = models_root_dir();
            if p.exists() && is_model_installed(LocalModelKind::Mlx, &root) {
                let model_dir = get_installed_model_path(LocalModelKind::Mlx, &root).unwrap();
                let pcm = audio::load_audio_samples_for_whisper(&p).expect("load audio");
                let text = mlx::transcribe_pcm(&model_dir, &pcm).expect("mlx transcribe");
                println!("MLX RESULT: '{text}'");
                assert!(text.to_lowercase().contains("transcribing"));
            }
        }
    }
}
