//! Générateur de documents (cahier des charges § 7.3) : plusieurs captures, chacune
//! avec un titre, une description et la légende de ses pastilles numérotées, mises
//! en page dans un PDF, une page HTML ou un GIF animé.
//!
//! - PDF : écrit en Rust pur avec `pdf-writer`. Le texte est en Outfit (la police de
//!   l'interface, licence OFL, embarquée dans le document) : métriques exactes, tous
//!   les caractères de la police, et le texte reste sélectionnable et cherchable.
//! - HTML : **un seul fichier**, images incluses en base64. Plus lourd d'un tiers
//!   qu'avec un dossier d'images à côté, mais il se joint à un courriel, se dépose
//!   dans un wiki ou se déplace sans rien casser.
//! - GIF : une image par capture, délai réglable (crate `gif` ; gifski est AGPL).
//!
//! Les images sont toujours celles de la capture **avec ses annotations** (fichier
//! `.json` à côté), comme à l'export.

mod animated;
mod html;
mod layout;
mod pdf;
pub mod rich;
pub mod window;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use image::RgbaImage;

/// Une capture du document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocItem {
    pub image_path: PathBuf,
    /// Titre (texte brut). Vide : nom du fichier si l'option le demande.
    pub title: String,
    /// Description en « HTML simple » : `<b>`, `<i>`, `<u>`, `<br>` (voir [`rich`]).
    pub description: String,
    /// Légende des pastilles : (numéro, texte). Les textes vides sont ignorés.
    pub step_descriptions: Vec<(u32, String)>,
}

impl DocItem {
    /// Élément prérempli depuis une capture : titre et description lus dans son
    /// fichier d'annotations s'ils y sont (champs `title` et `description`, lus sans
    /// dépendre de la structure `Doc`), une ligne vide par pastille numérotée.
    pub fn from_capture(path: &Path) -> Self {
        let (title, description) = capture_meta(path);
        let mut numbers: Vec<u32> = crate::history::load_doc(path)
            .annotations
            .iter()
            .filter_map(|a| match a.shape {
                crate::model::Shape::Step { number, .. } => Some(number),
                _ => None,
            })
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        Self {
            image_path: path.to_owned(),
            title: title.unwrap_or_default(),
            description: description.unwrap_or_default(),
            step_descriptions: numbers.into_iter().map(|n| (n, String::new())).collect(),
        }
    }

    /// Titre affiché, selon les options.
    fn display_title(&self, options: &DocOptions) -> String {
        let title = self.title.trim();
        if !title.is_empty() || !options.title_from_filename {
            return title.to_owned();
        }
        self.image_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Légendes non vides.
    fn steps(&self) -> impl Iterator<Item = &(u32, String)> {
        self.step_descriptions
            .iter()
            .filter(|(_, t)| !t.trim().is_empty())
    }
}

/// Réglages du document (§ 7.3).
#[derive(Debug, Clone, PartialEq)]
pub struct DocOptions {
    /// Page en paysage pour les images larges (plus larges que hautes et que la
    /// page en portrait). PDF seulement.
    pub landscape_for_large: bool,
    /// Sans titre saisi, le nom du fichier sert de titre.
    pub title_from_filename: bool,
    /// L'image avant la description (sinon : titre, description, légendes, image).
    pub image_before_text: bool,
    /// Saut de page entre l'image et le texte (PDF ; à l'impression pour le HTML).
    pub page_break: bool,
    /// Titre du document (PDF, HTML). Vide : nom du fichier de sortie.
    pub document_title: String,
    /// Délai entre deux images du GIF, en millisecondes.
    pub gif_delay_ms: u32,
}

impl Default for DocOptions {
    fn default() -> Self {
        Self {
            landscape_for_large: true,
            title_from_filename: true,
            image_before_text: false,
            page_break: false,
            document_title: String::new(),
            gif_delay_ms: 2000,
        }
    }
}

/// Format de sortie. Le DOCX n'est pas encore pris en charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Pdf,
    Html,
    Gif,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Pdf, Format::Html, Format::Gif];

    pub fn extension(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Html => "html",
            Format::Gif => "gif",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Pdf => "PDF",
            Format::Html => "HTML",
            Format::Gif => "GIF animé",
        }
    }
}

