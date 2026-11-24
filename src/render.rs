//! Rendu des annotations sur l'image d'origine, avec tiny-skia.
//!
//! Un seul moteur pour l'écran et l'export : l'éditeur affiche exactement ce qui sera
//! copié ou enregistré. Le texte et les tampons sont tracés à partir des contours des
//! glyphes (ab_glyph : Outfit pour le texte, Lucide pour les tampons), avec
//! l'anticrénelage de tiny-skia.

use std::sync::LazyLock;

use ab_glyph::{Font, FontRef, GlyphId, OutlineCurve, PxScale, ScaleFont};
use image::RgbaImage;
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, IntSize, LineCap, LineJoin, Paint, PathBuilder, Pattern,
    Pixmap, PixmapPaint, Rect, SpreadMode, Stroke, Transform,
};

use crate::model::{
    Annotation, ArrowStyle, Bounds, Pt, Rgba, Shape, arrow_points, bend_control, bubble_box,
    step_radius, text_padding,
};

/// Police des textes et des pastilles (Outfit SemiBold).
static FONT: LazyLock<FontRef<'static>> = LazyLock::new(|| {
    FontRef::try_from_slice(include_bytes!("../assets/fonts/Outfit-SemiBold.ttf"))
        .expect("police Outfit embarquée valide")
});

/// Police des tampons (icônes Lucide, réduites à celles de l'interface).
static ICONS: LazyLock<FontRef<'static>> = LazyLock::new(|| {
    FontRef::try_from_slice(include_bytes!("../assets/fonts/Lucide.ttf"))
        .expect("police Lucide embarquée valide")
});

/// Interligne, en multiple de la taille de police.
const LINE_HEIGHT: f32 = 1.2;
/// Opacité du surligneur.
const HIGHLIGHT_ALPHA: u8 = 110;
/// Assombrissement autour du projecteur.
const SPOTLIGHT_ALPHA: u8 = 150;
/// Opacité de l'ombre portée.
const SHADOW_ALPHA: f32 = 0.45;

/// Largeur et hauteur d'un texte (plusieurs lignes possibles) à la taille `size`.
pub fn text_size(text: &str, size: f32) -> (f32, f32) {
    let font = FONT.as_scaled(PxScale::from(size));
    let lines: Vec<&str> = text.split('\n').collect();
    let width = lines
        .iter()
        .map(|line| line_width(&font, line))
        .fold(0.0, f32::max);
    (width, lines.len() as f32 * size * LINE_HEIGHT)
}

fn line_width<F: Font>(font: &impl ScaleFont<F>, line: &str) -> f32 {
    let mut width = 0.0;
    let mut prev = None;
    for c in line.chars() {
        let id = font.glyph_id(c);
        if let Some(prev) = prev {
            width += font.kern(prev, id);
        }
        width += font.h_advance(id);
        prev = Some(id);
    }
    width
}

/// Image d'origine + annotations, aplaties.
pub fn render(base: &RgbaImage, annotations: &[Annotation]) -> RgbaImage {
    let (w, h) = base.dimensions();
    let Some(mut pixmap) = to_pixmap(base) else {
        return base.clone();
    };
    // Image d'origine intacte, pour les loupes (elles grossissent la capture, pas
    // les annotations posées dessus).
    let needs_source = annotations
        .iter()
        .any(|a| matches!(a.shape, Shape::Magnifier { .. }));
    let source = needs_source.then(|| pixmap.clone());
    for ann in annotations {
        if ann.shadow && ann.can_have_shadow() {
            draw_shadow(&mut pixmap, ann);
        }
        draw(&mut pixmap, ann, source.as_ref(), Transform::identity());
    }
    let mut out = RgbaImage::new(w, h);
    for (dst, src) in out.pixels_mut().zip(pixmap.pixels()) {
        let c = src.demultiply();
        dst.0 = [c.red(), c.green(), c.blue(), c.alpha()];
    }
    out
}

fn to_pixmap(img: &RgbaImage) -> Option<Pixmap> {
    let size = IntSize::from_wh(img.width(), img.height())?;
    let mut data = img.as_raw().clone();
    // Prémultiplication (sans effet sur une capture, toujours opaque).
    for px in data.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        if a < 255 {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    Pixmap::from_vec(data, size)
}

fn paint(color: Rgba) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.0, color.1, color.2, color.3);
    paint.anti_alias = true;
    paint
}

