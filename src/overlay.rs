//! Sélection de zone : **tous les écrans** figés au moment du raccourci, en plein
//! écran par-dessus tout (barre des menus comprise). Une fenêtre egui par écran :
//! la principale sur l'écran du curseur, une fenêtre secondaire (« viewport »)
//! sur chacun des autres.
//!
//! - survol : la fenêtre sous la souris est encadrée ; **clic** : elle est
//!   sélectionnée (le bord du haut de l'écran, ou un clic sur le bureau : l'écran
//!   entier) ;
//! - glisser : nouvelle sélection ; glisser dans la sélection : la déplacer ;
//!   poignées : la redimensionner ; flèches : la décaler d'un point (⇧ : dix) ;
//! - molette ou pincement : zoom sur l'écran figé, centré sur le curseur ;
//!   0 : taille réelle ;
//! - Entrée, ⌘C ou double-clic : copier ; ⌘S : enregistrer ; E : annoter ;
//!   Échap : annuler ;
//! - M : inclure ou non le curseur de la souris ; C : copier la couleur visée ;
//!   H : aide ;
//! - F : l'écran entier sous la souris ; O : pipette (un clic copie la couleur au
//!   lieu de sélectionner) ; D : couleurs en hexadécimal ou en décimal.
//!
//! Une sélection reste sur un seul écran. Les coordonnées de travail sont des
//! « points de base » de l'écran (pixels de l'image ÷ échelle), avant zoom.
//!
//! La même surcouche sert aussi, sans barre d'actions, à choisir une zone pour une
//! autre fonction ([`pick`] : OCR, QR, vidéo, défilement) et à la pipette seule
//! ([`pick_color`]).

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::{
    Align2, Color32, CornerRadius, CursorIcon, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Vec2, ViewportCommand, ViewportId, pos2, vec2,
};
use image::RgbaImage;

use crate::capture::{Capture, Shot};
use crate::config::{LastRegion, Store};
use crate::history;
use crate::model::CaptureContext;
use crate::platform::{self, ScreenRect};
use crate::ui::{icons, keys, theme};

/// Ce que l'utilisateur a choisi, à exécuter une fois la surcouche fermée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Cancelled,
    Copied,
    Saved(PathBuf),
    /// Ouvrir l'éditeur sur cette capture (enregistrée dans l'historique).
    Edit(PathBuf),
}

/// Réglages de la surcouche. Par défaut : le comportement historique.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Curseur inclus au départ (M l'inverse). Réglage `capture_cursor`.
    pub with_cursor: bool,
    /// Réticules gris sur tout l'écran à la position de la souris pendant la visée,
    /// comme Screenpresso. Désactivé par défaut (à brancher sur un réglage).
    pub guides: bool,
    /// Enregistrer l'application et le titre de la fenêtre choisie d'un clic dans le
    /// `.json` de la capture (`Doc::context`). Désactivé par défaut (réglage
    /// `capture_context` à brancher).
    pub capture_context: bool,
}

/// Affiche la surcouche sur tous les écrans capturés ; rend le choix de
/// l'utilisateur. `with_cursor` : curseur inclus au départ (M l'inverse).
pub fn run(store: Store, capture: Capture, with_cursor: bool) -> Result<Outcome> {
    run_with(
        store,
        capture,
        Options {
            with_cursor,
            ..Options::default()
        },
    )
}

/// [`run`] avec toutes les options.
pub fn run_with(store: Store, capture: Capture, options: Options) -> Result<Outcome> {
    Ok(match launch(store, capture, options, Flow::Actions)? {
        Some(Exit::Outcome(outcome)) => outcome,
        _ => Outcome::Cancelled,
    })
}

/// Pipette seule (`Mode::ColorPicker`) : un clic copie la couleur du pixel visé
/// (#RRGGBB, ou `rgb(r, g, b)` après D), affiche la confirmation un instant et
/// ferme. Rend le texte copié ; `None` : annulé (Échap).
pub fn pick_color(store: Store, capture: Capture, options: Options) -> Result<Option<String>> {
    Ok(match launch(store, capture, options, Flow::ColorPicker)? {
        Some(Exit::Color(text)) => Some(text),
        _ => None,
    })
}

/// Déroulement de la surcouche.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    /// Sélection puis barre d'actions (copier, enregistrer, annoter).
    Actions,
    /// Zone rendue dès qu'elle est choisie, pour une autre fonction.
    Pick(Purpose),
    /// Pipette seule.
    ColorPicker,
}

/// Fin de la surcouche.
enum Exit {
    Outcome(Outcome),
    Picked(Box<Picked>),
    Color(String),
}

/// Durée de la confirmation de la pipette seule, avant fermeture.
const CONFIRM: Duration = Duration::from_millis(700);

fn launch(store: Store, capture: Capture, options: Options, flow: Flow) -> Result<Option<Exit>> {
    anyhow::ensure!(!capture.shots.is_empty(), "aucun écran capturé");
    let root = capture.active.min(capture.shots.len() - 1);
    let root_shot = &capture.shots[root];
    let (frame, scale) = (screen_rect(root_shot), root_shot.scale);
    let viewport = overlay_builder(root_shot, &title(root));
    let exit = Rc::new(Cell::new(None));
    let out = Rc::clone(&exit);
    let settings = options;
    let native = eframe::NativeOptions {
        viewport,
        persist_window: false,
        event_loop_builder: Some(Box::new(|builder| {
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
                // Pas d'icône dans le Dock pour une fenêtre de quelques secondes.
                builder
                    .with_activation_policy(ActivationPolicy::Accessory)
                    .with_default_menu(false);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = builder;
        })),
        ..Default::default()
    };
    eframe::run_native(
        "Snappy",
        native,
        Box::new(move |cc| {
            platform::make_overlay(cc, frame, scale);
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(Overlay::new(
                store, capture, root, settings, flow, out,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("surcouche : {e}"))?;
    Ok(exit.take())
}

/// Titre (unique) de la fenêtre de l'écran `i` : sert à retrouver les fenêtres
/// secondaires côté système pour les placer.
fn title(i: usize) -> String {
    format!("Snappy · écran {}", i + 1)
}

fn screen_rect(shot: &Shot) -> ScreenRect {
    ScreenRect {
        x: shot.frame.x,
        y: shot.frame.y,
        width: shot.frame.width,
        height: shot.frame.height,
    }
}

/// Fenêtre sans bordure à la taille de l'écran. Le placement exact (au-dessus de la
/// barre des menus, en pixels physiques sous Windows) est fait ensuite par
/// [`platform`].
fn overlay_builder(shot: &Shot, title: &str) -> egui::ViewportBuilder {
    let f = shot.frame;
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_decorations(false)
        .with_resizable(false)
        .with_has_shadow(false)
        .with_taskbar(false)
        .with_position([f.x as f32, f.y as f32])
        .with_inner_size([f.width as f32, f.height as f32])
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
}

/// Taille d'une poignée de redimensionnement (points).
const HANDLE: f32 = 8.0;
/// En deçà, un glisser est un clic (points).
const CLICK_SLOP: f32 = 3.0;
/// Loupe : côté de la zone grossie (pixels de l'image) et grossissement.
const LOUPE_PIXELS: i32 = 15;
const LOUPE_ZOOM: f32 = 8.0;
/// Zoom maximal sur l'écran figé.
const MAX_ZOOM: f32 = 12.0;
/// Bande du haut de l'écran qui désigne l'écran entier (points).
const TOP_EDGE: f32 = 3.0;
/// Assombrissement hors sélection.
const DIM: Color32 = Color32::from_black_alpha(130);
/// Assombrissement léger pendant la visée.
const AIM_DIM: Color32 = Color32::from_black_alpha(60);

/// Bords qu'une poignée déplace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Edges {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

/// Geste en cours, en points de base de son écran.
#[derive(Debug, Clone, Copy)]
enum Drag {
    New {
        origin: Pos2,
    },
    Move {
        grab: Vec2,
    },
    Resize {
        edges: Edges,
        start: Rect,
        origin: Pos2,
    },
}

/// Zoom d'un écran : `écran = base × zoom + décalage`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct View {
    zoom: f32,
    offset: Vec2,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            offset: Vec2::ZERO,
        }
    }
}

impl View {
    fn to_screen(self, p: Pos2) -> Pos2 {
        (p.to_vec2() * self.zoom + self.offset).to_pos2()
    }

    fn to_base(self, q: Pos2) -> Pos2 {
        ((q.to_vec2() - self.offset) / self.zoom).to_pos2()
    }

    fn rect(self, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(r.min), self.to_screen(r.max))
    }

    /// Zoom multiplié par `factor` autour du point d'écran `at`, borné, l'image
    /// couvrant toujours toute la fenêtre de taille `size`.
    fn zoomed(self, factor: f32, at: Pos2, size: Vec2) -> Self {
        let zoom = (self.zoom * factor).clamp(1.0, MAX_ZOOM);
        let base = self.to_base(at);
        let offset = at.to_vec2() - base.to_vec2() * zoom;
        let min = size * (1.0 - zoom);
        Self {
            zoom,
            offset: vec2(offset.x.clamp(min.x, 0.0), offset.y.clamp(min.y, 0.0)),
        }
    }
}

