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
    // --- Lot « Éditeur : annotations » ---------------------------------------
    /// Polygone ou ligne brisée : sommets posés clic après clic. `closed` : le dernier
    /// sommet rejoint le premier. `start` / `end` : forme des bouts d'une ligne ouverte.
    Polygon {
        points: Vec<Pt>,
        #[serde(default)]
        closed: bool,
        #[serde(default)]
        start: EndShape,
        #[serde(default)]
        end: EndShape,
    },
    /// Accolade dans le rectangle (`a`, `b`), pointe du côté `side`.
    Bracket {
        a: Pt,
        b: Pt,
        #[serde(default)]
        side: BracketSide,
    },
    /// Image insérée (tampon personnel, glisser-déposer, collage) étirée dans le
    /// rectangle (`a`, `b`). `file` : nom du fichier dans le dossier `<capture>.assets/`,
    /// jamais l'image elle-même. `ratio` : largeur / hauteur d'origine, gardé au
    /// redimensionnement.
    Image {
        a: Pt,
        b: Pt,
        file: String,
        ratio: f32,
    },
    /// Tampon texte (« APPROUVÉ », « BROUILLON »…) : texte en capitales dans un cadre
    /// arrondi, centré sur `at`.
    Label {
        at: Pt,
        text: String,
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
    // --- Lot « Éditeur : annotations » : options de style, toutes facultatives (un
    // fichier qui ne les a pas se relit tel quel, et elles ne sont écrites que si
    // elles diffèrent du rendu d'origine).
    /// Opacité de toute l'annotation, de 0 à 1.
    #[serde(default = "opaque", skip_serializing_if = "is_opaque")]
    pub opacity: f32,
    /// Type de trait (traits, contours, polygones, accolades).
    #[serde(default, skip_serializing_if = "is_default")]
    pub dash: LineStyle,
    /// Rectangle et ellipse : assombrir tout l'extérieur (effet projecteur).
    #[serde(default, skip_serializing_if = "is_default")]
    pub dim_outside: bool,
    /// Rectangle et ellipse : flouter tout l'extérieur.
    #[serde(default, skip_serializing_if = "is_default")]
    pub blur_outside: bool,
    /// Textes et bulles : gras, italique, souligné, alignement, lueur.
    #[serde(default, skip_serializing_if = "is_default")]
    pub text_style: TextStyle,
    /// Bulles : avec pointe (défaut), sans pointe, ou cadre simple.
    #[serde(default, skip_serializing_if = "is_default")]
    pub bubble: BubbleKind,
    /// Pastilles : 1, 2, 3 / A, B, C / i, ii, iii.
    #[serde(default, skip_serializing_if = "is_default")]
    pub step_format: StepFormat,
    /// Pastilles : forme du lien vers l'élément désigné.
    #[serde(default, skip_serializing_if = "is_default")]
    pub step_tail: StepTail,
    /// Pastilles : couleur du numéro (`None` : noir ou blanc selon le fond).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_color: Option<Rgba>,
}

impl Annotation {
    pub fn new(shape: Shape, color: Rgba, width: f32) -> Self {
        Self {
            shape,
            color,
            width,
            fill: false,
            shadow: false,
            opacity: 1.0,
            dash: LineStyle::Solid,
            dim_outside: false,
            blur_outside: false,
            text_style: TextStyle::default(),
            bubble: BubbleKind::Tail,
            step_format: StepFormat::Number,
            step_tail: StepTail::Pointer,
            text_color: None,
        }
    }
}

// --- Lot « Éditeur : annotations » : types des nouvelles options ---------------------

fn opaque() -> f32 {
    1.0
}

fn is_opaque(v: &f32) -> bool {
    *v >= 1.0
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Type de trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineStyle {
    #[default]
    Solid,
    /// Pointillé.
    Dotted,
    /// Tiret-point.
    DashDot,
}

/// Alignement des lignes d'un texte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Lueur autour d'un texte, pour le lire sur n'importe quel fond. Remplace le liseré
/// contrasté du texte simple.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Glow {
    pub color: Rgba,
    /// Épaisseur, en pixels de l'image.
    pub size: f32,
    /// Flou, en pixels de l'image.
    pub blur: f32,
}

/// Options du texte (textes et bulles).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub align: TextAlign,
    pub glow: Option<Glow>,
}

/// Type de bulle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BubbleKind {
    /// Bulle de dialogue avec sa pointe.
    #[default]
    Tail,
    /// Bulle arrondie sans pointe.
    NoTail,
    /// Cadre simple, coins à peine arrondis, sans pointe.
    Simple,
}

