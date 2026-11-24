//! Options de l'enregistrement vidéo, lues dans `config.json`.
//!
//! Le lot « Réglages » ajoute ces champs à [`crate::config::Config`]. En attendant la
//! fusion, on les lit ici directement dans le JSON, avec indulgence : sous
//! `"video": { "fps": 30, … }`, sous `"video_fps"` ou sous `"fps"` (premier trouvé),
//! et des valeurs en nombre, booléen ou texte. Une valeur absente ou illisible garde
//! sa valeur par défaut. À la fusion, il suffira de construire [`VideoOptions`] à
//! partir de la `Config`.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))] // enregistrement propre à macOS

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Qualité de l'image enregistrée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Quality {
    /// Un pixel par point : fichier quatre fois plus léger sur un écran Retina.
    Low,
    /// Pixels de l'écran, largeur limitée à 1920 pixels.
    Medium,
    /// Pixels de l'écran (Retina : deux par point).
    #[default]
    High,
}

impl Quality {
    fn parse(v: &Value) -> Option<Self> {
        if let Some(n) = v.as_u64() {
            return Some(match n {
                0 => Self::Low,
                1 => Self::Medium,
                _ => Self::High,
            });
        }
        let s = v.as_str()?.trim().to_lowercase();
        Some(match s.as_str() {
            "low" | "basse" | "faible" => Self::Low,
            "medium" | "moyenne" | "normale" => Self::Medium,
            "high" | "haute" | "élevée" | "elevee" | "ultra" | "best" => Self::High,
            _ => return None,
        })
    }

    /// Taille en pixels de la vidéo pour une zone de `width` × `height` points sur
    /// un écran de `scale` pixels par point. Toujours paire (exigence du H.264) et
    /// au moins 2 × 2.
    pub fn output_size(self, width: f64, height: f64, scale: f64) -> (u32, u32) {
        let factor = match self {
            Self::Low => 1.0,
            Self::High => scale,
            Self::Medium => {
                let native = width * scale;
                if native > 1920.0 {
                    1920.0 / width
                } else {
                    scale
                }
            }
        };
        let even = |v: f64| {
            let n = (v * factor).round().max(2.0) as u32;
            n - n % 2
        };
        (even(width), even(height))
    }
}

/// Options de l'enregistrement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoOptions {
    pub quality: Quality,
    /// Images par seconde (1 à 60).
    pub fps: u32,
    /// Enregistrer le micro (autorisation « Microphone »).
    pub microphone: bool,
    /// Enregistrer le son de l'ordinateur.
    pub system_audio: bool,
    /// Compte à rebours avant le départ, en secondes (0 : départ immédiat).
    pub countdown: u32,
    /// Arrêt automatique après ce nombre de minutes (0 : jamais).
    pub stop_after_minutes: u32,
    /// Halo autour du curseur (pas encore réalisé : voir le compte rendu du lot).
    pub highlight_cursor: bool,
    /// Clics de la souris mis en évidence (ScreenCaptureKit, macOS 15+).
    pub mark_clicks: bool,
    /// Ouvrir le lecteur à la fin de l'enregistrement.
    pub show_preview_after: bool,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            quality: Quality::High,
            fps: 30,
            microphone: false,
            system_audio: false,
            countdown: 3,
            stop_after_minutes: 0,
            highlight_cursor: false,
            mark_clicks: false,
            show_preview_after: true,
        }
    }
}

impl VideoOptions {
    pub const FPS_RANGE: std::ops::RangeInclusive<u32> = 1..=60;

