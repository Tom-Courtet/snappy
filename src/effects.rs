//! Retouches de l'image et effets de bordure (lot « Éditeur : image et effets »).
//!
//! Tout reste **non destructif**, comme les annotations :
//! - les retouches ([`ImageEdit`] : recadrer, pivoter, retourner, redimensionner, zone
//!   de travail, couper une bande, changer l'image principale) sont enregistrées dans
//!   le `.json` de la capture et rejouées dans l'ordre sur l'image d'origine
//!   ([`working_base`]). Quand une retouche change la géométrie, les annotations sont
//!   déplacées avec elle ([`ImageEdit::map_point`], [`transform_annotations`]) ;
//! - les effets de bordure ([`BorderEffects`]) et la zone de travail automatique
//!   s'appliquent à l'export, après les annotations ([`apply_export`]).
//!
//! Sans retouche ni effet, l'export est exactement celui d'avant (mêmes octets).
//!
//! Fonctions pures, testées ci-dessous ; l'interface est dans `editor/image_tab.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use image::{Rgba as Px, RgbaImage, imageops};
use serde::{Deserialize, Serialize};
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, IntSize, Paint, PathBuilder, Pattern, Pixmap, PixmapPaint,
    SpreadMode, Transform,
};

use crate::model::{Annotation, Doc, Pt, Rgba, Shape};
use crate::render;

// --- Types enregistrés -----------------------------------------------------------

/// Sens de la bande retirée : horizontale (des lignes) ou verticale (des colonnes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

/// Jointure laissée à la place de la bande retirée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Join {
    /// Bords déchirés, séparés par un mince espace transparent.
    #[default]
    Torn,
    /// Raccord net.
    Straight,
    /// Raccord marqué d'un trait pointillé.
    Dotted,
}

/// Fond des zones ajoutées (zone de travail, effets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Background {
    /// Transparent (PNG avec canal alpha).
    #[default]
    Transparent,
    Color {
        color: Rgba,
    },
    /// « Étendre la couleur de fond » : la couleur la plus fréquente sur le bord de
    /// l'image, pour un agrandissement sans raccord visible.
    Extend,
}

/// Ancrage sur une grille 3 × 3 : colonne et ligne de 0 à 2 (1, 1 : centre).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub col: u8,
    pub row: u8,
}

impl Default for Anchor {
    fn default() -> Self {
        Self { col: 1, row: 1 }
    }
}

/// Une retouche de l'image, rejouée dans l'ordre sur l'image d'origine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ImageEdit {
    /// Garde le rectangle (`x`, `y`, `w`, `h`), en pixels de l'image d'entrée.
    Crop { x: u32, y: u32, w: u32, h: u32 },
    /// Quarts de tour dans le sens horaire (1 : 90°, 2 : 180°, 3 : 270°).
    Rotate { quarters: u8 },
    /// Miroir : gauche ↔ droite (`horizontal`) ou haut ↔ bas.
    Flip { horizontal: bool },
    /// Nouvelles dimensions.
    Resize { w: u32, h: u32 },
    /// Zone de travail fixe : l'image posée sur un fond `w` × `h` selon l'ancrage
    /// (plus petit que l'image : elle est rognée autour de l'ancrage).
    Canvas {
        w: u32,
        h: u32,
        #[serde(default)]
        anchor: Anchor,
        #[serde(default)]
        background: Background,
    },
    /// Retire la bande [`start`, `end`[ (lignes ou colonnes) et raccorde les deux
    /// morceaux.
    CutOut {
        orientation: Orientation,
        start: u32,
        end: u32,
        #[serde(default)]
        join: Join,
    },
    /// « Changer l'image principale » : repart de cette image, annotations gardées.
    ReplaceBase { path: PathBuf },
}

/// Côtés déchirés.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TornSides {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl TornSides {
    pub fn any(self) -> bool {
        self.top || self.right || self.bottom || self.left
    }
}

/// « Bordure et effets », appliqués à l'export après les annotations. Un effet se
/// désactive en mettant son réglage à zéro.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BorderEffects {
    /// Largeur du bord, en pixels (ajoutée autour de l'image).
    pub border_width: u32,
    pub border_color: Rgba,
    /// Coins arrondis, en pixels.
    pub corner_radius: u32,
    pub torn: TornSides,
    /// Profondeur de la déchirure, en % (100 % : 24 px).
    pub torn_ratio: u32,
    /// Noir et blanc.
    pub grayscale: bool,
    /// Fond des zones ajoutées (ombre, rotation, reflet, coins).
    pub background: Background,
    /// Hauteur du reflet, en % de la hauteur de l'image.
    pub reflection: u32,
    /// Espace entre l'image et son reflet, en pixels.
    pub reflection_offset: u32,
    /// Opacité de l'ombre portée, en %.
    pub shadow: u32,
    /// Rotation, en degrés (sens horaire).
    pub rotation: f32,
}

impl Default for BorderEffects {
    fn default() -> Self {
        Self {
            border_width: 0,
            border_color: Rgba::rgb(0, 0, 0),
            corner_radius: 0,
            torn: TornSides::default(),
            torn_ratio: 0,
            grayscale: false,
            background: Background::Transparent,
            reflection: 0,
            reflection_offset: 0,
            shadow: 0,
            rotation: 0.0,
        }
    }
}

impl BorderEffects {
    /// Préréglage « Effet d'ombre ».
    pub fn shadow_preset() -> Self {
        Self {
            shadow: 60,
            ..Self::default()
        }
    }

    /// Aucun effet : l'export reste celui d'avant.
    pub fn is_noop(&self) -> bool {
        self.border_width == 0
            && self.corner_radius == 0
            && (self.torn_ratio == 0 || !self.torn.any())
            && !self.grayscale
            && self.reflection == 0
            && self.shadow == 0
            && self.rotation.abs() < 0.05
            && self.background == Background::Transparent
    }
}

// --- Retouches -----------------------------------------------------------------

/// Bande [`start`, `end`[ bornée à `len`, ou `None` si elle est vide ou couvre tout.
fn cut_range(start: u32, end: u32, len: u32) -> Option<(u32, u32)> {
    let (s, e) = (start.min(end).min(len), start.max(end).min(len));
    (e > s && e - s < len).then_some((s, e))
}

/// Rectangle de recadrage borné à l'image (au moins 1 × 1).
fn crop_rect(x: u32, y: u32, w: u32, h: u32, src: (u32, u32)) -> (u32, u32, u32, u32) {
    let x = x.min(src.0.saturating_sub(1));
    let y = y.min(src.1.saturating_sub(1));
    (x, y, w.clamp(1, src.0 - x), h.clamp(1, src.1 - y))
}

/// Décalage de l'image d'origine (`src`) dans une zone de travail `dst` ancrée.
pub fn canvas_offset(src: (u32, u32), dst: (u32, u32), anchor: Anchor) -> (i64, i64) {
    let off =
        |s: u32, d: u32, k: u8| ((i64::from(d) - i64::from(s)) * i64::from(k.min(2))).div_euclid(2);
    (off(src.0, dst.0, anchor.col), off(src.1, dst.1, anchor.row))
}

