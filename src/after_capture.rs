//! Après une capture : filigrane, presse-papiers (« Copier dans le presse-papiers »)
//! et suite (« Ensuite »), selon les réglages.
//!
//! Lot « Réglages, raccourcis, après capture ». Avec les réglages par défaut
//! (`ClipboardContent::Snappy`, `AfterCapture::Snappy`, filigrane désactivé), Snappy
//! fait exactement ce qu'il faisait avant :
//! - zone précédente : image copiée, rien d'autre ;
//! - écran entier : image copiée, puis l'éditeur ;
//! - surcouche : « Copier » copie, « Enregistrer » exporte, « Annoter » ouvre
//!   l'éditeur, et rien d'autre.
//!
//! Les autres captures sans sélection (tous les écrans, fenêtre…) passent par
//! [`auto`] avec [`Usual::Copy`].

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use image::RgbaImage;

use crate::config::{AfterCapture, ClipboardContent, Config, Store};
use crate::ipc::Mode;
use crate::overlay::Outcome;
use crate::{editor, history, render};

/// Ce que Snappy fait d'une capture sans choix quand les réglages sont par défaut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Usual {
    /// Copier l'image (zone précédente, et les nouvelles captures sans sélection).
    Copy,
    /// Copier l'image, puis ouvrir l'éditeur (écran entier).
    CopyAndEdit,
}

/// Capture faite sans choix de l'utilisateur, déjà enregistrée dans l'historique
/// (`path`) : filigrane, presse-papiers, puis la suite. L'éditeur s'ouvre dans ce
/// processus (aucune fenêtre n'y a encore été ouverte).
pub fn auto(
    store: Store,
    config: &Config,
    img: &RgbaImage,
    path: &Path,
    usual: Usual,
) -> Result<()> {
    let img = with_watermark(path, config, img);
    match (config.clipboard, usual) {
        // Comme avant : l'échec de la copie est l'erreur de la zone précédente.
        (ClipboardContent::Snappy, Usual::Copy) => history::copy_image(&img)?,
        (ClipboardContent::Snappy, Usual::CopyAndEdit) => {
            if let Err(e) = history::copy_image(&img) {
                log::warn!("{e:#}");
            }
        }
        (content, _) => {
            if let Err(e) = history::copy_as(content, &img, path) {
                log::warn!("{e:#}");
            }
        }
    }
    match (config.after_capture, usual) {
        (AfterCapture::Snappy, Usual::Copy) => Ok(()),
        (AfterCapture::Snappy, Usual::CopyAndEdit) | (AfterCapture::Editor, _) => {
            editor::run(store, path.to_owned())
        }
        (then, _) => follow(then, Some(path)),
    }
}

/// Après la surcouche de sélection : l'action choisie y a déjà été faite (copie,
/// export, enregistrement dans l'historique).
pub fn after_overlay(store: &Store, outcome: Outcome) -> Result<()> {
    if matches!(outcome, Outcome::Cancelled) {
        return Ok(());
    }
    let config = store.load();
    match outcome {
        Outcome::Edit(path) => {
            if let Err(e) = history::apply_watermark(&path, &config) {
                log::warn!("filigrane : {e:#}");
            }
            copy_file(config.clipboard, &path);
            crate::open_editor(&path)
        }
        Outcome::Saved(exported) => {
            log::info!("enregistré : {}", exported.display());
            copy_file(config.clipboard, &exported);
            follow(
                config.after_capture,
                recent_capture(&store.captures_dir()).as_deref(),
            )
        }
        Outcome::Copied => {
            let original = recent_capture(&store.captures_dir());
            if let Some(path) = &original {
                recopy(&config, path);
            }
            follow(config.after_capture, original.as_deref())
        }
        Outcome::Cancelled => Ok(()),
    }
}

/// Ajoute le filigrane s'il est activé ; rend l'image à copier (avec le filigrane
/// s'il vient d'être ajouté).
fn with_watermark<'a>(path: &Path, config: &Config, img: &'a RgbaImage) -> Cow<'a, RgbaImage> {
    match history::apply_watermark(path, config) {
        Ok(true) => {
            let doc = history::load_doc(path);
            Cow::Owned(render::render(img, &doc.annotations))
        }
        Ok(_) => Cow::Borrowed(img),
        Err(e) => {
            log::warn!("filigrane : {e:#}");
            Cow::Borrowed(img)
        }
    }
}

