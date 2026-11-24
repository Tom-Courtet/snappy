//! Outils de l'éditeur, style courant (couleur, épaisseur, options de chaque outil)
//! et création des formes à la souris.

use egui::Key;
use serde::{Deserialize, Serialize};

use crate::model::{Annotation, ArrowStyle, PALETTE, Pt, Rgba, Shape};
use crate::ui::icons;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    Select,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Highlight,
    Spotlight,
    Blur,
    Pixelate,
    Pen,
    Text,
    Bubble,
    Step,
    Magnifier,
    Stamp,
}

/// Outils : icône, nom, touche.
pub const TOOLS: [(Tool, &str, &str, Key); 15] = [
    (Tool::Select, icons::SELECT, "Sélection", Key::V),
    (Tool::Arrow, icons::ARROW, "Flèche", Key::A),
    (Tool::Line, icons::LINE, "Ligne", Key::L),
    (Tool::Rect, icons::RECT, "Rectangle", Key::R),
    (Tool::Ellipse, icons::ELLIPSE, "Ellipse", Key::O),
    (Tool::Highlight, icons::HIGHLIGHT, "Surligneur", Key::H),
    (
        Tool::Spotlight,
        icons::SPOTLIGHT,
        "Projecteur (assombrit le reste)",
        Key::F,
    ),
    (Tool::Blur, icons::BLUR, "Flou", Key::B),
    (Tool::Pixelate, icons::PIXELATE, "Pixeliser", Key::X),
    (Tool::Pen, icons::PEN, "Crayon", Key::P),
    (Tool::Text, icons::TEXT, "Texte", Key::T),
    (Tool::Bubble, icons::BUBBLE, "Bulle", Key::U),
    (
        Tool::Step,
        icons::STEP,
        "Pastille numérotée (glisser : avec une pointe)",
        Key::N,
    ),
    (Tool::Magnifier, icons::MAGNIFIER, "Loupe", Key::M),
    (Tool::Stamp, icons::STAMP, "Tampon", Key::K),
];

/// Tampons proposés : glyphe Lucide et nom.
pub const STAMPS: [(&str, &str); 14] = [
    (icons::STAMP_CHECK, "Validé"),
    (icons::STAMP_CROSS, "Refusé"),
    (icons::WARNING, "Attention"),
    (icons::STAMP_ALERT, "Important"),
    (icons::STAMP_INFO, "Information"),
    (icons::STAMP_QUESTION, "Question"),
    (icons::SELECT, "Pointeur"),
    (icons::STAMP_CLICK, "Clic"),
    (icons::STAMP_THUMB, "Pouce"),
    (icons::STAMP_HEART, "Cœur"),
    (icons::STAR, "Étoile"),
    (icons::STAMP_BAN, "Interdit"),
    (icons::STAMP_FLAG, "Drapeau"),
    (icons::STAMP_IDEA, "Idée"),
];

/// Épaisseur du trait et taille du texte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Size {
    S,
    M,
    L,
}

impl Size {
    /// Trait, en points d'écran (multiplié ensuite par la densité de la capture).
    pub fn stroke(self) -> f32 {
        match self {
            Self::S => 2.0,
            Self::M => 4.0,
            Self::L => 7.0,
        }
    }

    /// Texte, en points d'écran.
    pub fn text(self) -> f32 {
        match self {
            Self::S => 14.0,
            Self::M => 20.0,
            Self::L => 32.0,
        }
    }

    /// Tampons, en points d'écran.
    pub fn stamp(self) -> f32 {
        match self {
            Self::S => 24.0,
            Self::M => 40.0,
            Self::L => 64.0,
        }
    }
}

