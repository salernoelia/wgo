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
| `groq_request.rs` | `transcribe_audio()` — ffmpeg video→audio extraction, Groq Whisper API call via `reqwest` blocking |
| `config.rs` | `AppConfig` — JSON config load/save, platform config path, merge-with-defaults |
| `shortcut_detector.rs` | Global hotkey registration (`rdev` / `global-hotkey` on macOS), sends `HotkeyCommand` over mpsc |
| `transcription_history.rs` | Append-only JSON history file, `TranscriptionRecord` |
| `utils.rs` | Misc helpers |
| `icon.rs` | Loads app icon for viewport |

**Key data flow:**

1. Hotkey fires → `HotkeyCommand` over mpsc → `WgoApp::update` picks it up each frame
2. Stop recording → spawns thread → `transcribe_audio(path)` → sends `UiEvent::TranscriptionReady` back over `ui_event_tx`
3. `groq_request.rs` handles video files by running ffmpeg as a subprocess (not via library API) to extract audio to a temp `.m4a`, then POSTs to Groq

**macOS-specific:** GUI apps don't inherit shell `PATH`, so `find_ffmpeg()` in `groq_request.rs` probes hardcoded Homebrew paths (`/opt/homebrew/bin`, `/usr/local/bin`, etc.) in addition to `which`. The patched `rdev` vendored in `vendor/rdev/` avoids a macOS abort when `TISCopyCurrentKeyboardInputSource` is called off the main thread.

**Config location:**
- macOS/Linux: `~/.local/share/wgo/config.json`
- Windows: `%APPDATA%\wgo\config.json`
- Legacy fallback: exe directory or CWD `config.json` (auto-migrated on load)

**Supported media:** Audio: mp3 wav m4a ogg opus flac webm. Video (→ffmpeg extraction): mp4 mov mkv avi f4v flv ts wmv 3gp + more.
