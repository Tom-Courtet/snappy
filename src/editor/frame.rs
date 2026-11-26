//! Poignées de cadre et « Options de dessin » (demandés après essai, d'après
//! Screenpresso) : la forme sélectionnée a 8 carrés noirs pour l'étirer, et un carré
//! orange en haut à droite ouvre ses options. Ce carré est aussi le coin haut droit
//! du cadre : un glisser l'étire, un clic ouvre les options.
//!
//! Les poignées de cadre passent par `Drag::Handle`, numérotées à partir de
//! [`FIRST`] pour ne pas se confondre avec les poignées propres aux formes (bouts des
//! flèches, pointes…), qui restent là où il n'y a pas de cadre.

use egui::{Color32, CursorIcon, Pos2, Rect, RichText, Stroke, StrokeKind, Ui, vec2};

use super::Editor;
use super::preview::View;
use super::tools::is_stroked;
use crate::model::{self, Annotation, Bounds, Pt, Shape};
use crate::render;

/// Numéro de la première poignée de cadre dans `Drag::Handle`.
pub(super) const FIRST: usize = 1000;
/// Poignée de cadre qui porte le carré des options (coin haut droit).
const OPTIONS_HANDLE: usize = 2;
/// Côté d'une poignée de cadre, en points d'écran.
const SIDE: f32 = 7.0;
/// Côté du carré des options.
const OPTIONS_SIDE: f32 = 9.0;
/// Rouge orangé du carré des options.
const OPTIONS_COLOR: Color32 = Color32::from_rgb(0xff, 0x55, 0x1e);
/// Marge autour d'une poignée pour l'attraper.
const GRAB: f32 = 4.0;

/// Curseur au-dessus d'une poignée (`Drag::Handle`).
pub(super) fn cursor(handle: usize) -> CursorIcon {
    match handle.checked_sub(FIRST) {
        Some(0 | 4) => CursorIcon::ResizeNwSe,
        Some(2 | 6) => CursorIcon::ResizeNeSw,
        Some(1 | 5) => CursorIcon::ResizeVertical,
        Some(_) => CursorIcon::ResizeHorizontal,
        None => CursorIcon::Grab,
    }
}

/// Poignée de cadre sous le point d'écran `p` (la plus proche).
pub(super) fn handle_at(frame: Bounds, p: Pos2, view: View) -> Option<usize> {
    model::frame_handles(frame)
        .iter()
        .enumerate()
        .map(|(k, h)| (k, view.to_screen(*h).distance(p)))
        .filter(|(_, d)| *d <= OPTIONS_SIDE / 2.0 + GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(k, _)| k)
}

impl Editor {
    /// Cadre de l'annotation `i`, si elle est seule sélectionnée et se règle par son
    /// cadre.
    pub(super) fn frame_of(&self, i: usize) -> Option<Bounds> {
        if self.text.is_some() || self.selection().len() > 1 {
            return None;
        }
        self.doc.annotations.get(i)?.frame(render::text_size)
    }

    /// Carré des options de l'annotation sélectionnée, à l'écran.
    fn options_square(&self, view: View) -> Option<(usize, Rect)> {
        let i = self.selected?;
        if self.text.is_some() || self.selection().len() > 1 {
            return None;
        }
        let ann = self.doc.annotations.get(i)?;
        let corner = match self.frame_of(i) {
            Some(frame) => view.to_screen(model::frame_handles(frame)[OPTIONS_HANDLE]),
            // Coin haut droit du cadre pointillé (voir `preview::selection`).
            None => view
                .rect(ann.bounds(render::text_size))
                .expand(4.0)
                .right_top(),
        };
        Some((
            i,
            Rect::from_center_size(corner, vec2(OPTIONS_SIDE, OPTIONS_SIDE)),
        ))
    }

