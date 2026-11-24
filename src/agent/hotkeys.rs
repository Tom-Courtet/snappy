//! Raccourcis globaux des captures, lus dans la config.
//!
//! À créer sur le thread principal, une fois la boucle d'événements lancée (exigence
//! de macOS). Sous macOS, `global-hotkey` passe par les raccourcis Carbon : aucune
//! autorisation d'accessibilité n'est nécessaire.
//!
//! `global-hotkey` n'accepte qu'**un** gestionnaire d'événements pour toute la vie
//! du processus (les appels suivants sont ignorés) : il est installé une fois, et
//! consulte la table des raccourcis actifs, remplacée à chaque rechargement.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, Once};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use super::layout;
use crate::config::Config;
use crate::ipc::{Mode, Refused};

/// Les captures qui ont un raccourci, dans l'ordre du menu.
pub const MODES: [Mode; 3] = [Mode::Region, Mode::Screen, Mode::Previous];

/// Action d'un appui sur un raccourci.
type OnCapture = Arc<dyn Fn(Mode) + Send + Sync>;

/// Raccourcis actifs (identifiant `global-hotkey` → capture) et leur action.
static ROUTES: Mutex<Option<(HashMap<u32, Mode>, OnCapture)>> = Mutex::new(None);
static HANDLER: Once = Once::new();

/// Installe le gestionnaire d'événements unique, qui suit [`ROUTES`].
fn install_handler() {
    HANDLER.call_once(|| {
        GlobalHotKeyEvent::set_event_handler(Some(|event: GlobalHotKeyEvent| {
            if event.state() != HotKeyState::Pressed {
                return;
            }
            let routes = ROUTES.lock().unwrap_or_else(|e| e.into_inner());
            let target = routes.as_ref().and_then(|(table, on_capture)| {
                table
                    .get(&event.id())
                    .map(|&mode| (mode, Arc::clone(on_capture)))
            });
            drop(routes);
            if let Some((mode, on_capture)) = target {
                on_capture(mode);
            }
        }));
    });
}

/// Garde les raccourcis enregistrés tant qu'elle vit.
pub struct Hotkeys {
    _manager: Option<GlobalHotKeyManager>,
    labels: Vec<(Mode, String)>,
    refused: Vec<Refused>,
}

