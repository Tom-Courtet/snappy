//! Moteur Windows 10 : `Windows.Media.Ocr` (hors ligne, langues des modules
//! « Reconnaissance optique de caractères » installés dans Windows).
//!
//! Windows donne la boîte de chaque mot mais pas d'indice de confiance (1.0).
//! Pas de détourage ni de lecture de codes-barres ici (QR : `rqrr`).

use std::ops::Range;

use anyhow::{Context, Result, anyhow, bail};
use image::RgbaImage;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

use super::geom::{self, Rect};
use super::{Recognized, TextLine};

/// Ligne lue par Windows : texte (mots séparés par une espace) et boîte de chaque mot.
struct WinLine {
    text: String,
    words: Vec<(Range<usize>, Rect)>,
}

fn engine() -> Result<OcrEngine> {
    OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| {
        anyhow!(
            "aucune langue d'OCR installée dans Windows (Paramètres > Heure et langue > \
             Langue : module « Reconnaissance optique de caractères »)"
        )
    })
}

/// Moteur et langue, pour la fenêtre de résultat.
pub fn engine_info() -> String {
    let language = std::thread::spawn(|| -> Option<String> {
        let lang = engine().ok()?.RecognizerLanguage().ok()?;
        Some(lang.DisplayName().ok()?.to_string())
    })
    .join()
    .ok()
    .flatten();
    match language {
        Some(l) => format!("OCR de Windows · {l}"),
        None => "OCR de Windows".into(),
    }
}

/// Texte de l'image, ligne par ligne, avec la boîte de chaque passage demandé par
/// `spans_of` : union des boîtes des mots qu'il touche.
pub fn recognize<T>(
    img: &RgbaImage,
    spans_of: impl Fn(&str) -> Vec<(T, Range<usize>)>,
) -> Result<Vec<Recognized<T>>> {
    let owned = img.clone();
    // Fil à part (MTA) : l'attente d'une opération WinRT sur le fil de l'interface
    // (STA) peut bloquer.
    let lines = std::thread::spawn(move || read_lines(&owned))
        .join()
        .map_err(|_| anyhow!("OCR de Windows interrompu"))??;
    Ok(lines
        .into_iter()
        .filter_map(|l| {
            let rect = l.words.iter().map(|(_, r)| *r).reduce(|a, b| a.union(&b))?;
            let spans = spans_of(&l.text)
                .into_iter()
                .map(|(tag, span)| {
                    let r = geom::words_union(&l.words, &span)
                        .unwrap_or_else(|| geom::split_line(rect, &l.text, span));
                    (tag, r)
                })
                .collect();
            Some(Recognized {
                line: TextLine {
                    text: l.text,
                    rect,
                    confidence: 1.0,
                },
                spans,
            })
        })
        .collect())
}

fn read_lines(img: &RgbaImage) -> Result<Vec<WinLine>> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        bail!("image vide");
    }
    let engine = engine()?;
    // Windows lit mal le petit texte : agrandi jusqu'à ×2, sans dépasser la taille
    // maximale acceptée par le moteur.
    let max = OcrEngine::MaxImageDimension().unwrap_or(2600).max(1) as f64;
    let scale = (max / f64::from(w.max(h))).min(2.0);
    let scaled;
    let src = if (scale - 1.0).abs() > 0.01 {
        let (sw, sh) = (
            ((f64::from(w) * scale).round() as u32).max(1),
            ((f64::from(h) * scale).round() as u32).max(1),
        );
        scaled = image::imageops::resize(img, sw, sh, image::imageops::FilterType::Triangle);
        &scaled
    } else {
        img
    };
    // BGRA posé sur du blanc.
    let mut bgra = Vec::with_capacity(src.as_raw().len());
    for p in src.pixels() {
        let a = u32::from(p.0[3]);
        let mix = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
        bgra.extend_from_slice(&[mix(p.0[2]), mix(p.0[1]), mix(p.0[0]), 255]);
    }
    let writer = DataWriter::new()?;
    writer.WriteBytes(&bgra)?;
    let buffer = writer.DetachBuffer()?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        src.width() as i32,
        src.height() as i32,
    )
    .context("image pour l'OCR de Windows")?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let back = |v: f32| f64::from(v) / scale;
    let mut lines = Vec::new();
    for line in result.Lines()? {
        let mut text = String::new();
        let mut words = Vec::new();
        for word in line.Words()? {
            let t = word.Text()?.to_string();
            let r = word.BoundingRect()?;
            if !text.is_empty() {
                text.push(' ');
            }
            let start = text.len();
            text.push_str(&t);
            words.push((
                start..text.len(),
                Rect::from_f64(back(r.X), back(r.Y), back(r.Width), back(r.Height), w, h),
            ));
        }
        if !words.is_empty() {
            lines.push(WinLine { text, words });
        }
    }
    Ok(lines)
}
