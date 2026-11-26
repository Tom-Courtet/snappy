//! Lot « Aperçus et tampons » : vignettes rendues par le vrai moteur de rendu
//! (`render.rs`, tiny-skia), comme dans Screenpresso.
//!
//! Chaque vignette (préréglage, favori, variante d'une option, tampon) est une petite
//! scène dessinée avec le style voulu, puis gardée en texture egui. La clé du cache
//! réunit ce qu'on dessine, le style, la taille de la vignette et l'échelle de
//! l'écran : rien n'est redessiné d'une image à l'autre, seulement quand le style
//! change.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::{
    Color32, ColorImage, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, TextureHandle,
    TextureId, TextureOptions, Ui, Vec2, pos2,
};
use image::RgbaImage;

use super::preview::color32;
use super::tools::{
    FILE_PREFIX, LABEL_PREFIX, Metrics, Size, Style, Tool, extend_shape, new_shape,
};
use crate::model::{
    Annotation, BracketSide, BubbleKind, Pt, Rgba, Shape, StepTail, bubble_box, label_box,
    stamp_box, text_padding,
};
use crate::render;
use crate::ui::theme;

/// Scène des aperçus d'outil, en unités (mise à l'échelle de la vignette).
const SCENE: (f32, f32) = (96.0, 60.0);
/// Texte des aperçus de texte et de bulle.
const SAMPLE_TEXT: &str = "AaBb";
/// Nombre maximal de textures gardées (au-delà, le cache repart de zéro).
const MAX_TEXTURES: usize = 600;

/// Fond clair des vignettes.
pub const PAPER_LIGHT: Rgba = Rgba::rgb(0xEC, 0xEC, 0xEF);
/// Fond sombre des vignettes (couleurs claires : blanc, jaune, lime).
pub const PAPER_DARK: Rgba = Rgba::rgb(0x2E, 0x2E, 0x34);

/// Fond d'une vignette pour un style de cette couleur : clair, sauf pour les couleurs
/// claires.
pub fn paper(color: Rgba) -> Rgba {
    if color.luminance() > 0.6 {
        PAPER_DARK
    } else {
        PAPER_LIGHT
    }
}

/// Ce que montre une vignette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sample {
    /// Forme type de l'outil, au style donné.
    Tool(Tool),
    /// Le tampon du style seul, à la taille de la vignette.
    Stamp,
}

/// Empreinte d'un style (toutes ses options).
pub fn style_hash(style: &Style) -> u64 {
    let mut h = DefaultHasher::new();
    serde_json::to_string(style)
        .unwrap_or_default()
        .hash(&mut h);
    h.finish()
}

/// Clé du cache : quoi, à quel style, à quelle taille (points) et quelle échelle
/// d'écran (pixels par point).
pub fn key(sample: Sample, style: u64, size: Vec2, ppp: f32) -> u64 {
    let mut h = DefaultHasher::new();
    sample.hash(&mut h);
    style.hash(&mut h);
    size.x.to_bits().hash(&mut h);
    size.y.to_bits().hash(&mut h);
    ppp.to_bits().hash(&mut h);
    h.finish()
}

/// Taille en pixels d'une vignette de `size` points.
pub fn pixels(size: Vec2, ppp: f32) -> (u32, u32) {
    (
        (size.x * ppp).round().max(1.0) as u32,
        (size.y * ppp).round().max(1.0) as u32,
    )
}

/// L'outil a-t-il besoin d'une image dessous (effets) ?
fn needs_content(tool: Tool) -> bool {
    matches!(
        tool,
        Tool::Highlight | Tool::Spotlight | Tool::Blur | Tool::Pixelate | Tool::Magnifier
    )
}

/// Fond d'une vignette d'outil, peint sous l'image rendue (transparente sauf pour
/// les effets, qui ont leur propre fond).
pub fn tile_paper(sample: Sample, style: &Style) -> Color32 {
    match sample {
        Sample::Tool(_) => color32(paper(style.color)),
        Sample::Stamp => theme::SURFACE_HI,
    }
}

