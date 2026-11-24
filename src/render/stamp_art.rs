//! Lot « Aperçus et tampons » : bibliothèque de tampons dessinés en couleur, dans
//! l'esprit de celle de Screenpresso (dessins originaux, en vecteurs tiny-skia : même
//! rendu à l'écran et à l'export).
//!
//! Chaque tampon est un [`Shape::Stamp`](crate::model::Shape::Stamp) dont l'icône a
//! un préfixe propre à sa famille, ce qui garde le format JSON :
//! - `dessin:<nom>` : icônes classiques en couleur (coches, croix, bulles, alertes,
//!   insectes, « NEW », grosses flèches) ;
//! - `curseur:<nom>` : pointeurs de souris (flèche, main, clics) ;
//! - `clic:<nom>` : corps de souris avec le bouton enfoncé et « 1× » / « 2× » ;
//! - `pastille:<texte>` : un pointeur et une étiquette bleue ;
//! - `tampon:<TEXTE>` : tampon encreur (cadre double, capitales, encre usée), à la
//!   couleur de l'annotation.
//!
//! Les dessins sont décrits dans un repère où la hauteur du tampon vaut 1.

use tiny_skia::{
    BlendMode, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Stroke,
    Transform,
};

use super::{fill_text, paint, rounded_rect, text_size};
use crate::model::{Bounds, NEW_PILL, PILL_PREFIX, Pt, RUBBER_PREFIX, Rgba, TextStyle, stamp_box};

pub const DRAWING_PREFIX: &str = "dessin:";
pub const CURSOR_PREFIX: &str = "curseur:";
pub const CLICK_PREFIX: &str = "clic:";

/// Tampons « Classique » : (icône, nom).
pub const CLASSIC: [(&str, &str); 23] = [
    ("dessin:coche-ronde", "Validé"),
    ("dessin:coche", "Coche"),
    ("dessin:croix-ronde", "Refusé"),
    ("dessin:croix", "Croix"),
    ("dessin:bulle-question", "Question"),
    ("dessin:bulle-exclamation", "Remarque"),
    ("dessin:rond-exclamation", "Important"),
    ("dessin:rond-exclamation-contour", "Important (contour)"),
    ("dessin:triangle-rouge", "Danger"),
    ("dessin:triangle-jaune", "Attention"),
    ("dessin:badge-jaune", "À noter"),
    ("dessin:insecte-rouge", "Bogue"),
    ("dessin:insecte-beige", "Bogue (beige)"),
    ("dessin:new-badge", "Nouveau (badge)"),
    (NEW_PILL, "Nouveau (pastille)"),
    ("dessin:fleche-gauche-vert", "Flèche verte vers la gauche"),
    ("dessin:fleche-droite-vert", "Flèche verte vers la droite"),
    ("dessin:fleche-haut-vert", "Flèche verte vers le haut"),
    ("dessin:fleche-bas-vert", "Flèche verte vers le bas"),
    ("dessin:fleche-gauche-rouge", "Flèche rouge vers la gauche"),
    ("dessin:fleche-droite-rouge", "Flèche rouge vers la droite"),
    ("dessin:fleche-haut-rouge", "Flèche rouge vers le haut"),
    ("dessin:fleche-bas-rouge", "Flèche rouge vers le bas"),
];

/// Pointeurs et souris : (icône, nom).
pub const MOUSE: [(&str, &str); 12] = [
    ("curseur:fleche", "Pointeur"),
    ("curseur:petite-fleche", "Petit pointeur"),
    ("curseur:fleche-clic", "Pointeur qui clique"),
    ("curseur:fleche-cercles", "Pointeur, clic en cercles"),
    ("curseur:main", "Main"),
    ("curseur:main-clic", "Main qui clique"),
    ("curseur:main-cercles", "Main, clic en cercles"),
    ("clic:souris", "Souris"),
    ("clic:gauche-1", "Clic gauche"),
    ("clic:gauche-2", "Double-clic gauche"),
    ("clic:droite-1", "Clic droit"),
    ("clic:droite-2", "Double-clic droit"),
];

/// Pointeurs à étiquette.
pub const PILLS: [&str; 5] = [
    "Cliquez ici",
    "Double-clic",
    "Clic gauche",
    "Clic droit",
    "Glisser-déposer",
];

