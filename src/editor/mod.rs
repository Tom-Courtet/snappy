//! Éditeur d'annotations : `snappy --edit <capture.png>`, une fenêtre egui dans son
//! propre processus (sa mémoire est rendue au système dès qu'on la ferme).
//!
//! L'image d'origine n'est jamais modifiée : les annotations sont enregistrées à côté
//! (`.json`) après chaque changement, et « Copier » / « Enregistrer » exportent une
//! image aplatie.
//!
//! Outils (touche) : V sélection, A flèche, L ligne, R rectangle, O ellipse,
//! H surligneur, F projecteur, B flou, X pixeliser, P crayon, T texte, U bulle,
//! N pastille, M loupe, K tampon. 1 2 3 : taille.
//!
//! Annotation sélectionnée : poignées pour l'éditer (bouts et milieu des flèches,
//! coins, pointes), ⌘D ou Alt+glisser pour la dupliquer, ⌘[ / ⌘] pour l'ordre, clic
//! droit pour le menu, ⌫ pour la supprimer. Alt+clic : pipette. ⌘Z / ⌘⇧Z ;
//! ⌘C copier ; ⌘S enregistrer ; ⌘0 ajuster ; ⌘1 taille réelle.
//!
//! Ajouts du lot « annotations » (voir `extras.rs`) : G polygone, J accolade ;
//! Maj+clic et rectangle de sélection pour plusieurs annotations (aligner,
//! répartir) ; ⌘X / ⌘V couper / coller des annotations ou une image ; image glissée
//! sur le canevas ; PgPréc / PgSuiv image précédente / suivante ; ⌘O ouvrir ; F11
//! plein écran.

mod bars;
mod extras;
mod image_tab;
mod links;
mod prefs;
mod preview;
mod tools;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::{
    Color32, CornerRadius, CursorIcon, Frame, Key, Margin, Modifiers, Rect, Sense, Stroke,
    TextEdit, TextureHandle, TextureOptions, Ui, pos2, vec2,
};
use image::RgbaImage;

use crate::config::Store;
use crate::history;
use crate::model::{Annotation, Doc, Pt, Rgba, Shape, bubble_box, text_padding};
use crate::platform;
use crate::render;
use crate::ui::{icons, theme};
use prefs::{Prefs, Preset};
use preview::View;
use tools::{Metrics, Style, TOOLS, Tool, extend_shape, new_shape};

