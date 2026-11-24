//! Annotations d'une capture, en pixels de l'image d'origine.
//!
//! Elles sont **non destructives** : l'image d'origine n'est jamais modifiée, les
//! annotations sont enregistrées à côté (`<capture>.json`) et rejouées par
//! [`crate::render`] à chaque ouverture et à chaque export. On peut donc retoucher une
//! capture des semaines plus tard.
//!
//! Compatibilité : les champs ajoutés depuis la première version ont une valeur par
//! défaut, les anciens fichiers se relisent tels quels.

use serde::{Deserialize, Serialize};

/// Point en pixels de l'image.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn offset(self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }

    pub fn dist(self, other: Pt) -> f32 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }

    pub fn lerp(self, other: Pt, t: f32) -> Self {
        Self::new(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
        )
    }
}

/// Couleur sRGB, alpha non prémultiplié.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self(r, g, b, 255)
    }

    /// Luminance relative (WCAG).
    pub fn luminance(self) -> f32 {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(self.0) + 0.7152 * lin(self.1) + 0.0722 * lin(self.2)
    }

    /// Noir ou blanc, selon ce qui se lit le mieux sur cette couleur.
    pub fn contrast(self) -> Self {
        if self.luminance() > 0.35 {
            Self::rgb(0, 0, 0)
        } else {
            Self::rgb(255, 255, 255)
        }
    }

    /// `#RRGGBB`.
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }

    /// Lit `#RRGGBB`, `RRGGBB` ou `#RGB` (opaque).
    pub fn from_hex(text: &str) -> Option<Self> {
        let t = text.trim().trim_start_matches('#');
        let byte = |s: &str| u8::from_str_radix(s, 16).ok();
        match t.len() {
            6 => Some(Self::rgb(byte(&t[0..2])?, byte(&t[2..4])?, byte(&t[4..6])?)),
            3 => {
                let d = |i: usize| byte(&t[i..i + 1]).map(|v| v * 17);
                Some(Self::rgb(d(0)?, d(1)?, d(2)?))
            }
            _ => None,
        }
    }
}

/// Palette proposée dans l'éditeur.
pub const PALETTE: [(Rgba, &str); 8] = [
    (Rgba::rgb(0xff, 0x3b, 0x30), "Rouge"),
    (Rgba::rgb(0xff, 0x95, 0x00), "Orange"),
    (Rgba::rgb(0xff, 0xd6, 0x0a), "Jaune"),
    (Rgba::rgb(0xc6, 0xff, 0x34), "Lime"),
    (Rgba::rgb(0x0a, 0x84, 0xff), "Bleu"),
    (Rgba::rgb(0x7f, 0x3a, 0xed), "Violet"),
    (Rgba::rgb(0xff, 0xff, 0xff), "Blanc"),
    (Rgba::rgb(0x00, 0x00, 0x00), "Noir"),
];

/// Forme d'une flèche.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrowStyle {
    /// Trait et pointe pleine.
    #[default]
    Simple,
    /// Pointe aux deux bouts.
    Double,
    /// Grosse flèche pleine, effilée vers la queue.
    Big,
    /// Trait terminé par deux ronds (relier deux éléments).
    Connector,
}