impl ImageEdit {
    /// Dimensions après la retouche (`None` : changement d'image, inconnues ici).
    pub fn output_size(&self, src: (u32, u32)) -> Option<(u32, u32)> {
        let (w, h) = src;
        Some(match *self {
            Self::Crop { x, y, w: cw, h: ch } => {
                let (_, _, cw, ch) = crop_rect(x, y, cw, ch, src);
                (cw, ch)
            }
            Self::Rotate { quarters } if quarters % 2 == 1 => (h, w),
            Self::Rotate { .. } | Self::Flip { .. } => (w, h),
            Self::Resize { w, h } | Self::Canvas { w, h, .. } => (w.max(1), h.max(1)),
            Self::CutOut {
                orientation,
                start,
                end,
                ..
            } => match orientation {
                Orientation::Horizontal => match cut_range(start, end, h) {
                    Some((s, e)) => (w, h - (e - s)),
                    None => (w, h),
                },
                Orientation::Vertical => match cut_range(start, end, w) {
                    Some((s, e)) => (w - (e - s), h),
                    None => (w, h),
                },
            },
            Self::ReplaceBase { .. } => return None,
        })
    }

    /// Applique la retouche (sauf le changement d'image, qui demande un fichier :
    /// voir [`working_base`]).
    pub fn apply(&self, img: &RgbaImage) -> RgbaImage {
        let src = img.dimensions();
        if src.0 == 0 || src.1 == 0 {
            return img.clone();
        }
        match self {
            &Self::Crop { x, y, w, h } => {
                let (x, y, w, h) = crop_rect(x, y, w, h, src);
                imageops::crop_imm(img, x, y, w, h).to_image()
            }
            Self::Rotate { quarters } => match quarters % 4 {
                1 => imageops::rotate90(img),
                2 => imageops::rotate180(img),
                3 => imageops::rotate270(img),
                _ => img.clone(),
            },
            Self::Flip { horizontal: true } => imageops::flip_horizontal(img),
            Self::Flip { horizontal: false } => imageops::flip_vertical(img),
            &Self::Resize { w, h } => {
                let (w, h) = (w.max(1), h.max(1));
                if (w, h) == src {
                    img.clone()
                } else {
                    imageops::resize(img, w, h, imageops::FilterType::Lanczos3)
                }
            }
            &Self::Canvas {
                w,
                h,
                anchor,
                background,
            } => {
                let (w, h) = (w.max(1), h.max(1));
                let fill = background_px(background, img);
                let mut out = RgbaImage::from_pixel(w, h, fill);
                let (dx, dy) = canvas_offset(src, (w, h), anchor);
                imageops::replace(&mut out, img, dx, dy);
                out
            }
            &Self::CutOut {
                orientation,
                start,
                end,
                join,
            } => match orientation {
                Orientation::Horizontal => cut_rows(img, start, end, join),
                // Colonnes : on tourne d'un quart, on retire des lignes, on revient.
                Orientation::Vertical => {
                    imageops::rotate270(&cut_rows(&imageops::rotate90(img), start, end, join))
                }
            },
            Self::ReplaceBase { .. } => img.clone(),
        }
    }

    /// Où va le point `p` de l'image d'entrée (dimensions `src`) après la retouche.
    pub fn map_point(&self, src: (u32, u32), p: Pt) -> Pt {
        let (w, h) = (src.0 as f32, src.1 as f32);
        match *self {
            Self::Crop { x, y, w: cw, h: ch } => {
                let (x, y, _, _) = crop_rect(x, y, cw, ch, src);
                p.offset(-(x as f32), -(y as f32))
            }
            Self::Rotate { quarters } => match quarters % 4 {
                1 => Pt::new(h - p.y, p.x),
                2 => Pt::new(w - p.x, h - p.y),
                3 => Pt::new(p.y, w - p.x),
                _ => p,
            },
            Self::Flip { horizontal: true } => Pt::new(w - p.x, p.y),
            Self::Flip { horizontal: false } => Pt::new(p.x, h - p.y),
            Self::Resize { w: nw, h: nh } => Pt::new(
                p.x * nw.max(1) as f32 / w.max(1.0),
                p.y * nh.max(1) as f32 / h.max(1.0),
            ),
            Self::Canvas {
                w: cw,
                h: ch,
                anchor,
                ..
            } => {
                let (dx, dy) = canvas_offset(src, (cw.max(1), ch.max(1)), anchor);
                p.offset(dx as f32, dy as f32)
            }
            Self::CutOut {
                orientation,
                start,
                end,
                ..
            } => {
                let squeeze = |v: f32, len: u32| match cut_range(start, end, len) {
                    Some((s, e)) if v >= e as f32 => v - (e - s) as f32,
                    Some((s, _)) if v > s as f32 => s as f32,
                    _ => v,
                };
                match orientation {
                    Orientation::Horizontal => Pt::new(p.x, squeeze(p.y, src.1)),
                    Orientation::Vertical => Pt::new(squeeze(p.x, src.0), p.y),
                }
            }
            Self::ReplaceBase { .. } => p,
        }
    }

    /// Inverse de [`Self::map_point`] (pour annuler la retouche). Exacte sauf pour la
    /// bande coupée : ce qui était dedans revient à son bord.
    pub fn unmap_point(&self, src: (u32, u32), p: Pt) -> Pt {
        let out = self.output_size(src).unwrap_or(src);
        match *self {
            Self::Crop { .. } | Self::Canvas { .. } => {
                let o = self.map_point(src, Pt::default());
                p.offset(-o.x, -o.y)
            }
            Self::Rotate { quarters } => Self::Rotate {
                quarters: (4 - quarters % 4) % 4,
            }
            .map_point(out, p),
            Self::Flip { .. } => self.map_point(out, p),
            Self::Resize { .. } => Self::Resize { w: src.0, h: src.1 }.map_point(out, p),
            Self::CutOut {
                orientation,
                start,
                end,
                ..
            } => {
                let grow = |v: f32, len: u32| match cut_range(start, end, len) {
                    Some((s, e)) if v > s as f32 => v + (e - s) as f32,
                    _ => v,
                };
                match orientation {
                    Orientation::Horizontal => Pt::new(p.x, grow(p.y, src.1)),
                    Orientation::Vertical => Pt::new(grow(p.x, src.0), p.y),
                }
            }
            Self::ReplaceBase { .. } => p,
        }
    }

    /// Facteur appliqué aux épaisseurs et tailles (traits, police) : seul le
    /// redimensionnement en change.
    pub fn size_factor(&self, src: (u32, u32)) -> f32 {
        match *self {
            Self::Resize { w, h } if src.0 > 0 && src.1 > 0 => {
                ((w.max(1) as f32 / src.0 as f32) * (h.max(1) as f32 / src.1 as f32)).sqrt()
            }
            _ => 1.0,
        }
    }
}

