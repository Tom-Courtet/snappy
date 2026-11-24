//! Capture avec défilement : l'image se construit pendant que l'utilisateur fait
//! défiler la zone.
//!
//! Socle : à compléter par le lot « Capture défilante ».

use anyhow::{Result, bail};

use crate::capture::Capture;
use crate::config::Store;

pub fn run(_store: Store, _capture: Capture) -> Result<()> {
    bail!("la capture avec défilement arrive bientôt")
}
