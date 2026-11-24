//! Enregistrement d'un raccourci global au clavier, dans une fenêtre egui.
//!
//! Les touches d'egui sont converties en touches de `global-hotkey` (`KeyS`,
//! `Digit2`, `PrintScreen`…), le format de la config. Particularités du clavier
//! sous egui :
//! - ⌘C, ⌘X et ⌘V (Ctrl sous Windows) arrivent comme `Event::Copy`, `Cut` et
//!   `Paste`, pas comme des touches ;
//! - la touche Windows n'est pas dans `egui::Modifiers` : on la suit par ses appuis ;
//! - Windows n'envoie pas l'appui d'Impr. écran, seulement son relâchement ; ⌘V
//!   avec un presse-papiers vide ne produit pas non plus d'appui. Pour ces touches,
//!   un relâchement sans appui vaut un appui.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use egui::{Event, Key};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};

use crate::agent::hotkeys::{MODES, label};
use crate::config::Config;
use crate::ipc::Mode;

/// Modificateurs maintenus. `cmd` : ⌘ sous macOS, la touche Windows ailleurs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

impl Mods {
    /// `super_held` : touche Windows maintenue (absente de `egui::Modifiers`).
    pub fn from_egui(m: egui::Modifiers, super_held: bool) -> Self {
        Self {
            ctrl: m.ctrl,
            alt: m.alt,
            shift: m.shift,
            cmd: m.mac_cmd || super_held,
        }
    }

    pub fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.cmd
    }

    fn global(self) -> Modifiers {
        let mut m = Modifiers::empty();
        for (on, flag) in [
            (self.ctrl, Modifiers::CONTROL),
            (self.alt, Modifiers::ALT),
            (self.shift, Modifiers::SHIFT),
            (self.cmd, Modifiers::META),
        ] {
            if on {
                m |= flag;
            }
        }
        m
    }

    /// Libellé des modificateurs seuls (⇧⌘, Ctrl+Maj+), pendant la saisie.
    pub fn label(self) -> String {
        // Le libellé d'un raccourci complet, sans sa touche (F1 : jamais traduite
        // selon la disposition du clavier, contrairement aux lettres).
        let full = label(&HotKey::new(Some(self.global()), Code::F1));
        full.strip_suffix("F1").unwrap_or(&full).to_owned()
    }
}

/// Touche egui → touche de `global-hotkey`. `None` : touche qu'un raccourci global
/// ne peut pas utiliser (modificateur, touche multimédia…).
pub fn code_of(key: Key) -> Option<Code> {
    use Code as C;
    Some(match key {
        Key::A => C::KeyA,
        Key::B => C::KeyB,
        Key::C => C::KeyC,
        Key::D => C::KeyD,
        Key::E => C::KeyE,
        Key::F => C::KeyF,
        Key::G => C::KeyG,
        Key::H => C::KeyH,
        Key::I => C::KeyI,
        Key::J => C::KeyJ,
        Key::K => C::KeyK,
        Key::L => C::KeyL,
        Key::M => C::KeyM,
        Key::N => C::KeyN,
        Key::O => C::KeyO,
        Key::P => C::KeyP,
        Key::Q => C::KeyQ,
        Key::R => C::KeyR,
        Key::S => C::KeyS,
        Key::T => C::KeyT,
        Key::U => C::KeyU,
        Key::V => C::KeyV,
        Key::W => C::KeyW,
        Key::X => C::KeyX,
        Key::Y => C::KeyY,
        Key::Z => C::KeyZ,
        Key::Num0 => C::Digit0,
        Key::Num1 => C::Digit1,
        Key::Num2 => C::Digit2,
        Key::Num3 => C::Digit3,
        Key::Num4 => C::Digit4,
        Key::Num5 => C::Digit5,
        Key::Num6 => C::Digit6,
        Key::Num7 => C::Digit7,
        Key::Num8 => C::Digit8,
        Key::Num9 => C::Digit9,
        Key::F1 => C::F1,
        Key::F2 => C::F2,
        Key::F3 => C::F3,
        Key::F4 => C::F4,
        Key::F5 => C::F5,
        Key::F6 => C::F6,
        Key::F7 => C::F7,
        Key::F8 => C::F8,
        Key::F9 => C::F9,
        Key::F10 => C::F10,
        Key::F11 => C::F11,
        Key::F12 => C::F12,
        Key::F13 => C::F13,
        Key::F14 => C::F14,
        Key::F15 => C::F15,
        Key::F16 => C::F16,
        Key::F17 => C::F17,
        Key::F18 => C::F18,
        Key::F19 => C::F19,
        Key::F20 => C::F20,
        Key::F21 => C::F21,
        Key::F22 => C::F22,
        Key::F23 => C::F23,
        Key::F24 => C::F24,
        Key::Space => C::Space,
        Key::Enter => C::Enter,
        Key::Tab => C::Tab,
        Key::Backspace => C::Backspace,
        Key::Delete => C::Delete,
        Key::Insert => C::Insert,
        Key::Escape => C::Escape,
        Key::Home => C::Home,
        Key::End => C::End,
        Key::PageUp => C::PageUp,
        Key::PageDown => C::PageDown,
        Key::ArrowLeft => C::ArrowLeft,
        Key::ArrowRight => C::ArrowRight,
        Key::ArrowUp => C::ArrowUp,
        Key::ArrowDown => C::ArrowDown,
        Key::Minus => C::Minus,
        Key::Equals => C::Equal,
        Key::OpenBracket => C::BracketLeft,
        Key::CloseBracket => C::BracketRight,
        Key::Backslash => C::Backslash,
        Key::IntlBackslash => C::IntlBackslash,
        Key::Semicolon => C::Semicolon,
        Key::Quote => C::Quote,
        Key::Backtick => C::Backquote,
        Key::Comma => C::Comma,
        Key::Period => C::Period,
        Key::Slash => C::Slash,
        Key::PrintScreen => C::PrintScreen,
        Key::Pause => C::Pause,
        Key::ScrollLock => C::ScrollLock,
        _ => return None,
    })
}