/// L'image de travail : l'image d'origine et ses retouches, dans l'ordre. Sans
/// retouche, l'image d'origine telle quelle.
pub fn working_base(original: RgbaImage, edits: &[ImageEdit]) -> RgbaImage {
    edits.iter().fold(original, |img, edit| match edit {
        ImageEdit::ReplaceBase { path } => match crate::history::read_png(path) {
            Ok(new) => new,
            Err(e) => {
                log::warn!("image principale introuvable, on garde la précédente : {e:#}");
                img
            }
        },
        _ => edit.apply(&img),
    })
}

/// Déplace (et met à l'échelle) les annotations avec une retouche de l'image
/// d'entrée `src` ; `inverse` : dans l'autre sens, pour annuler la retouche.
pub fn transform_annotations(
    annotations: &mut [Annotation],
    edit: &ImageEdit,
    src: (u32, u32),
    inverse: bool,
) {
    let factor = edit.size_factor(src);
    let factor = if inverse { 1.0 / factor } else { factor };
    let map = |p: Pt| {
        if inverse {
            edit.unmap_point(src, p)
        } else {
            edit.map_point(src, p)
        }
    };
    for ann in annotations {
        transform_annotation(ann, &map, factor);
    }
}

/// Applique `f` à tous les points d'une annotation et `factor` à ses tailles. Les
/// textes et bulles gardent leur centre au bon endroit (ils ne pivotent pas).
pub fn transform_annotation(ann: &mut Annotation, f: &impl Fn(Pt) -> Pt, factor: f32) {
    ann.width *= factor;
    let mv = |p: &mut Pt| *p = f(*p);
    // Texte posé par son coin haut gauche : on déplace son centre.
    let recentre = |at: &mut Pt, text: &str, size: &mut f32| {
        let (w, h) = render::text_size(text, *size);
        let c = f(at.offset(w / 2.0, h / 2.0));
        *size *= factor;
        let (w, h) = render::text_size(text, *size);
        *at = c.offset(-w / 2.0, -h / 2.0);
    };
    let handled = match &mut ann.shape {
        Shape::Arrow { from, to, bend, .. } => {
            mv(from);
            mv(to);
            if let Some(b) = bend {
                mv(b);
            }
            true
        }
        Shape::Line { from, to } => {
            mv(from);
            mv(to);
            true
        }
        Shape::Rect { a, b, radius } => {
            mv(a);
            mv(b);
            *radius *= factor;
            true
        }
        Shape::Ellipse { a, b }
        | Shape::Highlight { a, b }
        | Shape::Spotlight { a, b }
        | Shape::Pixelate { a, b }
        | Shape::Blur { a, b } => {
            mv(a);
            mv(b);
            true
        }
        Shape::Pen { points } => {
            points.iter_mut().for_each(mv);
            true
        }
        Shape::Text { at, text, size, .. } => {
            recentre(at, text, size);
            true
        }
        Shape::Bubble {
            at,
            text,
            size,
            tail,
        } => {
            recentre(at, text, size);
            mv(tail);
            true
        }
        Shape::Step { at, tail, .. } => {
            mv(at);
            if let Some(t) = tail {
                mv(t);
            }
            true
        }
        Shape::Magnifier {
            source, radius, at, ..
        } => {
            mv(source);
            mv(at);
            *radius *= factor;
            true
        }
        Shape::Stamp { at, size, .. } => {
            mv(at);
            *size *= factor;
            true
        }
        // Formes ajoutées plus tard : déplacées en bloc avec leur centre.
        #[allow(unreachable_patterns)]
        _ => false,
    };
    if !handled {
        let c = ann.bounds(render::text_size).center();
        let d = f(c);
        ann.translate(d.x - c.x, d.y - c.y);
    }
}

/// Retire les lignes [`start`, `end`[ et dessine la jointure.
fn cut_rows(img: &RgbaImage, start: u32, end: u32, join: Join) -> RgbaImage {
    let (w, h) = img.dimensions();
    let Some((s, e)) = cut_range(start, end, h) else {
        return img.clone();
    };
    let mut out = RgbaImage::new(w, h - (e - s));
    imageops::replace(&mut out, &*imageops::crop_imm(img, 0, 0, w, s), 0, 0);
    imageops::replace(
        &mut out,
        &*imageops::crop_imm(img, 0, e, w, h - e),
        0,
        i64::from(s),
    );
    let oh = out.height();
    match join {
        Join::Straight => {}
        Join::Dotted => {
            for y in [s.saturating_sub(1), s] {
                if y >= oh {
                    continue;
                }
                for x in (0..w).filter(|x| (x / 8) % 2 == 0) {
                    out.put_pixel(x, y, Px([128, 128, 128, 255]));
                }
            }
        }
        Join::Torn => {
            // Deux bords déchirés, chacun mordant jusqu'à `depth` pixels, et un mince
            // espace transparent entre eux.
            let depth = 6.0;
            let gap = 1.5;
            let top = torn_profile(w, depth, 9.0, 1);
            let bottom = torn_profile(w, depth, 11.0, 2);
            for x in 0..w {
                let edge = s as f32 - gap - top[x as usize];
                let from = edge.floor().max(0.0) as u32;
                for y in from..s.min(oh) {
                    fade(&mut out, x, y, (edge - y as f32).clamp(0.0, 1.0));
                }
                let edge = s as f32 + gap + bottom[x as usize];
                let to = (edge.ceil() as u32).min(oh);
                for y in s..to {
                    fade(&mut out, x, y, (y as f32 + 1.0 - edge).clamp(0.0, 1.0));
                }
            }
        }
    }
    out
}

/// Multiplie l'opacité d'un pixel.
fn fade(img: &mut RgbaImage, x: u32, y: u32, coverage: f32) {
    let px = img.get_pixel_mut(x, y);
    px.0[3] = (f32::from(px.0[3]) * coverage).round() as u8;
}

/// Bruit déterministe dans [0, 1].
fn noise(i: u32, seed: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9) ^ seed.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    (x & 0xFFFF) as f32 / 65535.0
}

/// Profondeur d'une déchirure le long d'un bord de `len` pixels : dents irrégulières
/// tous les `step` pixels, entre 20 % et 100 % de `depth`.
fn torn_profile(len: u32, depth: f32, step: f32, seed: u32) -> Vec<f32> {
    (0..len)
        .map(|p| {
            let t = (p as f32 + 0.5) / step.max(1.0);
            let i = t.floor();
            let f = t - i;
            let a = 0.2 + 0.8 * noise(i as u32, seed);
            let b = 0.2 + 0.8 * noise(i as u32 + 1, seed);
            depth * (a + (b - a) * f)
        })
        .collect()
}

// --- Zone de travail automatique ---------------------------------------------------

