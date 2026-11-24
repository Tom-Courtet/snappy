//! Analyse d'image : texte (OCR), QR codes, détourage, données sensibles.
//!
//! Bibliothèque sans interface ([`ocr`], [`decode_codes`], [`remove_background`],
//! [`sensitive_regions`]), utilisable par l'éditeur et l'espace de travail, et deux
//! parcours lancés par `snappy --capture ocr|qr` ([`run`]) : sélection d'une zone
//! sur l'écran figé, analyse, puis petite fenêtre de résultat.
//!
//! Moteurs : Vision sous macOS (aucune autorisation pour analyser une image déjà
//! capturée), `Windows.Media.Ocr` sous Windows 10, `rqrr` (pur Rust) pour les QR
//! codes partout.

mod codes;
mod geom;
#[cfg(target_os = "macos")]
mod macos;
mod sensitive;
mod window;
#[cfg(windows)]
mod windows;

use std::io::Write as _;
use std::ops::Range;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::capture::Capture;
use crate::config::Store;
use crate::history;
use crate::ipc::Mode;
use crate::overlay::{self, Purpose};

pub use codes::Code;
pub use geom::Rect;
pub use sensitive::SensitiveKind;

/// Argument de la fenêtre de résultat, lancée dans son propre processus (une seule
/// boucle egui par processus sous macOS, et la sélection vient de fermer la sienne).
/// Le résultat arrive en JSON sur l'entrée standard.
pub const WINDOW_ARG: &str = "--vision-window";

/// Essai sans sélection : `SNAPPY_VISION_FILE=<png> snappy --capture ocr|qr` analyse
/// ce fichier au lieu d'une zone de l'écran.
const TEST_FILE_ENV: &str = "SNAPPY_VISION_FILE";

/// Réglages lus dans `config.json` sans passer par `Config` (ajoutés par un autre
/// lot) : absents, valeurs par défaut ci-dessous.
const OCR_COPY_WITHOUT_WINDOW: (&str, bool) = ("ocr_copy_without_window", false);
const QR_OPEN_BROWSER: (&str, bool) = ("qr_open_browser", true);

/// Ligne de texte reconnue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextLine {
    pub text: String,
    /// Boîte de la ligne, en pixels de l'image.
    pub rect: Rect,
    /// Confiance du moteur, de 0 à 1 (Windows n'en donne pas : 1).
    pub confidence: f32,
}

/// Zone de l'image qui contient une donnée sensible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensitiveHit {
    pub kind: SensitiveKind,
    pub rect: Rect,
}

/// Ligne lue par un moteur et boîtes des passages demandés.
#[allow(dead_code)]
struct Recognized<T> {
    line: TextLine,
    spans: Vec<(T, Rect)>,
}

/// Texte de l'image, ligne par ligne, dans l'ordre de lecture du moteur.
pub fn ocr(img: &RgbaImage) -> Result<Vec<TextLine>> {
    Ok(recognize(img, |_| Vec::<((), Range<usize>)>::new())?
        .into_iter()
        .map(|r| r.line)
        .collect())
}

/// Texte complet : une ligne reconnue par ligne.
pub fn lines_text(lines: &[TextLine]) -> String {
    lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Moteur d'OCR et langues, en clair.
pub fn engine_info() -> String {
    #[cfg(target_os = "macos")]
    return macos::engine_info();
    #[cfg(windows)]
    return windows::engine_info();
    #[cfg(not(any(target_os = "macos", windows)))]
    return "aucun moteur d'OCR".into();
}

/// Zones à flouter : courriels, adresses IP, cartes bancaires, IBAN, clés d'API,
/// téléphones français trouvés par l'OCR.
#[allow(dead_code)]
pub fn sensitive_regions(img: &RgbaImage) -> Result<Vec<SensitiveHit>> {
    Ok(recognize(img, sensitive::find)?
        .into_iter()
        .flat_map(|r| r.spans)
        .map(|(kind, rect)| SensitiveHit { kind, rect })
        .collect())
}

/// QR codes (toutes plateformes) et, sous macOS, autres codes-barres lus par Vision.
pub fn decode_codes(img: &RgbaImage) -> Vec<Code> {
    #[allow(unused_mut)]
    let mut found = codes::decode_qr(img);
    log::debug!("rqrr : {} code(s)", found.len());
    #[cfg(target_os = "macos")]
    match macos::barcodes(img) {
        Ok(more) => {
            log::debug!("Vision : {} code(s)", more.len());
            for code in more {
                codes::push_unique(&mut found, code);
            }
        }
        Err(e) => log::warn!("codes-barres : {e:#}"),
    }
    found
}

/// Image dont l'arrière-plan est rendu transparent (macOS 14 et plus récent).
#[allow(dead_code)]
pub fn remove_background(img: &RgbaImage) -> Result<RgbaImage> {
    #[cfg(target_os = "macos")]
    return macos::remove_background(img);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = img;
        anyhow::bail!("suppression de l'arrière-plan pas encore disponible sous Windows")
    }
}