/// Raccourci au format de la config : `"control+alt+shift+super+KeyS"`.
pub fn config_text(mods: Mods, code: Code) -> String {
    let mut out = String::new();
    for (on, name) in [
        (mods.ctrl, "control+"),
        (mods.alt, "alt+"),
        (mods.shift, "shift+"),
        (mods.cmd, "super+"),
    ] {
        if on {
            out.push_str(name);
        }
    }
    out.push_str(&code.to_string());
    out
}

/// Touches utilisables seules (ou avec ⇧ seul) : elles ne tapent pas de texte.
fn standalone(code: Code) -> bool {
    use Code as C;
    matches!(
        code,
        C::F1
            | C::F2
            | C::F3
            | C::F4
            | C::F5
            | C::F6
            | C::F7
            | C::F8
            | C::F9
            | C::F10
            | C::F11
            | C::F12
            | C::F13
            | C::F14
            | C::F15
            | C::F16
            | C::F17
            | C::F18
            | C::F19
            | C::F20
            | C::F21
            | C::F22
            | C::F23
            | C::F24
            | C::PrintScreen
    )
}

/// Raccourci refusé par les réglages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invalid {
    /// Sans modificateur, la touche ne taperait plus rien ailleurs.
    NoModifier,
    /// ⇧ seul : la touche ne taperait plus sa majuscule (ou sa sélection).
    ShiftOnly,
    /// Déjà pris par une autre capture.
    Duplicate(Mode),
    /// Illisible pour `global-hotkey`.
    Unreadable,
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let modifiers = if cfg!(target_os = "macos") {
            "⌘, ⌥ ou ⌃"
        } else {
            "Ctrl, Alt ou Win"
        };
        match self {
            Self::NoModifier => write!(
                f,
                "Ajoute un modificateur ({modifiers}) : seules les touches F1 à F24 et \
                 Impr. écran s'utilisent seules."
            ),
            Self::ShiftOnly => write!(
                f,
                "Maj seule ne suffit pas : ajoute {modifiers}, sinon la touche ne taperait \
                 plus sa majuscule."
            ),
            Self::Duplicate(mode) => write!(f, "Déjà utilisé par « {} ».", mode_name(*mode)),
            Self::Unreadable => write!(f, "Raccourci illisible."),
        }
    }
}