/// « Enregistrer » et « Annoter » ne copiaient rien : seulement si le réglage le
/// demande, l'image aplatie du fichier `path`.
fn copy_file(content: ClipboardContent, path: &Path) {
    if matches!(
        content,
        ClipboardContent::Snappy | ClipboardContent::Nothing
    ) {
        return;
    }
    let result = flattened(path).and_then(|img| history::copy_as(content, &img, path));
    if let Err(e) = result {
        log::warn!("presse-papiers : {e:#}");
    }
}

/// « Copier » a copié l'image : le chemin en plus (ou à la place) si le réglage le
/// demande, et le filigrane s'il est activé.
fn recopy(config: &Config, path: &Path) {
    let marked = matches!(history::apply_watermark(path, config), Ok(true));
    let content = match config.clipboard {
        ClipboardContent::ImageAndPath => ClipboardContent::ImageAndPath,
        ClipboardContent::Path => ClipboardContent::Path,
        // « Copier » copie l'image, quel que soit le réglage.
        _ if marked => ClipboardContent::Image,
        _ => return,
    };
    let result = flattened(path).and_then(|img| history::copy_as(content, &img, path));
    if let Err(e) = result {
        log::warn!("presse-papiers : {e:#}");
    }
}

/// Image du fichier avec ses annotations.
fn flattened(path: &Path) -> Result<RgbaImage> {
    let base = image::open(path)
        .with_context(|| format!("ouverture de {}", path.display()))?
        .to_rgba8();
    let doc = history::load_doc(path);
    Ok(if doc.annotations.is_empty() {
        base
    } else {
        render::render(&base, &doc.annotations)
    })
}

/// La capture que la surcouche vient d'enregistrer dans l'historique (la plus
/// récente, de moins de deux minutes).
fn recent_capture(captures_dir: &Path) -> Option<PathBuf> {
    let path = history::latest_capture(captures_dir)?;
    let modified = path.metadata().ok()?.modified().ok()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    (age < Duration::from_secs(120)).then_some(path)
}

/// « Ensuite », hors comportement de Snappy.
fn follow(then: AfterCapture, capture: Option<&Path>) -> Result<()> {
    match then {
        AfterCapture::Snappy | AfterCapture::Nothing => Ok(()),
        AfterCapture::Editor => match capture {
            Some(path) => crate::open_editor(path),
            None => {
                log::warn!("éditeur : capture introuvable dans l'historique");
                Ok(())
            }
        },
        AfterCapture::Workspace => open_workspace(),
        AfterCapture::Notify => {
            notify(capture);
            Ok(())
        }
    }
}

/// L'espace de travail dans son propre processus. Pas par l'agent : il refuse une
/// nouvelle « capture » tant que celle-ci (ce processus) n'est pas terminée.
fn open_workspace() -> Result<()> {
    let exe = std::env::current_exe().context("chemin de l'exécutable")?;
    Command::new(exe)
        .args(["--capture", Mode::Workspace.arg()])
        .spawn()
        .context("ouverture de l'espace de travail")?;
    Ok(())
}

/// Notification « Capture enregistrée ».
fn notify(capture: Option<&Path>) {
    let name = capture
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification {} with title \"Snappy\" subtitle \"Capture enregistrée\"",
            applescript_string(&name)
        );
        if let Err(e) = Command::new("osascript").arg("-e").arg(script).spawn() {
            log::warn!("notification impossible : {e}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    log::info!("capture enregistrée ({name}) : notification pas encore disponible sous Windows");
}

/// Texte entre guillemets pour AppleScript.
#[cfg(any(target_os = "macos", test))]
fn applescript_string(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_strings_are_escaped() {
        assert_eq!(applescript_string("a.png"), "\"a.png\"");
        assert_eq!(applescript_string(r#"x"y\z"#), r#""x\"y\\z""#);
    }

    #[test]
    fn recent_capture_ignores_old_files() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(recent_capture(dir.path()), None);
        let path = dir.path().join("a.png");
        history::write_png(&path, &RgbaImage::new(1, 1)).unwrap();
        assert_eq!(recent_capture(dir.path()), Some(path));
    }
}
