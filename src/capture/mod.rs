//! Capture d'un écran complet, prise dès l'appui sur le raccourci.
//!
//! L'image figée sert de fond à la sélection de zone : on capture l'écran sous le
//! curseur avant d'afficher quoi que ce soit, puis on découpe dedans.
//!
//! macOS : CoreGraphics direct (`CGDisplayCreateImage`, repli sur
//! `CGWindowListCreateImage`). Ces fonctions sont marquées obsolètes depuis le SDK 15
//! (« utilisez ScreenCaptureKit ») mais restent présentes et fonctionnelles à
//! l'exécution, macOS 26 compris. Elles sont synchrones, rapides (~35 ms pour un
//! écran Retina, conversion sRGB comprise) et ne tirent aucune dépendance de plus :
//! `objc2-core-graphics` est déjà dans l'arbre. ScreenCaptureKit
//! (`SCScreenshotManager`, macOS 14+) est asynchrone, deux fois plus lent (~90 ms,
//! dont 30 ms d'inventaire `SCShareableContent`) et sa caisse Rust compile un pont
//! Swift : à garder en réserve le jour où Apple retirera vraiment ces fonctions.
//!
//! Couleurs : l'image brute est dans l'espace de l'écran (P3 « Color LCD » sur un
//! Mac récent), en BGRA prémultiplié avec des octets de bourrage en fin de ligne.
//! On la redessine dans un contexte bitmap sRGB RGBA 8 bits : ColorSync convertit
//! l'espace colorimétrique et l'ordre des octets en une seule passe, quel que soit
//! le format source (8, 10 ou 16 bits, flottant EDR).

use image::RgbaImage;

/// Rectangle en points, coordonnées globales CoreGraphics : origine en haut à
/// gauche de l'écran principal, y vers le bas. Un écran à gauche ou au-dessus du
/// principal a des coordonnées négatives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    /// Le point est dans le cadre (bords gauche et haut inclus, droit et bas
    /// exclus : deux écrans côte à côte ne revendiquent jamais le même point).
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))] // utilisé par la capture macOS
    pub fn is_empty(&self) -> bool {
        !(self.width > 0.0 && self.height > 0.0)
    }
}

/// Capture d'un écran, figée au moment de l'appui.
#[derive(Clone)]
pub struct Shot {
    /// Pixels natifs (Retina : 2 pixels par point), sRGB, opaques.
    pub image: RgbaImage,
    /// Cadre de l'écran capturé, en points globaux.
    pub frame: Frame,
    /// Pixels par point : `image.width() / frame.width`.
    pub scale: f64,
    /// Identifiant stable de l'écran tant que la configuration ne change pas
    /// (`CGDirectDisplayID` sous macOS, hachage du nom du périphérique sous Windows,
    /// `\\.\DISPLAY1`…). Sert à
    /// retrouver l'écran de la « zone précédente ».
    pub display_id: u32,
    /// Fenêtres visibles sur cet écran au moment de la capture, **de la plus en
    /// avant à la plus en arrière**, en pixels de `image` (peuvent déborder de
    /// l'image : à rogner par l'appelant). La surcouche les encadre au survol et
    /// capture celle sous la souris d'un simple clic.
    pub windows: Vec<WindowRect>,
    /// Curseur de la souris, s'il est sur cet écran.
    pub cursor: Option<CursorPatch>,
}

/// Fenêtre visible, en pixels de l'image de son écran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Titre de la fenêtre (peut être vide).
    pub title: String,
    /// Nom de l'application propriétaire (peut être vide).
    pub app: String,
}

/// Curseur de la souris : un morceau de l'écran **avec le curseur dessiné dessus**
/// (fond opaque compris), à poser en (`x`, `y`) pixels sur l'image de l'écran pour
/// inclure le curseur dans la capture. L'image de l'écran, elle, est sans curseur.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorPatch {
    pub image: RgbaImage,
    pub x: i32,
    pub y: i32,
}

