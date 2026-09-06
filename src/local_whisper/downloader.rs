use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalModelKind {
    WhisperCpp,
    #[cfg(target_os = "macos")]
    Mlx,
}

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub model_name: String,
    pub model_kind: Option<LocalModelKind>,
    pub current_file: String,
    pub file_index: usize,
    pub total_files: usize,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percent: f32,
    pub speed_mbps: f64,
    pub is_done: bool,
    pub error: Option<String>,
}

static DOWNLOAD_STATUS: Mutex<Option<DownloadProgress>> = Mutex::new(None);
static CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);
static IS_DOWNLOADING: AtomicBool = AtomicBool::new(false);

pub fn is_downloading() -> bool {
    IS_DOWNLOADING.load(Ordering::SeqCst)
}

pub fn get_download_progress() -> Option<DownloadProgress> {
    DOWNLOAD_STATUS.lock().ok().and_then(|guard| guard.clone())
}

pub fn cancel_download() {
    CANCEL_REQUESTED.store(true, Ordering::SeqCst);
}

pub fn clear_download_status() {
    if let Ok(mut guard) = DOWNLOAD_STATUS.lock() {
        *guard = None;
    }
}

pub struct FileDownloadSpec {
    pub url: String,
    pub relative_path: PathBuf,
}

pub fn get_model_specs(
    kind: LocalModelKind,
    models_root: &Path,
) -> (String, PathBuf, Vec<FileDownloadSpec>) {
    match kind {
        LocalModelKind::WhisperCpp => {
            let name = "Whisper Large v3 Turbo (4-bit/5-bit quantized, whisper.cpp)".to_string();
            let dir = models_root.join("whisper-cpp");
            let files = vec![
                FileDownloadSpec {
                    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin".to_string(),
                    relative_path: PathBuf::from("ggml-large-v3-turbo-q5_0.bin"),
                },
            ];
            (name, dir, files)
        }
        #[cfg(target_os = "macos")]
        LocalModelKind::Mlx => {
            let name = "openai/whisper-tiny (MLX Metal)".to_string();
            let dir = models_root.join("mlx").join("whisper-tiny");
            let files = vec![
                FileDownloadSpec {
                    url: "https://huggingface.co/openai/whisper-tiny/resolve/main/config.json".to_string(),
                    relative_path: PathBuf::from("config.json"),
                },
                FileDownloadSpec {
                    url: "https://huggingface.co/openai/whisper-tiny/resolve/main/tokenizer.json".to_string(),
                    relative_path: PathBuf::from("tokenizer.json"),
                },
                FileDownloadSpec {
                    url: "https://huggingface.co/openai/whisper-tiny/resolve/main/generation_config.json".to_string(),
                    relative_path: PathBuf::from("generation_config.json"),
                },
                FileDownloadSpec {
                    url: "https://huggingface.co/openai/whisper-tiny/resolve/main/model.safetensors".to_string(),
                    relative_path: PathBuf::from("model.safetensors"),
                },
            ];
            (name, dir, files)
        }
    }
}

pub fn is_model_installed(kind: LocalModelKind, models_root: &Path) -> bool {
    let (_, dir, files) = get_model_specs(kind, models_root);
    if !dir.exists() {
        return false;
    }
    for spec in files {
        let p = dir.join(&spec.relative_path);
        if !p.exists() || fs::metadata(&p).map(|m| m.len()).unwrap_or(0) == 0 {
            return false;
        }
    }
    true
}

pub fn get_installed_model_path(kind: LocalModelKind, models_root: &Path) -> Option<PathBuf> {
    if is_model_installed(kind, models_root) {
        let (_, dir, files) = get_model_specs(kind, models_root);
        match kind {
            LocalModelKind::WhisperCpp => files.first().map(|f| dir.join(&f.relative_path)),
            #[cfg(target_os = "macos")]
            LocalModelKind::Mlx => Some(dir),
        }
    } else {
        None
    }
}

pub fn delete_model(kind: LocalModelKind, models_root: &Path) -> Result<(), std::io::Error> {
    let (_, dir, files) = get_model_specs(kind, models_root);
    match kind {
        LocalModelKind::WhisperCpp => {
            for f in files {
                let p = dir.join(&f.relative_path);
                if p.exists() {
                    let _ = fs::remove_file(p);
                }
            }
        }
        #[cfg(target_os = "macos")]
        LocalModelKind::Mlx => {
            if dir.exists() {
                fs::remove_dir_all(&dir)?;
            }
        }
    }
    Ok(())
}

