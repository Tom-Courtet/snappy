//! Partage, impression et documents (cahier des charges § 7, § 13.1 « Imprimer » et
//! « Publier », § 11.2).
//!
//! API pour les autres parties de Snappy (espace de travail, éditeur) :
//! - [`share_sheet`] : feuille de partage macOS (AirDrop, Mail avec la pièce jointe,
//!   Messages, Notes…) dans un petit processus `snappy --share`, utilisable depuis
//!   n'importe où (agent, fenêtre egui) ;
//! - [`share_sheet_from_frame`] / [`share_sheet_from_view`] : la même feuille,
//!   accrochée à un bouton d'une fenêtre egui déjà ouverte (sans nouveau processus) ;
//! - [`email`] : nouveau courriel avec les fichiers joints (`snappy --mail`) ;
//! - [`print`] : impression d'une capture (`snappy --print`) ;
//! - [`docgen`] : générateur de documents PDF, HTML, GIF, et sa fenêtre
//!   (`snappy --document`, [`docgen::window::open`]).
//!
//! Sous Windows, pas encore de feuille de partage ni de courriel avec pièce jointe :
//! le fichier est montré dans l'Explorateur ([`Delivery::Revealed`]).
//!
//! Les captures de l'historique sont partagées et imprimées **avec leurs
//! annotations** : [`prepare`] aplatit celles qui ont un `.json` non vide dans un
//! dossier temporaire.

pub mod docgen;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use image::RgbaImage;

use crate::history;

/// Ce qui a été fait pour partager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// L'interface du système est ouverte (feuille de partage, courriel).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Native,
    /// Pas d'équivalent sur ce système : le fichier est montré dans le gestionnaire
    /// de fichiers, pour le glisser où on veut.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Revealed,
}

impl Delivery {
    /// Message à afficher à l'utilisateur, s'il y a lieu.
    pub fn message(self) -> Option<&'static str> {
        match self {
            Delivery::Native => None,
            Delivery::Revealed => Some(
                "Partage pas encore disponible sous Windows : le fichier est affiché \
                 dans l'Explorateur.",
            ),
        }
    }
}

/// Options d'impression (§ 13.1, « Aperçu avant impression »). L'aperçu, l'imprimante,
/// les copies et l'échelle sont dans le panneau d'impression du système.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrintOptions {
    pub orientation: Orientation,
    /// Agrandit les petites images jusqu'à la page (sinon : réduites seulement).
    pub fit_to_page: bool,
    /// Nom du fichier imprimé en bas à droite (macOS).
    pub file_name: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    /// Paysage si l'image est plus large que haute.
    #[default]
    Auto,
    Portrait,
    Landscape,
}

impl Orientation {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn landscape(self, (w, h): (u32, u32)) -> bool {
        match self {
            Orientation::Auto => w > h,
            Orientation::Portrait => false,
            Orientation::Landscape => true,
        }
    }
}

/// Image d'une capture avec ses annotations (`.json` à côté), comme à l'export.
pub fn flattened(path: &Path) -> Result<RgbaImage> {
    let base = history::read_png(path)?;
    let doc = history::load_doc(path);
    Ok(if doc.annotations.is_empty() {
        base
    } else {
        crate::render::render(&base, &doc.annotations)
    })
}

/// Fichiers à partager : les captures annotées sont aplaties dans un dossier
/// temporaire (`snappy-partage`), les autres fichiers passent tels quels.
pub fn prepare(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let dir = std::env::temp_dir().join("snappy-partage");
    paths
        .iter()
        .map(|path| {
            if !path.exists() {
                bail!("fichier introuvable : {}", path.display());
            }
            if history::load_doc(path).annotations.is_empty() {
                return Ok(path.clone());
            }
            let img = flattened(path)?;
            let name = path.file_name().context("nom de fichier")?;
            let out = dir.join(name).with_extension("png");
            history::write_png(&out, &img)?;
            Ok(out)
        })
        .collect()
}