impl Shot {
    /// Zone (`x`, `y`, `width`, `height` en pixels, déjà bornée à l'image) de
    /// l'écran, avec le curseur si `with_cursor` et s'il y est.
    pub fn region(&self, x: u32, y: u32, width: u32, height: u32, with_cursor: bool) -> RgbaImage {
        let mut out = image::imageops::crop_imm(&self.image, x, y, width, height).to_image();
        if with_cursor && let Some(cursor) = &self.cursor {
            let dx = i64::from(cursor.x) - i64::from(x);
            let dy = i64::from(cursor.y) - i64::from(y);
            image::imageops::replace(&mut out, &cursor.image, dx, dy);
        }
        out
    }
}

/// Tous les écrans, figés au même instant.
#[derive(Debug, Clone)]
pub struct Capture {
    pub shots: Vec<Shot>,
    /// Indice dans `shots` de l'écran sous le curseur.
    pub active: usize,
}

impl std::fmt::Debug for Shot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shot")
            .field(
                "image",
                &format_args!("{}x{}", self.image.width(), self.image.height()),
            )
            .field("frame", &self.frame)
            .field("scale", &self.scale)
            .field("display_id", &self.display_id)
            .field("windows", &self.windows.len())
            .field("cursor", &self.cursor.as_ref().map(|c| (c.x, c.y)))
            .finish()
    }
}

// `pixel_rect` et `crop` travaillent en points globaux : pour la sélection sur
// plusieurs écrans (à venir). La surcouche actuelle découpe en coordonnées locales.
#[allow(dead_code)]
impl Shot {
    /// Rectangle en points globaux (largeur ou hauteur négative acceptée : sélection
    /// tirée vers la gauche ou le haut) vers rectangle en pixels de l'image
    /// `(x, y, largeur, hauteur)`, élargi au pixel entier et rogné à l'image.
    /// `None` si rien ne reste.
    pub fn pixel_rect(
        &self,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Option<(u32, u32, u32, u32)> {
        let (x0, x1) = (x.min(x + width), x.max(x + width));
        let (y0, y1) = (y.min(y + height), y.max(y + height));
        let to_px =
            |v: f64, origin: f64, max: u32| ((v - origin) * self.scale).clamp(0.0, f64::from(max));
        let (w, h) = self.image.dimensions();
        let left = to_px(x0, self.frame.x, w).floor();
        let right = to_px(x1, self.frame.x, w).ceil();
        let top = to_px(y0, self.frame.y, h).floor();
        let bottom = to_px(y1, self.frame.y, h).ceil();
        if right <= left || bottom <= top {
            return None;
        }
        // Valeurs entières, bornées par la taille de l'image : conversions exactes.
        Some((
            left as u32,
            top as u32,
            (right - left) as u32,
            (bottom - top) as u32,
        ))
    }

    /// Découpe d'un rectangle en points globaux (voir [`Shot::pixel_rect`]).
    pub fn crop(&self, x: f64, y: f64, width: f64, height: f64) -> Option<RgbaImage> {
        let (px, py, pw, ph) = self.pixel_rect(x, y, width, height)?;
        Some(image::imageops::crop_imm(&self.image, px, py, pw, ph).to_image())
    }
}

/// Pixels par point. Cadre de largeur nulle (écran introuvable) : 1.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // propre à macOS
fn scale_factor(pixel_width: u32, frame_width: f64) -> f64 {
    if frame_width > 0.0 {
        f64::from(pixel_width) / frame_width
    } else {
        1.0
    }
}

/// Premier écran dont le cadre contient le point.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // propre à macOS
fn display_at(displays: &[(u32, Frame)], x: f64, y: f64) -> Option<u32> {
    displays
        .iter()
        .find(|(_, frame)| frame.contains(x, y))
        .map(|&(id, _)| id)
}

/// Rend les pixels opaques (alpha à 255) et dit s'il y avait quoi que ce soit à
/// voir. Un écran en veille ou une session verrouillée donnent une image
/// entièrement transparente (après ~1 s d'attente côté système).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // propre à macOS
fn make_opaque(rgba: &mut [u8]) -> bool {
    let mut visible = false;
    for px in rgba.as_chunks_mut::<4>().0 {
        visible |= px[3] != 0;
        px[3] = u8::MAX;
    }
    visible
}

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(not(any(target_os = "macos", windows)))]
#[path = "other.rs"]
mod platform;

