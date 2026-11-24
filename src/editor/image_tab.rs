//! Fenêtre « Image » de l'éditeur (lot « Éditeur : image et effets ») : recadrer,
//! pivoter, réduire (couper une bande), redimensionner, zone de travail, bordure et
//! effets, changer l'image principale. Les calculs sont dans [`crate::effects`] ;
//! ici, l'interface et l'application des retouches au document.
//!
//! Raccourcis : ⌘R pivote de 90° à droite ; en mode recadrage, Entrée valide et
//! Échap annule.

use egui::{
    Color32, CursorIcon, FontId, Key, Modifiers, Painter, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Ui, pos2, vec2,
};
use image::RgbaImage;

use super::Editor;
use super::preview::{self, View};
use crate::effects::{
    self, Anchor, Background, BorderEffects, ImageEdit, Join, Orientation, ResizeMode,
};
use crate::history;
use crate::model::{Bounds, Pt, Rgba};
use crate::ui::{icons, keys, theme};

/// État de la fenêtre « Image » et des modes posés sur l'image.
#[derive(Default)]
pub(super) struct ImageTab {
    open: bool,
    mode: Mode,
    resize: ResizeForm,
    canvas: CanvasForm,
    /// Réglages de « Bordure et effets » en cours (appliqués par « Appliquer »).
    border: BorderEffects,
    border_bg: BgPick,
    /// Aperçu en direct : réglages rendus et leur image.
    preview: Option<(BorderEffects, TextureHandle)>,
    /// Image affichée, réduite pour l'aperçu.
    thumb: Option<(RgbaImage, f32)>,
}

/// Mode posé sur l'image : il prend la main sur la souris.
#[derive(Default)]
enum Mode {
    #[default]
    None,
    /// Rectangle gardé, en pixels de l'image.
    Crop {
        rect: Bounds,
        drag: Option<CropDrag>,
    },
    /// Bande à retirer : on la trace en glissant.
    Cut {
        orientation: Orientation,
        join: Join,
        band: Option<(f32, f32)>,
    },
}

#[derive(Clone, Copy)]
enum CropDrag {
    New(Pt),
    Move(Pt),
    /// Poignée : 0 haut gauche, puis dans le sens horaire (8 poignées).
    Handle(usize),
}

/// Choix d'un fond : transparent, couleur ou couleur du bord étendue.
#[derive(Clone, Copy)]
struct BgPick {
    kind: u8,
    color: Color32,
}

impl Default for BgPick {
    fn default() -> Self {
        Self {
            kind: 0,
            color: Color32::WHITE,
        }
    }
}

impl BgPick {
    fn from(bg: Background) -> Self {
        match bg {
            Background::Transparent => Self::default(),
            Background::Color { color } => Self {
                kind: 1,
                color: preview::color32(color),
            },
            Background::Extend => Self {
                kind: 2,
                ..Self::default()
            },
        }
    }

    fn get(self) -> Background {
        match self.kind {
            1 => Background::Color {
                color: Rgba(self.color.r(), self.color.g(), self.color.b(), 255),
            },
            2 => Background::Extend,
            _ => Background::Transparent,
        }
    }

    fn ui(&mut self, ui: &mut Ui) -> bool {
        let before = (self.kind, self.color);
        ui.horizontal(|ui| {
            ui.label("Fond");
            ui.radio_value(&mut self.kind, 0, "Transparent");
            ui.radio_value(&mut self.kind, 1, "Couleur");
            if self.kind == 1 {
                ui.color_edit_button_srgba(&mut self.color);
            }
        });
        ui.radio_value(&mut self.kind, 2, "Étendre la couleur de fond")
            .on_hover_text("La couleur la plus fréquente sur le bord de l'image");
        (self.kind, self.color) != before
    }
}

/// « Redimensionner ».
struct ResizeForm {
    /// 0 pourcentage, 1 largeur, 2 hauteur, 3 taille maximale, 4 largeur et hauteur.
    mode: u8,
    percent: f32,
    width: u32,
    height: u32,
    max_side: u32,
    keep_ratio: bool,
    /// Largeur et hauteur avec ratio : combler pour obtenir exactement ce cadre.
    pad: bool,
    pad_bg: BgPick,
    dpi_on: bool,
    dpi: f32,
    /// Résolution lue dans le PNG d'origine.
    original_dpi: Option<f32>,
}

impl Default for ResizeForm {
    fn default() -> Self {
        Self {
            mode: 0,
            percent: 50.0,
            width: 0,
            height: 0,
            max_side: 0,
            keep_ratio: true,
            pad: false,
            pad_bg: BgPick::default(),
            dpi_on: false,
            dpi: 96.0,
            original_dpi: None,
        }
    }
}

impl ResizeForm {
    fn target(&self, src: (u32, u32)) -> (u32, u32) {
        effects::resize_target(self.resize_mode(), src)
    }

    fn resize_mode(&self) -> ResizeMode {
        match self.mode {
            0 => ResizeMode::Percent(self.percent),
            1 => ResizeMode::Width(self.width),
            2 => ResizeMode::Height(self.height),
            3 => ResizeMode::MaxSide(self.max_side),
            _ => ResizeMode::Exact {
                w: self.width,
                h: self.height,
                keep_ratio: self.keep_ratio,
            },
        }
    }
}

