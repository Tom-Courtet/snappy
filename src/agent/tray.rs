//! Icône de Snappy dans la barre des menus (un viseur, en image « modèle » : macOS
//! l'adapte au thème clair ou sombre) et son menu.
//!
//! Clic gauche : la fenêtre « Espace de travail » (un second clic la referme). Clic
//! droit : le menu, dans l'ordre de celui de Screenpresso, avec une petite icône
//! carrée de couleur en tête de chaque groupe.
//!
//! Vit sur le thread principal.

use anyhow::{Context, Result};
use tray_icon::menu::{IconMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::ipc::Mode;
use crate::workspace::Anchor;

/// Choix fait dans le menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuChoice {
    Capture(Mode),
    /// Capture de zone après le délai de la config.
    CaptureDelayed,
    OpenCaptures,
    OpenSaved,
    /// Fenêtre des réglages (ouverte, ou passée au premier plan).
    OpenSettings,
    Permission,
    Quit,
    // Lot « Icône Snappy + Espace de travail ».
    Help,
    About,
}

const IDS: [(&str, MenuChoice); 20] = [
    ("region", MenuChoice::Capture(Mode::Region)),
    ("screen", MenuChoice::Capture(Mode::Screen)),
    ("previous", MenuChoice::Capture(Mode::Previous)),
    ("delayed", MenuChoice::CaptureDelayed),
    ("captures", MenuChoice::OpenCaptures),
    ("saved", MenuChoice::OpenSaved),
    ("settings", MenuChoice::OpenSettings),
    ("permission", MenuChoice::Permission),
    ("quit", MenuChoice::Quit),
    // Lot « Icône Snappy + Espace de travail ».
    ("all-screens", MenuChoice::Capture(Mode::AllScreens)),
    ("window", MenuChoice::Capture(Mode::Window)),
    ("scrolling", MenuChoice::Capture(Mode::Scrolling)),
    ("ocr", MenuChoice::Capture(Mode::Ocr)),
    ("qr", MenuChoice::Capture(Mode::Qr)),
    ("color-picker", MenuChoice::Capture(Mode::ColorPicker)),
    ("video", MenuChoice::Capture(Mode::Video)),
    ("video-region", MenuChoice::Capture(Mode::VideoRegion)),
    ("workspace", MenuChoice::Capture(Mode::Workspace)),
    ("help", MenuChoice::Help),
    ("about", MenuChoice::About),
];

fn id_of(choice: MenuChoice) -> &'static str {
    IDS.iter()
        .find(|(_, c)| *c == choice)
        .map(|(id, _)| *id)
        .expect("identifiant de menu")
}

fn parse_menu_id(id: &str) -> Option<MenuChoice> {
    IDS.iter().find(|(i, _)| *i == id).map(|(_, c)| *c)
}

/// macOS : viseur noir en image « modèle », que le système teinte selon le thème.
#[cfg(target_os = "macos")]
const ICON_SIZE: u32 = 44;
#[cfg(target_os = "macos")]
const ICON: &[u8] = include_bytes!("../../assets/tray_44.rgba");
/// Windows : l'icône couleur de l'app, lisible sur une barre des tâches claire ou
/// sombre (Windows ne teinte pas les icônes de la zone de notification).
#[cfg(not(target_os = "macos"))]
const ICON_SIZE: u32 = 64;
#[cfg(not(target_os = "macos"))]
const ICON: &[u8] = include_bytes!("../../assets/icon_64.rgba");

pub struct Tray {
    icon: TrayIcon,
}

/// Contenu du menu, qui change avec la config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuState {
    /// Libellé du raccourci de chaque capture.
    pub labels: Vec<(Mode, String)>,
    /// Secondes de la capture différée.
    pub delay: u32,
    /// Ajoute l'entrée qui mène aux réglages de confidentialité.
    pub needs_permission: bool,
}

