//! Outils de l'éditeur, style courant (couleur, épaisseur, options de chaque outil)
//! et création des formes à la souris.

use egui::Key;
use serde::{Deserialize, Serialize};

use crate::model::{
    Annotation, ArrowStyle, BracketSide, BubbleKind, EndShape, LineStyle, PALETTE, Pt, Rgba, Shape,
    StepFormat, StepTail, TextStyle,
};
use crate::ui::icons;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    // --- Lot « Éditeur : annotations »
    /// Polygone / ligne brisée, clic après clic.
    Polygon,
    /// Accolade.
    Bracket,
}

/// Outils : icône, nom, touche.
pub const TOOLS: [(Tool, &str, &str, Key); 17] = [
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
    (Tool::Stamp, icons::STAMP, "Tampons et icônes", Key::K),
    // --- Lot « Éditeur : annotations » (G et J étaient libres).
    (
        Tool::Polygon,
        icons::CURVE,
        "Polygone (clics ; double-clic ou Entrée : finir ; clic sur le 1er point : fermer)",
        Key::G,
    ),
    (Tool::Bracket, "{}", "Accolade", Key::J),
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

/// Lot « Aperçus et tampons » : autres tampons « Classique », pris parmi les icônes
/// Lucide déjà embarquées (en plus de [`STAMPS`]).
pub const MORE_STAMPS: [(&str, &str); 10] = [
    (icons::OK, "Coche"),
    (icons::CLOSE, "Croix"),
    (icons::ADD, "Plus"),
    (icons::ARROW_BIG, "Grosse flèche"),
    (icons::PIN, "Punaise"),
    (icons::MAGNIFIER, "Loupe"),
    (icons::SETTINGS, "Réglages"),
    (icons::EDIT, "Crayon"),
    (icons::DELETE, "Corbeille"),
    (icons::SCREEN, "Écran"),
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
    // --- Lot « Éditeur : annotations » : options en plus (valeurs par défaut = rendu
    // d'origine).
    /// Opacité, de 0,1 à 1.
    pub opacity: f32,
    pub dash: LineStyle,
    /// Rectangle et ellipse : assombrir l'extérieur.
    pub dim_outside: bool,
    /// Rectangle et ellipse : flouter l'extérieur.
    pub blur_outside: bool,
    pub text_style: TextStyle,
    pub bubble: BubbleKind,
    pub step_format: StepFormat,
    pub step_tail: StepTail,
    /// Couleur du numéro des pastilles (`None` : automatique).
    pub step_text: Option<Rgba>,
    /// Polygone fermé.
    pub poly_closed: bool,
    pub poly_start: EndShape,
    pub poly_end: EndShape,
    pub bracket: BracketSide,
    // --- Lot « Aperçus et tampons »
    /// Taille des tampons, en multiple de S / M / L (curseur « Taille » du panneau des
    /// tampons). 1 : taille d'origine, non écrite dans `editor.json`.
    #[serde(skip_serializing_if = "is_unit_scale")]
    pub stamp_scale: f32,
}

fn is_unit_scale(v: &f32) -> bool {
    *v == 1.0
}

/// Bornes du curseur « Taille » des tampons.
pub const STAMP_SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.25..=4.0;

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
            opacity: 1.0,
            dash: LineStyle::Solid,
            dim_outside: false,
            blur_outside: false,
            text_style: TextStyle::default(),
            bubble: BubbleKind::Tail,
            step_format: StepFormat::Number,
            step_tail: StepTail::Pointer,
            step_text: None,
            poly_closed: false,
            poly_start: EndShape::Plain,
            poly_end: EndShape::Plain,
            bracket: BracketSide::Left,
            stamp_scale: 1.0,
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
            stamp: self.size.stamp() * density * self.stamp_factor(),
        }
    }

    /// Multiple de la taille des tampons, borné (une valeur lue de travers reste
    /// utilisable).
    pub fn stamp_factor(&self) -> f32 {
        if self.stamp_scale.is_finite() {
            self.stamp_scale
                .clamp(*STAMP_SCALE_RANGE.start(), *STAMP_SCALE_RANGE.end())
        } else {
            1.0
        }
    }

    /// Taille d'un tampon texte à ce style.
    pub fn label_size(&self, m: Metrics) -> f32 {
        label_size(m) * self.stamp_factor()
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
        ann.fill = self.fill
            && matches!(
                ann.shape,
                Shape::Rect { .. } | Shape::Ellipse { .. } | Shape::Polygon { .. }
            );
        ann.shadow = self.shadow && ann.can_have_shadow();
        self.apply_options(&mut ann);
        ann
    }

    /// Options du lot « annotations » qui ont un sens pour la forme de `ann`.
    pub fn apply_options(&self, ann: &mut Annotation) {
        ann.opacity = self.opacity.clamp(0.1, 1.0);
        ann.dash = if is_stroked(&ann.shape) {
            self.dash
        } else {
            LineStyle::Solid
        };
        let closed = matches!(ann.shape, Shape::Rect { .. } | Shape::Ellipse { .. });
        ann.dim_outside = closed && self.dim_outside;
        ann.blur_outside = closed && self.blur_outside;
        match ann.shape {
            Shape::Text { .. } => ann.text_style = self.text_style,
            Shape::Bubble { .. } => {
                ann.text_style = self.text_style;
                ann.bubble = self.bubble;
            }
            Shape::Step { .. } => {
                ann.step_format = self.step_format;
                ann.step_tail = self.step_tail;
                ann.text_color = self.step_text;
            }
            _ => {}
        }
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
                // Un tampon texte ou image choisi ne change pas une icône posée.
                if is_glyph_stamp(&self.stamp) {
                    icon.clone_from(&self.stamp);
                }
                *size = m.stamp;
            }
            Shape::Polygon {
                closed, start, end, ..
            } => {
                *closed = self.poly_closed;
                *start = self.poly_start;
                *end = self.poly_end;
                ann.fill = self.fill;
            }
            Shape::Bracket { side, .. } => *side = self.bracket,
            Shape::Label { size, .. } => *size = self.label_size(m),
            _ => {}
        }
        self.apply_options(ann);
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
            Shape::Text { boxed, .. } => {
                self.boxed = *boxed;
                self.text_style = ann.text_style;
            }
            Shape::Magnifier { zoom, .. } => self.zoom = *zoom,
            Shape::Stamp { icon, .. } => self.stamp.clone_from(icon),
            Shape::Polygon {
                closed, start, end, ..
            } => {
                self.poly_closed = *closed;
                self.poly_start = *start;
                self.poly_end = *end;
                self.fill = ann.fill;
            }
            Shape::Bracket { side, .. } => self.bracket = *side,
            Shape::Label { text, .. } => self.stamp = format!("{LABEL_PREFIX}{text}"),
            Shape::Bubble { .. } => {
                self.text_style = ann.text_style;
                self.bubble = ann.bubble;
            }
            Shape::Step { .. } => {
                self.step_format = ann.step_format;
                self.step_tail = ann.step_tail;
                self.step_text = ann.text_color;
            }
            _ => {}
        }
        self.opacity = ann.opacity;
        if is_stroked(&ann.shape) {
            self.dash = ann.dash;
        }
        if matches!(ann.shape, Shape::Rect { .. } | Shape::Ellipse { .. }) {
            self.dim_outside = ann.dim_outside;
            self.blur_outside = ann.blur_outside;
        }
    }
}