pub fn all_supported_model_kinds() -> Vec<LocalModelKind> {
    vec![
        #[cfg(target_os = "macos")]
        LocalModelKind::Mlx,
        LocalModelKind::WhisperCpp,
    ]
}

pub fn uninstalled_model_kinds(models_root: &Path) -> Vec<LocalModelKind> {
    all_supported_model_kinds()
        .into_iter()
        .filter(|&k| !is_model_installed(k, models_root))
        .collect()
}

pub fn start_download(kind: LocalModelKind, models_root: PathBuf) -> Result<(), String> {
    start_download_batch(vec![kind], models_root)
}

pub fn start_download_batch(
    kinds: Vec<LocalModelKind>,
    models_root: PathBuf,
) -> Result<(), String> {
    let uninstalled: Vec<LocalModelKind> = kinds
        .into_iter()
        .filter(|&k| !is_model_installed(k, &models_root))
        .collect();

    if uninstalled.is_empty() {
        return Ok(());
    }

    if IS_DOWNLOADING.swap(true, Ordering::SeqCst) {
        return Err("A download is already in progress.".to_string());
    }

    CANCEL_REQUESTED.store(false, Ordering::SeqCst);

    std::thread::spawn(move || {
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                set_error(
                    None,
                    "Download Client",
                    "",
                    0,
                    0,
                    format!("Failed to build HTTP client: {e}"),
                );
                IS_DOWNLOADING.store(false, Ordering::SeqCst);
                return;
            }
        };

        let total_models = uninstalled.len();
        for (model_idx, &kind) in uninstalled.iter().enumerate() {
            if CANCEL_REQUESTED.load(Ordering::SeqCst) {
                break;
            }

            let (model_name, dir, files) = get_model_specs(kind, &models_root);
            if let Err(e) = fs::create_dir_all(&dir) {
                set_error(
                    Some(kind),
                    &model_name,
                    "",
                    0,
                    files.len(),
                    format!("Failed to create model directory {}: {e}", dir.display()),
                );
                IS_DOWNLOADING.store(false, Ordering::SeqCst);
                return;
            }

            let total_files = files.len();
            for (idx, spec) in files.iter().enumerate() {
                if CANCEL_REQUESTED.load(Ordering::SeqCst) {
                    break;
                }

                let target_path = dir.join(&spec.relative_path);
                if target_path.exists()
                    && fs::metadata(&target_path).map(|m| m.len()).unwrap_or(0) > 0
                {
                    continue;
                }

                if let Some(parent) = target_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let tmp_path = target_path.with_extension("tmp_download");
                let file_name = spec
                    .relative_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("file")
                    .to_string();

                let display_model_name = if total_models > 1 {
                    format!("{} ({}/{})", model_name, model_idx + 1, total_models)
                } else {
                    model_name.clone()
                };

                if let Ok(mut g) = DOWNLOAD_STATUS.lock() {
                    *g = Some(DownloadProgress {
                        model_name: display_model_name.clone(),
                        model_kind: Some(kind),
                        current_file: file_name.clone(),
                        file_index: idx + 1,
                        total_files,
                        downloaded_bytes: 0,
                        total_bytes: None,
                        percent: 0.0,
                        speed_mbps: 0.0,
                        is_done: false,
                        error: None,
                    });
                }

                let mut resp = match client.get(&spec.url).send() {
                    Ok(r) => {
                        if !r.status().is_success() {
                            let err = format!("HTTP error {} downloading {}", r.status(), spec.url);
                            set_error(
                                Some(kind),
                                &display_model_name,
                                &file_name,
                                idx + 1,
                                total_files,
                                err,
                            );
                            let _ = fs::remove_file(&tmp_path);
                            IS_DOWNLOADING.store(false, Ordering::SeqCst);
                            return;
                        }
                        r
                    }
                    Err(e) => {
                        let err = format!("Request failed for {}: {e}", spec.url);
                        set_error(
                            Some(kind),
                            &display_model_name,
                            &file_name,
                            idx + 1,
                            total_files,
                            err,
                        );
                        let _ = fs::remove_file(&tmp_path);
                        IS_DOWNLOADING.store(false, Ordering::SeqCst);
                        return;
                    }
                };

                let total_size = resp.content_length();
                let mut file = match File::create(&tmp_path) {
                    Ok(f) => f,
                    Err(e) => {
                        let err = format!("Failed to create file {}: {e}", tmp_path.display());
                        set_error(
                            Some(kind),
                            &display_model_name,
                            &file_name,
                            idx + 1,
                            total_files,
                            err,
                        );
                        IS_DOWNLOADING.store(false, Ordering::SeqCst);
                        return;
                    }
                };

                let mut downloaded: u64 = 0;
                let mut buffer = [0u8; 65536];
                let start_time = Instant::now();
                let mut last_update = Instant::now();

                loop {
                    if CANCEL_REQUESTED.load(Ordering::SeqCst) {
                        drop(file);
                        let _ = fs::remove_file(&tmp_path);
                        if let Ok(mut g) = DOWNLOAD_STATUS.lock() {
                            *g = None;
                        }
                        IS_DOWNLOADING.store(false, Ordering::SeqCst);
                        return;
                    }

                    let n = match resp.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) => {
                            drop(file);
                            let _ = fs::remove_file(&tmp_path);
                            let err = format!("Error reading stream for {}: {e}", file_name);
                            set_error(
                                Some(kind),
                                &display_model_name,
                                &file_name,
                                idx + 1,
                                total_files,
                                err,
                            );
                            IS_DOWNLOADING.store(false, Ordering::SeqCst);
                            return;
                        }
                    };

                    if let Err(e) = file.write_all(&buffer[..n]) {
                        drop(file);
                        let _ = fs::remove_file(&tmp_path);
                        let err = format!("Error writing {}: {e}", file_name);
                        set_error(
                            Some(kind),
                            &display_model_name,
                            &file_name,
                            idx + 1,
                            total_files,
                            err,
                        );
                        IS_DOWNLOADING.store(false, Ordering::SeqCst);
                        return;
                    }

                    downloaded += n as u64;

                    if last_update.elapsed().as_millis() >= 100 {
                        last_update = Instant::now();
                        let elapsed_sec = start_time.elapsed().as_secs_f64().max(0.001);
                        let speed_mbps = (downloaded as f64 / 1_000_000.0) / elapsed_sec;
                        let percent = match total_size {
                            Some(total) if total > 0 => (downloaded as f32 / total as f32) * 100.0,
                            _ => 0.0,
                        };

                        if let Ok(mut g) = DOWNLOAD_STATUS.lock() {
                            *g = Some(DownloadProgress {
                                model_name: display_model_name.clone(),
                                model_kind: Some(kind),
                                current_file: file_name.clone(),
                                file_index: idx + 1,
                                total_files,
                                downloaded_bytes: downloaded,
                                total_bytes: total_size,
                                percent,
                                speed_mbps,
                                is_done: false,
                                error: None,
                            });
                        }
                    }
                }

                drop(file);
                if let Err(e) = fs::rename(&tmp_path, &target_path) {
                    let _ = fs::remove_file(&tmp_path);
                    let err = format!("Failed to move {} into place: {e}", target_path.display());
                    set_error(
                        Some(kind),
                        &display_model_name,
                        &file_name,
                        idx + 1,
                        total_files,
                        err,
                    );
                    IS_DOWNLOADING.store(false, Ordering::SeqCst);
                    return;
                }
            }
        }

        if CANCEL_REQUESTED.load(Ordering::SeqCst) {
            if let Ok(mut g) = DOWNLOAD_STATUS.lock() {
                *g = None;
            }
        } else {
            // All models & files completed
            if let Ok(mut g) = DOWNLOAD_STATUS.lock() {
                *g = Some(DownloadProgress {
                    model_name: if total_models > 1 {
                        "All models".to_string()
                    } else {
                        "Model".to_string()
                    },
                    model_kind: None,
                    current_file: "Completed".to_string(),
                    file_index: 1,
                    total_files: 1,
                    downloaded_bytes: 0,
                    total_bytes: None,
                    percent: 100.0,
                    speed_mbps: 0.0,
                    is_done: true,
                    error: None,
                });
            }
        }
        IS_DOWNLOADING.store(false, Ordering::SeqCst);
    });

    Ok(())
}

fn set_error(
    kind: Option<LocalModelKind>,
    model: &str,
    file: &str,
    file_idx: usize,
    total_files: usize,
    err: String,
) {
    if let Ok(mut g) = DOWNLOAD_STATUS.lock() {
        *g = Some(DownloadProgress {
            model_name: model.to_string(),
            model_kind: kind,
            current_file: file.to_string(),
            file_index: file_idx,
            total_files,
            downloaded_bytes: 0,
            total_bytes: None,
            percent: 0.0,
            speed_mbps: 0.0,
            is_done: false,
            error: Some(err),
        });
    }
}