/// Forme d'une annotation. Les rectangles sont donnés par deux coins opposés
/// quelconques (`a`, `b`), tels que tracés à la souris.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Shape {
    /// `bend` : point par lequel passe la flèche courbée (sa poignée du milieu).
    Arrow {
        from: Pt,
        to: Pt,
        #[serde(default)]
        style: ArrowStyle,
        #[serde(default)]
        bend: Option<Pt>,
    },
    Line {
        from: Pt,
        to: Pt,
    },
    /// `radius` : arrondi des coins (0 : coins carrés).
    Rect {
        a: Pt,
        b: Pt,
        #[serde(default)]
        radius: f32,
    },
    Ellipse {
        a: Pt,
        b: Pt,
    },
    /// Surligneur : rectangle translucide posé en mode « produit », le texte dessous
    /// reste lisible.
    Highlight {
        a: Pt,
        b: Pt,
    },
    /// Projecteur : tout est assombri sauf ce rectangle.
    Spotlight {
        a: Pt,
        b: Pt,
    },
    /// Zone pixelisée (données sensibles).
    Pixelate {
        a: Pt,
        b: Pt,
    },
    /// Zone floutée (données sensibles, ou pour attirer l'œil ailleurs).
    Blur {
        a: Pt,
        b: Pt,
    },
    /// Trait à main levée.
    Pen {
        points: Vec<Pt>,
    },
    /// Texte, `at` = coin haut gauche, `size` = hauteur de police en pixels.
    /// `boxed` : sur un fond de la couleur, texte en noir ou blanc.
    Text {
        at: Pt,
        text: String,
        size: f32,
        #[serde(default)]
        boxed: bool,
    },
    /// Bulle de dialogue : cadre arrondi autour du texte (coin haut gauche `at`) et
    /// pointe vers `tail`.
    Bubble {
        at: Pt,
        text: String,
        size: f32,
        tail: Pt,
    },
    /// Pastille numérotée (tutoriels), centrée sur `at`. `tail` : pointe vers
    /// l'élément désigné.
    Step {
        at: Pt,
        number: u32,
        #[serde(default)]
        tail: Option<Pt>,
    },
    /// Loupe : un disque centré sur `at` qui montre l'image d'origine autour de
    /// `source` (rayon `radius`), grossie `zoom` fois.
    Magnifier {
        source: Pt,
        radius: f32,
        at: Pt,
        zoom: f32,
    },
    /// Tampon : une icône (glyphe de la police Lucide) centrée sur `at`, de côté
    /// `size`.
    Stamp {
        at: Pt,
        icon: String,
        size: f32,
    },
}

/// Une annotation : forme, couleur et épaisseur du trait (pixels de l'image).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub shape: Shape,
    pub color: Rgba,
    pub width: f32,
    /// Rectangle et ellipse pleins (masquer, pastille de couleur).
    #[serde(default)]
    pub fill: bool,
    /// Ombre portée douce.
    #[serde(default)]
    pub shadow: bool,
}

impl Annotation {
    pub fn new(shape: Shape, color: Rgba, width: f32) -> Self {
        Self {
            shape,
            color,
            width,
            fill: false,
            shadow: false,
        }
    }
}

/// Rectangle aligné, `min` en haut à gauche.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: Pt,
    pub max: Pt,
}

