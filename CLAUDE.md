# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build                  # debug build
cargo build --release        # release build
cargo run                    # run app
cargo test                   # run all tests
cargo test <test_name>       # run single test (e.g. cargo test round_trip_preserves_api_key)
cargo test -- --nocapture    # show println output in tests
```

ffmpeg must be installed (`brew install ffmpeg`) for video transcription tests to pass.

## Architecture

Single-binary egui desktop app. Entry point: `src/main.rs` — loads config, sets up hotkey channels, launches `eframe` with `WgoApp`.

**Module overview:**

| Module | Role |
|---|---|
| `app.rs` | `WgoApp` — egui update loop, UI tabs (Recorder/History/Settings), owns all state |
| `audio_recorder.rs` | `AudioRecorder` — wraps `cpal` streams for mic and desktop capture, writes WAV via `hound` |
| `transcriber.rs` | Unified transcription dispatcher — routes to `local_whisper` or `groq_request` based on `AppConfig::transcription_provider` |
| `local_whisper/` | Pure Rust local Whisper inference engine: |
| ↳ `audio.rs` | Prepares 16 kHz mono `f32` PCM samples from any input audio/video |
| ↳ `mlx.rs` | macOS Apple Silicon Metal GPU inference via `vona-mlx-whisper` and `mlx-rs` |
| ↳ `whisper_cpp.rs` | Cross-platform fallback inference via `whisper-rs` bindings to `whisper.cpp` |
| ↳ `downloader.rs` | Background native streaming downloader from Hugging Face with progress/speed metrics |
| `groq_request.rs` | `transcribe_audio()` — ffmpeg video→audio extraction, Groq Whisper API call via `reqwest` blocking |
| `config.rs` | `AppConfig` — JSON config load/save, provider settings (`Local` [default] vs `Groq`), models dir path, merge-with-defaults |
| `shortcut_detector.rs` | Global hotkey registration (`rdev` / `global-hotkey` on macOS), sends `HotkeyCommand` over mpsc |
| `transcription_history.rs` | Append-only JSON history file, `TranscriptionRecord` |
| `utils.rs` | Misc helpers |
| `icon.rs` | Loads app icon for viewport |

**Key data flow:**

1. Hotkey fires → `HotkeyCommand` over mpsc → `WgoApp::update` picks it up each frame
2. Stop recording → spawns background thread → calls `crate::transcriber::transcribe(path)` → sends `UiEvent::TranscriptionReady` back over `ui_event_tx`
3. `transcriber::transcribe` dispatches to the primary provider with automatic bidirectional fallback:
   - **Local primary**: `local_whisper::transcribe_local`; if it fails and Groq API key is present, seamlessly falls back to `groq_request::transcribe_audio`.
   - **Groq primary**: `groq_request::transcribe_audio`; if it fails and a local model is installed, seamlessly falls back to `local_whisper::transcribe_local`.
   - **Live status indicators**: Current backend and fallback status are shown dynamically in the window title, top-right header badge in the tab bar, and on each record in the history list.

**macOS-specific:** GUI apps don't inherit shell `PATH`, so `find_ffmpeg()` in `groq_request.rs` probes hardcoded Homebrew paths (`/opt/homebrew/bin`, `/usr/local/bin`, etc.) in addition to `which`. The patched `rdev` vendored in `vendor/rdev/` avoids a macOS abort when `TISCopyCurrentKeyboardInputSource` is called off the main thread.

**Config location:**
- macOS/Linux: `~/.local/share/wgo/config.json`
- Windows: `%APPDATA%\wgo\config.json`
- Legacy fallback: exe directory or CWD `config.json` (auto-migrated on load)

**Supported media:** Audio: mp3 wav m4a ogg opus flac webm. Video (→ffmpeg extraction): mp4 mov mkv avi f4v flv ts wmv 3gp + more.
