//! Copie répétée de la zone, plusieurs fois par seconde.
//!
//! Plutôt que `capture::capture_all` (tous les écrans, la liste des fenêtres et le
//! curseur, puis une découpe), on ne copie que la zone :
//! - macOS : `CGDisplayCreateImage`, dont seule la zone est convertie en sRGB
//!   (comme les captures) ;
//! - Windows : `BitBlt` du rectangle, en pixels physiques (sans `CAPTUREBLT`, qui
//!   fait clignoter le curseur à chaque copie).
//!
//! Repli (autres systèmes, échec de la copie directe, ou `SNAPPY_SCROLL_GRAB=full`
//! pour comparer) : `capture_all` puis découpe.

use anyhow::{Context, Result};
use image::RgbaImage;

use super::Zone;

/// Copie la zone : pixels de l'écran, sRGB, opaques, sans le curseur.
pub struct Grabber {
    zone: Zone,
    full: bool,
}

impl Grabber {
    pub fn new(zone: Zone) -> Self {
        let full = std::env::var("SNAPPY_SCROLL_GRAB").is_ok_and(|v| v == "full")
            || !cfg!(any(target_os = "macos", windows));
        Self { zone, full }
    }

    pub fn grab(&mut self) -> Result<RgbaImage> {
        if !self.full {
            match platform::grab(&self.zone) {
                Ok(img) => return Ok(img),
                Err(e) => {
                    log::warn!(
                        "copie directe de la zone impossible, repli sur la capture complète : {e:#}"
                    );
                    self.full = true;
                }
            }
        }
        grab_full(&self.zone)
    }
}

/// Capture de tous les écrans, puis découpe de la zone.
fn grab_full(zone: &Zone) -> Result<RgbaImage> {
    let capture = crate::capture::capture_all()?;
    let shot = capture
        .shots
        .iter()
        .find(|s| s.display_id == zone.display_id)
        .context("l'écran de la zone a disparu")?;
    let (w, h) = shot.image.dimensions();
    anyhow::ensure!(
        zone.x + zone.width <= w && zone.y + zone.height <= h,
        "la définition de l'écran a changé"
    );
    Ok(shot.region(zone.x, zone.y, zone.width, zone.height, false))
}

#[cfg(target_os = "macos")]
mod platform {
    use anyhow::{Context, Result, ensure};
    use image::RgbaImage;
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBlendMode, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
        CGImageByteOrderInfo, CGInterpolationQuality, kCGColorSpaceSRGB,
    };

    use super::Zone;

    /// `CGDisplayCreateImage` (~15 ms pour un écran Retina) puis une sous-image
    /// sans copie, et seule la zone est convertie en sRGB (~2 à 7 ms).
    /// `CGDisplayCreateImageForRect` serait plus direct mais ne rend plus rien
    /// sous macOS 26, et `CGWindowListCreateImage` coûte ~100 ms à convertir
    /// (l'image est recomposée au moment du dessin).
    #[allow(deprecated)] // obsolète depuis le SDK 15, toujours là (voir `capture`)
    pub fn grab(zone: &Zone) -> Result<RgbaImage> {
        use objc2_core_graphics::CGDisplayCreateImage;
        let screen = CGDisplayCreateImage(zone.display_id)
            .context("copie de l'écran refusée par le système")?;
        let rect = CGRect::new(
            CGPoint::new(f64::from(zone.x), f64::from(zone.y)),
            CGSize::new(f64::from(zone.width), f64::from(zone.height)),
        );
        let part = CGImage::with_image_in_rect(Some(&screen), rect)
            .context("zone hors de l'écran (définition changée ?)")?;
        to_srgb(&part, zone.width as usize, zone.height as usize)
    }

    /// Redessine l'image dans un tampon RGBA 8 bits sRGB de la taille demandée
    /// (la même que la source, à l'arrondi près), opaque.
    fn to_srgb(source: &CGImage, width: usize, height: usize) -> Result<RgbaImage> {
        ensure!(width > 0 && height > 0, "zone vide");
        let stride = width * 4;
        let mut pixels = vec![0u8; stride * height];
        let same_size =
            CGImage::width(Some(source)) == width && CGImage::height(Some(source)) == height;
        // SAFETY : constante fournie par CoreGraphics.
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
            .context("espace sRGB indisponible")?;
        let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
        // SAFETY : `pixels` fait `stride * height` octets et survit au contexte,
        // détruit à la fin de cette fonction.
        let ctx = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                width,
                height,
                8,
                stride,
                Some(&srgb),
                info,
            )
        }
        .context("création du contexte bitmap sRGB")?;
        CGContext::set_blend_mode(Some(&ctx), CGBlendMode::Copy);
        CGContext::set_interpolation_quality(
            Some(&ctx),
            if same_size {
                CGInterpolationQuality::None
            } else {
                CGInterpolationQuality::High
            },
        );
        let rect = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(width as f64, height as f64),
        );
        CGContext::draw_image(Some(&ctx), rect, Some(source));
        drop(ctx);
        for px in pixels.as_chunks_mut::<4>().0 {
            px[3] = u8::MAX;
        }
        RgbaImage::from_raw(width as u32, height as u32, pixels).context("tampon incohérent")
    }
}