/// Nom d'une capture dans les réglages.
pub fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Region => "Capturer une zone",
        Mode::Screen => "Capturer l'écran",
        Mode::Previous => "Capturer la zone précédente",
        Mode::AllScreens => "Capturer tous les écrans",
        Mode::Window => "Capturer la fenêtre active",
        Mode::Delayed => "Capture différée",
        Mode::Scrolling => "Capture avec défilement",
        Mode::Ocr => "Copier le texte (OCR)",
        Mode::Qr => "Décoder un QR code",
        Mode::ColorPicker => "Pipette de couleur",
        Mode::Video => "Enregistrer une vidéo",
        Mode::VideoRegion => "Enregistrer une zone en vidéo",
        Mode::Workspace => "Espace de travail",
    }
}

/// Vérifie `text` comme raccourci de `mode`, face aux autres raccourcis de `config`.
pub fn validate(text: &str, mode: Mode, config: &Config) -> Result<(), Invalid> {
    let hotkey = HotKey::from_str(text).map_err(|_| Invalid::Unreadable)?;
    if !standalone(hotkey.key) {
        if hotkey.mods.is_empty() {
            return Err(Invalid::NoModifier);
        }
        if hotkey.mods == Modifiers::SHIFT {
            return Err(Invalid::ShiftOnly);
        }
    }
    for other in MODES {
        if other != mode && HotKey::from_str(config.hotkey(other)).ok() == Some(hotkey) {
            return Err(Invalid::Duplicate(other));
        }
    }
    Ok(())
}

/// Résultat d'un enregistrement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    /// Combinaison tapée (pas encore validée), au format de la config.
    Combo(String),
    /// Échap sans modificateur.
    Cancel,
    /// Touche sans équivalent pour un raccourci global.
    Unsupported,
}

/// Lit le clavier pendant l'enregistrement d'un raccourci.
#[derive(Debug, Default)]
pub struct Recorder {
    /// Touche Windows maintenue.
    super_held: bool,
    /// Touches vues enfoncées depuis le début de l'enregistrement.
    pressed: HashSet<Key>,
}

impl Recorder {
    /// Modificateurs maintenus en ce moment, pour l'affichage.
    pub fn live(&self, m: egui::Modifiers) -> Mods {
        Mods::from_egui(m, self.super_held)
    }

    /// Oublie la touche Windows quand la fenêtre perd le clavier (son relâchement
    /// arrivera ailleurs).
    pub fn focus_lost(&mut self) {
        self.super_held = false;
    }

    /// Lit les événements d'une frame ; `now` : modificateurs maintenus (pour
    /// `Copy`, `Cut` et `Paste`, qui n'en portent pas).
    pub fn read(&mut self, events: &[Event], now: egui::Modifiers) -> Option<Recorded> {
        for event in events {
            match event {
                Event::Key {
                    key,
                    physical_key,
                    pressed,
                    repeat,
                    modifiers,
                } => {
                    // macOS enregistre la position de la touche (clavier US) ;
                    // Windows, la touche telle qu'elle est gravée.
                    let key = if cfg!(target_os = "macos") {
                        physical_key.unwrap_or(*key)
                    } else {
                        *key
                    };
                    if matches!(key, Key::SuperLeft | Key::SuperRight) {
                        self.super_held = *pressed;
                        continue;
                    }
                    if is_modifier(key) || *repeat {
                        continue;
                    }
                    let mods = Mods::from_egui(*modifiers, self.super_held);
                    if *pressed {
                        self.pressed.insert(key);
                    } else if self.pressed.contains(&key) || !release_counts(key, mods) {
                        continue;
                    }
                    if key == Key::Escape && !mods.any() {
                        return Some(Recorded::Cancel);
                    }
                    return Some(match code_of(key) {
                        Some(code) => Recorded::Combo(config_text(mods, code)),
                        None => Recorded::Unsupported,
                    });
                }
                Event::Copy | Event::Cut | Event::Paste(_) | Event::PasteImage(_) => {
                    let key = match event {
                        Event::Copy => Key::C,
                        Event::Cut => Key::X,
                        _ => Key::V,
                    };
                    self.pressed.insert(key);
                    let mut mods = Mods::from_egui(now, self.super_held);
                    if cfg!(target_os = "macos") {
                        mods.cmd = true;
                    } else {
                        mods.ctrl = true;
                    }
                    let code = code_of(key).expect("lettre");
                    return Some(Recorded::Combo(config_text(mods, code)));
                }
                _ => {}
            }
        }
        None
    }
}