/// Agrandit l'image pour que toutes les annotations y tiennent (« Zone de travail
/// automatique ») : rend l'image agrandie et les annotations décalées d'autant.
pub fn auto_canvas(
    base: &RgbaImage,
    annotations: &[Annotation],
    background: Background,
) -> (RgbaImage, Vec<Annotation>) {
    let (w, h) = base.dimensions();
    let bounds = annotations
        .iter()
        .map(|a| {
            let margin = a.width + if a.shadow { 12.0 } else { 2.0 };
            a.bounds(render::text_size).expand(margin)
        })
        .reduce(|a, b| a.union(b));
    let Some(b) = bounds else {
        return (base.clone(), annotations.to_vec());
    };
    let left = (-b.min.x).ceil().max(0.0) as u32;
    let top = (-b.min.y).ceil().max(0.0) as u32;
    let right = (b.max.x - w as f32).ceil().max(0.0) as u32;
    let bottom = (b.max.y - h as f32).ceil().max(0.0) as u32;
    if left + top + right + bottom == 0 {
        return (base.clone(), annotations.to_vec());
    }
    let mut out = RgbaImage::from_pixel(
        w + left + right,
        h + top + bottom,
        background_px(background, base),
    );
    imageops::replace(&mut out, base, i64::from(left), i64::from(top));
    let moved = annotations
        .iter()
        .cloned()
        .map(|mut a| {
            a.translate(left as f32, top as f32);
            a
        })
        .collect();
    (out, moved)
}

/// Export : image de travail + annotations (zone de travail automatique comprise),
/// puis bordure et effets. Sans réglage de ce lot : exactement [`render::render`].
pub fn apply_export(base: &RgbaImage, doc: &Doc) -> RgbaImage {
    let img = match doc.canvas_auto {
        Some(background) => {
            let (grown, annotations) = auto_canvas(base, &doc.annotations, background);
            render::render(&grown, &annotations)
        }
        None => render::render(base, &doc.annotations),
    };
    match &doc.border {
        Some(fx) if !fx.is_noop() => apply_border(&img, fx),
        _ => img,
    }
}

// --- Bordure et effets ---------------------------------------------------------------

/// Applique les effets à l'image aplatie.
pub fn apply_border(img: &RgbaImage, fx: &BorderEffects) -> RgbaImage {
    apply_border_scaled(img, fx, 1.0)
}

/// Comme [`apply_border`], longueurs multipliées par `scale` (aperçu réduit).
pub fn apply_border_scaled(img: &RgbaImage, fx: &BorderEffects, scale: f32) -> RgbaImage {
    if img.width() == 0 || img.height() == 0 {
        return img.clone();
    }
    let background = match fx.background {
        Background::Transparent => None,
        bg => Some(background_px(bg, img)),
    };
    let mut cur = img.clone();
    if fx.grayscale {
        grayscale(&mut cur);
    }
    let border = (fx.border_width as f32 * scale).round();
    let radius = fx.corner_radius as f32 * scale;
    if border >= 1.0 || radius >= 0.5 {
        cur = frame(&cur, border, radius, fx.border_color);
    }
    let depth = fx.torn_ratio.min(100) as f32 / 100.0 * 24.0 * scale;
    if fx.torn.any() && depth >= 1.0 {
        tear_edges(&mut cur, fx.torn, depth, (12.0 * scale).max(3.0));
    }
    if fx.rotation.abs() >= 0.05 {
        cur = rotate_free(&cur, fx.rotation);
    }
    if fx.reflection > 0 {
        let offset = (fx.reflection_offset as f32 * scale).round() as u32;
        cur = reflect(&cur, fx.reflection, offset);
    }
    if fx.shadow > 0 {
        cur = drop_shadow(&cur, fx.shadow, scale);
    }
    if let Some(bg) = background {
        for px in cur.pixels_mut() {
            *px = over(*px, bg);
        }
    }
    cur
}

/// Couleur de remplissage d'un fond.
fn background_px(background: Background, img: &RgbaImage) -> Px<u8> {
    match background {
        Background::Transparent => Px([0, 0, 0, 0]),
        Background::Color { color } => Px([color.0, color.1, color.2, color.3]),
        Background::Extend => edge_color(img),
    }
}

/// Couleur la plus fréquente sur le pourtour de l'image.
pub fn edge_color(img: &RgbaImage) -> Px<u8> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Px([255, 255, 255, 255]);
    }
    let mut counts: HashMap<[u8; 4], u32> = HashMap::new();
    let edge = (0..w)
        .flat_map(|x| [(x, 0), (x, h - 1)])
        .chain((0..h).flat_map(|y| [(0, y), (w - 1, y)]));
    for (x, y) in edge {
        *counts.entry(img.get_pixel(x, y).0).or_default() += 1;
    }
    let best = counts
        .into_iter()
        .max_by_key(|&(c, n)| (n, c))
        .map(|(c, _)| c)
        .unwrap_or([255; 4]);
    Px(best)
}

/// `src` posé sur `dst` (alpha non prémultiplié).
fn over(src: Px<u8>, dst: Px<u8>) -> Px<u8> {
    let sa = f32::from(src.0[3]) / 255.0;
    let da = f32::from(dst.0[3]) / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return Px([0, 0, 0, 0]);
    }
    let mix = |s: u8, d: u8| {
        ((f32::from(s) * sa + f32::from(d) * da * (1.0 - sa)) / oa)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Px([
        mix(src.0[0], dst.0[0]),
        mix(src.0[1], dst.0[1]),
        mix(src.0[2], dst.0[2]),
        (oa * 255.0).round() as u8,
    ])
}

fn grayscale(img: &mut RgbaImage) {
    for px in img.pixels_mut() {
        let [r, g, b, a] = px.0;
        let l = (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)).round() as u8;
        px.0 = [l, l, l, a];
    }
}