struct Screen {
    shot: Shot,
    /// Créée à la première image : avant, egui ne connaît pas encore la taille
    /// maximale des textures du GPU (2048 par défaut, moins qu'un écran Retina).
    texture: Option<TextureHandle>,
    /// Morceau d'écran avec le curseur : sa texture et sa place en points de base.
    cursor: Option<(Option<TextureHandle>, Rect)>,
    viewport: ViewportId,
    title: String,
    view: View,
    /// Fenêtre secondaire placée par le système (la principale l'est d'office).
    placed: bool,
}

impl Screen {
    fn scale(&self) -> f32 {
        self.shot.scale as f32
    }

    /// L'écran entier, en points de base.
    fn base_rect(&self) -> Rect {
        let s = self.scale();
        Rect::from_min_size(
            Pos2::ZERO,
            vec2(
                self.shot.image.width() as f32,
                self.shot.image.height() as f32,
            ) / s,
        )
    }

    /// Fenêtre (rognée à l'écran) sous le point de base `p`, de devant à derrière,
    /// avec son libellé.
    fn window_at(&self, p: Pos2) -> Option<(Rect, String)> {
        self.window_hit(p)
            .map(|(r, w)| (r, window_label(&w.app, &w.title)))
    }

    /// Comme [`Screen::window_at`], avec la fenêtre elle-même (application, titre).
    fn window_hit(&self, p: Pos2) -> Option<(Rect, &crate::capture::WindowRect)> {
        let s = self.scale();
        let screen = self.base_rect();
        self.shot.windows.iter().find_map(|w| {
            let r = Rect::from_min_size(
                pos2(w.x as f32, w.y as f32) / s,
                vec2(w.width as f32, w.height as f32) / s,
            )
            .intersect(screen);
            (r.is_positive() && r.contains(p)).then_some((r, w))
        })
    }

    fn pixel_at(&self, base: Pos2) -> Option<[u8; 4]> {
        let s = self.scale();
        let (x, y) = ((base.x * s).floor(), (base.y * s).floor());
        if x < 0.0 || y < 0.0 {
            return None;
        }
        self.shot
            .image
            .get_pixel_checked(x as u32, y as u32)
            .map(|p| p.0)
    }

    /// Textures de l'écran et du curseur, au premier affichage.
    fn ensure_textures(&mut self, ctx: &egui::Context) {
        // Au plus proche : l'image s'affiche pixel pour pixel, et la loupe reste nette.
        if self.texture.is_none() {
            self.texture = Some(ctx.load_texture(
                format!("{} · image", self.title),
                color_image(&self.shot.image),
                TextureOptions::NEAREST,
            ));
        }
        if let (Some((tex @ None, _)), Some(c)) = (&mut self.cursor, &self.shot.cursor) {
            *tex = Some(ctx.load_texture(
                format!("{} · curseur", self.title),
                color_image(&c.image),
                TextureOptions::NEAREST,
            ));
        }
    }

    /// Taille en pixels d'un rectangle de base.
    fn pixel_size(&self, r: Rect) -> (u32, u32) {
        let (w, h) = self.shot.image.dimensions();
        pixel_rect(r, self.scale(), w, h).map_or((0, 0), |(_, _, w, h)| (w, h))
    }
}

/// « App — titre », sans répétition ni vide.
fn window_label(app: &str, title: &str) -> String {
    match (app.trim(), title.trim()) {
        ("", "") => "Fenêtre".into(),
        (a, "") => a.into(),
        ("", t) => t.into(),
        (a, t) if t.contains(a) => t.into(),
        (a, t) => format!("{a} — {t}"),
    }
}

/// Sélection : un écran et un rectangle en points de base.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Selection {
    screen: usize,
    rect: Rect,
}

struct Overlay {
    store: Store,
    screens: Vec<Screen>,
    root: usize,
    selection: Option<Selection>,
    drag: Option<(usize, Drag)>,
    show_cursor: bool,
    show_help: bool,
    /// Message temporaire (couleur copiée, curseur, erreur).
    message: Option<String>,
    exit: Rc<Cell<Option<Exit>>>,
    closing: bool,
    // --- Lot « Capture » ---
    flow: Flow,
    options: Options,
    /// Pipette (O) : un clic copie la couleur au lieu de sélectionner.
    pipette: bool,
    /// Couleurs en décimal `rgb(r, g, b)` plutôt qu'en #RRGGBB (D).
    decimal: bool,
    /// Dernier écran survolé par la souris (F : l'écran entier sous la souris).
    hover_screen: Option<usize>,
    /// Fenêtre choisie d'un clic pour la sélection courante : application, titre.
    clicked_window: Option<(String, String)>,
    /// Fermeture différée (confirmation de la pipette seule).
    close_at: Option<Instant>,
}

impl Overlay {
    fn new(
        store: Store,
        capture: Capture,
        root: usize,
        options: Options,
        flow: Flow,
        exit: Rc<Cell<Option<Exit>>>,
    ) -> Self {
        let show_cursor = options.with_cursor;
        let screens = capture
            .shots
            .into_iter()
            .enumerate()
            .map(|(i, shot)| {
                let s = shot.scale as f32;
                let cursor = shot.cursor.as_ref().map(|c| {
                    let at = pos2(c.x as f32, c.y as f32) / s;
                    let size = vec2(c.image.width() as f32, c.image.height() as f32) / s;
                    (None, Rect::from_min_size(at, size))
                });
                let viewport = if i == root {
                    ViewportId::ROOT
                } else {
                    ViewportId::from_hash_of(("snappy-écran", i))
                };
                Screen {
                    shot,
                    texture: None,
                    cursor,
                    viewport,
                    title: title(i),
                    view: View::default(),
                    placed: i == root,
                }
            })
            .collect();
        Self {
            store,
            screens,
            root,
            selection: None,
            drag: None,
            show_cursor,
            show_help: false,
            message: None,
            exit,
            closing: false,
            flow,
            options,
            pipette: flow == Flow::ColorPicker,
            decimal: false,
            hover_screen: None,
            clicked_window: None,
            close_at: None,
        }
    }