/// Image de fond des effets : du texte et deux formes de couleur sur le papier.
fn content(w: u32, h: u32, place: &impl Fn(f32, f32) -> Pt, d: f32, paper: Rgba) -> RgbaImage {
    let base = RgbaImage::from_pixel(w, h, image::Rgba([paper.0, paper.1, paper.2, 255]));
    let ink = if paper == PAPER_DARK {
        Rgba::rgb(0xD8, 0xD8, 0xDE)
    } else {
        Rgba::rgb(0x3A, 0x3A, 0x44)
    };
    let text = |at: Pt, text: &str, size: f32| {
        Annotation::new(
            Shape::Text {
                at,
                text: text.into(),
                size,
                boxed: false,
            },
            ink,
            1.0,
        )
    };
    let mut block = Annotation::new(
        Shape::Rect {
            a: place(64.0, 8.0),
            b: place(90.0, 28.0),
            radius: 3.0 * d,
        },
        Rgba::rgb(0x0A, 0x84, 0xFF),
        2.0 * d,
    );
    block.fill = true;
    let mut dot = Annotation::new(
        Shape::Ellipse {
            a: place(66.0, 34.0),
            b: place(86.0, 54.0),
        },
        Rgba::rgb(0xFF, 0x95, 0x00),
        2.0 * d,
    );
    dot.fill = true;
    let anns = [
        text(place(6.0, 6.0), "AaBb", 20.0 * d),
        text(place(6.0, 32.0), "Snappy 123", 11.0 * d),
        text(place(6.0, 46.0), "Lorem ipsum", 9.0 * d),
        block,
        dot,
    ];
    render::render(&base, &anns)
}

