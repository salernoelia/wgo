use rdev::{Event, EventType, Key};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ShortcutDetector {
    alt_pressed: AtomicBool,
    space_pressed: AtomicBool,
    h_pressed: AtomicBool,
}

#[derive(Debug, Clone, Copy)]
pub enum ShortcutType {
    ToggleRecording,
    ShowMenu,
}

impl ShortcutDetector {
    pub fn new() -> Self {
        Self {
            alt_pressed: AtomicBool::new(false),
            space_pressed: AtomicBool::new(false),
            h_pressed: AtomicBool::new(false),
        }
    }

    pub fn handle_event(&self, event: Event) -> Option<ShortcutType> {
        match event.event_type {
            EventType::KeyPress(Key::Alt) => {
                self.alt_pressed.store(true, Ordering::SeqCst);
            }
            EventType::KeyPress(Key::Space) => {
                self.space_pressed.store(true, Ordering::SeqCst);
            }
            EventType::KeyPress(Key::KeyH) => {
                self.h_pressed.store(true, Ordering::SeqCst);
            }
            EventType::KeyRelease(Key::Alt) => {
                self.alt_pressed.store(false, Ordering::SeqCst);
            }
            EventType::KeyRelease(Key::Space) => {
                self.space_pressed.store(false, Ordering::SeqCst);
            }
            EventType::KeyRelease(Key::KeyH) => {
                self.h_pressed.store(false, Ordering::SeqCst);
            }
            _ => {}
        }

        let alt = self.alt_pressed.load(Ordering::SeqCst);

        if alt && self.space_pressed.load(Ordering::SeqCst) {
            Some(ShortcutType::ToggleRecording)
        } else if alt && self.h_pressed.load(Ordering::SeqCst) {
            Some(ShortcutType::ShowMenu)
        } else {
            None
        }
    }
}
