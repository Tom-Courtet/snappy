//! Mise en page du PDF : pages A4, texte coupé en lignes avec les vraies métriques
//! d'Outfit, images réduites à la largeur utile. Coordonnées en points, origine en
//! haut à gauche (le PDF les retourne à l'écriture).

use ab_glyph::{Font, FontRef};

use super::rich::{self, Piece};
use super::{DocItem, DocOptions};

pub const OUTFIT: &[u8] = include_bytes!("../../../assets/fonts/Outfit-Regular.ttf");
pub const OUTFIT_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Outfit-SemiBold.ttf");

/// A4 en points, portrait.
pub const A4: (f32, f32) = (595.28, 841.89);
pub const MARGIN: f32 = 50.0;

pub const TITLE_SIZE: f32 = 16.0;
pub const BODY_SIZE: f32 = 11.0;
const LEADING: f32 = 1.4;
/// Espace entre deux blocs d'une capture, et entre deux captures.
const BLOCK_GAP: f32 = 10.0;
const ITEM_GAP: f32 = 28.0;
/// Taille naturelle d'un pixel : 96 ppp.
const PT_PER_PX: f32 = 0.75;
/// Pente des italiques (Outfit n'a pas d'italique : oblique de synthèse).
pub const ITALIC_SKEW: f32 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Face {
    Regular,
    Bold,
}

/// Les deux graisses d'Outfit, pour mesurer le texte.
pub struct Metrics {
    regular: FontRef<'static>,
    bold: FontRef<'static>,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            regular: FontRef::try_from_slice(OUTFIT).expect("police Outfit embarquée"),
            bold: FontRef::try_from_slice(OUTFIT_SEMIBOLD).expect("police Outfit embarquée"),
        }
    }

    pub fn font(&self, face: Face) -> &FontRef<'static> {
        match face {
            Face::Regular => &self.regular,
            Face::Bold => &self.bold,
        }
    }

    /// Avance d'un glyphe en millièmes de cadratin (unité des largeurs PDF).
    pub fn advance_1000(&self, face: Face, c: char) -> f32 {
        let font = self.font(face);
        let upem = font.units_per_em().unwrap_or(1000.0);
        font.h_advance_unscaled(font.glyph_id(c)) * 1000.0 / upem
    }

    pub fn width(&self, face: Face, size: f32, text: &str) -> f32 {
        text.chars()
            .map(|c| self.advance_1000(face, c))
            .sum::<f32>()
            * size
            / 1000.0
    }
}

/// Un morceau de texte posé : `y` est la ligne de base.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub x: f32,
    pub y: f32,
    pub face: Face,
    pub size: f32,
    pub italic: bool,
    pub underline: bool,
    pub text: String,
    pub width: f32,
    /// Couleur du texte (titres en violet).
    pub accent: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Text(Span),
    /// Image `index` dans le rectangle (x, y en haut à gauche, w, h).
    Image {
        index: usize,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub width: f32,
    pub height: f32,
    pub ops: Vec<Op>,
}

/// Une ligne de texte : morceaux avec `x` relatif au début de la ligne.
type Line = Vec<Span>;