    /// Image de la sélection (avec le curseur s'il est affiché), et mémorisation de
    /// la zone pour « Capturer la zone précédente ».
    fn cropped(&self) -> Option<RgbaImage> {
        let sel = self.selection?;
        let screen = &self.screens[sel.screen];
        let shot = &screen.shot;
        let (iw, ih) = shot.image.dimensions();
        let (x, y, w, h) = pixel_rect(sel.rect, screen.scale(), iw, ih)?;
        self.store.save_last_region(&LastRegion {
            display_id: shot.display_id,
            image_width: iw,
            image_height: ih,
            x,
            y,
            width: w,
            height: h,
        });
        Some(shot.region(x, y, w, h, self.show_cursor))
    }

    fn finish(&mut self, ctx: &egui::Context, outcome: Outcome) {
        self.close(ctx, Exit::Outcome(outcome));
    }

    fn close(&mut self, ctx: &egui::Context, exit: Exit) {
        self.exit.set(Some(exit));
        self.closing = true;
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
    }

    /// Contexte de la capture enregistrée en `path` (fenêtre choisie d'un clic), si
    /// l'option est activée.
    fn record_context(&self, path: &Path) {
        if !self.options.capture_context {
            return;
        }
        let Some((app, title)) = &self.clicked_window else {
            return;
        };
        if let Err(e) = history::save_context(path, &CaptureContext::now(app, title)) {
            log::warn!("contexte de la capture : {e:#}");
        }
    }

    fn copy(&mut self, ctx: &egui::Context) {
        let Some(img) = self.cropped() else { return };
        match history::save_capture(&self.store.captures_dir(), &img) {
            Ok(path) => self.record_context(&path),
            Err(e) => log::warn!("historique : {e:#}"),
        }
        match history::copy_image(&img) {
            Ok(()) => self.finish(ctx, Outcome::Copied),
            Err(e) => self.message = Some(format!("{e:#}")),
        }
    }

    fn save(&mut self, ctx: &egui::Context) {
        let Some(img) = self.cropped() else { return };
        let result = history::save_capture(&self.store.captures_dir(), &img).and_then(|original| {
            self.record_context(&original);
            let dir = self.store.save_dir(&self.store.load());
            history::export(&dir, &original, &img)
        });
        match result {
            Ok(path) => self.finish(ctx, Outcome::Saved(path)),
            Err(e) => self.message = Some(format!("{e:#}")),
        }
    }

    fn edit(&mut self, ctx: &egui::Context) {
        let Some(img) = self.cropped() else { return };
        match history::save_capture(&self.store.captures_dir(), &img) {
            Ok(path) => {
                self.record_context(&path);
                self.finish(ctx, Outcome::Edit(path));
            }
            Err(e) => self.message = Some(format!("{e:#}")),
        }
    }

    /// Mode [`pick`] : rend la sélection courante et ferme.
    fn deliver_pick(&mut self, ctx: &egui::Context) {
        let Some(sel) = self.selection else { return };
        let shot = &self.screens[sel.screen].shot;
        match picked_from(
            shot,
            sel.screen,
            sel.rect,
            self.show_cursor,
            self.clicked_window.clone(),
        ) {
            Some(picked) => self.close(ctx, Exit::Picked(Box::new(picked))),
            None => self.selection = None,
        }
    }

    /// Couleur du pixel sous `pos` (point de la fenêtre de l'écran `i`), au format
    /// choisi (D).
    fn color_at(&self, i: usize, pos: Pos2) -> Option<String> {
        let screen = &self.screens[i];
        let px = screen.pixel_at(screen.view.to_base(pos))?;
        Some(color_text(px, self.decimal))
    }

    /// Pipette : copie la couleur visée ; la pipette seule ferme peu après.
    fn pipette_click(&mut self, ctx: &egui::Context, i: usize, pos: Pos2) {
        let Some(text) = self.color_at(i, pos) else {
            return;
        };
        ctx.copy_text(text.clone());
        self.message = Some(format!("Couleur {text} copiée"));
        if self.flow == Flow::ColorPicker {
            self.exit.set(Some(Exit::Color(text)));
            self.close_at = Some(Instant::now() + CONFIRM);
        }
    }

    /// Clavier de la fenêtre de l'écran `i`.
    fn handle_keys(&mut self, ctx: &egui::Context, i: usize) {
        struct Keys {
            esc: bool,
            enter: bool,
            copy: bool,
            save: bool,
            edit: bool,
            hex: bool,
            cursor: bool,
            help: bool,
            reset_zoom: bool,
            nudge: Vec2,
            full_screen: bool,
            pipette: bool,
            decimal: bool,
        }
        let k = ctx.input_mut(|inp| {
            let nudge = [
                (Key::ArrowLeft, vec2(-1.0, 0.0)),
                (Key::ArrowRight, vec2(1.0, 0.0)),
                (Key::ArrowUp, vec2(0.0, -1.0)),
                (Key::ArrowDown, vec2(0.0, 1.0)),
            ]
            .into_iter()
            .filter(|(k, _)| inp.key_pressed(*k))
            .map(|(_, d)| if inp.modifiers.shift { d * 10.0 } else { d })
            .fold(Vec2::ZERO, |a, b| a + b);
            Keys {
                esc: inp.consume_key(Modifiers::NONE, Key::Escape),
                enter: inp.key_pressed(Key::Enter),
                // ⌘C arrive comme un événement « copier », pas comme une touche.
                copy: inp.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                save: inp.consume_key(Modifiers::COMMAND, Key::S),
                edit: inp.consume_key(Modifiers::NONE, Key::E),
                hex: inp.consume_key(Modifiers::NONE, Key::C),
                cursor: inp.consume_key(Modifiers::NONE, Key::M),
                help: inp.consume_key(Modifiers::NONE, Key::H),
                reset_zoom: inp.consume_key(Modifiers::NONE, Key::Num0),
                nudge,
                full_screen: inp.consume_key(Modifiers::NONE, Key::F),
                pipette: inp.consume_key(Modifiers::NONE, Key::O),
                decimal: inp.consume_key(Modifiers::NONE, Key::D),
            }
        });
        if k.esc {
            if self.drag.take().is_none() {
                self.finish(ctx, Outcome::Cancelled);
            }
            return;
        }
        if k.decimal {
            self.decimal = !self.decimal;
            self.message = Some(if self.decimal {
                "Couleurs en décimal : rgb(r, g, b)".into()
            } else {
                "Couleurs en hexadécimal : #RRGGBB".into()
            });
        }
        if k.hex {
            self.copy_hex_under_pointer(ctx, i);
        }
        if k.pipette && self.flow == Flow::Actions {
            self.pipette = !self.pipette;
            self.selection = None;
            self.drag = None;
            self.clicked_window = None;
            self.message = Some(if self.pipette {
                "Pipette : clic pour copier la couleur  ·  O : quitter la pipette".into()
            } else {
                "Pipette désactivée".into()
            });
        }
        if k.full_screen && !self.pipette {
            let s = self.hover_screen.unwrap_or(i);
            self.selection = Some(Selection {
                screen: s,
                rect: self.screens[s].base_rect(),
            });
            self.drag = None;
            self.clicked_window = None;
            if matches!(self.flow, Flow::Pick(_)) {
                self.deliver_pick(ctx);
                return;
            }
        }
        if k.cursor {
            self.show_cursor = !self.show_cursor;
            let has_cursor = self.screens.iter().any(|s| s.cursor.is_some());
            self.message = Some(match (self.show_cursor, has_cursor) {
                (true, true) => "Curseur inclus dans la capture".into(),
                (true, false) => "Curseur inclus (introuvable sur cette capture)".into(),
                (false, _) => "Curseur masqué".into(),
            });
        }
        if k.help {
            self.show_help = !self.show_help;
        }
        if k.reset_zoom {
            self.screens[i].view = View::default();
        }
        let Some(sel) = self.selection else { return };
        if k.nudge != Vec2::ZERO {
            let bounds = self.screens[sel.screen].base_rect();
            self.selection = Some(Selection {
                rect: keep_inside(sel.rect.translate(k.nudge), bounds),
                ..sel
            });
        }
        if k.enter || k.copy {
            self.copy(ctx);
        } else if k.save {
            self.save(ctx);
        } else if k.edit {
            self.edit(ctx);
        }
    }

