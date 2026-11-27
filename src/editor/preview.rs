//! Aperçu d'une annotation dessiné par egui, pendant qu'on la trace ou la déplace.
//!
//! Le rendu exact (tiny-skia) recalcule toute l'image : trop lent pour suivre la souris
//! sur une capture Retina. Pendant le geste, l'annotation en cours est donc retirée du
//! rendu exact et dessinée ici, de façon approchée ; le rendu exact reprend au
//! relâchement.

use egui::{
    Align2, Color32, CornerRadius, Painter, Pos2, Rect, Shape as EShape, Stroke, StrokeKind, Vec2,
};

use crate::model::{
    Annotation, ArrowStyle, Bounds, Pt, Rgba, Shape, arrow_points, bend_control, step_radius,
    text_padding,
};
use crate::render;
use crate::ui::theme;

/// Passage des pixels de l'image aux points de l'écran.
#[derive(Debug, Clone, Copy)]
pub struct View {
    /// Coin haut gauche de l'image à l'écran.
    pub origin: Pos2,
    /// Points d'écran par pixel d'image.
    pub scale: f32,
}

impl View {
    pub fn to_screen(self, p: Pt) -> Pos2 {
        self.origin + Vec2::new(p.x, p.y) * self.scale
    }

    pub fn to_image(self, p: Pos2) -> Pt {
        let v = (p - self.origin) / self.scale;
        Pt::new(v.x, v.y)
    }

    pub fn rect(self, b: Bounds) -> Rect {
        Rect::from_min_max(self.to_screen(b.min), self.to_screen(b.max))
    }
}

pub fn color32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.0, c.1, c.2, c.3)
}