#[allow(unused_imports)] // `displays` : pour la suite (choix d'écran)
pub use platform::{capture_all, cursor_position, displays, has_permission, request_permission};

// --- Lot « Capture » : tous les écrans, fenêtre au premier plan --------------

pub use platform::front_window;

/// Fenêtre au premier plan de l'application active, relevée au moment de la
/// capture (`Mode::Window`).
#[derive(Debug, Clone, PartialEq)]
pub struct FrontWindow {
    /// Cadre de la fenêtre dans l'espace du bureau (voir [`desktop_frame`]).
    pub bounds: Frame,
    pub title: String,
    pub app: String,
}

/// Plus grand côté de l'image de tous les écrans, en pixels : au-delà, l'éditeur ne
/// pourrait plus l'afficher (taille maximale des textures du GPU).
pub const MAX_COMPOSITE_SIDE: f64 = 16384.0;

/// Cadre d'un écran dans l'espace commun à tous les écrans (« espace du bureau ») :
/// les points globaux sous macOS ; les pixels physiques du bureau virtuel sous
/// Windows, où les cadres sont en points de chaque écran et ne s'alignent donc pas
/// entre écrans d'échelles différentes.
pub fn desktop_frame(shot: &Shot) -> Frame {
    if cfg!(windows) {
        Frame {
            x: (shot.frame.x * shot.scale).round(),
            y: (shot.frame.y * shot.scale).round(),
            width: f64::from(shot.image.width()),
            height: f64::from(shot.image.height()),
        }
    } else {
        shot.frame
    }
}

/// Place de chaque écran dans l'image de tous les écrans.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Coin haut gauche de l'ensemble, dans l'espace du bureau.
    pub origin: (f64, f64),
    /// Pixels de l'image par unité du bureau : celle de l'écran le plus fin (Retina),
    /// réduite si l'image dépasserait [`MAX_COMPOSITE_SIDE`].
    pub density: f64,
    /// Taille de l'image, en pixels.
    pub size: (u32, u32),
    /// Par écran (même ordre) : `(x, y, largeur, hauteur)` en pixels de l'image.
    pub places: Vec<(u32, u32, u32, u32)>,
}

/// Assemblage d'écrans donnés par leur cadre (espace du bureau) et la largeur de
/// leur image en pixels. `None` sans écran, ou si l'ensemble est vide.
pub fn layout(screens: &[(Frame, u32)]) -> Option<Layout> {
    let usable: Vec<&(Frame, u32)> = screens.iter().filter(|(f, _)| !f.is_empty()).collect();
    if usable.is_empty() || usable.len() != screens.len() {
        return None;
    }
    let min_x = usable
        .iter()
        .map(|(f, _)| f.x)
        .fold(f64::INFINITY, f64::min);
    let min_y = usable
        .iter()
        .map(|(f, _)| f.y)
        .fold(f64::INFINITY, f64::min);
    let max_x = usable
        .iter()
        .map(|(f, _)| f.x + f.width)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = usable
        .iter()
        .map(|(f, _)| f.y + f.height)
        .fold(f64::NEG_INFINITY, f64::max);
    let finest = usable
        .iter()
        .map(|(f, w)| f64::from(*w) / f.width)
        .fold(0.0, f64::max);
    let widest = (max_x - min_x).max(max_y - min_y);
    let density = finest.min(MAX_COMPOSITE_SIDE / widest);
    // Aussi faux pour NaN (écran de largeur nulle, déjà écarté plus haut).
    if density.is_nan() || density <= 0.0 {
        return None;
    }
    let size = (
        ((max_x - min_x) * density).round().max(1.0) as u32,
        ((max_y - min_y) * density).round().max(1.0) as u32,
    );
    let places = usable
        .iter()
        .map(|(f, _)| {
            let x = ((f.x - min_x) * density).round() as u32;
            let y = ((f.y - min_y) * density).round() as u32;
            let w = ((f.width * density).round() as u32).min(size.0 - x.min(size.0));
            let h = ((f.height * density).round() as u32).min(size.1 - y.min(size.1));
            (x, y, w, h)
        })
        .collect();
    Some(Layout {
        origin: (min_x, min_y),
        density,
        size,
        places,
    })
}

