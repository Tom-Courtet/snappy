//! Enregistrement vidéo d'une zone de l'écran, lecteur et export.
//!
//! Socle : à compléter par le lot « Vidéo ».

use anyhow::{Result, bail};

use crate::capture::Capture;
use crate::config::Store;
use crate::ipc::Mode;

/// `Mode::Video` ou `Mode::VideoRegion`.
pub fn run(_store: Store, _mode: Mode, _capture: Capture) -> Result<()> {
    bail!("l'enregistrement vidéo arrive bientôt")
}
