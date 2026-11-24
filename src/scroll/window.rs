//! Fenêtre de contrôle de la capture avec défilement : aperçu en direct de
//! l'image assemblée, sa taille, et Terminé / Recommencer / Annuler.
//!
//! Toujours au premier plan, placée hors de la zone, exclue des captures, et
//! ouverte sans prendre le focus : l'application qu'on fait défiler garde le
//! clavier (flèches, Page suivante, espace).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

use anyhow::Result;
use egui::{
    Color32, CornerRadius, Frame, Margin, RichText, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, vec2,
};

use super::native::{self, Rect};
use super::stitch::{Axis, Stitcher};
use super::{MAX_LEN, Shared, Status, Zone};
use crate::ui::{icons, theme};

/// Taille de la fenêtre, en points.
const PANEL: (f32, f32) = (280.0, 520.0);
/// Cadre de l'aperçu, en points.
const PREVIEW: egui::Vec2 = vec2(248.0, 300.0);
/// Écart entre la zone et la fenêtre.
const MARGIN: f64 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Done,
    Cancel,
}

/// Ouvre la fenêtre et le fil de copie ; rend le choix de l'utilisateur et
/// l'assemblage (fil arrêté).
pub fn run(zone: Zone, shared: Arc<Shared>) -> Result<(Decision, Option<Stitcher>)> {
    let screen = Rect {
        x: zone.frame.x,
        y: zone.frame.y,
        w: zone.frame.width,
        h: zone.frame.height,
    };
    let top_left = native::place_panel(
        screen,
        zone.points(),
        (f64::from(PANEL.0), f64::from(PANEL.1)),
        MARGIN,
    );
    let viewport = egui::ViewportBuilder::default()
        .with_title("Snappy · Capture avec défilement")
        .with_inner_size([PANEL.0, PANEL.1])
        .with_position([top_left.0 as f32, top_left.1 as f32])
        .with_resizable(false)
        .with_always_on_top()
        .with_active(false);
    let options = eframe::NativeOptions {
        viewport,
        persist_window: false,
        event_loop_builder: Some(Box::new(|builder| {
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
                // Ni icône dans le Dock, ni vol du focus à l'app qu'on fait défiler.
                builder
                    .with_activation_policy(ActivationPolicy::Accessory)
                    .with_activate_ignoring_other_apps(false)
                    .with_default_menu(false);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = builder;
        })),
        ..Default::default()
    };
    let decision = Rc::new(Cell::new(Decision::Cancel));
    let worker: Rc<RefCell<Option<JoinHandle<Stitcher>>>> = Rc::new(RefCell::new(None));
    let (out, slot, app_shared) = (
        Rc::clone(&decision),
        Rc::clone(&worker),
        Arc::clone(&shared),
    );
    let scale = zone.scale;
    let result = eframe::run_native(
        "Snappy",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            native::prepare(cc, top_left, scale);
            let ctx = cc.egui_ctx.clone();
            let thread_shared = Arc::clone(&app_shared);
            let handle = std::thread::Builder::new()
                .name("défilement".into())
                .spawn(move || super::worker(zone, thread_shared, move || ctx.request_repaint()))?;
            *slot.borrow_mut() = Some(handle);
            Ok(Box::new(Panel::new(app_shared, out)))
        }),
    );
    shared.stop.store(true, Ordering::Relaxed);
    let stitcher = worker.borrow_mut().take().and_then(|h| h.join().ok());
    result.map_err(|e| anyhow::anyhow!("capture avec défilement : {e}"))?;
    Ok((decision.get(), stitcher))
}

struct Panel {
    shared: Arc<Shared>,
    decision: Rc<Cell<Decision>>,
    texture: Option<TextureHandle>,
    texture_gen: u64,
    /// Partie visible déjà montrée dans l'aperçu (pour n'y défiler qu'au changement).
    shown_view: Option<(u32, u32)>,
    auto: bool,
}

impl Panel {
    fn new(shared: Arc<Shared>, decision: Rc<Cell<Decision>>) -> Self {
        Self {
            shared,
            decision,
            texture: None,
            texture_gen: 0,
            shown_view: None,
            auto: false,
        }
    }