fn recognize<T>(
    img: &RgbaImage,
    spans_of: impl Fn(&str) -> Vec<(T, Range<usize>)>,
) -> Result<Vec<Recognized<T>>> {
    #[cfg(target_os = "macos")]
    return macos::recognize(img, spans_of);
    #[cfg(windows)]
    return windows::recognize(img, spans_of);
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (img, spans_of);
        anyhow::bail!("OCR pas encore disponible sur ce système")
    }
}

// --- Parcours `snappy --capture ocr|qr` ---

/// Résultat envoyé à la fenêtre.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum Outcome {
    Text {
        text: String,
        info: String,
        millis: u64,
    },
    Codes {
        codes: Vec<Code>,
        /// Adresse ouverte dans le navigateur, le cas échéant.
        opened: Option<String>,
        copied: bool,
    },
    Error {
        title: String,
        message: String,
    },
}

/// `Mode::Ocr` ou `Mode::Qr` : sélection d'une zone sur l'écran figé, puis analyse.
pub fn run(store: Store, mode: Mode, capture: Capture) -> Result<()> {
    let image = match std::env::var_os(TEST_FILE_ENV) {
        Some(path) => history::read_png(Path::new(&path))?,
        None => {
            let purpose = if mode == Mode::Qr {
                Purpose::Qr
            } else {
                Purpose::Ocr
            };
            match overlay::pick(store.clone(), capture, false, purpose)? {
                Some(picked) => picked.image,
                None => return Ok(()),
            }
        }
    };
    let outcome = if mode == Mode::Qr {
        qr_flow(&store, &image)
    } else {
        ocr_flow(&store, &image)
    };
    match outcome {
        Some(outcome) => show(&outcome),
        None => Ok(()),
    }
}

/// OCR, puis copie directe (réglage) ou fenêtre. `None` : rien à afficher.
fn ocr_flow(store: &Store, image: &RgbaImage) -> Option<Outcome> {
    let started = Instant::now();
    let lines = match ocr(image) {
        Ok(lines) => lines,
        Err(e) => {
            return Some(Outcome::Error {
                title: "Texte non reconnu".into(),
                message: format!("{e:#}"),
            });
        }
    };
    let millis = started.elapsed().as_millis() as u64;
    log::info!("OCR : {} lignes en {millis} ms", lines.len());
    let text = lines_text(&lines);
    if config_flag(store, OCR_COPY_WITHOUT_WINDOW) && !text.is_empty() {
        match copy_text(&text) {
            Ok(()) => return None,
            Err(e) => log::warn!("{e:#}"),
        }
    }
    Some(Outcome::Text {
        text,
        info: engine_info(),
        millis,
    })
}

/// Lecture des codes : le premier est copié ; une adresse web s'ouvre dans le
/// navigateur (réglage, activé par défaut). La fenêtre ne s'affiche pas quand un
/// seul code a été ouvert : le navigateur suffit.
fn qr_flow(store: &Store, image: &RgbaImage) -> Option<Outcome> {
    let codes = decode_codes(image);
    let Some(first) = codes.first() else {
        return Some(Outcome::Codes {
            codes,
            opened: None,
            copied: false,
        });
    };
    let copied = match copy_text(&first.text) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("{e:#}");
            false
        }
    };
    let opened = first
        .web_url()
        .filter(|_| config_flag(store, QR_OPEN_BROWSER))
        .map(str::to_owned);
    if let Some(url) = &opened {
        open_url(url);
        if codes.len() == 1 {
            return None;
        }
    }
    Some(Outcome::Codes {
        codes,
        opened,
        copied,
    })
}