    fn copy_hex_under_pointer(&mut self, ctx: &egui::Context, i: usize) {
        let Some(pos) = ctx.pointer_latest_pos() else {
            return;
        };
        if let Some(text) = self.color_at(i, pos) {
            ctx.copy_text(text.clone());
            self.message = Some(format!("Couleur {text} copiée"));
        }
    }

    /// Molette et pincement : zoom centré sur le curseur.
    fn handle_zoom(&mut self, ctx: &egui::Context, i: usize, window: Rect) {
        let (scroll, pinch, pointer) = ctx.input(|inp| {
            (
                inp.smooth_scroll_delta.y,
                inp.zoom_delta(),
                inp.pointer.hover_pos(),
            )
        });
        let factor = (scroll / 300.0).exp() * pinch;
        if let Some(p) = pointer
            && (factor - 1.0).abs() > 1e-4
        {
            let screen = &mut self.screens[i];
            screen.view = screen.view.zoomed(factor, p, window.size());
            ctx.request_repaint();
        }
    }

    /// Souris de l'écran `i` : visée, sélection, déplacement, redimensionnement.
    fn handle_pointer(&mut self, ctx: &egui::Context, i: usize, response: &egui::Response) {
        let view = self.screens[i].view;
        let bounds = self.screens[i].base_rect();
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ctx.pointer_latest_pos());
        // Pipette : un clic copie la couleur, rien ne se sélectionne.
        if self.pipette {
            if response.clicked()
                && let Some(p) = pointer
            {
                self.pipette_click(ctx, i, p);
            }
            ctx.set_cursor_icon(CursorIcon::Crosshair);
            return;
        }
        let picking = matches!(self.flow, Flow::Pick(_));
        let sel_here = self.selection.filter(|s| s.screen == i);
        let sel_screen = sel_here.map(|s| view.rect(s.rect));

        if response.double_clicked()
            && let (Some(sel), Some(p)) = (sel_screen, pointer)
            && sel.contains(p)
        {
            self.copy(ctx);
            return;
        }
        if response.drag_started()
            && let Some(p) = ctx.input(|inp| inp.pointer.press_origin()).or(pointer)
        {
            let base = view.to_base(p);
            let drag = match (sel_here, sel_screen) {
                (Some(sel), Some(on_screen)) => match handle_at(on_screen, p) {
                    Some(edges) => Drag::Resize {
                        edges,
                        start: sel.rect,
                        origin: base,
                    },
                    None if on_screen.contains(p) => Drag::Move {
                        grab: base - sel.rect.min,
                    },
                    None => Drag::New { origin: base },
                },
                _ => Drag::New { origin: base },
            };
            if matches!(drag, Drag::New { .. }) {
                self.clicked_window = None;
            }
            self.drag = Some((i, drag));
        }
        if let (Some((di, drag)), Some(p)) = (self.drag, pointer)
            && di == i
        {
            let base = view.to_base(p).clamp(bounds.min, bounds.max);
            let rect = match drag {
                Drag::New { origin } => Some(Rect::from_two_pos(origin, base)),
                Drag::Move { grab } => sel_here
                    .map(|s| keep_inside(Rect::from_min_size(base - grab, s.rect.size()), bounds)),
                Drag::Resize {
                    edges,
                    start,
                    origin,
                } => Some(resize(start, edges, base - origin).intersect(bounds)),
            };
            if let Some(rect) = rect {
                self.selection = Some(Selection { screen: i, rect });
            }
        }
        if response.drag_stopped() {
            let new = matches!(self.drag, Some((_, Drag::New { .. })));
            if new
                && self
                    .selection
                    .is_some_and(|s| s.rect.width().max(s.rect.height()) * view.zoom < CLICK_SLOP)
            {
                self.selection = None;
            }
            self.drag = None;
            // Mode « pick » : la zone part dès qu'on relâche.
            if picking && new && self.selection.is_some() {
                self.deliver_pick(ctx);
                return;
            }
        }
        // Clic hors de la sélection (ou sans sélection) : la fenêtre visée, sinon
        // l'écran entier.
        if response.clicked()
            && let Some(p) = pointer
            && !sel_screen.is_some_and(|s| s.contains(p))
        {
            let rect = self.target_at(i, p).map_or(bounds, |(r, _)| r);
            self.selection = Some(Selection { screen: i, rect });
            self.clicked_window = self.clicked_window_at(i, p);
            if picking {
                self.deliver_pick(ctx);
                return;
            }
        }