/// « Zone de travail ».
struct CanvasForm {
    auto: bool,
    by_percent: bool,
    percent: f32,
    w: u32,
    h: u32,
    anchor: Anchor,
    bg: BgPick,
}

impl Default for CanvasForm {
    fn default() -> Self {
        Self {
            auto: false,
            by_percent: true,
            percent: 120.0,
            w: 0,
            h: 0,
            anchor: Anchor::default(),
            bg: BgPick::default(),
        }
    }
}

/// Taille de l'aperçu de « Bordure et effets », en points.
const PREVIEW: (f32, f32) = (280.0, 180.0);

impl Editor {
    // --- Points d'entrée (appelés depuis mod.rs et bars.rs) -------------------------

    /// Bouton « Image » de la barre d'outils.
    pub(super) fn image_button(&mut self, ui: &mut Ui) {
        ui.separator();
        let text = RichText::new(format!("{}  Image", icons::IMAGE)).size(theme::SIZE_M);
        let open = self.image.open;
        if ui
            .selectable_label(open, text)
            .on_hover_text(
                "Recadrer, pivoter, réduire, redimensionner, zone de travail, bordure et effets",
            )
            .clicked()
        {
            if open {
                self.image.open = false;
            } else {
                self.open_image_panel();
            }
        }
    }

    /// Raccourcis du lot et fenêtre « Image ». Appelé avant les raccourcis de
    /// l'éditeur : Entrée et Échap vont au mode en cours.
    pub(super) fn image_ui(&mut self, ctx: &egui::Context) {
        if !ctx.egui_wants_keyboard_input() {
            let (enter, escape, rotate) = ctx.input_mut(|i| {
                let active = !matches!(self.image.mode, Mode::None);
                (
                    active && i.consume_key(Modifiers::NONE, Key::Enter),
                    active && i.consume_key(Modifiers::NONE, Key::Escape),
                    i.consume_key(Modifiers::COMMAND, Key::R),
                )
            });
            if enter {
                self.apply_crop();
            }
            if escape {
                self.image.mode = Mode::None;
            }
            if rotate {
                self.push_edit(ImageEdit::Rotate { quarters: 1 });
            }
        }
        if !self.image.open {
            return;
        }
        let mut open = true;
        egui::Window::new(format!("{}  Image", icons::IMAGE))
            .open(&mut open)
            .resizable(false)
            .default_width(320.0)
            .default_pos(pos2(ctx.content_rect().right() - 360.0, 110.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(ctx.content_rect().height() - 180.0)
                    .show(ui, |ui| self.image_panel(ui));
            });
        if !open {
            self.image.open = false;
            self.image.mode = Mode::None;
        }
    }

