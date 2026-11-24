//! Préférences de la fenêtre « Espace de travail », dans leur propre fichier
//! (`workspace.json` à côté de `config.json`) : elles ne concernent que cette
//! fenêtre et s'écrivent à chaque changement.
//!
//! Valeurs par défaut reprises de Screenpresso (`WorkspaceAlwaysOnTop`,
//! `WorkspaceAutoClose`, `WorkspaceAutoCloseDelay`, `WorkspaceThumbnailSize`).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::library::Sort;
use crate::config::{Store, write_atomic};

const FILE: &str = "workspace.json";

/// Bornes de la taille des vignettes, en points.
pub const THUMB_MIN: f32 = 80.0;
pub const THUMB_MAX: f32 = 300.0;
/// Pas de ⌘+ / ⌘−.
pub const THUMB_STEP: f32 = 20.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Fenêtre au-dessus des autres (F8).
    pub always_on_top: bool,
    /// Fermeture quelques secondes après la perte du focus (F9).
    pub auto_close: bool,
    /// Délai de la fermeture automatique, en secondes.
    pub auto_close_delay: f32,
    /// Côté des vignettes, en points.
    pub thumb_size: f32,
    /// Rouvrir la fenêtre là où on l'a laissée plutôt que sous l'icône.
    pub remember_position: bool,
    /// Dernière position (coin haut gauche, points), si `remember_position`.
    pub position: Option<[f32; 2]>,
    pub sort: Sort,
    /// Panneau des espaces de travail visible (F10).
    pub show_workspaces: bool,
    /// Espace ouvert : sous-dossier des captures ; vide pour « Base ».
    pub workspace: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            always_on_top: true,
            auto_close: true,
            auto_close_delay: 3.0,
            thumb_size: 150.0,
            remember_position: false,
            position: None,
            sort: Sort::DateAsc,
            show_workspaces: true,
            workspace: String::new(),
        }
    }
}

impl Prefs {
    pub fn path(store: &Store) -> PathBuf {
        store.config_path().with_file_name(FILE)
    }

    /// Préférences enregistrées ; absentes ou illisibles : les valeurs par défaut.
    pub fn load(store: &Store) -> Self {
        let mut prefs: Self = fs::read_to_string(Self::path(store))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        prefs.thumb_size = prefs.thumb_size.clamp(THUMB_MIN, THUMB_MAX);
        prefs.auto_close_delay = prefs.auto_close_delay.clamp(0.5, 60.0);
        prefs
    }

    pub fn save(&self, store: &Store) {
        let result = serde_json::to_vec_pretty(self)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| write_atomic(&Self::path(store), &bytes));
        if let Err(e) = result {
            log::warn!("préférences de l'espace de travail non enregistrées : {e:#}");
        }
    }

    /// Vignettes plus grandes (`delta` > 0) ou plus petites.
    pub fn zoom_thumbs(&mut self, delta: f32) {
        self.thumb_size = (self.thumb_size + delta).clamp(THUMB_MIN, THUMB_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_screenpresso() {
        let p = Prefs::default();
        assert!(p.always_on_top && p.auto_close);
        assert_eq!(p.auto_close_delay, 3.0);
        assert_eq!(p.thumb_size, 150.0);
        assert_eq!(p.workspace, "");
    }

    #[test]
    fn prefs_round_trip_in_their_own_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(Prefs::load(&store), Prefs::default());
        let prefs = Prefs {
            auto_close: false,
            thumb_size: 220.0,
            sort: Sort::NameAsc,
            workspace: "Projet".into(),
            ..Prefs::default()
        };
        prefs.save(&store);
        assert_eq!(Prefs::load(&store), prefs);
        assert!(dir.path().join(FILE).exists());
        assert!(
            !store.config_path().exists(),
            "config.json n'est pas touché"
        );
    }

    #[test]
    fn broken_or_partial_files_fall_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        fs::write(Prefs::path(&store), "{ pas du json").unwrap();
        assert_eq!(Prefs::load(&store), Prefs::default());
        fs::write(Prefs::path(&store), r#"{ "thumb_size": 9999 }"#).unwrap();
        let p = Prefs::load(&store);
        assert_eq!(p.thumb_size, THUMB_MAX);
        assert!(p.always_on_top);
    }

    #[test]
    fn thumbnail_zoom_is_bounded() {
        let mut p = Prefs::default();
        p.zoom_thumbs(THUMB_STEP);
        assert_eq!(p.thumb_size, 170.0);
        p.zoom_thumbs(-1000.0);
        assert_eq!(p.thumb_size, THUMB_MIN);
    }
}