fn stroke(width: f32) -> Stroke {
    Stroke {
        width: width.max(0.5),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

fn rect(b: Bounds) -> Option<Rect> {
    Rect::from_ltrb(b.min.x, b.min.y, b.max.x, b.max.y)
}

/// Rectangle aux coins arrondis (rayon borné à la moitié du petit côté).
fn rounded_rect(b: Bounds, radius: f32) -> Option<tiny_skia::Path> {
    let r = radius.min(b.width() / 2.0).min(b.height() / 2.0).max(0.0);
    if r <= 0.0 {
        return rect(b).map(PathBuilder::from_rect);
    }
    // Arc de quart de cercle par une cubique (constante classique).
    let k = r * 0.552_284_8;
    let (l, t, rt, bt) = (b.min.x, b.min.y, b.max.x, b.max.y);
    let mut pb = PathBuilder::new();
    pb.move_to(l + r, t);
    pb.line_to(rt - r, t);
    pb.cubic_to(rt - r + k, t, rt, t + r - k, rt, t + r);
    pb.line_to(rt, bt - r);
    pb.cubic_to(rt, bt - r + k, rt - r + k, bt, rt - r, bt);
    pb.line_to(l + r, bt);
    pb.cubic_to(l + r - k, bt, l, bt - r + k, l, bt - r);
    pb.line_to(l, t + r);
    pb.cubic_to(l, t + r - k, l + r - k, t, l + r, t);
    pb.close();
    pb.finish()
}

fn polyline(points: &[Pt]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let (first, rest) = points.split_first()?;
    pb.move_to(first.x, first.y);
    if rest.is_empty() {
        // Un seul point : un petit trait pour laisser une trace ronde.
        pb.line_to(first.x + 0.01, first.y);
    }
    for p in rest {
        pb.line_to(p.x, p.y);
    }
    pb.finish()
}

fn triangle(a: Pt, b: Pt, c: Pt) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(a.x, a.y);
    pb.line_to(b.x, b.y);
    pb.line_to(c.x, c.y);
    pb.close();
    pb.finish()
}

/// Dessine une annotation. `source` : l'image d'origine (loupes) ; `ts` : décalage
/// (dessin de l'ombre dans un tampon à part).
fn draw(pixmap: &mut Pixmap, ann: &Annotation, source: Option<&Pixmap>, ts: Transform) {
    let color = ann.color;
    let w = ann.width;
    match &ann.shape {
        Shape::Line { from, to } => {
            if let Some(path) = polyline(&[*from, *to]) {
                pixmap.stroke_path(&path, &paint(color), &stroke(w), ts, None);
            }
        }
        Shape::Arrow {
            from,
            to,
            style,
            bend,
        } => draw_arrow(pixmap, *from, *to, *style, *bend, color, w, ts),
        Shape::Rect { a, b, radius } => {
            if let Some(path) = rounded_rect(Bounds::from_corners(*a, *b), *radius) {
                if ann.fill {
                    pixmap.fill_path(&path, &paint(color), FillRule::Winding, ts, None);
                }
                let mut s = stroke(w);
                if *radius <= 0.0 {
                    s.line_cap = LineCap::Square;
                    s.line_join = LineJoin::Miter;
                }
                pixmap.stroke_path(&path, &paint(color), &s, ts, None);
            }
        }
        Shape::Ellipse { a, b } => {
            if let Some(path) = rect(Bounds::from_corners(*a, *b)).and_then(PathBuilder::from_oval)
            {
                if ann.fill {
                    pixmap.fill_path(&path, &paint(color), FillRule::Winding, ts, None);
                }
                pixmap.stroke_path(&path, &paint(color), &stroke(w), ts, None);
            }
        }
        Shape::Highlight { a, b } => {
            if let Some(r) = rect(Bounds::from_corners(*a, *b)) {
                let mut p = paint(Rgba(color.0, color.1, color.2, HIGHLIGHT_ALPHA));
                p.blend_mode = BlendMode::Multiply;
                pixmap.fill_rect(r, &p, ts, None);
            }
        }
        Shape::Spotlight { a, b } => {
            let (pw, ph) = (pixmap.width() as f32, pixmap.height() as f32);
            let hole = Bounds::from_corners(*a, *b);
            let mut pb = PathBuilder::new();
            if let Some(all) = Rect::from_xywh(0.0, 0.0, pw, ph) {
                pb.push_rect(all);
            }
            if let Some(hole) = rect(hole) {
                pb.push_rect(hole);
            }
            if let Some(path) = pb.finish() {
                let dim = paint(Rgba(0, 0, 0, SPOTLIGHT_ALPHA));
                pixmap.fill_path(&path, &dim, FillRule::EvenOdd, ts, None);
            }
        }
        Shape::Pixelate { a, b } => pixelate(pixmap, Bounds::from_corners(*a, *b), pixel_block(w)),
        Shape::Blur { a, b } => {
            let radius = blur_radius(w);
            for _ in 0..3 {
                box_blur(pixmap, Bounds::from_corners(*a, *b), radius);
            }
        }
        Shape::Pen { points } => {
            if let Some(path) = polyline(points) {
                pixmap.stroke_path(&path, &paint(color), &stroke(w), ts, None);
            }
        }
        Shape::Text {
            at,
            text,
            size,
            boxed: false,
        } => draw_text(pixmap, *at, text, *size, color, ts),
        Shape::Text {
            at,
            text,
            size,
            boxed: true,
        } => {
            let (tw, th) = text_size(text, *size);
            let (px, py) = text_padding(*size);
            let b = Bounds {
                min: at.offset(-px, -py),
                max: at.offset(tw + px, th + py),
            };
            if let Some(path) = rounded_rect(b, size * 0.3) {
                pixmap.fill_path(&path, &paint(color), FillRule::Winding, ts, None);
            }
            fill_text(pixmap, *at, text, *size, color.contrast(), ts);
        }
        Shape::Bubble {
            at,
            text,
            size,
            tail,
        } => {
            let b = bubble_box(*at, text, *size, &text_size);
            draw_pointer(
                pixmap,
                b.center(),
                *tail,
                (size * 0.9).min(b.width() / 3.0),
                color,
                ts,
            );
            if let Some(path) = rounded_rect(b, size * 0.45) {
                pixmap.fill_path(&path, &paint(color), FillRule::Winding, ts, None);
            }
            fill_text(pixmap, *at, text, *size, color.contrast(), ts);
        }
        Shape::Step { at, number, tail } => {
            let r = step_radius(w);
            if let Some(t) = tail {
                draw_pointer(pixmap, *at, *t, r * 1.1, color, ts);
            }
            if let Some(circle) = PathBuilder::from_circle(at.x, at.y, r) {
                pixmap.fill_path(&circle, &paint(color), FillRule::Winding, ts, None);
                // Liseré contrasté : la pastille reste visible sur un fond de même couleur.
                let ring = color.contrast();
                pixmap.stroke_path(
                    &circle,
                    &paint(Rgba(ring.0, ring.1, ring.2, 200)),
                    &stroke((r * 0.12).max(1.5)),
                    ts,
                    None,
                );
            }
            let label = number.to_string();
            let size = r * 1.15;
            let (tw, th) = text_size(&label, size);
            let text_at = Pt::new(at.x - tw / 2.0, at.y - th / 2.0);
            fill_text(pixmap, text_at, &label, size, color.contrast(), ts);
        }
        Shape::Magnifier {
            source: src,
            radius,
            at,
            zoom,
        } => draw_magnifier(pixmap, source, *src, *radius, *at, *zoom, color, w, ts),
        Shape::Stamp { at, icon, size } => draw_stamp(pixmap, *at, icon, *size, color, ts),
    }
}

/// Pointe pleine de `from` (large de `width`) vers `to` : queue des bulles et des
/// pastilles.
fn draw_pointer(pixmap: &mut Pixmap, from: Pt, to: Pt, width: f32, color: Rgba, ts: Transform) {
    let len = from.dist(to);
    if len < 1.0 {
        return;
    }
    let (ux, uy) = ((to.x - from.x) / len, (to.y - from.y) / len);
    let half = width / 2.0;
    let left = Pt::new(from.x - uy * half, from.y + ux * half);
    let right = Pt::new(from.x + uy * half, from.y - ux * half);
    if let Some(path) = triangle(left, to, right) {
        pixmap.fill_path(&path, &paint(color), FillRule::Winding, ts, None);
    }
}

/// Pointe pleine en `tip`, dans la direction (`ux`, `uy`), proportionnée au trait.
#[allow(clippy::too_many_arguments)]
fn arrow_head(
    pixmap: &mut Pixmap,
    tip: Pt,
    ux: f32,
    uy: f32,
    len: f32,
    color: Rgba,
    width: f32,
    ts: Transform,
) {
    let half = len * 0.6;
    let base = Pt::new(tip.x - ux * len, tip.y - uy * len);
    let left = Pt::new(base.x - uy * half, base.y + ux * half);
    let right = Pt::new(base.x + uy * half, base.y - ux * half);
    if let Some(path) = triangle(tip, left, right) {
        pixmap.fill_path(&path, &paint(color), FillRule::Winding, ts, None);
        pixmap.stroke_path(&path, &paint(color), &stroke(width * 0.5), ts, None);
    }
}

/// Direction unitaire de `a` vers `b`.
fn unit(a: Pt, b: Pt) -> Option<(f32, f32)> {
    let len = a.dist(b);
    (len > 1e-3).then(|| ((b.x - a.x) / len, (b.y - a.y) / len))
}

/// Retire `cut` pixels au bout d'une ligne brisée (et autant au début si `both`).
fn trim_end(points: &[Pt], cut: f32) -> Vec<Pt> {
    let mut pts = points.to_vec();
    let mut left = cut;
    while pts.len() >= 2 {
        let n = pts.len();
        let seg = pts[n - 2].dist(pts[n - 1]);
        if seg > left {
            pts[n - 1] = pts[n - 1].lerp(pts[n - 2], left / seg);
            break;
        }
        left -= seg;
        pts.pop();
    }
    pts
}

#[allow(clippy::too_many_arguments)]
fn draw_arrow(
    pixmap: &mut Pixmap,
    from: Pt,
    to: Pt,
    style: ArrowStyle,
    bend: Option<Pt>,
    color: Rgba,
    width: f32,
    ts: Transform,
) {
    let chord = from.dist(to);
    if chord < 0.5 {
        return;
    }
    if style == ArrowStyle::Big {
        draw_big_arrow(pixmap, from, to, color, width, ts);
        return;
    }
    let points = arrow_points(from, to, bend);
    // Directions des bouts, tangentes à la courbe.
    let control = bend.map(|b| bend_control(from, to, b));
    let end_dir = unit(control.unwrap_or(from), to);
    let start_dir = unit(control.unwrap_or(to), from);
    let head = (width * 4.0).max(12.0).min(chord * 0.5);
    let mut shaft = points.clone();
    let heads_at_start = style == ArrowStyle::Double;
    let heads_at_end = matches!(style, ArrowStyle::Simple | ArrowStyle::Double);
    // Le trait s'arrête dans la pointe, sans dépasser de son extrémité.
    if heads_at_end {
        shaft = trim_end(&shaft, head * 0.6);
    }
    if heads_at_start {
        shaft.reverse();
        shaft = trim_end(&shaft, head * 0.6);
        shaft.reverse();
    }
    if let Some(path) = polyline(&shaft) {
        pixmap.stroke_path(&path, &paint(color), &stroke(width), ts, None);
    }
    if heads_at_end && let Some((ux, uy)) = end_dir {
        arrow_head(pixmap, to, ux, uy, head, color, width, ts);
    }
    if heads_at_start && let Some((ux, uy)) = start_dir {
        arrow_head(pixmap, from, ux, uy, head, color, width, ts);
    }
    if style == ArrowStyle::Connector {
        let r = (width * 1.4).max(4.0);
        for p in [from, to] {
            if let Some(dot) = PathBuilder::from_circle(p.x, p.y, r) {
                pixmap.fill_path(&dot, &paint(color), FillRule::Winding, ts, None);
            }
        }
    }
}

/// Grosse flèche pleine, effilée vers la queue (toujours droite).
fn draw_big_arrow(pixmap: &mut Pixmap, from: Pt, to: Pt, color: Rgba, width: f32, ts: Transform) {
    let Some((ux, uy)) = unit(from, to) else {
        return;
    };
    let len = from.dist(to);
    let head = (width * 6.0).max(22.0).min(len * 0.6);
    let head_half = (width * 3.5).max(12.0);
    let neck_half = (width * 1.6).max(5.0);
    let tail_half = (width * 0.5).max(1.5);
    let at = |d: f32, side: f32| Pt::new(from.x + ux * d - uy * side, from.y + uy * d + ux * side);
    let neck = len - head;
    let outline = [
        at(0.0, tail_half),
        at(neck, neck_half),
        at(neck, head_half),
        to,
        at(neck, -head_half),
        at(neck, -neck_half),
        at(0.0, -tail_half),
    ];
    let mut pb = PathBuilder::new();
    pb.move_to(outline[0].x, outline[0].y);
    for p in &outline[1..] {
        pb.line_to(p.x, p.y);
    }
    pb.close();
    if let Some(path) = pb.finish() {
        pixmap.fill_path(&path, &paint(color), FillRule::Winding, ts, None);
        let mut s = stroke(1.0);
        s.line_join = LineJoin::Round;
        pixmap.stroke_path(&path, &paint(color), &s, ts, None);
    }
}

/// Loupe : disque qui montre l'image d'origine autour de `src`, grossie.
#[allow(clippy::too_many_arguments)]
fn draw_magnifier(
    pixmap: &mut Pixmap,
    source: Option<&Pixmap>,
    src: Pt,
    radius: f32,
    at: Pt,
    zoom: f32,
    color: Rgba,
    width: f32,
    ts: Transform,
) {
    let lens = radius * zoom;
    let thin = stroke((width * 0.5).max(1.0));
    // Repère sur la zone grossie, et lien jusqu'à la loupe.
    if let Some(ring) = PathBuilder::from_circle(src.x, src.y, radius) {
        pixmap.stroke_path(&ring, &paint(color), &thin, ts, None);
    }
    if let Some((ux, uy)) = unit(src, at)
        && src.dist(at) > radius + lens
    {
        let a = src.offset(ux * radius, uy * radius);
        let b = at.offset(-ux * lens, -uy * lens);
        if let Some(path) = polyline(&[a, b]) {
            pixmap.stroke_path(&path, &paint(color), &thin, ts, None);
        }
    }
    let Some(disc) = PathBuilder::from_circle(at.x, at.y, lens) else {
        return;
    };
    match source {
        Some(image) => {
            let zoomed = Transform::from_scale(zoom, zoom)
                .post_translate(at.x - src.x * zoom, at.y - src.y * zoom);
            let shader = Pattern::new(
                image.as_ref(),
                SpreadMode::Pad,
                FilterQuality::Bicubic,
                1.0,
                zoomed,
            );
            let lens_paint = Paint {
                shader,
                anti_alias: true,
                ..Paint::default()
            };
            pixmap.fill_path(&disc, &lens_paint, FillRule::Winding, ts, None);
        }
        // Dessin de l'ombre : seule la forme compte.
        None => pixmap.fill_path(&disc, &paint(color), FillRule::Winding, ts, None),
    }
    pixmap.stroke_path(&disc, &paint(color), &stroke(width), ts, None);
}

/// Tampon : une icône Lucide de côté `size`, centrée sur `at`, avec un liseré
/// contrasté pour rester lisible sur n'importe quel fond.
fn draw_stamp(pixmap: &mut Pixmap, at: Pt, icon: &str, size: f32, color: Rgba, ts: Transform) {
    let Some(c) = icon.chars().next() else { return };
    let id = ICONS.glyph_id(c);
    let Some(outline) = ICONS.outline(id) else {
        return;
    };
    let units = ICONS.units_per_em().unwrap_or(1000.0);
    let s = size / units;
    let b = outline.bounds;
    let (cx, cy) = ((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
    let mut pb = PathBuilder::new();
    push_outline(&mut pb, &outline.curves, |p| {
        (at.x + (p.x - cx) * s, at.y - (p.y - cy) * s)
    });
    if let Some(path) = pb.finish() {
        let halo = color.contrast();
        pixmap.stroke_path(
            &path,
            &paint(Rgba(halo.0, halo.1, halo.2, 230)),
            &stroke(size * 0.07),
            ts,
            None,
        );
        pixmap.fill_path(&path, &paint(color), FillRule::Winding, ts, None);
    }
}

/// Côté des blocs de la pixelisation pour une épaisseur de trait.
fn pixel_block(width: f32) -> u32 {
    (width * 3.0).round().clamp(6.0, 64.0) as u32
}

/// Rayon du flou (3 passes de flou par boîte ≈ flou gaussien).
fn blur_radius(width: f32) -> u32 {
    (width * 2.5).round().clamp(3.0, 60.0) as u32
}

/// Zone de `b` dans l'image, en pixels entiers bornés.
fn pixel_bounds(pixmap: &Pixmap, b: Bounds) -> (usize, usize, usize, usize) {
    let (pw, ph) = (pixmap.width() as f32, pixmap.height() as f32);
    let x0 = b.min.x.floor().clamp(0.0, pw) as usize;
    let y0 = b.min.y.floor().clamp(0.0, ph) as usize;
    let x1 = b.max.x.ceil().clamp(0.0, pw) as usize;
    let y1 = b.max.y.ceil().clamp(0.0, ph) as usize;
    (x0, y0, x1, y1)
}

/// Moyenne de chaque bloc de `block` pixels : le contenu devient illisible.
fn pixelate(pixmap: &mut Pixmap, b: Bounds, block: u32) {
    let (x0, y0, x1, y1) = pixel_bounds(pixmap, b);
    let block = block as usize;
    let stride = pixmap.width() as usize;
    let data = pixmap.data_mut();
    for by in (y0..y1).step_by(block) {
        let bh = block.min(y1 - by);
        for bx in (x0..x1).step_by(block) {
            let bw = block.min(x1 - bx);
            let mut sum = [0u64; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = (y * stride + x) * 4;
                    for (s, v) in sum.iter_mut().zip(&data[i..i + 4]) {
                        *s += u64::from(*v);
                    }
                }
            }
            let n = (bw * bh) as u64;
            let avg = sum.map(|s| (s / n) as u8);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = (y * stride + x) * 4;
                    data[i..i + 4].copy_from_slice(&avg);
                }
            }
        }
    }
}