impl Bounds {
    pub fn from_corners(a: Pt, b: Pt) -> Self {
        Self {
            min: Pt::new(a.x.min(b.x), a.y.min(b.y)),
            max: Pt::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    pub fn from_points(points: impl IntoIterator<Item = Pt>) -> Option<Self> {
        let mut it = points.into_iter();
        let first = it.next()?;
        Some(it.fold(
            Self {
                min: first,
                max: first,
            },
            |b, p| Self {
                min: Pt::new(b.min.x.min(p.x), b.min.y.min(p.y)),
                max: Pt::new(b.max.x.max(p.x), b.max.y.max(p.y)),
            },
        ))
    }

    pub fn union(self, other: Bounds) -> Self {
        Self::from_points([self.min, self.max, other.min, other.max]).unwrap_or(self)
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn center(&self) -> Pt {
        self.min.lerp(self.max, 0.5)
    }

    pub fn expand(self, by: f32) -> Self {
        Self {
            min: self.min.offset(-by, -by),
            max: self.max.offset(by, by),
        }
    }

    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// Les 4 coins, dans l'ordre : haut gauche, haut droit, bas droit, bas gauche.
    pub fn corners(&self) -> [Pt; 4] {
        [
            self.min,
            Pt::new(self.max.x, self.min.y),
            self.max,
            Pt::new(self.min.x, self.max.y),
        ]
    }
}

/// Rayon d'une pastille numérotée pour une épaisseur de trait donnée.
pub fn step_radius(width: f32) -> f32 {
    (width * 3.5).max(14.0)
}

/// Marges intérieures d'un texte sur fond ou d'une bulle, pour une taille de police.
pub fn text_padding(size: f32) -> (f32, f32) {
    (size * 0.45, size * 0.28)
}

/// Point de contrôle de la courbe quadratique qui passe par `bend` à mi-chemin.
pub fn bend_control(from: Pt, to: Pt, bend: Pt) -> Pt {
    let mid = from.lerp(to, 0.5);
    Pt::new(2.0 * bend.x - mid.x, 2.0 * bend.y - mid.y)
}

/// Points le long d'une flèche (droite ou courbée), pour le tracé et la sélection.
pub fn arrow_points(from: Pt, to: Pt, bend: Option<Pt>) -> Vec<Pt> {
    match bend {
        None => vec![from, to],
        Some(b) => {
            let c = bend_control(from, to, b);
            (0..=24)
                .map(|i| {
                    let t = i as f32 / 24.0;
                    let u = 1.0 - t;
                    Pt::new(
                        u * u * from.x + 2.0 * u * t * c.x + t * t * to.x,
                        u * u * from.y + 2.0 * u * t * c.y + t * t * to.y,
                    )
                })
                .collect()
        }
    }
}

/// Position par défaut de la poignée de pointe d'une pastille sans pointe.
pub fn step_tail_handle(at: Pt, width: f32) -> Pt {
    let r = step_radius(width) * 1.6;
    at.offset(r, r)
}

/// Distance d'un point à un segment.
fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.dist(Pt::new(a.x + t * dx, a.y + t * dy))
}

fn dist_to_polyline(p: Pt, points: &[Pt]) -> f32 {
    match points {
        [] => f32::INFINITY,
        [only] => p.dist(*only),
        _ => points
            .windows(2)
            .map(|w| dist_to_segment(p, w[0], w[1]))
            .fold(f32::INFINITY, f32::min),
    }
}

/// Mesure d'un texte : `(largeur, hauteur)` pour un texte et une taille de police.
/// Fournie par le moteur de rendu, seul à connaître la police.
pub trait TextSize: Fn(&str, f32) -> (f32, f32) {}
impl<F: Fn(&str, f32) -> (f32, f32)> TextSize for F {}

impl Annotation {
    /// Boîte englobante, sans l'épaisseur du trait.
    pub fn bounds(&self, text_size: impl TextSize) -> Bounds {
        let r = |a: Pt, b: Pt| Bounds::from_corners(a, b);
        match &self.shape {
            Shape::Arrow { from, to, bend, .. } => {
                Bounds::from_points(arrow_points(*from, *to, *bend)).unwrap_or(r(*from, *to))
            }
            Shape::Line { from, to } => r(*from, *to),
            Shape::Rect { a, b, .. }
            | Shape::Ellipse { a, b }
            | Shape::Highlight { a, b }
            | Shape::Spotlight { a, b }
            | Shape::Pixelate { a, b }
            | Shape::Blur { a, b } => r(*a, *b),
            Shape::Pen { points } => {
                Bounds::from_points(points.iter().copied()).unwrap_or(Bounds {
                    min: Pt::default(),
                    max: Pt::default(),
                })
            }
            Shape::Text {
                at,
                text,
                size,
                boxed,
            } => {
                let (w, h) = text_size(text, *size);
                let (px, py) = if *boxed {
                    text_padding(*size)
                } else {
                    (0.0, 0.0)
                };
                Bounds {
                    min: at.offset(-px, -py),
                    max: at.offset(w + px, h + py),
                }
            }
            Shape::Bubble {
                at,
                text,
                size,
                tail,
            } => bubble_box(*at, text, *size, &text_size).union(r(*tail, *tail)),
            Shape::Step { at, tail, .. } => {
                let rad = step_radius(self.width);
                let b = Bounds {
                    min: at.offset(-rad, -rad),
                    max: at.offset(rad, rad),
                };
                match tail {
                    Some(t) => b.union(r(*t, *t)),
                    None => b,
                }
            }
            Shape::Magnifier {
                source,
                radius,
                at,
                zoom,
            } => {
                let lens = radius * zoom;
                Bounds {
                    min: source.offset(-radius, -radius),
                    max: source.offset(*radius, *radius),
                }
                .union(Bounds {
                    min: at.offset(-lens, -lens),
                    max: at.offset(lens, lens),
                })
            }
            Shape::Stamp { at, size, .. } => Bounds {
                min: at.offset(-size / 2.0, -size / 2.0),
                max: at.offset(size / 2.0, size / 2.0),
            },
        }
    }

    /// Le point `p` touche-t-il l'annotation, à `tolerance` pixels près ?
    /// Les formes pleines se prennent partout, les contours seulement sur le trait.
    pub fn hit(&self, p: Pt, tolerance: f32, text_size: impl TextSize) -> bool {
        let reach = tolerance + self.width / 2.0;
        match &self.shape {
            Shape::Arrow {
                from,
                to,
                bend,
                style,
                ..
            } => {
                let extra = if *style == ArrowStyle::Big {
                    self.width * 2.0
                } else {
                    0.0
                };
                dist_to_polyline(p, &arrow_points(*from, *to, *bend)) <= reach + extra
            }
            Shape::Line { from, to } => dist_to_segment(p, *from, *to) <= reach,
            Shape::Pen { points } => dist_to_polyline(p, points) <= reach,
            Shape::Rect { a, b, .. } if !self.fill => {
                let outer = Bounds::from_corners(*a, *b).expand(reach);
                let inner = Bounds::from_corners(*a, *b).expand(-reach);
                outer.contains(p)
                    && !(inner.width() > 0.0 && inner.height() > 0.0 && inner.contains(p))
            }
            Shape::Ellipse { a, b } => {
                let bounds = Bounds::from_corners(*a, *b);
                let (rx, ry) = (bounds.width() / 2.0, bounds.height() / 2.0);
                if rx <= 0.0 || ry <= 0.0 {
                    return bounds.expand(reach).contains(p);
                }
                let c = bounds.center();
                // Distance approchée au contour : écart radial normalisé.
                let n = (((p.x - c.x) / rx).powi(2) + ((p.y - c.y) / ry).powi(2)).sqrt();
                if self.fill {
                    n <= 1.0 + reach / rx.min(ry)
                } else {
                    (n - 1.0).abs() * rx.min(ry) <= reach
                }
            }
            Shape::Step { at, tail, .. } => {
                p.dist(*at) <= step_radius(self.width) + tolerance
                    || tail
                        .is_some_and(|t| dist_to_segment(p, *at, t) <= reach.max(tolerance * 2.0))
            }
            Shape::Bubble {
                at,
                text,
                size,
                tail,
            } => {
                let b = bubble_box(*at, text, *size, &text_size);
                b.expand(tolerance).contains(p)
                    || dist_to_segment(p, b.center(), *tail) <= tolerance * 2.0
            }
            Shape::Magnifier {
                source,
                radius,
                at,
                zoom,
            } => p.dist(*at) <= radius * zoom + tolerance || p.dist(*source) <= radius + tolerance,
            Shape::Rect { .. }
            | Shape::Highlight { .. }
            | Shape::Spotlight { .. }
            | Shape::Pixelate { .. }
            | Shape::Blur { .. }
            | Shape::Text { .. }
            | Shape::Stamp { .. } => self.bounds(text_size).expand(tolerance).contains(p),
        }
    }

    /// Déplace l'annotation de (`dx`, `dy`) pixels.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        let mv = |p: &mut Pt| *p = p.offset(dx, dy);
        match &mut self.shape {
            Shape::Arrow { from, to, bend, .. } => {
                mv(from);
                mv(to);
                if let Some(b) = bend {
                    mv(b);
                }
            }
            Shape::Line { from, to } => {
                mv(from);
                mv(to);
            }
            Shape::Rect { a, b, .. }
            | Shape::Ellipse { a, b }
            | Shape::Highlight { a, b }
            | Shape::Spotlight { a, b }
            | Shape::Pixelate { a, b }
            | Shape::Blur { a, b } => {
                mv(a);
                mv(b);
            }
            Shape::Pen { points } => points.iter_mut().for_each(mv),
            Shape::Text { at, .. } | Shape::Stamp { at, .. } => mv(at),
            Shape::Bubble { at, tail, .. } => {
                mv(at);
                mv(tail);
            }
            Shape::Step { at, tail, .. } => {
                mv(at);
                if let Some(t) = tail {
                    mv(t);
                }
            }
            Shape::Magnifier { source, at, .. } => {
                mv(source);
                mv(at);
            }
        }
    }

    /// Poignées d'édition de l'annotation sélectionnée (extrémités, coins, pointes).
    pub fn handles(&self) -> Vec<Pt> {
        match &self.shape {
            Shape::Arrow { from, to, bend, .. } => {
                vec![*from, *to, bend.unwrap_or_else(|| from.lerp(*to, 0.5))]
            }
            Shape::Line { from, to } => vec![*from, *to],
            Shape::Rect { a, b, .. }
            | Shape::Ellipse { a, b }
            | Shape::Highlight { a, b }
            | Shape::Spotlight { a, b }
            | Shape::Pixelate { a, b }
            | Shape::Blur { a, b } => Bounds::from_corners(*a, *b).corners().to_vec(),
            Shape::Bubble { tail, .. } => vec![*tail],
            Shape::Step { at, tail, .. } => {
                vec![tail.unwrap_or_else(|| step_tail_handle(*at, self.width))]
            }
            Shape::Magnifier {
                source,
                radius,
                at,
                zoom,
            } => vec![*source, *at, at.offset(radius * zoom, 0.0)],
            Shape::Pen { .. } | Shape::Text { .. } | Shape::Stamp { .. } => Vec::new(),
        }
    }

    /// Déplace la poignée `index` (voir [`Self::handles`]) au point `p`.
    pub fn move_handle(&mut self, index: usize, p: Pt) {
        match &mut self.shape {
            Shape::Arrow { from, to, bend, .. } => match index {
                0 => *from = p,
                1 => *to = p,
                _ => {
                    // Presque alignée : redevient droite.
                    let straight = dist_to_segment(p, *from, *to) < 3.0;
                    *bend = (!straight).then_some(p);
                }
            },
            Shape::Line { from, to } => {
                if index == 0 {
                    *from = p;
                } else {
                    *to = p;
                }
            }
            Shape::Rect { a, b, .. }
            | Shape::Ellipse { a, b }
            | Shape::Highlight { a, b }
            | Shape::Spotlight { a, b }
            | Shape::Pixelate { a, b }
            | Shape::Blur { a, b } => {
                // Le coin opposé reste en place.
                let corners = Bounds::from_corners(*a, *b).corners();
                *a = corners[(index + 2) % 4];
                *b = p;
            }
            Shape::Bubble { tail, .. } => *tail = p,
            Shape::Step { at, tail, .. } => {
                // Ramenée dans la pastille : plus de pointe.
                let inside = p.dist(*at) <= step_radius(self.width);
                *tail = (!inside).then_some(p);
            }
            Shape::Magnifier {
                source,
                radius,
                at,
                zoom,
            } => match index {
                0 => *source = p,
                1 => *at = p,
                _ => *zoom = (p.dist(*at) / radius.max(1.0)).clamp(1.2, 8.0),
            },
            Shape::Pen { .. } | Shape::Text { .. } | Shape::Stamp { .. } => {}
        }
    }

    /// Annotation trop petite pour être gardée (simple clic avec un outil de tracé).
    pub fn is_degenerate(&self) -> bool {
        const MIN: f32 = 3.0;
        match &self.shape {
            Shape::Arrow { from, to, .. } | Shape::Line { from, to } => from.dist(*to) < MIN,
            Shape::Rect { a, b, .. }
            | Shape::Ellipse { a, b }
            | Shape::Highlight { a, b }
            | Shape::Spotlight { a, b }
            | Shape::Pixelate { a, b }
            | Shape::Blur { a, b } => {
                let r = Bounds::from_corners(*a, *b);
                r.width() < MIN || r.height() < MIN
            }
            Shape::Pen { points } => points.len() < 2,
            Shape::Text { text, .. } | Shape::Bubble { text, .. } => text.trim().is_empty(),
            Shape::Magnifier { radius, .. } => *radius < MIN,
            Shape::Step { .. } | Shape::Stamp { .. } => false,
        }
    }

    /// L'ombre portée a-t-elle un sens pour cette forme ? (Pas pour les effets qui
    /// travaillent sur les pixels.)
    pub fn can_have_shadow(&self) -> bool {
        !matches!(
            self.shape,
            Shape::Highlight { .. }
                | Shape::Spotlight { .. }
                | Shape::Pixelate { .. }
                | Shape::Blur { .. }
        )
    }
}

/// Cadre d'une bulle : le texte et ses marges.
pub fn bubble_box(at: Pt, text: &str, size: f32, text_size: &impl TextSize) -> Bounds {
    let (w, h) = text_size(text, size);
    let (px, py) = text_padding(size);
    Bounds {
        min: at.offset(-px, -py),
        max: at.offset(w + px, h + py),
    }
}

/// Annotations d'une capture, telles qu'enregistrées à côté de l'image.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Doc {
    pub annotations: Vec<Annotation>,
    // --- Lot « Capture » ---
    /// Contexte de la capture (§ 3.1 « Save capture context ») : application et
    /// fenêtre d'origine. Seulement si le réglage est activé ; absent des fichiers
    /// sinon (format inchangé).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<CaptureContext>,
    // --- Lot « Éditeur : image et effets » (voir `crate::effects`) ---
    /// Retouches de l'image d'origine (recadrage, rotation…), rejouées dans l'ordre.
    /// Les annotations sont en pixels de l'image retouchée.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<crate::effects::ImageEdit>,
    /// Bordure et effets, appliqués à l'export après les annotations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<crate::effects::BorderEffects>,
    /// Zone de travail automatique : à l'export, l'image s'agrandit (sur ce fond)
    /// pour contenir les annotations qui dépassent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas_auto: Option<crate::effects::Background>,
    /// Résolution écrite dans le PNG exporté (métadonnée seule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpi: Option<f32>,
}

