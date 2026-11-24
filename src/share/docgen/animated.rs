//! GIF animé : une image par capture, centrée sur un fond blanc de la taille de la
//! plus grande (réduite à [`MAX_SIDE`] pixels de côté), en boucle.

use anyhow::{Context, Result, bail};
use image::imageops::{self, FilterType};
use image::{Rgba, RgbaImage};

/// Plus grand côté du GIF : au-delà, le fichier devient énorme pour peu de gain.
pub const MAX_SIDE: u32 = 1200;

pub fn write(images: &[RgbaImage], delay_ms: u32) -> Result<Vec<u8>> {
    let Some((w, h)) = canvas(images) else {
        bail!("aucune image pour le GIF");
    };
    let delay = u16::try_from(delay_ms.div_ceil(10))
        .unwrap_or(u16::MAX)
        .max(1);
    let mut out = Vec::new();
    {
        let mut encoder =
            gif::Encoder::new(&mut out, w as u16, h as u16, &[]).context("encodage GIF")?;
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .context("encodage GIF")?;
        for img in images {
            let mut frame_img = fit(img, w, h);
            let mut frame = gif::Frame::from_rgba_speed(w as u16, h as u16, frame_img.as_mut(), 10);
            frame.delay = delay;
            encoder.write_frame(&frame).context("encodage GIF")?;
        }
    }
    Ok(out)
}

/// Taille commune : la plus grande largeur et la plus grande hauteur, réduites.
fn canvas(images: &[RgbaImage]) -> Option<(u32, u32)> {
    let w = images.iter().map(RgbaImage::width).max()?;
    let h = images.iter().map(RgbaImage::height).max()?;
    let scale = (MAX_SIDE as f32 / w.max(h) as f32).min(1.0);
    Some((
        ((w as f32 * scale).round() as u32).clamp(1, u32::from(u16::MAX)),
        ((h as f32 * scale).round() as u32).clamp(1, u32::from(u16::MAX)),
    ))
}

/// L'image réduite pour tenir dans `w`×`h`, centrée sur du blanc (le GIF n'a pas
/// de transparence partielle).
fn fit(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let scale = (w as f32 / img.width() as f32)
        .min(h as f32 / img.height() as f32)
        .min(1.0);
    let sw = ((img.width() as f32 * scale).round() as u32).max(1);
    let sh = ((img.height() as f32 * scale).round() as u32).max(1);
    let scaled = if (sw, sh) == img.dimensions() {
        img.clone()
    } else {
        imageops::resize(img, sw, sh, FilterType::Triangle)
    };
    let mut canvas = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
    let x = i64::from((w - sw) / 2);
    let y = i64::from((h - sh) / 2);
    imageops::overlay(&mut canvas, &scaled, x, y);
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_is_the_largest_image_capped() {
        let a = RgbaImage::new(300, 100);
        let b = RgbaImage::new(100, 200);
        assert_eq!(canvas(&[a.clone(), b]), Some((300, 200)));
        let big = RgbaImage::new(4800, 1200);
        assert_eq!(canvas(&[big, a]), Some((1200, 300)));
        assert_eq!(canvas(&[]), None);
    }

    #[test]
    fn frames_are_centered_on_white() {
        let img = RgbaImage::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
        let f = fit(&img, 30, 10);
        assert_eq!(f.get_pixel(0, 5), &Rgba([255, 255, 255, 255]));
        assert_eq!(f.get_pixel(15, 5), &Rgba([255, 0, 0, 255]));
    }
}
