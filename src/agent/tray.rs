//! Icône de Snappy dans la barre des menus (un viseur, en image « modèle » : macOS
//! l'adapte au thème clair ou sombre) et son menu.
//!
//! Vit sur le thread principal.

use anyhow::{Context, Result};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::ipc::Mode;

/// Choix fait dans le menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuChoice {
    Capture(Mode),
    /// Capture de zone après le délai de la config.
    CaptureDelayed,
    OpenCaptures,
    OpenSaved,
    /// Fenêtre des réglages (ouverte, ou passée au premier plan).
    OpenSettings,
    Permission,
    Quit,
}

const IDS: [(&str, MenuChoice); 9] = [
    ("region", MenuChoice::Capture(Mode::Region)),
    ("screen", MenuChoice::Capture(Mode::Screen)),
    ("previous", MenuChoice::Capture(Mode::Previous)),
    ("delayed", MenuChoice::CaptureDelayed),
    ("captures", MenuChoice::OpenCaptures),
    ("saved", MenuChoice::OpenSaved),
    ("settings", MenuChoice::OpenSettings),
    ("permission", MenuChoice::Permission),
    ("quit", MenuChoice::Quit),
];

fn id_of(choice: MenuChoice) -> &'static str {
    IDS.iter()
        .find(|(_, c)| *c == choice)
        .map(|(id, _)| *id)
        .expect("identifiant de menu")
}

fn parse_menu_id(id: &str) -> Option<MenuChoice> {
    IDS.iter().find(|(i, _)| *i == id).map(|(_, c)| *c)
}

/// macOS : viseur noir en image « modèle », que le système teinte selon le thème.
#[cfg(target_os = "macos")]
const ICON_SIZE: u32 = 44;
#[cfg(target_os = "macos")]
const ICON: &[u8] = include_bytes!("../../assets/tray_44.rgba");
/// Windows : l'icône couleur de l'app, lisible sur une barre des tâches claire ou
/// sombre (Windows ne teinte pas les icônes de la zone de notification).
#[cfg(not(target_os = "macos"))]
const ICON_SIZE: u32 = 64;
#[cfg(not(target_os = "macos"))]
const ICON: &[u8] = include_bytes!("../../assets/icon_64.rgba");

pub struct Tray {
    icon: TrayIcon,
}

/// Contenu du menu, qui change avec la config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuState {
    /// Libellé du raccourci de chaque capture.
    pub labels: Vec<(Mode, String)>,
    /// Secondes de la capture différée.
    pub delay: u32,
    /// Ajoute l'entrée qui mène aux réglages de confidentialité.
    pub needs_permission: bool,
}

impl Tray {
    pub fn new(
        state: &MenuState,
        on_choice: impl Fn(MenuChoice) + Send + Sync + 'static,
    ) -> Result<Self> {
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(choice) = parse_menu_id(&event.id().0) {
                on_choice(choice);
            }
        }));
        let icon =
            Icon::from_rgba(ICON.to_vec(), ICON_SIZE, ICON_SIZE).context("icône invalide")?;
        let builder = TrayIconBuilder::new();
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(icon);
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(icon);
        let icon = builder
            .with_tooltip("Snappy")
            .with_menu(Box::new(menu(state)?))
            .with_menu_on_left_click(true)
            .build()
            .context("icône de la barre des menus")?;
        Ok(Self { icon })
    }

    /// Reconstruit le menu (raccourcis ou délai changés dans les réglages).
    pub fn set_menu(&self, state: &MenuState) -> Result<()> {
        self.icon.set_menu(Some(Box::new(menu(state)?)));
        Ok(())
    }
}

/// Menu de l'icône.
fn menu(state: &MenuState) -> Result<Menu> {
    let item = |choice, text: String| MenuItem::with_id(id_of(choice), text, true, None);
    let capture = |mode, text: &str| {
        let shortcut = state
            .labels
            .iter()
            .find(|(m, _)| *m == mode)
            .map(|(_, l)| l.as_str());
        let text = match shortcut {
            Some(s) => format!("{text}\t{s}"),
            None => text.to_owned(),
        };
        item(MenuChoice::Capture(mode), text)
    };
    let menu = Menu::new();
    menu.append_items(&[
        &capture(Mode::Region, "Capturer une zone"),
        &capture(Mode::Screen, "Capturer l'écran"),
        &capture(Mode::Previous, "Capturer la zone précédente"),
        &item(
            MenuChoice::CaptureDelayed,
            format!("Capturer une zone dans {} s", state.delay),
        ),
        &PredefinedMenuItem::separator(),
        &item(MenuChoice::OpenCaptures, "Historique des captures".into()),
        &item(MenuChoice::OpenSaved, "Images enregistrées".into()),
        &item(MenuChoice::OpenSettings, "Réglages…".into()),
    ])?;
    if state.needs_permission {
        menu.append(&item(
            MenuChoice::Permission,
            "Autoriser l'enregistrement de l'écran…".into(),
        ))?;
    }
    menu.append_items(&[
        &PredefinedMenuItem::separator(),
        &item(MenuChoice::Quit, "Quitter Snappy".into()),
    ])?;
    Ok(menu)
}

impl Tray {
    /// Compte à rebours d'une capture différée : le nombre de secondes à côté de
    /// l'icône (macOS) et dans l'info-bulle. `None` : retour à la normale.
    pub fn countdown(&mut self, seconds: Option<u32>) {
        let (title, tip) = match seconds {
            Some(n) => (Some(n.to_string()), format!("Snappy · capture dans {n} s")),
            None => (None, "Snappy".to_owned()),
        };
        self.icon.set_title(title);
        if let Err(e) = self.icon.set_tooltip(Some(tip)) {
            log::warn!("info-bulle : {e}");
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_round_trip() {
        for (id, choice) in IDS {
            assert_eq!(parse_menu_id(id), Some(choice));
            assert_eq!(id_of(choice), id);
        }
        assert_eq!(parse_menu_id("nope"), None);
    }

    #[test]
    fn tray_icon_has_the_right_size() {
        assert_eq!(ICON.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
    }
}
