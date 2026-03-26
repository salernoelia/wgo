
# wgo

A lightweight, cross-platform voice transcription tool with a GUI, global hotkey support, microphone selection, and real-time transcription using Groq's Whisper API.

![](/images/demo_1.png)
![](/images/demo_2.png)
![](/images/demo_3.png)


## Features

- **Modern GUI**: Clean, responsive interface for recording and managing transcriptions
- **Global hotkeys**: Alt+Space or Meta+Space to toggle recording, Alt+H to show/hide the window (customizable)
- **Device selection**: Choose your preferred microphone from the settings
- **Real-time transcription**: Audio is transcribed instantly using Groq's Whisper API
- **Automatic clipboard copying**: Transcriptions are automatically copied to your clipboard
- **History management**: View, re-transcribe, and copy previous recordings from the app
- **Markdown export**: Save transcriptions as Markdown files to a configurable directory
- **Cross-platform**: Works on macOS, Linux, and Windows


## Installation

Check out the releases to get a packed executable for your system.

### Quick Install Binary

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

On first run, you'll be prompted to enter your Groq API key. The config file is created automatically at:
- **Linux/macOS**: `~/.config/wgo/config.json` or executable directory
- **Windows**: Next to the executable

Get your API key from [Groq Console](https://console.groq.com/).


## Usage

```bash
wgo
```

### Keyboard Shortcuts (default)

- **Alt+Space** (or **Meta+Space**): Start/stop recording
- **Alt+H**: Show/hide window

Shortcuts can be changed in the app settings.

### Main Features in the App

- **Record audio**: Start/stop recording with a hotkey or from the GUI
- **Transcription history**: Browse, re-transcribe, and copy previous recordings
- **Device selection**: Change microphone from the settings tab
- **Markdown export**: Save transcriptions as Markdown files
- **Clipboard**: Transcriptions are copied automatically


## Data Storage

Recordings and history are stored at:
- **Linux/macOS**: `~/.local/share/wgo/recordings/`
- **Windows**: `%APPDATA%\wgo\recordings\`

## License

MIT
