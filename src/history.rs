//! Historique des captures : l'image d'origine (`captures/snappy-AAAA-MM-JJ-HHMMSS.png`)
//! et ses annotations à côté (même nom, `.json`). Aussi l'export (image aplatie dans
//! le dossier d'enregistrement), le presse-papiers et « Afficher dans le Finder ».

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ImageEncoder, RgbaImage};

use crate::config::write_atomic;
use crate::model::Doc;

/// Nom de base d'une capture prise maintenant (heure locale), sans extension.
pub fn new_name() -> String {
    let now = jiff::ZonedDateTime::now();
    now.strftime("snappy-%Y-%m-%d-%H%M%S").to_string()
}

/// Chemin libre dans `dir` pour `stem` + `ext` : ajoute `-2`, `-3`… si besoin
/// (deux captures dans la même seconde).
pub fn free_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem}-{n}.{ext}")))
        .find(|p| !p.exists())
        .expect("suite infinie")
}

/// PNG rapide à écrire : les captures Retina sont grandes, la compression maximale
/// coûterait plusieurs centaines de millisecondes pour quelques pour cent.
pub fn write_png(path: &Path, img: &RgbaImage) -> Result<()> {
    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::Adaptive)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .context("encodage PNG")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    }
    write_atomic(path, &bytes)
}

pub fn read_png(path: &Path) -> Result<RgbaImage> {
    Ok(image::open(path)
        .with_context(|| format!("ouverture de {}", path.display()))?
        .to_rgba8())
}

/// Enregistre une nouvelle capture dans l'historique ; rend le chemin de l'image.
pub fn save_capture(captures_dir: &Path, img: &RgbaImage) -> Result<PathBuf> {
    let path = free_path(captures_dir, &new_name(), "png");
    write_png(&path, img)?;
    Ok(path)
}

/// Fichier des annotations d'une capture.
pub fn doc_path(image: &Path) -> PathBuf {
    image.with_extension("json")
}

/// Annotations d'une capture. Absentes ou illisibles : aucune (l'image reste ouvrable).
pub fn load_doc(image: &Path) -> Doc {
    let path = doc_path(image);
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("annotations illisibles ({}) : {e}", path.display());
            Doc::default()
        }),
        Err(_) => Doc::default(),
    }
}

pub fn save_doc(image: &Path, doc: &Doc) -> Result<()> {
    write_atomic(
        &doc_path(image),
        serde_json::to_string_pretty(doc)?.as_bytes(),
    )
}

/// Copie l'image dans le presse-papiers.
pub fn copy_image(img: &RgbaImage) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("presse-papiers indisponible")?;
    clipboard
        .set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: img.as_raw().into(),
        })
        .context("copie de l'image")
}

/// Exporte l'image aplatie dans `dir`, sous le nom de la capture d'origine.
pub fn export(dir: &Path, original: &Path, img: &RgbaImage) -> Result<PathBuf> {
    let stem = original
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_owned)
        .unwrap_or_else(new_name);
    let path = free_path(dir, &stem, "png");
    write_png(&path, img)?;
    Ok(path)
}

/// Montre le fichier dans le Finder (l'Explorateur sous Windows).
pub fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    // Explorer veut `/select,"chemin"` tel quel : l'échappement habituel des
    // arguments le casse dès que le chemin contient un espace.
    #[cfg(windows)]
    let result = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let result = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
    if let Err(e) = result {
        log::warn!("affichage de {} impossible : {e}", path.display());
    }
}

/// Ouvre un dossier dans le Finder.
pub fn open_dir(dir: &Path) {
    let _ = fs::create_dir_all(dir);
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(windows)]
    let result = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let result = std::process::Command::new("xdg-open").arg(dir).spawn();
    if let Err(e) = result {
        log::warn!("ouverture de {} impossible : {e}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Annotation, PALETTE, Pt, Shape};

    #[test]
    fn names_are_sortable_timestamps() {
        let name = new_name();
        assert!(name.starts_with("snappy-20"), "{name}");
        assert_eq!(name.len(), "snappy-2026-11-20-140312".len());
    }

    #[test]
    fn free_path_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let a = free_path(dir.path(), "x", "png");
        fs::write(&a, b"").unwrap();
        let b = free_path(dir.path(), "x", "png");
        assert_eq!(b, dir.path().join("x-2.png"));
    }

    #[test]
    fn capture_and_annotations_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]));
        let path = save_capture(dir.path(), &img).unwrap();
        assert_eq!(read_png(&path).unwrap(), img);
        assert_eq!(load_doc(&path), Doc::default());
        let doc = Doc {
            annotations: vec![Annotation::new(
                Shape::Step {
                    at: Pt::new(1.0, 1.0),
                    number: 1,
                    tail: None,
                },
                PALETTE[0].0,
                4.0,
            )],
        };
        save_doc(&path, &doc).unwrap();
        assert_eq!(load_doc(&path), doc);
    }

    #[test]
    fn export_keeps_the_capture_name() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbaImage::new(2, 2);
        let out = export(
            dir.path(),
            Path::new("/x/snappy-2026-01-02-030405.png"),
            &img,
        )
        .unwrap();
        assert_eq!(out, dir.path().join("snappy-2026-01-02-030405.png"));
    }
}