impl Hotkeys {
    /// Enregistre les raccourcis de la config ; `on_capture(mode)` est appelé à chaque
    /// appui. Un raccourci vide est ignoré ; un raccourci invalide ou déjà pris est
    /// noté dans [`Self::refused`], les autres restent actifs.
    pub fn register(config: &Config, on_capture: impl Fn(Mode) + Send + Sync + 'static) -> Self {
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => m,
            Err(e) => {
                log::error!("raccourcis globaux indisponibles : {e}");
                let refused = MODES
                    .iter()
                    .filter(|&&mode| !config.hotkey(mode).is_empty())
                    .map(|&mode| Refused {
                        mode,
                        hotkey: config.hotkey(mode).to_owned(),
                        reason: "raccourcis globaux indisponibles".into(),
                    })
                    .collect();
                return Self {
                    _manager: None,
                    labels: Vec::new(),
                    refused,
                };
            }
        };
        let mut table = HashMap::new();
        let mut labels = Vec::new();
        let mut refused = Vec::new();
        for mode in MODES {
            let text = config.hotkey(mode);
            if text.trim().is_empty() {
                continue;
            }
            let mut refuse = |reason: String| {
                log::warn!("raccourci « {text} » refusé : {reason}");
                refused.push(Refused {
                    mode,
                    hotkey: text.to_owned(),
                    reason,
                });
            };
            let hotkey = match HotKey::from_str(text) {
                Ok(h) => h,
                Err(e) => {
                    refuse(format!("raccourci invalide ({e})"));
                    continue;
                }
            };
            if table.contains_key(&hotkey.id()) {
                refuse("déjà utilisé par une autre capture".into());
                continue;
            }
            match manager.register(hotkey) {
                Ok(()) => {
                    table.insert(hotkey.id(), mode);
                    labels.push((mode, label(&hotkey)));
                }
                Err(global_hotkey::Error::AlreadyRegistered(_)) => {
                    refuse("déjà pris par une autre application".into());
                }
                Err(e) => refuse(format!("refusé par le système ({e})")),
            }
        }
        log::info!("{} raccourcis enregistrés", table.len());
        let on_capture: OnCapture = Arc::new(on_capture);
        *ROUTES.lock().unwrap_or_else(|e| e.into_inner()) = Some((table, on_capture));
        install_handler();
        Self {
            _manager: Some(manager),
            labels,
            refused,
        }
    }

    /// Libellé (⌘⇧2…) de chaque raccourci actif.
    pub fn labels(&self) -> Vec<(Mode, String)> {
        self.labels.clone()
    }

    /// Raccourcis de la config que le système n'a pas acceptés.
    pub fn refused(&self) -> Vec<Refused> {
        self.refused.clone()
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        *ROUTES.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Modificateurs dans l'ordre et sous le nom d'usage du système.
#[cfg(target_os = "macos")]
const MODIFIERS: [(Modifiers, &str); 4] = [
    (Modifiers::CONTROL, "⌃"),
    (Modifiers::ALT, "⌥"),
    (Modifiers::SHIFT, "⇧"),
    (Modifiers::META, "⌘"),
];
#[cfg(not(target_os = "macos"))]
const MODIFIERS: [(Modifiers, &str); 4] = [
    (Modifiers::CONTROL, "Ctrl+"),
    (Modifiers::SHIFT, "Maj+"),
    (Modifiers::ALT, "Alt+"),
    (Modifiers::META, "Win+"),
];

/// Libellé du raccourci : ⇧⌘2 sous macOS, Maj+Impr. écran sous Windows. Sous
/// macOS, sur le thread principal, les lettres suivent la disposition du clavier
/// (⌘A sur la touche A d'un clavier AZERTY, enregistrée comme `KeyQ`).
pub fn label(hotkey: &HotKey) -> String {
    let mut out = String::new();
    for (m, sym) in MODIFIERS {
        if hotkey.mods.contains(m) {
            out.push_str(sym);
        }
    }
    if let Some(engraved) = layout::engraved(hotkey.key) {
        out.push_str(&engraved);
        return out;
    }
    match key_name(hotkey.key) {
        Some(name) => out.push_str(name),
        None => {
            let key = hotkey.key.to_string();
            out.push_str(
                key.strip_prefix("Digit")
                    .or_else(|| key.strip_prefix("Key"))
                    .unwrap_or(&key),
            );
        }
    }
    out
}

/// Nom affiché des touches qui ne sont ni une lettre, ni un chiffre, ni F1–F24.
fn key_name(code: Code) -> Option<&'static str> {
    // Symbole sous macOS, nom ailleurs.
    let os = |mac: &'static str, other: &'static str| {
        if cfg!(target_os = "macos") {
            mac
        } else {
            other
        }
    };
    Some(match code {
        Code::Space => "Espace",
        Code::PrintScreen => "Impr. écran",
        Code::Enter => os("↩", "Entrée"),
        Code::Backspace => os("⌫", "Retour arrière"),
        Code::Delete => os("⌦", "Suppr"),
        Code::Tab => os("⇥", "Tab"),
        Code::Escape => "Échap",
        Code::ArrowLeft => "←",
        Code::ArrowRight => "→",
        Code::ArrowUp => "↑",
        Code::ArrowDown => "↓",
        Code::Home => "Début",
        Code::End => "Fin",
        Code::PageUp => "Page préc.",
        Code::PageDown => "Page suiv.",
        Code::Insert => "Inser",
        Code::Pause => "Pause",
        Code::ScrollLock => "Arrêt défil",
        Code::Minus => "-",
        Code::Equal => "=",
        Code::BracketLeft => "[",
        Code::BracketRight => "]",
        Code::Backslash => "\\",
        Code::IntlBackslash => "<",
        Code::Semicolon => ";",
        Code::Quote => "'",
        Code::Backquote => "`",
        Code::Comma => ",",
        Code::Period => ".",
        Code::Slash => "/",
        _ => return None,
    })
}

/// Libellé d'un raccourci écrit dans la config ; `None` s'il est vide ou illisible.
pub fn label_of(text: &str) -> Option<String> {
    HotKey::from_str(text.trim()).ok().map(|h| label(&h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hotkeys_parse_and_differ() {
        let config = Config::default();
        let ids: std::collections::HashSet<u32> = [
            &config.region_hotkey,
            &config.screen_hotkey,
            &config.previous_hotkey,
        ]
        .iter()
        .map(|t| HotKey::from_str(t).unwrap().id())
        .collect();
        assert_eq!(ids.len(), 3);
    }

    /// Les raccourcis Windows se lisent aussi sur Mac : on les vérifie partout.
    #[test]
    fn windows_defaults_are_print_screen() {
        let region = HotKey::from_str("PrintScreen").unwrap();
        let previous = HotKey::from_str("shift+PrintScreen").unwrap();
        let screen = HotKey::from_str("control+alt+shift+PrintScreen").unwrap();
        assert_eq!(region, HotKey::new(None, Code::PrintScreen));
        assert_eq!(
            previous,
            HotKey::new(Some(Modifiers::SHIFT), Code::PrintScreen)
        );
        let all = Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT;
        assert_eq!(screen, HotKey::new(Some(all), Code::PrintScreen));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn labels_look_like_macos() {
        let hk = HotKey::from_str("super+shift+Digit2").unwrap();
        assert_eq!(
            hk,
            HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::Digit2)
        );
        assert_eq!(label(&hk), "⇧⌘2");
        let hk = HotKey::from_str("control+alt+KeyS").unwrap();
        assert_eq!(label(&hk), "⌃⌥S");
    }

    #[test]
    fn label_of_config_text() {
        assert_eq!(label_of(""), None);
        assert_eq!(label_of("pas+un+raccourci"), None);
        assert!(label_of("F13").is_some());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn labels_look_like_windows() {
        let hk = HotKey::from_str("shift+PrintScreen").unwrap();
        assert_eq!(label(&hk), "Maj+Impr. écran");
    }
}
