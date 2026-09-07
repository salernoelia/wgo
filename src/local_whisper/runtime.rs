//! One worker owns native model allocations and serializes loading/inference/unloading.
use super::{downloader, whisper_cpp, LocalModelKind};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::Duration,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Unloaded,
    Loading,
    Ready(LocalModelKind),
    Failed(String),
}
enum Command {
    Wake,
    Transcribe(Vec<f32>, mpsc::Sender<Result<String, String>>),
    Shutdown,
}
enum Model {
    Cpp(whisper_cpp::Model),
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    Mlx(super::mlx::Model),
}
impl Model {
    fn load(kind: LocalModelKind, path: &std::path::Path) -> Result<Self, String> {
        match kind {
            LocalModelKind::WhisperCpp => whisper_cpp::Model::load(path).map(Self::Cpp),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            LocalModelKind::Mlx => super::mlx::Model::load(path).map(Self::Mlx),
        }
        .map_err(|e| e.to_string())
    }
    fn transcribe(&self, pcm: &[f32]) -> Result<String, String> {
        match self {
            Self::Cpp(model) => model.transcribe(pcm),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Self::Mlx(model) => model.transcribe(pcm),
        }
        .map_err(|e| e.to_string())
    }
}
fn clear_cache() {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    super::mlx::clear_memory_cache();
}

pub struct Runtime {
    local: Arc<AtomicBool>,
    tx: mpsc::Sender<Command>,
    status: Arc<Mutex<Status>>,
}
impl Runtime {
    fn new(root: PathBuf) -> Self {
        let local = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Status::Unloaded));
        let (tx, rx) = mpsc::channel();
        let wanted = local.clone();
        let shared_status = status.clone();
        std::thread::spawn(move || {
            let set_status = |s| *shared_status.lock().unwrap() = s;
            let mut loaded: Option<Model> = None;
            let mut selected = None;
            loop {
                let desired = if wanted.load(Ordering::SeqCst) {
                    downloader::all_supported_model_kinds()
                        .into_iter()
                        .find_map(|kind| {
                            downloader::get_installed_model_path(kind, &root).map(|p| (kind, p))
                        })
                } else {
                    None
                };
                if desired != selected {
                    drop(loaded.take());
                    clear_cache();
                    selected = desired.clone();
                    if let Some((kind, path)) = desired {
                        set_status(Status::Loading);
                        match Model::load(kind, &path) {
                            Ok(model) if wanted.load(Ordering::SeqCst) => {
                                loaded = Some(model);
                                clear_cache();
                                set_status(Status::Ready(kind));
                            }
                            Ok(model) => {
                                drop(model);
                                clear_cache();
                                set_status(Status::Unloaded);
                            }
                            Err(error) => set_status(Status::Failed(error)),
                        }
                    } else {
                        set_status(Status::Unloaded);
                    }
                }
                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(Command::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => (),
                    Ok(Command::Transcribe(pcm, reply)) => {
                        let result = if !wanted.load(Ordering::SeqCst) {
                            Err("Local inference is disabled in Cloud mode".into())
                        } else if let Some(model) = loaded.as_ref() {
                            model.transcribe(&pcm)
                        } else {
                            Err(match &*shared_status.lock().unwrap() {
                                Status::Failed(error) => error.clone(),
                                _ => {
                                    "No local model is installed. Download one in Settings.".into()
                                }
                            })
                        };
                        // Release per-recording scratch buffers, retaining only model weights.
                        clear_cache();
                        let _ = reply.send(result);
                    }
                    Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            drop(loaded);
            clear_cache();
            set_status(Status::Unloaded);
        });
        Self { local, tx, status }
    }
    pub fn set_local(&self, enabled: bool) {
        if self.local.swap(enabled, Ordering::SeqCst) != enabled {
            let _ = self.tx.send(Command::Wake);
        }
    }
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
    pub fn transcribe(&self, pcm: Vec<f32>) -> Result<String, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Command::Transcribe(pcm, tx))
            .map_err(|_| "Local model worker stopped")?;
        rx.recv().map_err(|_| "Local model worker stopped")?
    }
    pub fn shutdown(&self) {
        self.local.store(false, Ordering::SeqCst);
        let _ = self.tx.send(Command::Shutdown);
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn shared() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new(super::models_root_dir()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait(runtime: &Runtime, expected: Status) {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while runtime.status() != expected {
            assert!(
                std::time::Instant::now() < deadline,
                "status: {:?}",
                runtime.status()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    #[test]
    fn cloud_does_not_load_or_infer() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::new(dir.path().into());
        assert_eq!(runtime.status(), Status::Unloaded);
        assert!(runtime
            .transcribe(vec![0.0; 160])
            .unwrap_err()
            .contains("Cloud mode"));
        runtime.set_local(true);
        assert!(runtime
            .transcribe(vec![0.0; 160])
            .unwrap_err()
            .contains("No local model"));
        runtime.set_local(false);
        wait(&runtime, Status::Unloaded);
    }
    #[test]
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "requires installed MLX Q4 model and recording"]
    fn resident_model_lifecycle() {
        let runtime = Runtime::new(super::super::models_root_dir());
        let pcm = super::super::audio::load_audio_samples_for_whisper(std::path::Path::new(
            "/Users/elia/Documents/wgo-recordings/recording_1788696371.wav",
        ))
        .unwrap();
        for round in 0..2 {
            runtime.set_local(true);
            wait(&runtime, Status::Ready(LocalModelKind::Mlx));
            println!("LOADED {round}: {:?}", super::super::mlx::memory_usage());
            for _ in 0..2 {
                let text = runtime.transcribe(pcm.clone()).unwrap();
                assert!(text.contains("transcribing something locally"));
            }
            println!(
                "AFTER INFERENCE {round}: {:?}",
                super::super::mlx::memory_usage()
            );
            runtime.set_local(false);
            wait(&runtime, Status::Unloaded);
            let (active, cache) = super::super::mlx::memory_usage();
            println!("UNLOADED {round}: active={active}, cache={cache}");
            assert!(active < 1024 * 1024, "model allocations retained");
            assert_eq!(cache, 0);
        }
        runtime.set_local(true);
        wait(&runtime, Status::Ready(LocalModelKind::Mlx));
        runtime.shutdown();
        wait(&runtime, Status::Unloaded);
    }
}
