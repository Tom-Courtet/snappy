//! Fenêtre de résultat de l'OCR et de la lecture des codes : texte modifiable,
//! « Copier tout », « Copier la sélection » ; liste des codes lus.

use std::time::{Duration, Instant};

use anyhow::Result;
use egui::{Frame, Key, Margin, RichText, Stroke, Ui};

use super::Outcome;
use super::codes::Code;
use crate::platform;
use crate::ui::icons;
use crate::ui::theme::{self, Typo};

const STATUS_DURATION: Duration = Duration::from_secs(3);

pub fn run(outcome: Outcome) -> Result<()> {
    let (title, size) = match &outcome {
        Outcome::Text { .. } => ("Snappy · Texte reconnu", [560.0, 420.0]),
        Outcome::Codes { .. } => ("Snappy · Codes lus", [520.0, 300.0]),
        Outcome::Error { .. } => ("Snappy", [440.0, 200.0]),
    };
    let viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(size)
        .with_min_inner_size([360.0, 180.0])
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
            platform::show_in_dock();
            platform::activate(cc);
            Ok(Box::new(ResultWindow::new(outcome)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("fenêtre de résultat : {e}"))
}

/// Icône de la fenêtre (et du Dock sous macOS), comme celle des réglages.
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

struct ResultWindow {
    outcome: Outcome,
    /// Dernière sélection non vide de la zone de texte : le clic sur un bouton lui
    /// retire le focus, pas la sélection mémorisée.
    selection: String,
    status: Option<(String, Instant)>,
}

impl ResultWindow {
    fn new(outcome: Outcome) -> Self {
        let status = match &outcome {
            Outcome::Codes {
                opened: Some(url), ..
            } => Some(format!("{} Ouvert dans le navigateur : {url}", icons::OPEN)),
            Outcome::Codes { copied: true, .. } => {
                Some(format!("{} Premier code copié", icons::OK))
            }
            _ => None,
        }
        .map(|s| (s, Instant::now()));
        Self {
            outcome,
            selection: String::new(),
            status,
        }
    }

    fn copy(&mut self, text: &str) {
        let message = match super::copy_text(text) {
            Ok(()) => format!("{} Copié", icons::OK),
            Err(e) => format!("{} {e:#}", icons::WARNING),
        };
        self.status = Some((message, Instant::now()));
    }

    fn text_view(&mut self, ui: &mut Ui) {
        let Outcome::Text { text, .. } = &mut self.outcome else {
            return;
        };
        if text.is_empty() {
            ui.label(RichText::new("Aucun texte reconnu dans la zone.").color(theme::TEXT_DIM));
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let output = egui::TextEdit::multiline(text)
                    .desired_width(f32::INFINITY)
                    .desired_rows(12)
                    .font(egui::FontId::proportional(theme::SIZE_M))
                    .show(ui);
                if let Some(range) = output.cursor_range
                    && !range.is_empty()
                {
                    self.selection = range.slice_str(text).to_owned();
                }
            });
    }

    fn codes_view(&mut self, ui: &mut Ui) {
        let Outcome::Codes { codes, .. } = &self.outcome else {
            return;
        };
        if codes.is_empty() {
            ui.label(
                RichText::new("Aucun QR code ni code-barres trouvé dans la zone.")
                    .color(theme::TEXT_DIM),
            );
            return;
        }
        let codes: Vec<Code> = codes.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for code in &codes {
                    self.code_row(ui, code);
                    ui.add_space(6.0);
                }
            });
    }

    fn code_row(&mut self, ui: &mut Ui, code: &Code) {
        Frame::new()
            .fill(theme::SURFACE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(10)
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&code.kind)
                            .size(theme::SIZE_S)
                            .color(theme::ACCENT_2),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(format!("{} Copier", icons::COPY)).clicked() {
                            self.copy(&code.text);
                        }
                        if let Some(url) = code.web_url()
                            && ui.button(format!("{} Ouvrir", icons::OPEN)).clicked()
                        {
                            super::open_url(url);
                        }
                    });
                });
                ui.add(egui::Label::new(RichText::new(&code.text).size(theme::SIZE_M)).wrap());
            });
    }

    fn header(&self, ui: &mut Ui) {
        let (icon, title, detail) = match &self.outcome {
            Outcome::Text { info, millis, .. } => (
                icons::TEXT,
                "Texte reconnu",
                format!("{info} · {millis} ms"),
            ),
            Outcome::Codes { codes, .. } => (
                icons::REGION,
                "Codes lus",
                match codes.len() {
                    0 => "aucun".to_owned(),
                    1 => "1 code".to_owned(),
                    n => format!("{n} codes"),
                },
            ),
            Outcome::Error { title, .. } => (icons::WARNING, title.as_str(), String::new()),
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(icon)
                    .size(theme::SIZE_L)
                    .color(theme::ACCENT_2),
            );
            ui.label(RichText::new(title).bold().size(theme::SIZE_L));
        });
        if !detail.is_empty() {
            ui.label(
                RichText::new(detail)
                    .size(theme::SIZE_S)
                    .color(theme::TEXT_DIM),
            );
        }
    }

    fn footer(&mut self, ui: &mut Ui) -> bool {
        let mut close = false;
        ui.horizontal(|ui| {
            if let Outcome::Text { text, .. } = &self.outcome {
                let all = text.clone();
                let primary = egui::Button::new(
                    RichText::new(format!("{} Copier tout", icons::COPY)).color(theme::ON_ACCENT_2),
                )
                .fill(theme::ACCENT_2);
                if ui.add_enabled(!all.is_empty(), primary).clicked() {
                    self.copy(&all);
                }
                let selection = self.selection.clone();
                if ui
                    .add_enabled(
                        !selection.is_empty(),
                        egui::Button::new(format!("{} Copier la sélection", icons::COPY)),
                    )
                    .clicked()
                {
                    self.copy(&selection);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(format!("{} Fermer", icons::CLOSE)).clicked() {
                    close = true;
                }
                if let Some((message, since)) = &self.status {
                    if since.elapsed() < STATUS_DURATION {
                        ui.label(
                            RichText::new(message)
                                .size(theme::SIZE_S)
                                .color(theme::TEXT_DIM),
                        );
                        ui.ctx().request_repaint_after(STATUS_DURATION);
                    } else {
                        self.status = None;
                    }
                }
            });
        });
        close
    }
}