/// Forme type de l'outil (sans le fond), en pixels de la vignette.
fn tool_shapes(
    tool: Tool,
    style: &Style,
    m: Metrics,
    place: &impl Fn(f32, f32) -> Pt,
    d: f32,
    w: f32,
    h: f32,
) -> Vec<Shape> {
    let drag = |from: Pt, to: Pt| {
        new_shape(tool, from, style, m, 1).map(|mut s| {
            extend_shape(&mut s, to, false);
            s
        })
    };
    let center = Pt::new(w / 2.0, h / 2.0);
    let shape = match tool {
        Tool::Select => None,
        // Cote par l'extérieur : à plat, assez courte pour que ses bouts tiennent.
        Tool::Arrow if style.arrow == crate::model::ArrowStyle::Inward => {
            drag(place(36.0, 30.0), place(62.0, 30.0))
        }
        Tool::Arrow => drag(place(14.0, 50.0), place(82.0, 12.0)),
        Tool::Line => drag(place(14.0, 46.0), place(82.0, 14.0)),
        Tool::Rect | Tool::Ellipse => drag(place(18.0, 12.0), place(78.0, 48.0)),
        Tool::Highlight => drag(place(4.0, 6.0), place(58.0, 30.0)),
        Tool::Spotlight => drag(place(26.0, 12.0), place(72.0, 48.0)),
        Tool::Blur | Tool::Pixelate => drag(place(40.0, 2.0), place(94.0, 58.0)),
        Tool::Pen => Some(Shape::Pen {
            points: (0..=36)
                .map(|i| {
                    let t = i as f32;
                    place(12.0 + t * 2.0, 30.0 + (t * 0.35).sin() * 14.0)
                })
                .collect(),
        }),
        Tool::Text => {
            let mut size = m.text;
            let fit = |size: f32| {
                let (tw, th) = render::text_size(SAMPLE_TEXT, size);
                let (px, py) = if style.boxed {
                    text_padding(size)
                } else {
                    (0.0, 0.0)
                };
                (tw + 2.0 * px, th + 2.0 * py)
            };
            let (bw, bh) = fit(size);
            size *= (w * 0.9 / bw).min(h * 0.85 / bh).min(1.0);
            let (tw, th) = render::text_size(SAMPLE_TEXT, size);
            Some(Shape::Text {
                at: Pt::new(center.x - tw / 2.0, center.y - th / 2.0),
                text: SAMPLE_TEXT.into(),
                size,
                boxed: style.boxed,
            })
        }
        Tool::Bubble => {
            let tail_room = if style.bubble == BubbleKind::Tail {
                0.7
            } else {
                0.9
            };
            let mut size = m.text;
            let b = bubble_box(Pt::new(0.0, 0.0), SAMPLE_TEXT, size, &render::text_size);
            size *= (w * 0.9 / b.width())
                .min(h * tail_room / b.height())
                .min(1.0);
            let b = bubble_box(Pt::new(0.0, 0.0), SAMPLE_TEXT, size, &render::text_size);
            // `at` : coin du texte ; la boîte déborde de la marge intérieure.
            let cy = if style.bubble == BubbleKind::Tail {
                h * 0.38
            } else {
                h / 2.0
            };
            let at = Pt::new(
                center.x - b.width() / 2.0 - b.min.x,
                cy - b.height() / 2.0 - b.min.y,
            );
            Some(Shape::Bubble {
                at,
                text: SAMPLE_TEXT.into(),
                size,
                tail: Pt::new(center.x - b.width() * 0.3, h * 0.95),
            })
        }
        Tool::Step => {
            let linked = matches!(
                style.step_tail,
                StepTail::Pointer | StepTail::Line | StepTail::Arrow
            );
            let (at, tail) = match style.step_tail {
                _ if linked => (place(34.0, 24.0), Some(place(78.0, 50.0))),
                StepTail::Pin => (place(48.0, 22.0), None),
                _ => (place(48.0, 30.0), None),
            };
            Some(Shape::Step {
                at,
                number: 1,
                tail,
            })
        }
        Tool::Magnifier => {
            let zoom = style.zoom.max(1.0);
            let lens = 22.0 * d;
            Some(Shape::Magnifier {
                source: place(22.0, 16.0),
                radius: lens / zoom,
                at: place(66.0, 34.0),
                zoom,
            })
        }
        Tool::Stamp => stamp_shape(&style.stamp, style, m, center, w, h, None),
        Tool::Polygon => Some(Shape::Polygon {
            points: vec![
                place(12.0, 46.0),
                place(34.0, 14.0),
                place(58.0, 44.0),
                place(84.0, 16.0),
            ],
            closed: style.poly_closed,
            start: style.poly_start,
            end: style.poly_end,
        }),
        Tool::Bracket => {
            if matches!(style.bracket, BracketSide::Left | BracketSide::Right) {
                drag(place(38.0, 6.0), place(58.0, 54.0))
            } else {
                drag(place(12.0, 20.0), place(84.0, 40.0))
            }
        }
    };
    shape.into_iter().collect()
}

/// Tampon `stamp` centré sur `at`, tenant dans `w` × `h` pixels. `image` : côtés
/// d'un tampon personnel (sinon il n'est pas dessiné).
fn stamp_shape(
    stamp: &str,
    style: &Style,
    m: Metrics,
    at: Pt,
    w: f32,
    h: f32,
    image: Option<(f32, f32)>,
) -> Option<Shape> {
    if let Some(text) = stamp.strip_prefix(LABEL_PREFIX) {
        let size = style.label_size(m);
        let b = label_box(Pt::new(0.0, 0.0), text, size, &render::text_size);
        let size = size * (w * 0.94 / b.width()).min(h * 0.8 / b.height()).min(1.0);
        return Some(Shape::Label {
            at,
            text: text.into(),
            size,
        });
    }
    if let Some(file) = stamp.strip_prefix(FILE_PREFIX) {
        let (iw, ih) = image?;
        let k = (w * 0.86 / iw).min(h * 0.86 / ih);
        let (iw, ih) = (iw * k, ih * k);
        return Some(Shape::Image {
            a: at.offset(-iw / 2.0, -ih / 2.0),
            b: at.offset(iw / 2.0, ih / 2.0),
            file: file.into(),
            ratio: iw / ih.max(1.0),
        });
    }
    // Tampons plus larges que hauts (tampons encreurs, étiquettes) : tenir en largeur.
    let aspect = stamp_box(Pt::new(0.0, 0.0), stamp, 100.0, &render::text_size).width() / 100.0;
    Some(Shape::Stamp {
        at,
        icon: stamp.into(),
        size: m.stamp.min(h * 0.8).min(w * 0.92 / aspect),
    })
}

