#[cfg(target_os = "macos")]
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
#[cfg(target_os = "macos")]
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
#[cfg(not(target_os = "macos"))]
use rdev::grab;
use rdev::{listen, Event, EventType, Key};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use std::sync::Arc;
#[cfg(not(target_os = "macos"))]
use std::sync::RwLock;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use std::sync::RwLock;

#[derive(Debug, Clone)]
pub enum HotkeyCommand {
    ToggleRecording,
    ShowWindow,
    StartHoldRecording,
    StopHoldRecording,
    HoldKeyCaptured(String),
    /// macOS only: rdev listener could not start — accessibility permission not granted.
    AccessibilityRequired,
}

/// Returns true if this process has been granted macOS Accessibility permission.
/// The hold-to-record feature uses CGEventTap which requires this permission.
#[cfg(target_os = "macos")]
pub fn is_accessibility_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

/// On non-macOS platforms this always returns true.
#[cfg(not(target_os = "macos"))]
pub fn is_accessibility_trusted() -> bool {
    true
}

/// Returns true if the given rdev key is a standard modifier key.
/// Used for the temporal-isolation check: modifier-only presses don't count
/// as "another key was pressed" when deciding whether to start hold recording.
fn is_modifier_key(key: Key) -> bool {
    matches!(
        key,
        Key::Alt
            | Key::AltGr
            | Key::MetaLeft
            | Key::MetaRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::ShiftLeft
            | Key::ShiftRight
            | Key::CapsLock
    )
}

#[derive(Debug, Clone)]
pub struct HotkeyBindings {
    pub toggle_shortcut: String,
    pub show_window_shortcut: String,
    pub hold_to_record_key: Option<String>,
}

impl HotkeyBindings {
    pub fn new(
        toggle_shortcut: String,
        show_window_shortcut: String,
        hold_to_record_key: Option<String>,
    ) -> Self {
        Self {
            toggle_shortcut,
            show_window_shortcut,
            hold_to_record_key,
        }
    }
}

enum RuntimeControl {
    Rebind(HotkeyBindings),
    StartCaptureHoldKey,
}

pub struct HotkeyRuntime {
    control_tx: mpsc::Sender<RuntimeControl>,
    #[cfg(target_os = "macos")]
    _listener: Option<std::thread::JoinHandle<()>>,
    #[cfg(target_os = "macos")]
    _hold_listener: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyRuntime {
    pub fn update_bindings(&self, bindings: HotkeyBindings) {
        let _ = self.control_tx.send(RuntimeControl::Rebind(bindings));
    }

    pub fn start_capture_hold_key(&self) {
        let _ = self.control_tx.send(RuntimeControl::StartCaptureHoldKey);
    }
}

#[cfg(not(target_os = "macos"))]
pub struct ShortcutDetector {
    alt_pressed: AtomicBool,
    meta_pressed: AtomicBool,
    ctrl_pressed: AtomicBool,
    shift_pressed: AtomicBool,
    bindings: RwLock<HotkeyBindings>,
    capture_mode: AtomicBool,
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
            capture_mode: AtomicBool::new(false),
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

    pub fn set_capture_mode(&self) {
        self.capture_mode.store(true, Ordering::SeqCst);
    }