/// Tampons encreurs.
pub const RUBBER: [&str; 10] = [
    "APPROUVÉ",
    "VRAI",
    "FAUX",
    "VALIDÉ",
    "ANNULÉ",
    "ÉCHEC",
    "BROUILLON",
    "CONFIDENTIEL",
    "AVANT",
    "APRÈS",
];

const GREEN: Rgba = Rgba::rgb(0x2E, 0xA0, 0x43);
const GREEN_DARK: Rgba = Rgba::rgb(0x1A, 0x66, 0x2A);
const RED: Rgba = Rgba::rgb(0xD9, 0x2D, 0x20);
const RED_DARK: Rgba = Rgba::rgb(0x8A, 0x14, 0x0E);
const YELLOW: Rgba = Rgba::rgb(0xFF, 0xC8, 0x1E);
const GOLD: Rgba = Rgba::rgb(0xE3, 0xA5, 0x08);
const GOLD_DARK: Rgba = Rgba::rgb(0x8C, 0x5E, 0x00);
const ORANGE: Rgba = Rgba::rgb(0xF0, 0x62, 0x1A);
const ORANGE_DARK: Rgba = Rgba::rgb(0xA8, 0x3A, 0x08);
const BLUE: Rgba = Rgba::rgb(0x1C, 0x6F, 0xD6);
const WHITE: Rgba = Rgba::rgb(0xFF, 0xFF, 0xFF);
const PAPER: Rgba = Rgba::rgb(0xF6, 0xF6, 0xF4);
const INK: Rgba = Rgba::rgb(0x22, 0x22, 0x26);
const GRAY: Rgba = Rgba::rgb(0x55, 0x55, 0x5C);
const BEIGE: Rgba = Rgba::rgb(0xDD, 0xC8, 0x92);
const BROWN: Rgba = Rgba::rgb(0x6E, 0x52, 0x26);

/// Ce tampon est-il dessiné ici ?
pub fn handles(icon: &str) -> bool {
    [
        DRAWING_PREFIX,
        CURSOR_PREFIX,
        CLICK_PREFIX,
        PILL_PREFIX,
        RUBBER_PREFIX,
    ]
    .iter()
    .any(|p| icon.starts_with(p))
}

/// Pinceau dans le repère du tampon (hauteur 1) posé dans un pixmap.
struct Art<'a> {
    pixmap: &'a mut Pixmap,
    /// Coin haut gauche, en pixels du pixmap.
    x0: f32,
    y0: f32,
    /// Pixels par unité (la hauteur du tampon).
    s: f32,
    ts: Transform,
}

fn ustroke(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

fn poly(points: &[(f32, f32)], close: bool) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let ((x, y), rest) = points.split_first()?;
    pb.move_to(*x, *y);
    for (x, y) in rest {
        pb.line_to(*x, *y);
    }
    if close {
        pb.close();
    }
    pb.finish()
}

fn circle(x: f32, y: f32, r: f32) -> Option<Path> {
    PathBuilder::from_circle(x, y, r)
}

fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    rounded_rect(
        Bounds {
            min: Pt::new(x, y),
            max: Pt::new(x + w, y + h),
        },
        r,
    )
}