/// Style courant : appliqué aux nouvelles annotations, et à la sélection quand on le
/// change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub color: Rgba,
    pub size: Size,
    pub arrow: ArrowStyle,
    /// Rectangle et ellipse pleins.
    pub fill: bool,
    /// Rectangle aux coins arrondis.
    pub rounded: bool,
    /// Texte sur un fond de la couleur.
    pub boxed: bool,
    pub shadow: bool,
    /// Grossissement des loupes.
    pub zoom: f32,
    /// Glyphe du tampon.
    pub stamp: String,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            color: PALETTE[0].0,
            size: Size::M,
            arrow: ArrowStyle::Simple,
            fill: false,
            rounded: false,
            boxed: false,
            shadow: false,
            zoom: 2.0,
            stamp: STAMPS[0].0.to_owned(),
        }
    }
}

/// Mesures du style en pixels de l'image (`density` : pixels par point).
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub stroke: f32,
    pub text: f32,
    pub stamp: f32,
}

impl Style {
    pub fn metrics(&self, density: f32) -> Metrics {
        Metrics {
            stroke: self.size.stroke() * density,
            text: self.size.text() * density,
            stamp: self.size.stamp() * density,
        }
    }

    /// Arrondi des coins d'un rectangle.
    pub fn radius(&self, m: Metrics) -> f32 {
        if self.rounded {
            (m.stroke * 3.0).max(8.0)
        } else {
            0.0
        }
    }

    /// Nouvelle annotation de cette forme, à ce style.
    pub fn annotate(&self, shape: Shape, m: Metrics) -> Annotation {
        let mut ann = Annotation::new(shape, self.color, m.stroke);
        ann.fill = self.fill && matches!(ann.shape, Shape::Rect { .. } | Shape::Ellipse { .. });
        ann.shadow = self.shadow && ann.can_have_shadow();
        ann
    }

    /// Applique ce style à une annotation existante (ce qui a un sens pour sa forme).
    pub fn apply(&self, ann: &mut Annotation, m: Metrics) {
        ann.color = self.color;
        ann.width = m.stroke;
        ann.shadow = self.shadow && ann.can_have_shadow();
        let radius = self.radius(m);
        match &mut ann.shape {
            Shape::Arrow { style, .. } => *style = self.arrow,
            Shape::Rect { radius: r, .. } => {
                *r = radius;
                ann.fill = self.fill;
            }
            Shape::Ellipse { .. } => ann.fill = self.fill,
            Shape::Text { size, boxed, .. } => {
                *size = m.text;
                *boxed = self.boxed;
            }
            Shape::Bubble { size, .. } => *size = m.text,
            Shape::Magnifier { zoom, .. } => *zoom = self.zoom,
            Shape::Stamp { icon, size, .. } => {
                icon.clone_from(&self.stamp);
                *size = m.stamp;
            }
            _ => {}
        }
    }

    /// Reprend les options d'une annotation qu'on vient de sélectionner : les
    /// réglages affichés sont les siens.
    pub fn adopt(&mut self, ann: &Annotation) {
        self.color = ann.color;
        self.shadow = ann.shadow;
        match &ann.shape {
            Shape::Arrow { style, .. } => self.arrow = *style,
            Shape::Rect { radius, .. } => {
                self.rounded = *radius > 0.0;
                self.fill = ann.fill;
            }
            Shape::Ellipse { .. } => self.fill = ann.fill,
            Shape::Text { boxed, .. } => self.boxed = *boxed,
            Shape::Magnifier { zoom, .. } => self.zoom = *zoom,
            Shape::Stamp { icon, .. } => self.stamp.clone_from(icon),
            _ => {}
        }
    }
}