/// Une passe de flou par boîte (horizontale puis verticale) sur la zone `b`, bords
/// répétés. Trois passes donnent un flou proche du gaussien.
fn box_blur(pixmap: &mut Pixmap, b: Bounds, radius: u32) {
    let (x0, y0, x1, y1) = pixel_bounds(pixmap, b);
    if x1 <= x0 || y1 <= y0 || radius == 0 {
        return;
    }
    let stride = pixmap.width() as usize;
    let r = radius as i64;
    let data = pixmap.data_mut();
    let mut line: Vec<[u32; 4]> = Vec::new();
    // Une ligne de pixels (horizontale ou verticale) floutée sur place.
    let mut blur_line = |data: &mut [u8], index: &dyn Fn(usize) -> usize, len: usize| {
        line.clear();
        line.extend((0..len).map(|k| {
            let i = index(k);
            [0, 1, 2, 3].map(|c| u32::from(data[i + c]))
        }));
        let at = |k: i64| line[k.clamp(0, len as i64 - 1) as usize];
        let mut sum = [0u32; 4];
        for k in -r..=r {
            let p = at(k);
            for c in 0..4 {
                sum[c] += p[c];
            }
        }
        let n = (2 * r + 1) as u32;
        for k in 0..len as i64 {
            let i = index(k as usize);
            for c in 0..4 {
                data[i + c] = (sum[c] / n) as u8;
            }
            let (add, sub) = (at(k + r + 1), at(k - r));
            for c in 0..4 {
                sum[c] = sum[c] + add[c] - sub[c];
            }
        }
    };
    for y in y0..y1 {
        blur_line(data, &|k| (y * stride + x0 + k) * 4, x1 - x0);
    }
    for x in x0..x1 {
        blur_line(data, &|k| ((y0 + k) * stride + x) * 4, y1 - y0);
    }
}