    fn finish(&self, ui: &Ui, decision: Decision) {
        self.decision.set(decision);
        self.shared.stop.store(true, Ordering::Relaxed);
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Aperçu : l'image assemblée à la largeur du cadre (à sa hauteur en
    /// horizontal), la partie visible encadrée et suivie.
    fn preview(&mut self, ui: &mut Ui, view: Option<(u32, u32)>, axis: Option<Axis>) {
        Frame::new()
            .fill(theme::SURFACE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                ui.set_min_size(PREVIEW);
                ui.set_max_size(PREVIEW);
                let Some(texture) = &self.texture else {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Aperçu").color(theme::TEXT_DIM));
                    });
                    return;
                };
                let [tw, th] = texture.size().map(|v| v as f32);
                let horizontal = axis == Some(Axis::Horizontal);
                let size = if horizontal {
                    vec2(tw * PREVIEW.y / th, PREVIEW.y)
                } else {
                    vec2(PREVIEW.x, th * PREVIEW.x / tw)
                };
                let id = texture.id();
                let follow = view != self.shown_view;
                self.shown_view = view;
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                        ui.painter().image(
                            id,
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        let Some((start, len)) = view else {
                            return;
                        };
                        let total = self.shared.state.lock().expect("état").size;
                        let Some((fw, fh)) = total else {
                            return;
                        };
                        let shown = if horizontal {
                            let k = rect.width() / fw as f32;
                            egui::Rect::from_min_size(
                                rect.min + vec2(start as f32 * k, 0.0),
                                vec2(len as f32 * k, rect.height()),
                            )
                        } else {
                            let k = rect.height() / fh as f32;
                            egui::Rect::from_min_size(
                                rect.min + vec2(0.0, start as f32 * k),
                                vec2(rect.width(), len as f32 * k),
                            )
                        };
                        ui.painter().rect_stroke(
                            shown,
                            CornerRadius::same(2),
                            Stroke::new(1.5, theme::ACCENT_2),
                            StrokeKind::Inside,
                        );
                        if follow {
                            ui.scroll_to_rect(shown, None);
                        }
                    });
            });
    }
}

/// « 1 600 × 4 820 px » (espaces insécables entre les milliers).
fn size_text((w, h): (u32, u32)) -> String {
    format!("{} × {} px", group(w), group(h))
}

fn group(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{a0}');
        }
        out.push(c);
    }
    out
}