// --- Lot « Éditeur : annotations » : tampons par catégorie --------------------------

/// Préfixe d'un tampon texte dans [`Style::stamp`] (`texte:BROUILLON`).
pub const LABEL_PREFIX: &str = "texte:";
/// Préfixe d'un tampon personnel (`fichier:logo.png`, dans `<données>/stamps/`).
pub const FILE_PREFIX: &str = "fichier:";

/// La forme a-t-elle un trait (qui peut être pointillé) ?
pub fn is_stroked(shape: &Shape) -> bool {
    matches!(
        shape,
        Shape::Arrow { .. }
            | Shape::Line { .. }
            | Shape::Rect { .. }
            | Shape::Ellipse { .. }
            | Shape::Pen { .. }
            | Shape::Polygon { .. }
            | Shape::Bracket { .. }
    )
}

/// Le tampon est-il une icône (glyphe Lucide ou souris en vecteurs) ?
pub fn is_glyph_stamp(stamp: &str) -> bool {
    !stamp.starts_with(LABEL_PREFIX) && !stamp.starts_with(FILE_PREFIX)
}

/// Le tampon est-il un glyphe de la police d'icônes (ni souris, ni émoji, ni texte,
/// ni image) ?
pub fn is_icon_glyph(stamp: &str) -> bool {
    // Les glyphes sont un seul caractère ; les autres tampons ont un préfixe `nom:`.
    is_glyph_stamp(stamp) && !stamp.contains(':')
}

/// Taille d'un tampon texte.
pub fn label_size(m: Metrics) -> f32 {
    m.text * 1.3
}

/// Tampons « souris » : pointeurs, clics et boutons.
pub const MOUSE_STAMPS: [(&str, &str); 10] = [
    (icons::SELECT, "Pointeur"),
    (icons::STAMP_CLICK, "Clic"),
    ("souris:gauche", "Bouton gauche"),
    ("souris:droite", "Bouton droit"),
    ("souris:molette", "Molette"),
    ("texte:Cliquer ici", "Cliquer ici"),
    ("texte:Double-clic", "Double-clic"),
    ("texte:Clic gauche", "Clic gauche"),
    ("texte:Clic droit", "Clic droit"),
    ("texte:Glisser-déposer", "Glisser-déposer"),
];