// --- Lot « Capture » ---

/// D'où vient une capture : application, titre de la fenêtre, date.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureContext {
    pub app: String,
    pub title: String,
    /// Date et heure locales de la capture, ISO 8601 (`2026-11-24T10:02:37+01:00`).
    pub date: String,
}

impl CaptureContext {
    /// Contexte daté de maintenant.
    pub fn now(app: &str, title: &str) -> Self {
        Self {
            app: app.trim().to_owned(),
            title: title.trim().to_owned(),
            date: jiff::ZonedDateTime::now()
                .strftime("%Y-%m-%dT%H:%M:%S%:z")
                .to_string(),
        }
    }
}

impl Doc {
    /// Numéro de la prochaine pastille : un de plus que la plus grande.
    pub fn next_step(&self) -> u32 {
        self.annotations
            .iter()
            .filter_map(|a| match a.shape {
                Shape::Step { number, .. } => Some(number),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_text(_: &str, _: f32) -> (f32, f32) {
        (0.0, 0.0)
    }

    fn ann(shape: Shape) -> Annotation {
        Annotation::new(shape, PALETTE[0].0, 4.0)
    }

    fn arrow(from: Pt, to: Pt) -> Shape {
        Shape::Arrow {
            from,
            to,
            style: ArrowStyle::Simple,
            bend: None,
        }
    }

    fn rect(a: Pt, b: Pt) -> Shape {
        Shape::Rect { a, b, radius: 0.0 }
    }

    #[test]
    fn rect_is_hit_on_its_border_only_unless_filled() {
        let mut r = ann(rect(Pt::new(100.0, 100.0), Pt::new(10.0, 10.0)));
        assert!(r.hit(Pt::new(10.0, 50.0), 2.0, no_text));
        assert!(r.hit(Pt::new(55.0, 101.0), 2.0, no_text));
        assert!(!r.hit(Pt::new(55.0, 55.0), 2.0, no_text));
        assert!(!r.hit(Pt::new(150.0, 55.0), 2.0, no_text));
        r.fill = true;
        assert!(r.hit(Pt::new(55.0, 55.0), 2.0, no_text));
    }

    #[test]
    fn ellipse_is_hit_near_its_outline() {
        let e = ann(Shape::Ellipse {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(200.0, 100.0),
        });
        assert!(e.hit(Pt::new(100.0, 1.0), 2.0, no_text));
        assert!(e.hit(Pt::new(199.0, 50.0), 2.0, no_text));
        assert!(!e.hit(Pt::new(100.0, 50.0), 2.0, no_text));
    }

    #[test]
    fn arrow_and_pen_are_hit_along_the_stroke() {
        let a = ann(arrow(Pt::new(0.0, 0.0), Pt::new(100.0, 0.0)));
        assert!(a.hit(Pt::new(50.0, 3.0), 2.0, no_text));
        assert!(!a.hit(Pt::new(50.0, 10.0), 2.0, no_text));
        let p = ann(Shape::Pen {
            points: vec![
                Pt::new(0.0, 0.0),
                Pt::new(0.0, 100.0),
                Pt::new(100.0, 100.0),
            ],
        });
        assert!(p.hit(Pt::new(50.0, 99.0), 2.0, no_text));
        assert!(!p.hit(Pt::new(50.0, 50.0), 2.0, no_text));
    }

    #[test]
    fn a_bent_arrow_passes_through_its_middle_handle() {
        let mut a = ann(arrow(Pt::new(0.0, 0.0), Pt::new(100.0, 0.0)));
        a.move_handle(2, Pt::new(50.0, 40.0));
        let Shape::Arrow { from, to, bend, .. } = &a.shape else {
            unreachable!()
        };
        let pts = arrow_points(*from, *to, *bend);
        assert!(pts.iter().any(|p| p.dist(Pt::new(50.0, 40.0)) < 0.5));
        assert!(a.hit(Pt::new(50.0, 40.0), 2.0, no_text));
        assert!(!a.hit(Pt::new(50.0, 0.0), 2.0, no_text), "plus droite");
        // Ramenée sur la ligne : redevient droite.
        a.move_handle(2, Pt::new(50.0, 1.0));
        assert!(matches!(a.shape, Shape::Arrow { bend: None, .. }));
    }

    #[test]
    fn corner_handles_resize_from_the_opposite_corner() {
        let mut r = ann(rect(Pt::new(10.0, 10.0), Pt::new(50.0, 40.0)));
        assert_eq!(r.handles().len(), 4);
        r.move_handle(2, Pt::new(80.0, 90.0)); // coin bas droit
        let b = r.bounds(no_text);
        assert_eq!((b.min, b.max), (Pt::new(10.0, 10.0), Pt::new(80.0, 90.0)));
        r.move_handle(0, Pt::new(0.0, 5.0)); // coin haut gauche
        let b = r.bounds(no_text);
        assert_eq!((b.min, b.max), (Pt::new(0.0, 5.0), Pt::new(80.0, 90.0)));
    }

    #[test]
    fn step_tail_appears_and_disappears() {
        let mut s = ann(Shape::Step {
            at: Pt::new(50.0, 50.0),
            number: 1,
            tail: None,
        });
        s.move_handle(0, Pt::new(150.0, 50.0));
        assert!(matches!(s.shape, Shape::Step { tail: Some(_), .. }));
        assert!(s.hit(Pt::new(120.0, 51.0), 2.0, no_text));
        s.move_handle(0, Pt::new(52.0, 50.0));
        assert!(matches!(s.shape, Shape::Step { tail: None, .. }));
    }

    #[test]
    fn translate_moves_every_point() {
        let mut p = ann(Shape::Pen {
            points: vec![Pt::new(1.0, 2.0), Pt::new(3.0, 4.0)],
        });
        p.translate(10.0, -1.0);
        assert_eq!(
            p.shape,
            Shape::Pen {
                points: vec![Pt::new(11.0, 1.0), Pt::new(13.0, 3.0)]
            }
        );
        let mut m = ann(Shape::Magnifier {
            source: Pt::new(0.0, 0.0),
            radius: 10.0,
            at: Pt::new(50.0, 0.0),
            zoom: 2.0,
        });
        m.translate(1.0, 1.0);
        assert!(matches!(m.shape, Shape::Magnifier { source, at, .. }
            if source == Pt::new(1.0, 1.0) && at == Pt::new(51.0, 1.0)));
    }

    #[test]
    fn clicks_without_drag_are_dropped() {
        let p = Pt::new(5.0, 5.0);
        assert!(ann(rect(p, p.offset(1.0, 40.0))).is_degenerate());
        assert!(ann(arrow(p, p)).is_degenerate());
        assert!(
            !ann(Shape::Step {
                at: p,
                number: 1,
                tail: None
            })
            .is_degenerate()
        );
        assert!(
            ann(Shape::Text {
                at: p,
                text: "  ".into(),
                size: 20.0,
                boxed: false,
            })
            .is_degenerate()
        );
    }

    #[test]
    fn steps_count_up_from_the_highest() {
        let mut doc = Doc::default();
        assert_eq!(doc.next_step(), 1);
        for number in [4, 2] {
            doc.annotations.push(ann(Shape::Step {
                at: Pt::default(),
                number,
                tail: None,
            }));
        }
        assert_eq!(doc.next_step(), 5);
    }

    #[test]
    fn doc_round_trips_as_tagged_json() {
        let doc = Doc {
            annotations: vec![ann(Shape::Bubble {
                at: Pt::new(1.0, 2.0),
                text: "Clique ici".into(),
                size: 24.0,
                tail: Pt::new(0.0, 80.0),
            })],
            ..Default::default()
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains(r#""kind":"bubble""#), "{json}");
        assert_eq!(serde_json::from_str::<Doc>(&json).unwrap(), doc);
    }

    #[test]
    fn first_version_files_still_load() {
        let json = r#"{"annotations":[
            {"shape":{"kind":"arrow","from":{"x":0,"y":0},"to":{"x":9,"y":9}},"color":[255,0,0,255],"width":4},
            {"shape":{"kind":"rect","a":{"x":0,"y":0},"b":{"x":9,"y":9}},"color":[255,0,0,255],"width":4},
            {"shape":{"kind":"step","at":{"x":0,"y":0},"number":3},"color":[255,0,0,255],"width":4},
            {"shape":{"kind":"text","at":{"x":0,"y":0},"text":"a","size":20},"color":[255,0,0,255],"width":4}
        ]}"#;
        let doc: Doc = serde_json::from_str(json).unwrap();
        assert_eq!(doc.annotations.len(), 4);
        assert!(matches!(
            doc.annotations[0].shape,
            Shape::Arrow {
                style: ArrowStyle::Simple,
                bend: None,
                ..
            }
        ));
        assert!(!doc.annotations[1].fill && !doc.annotations[1].shadow);
    }

    #[test]
    fn colors_read_and_write_hex() {
        assert_eq!(Rgba::from_hex("#C6FF34"), Some(Rgba::rgb(0xc6, 0xff, 0x34)));
        assert_eq!(Rgba::from_hex("0a84ff"), Some(Rgba::rgb(0x0a, 0x84, 0xff)));
        assert_eq!(Rgba::from_hex("#f00"), Some(Rgba::rgb(255, 0, 0)));
        assert_eq!(Rgba::from_hex("#12345"), None);
        assert_eq!(Rgba::from_hex("zzzzzz"), None);
        assert_eq!(Rgba::rgb(0x7f, 0x3a, 0xed).hex(), "#7F3AED");
    }

    #[test]
    fn contrast_picks_a_readable_color() {
        assert_eq!(Rgba::rgb(0xc6, 0xff, 0x34).contrast(), Rgba::rgb(0, 0, 0));
        assert_eq!(
            Rgba::rgb(0x7f, 0x3a, 0xed).contrast(),
            Rgba::rgb(255, 255, 255)
        );
    }
}
