//! Autres systèmes (Linux…) : pas encore de capture.

use anyhow::{Result, bail};

use super::{Frame, Shot};

pub fn cursor_position() -> Option<(f64, f64)> {
    None
}

pub fn displays() -> Vec<(u32, Frame)> {
    Vec::new()
}

/// Pas d'autorisation à demander hors macOS.
pub fn has_permission() -> bool {
    true
}

pub fn request_permission() -> bool {
    true
}

pub fn capture_all() -> Result<super::Capture> {
    bail!("capture d'écran pas encore prise en charge sur ce système")
}

/// Fenêtre au premier plan : pas encore disponible sur ce système.
pub fn front_window() -> Option<super::FrontWindow> {
    None
}