/// Génère le document `out` à partir des captures `items`.
pub fn generate(items: &[DocItem], options: &DocOptions, format: Format, out: &Path) -> Result<()> {
    if items.is_empty() {
        bail!("aucune image dans le document");
    }
    let images = items
        .iter()
        .map(|item| crate::share::flattened(&item.image_path))
        .collect::<Result<Vec<_>>>()?;
    let title = if options.document_title.trim().is_empty() {
        out.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Document".into())
    } else {
        options.document_title.trim().to_owned()
    };
    let bytes = match format {
        Format::Pdf => pdf::write(items, &images, options, &title)?,
        Format::Html => html::write(items, &images, options, &title)?.into_bytes(),
        Format::Gif => animated::write(&images, options.gif_delay_ms)?,
    };
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    }
    crate::config::write_atomic(out, &bytes)
}

/// Chemin libre pour un nouveau document dans `dir` : `document-AAAA-MM-JJ-HHMMSS.ext`.
pub fn new_path(dir: &Path, format: Format) -> PathBuf {
    let stem = crate::history::new_name().replacen("snappy-", "document-", 1);
    crate::history::free_path(dir, &stem, format.extension())
}

/// Titre et description dans le `.json` d'une capture, s'il les a.
fn capture_meta(image: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(crate::history::doc_path(image)) else {
        return (None, None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (None, None);
    };
    let field = |name: &str| {
        value
            .get(name)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .filter(|s| !s.trim().is_empty())
    };
    (field("title"), field("description"))
}

/// PNG d'une image (rapide : les captures sont grandes).
pub(crate) fn png_bytes(img: &RgbaImage) -> Result<Vec<u8>> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::Adaptive)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .context("encodage PNG")?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Annotation, Doc, PALETTE, Pt, Shape};

    /// Trois captures : une large, une haute, une petite ; la première a deux pastilles.
    fn fixtures(dir: &Path) -> Vec<DocItem> {
        let sizes = [(1600, 700), (400, 900), (120, 80)];
        sizes
            .iter()
            .enumerate()
            .map(|(i, &(w, h))| {
                let img = RgbaImage::from_fn(w, h, |x, y| {
                    image::Rgba([(x % 256) as u8, (y % 256) as u8, (i * 80) as u8, 255])
                });
                let path = dir.join(format!("capture-{i}.png"));
                crate::history::write_png(&path, &img).unwrap();
                if i == 0 {
                    let step = |n| {
                        Annotation::new(
                            Shape::Step {
                                at: Pt::new(10.0 * n as f32, 10.0),
                                number: n,
                                tail: None,
                            },
                            PALETTE[0].0,
                            4.0,
                        )
                    };
                    crate::history::save_doc(
                        &path,
                        &Doc {
                            annotations: vec![step(2), step(1)],
                        },
                    )
                    .unwrap();
                }
                DocItem::from_capture(&path)
            })
            .collect()
    }

    fn filled(mut items: Vec<DocItem>) -> Vec<DocItem> {
        items[0].title = "Ouvrir les réglages".into();
        items[0].description = "Cliquer sur <b>Réglages</b>, puis <i>Général</i>.".into();
        items[0].step_descriptions[0].1 = "Le menu".into();
        items[1].description = "Une longue description ".repeat(40);
        items
    }

    #[test]
    fn items_read_steps_and_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let items = fixtures(dir.path());
        assert_eq!(
            items[0].step_descriptions,
            vec![(1, String::new()), (2, String::new())]
        );
        assert!(items[1].step_descriptions.is_empty());
        // Titre et description ajoutés au .json par un autre lot : lus sans `Doc`.
        let json = r#"{"annotations": [], "title": "Mon titre", "description": "<b>x</b>"}"#;
        std::fs::write(dir.path().join("capture-2.json"), json).unwrap();
        let item = DocItem::from_capture(&dir.path().join("capture-2.png"));
        assert_eq!(item.title, "Mon titre");
        assert_eq!(item.description, "<b>x</b>");
        let options = DocOptions::default();
        assert_eq!(items[1].display_title(&options), "capture-1");
        let options = DocOptions {
            title_from_filename: false,
            ..options
        };
        assert_eq!(items[1].display_title(&options), "");
    }

    /// Nombre de pages d'un PDF : les objets `/Type /Page` (pas `/Pages`).
    fn page_count(pdf: &[u8]) -> usize {
        let text = String::from_utf8_lossy(pdf);
        text.match_indices("/Type /Page")
            .filter(|(i, _)| !text[i + 11..].starts_with('s'))
            .count()
    }

    #[test]
    fn pdf_is_valid_and_paginated() {
        let dir = tempfile::tempdir().unwrap();
        let items = filled(fixtures(dir.path()));
        let out = dir.path().join("doc.pdf");
        generate(&items, &DocOptions::default(), Format::Pdf, &out).unwrap();
        let pdf = std::fs::read(&out).unwrap();
        assert!(pdf.starts_with(b"%PDF-1."));
        assert!(pdf.trim_ascii_end().ends_with(b"%%EOF"));
        let pages = page_count(&pdf);
        // La large en paysage (seule), la haute et la petite en portrait.
        assert!((2..=4).contains(&pages), "{pages} pages");
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains(&format!("/Count {pages}")));
        assert!(text.contains("/FontFile2"));

        // Saut de page entre image et texte : une page de plus pour chacune des deux
        // captures décrites.
        let options = DocOptions {
            page_break: true,
            landscape_for_large: false,
            ..DocOptions::default()
        };
        generate(&items, &options, Format::Pdf, &out).unwrap();
        let more = page_count(&std::fs::read(&out).unwrap());
        assert_eq!(more, 5);
    }

    #[test]
    fn html_contains_titles_and_images() {
        let dir = tempfile::tempdir().unwrap();
        let items = filled(fixtures(dir.path()));
        let out = dir.path().join("doc.html");
        let options = DocOptions {
            document_title: "Guide <test>".into(),
            ..DocOptions::default()
        };
        generate(&items, &options, Format::Html, &out).unwrap();
        let html = std::fs::read_to_string(&out).unwrap();
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<title>Guide &lt;test&gt;</title>"));
        assert!(html.contains("Ouvrir les réglages"));
        assert!(html.contains("capture-1"));
        assert!(html.contains("<b>Réglages</b>"));
        assert!(html.contains("Le menu"));
        assert_eq!(html.matches("data:image/png;base64,").count(), 3);
    }

    #[test]
    fn gif_has_one_frame_per_image() {
        let dir = tempfile::tempdir().unwrap();
        let items = fixtures(dir.path());
        let out = dir.path().join("doc.gif");
        let options = DocOptions {
            gif_delay_ms: 500,
            ..DocOptions::default()
        };
        generate(&items, &options, Format::Gif, &out).unwrap();
        let mut decoder = ::gif::DecodeOptions::new()
            .read_info(std::fs::File::open(&out).unwrap())
            .unwrap();
        let mut frames = 0;
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            assert_eq!(frame.delay, 50);
            frames += 1;
        }
        assert_eq!(frames, 3);
    }

    #[test]
    fn empty_documents_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("x.pdf");
        assert!(generate(&[], &DocOptions::default(), Format::Pdf, &out).is_err());
    }

    #[test]
    fn new_documents_get_free_names() {
        let dir = tempfile::tempdir().unwrap();
        let p = new_path(dir.path(), Format::Html);
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("document-20") && name.ends_with(".html"),
            "{name}"
        );
    }
}