/// Coupe un paragraphe en lignes d'au plus `max_width` points.
pub fn wrap(
    pieces: &[Piece],
    base: Face,
    size: f32,
    max_width: f32,
    accent: bool,
    metrics: &Metrics,
) -> Vec<Line> {
    let mut lines: Vec<Line> = vec![Vec::new()];
    let mut line_width = 0.0f32;
    let mut pending_space = false;
    for piece in pieces {
        let run = match piece {
            Piece::Break => {
                lines.push(Vec::new());
                line_width = 0.0;
                pending_space = false;
                continue;
            }
            Piece::Run(run) => run,
        };
        let face = if run.bold { Face::Bold } else { base };
        let mut words = Vec::new();
        let mut word = String::new();
        for c in run.text.chars() {
            if c == ' ' || c == '\t' {
                if !word.is_empty() {
                    words.push((std::mem::take(&mut word), pending_space));
                }
                pending_space = true;
            } else {
                word.push(c);
            }
        }
        if !word.is_empty() {
            words.push((word, pending_space));
            pending_space = false;
        }
        for (word, space_before) in words {
            let line = lines.last_mut().expect("au moins une ligne");
            let space = if space_before && !line.is_empty() {
                metrics.width(face, size, " ")
            } else {
                0.0
            };
            let w = metrics.width(face, size, &word);
            let breakable = space_before || line.is_empty();
            if breakable && !line.is_empty() && line_width + space + w > max_width {
                lines.push(Vec::new());
                line_width = 0.0;
                place_word(
                    &mut lines,
                    &mut line_width,
                    word,
                    face,
                    size,
                    run,
                    accent,
                    0.0,
                    max_width,
                    metrics,
                );
            } else {
                place_word(
                    &mut lines,
                    &mut line_width,
                    word,
                    face,
                    size,
                    run,
                    accent,
                    space,
                    max_width,
                    metrics,
                );
            }
        }
    }
    // Pas de ligne vide finale (texte terminé par un retour à la ligne).
    while lines.len() > 1 && lines.last().is_some_and(Vec::is_empty) {
        lines.pop();
    }
    lines
}

/// Pose un mot au bout de la dernière ligne ; un mot plus long qu'une ligne entière
/// est coupé entre deux caractères.
#[allow(clippy::too_many_arguments)]
fn place_word(
    lines: &mut Vec<Line>,
    line_width: &mut f32,
    word: String,
    face: Face,
    size: f32,
    run: &rich::Run,
    accent: bool,
    space: f32,
    max_width: f32,
    metrics: &Metrics,
) {
    let mut chunks = Vec::new();
    if *line_width == 0.0 && metrics.width(face, size, &word) > max_width {
        let mut chunk = String::new();
        for c in word.chars() {
            let mut next = chunk.clone();
            next.push(c);
            if !chunk.is_empty() && metrics.width(face, size, &next) > max_width {
                chunks.push(std::mem::take(&mut chunk));
                chunk.push(c);
            } else {
                chunk = next;
            }
        }
        chunks.push(chunk);
    } else {
        chunks.push(word);
    }
    for (i, chunk) in chunks.into_iter().enumerate() {
        if i > 0 {
            lines.push(Vec::new());
            *line_width = 0.0;
        }
        let line = lines.last_mut().expect("au moins une ligne");
        let space = if i == 0 { space } else { 0.0 };
        let w = metrics.width(face, size, &chunk);
        let same_style = line.last().is_some_and(|s: &Span| {
            s.face == face && s.italic == run.italic && s.underline == run.underline
        });
        match line.last_mut() {
            Some(last) if same_style => {
                if space > 0.0 {
                    last.text.push(' ');
                }
                last.text.push_str(&chunk);
                last.width += space + w;
            }
            _ => line.push(Span {
                x: *line_width + space,
                y: 0.0,
                face,
                size,
                italic: run.italic,
                underline: run.underline,
                text: chunk,
                width: w,
                accent,
            }),
        }
        *line_width += space + w;
    }
}

/// Bloc d'une capture : des lignes de texte ou l'image.
enum Block {
    Text { lines: Vec<Line>, size: f32 },
    Image { index: usize, w: f32, h: f32 },
}

impl Block {
    fn height(&self) -> f32 {
        match self {
            Block::Text { lines, size } => lines.len() as f32 * size * LEADING,
            Block::Image { h, .. } => *h,
        }
    }
}

/// Page en paysage pour cette image ?
pub fn wants_landscape(options: &DocOptions, (w, h): (u32, u32)) -> bool {
    options.landscape_for_large && w > h && w as f32 * PT_PER_PX > A4.0 - 2.0 * MARGIN
}

struct Builder {
    pages: Vec<Page>,
    y: f32,
}

impl Builder {
    fn page(&mut self) -> &mut Page {
        self.pages.last_mut().expect("une page ouverte")
    }