    /// Options lues dans le contenu de `config.json` (voir l'en-tête du module).
    pub fn from_config_json(json: &Value) -> Self {
        let get = |key: &str| -> Option<&Value> {
            json.get("video")
                .and_then(|v| v.get(key))
                .or_else(|| json.get(format!("video_{key}")))
                .or_else(|| json.get(key))
                .filter(|v| !v.is_null())
        };
        let flag = |keys: &[&str], default: bool| {
            keys.iter()
                .find_map(|k| get(k).and_then(as_bool))
                .unwrap_or(default)
        };
        let number = |keys: &[&str]| keys.iter().find_map(|k| get(k).and_then(as_u32));
        let d = Self::default();
        let countdown = ["countdown", "countdown_seconds"]
            .iter()
            .find_map(|k| {
                let v = get(k)?;
                // `true` : le compte à rebours habituel ; `false` : aucun.
                v.as_bool()
                    .map(|on| if on { d.countdown } else { 0 })
                    .or_else(|| as_u32(v))
            })
            .unwrap_or(d.countdown);
        Self {
            quality: ["quality", "video_quality"]
                .iter()
                .find_map(|k| get(k).and_then(Quality::parse))
                .unwrap_or(d.quality),
            fps: number(&["fps", "frame_rate", "framerate"])
                .map(|n| n.clamp(*Self::FPS_RANGE.start(), *Self::FPS_RANGE.end()))
                .unwrap_or(d.fps),
            microphone: flag(&["microphone", "mic", "record_microphone"], d.microphone),
            system_audio: flag(&["system_audio", "record_system_audio"], d.system_audio),
            countdown: countdown.min(60),
            stop_after_minutes: number(&["stop_after_minutes", "stop_after"])
                .unwrap_or(d.stop_after_minutes),
            highlight_cursor: flag(&["highlight_cursor"], d.highlight_cursor),
            mark_clicks: flag(&["mark_clicks", "highlight_clicks"], d.mark_clicks),
            show_preview_after: flag(
                &["show_preview_after", "open_player_after"],
                d.show_preview_after,
            ),
        }
    }

    /// Options de `config.json` dans le dossier de données ; défaut s'il est absent.
    pub fn load(store: &crate::config::Store) -> Self {
        std::fs::read_to_string(store.config_path())
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|json| Self::from_config_json(&json))
            .unwrap_or_default()
    }
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_u64().map(|n| n != 0),
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "true" | "oui" | "yes" | "on" | "1" => Some(true),
            "false" | "non" | "no" | "off" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn as_u32(v: &Value) -> Option<u32> {
    match v {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f.round() as u64))
            .map(|n| n.min(u64::from(u32::MAX)) as u32),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_fields_keep_the_defaults() {
        assert_eq!(
            VideoOptions::from_config_json(&json!({})),
            VideoOptions::default()
        );
        assert_eq!(
            VideoOptions::from_config_json(&json!({ "region_hotkey": "x", "fps": null })),
            VideoOptions::default()
        );
    }

    #[test]
    fn fields_are_read_leniently() {
        let o = VideoOptions::from_config_json(&json!({
            "video": { "fps": 60, "microphone": true },
            "video_system_audio": "oui",
            "countdown": false,
            "stop_after_minutes": "5",
            "quality": "basse",
            "mark_clicks": 1,
        }));
        assert_eq!(o.fps, 60);
        assert!(o.microphone && o.system_audio && o.mark_clicks);
        assert_eq!(o.countdown, 0);
        assert_eq!(o.stop_after_minutes, 5);
        assert_eq!(o.quality, Quality::Low);
        assert!(o.show_preview_after);
    }

    #[test]
    fn nested_values_win_and_bad_values_are_ignored() {
        let o = VideoOptions::from_config_json(&json!({
            "video": { "fps": 500, "countdown": 2 },
            "fps": 10,
            "quality": "???",
            "microphone": "peut-être",
        }));
        assert_eq!(o.fps, 60, "borné");
        assert_eq!(o.countdown, 2);
        assert_eq!(o.quality, Quality::High);
        assert!(!o.microphone);
    }

    #[test]
    fn output_sizes_are_even() {
        assert_eq!(Quality::High.output_size(641.0, 481.0, 2.0), (1282, 962));
        assert_eq!(Quality::Low.output_size(641.0, 481.0, 2.0), (640, 480));
        assert_eq!(Quality::Medium.output_size(400.0, 300.0, 2.0), (800, 600));
        let (w, h) = Quality::Medium.output_size(1512.0, 982.0, 2.0);
        assert_eq!(w, 1920);
        assert_eq!(h % 2, 0);
        assert_eq!(Quality::High.output_size(0.5, 0.5, 1.0), (2, 2));
    }
}
