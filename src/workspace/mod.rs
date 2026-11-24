//! Fenêtre « Espace de travail » : les captures en vignettes (clic gauche sur
//! l'icône de la barre des menus, raccourci « Espace de travail »).
//!
//! Socle : à compléter par le lot « Espace de travail ».

use anyhow::{Result, bail};

use crate::config::Store;

/// Ouvre la fenêtre (processus `snappy --capture workspace`).
pub fn run(_store: Store) -> Result<()> {
    bail!("l'espace de travail arrive bientôt")
}