pub fn paint(painter: &Painter, view: View, ann: &Annotation) {
    let color = color32(ann.color);
    let s = view.scale;
    let stroke = Stroke::new((ann.width * s).max(1.0), color);
    let rect = |a: Pt, b: Pt| view.rect(Bounds::from_corners(a, b));
    match &ann.shape {
        Shape::Line { from, to } => {
            painter.line_segment([view.to_screen(*from), view.to_screen(*to)], stroke);
        }
        Shape::Arrow {
            from,
            to,
            style,
            bend,
            ends,
        } => arrow(
            painter,
            view,
            *from,
            *to,
            *style,
            (*bend, *ends),
            ann.width,
            color,
        ),
        Shape::Rect { a, b, radius } => {
            let r = rect(*a, *b);
            let corner = CornerRadius::same((radius * s).min(255.0) as u8);
            let level = ann.fill_amount();
            if level > 0.0 {
                painter.rect_filled(r, corner, color.gamma_multiply(level));
            } else if level < 0.0 {
                outside_bands(painter, r, color.gamma_multiply(-level));
            }
            painter.rect_stroke(r, corner, stroke, StrokeKind::Middle);
        }
        Shape::Ellipse { a, b } => {
            let r = rect(*a, *b);
            let level = ann.fill_amount();
            let fill = if level > 0.0 {
                color.gamma_multiply(level)
            } else {
                if level < 0.0 {
                    // Aperçu : l'extérieur du cadre (le rendu exact suit l'ellipse).
                    outside_bands(painter, r, color.gamma_multiply(-level));
                }
                Color32::TRANSPARENT
            };
            painter.add(EShape::ellipse_filled(r.center(), r.size() / 2.0, fill));
            painter.add(EShape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
        }
        Shape::Highlight { a, b } => {
            painter.rect_filled(rect(*a, *b), 0.0, color.gamma_multiply(0.45));
        }
        Shape::Spotlight { a, b } => {
            let r = rect(*a, *b);
            outside_bands(painter, r, Color32::from_black_alpha(150));
        }
        Shape::Pixelate { a, b } | Shape::Blur { a, b } => {
            let r = rect(*a, *b);
            painter.rect_filled(r, 0.0, Color32::from_gray(128).gamma_multiply(0.6));
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(1.0, theme::TEXT_DIM),
                StrokeKind::Inside,
            );
        }
        Shape::Pen { points } => {
            let pts: Vec<Pos2> = points.iter().map(|p| view.to_screen(*p)).collect();
            painter.add(EShape::line(pts, stroke));
        }
        Shape::Text {
            at,
            text,
            size,
            boxed,
        } => {
            let text_color = if *boxed {
                let (w, h) = render::text_size(text, *size);
                let (px, py) = text_padding(*size);
                let b = Bounds {
                    min: at.offset(-px, -py),
                    max: at.offset(w + px, h + py),
                };
                painter.rect_filled(
                    view.rect(b),
                    CornerRadius::same((size * 0.3 * s) as u8),
                    color,
                );
                color32(ann.color.contrast())
            } else {
                color
            };
            painter.text(
                view.to_screen(*at),
                Align2::LEFT_TOP,
                text,
                theme::strong(size * s),
                text_color,
            );
        }
        Shape::Bubble {
            at,
            text,
            size,
            tail,
        } => {
            let b = crate::model::bubble_box(*at, text, *size, &render::text_size);
            if ann.bubble == crate::model::BubbleKind::Tail {
                pointer(painter, view, b.center(), *tail, size * 0.9, color);
            }
            painter.rect_filled(
                view.rect(b),
                CornerRadius::same((size * 0.45 * s).min(255.0) as u8),
                color,
            );
            let text_color = color32(ann.color.contrast());
            painter.text(
                view.to_screen(*at),
                Align2::LEFT_TOP,
                text,
                theme::strong(size * s),
                text_color,
            );
        }
        Shape::Step { at, number, tail } => {
            use crate::model::StepTail;
            let r = step_radius(ann.width);
            match (ann.step_tail, tail) {
                (StepTail::Pin, _) => pointer(
                    painter,
                    view,
                    *at,
                    crate::model::pin_tip(*at, ann.width),
                    r * 1.5,
                    color,
                ),
                (StepTail::Hidden, _) | (_, None) => {}
                (StepTail::Pointer, Some(t)) => pointer(painter, view, *at, *t, r * 1.1, color),
                (_, Some(t)) => {
                    painter.line_segment(
                        [view.to_screen(*at), view.to_screen(*t)],
                        Stroke::new(((ann.width * 0.6).max(2.0) * s).max(1.0), color),
                    );
                }
            }
            let c = view.to_screen(*at);
            painter.circle_filled(c, r * s, color);
            painter.text(
                c,
                Align2::CENTER_CENTER,
                crate::model::step_label(*number, ann.step_format),
                theme::strong(r * s * 1.15),
                color32(ann.text_color.unwrap_or_else(|| ann.color.contrast())),
            );
        }
        Shape::Magnifier {
            source,
            radius,
            at,
            zoom,
        } => {
            let thin = Stroke::new((ann.width * s * 0.5).max(1.0), color);
            painter.circle_stroke(view.to_screen(*source), radius * s, thin);
            painter.line_segment([view.to_screen(*source), view.to_screen(*at)], thin);
            let lens = view.to_screen(*at);
            painter.circle_filled(lens, radius * zoom * s, Color32::from_black_alpha(90));
            painter.circle_stroke(lens, radius * zoom * s, stroke);
        }
        // Lot « Aperçus et tampons » : émojis et souris ne sont pas des glyphes de la
        // police d'icônes, ni les dessins (un émoji ne passe jamais par le texte d'egui) :
        // un cadre.
        Shape::Stamp { at, icon, size } if !super::tools::is_icon_glyph(icon) => {
            let b = crate::model::stamp_box(*at, icon, *size, &render::text_size);
            painter.rect_stroke(
                view.rect(b),
                CornerRadius::same((size * s * 0.2).min(255.0) as u8),
                stroke,
                StrokeKind::Middle,
            );
        }
        Shape::Stamp { at, icon, size } => {
            painter.text(
                view.to_screen(*at),
                Align2::CENTER_CENTER,
                icon,
                egui::FontId::new(size * s, egui::FontFamily::Name(theme::ICONS.into())),
                color,
            );
        }
        // --- Lot « Éditeur : annotations »
        Shape::Polygon { points, closed, .. } => {
            let mut pts: Vec<Pos2> = points.iter().map(|p| view.to_screen(*p)).collect();
            if ann.fill_amount() > 0.0 && pts.len() >= 3 {
                painter.add(EShape::convex_polygon(
                    pts.clone(),
                    color.gamma_multiply(0.5),
                    Stroke::NONE,
                ));
            }
            if *closed && let Some(first) = pts.first().copied() {
                pts.push(first);
            }
            painter.add(EShape::line(pts, stroke));
        }
        Shape::Bracket { a, b, .. } => {
            painter.rect_stroke(rect(*a, *b), 4.0, stroke, StrokeKind::Middle);
        }
        Shape::Image { a, b, .. } => {
            let r = rect(*a, *b);
            painter.rect_filled(r, 0.0, Color32::from_gray(128).gamma_multiply(0.5));
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(1.0, theme::TEXT_DIM),
                StrokeKind::Inside,
            );
        }
        Shape::Label { at, text, size } => {
            let b = crate::model::label_box(*at, text, *size, &render::text_size);
            painter.rect_stroke(
                view.rect(b),
                CornerRadius::same((size * 0.3 * s).min(255.0) as u8),
                Stroke::new((size * 0.12 * s).max(1.0), color),
                StrokeKind::Middle,
            );
            painter.text(
                view.to_screen(*at),
                Align2::CENTER_CENTER,
                text,
                theme::strong(size * s),
                color,
            );
        }
    }
}