/// Lance `snappy <flag> <fichiers…>` : AppKit veut sa propre boucle, et une fenêtre
/// egui ne peut être ouverte qu'une fois par processus.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn spawn(flag: &str, paths: &[PathBuf]) -> Result<()> {
    let exe = std::env::current_exe().context("chemin de l'exécutable")?;
    Command::new(exe)
        .arg(flag)
        .args(paths)
        .spawn()
        .with_context(|| format!("lancement de snappy {flag}"))?;
    Ok(())
}

/// Feuille de partage du système pour ces fichiers, à côté de la souris.
pub fn share_sheet(paths: &[PathBuf]) -> Result<Delivery> {
    if paths.is_empty() {
        bail!("rien à partager");
    }
    #[cfg(target_os = "macos")]
    {
        spawn("--share", paths)?;
        Ok(Delivery::Native)
    }
    #[cfg(not(target_os = "macos"))]
    {
        history::reveal(&paths[0]);
        Ok(Delivery::Revealed)
    }
}

/// Nouveau courriel avec ces fichiers en pièces jointes (client de messagerie par
/// défaut, via le service « Composer un message » de macOS).
// API pour l'espace de travail et l'éditeur (branchée à la fusion des lots).
#[allow(dead_code)]
pub fn email(paths: &[PathBuf]) -> Result<Delivery> {
    if paths.is_empty() {
        bail!("rien à envoyer");
    }
    #[cfg(target_os = "macos")]
    {
        spawn("--mail", paths)?;
        Ok(Delivery::Native)
    }
    #[cfg(not(target_os = "macos"))]
    {
        history::reveal(&paths[0]);
        Ok(Delivery::Revealed)
    }
}

/// Imprime une capture (avec ses annotations) dans un processus `snappy --print`.
// API pour l'espace de travail et l'éditeur (branchée à la fusion des lots).
#[allow(dead_code)]
pub fn print(path: &Path, options: PrintOptions) -> Result<()> {
    let exe = std::env::current_exe().context("chemin de l'exécutable")?;
    let mut cmd = Command::new(exe);
    cmd.arg("--print").arg(path);
    if options.file_name {
        cmd.arg("--filename");
    }
    match options.orientation {
        Orientation::Auto => {}
        Orientation::Portrait => {
            cmd.arg("--portrait");
        }
        Orientation::Landscape => {
            cmd.arg("--landscape");
        }
    }
    if options.fit_to_page {
        cmd.arg("--fit");
    }
    cmd.spawn().context("lancement de l'impression")?;
    Ok(())
}

/// Feuille de partage accrochée au rectangle `rect` (points egui) d'une fenêtre
/// eframe : à appeler depuis `App::update`, au clic sur le bouton « Partager ».
pub fn share_sheet_from_frame(
    frame: &eframe::Frame,
    rect: egui::Rect,
    paths: &[PathBuf],
) -> Result<Delivery> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = frame
        .window_handle()
        .map_err(|e| anyhow::anyhow!("fenêtre native : {e}"))?;
    match handle.as_raw() {
        RawWindowHandle::AppKit(h) => share_sheet_from_view(
            h.ns_view.as_ptr(),
            [
                f64::from(rect.min.x),
                f64::from(rect.min.y),
                f64::from(rect.width()),
                f64::from(rect.height()),
            ],
            paths,
        ),
        _ => share_sheet_from_view(std::ptr::null_mut(), [0.0; 4], paths),
    }
}

/// Feuille de partage accrochée à une `NSView` (`view`, pointeur brut) : `rect` est
/// `[x, y, largeur, hauteur]` en points, origine en haut à gauche comme dans egui.
/// Doit être appelée sur le thread principal. Hors macOS (ou sans vue) : comme
/// [`share_sheet`].
pub fn share_sheet_from_view(
    view: *mut std::ffi::c_void,
    rect: [f64; 4],
    paths: &[PathBuf],
) -> Result<Delivery> {
    #[cfg(target_os = "macos")]
    if !view.is_null() {
        let files = prepare(paths)?;
        // SAFETY : l'appelant garantit une NSView vivante (celle de sa fenêtre).
        unsafe { macos::picker_in_view(view, rect, &files)? };
        return Ok(Delivery::Native);
    }
    let _ = (view, rect);
    share_sheet(paths)
}