        // Curseur.
        let icon = match (self.drag, sel_screen, pointer) {
            (Some((_, Drag::Move { .. })), ..) => CursorIcon::Grabbing,
            (Some((_, Drag::Resize { edges, .. })), ..) => resize_cursor(edges),
            (Some((_, Drag::New { .. })), ..) | (None, None, _) => CursorIcon::Crosshair,
            (None, Some(sel), Some(p)) => match handle_at(sel, p) {
                Some(edges) => resize_cursor(edges),
                None if sel.contains(p) => CursorIcon::Grab,
                None => CursorIcon::Crosshair,
            },
            (None, Some(_), None) => CursorIcon::Default,
        };
        ctx.set_cursor_icon(icon);
    }

    /// Cible d'un clic au point d'écran `p` : l'écran entier tout en haut, sinon la
    /// fenêtre sous la souris. En points de base, avec un libellé.
    fn target_at(&self, i: usize, p: Pos2) -> Option<(Rect, String)> {
        let screen = &self.screens[i];
        if p.y <= TOP_EDGE {
            return Some((screen.base_rect(), "Écran entier".into()));
        }
        screen.window_at(screen.view.to_base(p))
    }

    /// Application et titre de la fenêtre qu'un clic en `p` choisit (rien pour
    /// l'écran entier).
    fn clicked_window_at(&self, i: usize, p: Pos2) -> Option<(String, String)> {
        if p.y <= TOP_EDGE {
            return None;
        }
        let screen = &self.screens[i];
        screen
            .window_hit(screen.view.to_base(p))
            .map(|(_, w)| (w.app.clone(), w.title.clone()))
    }

    fn paint(&self, painter: &egui::Painter, i: usize, window: Rect, pointer: Option<Pos2>) {
        let screen = &self.screens[i];
        let view = screen.view;
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        if let Some(tex) = &screen.texture {
            painter.image(tex.id(), view.rect(screen.base_rect()), uv, Color32::WHITE);
        }
        if self.show_cursor
            && let Some((Some(tex), at)) = &screen.cursor
        {
            painter.image(tex.id(), view.rect(*at), uv, Color32::WHITE);
        }
        // Pipette : l'écran tel quel, sans assombrissement qui fausserait les
        // couleurs à l'œil (réticules gris : voir `screen_ui`).
        if self.pipette {
            return;
        }
        match self.selection {
            None => {
                painter.rect_filled(window, 0.0, AIM_DIM);
                let Some(p) = pointer else { return };
                if let Some((target, label)) = self.target_at(i, p)
                    && self.drag.is_none()
                {
                    let r = view.rect(target);
                    painter.rect_filled(r, 0.0, theme::ACCENT_2.gamma_multiply(0.12));
                    painter.rect_stroke(
                        r,
                        0.0,
                        Stroke::new(2.0, theme::ACCENT_2),
                        StrokeKind::Inside,
                    );
                    let (w, h) = screen.pixel_size(target);
                    pill(painter, r, window, &format!("{label}  ·  {w} × {h}"));
                } else if !self.options.guides {
                    // Repères en croix sur tout l'écran.
                    let guide = Stroke::new(1.0, theme::ACCENT_2.gamma_multiply(0.5));
                    painter.hline(window.x_range(), p.y, guide);
                    painter.vline(p.x, window.y_range(), guide);
                }
            }
            Some(sel) if sel.screen != i => {
                painter.rect_filled(window, 0.0, DIM);
            }
            Some(sel) => {
                let r = view.rect(sel.rect);
                // Assombrit tout sauf la sélection (4 bandes).
                for band in [
                    Rect::from_min_max(window.min, pos2(window.max.x, r.min.y)),
                    Rect::from_min_max(pos2(window.min.x, r.max.y), window.max),
                    Rect::from_min_max(pos2(window.min.x, r.min.y), pos2(r.min.x, r.max.y)),
                    Rect::from_min_max(pos2(r.max.x, r.min.y), pos2(window.max.x, r.max.y)),
                ] {
                    if band.is_positive() {
                        painter.rect_filled(band, 0.0, DIM);
                    }
                }
                painter.rect_stroke(
                    r,
                    0.0,
                    Stroke::new(1.5, theme::ACCENT_2),
                    StrokeKind::Outside,
                );
                if !matches!(self.drag, Some((_, Drag::New { .. }))) {
                    for (center, _) in handles(r) {
                        let h = Rect::from_center_size(center, Vec2::splat(HANDLE));
                        painter.rect_filled(h, CornerRadius::same(2), theme::ACCENT_2);
                        painter.rect_stroke(
                            h,
                            2.0,
                            Stroke::new(1.0, Color32::BLACK),
                            StrokeKind::Inside,
                        );
                    }
                }
                let (w, h) = screen.pixel_size(sel.rect);
                pill(painter, r, window, &format!("{w} × {h}"));
            }
        }
    }

    /// Loupe près du curseur : pixels grossis et couleur visée.
    fn paint_loupe(&self, painter: &egui::Painter, i: usize, window: Rect, p: Pos2) {
        let screen = &self.screens[i];
        let side = LOUPE_PIXELS as f32 * LOUPE_ZOOM;
        let mut at = p + vec2(24.0, 24.0);
        if at.x + side > window.max.x {
            at.x = p.x - 24.0 - side;
        }
        if at.y + side + 26.0 > window.max.y {
            at.y = p.y - 24.0 - side - 26.0;
        }
        let rect = Rect::from_min_size(at, Vec2::splat(side));
        let (iw, ih) = (
            screen.shot.image.width() as f32,
            screen.shot.image.height() as f32,
        );
        let base = screen.view.to_base(p);
        let cx = (base.x * screen.scale()).floor();
        let cy = (base.y * screen.scale()).floor();
        let half = (LOUPE_PIXELS / 2) as f32;
        let uv = Rect::from_min_max(
            pos2((cx - half) / iw, (cy - half) / ih),
            pos2((cx + half + 1.0) / iw, (cy + half + 1.0) / ih),
        );
        painter.rect_filled(rect.expand(2.0), CornerRadius::same(10), Color32::BLACK);
        if let Some(tex) = &screen.texture {
            painter.image(tex.id(), rect, uv, Color32::WHITE);
        }
        // Pixel visé.
        let cell = Rect::from_min_size(
            rect.min + Vec2::splat(half * LOUPE_ZOOM),
            Vec2::splat(LOUPE_ZOOM),
        );
        painter.rect_stroke(
            cell,
            0.0,
            Stroke::new(1.5, theme::ACCENT_2),
            StrokeKind::Outside,
        );
        painter.rect_stroke(
            rect,
            8.0,
            Stroke::new(2.0, theme::ACCENT_2),
            StrokeKind::Outside,
        );
        if let Some(px) = screen.pixel_at(base) {
            let how = if self.pipette {
                "clic pour copier"
            } else {
                "C pour copier"
            };
            let hex = format!("{}  ·  {how}", color_text(px, self.decimal));
            let font = FontId::new(theme::SIZE_XS + 1.0, egui::FontFamily::Proportional);
            // Fond à la largeur du texte (pastille de couleur + marges), au moins celle
            // de la loupe, et toujours dans l'écran.
            let galley = painter.layout_no_wrap(hex, font, theme::TEXT);
            let width = (galley.size().x + 32.0).max(side);
            let x = rect
                .min
                .x
                .min(window.max.x - width - 4.0)
                .max(window.min.x + 4.0);
            let label = Rect::from_min_size(pos2(x, rect.max.y + 4.0), vec2(width, 20.0));
            painter.rect_filled(label, CornerRadius::same(6), theme::SURFACE);
            let swatch = Rect::from_center_size(
                pos2(label.min.x + 12.0, label.center().y),
                Vec2::splat(10.0),
            );
            painter.rect_filled(swatch, 2.0, Color32::from_rgb(px[0], px[1], px[2]));
            let text_at = pos2(label.min.x + 22.0, label.center().y - galley.size().y / 2.0);
            painter.galley(text_at, galley, theme::TEXT);
        }
    }

    /// Barre d'actions sous la sélection (au-dessus, ou dedans, si pas la place).
    fn action_bar(&mut self, ctx: &egui::Context, i: usize, sel: Rect, window: Rect) {
        const BAR: Vec2 = vec2(430.0, 44.0);
        let mut at = pos2(sel.max.x - BAR.x, sel.max.y + 10.0);
        if at.y + BAR.y > window.max.y {
            at.y = sel.min.y - BAR.y - 10.0;
        }
        if at.y < window.min.y {
            at.y = sel.max.y - BAR.y - 10.0;
        }
        let max_x = (window.max.x - BAR.x - 8.0).max(window.min.x + 8.0);
        at.x = at.x.clamp(window.min.x + 8.0, max_x);
        let mut action = None;
        egui::Area::new(egui::Id::unique(("actions", i)))
            .fixed_pos(at)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let primary = egui::Button::new(
                            egui::RichText::new(format!("{}  Copier", icons::COPY))
                                .color(theme::ON_ACCENT_2),
                        )
                        .fill(theme::ACCENT_2);
                        let copy_hint = format!("Entrée · {} · double-clic", keys::cmd(false, "C"));
                        if ui.add(primary).on_hover_text(copy_hint).clicked() {
                            action = Some(0);
                        }
                        if ui
                            .button(format!("{}  Enregistrer", icons::SAVE))
                            .on_hover_text(keys::cmd(false, "S"))
                            .clicked()
                        {
                            action = Some(1);
                        }
                        if ui
                            .button(format!("{}  Annoter", icons::EDIT))
                            .on_hover_text("E")
                            .clicked()
                        {
                            action = Some(2);
                        }
                        if ui.button(icons::CLOSE).on_hover_text("Échap").clicked() {
                            action = Some(3);
                        }
                    });
                });
            });
        match action {
            Some(0) => self.copy(ctx),
            Some(1) => self.save(ctx),
            Some(2) => self.edit(ctx),
            Some(3) => self.finish(ctx, Outcome::Cancelled),
            _ => {}
        }
    }

    /// Bandeau en bas : consigne, message, ou l'aide complète (H).
    fn hint(&self, ctx: &egui::Context, i: usize, window: Rect) {
        let lines: Vec<String> = if self.show_help {
            help_lines()
        } else {
            vec![match &self.message {
                Some(m) => m.clone(),
                None => self.default_hint(),
            }]
        };
        egui::Area::new(egui::Id::unique(("hint", i)))
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -40.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_max_width(window.width() * 0.8);
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    for line in lines {
                        ui.label(egui::RichText::new(line).size(theme::SIZE_M));
                    }
                });
            });
    }

    /// Consigne du bandeau quand il n'y a pas de message.
    fn default_hint(&self) -> String {
        match (self.flow, self.pipette) {
            (Flow::ColorPicker, _) => {
                "Clique pour copier la couleur  ·  D : hexadécimal ou décimal  ·  Échap : annuler"
                    .into()
            }
            (_, true) => "Pipette : clic pour copier la couleur  ·  D : hexadécimal ou décimal  ·  O : quitter la pipette".into(),
            (Flow::Pick(purpose), false) => format!(
                "{}  ·  clic : la fenêtre encadrée  ·  F : l'écran entier  ·  Échap : annuler",
                purpose_hint(purpose)
            ),
            (Flow::Actions, false) => "Glisse pour sélectionner  ·  clic : la fenêtre encadrée  ·  H : aide  ·  Échap : annuler".into(),
        }
    }

    /// Tout ce qui concerne l'écran `i`, dans sa fenêtre.
    fn screen_ui(&mut self, ui: &mut egui::Ui, i: usize) {
        let ctx = ui.ctx().clone();
        self.screens[i].ensure_textures(&ctx);
        if !self.screens[i].placed {
            let s = &self.screens[i];
            let placed =
                platform::place_overlay_titled(&s.title, screen_rect(&s.shot), s.shot.scale);
            self.screens[i].placed = placed;
            if !placed {
                ctx.request_repaint();
            }
        }
        // Pendant la confirmation de la pipette seule, plus aucune saisie.
        let confirming = self.close_at.is_some();
        if !confirming {
            self.handle_keys(&ctx, i);
        }
        if self.closing {
            return;
        }
        let window = ctx.content_rect();
        if !confirming {
            self.handle_zoom(&ctx, i, window);
            let response = ui.interact(
                window,
                egui::Id::unique(("canvas", i)),
                Sense::click_and_drag(),
            );
            self.handle_pointer(&ctx, i, &response);
        }
        if self.closing {
            return;
        }
        let pointer = ctx.pointer_hover_pos();
        if pointer.is_some() {
            self.hover_screen = Some(i);
        }
        let painter = ui.painter_at(window);
        self.paint(&painter, i, window, pointer);
        let aiming = self.pipette
            || self.selection.is_none()
            || matches!(self.drag, Some((d, Drag::New { .. })) if d == i);
        // Réticules gris : toujours avec la pipette, en option sinon.
        if aiming
            && (self.pipette || self.options.guides)
            && let Some(p) = pointer
        {
            paint_guides(&painter, window, p);
        }
        if aiming && let Some(p) = pointer {
            self.paint_loupe(&painter, i, window, p);
        }
        if let Some(sel) = self.selection
            && sel.screen == i
            && self.drag.is_none()
            && self.flow == Flow::Actions
            && !self.pipette
        {
            let r = self.screens[i].view.rect(sel.rect);
            self.action_bar(&ctx, i, r, window);
        }
        // Consigne sur l'écran visé (ou l'écran principal tant que la souris n'est
        // nulle part).
        let here = pointer.is_some() || (i == self.root && ctx.pointer_latest_pos().is_none());
        if here && (self.selection.is_none() || self.message.is_some() || self.show_help) {
            self.hint(&ctx, i, window);
        }
    }
}