    /// Étire l'annotation `index` : la poignée de cadre `handle` est en `p`. Le
    /// calcul repart toujours de l'annotation au début du glisser.
    pub(super) fn stretch(&mut self, index: usize, handle: usize, p: Pt, shift: bool) {
        let start = match &self.frame_start {
            Some((j, ann)) if *j == index => ann.clone(),
            _ => {
                let Some(ann) = self.doc.annotations.get(index).cloned() else {
                    return;
                };
                self.frame_start = Some((index, ann.clone()));
                ann
            }
        };
        let Some(from) = start.frame(render::text_size) else {
            return;
        };
        // ⇧ : proportions gardées pour toutes les formes.
        let to = model::drag_frame(from, handle, p, shift || start.keeps_ratio());
        let mut ann = start;
        ann.fit(from, to);
        if let Some(slot) = self.doc.annotations.get_mut(index) {
            *slot = ann;
        }
    }

    /// Clic sur le carré des options : ouvre ou ferme « Options de dessin ». Tout
    /// autre clic sur l'image les ferme. `true` : le clic est traité ici.
    pub(super) fn options_click(&mut self, response: &egui::Response, view: View) -> bool {
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
            && let Some((i, square)) = self.options_square(view)
            && square.expand(GRAB).contains(p)
        {
            self.drawing_options = if self.drawing_options == Some(i) {
                None
            } else {
                Some(i)
            };
            return true;
        }
        if response.clicked() || response.drag_started() || response.secondary_clicked() {
            self.drawing_options = None;
        }
        false
    }

    /// Cadre, 8 poignées et carré des options de l'annotation sélectionnée.
    pub(super) fn paint_frame(&self, ctx: &egui::Context, painter: &egui::Painter, view: View) {
        let Some(i) = self.selected else { return };
        if let Some(frame) = self.frame_of(i) {
            let r = view.rect(frame);
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(1.0, Color32::from_black_alpha(150)),
                StrokeKind::Middle,
            );
            for (k, h) in model::frame_handles(frame).iter().enumerate() {
                if k == OPTIONS_HANDLE {
                    continue;
                }
                let square = Rect::from_center_size(view.to_screen(*h), vec2(SIDE, SIDE));
                painter.rect_filled(square, 0.0, Color32::BLACK);
                painter.rect_stroke(
                    square,
                    0.0,
                    Stroke::new(1.0, Color32::from_white_alpha(200)),
                    StrokeKind::Outside,
                );
            }
        }
        if let Some((_, square)) = self.options_square(view) {
            let open = self.drawing_options.is_some();
            painter.rect_filled(square, 0.0, OPTIONS_COLOR);
            painter.rect_stroke(
                square,
                0.0,
                if open {
                    Stroke::new(1.5, Color32::WHITE)
                } else {
                    Stroke::new(1.0, Color32::BLACK)
                },
                StrokeKind::Outside,
            );
            // Sans cadre, le carré n'est qu'un bouton.
            if self.drag.is_none()
                && self.frame_of(i).is_none()
                && ctx
                    .pointer_hover_pos()
                    .is_some_and(|p| square.expand(GRAB).contains(p))
            {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
        }
    }

