//! Captures sans sélection libre : tous les écrans, fenêtre active, pipette.
//!
//! Socle : à compléter par le lot « Capture ».

use anyhow::{Result, bail};

use crate::capture::Capture;
use crate::config::Store;
use crate::ipc::Mode;

/// `Mode::AllScreens`, `Mode::Window` ou `Mode::ColorPicker`.
pub fn run(_store: Store, mode: Mode, _capture: Capture) -> Result<()> {
    bail!("« {} » arrive bientôt", mode.arg())
}
