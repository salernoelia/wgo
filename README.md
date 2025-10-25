# wgo

A lightweight voice transcription tool that listens for global keyboard shortcuts to record audio and transcribe it using Groq's Whisper API.

## Features

- **Global hotkeys**: Alt+Space or Meta+Space to toggle recording, Alt+H to show menu
- **Real-time transcription**: Audio is transcribed using Groq's Whisper API
- **Automatic clipboard copying**: Transcriptions are automatically copied to clipboard
- **History management**: View, re-transcribe, and copy previous recordings
- **Cross-platform**: Works on macOS, Linux, and Windows

## Installation

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

Create a `config.json` file in the same directory as the executable:

```json
{
  "groq_api_key": "your_groq_api_key_here"
}
```

You can copy the example configuration file:

```bash
# For development (debug build)
cp config.json.example target/debug/config.json

# For release build
cp config.json.example target/release/config.json
```

Get your API key from [Groq Console](https://console.groq.com/).

## Usage

Run the application:

```bash
wgo
```

### Menu options

1. **List transcription history** - View all previous transcriptions
2. **Re-transcribe recording** - Re-process a previous recording
3. **Copy transcription to clipboard** - Copy any transcription to clipboard
4. **Clear history** - Remove all stored transcriptions
5. **Return to listening mode** - Continue listening for shortcuts

## File structure

- `transcription_history.json` - Local storage for transcription history
- `recording_*.wav` - Temporary audio files (created during recording)

## Dependencies

- `cpal` - Cross-platform audio I/O
- `hound` - WAV file handling
- `rdev` - Global keyboard event detection
- `reqwest` - HTTP client for API requests
- `arboard` - Clipboard management
- `serde` - JSON serialization