    /// Fenêtre « Options de dessin », à côté du carré orange.
    pub(super) fn options_window(&mut self, ctx: &egui::Context, view: View) {
        let Some(i) = self.drawing_options else {
            return;
        };
        let square = match self.options_square(view) {
            Some((j, square)) if j == i => square,
            _ => {
                self.drawing_options = None;
                return;
            }
        };
        // Un réglage tiré au curseur ne fait qu'un pas d'annulation.
        if !ctx.input(|inp| inp.pointer.any_down()) {
            self.options_editing = false;
        }
        egui::Area::new(egui::Id::unique("options_de_dessin"))
            .order(egui::Order::Foreground)
            .fixed_pos(square.right_top() + vec2(10.0, 0.0))
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(340.0);
                    self.options_contents(ui, i);
                });
            });
    }

    fn options_contents(&mut self, ui: &mut Ui, i: usize) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Options de dessin").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Fermer").clicked() {
                    self.drawing_options = None;
                }
            });
        });
        ui.separator();
        let Some(ann) = self.doc.annotations.get(i).cloned() else {
            return;
        };
        egui::Grid::new("options_de_dessin_grille")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Couleur :");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    self.color_options(ui);
                });
                ui.end_row();

                let mut pct = (ann.opacity * 100.0).round();
                ui.label("Opacité :");
                if ui
                    .add(
                        egui::Slider::new(&mut pct, 10.0..=100.0)
                            .suffix(" %")
                            .integer(),
                    )
                    .changed()
                {
                    self.style.opacity = pct / 100.0;
                    self.edit_annotation(i, |a| a.opacity = pct / 100.0);
                }
                ui.end_row();

                if is_stroked(&ann.shape) || matches!(ann.shape, Shape::Step { .. }) {
                    let mut width = ann.width.round();
                    ui.label("Épaisseur :");
                    if ui
                        .add(
                            egui::Slider::new(&mut width, 1.0..=40.0)
                                .suffix(" px")
                                .integer(),
                        )
                        .changed()
                    {
                        self.edit_annotation(i, |a| a.width = width);
                    }
                    ui.end_row();
                }

                let size = match &ann.shape {
                    Shape::Text { size, .. }
                    | Shape::Bubble { size, .. }
                    | Shape::Label { size, .. } => Some(("Taille du texte :", *size, 6.0..=200.0)),
                    Shape::Stamp { size, .. } => Some(("Taille :", *size, 8.0..=800.0)),
                    _ => None,
                };
                if let Some((label, size, range)) = size {
                    let mut v = size.round();
                    ui.label(label);
                    if ui
                        .add(egui::Slider::new(&mut v, range).suffix(" px").integer())
                        .changed()
                    {
                        self.edit_annotation(i, |a| set_size(a, v));
                    }
                    ui.end_row();
                }

                if let Shape::Rect { radius, .. } = ann.shape {
                    let mut v = radius.round();
                    ui.label("Coins arrondis :");
                    if ui
                        .add(
                            egui::Slider::new(&mut v, 0.0..=80.0)
                                .suffix(" px")
                                .integer(),
                        )
                        .changed()
                    {
                        self.style.rounded = v > 0.0;
                        self.edit_annotation(i, |a| {
                            if let Shape::Rect { radius, .. } = &mut a.shape {
                                *radius = v;
                            }
                        });
                    }
                    ui.end_row();
                }
            });
        // Les options de la barre pour cette forme (type de trait, plein, ombre…).
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            self.in_drawing_options = true;
            self.shape_options(ui);
            self.extra_options(ui);
            self.in_drawing_options = false;
        });
    }

    /// Modifie l'annotation `i` depuis « Options de dessin ».
    fn edit_annotation(&mut self, i: usize, f: impl FnOnce(&mut Annotation)) {
        if !self.options_editing {
            self.checkpoint();
            self.options_editing = true;
        }
        if let Some(ann) = self.doc.annotations.get_mut(i) {
            f(ann);
        }
        self.changed();
    }
}

/// Taille de police (textes, bulles, tampons texte) ou côté (tampons).
fn set_size(ann: &mut Annotation, v: f32) {
    match &mut ann.shape {
        Shape::Text { size, .. }
        | Shape::Bubble { size, .. }
        | Shape::Label { size, .. }
        | Shape::Stamp { size, .. } => *size = v,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_frame_handle_is_grabbed() {
        let view = View {
            origin: Pos2::ZERO,
            scale: 1.0,
        };
        let frame = Bounds::from_corners(Pt::new(0.0, 0.0), Pt::new(100.0, 50.0));
        assert_eq!(handle_at(frame, Pos2::new(101.0, -1.0), view), Some(2));
        assert_eq!(handle_at(frame, Pos2::new(50.0, 52.0), view), Some(5));
        assert_eq!(handle_at(frame, Pos2::new(50.0, 25.0), view), None);
        assert_eq!(cursor(FIRST + 3), CursorIcon::ResizeHorizontal);
        assert_eq!(cursor(1), CursorIcon::Grab);
    }
}