/// Vignette d'un outil à ce style : `w` × `h` pixels, la forme type au centre.
/// `assets` : dossier des tampons personnels (aperçu d'un tampon image).
pub fn render_tool(tool: Tool, style: &Style, w: u32, h: u32, assets: Option<&Path>) -> RgbaImage {
    let (wf, hf) = (w as f32, h as f32);
    let d = (wf / SCENE.0).min(hf / SCENE.1);
    let (ox, oy) = ((wf - SCENE.0 * d) / 2.0, (hf - SCENE.1 * d) / 2.0);
    let place = move |x: f32, y: f32| Pt::new(ox + x * d, oy + y * d);
    let m = style.metrics(d);
    let base = if needs_content(tool) {
        content(w, h, &place, d, paper(style.color))
    } else {
        RgbaImage::new(w, h)
    };
    let mut shapes = tool_shapes(tool, style, m, &place, d, wf, hf);
    // Tampon personnel : ses côtés, lus dans le dossier des tampons.
    if tool == Tool::Stamp
        && let Some(file) = style.stamp.strip_prefix(FILE_PREFIX)
        && let Some(dir) = assets
        && let Ok((iw, ih)) = image::image_dimensions(dir.join(file))
    {
        let c = Pt::new(wf / 2.0, hf / 2.0);
        let dims = Some((iw as f32, ih as f32));
        shapes = stamp_shape(&style.stamp, style, m, c, wf, hf, dims)
            .into_iter()
            .collect();
    }
    let anns: Vec<Annotation> = shapes
        .into_iter()
        .map(|s| {
            let mut ann = style.annotate(s, m);
            if let Shape::Text { .. } = ann.shape {
                ann.text_style = style.text_style;
            }
            ann
        })
        .collect();
    render::render_with(&base, &anns, assets)
}

/// Vignette d'un tampon seul (panneau des tampons) : fond transparent, le tampon au
/// centre, à la couleur et aux options du style.
pub fn render_stamp(stamp: &str, style: &Style, w: u32, h: u32) -> RgbaImage {
    let (wf, hf) = (w as f32, h as f32);
    let m = Metrics {
        stroke: Size::M.stroke(),
        text: hf * 0.5,
        stamp: wf.min(hf) * 0.72,
    };
    let unit = Style {
        stamp_scale: 1.0,
        ..style.clone()
    };
    let anns: Vec<Annotation> =
        stamp_shape(stamp, &unit, m, Pt::new(wf / 2.0, hf / 2.0), wf, hf, None)
            .into_iter()
            .map(|s| style.annotate(s, m))
            .collect();
    render::render(&RgbaImage::new(w, h), &anns)
}

/// Miniature d'une image (tampon personnel) : `max` pixels au plus de côté, mêmes
/// proportions. `None` si le fichier ne se lit pas.
pub fn thumbnail(path: &Path, max: u32) -> Option<RgbaImage> {
    let img = image::open(path).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    if w <= max && h <= max {
        return Some(img);
    }
    let k = max as f32 / w.max(h) as f32;
    let (tw, th) = (
        ((w as f32 * k).round() as u32).max(1),
        ((h as f32 * k).round() as u32).max(1),
    );
    Some(image::imageops::resize(
        &img,
        tw,
        th,
        image::imageops::FilterType::Triangle,
    ))
}

fn texture(ctx: &egui::Context, name: String, img: &RgbaImage) -> TextureHandle {
    let color = ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    );
    ctx.load_texture(name, color, TextureOptions::LINEAR)
}

/// Miniature gardée : date du fichier, texture et taille affichée.
type Thumb = (Option<SystemTime>, Option<(TextureHandle, Vec2)>);

/// Textures des vignettes, par clé ([`key`]), et miniatures des tampons personnels.
#[derive(Default)]
pub struct Previews {
    textures: HashMap<u64, TextureHandle>,
    thumbs: HashMap<PathBuf, Thumb>,
}