impl eframe::App for Overlay {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.closing {
            return;
        }
        let ctx = ui.ctx().clone();
        if let Some(at) = self.close_at {
            let now = Instant::now();
            if now >= at {
                self.closing = true;
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
                return;
            }
            ctx.request_repaint_after(at - now);
        }
        self.screen_ui(ui, self.root);
        for i in 0..self.screens.len() {
            if i == self.root || self.closing {
                continue;
            }
            let (id, builder) = {
                let s = &self.screens[i];
                (s.viewport, overlay_builder(&s.shot, &s.title))
            };
            ctx.show_viewport_immediate(id, builder, |ui, _class| self.screen_ui(ui, i));
        }
    }
}

fn color_image(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    )
}

/// Étiquette lime au-dessus du rectangle `r` (dedans si pas la place).
fn pill(painter: &egui::Painter, r: Rect, window: Rect, text: &str) {
    let font = FontId::new(theme::SIZE_S, egui::FontFamily::Proportional);
    let galley = painter.layout_no_wrap(text.to_owned(), font, theme::ON_ACCENT_2);
    let pad = vec2(8.0, 4.0);
    let size = galley.size() + pad * 2.0;
    let mut at = pos2(r.min.x.max(window.min.x + 4.0), r.min.y - size.y - 6.0);
    if at.y < window.min.y {
        at = r.min + vec2(6.0, 6.0);
    }
    at.x = at.x.min(window.max.x - size.x - 4.0);
    let rect = Rect::from_min_size(at, size);
    painter.rect_filled(rect, CornerRadius::same(6), theme::ACCENT_2);
    painter.galley(rect.min + pad, galley, theme::ON_ACCENT_2);
}

/// Contenu de l'aide (H).
fn help_lines() -> Vec<String> {
    vec![
        "Glisser : sélectionner une zone  ·  clic : la fenêtre encadrée".into(),
        "Clic tout en haut de l'écran ou sur le bureau : l'écran entier".into(),
        "Poignées : redimensionner  ·  flèches : déplacer (Maj : ×10)".into(),
        "Molette : zoomer  ·  0 : taille réelle".into(),
        "M : inclure le curseur  ·  C : copier la couleur visée".into(),
        "F : l'écran entier  ·  O : pipette  ·  D : couleurs en hexadécimal ou décimal".into(),
        format!(
            "Entrée, {} ou double-clic : copier  ·  {} : enregistrer  ·  E : annoter",
            keys::cmd(false, "C"),
            keys::cmd(false, "S")
        ),
        "Échap : annuler  ·  H : masquer l'aide".into(),
    ]
}

