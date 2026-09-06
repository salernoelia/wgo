use crate::config::{AppConfig, TranscriptionProvider};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendUsed {
    Local,
    Groq,
}

impl BackendUsed {
    pub fn label(&self) -> &'static str {
        match self {
            BackendUsed::Local => "Local",
            BackendUsed::Groq => "Groq",
        }
    }
}

pub struct TranscriptionSuccess {
    pub text: String,
    pub backend_used: BackendUsed,
    pub fallback_note: Option<String>,
}

pub fn resolve_audio_file_path(file_path: &str) -> PathBuf {
    crate::groq_request::resolve_audio_file_path(file_path)
}

/// Dispatches transcription to the primary provider with automatic fallback:
/// - If Local is primary: tries Local first; if that fails and Groq API key is set, falls back to Groq.
/// - If Groq is primary: tries Groq first; if that fails and a Local model is installed, falls back to Local.
pub fn transcribe(file_path: &str) -> Result<TranscriptionSuccess, Box<dyn std::error::Error>> {
    let config = AppConfig::load();
    let media_path = resolve_audio_file_path(file_path);
    let has_local = crate::local_whisper::is_any_local_model_installed();
    let has_groq = config.has_api_key();

    match config.transcription_provider {
        TranscriptionProvider::Local => {
            // Attempt 1: Local
            match crate::local_whisper::transcribe_local(&media_path) {
                Ok(text) => Ok(TranscriptionSuccess {
                    text,
                    backend_used: BackendUsed::Local,
                    fallback_note: None,
                }),
                Err(local_err) => {
                    eprintln!("Local transcription failed: {local_err}");
                    if has_groq {
                        eprintln!("Falling back to Groq Cloud API...");
                        match crate::groq_request::transcribe_audio(file_path) {
                            Ok(text) => Ok(TranscriptionSuccess {
                                text,
                                backend_used: BackendUsed::Groq,
                                fallback_note: Some(format!(
                                    "Local transcription failed ({local_err}); fell back to Groq Cloud API."
                                )),
                            }),
                            Err(groq_err) => Err(format!(
                                "Local transcription failed: {local_err} | Fallback Groq transcription also failed: {groq_err}"
                            ).into()),
                        }
                    } else {
                        Err(format!(
                            "Local transcription failed: {local_err}. (Groq fallback unavailable: no API key configured)"
                        ).into())
                    }
                }
            }
        }
        TranscriptionProvider::Groq => {
            // Attempt 1: Groq
            match crate::groq_request::transcribe_audio(file_path) {
                Ok(text) => Ok(TranscriptionSuccess {
                    text,
                    backend_used: BackendUsed::Groq,
                    fallback_note: None,
                }),
                Err(groq_err) => {
                    eprintln!("Groq transcription failed: {groq_err}");
                    if has_local {
                        eprintln!("Falling back to Local Whisper model...");
                        match crate::local_whisper::transcribe_local(&media_path) {
                            Ok(text) => Ok(TranscriptionSuccess {
                                text,
                                backend_used: BackendUsed::Local,
                                fallback_note: Some(format!(
                                    "Groq transcription failed ({groq_err}); fell back to Local Whisper model."
                                )),
                            }),
                            Err(local_err) => Err(format!(
                                "Groq transcription failed: {groq_err} | Fallback Local transcription also failed: {local_err}"
                            ).into()),
                        }
                    } else {
                        Err(format!(
                            "Groq transcription failed: {groq_err}. (Local fallback unavailable: no model installed)"
                        ).into())
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_used_labels() {
        assert_eq!(BackendUsed::Local.label(), "Local");
        assert_eq!(BackendUsed::Groq.label(), "Groq");
    }
}