impl eframe::App for ResultWindow {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("header")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(16, 12)),
            )
            .show(ui, |ui| self.header(ui));
        let mut close = false;
        egui::Panel::bottom("actions")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(16, 10)),
            )
            .show_separator_line(false)
            .show(ui, |ui| close = self.footer(ui));
        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| match &self.outcome {
                Outcome::Text { .. } => self.text_view(ui),
                Outcome::Codes { .. } => self.codes_view(ui),
                Outcome::Error { message, .. } => {
                    ui.add(egui::Label::new(RichText::new(message).color(theme::WARNING)).wrap());
                }
            });
        if close || ctx.input(|i| i.key_pressed(Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::text::{Fonts, TextOptions};
    use egui::{FontFamily, FontId};

    /// Chaque caractère non ASCII des textes affichés existe dans les polices
    /// embarquées (pas de carré à la place d'un accent ou d'un symbole).
    #[test]
    fn every_window_character_has_a_glyph() {
        let mut fonts = Fonts::new(TextOptions::default(), crate::ui::theme::fonts());
        let font = FontId::new(14.0, FontFamily::Proportional);
        for (file, source) in [
            ("vision/window.rs", include_str!("window.rs")),
            ("vision/mod.rs", include_str!("mod.rs")),
            ("vision/sensitive.rs", include_str!("sensitive.rs")),
        ] {
            for line in source.lines() {
                let code = line.split("//").next().unwrap_or("");
                for part in code.split('"').skip(1).step_by(2) {
                    for c in part.chars().filter(|c| !c.is_ascii()) {
                        assert!(fonts.has_glyph(&font, c), "{file} : {c:?} absent");
                    }
                }
            }
        }
    }
}