/// Pointe pleine (bulles, pastilles).
fn pointer(painter: &Painter, view: View, from: Pt, to: Pt, width: f32, color: Color32) {
    let len = from.dist(to);
    if len < 1.0 {
        return;
    }
    let (ux, uy) = ((to.x - from.x) / len, (to.y - from.y) / len);
    let half = width / 2.0;
    let pts = [
        Pt::new(from.x - uy * half, from.y + ux * half),
        to,
        Pt::new(from.x + uy * half, from.y - ux * half),
    ];
    let pts = pts.iter().map(|p| view.to_screen(*p)).collect();
    painter.add(EShape::convex_polygon(pts, color, Stroke::NONE));
}

#[allow(clippy::too_many_arguments)]
fn arrow(
    painter: &Painter,
    view: View,
    from: Pt,
    to: Pt,
    style: ArrowStyle,
    (bend, ends): (Option<Pt>, crate::model::DimensionEnds),
    width: f32,
    color: Color32,
) {
    let dim = crate::model::Dimension::new(from, to, bend, ends, width);
    let len = from.dist(to);
    if len < 0.5 {
        return;
    }
    if style.is_dimension() {
        let thin = Stroke::new((width * 0.5 * view.scale).max(1.0), color);
        for [a, b] in dim.extensions() {
            painter.line_segment([view.to_screen(a), view.to_screen(b)], thin);
        }
    }
    if style == ArrowStyle::Dimension {
        arrow(
            painter,
            view,
            from,
            to,
            ArrowStyle::Double,
            (None, Default::default()),
            width,
            color,
        );
        return;
    }
    if style == ArrowStyle::Inward {
        // Cote par l'extérieur : trait prolongé, pointes vers l'intérieur.
        let [a, b] = dim.line();
        painter.line_segment(
            [view.to_screen(a), view.to_screen(b)],
            Stroke::new((width * view.scale).max(1.0), color),
        );
        let head = dim.head;
        for (tip, back) in [(from, a), (to, b)] {
            let d = tip.dist(back).max(1e-3);
            let (ux, uy) = ((tip.x - back.x) / d, (tip.y - back.y) / d);
            let half = head * 0.6;
            let base = Pt::new(tip.x - ux * head, tip.y - uy * head);
            let pts = [
                tip,
                Pt::new(base.x - uy * half, base.y + ux * half),
                Pt::new(base.x + uy * half, base.y - ux * half),
            ];
            let pts = pts.iter().map(|p| view.to_screen(*p)).collect();
            painter.add(EShape::convex_polygon(pts, color, Stroke::NONE));
        }
        return;
    }
    let bend = if style == ArrowStyle::Big { None } else { bend };
    let line_width = if style == ArrowStyle::Big {
        width * 2.2
    } else {
        width
    };
    let pts: Vec<Pos2> = arrow_points(from, to, bend)
        .into_iter()
        .map(|p| view.to_screen(p))
        .collect();
    painter.add(EShape::line(
        pts,
        Stroke::new((line_width * view.scale).max(1.0), color),
    ));
    let control = bend.map(|b| bend_control(from, to, b));
    let head = (width * if style == ArrowStyle::Big { 5.0 } else { 4.0 })
        .max(12.0)
        .min(len * 0.5);
    let tip = |tip: Pt, back: Pt| {
        let d = tip.dist(back).max(1e-3);
        let (ux, uy) = ((tip.x - back.x) / d, (tip.y - back.y) / d);
        let half = head * 0.6;
        let base = Pt::new(tip.x - ux * head, tip.y - uy * head);
        let pts = [
            tip,
            Pt::new(base.x - uy * half, base.y + ux * half),
            Pt::new(base.x + uy * half, base.y - ux * half),
        ];
        let pts = pts.iter().map(|p| view.to_screen(*p)).collect();
        painter.add(EShape::convex_polygon(pts, color, Stroke::NONE));
    };
    match style {
        ArrowStyle::Simple | ArrowStyle::Big => tip(to, control.unwrap_or(from)),
        ArrowStyle::Double => {
            tip(to, control.unwrap_or(from));
            tip(from, control.unwrap_or(to));
        }
        ArrowStyle::Inward | ArrowStyle::Dimension => {}
        ArrowStyle::Connector => {
            let r = (width * 1.4).max(4.0) * view.scale;
            painter.circle_filled(view.to_screen(from), r, color);
            painter.circle_filled(view.to_screen(to), r, color);
        }
    }
}

