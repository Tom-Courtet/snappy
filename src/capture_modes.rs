//! Captures sans sélection libre : tous les écrans, fenêtre active, pipette.
//!
//! - `Mode::AllScreens` : tous les écrans en une image, chacun à sa place sur le
//!   bureau, à la définition de l'écran le plus fin ; enregistrée dans l'historique,
//!   copiée et ouverte dans l'éditeur, comme `Mode::Screen`.
//! - `Mode::Window` : la fenêtre au premier plan de l'application active, découpée
//!   dans l'écran figé ; même suite que `Mode::Screen`. Sans fenêtre (bureau seul),
//!   la sélection de zone habituelle.
//! - `Mode::ColorPicker` : la surcouche en pipette ; un clic copie la couleur.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use image::RgbaImage;

use crate::capture::{self, Capture};
use crate::config::Store;
use crate::history;
use crate::ipc::Mode;
use crate::model::CaptureContext;
use crate::overlay::{self, Options, Outcome};

/// `Mode::AllScreens`, `Mode::Window` ou `Mode::ColorPicker`, avec le curseur selon
/// le réglage `capture_cursor` et les autres options par défaut.
pub fn run(store: Store, mode: Mode, capture: Capture) -> Result<()> {
    let options = Options {
        with_cursor: store.load().capture_cursor,
        ..Options::default()
    };
    run_with(store, mode, capture, options)
}

/// [`run`] avec toutes les options (réticules, contexte de capture), à appeler par
/// `main.rs` quand les réglages correspondants existeront.
pub fn run_with(store: Store, mode: Mode, capture: Capture, options: Options) -> Result<()> {
    match mode {
        Mode::AllScreens => {
            let img = capture
                .compose_all(options.with_cursor)
                .context("aucun écran à assembler")?;
            open_in_editor(store, &img, None)
        }
        Mode::Window => window(store, capture, options),
        Mode::ColorPicker => {
            if let Some(text) = overlay::pick_color(store, capture, options)? {
                log::info!("couleur copiée : {text}");
            }
            Ok(())
        }
        other => bail!("« {} » n'est pas une capture de ce module", other.arg()),
    }
}

/// Fenêtre au premier plan ; sans elle, la sélection de zone.
fn window(store: Store, capture: Capture, options: Options) -> Result<()> {
    let found = capture::front_window().and_then(|w| {
        let (img, _) = capture.crop_desktop(&w.bounds, options.with_cursor)?;
        Some((w, img))
    });
    match found {
        Some((w, img)) => {
            let context = options
                .capture_context
                .then(|| CaptureContext::now(&w.app, &w.title));
            open_in_editor(store, &img, context)
        }
        None => {
            log::info!("aucune fenêtre au premier plan : sélection de zone");
            match overlay::run_with(store, capture, options)? {
                Outcome::Edit(path) => spawn_editor(&path),
                Outcome::Saved(path) => {
                    log::info!("enregistré : {}", path.display());
                    Ok(())
                }
                Outcome::Copied | Outcome::Cancelled => Ok(()),
            }
        }
    }
}

/// Comme `Mode::Screen` : historique (avec le contexte s'il y en a un),
/// presse-papiers, éditeur.
fn open_in_editor(store: Store, img: &RgbaImage, context: Option<CaptureContext>) -> Result<()> {
    let path = history::save_capture(&store.captures_dir(), img)?;
    if let Some(context) = context
        && let Err(e) = history::save_context(&path, &context)
    {
        log::warn!("contexte de la capture : {e:#}");
    }
    if let Err(e) = history::copy_image(img) {
        log::warn!("{e:#}");
    }
    crate::editor::run(store, path)
}

/// L'éditeur dans un nouveau processus : une fenêtre egui ne peut être ouverte
/// qu'une fois par processus sous macOS, et la surcouche vient de se fermer.
fn spawn_editor(path: &Path) -> Result<()> {
    let exe: PathBuf = std::env::current_exe().context("chemin de l'exécutable")?;
    Command::new(exe)
        .arg("--edit")
        .arg(path)
        .spawn()
        .context("lancement de l'éditeur")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_saved_next_to_the_capture_and_keeps_annotations() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbaImage::new(2, 2);
        let path = history::save_capture(dir.path(), &img).unwrap();
        // Sans contexte : le .json n'en parle pas (format inchangé).
        history::save_doc(&path, &history::load_doc(&path)).unwrap();
        let json = std::fs::read_to_string(history::doc_path(&path)).unwrap();
        assert!(!json.contains("context"), "{json}");

        let context = CaptureContext::now(" Safari ", "Accueil");
        assert_eq!(
            (context.app.as_str(), context.title.as_str()),
            ("Safari", "Accueil")
        );
        assert!(
            context.date.starts_with("20") && context.date.contains('T'),
            "{context:?}"
        );
        history::save_context(&path, &context).unwrap();
        let doc = history::load_doc(&path);
        assert_eq!(doc.context, Some(context));
        assert!(doc.annotations.is_empty());
    }
}