/// Outil correspondant à une annotation (pour savoir quelles options afficher).
pub fn tool_of(shape: &Shape) -> Tool {
    match shape {
        Shape::Arrow { .. } => Tool::Arrow,
        Shape::Line { .. } => Tool::Line,
        Shape::Rect { .. } => Tool::Rect,
        Shape::Ellipse { .. } => Tool::Ellipse,
        Shape::Highlight { .. } => Tool::Highlight,
        Shape::Spotlight { .. } => Tool::Spotlight,
        Shape::Pixelate { .. } => Tool::Pixelate,
        Shape::Blur { .. } => Tool::Blur,
        Shape::Pen { .. } => Tool::Pen,
        Shape::Text { .. } => Tool::Text,
        Shape::Bubble { .. } => Tool::Bubble,
        Shape::Step { .. } => Tool::Step,
        Shape::Magnifier { .. } => Tool::Magnifier,
        Shape::Stamp { .. } => Tool::Stamp,
    }
}

/// Forme qui commence au point `p` quand on glisse avec cet outil. `None` : outil
/// qui ne se trace pas (sélection, texte, bulle, tampon).
pub fn new_shape(tool: Tool, p: Pt, style: &Style, m: Metrics, next_step: u32) -> Option<Shape> {
    Some(match tool {
        Tool::Arrow => Shape::Arrow {
            from: p,
            to: p,
            style: style.arrow,
            bend: None,
        },
        Tool::Line => Shape::Line { from: p, to: p },
        Tool::Rect => Shape::Rect {
            a: p,
            b: p,
            radius: style.radius(m),
        },
        Tool::Ellipse => Shape::Ellipse { a: p, b: p },
        Tool::Highlight => Shape::Highlight { a: p, b: p },
        Tool::Spotlight => Shape::Spotlight { a: p, b: p },
        Tool::Blur => Shape::Blur { a: p, b: p },
        Tool::Pixelate => Shape::Pixelate { a: p, b: p },
        Tool::Pen => Shape::Pen { points: vec![p] },
        Tool::Step => Shape::Step {
            at: p,
            number: next_step,
            tail: None,
        },
        Tool::Magnifier => Shape::Magnifier {
            source: p,
            radius: 0.0,
            at: p,
            zoom: style.zoom,
        },
        Tool::Select | Tool::Text | Tool::Bubble | Tool::Stamp => return None,
    })
}

/// Prolonge le tracé jusqu'à `p`. ⇧ : angle multiple de 45° (traits) ou carré/cercle.
pub fn extend_shape(shape: &mut Shape, p: Pt, shift: bool) {
    match shape {
        Shape::Arrow { from, to, .. } | Shape::Line { from, to } => {
            *to = if shift { snap_45(*from, p) } else { p };
        }
        Shape::Rect { a, b, .. } | Shape::Ellipse { a, b } => {
            *b = if shift { square(*a, p) } else { p };
        }
        Shape::Highlight { b, .. }
        | Shape::Spotlight { b, .. }
        | Shape::Blur { b, .. }
        | Shape::Pixelate { b, .. } => *b = p,
        Shape::Pen { points } => {
            if points.last().is_none_or(|last| last.dist(p) >= 1.5) {
                points.push(p);
            }
        }
        // Glisser depuis la pastille : sa pointe suit la souris.
        Shape::Step { at, tail, .. } => *tail = (at.dist(p) > 4.0).then_some(p),
        // Le rayon de la zone grossie suit la souris ; la loupe se place en haut à
        // droite, sans la recouvrir.
        Shape::Magnifier {
            source,
            radius,
            at,
            zoom,
        } => {
            *radius = source.dist(p);
            let d = *radius * (*zoom + 1.0) * 0.85;
            *at = source.offset(d, -d);
        }
        Shape::Text { .. } | Shape::Bubble { .. } | Shape::Stamp { .. } => {}
    }
}

pub fn snap_45(from: Pt, p: Pt) -> Pt {
    let (dx, dy) = (p.x - from.x, p.y - from.y);
    let len = (dx * dx + dy * dy).sqrt();
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    Pt::new(from.x + len * angle.cos(), from.y + len * angle.sin())
}