/// Rectangle de l'espace du bureau vers pixels d'une image qui couvre `area` à
/// `density` pixels par unité : `(x, y, largeur, hauteur)`, élargi au pixel entier et
/// rogné à l'image (`width` × `height`). `None` si rien ne reste.
pub fn desktop_to_pixels(
    rect: &Frame,
    area: &Frame,
    density: f64,
    width: u32,
    height: u32,
) -> Option<(u32, u32, u32, u32)> {
    let px = |v: f64, origin: f64, max: u32| ((v - origin) * density).clamp(0.0, f64::from(max));
    let left = px(rect.x, area.x, width).floor();
    let top = px(rect.y, area.y, height).floor();
    let right = px(rect.x + rect.width, area.x, width).ceil();
    let bottom = px(rect.y + rect.height, area.y, height).ceil();
    // Valeurs entières bornées par la taille de l'image : conversions exactes.
    (right > left && bottom > top).then_some((
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

/// Surface commune de deux cadres (0 s'ils ne se recouvrent pas).
fn overlap(a: &Frame, b: &Frame) -> f64 {
    let w = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
    let h = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
}

impl Capture {
    /// Tous les écrans en une image, chacun à sa place sur le bureau, à la
    /// définition de l'écran le plus fin (les autres sont agrandis). Les vides entre
    /// écrans de tailles différentes restent transparents.
    pub fn compose_all(&self, with_cursor: bool) -> Option<RgbaImage> {
        let frames: Vec<(Frame, u32)> = self
            .shots
            .iter()
            .map(|s| (desktop_frame(s), s.image.width()))
            .collect();
        let layout = layout(&frames)?;
        let mut out = RgbaImage::new(layout.size.0, layout.size.1);
        for (shot, &(x, y, w, h)) in self.shots.iter().zip(&layout.places) {
            let (iw, ih) = shot.image.dimensions();
            let mut img = shot.region(0, 0, iw, ih, with_cursor);
            if (w, h) != (iw, ih) && w > 0 && h > 0 {
                img = image::imageops::resize(&img, w, h, image::imageops::FilterType::CatmullRom);
            }
            image::imageops::replace(&mut out, &img, i64::from(x), i64::from(y));
        }
        Some(out)
    }

    /// Pixels du rectangle `rect` (espace du bureau), et l'écran qui en montre le
    /// plus. Sur un seul écran : à sa définition ; à cheval sur plusieurs : découpé
    /// dans l'image de tous les écrans. `None` s'il ne touche aucun écran.
    pub fn crop_desktop(&self, rect: &Frame, with_cursor: bool) -> Option<(RgbaImage, usize)> {
        let touched: Vec<(usize, f64)> = self
            .shots
            .iter()
            .enumerate()
            .map(|(i, s)| (i, overlap(&desktop_frame(s), rect)))
            .filter(|&(_, a)| a > 0.0)
            .collect();
        let &(main, _) = touched.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
        if touched.len() == 1 {
            let shot = &self.shots[main];
            let area = desktop_frame(shot);
            let (iw, ih) = shot.image.dimensions();
            let density = f64::from(iw) / area.width;
            let (x, y, w, h) = desktop_to_pixels(rect, &area, density, iw, ih)?;
            return Some((shot.region(x, y, w, h, with_cursor), main));
        }
        let frames: Vec<(Frame, u32)> = self
            .shots
            .iter()
            .map(|s| (desktop_frame(s), s.image.width()))
            .collect();
        let layout = layout(&frames)?;
        let all = self.compose_all(with_cursor)?;
        let area = Frame {
            x: layout.origin.0,
            y: layout.origin.1,
            width: f64::from(layout.size.0) / layout.density,
            height: f64::from(layout.size.1) / layout.density,
        };
        let (x, y, w, h) =
            desktop_to_pixels(rect, &area, layout.density, all.width(), all.height())?;
        Some((image::imageops::crop_imm(&all, x, y, w, h).to_image(), main))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f64, y: f64, width: f64, height: f64) -> Frame {
        Frame {
            x,
            y,
            width,
            height,
        }
    }

    /// Écran Retina de 100 × 50 points à l'origine, image 200 × 100 dégradée.
    fn retina_shot(origin: (f64, f64)) -> Shot {
        let image = RgbaImage::from_fn(200, 100, |x, y| image::Rgba([x as u8, y as u8, 0, 255]));
        Shot {
            scale: scale_factor(image.width(), 100.0),
            image,
            frame: frame(origin.0, origin.1, 100.0, 50.0),
            display_id: 1,
            windows: Vec::new(),
            cursor: None,
        }
    }

    #[test]
    fn frame_contains_is_half_open() {
        let f = frame(0.0, 0.0, 1470.0, 956.0);
        assert!(f.contains(0.0, 0.0));
        assert!(f.contains(1469.9, 955.9));
        assert!(!f.contains(1470.0, 10.0));
        assert!(!f.contains(10.0, 956.0));
        assert!(!f.contains(-0.1, 10.0));
        assert!(frame(0.0, 0.0, 0.0, 10.0).is_empty());
        assert!(!f.is_empty());
    }

    #[test]
    fn display_at_picks_the_screen_under_the_point() {
        // Principal à l'origine, externe à gauche (x négatif) un peu plus haut.
        let displays = [
            (1, frame(0.0, 0.0, 1470.0, 956.0)),
            (7, frame(-2560.0, -300.0, 2560.0, 1440.0)),
        ];
        assert_eq!(display_at(&displays, 100.0, 100.0), Some(1));
        assert_eq!(display_at(&displays, -1.0, 0.0), Some(7));
        assert_eq!(display_at(&displays, -2560.0, -300.0), Some(7));
        // Pile sur la frontière : l'écran de droite.
        assert_eq!(display_at(&displays, 0.0, 500.0), Some(1));
        // Hors de tout écran : à l'appelant de prendre le principal.
        assert_eq!(display_at(&displays, 5000.0, 0.0), None);
        assert_eq!(display_at(&[], 0.0, 0.0), None);
    }

    #[test]
    fn scale_factor_from_pixels_and_points() {
        assert_eq!(scale_factor(2940, 1470.0), 2.0);
        assert_eq!(scale_factor(1920, 1920.0), 1.0);
        assert!((scale_factor(3456, 1728.0) - 2.0).abs() < 1e-12);
        assert_eq!(scale_factor(2940, 0.0), 1.0);
    }

    #[test]
    fn pixel_rect_converts_points_to_pixels() {
        let shot = retina_shot((0.0, 0.0));
        assert_eq!(
            shot.pixel_rect(10.0, 5.0, 20.0, 10.0),
            Some((20, 10, 40, 20))
        );
        // Sélection tirée vers le haut à gauche : même résultat.
        assert_eq!(
            shot.pixel_rect(30.0, 15.0, -20.0, -10.0),
            Some((20, 10, 40, 20))
        );
        // Demi-points : élargi au pixel entier.
        assert_eq!(shot.pixel_rect(10.25, 5.0, 0.5, 1.0), Some((20, 10, 2, 2)));
    }

    #[test]
    fn pixel_rect_clamps_to_the_screen() {
        let shot = retina_shot((-100.0, 0.0));
        assert_eq!(
            shot.pixel_rect(-120.0, -10.0, 40.0, 20.0),
            Some((0, 0, 40, 20))
        );
        assert_eq!(
            shot.pixel_rect(-10.0, 40.0, 50.0, 50.0),
            Some((180, 80, 20, 20))
        );
        assert_eq!(shot.pixel_rect(10.0, 10.0, 5.0, 5.0), None);
        assert_eq!(shot.pixel_rect(-50.0, 10.0, 0.0, 5.0), None);
    }

    #[test]
    fn crop_takes_the_right_pixels() {
        let shot = retina_shot((0.0, 0.0));
        let crop = shot.crop(10.0, 5.0, 2.0, 1.0).unwrap();
        assert_eq!(crop.dimensions(), (4, 2));
        assert_eq!(crop.get_pixel(0, 0).0, [20, 10, 0, 255]);
        assert_eq!(crop.get_pixel(3, 1).0, [23, 11, 0, 255]);
    }

    #[test]
    fn region_includes_the_cursor_only_on_request() {
        let mut shot = retina_shot((0.0, 0.0));
        let red = image::Rgba([255, 0, 0, 255]);
        shot.cursor = Some(CursorPatch {
            image: RgbaImage::from_pixel(4, 4, red),
            x: 10,
            y: 10,
        });
        let without = shot.region(8, 8, 8, 8, false);
        assert_ne!(*without.get_pixel(2, 2), red);
        let with = shot.region(8, 8, 8, 8, true);
        assert_eq!(*with.get_pixel(2, 2), red);
        assert_eq!(*with.get_pixel(5, 5), red);
        assert_ne!(*with.get_pixel(6, 6), red);
        // Curseur à cheval sur le bord de la zone : seule la partie visible est posée.
        let edge = shot.region(12, 12, 8, 8, true);
        assert_eq!(*edge.get_pixel(0, 0), red);
        assert_eq!(edge.dimensions(), (8, 8));
    }

    #[test]
    fn make_opaque_detects_blank_captures() {
        let mut blank = vec![0u8; 16];
        assert!(!make_opaque(&mut blank));
        assert!(blank.chunks(4).all(|p| p[3] == 255));
        let mut screen = vec![10, 20, 30, 255, 0, 0, 0, 0];
        assert!(make_opaque(&mut screen));
        assert_eq!(screen, [10, 20, 30, 255, 0, 0, 0, 255]);
    }

    // --- Lot « Capture » ---

    /// Écran uni de `w` × `h` points à l'échelle `scale`, en (`x`, `y`) points.
    fn plain_shot(x: f64, y: f64, w: f64, h: f64, scale: f64, color: [u8; 4]) -> Shot {
        let image =
            RgbaImage::from_pixel((w * scale) as u32, (h * scale) as u32, image::Rgba(color));
        Shot {
            image,
            // Sous Windows, les cadres sont en points de chaque écran.
            frame: if cfg!(windows) {
                frame(x / scale, y / scale, w, h)
            } else {
                frame(x, y, w, h)
            },
            scale,
            display_id: 1,
            windows: Vec::new(),
            cursor: None,
        }
    }

    #[test]
    fn layout_places_screens_at_the_finest_density() {
        // Retina 1470 × 956 à l'origine ; écran externe 1920 × 1080 à droite, plus haut.
        let l = layout(&[
            (frame(0.0, 0.0, 1470.0, 956.0), 2940),
            (frame(1470.0, -100.0, 1920.0, 1080.0), 1920),
        ])
        .unwrap();
        assert_eq!(l.density, 2.0);
        assert_eq!(l.origin, (0.0, -100.0));
        assert_eq!(l.size, (6780, 2160));
        assert_eq!(l.places, vec![(0, 200, 2940, 1912), (2940, 0, 3840, 2160)]);
        assert_eq!(layout(&[]), None);
        assert_eq!(layout(&[(frame(0.0, 0.0, 0.0, 10.0), 0)]), None);
    }

    #[test]
    fn layout_is_capped_for_huge_desktops() {
        let l = layout(&[
            (frame(0.0, 0.0, 15000.0, 1000.0), 30000),
            (frame(15000.0, 0.0, 15000.0, 1000.0), 15000),
        ])
        .unwrap();
        assert!(l.size.0 as f64 <= MAX_COMPOSITE_SIDE, "{l:?}");
        assert!(l.density < 1.0);
    }

    #[test]
    fn compose_all_fills_each_place_and_leaves_gaps_transparent() {
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        // Retina 10 × 10 points en (0, 0) ; écran 1x 10 × 5 points à droite.
        let capture = Capture {
            shots: vec![
                plain_shot(0.0, 0.0, 10.0, 10.0, 2.0, red),
                plain_shot(10.0, 0.0, 10.0, 5.0, 1.0, blue),
            ],
            active: 0,
        };
        if cfg!(windows) {
            // Sous Windows, l'espace commun est en pixels physiques : pas d'agrandissement.
            return;
        }
        let all = capture.compose_all(false).unwrap();
        assert_eq!(all.dimensions(), (40, 20));
        assert_eq!(all.get_pixel(0, 0).0, red);
        assert_eq!(all.get_pixel(19, 19).0, red);
        // Écran 1x agrandi ×2.
        assert_eq!(all.get_pixel(20, 0).0, blue);
        assert_eq!(all.get_pixel(39, 9).0, blue);
        // Sous l'écran plus petit : rien.
        assert_eq!(all.get_pixel(30, 15).0, [0, 0, 0, 0]);
    }

    #[test]
    fn desktop_rectangles_become_pixels() {
        let area = frame(-100.0, 0.0, 100.0, 50.0);
        assert_eq!(
            desktop_to_pixels(&frame(-90.0, 5.0, 20.0, 10.0), &area, 2.0, 200, 100),
            Some((20, 10, 40, 20))
        );
        // Débordement : rogné ; demi-points : élargi.
        assert_eq!(
            desktop_to_pixels(&frame(-110.0, 40.25, 20.0, 20.0), &area, 2.0, 200, 100),
            Some((0, 80, 20, 20))
        );
        assert_eq!(
            desktop_to_pixels(&frame(10.0, 0.0, 5.0, 5.0), &area, 2.0, 200, 100),
            None
        );
    }

    #[test]
    fn crop_desktop_uses_one_screen_or_the_assembly() {
        if cfg!(windows) {
            return;
        }
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let capture = Capture {
            shots: vec![
                plain_shot(0.0, 0.0, 10.0, 10.0, 2.0, red),
                plain_shot(10.0, 0.0, 10.0, 10.0, 1.0, blue),
            ],
            active: 0,
        };
        let (img, screen) = capture
            .crop_desktop(&frame(12.0, 1.0, 4.0, 3.0), false)
            .unwrap();
        assert_eq!(
            (img.dimensions(), screen),
            ((4, 3), 1),
            "à la définition de l'écran"
        );
        // À cheval, surtout sur le premier : assemblé à la définition la plus fine.
        let (img, screen) = capture
            .crop_desktop(&frame(4.0, 0.0, 8.0, 2.0), false)
            .unwrap();
        assert_eq!((img.dimensions(), screen), ((16, 4), 0));
        assert_eq!(img.get_pixel(0, 0).0, red);
        assert_eq!(img.get_pixel(15, 0).0, blue);
        assert!(
            capture
                .crop_desktop(&frame(50.0, 50.0, 5.0, 5.0), false)
                .is_none()
        );
    }

    #[test]
    fn shot_debug_does_not_dump_pixels() {
        let text = format!("{:?}", retina_shot((0.0, 0.0)));
        assert!(text.contains("200x100"), "{text}");
        assert!(text.len() < 200, "{text}");
    }

    /// Conversion des pixels sur des images synthétiques : pas besoin d'autorisation.
    #[cfg(target_os = "macos")]
    mod pixels {
        use objc2_core_foundation::{CFRetained, CFString};
        use objc2_core_graphics::{
            CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImage,
            CGImageAlphaInfo, CGImageByteOrderInfo, kCGColorSpaceDisplayP3, kCGColorSpaceSRGB,
        };

        use super::super::platform::to_srgb_rgba;

        /// Image CoreGraphics BGRA prémultiplié petit-boutiste, comme une capture d'écran.
        fn bgra_image(
            space: &CFString,
            width: usize,
            height: usize,
            bgra: &[u8],
        ) -> CFRetained<CGImage> {
            assert_eq!(bgra.len(), width * height * 4);
            let mut data = bgra.to_vec();
            let space = CGColorSpace::with_name(Some(space)).unwrap();
            let info =
                CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
            // SAFETY : `data` a la taille annoncée et survit au contexte ; l'image
            // créée garde sa propre copie des pixels.
            let ctx = unsafe {
                CGBitmapContextCreate(
                    data.as_mut_ptr().cast(),
                    width,
                    height,
                    8,
                    width * 4,
                    Some(&space),
                    info,
                )
            }
            .unwrap();
            CGBitmapContextCreateImage(Some(&ctx)).unwrap()
        }

        #[test]
        fn bgra_becomes_rgba() {
            // SAFETY : constante fournie par CoreGraphics.
            let srgb = unsafe { kCGColorSpaceSRGB };
            let source = bgra_image(srgb, 2, 1, &[10, 20, 30, 255, 200, 100, 50, 255]);
            let image = to_srgb_rgba(&source).unwrap();
            assert_eq!(image.dimensions(), (2, 1));
            assert_eq!(image.get_pixel(0, 0).0, [30, 20, 10, 255]);
            assert_eq!(image.get_pixel(1, 0).0, [50, 100, 200, 255]);
        }

        #[test]
        fn rows_keep_their_order() {
            // SAFETY : constante fournie par CoreGraphics.
            let srgb = unsafe { kCGColorSpaceSRGB };
            // Ligne du haut rouge, ligne du bas bleue (BGRA).
            let source = bgra_image(srgb, 1, 2, &[0, 0, 255, 255, 255, 0, 0, 255]);
            let image = to_srgb_rgba(&source).unwrap();
            assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 255]);
            assert_eq!(image.get_pixel(0, 1).0, [0, 0, 255, 255]);
        }

        #[test]
        fn display_p3_is_converted_to_srgb() {
            // SAFETY : constantes fournies par CoreGraphics.
            let p3 = unsafe { kCGColorSpaceDisplayP3 };
            // Rouge sRGB pur exprimé en Display P3 ≈ (234, 51, 35) ; gris neutre ;
            // rouge P3 pur, hors gamut sRGB.
            let source = bgra_image(
                p3,
                3,
                1,
                &[35, 51, 234, 255, 128, 128, 128, 255, 0, 0, 255, 255],
            );
            let image = to_srgb_rgba(&source).unwrap();
            let [r, g, b, a] = image.get_pixel(0, 0).0;
            assert!(
                r >= 250 && g <= 10 && b <= 10 && a == 255,
                "rouge sRGB : {:?}",
                (r, g, b)
            );
            let [r, g, b, _] = image.get_pixel(1, 0).0;
            assert!(
                [r, g, b].iter().all(|&c| c.abs_diff(128) <= 2),
                "gris : {:?}",
                (r, g, b)
            );
            // Hors gamut : écrêté au rouge sRGB.
            let [r, g, b, _] = image.get_pixel(2, 0).0;
            assert!(r == 255 && g <= 10 && b <= 10, "rouge P3 : {:?}", (r, g, b));
        }
    }

    /// Vraie capture : demande l'autorisation « Enregistrement de l'écran » et un
    /// écran allumé, session déverrouillée. `cargo test -- --ignored`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn captures_the_screen_under_the_cursor() {
        assert!(
            has_permission(),
            "autorisation « Enregistrement de l'écran » absente"
        );
        let (x, y) = cursor_position().expect("position du curseur");
        let start = std::time::Instant::now();
        let capture = capture_all().expect("capture");
        let shot = &capture.shots[capture.active];
        eprintln!("{shot:?} en {:?}", start.elapsed());
        assert!(shot.frame.contains(x, y) || displays().iter().all(|(_, f)| !f.contains(x, y)));
        assert!(shot.scale >= 1.0);
        let expected_height = (shot.frame.height * shot.scale).round() as u32;
        assert_eq!(shot.image.height(), expected_height);
        assert!(shot.image.pixels().all(|p| p[3] == 255));
        let first = *shot.image.get_pixel(0, 0);
        assert!(shot.image.pixels().any(|p| *p != first), "image uniforme");
    }
}
