# wgo

A lightweight voice transcription tool that listens for global keyboard shortcuts to record audio and transcribe it using Groq's Whisper API.

## Features

- **Global hotkeys**: Alt+Space or Meta+Space to toggle recording, Alt+H to show menu
- **Device selection**: Choose your preferred microphone from the menu
- **Real-time transcription**: Audio transcribed using Groq's Whisper API
- **Automatic clipboard copying**: Transcriptions automatically copied to clipboard
- **History management**: View, re-transcribe, and copy previous recordings
- **Cross-platform**: Works on macOS, Linux, and Windows

## Installation

### Quick Install (Recommended)

```bash
cargo install --git https://github.com/salernoelia/wgo
```

### From Source

```bash
git clone https://github.com/salernoelia/wgo.git
cd wgo
cargo install --path .
```

## Configuration

On first run, you'll be prompted to enter your Groq API key. The config file will be created automatically at:
- **Linux/macOS**: `~/.config/wgo/config.json` or executable directory
- **Windows**: Next to the executable

Get your API key from [Groq Console](https://console.groq.com/).

## Usage

```bash
wgo
```

### Keyboard Shortcuts

- **Alt+Space** (or **Meta+Space**): Start/stop recording
- **Alt+H**: Open menu

### Menu Options

1. List transcription history
2. Re-transcribe recording
3. Copy transcription to clipboard
4. Clear history
5. **Select microphone device**
0. Return to listening mode

## Data Storage

Recordings and history are stored at:
- **Linux**: `~/.local/share/wgo/recordings/`
- **macOS**: `~/.local/share/wgo/recordings/`
- **Windows**: `%APPDATA%\wgo\recordings\` (planned)

## License

MIT