pub fn square(a: Pt, p: Pt) -> Pt {
    let side = (p.x - a.x).abs().max((p.y - a.y).abs());
    Pt::new(
        a.x + side * (p.x - a.x).signum(),
        a.y + side * (p.y - a.y).signum(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: Metrics = Metrics {
        stroke: 8.0,
        text: 40.0,
        stamp: 80.0,
    };

    #[test]
    fn shift_snaps_lines_to_45_degrees() {
        let p = snap_45(Pt::new(0.0, 0.0), Pt::new(100.0, 8.0));
        assert!((p.y).abs() < 1e-3 && (p.x - 100.32).abs() < 0.1, "{p:?}");
        let p = snap_45(Pt::new(0.0, 0.0), Pt::new(50.0, 46.0));
        assert!((p.x - p.y).abs() < 1e-3, "{p:?}");
    }

    #[test]
    fn shift_makes_squares_in_any_direction() {
        assert_eq!(
            square(Pt::new(10.0, 10.0), Pt::new(0.0, 40.0)),
            Pt::new(-20.0, 40.0)
        );
    }

    #[test]
    fn pen_skips_points_too_close() {
        let mut shape = new_shape(Tool::Pen, Pt::new(0.0, 0.0), &Style::default(), M, 1).unwrap();
        extend_shape(&mut shape, Pt::new(0.5, 0.5), false);
        extend_shape(&mut shape, Pt::new(3.0, 0.0), false);
        assert_eq!(
            shape,
            Shape::Pen {
                points: vec![Pt::new(0.0, 0.0), Pt::new(3.0, 0.0)]
            }
        );
    }

    #[test]
    fn dragging_a_step_gives_it_a_tail() {
        let mut shape =
            new_shape(Tool::Step, Pt::new(10.0, 10.0), &Style::default(), M, 3).unwrap();
        extend_shape(&mut shape, Pt::new(80.0, 10.0), false);
        assert_eq!(
            shape,
            Shape::Step {
                at: Pt::new(10.0, 10.0),
                number: 3,
                tail: Some(Pt::new(80.0, 10.0))
            }
        );
    }

    #[test]
    fn magnifier_lens_never_covers_its_source() {
        let style = Style {
            zoom: 3.0,
            ..Style::default()
        };
        let mut shape = new_shape(Tool::Magnifier, Pt::new(100.0, 100.0), &style, M, 1).unwrap();
        extend_shape(&mut shape, Pt::new(120.0, 100.0), false);
        let Shape::Magnifier {
            source,
            radius,
            at,
            zoom,
        } = shape
        else {
            unreachable!()
        };
        assert_eq!(radius, 20.0);
        assert!(source.dist(at) > radius + radius * zoom);
    }

    #[test]
    fn style_round_trips_through_an_annotation() {
        let style = Style {
            arrow: ArrowStyle::Double,
            shadow: true,
            color: PALETTE[4].0,
            ..Style::default()
        };
        let shape = new_shape(Tool::Arrow, Pt::default(), &style, M, 1).unwrap();
        let ann = style.annotate(shape, M);
        let mut other = Style::default();
        other.adopt(&ann);
        assert_eq!(other.arrow, ArrowStyle::Double);
        assert!(other.shadow);
        assert_eq!(other.color, PALETTE[4].0);
    }

    #[test]
    fn fill_only_applies_to_closed_shapes() {
        let style = Style {
            fill: true,
            ..Style::default()
        };
        let rect = new_shape(Tool::Rect, Pt::default(), &style, M, 1).unwrap();
        assert!(style.annotate(rect, M).fill);
        let line = new_shape(Tool::Line, Pt::default(), &style, M, 1).unwrap();
        assert!(!style.annotate(line, M).fill);
    }

    #[test]
    fn every_tool_has_its_own_key() {
        let mut keys: Vec<Key> = TOOLS.iter().map(|t| t.3).collect();
        keys.sort_by_key(|k| k.name());
        keys.dedup();
        assert_eq!(keys.len(), TOOLS.len());
    }
}
