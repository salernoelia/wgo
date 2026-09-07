pub mod audio;
pub mod downloader;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod mlx;
pub mod runtime;
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
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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
    let pcm = audio::load_audio_samples_for_whisper(media_path)?;
    runtime::shared().transcribe(pcm).map_err(Into::into)
}

/// Explicit engine selection for reproducible local verification; never falls back.
pub fn transcribe_cli(args: &[String]) -> Result<String, Box<dyn std::error::Error>> {
    if args.len() != 2 {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let usage = "Usage: wgo --transcribe-local <whisper-cpp|mlx> <audio-file>";
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let usage = "Usage: wgo --transcribe-local whisper-cpp <audio-file>";
        return Err(usage.into());
    }
    let root = models_root_dir();
    let kind = match args[0].as_str() {
        "whisper-cpp" => LocalModelKind::WhisperCpp,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        "mlx" => LocalModelKind::Mlx,
        _ => return Err("Unsupported local backend on this device".into()),
    };
    let model =
        get_installed_model_path(kind, &root).ok_or("Download this model in Settings first")?;
    let pcm = audio::load_audio_samples_for_whisper(Path::new(&args[1]))?;
    let started = std::time::Instant::now();
    let result = match kind {
        LocalModelKind::WhisperCpp => whisper_cpp::transcribe_pcm(&model, &pcm),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        LocalModelKind::Mlx => mlx::transcribe_pcm(&model, &pcm),
    };
    eprintln!(
        "Backend: {:?}; model: {}; elapsed: {:.2}s",
        kind,
        model.display(),
        started.elapsed().as_secs_f64()
    );
    result
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

        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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

    // Explicit integration tests: missing models/audio must fail, never silently pass.
    fn recording() -> PathBuf {
        std::env::var_os("WGO_TEST_AUDIO")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_material/recording_test.wav")
            })
    }

    #[test]
    #[ignore = "requires the downloaded whisper.cpp model and test recording"]
    fn test_whisper_cpp_on_real_recording() {
        let text =
            transcribe_cli(&["whisper-cpp".into(), recording().to_string_lossy().into()]).unwrap();
        println!("WHISPER.CPP RESULT: {text}");
        assert!(text.to_lowercase().contains("transcribing"));
    }

    #[test]
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "requires the downloaded MLX Q4 model and test recording"]
    fn test_mlx_on_real_recording() {
        let text = transcribe_cli(&["mlx".into(), recording().to_string_lossy().into()]).unwrap();
        println!("MLX RESULT: {text}");
        assert!(text.to_lowercase().contains("transcribing"));
    }

    #[test]
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn tiny_install_is_not_the_requested_mlx_model() {
        let tmp = tempdir().unwrap();
        let old = tmp.path().join("mlx/whisper-tiny");
        std::fs::create_dir_all(&old).unwrap();
        for name in [
            "config.json",
            "model.safetensors",
            "tokenizer.json",
            "generation_config.json",
        ] {
            std::fs::write(old.join(name), b"old tiny model").unwrap();
        }
        assert!(!is_model_installed(LocalModelKind::Mlx, tmp.path()));
        let (_, dir, files) = downloader::get_model_specs(LocalModelKind::Mlx, tmp.path());
        assert!(dir.ends_with("mlx/whisper-large-v3-turbo-q4"));
        assert!(files.iter().any(|f| f.url == "https://huggingface.co/mlx-community/whisper-large-v3-turbo-q4/resolve/main/weights.npz"));
    }

    #[test]
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "downloads 467 MB from Hugging Face and requires the test recording"]
    fn download_mlx_and_transcribe() {
        let tmp = tempdir().unwrap();
        start_download(LocalModelKind::Mlx, tmp.path().to_owned()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        while is_downloading() {
            assert!(
                std::time::Instant::now() < deadline,
                "model download timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let progress = get_download_progress().expect("download status");
        assert!(
            progress.error.is_none(),
            "download failed: {:?}",
            progress.error
        );
        assert!(progress.is_done);
        let dir = get_installed_model_path(LocalModelKind::Mlx, tmp.path())
            .expect("complete Q4 installation");
        let pcm = audio::load_audio_samples_for_whisper(&recording()).unwrap();
        let text = mlx::transcribe_pcm(&dir, &pcm).unwrap();
        println!("Downloaded MLX Q4 transcript: {text}");
        assert!(text
            .to_lowercase()
            .contains("transcribing something locally"));
    }

    #[test]
    fn supported_models_match_the_platform() {
        let kinds = downloader::all_supported_model_kinds();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(kinds, vec![LocalModelKind::Mlx, LocalModelKind::WhisperCpp]);
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        assert_eq!(kinds, vec![LocalModelKind::WhisperCpp]);
    }
}
