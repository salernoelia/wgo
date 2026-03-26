use arboard::Clipboard;

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