// --- Lot « Capture » ---

/// Réticules gris (Screenpresso) : une ligne horizontale et une verticale sur tout
/// l'écran, à la position de la souris.
fn paint_guides(painter: &egui::Painter, window: Rect, p: Pos2) {
    let guide = Stroke::new(1.0, Color32::from_rgba_unmultiplied(190, 190, 190, 150));
    painter.hline(window.x_range(), p.y, guide);
    painter.vline(p.x, window.y_range(), guide);
}

/// Couleur d'un pixel : `#RRGGBB`, ou `rgb(r, g, b)` en décimal.
fn color_text(px: [u8; 4], decimal: bool) -> String {
    let [r, g, b, _] = px;
    if decimal {
        format!("rgb({r}, {g}, {b})")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}")
    }
}

/// Consigne du bandeau en mode [`pick`].
fn purpose_hint(purpose: Purpose) -> &'static str {
    match purpose {
        Purpose::Ocr => "Sélectionne le texte à copier",
        Purpose::Qr => "Sélectionne le QR code",
        Purpose::Video => "Sélectionne la zone à filmer",
        Purpose::Scrolling => "Sélectionne la zone à faire défiler",
    }
}

/// Zone `rect` (points de base) de l'écran `screen` en [`Picked`]. `None` si vide.
fn picked_from(
    shot: &Shot,
    screen: usize,
    rect: Rect,
    with_cursor: bool,
    window: Option<(String, String)>,
) -> Option<Picked> {
    let (iw, ih) = shot.image.dimensions();
    let (x, y, width, height) = pixel_rect(rect, shot.scale as f32, iw, ih)?;
    let (app, title) = window.unzip();
    Some(Picked {
        image: shot.region(x, y, width, height, with_cursor),
        screen,
        display_id: shot.display_id,
        frame: shot.frame,
        scale: shot.scale,
        x,
        y,
        width,
        height,
        app,
        title,
    })
}