impl Previews {
    /// Texture de la clé `key`, dessinée par `make(largeur, hauteur)` si elle manque.
    fn get(
        &mut self,
        ctx: &egui::Context,
        key: u64,
        size: (u32, u32),
        make: impl FnOnce(u32, u32) -> RgbaImage,
    ) -> TextureId {
        if let Some(t) = self.textures.get(&key) {
            return t.id();
        }
        if self.textures.len() >= MAX_TEXTURES {
            self.textures.clear();
        }
        let img = make(size.0, size.1);
        let tex = texture(ctx, format!("apercu-{key:x}"), &img);
        let id = tex.id();
        self.textures.insert(key, tex);
        id
    }

    /// Vignette d'un outil à ce style.
    pub fn tool(
        &mut self,
        ctx: &egui::Context,
        tool: Tool,
        style: &Style,
        size: Vec2,
        assets: Option<&Path>,
    ) -> TextureId {
        let ppp = ctx.pixels_per_point();
        let k = key(Sample::Tool(tool), style_hash(style), size, ppp);
        self.get(ctx, k, pixels(size, ppp), |w, h| {
            render_tool(tool, style, w, h, assets)
        })
    }

    /// Vignette du tampon `stamp`. `base` : [`stamp_base`] du style courant, calculé
    /// une fois pour tout le panneau.
    pub fn stamp(
        &mut self,
        ctx: &egui::Context,
        stamp: &str,
        style: &Style,
        base: u64,
        size: Vec2,
    ) -> TextureId {
        let ppp = ctx.pixels_per_point();
        let mut h = DefaultHasher::new();
        stamp.hash(&mut h);
        base.hash(&mut h);
        let k = key(Sample::Stamp, h.finish(), size, ppp);
        self.get(ctx, k, pixels(size, ppp), |w, h| {
            render_stamp(stamp, style, w, h)
        })
    }

    /// Miniature d'un tampon personnel et sa taille en points (dans `size`), relue
    /// quand le fichier change.
    pub fn thumb(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        size: Vec2,
    ) -> Option<(TextureId, Vec2)> {
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if let Some((when, tex)) = self.thumbs.get(path)
            && *when == modified
        {
            return tex.as_ref().map(|(t, s)| (t.id(), *s));
        }
        let ppp = ctx.pixels_per_point();
        let max = (size.x.max(size.y) * ppp).round() as u32;
        let tex = thumbnail(path, max).map(|img| {
            let k = (size.x / img.width() as f32).min(size.y / img.height() as f32);
            let shown = Vec2::new(img.width() as f32, img.height() as f32) * k;
            let name = format!("miniature-{}", path.display());
            (texture(ctx, name, &img), shown)
        });
        let out = tex.as_ref().map(|(t, s)| (t.id(), *s));
        self.thumbs.insert(path.to_owned(), (modified, tex));
        out
    }
}

/// Empreinte du style pour les vignettes de tampons : sans le tampon choisi ni la
/// taille (la vignette a sa propre taille), pour ne pas tout redessiner quand on
/// choisit un autre tampon.
pub fn stamp_base(style: &Style) -> u64 {
    style_hash(&Style {
        stamp: String::new(),
        size: Size::M,
        stamp_scale: 1.0,
        ..style.clone()
    })
}