/// Format du numéro d'une pastille.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepFormat {
    /// 1, 2, 3…
    #[default]
    Number,
    /// A, B, C… (puis AA, AB…)
    Letter,
    /// i, ii, iii…
    Roman,
}

/// Lien d'une pastille vers l'élément désigné.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepTail {
    /// Pointe pleine (rendu d'origine).
    #[default]
    Pointer,
    /// Trait terminé par un point.
    Line,
    /// Trait terminé par une pointe de flèche.
    Arrow,
    /// Épingle : la pastille devient une punaise pointée vers le bas.
    Pin,
    /// Aucun lien, même si une pointe a été tirée.
    Hidden,
}

/// Bout d'une ligne brisée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndShape {
    #[default]
    Plain,
    Arrow,
    Round,
}

/// Côté de la pointe d'une accolade : { } ︵ ︶.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BracketSide {
    #[default]
    Left,
    Right,
    Top,
    Bottom,
}

/// Texte du numéro `n` d'une pastille dans ce format.
pub fn step_label(n: u32, format: StepFormat) -> String {
    match format {
        StepFormat::Number => n.to_string(),
        StepFormat::Letter => {
            // 1 → A, 26 → Z, 27 → AA (comme les colonnes d'un tableur).
            let mut n = n.max(1);
            let mut out = Vec::new();
            while n > 0 {
                n -= 1;
                out.push(char::from(b'A' + (n % 26) as u8));
                n /= 26;
            }
            out.iter().rev().collect()
        }
        StepFormat::Roman => {
            const TABLE: [(u32, &str); 13] = [
                (1000, "m"),
                (900, "cm"),
                (500, "d"),
                (400, "cd"),
                (100, "c"),
                (90, "xc"),
                (50, "l"),
                (40, "xl"),
                (10, "x"),
                (9, "ix"),
                (5, "v"),
                (4, "iv"),
                (1, "i"),
            ];
            let mut n = n.clamp(1, 3999);
            let mut out = String::new();
            for (value, digits) in TABLE {
                while n >= value {
                    out.push_str(digits);
                    n -= value;
                }
            }
            out
        }
    }
}

/// Pointe d'une pastille en épingle.
pub fn pin_tip(at: Pt, width: f32) -> Pt {
    at.offset(0.0, step_radius(width) * 1.9)
}

/// Cadre d'un tampon texte centré sur `at`.
pub fn label_box(at: Pt, text: &str, size: f32, text_size: &impl TextSize) -> Bounds {
    let (w, h) = text_size(text, size);
    let (px, py) = (size * 0.6, size * 0.3);
    Bounds {
        min: at.offset(-w / 2.0 - px, -h / 2.0 - py),
        max: at.offset(w / 2.0 + px, h / 2.0 + py),
    }
}

/// Le point est-il dans le polygone (règle pair-impair) ?
fn inside_polygon(p: Pt, points: &[Pt]) -> bool {
    let mut inside = false;
    let n = points.len();
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

/// Coin opposé à `fixed` d'un rectangle tiré vers `p` qui garde le rapport largeur /
/// hauteur `ratio`.
pub fn keep_ratio(fixed: Pt, p: Pt, ratio: f32) -> Pt {
    if ratio <= 0.0 || !ratio.is_finite() {
        return p;
    }
    let (dx, dy) = (p.x - fixed.x, p.y - fixed.y);
    let (w, h) = if dx.abs() / ratio > dy.abs() {
        (dx.abs(), dx.abs() / ratio)
    } else {
        (dy.abs() * ratio, dy.abs())
    };
    let sign = |v: f32| if v < 0.0 { -1.0 } else { 1.0 };
    Pt::new(fixed.x + w * sign(dx), fixed.y + h * sign(dy))
}

/// Alignement d'une sélection de plusieurs annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Top,
    Middle,
    Bottom,
}

/// Décalages (dx, dy) qui alignent chaque boîte sur un bord ou le milieu de leur
/// ensemble.
pub fn align_offsets(boxes: &[Bounds], how: Align) -> Vec<(f32, f32)> {
    let Some(all) = boxes.iter().copied().reduce(Bounds::union) else {
        return Vec::new();
    };
    boxes
        .iter()
        .map(|b| match how {
            Align::Left => (all.min.x - b.min.x, 0.0),
            Align::Center => (all.center().x - b.center().x, 0.0),
            Align::Right => (all.max.x - b.max.x, 0.0),
            Align::Top => (0.0, all.min.y - b.min.y),
            Align::Middle => (0.0, all.center().y - b.center().y),
            Align::Bottom => (0.0, all.max.y - b.max.y),
        })
        .collect()
}

