//! Analyse d'image : texte (OCR), QR codes, détourage, données sensibles.
//!
//! Socle : à compléter par le lot « Vision ».

use anyhow::{Result, bail};

use crate::capture::Capture;
use crate::config::Store;
use crate::ipc::Mode;

/// `Mode::Ocr` ou `Mode::Qr` : sélection d'une zone sur l'écran figé, puis analyse.
pub fn run(_store: Store, mode: Mode, _capture: Capture) -> Result<()> {
    bail!("« {} » arrive bientôt", mode.arg())
}