fn to_pixmap(img: &RgbaImage) -> Option<Pixmap> {
    let size = IntSize::from_wh(img.width(), img.height())?;
    let mut data = img.as_raw().clone();
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

fn from_pixmap(pixmap: &Pixmap) -> RgbaImage {
    let mut out = RgbaImage::new(pixmap.width(), pixmap.height());
    for (dst, src) in out.pixels_mut().zip(pixmap.pixels()) {
        let c = src.demultiply();
        dst.0 = [c.red(), c.green(), c.blue(), c.alpha()];
    }
    out
}

/// Rectangle aux coins arrondis.
fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = r * (1.0 - 0.552_284_8);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - k, y, x + w, y + k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - k, x + w - k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + k, y + h, x, y + h - k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + k, x + k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Bord de `border` pixels autour de l'image et coins arrondis (`radius` : rayon
/// extérieur).
fn frame(img: &RgbaImage, border: f32, radius: f32, color: Rgba) -> RgbaImage {
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let (w, h) = (
        img.width() + 2 * border as u32,
        img.height() + 2 * border as u32,
    );
    let (Some(src), Some(mut out)) = (to_pixmap(img), Pixmap::new(w, h)) else {
        return img.clone();
    };
    if border > 0.0
        && let Some(path) = rounded_rect(0.0, 0.0, w as f32, h as f32, radius)
    {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.0, color.1, color.2, color.3);
        paint.anti_alias = true;
        out.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if let Some(path) = rounded_rect(border, border, iw, ih, (radius - border).max(0.0)) {
        let paint = Paint {
            shader: Pattern::new(
                src.as_ref(),
                SpreadMode::Pad,
                FilterQuality::Nearest,
                1.0,
                Transform::from_translate(border, border),
            ),
            blend_mode: BlendMode::Source,
            anti_alias: true,
            ..Paint::default()
        };
        out.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    from_pixmap(&out)
}

/// Bords déchirés sur les côtés choisis, jusqu'à `depth` pixels de profondeur.
fn tear_edges(img: &mut RgbaImage, sides: TornSides, depth: f32, step: f32) {
    let (w, h) = img.dimensions();
    let reach = (depth.ceil() as u32 + 1).min(w.min(h));
    // (actif, longueur du bord, graine, pixel à `d` du bord à la position `p`)
    type At = fn(u32, u32, u32, u32) -> (u32, u32);
    let all: [(bool, u32, u32, At); 4] = [
        (sides.top, w, 11, |p, d, _, _| (p, d)),
        (sides.bottom, w, 12, |p, d, _, h| (p, h - 1 - d)),
        (sides.left, h, 13, |p, d, _, _| (d, p)),
        (sides.right, h, 14, |p, d, w, _| (w - 1 - d, p)),
    ];
    for (on, len, seed, at) in all {
        if !on {
            continue;
        }
        let profile = torn_profile(len, depth, step, seed);
        for (p, cut) in profile.iter().enumerate() {
            for d in 0..reach {
                let (x, y) = at(p as u32, d, w, h);
                fade(img, x, y, (d as f32 + 1.0 - cut).clamp(0.0, 1.0));
            }
        }
    }
}

/// Rotation libre, dans un cadre agrandi (coins transparents).
fn rotate_free(img: &RgbaImage, degrees: f32) -> RgbaImage {
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let rad = degrees.to_radians();
    let (sin, cos) = (rad.sin().abs(), rad.cos().abs());
    let w = (iw * cos + ih * sin).ceil().max(1.0) as u32;
    let h = (iw * sin + ih * cos).ceil().max(1.0) as u32;
    let (Some(src), Some(mut out)) = (to_pixmap(img), Pixmap::new(w, h)) else {
        return img.clone();
    };
    let ts = Transform::from_translate(-iw / 2.0, -ih / 2.0)
        .post_rotate(degrees)
        .post_translate(w as f32 / 2.0, h as f32 / 2.0);
    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };
    out.draw_pixmap(0, 0, src.as_ref(), &paint, ts, None);
    from_pixmap(&out)
}

/// Reflet sous l'image : `percent` de sa hauteur, à `offset` pixels, en fondu.
fn reflect(img: &RgbaImage, percent: u32, offset: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let rh = ((h as f32 * percent.min(100) as f32 / 100.0).round() as u32).clamp(1, h);
    let mut out = RgbaImage::new(w, h + offset + rh);
    imageops::replace(&mut out, img, 0, 0);
    for i in 0..rh {
        let k = 0.5 * (1.0 - (i as f32 + 0.5) / rh as f32);
        for x in 0..w {
            let mut px = *img.get_pixel(x, h - 1 - i);
            px.0[3] = (f32::from(px.0[3]) * k).round() as u8;
            out.put_pixel(x, h + offset + i, px);
        }
    }
    out
}

/// Flou « boîte » d'une ligne de valeurs espacées de `stride` (zéro au-delà des bords).
fn blur_line(
    data: &mut [f32],
    first: usize,
    len: usize,
    stride: usize,
    radius: usize,
    buf: &mut Vec<f32>,
) {
    buf.clear();
    buf.extend((0..len).map(|j| data[first + j * stride]));
    let get = |k: isize| {
        if k < 0 || k >= len as isize {
            0.0
        } else {
            buf[k as usize]
        }
    };
    let r = radius as isize;
    let norm = 1.0 / (2 * radius + 1) as f32;
    let mut sum: f32 = (-r..=r).map(get).sum();
    for j in 0..len {
        data[first + j * stride] = sum * norm;
        let j = j as isize;
        sum += get(j + r + 1) - get(j - r);
    }
}

/// Flou « boîte » d'un plan, répété trois fois (proche d'un flou gaussien).
fn blur_plane(data: &mut [f32], w: usize, h: usize, radius: usize) {
    if radius == 0 {
        return;
    }
    let mut buf = Vec::new();
    for _ in 0..3 {
        for y in 0..h {
            blur_line(data, y * w, w, 1, radius, &mut buf);
        }
        for x in 0..w {
            blur_line(data, x, h, w, radius, &mut buf);
        }
    }
}

/// Ombre portée douce, en bas à droite, d'opacité `percent` : agrandit le cadre.
fn drop_shadow(img: &RgbaImage, percent: u32, scale: f32) -> RgbaImage {
    let (iw, ih) = img.dimensions();
    let blur = (14.0 * scale).round().max(1.0) as u32;
    let (ox, oy) = (
        (blur as f32 * 0.4).round() as u32,
        (blur as f32 * 0.6).round() as u32,
    );
    let (w, h) = (iw + 2 * blur + ox, ih + 2 * blur + oy);
    let (wu, hu) = (w as usize, h as usize);
    let mut alpha = vec![0.0f32; wu * hu];
    for (x, y, px) in img.enumerate_pixels() {
        alpha[(y + blur + oy) as usize * wu + (x + blur + ox) as usize] = f32::from(px.0[3]);
    }
    blur_plane(&mut alpha, wu, hu, (blur / 3).max(1) as usize);
    let strength = percent.min(100) as f32 / 100.0 * 0.8;
    let mut out = RgbaImage::new(w, h);
    for (i, px) in out.pixels_mut().enumerate() {
        px.0 = [
            0,
            0,
            0,
            (alpha[i] * strength).round().clamp(0.0, 255.0) as u8,
        ];
    }
    for (x, y, px) in img.enumerate_pixels() {
        let dst = out.get_pixel_mut(x + blur, y + blur);
        *dst = over(*px, *dst);
    }
    out
}

// --- Résolution (DPI) ---------------------------------------------------------------

fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in bytes {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// Morceaux d'un PNG : (début, type, données), après la signature.
fn png_chunks(png: &[u8]) -> Vec<(usize, [u8; 4], &[u8])> {
    let mut chunks = Vec::new();
    let mut i = 8;
    while i + 12 <= png.len() {
        let len = u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]) as usize;
        let kind = [png[i + 4], png[i + 5], png[i + 6], png[i + 7]];
        let Some(data) = png.get(i + 8..i + 8 + len) else {
            break;
        };
        chunks.push((i, kind, data));
        i += 12 + len;
    }
    chunks
}

/// Résolution enregistrée dans un PNG (morceau `pHYs` en pixels par mètre).
pub fn png_dpi(png: &[u8]) -> Option<f32> {
    png_chunks(png)
        .into_iter()
        .find(|(_, kind, data)| kind == b"pHYs" && data.len() == 9 && data[8] == 1)
        .map(|(_, _, d)| u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as f32 * 0.0254)
}