/// Lance la fenêtre de résultat dans un nouveau processus.
fn show(outcome: &Outcome) -> Result<()> {
    let exe = std::env::current_exe().context("chemin de l'exécutable")?;
    let mut child = Command::new(exe)
        .arg(WINDOW_ARG)
        .stdin(Stdio::piped())
        .spawn()
        .context("lancement de la fenêtre de résultat")?;
    let json = serde_json::to_vec(outcome)?;
    child
        .stdin
        .take()
        .context("entrée de la fenêtre de résultat")?
        .write_all(&json)
        .context("envoi du résultat à la fenêtre")?;
    Ok(())
}

/// `snappy --vision-window` : la fenêtre, avec le résultat lu sur l'entrée standard.
pub fn window() -> Result<()> {
    let mut json = Vec::new();
    std::io::Read::read_to_end(&mut std::io::stdin(), &mut json).context("lecture du résultat")?;
    let outcome: Outcome = serde_json::from_slice(&json).context("résultat illisible")?;
    window::run(outcome)
}

/// Réglage booléen de `config.json`, lu sans `Config` ; absent : `default`.
fn config_flag(store: &Store, (key, default): (&str, bool)) -> bool {
    std::fs::read(store.config_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|v| v.get(key)?.as_bool())
        .unwrap_or(default)
}

pub(crate) fn copy_text(text: &str) -> Result<()> {
    arboard::Clipboard::new()
        .context("presse-papiers indisponible")?
        .set_text(text)
        .context("copie du texte")
}

/// Ouvre une adresse http(s) dans le navigateur par défaut.
pub(crate) fn open_url(url: &str) {
    let Some(url) = codes::web_url(url) else {
        return;
    };
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(url).spawn();
    #[cfg(windows)]
    let result = Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let result = Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = result {
        log::warn!("ouverture de {url} impossible : {e}");
    }
}

// --- Ligne de commande (scripts, essais) ---

/// `snappy --ocr <png>` : texte reconnu sur la sortie standard, durée sur la sortie
/// d'erreur.
pub fn ocr_file(path: &Path) -> Result<()> {
    let img = history::read_png(path)?;
    let started = Instant::now();
    let lines = ocr(&img)?;
    let elapsed = started.elapsed();
    println!("{}", lines_text(&lines));
    eprintln!(
        "{} lignes, {}×{} px, {} ms ({})",
        lines.len(),
        img.width(),
        img.height(),
        elapsed.as_millis(),
        engine_info()
    );
    Ok(())
}

/// `snappy --qr <png>` : un code par ligne (symbologie, tabulation, contenu).
pub fn qr_file(path: &Path) -> Result<()> {
    let img = history::read_png(path)?;
    let codes = decode_codes(&img);
    if codes.is_empty() {
        eprintln!("aucun code trouvé");
    }
    for code in codes {
        println!("{}\t{}", code.kind, code.text);
    }
    Ok(())
}

/// `snappy --sensitive <png>` : zones sensibles (nature, x, y, largeur, hauteur).
pub fn sensitive_file(path: &Path) -> Result<()> {
    let img = history::read_png(path)?;
    let started = Instant::now();
    let hits = sensitive_regions(&img)?;
    for hit in &hits {
        let r = hit.rect;
        println!(
            "{}\t{}\t{}\t{}\t{}",
            hit.kind.label(),
            r.x,
            r.y,
            r.width,
            r.height
        );
    }
    eprintln!("{} zones, {} ms", hits.len(), started.elapsed().as_millis());
    Ok(())
}