/// Sélection en pixels, bornée à l'image. `None` si vide.
fn pixel_rect(sel: Rect, scale: f32, width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let x0 = (sel.min.x * scale).round().clamp(0.0, width as f32) as u32;
    let y0 = (sel.min.y * scale).round().clamp(0.0, height as f32) as u32;
    let x1 = (sel.max.x * scale).round().clamp(0.0, width as f32) as u32;
    let y1 = (sel.max.y * scale).round().clamp(0.0, height as f32) as u32;
    (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
}

/// Ramène `r` dans `bounds` sans changer sa taille (si elle y tient).
fn keep_inside(r: Rect, bounds: Rect) -> Rect {
    let dx = (bounds.min.x - r.min.x).max(0.0) + (bounds.max.x - r.max.x).min(0.0);
    let dy = (bounds.min.y - r.min.y).max(0.0) + (bounds.max.y - r.max.y).min(0.0);
    r.translate(vec2(dx, dy))
}

/// Les 8 poignées : centre et bords déplacés.
fn handles(sel: Rect) -> [(Pos2, Edges); 8] {
    let e = |left, right, top, bottom| Edges {
        left,
        right,
        top,
        bottom,
    };
    let (c, mn, mx) = (sel.center(), sel.min, sel.max);
    [
        (mn, e(true, false, true, false)),
        (pos2(c.x, mn.y), e(false, false, true, false)),
        (pos2(mx.x, mn.y), e(false, true, true, false)),
        (pos2(mx.x, c.y), e(false, true, false, false)),
        (mx, e(false, true, false, true)),
        (pos2(c.x, mx.y), e(false, false, false, true)),
        (pos2(mn.x, mx.y), e(true, false, false, true)),
        (pos2(mn.x, c.y), e(true, false, false, false)),
    ]
}

fn handle_at(sel: Rect, p: Pos2) -> Option<Edges> {
    handles(sel)
        .into_iter()
        .find(|(c, _)| (p - *c).abs().max_elem() <= HANDLE)
        .map(|(_, e)| e)
}

/// `start` dont les bords `edges` sont décalés de `delta` ; reste un rectangle valide
/// même si on dépasse le bord opposé.
fn resize(start: Rect, edges: Edges, delta: Vec2) -> Rect {
    let mut min = start.min;
    let mut max = start.max;
    if edges.left {
        min.x += delta.x;
    }
    if edges.right {
        max.x += delta.x;
    }
    if edges.top {
        min.y += delta.y;
    }
    if edges.bottom {
        max.y += delta.y;
    }
    Rect::from_two_pos(min, max)
}

fn resize_cursor(e: Edges) -> CursorIcon {
    match (e.left || e.right, e.top || e.bottom) {
        (true, true) if e.left == e.top => CursorIcon::ResizeNwSe,
        (true, true) => CursorIcon::ResizeNeSw,
        (true, false) => CursorIcon::ResizeHorizontal,
        _ => CursorIcon::ResizeVertical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Frame, WindowRect};

    #[test]
    fn selection_is_converted_to_pixels() {
        let sel = Rect::from_min_max(pos2(10.0, 20.0), pos2(110.25, 70.0));
        assert_eq!(pixel_rect(sel, 2.0, 4000, 3000), Some((20, 40, 201, 100)));
        // Débordement : borné à l'image.
        let sel = Rect::from_min_max(pos2(-5.0, -5.0), pos2(5.0, 5.0));
        assert_eq!(pixel_rect(sel, 2.0, 100, 100), Some((0, 0, 10, 10)));
        let empty = Rect::from_min_max(pos2(3.0, 3.0), pos2(3.0, 9.0));
        assert_eq!(pixel_rect(empty, 2.0, 100, 100), None);
    }

    #[test]
    fn moved_selection_stays_on_screen() {
        let screen = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
        let r = Rect::from_min_size(pos2(90.0, -5.0), vec2(20.0, 20.0));
        assert_eq!(
            keep_inside(r, screen),
            Rect::from_min_size(pos2(80.0, 0.0), vec2(20.0, 20.0))
        );
    }

    #[test]
    fn resizing_past_the_opposite_edge_flips() {
        let start = Rect::from_min_max(pos2(10.0, 10.0), pos2(50.0, 50.0));
        let left = Edges {
            left: true,
            right: false,
            top: false,
            bottom: false,
        };
        let r = resize(start, left, vec2(60.0, 999.0));
        assert_eq!(r, Rect::from_min_max(pos2(50.0, 10.0), pos2(70.0, 50.0)));
    }

    #[test]
    fn handles_are_found_near_corners_and_edges() {
        let sel = Rect::from_min_max(pos2(10.0, 10.0), pos2(110.0, 60.0));
        let e = handle_at(sel, pos2(112.0, 58.0)).unwrap();
        assert!(e.right && e.bottom && !e.left && !e.top);
        let e = handle_at(sel, pos2(60.0, 9.0)).unwrap();
        assert!(e.top && !e.left && !e.right);
        assert_eq!(handle_at(sel, pos2(60.0, 35.0)), None);
        let nw = Edges {
            left: true,
            right: false,
            top: true,
            bottom: false,
        };
        assert_eq!(resize_cursor(nw), CursorIcon::ResizeNwSe);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor_and_covers_the_window() {
        let size = vec2(1000.0, 600.0);
        let at = pos2(250.0, 150.0);
        let v = View::default().zoomed(2.0, at, size);
        assert_eq!(v.zoom, 2.0);
        assert!((v.to_screen(View::default().to_base(at)) - at).length() < 1e-3);
        // Zoom près du bord : l'image ne laisse jamais de vide.
        let v = View::default().zoomed(2.0, pos2(0.0, 0.0), size);
        assert_eq!(v.offset, Vec2::ZERO);
        let v = v.zoomed(0.1, pos2(500.0, 300.0), size);
        assert_eq!(v, View::default(), "jamais en dessous de 1");
        let base = pos2(123.0, 45.0);
        let v = View::default().zoomed(3.0, pos2(400.0, 300.0), size);
        assert!((v.to_base(v.to_screen(base)) - base).length() < 1e-3);
    }

    #[test]
    fn the_frontmost_window_under_the_pointer_wins() {
        let win = |x, y, w, h, app: &str| WindowRect {
            x,
            y,
            width: w,
            height: h,
            title: String::new(),
            app: app.into(),
        };
        let shot = Shot {
            image: RgbaImage::new(400, 200),
            frame: Frame {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 100.0,
            },
            scale: 2.0,
            display_id: 1,
            // Devant : une petite fenêtre ; derrière : une grande qui déborde de l'écran.
            windows: vec![
                win(20, 20, 100, 60, "Notes"),
                win(-50, 0, 600, 300, "Safari"),
            ],
            cursor: None,
        };
        let screen = Screen {
            texture: None,
            shot,
            cursor: None,
            viewport: ViewportId::ROOT,
            title: title(0),
            view: View::default(),
            placed: true,
        };
        let (r, label) = screen.window_at(pos2(20.0, 20.0)).unwrap();
        assert_eq!(label, "Notes");
        assert_eq!(r, Rect::from_min_size(pos2(10.0, 10.0), vec2(50.0, 30.0)));
        let (r, label) = screen.window_at(pos2(150.0, 90.0)).unwrap();
        assert_eq!(label, "Safari");
        assert_eq!(r, screen.base_rect(), "rognée à l'écran");
    }

    #[test]
    fn colors_are_written_in_hex_or_decimal() {
        assert_eq!(color_text([255, 8, 171, 255], false), "#FF08AB");
        assert_eq!(color_text([0, 0, 0, 0], false), "#000000");
        assert_eq!(color_text([255, 8, 171, 255], true), "rgb(255, 8, 171)");
    }

    #[test]
    fn each_purpose_has_its_hint() {
        assert_eq!(purpose_hint(Purpose::Ocr), "Sélectionne le texte à copier");
        assert_eq!(purpose_hint(Purpose::Qr), "Sélectionne le QR code");
        assert_eq!(purpose_hint(Purpose::Video), "Sélectionne la zone à filmer");
        assert_eq!(
            purpose_hint(Purpose::Scrolling),
            "Sélectionne la zone à faire défiler"
        );
    }

    #[test]
    fn picked_zone_is_in_image_pixels() {
        let image = RgbaImage::from_fn(400, 200, |x, y| image::Rgba([x as u8, y as u8, 0, 255]));
        let red = image::Rgba([255, 0, 0, 255]);
        let shot = Shot {
            image,
            frame: Frame {
                x: -200.0,
                y: 0.0,
                width: 200.0,
                height: 100.0,
            },
            scale: 2.0,
            display_id: 7,
            windows: Vec::new(),
            cursor: Some(crate::capture::CursorPatch {
                image: RgbaImage::from_pixel(2, 2, red),
                x: 22,
                y: 42,
            }),
        };
        // Points de base (10, 20)–(60, 45) → pixels (20, 40) 100 × 50.
        let rect = Rect::from_min_max(pos2(10.0, 20.0), pos2(60.0, 45.0));
        let window = Some(("Safari".to_owned(), "Accueil".to_owned()));
        let p = picked_from(&shot, 1, rect, false, window).unwrap();
        assert_eq!((p.x, p.y, p.width, p.height), (20, 40, 100, 50));
        assert_eq!(p.image.dimensions(), (100, 50));
        assert_eq!(p.image.get_pixel(0, 0).0, [20, 40, 0, 255]);
        assert_eq!((p.screen, p.display_id, p.scale), (1, 7, 2.0));
        assert_eq!(p.frame, shot.frame);
        assert_eq!(
            (p.app.as_deref(), p.title.as_deref()),
            (Some("Safari"), Some("Accueil"))
        );
        // Curseur demandé : posé dans la zone ; zone tirée à l'envers : même résultat.
        let reversed = Rect::from_two_pos(pos2(60.0, 45.0), pos2(10.0, 20.0));
        let p = picked_from(&shot, 1, reversed, true, None).unwrap();
        assert_eq!(*p.image.get_pixel(2, 2), red);
        assert_eq!((p.app, p.title), (None, None));
        // Zone vide.
        let empty = Rect::from_min_max(pos2(10.0, 20.0), pos2(10.0, 45.0));
        assert!(picked_from(&shot, 0, empty, false, None).is_none());
    }

    #[test]
    fn window_labels_are_short() {
        assert_eq!(window_label("", ""), "Fenêtre");
        assert_eq!(window_label("Safari", "Accueil"), "Safari — Accueil");
        assert_eq!(window_label("Code", "main.rs — Code"), "main.rs — Code");
        assert_eq!(window_label("Finder", " "), "Finder");
    }
}

/// Pourquoi on sélectionne une zone sans passer par les actions habituelles
/// (copier, enregistrer, éditer) : la zone part directement à une autre fonction.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Ocr,
    Qr,
    Video,
    Scrolling,
}

/// Zone choisie avec [`pick`].
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Picked {
    /// Pixels de la zone (curseur compris si demandé).
    pub image: RgbaImage,
    /// Écran de la zone (indice dans `Capture::shots`) et ses caractéristiques.
    pub screen: usize,
    pub display_id: u32,
    pub frame: crate::capture::Frame,
    pub scale: f64,
    /// Zone en pixels de l'image de l'écran.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Fenêtre sous la zone, si elle a été choisie d'un clic : application et titre.
    pub app: Option<String>,
    pub title: Option<String>,
}

/// Sélection d'une zone sur les écrans figés, rendue dès que l'utilisateur relâche
/// la souris (ou clique une fenêtre, ou appuie sur F pour l'écran entier), sans
/// barre d'actions. Le bandeau du bas rappelle le but (`purpose`). `None` : annulé
/// (Échap). Rien n'est enregistré dans l'historique.
#[allow(dead_code)]
pub fn pick(
    store: Store,
    capture: Capture,
    with_cursor: bool,
    purpose: Purpose,
) -> Result<Option<Picked>> {
    pick_with(
        store,
        capture,
        Options {
            with_cursor,
            ..Options::default()
        },
        purpose,
    )
}

/// [`pick`] avec toutes les options (réticules…).
#[allow(dead_code)]
pub fn pick_with(
    store: Store,
    capture: Capture,
    options: Options,
    purpose: Purpose,
) -> Result<Option<Picked>> {
    Ok(
        match launch(store, capture, options, Flow::Pick(purpose))? {
            Some(Exit::Picked(picked)) => Some(*picked),
            _ => None,
        },
    )
}