/// Le même PNG avec la résolution `dpi` (métadonnée seule, les pixels ne changent
/// pas). `None` si ce n'est pas un PNG.
pub fn png_with_dpi(png: &[u8], dpi: f32) -> Option<Vec<u8>> {
    let chunks = png_chunks(png);
    if png.get(..8) != Some(b"\x89PNG\r\n\x1a\n".as_slice())
        || chunks.first().map(|c| &c.1) != Some(b"IHDR")
    {
        return None;
    }
    let ppm = (dpi.max(1.0) / 0.0254).round() as u32;
    let mut body = b"pHYs".to_vec();
    body.extend_from_slice(&ppm.to_be_bytes());
    body.extend_from_slice(&ppm.to_be_bytes());
    body.push(1);
    let mut out = png[..8].to_vec();
    for (n, (start, kind, data)) in chunks.iter().enumerate() {
        if kind != b"pHYs" {
            out.extend_from_slice(&png[*start..start + 12 + data.len()]);
        }
        if n == 0 {
            out.extend_from_slice(&9u32.to_be_bytes());
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
        }
    }
    Some(out)
}

/// Écrit la résolution dans un PNG exporté (rien sans résolution choisie).
pub fn write_dpi(path: &Path, dpi: Option<f32>) {
    let Some(dpi) = dpi else { return };
    let result = std::fs::read(path)
        .map_err(anyhow::Error::from)
        .and_then(|bytes| {
            let out = png_with_dpi(&bytes, dpi).ok_or_else(|| anyhow::anyhow!("PNG illisible"))?;
            crate::config::write_atomic(path, &out)
        });
    if let Err(e) = result {
        log::warn!("résolution non écrite dans {} : {e:#}", path.display());
    }
}

// --- Calculs des boîtes de dialogue -------------------------------------------------

/// Façon de choisir la nouvelle taille (« Redimensionner »).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResizeMode {
    Percent(f32),
    /// Largeur, hauteur proportionnelle.
    Width(u32),
    /// Hauteur, largeur proportionnelle.
    Height(u32),
    /// Plus grand côté (l'image n'est jamais agrandie).
    MaxSide(u32),
    /// Largeur et hauteur ; avec `keep_ratio`, l'image tient dans ce cadre.
    Exact {
        w: u32,
        h: u32,
        keep_ratio: bool,
    },
}