impl Tray {
    /// `on_left_click` reçoit le rectangle de l'icône (points logiques), pour ouvrir
    /// l'espace de travail juste à côté.
    pub fn new(
        state: &MenuState,
        on_choice: impl Fn(MenuChoice) + Send + Sync + 'static,
        on_left_click: impl Fn(Option<Anchor>) + Send + Sync + 'static,
    ) -> Result<Self> {
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(choice) = parse_menu_id(&event.id().0) {
                on_choice(choice);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                on_left_click(anchor_of(rect));
            }
        }));
        let icon =
            Icon::from_rgba(ICON.to_vec(), ICON_SIZE, ICON_SIZE).context("icône invalide")?;
        let builder = TrayIconBuilder::new();
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(icon);
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(icon);
        let icon = builder
            .with_tooltip("Snappy")
            .with_menu(Box::new(menu(state)?))
            // Clic gauche : l'espace de travail ; le menu vient au clic droit.
            .with_menu_on_left_click(false)
            .build()
            .context("icône de la barre des menus")?;
        Ok(Self { icon })
    }

    /// Rectangle de l'icône, s'il est déjà connu.
    pub fn anchor(&self) -> Option<Anchor> {
        anchor_of(self.icon.rect()?)
    }

    /// Reconstruit le menu (raccourcis ou délai changés dans les réglages).
    pub fn set_menu(&self, state: &MenuState) -> Result<()> {
        self.icon.set_menu(Some(Box::new(menu(state)?)));
        Ok(())
    }
}

/// Rectangle de l'icône en points logiques. tray-icon le donne en pixels physiques,
/// à l'échelle de l'écran de l'icône.
fn anchor_of(rect: tray_icon::Rect) -> Option<Anchor> {
    let scale = scale_at(rect.position.x, rect.position.y) as f32;
    let anchor = Anchor {
        x: rect.position.x as f32 / scale,
        y: rect.position.y as f32 / scale,
        w: rect.size.width as f32 / scale,
        h: rect.size.height as f32 / scale,
    };
    (anchor.w > 0.0 && anchor.h > 0.0).then_some(anchor)
}

/// Échelle de l'écran où se trouve le curseur (il est sur l'icône qu'on vient de
/// cliquer).
#[cfg(target_os = "macos")]
fn scale_at(_x: f64, _y: f64) -> f64 {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSScreen};
    let Some(mtm) = MainThreadMarker::new() else {
        return 1.0;
    };
    let p = NSEvent::mouseLocation();
    let screens = NSScreen::screens(mtm);
    let inside = screens.iter().find(|s| {
        let f = s.frame();
        p.x >= f.origin.x
            && p.x <= f.origin.x + f.size.width
            && p.y >= f.origin.y
            && p.y <= f.origin.y + f.size.height
    });
    inside
        .or_else(|| screens.firstObject())
        .map(|s| s.backingScaleFactor())
        .unwrap_or(1.0)
}

