//! Émojis en couleur pour les tampons « Réactions » (lot « Aperçus et tampons »).
//!
//! Rien n'est téléchargé : sous macOS, l'émoji est dessiné par CoreText avec la police
//! du système (Apple Color Emoji) dans un contexte bitmap, puis rendu comme une image.
//! Ailleurs, [`rasterize`] rend `None` et le moteur de rendu dessine un cadre à la place
//! (la catégorie « Réactions » n'est pas proposée sous Windows).
//!
//! Ce fichier n'est pas dans la liste du test des glyphes (`src/ui/mod.rs`) : les
//! émojis ne passent jamais par le texte d'egui, ils sont toujours dessinés en images.

use image::RgbaImage;

/// Émojis proposés : (émoji, nom affiché en infobulle).
pub const POSITIVE: [(&str, &str); 11] = [
    ("\u{1F44D}", "Pouce levé"),
    ("\u{2764}\u{FE0F}", "Cœur"),
    ("\u{1F606}", "Rire"),
    ("\u{1F60A}", "Sourire"),
    ("\u{1F60D}", "Coup de cœur"),
    ("\u{1F602}", "Larmes de joie"),
    ("\u{1F605}", "Soulagement"),
    ("\u{270C}\u{FE0F}", "Victoire"),
    ("\u{1F60E}", "Cool"),
    ("\u{1F642}", "Content"),
    ("\u{1F603}", "Ravi"),
];

pub const NEGATIVE: [(&str, &str); 10] = [
    ("\u{1F44E}", "Pouce baissé"),
    ("\u{1F494}", "Cœur brisé"),
    ("\u{1F92E}", "Dégoût"),
    ("\u{1F633}", "Gêne"),
    ("\u{1F622}", "Tristesse"),
    ("\u{1F621}", "Colère"),
    ("\u{1F4A9}", "Crotte"),
    ("\u{1F62D}", "Pleurs"),
    ("\u{1F641}", "Déception"),
    ("\u{1F61F}", "Inquiétude"),
];

pub const SIGNS: [(&str, &str); 8] = [
    ("\u{2705}", "Validé"),
    ("\u{274C}", "Refusé"),
    ("\u{26A0}\u{FE0F}", "Attention"),
    ("\u{2B50}", "Étoile"),
    ("\u{1F525}", "Urgent"),
    ("\u{1F4A1}", "Idée"),
    ("\u{1F449}", "Ici"),
    ("\u{1F440}", "À regarder"),
];

/// Nom d'un émoji de la liste (infobulles), sinon « Émoji ».
pub fn name(emoji: &str) -> &'static str {
    POSITIVE
        .iter()
        .chain(&NEGATIVE)
        .chain(&SIGNS)
        .find(|e| e.0 == emoji)
        .map_or("Émoji", |e| e.1)
}

/// Les émojis en couleur sont-ils dessinés sur ce système ?
pub const AVAILABLE: bool = cfg!(target_os = "macos");

/// Plus grand côté dessiné (au-delà, l'image est agrandie par le rendu).
pub const MAX_SIDE: u32 = 512;