/// Tampons texte, encadrés comme un tampon encreur.
pub const TEXT_STAMPS: [&str; 14] = [
    "APPROUVÉ",
    "VALIDÉ",
    "REFUSÉ",
    "BROUILLON",
    "CONFIDENTIEL",
    "URGENT",
    "À FAIRE",
    "AVANT",
    "APRÈS",
    "VRAI",
    "FAUX",
    "APPROVED",
    "DRAFT",
    "CONFIDENTIAL",
];

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
        Shape::Stamp { .. } | Shape::Image { .. } | Shape::Label { .. } => Tool::Stamp,
        Shape::Polygon { .. } => Tool::Polygon,
        Shape::Bracket { .. } => Tool::Bracket,
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
        Tool::Bracket => Shape::Bracket {
            a: p,
            b: p,
            side: style.bracket,
        },
        Tool::Select | Tool::Text | Tool::Bubble | Tool::Stamp | Tool::Polygon => return None,
    })
}

/// Prolonge le tracé jusqu'à `p`. ⇧ : angle multiple de 45° (traits) ou carré/cercle.
pub fn extend_shape(shape: &mut Shape, p: Pt, shift: bool) {
    match shape {
        // Flèche : on appuie sur ce qu'on montre (la pointe reste là) et on glisse
        // vers le départ de la flèche.
        Shape::Arrow { from, to, .. } => {
            *from = if shift { snap_45(*to, p) } else { p };
        }
        Shape::Line { from, to } => {
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
        // Glisser : on appuie sur ce qu'on montre (la pointe y reste) et la pastille
        // suit la souris. Revenu sur le point de départ : pastille sans pointe.
        Shape::Step { at, tail, .. } => {
            let target = tail.unwrap_or(*at);
            if target.dist(p) > 4.0 {
                *tail = Some(target);
                *at = p;
            } else {
                *at = target;
                *tail = None;
            }
        }
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
        Shape::Bracket { b, .. } => *b = p,
        Shape::Text { .. }
        | Shape::Bubble { .. }
        | Shape::Stamp { .. }
        | Shape::Polygon { .. }
        | Shape::Image { .. }
        | Shape::Label { .. } => {}
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
    fn dragging_a_step_points_at_the_press() {
        // On appuie sur ce qu'on montre, la pastille suit la souris.
        let mut shape =
            new_shape(Tool::Step, Pt::new(10.0, 10.0), &Style::default(), M, 3).unwrap();
        extend_shape(&mut shape, Pt::new(80.0, 10.0), false);
        extend_shape(&mut shape, Pt::new(90.0, 30.0), false);
        assert_eq!(
            shape,
            Shape::Step {
                at: Pt::new(90.0, 30.0),
                number: 3,
                tail: Some(Pt::new(10.0, 10.0))
            }
        );
        // Revenue sur le point d'appui : plus de pointe.
        extend_shape(&mut shape, Pt::new(11.0, 10.0), false);
        assert_eq!(
            shape,
            Shape::Step {
                at: Pt::new(10.0, 10.0),
                number: 3,
                tail: None
            }
        );
    }

    #[test]
    fn arrows_point_where_the_drag_starts() {
        let mut shape =
            new_shape(Tool::Arrow, Pt::new(10.0, 10.0), &Style::default(), M, 1).unwrap();
        extend_shape(&mut shape, Pt::new(100.0, 60.0), false);
        let Shape::Arrow { from, to, .. } = shape else {
            panic!("flèche attendue")
        };
        assert_eq!(to, Pt::new(10.0, 10.0), "la pointe reste sur l'appui");
        assert_eq!(from, Pt::new(100.0, 60.0));
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
    fn stamp_scale_is_optional_in_json() {
        // Taille d'origine : rien de nouveau n'est écrit dans editor.json.
        let json = serde_json::to_string(&Style::default()).unwrap();
        assert!(!json.contains("stamp_scale"), "{json}");
        let big = Style {
            stamp_scale: 2.0,
            ..Style::default()
        };
        let back: Style = serde_json::from_str(&serde_json::to_string(&big).unwrap()).unwrap();
        assert_eq!(back.metrics(1.0).stamp, Size::M.stamp() * 2.0);
        // Un style écrit avant ce lot se relit à la taille d'origine.
        let old: Style = serde_json::from_str(r#"{"size":"L"}"#).unwrap();
        assert_eq!(old.stamp_factor(), 1.0);
        let emoji = Style {
            stamp: format!("{}x", crate::render::EMOJI_PREFIX),
            ..Style::default()
        };
        assert!(is_glyph_stamp(&emoji.stamp) && !is_icon_glyph(&emoji.stamp));
        assert!(is_icon_glyph(STAMPS[0].0));
    }

    #[test]
    fn every_tool_has_its_own_key() {
        let mut keys: Vec<Key> = TOOLS.iter().map(|t| t.3).collect();
        keys.sort_by_key(|k| k.name());
        keys.dedup();
        assert_eq!(keys.len(), TOOLS.len());
    }
}
