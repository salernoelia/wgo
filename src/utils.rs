use arboard::Clipboard;
use std::process::Command;

pub fn copy_to_clipboard(text: &str) {
    let mut clipboard = match Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(e) => {
            eprintln!("Failed to access clipboard: {}", e);
            return;
        }
    };

    match clipboard.set_text(text) {
        Ok(_) => println!("Transcription copied to clipboard!"),
        Err(e) => eprintln!("Failed to copy to clipboard: {}", e),
    }
}

pub fn open_folder_in_file_explorer(path: &str) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer").arg(path).spawn()?;

    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(path).spawn()?;

    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(path).spawn()?;

    Ok(())
}

pub fn open_markdown_in_editor(path: &str) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(path)
        .spawn()?;

    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(path).spawn()?;

    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(path).spawn()?;

    Ok(())
}

/// Reveal a specific file in the system file manager (selects it).
pub fn reveal_file_in_finder(path: &str) -> Result<(), std::io::Error> {
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").args(["-R", path]).spawn()?;

    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .args(["/select,", path])
        .spawn()?;

    #[cfg(target_os = "linux")]
    {
        // Best-effort: open the parent folder
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::process::Command::new("xdg-open")
                .arg(parent)
                .spawn()?;
        }
    }

    Ok(())
}

pub fn open_microphone_permissions_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .status()
            .map_err(|e| format!("Failed to open System Settings: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err("Failed to open System Settings for microphone permissions".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let status = Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:privacy-microphone"])
            .status()
            .map_err(|e| format!("Failed to open microphone privacy settings: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err("Failed to open microphone privacy settings".to_string());
    }

    #[cfg(target_os = "linux")]
    {
        let status = Command::new("xdg-open")
            .arg("settings://privacy")
            .status()
            .map_err(|e| format!("Failed to open system privacy settings: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err("Failed to open system privacy settings".to_string());
    }
}
