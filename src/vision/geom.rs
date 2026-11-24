//! Rectangles en pixels et conversions communes aux moteurs d'analyse (pur, testé).

use std::ops::Range;

use serde::{Deserialize, Serialize};

/// Rectangle en pixels de l'image analysée, origine en haut à gauche.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

// Union et bornes : boîtes des mots de l'OCR de Windows.
#[cfg_attr(not(windows), allow(dead_code))]
impl Rect {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> u32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> u32 {
        self.y + self.height
    }

    /// Plus petit rectangle qui contient les deux.
    pub fn union(&self, other: &Rect) -> Rect {
        let (x, y) = (self.x.min(other.x), self.y.min(other.y));
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    /// Rectangle en coordonnées réelles (pixels), arrondi vers l'extérieur et rogné à
    /// l'image `width × height`.
    pub fn from_f64(x: f64, y: f64, w: f64, h: f64, width: u32, height: u32) -> Rect {
        let clamp = |v: f64, max: u32| v.clamp(0.0, f64::from(max));
        let (x0, y0) = (clamp(x.floor(), width), clamp(y.floor(), height));
        let (x1, y1) = (clamp((x + w).ceil(), width), clamp((y + h).ceil(), height));
        Rect::new(
            x0 as u32,
            y0 as u32,
            (x1 - x0).max(0.0) as u32,
            (y1 - y0).max(0.0) as u32,
        )
    }
}

/// Boîte normalisée de Vision (0…1, origine en bas à gauche) → pixels de l'image.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn normalized_to_px(nx: f64, ny: f64, nw: f64, nh: f64, width: u32, height: u32) -> Rect {
    let (w, h) = (f64::from(width), f64::from(height));
    Rect::from_f64(nx * w, (1.0 - ny - nh) * h, nw * w, nh * h, width, height)
}

/// Position en unités UTF-16 (celles de `NSString`) d'un indice d'octet de `text`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

/// Part de la boîte d'une ligne occupée par le passage `span` (octets de `text`),
/// au prorata du nombre de caractères : quand le moteur ne donne pas mieux.
pub fn split_line(line: Rect, text: &str, span: Range<usize>) -> Rect {
    let total = text.chars().count().max(1) as f64;
    let before = text[..span.start].chars().count() as f64;
    let inside = text[span].chars().count() as f64;
    let w = f64::from(line.width);
    let x0 = f64::from(line.x) + w * before / total;
    let x1 = f64::from(line.x) + w * (before + inside) / total;
    let x0f = x0.floor() as u32;
    Rect::new(
        x0f,
        line.y,
        (x1.ceil() as u32).saturating_sub(x0f).max(1),
        line.height,
    )
}

/// `span` élargi aux espaces qui l'entourent : le ou les mots qui le contiennent.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn word_around(text: &str, span: &Range<usize>) -> Range<usize> {
    let start = text[..span.start]
        .rfind(char::is_whitespace)
        .map_or(0, |i| {
            i + text[i..].chars().next().map_or(1, char::len_utf8)
        });
    let end = text[span.end..]
        .find(char::is_whitespace)
        .map_or(text.len(), |i| span.end + i);
    start..end
}

/// Union des boîtes des mots qui chevauchent `span` (mots repérés par leurs octets
/// dans le texte de la ligne). `None` : aucun mot.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn words_union(words: &[(Range<usize>, Rect)], span: &Range<usize>) -> Option<Rect> {
    words
        .iter()
        .filter(|(r, _)| r.start < span.end && span.start < r.end)
        .map(|(_, rect)| *rect)
        .reduce(|a, b| a.union(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_boxes_are_flipped_to_top_left() {
        // En bas à gauche chez Vision : le quart haut gauche de l'image.
        let r = normalized_to_px(0.0, 0.5, 0.5, 0.5, 200, 100);
        assert_eq!(r, Rect::new(0, 0, 100, 50));
        let r = normalized_to_px(0.25, 0.0, 0.5, 0.1, 200, 100);
        assert_eq!(r, Rect::new(50, 90, 100, 10));
    }

    #[test]
    fn boxes_are_clamped_to_the_image() {
        let r = normalized_to_px(-0.1, -0.1, 1.5, 1.5, 100, 100);
        assert_eq!(r, Rect::new(0, 0, 100, 100));
    }

    #[test]
    fn union_covers_both() {
        let a = Rect::new(10, 10, 10, 10);
        let b = Rect::new(30, 5, 5, 30);
        assert_eq!(a.union(&b), Rect::new(10, 5, 25, 30));
    }

    #[test]
    fn utf16_offsets_count_code_units() {
        let text = "é😀a";
        assert_eq!(utf16_offset(text, 0), 0);
        assert_eq!(utf16_offset(text, 2), 1);
        assert_eq!(utf16_offset(text, 6), 3);
        assert_eq!(utf16_offset(text, 7), 4);
    }

    #[test]
    fn proportional_split_follows_characters() {
        let line = Rect::new(100, 20, 200, 10);
        // 10 caractères : « 56789 » occupe la seconde moitié.
        let r = split_line(line, "01234 6789", 6..10);
        assert_eq!(r, Rect::new(220, 20, 80, 10));
        let r = split_line(line, "éé", 2..4);
        assert_eq!(r, Rect::new(200, 20, 100, 10));
    }

    #[test]
    fn span_is_widened_to_its_words() {
        let text = "Contact:jean@a.fr ou 06 12";
        assert_eq!(word_around(text, &(8..17)), 0..17);
        assert_eq!(word_around(text, &(21..26)), 21..26);
        assert_eq!(word_around("é ab", &(3..4)), 3..5);
        assert_eq!(word_around("x\u{a0}yz", &(3..4)), 3..5);
    }

    #[test]
    fn words_overlapping_a_span_are_merged() {
        let words = vec![
            (0..5, Rect::new(0, 0, 50, 10)),
            (6..10, Rect::new(60, 0, 40, 12)),
            (11..15, Rect::new(110, 0, 40, 10)),
        ];
        assert_eq!(
            words_union(&words, &(6..15)),
            Some(Rect::new(60, 0, 90, 12))
        );
        assert_eq!(words_union(&words, &(2..3)), Some(Rect::new(0, 0, 50, 10)));
        assert_eq!(words_union(&words, &(5..6)), None);
    }
}
