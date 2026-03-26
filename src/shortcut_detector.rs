#[cfg(target_os = "macos")]
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
#[cfg(target_os = "macos")]
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
#[cfg(not(target_os = "macos"))]
use rdev::grab;
#[cfg(not(target_os = "macos"))]
use rdev::{listen, Event, EventType, Key};
#[cfg(not(target_os = "macos"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
#[cfg(not(target_os = "macos"))]
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub enum HotkeyCommand {
    ToggleRecording,
    ShowWindow,
}

pub struct HotkeyRuntime {
    #[cfg(target_os = "macos")]
    _manager: Option<GlobalHotKeyManager>,
}

#[cfg(not(target_os = "macos"))]
pub struct ShortcutDetector {
    alt_pressed: AtomicBool,
    meta_pressed: AtomicBool,
}

#[cfg(not(target_os = "macos"))]
impl ShortcutDetector {
    pub fn new() -> Self {
        Self {
            alt_pressed: AtomicBool::new(false),
            meta_pressed: AtomicBool::new(false),
        }
    }

    fn modifiers_pressed(&self) -> bool {
        self.alt_pressed.load(Ordering::SeqCst) || self.meta_pressed.load(Ordering::SeqCst)
    }

    pub fn handle_event(&self, event: Event) -> Option<HotkeyCommand> {
        match event.event_type {
            EventType::KeyPress(Key::Alt) | EventType::KeyPress(Key::AltGr) => {
                self.alt_pressed.store(true, Ordering::SeqCst);
            }
            EventType::KeyPress(Key::MetaLeft) | EventType::KeyPress(Key::MetaRight) => {
                self.meta_pressed.store(true, Ordering::SeqCst);
            }
            EventType::KeyRelease(Key::Alt) | EventType::KeyRelease(Key::AltGr) => {
                self.alt_pressed.store(false, Ordering::SeqCst);
            }
            EventType::KeyRelease(Key::MetaLeft) | EventType::KeyRelease(Key::MetaRight) => {
                self.meta_pressed.store(false, Ordering::SeqCst);
            }
            EventType::KeyPress(Key::Space) if self.modifiers_pressed() => {
                return Some(HotkeyCommand::ToggleRecording);
            }
            EventType::KeyPress(Key::KeyH) if self.modifiers_pressed() => {
                return Some(HotkeyCommand::ShowWindow);
            }
            _ => {}
        }

        None
    }
}

pub fn start_global_hotkeys(sender: Sender<HotkeyCommand>) -> HotkeyRuntime {
    #[cfg(not(target_os = "macos"))]
    let detector = Arc::new(ShortcutDetector::new());

    #[cfg(target_os = "macos")]
    {
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(err) => {
                eprintln!("Failed to initialize macOS global hotkeys: {err}");
                return HotkeyRuntime { _manager: None };
            }
        };

        // macOS backend uses explicit global hotkeys to avoid HIToolbox queue assertions
        // seen when rdev keyboard translation runs outside the expected queue.
        let alt_space = HotKey::new(Some(Modifiers::ALT), Code::Space);
        let super_space = HotKey::new(Some(Modifiers::SUPER), Code::Space);
        let alt_h = HotKey::new(Some(Modifiers::ALT), Code::KeyH);
        let super_h = HotKey::new(Some(Modifiers::SUPER), Code::KeyH);

        let alt_space_id = alt_space.id();
        let super_space_id = super_space.id();
        let alt_h_id = alt_h.id();
        let super_h_id = super_h.id();

        if let Err(err) = manager.register(alt_space) {
            eprintln!("Failed to register Alt+Space: {err}");
        }
        if let Err(err) = manager.register(super_space) {
            eprintln!("Failed to register Meta+Space: {err}");
        }
        if let Err(err) = manager.register(alt_h) {
            eprintln!("Failed to register Alt+H: {err}");
        }
        if let Err(err) = manager.register(super_h) {
            eprintln!("Failed to register Meta+H: {err}");
        }

        std::thread::spawn(move || loop {
            match GlobalHotKeyEvent::receiver().recv() {
                Ok(event) => {
                    if event.state != HotKeyState::Pressed {
                        continue;
                    }

                    let cmd = if event.id == alt_space_id || event.id == super_space_id {
                        Some(HotkeyCommand::ToggleRecording)
                    } else if event.id == alt_h_id || event.id == super_h_id {
                        Some(HotkeyCommand::ShowWindow)
                    } else {
                        None
                    };

                    if let Some(cmd) = cmd {
                        let _ = sender.send(cmd);
                    }
                }
                Err(err) => {
                    eprintln!("Global shortcut listener channel failed: {err}");
                    break;
                }
            }
        });

        return HotkeyRuntime {
            _manager: Some(manager),
        };
    }

    #[cfg(not(target_os = "macos"))]
    {
        std::thread::spawn(move || {
            let detector_for_grab = detector.clone();
            let sender_for_grab = sender.clone();
            let grab_result = grab(move |event: Event| {
                if let Some(cmd) = detector_for_grab.handle_event(event.clone()) {
                    let _ = sender_for_grab.send(cmd);
                    return None;
                }
                Some(event)
            });

            if let Err(err) = grab_result {
                eprintln!(
                    "Global event grab unavailable, falling back to passive listener: {:?}",
                    err
                );

                let detector_for_listen = detector;
                let sender_for_listen = sender;
                if let Err(listen_err) = listen(move |event: Event| {
                    if let Some(cmd) = detector_for_listen.handle_event(event) {
                        let _ = sender_for_listen.send(cmd);
                    }
                }) {
                    eprintln!("Global shortcut listener failed: {:?}", listen_err);
                }
            }
        });

        HotkeyRuntime {}
    }
}