fn is_modifier(key: Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
    )
}

/// Touches dont le système peut ne transmettre que le relâchement (voir le haut du
/// fichier).
fn release_counts(key: Key, mods: Mods) -> bool {
    let command = if cfg!(target_os = "macos") {
        mods.cmd
    } else {
        mods.ctrl
    };
    key == Key::PrintScreen || (command && matches!(key, Key::C | Key::X | Key::V))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, pressed: bool, modifiers: egui::Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        }
    }

    /// ⌘ sous macOS, Ctrl ailleurs, tels qu'egui-winit les remplit.
    fn command() -> egui::Modifiers {
        if cfg!(target_os = "macos") {
            egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND
        } else {
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND
        }
    }

    fn command_text() -> &'static str {
        if cfg!(target_os = "macos") {
            "super+"
        } else {
            "control+"
        }
    }

    #[test]
    fn every_mapped_key_is_readable_by_global_hotkey() {
        let mut seen = HashSet::new();
        for &k in Key::ALL {
            let Some(code) = code_of(k) else { continue };
            assert!(seen.insert(code), "{code:?} deux fois");
            let text = config_text(
                Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                code,
            );
            let hotkey = HotKey::from_str(&text).unwrap_or_else(|e| panic!("{text} : {e}"));
            assert_eq!(hotkey.key, code, "{text}");
            assert_eq!(hotkey.mods, Modifiers::CONTROL, "{text}");
        }
        assert_eq!(seen.len(), 90);
    }

    #[test]
    fn egui_keys_become_config_text() {
        assert_eq!(code_of(Key::Num2), Some(Code::Digit2));
        assert_eq!(code_of(Key::S), Some(Code::KeyS));
        assert_eq!(code_of(Key::PrintScreen), Some(Code::PrintScreen));
        assert_eq!(code_of(Key::Equals), Some(Code::Equal));
        assert_eq!(code_of(Key::ShiftLeft), None);
        let all = Mods {
            ctrl: true,
            alt: true,
            shift: true,
            cmd: true,
        };
        assert_eq!(
            config_text(all, Code::Digit2),
            "control+alt+shift+super+Digit2"
        );
        assert_eq!(config_text(Mods::default(), Code::F13), "F13");
        // Même raccourci que la valeur par défaut, écrite autrement.
        let text = config_text(
            Mods {
                shift: true,
                cmd: true,
                ..Mods::default()
            },
            Code::Digit2,
        );
        assert_eq!(
            HotKey::from_str(&text).unwrap(),
            HotKey::from_str("super+shift+Digit2").unwrap()
        );
    }

    #[test]
    fn modifiers_are_required_except_for_function_keys() {
        let config = Config {
            region_hotkey: String::new(),
            screen_hotkey: String::new(),
            previous_hotkey: String::new(),
            ..Config::default()
        };
        let check = |text: &str| validate(text, Mode::Region, &config);
        assert_eq!(check("KeyA"), Err(Invalid::NoModifier));
        assert_eq!(check("Space"), Err(Invalid::NoModifier));
        assert_eq!(check("shift+KeyA"), Err(Invalid::ShiftOnly));
        assert_eq!(check("pas un raccourci"), Err(Invalid::Unreadable));
        assert_eq!(check("F5"), Ok(()));
        assert_eq!(check("F24"), Ok(()));
        assert_eq!(check("PrintScreen"), Ok(()));
        assert_eq!(check("shift+PrintScreen"), Ok(()));
        assert_eq!(check("control+KeyA"), Ok(()));
        assert_eq!(check("shift+super+Digit2"), Ok(()));
    }

    #[test]
    fn a_hotkey_cannot_serve_two_captures() {
        let config = Config::default();
        // Le raccourci de l'écran, écrit autrement, pour la zone.
        let screen = HotKey::from_str(&config.screen_hotkey).unwrap();
        assert_eq!(
            validate(&screen.into_string(), Mode::Region, &config),
            Err(Invalid::Duplicate(Mode::Screen))
        );
        // Le sien reste valable.
        assert_eq!(
            validate(&config.region_hotkey, Mode::Region, &config),
            Ok(())
        );
        assert!(
            Invalid::Duplicate(Mode::Screen)
                .to_string()
                .contains("Capturer l'écran")
        );
    }

    #[test]
    fn typed_combination_is_recorded() {
        let mut r = Recorder::default();
        let mods = egui::Modifiers {
            shift: true,
            ..command()
        };
        let events = [
            key(Key::ShiftLeft, true, egui::Modifiers::SHIFT),
            key(Key::Num2, true, mods),
        ];
        let expected = if cfg!(target_os = "macos") {
            "shift+super+Digit2"
        } else {
            "control+shift+Digit2"
        };
        assert_eq!(
            r.read(&events, mods),
            Some(Recorded::Combo(expected.into()))
        );
    }

    #[test]
    fn escape_cancels_but_not_with_modifiers() {
        let mut r = Recorder::default();
        let none = egui::Modifiers::NONE;
        assert_eq!(
            r.read(&[key(Key::Escape, true, none)], none),
            Some(Recorded::Cancel)
        );
        let ctrl = egui::Modifiers::CTRL;
        assert_eq!(
            r.read(&[key(Key::Escape, true, ctrl)], ctrl),
            Some(Recorded::Combo("control+Escape".into()))
        );
    }

    #[test]
    fn clipboard_shortcuts_are_recorded() {
        let mut r = Recorder::default();
        let expected = format!("{}KeyC", command_text());
        assert_eq!(
            r.read(&[Event::Copy], command()),
            Some(Recorded::Combo(expected))
        );
        // Son relâchement ne compte pas une deuxième fois.
        assert_eq!(r.read(&[key(Key::C, false, command())], command()), None);
        let shift = egui::Modifiers {
            shift: true,
            ..command()
        };
        let Some(Recorded::Combo(text)) = r.read(&[Event::Paste("x".into())], shift) else {
            panic!("⇧⌘V doit être enregistré");
        };
        assert!(text.contains("shift+") && text.ends_with("KeyV"), "{text}");
    }

    #[test]
    fn release_without_press_counts_for_print_screen_only() {
        let mut r = Recorder::default();
        let none = egui::Modifiers::NONE;
        // Windows : seul le relâchement d'Impr. écran arrive.
        assert_eq!(
            r.read(&[key(Key::PrintScreen, false, none)], none),
            Some(Recorded::Combo("PrintScreen".into()))
        );
        // Une autre touche relâchée (appuyée avant l'enregistrement) : rien.
        assert_eq!(r.read(&[key(Key::Enter, false, none)], none), None);
        // ⌘V sans appui (presse-papiers vide) : le relâchement compte.
        let mut r = Recorder::default();
        let Some(Recorded::Combo(text)) = r.read(&[key(Key::V, false, command())], command())
        else {
            panic!("⌘V relâché doit compter");
        };
        assert!(text.ends_with("KeyV"), "{text}");
    }

    #[test]
    fn windows_key_is_tracked_from_its_presses() {
        let mut r = Recorder::default();
        let none = egui::Modifiers::NONE;
        let events = [key(Key::SuperLeft, true, none), key(Key::F16, true, none)];
        assert_eq!(
            r.read(&events, none),
            Some(Recorded::Combo("super+F16".into()))
        );
        assert!(r.live(none).cmd);
        r.focus_lost();
        assert!(!r.live(none).cmd);
    }

    #[test]
    fn unsupported_keys_are_reported() {
        let mut r = Recorder::default();
        let ctrl = egui::Modifiers::CTRL;
        assert_eq!(
            r.read(&[key(Key::BrowserBack, true, ctrl)], ctrl),
            Some(Recorded::Unsupported)
        );
    }

    #[test]
    fn modifier_labels_follow_the_platform() {
        let mods = Mods {
            shift: true,
            cmd: true,
            ..Mods::default()
        };
        if cfg!(target_os = "macos") {
            assert_eq!(mods.label(), "⇧⌘");
        } else {
            assert_eq!(mods.label(), "Maj+Win+");
        }
        assert_eq!(Mods::default().label(), "");
    }
}