/// L'émoji `text` dessiné dans un carré de `side` pixels (fond transparent, alpha non
/// prémultiplié), centré. `None` hors de macOS, ou si rien n'a pu être dessiné.
pub fn rasterize(text: &str, side: u32) -> Option<RgbaImage> {
    let side = side.clamp(4, MAX_SIDE);
    if text.is_empty() {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        macos::rasterize(text, side)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = side;
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use image::RgbaImage;
    use objc2_core_foundation::{
        CFAttributedString, CFDictionary, CFRetained, CFString, CFType, CGAffineTransform, CGFloat,
    };
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo, CGImageByteOrderInfo,
        kCGColorSpaceSRGB,
    };
    use objc2_core_text::{CTFont, CTLine, kCTFontAttributeName};

    /// Ligne CoreText de `text` en Apple Color Emoji à la taille `size`.
    fn line(text: &str, size: CGFloat) -> Option<CFRetained<CTLine>> {
        let name = CFString::from_str("AppleColorEmoji");
        // SAFETY : `matrix` nul est permis (matrice identité).
        let font = unsafe { CTFont::with_name(&name, size, std::ptr::null()) };
        // SAFETY : constante fournie par CoreText.
        let key: &CFString = unsafe { kCTFontAttributeName };
        let font_ref: &CFType = &font;
        let attrs = CFDictionary::<CFString, CFType>::from_slices(&[key], &[font_ref]);
        let string = CFString::from_str(text);
        // SAFETY : chaîne et attributs valides ; le dictionnaire est de type
        // CFString → CFType, comme l'attend CoreText.
        let attributed =
            unsafe { CFAttributedString::new(None, Some(&string), Some(attrs.as_opaque())) }?;
        // SAFETY : chaîne attribuée valide.
        Some(unsafe { CTLine::with_attributed_string(&attributed) })
    }

    /// Largeur, montée et descente typographiques d'une ligne.
    fn metrics(line: &CTLine) -> (CGFloat, CGFloat, CGFloat) {
        let (mut ascent, mut descent, mut leading) = (0.0, 0.0, 0.0);
        // SAFETY : pointeurs vers des variables locales valides.
        let width = unsafe { line.typographic_bounds(&mut ascent, &mut descent, &mut leading) };
        (width, ascent, descent)
    }

    pub fn rasterize(text: &str, side: u32) -> Option<RgbaImage> {
        let px = CGFloat::from(side);
        // Première mesure à la taille du carré, puis taille ajustée pour qu'il y tienne.
        let probe = line(text, px)?;
        let (w, a, d) = metrics(&probe);
        let extent = w.max(a + d);
        if extent <= 0.0 {
            return None;
        }
        let size = px * 0.94 * px / extent;
        let line = line(text, size)?;
        let (w, a, d) = metrics(&line);

        let (width, height) = (side as usize, side as usize);
        let stride = width * 4;
        let mut pixels = vec![0u8; stride * height];
        // SAFETY : constante fournie par CoreGraphics.
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
        // R, G, B, A en mémoire, alpha prémultiplié.
        let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
        // SAFETY : `pixels` fait `stride * height` octets et survit au contexte, détruit
        // à la fin de ce bloc.
        {
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
            }?;
            CGContext::set_should_antialias(Some(&ctx), true);
            CGContext::set_text_matrix(
                Some(&ctx),
                CGAffineTransform {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    tx: 0.0,
                    ty: 0.0,
                },
            );
            // Origine en bas à gauche : ligne de base placée pour centrer l'émoji.
            let x = (px - w) / 2.0;
            let y = (px - (a + d)) / 2.0 + d;
            CGContext::set_text_position(Some(&ctx), x, y);
            // SAFETY : ligne et contexte valides.
            unsafe { line.draw(&ctx) };
        }
        // Mémoire de haut en bas (la première rangée est le haut de l'image) ;
        // alpha prémultiplié → non prémultiplié.
        for px in pixels.as_chunks_mut::<4>().0 {
            let alpha = px[3];
            if alpha != 0 && alpha != 255 {
                for c in &mut px[..3] {
                    *c = ((u16::from(*c) * 255 + u16::from(alpha) / 2) / u16::from(alpha)).min(255)
                        as u8;
                }
            }
        }
        let img = RgbaImage::from_raw(side, side, pixels)?;
        img.pixels().any(|p| p[3] > 0).then_some(img)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_emoji_has_a_french_name() {
        for (emoji, label) in POSITIVE.iter().chain(&NEGATIVE).chain(&SIGNS) {
            assert_eq!(name(emoji), *label);
            assert!(label.is_ascii() || label.chars().all(|c| !is_emoji_char(c)));
        }
        assert_eq!(name("?"), "Émoji");
    }

    fn is_emoji_char(c: char) -> bool {
        u32::from(c) >= 0x2600
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_draws_color_emoji() {
        let img = rasterize("\u{1F44D}", 64).expect("émoji dessiné");
        assert_eq!(img.dimensions(), (64, 64));
        let opaque = img.pixels().filter(|p| p[3] > 200).count();
        assert!(opaque > 64 * 64 / 5, "assez de pixels couvrants : {opaque}");
        // En couleur : au moins un pixel nettement coloré (pas un glyphe gris).
        assert!(
            img.pixels().any(|p| p[3] > 200 && p[0].abs_diff(p[2]) > 60),
            "émoji en couleur"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_systems_have_no_emoji() {
        assert!(rasterize("\u{1F44D}", 64).is_none());
    }
}