/// Étoile à `n` branches (badges).
fn burst(cx: f32, cy: f32, n: usize, outer: f32, inner: f32) -> Option<Path> {
    let points: Vec<(f32, f32)> = (0..n * 2)
        .map(|i| {
            let a = i as f32 * std::f32::consts::PI / n as f32 - std::f32::consts::FRAC_PI_2;
            let r = if i % 2 == 0 { outer } else { inner };
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect();
    poly(&points, true)
}

impl Art<'_> {
    fn unit(&self) -> Transform {
        self.ts.pre_concat(Transform::from_row(
            self.s, 0.0, 0.0, self.s, self.x0, self.y0,
        ))
    }

    fn fill(&mut self, path: Option<Path>, color: Rgba) {
        if let Some(path) = path {
            let ts = self.unit();
            self.pixmap
                .fill_path(&path, &paint(color), FillRule::Winding, ts, None);
        }
    }

    fn stroke(&mut self, path: Option<Path>, color: Rgba, width: f32) {
        if let Some(path) = path {
            let ts = self.unit();
            self.pixmap
                .stroke_path(&path, &paint(color), &ustroke(width), ts, None);
        }
    }

    /// Contour épais puis remplissage : un dessin cerné, sans traits intérieurs entre
    /// ses morceaux.
    fn outlined(&mut self, parts: &[Option<Path>], fill: Rgba, line: Rgba, width: f32) {
        for p in parts {
            self.stroke(p.clone(), line, width * 2.0);
        }
        for p in parts {
            self.fill(p.clone(), fill);
        }
    }

    /// Texte centré sur (`cx`, `cy`), de taille `size` (unités).
    fn text(&mut self, cx: f32, cy: f32, size: f32, text: &str, color: Rgba) {
        let px = size * self.s;
        let (w, h) = text_size(text, px);
        // Les capitales et les chiffres sont un peu au-dessus du milieu de la ligne.
        let at = Pt::new(
            self.x0 + cx * self.s - w / 2.0,
            self.y0 + cy * self.s - h / 2.0 + px * 0.04,
        );
        let style = TextStyle {
            bold: true,
            ..TextStyle::default()
        };
        fill_text(self.pixmap, at, text, px, color, &style, self.ts);
    }
}

/// Dessine le tampon `icon` de hauteur `size`, centré sur `at`.
pub fn draw(pixmap: &mut Pixmap, at: Pt, icon: &str, size: f32, color: Rgba, ts: Transform) {
    let b = stamp_box(at, icon, size, &text_size);
    if size < 1.0 {
        return;
    }
    if let Some(text) = icon.strip_prefix(RUBBER_PREFIX) {
        rubber(pixmap, b, text, size, color, ts);
        return;
    }
    let mut art = Art {
        pixmap,
        x0: b.min.x,
        y0: b.min.y,
        s: size,
        ts,
    };
    let width = b.width() / size;
    if let Some(name) = icon.strip_prefix(DRAWING_PREFIX) {
        drawing(&mut art, name, width);
    } else if let Some(name) = icon.strip_prefix(CURSOR_PREFIX) {
        cursor(&mut art, name);
    } else if let Some(name) = icon.strip_prefix(CLICK_PREFIX) {
        mouse(&mut art, name);
    } else if let Some(text) = icon.strip_prefix(PILL_PREFIX) {
        art.fill(rrect(0.24, 0.46, width - 0.26, 0.46, 0.23), BLUE);
        art.stroke(rrect(0.24, 0.46, width - 0.26, 0.46, 0.23), WHITE, 0.035);
        art.text(0.24 + (width - 0.26) / 2.0, 0.69, 0.27, text, WHITE);
        arrow(&mut art, 0.03, 0.02, 0.56);
    }
}

fn check(points: &[(f32, f32)]) -> Option<Path> {
    poly(points, false)
}

fn drawing(art: &mut Art, name: &str, width: f32) {
    let cross = |a: f32, z: f32| {
        let mut pb = PathBuilder::new();
        pb.move_to(a, a);
        pb.line_to(z, z);
        pb.move_to(z, a);
        pb.line_to(a, z);
        pb.finish()
    };
    let disc = || circle(0.5, 0.5, 0.45);
    match name {
        "coche-ronde" => {
            art.fill(disc(), GREEN);
            art.stroke(disc(), GREEN_DARK, 0.035);
            art.stroke(
                check(&[(0.27, 0.52), (0.44, 0.68), (0.74, 0.34)]),
                WHITE,
                0.11,
            );
        }
        "coche" => {
            let c = || check(&[(0.12, 0.54), (0.38, 0.80), (0.88, 0.24)]);
            art.stroke(c(), GREEN_DARK, 0.25);
            art.stroke(c(), GREEN, 0.17);
        }
        "croix-ronde" => {
            art.fill(disc(), RED);
            art.stroke(disc(), RED_DARK, 0.035);
            art.stroke(cross(0.33, 0.67), WHITE, 0.11);
        }
        "croix" => {
            art.stroke(cross(0.18, 0.82), RED_DARK, 0.26);
            art.stroke(cross(0.18, 0.82), RED, 0.18);
        }
        "bulle-question" | "bulle-exclamation" => {
            let parts = [
                rrect(0.06, 0.08, 0.88, 0.64, 0.16),
                poly(&[(0.24, 0.66), (0.20, 0.94), (0.48, 0.70)], true),
            ];
            art.outlined(&parts, PAPER, GRAY, 0.03);
            let mark = if name == "bulle-question" { "?" } else { "!" };
            art.text(0.5, 0.40, 0.52, mark, INK);
        }
        "rond-exclamation" => {
            art.fill(disc(), RED);
            art.stroke(disc(), RED_DARK, 0.035);
            art.text(0.5, 0.5, 0.66, "!", WHITE);
        }
        "rond-exclamation-contour" => {
            art.fill(disc(), WHITE);
            art.stroke(circle(0.5, 0.5, 0.42), RED, 0.09);
            art.text(0.5, 0.5, 0.62, "!", RED);
        }
        "triangle-rouge" | "triangle-jaune" => {
            let tri = || poly(&[(0.5, 0.1), (0.93, 0.86), (0.07, 0.86)], true);
            if name == "triangle-rouge" {
                art.stroke(tri(), RED_DARK, 0.13);
                art.stroke(tri(), RED, 0.08);
                art.fill(tri(), RED);
                art.text(0.5, 0.6, 0.5, "!", WHITE);
            } else {
                art.stroke(tri(), INK, 0.14);
                art.stroke(tri(), YELLOW, 0.07);
                art.fill(tri(), YELLOW);
                art.text(0.5, 0.6, 0.5, "!", INK);
            }
        }
        "badge-jaune" => {
            art.fill(burst(0.5, 0.5, 14, 0.48, 0.39), GOLD);
            art.stroke(burst(0.5, 0.5, 14, 0.48, 0.39), GOLD_DARK, 0.025);
            art.text(0.5, 0.5, 0.56, "!", Rgba::rgb(0x4A, 0x32, 0x00));
        }
        "insecte-rouge" => bug(art, RED, RED_DARK, true),
        "insecte-beige" => bug(art, BEIGE, BROWN, false),
        "new-badge" => {
            art.fill(burst(0.5, 0.5, 16, 0.48, 0.40), ORANGE);
            art.stroke(burst(0.5, 0.5, 16, 0.48, 0.40), ORANGE_DARK, 0.025);
            art.text(0.5, 0.5, 0.27, "NEW", WHITE);
        }
        "new-pilule" => {
            art.fill(rrect(0.04, 0.2, width - 0.08, 0.6, 0.3), ORANGE);
            art.stroke(rrect(0.04, 0.2, width - 0.08, 0.6, 0.3), ORANGE_DARK, 0.03);
            art.text(width / 2.0, 0.5, 0.44, "new", WHITE);
        }
        _ => {
            if let Some(rest) = name.strip_prefix("fleche-") {
                let (dir, color) = rest.split_once('-').unwrap_or((rest, "vert"));
                let (fill, line) = if color == "rouge" {
                    (RED, RED_DARK)
                } else {
                    (GREEN, GREEN_DARK)
                };
                let angle: f32 = match dir {
                    "bas" => 90.0,
                    "gauche" => 180.0,
                    "haut" => 270.0,
                    _ => 0.0,
                };
                let (sin, cos) = angle.to_radians().sin_cos();
                let pts: Vec<(f32, f32)> = [
                    (0.08, 0.36),
                    (0.50, 0.36),
                    (0.50, 0.12),
                    (0.94, 0.5),
                    (0.50, 0.88),
                    (0.50, 0.64),
                    (0.08, 0.64),
                ]
                .iter()
                .map(|(x, y)| {
                    let (dx, dy) = (x - 0.5, y - 0.5);
                    (0.5 + dx * cos - dy * sin, 0.5 + dx * sin + dy * cos)
                })
                .collect();
                art.stroke(poly(&pts, true), line, 0.07);
                art.fill(poly(&pts, true), fill);
            }
        }
    }
}

/// Insecte vu de dessus.
fn bug(art: &mut Art, body: Rgba, line: Rgba, spots: bool) {
    let mut legs = PathBuilder::new();
    for (from, to) in [
        ((0.34, 0.46), (0.12, 0.36)),
        ((0.32, 0.60), (0.10, 0.62)),
        ((0.34, 0.74), (0.14, 0.88)),
    ] {
        legs.move_to(from.0, from.1);
        legs.line_to(to.0, to.1);
        legs.move_to(1.0 - from.0, from.1);
        legs.line_to(1.0 - to.0, to.1);
    }
    for (from, to) in [((0.45, 0.16), (0.36, 0.04)), ((0.55, 0.16), (0.64, 0.04))] {
        legs.move_to(from.0, from.1);
        legs.line_to(to.0, to.1);
    }
    art.stroke(legs.finish(), line, 0.05);
    let shell = || PathBuilder::from_oval(tiny_skia::Rect::from_ltrb(0.28, 0.30, 0.72, 0.92)?);
    art.fill(circle(0.5, 0.25, 0.12), line);
    art.fill(shell(), body);
    art.stroke(shell(), line, 0.035);
    art.stroke(poly(&[(0.5, 0.33), (0.5, 0.9)], false), line, 0.03);
    if spots {
        for (x, y, r) in [
            (0.40, 0.48, 0.045),
            (0.60, 0.48, 0.045),
            (0.38, 0.68, 0.05),
            (0.62, 0.68, 0.05),
        ] {
            art.fill(circle(x, y, r), Rgba::rgb(0x2A, 0x10, 0x0C));
        }
    } else {
        art.stroke(poly(&[(0.34, 0.56), (0.66, 0.56)], false), line, 0.025);
        art.stroke(poly(&[(0.33, 0.72), (0.67, 0.72)], false), line, 0.025);
    }
}

/// Flèche de pointeur, pointe en (`x`, `y`), de hauteur ≈ `k`.
fn arrow(art: &mut Art, x: f32, y: f32, k: f32) {
    let pts: Vec<(f32, f32)> = [
        (0.0, 0.0),
        (0.0, 0.80),
        (0.19, 0.63),
        (0.32, 0.92),
        (0.45, 0.86),
        (0.32, 0.58),
        (0.56, 0.58),
    ]
    .iter()
    .map(|(px, py)| (x + px * k, y + py * k))
    .collect();
    art.outlined(&[poly(&pts, true)], WHITE, INK, 0.035 * k.max(0.5));
}

/// Main qui pointe, bout de l'index en (`x`, `y`), de hauteur ≈ `k`.
fn hand(art: &mut Art, x: f32, y: f32, k: f32) {
    let r = |px: f32, py: f32, w: f32, h: f32, rad: f32| {
        rrect(x + (px - 0.30) * k, y + py * k, w * k, h * k, rad * k)
    };
    let thumb = r(0.04, 0.50, 0.28, 0.14, 0.07).and_then(|p| {
        p.transform(Transform::from_rotate_at(
            45.0,
            x + (0.18 - 0.30) * k,
            y + 0.57 * k,
        ))
    });
    let parts = [
        r(0.22, 0.0, 0.16, 0.58, 0.08),
        r(0.38, 0.30, 0.14, 0.32, 0.07),
        r(0.52, 0.33, 0.14, 0.32, 0.07),
        r(0.66, 0.37, 0.13, 0.30, 0.065),
        r(0.22, 0.44, 0.57, 0.50, 0.17),
        thumb,
    ];
    art.outlined(&parts, WHITE, INK, 0.035 * k.max(0.5));
}

/// Rayons d'un clic autour de (`x`, `y`).
fn rays(art: &mut Art, x: f32, y: f32) {
    let mut pb = PathBuilder::new();
    for deg in [-150.0f32, -112.0, -75.0, -38.0, 175.0] {
        let (s, c) = deg.to_radians().sin_cos();
        pb.move_to(x + c * 0.09, y + s * 0.09);
        pb.line_to(x + c * 0.25, y + s * 0.25);
    }
    art.stroke(pb.finish(), ORANGE, 0.05);
}

/// Cercles d'un clic autour de (`x`, `y`).
fn rings(art: &mut Art, x: f32, y: f32) {
    for (r, a) in [(0.09, 230), (0.17, 170), (0.25, 110)] {
        art.stroke(circle(x, y, r), Rgba(BLUE.0, BLUE.1, BLUE.2, a), 0.04);
    }
}

fn cursor(art: &mut Art, name: &str) {
    match name {
        "petite-fleche" => arrow(art, 0.34, 0.22, 0.58),
        "fleche-clic" => {
            rays(art, 0.34, 0.30);
            arrow(art, 0.34, 0.30, 0.66);
        }
        "fleche-cercles" => {
            rings(art, 0.34, 0.30);
            arrow(art, 0.34, 0.30, 0.66);
        }
        "main" => hand(art, 0.40, 0.04, 0.92),
        "main-clic" => {
            rays(art, 0.42, 0.28);
            hand(art, 0.42, 0.28, 0.70);
        }
        "main-cercles" => {
            rings(art, 0.42, 0.28);
            hand(art, 0.42, 0.28, 0.70);
        }
        _ => arrow(art, 0.27, 0.06, 0.9),
    }
}

/// Corps de souris, bouton gauche ou droit enfoncé, et « 1× » / « 2× ».
fn mouse(art: &mut Art, name: &str) {
    let body = || rrect(0.24, 0.06, 0.52, 0.88, 0.26);
    art.fill(body(), WHITE);
    let (side, times) = name.split_once('-').unwrap_or((name, ""));
    let k = 0.552_284_8 * 0.26;
    let half = |left: bool| {
        let (edge, mid) = if left { (0.24, 0.5) } else { (0.76, 0.5) };
        let dir = if left { 1.0 } else { -1.0 };
        let mut pb = PathBuilder::new();
        pb.move_to(mid, 0.42);
        pb.line_to(edge, 0.42);
        pb.line_to(edge, 0.32);
        pb.cubic_to(edge, 0.32 - k, mid - dir * k, 0.06, mid, 0.06);
        pb.close();
        pb.finish()
    };
    match side {
        "gauche" => art.fill(half(true), BLUE),
        "droite" => art.fill(half(false), BLUE),
        _ => {}
    }
    let mut lines = PathBuilder::new();
    lines.move_to(0.24, 0.42);
    lines.line_to(0.76, 0.42);
    lines.move_to(0.5, 0.06);
    lines.line_to(0.5, 0.42);
    art.stroke(lines.finish(), GRAY, 0.03);
    art.stroke(body(), GRAY, 0.04);
    if !times.is_empty() {
        let (x, y) = if side == "droite" {
            (0.80, 0.18)
        } else {
            (0.20, 0.18)
        };
        art.fill(circle(x, y, 0.17), BLUE);
        art.stroke(circle(x, y, 0.17), WHITE, 0.03);
        art.text(x, y, 0.2, &format!("{times}×"), WHITE);
    }
}

/// Graine du grain d'encre : le même texte est toujours usé de la même façon.
fn seed(text: &str) -> u32 {
    text.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    })
}