/// Dimensions obtenues (au moins 1 × 1).
pub fn resize_target(mode: ResizeMode, src: (u32, u32)) -> (u32, u32) {
    let (w, h) = (src.0.max(1) as f32, src.1.max(1) as f32);
    let scaled = |k: f32| ((w * k).round(), (h * k).round());
    let (nw, nh) = match mode {
        ResizeMode::Percent(p) => scaled(p.max(0.0) / 100.0),
        ResizeMode::Width(nw) => scaled(nw as f32 / w),
        ResizeMode::Height(nh) => scaled(nh as f32 / h),
        ResizeMode::MaxSide(m) => scaled((m as f32 / w.max(h)).min(1.0)),
        ResizeMode::Exact {
            w: nw,
            h: nh,
            keep_ratio: false,
        } => (nw as f32, nh as f32),
        ResizeMode::Exact {
            w: nw,
            h: nh,
            keep_ratio: true,
        } => scaled((nw as f32 / w).min(nh as f32 / h)),
    };
    (
        nw.clamp(1.0, 65_535.0) as u32,
        nh.clamp(1.0, 65_535.0) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PALETTE;

    fn gradient(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| Px([x as u8, y as u8, 7, 255]))
    }

    fn step_at(x: f32, y: f32) -> Annotation {
        Annotation::new(
            Shape::Step {
                at: Pt::new(x, y),
                number: 1,
                tail: None,
            },
            PALETTE[0].0,
            4.0,
        )
    }

    #[test]
    fn crop_keeps_the_rectangle() {
        let img = gradient(40, 30);
        let edit = ImageEdit::Crop {
            x: 5,
            y: 4,
            w: 10,
            h: 8,
        };
        let out = edit.apply(&img);
        assert_eq!(out.dimensions(), (10, 8));
        assert_eq!(out.get_pixel(0, 0), img.get_pixel(5, 4));
        assert_eq!(edit.output_size((40, 30)), Some((10, 8)));
        assert_eq!(
            edit.map_point((40, 30), Pt::new(6.0, 5.0)),
            Pt::new(1.0, 1.0)
        );
        // Débordement : borné à l'image.
        let out = ImageEdit::Crop {
            x: 35,
            y: 0,
            w: 50,
            h: 50,
        }
        .apply(&img);
        assert_eq!(out.dimensions(), (5, 30));
    }

    #[test]
    fn rotation_moves_pixels_and_points_together() {
        let img = gradient(40, 30);
        for quarters in 1..=3u8 {
            let edit = ImageEdit::Rotate { quarters };
            let out = edit.apply(&img);
            assert_eq!(Some(out.dimensions()), edit.output_size((40, 30)));
            // Le centre du pixel (3, 2) va au centre du pixel correspondant.
            let p = edit.map_point((40, 30), Pt::new(3.5, 2.5));
            assert_eq!(
                out.get_pixel(p.x as u32, p.y as u32),
                img.get_pixel(3, 2),
                "{quarters}"
            );
            let back = edit.unmap_point((40, 30), p);
            assert!(back.dist(Pt::new(3.5, 2.5)) < 1e-4, "{quarters} : {back:?}");
        }
        assert_eq!(
            ImageEdit::Rotate { quarters: 1 }.map_point((40, 30), Pt::new(0.0, 0.0)),
            Pt::new(30.0, 0.0)
        );
    }

    #[test]
    fn flip_mirrors_points() {
        let img = gradient(40, 30);
        let edit = ImageEdit::Flip { horizontal: true };
        let out = edit.apply(&img);
        let p = edit.map_point((40, 30), Pt::new(3.5, 2.5));
        assert_eq!(out.get_pixel(p.x as u32, p.y as u32), img.get_pixel(3, 2));
        let edit = ImageEdit::Flip { horizontal: false };
        assert_eq!(
            edit.map_point((40, 30), Pt::new(1.0, 1.0)),
            Pt::new(1.0, 29.0)
        );
    }

    #[test]
    fn resize_scales_points_and_sizes() {
        let img = gradient(40, 30);
        let edit = ImageEdit::Resize { w: 20, h: 15 };
        assert_eq!(edit.apply(&img).dimensions(), (20, 15));
        let mut anns = vec![step_at(10.0, 10.0)];
        transform_annotations(&mut anns, &edit, (40, 30), false);
        assert_eq!(anns[0].width, 2.0);
        assert!(matches!(anns[0].shape, Shape::Step { at, .. } if at == Pt::new(5.0, 5.0)));
        transform_annotations(&mut anns, &edit, (40, 30), true);
        assert_eq!(anns[0], step_at(10.0, 10.0));
    }

    #[test]
    fn resize_targets() {
        let src = (1400, 800);
        assert_eq!(resize_target(ResizeMode::Percent(50.0), src), (700, 400));
        assert_eq!(resize_target(ResizeMode::Width(700), src), (700, 400));
        assert_eq!(resize_target(ResizeMode::Height(200), src), (350, 200));
        assert_eq!(resize_target(ResizeMode::MaxSide(700), src), (700, 400));
        assert_eq!(resize_target(ResizeMode::MaxSide(5000), src), src);
        let exact = |keep_ratio| ResizeMode::Exact {
            w: 500,
            h: 500,
            keep_ratio,
        };
        assert_eq!(resize_target(exact(false), src), (500, 500));
        assert_eq!(resize_target(exact(true), src), (500, 286));
    }

    #[test]
    fn canvas_anchors_the_image() {
        let img = RgbaImage::from_pixel(10, 10, Px([255, 0, 0, 255]));
        let edit = ImageEdit::Canvas {
            w: 30,
            h: 20,
            anchor: Anchor { col: 2, row: 0 },
            background: Background::Transparent,
        };
        let out = edit.apply(&img);
        assert_eq!(out.dimensions(), (30, 20));
        assert_eq!(out.get_pixel(25, 5).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(5, 5).0[3], 0, "fond transparent");
        assert_eq!(
            edit.map_point((10, 10), Pt::new(0.0, 0.0)),
            Pt::new(20.0, 0.0)
        );
        assert_eq!(
            canvas_offset((10, 10), (30, 20), Anchor::default()),
            (10, 5)
        );
        // Plus petit : rogné autour du centre.
        assert_eq!(canvas_offset((10, 10), (4, 4), Anchor::default()), (-3, -3));
        let colored = ImageEdit::Canvas {
            w: 12,
            h: 12,
            anchor: Anchor::default(),
            background: Background::Color {
                color: Rgba::rgb(0, 0, 255),
            },
        }
        .apply(&img);
        assert_eq!(colored.get_pixel(0, 0).0, [0, 0, 255, 255]);
    }

    #[test]
    fn cut_out_removes_the_band() {
        let img = gradient(40, 30);
        for join in [Join::Straight, Join::Dotted, Join::Torn] {
            let edit = ImageEdit::CutOut {
                orientation: Orientation::Horizontal,
                start: 10,
                end: 20,
                join,
            };
            let out = edit.apply(&img);
            assert_eq!(out.dimensions(), (40, 20), "{join:?}");
            assert_eq!(edit.output_size((40, 30)), Some((40, 20)));
            let edit = ImageEdit::CutOut {
                orientation: Orientation::Vertical,
                start: 5,
                end: 15,
                join,
            };
            assert_eq!(edit.apply(&img).dimensions(), (30, 30), "{join:?}");
        }
        let straight = ImageEdit::CutOut {
            orientation: Orientation::Horizontal,
            start: 10,
            end: 20,
            join: Join::Straight,
        };
        let out = straight.apply(&img);
        assert_eq!(out.get_pixel(3, 10), img.get_pixel(3, 20));
        assert_eq!(out.get_pixel(3, 9), img.get_pixel(3, 9));
        // Points : au-dessus inchangés, en dessous remontés, dans la bande au bord.
        let m = |y| straight.map_point((40, 30), Pt::new(1.0, y)).y;
        assert_eq!((m(5.0), m(15.0), m(25.0)), (5.0, 10.0, 15.0));
        // Vertical : la colonne 15 devient la 5.
        let vertical = ImageEdit::CutOut {
            orientation: Orientation::Vertical,
            start: 5,
            end: 15,
            join: Join::Straight,
        };
        assert_eq!(vertical.apply(&img).get_pixel(5, 3), img.get_pixel(15, 3));
        // Bande vide ou couvrant tout : rien.
        let none = ImageEdit::CutOut {
            orientation: Orientation::Horizontal,
            start: 0,
            end: 30,
            join: Join::Straight,
        };
        assert_eq!(none.apply(&img), img);
    }

    #[test]
    fn torn_join_is_transparent_at_the_seam() {
        let img = RgbaImage::from_pixel(50, 40, Px([200, 200, 200, 255]));
        let out = ImageEdit::CutOut {
            orientation: Orientation::Horizontal,
            start: 20,
            end: 25,
            join: Join::Torn,
        }
        .apply(&img);
        assert!((0..50).all(|x| out.get_pixel(x, 20).0[3] == 0));
        assert!((0..50).all(|x| out.get_pixel(x, 0).0[3] == 255));
        assert!((0..50).all(|x| out.get_pixel(x, 34).0[3] == 255));
    }

    #[test]
    fn text_keeps_its_centre_when_rotated() {
        let mut ann = Annotation::new(
            Shape::Text {
                at: Pt::new(10.0, 10.0),
                text: "Bonjour".into(),
                size: 20.0,
                boxed: false,
            },
            PALETTE[0].0,
            4.0,
        );
        let before = ann.bounds(render::text_size).center();
        let edit = ImageEdit::Rotate { quarters: 1 };
        transform_annotations(std::slice::from_mut(&mut ann), &edit, (200, 100), false);
        let after = ann.bounds(render::text_size).center();
        assert!(after.dist(edit.map_point((200, 100), before)) < 1e-3);
    }

    #[test]
    fn untouched_documents_export_identically() {
        let img = gradient(20, 20);
        let doc = Doc {
            annotations: vec![step_at(10.0, 10.0)],
            ..Doc::default()
        };
        assert_eq!(working_base(img.clone(), &[]), img);
        assert_eq!(
            apply_export(&img, &doc),
            render::render(&img, &doc.annotations)
        );
        let noop = Doc {
            border: Some(BorderEffects::default()),
            ..doc.clone()
        };
        assert_eq!(apply_export(&img, &noop), apply_export(&img, &doc));
        // Le JSON d'un document sans retouche ne change pas.
        let json = serde_json::to_string(&Doc::default()).unwrap();
        assert_eq!(json, r#"{"annotations":[]}"#);
    }

    #[test]
    fn edits_and_effects_round_trip_in_json() {
        let doc = Doc {
            edits: vec![
                ImageEdit::Crop {
                    x: 1,
                    y: 2,
                    w: 3,
                    h: 4,
                },
                ImageEdit::CutOut {
                    orientation: Orientation::Vertical,
                    start: 1,
                    end: 2,
                    join: Join::Dotted,
                },
                ImageEdit::ReplaceBase {
                    path: PathBuf::from("/tmp/x.png"),
                },
            ],
            border: Some(BorderEffects::shadow_preset()),
            canvas_auto: Some(Background::Extend),
            dpi: Some(144.0),
            ..Doc::default()
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert_eq!(serde_json::from_str::<Doc>(&json).unwrap(), doc);
    }

    #[test]
    fn replace_base_keeps_annotations_and_reads_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autre.png");
        let other = RgbaImage::from_pixel(7, 9, Px([1, 2, 3, 255]));
        crate::history::write_png(&path, &other).unwrap();
        let edits = [
            ImageEdit::Rotate { quarters: 1 },
            ImageEdit::ReplaceBase { path: path.clone() },
        ];
        assert_eq!(working_base(gradient(4, 4), &edits), other);
        // Fichier disparu : on garde l'image précédente.
        let missing = [ImageEdit::ReplaceBase {
            path: dir.path().join("absent.png"),
        }];
        assert_eq!(working_base(gradient(4, 4), &missing), gradient(4, 4));
    }

    #[test]
    fn auto_canvas_grows_around_outside_annotations() {
        let img = gradient(20, 20);
        let (grown, anns) = auto_canvas(&img, &[step_at(-10.0, 10.0)], Background::Transparent);
        assert!(grown.width() > 20 + 10, "{}", grown.width());
        let Shape::Step { at, .. } = anns[0].shape else {
            panic!("pastille attendue")
        };
        let (dx, dy) = (at.x + 10.0, at.y - 10.0);
        assert!(dx >= 10.0 && dy >= 0.0);
        assert_eq!(grown.get_pixel(dx as u32, dy as u32), img.get_pixel(0, 0));
        assert_eq!(grown.get_pixel(0, 0).0[3], 0, "fond transparent");
        // Tout tient : rien ne change.
        let (same, _) = auto_canvas(&img, &[], Background::Transparent);
        assert_eq!(same, img);
    }

    #[test]
    fn border_and_rounded_corners() {
        let img = RgbaImage::from_pixel(20, 10, Px([255, 255, 255, 255]));
        let fx = BorderEffects {
            border_width: 3,
            border_color: Rgba::rgb(255, 0, 0),
            ..BorderEffects::default()
        };
        let out = apply_border(&img, &fx);
        assert_eq!(out.dimensions(), (26, 16));
        assert_eq!(out.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(13, 8).0, [255, 255, 255, 255]);
        let round = BorderEffects {
            corner_radius: 5,
            ..BorderEffects::default()
        };
        let out = apply_border(&img, &round);
        assert_eq!(out.dimensions(), (20, 10));
        assert_eq!(out.get_pixel(0, 0).0[3], 0);
        assert_eq!(out.get_pixel(10, 5).0[3], 255);
    }

    #[test]
    fn torn_edges_cut_only_the_chosen_sides() {
        let img = RgbaImage::from_pixel(100, 80, Px([255, 255, 255, 255]));
        let fx = BorderEffects {
            torn: TornSides {
                top: true,
                ..TornSides::default()
            },
            torn_ratio: 100,
            ..BorderEffects::default()
        };
        let out = apply_border(&img, &fx);
        assert!((0..100).all(|x| out.get_pixel(x, 0).0[3] == 0));
        assert!((0..100).all(|x| out.get_pixel(x, 79).0[3] == 255));
        assert!((0..80).skip(30).all(|y| out.get_pixel(0, y).0[3] == 255));
        // Ratio nul : effet désactivé.
        let off = BorderEffects {
            torn_ratio: 0,
            ..fx
        };
        assert!(off.is_noop());
    }

    #[test]
    fn shadow_extends_the_canvas() {
        let img = RgbaImage::from_pixel(40, 30, Px([255, 255, 255, 255]));
        let out = apply_border(&img, &BorderEffects::shadow_preset());
        assert!(out.width() > 40 && out.height() > 30);
        // L'image est intacte, l'ombre visible en bas à droite, le coin opposé vide.
        assert_eq!(out.get_pixel(14 + 5, 14 + 5).0, [255, 255, 255, 255]);
        let (w, h) = out.dimensions();
        assert!(out.get_pixel(w - 12, h - 10).0[3] > 0);
        assert_eq!(out.get_pixel(0, 0).0[3], 0);
        // Avec un fond coloré, plus de transparence.
        let fx = BorderEffects {
            background: Background::Color {
                color: Rgba::rgb(0, 128, 0),
            },
            ..BorderEffects::shadow_preset()
        };
        let out = apply_border(&img, &fx);
        assert_eq!(out.get_pixel(0, 0).0, [0, 128, 0, 255]);
    }

    #[test]
    fn reflection_adds_a_faded_copy_below() {
        let img = gradient(10, 20);
        let fx = BorderEffects {
            reflection: 50,
            reflection_offset: 4,
            ..BorderEffects::default()
        };
        let out = apply_border(&img, &fx);
        assert_eq!(out.dimensions(), (10, 20 + 4 + 10));
        assert_eq!(out.get_pixel(0, 21).0[3], 0, "espace");
        let first = out.get_pixel(3, 24);
        assert_eq!(&first.0[..3], &img.get_pixel(3, 19).0[..3]);
        assert!(first.0[3] > out.get_pixel(3, 33).0[3]);
    }

    #[test]
    fn rotation_and_grayscale() {
        let img = RgbaImage::from_pixel(40, 20, Px([255, 0, 0, 255]));
        let rotated = apply_border(
            &img,
            &BorderEffects {
                rotation: 90.0,
                ..BorderEffects::default()
            },
        );
        assert!(rotated.width().abs_diff(20) <= 1 && rotated.height().abs_diff(40) <= 1);
        let tilted = apply_border(
            &img,
            &BorderEffects {
                rotation: 10.0,
                ..BorderEffects::default()
            },
        );
        assert!(tilted.width() > 40 && tilted.height() > 20);
        assert_eq!(tilted.get_pixel(0, 0).0[3], 0);
        let gray = apply_border(
            &img,
            &BorderEffects {
                grayscale: true,
                ..BorderEffects::default()
            },
        );
        let [r, g, b, _] = gray.get_pixel(0, 0).0;
        assert!(r == g && g == b && r == 76);
    }

    #[test]
    fn extend_takes_the_edge_colour() {
        let mut img = RgbaImage::from_pixel(10, 10, Px([9, 9, 9, 255]));
        img.put_pixel(0, 0, Px([1, 1, 1, 255]));
        assert_eq!(edge_color(&img).0, [9, 9, 9, 255]);
        let out = ImageEdit::Canvas {
            w: 20,
            h: 20,
            anchor: Anchor::default(),
            background: Background::Extend,
        }
        .apply(&img);
        assert_eq!(out.get_pixel(0, 0).0, [9, 9, 9, 255]);
    }

    #[test]
    fn dpi_is_written_as_phys() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.png");
        let img = gradient(5, 4);
        crate::history::write_png(&path, &img).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(png_dpi(&bytes), None);
        write_dpi(&path, Some(144.0));
        let bytes = std::fs::read(&path).unwrap();
        assert!((png_dpi(&bytes).unwrap() - 144.0).abs() < 0.1);
        assert_eq!(crate::history::read_png(&path).unwrap(), img);
        // Une seconde fois : remplacé, pas ajouté.
        let again = png_with_dpi(&bytes, 72.0).unwrap();
        assert_eq!(again.len(), bytes.len());
        assert!((png_dpi(&again).unwrap() - 72.0).abs() < 0.1);
        assert_eq!(png_with_dpi(b"pas un png", 72.0), None);
    }
}
