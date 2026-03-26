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
use std::sync::mpsc;
use std::sync::mpsc::Sender;
#[cfg(not(target_os = "macos"))]
use std::sync::Arc;
#[cfg(not(target_os = "macos"))]
use std::sync::RwLock;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub enum HotkeyCommand {
    ToggleRecording,
    ShowWindow,
}

#[derive(Debug, Clone)]
pub struct HotkeyBindings {
    pub toggle_shortcut: String,
    pub show_window_shortcut: String,
}

impl HotkeyBindings {
    pub fn new(toggle_shortcut: String, show_window_shortcut: String) -> Self {
        Self {
            toggle_shortcut,
            show_window_shortcut,
        }
    }
}

enum RuntimeControl {
    Rebind(HotkeyBindings),
}

pub struct HotkeyRuntime {
    control_tx: mpsc::Sender<RuntimeControl>,
    #[cfg(target_os = "macos")]
    _listener: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyRuntime {
    pub fn update_bindings(&self, bindings: HotkeyBindings) {
        let _ = self.control_tx.send(RuntimeControl::Rebind(bindings));
    }
}

#[cfg(not(target_os = "macos"))]
pub struct ShortcutDetector {
    alt_pressed: AtomicBool,
    meta_pressed: AtomicBool,
    ctrl_pressed: AtomicBool,
    shift_pressed: AtomicBool,
    bindings: RwLock<HotkeyBindings>,
}

#[cfg(not(target_os = "macos"))]
impl ShortcutDetector {
    pub fn new(bindings: HotkeyBindings) -> Self {
        Self {
            alt_pressed: AtomicBool::new(false),
            meta_pressed: AtomicBool::new(false),
            ctrl_pressed: AtomicBool::new(false),
            shift_pressed: AtomicBool::new(false),
            bindings: RwLock::new(bindings),
        }
    }

    fn current_modifiers(&self) -> ParsedModifiers {
        ParsedModifiers {
            alt: self.alt_pressed.load(Ordering::SeqCst),
            meta: self.meta_pressed.load(Ordering::SeqCst),
            ctrl: self.ctrl_pressed.load(Ordering::SeqCst),
            shift: self.shift_pressed.load(Ordering::SeqCst),
        }
    }

    pub fn update_bindings(&self, bindings: HotkeyBindings) {
        if let Ok(mut guard) = self.bindings.write() {
            *guard = bindings;
        }
    }

