use crate::audio_recorder::AudioRecorder;
use crate::groq_request::transcribe_audio;
use crate::transcription_history::{TranscriptionHistory, TranscriptionRecord};
use arboard::Clipboard;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

pub fn show_menu(recorder: &Arc<Mutex<AudioRecorder>>) {
    loop {
        println!("\n{:=<50}", "");
        println!("WHISGO MENU");
        println!("{:=<50}", "");
        println!("1. List transcription history");
        println!("2. Re-transcribe recording");
        println!("3. Copy transcription to clipboard");
        println!("4. Clear history");
        println!("5. Select microphone device");
        println!("0. Return to listening mode");
        println!("{:-<50}", "");
        print!("Choose an option: ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            continue;
        }

        match input.trim() {
            "1" => list_history(),
            "2" => resend_menu(),
            "3" => copy_menu(),
            "4" => clear_history(),
            "5" => select_microphone(recorder),
            "0" => break,
            _ => println!("Invalid option. Try again."),
        }
    }

    println!("Returning to listening mode...");
}

fn list_history() {
    let history = TranscriptionHistory::load();
    let records = history.list_records();

    if records.is_empty() {
        println!("No transcription history found.");
        return;
    }

    println!("\n{:-<80}", "");
    println!("TRANSCRIPTION HISTORY");
    println!("{:-<80}", "");

    for (i, record) in records.iter().enumerate() {
        let timestamp = format_timestamp(record.timestamp);
        println!("{}. {} [{}]", i + 1, record.filename, timestamp);
        println!("   {}", truncate_text(&record.transcription, 100));
        println!();
    }
}

fn resend_menu() {
    let history = TranscriptionHistory::load();
    let records = history.list_records();

    if records.is_empty() {
        println!("No recordings to re-transcribe.");
        return;
    }

    list_history();
    print!("Enter recording number to re-transcribe (0 to cancel): ");
    io::stdout().flush().unwrap();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return;
    }

    match input.trim().parse::<usize>() {
        Ok(0) => return,
        Ok(index) if index <= records.len() => {
            let record = &records[index - 1];
            println!("Re-transcribing: {}", record.filename);

            match transcribe_audio(&record.filename) {
                Ok(transcription) => {
                    println!("New transcription: {}", transcription);
                    copy_to_clipboard(&transcription);

                    let new_record = TranscriptionRecord {
                        filename: record.filename.clone(),
                        transcription,
                        timestamp: SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    };

                    let mut history = TranscriptionHistory::load();
                    history.add_record(new_record);
                }
                Err(e) => {
                    eprintln!("Transcription error: {}", e);
                }
            }
        }
        _ => println!("Invalid selection."),
    }
}

fn copy_menu() {
    let history = TranscriptionHistory::load();
    let records = history.list_records();

    if records.is_empty() {
        println!("No transcriptions to copy.");
        return;
    }

    list_history();
    print!("Enter transcription number to copy (0 to cancel): ");
    io::stdout().flush().unwrap();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return;
    }

    match input.trim().parse::<usize>() {
        Ok(0) => return,
        Ok(index) if index <= records.len() => {
            let record = &records[index - 1];
            copy_to_clipboard(&record.transcription);
            println!("Copied transcription from {} to clipboard", record.filename);
        }
        _ => println!("Invalid selection."),
    }
}

fn clear_history() {
    print!("Are you sure you want to clear all history? (y/N): ");
    io::stdout().flush().unwrap();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return;
    }

    if input.trim().to_lowercase() == "y" {
        let history = TranscriptionHistory::new();
        history.save();
        println!("History cleared.");
    }
}

fn copy_to_clipboard(text: &str) {
    match Clipboard::new() {
        Ok(mut clipboard) => match clipboard.set_text(text) {
            Ok(_) => println!("Transcription copied to clipboard!"),
            Err(e) => eprintln!("Failed to copy to clipboard: {}", e),
        },
        Err(e) => {
            eprintln!("Failed to access clipboard: {}", e);
        }
    }
}

fn format_timestamp(timestamp: u64) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    let datetime = UNIX_EPOCH + Duration::from_secs(timestamp);
    match datetime.elapsed() {
        Ok(elapsed) => {
            let secs = elapsed.as_secs();
            if secs < 60 {
                format!("{}s ago", secs)
            } else if secs < 3600 {
                format!("{}m ago", secs / 60)
            } else if secs < 86400 {
                format!("{}h ago", secs / 3600)
            } else {
                format!("{}d ago", secs / 86400)
            }
        }
        Err(_) => "unknown".to_string(),
    }
}

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}...", &text[..max_len])
    }
}

fn select_microphone(recorder: &Arc<Mutex<AudioRecorder>>) {
    let mut recorder = match recorder.lock() {
        Ok(r) => r,
        Err(_) => {
            eprintln!("Failed to lock recorder");
            return;
        }
    };

    match recorder.select_device() {
        Ok(_) => println!("\nDevice selection saved. It will be used for the next recording."),
        Err(e) => eprintln!("Error selecting device: {}", e),
    }
}
