//! Préférences de l'éditeur, gardées d'une ouverture à l'autre (`editor.json` à côté
//! de `config.json`) : dernier outil, style courant et styles favoris.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::tools::{Style, Tool};
use crate::config::{Store, write_atomic};

/// Nombre maximal de favoris.
pub const MAX_PRESETS: usize = 8;

/// Un style favori : un outil et ses réglages, rappelés d'un clic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub tool: Tool,
    pub style: Style,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub tool: Tool,
    pub style: Style,
    pub presets: Vec<Preset>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            tool: Tool::Arrow,
            style: Style::default(),
            presets: Vec::new(),
        }
    }
}

impl Prefs {
    pub fn path(store: &Store) -> PathBuf {
        store.config_path().with_file_name("editor.json")
    }

    /// Préférences enregistrées ; absentes ou illisibles : celles par défaut.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        let result = serde_json::to_vec_pretty(self)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| write_atomic(path, &bytes));
        if let Err(e) = result {
            log::warn!("préférences de l'éditeur non enregistrées : {e:#}");
        }
    }

    /// Ajoute un favori (sans doublon) ; le plus ancien part au-delà du maximum.
    /// `false` s'il existait déjà.
    pub fn add_preset(&mut self, preset: Preset) -> bool {
        if self.presets.contains(&preset) {
            return false;
        }
        self.presets.push(preset);
        if self.presets.len() > MAX_PRESETS {
            self.presets.remove(0);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArrowStyle, PALETTE};

    #[test]
    fn prefs_round_trip_and_survive_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("editor.json");
        assert_eq!(Prefs::load(&path), Prefs::default());
        let mut prefs = Prefs {
            tool: Tool::Stamp,
            ..Prefs::default()
        };
        prefs.style.arrow = ArrowStyle::Big;
        prefs.save(&path);
        assert_eq!(Prefs::load(&path), prefs);
        std::fs::write(&path, "{ cassé").unwrap();
        assert_eq!(Prefs::load(&path), Prefs::default());
    }

    #[test]
    fn presets_are_unique_and_capped() {
        let mut prefs = Prefs::default();
        let preset = |i: usize| Preset {
            tool: Tool::Rect,
            style: Style {
                color: PALETTE[i % PALETTE.len()].0,
                shadow: i >= PALETTE.len(),
                ..Style::default()
            },
        };
        assert!(prefs.add_preset(preset(0)));
        assert!(!prefs.add_preset(preset(0)), "doublon");
        for i in 1..12 {
            prefs.add_preset(preset(i));
        }
        assert_eq!(prefs.presets.len(), MAX_PRESETS);
        assert_eq!(prefs.presets[0], preset(4), "les plus anciens partent");
    }
}