/// Vignette cliquable : fond `bg`, image `tex` (étirée sur toute la vignette, ou de
/// la taille `fit` au centre), cadre vert quand elle est choisie.
pub fn tile(
    ui: &mut Ui,
    tex: Option<TextureId>,
    fit: Option<Vec2>,
    size: Vec2,
    selected: bool,
    bg: Color32,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = CornerRadius::same(5);
        painter.rect_filled(rect, radius, bg);
        if let Some(tex) = tex {
            let target = fit.map_or(rect, |s| Rect::from_center_size(rect.center(), s));
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            painter.image(tex, target, uv, Color32::WHITE);
        }
        let (width, color) = if selected {
            (2.0, theme::ACCENT_2)
        } else if response.hovered() {
            (1.0, theme::TEXT_DIM)
        } else {
            (1.0, theme::BORDER)
        };
        painter.rect_stroke(rect, radius, Stroke::new(width, color), StrokeKind::Inside);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArrowStyle, PALETTE};

    #[test]
    fn cache_keys_follow_style_size_and_scale() {
        let style = Style::default();
        let h = style_hash(&style);
        let size = Vec2::new(60.0, 40.0);
        let k = key(Sample::Tool(Tool::Arrow), h, size, 2.0);
        assert_eq!(
            k,
            key(
                Sample::Tool(Tool::Arrow),
                style_hash(&style.clone()),
                size,
                2.0
            )
        );
        let red_big = Style {
            arrow: ArrowStyle::Big,
            ..style.clone()
        };
        let blue = Style {
            color: PALETTE[4].0,
            ..style.clone()
        };
        let others = [
            key(Sample::Tool(Tool::Arrow), style_hash(&red_big), size, 2.0),
            key(Sample::Tool(Tool::Arrow), style_hash(&blue), size, 2.0),
            key(Sample::Tool(Tool::Rect), h, size, 2.0),
            key(Sample::Stamp, h, size, 2.0),
            key(Sample::Tool(Tool::Arrow), h, Vec2::new(61.0, 40.0), 2.0),
            key(Sample::Tool(Tool::Arrow), h, size, 1.0),
        ];
        for other in others {
            assert_ne!(k, other);
        }
        // Le tampon choisi et la taille ne changent pas les vignettes du panneau.
        let other_stamp = Style {
            stamp: "texte:VU".into(),
            size: Size::L,
            stamp_scale: 2.0,
            ..style.clone()
        };
        assert_eq!(stamp_base(&style), stamp_base(&other_stamp));
        assert_ne!(stamp_base(&style), stamp_base(&blue));
    }

    #[test]
    fn pixel_size_follows_the_screen_scale() {
        assert_eq!(pixels(Vec2::new(60.0, 40.0), 2.0), (120, 80));
        assert_eq!(pixels(Vec2::new(0.1, 0.1), 1.0), (1, 1));
    }

    /// Chaque outil dessine quelque chose dans sa vignette.
    #[test]
    fn every_tool_sample_draws_something() {
        let style = Style::default();
        for (tool, ..) in super::super::tools::TOOLS {
            if tool == Tool::Select {
                continue;
            }
            let img = render_tool(tool, &style, 120, 76, None);
            assert_eq!(img.dimensions(), (120, 76));
            let painted = img
                .pixels()
                .filter(|p| p[3] > 0 && p.0 != [PAPER_LIGHT.0, PAPER_LIGHT.1, PAPER_LIGHT.2, 255])
                .count();
            assert!(painted > 60, "{tool:?} : {painted} pixels");
        }
    }

    #[test]
    fn stamp_tiles_draw_every_kind() {
        let style = Style::default();
        for stamp in [
            super::super::tools::STAMPS[0].0.to_owned(),
            "texte:CONFIDENTIEL".to_owned(),
            "souris:gauche".to_owned(),
            format!("{}{}", render::EMOJI_PREFIX, crate::emoji::POSITIVE[0].0),
        ] {
            let img = render_stamp(&stamp, &style, 80, 80);
            let painted = img.pixels().filter(|p| p[3] > 0).count();
            assert!(painted > 200, "{stamp} : {painted} pixels");
        }
    }

    /// Toute la bibliothèque dessinée (classique, souris, étiquettes, tampons
    /// encreurs) laisse une trace, et les tampons larges restent larges.
    #[test]
    fn every_drawn_stamp_draws_something() {
        use render::stamp_art::{CLASSIC, MOUSE, PILLS, RUBBER};
        let style = Style::default();
        let stamps = CLASSIC
            .iter()
            .chain(&MOUSE)
            .map(|s| s.0.to_owned())
            .chain(
                PILLS
                    .iter()
                    .map(|t| format!("{}{t}", crate::model::PILL_PREFIX)),
            )
            .chain(
                RUBBER
                    .iter()
                    .map(|t| format!("{}{t}", crate::model::RUBBER_PREFIX)),
            );
        for stamp in stamps {
            let img = render_stamp(&stamp, &style, 120, 80);
            let painted = img.pixels().filter(|p| p[3] > 128).count();
            assert!(painted > 300, "{stamp} : {painted} pixels");
            let b = stamp_box(Pt::new(0.0, 0.0), &stamp, 40.0, &render::text_size);
            let wide = stamp.contains("pastille:")
                || stamp.contains("tampon:")
                || stamp.ends_with("new-pilule");
            assert_eq!(b.height(), 40.0);
            assert_eq!(b.width() > 60.0, wide, "{stamp} : {b:?}");
        }
    }

    #[test]
    fn user_stamp_thumbnails_keep_their_proportions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logo.png");
        RgbaImage::from_pixel(300, 150, image::Rgba([10, 200, 30, 255]))
            .save(&path)
            .unwrap();
        let thumb = thumbnail(&path, 64).unwrap();
        assert_eq!(thumb.dimensions(), (64, 32));
        assert_eq!(thumb.get_pixel(30, 15).0, [10, 200, 30, 255]);
        // Petite image : gardée telle quelle.
        assert_eq!(thumbnail(&path, 400).unwrap().dimensions(), (300, 150));
        assert!(thumbnail(&dir.path().join("absent.png"), 64).is_none());
    }

    /// Images des vignettes, pour les regarder : `SNAPPY_PREVIEW_DIR=… cargo test
    /// dump_previews -- --ignored`.
    #[test]
    #[ignore]
    fn dump_previews() {
        let Ok(dir) = std::env::var("SNAPPY_PREVIEW_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tools = super::super::tools::TOOLS;
        let mut sheet = RgbaImage::new(4 * 150, tools.len().div_ceil(4) as u32 * 100);
        let styles = [
            Style::default(),
            Style {
                color: PALETTE[4].0,
                size: Size::L,
                shadow: true,
                ..Style::default()
            },
        ];
        for (n, style) in styles.iter().enumerate() {
            for (i, (tool, ..)) in tools.iter().enumerate() {
                let tile = render_tool(*tool, style, 144, 90, None);
                let mut bg = RgbaImage::from_pixel(144, 90, {
                    let p = paper(style.color);
                    image::Rgba([p.0, p.1, p.2, 255])
                });
                image::imageops::overlay(&mut bg, &tile, 0, 0);
                let (x, y) = ((i % 4) as i64 * 150, (i / 4) as i64 * 100);
                image::imageops::overlay(&mut sheet, &bg, x, y);
            }
            sheet.save(dir.join(format!("outils-{n}.png"))).unwrap();
        }
        let stamps: Vec<String> = super::super::tools::STAMPS
            .iter()
            .map(|s| s.0.to_owned())
            .chain(["texte:CONFIDENTIEL".into(), "souris:gauche".into()])
            .chain(render::stamp_art::CLASSIC.iter().map(|s| s.0.to_owned()))
            .chain(render::stamp_art::MOUSE.iter().map(|s| s.0.to_owned()))
            .chain(
                render::stamp_art::PILLS
                    .iter()
                    .map(|t| format!("pastille:{t}")),
            )
            .chain(
                render::stamp_art::RUBBER
                    .iter()
                    .map(|t| format!("tampon:{t}")),
            )
            .chain(
                crate::emoji::POSITIVE
                    .iter()
                    .chain(&crate::emoji::NEGATIVE)
                    .map(|e| format!("{}{}", render::EMOJI_PREFIX, e.0)),
            )
            .collect();
        let mut sheet = RgbaImage::from_pixel(
            6 * 124,
            stamps.len().div_ceil(6) as u32 * 84,
            image::Rgba([0x1b, 0x1b, 0x1f, 255]),
        );
        for (i, stamp) in stamps.iter().enumerate() {
            let tile = render_stamp(stamp, &Style::default(), 120, 80);
            let (x, y) = ((i % 6) as i64 * 124, (i / 6) as i64 * 84);
            image::imageops::overlay(&mut sheet, &tile, x, y);
        }
        sheet.save(dir.join("tampons.png")).unwrap();
    }
}