/// Cadre pointillé autour de l'annotation sélectionnée.
pub fn selection(painter: &Painter, rect: Rect) {
    let r = rect.expand(4.0);
    let stroke = Stroke::new(1.5, theme::ACCENT_2);
    let corners = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.extend(EShape::dashed_line(&corners, stroke, 6.0, 4.0));
}

/// Tout ce qui est autour du rectangle `r` (dans la zone de dessin), de cette
/// couleur : projecteur, remplissage de l'extérieur.
fn outside_bands(painter: &Painter, r: Rect, color: Color32) {
    let all = painter.clip_rect();
    for band in [
        Rect::from_min_max(all.min, Pos2::new(all.max.x, r.min.y)),
        Rect::from_min_max(Pos2::new(all.min.x, r.max.y), all.max),
        Rect::from_min_max(Pos2::new(all.min.x, r.min.y), Pos2::new(r.min.x, r.max.y)),
        Rect::from_min_max(Pos2::new(r.max.x, r.min.y), Pos2::new(all.max.x, r.max.y)),
    ] {
        if band.is_positive() {
            painter.rect_filled(band, 0.0, color);
        }
    }
}

/// Rayon d'une poignée d'édition, en points.
pub const HANDLE_RADIUS: f32 = 5.5;

/// Poignées d'édition de l'annotation sélectionnée.
pub fn handles(painter: &Painter, points: &[Pos2]) {
    for p in points {
        painter.circle_filled(*p, HANDLE_RADIUS, theme::ACCENT_2);
        painter.circle_stroke(*p, HANDLE_RADIUS, Stroke::new(1.5, Color32::BLACK));
    }
}

// --- Lot « Éditeur : annotations » -----------------------------------------------------

/// Rectangle de sélection tiré à la souris (outil Sélection, dans le vide).
pub fn marquee(painter: &Painter, rect: Rect) {
    painter.rect_filled(rect, 0.0, theme::ACCENT_2.gamma_multiply(0.08));
    selection(painter, rect.shrink(4.0));
}