    /// Mode posé sur l'image (recadrage, bande à couper) : dessin et souris.
    /// Rend `true` s'il a pris la main (les outils de dessin ne réagissent pas).
    pub(super) fn image_canvas(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
        painter: &Painter,
        view: View,
    ) -> bool {
        let (w, h) = (self.base.width() as f32, self.base.height() as f32);
        let clamp = |p: Pt| Pt::new(p.x.clamp(0.0, w), p.y.clamp(0.0, h));
        let pointer = response
            .interact_pointer_pos()
            .map(|p| clamp(view.to_image(p)));
        let origin = ctx
            .input(|i| i.pointer.press_origin())
            .map(|p| view.to_image(p));
        let full = view.rect(Bounds {
            min: Pt::default(),
            max: Pt::new(w, h),
        });
        match &mut self.image.mode {
            Mode::None => return false,
            Mode::Crop { rect, drag } => {
                let handles = crop_handles(*rect);
                let on_handle = |p: Pos2| {
                    handles.iter().position(|h| {
                        view.to_screen(*h).distance(p) <= preview::HANDLE_RADIUS + 4.0
                    })
                };
                if let Some(p) = response.hover_pos() {
                    let icon = match (drag.as_ref(), on_handle(p)) {
                        (Some(_), _) => CursorIcon::Grabbing,
                        (None, Some(i)) => handle_cursor(i),
                        (None, None) if rect.contains(view.to_image(p)) => CursorIcon::Move,
                        _ => CursorIcon::Crosshair,
                    };
                    ctx.set_cursor_icon(icon);
                }
                if response.drag_started()
                    && let Some(o) = origin
                {
                    let screen = view.to_screen(o);
                    *drag = Some(match on_handle(screen) {
                        Some(i) => CropDrag::Handle(i),
                        None if rect.contains(o) => CropDrag::Move(o),
                        None => CropDrag::New(clamp(o)),
                    });
                }
                if let (Some(d), Some(p)) = (*drag, pointer) {
                    match d {
                        CropDrag::New(o) => *rect = Bounds::from_corners(o, p),
                        CropDrag::Move(last) => {
                            let raw =
                                view.to_image(response.interact_pointer_pos().unwrap_or_default());
                            let dx = (raw.x - last.x).clamp(-rect.min.x, w - rect.max.x);
                            let dy = (raw.y - last.y).clamp(-rect.min.y, h - rect.max.y);
                            rect.min = rect.min.offset(dx, dy);
                            rect.max = rect.max.offset(dx, dy);
                            *drag = Some(CropDrag::Move(Pt::new(last.x + dx, last.y + dy)));
                        }
                        CropDrag::Handle(i) => *rect = move_crop_handle(*rect, i, p),
                    }
                }
                if response.drag_stopped() {
                    *drag = None;
                    if rect.width() < 2.0 || rect.height() < 2.0 {
                        *rect = Bounds {
                            min: Pt::default(),
                            max: Pt::new(w, h),
                        };
                    }
                }
                let apply = response.double_clicked();
                // Dessin : extérieur assombri, cadre, poignées, dimensions.
                let r = view.rect(*rect);
                let veil = Color32::from_black_alpha(150);
                for part in [
                    Rect::from_min_max(full.min, pos2(full.max.x, r.min.y)),
                    Rect::from_min_max(pos2(full.min.x, r.max.y), full.max),
                    Rect::from_min_max(pos2(full.min.x, r.min.y), pos2(r.min.x, r.max.y)),
                    Rect::from_min_max(pos2(r.max.x, r.min.y), pos2(full.max.x, r.max.y)),
                ] {
                    if part.is_positive() {
                        painter.rect_filled(part, 0.0, veil);
                    }
                }
                for k in 1..3 {
                    let t = k as f32 / 3.0;
                    let thin = Stroke::new(1.0, Color32::from_white_alpha(70));
                    let x = r.min.x + r.width() * t;
                    let y = r.min.y + r.height() * t;
                    painter.line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], thin);
                    painter.line_segment([pos2(r.min.x, y), pos2(r.max.x, y)], thin);
                }
                painter.rect_stroke(
                    r,
                    0.0,
                    Stroke::new(1.5, theme::ACCENT_2),
                    StrokeKind::Middle,
                );
                let points: Vec<Pos2> = handles.iter().map(|p| view.to_screen(*p)).collect();
                preview::handles(painter, &points);
                let label = format!(
                    "{} × {}",
                    rect.width().round() as u32,
                    rect.height().round() as u32
                );
                tag(
                    painter,
                    pos2(r.center().x, r.min.y - 26.0).max(full.min),
                    &label,
                );
                hint(
                    painter,
                    full,
                    "Recadrer : glisse pour choisir la zone · Entrée valide · Échap annule",
                );
                if apply {
                    self.apply_crop();
                }
            }
            Mode::Cut {
                orientation,
                join,
                band,
            } => {
                let (orientation, join) = (*orientation, *join);
                let horizontal = orientation == Orientation::Horizontal;
                ctx.set_cursor_icon(if horizontal {
                    CursorIcon::ResizeVertical
                } else {
                    CursorIcon::ResizeHorizontal
                });
                let coord = |p: Pt| if horizontal { p.y } else { p.x };
                if response.drag_started()
                    && let Some(o) = origin
                {
                    let v = coord(clamp(o));
                    *band = Some((v, v));
                }
                if let (Some((a, _)), Some(p)) = (*band, pointer)
                    && response.dragged()
                {
                    *band = Some((a, coord(p)));
                }
                // Ligne sous le curseur, puis la bande tracée.
                let line = |v: f32| {
                    let (a, b) = if horizontal {
                        (Pt::new(0.0, v), Pt::new(w, v))
                    } else {
                        (Pt::new(v, 0.0), Pt::new(v, h))
                    };
                    [view.to_screen(a), view.to_screen(b)]
                };
                if let Some(p) = response.hover_pos() {
                    let v = coord(clamp(view.to_image(p)));
                    painter.line_segment(line(v), Stroke::new(1.0, theme::DANGER));
                }
                let current = *band;
                if let Some((a, b)) = current {
                    let (lo, hi) = (a.min(b), a.max(b));
                    let shown = if horizontal {
                        Bounds::from_corners(Pt::new(0.0, lo), Pt::new(w, hi))
                    } else {
                        Bounds::from_corners(Pt::new(lo, 0.0), Pt::new(hi, h))
                    };
                    painter.rect_filled(
                        view.rect(shown),
                        0.0,
                        Color32::from_rgba_unmultiplied(255, 77, 77, 70),
                    );
                    for v in [lo, hi] {
                        painter.line_segment(line(v), Stroke::new(1.5, theme::DANGER));
                    }
                    tag(
                        painter,
                        view.to_screen(shown.center()),
                        &format!("{} px", (hi - lo).round()),
                    );
                }
                hint(
                    painter,
                    full,
                    "Réduire : glisse sur l'image pour choisir la bande à retirer · Échap annule",
                );
                if response.drag_stopped() {
                    self.image.mode = Mode::None;
                    if let Some((a, b)) = current {
                        let (start, end) = (a.min(b).round() as u32, a.max(b).round() as u32);
                        if end > start {
                            self.push_edit(ImageEdit::CutOut {
                                orientation,
                                start,
                                end,
                                join,
                            });
                        }
                    }
                }
            }
        }
        true
    }

    /// Éléments du lot dans le menu du clic droit sur l'image.
    pub(super) fn image_menu(&mut self, ui: &mut Ui) {
        if ui
            .button(format!("{}  Changer l'image principale…", icons::IMAGE))
            .on_hover_text("Remplace la capture de fond en gardant les annotations")
            .clicked()
        {
            ui.close();
            self.change_base();
        }
        ui.separator();
    }

    // --- Retouches --------------------------------------------------------------------

    fn open_image_panel(&mut self) {
        let (w, h) = self.base.dimensions();
        let tab = &mut self.image;
        tab.open = true;
        tab.resize.width = w;
        tab.resize.height = h;
        tab.resize.max_side = w.max(h);
        tab.resize.dpi_on = self.doc.dpi.is_some();
        tab.resize.original_dpi = std::fs::read(&self.path)
            .ok()
            .and_then(|b| effects::png_dpi(&b));
        tab.resize.dpi = self.doc.dpi.or(tab.resize.original_dpi).unwrap_or(96.0);
        tab.canvas.w = w;
        tab.canvas.h = h;
        tab.canvas.auto = self.doc.canvas_auto.is_some();
        if let Some(bg) = self.doc.canvas_auto {
            tab.canvas.bg = BgPick::from(bg);
        }
        tab.border = self.doc.border.clone().unwrap_or_default();
        tab.border_bg = BgPick::from(tab.border.background);
        tab.preview = None;
        tab.thumb = None;
    }

    /// Ajoute une retouche : nouvelle image de travail, annotations (et historique
    /// d'annulation) déplacées avec elle.
    fn push_edit(&mut self, edit: ImageEdit) {
        self.commit_text();
        self.image.mode = Mode::None;
        let src = self.base.dimensions();
        let base = match &edit {
            ImageEdit::ReplaceBase { path } => match history::read_png(path) {
                Ok(img) => img,
                Err(e) => {
                    self.say(format!("{} {e:#}", icons::WARNING));
                    return;
                }
            },
            _ => edit.apply(&self.base),
        };
        effects::transform_annotations(&mut self.doc.annotations, &edit, src, false);
        for snapshot in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            effects::transform_annotations(snapshot, &edit, src, false);
        }
        let (w, h) = base.dimensions();
        self.doc.edits.push(edit);
        self.base = base;
        self.after_edit();
        self.say(format!("{} Image : {w} × {h} px", icons::OK));
    }

    /// Annule la dernière retouche (l'image de travail est recalculée depuis
    /// l'original).
    fn undo_edit(&mut self) {
        self.commit_text();
        let Some(edit) = self.doc.edits.pop() else {
            return;
        };
        let original = match history::read_png(&self.path) {
            Ok(img) => img,
            Err(e) => {
                self.doc.edits.push(edit);
                self.say(format!("{} {e:#}", icons::WARNING));
                return;
            }
        };
        let base = effects::working_base(original, &self.doc.edits);
        let src = base.dimensions();
        effects::transform_annotations(&mut self.doc.annotations, &edit, src, true);
        for snapshot in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            effects::transform_annotations(snapshot, &edit, src, true);
        }
        self.base = base;
        self.after_edit();
        self.say(format!("{} Retouche annulée", icons::UNDO));
    }

    fn after_edit(&mut self) {
        self.image.mode = Mode::None;
        self.image.thumb = None;
        self.image.preview = None;
        self.selected = None;
        let (w, h) = self.base.dimensions();
        self.image.resize.width = w;
        self.image.resize.height = h;
        self.image.resize.max_side = w.max(h);
        self.image.canvas.w = w;
        self.image.canvas.h = h;
        self.changed();
    }

    fn start_crop(&mut self) {
        self.commit_text();
        self.selected = None;
        let (w, h) = (self.base.width() as f32, self.base.height() as f32);
        self.image.mode = Mode::Crop {
            rect: Bounds {
                min: Pt::default(),
                max: Pt::new(w, h),
            },
            drag: None,
        };
    }

    fn apply_crop(&mut self) {
        let Mode::Crop { rect, .. } = self.image.mode else {
            return;
        };
        self.image.mode = Mode::None;
        let (x, y) = (rect.min.x.round() as u32, rect.min.y.round() as u32);
        let (w, h) = (
            (rect.max.x.round() as u32).saturating_sub(x),
            (rect.max.y.round() as u32).saturating_sub(y),
        );
        if w == 0 || h == 0 || (x, y, w, h) == (0, 0, self.base.width(), self.base.height()) {
            return;
        }
        self.push_edit(ImageEdit::Crop { x, y, w, h });
    }

    fn change_base(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Changer l'image principale")
            .add_filter("Image PNG", &["png"]);
        if let Some(dir) = self.path.parent() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            let path = std::path::absolute(&path).unwrap_or(path);
            self.push_edit(ImageEdit::ReplaceBase { path });
        }
    }

    // --- Fenêtre ------------------------------------------------------------------------

    fn image_panel(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 6.0;
        self.crop_section(ui);
        self.cut_section(ui);
        egui::CollapsingHeader::new("Redimensionner")
            .default_open(false)
            .show(ui, |ui| self.resize_section(ui));
        egui::CollapsingHeader::new("Zone de travail")
            .default_open(false)
            .show(ui, |ui| self.canvas_section(ui));
        egui::CollapsingHeader::new("Bordure et effets")
            .default_open(false)
            .show(ui, |ui| self.border_section(ui));
        ui.separator();
        if ui
            .button(format!("{}  Changer l'image principale…", icons::IMAGE))
            .on_hover_text("Remplace la capture de fond en gardant les annotations")
            .clicked()
        {
            self.change_base();
        }
        let n = self.doc.edits.len();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(match n {
                    0 => "Aucune retouche".to_owned(),
                    1 => "1 retouche".to_owned(),
                    n => format!("{n} retouches"),
                })
                .color(theme::TEXT_DIM),
            );
            if ui
                .add_enabled(
                    n > 0,
                    egui::Button::new(format!("{}  Annuler la dernière", icons::UNDO)),
                )
                .on_hover_text("L'image d'origine n'est jamais modifiée")
                .clicked()
            {
                self.undo_edit();
            }
        });
    }

    fn crop_section(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Recadrer et pivoter").strong());
        ui.horizontal_wrapped(|ui| {
            let cropping = matches!(self.image.mode, Mode::Crop { .. });
            if cropping {
                if ui.button(format!("{}  Valider", icons::OK)).clicked() {
                    self.apply_crop();
                }
                if ui.button("Annuler").clicked() {
                    self.image.mode = Mode::None;
                }
            } else if ui
                .button(format!("{}  Recadrer", icons::CROP))
                .on_hover_text("Glisse sur l'image, puis Entrée")
                .clicked()
            {
                self.start_crop();
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (label, tip, edit) in [
                (
                    "90° à gauche",
                    String::new(),
                    ImageEdit::Rotate { quarters: 3 },
                ),
                (
                    "90° à droite",
                    keys::cmd(false, "R"),
                    ImageEdit::Rotate { quarters: 1 },
                ),
                ("180°", String::new(), ImageEdit::Rotate { quarters: 2 }),
                (
                    "Miroir ↔",
                    "Retourner gauche-droite".to_owned(),
                    ImageEdit::Flip { horizontal: true },
                ),
                (
                    "Miroir ↕",
                    "Retourner haut-bas".to_owned(),
                    ImageEdit::Flip { horizontal: false },
                ),
            ] {
                let button = ui.button(label);
                let button = if tip.is_empty() {
                    button
                } else {
                    button.on_hover_text(tip)
                };
                if button.clicked() {
                    self.push_edit(edit);
                }
            }
        });
        ui.separator();
    }

    fn cut_section(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Réduire (couper une bande)").strong());
        let current = match self.image.mode {
            Mode::Cut {
                orientation, join, ..
            } => Some((orientation, join)),
            _ => None,
        };
        egui::Grid::new("cut_modes")
            .spacing([6.0, 4.0])
            .show(ui, |ui| {
                for (orientation, name) in [
                    (Orientation::Horizontal, "Horizontale"),
                    (Orientation::Vertical, "Verticale"),
                ] {
                    ui.label(name);
                    for (join, label) in [
                        (Join::Torn, "déchirée"),
                        (Join::Straight, "nette"),
                        (Join::Dotted, "pointillée"),
                    ] {
                        let on = current == Some((orientation, join));
                        if ui.selectable_label(on, label).clicked() {
                            self.commit_text();
                            self.selected = None;
                            self.image.mode = if on {
                                Mode::None
                            } else {
                                Mode::Cut {
                                    orientation,
                                    join,
                                    band: None,
                                }
                            };
                        }
                    }
                    ui.end_row();
                }
            });
        if current.is_some() {
            ui.label(
                RichText::new("Glisse sur l'image pour choisir la bande à retirer.")
                    .color(theme::ACCENT_2),
            );
        }
        ui.separator();
    }

    fn resize_section(&mut self, ui: &mut Ui) {
        let src = self.base.dimensions();
        let original = self.original_size();
        let form = &mut self.image.resize;
        ui.label(
            RichText::new(format!(
                "Actuelle : {} × {} px · origine : {} × {} px · {}",
                src.0,
                src.1,
                original.0,
                original.1,
                match form.original_dpi {
                    Some(d) => format!("{} DPI", d.round()),
                    None => "DPI non renseignée".to_owned(),
                }
            ))
            .color(theme::TEXT_DIM),
        );
        for (mode, label) in [
            (0, "Pourcentage"),
            (1, "Modifier la largeur"),
            (2, "Modifier la hauteur"),
            (3, "Modifier la taille maximale"),
            (4, "Modifier la largeur et la hauteur"),
        ] {
            ui.radio_value(&mut form.mode, mode, label);
        }
        ui.horizontal(|ui| match form.mode {
            0 => {
                ui.add(
                    egui::DragValue::new(&mut form.percent)
                        .range(1.0..=1000.0)
                        .suffix(" %"),
                );
            }
            1 => {
                ui.add(
                    egui::DragValue::new(&mut form.width)
                        .range(1..=65_535)
                        .suffix(" px"),
                );
            }
            2 => {
                ui.add(
                    egui::DragValue::new(&mut form.height)
                        .range(1..=65_535)
                        .suffix(" px"),
                );
            }
            3 => {
                ui.add(
                    egui::DragValue::new(&mut form.max_side)
                        .range(1..=65_535)
                        .suffix(" px"),
                );
            }
            _ => {
                ui.add(egui::DragValue::new(&mut form.width).range(1..=65_535));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut form.height).range(1..=65_535));
                ui.checkbox(&mut form.keep_ratio, "Conserver le ratio");
            }
        });
        if form.mode == 4 && form.keep_ratio {
            ui.checkbox(
                &mut form.pad,
                format!("Combler pour obtenir {} × {}", form.width, form.height),
            );
            if form.pad {
                form.pad_bg.ui(ui);
            }
        }
        let target = form.target(src);
        ui.label(format!("Résultat : {} × {} px", target.0, target.1));
        ui.horizontal(|ui| {
            ui.checkbox(&mut form.dpi_on, "Résolution");
            ui.add_enabled(
                form.dpi_on,
                egui::DragValue::new(&mut form.dpi)
                    .range(1.0..=2400.0)
                    .suffix(" DPI"),
            );
        })
        .response
        .on_hover_text("Écrite dans le PNG exporté ; les pixels ne changent pas");
        let pad = (form.mode == 4 && form.keep_ratio && form.pad)
            .then(|| (form.width.max(1), form.height.max(1), form.pad_bg.get()));
        let dpi = form.dpi_on.then_some(form.dpi);
        if ui.button(format!("{}  Appliquer", icons::OK)).clicked() {
            if dpi != self.doc.dpi {
                self.doc.dpi = dpi;
                self.changed();
            }
            if target != src {
                self.push_edit(ImageEdit::Resize {
                    w: target.0,
                    h: target.1,
                });
            }
            if let Some((w, h, background)) = pad
                && (w, h) != self.base.dimensions()
            {
                self.push_edit(ImageEdit::Canvas {
                    w,
                    h,
                    anchor: Anchor::default(),
                    background,
                });
            }
        }
    }

    /// Dimensions de l'image d'origine (avant retouches), lues dans l'en-tête du PNG.
    fn original_size(&self) -> (u32, u32) {
        image::image_dimensions(&self.path).unwrap_or(self.base.dimensions())
    }

    fn canvas_section(&mut self, ui: &mut Ui) {
        let src = self.base.dimensions();
        let form = &mut self.image.canvas;
        ui.horizontal(|ui| {
            ui.radio_value(&mut form.auto, false, "Fixe");
            ui.radio_value(&mut form.auto, true, "Automatique");
        });
        if form.auto {
            ui.label(
                RichText::new(
                    "À l'export, la zone s'agrandit pour contenir les annotations qui dépassent \
                     de l'image.",
                )
                .color(theme::TEXT_DIM),
            );
        } else {
            ui.horizontal(|ui| {
                ui.radio_value(&mut form.by_percent, true, "Pourcentage");
                ui.radio_value(&mut form.by_percent, false, "Largeur × hauteur");
            });
            ui.horizontal(|ui| {
                if form.by_percent {
                    ui.add(
                        egui::DragValue::new(&mut form.percent)
                            .range(10.0..=1000.0)
                            .suffix(" %"),
                    );
                } else {
                    ui.add(egui::DragValue::new(&mut form.w).range(1..=65_535));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut form.h).range(1..=65_535));
                }
            });
            ui.horizontal(|ui| {
                ui.label("Ancrage");
                anchor_grid(ui, &mut form.anchor);
            });
        }
        form.bg.ui(ui);
        let target = if form.by_percent {
            effects::resize_target(ResizeMode::Percent(form.percent), src)
        } else {
            (form.w.max(1), form.h.max(1))
        };
        if !form.auto {
            ui.label(format!("Zone : {} × {} px", target.0, target.1));
        }
        let (auto, anchor, background) = (form.auto, form.anchor, form.bg.get());
        ui.horizontal(|ui| {
            if ui.button(format!("{}  Appliquer", icons::OK)).clicked() {
                if auto {
                    self.doc.canvas_auto = Some(background);
                    self.changed();
                    self.say(format!("{} Zone de travail automatique", icons::OK));
                } else {
                    if self.doc.canvas_auto.take().is_some() {
                        self.changed();
                    }
                    if target != src {
                        self.push_edit(ImageEdit::Canvas {
                            w: target.0,
                            h: target.1,
                            anchor,
                            background,
                        });
                    }
                }
            }
            if self.doc.canvas_auto.is_some() && ui.button("Désactiver l'automatique").clicked() {
                self.doc.canvas_auto = None;
                self.image.canvas.auto = false;
                self.changed();
            }
        });
    }

    fn border_section(&mut self, ui: &mut Ui) {
        let tab = &mut self.image;
        let fx = &mut tab.border;
        let slider = |ui: &mut Ui, value: &mut u32, max: u32, label: &str, suffix: &str| {
            ui.add(egui::Slider::new(value, 0..=max).text(label).suffix(suffix));
        };
        slider(ui, &mut fx.border_width, 100, "Largeur du bord", " px");
        ui.horizontal(|ui| {
            let mut c = preview::color32(fx.border_color);
            ui.color_edit_button_srgba(&mut c);
            ui.label("Couleur du bord");
            fx.border_color = Rgba(c.r(), c.g(), c.b(), 255);
        });
        slider(ui, &mut fx.corner_radius, 200, "Coins arrondis", " px");
        ui.horizontal(|ui| {
            ui.label("Bords déchirés");
            torn_picker(ui, &mut fx.torn);
        });
        slider(ui, &mut fx.torn_ratio, 100, "Ratio de déchirure", " %");
        ui.checkbox(&mut fx.grayscale, "Convertir en noir et blanc");
        slider(ui, &mut fx.reflection, 100, "Réflexion", " %");
        slider(
            ui,
            &mut fx.reflection_offset,
            100,
            "Offset de réflexion",
            " px",
        );
        slider(ui, &mut fx.shadow, 100, "Ombre", " %");
        ui.add(
            egui::Slider::new(&mut fx.rotation, -180.0..=180.0)
                .text("Rotation")
                .suffix("°"),
        );
        tab.border_bg.ui(ui);
        fx.background = tab.border_bg.get();

        // Aperçu en direct, sur l'image annotée réduite.
        self.refresh_border_preview(ui.ctx());
        if let Some((_, tex)) = &self.image.preview {
            let size = tex.size_vec2();
            let k = (PREVIEW.0 / size.x).min(PREVIEW.1 / size.y).min(1.0);
            let (rect, _) = ui.allocate_exact_size(size * k, Sense::hover());
            checkerboard(ui.painter(), rect);
            ui.painter().image(
                tex.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        ui.horizontal_wrapped(|ui| {
            if ui
                .button(format!("{}  Effet d'ombre", icons::SHADOW))
                .on_hover_text("Préréglage")
                .clicked()
            {
                self.image.border = BorderEffects::shadow_preset();
                self.image.border_bg = BgPick::default();
            }
            if ui.button(format!("{}  Appliquer", icons::OK)).clicked() {
                let fx = self.image.border.clone();
                self.doc.border = (!fx.is_noop()).then_some(fx);
                self.changed();
                self.say(format!(
                    "{} Bordure et effets : appliqués à la copie et à l'enregistrement",
                    icons::OK
                ));
            }
            if self.doc.border.is_some() && ui.button("Retirer").clicked() {
                self.doc.border = None;
                self.image.border = BorderEffects::default();
                self.image.border_bg = BgPick::default();
                self.changed();
            }
        });
    }

    fn refresh_border_preview(&mut self, ctx: &egui::Context) {
        let fx = self.image.border.clone();
        if self
            .image
            .preview
            .as_ref()
            .is_some_and(|(done, _)| *done == fx)
        {
            return;
        }
        if self.image.thumb.is_none() {
            let Some(img) = &self.composite else { return };
            let k = (2.0 * PREVIEW.0 / img.width() as f32)
                .min(2.0 * PREVIEW.1 / img.height() as f32)
                .min(1.0);
            let (w, h) = (
                ((img.width() as f32 * k).round() as u32).max(1),
                ((img.height() as f32 * k).round() as u32).max(1),
            );
            let small = image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle);
            self.image.thumb = Some((small, k));
        }
        let Some((thumb, k)) = &self.image.thumb else {
            return;
        };
        let out = effects::apply_border_scaled(thumb, &fx, *k);
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [out.width() as usize, out.height() as usize],
            out.as_raw(),
        );
        let tex = match self.image.preview.take() {
            Some((_, mut t)) => {
                t.set(color, TextureOptions::LINEAR);
                t
            }
            None => ctx.load_texture("bordure", color, TextureOptions::LINEAR),
        };
        self.image.preview = Some((fx, tex));
    }
}

