use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub(super) struct Model(WhisperContext);

impl Model {
    pub(super) fn load(model_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if !model_path.exists() {
            return Err(format!("Whisper model file not found: {}", model_path.display()).into());
        }

        let model_str = model_path.to_str().ok_or("Invalid model path UTF-8")?;

        let ctx_params = WhisperContextParameters::default();
        let ctx = WhisperContext::new_with_params(model_str, ctx_params)
            .map_err(|e| format!("Failed to load whisper.cpp model at {model_str}: {e}"))?;

        Ok(Self(ctx))
    }

    pub(super) fn transcribe(
        &self,
        pcm_samples: &[f32],
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut state = self
            .0
            .create_state()
            .map_err(|e| format!("Failed to initialize whisper state: {e}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_translate(false);
        params.set_language(Some("auto"));

        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(8) as i32)
            .unwrap_or(4);
        params.set_n_threads(threads);

        state
            .full(params, pcm_samples)
            .map_err(|e| format!("whisper.cpp transcription execution failed: {e}"))?;

        let num_segments = state.full_n_segments();

        let mut text = String::new();
        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                if let Ok(seg_text) = segment.to_str() {
                    text.push_str(seg_text);
                }
            }
        }

        Ok(text.trim().to_string())
    }
}

/// One-shot entry point for explicit CLI verification.
pub fn transcribe_pcm(
    model_path: &Path,
    pcm_samples: &[f32],
) -> Result<String, Box<dyn std::error::Error>> {
    Model::load(model_path)?.transcribe(pcm_samples)
}