#[cfg(windows)]
fn scale_at(x: f64, y: f64) -> f64 {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    let point = POINT {
        x: x as i32,
        y: y as i32,
    };
    // SAFETY : simples appels sans pointeur conservé.
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let (mut dx, mut dy) = (96u32, 96u32);
    // SAFETY : pointeurs vers deux u32 locaux.
    let hr = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
    if hr == 0 && dx > 0 {
        f64::from(dx) / 96.0
    } else {
        1.0
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn scale_at(_x: f64, _y: f64) -> f64 {
    1.0
}

/// Menu de l'icône : même contenu et même ordre que celui de Screenpresso (§ 11.6),
/// avec les libellés de Snappy, et ses propres entrées (historique, images
/// enregistrées, autorisation) dans le groupe « Application ».
fn menu(state: &MenuState) -> Result<Menu> {
    let shortcut = |mode: Mode| {
        state
            .labels
            .iter()
            .find(|(m, _)| *m == mode)
            .map(|(_, l)| l.as_str())
    };
    let with_shortcut = |text: &str, mode: Option<Mode>| match mode.and_then(shortcut) {
        Some(s) => format!("{text}\t{s}"),
        None => text.to_owned(),
    };
    let item = |choice, text: &str, mode: Option<Mode>| -> Box<dyn tray_icon::menu::IsMenuItem> {
        Box::new(MenuItem::with_id(
            id_of(choice),
            with_shortcut(text, mode),
            true,
            None,
        ))
    };
    let iconed = |choice,
                  text: &str,
                  mode: Option<Mode>,
                  glyph: Glyph|
     -> Box<dyn tray_icon::menu::IsMenuItem> {
        Box::new(IconMenuItem::with_id(
            id_of(choice),
            with_shortcut(text, mode),
            true,
            glyph.icon(),
            None,
        ))
    };
    let capture = |mode: Mode, text: &str| item(MenuChoice::Capture(mode), text, Some(mode));
    let capture_iconed =
        |mode: Mode, text: &str, glyph| iconed(MenuChoice::Capture(mode), text, Some(mode), glyph);
    let separator =
        || -> Box<dyn tray_icon::menu::IsMenuItem> { Box::new(PredefinedMenuItem::separator()) };

    let mut items: Vec<Box<dyn tray_icon::menu::IsMenuItem>> = vec![
        // Capture d'image.
        capture_iconed(Mode::Region, "Capturer une zone", Glyph::Region),
        capture(Mode::Previous, "Capturer la zone précédente"),
        capture(Mode::Screen, "Capturer l'écran"),
        capture(Mode::AllScreens, "Capturer tous les écrans"),
        capture(Mode::Window, "Capturer la fenêtre active"),
        iconed(
            MenuChoice::CaptureDelayed,
            &format!("Capturer une zone dans {} s", state.delay),
            Some(Mode::Delayed),
            Glyph::Clock,
        ),
        capture_iconed(Mode::Scrolling, "Capture avec défilement", Glyph::Scroll),
        capture_iconed(Mode::Ocr, "Copier le texte (OCR)", Glyph::Ocr),
        capture(Mode::Qr, "Décoder un QR code"),
        capture_iconed(Mode::ColorPicker, "Pipette de couleur", Glyph::Pipette),
        separator(),
        // Vidéo.
        capture_iconed(Mode::Video, "Enregistrer une vidéo", Glyph::Video),
        capture(Mode::VideoRegion, "Enregistrer une zone en vidéo"),
        separator(),
        // Application.
        capture_iconed(Mode::Workspace, "Espace de travail", Glyph::Workspace),
        iconed(MenuChoice::OpenSettings, "Réglages…", None, Glyph::Settings),
        item(MenuChoice::OpenCaptures, "Historique des captures", None),
        item(MenuChoice::OpenSaved, "Images enregistrées", None),
    ];
    if state.needs_permission {
        items.push(item(
            MenuChoice::Permission,
            "Autoriser l'enregistrement de l'écran…",
            None,
        ));
    }
    items.extend([
        separator(),
        iconed(MenuChoice::Help, "Aide", None, Glyph::Help),
        iconed(MenuChoice::About, "À propos de Snappy", None, Glyph::About),
        separator(),
        iconed(MenuChoice::Quit, "Quitter Snappy", None, Glyph::Quit),
    ]);
    let menu = Menu::new();
    let refs: Vec<&dyn tray_icon::menu::IsMenuItem> = items.iter().map(|b| b.as_ref()).collect();
    menu.append_items(&refs)?;
    Ok(menu)
}

impl Tray {
    /// Compte à rebours d'une capture différée : le nombre de secondes à côté de
    /// l'icône (macOS) et dans l'info-bulle. `None` : retour à la normale.
    pub fn countdown(&mut self, seconds: Option<u32>) {
        let (title, tip) = match seconds {
            Some(n) => (Some(n.to_string()), format!("Snappy · capture dans {n} s")),
            None => (None, "Snappy".to_owned()),
        };
        self.icon.set_title(title);
        if let Err(e) = self.icon.set_tooltip(Some(tip)) {
            log::warn!("info-bulle : {e}");
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
    }
}

// --- Icônes du menu ------------------------------------------------------------------

/// Icône carrée de couleur d'une entrée du menu, dessinée au lancement (comme
/// celles de Screenpresso : rouge capture et vidéo, bleu différé, défilement,
/// réglages et aide, vert pipette et espace de travail, orange quitter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Glyph {
    Region,
    Clock,
    Scroll,
    Ocr,
    Pipette,
    Video,
    Workspace,
    Settings,
    Help,
    About,
    Quit,
}

const RED: [u8; 3] = [0xd3, 0x2f, 0x2f];
const BLUE: [u8; 3] = [0x3b, 0x6e, 0xb4];
const GREEN: [u8; 3] = [0x4c, 0x9a, 0x3a];
const ORANGE: [u8; 3] = [0xe0, 0x8a, 0x1e];

/// Côté des icônes, en pixels : 18 points sur un écran Retina.
const MENU_ICON_PX: u32 = 36;

impl Glyph {
    fn color(self) -> [u8; 3] {
        match self {
            Self::Region | Self::Video | Self::Ocr => RED,
            Self::Clock | Self::Scroll | Self::Settings | Self::Help | Self::About => BLUE,
            Self::Pipette | Self::Workspace => GREEN,
            Self::Quit => ORANGE,
        }
    }

    fn icon(self) -> Option<tray_icon::menu::Icon> {
        let rgba = menu_icon::draw(self);
        tray_icon::menu::Icon::from_rgba(rgba, MENU_ICON_PX, MENU_ICON_PX)
            .map_err(|e| log::warn!("icône de menu : {e}"))
            .ok()
    }
}

mod menu_icon {
    use std::sync::LazyLock;

    use ab_glyph::{Font, FontRef, OutlineCurve};
    use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

    use super::{Glyph, MENU_ICON_PX};
    use crate::ui::icons;

    static LUCIDE: LazyLock<Option<FontRef<'static>>> = LazyLock::new(|| {
        FontRef::try_from_slice(include_bytes!("../../assets/fonts/Lucide.ttf")).ok()
    });

    fn paint(rgb: [u8; 3], alpha: u8) -> Paint<'static> {
        let mut p = Paint::default();
        p.set_color_rgba8(rgb[0], rgb[1], rgb[2], alpha);
        p.anti_alias = true;
        p
    }

    const WHITE: [u8; 3] = [255, 255, 255];

    /// Pixels RGBA (non prémultipliés) de l'icône.
    pub fn draw(glyph: Glyph) -> Vec<u8> {
        let n = MENU_ICON_PX as f32;
        let Some(mut pm) = Pixmap::new(MENU_ICON_PX, MENU_ICON_PX) else {
            return vec![0; (MENU_ICON_PX * MENU_ICON_PX * 4) as usize];
        };
        if let Some(rect) = Rect::from_xywh(1.0, 1.0, n - 2.0, n - 2.0) {
            let path = PathBuilder::from_rect(rect);
            pm.fill_path(
                &path,
                &paint(glyph.color(), 255),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        let c = n / 2.0;
        match glyph {
            Glyph::Region => lucide(&mut pm, icons::REGION, 22.0, 0.0),
            Glyph::Scroll => lucide(&mut pm, icons::ARROW_BIG, 22.0, 90.0),
            Glyph::Ocr => lucide(&mut pm, icons::TEXT, 22.0, 0.0),
            Glyph::Pipette => lucide(&mut pm, icons::PIPETTE, 22.0, 0.0),
            Glyph::Workspace => lucide(&mut pm, icons::IMAGE, 22.0, 0.0),
            Glyph::Settings => lucide(&mut pm, icons::SETTINGS, 22.0, 0.0),
            Glyph::Help => lucide(&mut pm, icons::STAMP_QUESTION, 22.0, 0.0),
            Glyph::About => lucide(&mut pm, icons::STAMP_INFO, 22.0, 0.0),
            Glyph::Quit => lucide(&mut pm, icons::CLOSE, 24.0, 0.0),
            Glyph::Clock => {
                // Cadran et aiguilles.
                let stroke = Stroke {
                    width: 2.6,
                    line_cap: tiny_skia::LineCap::Round,
                    ..Stroke::default()
                };
                if let Some(circle) = PathBuilder::from_circle(c, c + 1.0, 9.5) {
                    pm.stroke_path(
                        &circle,
                        &paint(WHITE, 255),
                        &stroke,
                        Transform::identity(),
                        None,
                    );
                }
                let mut pb = PathBuilder::new();
                pb.move_to(c, c - 5.0);
                pb.line_to(c, c + 1.0);
                pb.line_to(c + 4.5, c + 3.5);
                // Bouton du chronomètre.
                pb.move_to(c - 3.0, c - 12.0);
                pb.line_to(c + 3.0, c - 12.0);
                if let Some(path) = pb.finish() {
                    pm.stroke_path(
                        &path,
                        &paint(WHITE, 255),
                        &stroke,
                        Transform::identity(),
                        None,
                    );
                }
            }
            Glyph::Video => {
                // Caméra : boîtier et objectif.
                if let Some(body) = Rect::from_xywh(7.0, 11.0, 15.0, 14.0) {
                    pm.fill_path(
                        &PathBuilder::from_rect(body),
                        &paint(WHITE, 255),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
                let mut pb = PathBuilder::new();
                pb.move_to(23.0, 16.0);
                pb.line_to(30.0, 11.5);
                pb.line_to(30.0, 24.5);
                pb.line_to(23.0, 20.0);
                pb.close();
                if let Some(path) = pb.finish() {
                    pm.fill_path(
                        &path,
                        &paint(WHITE, 255),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
        pm.pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect()
    }

    /// Glyphe Lucide blanc, centré, de côté `size`, tourné de `degrees`.
    fn lucide(pm: &mut Pixmap, icon: &str, size: f32, degrees: f32) {
        let (Some(font), Some(ch)) = (LUCIDE.as_ref(), icon.chars().next()) else {
            return;
        };
        let Some(outline) = font.outline(font.glyph_id(ch)) else {
            return;
        };
        let units = font.units_per_em().unwrap_or(1000.0);
        let s = size / units;
        let b = outline.bounds;
        let (cx, cy) = ((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
        let n = MENU_ICON_PX as f32 / 2.0;
        let to = |p: ab_glyph::Point| (n + (p.x - cx) * s, n - (p.y - cy) * s);
        let mut pb = PathBuilder::new();
        let mut last: Option<ab_glyph::Point> = None;
        for curve in &outline.curves {
            let start = match curve {
                OutlineCurve::Line(a, _)
                | OutlineCurve::Quad(a, _, _)
                | OutlineCurve::Cubic(a, _, _, _) => *a,
            };
            if last != Some(start) {
                if last.is_some() {
                    pb.close();
                }
                let (x, y) = to(start);
                pb.move_to(x, y);
            }
            match curve {
                OutlineCurve::Line(_, b) => {
                    let (x, y) = to(*b);
                    pb.line_to(x, y);
                    last = Some(*b);
                }
                OutlineCurve::Quad(_, c1, b) => {
                    let ((x1, y1), (x, y)) = (to(*c1), to(*b));
                    pb.quad_to(x1, y1, x, y);
                    last = Some(*b);
                }
                OutlineCurve::Cubic(_, c1, c2, b) => {
                    let ((x1, y1), (x2, y2), (x, y)) = (to(*c1), to(*c2), to(*b));
                    pb.cubic_to(x1, y1, x2, y2, x, y);
                    last = Some(*b);
                }
            }
        }
        if last.is_some() {
            pb.close();
        }
        let Some(path) = pb.finish() else { return };
        let ts = Transform::from_rotate_at(degrees, n, n);
        pm.fill_path(&path, &paint(WHITE, 255), FillRule::Winding, ts, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_round_trip() {
        for (id, choice) in IDS {
            assert_eq!(parse_menu_id(id), Some(choice));
            assert_eq!(id_of(choice), id);
        }
        assert_eq!(parse_menu_id("nope"), None);
    }

    #[test]
    fn tray_icon_has_the_right_size() {
        assert_eq!(ICON.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
    }

    #[test]
    fn every_action_but_delayed_has_a_menu_entry() {
        for mode in Mode::ALL {
            let has = IDS.iter().any(|(_, c)| *c == MenuChoice::Capture(mode));
            // La capture différée a son entrée à elle (délai de la config).
            assert_eq!(has, mode != Mode::Delayed, "{mode:?}");
        }
    }

    #[test]
    fn menu_icons_are_colored_squares_with_a_white_glyph() {
        for glyph in [
            Glyph::Region,
            Glyph::Clock,
            Glyph::Scroll,
            Glyph::Ocr,
            Glyph::Pipette,
            Glyph::Video,
            Glyph::Workspace,
            Glyph::Settings,
            Glyph::Help,
            Glyph::About,
            Glyph::Quit,
        ] {
            let px = menu_icon::draw(glyph);
            assert_eq!(px.len(), (MENU_ICON_PX * MENU_ICON_PX * 4) as usize);
            let corner = &px[(MENU_ICON_PX as usize + 2) * 4..][..3];
            assert_eq!(corner, glyph.color(), "{glyph:?} : fond");
            let white = px.chunks(4).filter(|p| p[..3] == [255, 255, 255]).count();
            assert!(white > 20, "{glyph:?} : pas de dessin ({white} px blancs)");
        }
    }
}