    fn new_page(&mut self, landscape: bool) {
        let (w, h) = if landscape { (A4.1, A4.0) } else { A4 };
        self.pages.push(Page {
            width: w,
            height: h,
            ops: Vec::new(),
        });
        self.y = MARGIN;
    }

    fn bottom(&self) -> f32 {
        self.pages.last().map_or(0.0, |p| p.height - MARGIN)
    }

    fn landscape(&self) -> bool {
        self.pages.last().is_some_and(|p| p.width > p.height)
    }

    fn at_top(&self) -> bool {
        self.y <= MARGIN
    }

    /// Nouvelle page si `h` ne tient plus (sauf en haut d'une page vide).
    fn ensure(&mut self, h: f32) {
        if self.y + h > self.bottom() && !self.at_top() {
            let landscape = self.landscape();
            self.new_page(landscape);
        }
    }

    fn text(&mut self, lines: Vec<Line>, size: f32) {
        let line_h = size * LEADING;
        for line in lines {
            self.ensure(line_h);
            let baseline = self.y + size;
            for mut span in line {
                span.x += MARGIN;
                span.y = baseline;
                self.page().ops.push(Op::Text(span));
            }
            self.y += line_h;
        }
    }

    fn image(&mut self, index: usize, w: f32, h: f32) {
        self.ensure(h);
        let y = self.y;
        self.page().ops.push(Op::Image {
            index,
            x: MARGIN,
            y,
            w,
            h,
        });
        self.y += h;
    }
}

