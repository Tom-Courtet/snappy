//! QR codes en pur Rust (`rqrr`), sur toutes les plateformes. Sous macOS, Vision
//! complète la liste avec les autres symbologies (EAN, Code 128, Data Matrix…).

use image::RgbaImage;
use serde::{Deserialize, Serialize};

use super::geom::Rect;

/// Code lu dans l'image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Code {
    /// Contenu décodé.
    pub text: String,
    /// Symbologie : « QR », « EAN13 », « Code128 »…
    pub kind: String,
    /// Position dans l'image.
    pub rect: Rect,
}

impl Code {
    /// Adresse web ouvrable dans le navigateur (seulement http et https : un code
    /// peut contenir n'importe quel schéma).
    pub fn web_url(&self) -> Option<&str> {
        web_url(&self.text)
    }
}

/// `text` s'il s'agit d'une adresse http(s) sans espace.
pub fn web_url(text: &str) -> Option<&str> {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    let ok = (lower.starts_with("https://") || lower.starts_with("http://"))
        && t.len() > "http://".len() + 1
        && !t.chars().any(char::is_whitespace);
    ok.then_some(t)
}

/// QR codes de l'image, décodés par `rqrr`.
pub fn decode_qr(img: &RgbaImage) -> Vec<Code> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
        let p = img.get_pixel(x as u32, y as u32).0;
        // Composé sur blanc, luminance Rec. 601.
        let a = u32::from(p[3]);
        let mix = |c: u8| (u32::from(c) * a + 255 * (255 - a)) / 255;
        ((299 * mix(p[0]) + 587 * mix(p[1]) + 114 * mix(p[2])) / 1000) as u8
    });
    let mut codes: Vec<Code> = Vec::new();
    for grid in prepared.detect_grids() {
        let Ok((_, text)) = grid.decode() else {
            continue;
        };
        let xs = grid.bounds.iter().map(|p| p.x);
        let ys = grid.bounds.iter().map(|p| p.y);
        let (x0, x1) = (xs.clone().min().unwrap_or(0), xs.max().unwrap_or(0));
        let (y0, y1) = (ys.clone().min().unwrap_or(0), ys.max().unwrap_or(0));
        let rect = Rect::from_f64(
            f64::from(x0),
            f64::from(y0),
            f64::from(x1 - x0),
            f64::from(y1 - y0),
            w as u32,
            h as u32,
        );
        push_unique(
            &mut codes,
            Code {
                text,
                kind: "QR".into(),
                rect,
            },
        );
    }
    codes
}

/// Ajoute `code` s'il n'est pas déjà dans la liste (même contenu).
pub fn push_unique(codes: &mut Vec<Code>, code: Code) {
    if !codes.iter().any(|c| c.text == code.text) {
        codes.push(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_urls_are_opened() {
        assert_eq!(
            web_url(" https://exemple.fr/a?b=c "),
            Some("https://exemple.fr/a?b=c")
        );
        assert_eq!(web_url("HTTP://EXEMPLE.FR"), Some("HTTP://EXEMPLE.FR"));
        assert_eq!(web_url("file:///etc/passwd"), None);
        assert_eq!(web_url("javascript:alert(1)"), None);
        assert_eq!(web_url("https://"), None);
        assert_eq!(web_url("https://a b"), None);
        assert_eq!(web_url("WIFI:S:maison;T:WPA;P:secret;;"), None);
    }

    #[test]
    fn duplicates_are_dropped() {
        let mut codes = Vec::new();
        let code = |kind: &str| Code {
            text: "x".into(),
            kind: kind.into(),
            rect: Rect::default(),
        };
        push_unique(&mut codes, code("QR"));
        push_unique(&mut codes, code("QR"));
        assert_eq!(codes.len(), 1);
    }

    #[test]
    fn blank_image_has_no_code() {
        let img = RgbaImage::from_pixel(64, 64, image::Rgba([255, 255, 255, 255]));
        assert!(decode_qr(&img).is_empty());
        assert!(decode_qr(&RgbaImage::new(0, 0)).is_empty());
    }
}
