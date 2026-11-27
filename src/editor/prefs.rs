//! Préférences de l'éditeur, gardées d'une ouverture à l'autre (`editor.json` à côté
//! de `config.json`) : dernier outil, style courant et styles favoris.

use std::collections::BTreeMap;
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
    // --- Lot « Éditeur : annotations »
    /// Préréglages numérotés de chaque outil (en plus des favoris ★).
    pub tool_presets: BTreeMap<Tool, Vec<Style>>,
    /// « Valider » copie aussi le chemin : l'image est exportée et placée dans le
    /// presse-papiers comme fichier.
    pub validate_with_path: bool,
    /// Barres d'outils et d'options à leur taille d'origine, plus petite.
    pub compact_bars: bool,
    /// Place de la barre d'outils.
    pub toolbar_side: ToolbarSide,
}

/// Place de la barre d'outils : en haut (d'origine), ou en colonne à gauche ou à
/// droite de l'image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolbarSide {
    #[default]
    Top,
    Left,
    Right,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            tool: Tool::Arrow,
            style: Style::default(),
            presets: Vec::new(),
            tool_presets: BTreeMap::new(),
            validate_with_path: false,
            compact_bars: false,
            toolbar_side: ToolbarSide::Top,
        }
    }
}

/// Nombre maximal de préréglages par outil.
pub const MAX_TOOL_PRESETS: usize = 12;

/// Préréglages numérotés d'un outil : ajout, écrasement, suppression.
pub trait ToolPresets {
    /// Ajoute `style` en dernier (le plus ancien part au-delà du maximum) ; rend son
    /// numéro (à partir de 0).
    fn add(&mut self, tool: Tool, style: Style) -> usize;
    fn overwrite(&mut self, tool: Tool, index: usize, style: Style);
    fn delete(&mut self, tool: Tool, index: usize);
}

impl ToolPresets for BTreeMap<Tool, Vec<Style>> {
    fn add(&mut self, tool: Tool, style: Style) -> usize {
        let list = self.entry(tool).or_default();
        list.push(style);
        if list.len() > MAX_TOOL_PRESETS {
            list.remove(0);
        }
        list.len() - 1
    }

    fn overwrite(&mut self, tool: Tool, index: usize, style: Style) {
        if let Some(slot) = self.get_mut(&tool).and_then(|l| l.get_mut(index)) {
            *slot = style;
        }
    }

    fn delete(&mut self, tool: Tool, index: usize) {
        if let Some(list) = self.get_mut(&tool) {
            if index < list.len() {
                list.remove(index);
            }
            if list.is_empty() {
                self.remove(&tool);
            }
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
    fn older_prefs_keep_the_toolbar_on_top_and_big() {
        let prefs: Prefs = serde_json::from_str(r#"{"tool":"rect"}"#).unwrap();
        assert_eq!(prefs.toolbar_side, ToolbarSide::Top);
        assert!(!prefs.compact_bars);
        let left: Prefs = serde_json::from_str(r#"{"toolbar_side":"left"}"#).unwrap();
        assert_eq!(left.toolbar_side, ToolbarSide::Left);
    }

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

    #[test]
    fn older_editor_json_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("editor.json");
        std::fs::write(
            &path,
            r#"{"tool":"rect","style":{"color":[255,0,0,255],"size":"L","fill":true},"presets":[]}"#,
        )
        .unwrap();
        let prefs = Prefs::load(&path);
        assert_eq!(prefs.tool, Tool::Rect);
        assert!(prefs.style.fill);
        assert_eq!(
            prefs.style.opacity, 1.0,
            "nouvelle option : valeur d'origine"
        );
        assert!(prefs.tool_presets.is_empty());
    }

    #[test]
    fn tool_presets_add_overwrite_delete_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("editor.json");
        let mut prefs = Prefs::default();
        let red = Style::default();
        let green = Style {
            color: PALETTE[3].0,
            ..Style::default()
        };
        assert_eq!(prefs.tool_presets.add(Tool::Step, red.clone()), 0);
        assert_eq!(prefs.tool_presets.add(Tool::Step, red.clone()), 1);
        prefs.tool_presets.overwrite(Tool::Step, 1, green.clone());
        prefs.save(&path);
        let loaded = Prefs::load(&path);
        assert_eq!(loaded.tool_presets[&Tool::Step], vec![red, green]);
        let mut presets = loaded.tool_presets;
        presets.delete(Tool::Step, 0);
        presets.delete(Tool::Step, 0);
        assert!(presets.is_empty());
        for _ in 0..20 {
            presets.add(Tool::Arrow, Style::default());
        }
        assert_eq!(presets[&Tool::Arrow].len(), MAX_TOOL_PRESETS);
    }
}