    pub fn handle_event(&self, event: Event) -> Option<HotkeyCommand> {
        // Handle capture mode: intercept the next key press
        if let EventType::KeyPress(key) = event.event_type {
            if self.capture_mode.swap(false, Ordering::SeqCst) {
                return Some(HotkeyCommand::HoldKeyCaptured(format!("{:?}", key)));
            }
        }

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

                // Check hold-to-record key
                if let Some(ref hold_key_name) = bindings.hold_to_record_key {
                    if let Some(hold_key) = rdev_key_from_name(hold_key_name) {
                        if hold_key == key {
                            return Some(HotkeyCommand::StartHoldRecording);
                        }
                    }
                }

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
            EventType::KeyRelease(key) => {
                let bindings = match self.bindings.read() {
                    Ok(guard) => guard.clone(),
                    Err(_) => return None,
                };

                if let Some(ref hold_key_name) = bindings.hold_to_record_key {
                    if let Some(hold_key) = rdev_key_from_name(hold_key_name) {
                        if hold_key == key {
                            return Some(HotkeyCommand::StopHoldRecording);
                        }
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

/// Maps canonical rdev Debug key names to `Key` values.
/// Used for hold-to-record key matching on all platforms.
pub fn rdev_key_from_name(name: &str) -> Option<Key> {
    match name {
        "Alt" => Some(Key::Alt),
        "AltGr" => Some(Key::AltGr),
        "MetaLeft" => Some(Key::MetaLeft),
        "MetaRight" => Some(Key::MetaRight),
        "ControlLeft" => Some(Key::ControlLeft),
        "ControlRight" => Some(Key::ControlRight),
        "ShiftLeft" => Some(Key::ShiftLeft),
        "ShiftRight" => Some(Key::ShiftRight),
        "CapsLock" => Some(Key::CapsLock),
        "Tab" => Some(Key::Tab),
        "Space" => Some(Key::Space),
        "Return" => Some(Key::Return),
        "Escape" => Some(Key::Escape),
        "Backspace" => Some(Key::Backspace),
        "F1" => Some(Key::F1),
        "F2" => Some(Key::F2),
        "F3" => Some(Key::F3),
        "F4" => Some(Key::F4),
        "F5" => Some(Key::F5),
        "F6" => Some(Key::F6),
        "F7" => Some(Key::F7),
        "F8" => Some(Key::F8),
        "F9" => Some(Key::F9),
        "F10" => Some(Key::F10),
        "F11" => Some(Key::F11),
        "F12" => Some(Key::F12),
        "KeyA" => Some(Key::KeyA),
        "KeyB" => Some(Key::KeyB),
        "KeyC" => Some(Key::KeyC),
        "KeyD" => Some(Key::KeyD),
        "KeyE" => Some(Key::KeyE),
        "KeyF" => Some(Key::KeyF),
        "KeyG" => Some(Key::KeyG),
        "KeyH" => Some(Key::KeyH),
        "KeyI" => Some(Key::KeyI),
        "KeyJ" => Some(Key::KeyJ),
        "KeyK" => Some(Key::KeyK),
        "KeyL" => Some(Key::KeyL),
        "KeyM" => Some(Key::KeyM),
        "KeyN" => Some(Key::KeyN),
        "KeyO" => Some(Key::KeyO),
        "KeyP" => Some(Key::KeyP),
        "KeyQ" => Some(Key::KeyQ),
        "KeyR" => Some(Key::KeyR),
        "KeyS" => Some(Key::KeyS),
        "KeyT" => Some(Key::KeyT),
        "KeyU" => Some(Key::KeyU),
        "KeyV" => Some(Key::KeyV),
        "KeyW" => Some(Key::KeyW),
        "KeyX" => Some(Key::KeyX),
        "KeyY" => Some(Key::KeyY),
        "KeyZ" => Some(Key::KeyZ),
        "Num0" => Some(Key::Num0),
        "Num1" => Some(Key::Num1),
        "Num2" => Some(Key::Num2),
        "Num3" => Some(Key::Num3),
        "Num4" => Some(Key::Num4),
        "Num5" => Some(Key::Num5),
        "Num6" => Some(Key::Num6),
        "Num7" => Some(Key::Num7),
        "Num8" => Some(Key::Num8),
        "Num9" => Some(Key::Num9),
        _ => None,
    }
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
        // Shared state for hold key listener
        let hold_key_shared: Arc<RwLock<Option<Key>>> = Arc::new(RwLock::new(
            initial.hold_to_record_key.as_deref().and_then(rdev_key_from_name),
        ));
        let capture_mode: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

        // Temporal-isolation shared state (defined here so both threads can borrow).
        let hold_pressed_at: Arc<RwLock<Option<Instant>>> = Arc::new(RwLock::new(None));
        let last_other_key_at: Arc<RwLock<Option<Instant>>> = Arc::new(RwLock::new(None));
        let hold_active: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

        // Clones for the global_hotkey/timer thread (Thread 1).
        let hold_key_for_ctrl = hold_key_shared.clone();
        let capture_for_ctrl = capture_mode.clone();
        let hold_pressed_for_ctrl = hold_pressed_at.clone();
        let hold_active_for_ctrl = hold_active.clone();
        let sender_for_ctrl_hold = sender.clone();

        // Clones for the rdev hold-listener thread (Thread 2).
        let sender_for_hold = sender.clone();
        let sender_for_hold_err = sender.clone();
        let hold_key_for_listen = hold_key_shared.clone();
        let capture_for_listen = capture_mode.clone();
        let hold_pressed_for_listen = hold_pressed_at.clone();
        let last_other_for_listen = last_other_key_at.clone();
        let hold_active_for_listen = hold_active.clone();

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
                        RuntimeControl::Rebind(bindings) => {
                            // Update shared hold key
                            if let Ok(mut guard) = hold_key_for_ctrl.write() {
                                *guard = bindings.hold_to_record_key.as_deref().and_then(rdev_key_from_name);
                            }
                            apply_bindings_macos(
                                &manager,
                                &mut registered,
                                &mut toggle_id,
                                &mut show_id,
                                &bindings,
                            );
                        }
                        RuntimeControl::StartCaptureHoldKey => {
                            capture_for_ctrl.store(true, Ordering::SeqCst);
                        }
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

                // Check if the hold key has been held long enough to start recording.
                {
                    let pressed_at = hold_pressed_for_ctrl.read().ok().and_then(|g| *g);
                    if let Some(t) = pressed_at {
                        if t.elapsed() >= Duration::from_millis(HOLD_THRESHOLD_MS)
                            && !hold_active_for_ctrl.load(Ordering::SeqCst)
                        {
                            hold_active_for_ctrl.store(true, Ordering::SeqCst);
                            let _ = sender_for_ctrl_hold.send(HotkeyCommand::StartHoldRecording);
                        }
                    }
                }

                std::thread::sleep(Duration::from_millis(10));
            }
        });

        // Temporal-isolation state shared between the rdev callback and the timer check.
        // When the hold key is pressed we record the time and wait HOLD_THRESHOLD_MS before
        // firing StartHoldRecording.  Any non-modifier keypress during that window cancels.
        //
        // HOLD_THRESHOLD_MS: how long the key must be held before recording starts.
        //   Prevents accidental triggers when the key is tapped.
        // RECENT_KEY_WINDOW_MS: if another key was pressed within this window before the
        //   hold key, we assume it's part of a shortcut and ignore the hold.
        //   Set longer than HOLD_THRESHOLD_MS to handle OS-level dropped KeyRelease events.
        const HOLD_THRESHOLD_MS: u64 = 500;
        const RECENT_KEY_WINDOW_MS: u64 = 800;

        // Spawn separate rdev::listen thread for hold-to-record on macOS.
        // rdev requires Accessibility permission (CGEventTap).  We wait until either
        // a hold key is set or capture mode fires so we don't request the permission
        // on first launch before the user has configured anything.
        let hold_listener = std::thread::spawn(move || {
            // Wait until a hold key is actually configured before starting the listener.
            loop {
                {
                    let guard = hold_key_for_listen.read().ok();
                    if guard.as_deref().and_then(|k| *k).is_some() {
                        break;
                    }
                    if capture_for_listen.load(Ordering::SeqCst) {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(200));
            }

            let result = listen(move |event: Event| {
                match event.event_type {
                    EventType::KeyPress(key) => {
                        // Capture-mode: record whatever key the user presses next.
                        if capture_for_listen.swap(false, Ordering::SeqCst) {
                            let _ = sender_for_hold
                                .send(HotkeyCommand::HoldKeyCaptured(format!("{:?}", key)));
                            return;
                        }

                        let hold_key_opt = hold_key_for_listen
                            .read()
                            .ok()
                            .and_then(|g| *g);

                        if let Some(hold_key) = hold_key_opt {
                            if key == hold_key {
                                // Only arm the timer if no non-modifier key was pressed recently.
                                let recently_used = last_other_for_listen
                                    .read()
                                    .ok()
                                    .and_then(|t| *t)
                                    .map(|t| {
                                        t.elapsed()
                                            < Duration::from_millis(RECENT_KEY_WINDOW_MS)
                                    })
                                    .unwrap_or(false);

                                if !recently_used {
                                    if let Ok(mut guard) = hold_pressed_for_listen.write() {
                                        *guard = Some(Instant::now());
                                    }
                                }
                                return; // don't register the hold key itself as "other key"
                            }
                        }

                        // Any other key press: cancel a pending hold start and mark the time.
                        if !is_modifier_key(key) {
                            if let Ok(mut guard) = hold_pressed_for_listen.write() {
                                *guard = None;
                            }
                            hold_active_for_listen.store(false, Ordering::SeqCst);
                            if let Ok(mut guard) = last_other_for_listen.write() {
                                *guard = Some(Instant::now());
                            }
                        }
                    }
                    EventType::KeyRelease(key) => {
                        let hold_key_opt = hold_key_for_listen
                            .read()
                            .ok()
                            .and_then(|g| *g);

                        if let Some(hold_key) = hold_key_opt {
                            if key == hold_key {
                                // Cancel pending arm.
                                if let Ok(mut guard) = hold_pressed_for_listen.write() {
                                    *guard = None;
                                }
                                // Stop if recording was active.
                                if hold_active_for_listen.swap(false, Ordering::SeqCst) {
                                    let _ = sender_for_hold.send(HotkeyCommand::StopHoldRecording);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            });

            if let Err(err) = result {
                eprintln!("Hold key rdev listener failed: {:?}", err);
                let _ = sender_for_hold_err.send(HotkeyCommand::AccessibilityRequired);
            }
        });

        return HotkeyRuntime {
            control_tx,
            _listener: Some(listener),
            _hold_listener: Some(hold_listener),
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
                    RuntimeControl::StartCaptureHoldKey => detector.set_capture_mode(),
                }
            }
        });

        HotkeyRuntime { control_tx }
    }
}