/// Met en page toutes les captures. `sizes` : dimensions des images, en pixels.
pub fn layout(
    items: &[DocItem],
    sizes: &[(u32, u32)],
    options: &DocOptions,
    metrics: &Metrics,
) -> Vec<Page> {
    let mut b = Builder {
        pages: Vec::new(),
        y: MARGIN,
    };
    for (index, (item, &(px_w, px_h))) in items.iter().zip(sizes).enumerate() {
        let landscape = wants_landscape(options, (px_w, px_h));
        let (page_w, page_h) = if landscape { (A4.1, A4.0) } else { A4 };
        let content_w = page_w - 2.0 * MARGIN;
        let content_h = page_h - 2.0 * MARGIN;

        let title = item.display_title(options);
        let title_lines = if title.is_empty() {
            Vec::new()
        } else {
            let pieces = [Piece::Run(rich::Run {
                text: title,
                bold: true,
                italic: false,
                underline: false,
            })];
            wrap(&pieces, Face::Bold, TITLE_SIZE, content_w, true, metrics)
        };
        let mut text_lines = Vec::new();
        if !item.description.trim().is_empty() {
            text_lines.extend(wrap(
                &rich::parse(&item.description),
                Face::Regular,
                BODY_SIZE,
                content_w,
                false,
                metrics,
            ));
        }
        for (number, step) in item.steps() {
            let mut pieces = vec![Piece::Run(rich::Run {
                text: format!("{number}. "),
                bold: true,
                italic: false,
                underline: false,
            })];
            pieces.extend(rich::parse(step));
            text_lines.extend(wrap(
                &pieces,
                Face::Regular,
                BODY_SIZE,
                content_w,
                false,
                metrics,
            ));
        }
        let natural = (px_w as f32 * PT_PER_PX, px_h as f32 * PT_PER_PX);
        let title_h = title_lines.len() as f32 * TITLE_SIZE * LEADING;
        // L'image tient sur une page avec le titre et le texte (sauf saut de page
        // entre les deux), sans descendre sous la moitié de la hauteur utile.
        let text_h = if options.page_break {
            0.0
        } else {
            text_lines.len() as f32 * BODY_SIZE * LEADING + BLOCK_GAP
        };
        let room = (content_h - title_h - BLOCK_GAP - text_h).max(content_h / 2.0);
        let scale = 1f32
            .min(content_w / natural.0.max(1.0))
            .min(room / natural.1.max(1.0));
        let image = Block::Image {
            index,
            w: natural.0 * scale,
            h: natural.1 * scale,
        };
        let text = Block::Text {
            lines: text_lines,
            size: BODY_SIZE,
        };
        let has_text = text.height() > 0.0;
        let blocks = if options.image_before_text {
            [image, text]
        } else {
            [text, image]
        };

        // Nouvelle page : première capture, changement d'orientation, ou capture qui
        // ne tient pas dans la place restante.
        let total = title_h
            + blocks.iter().map(Block::height).sum::<f32>()
            + BLOCK_GAP * (1.0 + f32::from(has_text));
        if b.pages.is_empty() || b.landscape() != landscape {
            b.new_page(landscape);
        } else {
            b.y += ITEM_GAP;
            if b.y + total > b.bottom() {
                b.new_page(landscape);
            }
        }
        if !title_lines.is_empty() {
            b.text(title_lines, TITLE_SIZE);
            b.y += BLOCK_GAP * 0.6;
        }
        let mut first = true;
        for block in blocks {
            if block.height() <= 0.0 {
                continue;
            }
            if !first {
                if options.page_break && has_text {
                    b.new_page(landscape);
                } else {
                    b.y += BLOCK_GAP;
                }
            }
            first = false;
            match block {
                Block::Text { lines, size } => b.text(lines, size),
                Block::Image { index, w, h } => b.image(index, w, h),
            }
        }
    }
    b.pages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_wraps_within_the_width() {
        let m = Metrics::new();
        let pieces = rich::parse(&"mot ".repeat(200));
        let lines = wrap(&pieces, Face::Regular, 11.0, 300.0, false, &m);
        assert!(lines.len() > 5);
        for line in &lines {
            let end = line.iter().map(|s| s.x + s.width).fold(0.0, f32::max);
            assert!(end <= 300.0 + 0.01, "{end}");
        }
    }

    #[test]
    fn styles_split_spans_and_breaks_make_lines() {
        let m = Metrics::new();
        let lines = wrap(
            &rich::parse("a <b>b</b> c<br>d"),
            Face::Regular,
            11.0,
            500.0,
            false,
            &m,
        );
        assert_eq!(lines.len(), 2);
        let texts: Vec<_> = lines[0].iter().map(|s| (s.text.as_str(), s.face)).collect();
        assert_eq!(
            texts,
            vec![
                ("a", Face::Regular),
                ("b", Face::Bold),
                ("c", Face::Regular)
            ]
        );
        assert!(lines[0][1].x > lines[0][0].x + lines[0][0].width);
    }

    #[test]
    fn very_long_words_are_cut() {
        let m = Metrics::new();
        let lines = wrap(
            &rich::parse(&"x".repeat(500)),
            Face::Regular,
            11.0,
            100.0,
            false,
            &m,
        );
        assert!(lines.len() > 3);
    }

    #[test]
    fn wide_images_get_landscape_pages() {
        let o = DocOptions::default();
        assert!(wants_landscape(&o, (2000, 900)));
        assert!(!wants_landscape(&o, (500, 300)));
        assert!(!wants_landscape(&o, (900, 2000)));
        let o = DocOptions {
            landscape_for_large: false,
            ..o
        };
        assert!(!wants_landscape(&o, (2000, 900)));
    }

    #[test]
    fn small_items_share_a_page_and_images_fit() {
        let m = Metrics::new();
        let items: Vec<DocItem> = (0..3)
            .map(|i| DocItem {
                image_path: format!("/x/c{i}.png").into(),
                ..DocItem::default()
            })
            .collect();
        let sizes = [(200, 100); 3];
        let pages = layout(&items, &sizes, &DocOptions::default(), &m);
        assert_eq!(pages.len(), 1);
        let pages = layout(&items, &[(3000, 6000); 3], &DocOptions::default(), &m);
        assert_eq!(pages.len(), 3);
        for page in &pages {
            for op in &page.ops {
                if let Op::Image { x, y, w, h, .. } = op {
                    assert!(x + w <= page.width - MARGIN + 0.01);
                    assert!(y + h <= page.height - MARGIN + 0.01);
                }
            }
        }
    }
}