#[cfg(windows)]
mod platform {
    use std::mem::size_of;
    use std::ptr::null_mut;

    use anyhow::{Result, bail, ensure};
    use image::RgbaImage;
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
    };
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
    };

    use super::Zone;

    pub fn grab(zone: &Zone) -> Result<RgbaImage> {
        let (left, top) = zone.physical_origin();
        let (w, h) = (zone.width as i32, zone.height as i32);
        ensure!(w > 0 && h > 0, "zone vide");
        // SAFETY : réglage du fil le temps de la copie, rétabli ensuite ; chaque
        // objet GDI créé est libéré avant de sortir ; le tampon a la taille
        // demandée à GetDIBits.
        unsafe {
            let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let result = (|| {
                let screen = GetDC(null_mut());
                ensure!(!screen.is_null(), "contexte de l'écran indisponible");
                let mem = CreateCompatibleDC(screen);
                let bitmap = CreateCompatibleBitmap(screen, w, h);
                if mem.is_null() || bitmap.is_null() {
                    if !bitmap.is_null() {
                        DeleteObject(bitmap);
                    }
                    if !mem.is_null() {
                        DeleteDC(mem);
                    }
                    ReleaseDC(null_mut(), screen);
                    bail!("mémoire GDI insuffisante pour {w}x{h} px");
                }
                let old = SelectObject(mem, bitmap);
                let copied = BitBlt(mem, 0, 0, w, h, screen, left, top, SRCCOPY) != 0;
                SelectObject(mem, old);
                let mut info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut buf = vec![0u8; w as usize * h as usize * 4];
                let lines = GetDIBits(
                    mem,
                    bitmap,
                    0,
                    h as u32,
                    buf.as_mut_ptr().cast(),
                    &mut info,
                    DIB_RGB_COLORS,
                );
                DeleteObject(bitmap);
                DeleteDC(mem);
                ReleaseDC(null_mut(), screen);
                ensure!(copied, "copie de l'écran refusée par GDI (BitBlt)");
                ensure!(lines == h, "lecture de la copie refusée par GDI");
                for px in buf.as_chunks_mut::<4>().0 {
                    px.swap(0, 2);
                    px[3] = u8::MAX;
                }
                RgbaImage::from_raw(w as u32, h as u32, buf)
                    .ok_or_else(|| anyhow::anyhow!("taille d'image incohérente"))
            })();
            if !previous.is_null() {
                SetThreadDpiAwarenessContext(previous);
            }
            result
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use anyhow::{Result, bail};
    use image::RgbaImage;

    use super::Zone;

    pub fn grab(_zone: &Zone) -> Result<RgbaImage> {
        bail!("copie directe indisponible sur ce système")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vraie copie (autorisation « Enregistrement de l'écran », écran allumé) :
    /// la copie directe donne les mêmes pixels que la capture complète, et coûte
    /// moins cher. `cargo test grab_matches -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn grab_matches_the_full_capture() {
        let capture = crate::capture::capture_all().expect("capture");
        let shot = &capture.shots[capture.active];
        let (w, h) = shot.image.dimensions();
        let zone = Zone {
            display_id: shot.display_id,
            frame: shot.frame,
            scale: shot.scale,
            x: w / 4,
            y: h / 4,
            width: w / 2,
            height: h / 2,
        };
        let n = 20;
        let start = std::time::Instant::now();
        let mut img = platform::grab(&zone).expect("copie directe");
        for _ in 1..n {
            img = platform::grab(&zone).expect("copie directe");
        }
        let direct_ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
        let start = std::time::Instant::now();
        let mut full = grab_full(&zone).unwrap();
        for _ in 1..n {
            full = grab_full(&zone).unwrap();
        }
        let full_ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
        eprintln!(
            "zone {}x{} px : copie directe {direct_ms:.1} ms, capture complète + découpe {full_ms:.1} ms",
            zone.width, zone.height
        );
        assert_eq!(img.dimensions(), full.dimensions());
        // L'écran a pu changer entre les deux (horloge…) : presque tout identique.
        let same = img
            .pixels()
            .zip(full.pixels())
            .filter(|(a, b)| a == b)
            .count();
        let total = (img.width() * img.height()) as usize;
        assert!(same * 100 >= total * 95, "{same}/{total} pixels identiques");
    }
}