    pub fn handle_event(&self, event: Event) -> Option<HotkeyCommand> {
        match event.event_type {
            EventType::KeyPress(Key::Alt) | EventType::KeyPress(Key::AltGr) => {
                self.alt_pressed.store(true, Ordering::SeqCst);
            }
            EventType::KeyPress(Key::MetaLeft) | EventType::KeyPress(Key::MetaRight) => {
                self.meta_pressed.store(true, Ordering::SeqCst);
            }
            EventType::KeyPress(Key::ControlLeft) | EventType::KeyPress(Key::ControlRight) => {
                self.ctrl_pressed.store(true, Ordering::SeqCst);
            }
            EventType::KeyPress(Key::ShiftLeft) | EventType::KeyPress(Key::ShiftRight) => {
                self.shift_pressed.store(true, Ordering::SeqCst);
            }
            EventType::KeyRelease(Key::Alt) | EventType::KeyRelease(Key::AltGr) => {
                self.alt_pressed.store(false, Ordering::SeqCst);
            }
            EventType::KeyRelease(Key::MetaLeft) | EventType::KeyRelease(Key::MetaRight) => {
                self.meta_pressed.store(false, Ordering::SeqCst);
            }
            EventType::KeyRelease(Key::ControlLeft) | EventType::KeyRelease(Key::ControlRight) => {
                self.ctrl_pressed.store(false, Ordering::SeqCst);
            }
            EventType::KeyRelease(Key::ShiftLeft) | EventType::KeyRelease(Key::ShiftRight) => {
                self.shift_pressed.store(false, Ordering::SeqCst);
            }
            EventType::KeyPress(key) => {
                let bindings = match self.bindings.read() {
                    Ok(guard) => guard.clone(),
                    Err(_) => return None,
                };

                let mods = self.current_modifiers();
                if let Some(parsed) = parse_shortcut(&bindings.toggle_shortcut) {
                    if parsed.matches_rdev(mods, key) {
                        return Some(HotkeyCommand::ToggleRecording);
                    }
                }

                if let Some(parsed) = parse_shortcut(&bindings.show_window_shortcut) {
                    if parsed.matches_rdev(mods, key) {
                        return Some(HotkeyCommand::ShowWindow);
                    }
                }
            }
            _ => {}
        }

        None
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ParsedModifiers {
    alt: bool,
    meta: bool,
    ctrl: bool,
    shift: bool,
}

#[derive(Debug, Clone)]
struct ParsedShortcut {
    modifiers: ParsedModifiers,
    key_name: String,
}

impl ParsedShortcut {
    #[cfg(target_os = "macos")]
    fn to_hotkey(&self) -> Option<HotKey> {
        let code = code_from_name(&self.key_name)?;
        let mut modifiers = Modifiers::empty();
        if self.modifiers.alt {
            modifiers |= Modifiers::ALT;
        }
        if self.modifiers.meta {
            modifiers |= Modifiers::SUPER;
        }
        if self.modifiers.ctrl {
            modifiers |= Modifiers::CONTROL;
        }
        if self.modifiers.shift {
            modifiers |= Modifiers::SHIFT;
        }

        let mods = if modifiers.is_empty() {
            None
        } else {
            Some(modifiers)
        };
        Some(HotKey::new(mods, code))
    }

    #[cfg(not(target_os = "macos"))]
    fn matches_rdev(&self, active_mods: ParsedModifiers, key: Key) -> bool {
        let Some(expected_key) = key_from_name(&self.key_name) else {
            return false;
        };

        self.modifiers.alt == active_mods.alt
            && self.modifiers.meta == active_mods.meta
            && self.modifiers.ctrl == active_mods.ctrl
            && self.modifiers.shift == active_mods.shift
            && expected_key == key
    }
}

fn parse_shortcut(raw: &str) -> Option<ParsedShortcut> {
    let mut modifiers = ParsedModifiers::default();
    let mut key_name: Option<String> = None;

    for part in raw.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()) {
        let normalized = part.to_ascii_lowercase();
        match normalized.as_str() {
            "alt" | "option" => modifiers.alt = true,
            "meta" | "cmd" | "command" | "super" | "win" | "windows" => modifiers.meta = true,
            "ctrl" | "control" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            other => key_name = Some(other.to_string()),
        }
    }

    key_name.map(|key_name| ParsedShortcut {
        modifiers,
        key_name,
    })
}

#[cfg(target_os = "macos")]
fn code_from_name(name: &str) -> Option<Code> {
    match name {
        "space" => Some(Code::Space),
        "h" => Some(Code::KeyH),
        "a" => Some(Code::KeyA),
        "b" => Some(Code::KeyB),
        "c" => Some(Code::KeyC),
        "d" => Some(Code::KeyD),
        "e" => Some(Code::KeyE),
        "f" => Some(Code::KeyF),
        "g" => Some(Code::KeyG),
        "i" => Some(Code::KeyI),
        "j" => Some(Code::KeyJ),
        "k" => Some(Code::KeyK),
        "l" => Some(Code::KeyL),
        "m" => Some(Code::KeyM),
        "n" => Some(Code::KeyN),
        "o" => Some(Code::KeyO),
        "p" => Some(Code::KeyP),
        "q" => Some(Code::KeyQ),
        "r" => Some(Code::KeyR),
        "s" => Some(Code::KeyS),
        "t" => Some(Code::KeyT),
        "u" => Some(Code::KeyU),
        "v" => Some(Code::KeyV),
        "w" => Some(Code::KeyW),
        "x" => Some(Code::KeyX),
        "y" => Some(Code::KeyY),
        "z" => Some(Code::KeyZ),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn apply_bindings_macos(
    manager: &GlobalHotKeyManager,
    registered: &mut Vec<HotKey>,
    toggle_id: &mut Option<u32>,
    show_id: &mut Option<u32>,
    bindings: &HotkeyBindings,
) {
    for hotkey in registered.drain(..) {
        let _ = manager.unregister(hotkey);
    }

    *toggle_id = None;
    *show_id = None;

    if let Some(toggle) = parse_shortcut(&bindings.toggle_shortcut).and_then(|s| s.to_hotkey()) {
        *toggle_id = Some(toggle.id());
        if let Err(err) = manager.register(toggle.clone()) {
            eprintln!(
                "Failed to register toggle shortcut '{}': {err}",
                bindings.toggle_shortcut
            );
        } else {
            registered.push(toggle);
        }
    } else {
        eprintln!("Invalid toggle shortcut '{}'.", bindings.toggle_shortcut);
    }

    if let Some(show) = parse_shortcut(&bindings.show_window_shortcut).and_then(|s| s.to_hotkey()) {
        *show_id = Some(show.id());
        if let Err(err) = manager.register(show.clone()) {
            eprintln!(
                "Failed to register show-window shortcut '{}': {err}",
                bindings.show_window_shortcut
            );
        } else {
            registered.push(show);
        }
    } else {
        eprintln!(
            "Invalid show-window shortcut '{}'.",
            bindings.show_window_shortcut
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn key_from_name(name: &str) -> Option<Key> {
    match name {
        "space" => Some(Key::Space),
        "h" => Some(Key::KeyH),
        "a" => Some(Key::KeyA),
        "b" => Some(Key::KeyB),
        "c" => Some(Key::KeyC),
        "d" => Some(Key::KeyD),
        "e" => Some(Key::KeyE),
        "f" => Some(Key::KeyF),
        "g" => Some(Key::KeyG),
        "i" => Some(Key::KeyI),
        "j" => Some(Key::KeyJ),
        "k" => Some(Key::KeyK),
        "l" => Some(Key::KeyL),
        "m" => Some(Key::KeyM),
        "n" => Some(Key::KeyN),
        "o" => Some(Key::KeyO),
        "p" => Some(Key::KeyP),
        "q" => Some(Key::KeyQ),
        "r" => Some(Key::KeyR),
        "s" => Some(Key::KeyS),
        "t" => Some(Key::KeyT),
        "u" => Some(Key::KeyU),
        "v" => Some(Key::KeyV),
        "w" => Some(Key::KeyW),
        "x" => Some(Key::KeyX),
        "y" => Some(Key::KeyY),
        "z" => Some(Key::KeyZ),
        _ => None,
    }
}

pub fn start_global_hotkeys(
    sender: Sender<HotkeyCommand>,
    initial: HotkeyBindings,
) -> HotkeyRuntime {
    let (control_tx, control_rx) = mpsc::channel();

    #[cfg(not(target_os = "macos"))]
    let detector = Arc::new(ShortcutDetector::new(initial.clone()));

    #[cfg(target_os = "macos")]
    {
        let listener = std::thread::spawn(move || {
            let manager = match GlobalHotKeyManager::new() {
                Ok(manager) => manager,
                Err(err) => {
                    eprintln!("Failed to initialize macOS global hotkeys: {err}");
                    return;
                }
            };

            let mut registered: Vec<HotKey> = Vec::new();
            let mut toggle_id: Option<u32> = None;
            let mut show_id: Option<u32> = None;

            apply_bindings_macos(
                &manager,
                &mut registered,
                &mut toggle_id,
                &mut show_id,
                &initial,
            );

            let event_rx = GlobalHotKeyEvent::receiver();

            loop {
                while let Ok(msg) = control_rx.try_recv() {
                    match msg {
                        RuntimeControl::Rebind(bindings) => apply_bindings_macos(
                            &manager,
                            &mut registered,
                            &mut toggle_id,
                            &mut show_id,
                            &bindings,
                        ),
                    }
                }

                while let Ok(event) = event_rx.try_recv() {
                    if event.state != HotKeyState::Pressed {
                        continue;
                    }

                    let cmd = if toggle_id.is_some() && Some(event.id) == toggle_id {
                        Some(HotkeyCommand::ToggleRecording)
                    } else if show_id.is_some() && Some(event.id) == show_id {
                        Some(HotkeyCommand::ShowWindow)
                    } else {
                        None
                    };

                    if let Some(cmd) = cmd {
                        let _ = sender.send(cmd);
                    }
                }

                std::thread::sleep(Duration::from_millis(10));
            }
        });

        return HotkeyRuntime {
            control_tx,
            _listener: Some(listener),
        };
    }

    #[cfg(not(target_os = "macos"))]
    {
        let detector_clone = detector.clone();
        std::thread::spawn(move || {
            let detector_for_grab = detector_clone.clone();
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

                let detector_for_listen = detector_clone;
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

        std::thread::spawn(move || {
            while let Ok(msg) = control_rx.recv() {
                match msg {
                    RuntimeControl::Rebind(bindings) => detector.update_bindings(bindings),
                }
            }
        });

        HotkeyRuntime { control_tx }
    }
}