// --- Ligne de commande -------------------------------------------------------------

/// `snappy --share <fichier>…` : petite fenêtre sans bord sous la souris, et la
/// feuille de partage du système. Se termine une fois le partage fait ou annulé.
pub fn run_share(paths: Vec<PathBuf>) -> Result<()> {
    if paths.is_empty() {
        bail!("--share attend au moins un fichier");
    }
    let files = prepare(&paths)?;
    #[cfg(target_os = "macos")]
    return macos::run_picker(&files);
    #[cfg(not(target_os = "macos"))]
    {
        history::reveal(&files[0]);
        println!("{}", Delivery::Revealed.message().unwrap_or_default());
        Ok(())
    }
}

/// `snappy --mail <fichier>…` : nouveau courriel avec les pièces jointes.
pub fn run_mail(paths: Vec<PathBuf>) -> Result<()> {
    if paths.is_empty() {
        bail!("--mail attend au moins un fichier");
    }
    let files = prepare(&paths)?;
    #[cfg(target_os = "macos")]
    return macos::run_mail(&files);
    #[cfg(not(target_os = "macos"))]
    {
        history::reveal(&files[0]);
        println!(
            "Courriel avec pièce jointe pas encore disponible sous Windows : le fichier \
             est affiché dans l'Explorateur."
        );
        Ok(())
    }
}

/// `snappy --print <image> [--filename] [--portrait|--landscape] [--fit]`.
pub fn run_print(mut args: impl Iterator<Item = String>) -> Result<()> {
    let path = PathBuf::from(
        args.next()
            .context("--print attend le chemin d'une image")?,
    );
    let mut options = PrintOptions::default();
    for arg in args {
        match arg.as_str() {
            "--filename" => options.file_name = true,
            "--portrait" => options.orientation = Orientation::Portrait,
            "--landscape" => options.orientation = Orientation::Landscape,
            "--fit" => options.fit_to_page = true,
            other => bail!("--print : option inconnue « {other} »"),
        }
    }
    let img = flattened(&path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    return macos::print_image(&img, &name, options);
    #[cfg(windows)]
    return windows::print_image(&img, &name, options);
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (img, name, options);
        bail!("impression pas encore disponible sur ce système")
    }
}

/// `snappy --document <png>…` : la fenêtre « Nouveau document ».
pub fn run_document(store: crate::config::Store, paths: Vec<PathBuf>) -> Result<()> {
    docgen::window::run(store, paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Annotation, Doc, PALETTE, Pt, Shape};

    #[test]
    fn orientation_auto_follows_the_image() {
        assert!(Orientation::Auto.landscape((300, 200)));
        assert!(!Orientation::Auto.landscape((200, 300)));
        assert!(!Orientation::Portrait.landscape((300, 200)));
        assert!(Orientation::Landscape.landscape((200, 300)));
    }

    #[test]
    fn annotated_captures_are_flattened_for_sharing() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbaImage::from_pixel(40, 30, image::Rgba([255, 255, 255, 255]));
        let plain = dir.path().join("plain-share-test.png");
        let annotated = dir.path().join("annotated-share-test.png");
        history::write_png(&plain, &img).unwrap();
        history::write_png(&annotated, &img).unwrap();
        let doc = Doc {
            annotations: vec![Annotation::new(
                Shape::Step {
                    at: Pt::new(20.0, 15.0),
                    number: 1,
                    tail: None,
                },
                PALETTE[0].0,
                4.0,
            )],
            ..Doc::default()
        };
        history::save_doc(&annotated, &doc).unwrap();
        let out = prepare(&[plain.clone(), annotated.clone()]).unwrap();
        assert_eq!(out[0], plain);
        assert_ne!(out[1], annotated);
        assert_ne!(history::read_png(&out[1]).unwrap(), img);
        assert!(prepare(&[dir.path().join("absent.png")]).is_err());
    }
}