/// Les 8 poignées du recadrage : coins et milieux, dans le sens horaire depuis le
/// coin haut gauche.
fn crop_handles(r: Bounds) -> [Pt; 8] {
    let c = r.center();
    [
        r.min,
        Pt::new(c.x, r.min.y),
        Pt::new(r.max.x, r.min.y),
        Pt::new(r.max.x, c.y),
        r.max,
        Pt::new(c.x, r.max.y),
        Pt::new(r.min.x, r.max.y),
        Pt::new(r.min.x, c.y),
    ]
}

/// Déplace la poignée `i` du recadrage en `p`.
fn move_crop_handle(r: Bounds, i: usize, p: Pt) -> Bounds {
    let (mut min, mut max) = (r.min, r.max);
    if matches!(i, 0 | 6 | 7) {
        min.x = p.x;
    }
    if matches!(i, 2..=4) {
        max.x = p.x;
    }
    if matches!(i, 0..=2) {
        min.y = p.y;
    }
    if matches!(i, 4..=6) {
        max.y = p.y;
    }
    Bounds::from_corners(min, max)
}

fn handle_cursor(i: usize) -> CursorIcon {
    match i {
        0 | 4 => CursorIcon::ResizeNwSe,
        2 | 6 => CursorIcon::ResizeNeSw,
        1 | 5 => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeHorizontal,
    }
}