impl eframe::App for Panel {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let (status, size, view, axis, fps, grab_ms, error, note, preview) = {
            let mut st = self.shared.state.lock().expect("état");
            let preview = (st.preview_gen != self.texture_gen)
                .then(|| st.preview.clone())
                .flatten();
            self.texture_gen = st.preview_gen;
            (
                st.status,
                st.size,
                st.view,
                st.axis,
                st.fps,
                st.grab_ms,
                st.error.clone(),
                st.auto_note.take(),
                preview,
            )
        };
        if let Some(img) = preview {
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            match &mut self.texture {
                Some(t) => t.set(color, TextureOptions::LINEAR),
                None => {
                    self.texture = Some(ui.ctx().load_texture(
                        "aperçu",
                        color,
                        TextureOptions::LINEAR,
                    ));
                }
            }
        }
        if note.is_some() {
            self.auto = self.shared.auto.load(Ordering::Relaxed);
        }
        if let Some(note) = &note {
            log::info!("{note}");
        }

        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Défilement").font(theme::strong(theme::SIZE_L)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(s) = size {
                            ui.label(RichText::new(size_text(s)).color(theme::TEXT_DIM));
                        }
                    });
                });
                self.preview(ui, view, axis);

                let (text, color) = match (&error, status) {
                    (Some(e), _) => (format!("{} {e}", icons::WARNING), theme::DANGER),
                    (None, Status::Waiting) => (
                        "Faites défiler la zone : molette, barre de défilement ou clavier."
                            .to_owned(),
                        theme::TEXT_DIM,
                    ),
                    (None, Status::Running) => (
                        "Continuez, ou cliquez sur Terminé. Vous pouvez remonter sans doublon."
                            .to_owned(),
                        theme::TEXT_DIM,
                    ),
                    (None, Status::Lost) => (
                        format!(
                            "{} Raccord perdu : revenez un peu en arrière ou défilez plus lentement.",
                            icons::WARNING
                        ),
                        theme::WARNING,
                    ),
                    (None, Status::Full) => (
                        format!(
                            "{} Longueur maximale atteinte ({} px) : cliquez sur Terminé.",
                            icons::WARNING,
                            group(MAX_LEN)
                        ),
                        theme::WARNING,
                    ),
                };
                ui.label(RichText::new(text).color(color).size(theme::SIZE_S));

                if ui
                    .checkbox(&mut self.auto, "Défilement automatique")
                    .on_hover_text(
                        "Simule la molette au centre de la zone jusqu'au bas de la page. \
                         macOS : autorisation « Accessibilité » requise.",
                    )
                    .changed()
                {
                    self.shared.auto.store(self.auto, Ordering::Relaxed);
                }
                if let Some(note) = note {
                    ui.label(RichText::new(note).color(theme::TEXT_DIM).size(theme::SIZE_S));
                }

                ui.add_space(4.0);
                let full = vec2(ui.available_width(), 34.0);
                let done = egui::Button::new(
                    RichText::new(format!("{} Terminé", icons::OK))
                        .color(theme::ON_ACCENT_2)
                        .strong(),
                )
                .fill(theme::ACCENT_2)
                .min_size(full);
                let has_image = size.is_some();
                if ui.add_enabled(has_image, done).on_hover_text("Entrée").clicked()
                    || (has_image && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    self.finish(ui, Decision::Done);
                }
                let half = vec2((ui.available_width() - 8.0) / 2.0, 30.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(format!("{} Recommencer", icons::UNDO)).min_size(half))
                        .clicked()
                    {
                        self.shared.restart.store(true, Ordering::Relaxed);
                        self.shown_view = None;
                    }
                    if ui
                        .add(egui::Button::new(format!("{} Annuler", icons::CLOSE)).min_size(half))
                        .on_hover_text("Échap")
                        .clicked()
                    {
                        self.finish(ui, Decision::Cancel);
                    }
                });
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.finish(ui, Decision::Cancel);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.label(
                        RichText::new(format!("{fps:.0} im/s · copie {grab_ms:.0} ms"))
                            .color(theme::TEXT_DIM)
                            .size(theme::SIZE_XS),
                    );
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_grouped_by_thousands() {
        assert_eq!(group(7), "7");
        assert_eq!(group(999), "999");
        assert_eq!(group(1600), "1\u{a0}600");
        assert_eq!(group(30_000), "30\u{a0}000");
        assert_eq!(group(1_234_567), "1\u{a0}234\u{a0}567");
        assert_eq!(size_text((1600, 4820)), "1\u{a0}600 × 4\u{a0}820 px");
    }

    /// Les textes de ce module s'affichent avec les polices embarquées (même
    /// vérification que `ui::tests` pour les autres fenêtres).
    #[test]
    fn every_character_has_a_glyph() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontFamily, FontId};
        let mut fonts = Fonts::new(TextOptions::default(), theme::fonts());
        let font = FontId::new(14.0, FontFamily::Proportional);
        let sources = [include_str!("window.rs"), include_str!("mod.rs")];
        let extra = ['\u{a0}'];
        for source in sources {
            for line in source.lines() {
                let code = line.split("//").next().unwrap_or("");
                for (i, part) in code.split('"').enumerate() {
                    if i % 2 == 0 {
                        continue;
                    }
                    for c in part.chars().filter(|c| !c.is_ascii()) {
                        assert!(fonts.has_glyph(&font, c), "{c:?} absent : {line}");
                    }
                }
            }
        }
        for c in extra {
            assert!(fonts.has_glyph(&font, c), "{c:?} absent");
        }
        for icon in [icons::OK, icons::UNDO, icons::CLOSE, icons::WARNING] {
            assert!(fonts.has_glyph(&font, icon.chars().next().unwrap()));
        }
    }
}