/// Tampon encreur dans un calque à part : l'encre usée efface le tampon, jamais
/// l'image dessous.
fn rubber(pixmap: &mut Pixmap, b: Bounds, text: &str, size: f32, color: Rgba, ts: Transform) {
    let (x0, y0) = (b.min.x.floor(), b.min.y.floor());
    let (w, h) = (b.width().ceil() as u32 + 2, b.height().ceil() as u32 + 2);
    if w > 8192 || h > 8192 {
        return;
    }
    let Some(mut layer) = Pixmap::new(w, h) else {
        return;
    };
    let width = b.width() / size;
    let ink = Rgba(color.0, color.1, color.2, 235);
    {
        let mut art = Art {
            pixmap: &mut layer,
            x0: b.min.x - x0,
            y0: b.min.y - y0,
            s: size,
            ts: Transform::identity(),
        };
        art.stroke(rrect(0.06, 0.08, width - 0.12, 0.84, 0.12), ink, 0.075);
        art.stroke(rrect(0.14, 0.18, width - 0.28, 0.64, 0.07), ink, 0.03);
        art.text(width / 2.0, 0.5, 0.5, text, ink);
    }
    // Grain : petits points effacés, répartis au hasard (toujours les mêmes).
    let mut state = seed(text);
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as f32 / (1u32 << 24) as f32
    };
    let erase = Paint {
        blend_mode: BlendMode::DestinationOut,
        anti_alias: true,
        ..paint(Rgba(0, 0, 0, 200))
    };
    let specks = (width * 70.0) as usize;
    let mut pb = PathBuilder::new();
    for _ in 0..specks {
        let (x, y) = (next() * w as f32, next() * h as f32);
        let r = size * (0.006 + next() * 0.016);
        pb.push_circle(x, y, r);
    }
    if let Some(path) = pb.finish() {
        layer.fill_path(
            &path,
            &erase,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    pixmap.draw_pixmap(
        x0 as i32,
        y0 as i32,
        layer.as_ref(),
        &PixmapPaint::default(),
        ts,
        None,
    );
}