/// Décalages qui répartissent les boîtes avec des écarts égaux, les deux extrêmes
/// restant en place (`horizontal` : de gauche à droite, sinon de haut en bas).
pub fn distribute_offsets(boxes: &[Bounds], horizontal: bool) -> Vec<(f32, f32)> {
    let n = boxes.len();
    let mut out = vec![(0.0, 0.0); n];
    if n < 3 {
        return out;
    }
    let lo = |b: &Bounds| if horizontal { b.min.x } else { b.min.y };
    let size = |b: &Bounds| if horizontal { b.width() } else { b.height() };
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&i, &j| lo(&boxes[i]).total_cmp(&lo(&boxes[j])));
    let first = lo(&boxes[order[0]]);
    let end = boxes
        .iter()
        .map(|b| lo(b) + size(b))
        .fold(f32::MIN, f32::max);
    let total: f32 = boxes.iter().map(size).sum();
    let gap = (end - first - total) / (n - 1) as f32;
    let mut pos = first;
    for &i in &order {
        let d = pos - lo(&boxes[i]);
        out[i] = if horizontal { (d, 0.0) } else { (0.0, d) };
        pos += size(&boxes[i]) + gap;
    }
    out
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
            } => {
                let b = bubble_box(*at, text, *size, &text_size);
                if self.bubble == BubbleKind::Tail {
                    b.union(r(*tail, *tail))
                } else {
                    b
                }
            }
            Shape::Step { at, tail, .. } => {
                let rad = step_radius(self.width);
                let b = Bounds {
                    min: at.offset(-rad, -rad),
                    max: at.offset(rad, rad),
                };
                match (self.step_tail, tail) {
                    (StepTail::Pin, _) => {
                        let tip = pin_tip(*at, self.width);
                        b.union(r(tip, tip))
                    }
                    (StepTail::Hidden, _) | (_, None) => b,
                    (_, Some(t)) => b.union(r(*t, *t)),
                }
            }
            Shape::Polygon { points, .. } => {
                Bounds::from_points(points.iter().copied()).unwrap_or(Bounds {
                    min: Pt::default(),
                    max: Pt::default(),
                })
            }
            Shape::Bracket { a, b, .. } | Shape::Image { a, b, .. } => r(*a, *b),
            Shape::Label { at, text, size } => label_box(*at, text, *size, &text_size),
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
                let tail = match self.step_tail {
                    StepTail::Pin => Some(pin_tip(*at, self.width)),
                    StepTail::Hidden => None,
                    _ => *tail,
                };
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
                    || (self.bubble == BubbleKind::Tail
                        && dist_to_segment(p, b.center(), *tail) <= tolerance * 2.0)
            }
            Shape::Polygon { points, closed, .. } => {
                let mut ring = points.clone();
                if *closed && let Some(first) = points.first() {
                    ring.push(*first);
                }
                dist_to_polyline(p, &ring) <= reach
                    || (self.fill && points.len() >= 3 && inside_polygon(p, points))
            }
            Shape::Bracket { .. } | Shape::Image { .. } | Shape::Label { .. } => {
                self.bounds(text_size).expand(tolerance).contains(p)
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
            Shape::Polygon { points, .. } => points.iter_mut().for_each(mv),
            Shape::Bracket { a, b, .. } | Shape::Image { a, b, .. } => {
                mv(a);
                mv(b);
            }
            Shape::Label { at, .. } => mv(at),
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
            Shape::Bubble { tail, .. } if self.bubble == BubbleKind::Tail => vec![*tail],
            Shape::Bubble { .. } => Vec::new(),
            Shape::Step { .. } if matches!(self.step_tail, StepTail::Pin | StepTail::Hidden) => {
                Vec::new()
            }
            Shape::Step { at, tail, .. } => {
                vec![tail.unwrap_or_else(|| step_tail_handle(*at, self.width))]
            }
            Shape::Polygon { points, .. } => points.clone(),
            Shape::Bracket { a, b, .. } | Shape::Image { a, b, .. } => {
                Bounds::from_corners(*a, *b).corners().to_vec()
            }
            Shape::Label { .. } => Vec::new(),
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
            Shape::Polygon { points, .. } => {
                if let Some(v) = points.get_mut(index) {
                    *v = p;
                }
            }
            Shape::Bracket { a, b, .. } => {
                let corners = Bounds::from_corners(*a, *b).corners();
                *a = corners[(index + 2) % 4];
                *b = p;
            }
            Shape::Image { a, b, ratio, .. } => {
                let corners = Bounds::from_corners(*a, *b).corners();
                *a = corners[(index + 2) % 4];
                *b = keep_ratio(*a, p, *ratio);
            }
            Shape::Pen { .. } | Shape::Text { .. } | Shape::Stamp { .. } | Shape::Label { .. } => {}
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
            Shape::Polygon { points, .. } => {
                points.len() < 2
                    || Bounds::from_points(points.iter().copied())
                        .is_none_or(|b| b.width().max(b.height()) < MIN)
            }
            Shape::Bracket { a, b, .. } | Shape::Image { a, b, .. } => {
                let r = Bounds::from_corners(*a, *b);
                r.width() < MIN || r.height() < MIN
            }
            Shape::Label { text, .. } => text.trim().is_empty(),
        }
    }

    /// Le rectangle ou l'ellipse a-t-il un effet sur tout l'extérieur ?
    pub fn affects_outside(&self) -> bool {
        (self.dim_outside || self.blur_outside)
            && matches!(self.shape, Shape::Rect { .. } | Shape::Ellipse { .. })
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
    // --- Lot « Éditeur : annotations » : propriétés de l'image (bandeau titre et
    // description au-dessus de l'image, seulement si `show_properties`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub show_properties: bool,
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
    /// Le bandeau des propriétés est-il à dessiner ?
    pub fn has_banner(&self) -> bool {
        self.show_properties
            && !(self.title.trim().is_empty() && self.description.trim().is_empty())
    }

    /// Renumérote les pastilles dans l'ordre des annotations (de la plus ancienne à la
    /// plus récente), à partir du plus petit numéro présent. `true` si quelque chose a
    /// changé.
    pub fn renumber_steps(&mut self) -> bool {
        let start = self
            .annotations
            .iter()
            .filter_map(|a| match a.shape {
                Shape::Step { number, .. } => Some(number),
                _ => None,
            })
            .min()
            .unwrap_or(1);
        let mut next = start;
        let mut changed = false;
        for ann in &mut self.annotations {
            if let Shape::Step { number, .. } = &mut ann.shape {
                changed |= *number != next;
                *number = next;
                next += 1;
            }
        }
        changed
    }

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

    // --- Lot « Éditeur : annotations » -----------------------------------------------

    /// Fichier écrit par la version actuelle (avant ce lot), toutes formes et options
    /// confondues : il se relit, et se réécrit à l'identique (aucun champ nouveau
    /// n'apparaît tant qu'on ne s'en sert pas).
    #[test]
    fn current_version_files_still_load_and_rewrite_identically() {
        let json = r#"{"annotations":[{"shape":{"kind":"arrow","from":{"x":0.0,"y":0.0},"to":{"x":9.0,"y":9.0},"style":"big","bend":{"x":4.0,"y":1.0}},"color":[255,0,0,255],"width":4.0,"fill":false,"shadow":true},{"shape":{"kind":"line","from":{"x":0.0,"y":0.0},"to":{"x":9.0,"y":9.0}},"color":[0,0,0,255],"width":2.0,"fill":false,"shadow":false},{"shape":{"kind":"rect","a":{"x":0.0,"y":0.0},"b":{"x":9.0,"y":9.0},"radius":12.0},"color":[255,0,0,255],"width":4.0,"fill":true,"shadow":false},{"shape":{"kind":"ellipse","a":{"x":0.0,"y":0.0},"b":{"x":9.0,"y":9.0}},"color":[255,0,0,255],"width":4.0,"fill":false,"shadow":false},{"shape":{"kind":"highlight","a":{"x":0.0,"y":0.0},"b":{"x":9.0,"y":9.0}},"color":[255,214,10,255],"width":4.0,"fill":false,"shadow":false},{"shape":{"kind":"spotlight","a":{"x":0.0,"y":0.0},"b":{"x":9.0,"y":9.0}},"color":[255,0,0,255],"width":4.0,"fill":false,"shadow":false},{"shape":{"kind":"pixelate","a":{"x":0.0,"y":0.0},"b":{"x":9.0,"y":9.0}},"color":[255,0,0,255],"width":4.0,"fill":false,"shadow":false},{"shape":{"kind":"blur","a":{"x":0.0,"y":0.0},"b":{"x":9.0,"y":9.0}},"color":[255,0,0,255],"width":4.0,"fill":false,"shadow":false},{"shape":{"kind":"pen","points":[{"x":0.0,"y":0.0},{"x":3.0,"y":4.0}]},"color":[255,0,0,255],"width":4.0,"fill":false,"shadow":false},{"shape":{"kind":"text","at":{"x":0.0,"y":0.0},"text":"a\nb","size":40.0,"boxed":true},"color":[255,0,0,255],"width":4.0,"fill":false,"shadow":false},{"shape":{"kind":"bubble","at":{"x":1.0,"y":2.0},"text":"Clique","size":24.0,"tail":{"x":0.0,"y":80.0}},"color":[255,0,0,255],"width":4.0,"fill":false,"shadow":false},{"shape":{"kind":"step","at":{"x":0.0,"y":0.0},"number":3,"tail":{"x":40.0,"y":40.0}},"color":[255,0,0,255],"width":4.0,"fill":false,"shadow":false},{"shape":{"kind":"magnifier","source":{"x":10.0,"y":10.0},"radius":20.0,"at":{"x":90.0,"y":10.0},"zoom":2.0},"color":[255,0,0,255],"width":4.0,"fill":false,"shadow":false},{"shape":{"kind":"stamp","at":{"x":10.0,"y":10.0},"icon":"\u{e1c3}","size":80.0},"color":[255,0,0,255],"width":4.0,"fill":false,"shadow":false}]}"#;
        let json = json.replace("\\u{e1c3}", "\u{e1c3}");
        let doc: Doc = serde_json::from_str(&json).unwrap();
        assert_eq!(doc.annotations.len(), 14);
        for a in &doc.annotations {
            assert_eq!(a.opacity, 1.0);
            assert_eq!(a.dash, LineStyle::Solid);
            assert_eq!(a.bubble, BubbleKind::Tail);
            assert_eq!(a.step_tail, StepTail::Pointer);
            assert_eq!(a.text_style, TextStyle::default());
        }
        assert!(!doc.show_properties && doc.title.is_empty());
        assert_eq!(serde_json::to_string(&doc).unwrap(), json);
    }

    #[test]
    fn new_options_and_shapes_round_trip() {
        let mut a = ann(Shape::Polygon {
            points: vec![Pt::new(0.0, 0.0), Pt::new(50.0, 0.0), Pt::new(0.0, 50.0)],
            closed: true,
            start: EndShape::Round,
            end: EndShape::Arrow,
        });
        a.opacity = 0.5;
        a.dash = LineStyle::DashDot;
        a.text_style = TextStyle {
            bold: true,
            italic: true,
            underline: true,
            align: TextAlign::Center,
            glow: Some(Glow {
                color: Rgba::rgb(255, 255, 255),
                size: 4.0,
                blur: 2.0,
            }),
        };
        a.step_format = StepFormat::Roman;
        a.step_tail = StepTail::Pin;
        a.text_color = Some(Rgba::rgb(1, 2, 3));
        let doc = Doc {
            annotations: vec![
                a,
                ann(Shape::Bracket {
                    a: Pt::new(0.0, 0.0),
                    b: Pt::new(10.0, 40.0),
                    side: BracketSide::Bottom,
                }),
                ann(Shape::Image {
                    a: Pt::new(0.0, 0.0),
                    b: Pt::new(20.0, 10.0),
                    file: "image.png".into(),
                    ratio: 2.0,
                }),
                ann(Shape::Label {
                    at: Pt::new(5.0, 5.0),
                    text: "BROUILLON".into(),
                    size: 30.0,
                }),
            ],
            title: "Titre".into(),
            description: "Description".into(),
            show_properties: true,
            ..Doc::default()
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains(r#""file":"image.png""#), "{json}");
        assert!(!json.contains("base64"));
        assert_eq!(serde_json::from_str::<Doc>(&json).unwrap(), doc);
        assert!(doc.has_banner());
    }

    #[test]
    fn step_labels_follow_their_format() {
        let label = |n, f| step_label(n, f);
        assert_eq!(label(12, StepFormat::Number), "12");
        assert_eq!(label(1, StepFormat::Letter), "A");
        assert_eq!(label(26, StepFormat::Letter), "Z");
        assert_eq!(label(28, StepFormat::Letter), "AB");
        assert_eq!(label(4, StepFormat::Roman), "iv");
        assert_eq!(label(14, StepFormat::Roman), "xiv");
    }

    #[test]
    fn renumbering_counts_from_the_lowest_in_order() {
        let mut doc = Doc::default();
        for number in [5, 9, 2] {
            doc.annotations.push(ann(Shape::Step {
                at: Pt::default(),
                number,
                tail: None,
            }));
        }
        doc.annotations
            .insert(1, ann(rect(Pt::default(), Pt::new(9.0, 9.0))));
        assert!(doc.renumber_steps());
        let numbers: Vec<u32> = doc
            .annotations
            .iter()
            .filter_map(|a| match a.shape {
                Shape::Step { number, .. } => Some(number),
                _ => None,
            })
            .collect();
        assert_eq!(numbers, [2, 3, 4]);
        assert!(!doc.renumber_steps());
    }

    #[test]
    fn polygons_are_hit_on_edges_and_inside_when_filled() {
        let mut p = ann(Shape::Polygon {
            points: vec![
                Pt::new(0.0, 0.0),
                Pt::new(100.0, 0.0),
                Pt::new(100.0, 100.0),
            ],
            closed: false,
            start: EndShape::Plain,
            end: EndShape::Plain,
        });
        assert!(p.hit(Pt::new(50.0, 1.0), 2.0, no_text));
        assert!(
            !p.hit(Pt::new(50.0, 50.0), 2.0, no_text),
            "côté fermant absent"
        );
        if let Shape::Polygon { closed, .. } = &mut p.shape {
            *closed = true;
        }
        assert!(p.hit(Pt::new(50.0, 50.0), 2.0, no_text), "côté fermant");
        assert!(!p.hit(Pt::new(80.0, 30.0), 2.0, no_text));
        p.fill = true;
        assert!(p.hit(Pt::new(80.0, 30.0), 2.0, no_text), "intérieur plein");
        p.move_handle(1, Pt::new(120.0, 0.0));
        assert_eq!(p.handles()[1], Pt::new(120.0, 0.0));
    }

    #[test]
    fn images_keep_their_ratio_when_resized() {
        let mut img = ann(Shape::Image {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(40.0, 20.0),
            file: "x.png".into(),
            ratio: 2.0,
        });
        img.move_handle(2, Pt::new(100.0, 30.0));
        let b = img.bounds(no_text);
        assert_eq!((b.min, b.max), (Pt::new(0.0, 0.0), Pt::new(100.0, 50.0)));
        assert_eq!(
            keep_ratio(Pt::new(0.0, 0.0), Pt::new(-10.0, -80.0), 0.5),
            Pt::new(-40.0, -80.0)
        );
    }

    #[test]
    fn tails_follow_the_bubble_and_step_options() {
        let mut b = ann(Shape::Bubble {
            at: Pt::new(0.0, 0.0),
            text: "x".into(),
            size: 10.0,
            tail: Pt::new(0.0, 200.0),
        });
        assert_eq!(b.handles().len(), 1);
        b.bubble = BubbleKind::NoTail;
        assert!(b.handles().is_empty());
        assert!(b.bounds(no_text).max.y < 100.0, "la pointe ne compte plus");
        let mut s = ann(Shape::Step {
            at: Pt::new(50.0, 50.0),
            number: 1,
            tail: None,
        });
        s.step_tail = StepTail::Pin;
        assert!(s.bounds(no_text).max.y > 50.0 + step_radius(4.0) * 1.5);
        assert!(s.handles().is_empty());
    }

    #[test]
    fn align_and_distribute_move_the_right_boxes() {
        let boxes = [
            Bounds::from_corners(Pt::new(0.0, 0.0), Pt::new(10.0, 10.0)),
            Bounds::from_corners(Pt::new(30.0, 40.0), Pt::new(50.0, 50.0)),
            Bounds::from_corners(Pt::new(100.0, 5.0), Pt::new(110.0, 15.0)),
        ];
        assert_eq!(
            align_offsets(&boxes, Align::Left),
            [(0.0, 0.0), (-30.0, 0.0), (-100.0, 0.0)]
        );
        assert_eq!(
            align_offsets(&boxes, Align::Bottom),
            [(0.0, 40.0), (0.0, 0.0), (0.0, 35.0)]
        );
        assert_eq!(align_offsets(&boxes, Align::Center)[0], (50.0, 0.0));
        // Largeurs 10 + 20 + 10 sur 110 : écarts de 35.
        let d = distribute_offsets(&boxes, true);
        assert_eq!(d, [(0.0, 0.0), (15.0, 0.0), (0.0, 0.0)]);
        assert_eq!(distribute_offsets(&boxes[..2], true), [(0.0, 0.0); 2]);
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