pub fn run(store: Store, path: PathBuf) -> Result<()> {
    let base = history::read_png(&path)?;
    let doc = history::load_doc(&path);
    let base = crate::effects::working_base(base, &doc.edits);
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("capture")
        .to_owned();
    // Fenêtre à la taille de l'image (Retina : 2 pixels par point), bornée.
    let density = if cfg!(target_os = "macos") { 2.0 } else { 1.0 };
    let w = (base.width() as f32 / density + 48.0).clamp(900.0, 1500.0);
    let h = (base.height() as f32 / density + 170.0).clamp(560.0, 980.0);
    let viewport = egui::ViewportBuilder::default()
        .with_title(format!("Snappy · {name}"))
        .with_inner_size([w, h])
        .with_min_inner_size([760.0, 460.0])
        .with_drag_and_drop(true)
        .with_icon(window_icon());
    let options = eframe::NativeOptions {
        viewport,
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "Snappy",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            platform::activate(cc);
            Ok(Box::new(Editor::new(store, path, base, doc)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("éditeur : {e}"))
}

/// Icône de la fenêtre (et du Dock sous macOS, d'où la taille de 256 px).
fn window_icon() -> egui::IconData {
    #[cfg(target_os = "macos")]
    let (rgba, size): (&[u8], u32) = (include_bytes!("../../assets/icon_mac_256.rgba"), 256);
    #[cfg(not(target_os = "macos"))]
    let (rgba, size): (&[u8], u32) = (include_bytes!("../../assets/icon_64.rgba"), 64);
    egui::IconData {
        rgba: rgba.to_vec(),
        width: size,
        height: size,
    }
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    /// Tracé de la nouvelle annotation `index`.
    Create { index: usize },
    /// Déplacement de l'annotation `index`.
    Move { index: usize, last: Pt, moved: bool },
    /// Poignée `handle` de l'annotation `index`.
    Handle {
        index: usize,
        handle: usize,
        moved: bool,
    },
    // --- Lot « Éditeur : annotations »
    /// Rectangle de sélection (outil Sélection, glissé dans le vide).
    Marquee { from: Pt, to: Pt },
    /// Zone à copier (« Copier une partie de l'image »).
    Grab { from: Pt, to: Pt },
}

/// Texte en cours de saisie : texte simple (sur fond ou non) ou bulle.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TextKind {
    Plain { boxed: bool },
    Bubble { tail: Pt },
}

/// Texte en cours de saisie, posé sur l'image.
struct TextEditing {
    /// Annotation modifiée, `None` pour un nouveau texte.
    index: Option<usize>,
    at: Pt,
    text: String,
    size: f32,
    color: Rgba,
    shadow: bool,
    kind: TextKind,
    focus: bool,
    /// Options de l'annotation (gras, lueur, type de bulle…), gardées à la validation.
    template: Annotation,
}

pub(crate) struct Editor {
    store: Store,
    path: PathBuf,
    base: RgbaImage,
    doc: Doc,
    undo: Vec<Vec<Annotation>>,
    redo: Vec<Vec<Annotation>>,
    tool: Tool,
    style: Style,
    presets: Vec<Preset>,
    prefs_path: PathBuf,
    selected: Option<usize>,
    drag: Option<Drag>,
    text: Option<TextEditing>,
    texture: Option<TextureHandle>,
    /// Rendu affiché (pour la pipette).
    composite: Option<RgbaImage>,
    /// Annotations absentes du rendu affiché, `None` : à refaire.
    rendered: Option<Vec<usize>>,
    /// `None` : image ajustée à la fenêtre.
    zoom: Option<f32>,
    status: Option<(String, Instant)>,
    last_export: Option<PathBuf>,
    /// Pixels de l'image par point d'écran (2 sur un écran Retina).
    density: f32,
    /// Pipette armée : le prochain clic prend la couleur.
    picking: bool,
    /// Champ hexadécimal de la couleur personnalisée.
    hex_input: String,
    /// Lot « Éditeur : image et effets » : fenêtre « Image » et modes.
    image: image_tab::ImageTab,
    // --- Lot « Éditeur : annotations »
    /// Autres annotations sélectionnées, en plus de `selected` (valables tant que
    /// `selected` vaut `multi_for`).
    multi: Vec<usize>,
    multi_for: Option<usize>,
    /// Polygone en cours de tracé : son dernier sommet suit la souris.
    poly: Option<usize>,
    /// Annotations copiées (menu Couper / Copier, ⌘X), le dossier de leurs images,
    /// et l'empreinte de l'image du presse-papiers système à ce moment-là.
    clip: Vec<Annotation>,
    clip_assets: Option<PathBuf>,
    clip_mark: Option<u64>,
    pastes: u32,
    tool_presets: BTreeMap<Tool, Vec<tools::Style>>,
    validate_with_path: bool,
    /// « Continuer la numérotation depuis cette valeur » : prochaine pastille imposée.
    step_from: Option<u32>,
    /// « Copier une partie de l'image » : le prochain glisser choisit la zone.
    grab_part: bool,
    /// Demande de confirmation avant de mettre l'image à la corbeille.
    confirm_delete: bool,
    /// Marqueur posé dans le presse-papiers interne d'egui, pour recevoir ⌘V.
    paste_marker: bool,
    /// Hauteur du bandeau des propriétés au-dessus de l'image (pixels).
    banner: f32,
    /// Échelle d'affichage du dernier rendu (curseur de zoom).
    shown_scale: f32,
}

/// Historique d'annulation maximal.
const UNDO_LIMIT: usize = 200;
/// Tolérance de sélection, en points d'écran.
const HIT_TOLERANCE: f32 = 6.0;
const STATUS_DURATION: Duration = Duration::from_secs(4);

impl Editor {
    fn new(store: Store, path: PathBuf, base: RgbaImage, doc: Doc) -> Self {
        let prefs_path = Prefs::path(&store);
        let prefs = Prefs::load(&prefs_path);
        Self {
            hex_input: prefs.style.color.hex(),
            store,
            path,
            base,
            doc,
            undo: Vec::new(),
            redo: Vec::new(),
            tool: prefs.tool,
            style: prefs.style,
            presets: prefs.presets,
            prefs_path,
            selected: None,
            drag: None,
            text: None,
            texture: None,
            composite: None,
            rendered: None,
            zoom: None,
            status: None,
            last_export: None,
            density: 2.0,
            picking: false,
            image: Default::default(),
            multi: Vec::new(),
            multi_for: None,
            poly: None,
            clip: Vec::new(),
            clip_assets: None,
            clip_mark: None,
            pastes: 0,
            tool_presets: prefs.tool_presets,
            validate_with_path: prefs.validate_with_path,
            step_from: None,
            grab_part: false,
            confirm_delete: false,
            paste_marker: false,
            banner: 0.0,
            shown_scale: 1.0,
        }
    }

    fn metrics(&self) -> Metrics {
        self.style.metrics(self.density)
    }

    fn say(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), Instant::now()));
    }

    fn save_prefs(&self) {
        Prefs {
            tool: self.tool,
            style: self.style.clone(),
            presets: self.presets.clone(),
            tool_presets: self.tool_presets.clone(),
            validate_with_path: self.validate_with_path,
        }
        .save(&self.prefs_path);
    }

    // --- Historique -------------------------------------------------------------

    /// À appeler avant toute modification des annotations.
    fn checkpoint(&mut self) {
        self.undo.push(self.doc.annotations.clone());
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Après une modification : nouveau rendu et sauvegarde des annotations.
    fn changed(&mut self) {
        self.rendered = None;
        if let Err(e) = history::save_doc(&self.path, &self.doc) {
            self.say(format!("Annotations non enregistrées : {e:#}"));
        }
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo
                .push(std::mem::replace(&mut self.doc.annotations, prev));
            self.selected = None;
            self.changed();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo
                .push(std::mem::replace(&mut self.doc.annotations, next));
            self.selected = None;
            self.changed();
        }
    }

    fn select(&mut self, index: Option<usize>) {
        self.multi.clear();
        self.selected = index.filter(|&i| i < self.doc.annotations.len());
        if let Some(i) = self.selected {
            let ann = self.doc.annotations[i].clone();
            self.style.adopt(&ann);
            self.hex_input = self.style.color.hex();
        }
    }

    fn delete_selected(&mut self) {
        if self.selection().len() > 1 {
            self.delete_many();
            return;
        }
        if let Some(i) = self.selected.take()
            && i < self.doc.annotations.len()
        {
            self.checkpoint();
            self.doc.annotations.remove(i);
            self.changed();
        }
    }

    /// Copie décalée de l'annotation sélectionnée, sélectionnée à son tour.
    fn duplicate_selected(&mut self) {
        if self.selection().len() > 1 {
            self.duplicate_many();
            return;
        }
        let Some(i) = self.selected else { return };
        let Some(mut copy) = self.doc.annotations.get(i).cloned() else {
            return;
        };
        self.checkpoint();
        let d = 12.0 * self.density;
        copy.translate(d, d);
        self.doc.annotations.push(copy);
        self.selected = Some(self.doc.annotations.len() - 1);
        self.changed();
    }

    /// Passe l'annotation sélectionnée tout devant (`true`) ou tout derrière.
    fn reorder_selected(&mut self, to_front: bool) {
        let Some(i) = self.selected else { return };
        let n = self.doc.annotations.len();
        let target = if to_front { n - 1 } else { 0 };
        if i >= n || i == target {
            return;
        }
        self.checkpoint();
        let ann = self.doc.annotations.remove(i);
        self.doc.annotations.insert(target, ann);
        self.selected = Some(target);
        self.changed();
    }

    /// Le style vient de changer : il s'applique aussi à l'annotation sélectionnée
    /// (et au texte en cours de saisie).
    fn style_changed(&mut self) {
        self.hex_input = self.style.color.hex();
        let m = self.metrics();
        if let Some(edit) = &mut self.text {
            edit.color = self.style.color;
            edit.size = m.text;
            edit.shadow = self.style.shadow;
            if let TextKind::Plain { boxed } = &mut edit.kind {
                *boxed = self.style.boxed;
            }
        }
        if let Some(edit) = &mut self.text {
            self.style.apply_options(&mut edit.template);
        }
        if self.selection().len() > 1 {
            self.restyle_many();
            return;
        }
        let Some(i) = self.selected else { return };
        let Some(ann) = self.doc.annotations.get(i) else {
            return;
        };
        let mut new = ann.clone();
        self.style.apply(&mut new, m);
        if &new != ann {
            self.checkpoint();
            self.doc.annotations[i] = new;
            self.changed();
        }
    }

    /// Couleur sous le point `p` de l'image affichée (pipette).
    fn pick_color(&mut self, p: Pt) {
        // Le rendu affiché commence par le bandeau des propriétés, s'il y en a un.
        let y = p.y + self.banner;
        let px = self.composite.as_ref().and_then(|img| {
            (p.x >= 0.0 && y >= 0.0)
                .then(|| img.get_pixel_checked(p.x as u32, y as u32))
                .flatten()
        });
        if let Some(px) = px {
            self.style.color = Rgba::rgb(px[0], px[1], px[2]);
            self.style_changed();
            self.say(format!(
                "{} Couleur {}",
                icons::PIPETTE,
                self.style.color.hex()
            ));
        }
        self.picking = false;
    }

    // --- Export -----------------------------------------------------------------

    fn flattened(&mut self) -> RgbaImage {
        self.commit_text();
        self.finish_polygon();
        crate::effects::apply_export_with(
            &self.base,
            &self.doc,
            Some(&render::assets_dir(&self.path)),
        )
    }

    fn copy(&mut self) {
        let img = self.flattened();
        match history::copy_image(&img) {
            Ok(()) => self.say(format!("{} Copié dans le presse-papiers", icons::OK)),
            Err(e) => self.say(format!("{} {e:#}", icons::WARNING)),
        }
    }

    fn save(&mut self) {
        let img = self.flattened();
        let dir = self.store.save_dir(&self.store.load());
        match history::export(&dir, &self.path, &img) {
            Ok(path) => {
                crate::effects::write_dpi(&path, self.doc.dpi);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                self.say(format!(
                    "{} Enregistré : {}",
                    icons::OK,
                    name.unwrap_or_default()
                ));
                self.last_export = Some(path);
            }
            Err(e) => self.say(format!("{} {e:#}", icons::WARNING)),
        }
    }

    // --- Texte et bulles --------------------------------------------------------

    /// Ouvre la saisie : nouveau texte (`index` vide) ou annotation existante.
    fn start_text(&mut self, at: Pt, index: Option<usize>, kind: TextKind) {
        self.commit_text();
        let m = self.metrics();
        let existing = index.and_then(|i| self.doc.annotations.get(i)).cloned();
        let template = existing.clone();
        let edit = match existing {
            Some(Annotation {
                shape:
                    Shape::Text {
                        at,
                        text,
                        size,
                        boxed,
                    },
                color,
                shadow,
                ..
            }) => TextEditing {
                index,
                at,
                text,
                size,
                color,
                shadow,
                kind: TextKind::Plain { boxed },
                focus: true,
                template: template
                    .clone()
                    .unwrap_or_else(|| self.style.annotate(Shape::Pen { points: Vec::new() }, m)),
            },
            Some(Annotation {
                shape:
                    Shape::Bubble {
                        at,
                        text,
                        size,
                        tail,
                    },
                color,
                shadow,
                ..
            }) => TextEditing {
                index,
                at,
                text,
                size,
                color,
                shadow,
                kind: TextKind::Bubble { tail },
                focus: true,
                template: template
                    .clone()
                    .unwrap_or_else(|| self.style.annotate(Shape::Pen { points: Vec::new() }, m)),
            },
            _ => TextEditing {
                index: None,
                at,
                text: String::new(),
                size: m.text,
                color: self.style.color,
                shadow: self.style.shadow,
                kind,
                focus: true,
                template: self.style.annotate(
                    match kind {
                        TextKind::Plain { .. } => Shape::Text {
                            at,
                            text: String::new(),
                            size: m.text,
                            boxed: false,
                        },
                        TextKind::Bubble { tail } => Shape::Bubble {
                            at,
                            text: String::new(),
                            size: m.text,
                            tail,
                        },
                    },
                    m,
                ),
            },
        };
        self.text = Some(edit);
        self.selected = None;
        self.rendered = None;
    }

    /// Termine la saisie en cours (texte vide : rien, ou suppression du texte modifié).
    fn commit_text(&mut self) {
        let Some(edit) = self.text.take() else { return };
        self.rendered = None;
        let text = edit.text.trim_end().to_owned();
        let shape = match edit.kind {
            TextKind::Plain { boxed } => Shape::Text {
                at: edit.at,
                text,
                size: edit.size,
                boxed,
            },
            TextKind::Bubble { tail } => Shape::Bubble {
                at: edit.at,
                text,
                size: edit.size,
                tail,
            },
        };
        let mut ann = Annotation::new(shape, edit.color, self.metrics().stroke);
        ann.shadow = edit.shadow;
        // Options du lot « annotations » : celles du texte modifié (ou du style).
        ann.opacity = edit.template.opacity;
        ann.text_style = edit.template.text_style;
        if matches!(ann.shape, Shape::Bubble { .. }) {
            ann.bubble = edit.template.bubble;
        }
        match edit.index {
            Some(i) if i < self.doc.annotations.len() => {
                if self.doc.annotations[i] != ann {
                    self.checkpoint();
                    if ann.is_degenerate() {
                        self.doc.annotations.remove(i);
                    } else {
                        self.doc.annotations[i] = ann;
                    }
                    self.changed();
                }
            }
            _ if !ann.is_degenerate() => {
                self.checkpoint();
                self.doc.annotations.push(ann);
                self.changed();
            }
            _ => {}
        }
    }

    /// Clic avec l'outil texte ou bulle : modifie celui qu'on touche, sinon en
    /// crée un, centré verticalement sur le clic.
    fn text_click(&mut self, pt: Pt, scale: f32, bubble: bool) {
        let m = self.metrics();
        let existing = self.hit(pt, scale).filter(|&i| {
            matches!(
                self.doc.annotations[i].shape,
                Shape::Text { .. } | Shape::Bubble { .. }
            )
        });
        if let Some(i) = existing {
            self.start_text(Pt::default(), Some(i), TextKind::Plain { boxed: false });
            return;
        }
        let at = Pt::new(pt.x, pt.y - m.text * 0.6);
        let kind = if bubble {
            // Pointe en bas à gauche de la bulle.
            let tail = Pt::new(at.x - m.text * 0.3, at.y + m.text * 3.0);
            TextKind::Bubble { tail }
        } else {
            TextKind::Plain {
                boxed: self.style.boxed,
            }
        };
        self.start_text(at, None, kind);
    }

    // --- Rendu affiché ----------------------------------------------------------

    /// Annotation retirée du rendu exact (dessinée par l'aperçu ou en cours de saisie).
    fn hidden(&self) -> Option<usize> {
        match (self.drag, &self.text) {
            (
                Some(
                    Drag::Create { index } | Drag::Move { index, .. } | Drag::Handle { index, .. },
                ),
                _,
            ) => Some(index),
            (None, Some(TextEditing { index: Some(i), .. })) => Some(*i),
            _ => None,
        }
    }

    fn refresh_texture(&mut self, ctx: &egui::Context) {
        let hidden = self.hidden_all();
        if self.rendered.as_ref() == Some(&hidden) && self.texture.is_some() {
            return;
        }
        let shown: Vec<Annotation> = self
            .doc
            .annotations
            .iter()
            .enumerate()
            .filter(|(i, _)| !hidden.contains(i))
            .map(|(_, a)| a.clone())
            .collect();
        // Le document tel qu'enregistré (bandeau compris), moins ce qui est masqué.
        let mut doc = self.doc.clone();
        doc.annotations = shown;
        self.banner = render::banner_height(&doc, self.base.width()) as f32;
        let img = render::render_doc(&self.base, &doc, Some(&render::assets_dir(&self.path)));
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        );
        // Linéaire : l'image est souvent réduite pour tenir dans la fenêtre.
        match &mut self.texture {
            Some(t) => t.set(color, TextureOptions::LINEAR),
            None => {
                self.texture = Some(ctx.load_texture("rendu", color, TextureOptions::LINEAR));
            }
        }
        self.composite = Some(img);
        self.rendered = Some(hidden);
    }

    // --- Clavier ----------------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        enum Act {
            Tool(Tool),
            Size(tools::Size),
            Undo,
            Redo,
            Delete,
            Escape,
            Copy,
            Save,
            Fit,
            Actual,
            Duplicate,
            Front,
            Back,
            // Lot « Éditeur : annotations » (raccourcis jusque-là libres).
            Cut,
            Paste,
            Previous,
            Next,
            Open,
            Fullscreen,
            FinishPolygon,
        }
        let mut acts = Vec::new();
        let drawing_polygon = self.poly.is_some();
        ctx.input_mut(|i| {
            if i.events.iter().any(|e| matches!(e, egui::Event::Copy)) {
                acts.push(Act::Copy);
            }
            // ⌘X et ⌘V n'arrivent que sous forme d'évènements (voir `extras.rs`).
            if i.events.iter().any(|e| matches!(e, egui::Event::Cut)) {
                acts.push(Act::Cut);
            }
            if i.events
                .iter()
                .any(|e| matches!(e, egui::Event::Paste(_) | egui::Event::PasteImage(_)))
            {
                acts.push(Act::Paste);
            }
            for (mods, key, act) in [
                (Modifiers::NONE, Key::PageUp, Act::Previous),
                (Modifiers::NONE, Key::PageDown, Act::Next),
                (Modifiers::COMMAND, Key::O, Act::Open),
                (Modifiers::NONE, Key::F11, Act::Fullscreen),
            ] {
                if i.consume_key(mods, key) {
                    acts.push(act);
                }
            }
            if drawing_polygon && i.consume_key(Modifiers::NONE, Key::Enter) {
                acts.push(Act::FinishPolygon);
            }
            let cmd = Modifiers::COMMAND;
            for (mods, key, act) in [
                (cmd | Modifiers::SHIFT, Key::Z, Act::Redo),
                (cmd, Key::Z, Act::Undo),
                (cmd, Key::S, Act::Save),
                (cmd, Key::Num0, Act::Fit),
                (cmd, Key::Num1, Act::Actual),
                (cmd, Key::D, Act::Duplicate),
                (cmd, Key::CloseBracket, Act::Front),
                (cmd, Key::OpenBracket, Act::Back),
                (Modifiers::NONE, Key::Backspace, Act::Delete),
                (Modifiers::NONE, Key::Delete, Act::Delete),
                (Modifiers::NONE, Key::Escape, Act::Escape),
            ] {
                if i.consume_key(mods, key) {
                    acts.push(act);
                }
            }
            for (key, size) in [
                (Key::Num1, tools::Size::S),
                (Key::Num2, tools::Size::M),
                (Key::Num3, tools::Size::L),
            ] {
                if i.consume_key(Modifiers::NONE, key) {
                    acts.push(Act::Size(size));
                }
            }
            for (tool, _, _, key) in TOOLS {
                if i.consume_key(Modifiers::NONE, key) {
                    acts.push(Act::Tool(tool));
                }
            }
        });
        for act in acts {
            match act {
                Act::Tool(t) => self.set_tool(t),
                Act::Size(s) => {
                    self.style.size = s;
                    self.style_changed();
                }
                Act::Undo => self.undo(),
                Act::Redo => self.redo(),
                Act::Delete => self.delete_selected(),
                Act::Escape if self.poly.is_some() => self.finish_polygon(),
                Act::Escape if self.grab_part => {
                    self.grab_part = false;
                    self.drag = None;
                }
                Act::Escape => {
                    if self.picking {
                        self.picking = false;
                    } else {
                        self.selected = None;
                    }
                }
                Act::Copy => self.copy(),
                Act::Save => self.save(),
                Act::Fit => self.zoom = None,
                Act::Actual => self.zoom = Some(1.0 / self.density),
                Act::Duplicate => self.duplicate_selected(),
                Act::Front => self.reorder_selected(true),
                Act::Back => self.reorder_selected(false),
                Act::Cut => self.cut_annotations(),
                Act::Paste => self.paste(None),
                Act::Previous => self.open_sibling(ctx, -1),
                Act::Next => self.open_sibling(ctx, 1),
                Act::Open => self.open_dialog(ctx),
                Act::Fullscreen => extras::toggle_fullscreen(ctx),
                Act::FinishPolygon => self.finish_polygon(),
            }
        }
    }

    fn set_tool(&mut self, tool: Tool) {
        self.commit_text();
        self.finish_polygon();
        self.tool = tool;
        self.picking = false;
    }

    // --- Zone de dessin ---------------------------------------------------------

    fn canvas(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.refresh_texture(&ctx);
        let avail = ui.available_rect_before_wrap();
        // Hauteur affichée : l'image et le bandeau des propriétés au-dessus.
        let (iw, ih) = (
            self.base.width() as f32,
            self.base.height() as f32 + self.banner,
        );
        let fit = (avail.width() / iw)
            .min(avail.height() / ih)
            .min(1.0 / self.density);
        let scale = self.zoom.unwrap_or(fit);
        self.shown_scale = scale;
        let size = vec2(iw, ih) * scale;

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Centrée quand elle tient, collée en haut à gauche sinon.
                let area = ui.available_size().max(size);
                let (outer, _) = ui.allocate_exact_size(area, Sense::hover());
                let image_rect = Rect::from_center_size(outer.center(), size);
                let view = View {
                    origin: image_rect.min + vec2(0.0, self.banner * scale),
                    scale,
                };
                let painter = ui.painter_at(outer);
                painter.rect_filled(image_rect.expand(1.0), 0.0, theme::BORDER);
                if let Some(tex) = &self.texture {
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    painter.image(tex.id(), image_rect, uv, Color32::WHITE);
                }
                let response =
                    ui.interact(outer, ui.scope_id().with("canvas"), Sense::click_and_drag());
                self.dropped_files(&ctx, &response, view);
                if !self.image_canvas(&ctx, &response, &painter, view) {
                    if !self.extra_pointer(&ctx, &response, view) {
                        self.pointer(&ctx, &response, view);
                    }
                    self.context_menu(&response, view);
                }

                if let Some(i) = self.hidden()
                    && self.text.is_none()
                    && let Some(ann) = self.doc.annotations.get(i)
                {
                    preview::paint(&painter, view, ann);
                }
                self.extra_paint(&painter, view);
                if let Some(i) = self.selected
                    && let Some(ann) = self.doc.annotations.get(i)
                {
                    if self.drag.is_none() {
                        preview::selection(&painter, view.rect(ann.bounds(render::text_size)));
                    }
                    let points: Vec<_> = ann.handles().iter().map(|p| view.to_screen(*p)).collect();
                    preview::handles(&painter, &points);
                }
                self.text_box(ui, &painter, view);
            });
    }

    fn hit(&self, p: Pt, scale: f32) -> Option<usize> {
        let tolerance = HIT_TOLERANCE / scale;
        // Du dessus vers le dessous.
        (0..self.doc.annotations.len())
            .rev()
            .find(|&i| self.doc.annotations[i].hit(p, tolerance, render::text_size))
    }

    /// Poignée de l'annotation sélectionnée sous le point d'écran `p`.
    fn handle_at(&self, p: egui::Pos2, view: View) -> Option<(usize, usize)> {
        let i = self.selected?;
        let ann = self.doc.annotations.get(i)?;
        ann.handles()
            .iter()
            .position(|h| view.to_screen(*h).distance(p) <= preview::HANDLE_RADIUS + 3.0)
            .map(|h| (i, h))
    }

    fn pointer(&mut self, ctx: &egui::Context, response: &egui::Response, view: View) {
        let pos = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos());
        let (shift, alt) = ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));

        // Curseur.
        if let Some(p) = response.hover_pos() {
            let icon = if self.picking || (alt && self.drag.is_none()) {
                CursorIcon::Crosshair
            } else if self.drag.is_some() {
                CursorIcon::Grabbing
            } else if self.handle_at(p, view).is_some() {
                CursorIcon::Grab
            } else {
                match self.tool {
                    Tool::Select => match self.hit(view.to_image(p), view.scale) {
                        Some(_) => CursorIcon::Grab,
                        None => CursorIcon::Default,
                    },
                    Tool::Text | Tool::Bubble => CursorIcon::Text,
                    _ => CursorIcon::Crosshair,
                }
            };
            ctx.set_cursor_icon(icon);
        }

        // Clic droit : sélectionne ce qui est dessous, pour le menu.
        if response.secondary_clicked()
            && let Some(p) = pos
        {
            self.commit_text();
            let hit = self.hit(view.to_image(p), view.scale);
            self.select(hit);
        }

        if response.double_clicked()
            && let Some(p) = pos
            && let Some(i) = self.hit(view.to_image(p), view.scale)
            && matches!(
                self.doc.annotations[i].shape,
                Shape::Text { .. } | Shape::Bubble { .. }
            )
        {
            self.start_text(Pt::default(), Some(i), TextKind::Plain { boxed: false });
            return;
        }

        if response.clicked()
            && let Some(p) = pos
        {
            let pt = view.to_image(p);
            if self.picking || alt {
                self.pick_color(pt);
                return;
            }
            let m = self.metrics();
            match self.tool {
                Tool::Select => {
                    self.commit_text();
                    let hit = self.hit(pt, view.scale);
                    self.select(hit);
                }
                Tool::Text => self.text_click(pt, view.scale, false),
                Tool::Bubble => self.text_click(pt, view.scale, true),
                Tool::Step | Tool::Stamp => {
                    self.commit_text();
                    let shape = if self.tool == Tool::Step {
                        Shape::Step {
                            at: pt,
                            number: self.take_step(),
                            tail: None,
                        }
                    } else {
                        Shape::Stamp {
                            at: pt,
                            icon: self.style.stamp.clone(),
                            size: m.stamp,
                        }
                    };
                    self.checkpoint();
                    self.doc.annotations.push(self.style.annotate(shape, m));
                    self.selected = Some(self.doc.annotations.len() - 1);
                    self.changed();
                }
                _ => {
                    self.commit_text();
                    self.selected = None;
                }
            }
            return;
        }

        if response.drag_started()
            && let Some(p) = response.interact_pointer_pos()
        {
            self.commit_text();
            // Point d'appui, pas la position actuelle : le glisser n'est reconnu
            // qu'après quelques points de mouvement.
            let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(p);
            let start = view.to_image(origin);
            let m = self.metrics();
            if let Some((index, handle)) = self.handle_at(origin, view) {
                self.checkpoint();
                self.drag = Some(Drag::Handle {
                    index,
                    handle,
                    moved: false,
                });
            } else if self.tool == Tool::Select {
                match self.hit(start, view.scale) {
                    Some(mut i) => {
                        self.checkpoint();
                        // Alt+glisser : une copie part, l'original reste.
                        if alt {
                            let copy = self.doc.annotations[i].clone();
                            self.doc.annotations.push(copy);
                            i = self.doc.annotations.len() - 1;
                        }
                        self.select(Some(i));
                        self.drag = Some(Drag::Move {
                            index: i,
                            last: start,
                            moved: alt,
                        });
                    }
                    None => self.selected = None,
                }
            } else if let Some(shape) =
                new_shape(self.tool, start, &self.style, m, self.peek_step())
            {
                if self.tool == Tool::Step {
                    self.take_step();
                }
                self.checkpoint();
                self.doc.annotations.push(self.style.annotate(shape, m));
                self.selected = None;
                self.drag = Some(Drag::Create {
                    index: self.doc.annotations.len() - 1,
                });
            }
        }

        if let (Some(drag), Some(p)) = (self.drag, response.interact_pointer_pos()) {
            let pt = view.to_image(p);
            match drag {
                Drag::Create { index } => {
                    if let Some(ann) = self.doc.annotations.get_mut(index) {
                        extend_shape(&mut ann.shape, pt, shift);
                    }
                }
                Drag::Move { index, last, .. } => {
                    if let Some(ann) = self.doc.annotations.get_mut(index) {
                        ann.translate(pt.x - last.x, pt.y - last.y);
                    }
                    self.drag = Some(Drag::Move {
                        index,
                        last: pt,
                        moved: true,
                    });
                }
                Drag::Handle { index, handle, .. } => {
                    if let Some(ann) = self.doc.annotations.get_mut(index) {
                        ann.move_handle(handle, pt);
                    }
                    self.drag = Some(Drag::Handle {
                        index,
                        handle,
                        moved: true,
                    });
                }
                // Gérés par `extra_pointer`.
                Drag::Marquee { .. } | Drag::Grab { .. } => {}
            }
        }

        if response.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Create { index }) => {
                    if self
                        .doc
                        .annotations
                        .get(index)
                        .is_some_and(Annotation::is_degenerate)
                    {
                        self.doc.annotations.remove(index);
                        self.undo.pop();
                    } else {
                        // Sélectionnée tout de suite : ses poignées sont à portée
                        // (courber une flèche, placer la loupe).
                        self.selected = Some(index);
                        self.changed();
                    }
                }
                Some(Drag::Move { moved: false, .. } | Drag::Handle { moved: false, .. }) => {
                    self.undo.pop();
                }
                Some(Drag::Move { .. } | Drag::Handle { .. }) => self.changed(),
                Some(Drag::Marquee { .. } | Drag::Grab { .. }) | None => {}
            }
            self.rendered = None;
        }
    }

    /// Menu du clic droit sur une annotation.
    fn context_menu(&mut self, response: &egui::Response, _view: View) {
        response.context_menu(|ui| {
            self.image_menu(ui);
            if self.selected.is_none() {
                ui.label(egui::RichText::new("Rien de sélectionné").color(theme::TEXT_DIM));
                self.extra_menu(ui, false);
                return;
            }
            let cmd = |key: &str| crate::ui::keys::cmd(false, key);
            if ui
                .button(format!("{}  Dupliquer\t{}", icons::DUPLICATE, cmd("D")))
                .clicked()
            {
                self.duplicate_selected();
                ui.close();
            }
            if ui
                .button(format!(
                    "{}  Premier plan\t{}",
                    icons::BRING_FRONT,
                    cmd("]")
                ))
                .clicked()
            {
                self.reorder_selected(true);
                ui.close();
            }
            if ui
                .button(format!("{}  Arrière-plan\t{}", icons::SEND_BACK, cmd("[")))
                .clicked()
            {
                self.reorder_selected(false);
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!(
                    "{}  Supprimer\t{}",
                    icons::DELETE,
                    crate::ui::keys::DELETE
                ))
                .clicked()
            {
                self.delete_selected();
                ui.close();
            }
            self.extra_menu(ui, true);
        });
    }

    /// Champ de saisie posé sur l'image, à la taille et la couleur du texte (sur son
    /// fond pour un texte encadré ou une bulle).
    fn text_box(&mut self, ui: &mut Ui, painter: &egui::Painter, view: View) {
        let Some(edit) = &mut self.text else { return };
        // Le marqueur de ⌘V (voir `extras.rs`) ne doit pas finir dans le texte.
        edit.text.retain(|c| c != extras::PASTE_MARKER);
        // Entrée valide, ⇧Entrée va à la ligne, Échap annule.
        let (enter, escape) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.text = None;
            self.rendered = None;
            return;
        }
        if enter {
            self.commit_text();
            return;
        }
        let shown = if edit.text.is_empty() {
            "Texte"
        } else {
            &edit.text
        };
        let framed = match edit.kind {
            TextKind::Plain { boxed } => boxed.then(|| {
                let (w, h) = render::text_size(shown, edit.size);
                let (px, py) = text_padding(edit.size);
                crate::model::Bounds {
                    min: edit.at.offset(-px, -py),
                    max: edit.at.offset(w + px, h + py),
                }
            }),
            TextKind::Bubble { .. } if edit.template.bubble != crate::model::BubbleKind::Tail => {
                Some(bubble_box(edit.at, shown, edit.size, &render::text_size))
            }
            TextKind::Bubble { tail } => {
                let b = bubble_box(edit.at, shown, edit.size, &render::text_size);
                let tip = view.to_screen(tail);
                let c = view.to_screen(b.center());
                let side = (tip - c).normalized().rot90() * edit.size * 0.45 * view.scale;
                painter.add(egui::Shape::convex_polygon(
                    vec![c + side, tip, c - side],
                    preview::color32(edit.color),
                    Stroke::NONE,
                ));
                Some(b)
            }
        };
        let text_color = match framed {
            Some(b) => {
                let radius = CornerRadius::same((edit.size * 0.35 * view.scale).min(255.0) as u8);
                painter.rect_filled(view.rect(b), radius, preview::color32(edit.color));
                preview::color32(edit.color.contrast())
            }
            None => preview::color32(edit.color),
        };
        let at = view.to_screen(edit.at);
        let font = theme::strong(edit.size * view.scale);
        let focus = std::mem::take(&mut edit.focus);
        egui::Area::new(egui::Id::unique("text_edit"))
            .fixed_pos(at - vec2(4.0, 2.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                let stroke = if framed.is_some() {
                    Stroke::NONE
                } else {
                    Stroke::new(1.0, theme::ACCENT_2)
                };
                Frame::new()
                    .stroke(stroke)
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(Margin::symmetric(4, 2))
                    .show(ui, |ui| {
                        let width = render::text_size(&edit.text, edit.size).0 * view.scale;
                        let response = ui.add(
                            TextEdit::multiline(&mut edit.text)
                                .font(font)
                                .text_color(text_color)
                                .frame(egui::Frame::NONE)
                                .desired_rows(1)
                                .desired_width(width.max(40.0) + 12.0)
                                .hint_text("Texte"),
                        );
                        if focus {
                            response.request_focus();
                        }
                    });
            });
    }
}

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.density = ctx.pixels_per_point().max(1.0);
        self.image_ui(&ctx);
        self.frame_extras(&ctx);
        self.shortcuts(&ctx);
        egui::Panel::top("title")
            .frame(
                Frame::new()
                    .fill(theme::BG)
                    .inner_margin(Margin::symmetric(12, 4)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.title_bar(ui));
        egui::Panel::top("tools")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::top("options")
            .frame(
                Frame::new()
                    .fill(theme::BG)
                    .inner_margin(Margin::symmetric(12, 6)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.options_bar(ui));
        egui::Panel::bottom("status")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(12, 4)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| self.canvas(ui));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.commit_text();
        self.finish_polygon();
        self.save_prefs();
    }
}