/// Étiquette sur fond sombre.
fn tag(painter: &Painter, at: Pos2, text: &str) {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(13.0), theme::TEXT);
    let rect = Rect::from_center_size(at + vec2(0.0, 12.0), galley.size() + vec2(12.0, 6.0));
    painter.rect_filled(rect, 6.0, Color32::from_black_alpha(200));
    painter.galley(rect.min + vec2(6.0, 3.0), galley, theme::TEXT);
}

/// Consigne en haut de l'image.
fn hint(painter: &Painter, image: Rect, text: &str) {
    let at = pos2(image.center().x, painter.clip_rect().min.y + 4.0);
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        FontId::proportional(13.0),
        theme::ON_ACCENT_2,
    );
    let rect = Rect::from_center_size(at + vec2(0.0, 14.0), galley.size() + vec2(16.0, 8.0));
    painter.rect_filled(rect, 8.0, theme::ACCENT_2);
    painter.galley(rect.min + vec2(8.0, 4.0), galley, theme::ON_ACCENT_2);
}

/// Damier sous un aperçu (pour voir la transparence).
fn checkerboard(painter: &Painter, rect: Rect) {
    painter.rect_filled(rect, 0.0, Color32::from_gray(200));
    let cell = 8.0;
    let (nx, ny) = (
        (rect.width() / cell).ceil() as i32,
        (rect.height() / cell).ceil() as i32,
    );
    for y in 0..ny {
        for x in (0..nx).filter(|x| (x + y) % 2 == 0) {
            let min = rect.min + vec2(x as f32 * cell, y as f32 * cell);
            let cell_rect = Rect::from_min_size(min, vec2(cell, cell)).intersect(rect);
            painter.rect_filled(cell_rect, 0.0, Color32::from_gray(150));
        }
    }
}

