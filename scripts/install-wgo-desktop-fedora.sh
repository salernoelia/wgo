#!/usr/bin/env bash

# Define paths
APP_DIR="$HOME/.local/share/applications"
DESKTOP_FILE="$APP_DIR/wgo.desktop"
BINARY_PATH="$HOME/.cargo/bin/wgo"

# 1. Ensure the desktop applications directory exists
mkdir -p "$APP_DIR"

# 2. Verify binary exists before creating entry
if [ ! -f "$BINARY_PATH" ]; then
    echo "Error: Binary not found at $BINARY_PATH"
    exit 1
fi

# 3. Generate the .desktop entry
cat << DESKTOP_CONTENT > "$DESKTOP_FILE"
[Desktop Entry]
Name=Wgo
Comment=Wgo Audio Application
Exec=$BINARY_PATH
Icon=audio-input-microphone
Terminal=false
Type=Application
Categories=Utility;Audio;
DESKTOP_CONTENT

# 4. Set appropriate permissions
chmod 644 "$DESKTOP_FILE"

# 5. Refresh the desktop database
update-desktop-database "$APP_DIR"

echo "Success: Wgo desktop entry created and indexed at $DESKTOP_FILE"