/// `snappy --remove-background <png> <sortie.png>`.
pub fn remove_background_file(input: &Path, output: &Path) -> Result<()> {
    let img = history::read_png(input)?;
    let started = Instant::now();
    let out = remove_background(&img)?;
    history::write_png(output, &out)?;
    eprintln!(
        "{} ({} ms)",
        output.display(),
        started.elapsed().as_millis()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_flags_use_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert!(!config_flag(&store, OCR_COPY_WITHOUT_WINDOW));
        assert!(config_flag(&store, QR_OPEN_BROWSER));
        std::fs::write(
            store.config_path(),
            r#"{"ocr_copy_without_window": true, "qr_open_browser": false}"#,
        )
        .unwrap();
        assert!(config_flag(&store, OCR_COPY_WITHOUT_WINDOW));
        assert!(!config_flag(&store, QR_OPEN_BROWSER));
    }

    #[test]
    fn outcome_survives_the_trip_to_the_window() {
        let outcome = Outcome::Codes {
            codes: vec![Code {
                text: "https://exemple.fr".into(),
                kind: "QR".into(),
                rect: Rect::new(1, 2, 3, 4),
            }],
            opened: Some("https://exemple.fr".into()),
            copied: true,
        };
        let json = serde_json::to_vec(&outcome).unwrap();
        let back: Outcome = serde_json::from_slice(&json).unwrap();
        assert!(matches!(back, Outcome::Codes { codes, .. } if codes[0].rect.width == 3));
    }

    #[test]
    fn lines_are_joined() {
        let line = |t: &str| TextLine {
            text: t.into(),
            rect: Rect::default(),
            confidence: 1.0,
        };
        assert_eq!(lines_text(&[line("a"), line("b")]), "a\nb");
        assert_eq!(lines_text(&[]), "");
    }

    /// Lignes de texte noir sur blanc, dessinées avec Outfit.
    #[cfg(target_os = "macos")]
    fn text_image(lines: &[&str], size: f32) -> RgbaImage {
        use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
        let font = FontRef::try_from_slice(include_bytes!("../../assets/fonts/Outfit-Regular.ttf"))
            .unwrap();
        let scaled = font.as_scaled(PxScale::from(size));
        let line_h = (size * 1.6) as u32;
        let mut img = RgbaImage::from_pixel(
            1400,
            line_h * lines.len() as u32 + 40,
            image::Rgba([255, 255, 255, 255]),
        );
        for (i, text) in lines.iter().enumerate() {
            let mut x = 20.0;
            let baseline = 20.0 + (i as f32 + 1.0) * line_h as f32 - size * 0.4;
            for c in text.chars() {
                let glyph = scaled.scaled_glyph(c);
                let advance = scaled.h_advance(glyph.id);
                let glyph = ab_glyph::Glyph {
                    position: point(x, baseline),
                    ..glyph
                };
                if let Some(outlined) = font.outline_glyph(glyph) {
                    let b = outlined.px_bounds();
                    outlined.draw(|gx, gy, cov| {
                        let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                        if px >= 0
                            && py >= 0
                            && (px as u32) < img.width()
                            && (py as u32) < img.height()
                        {
                            let v = (255.0 * (1.0 - cov.clamp(0.0, 1.0))) as u8;
                            let p = img.get_pixel_mut(px as u32, py as u32);
                            p.0[..3].iter_mut().for_each(|c| *c = (*c).min(v));
                        }
                    });
                }
                x += advance;
            }
        }
        img
    }

    /// Vision sur une image de texte : lignes lues et zones sensibles à leur place.
    /// Lancer avec `cargo test -- --ignored vision` (moteur du système, ~1 s).
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "moteur Vision du système"]
    fn vision_reads_text_and_finds_sensitive_data() {
        let img = text_image(
            &[
                "Bonjour, voici le compte rendu de la réunion.",
                "Contact : jean.dupont@exemple.fr ou 06 12 34 56 78",
                "Serveur 192.168.10.20, carte 4111 1111 1111 1111",
            ],
            32.0,
        );
        let lines = ocr(&img).unwrap();
        let text = lines_text(&lines);
        assert!(text.contains("réunion"), "{text}");
        assert!(text.contains("jean.dupont@exemple.fr"), "{text}");
        let hits = sensitive_regions(&img).unwrap();
        let kinds: Vec<SensitiveKind> = hits.iter().map(|h| h.kind).collect();
        for kind in [
            SensitiveKind::Email,
            SensitiveKind::Phone,
            SensitiveKind::Ipv4,
            SensitiveKind::CreditCard,
        ] {
            assert!(kinds.contains(&kind), "{kind:?} absent de {hits:?}\n{text}");
        }
        // Le courriel est sur la 2e ligne, après « Contact : » (que Vision peut
        // coller au courriel : boîte au prorata dans le mot).
        let email = hits
            .iter()
            .find(|h| h.kind == SensitiveKind::Email)
            .unwrap();
        let line_h = (32.0 * 1.6) as u32;
        assert!(email.rect.y >= 20 + line_h - 10 && email.rect.bottom() <= 20 + 2 * line_h + 10);
        assert!(email.rect.x > 100, "{email:?}");
        assert!(email.rect.width < 380, "{email:?}");
    }
}
