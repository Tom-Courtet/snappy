//! Rendu des annotations sur l'image d'origine, avec tiny-skia.
//!
//! Un seul moteur pour l'écran et l'export : l'éditeur affiche exactement ce qui sera
//! copié ou enregistré. Le texte et les tampons sont tracés à partir des contours des
//! glyphes (ab_glyph : Outfit pour le texte, Lucide pour les tampons), avec
//! l'anticrénelage de tiny-skia.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use ab_glyph::{Font, FontRef, GlyphId, OutlineCurve, PxScale, ScaleFont};
use image::RgbaImage;
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, IntSize, LineCap, LineJoin, Paint, PathBuilder, Pattern,
    Pixmap, PixmapPaint, Rect, SpreadMode, Stroke, StrokeDash, Transform,
};

// Lot « Aperçus et tampons » : tampons dessinés en couleur.
pub(crate) mod stamp_art;

use crate::model::{
    Annotation, ArrowStyle, Bounds, BracketSide, BubbleKind, Doc, EndShape, LineStyle, Pt, Rgba,
    Shape, StepFormat, StepTail, TextAlign, TextStyle, arrow_points, bend_control, bubble_box,
    label_box, pin_tip, step_label, step_radius, text_padding,
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

/// Image d'origine + annotations, aplaties. Les images insérées
/// ([`Shape::Image`]) n'y sont que des cadres : voir [`render_with`].
pub fn render(base: &RgbaImage, annotations: &[Annotation]) -> RgbaImage {
    render_with(base, annotations, None)
}

/// Comme [`render`], avec le dossier des images insérées (`<capture>.assets/`, voir
/// [`assets_dir`]).
pub fn render_with(
    base: &RgbaImage,
    annotations: &[Annotation],
    assets: Option<&Path>,
) -> RgbaImage {
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
        // Opacité : l'annotation est posée normalement, puis mélangée à l'image
        // d'avant (effets sur les pixels compris).
        let before = (ann.opacity < 1.0).then(|| pixmap.clone());
        if ann.affects_outside() {
            draw_outside(&mut pixmap, ann);
        }
        if ann.shadow && ann.can_have_shadow() {
            draw_shadow(&mut pixmap, ann, assets);
        }
        draw(
            &mut pixmap,
            ann,
            source.as_ref(),
            assets,
            Transform::identity(),
        );
        if let Some(before) = before {
            mix(&mut pixmap, &before, ann.opacity.max(0.0));
        }
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
fn draw(
    pixmap: &mut Pixmap,
    ann: &Annotation,
    source: Option<&Pixmap>,
    assets: Option<&Path>,
    ts: Transform,
) {
    let color = ann.color;
    let w = ann.width;
    let dash = ann.dash;
    let ts_text = &ann.text_style;
    match &ann.shape {
        Shape::Line { from, to } => {
            if let Some(path) = polyline(&[*from, *to]) {
                pixmap.stroke_path(&path, &paint(color), &dashed(stroke(w), dash), ts, None);
            }
        }
        Shape::Arrow {
            from,
            to,
            style,
            bend,
        } => draw_arrow(pixmap, *from, *to, *style, *bend, color, w, dash, ts),
        Shape::Rect { a, b, radius } => {
            if let Some(path) = rounded_rect(Bounds::from_corners(*a, *b), *radius) {
                if let Some(inside) = inside_fill(ann) {
                    pixmap.fill_path(&path, &paint(inside), FillRule::Winding, ts, None);
                }
                let mut s = stroke(w);
                if *radius <= 0.0 {
                    s.line_cap = LineCap::Square;
                    s.line_join = LineJoin::Miter;
                }
                pixmap.stroke_path(&path, &paint(color), &dashed(s, dash), ts, None);
            }
        }
        Shape::Ellipse { a, b } => {
            if let Some(path) = rect(Bounds::from_corners(*a, *b)).and_then(PathBuilder::from_oval)
            {
                if let Some(inside) = inside_fill(ann) {
                    pixmap.fill_path(&path, &paint(inside), FillRule::Winding, ts, None);
                }
                pixmap.stroke_path(&path, &paint(color), &dashed(stroke(w), dash), ts, None);
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
                pixmap.stroke_path(&path, &paint(color), &dashed(stroke(w), dash), ts, None);
            }
        }
        Shape::Text {
            at,
            text,
            size,
            boxed: false,
        } => draw_text(pixmap, *at, text, *size, color, ts_text, ts),
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
            fill_text(pixmap, *at, text, *size, color.contrast(), ts_text, ts);
        }
        Shape::Bubble {
            at,
            text,
            size,
            tail,
        } => {
            let b = bubble_box(*at, text, *size, &text_size);
            if ann.bubble == BubbleKind::Tail {
                draw_pointer(
                    pixmap,
                    b.center(),
                    *tail,
                    (size * 0.9).min(b.width() / 3.0),
                    color,
                    ts,
                );
            }
            let corner = if ann.bubble == BubbleKind::Simple {
                size * 0.12
            } else {
                size * 0.45
            };
            if let Some(path) = rounded_rect(b, corner) {
                pixmap.fill_path(&path, &paint(color), FillRule::Winding, ts, None);
            }
            fill_text(pixmap, *at, text, *size, color.contrast(), ts_text, ts);
        }
        Shape::Step { at, number, tail } => {
            let r = step_radius(w);
            match (ann.step_tail, tail) {
                (StepTail::Pointer, Some(t)) => draw_pointer(pixmap, *at, *t, r * 1.1, color, ts),
                (StepTail::Line | StepTail::Arrow, Some(t)) => {
                    draw_step_connector(pixmap, *at, *t, r, ann.step_tail, color, w, ts);
                }
                (StepTail::Pin, _) => {
                    draw_pointer(pixmap, *at, pin_tip(*at, w), r * 1.5, color, ts);
                }
                _ => {}
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
            let label = step_label(*number, ann.step_format);
            let mut size = r * 1.15;
            let (mut tw, mut th) = text_size(&label, size);
            // Lettres et chiffres romains peuvent être longs : réduits pour tenir.
            if ann.step_format != StepFormat::Number && tw > r * 1.6 {
                size *= r * 1.6 / tw;
                (tw, th) = text_size(&label, size);
            }
            let text_at = Pt::new(at.x - tw / 2.0, at.y - th / 2.0);
            let ink = ann.text_color.unwrap_or_else(|| color.contrast());
            fill_text(
                pixmap,
                text_at,
                &label,
                size,
                ink,
                &TextStyle::default(),
                ts,
            );
        }
        Shape::Magnifier {
            source: src,
            radius,
            at,
            zoom,
        } => draw_magnifier(pixmap, source, *src, *radius, *at, *zoom, color, w, ts),
        Shape::Stamp { at, icon, size } => {
            if let Some(button) = icon.strip_prefix(MOUSE_PREFIX) {
                draw_mouse(pixmap, *at, button, *size, color, ts);
            } else if let Some(emoji) = icon.strip_prefix(EMOJI_PREFIX) {
                draw_emoji(pixmap, *at, emoji, *size, color, ts);
            } else if stamp_art::handles(icon) {
                stamp_art::draw(pixmap, *at, icon, *size, color, ts);
            } else {
                draw_stamp(pixmap, *at, icon, *size, color, ts);
            }
        }
        Shape::Polygon {
            points,
            closed,
            start,
            end,
        } => draw_polygon(pixmap, ann, points, *closed, *start, *end, ts),
        Shape::Bracket { a, b, side } => {
            if let Some(path) = bracket_path(Bounds::from_corners(*a, *b), *side) {
                pixmap.stroke_path(&path, &paint(color), &dashed(stroke(w), dash), ts, None);
            }
        }
        Shape::Image { a, b, file, .. } => {
            draw_image(
                pixmap,
                Bounds::from_corners(*a, *b),
                file,
                assets,
                color,
                ts,
            );
        }
        Shape::Label { at, text, size } => draw_label(pixmap, *at, text, *size, color, ts),
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
    dash: LineStyle,
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
        pixmap.stroke_path(&path, &paint(color), &dashed(stroke(width), dash), ts, None);
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
fn draw_shadow(pixmap: &mut Pixmap, ann: &Annotation, assets: Option<&Path>) {
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
    draw(
        &mut layer,
        ann,
        None,
        assets,
        Transform::from_translate(-x0, -y0),
    );
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
/// Avec une lueur (`style.glow`), la lueur remplace ce contour.
fn draw_text(
    pixmap: &mut Pixmap,
    at: Pt,
    text: &str,
    size: f32,
    color: Rgba,
    style: &TextStyle,
    ts: Transform,
) {
    if let Some(path) = styled_text_path(at, text, size, style) {
        if style.glow.is_none() {
            let halo = color.contrast();
            pixmap.stroke_path(
                &path,
                &paint(Rgba(halo.0, halo.1, halo.2, 220)),
                &stroke((size * 0.12).max(1.5)),
                ts,
                None,
            );
        }
        paint_text(pixmap, &path, size, color, style, ts);
    }
}

fn fill_text(
    pixmap: &mut Pixmap,
    at: Pt,
    text: &str,
    size: f32,
    color: Rgba,
    style: &TextStyle,
    ts: Transform,
) {
    if let Some(path) = styled_text_path(at, text, size, style) {
        paint_text(pixmap, &path, size, color, style, ts);
    }
}

/// Lueur éventuelle, puis le texte, épaissi s'il est en gras.
fn paint_text(
    pixmap: &mut Pixmap,
    path: &tiny_skia::Path,
    size: f32,
    color: Rgba,
    style: &TextStyle,
    ts: Transform,
) {
    if let Some(glow) = style.glow {
        draw_glow(pixmap, path, glow, ts);
    }
    pixmap.fill_path(path, &paint(color), FillRule::Winding, ts, None);
    if style.bold {
        // Faux gras : le contour des glyphes, de la couleur du texte.
        pixmap.stroke_path(path, &paint(color), &stroke(size * BOLD), ts, None);
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

/// Contours des glyphes d'un texte, coin haut gauche en `at`, avec l'italique,
/// l'alignement des lignes et le soulignement de `style` (le style par défaut donne
/// le tracé d'origine).
fn styled_text_path(at: Pt, text: &str, size: f32, style: &TextStyle) -> Option<tiny_skia::Path> {
    let font = FONT.as_scaled(PxScale::from(size));
    let (sx, sy) = (font.h_scale_factor(), font.v_scale_factor());
    let slant = if style.italic { ITALIC_SLANT } else { 0.0 };
    let block = text_size(text, size).0;
    let mut pb = PathBuilder::new();
    for (row, line) in text.split('\n').enumerate() {
        let baseline = at.y
            + row as f32 * size * LINE_HEIGHT
            + font.ascent()
            + (size * LINE_HEIGHT - font.height()) / 2.0;
        let width = line_width(&font, line);
        let shift = match style.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => (block - width) / 2.0,
            TextAlign::Right => block - width,
        };
        let mut x = at.x + shift;
        let mut prev: Option<GlyphId> = None;
        for c in line.chars() {
            let id = font.glyph_id(c);
            if let Some(prev) = prev {
                x += font.kern(prev, id);
            }
            if let Some(outline) = FONT.outline(id) {
                let left = x;
                push_outline(&mut pb, &outline.curves, |p| {
                    (left + p.x * sx + p.y * sy * slant, baseline - p.y * sy)
                });
            }
            x += font.h_advance(id);
            prev = Some(id);
        }
        if style.underline
            && width > 0.0
            && let Some(r) = Rect::from_xywh(
                at.x + shift,
                baseline + size * 0.1,
                width,
                (size * 0.07).max(1.0),
            )
        {
            pb.push_rect(r);
        }
    }
    pb.finish()
}

// --- Lot « Éditeur : annotations » -----------------------------------------------------

/// Pente de l'italique (décalage horizontal par pixel de hauteur).
const ITALIC_SLANT: f32 = 0.2;
/// Épaisseur du contour du faux gras, en fraction de la taille de police.
const BOLD: f32 = 0.05;
/// Préfixe des tampons « souris » dessinés en vecteurs (`souris:gauche`…).
pub const MOUSE_PREFIX: &str = "souris:";

/// Dossier des images insérées dans une capture : `<capture>.assets/`.
pub fn assets_dir(image: &Path) -> PathBuf {
    image.with_extension("assets")
}

/// Trait pointillé ou tiret-point, proportionné à l'épaisseur.
fn dashed(mut s: Stroke, dash: LineStyle) -> Stroke {
    let w = s.width;
    let array = match dash {
        LineStyle::Solid => return s,
        // Bouts ronds : des tirets de longueur quasi nulle font des points.
        LineStyle::Dotted => vec![0.001, w * 2.0],
        LineStyle::DashDot => vec![w * 3.0, w * 2.0, 0.001, w * 2.0],
    };
    s.line_cap = LineCap::Round;
    s.dash = StrokeDash::new(array, 0.0);
    s
}

/// `pixmap` = mélange de `before` (l'image avant l'annotation) et de `pixmap` (après),
/// à `opacity` près.
fn mix(pixmap: &mut Pixmap, before: &Pixmap, opacity: f32) {
    let t = opacity.clamp(0.0, 1.0);
    for (after, old) in pixmap.data_mut().iter_mut().zip(before.data()) {
        let (a, o) = (f32::from(*after), f32::from(*old));
        *after = (o + (a - o) * t).round() as u8;
    }
}

/// Tout l'extérieur d'un rectangle ou d'une ellipse : image moins la forme (règle
/// pair-impair).
fn outside_path(pixmap: &Pixmap, ann: &Annotation) -> Option<tiny_skia::Path> {
    let shape = match &ann.shape {
        Shape::Rect { a, b, radius } => rounded_rect(Bounds::from_corners(*a, *b), *radius)?,
        Shape::Ellipse { a, b } => PathBuilder::from_oval(rect(Bounds::from_corners(*a, *b))?)?,
        Shape::Polygon { points, .. } if points.len() >= 3 => {
            let mut pb = PathBuilder::new();
            pb.move_to(points[0].x, points[0].y);
            for p in &points[1..] {
                pb.line_to(p.x, p.y);
            }
            pb.close();
            pb.finish()?
        }
        _ => return None,
    };
    let mut pb = PathBuilder::new();
    pb.push_rect(Rect::from_xywh(
        0.0,
        0.0,
        pixmap.width() as f32,
        pixmap.height() as f32,
    )?);
    pb.push_path(&shape);
    pb.finish()
}

/// Effets « extérieur » d'un rectangle ou d'une ellipse : flou, puis voile sombre
/// (comme le projecteur).
fn draw_outside(pixmap: &mut Pixmap, ann: &Annotation) {
    let Some(path) = outside_path(pixmap, ann) else {
        return;
    };
    if ann.blur_outside {
        let mut blurred = pixmap.clone();
        let all = Bounds {
            min: Pt::new(0.0, 0.0),
            max: Pt::new(pixmap.width() as f32, pixmap.height() as f32),
        };
        let radius = blur_radius(ann.width);
        for _ in 0..3 {
            box_blur(&mut blurred, all, radius);
        }
        let shader = Pattern::new(
            blurred.as_ref(),
            SpreadMode::Pad,
            FilterQuality::Nearest,
            1.0,
            Transform::identity(),
        );
        let p = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        pixmap.fill_path(&path, &p, FillRule::EvenOdd, Transform::identity(), None);
    }
    if ann.dim_outside {
        let dim = paint(Rgba(0, 0, 0, SPOTLIGHT_ALPHA));
        pixmap.fill_path(&path, &dim, FillRule::EvenOdd, Transform::identity(), None);
    }
    let level = ann.fill_amount();
    if level < 0.0 {
        let c = ann.color;
        let alpha = (f32::from(c.3) * -level).round() as u8;
        let outside = paint(Rgba(c.0, c.1, c.2, alpha));
        pixmap.fill_path(
            &path,
            &outside,
            FillRule::EvenOdd,
            Transform::identity(),
            None,
        );
    }
}

/// Couleur du remplissage intérieur (rectangle, ellipse, polygone), à l'opacité du
/// niveau de remplissage ; `None` sans remplissage intérieur.
fn inside_fill(ann: &Annotation) -> Option<Rgba> {
    let level = ann.fill_amount();
    (level > 0.0).then(|| {
        let c = ann.color;
        Rgba(c.0, c.1, c.2, (f32::from(c.3) * level).round() as u8)
    })
}

/// Lueur d'un texte : contour épais de la couleur de la lueur, flouté, sous le texte.
fn draw_glow(pixmap: &mut Pixmap, path: &tiny_skia::Path, glow: crate::model::Glow, ts: Transform) {
    let b = path.bounds();
    let margin = glow.size + glow.blur * 3.0 + 2.0;
    let (x0, y0) = (
        (b.left() + ts.tx - margin).floor(),
        (b.top() + ts.ty - margin).floor(),
    );
    let (w, h) = (
        (b.width() + margin * 2.0).ceil() as u32 + 1,
        (b.height() + margin * 2.0).ceil() as u32 + 1,
    );
    if w > pixmap.width() * 2 + 64 || h > pixmap.height() * 2 + 64 {
        return;
    }
    let Some(mut layer) = Pixmap::new(w, h) else {
        return;
    };
    let local = ts.post_translate(-x0, -y0);
    let p = paint(glow.color);
    layer.fill_path(path, &p, FillRule::Winding, local, None);
    if glow.size > 0.0 {
        layer.stroke_path(path, &p, &stroke(glow.size * 2.0), local, None);
    }
    let all = Bounds {
        min: Pt::new(0.0, 0.0),
        max: Pt::new(w as f32, h as f32),
    };
    let radius = (glow.blur / 2.0).round() as u32;
    if radius > 0 {
        for _ in 0..3 {
            box_blur(&mut layer, all, radius);
        }
    }
    pixmap.draw_pixmap(
        x0 as i32,
        y0 as i32,
        layer.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// Lien d'une pastille : trait terminé par un point ou par une pointe.
#[allow(clippy::too_many_arguments)]
fn draw_step_connector(
    pixmap: &mut Pixmap,
    at: Pt,
    tail: Pt,
    r: f32,
    kind: StepTail,
    color: Rgba,
    width: f32,
    ts: Transform,
) {
    let Some((ux, uy)) = unit(at, tail) else {
        return;
    };
    let line_w = (width * 0.6).max(2.0);
    let head = (line_w * 4.0).max(12.0).min(at.dist(tail) * 0.5);
    let start = at.offset(ux * r, uy * r);
    let end = if kind == StepTail::Arrow {
        tail.offset(-ux * head * 0.6, -uy * head * 0.6)
    } else {
        tail
    };
    if at.dist(tail) > r
        && let Some(path) = polyline(&[start, end])
    {
        pixmap.stroke_path(&path, &paint(color), &stroke(line_w), ts, None);
    }
    if kind == StepTail::Arrow {
        arrow_head(pixmap, tail, ux, uy, head, color, line_w, ts);
    } else if let Some(dot) = PathBuilder::from_circle(tail.x, tail.y, (width * 1.2).max(4.0)) {
        pixmap.fill_path(&dot, &paint(color), FillRule::Winding, ts, None);
    }
}

/// Polygone ou ligne brisée, avec ses bouts (flèche, rond) quand elle est ouverte.
fn draw_polygon(
    pixmap: &mut Pixmap,
    ann: &Annotation,
    points: &[Pt],
    closed: bool,
    start: EndShape,
    end: EndShape,
    ts: Transform,
) {
    let (color, w) = (ann.color, ann.width);
    if points.len() < 2 {
        return;
    }
    let head = (w * 4.0).max(12.0);
    let mut line = points.to_vec();
    if !closed {
        if end == EndShape::Arrow {
            line = trim_end(&line, head * 0.6);
        }
        if start == EndShape::Arrow {
            line.reverse();
            line = trim_end(&line, head * 0.6);
            line.reverse();
        }
    }
    let mut pb = PathBuilder::new();
    pb.move_to(line[0].x, line[0].y);
    for p in &line[1..] {
        pb.line_to(p.x, p.y);
    }
    if closed {
        pb.close();
    }
    let Some(path) = pb.finish() else { return };
    if let Some(inside) = inside_fill(ann) {
        // Plein : la forme est toujours fermée pour le remplissage.
        let mut fill = PathBuilder::new();
        fill.move_to(points[0].x, points[0].y);
        for p in &points[1..] {
            fill.line_to(p.x, p.y);
        }
        fill.close();
        if let Some(fill) = fill.finish() {
            pixmap.fill_path(&fill, &paint(inside), FillRule::EvenOdd, ts, None);
        }
    }
    pixmap.stroke_path(&path, &paint(color), &dashed(stroke(w), ann.dash), ts, None);
    if closed {
        return;
    }
    let n = points.len();
    for (shape, tip, back) in [
        (end, points[n - 1], points[n - 2]),
        (start, points[0], points[1]),
    ] {
        match shape {
            EndShape::Plain => {}
            EndShape::Arrow => {
                if let Some((ux, uy)) = unit(back, tip) {
                    let len = head.min(tip.dist(back) * 0.8);
                    arrow_head(pixmap, tip, ux, uy, len, color, w, ts);
                }
            }
            EndShape::Round => {
                if let Some(dot) = PathBuilder::from_circle(tip.x, tip.y, (w * 1.4).max(4.0)) {
                    pixmap.fill_path(&dot, &paint(color), FillRule::Winding, ts, None);
                }
            }
        }
    }
}

/// Accolade inscrite dans `b`, pointe du côté `side`.
fn bracket_path(b: Bounds, side: BracketSide) -> Option<tiny_skia::Path> {
    // Tracé dans un repère (u le long de l'accolade, v vers la pointe), puis placé.
    let vertical = matches!(side, BracketSide::Left | BracketSide::Right);
    let (len, depth) = if vertical {
        (b.height(), b.width())
    } else {
        (b.width(), b.height())
    };
    let to = |u: f32, v: f32| -> (f32, f32) {
        match side {
            BracketSide::Left => (b.max.x - v, b.min.y + u),
            BracketSide::Right => (b.min.x + v, b.min.y + u),
            BracketSide::Top => (b.min.x + u, b.max.y - v),
            BracketSide::Bottom => (b.min.x + u, b.min.y + v),
        }
    };
    let mid = depth / 2.0;
    let r = (len / 4.0).min(mid.max(1.0) * 2.0);
    let half = len / 2.0;
    let mut pb = PathBuilder::new();
    let (x, y) = to(0.0, 0.0);
    pb.move_to(x, y);
    let mut quad = |c: (f32, f32), e: (f32, f32)| {
        let (cx, cy) = to(c.0, c.1);
        let (ex, ey) = to(e.0, e.1);
        pb.quad_to(cx, cy, ex, ey);
    };
    quad((0.0, mid), (r, mid));
    let (x, y) = to(half - r, mid);
    pb.line_to(x, y);
    let mut quad = |c: (f32, f32), e: (f32, f32)| {
        let (cx, cy) = to(c.0, c.1);
        let (ex, ey) = to(e.0, e.1);
        pb.quad_to(cx, cy, ex, ey);
    };
    quad((half, mid), (half, depth));
    quad((half, mid), (half + r, mid));
    let (x, y) = to(len - r, mid);
    pb.line_to(x, y);
    let (cx, cy) = to(len, mid);
    let (ex, ey) = to(len, 0.0);
    pb.quad_to(cx, cy, ex, ey);
    pb.finish()
}

/// Images insérées, lues une fois par processus (le nom d'un fichier inséré ne
/// change jamais de contenu).
static IMAGES: LazyLock<Mutex<HashMap<PathBuf, Option<Arc<Pixmap>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn load_asset(path: &Path) -> Option<Arc<Pixmap>> {
    let mut cache = IMAGES.lock().ok()?;
    cache
        .entry(path.to_owned())
        .or_insert_with(|| {
            let img = image::open(path).ok()?.to_rgba8();
            to_pixmap(&img).map(Arc::new)
        })
        .clone()
}

/// Image insérée, étirée dans `b`. Introuvable : un cadre barré à la place.
fn draw_image(
    pixmap: &mut Pixmap,
    b: Bounds,
    file: &str,
    assets: Option<&Path>,
    color: Rgba,
    ts: Transform,
) {
    let Some(r) = rect(b) else { return };
    let img = assets.and_then(|dir| load_asset(&dir.join(file)));
    match img {
        Some(img) => {
            let (iw, ih) = (img.width() as f32, img.height() as f32);
            let fit = Transform::from_scale(b.width() / iw, b.height() / ih)
                .post_translate(b.min.x, b.min.y);
            let shader = Pattern::new(
                img.as_ref().as_ref(),
                SpreadMode::Pad,
                FilterQuality::Bicubic,
                1.0,
                fit,
            );
            let p = Paint {
                shader,
                anti_alias: true,
                ..Paint::default()
            };
            pixmap.fill_rect(r, &p, ts, None);
        }
        None => {
            pixmap.fill_rect(r, &paint(Rgba(128, 128, 128, 90)), ts, None);
            let thin = stroke(2.0);
            let path = PathBuilder::from_rect(r);
            pixmap.stroke_path(&path, &paint(color), &thin, ts, None);
            for (a, c) in [
                (b.min, b.max),
                (Pt::new(b.max.x, b.min.y), Pt::new(b.min.x, b.max.y)),
            ] {
                if let Some(line) = polyline(&[a, c]) {
                    pixmap.stroke_path(&line, &paint(color), &thin, ts, None);
                }
            }
        }
    }
}

/// Tampon texte : capitales en gras dans un cadre arrondi, à la manière d'un tampon
/// encreur.
fn draw_label(pixmap: &mut Pixmap, at: Pt, text: &str, size: f32, color: Rgba, ts: Transform) {
    let b = label_box(at, text, size, &text_size);
    if let Some(frame) = rounded_rect(b, size * 0.3) {
        pixmap.fill_path(
            &frame,
            &paint(Rgba(255, 255, 255, 150)),
            FillRule::Winding,
            ts,
            None,
        );
        pixmap.stroke_path(&frame, &paint(color), &stroke(size * 0.12), ts, None);
    }
    let (tw, th) = text_size(text, size);
    let style = TextStyle {
        bold: true,
        ..TextStyle::default()
    };
    fill_text(
        pixmap,
        Pt::new(at.x - tw / 2.0, at.y - th / 2.0),
        text,
        size,
        color,
        &style,
        ts,
    );
}

/// Tampon souris en vecteurs : le corps et le bouton `button` (gauche, droite,
/// molette) rempli.
fn draw_mouse(pixmap: &mut Pixmap, at: Pt, button: &str, size: f32, color: Rgba, ts: Transform) {
    let (w, h) = (size * 0.62, size * 0.92);
    let body = Bounds {
        min: at.offset(-w / 2.0, -h / 2.0),
        max: at.offset(w / 2.0, h / 2.0),
    };
    let split = body.min.y + h * 0.42;
    let line = (size * 0.06).max(1.5);
    let halo = color.contrast();
    let Some(outline) = rounded_rect(body, w / 2.0) else {
        return;
    };
    pixmap.fill_path(
        &outline,
        &paint(Rgba(halo.0, halo.1, halo.2, 230)),
        FillRule::Winding,
        ts,
        None,
    );
    // Bouton enfoncé : la moitié haute du côté voulu, découpée dans le corps.
    let half = match button {
        "gauche" => Some(Bounds {
            min: body.min,
            max: Pt::new(at.x, split),
        }),
        "droite" => Some(Bounds {
            min: Pt::new(at.x, body.min.y),
            max: Pt::new(body.max.x, split),
        }),
        _ => None,
    };
    if let Some(half) = half
        && let Some(r) = rect(half)
        && let Some(mask) = {
            let mut m = tiny_skia::Mask::new(pixmap.width(), pixmap.height());
            if let Some(m) = &mut m {
                m.fill_path(&outline, FillRule::Winding, true, ts);
            }
            m
        }
    {
        pixmap.fill_rect(r, &paint(color), ts, Some(&mask));
    }
    pixmap.stroke_path(&outline, &paint(color), &stroke(line), ts, None);
    let mut pb = PathBuilder::new();
    pb.move_to(body.min.x, split);
    pb.line_to(body.max.x, split);
    pb.move_to(at.x, body.min.y);
    pb.line_to(at.x, split);
    if let Some(p) = pb.finish() {
        pixmap.stroke_path(&p, &paint(color), &stroke(line), ts, None);
    }
    if button == "molette"
        && let Some(wheel) = rounded_rect(
            Bounds {
                min: Pt::new(at.x - w * 0.09, body.min.y + h * 0.12),
                max: Pt::new(at.x + w * 0.09, body.min.y + h * 0.3),
            },
            w * 0.09,
        )
    {
        pixmap.fill_path(&wheel, &paint(color), FillRule::Winding, ts, None);
    }
}

// --- Lot « Aperçus et tampons » : tampons émoji -------------------------------------

/// Préfixe d'un tampon émoji (`emoji:👍`) dans [`Shape::Stamp`] : dessiné en couleur
/// par le système (voir `crate::emoji`).
pub const EMOJI_PREFIX: &str = "emoji:";

/// Émojis déjà dessinés, par texte et côté en pixels.
type EmojiCache = HashMap<(String, u32), Option<Arc<Pixmap>>>;
static EMOJIS: LazyLock<Mutex<EmojiCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Émoji dessiné dans un carré de `side` pixels (gardé en cache).
fn emoji_pixmap(text: &str, side: u32) -> Option<Arc<Pixmap>> {
    let side = side.clamp(4, crate::emoji::MAX_SIDE);
    let mut cache = EMOJIS.lock().ok()?;
    // Redimensionner un tampon crée une taille par image : le cache reste borné.
    if cache.len() > 256 {
        cache.clear();
    }
    cache
        .entry((text.to_owned(), side))
        .or_insert_with(|| {
            let img = crate::emoji::rasterize(text, side)?;
            to_pixmap(&img).map(Arc::new)
        })
        .clone()
}

/// Tampon émoji de côté `size`, centré sur `at`. Sans émojis sur ce système : un
/// cadre avec un point d'interrogation.
fn draw_emoji(pixmap: &mut Pixmap, at: Pt, text: &str, size: f32, color: Rgba, ts: Transform) {
    let b = Bounds {
        min: at.offset(-size / 2.0, -size / 2.0),
        max: at.offset(size / 2.0, size / 2.0),
    };
    let Some(r) = rect(b) else { return };
    match emoji_pixmap(text, size.ceil().max(1.0) as u32) {
        Some(img) => {
            let side = img.width() as f32;
            let fit =
                Transform::from_scale(size / side, size / side).post_translate(b.min.x, b.min.y);
            let shader = Pattern::new(
                img.as_ref().as_ref(),
                SpreadMode::Pad,
                FilterQuality::Bicubic,
                1.0,
                fit,
            );
            let p = Paint {
                shader,
                anti_alias: true,
                ..Paint::default()
            };
            pixmap.fill_rect(r, &p, ts, None);
        }
        None => {
            if let Some(frame) = rounded_rect(b, size * 0.2) {
                pixmap.fill_path(
                    &frame,
                    &paint(Rgba(128, 128, 128, 90)),
                    FillRule::Winding,
                    ts,
                    None,
                );
                pixmap.stroke_path(&frame, &paint(color), &stroke(size * 0.05), ts, None);
            }
            draw_stamp(
                pixmap,
                at,
                crate::ui::icons::STAMP_QUESTION,
                size * 0.6,
                color,
                ts,
            );
        }
    }
}

/// Hauteur du bandeau des propriétés (titre, description) pour une image de cette
/// largeur ; 0 sans bandeau.
pub fn banner_height(doc: &Doc, width: u32) -> u32 {
    if !doc.has_banner() {
        return 0;
    }
    let (title, desc, pad) = banner_sizes(width);
    let mut h = pad * 2.0;
    if !doc.title.trim().is_empty() {
        h += text_size(doc.title.trim(), title).1;
    }
    if !doc.description.trim().is_empty() {
        if !doc.title.trim().is_empty() {
            h += pad * 0.4;
        }
        h += text_size(doc.description.trim(), desc).1;
    }
    h.ceil() as u32
}

/// Tailles du titre, de la description et marge du bandeau.
fn banner_sizes(width: u32) -> (f32, f32, f32) {
    let title = (width as f32 / 30.0).clamp(16.0, 60.0);
    (title, title * 0.62, title * 0.6)
}

/// Image annotée, avec le bandeau des propriétés au-dessus quand il est activé.
pub fn render_doc(base: &RgbaImage, doc: &Doc, assets: Option<&Path>) -> RgbaImage {
    let img = render_with(base, &doc.annotations, assets);
    let banner = banner_height(doc, img.width());
    if banner == 0 {
        return img;
    }
    let (w, h) = img.dimensions();
    let Some(mut pixmap) = Pixmap::new(w, h + banner) else {
        return img;
    };
    pixmap.fill(tiny_skia::Color::WHITE);
    let (title, desc, pad) = banner_sizes(w);
    let mut y = pad;
    let id = Transform::identity();
    if !doc.title.trim().is_empty() {
        let style = TextStyle {
            bold: true,
            ..TextStyle::default()
        };
        let t = doc.title.trim();
        fill_text(
            &mut pixmap,
            Pt::new(pad, y),
            t,
            title,
            Rgba::rgb(20, 20, 24),
            &style,
            id,
        );
        y += text_size(t, title).1 + pad * 0.4;
    }
    if !doc.description.trim().is_empty() {
        let d = doc.description.trim();
        let style = TextStyle::default();
        fill_text(
            &mut pixmap,
            Pt::new(pad, y),
            d,
            desc,
            Rgba::rgb(80, 80, 90),
            &style,
            id,
        );
    }
    if let Some(image) = to_pixmap(&img) {
        pixmap.draw_pixmap(
            0,
            banner as i32,
            image.as_ref(),
            &PixmapPaint::default(),
            id,
            None,
        );
    }
    let mut out = RgbaImage::new(w, h + banner);
    for (dst, src) in out.pixels_mut().zip(pixmap.pixels()) {
        let c = src.demultiply();
        dst.0 = [c.red(), c.green(), c.blue(), c.alpha()];
    }
    out
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

    // --- Lot « Éditeur : annotations » -----------------------------------------------

    use crate::model::{Doc, Glow};

    fn rect_ann(a: Pt, b: Pt) -> Annotation {
        ann(Shape::Rect { a, b, radius: 0.0 }, RED)
    }

    /// Les options à leur valeur par défaut ne changent rien au rendu d'origine.
    #[test]
    fn default_options_render_like_before() {
        let img = white(120, 90);
        let anns = [
            rect_ann(Pt::new(10.0, 10.0), Pt::new(60.0, 50.0)),
            ann(
                Shape::Text {
                    at: Pt::new(5.0, 50.0),
                    text: "Ab\nc".into(),
                    size: 18.0,
                    boxed: false,
                },
                RED,
            ),
        ];
        let out = render(&img, &anns);
        let mut explicit = anns.clone();
        for a in &mut explicit {
            a.opacity = 1.0;
            a.dash = LineStyle::Solid;
            a.text_style = TextStyle::default();
        }
        assert_eq!(render(&img, &explicit), out);
        let doc = Doc {
            annotations: anns.to_vec(),
            title: "Titre".into(),
            show_properties: false,
            ..Doc::default()
        };
        assert_eq!(render_doc(&img, &doc, None), out, "bandeau désactivé");
    }

    #[test]
    fn dashes_leave_gaps_in_the_stroke() {
        let line = |dash| {
            let mut a = ann(
                Shape::Line {
                    from: Pt::new(5.0, 20.0),
                    to: Pt::new(115.0, 20.0),
                },
                RED,
            );
            a.dash = dash;
            render(&white(120, 40), &[a])
        };
        let solid = line(LineStyle::Solid);
        let dotted = line(LineStyle::Dotted);
        let dash_dot = line(LineStyle::DashDot);
        let row = |img: &RgbaImage| {
            (5..115)
                .filter(|&x| img.get_pixel(x, 20).0 == RED_PX)
                .count()
        };
        assert!(row(&solid) > 100);
        assert!(row(&dotted) < row(&solid) * 2 / 3, "{}", row(&dotted));
        assert!(row(&dash_dot) < row(&solid) && row(&dash_dot) > row(&dotted));
    }

    #[test]
    fn opacity_blends_with_the_image() {
        let mut a = rect_ann(Pt::new(10.0, 10.0), Pt::new(50.0, 50.0));
        a.fill = true;
        a.opacity = 0.5;
        let px = render(&white(60, 60), &[a]).get_pixel(30, 30).0;
        assert_eq!(px[0], 255);
        assert!((120..=135).contains(&px[1]), "moitié de blanc : {px:?}");
    }

    #[test]
    fn fill_level_paints_inside_or_outside() {
        let mut a = rect_ann(Pt::new(10.0, 10.0), Pt::new(50.0, 50.0));
        // Moitié dedans : rouge à moitié sur blanc, dehors intact.
        a.fill_level = 0.5;
        let img = render(&white(60, 60), &[a.clone()]);
        let inside = img.get_pixel(30, 30).0;
        assert_eq!(inside[0], 255);
        assert!((120..=135).contains(&inside[1]), "{inside:?}");
        assert_eq!(img.get_pixel(2, 2).0, [255, 255, 255, 255]);
        // Tout l'extérieur : rouge plein dehors, blanc dedans.
        a.fill_level = -1.0;
        let img = render(&white(60, 60), &[a.clone()]);
        assert_eq!(img.get_pixel(2, 2).0, RED_PX);
        assert_eq!(img.get_pixel(30, 30).0, [255, 255, 255, 255]);
        // Un ancien fichier (« plein », sans niveau) reste plein.
        a.fill_level = 0.0;
        a.fill = true;
        assert_eq!(render(&white(60, 60), &[a]).get_pixel(30, 30).0, RED_PX);
    }

    #[test]
    fn outside_effects_dim_and_blur_only_outside() {
        let mut img = white(100, 100);
        for y in 0..100 {
            for x in 0..50 {
                img.put_pixel(x, y, Px([0, 0, 0, 255]));
            }
        }
        let mut a = ann(
            Shape::Ellipse {
                a: Pt::new(60.0, 30.0),
                b: Pt::new(95.0, 70.0),
            },
            RED,
        );
        a.dim_outside = true;
        let out = render(&img, &[a.clone()]);
        assert_eq!(out.get_pixel(78, 50).0, WHITE_PX, "intérieur intact");
        assert!(out.get_pixel(55, 5).0[0] < 150, "extérieur assombri");
        a.dim_outside = false;
        a.blur_outside = true;
        let out = render(&img, &[a]);
        let edge = out.get_pixel(50, 10).0[0];
        assert!(edge > 30 && edge < 230, "bord flouté : {edge}");
        assert_eq!(out.get_pixel(78, 50).0, WHITE_PX, "intérieur net");
    }

    #[test]
    fn text_style_options_change_the_ink() {
        let text = |style: TextStyle| {
            let mut a = ann(
                Shape::Text {
                    at: Pt::new(10.0, 10.0),
                    text: "Hello".into(),
                    size: 30.0,
                    boxed: false,
                },
                RED,
            );
            a.text_style = style;
            render(&white(160, 60), &[a])
        };
        let plain = text(TextStyle::default());
        let bold = text(TextStyle {
            bold: true,
            ..TextStyle::default()
        });
        assert!(reds(&bold) > reds(&plain) * 11 / 10, "gras plus épais");
        let underline = text(TextStyle {
            underline: true,
            ..TextStyle::default()
        });
        assert!(reds(&underline) > reds(&plain), "souligné");
        let italic = text(TextStyle {
            italic: true,
            ..TextStyle::default()
        });
        assert_ne!(italic, plain, "italique penché");
        let glow = text(TextStyle {
            glow: Some(Glow {
                color: Rgba::rgb(0, 0, 255),
                size: 4.0,
                blur: 2.0,
            }),
            ..TextStyle::default()
        });
        assert!(
            glow.pixels().any(|p| p.0[2] > 200 && p.0[0] < 100),
            "lueur bleue autour du texte"
        );
    }

    #[test]
    fn centered_lines_move_right() {
        let draw = |align| {
            let mut a = ann(
                Shape::Text {
                    at: Pt::new(5.0, 5.0),
                    text: "MMMMMM\ni".into(),
                    size: 20.0,
                    boxed: false,
                },
                RED,
            );
            a.text_style.align = align;
            render(&white(140, 60), &[a])
        };
        // Deuxième ligne (« i ») : x du premier pixel rouge.
        let first_x = |img: &RgbaImage| {
            (0..140)
                .find(|&x| (30..52).any(|y| img.get_pixel(x, y).0 == RED_PX))
                .unwrap_or(0)
        };
        let left = first_x(&draw(TextAlign::Left));
        let center = first_x(&draw(TextAlign::Center));
        let right = first_x(&draw(TextAlign::Right));
        assert!(left < center && center < right, "{left} {center} {right}");
    }

    #[test]
    fn bubbles_without_tail_draw_no_pointer() {
        let bubble = |kind| {
            let mut a = ann(
                Shape::Bubble {
                    at: Pt::new(20.0, 20.0),
                    text: "Ok".into(),
                    size: 30.0,
                    tail: Pt::new(30.0, 140.0),
                },
                RED,
            );
            a.bubble = kind;
            render(&white(140, 160), &[a])
        };
        assert_eq!(bubble(BubbleKind::Tail).get_pixel(30, 125).0, RED_PX);
        assert_eq!(bubble(BubbleKind::NoTail).get_pixel(30, 125).0, WHITE_PX);
        let simple = bubble(BubbleKind::Simple);
        assert_eq!(simple.get_pixel(30, 125).0, WHITE_PX);
        // Coins presque carrés : le coin haut gauche du cadre est rempli.
        assert_eq!(simple.get_pixel(8, 13).0, RED_PX);
        assert_eq!(bubble(BubbleKind::NoTail).get_pixel(8, 13).0, WHITE_PX);
    }

    #[test]
    fn step_tails_and_formats() {
        let step = |tail_kind, format| {
            let mut a = ann(
                Shape::Step {
                    at: Pt::new(30.0, 30.0),
                    number: 4,
                    tail: Some(Pt::new(100.0, 30.0)),
                },
                RED,
            );
            a.step_tail = tail_kind;
            a.step_format = format;
            a.text_color = Some(Rgba::rgb(0, 0, 255));
            render(&white(120, 90), &[a])
        };
        let line = step(StepTail::Line, StepFormat::Number);
        assert_eq!(line.get_pixel(70, 30).0, RED_PX, "trait");
        assert_eq!(line.get_pixel(70, 36).0, WHITE_PX, "pas une pointe pleine");
        assert_eq!(line.get_pixel(100, 30).0, RED_PX, "point au bout");
        let hidden = step(StepTail::Hidden, StepFormat::Number);
        assert_eq!(hidden.get_pixel(70, 30).0, WHITE_PX);
        let pin = step(StepTail::Pin, StepFormat::Number);
        assert_eq!(
            pin.get_pixel(70, 30).0,
            WHITE_PX,
            "l'épingle ignore la pointe"
        );
        assert_eq!(pin.get_pixel(30, 30 + 22).0, RED_PX, "pointe de l'épingle");
        let roman = step(StepTail::Hidden, StepFormat::Roman);
        assert_ne!(roman, hidden, "« iv » au lieu de « 4 »");
        assert!(
            hidden.pixels().any(|p| p.0 == [0, 0, 255, 255]),
            "numéro de la couleur choisie"
        );
    }

    #[test]
    fn polygons_fill_close_and_end_with_arrows() {
        let points = vec![
            Pt::new(10.0, 10.0),
            Pt::new(90.0, 10.0),
            Pt::new(90.0, 70.0),
        ];
        let poly = |closed, fill, end| {
            let mut a = ann(
                Shape::Polygon {
                    points: points.clone(),
                    closed,
                    start: EndShape::Plain,
                    end,
                },
                RED,
            );
            a.fill = fill;
            render(&white(100, 80), &[a])
        };
        let open = poly(false, false, EndShape::Plain);
        assert_eq!(open.get_pixel(50, 10).0, RED_PX);
        assert_eq!(open.get_pixel(50, 40).0, WHITE_PX, "pas de côté fermant");
        let closed = poly(true, false, EndShape::Plain);
        assert_eq!(closed.get_pixel(50, 40).0, RED_PX, "côté fermant");
        let filled = poly(true, true, EndShape::Plain);
        assert_eq!(filled.get_pixel(75, 25).0, RED_PX, "intérieur plein");
        let arrow = poly(false, false, EndShape::Arrow);
        assert!(reds(&arrow) > reds(&open), "pointe au bout");
        assert_eq!(arrow.get_pixel(86, 60).0, RED_PX, "large pointe");
        assert_ne!(open.get_pixel(86, 60).0, RED_PX);
    }

    #[test]
    fn brackets_point_to_their_side() {
        let draw = |side| {
            render(
                &white(100, 100),
                &[ann(
                    Shape::Bracket {
                        a: Pt::new(20.0, 10.0),
                        b: Pt::new(60.0, 90.0),
                        side,
                    },
                    RED,
                )],
            )
        };
        let left = draw(BracketSide::Left);
        assert_ne!(left.get_pixel(21, 50).0, WHITE_PX, "pointe à gauche");
        assert_eq!(left.get_pixel(59, 50).0, WHITE_PX);
        let right = draw(BracketSide::Right);
        assert_ne!(right.get_pixel(59, 50).0, WHITE_PX, "pointe à droite");
        let top = draw(BracketSide::Top);
        assert_ne!(top.get_pixel(40, 11).0, WHITE_PX, "pointe en haut");
    }

    #[test]
    fn inserted_images_come_from_the_assets_folder() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("capture.png");
        let assets = assets_dir(&capture);
        assert_eq!(assets, dir.path().join("capture.assets"));
        std::fs::create_dir_all(&assets).unwrap();
        let blue = RgbaImage::from_pixel(10, 5, Px([0, 0, 255, 255]));
        crate::history::write_png(&assets.join("logo.png"), &blue).unwrap();
        let shape = Shape::Image {
            a: Pt::new(10.0, 10.0),
            b: Pt::new(50.0, 30.0),
            file: "logo.png".into(),
            ratio: 2.0,
        };
        let out = render_with(&white(60, 40), &[ann(shape.clone(), RED)], Some(&assets));
        assert_eq!(out.get_pixel(30, 20).0, [0, 0, 255, 255]);
        assert_eq!(out.get_pixel(5, 5).0, WHITE_PX);
        // Sans le dossier : un cadre barré, jamais une erreur.
        let missing = render(&white(60, 40), &[ann(shape, RED)]);
        assert_ne!(missing.get_pixel(30, 20).0, [0, 0, 255, 255]);
    }

    #[test]
    fn labels_and_mouse_stamps_are_drawn() {
        let label = ann(
            Shape::Label {
                at: Pt::new(80.0, 30.0),
                text: "BROUILLON".into(),
                size: 20.0,
            },
            RED,
        );
        let out = render(&white(160, 60), &[label]);
        assert!(reds(&out) > 300, "cadre et texte ({})", reds(&out));
        let mouse = |button: &str| {
            render(
                &white(80, 80),
                &[ann(
                    Shape::Stamp {
                        at: Pt::new(40.0, 40.0),
                        icon: format!("{MOUSE_PREFIX}{button}"),
                        size: 60.0,
                    },
                    RED,
                )],
            )
        };
        let left = mouse("gauche");
        let right = mouse("droite");
        assert_eq!(left.get_pixel(33, 22).0, RED_PX, "bouton gauche rempli");
        assert_ne!(right.get_pixel(33, 22).0, RED_PX);
        assert_eq!(right.get_pixel(47, 22).0, RED_PX, "bouton droit rempli");
    }

    #[test]
    fn the_properties_banner_goes_above_the_image() {
        let img = RgbaImage::from_pixel(200, 50, Px([0, 128, 0, 255]));
        let doc = Doc {
            annotations: Vec::new(),
            title: "Titre".into(),
            description: "Une description".into(),
            show_properties: true,
            ..Doc::default()
        };
        let h = banner_height(&doc, 200);
        assert!(h > 20, "{h}");
        let out = render_doc(&img, &doc, None);
        assert_eq!(out.dimensions(), (200, 50 + h));
        assert_eq!(out.get_pixel(199, 1).0, WHITE_PX, "fond du bandeau");
        assert!(
            (0..h).any(|y| (0..200).any(|x| out.get_pixel(x, y).0[0] < 100)),
            "titre écrit"
        );
        assert_eq!(
            out.get_pixel(100, h + 10).0,
            [0, 128, 0, 255],
            "image dessous"
        );
    }

    // --- Lot « Aperçus et tampons »

    #[test]
    fn drawn_stamps_keep_the_json_format_and_their_box() {
        let stamp = ann(
            Shape::Stamp {
                at: Pt::new(100.0, 40.0),
                icon: "tampon:APPROUVÉ".into(),
                size: 40.0,
            },
            RED,
        );
        let back: Annotation =
            serde_json::from_str(&serde_json::to_string(&stamp).unwrap()).unwrap();
        assert_eq!(back, stamp);
        let b = stamp.bounds(text_size);
        assert!(b.width() > 100.0 && b.height() == 40.0, "{b:?}");
        let out = render(&white(200, 80), std::slice::from_ref(&stamp));
        // Encre rouge dans le cadre, rien en dehors.
        assert!(out.pixels().any(|p| p.0[0] > 180 && p.0[1] < 120));
        assert_eq!(out.get_pixel(2, 2).0, WHITE_PX);
        // Une icône Lucide d'avant reste dessinée comme avant (carré de 40).
        let lucide = ann(
            Shape::Stamp {
                at: Pt::new(40.0, 40.0),
                icon: crate::ui::icons::STAMP_CHECK.into(),
                size: 40.0,
            },
            RED,
        );
        let b = lucide.bounds(text_size);
        assert_eq!((b.width(), b.height()), (40.0, 40.0));
    }

    #[test]
    fn emoji_stamps_round_trip_and_draw() {
        let stamp = ann(
            Shape::Stamp {
                at: Pt::new(40.0, 40.0),
                icon: format!("{EMOJI_PREFIX}{}", crate::emoji::POSITIVE[0].0),
                size: 60.0,
            },
            RED,
        );
        // Même format JSON que les autres tampons : le texte de l'icône, préfixé.
        let json = serde_json::to_string(&stamp).unwrap();
        assert!(json.contains("emoji:"), "{json}");
        let back: Annotation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, stamp);
        let out = render(&white(80, 80), std::slice::from_ref(&stamp));
        let changed = out.pixels().filter(|p| p.0 != WHITE_PX).count();
        assert!(changed > 400, "quelque chose est dessiné : {changed}");
        if crate::emoji::AVAILABLE {
            // L'émoji en couleur, pas le cadre de remplacement (rouge).
            assert!(
                out.pixels()
                    .any(|p| p.0[1] > 120 && p.0[2] < 100 && p.0 != WHITE_PX),
                "pouce jaune dessiné"
            );
        }
    }
}