/// Ombre portée douce : l'annotation dessinée en noir translucide dans un tampon à
/// sa taille, floutée, puis posée un peu décalée sous l'annotation.
fn draw_shadow(pixmap: &mut Pixmap, ann: &Annotation) {
    let blur = (ann.width * 1.2).clamp(3.0, 24.0);
    let offset = (ann.width * 0.7).clamp(2.0, 14.0);
    let margin = ann.width * 2.0 + blur * 3.0 + 4.0;
    let b = ann.bounds(text_size).expand(margin);
    let (x0, y0) = (b.min.x.floor(), b.min.y.floor());
    let (w, h) = (
        (b.width().ceil() as u32).max(1),
        (b.height().ceil() as u32).max(1),
    );
    // Garde-fou : pas de tampon plus grand que l'image elle-même.
    if w > pixmap.width() * 2 || h > pixmap.height() * 2 {
        return;
    }
    let Some(mut layer) = Pixmap::new(w, h) else {
        return;
    };
    // Sans l'image d'origine : une loupe y est un simple disque, seule la forme compte.
    draw(&mut layer, ann, None, Transform::from_translate(-x0, -y0));
    // Noir, alpha réduit (prémultiplié : r = g = b = 0).
    for px in layer.data_mut().as_chunks_mut::<4>().0 {
        let a = (f32::from(px[3]) * SHADOW_ALPHA) as u8;
        *px = [0, 0, 0, a];
    }
    let all = Bounds {
        min: Pt::new(0.0, 0.0),
        max: Pt::new(w as f32, h as f32),
    };
    for _ in 0..3 {
        box_blur(&mut layer, all, (blur / 2.0) as u32);
    }
    pixmap.draw_pixmap(
        (x0 + offset) as i32,
        (y0 + offset) as i32,
        layer.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// Texte avec un contour de la couleur contrastée : lisible sur n'importe quel fond.
fn draw_text(pixmap: &mut Pixmap, at: Pt, text: &str, size: f32, color: Rgba, ts: Transform) {
    if let Some(path) = text_path(at, text, size) {
        let halo = color.contrast();
        pixmap.stroke_path(
            &path,
            &paint(Rgba(halo.0, halo.1, halo.2, 220)),
            &stroke((size * 0.12).max(1.5)),
            ts,
            None,
        );
        pixmap.fill_path(&path, &paint(color), FillRule::Winding, ts, None);
    }
}

fn fill_text(pixmap: &mut Pixmap, at: Pt, text: &str, size: f32, color: Rgba, ts: Transform) {
    if let Some(path) = text_path(at, text, size) {
        pixmap.fill_path(&path, &paint(color), FillRule::Winding, ts, None);
    }
}

/// Ajoute les contours d'un glyphe ; `map` passe des unités de la police aux pixels.
fn push_outline(
    pb: &mut PathBuilder,
    curves: &[OutlineCurve],
    map: impl Fn(ab_glyph::Point) -> (f32, f32),
) {
    let mut last: Option<ab_glyph::Point> = None;
    for curve in curves {
        let (start, end) = match curve {
            OutlineCurve::Line(a, b) => (*a, *b),
            OutlineCurve::Quad(a, _, b) => (*a, *b),
            OutlineCurve::Cubic(a, _, _, b) => (*a, *b),
        };
        // Nouveau contour quand la courbe ne reprend pas où la précédente s'arrête.
        if last != Some(start) {
            if last.is_some() {
                pb.close();
            }
            let (mx, my) = map(start);
            pb.move_to(mx, my);
        }
        match curve {
            OutlineCurve::Line(_, b) => {
                let (bx, by) = map(*b);
                pb.line_to(bx, by);
            }
            OutlineCurve::Quad(_, c, b) => {
                let ((cx, cy), (bx, by)) = (map(*c), map(*b));
                pb.quad_to(cx, cy, bx, by);
            }
            OutlineCurve::Cubic(_, c1, c2, b) => {
                let ((ax, ay), (cx, cy), (bx, by)) = (map(*c1), map(*c2), map(*b));
                pb.cubic_to(ax, ay, cx, cy, bx, by);
            }
        }
        last = Some(end);
    }
    if last.is_some() {
        pb.close();
    }
}

/// Contours des glyphes d'un texte, coin haut gauche en `at`.
fn text_path(at: Pt, text: &str, size: f32) -> Option<tiny_skia::Path> {
    let font = FONT.as_scaled(PxScale::from(size));
    let (sx, sy) = (font.h_scale_factor(), font.v_scale_factor());
    let mut pb = PathBuilder::new();
    for (row, line) in text.split('\n').enumerate() {
        let baseline = at.y
            + row as f32 * size * LINE_HEIGHT
            + font.ascent()
            + (size * LINE_HEIGHT - font.height()) / 2.0;
        let mut x = at.x;
        let mut prev: Option<GlyphId> = None;
        for c in line.chars() {
            let id = font.glyph_id(c);
            if let Some(prev) = prev {
                x += font.kern(prev, id);
            }
            if let Some(outline) = FONT.outline(id) {
                let left = x;
                push_outline(&mut pb, &outline.curves, |p| {
                    (left + p.x * sx, baseline - p.y * sy)
                });
            }
            x += font.h_advance(id);
            prev = Some(id);
        }
    }
    pb.finish()
}

#[cfg(test)]
mod tests {
    use image::Rgba as Px;

    use super::*;
    use crate::ui::icons;

    fn white(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Px([255, 255, 255, 255]))
    }

    fn ann(shape: Shape, color: Rgba) -> Annotation {
        Annotation::new(shape, color, 4.0)
    }

    const RED: Rgba = Rgba::rgb(255, 0, 0);
    const RED_PX: [u8; 4] = [255, 0, 0, 255];
    const WHITE_PX: [u8; 4] = [255, 255, 255, 255];

    fn arrow(from: Pt, to: Pt, style: ArrowStyle) -> Shape {
        Shape::Arrow {
            from,
            to,
            style,
            bend: None,
        }
    }

    fn reds(img: &RgbaImage) -> usize {
        img.pixels().filter(|p| p.0 == RED_PX).count()
    }

    #[test]
    fn no_annotation_keeps_the_image() {
        let mut img = white(20, 10);
        img.put_pixel(3, 4, Px([10, 20, 30, 255]));
        assert_eq!(render(&img, &[]), img);
    }

    #[test]
    fn rect_draws_its_border_not_its_inside_unless_filled() {
        let shape = Shape::Rect {
            a: Pt::new(10.0, 10.0),
            b: Pt::new(50.0, 50.0),
            radius: 0.0,
        };
        let out = render(&white(60, 60), &[ann(shape.clone(), RED)]);
        assert_eq!(out.get_pixel(10, 30).0, RED_PX);
        assert_eq!(out.get_pixel(30, 30).0, WHITE_PX);
        let mut filled = ann(shape, RED);
        filled.fill = true;
        assert_eq!(
            render(&white(60, 60), &[filled]).get_pixel(30, 30).0,
            RED_PX
        );
    }

    #[test]
    fn rounded_corners_stay_empty() {
        let shape = Shape::Rect {
            a: Pt::new(10.0, 10.0),
            b: Pt::new(90.0, 90.0),
            radius: 20.0,
        };
        let out = render(&white(100, 100), &[ann(shape, RED)]);
        assert_eq!(out.get_pixel(11, 11).0, WHITE_PX, "coin arrondi");
        assert_eq!(out.get_pixel(10, 50).0, RED_PX, "bord droit");
    }

    #[test]
    fn highlight_keeps_dark_text_dark() {
        let mut img = white(20, 20);
        img.put_pixel(5, 5, Px([0, 0, 0, 255]));
        let shape = Shape::Highlight {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(20.0, 20.0),
        };
        let out = render(&img, &[ann(shape, Rgba::rgb(255, 214, 10))]);
        assert_eq!(
            out.get_pixel(5, 5).0,
            [0, 0, 0, 255],
            "produit : le noir reste noir"
        );
        let tinted = out.get_pixel(15, 15).0;
        assert!(
            tinted[2] < 200 && tinted[0] == 255,
            "fond teinté en jaune : {tinted:?}"
        );
    }

    #[test]
    fn spotlight_darkens_only_the_outside() {
        let shape = Shape::Spotlight {
            a: Pt::new(20.0, 20.0),
            b: Pt::new(60.0, 60.0),
        };
        let out = render(&white(80, 80), &[ann(shape, RED)]);
        assert_eq!(out.get_pixel(40, 40).0, WHITE_PX);
        assert!(out.get_pixel(5, 5).0[0] < 150);
    }

    #[test]
    fn pixelate_averages_each_block() {
        let mut img = white(24, 12);
        for y in 0..12 {
            for x in 0..6 {
                img.put_pixel(x, y, Px([0, 0, 0, 255]));
            }
        }
        let shape = Shape::Pixelate {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(24.0, 12.0),
        };
        let out = render(&img, &[ann(shape, RED)]);
        // Épaisseur 4 → blocs de 12 : moitié noire, moitié blanche → gris uniforme.
        assert_eq!(out.get_pixel(0, 0), out.get_pixel(11, 11));
        assert_eq!(out.get_pixel(0, 0).0[0], 127);
        assert_eq!(out.get_pixel(12, 0).0, WHITE_PX);
    }

    #[test]
    fn blur_softens_edges_inside_its_zone_only() {
        let mut img = white(60, 20);
        for y in 0..20 {
            for x in 0..30 {
                img.put_pixel(x, y, Px([0, 0, 0, 255]));
            }
        }
        let shape = Shape::Blur {
            a: Pt::new(10.0, 0.0),
            b: Pt::new(50.0, 20.0),
        };
        let out = render(&img, &[ann(shape, RED)]);
        let edge = out.get_pixel(30, 10).0[0];
        assert!(edge > 60 && edge < 200, "bord adouci : {edge}");
        assert_eq!(out.get_pixel(5, 10).0, [0, 0, 0, 255], "hors zone");
        assert_eq!(out.get_pixel(55, 10).0, WHITE_PX, "hors zone");
    }

    #[test]
    fn effects_outside_the_image_are_ignored() {
        let img = white(8, 8);
        for shape in [
            Shape::Pixelate {
                a: Pt::new(-50.0, -50.0),
                b: Pt::new(-10.0, 400.0),
            },
            Shape::Blur {
                a: Pt::new(-50.0, -50.0),
                b: Pt::new(-10.0, 400.0),
            },
        ] {
            assert_eq!(render(&img, &[ann(shape, RED)]), img);
        }
    }

    #[test]
    fn text_is_drawn_inside_its_measured_box() {
        let size = 40.0;
        let (w, h) = text_size("Hé", size);
        assert!(w > 20.0 && w < 80.0, "largeur {w}");
        assert!((h - size * LINE_HEIGHT).abs() < 0.01);
        let at = Pt::new(10.0, 10.0);
        let shape = Shape::Text {
            at,
            text: "Hé".into(),
            size,
            boxed: false,
        };
        let out = render(&white(120, 80), &[ann(shape, RED)]);
        let mut n = 0;
        for (x, y, _) in out.enumerate_pixels().filter(|(_, _, p)| p.0 == RED_PX) {
            n += 1;
            assert!(
                x as f32 >= at.x - 1.0 && (x as f32) <= at.x + w + 1.0,
                "x={x}"
            );
            assert!(
                y as f32 >= at.y - 1.0 && (y as f32) <= at.y + h + 1.0,
                "y={y}"
            );
        }
        assert!(n > 50, "le texte doit être tracé ({n} pixels)");
    }

    #[test]
    fn boxed_text_and_bubbles_fill_their_box() {
        let text = Shape::Text {
            at: Pt::new(20.0, 20.0),
            text: "Ok".into(),
            size: 30.0,
            boxed: true,
        };
        let out = render(&white(140, 100), &[ann(text, RED)]);
        assert_eq!(out.get_pixel(14, 30).0, RED_PX, "marge du fond");
        let bubble = Shape::Bubble {
            at: Pt::new(20.0, 20.0),
            text: "Ok".into(),
            size: 30.0,
            tail: Pt::new(30.0, 140.0),
        };
        let out = render(&white(140, 160), &[ann(bubble, RED)]);
        assert_eq!(out.get_pixel(14, 30).0, RED_PX, "cadre de la bulle");
        assert_eq!(out.get_pixel(30, 125).0, RED_PX, "pointe de la bulle");
        assert_eq!(out.get_pixel(120, 140).0, WHITE_PX);
    }

    #[test]
    fn multiline_text_is_taller() {
        let (w1, h1) = text_size("abc", 20.0);
        let (w2, h2) = text_size("abc\nabcdef", 20.0);
        assert!(w2 > w1);
        assert!((h2 - 2.0 * h1).abs() < 0.01);
    }

    #[test]
    fn step_and_arrow_paint_their_color() {
        let step = ann(
            Shape::Step {
                at: Pt::new(30.0, 30.0),
                number: 7,
                tail: Some(Pt::new(30.0, 90.0)),
            },
            RED,
        );
        let a = ann(
            arrow(
                Pt::new(0.0, 110.0),
                Pt::new(60.0, 110.0),
                ArrowStyle::Simple,
            ),
            RED,
        );
        let out = render(&white(80, 120), &[step, a]);
        // Bord de la pastille (le centre porte le chiffre), pointe, milieu de la flèche.
        assert_eq!(out.get_pixel(30, 20).0, RED_PX);
        assert_eq!(out.get_pixel(30, 75).0, RED_PX, "pointe de la pastille");
        assert_eq!(out.get_pixel(30, 110).0, RED_PX);
        assert_eq!(out.get_pixel(70, 60).0, WHITE_PX);
    }

    #[test]
    fn arrow_styles_differ_where_expected() {
        let (from, to) = (Pt::new(10.0, 40.0), Pt::new(110.0, 40.0));
        let draw = |style| render(&white(120, 80), &[ann(arrow(from, to, style), RED)]);
        let simple = draw(ArrowStyle::Simple);
        let double = draw(ArrowStyle::Double);
        let big = draw(ArrowStyle::Big);
        let connector = draw(ArrowStyle::Connector);
        // Pointe au départ : seulement la double.
        assert_ne!(double.get_pixel(20, 35).0, WHITE_PX);
        assert_eq!(simple.get_pixel(20, 35).0, WHITE_PX);
        // La grosse flèche couvre bien plus de pixels.
        assert!(
            reds(&big) * 2 > reds(&simple) * 3,
            "{} contre {}",
            reds(&big),
            reds(&simple)
        );
        // Le connecteur a un rond au départ, pas de pointe à l'arrivée.
        assert_eq!(connector.get_pixel(10, 44).0, RED_PX);
        assert_eq!(connector.get_pixel(104, 33).0, WHITE_PX);
    }

    #[test]
    fn a_bent_arrow_goes_through_its_bend() {
        let shape = Shape::Arrow {
            from: Pt::new(10.0, 70.0),
            to: Pt::new(110.0, 70.0),
            style: ArrowStyle::Simple,
            bend: Some(Pt::new(60.0, 20.0)),
        };
        let out = render(&white(120, 90), &[ann(shape, RED)]);
        assert_eq!(out.get_pixel(60, 20).0, RED_PX);
        assert_eq!(out.get_pixel(60, 70).0, WHITE_PX);
    }

    #[test]
    fn magnifier_shows_the_original_zoomed() {
        let mut img = white(200, 100);
        // Un point noir de 4×4 en (20, 20) : grossi 3 fois dans la loupe en (140, 50).
        for y in 18..22 {
            for x in 18..22 {
                img.put_pixel(x, y, Px([0, 0, 0, 255]));
            }
        }
        let shape = Shape::Magnifier {
            source: Pt::new(20.0, 20.0),
            radius: 10.0,
            at: Pt::new(140.0, 50.0),
            zoom: 3.0,
        };
        let out = render(&img, &[ann(shape, RED)]);
        assert!(out.get_pixel(140, 50).0[0] < 60, "centre grossi noir");
        assert!(
            out.get_pixel(143, 50).0[0] < 60,
            "3 px plus loin, toujours noir (1 px de l'original)"
        );
        assert!(out.get_pixel(155, 50).0[0] > 200, "au-delà du point grossi");
        assert_eq!(out.get_pixel(140, 20).0, RED_PX, "bord de la loupe");
    }

    #[test]
    fn stamps_draw_their_icon() {
        let shape = Shape::Stamp {
            at: Pt::new(40.0, 40.0),
            icon: icons::STAMP_CHECK.into(),
            size: 60.0,
        };
        let out = render(&white(80, 80), &[ann(shape, RED)]);
        let n = reds(&out);
        assert!(n > 200, "icône tracée ({n} pixels)");
        assert_eq!(out.get_pixel(2, 2).0, WHITE_PX);
    }

    #[test]
    fn shadow_darkens_below_and_right_only() {
        let mut a = ann(
            Shape::Rect {
                a: Pt::new(20.0, 20.0),
                b: Pt::new(60.0, 60.0),
                radius: 0.0,
            },
            RED,
        );
        a.fill = true;
        a.shadow = true;
        let out = render(&white(100, 100), &[a]);
        assert!(out.get_pixel(62, 62).0[0] < 250, "ombre en bas à droite");
        assert_eq!(out.get_pixel(15, 15).0, WHITE_PX, "rien en haut à gauche");
        assert_eq!(out.get_pixel(40, 40).0, RED_PX, "l'objet reste dessus");
    }
}
