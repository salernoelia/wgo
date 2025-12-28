# wgo

A lightweight voice transcription tool that listens for global keyboard shortcuts to record audio and transcribe it using Groq's Whisper API.

## Features

- **Global hotkeys**: Alt+Space to toggle recording, Alt+H to show menu
- **Real-time transcription**: Audio is transcribed using Groq's Whisper API
- **Automatic clipboard copying**: Transcriptions are automatically copied to clipboard
- **History management**: View, re-transcribe, and copy previous recordings
- **Cross-platform**: Works on macOS, Linux, and Windows


## Installation

### Download from GitHub Releases

Pre-built binaries for **macOS**, **Windows**, and **Linux** are available for each release on the [GitHub Releases](https://github.com/yourusername/wgo/releases) page. Download the binary for your platform and follow the instructions below to add it to your PATH.

### Prerequisites

- Rust (latest stable version)
- Groq API key

### Build from source

```bash
git clone https://github.com/yourusername/wgo.git
cd whisgo-rust
cargo build --release
```

### Add to PATH

#### macOS/Linux

```bash
# Add to your shell profile (.zshrc, .bashrc, etc.)
echo 'export PATH="/path/to/whisgo-rust/target/release:$PATH"' >> ~/.zshrc
source ~/.zshrc

# Or install globally
cargo install --path .
```

#### Windows

```cmd
# Add the target\release directory to your system PATH
# Or install globally
cargo install --path .
```

## Configuration

Create a `.env` file in your project directory or set environment variable:

```env
GROQ_API_KEY=your_groq_api_key_here
```

Get your API key from [Groq Console](https://console.groq.com/).

## Usage

Run the application:

```bash
wgo
```

### Keyboard shortcuts

- **Alt+Space**: Start/stop audio recording
- **Alt+H**: Open menu for history management

## File structure (in `~` (home) directory)

- `transcription_history.json` - Local storage for transcription history
- `recording_*.wav` - Temporary audio files (created during recording)

## Dependencies

- `cpal` - Cross-platform audio I/O
- `hound` - WAV file handling
- `rdev` - Global keyboard event detection
- `reqwest` - HTTP client for API requests
- `arboard` - Clipboard management
- `serde` - JSON serialization
