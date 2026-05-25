# wgo

A lightweight, cross-platform voice transcription utility designed for seamless dictation and automated note-taking. Built with Rust and `egui`, `wgo` captures system or microphone audio, processes media file imports, and uses Groq’s Whisper API to deliver near-instantaneous transcription directly to your system clipboard and local workspace.

<table>
  <tr>
    <td><img src="images/demo_1.png" alt="Demo - Main View"></td>
    <td><img src="images/demo_2.png" alt="Demo - History"></td>
    <td><img src="images/demo_3.png" alt="Demo - Settings"></td>
  </tr>
</table>

---

## Capabilities

*   **Versatile Audio Routing**: Supports capturing from standard microphone hardware, direct desktop loopback audio, or both simultaneously (microphone + desktop mix). 
*   **Dictation & Hold-to-Record**: Dictate your thoughts naturally with custom hotkeys. In addition to a global toggle, the app supports physical **Hold-to-Record** keys (such as `ControlLeft` or `AltGr`). The recording runs only as long as you hold the key down and instantly transcribes upon release.
*   **Drag-and-Drop Media Processing**: Drop existing audio or video files directly into the window. For video formats, `wgo` automatically leverages local `ffmpeg` in a background subprocess to extract and compress the audio stream to a space-efficient AAC format before transcription.
*   **Automated Clipboard & Note Export**: Transcriptions are copied to your system clipboard the moment they are ready. Simultaneously, `wgo` generates clean Markdown files in a customizable output directory using custom tokenized patterns (e.g., `transcription_{date}_{time}.md`) complete with metadata YAML frontmatter.
*   **Local History & Playback**: Browse, copy, or open previous note files directly within the application. The append-only historical database retains file paths allowing you to play back original recorded source files or reveal them in your system's file manager.
*   **Hardware Native & Cross-Platform**: Package builds are available for macOS, Linux, and Windows, offering platform-native styling and exceptionally low memory footprint.

---

## Technical Architecture

The diagram below maps out how `wgo` isolates background worker threads and subprocesses from the primary GUI rendering thread, ensuring a smooth, non-blocking UI:

![WGO Architecture](diagrams/architecture.png)

---

## Installation

### Binary Releases
Packaged releases for your system (including a signed macOS `.app` bundle, a Windows portable executable `.exe`, and a Linux Debian `.deb` package) can be downloaded from the **Releases** tab.

### Installing with Cargo
If you have a local Rust toolchain configured, you can install the utility directly from GitHub:

```bash
cargo install --git https://github.com/salernoelia/wgo
```

To compile manually from source:

```bash
git clone https://github.com/salernoelia/wgo.git
cd wgo
cargo build --release
```
The compiled binary will be located in the `target/release/` directory.

### Prerequisites (For Video Transcription)
While voice dictation relies entirely on internal audio engines, drag-and-drop **video transcription** requires a system installation of `ffmpeg` to extract and process audio tracks.
*   **macOS**: `brew install ffmpeg`
*   **Linux**: `sudo apt install ffmpeg`
*   **Windows**: Download and add `ffmpeg` to your system `PATH`.

---

## Data Privacy & System Integration

### System Permissions
*   **macOS Accessibility**: On macOS, global keyboard listeners that hook outside the focused window require Accessibility permissions. To use **Hold-to-Record** or customized background hotkeys, you will be prompted to allow `wgo` in `System Settings → Privacy & Security → Accessibility`.
*   **Desktop Audio Capture**: Capturing desktop audio on Windows and Linux relies on your system’s active output device configuration. On macOS, system capture requires setting up a virtual loopback device (such as the open-source **BlackHole** driver) and selecting it as the capture source in the settings tab.

### Local Storage Structure
Your recordings, system config, and history remain strictly under your control. By default, `wgo` structures files in platform-native local paths:

| Platform | Configuration File (`config.json`) | Recordings Directory | Local Database (`history.json`) |
| :--- | :--- | :--- | :--- |
| **macOS** | `~/Library/Application Support/wgo/` | `~/Documents/wgo-recordings/` | `~/Library/Application Support/wgo/` |
| **Linux** | `~/.config/wgo/` | `~/Documents/wgo-recordings/` | `~/.local/share/wgo/` |
| **Windows** | `%LOCALAPPDATA%\wgo\` | `%USERPROFILE%\Documents\wgo-recordings\` | `%APPDATA%\wgo\` |

### External Transmission
Audio files are sent directly to the Groq API securely over HTTPS using your private key. The application contains zero telemetry or secondary tracking platforms, meaning no audio data or transcription text is ever seen by third parties other than the endpoint API.

---

## Usage

Start the app from your application launcher or your terminal:

```bash
wgo
```

On first run, navigate to the **Settings** tab to enter your Groq API key, which can be acquired directly from the [Groq Console](https://console.groq.com/).

### Default Global Controls

*   <kbd>Alt</kbd> + <kbd>Space</kbd> (or <kbd>Cmd</kbd> + <kbd>Space</kbd> on macOS): **Start / Stop Recording**
*   <kbd>Alt</kbd> + <kbd>H</kbd>: **Show / Hide Main Window**
*   <kbd>Control</kbd> (Hold): **Hold-to-Record** (The app records while held, then stops and transcribes upon release)

*Shortcuts and behavior (such as auto-minimizing on stop) can be fully remapped or disabled in the Settings tab.*

## License

This project is licensed under the [MIT License](LICENSE).