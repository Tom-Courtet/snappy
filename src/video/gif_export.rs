//! Export GIF animé, en pur Rust : crate `gif` et sa quantification NeuQuant (une
//! palette de 256 couleurs par image). Les images viennent du lecteur (AVFoundation
//! sous macOS) ; ce module ne fait que le minutage et l'encodage.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))] // lecteur propre à macOS

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use image::RgbaImage;

/// Cadences proposées (images par seconde) : Screenpresso réduit à environ 5.
pub const FPS_CHOICES: &[u32] = &[5, 10, 15, 20];

/// Instants de la vidéo source (en secondes) à reprendre dans le GIF pour l'extrait
/// `start..end` joué à `speed` (2 : deux fois plus vite) et `fps` images par seconde
/// de GIF.
pub fn frame_times(start: f64, end: f64, fps: u32, speed: f64) -> Vec<f64> {
    if end <= start || fps == 0 || speed <= 0.0 || end.is_nan() || speed.is_nan() {
        return Vec::new();
    }
    let step = speed / f64::from(fps);
    // Marge : 1,0 / 0,2 ne doit pas donner 5,000…1 donc 6 images.
    let count = ((end - start) / step - 1e-9).ceil().max(1.0) as usize;
    (0..count).map(|i| start + i as f64 * step).collect()
}

/// Durée d'affichage d'une image, en centièmes de seconde (unité du GIF). Les
/// navigateurs ralentissent les durées inférieures à 2 centièmes.
pub fn delay_centis(fps: u32) -> u16 {
    (100.0 / f64::from(fps.max(1))).round().clamp(2.0, 6553.0) as u16
}

/// Écrit un GIF qui boucle sans fin. `frames` donne les images une à une (toutes de
/// la même taille), `progress` reçoit le nombre d'images écrites. Fichier écrit à
/// côté puis renommé : jamais de GIF à moitié écrit sous le nom final.
pub fn write_gif(
    path: &Path,
    fps: u32,
    frames: impl IntoIterator<Item = Result<RgbaImage>>,
    mut progress: impl FnMut(usize),
) -> Result<()> {
    let tmp = PathBuf::from(format!("{}.part", path.display()));
    let result = (|| {
        let file = File::create(&tmp).with_context(|| format!("création de {}", tmp.display()))?;
        let mut encoder: Option<gif::Encoder<BufWriter<File>>> = None;
        let mut file = Some(BufWriter::new(file));
        let mut size = (0u16, 0u16);
        let mut count = 0;
        for frame in frames {
            let mut img = frame?;
            let (w, h) = (
                u16::try_from(img.width()).context("image trop large pour un GIF")?,
                u16::try_from(img.height()).context("image trop haute pour un GIF")?,
            );
            ensure!(w > 0 && h > 0, "image vide");
            if encoder.is_none() {
                size = (w, h);
                let mut e = gif::Encoder::new(file.take().expect("une fois"), w, h, &[])
                    .context("en-tête GIF")?;
                e.set_repeat(gif::Repeat::Infinite)?;
                encoder = Some(e);
            }
            ensure!((w, h) == size, "images de tailles différentes");
            // Vitesse 10 : bon compromis qualité / temps de NeuQuant (1 = lent).
            let mut gif_frame = gif::Frame::from_rgba_speed(w, h, img.as_mut(), 10);
            gif_frame.delay = delay_centis(fps);
            encoder
                .as_mut()
                .expect("créé plus haut")
                .write_frame(&gif_frame)
                .context("écriture d'une image GIF")?;
            count += 1;
            progress(count);
        }
        ensure!(count > 0, "aucune image à écrire");
        drop(encoder);
        Ok(())
    })();
    match result {
        Ok(()) => fs::rename(&tmp, path).with_context(|| format!("écriture de {}", path.display())),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_times_follow_fps_and_speed() {
        let close = |a: Vec<f64>, b: &[f64]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
        };
        assert!(close(
            frame_times(0.0, 1.0, 5, 1.0),
            &[0.0, 0.2, 0.4, 0.6, 0.8]
        ));
        assert!(close(
            frame_times(2.0, 4.0, 5, 2.0),
            &[2.0, 2.4, 2.8, 3.2, 3.6]
        ));
        assert!(frame_times(1.0, 1.0, 10, 1.0).is_empty());
        assert!(
            close(frame_times(0.0, 0.01, 10, 1.0), &[0.0]),
            "au moins une image"
        );
    }

    #[test]
    fn delays_are_in_centiseconds() {
        assert_eq!(delay_centis(5), 20);
        assert_eq!(delay_centis(10), 10);
        assert_eq!(delay_centis(15), 7);
        assert_eq!(delay_centis(100), 2);
    }

    #[test]
    fn synthetic_frames_make_a_valid_gif() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.gif");
        let frames = (0..4u8).map(|i| {
            Ok(RgbaImage::from_fn(32, 16, move |x, y| {
                image::Rgba([x as u8 * 8, y as u8 * 16, i * 60, 255])
            }))
        });
        let mut seen = 0;
        write_gif(&path, 10, frames, |n| seen = n).unwrap();
        assert_eq!(seen, 4);
        assert!(!dir.path().join("test.gif.part").exists());
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options.read_info(File::open(&path).unwrap()).unwrap();
        assert_eq!((decoder.width(), decoder.height()), (32, 16));
        let mut decoded = 0;
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            assert_eq!(frame.delay, 10);
            assert_eq!((frame.width, frame.height), (32, 16));
            decoded += 1;
        }
        assert_eq!(decoded, 4);
    }

    #[test]
    fn mixed_sizes_and_empty_input_fail_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.gif");
        let frames = vec![Ok(RgbaImage::new(4, 4)), Ok(RgbaImage::new(5, 4))];
        assert!(write_gif(&path, 10, frames, |_| {}).is_err());
        assert!(!path.exists() && !dir.path().join("bad.gif.part").exists());
        assert!(write_gif(&path, 10, Vec::new(), |_| {}).is_err());
    }
}