/// Grille 3 × 3 de l'ancrage.
fn anchor_grid(ui: &mut Ui, anchor: &mut Anchor) {
    egui::Grid::new("anchor")
        .spacing([2.0, 2.0])
        .show(ui, |ui| {
            for row in 0..3u8 {
                for col in 0..3u8 {
                    let on = anchor.col == col && anchor.row == row;
                    if ui
                        .selectable_label(on, if on { "●" } else { "○" })
                        .clicked()
                    {
                        *anchor = Anchor { col, row };
                    }
                }
                ui.end_row();
            }
        });
}

/// Choix des côtés déchirés : un carré, un bouton par côté.
fn torn_picker(ui: &mut Ui, sides: &mut effects::TornSides) {
    let size = vec2(56.0, 40.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter_at(rect);
    let inner = rect.shrink(9.0);
    painter.rect_filled(inner, 2.0, theme::SURFACE_HI);
    let edges = [
        (
            Rect::from_min_max(rect.min, pos2(rect.max.x, inner.min.y)),
            0,
        ),
        (
            Rect::from_min_max(pos2(inner.max.x, rect.min.y), rect.max),
            1,
        ),
        (
            Rect::from_min_max(pos2(rect.min.x, inner.max.y), rect.max),
            2,
        ),
        (
            Rect::from_min_max(rect.min, pos2(inner.min.x, rect.max.y)),
            3,
        ),
    ];
    let lines = [
        [inner.left_top(), inner.right_top()],
        [inner.right_top(), inner.right_bottom()],
        [inner.left_bottom(), inner.right_bottom()],
        [inner.left_top(), inner.left_bottom()],
    ];
    let flags = [
        &mut sides.top,
        &mut sides.right,
        &mut sides.bottom,
        &mut sides.left,
    ];
    let click = response
        .clicked()
        .then(|| response.interact_pointer_pos())
        .flatten();
    let hover = response.hover_pos();
    for ((zone, i), flag) in edges.into_iter().zip(flags) {
        if click.is_some_and(|p| zone.contains(p)) {
            *flag = !*flag;
        }
        let color = if *flag {
            theme::ACCENT_2
        } else if hover.is_some_and(|p| zone.contains(p)) {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        };
        painter.line_segment(lines[i], Stroke::new(if *flag { 3.0 } else { 1.0 }, color));
    }
    response.on_hover_text("Clique un côté pour le déchirer");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_handles_resize_the_right_edges() {
        let r = Bounds::from_corners(Pt::new(10.0, 10.0), Pt::new(50.0, 40.0));
        let moved = move_crop_handle(r, 0, Pt::new(0.0, 5.0));
        assert_eq!(
            moved,
            Bounds::from_corners(Pt::new(0.0, 5.0), Pt::new(50.0, 40.0))
        );
        let moved = move_crop_handle(r, 3, Pt::new(80.0, 0.0));
        assert_eq!(
            moved,
            Bounds::from_corners(Pt::new(10.0, 10.0), Pt::new(80.0, 40.0))
        );
        // Poignée passée de l'autre côté : le rectangle se retourne proprement.
        let moved = move_crop_handle(r, 5, Pt::new(0.0, 0.0));
        assert_eq!(
            moved,
            Bounds::from_corners(Pt::new(10.0, 0.0), Pt::new(50.0, 10.0))
        );
        assert_eq!(crop_handles(r)[4], r.max);
    }

    #[test]
    fn background_choice_round_trips() {
        for bg in [
            Background::Transparent,
            Background::Extend,
            Background::Color {
                color: Rgba::rgb(1, 2, 3),
            },
        ] {
            assert_eq!(BgPick::from(bg).get(), bg);
        }
    }
}
