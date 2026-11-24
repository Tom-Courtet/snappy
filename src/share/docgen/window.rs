//! Fenêtre « Nouveau document » (`snappy --document <png>…`) : les captures
//! choisies, leur titre, leur description et la légende de leurs pastilles ; ordre,
//! retrait, ajout ; options et format ; « Générer » écrit le document dans le dossier
//! d'enregistrement et le montre dans le Finder.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use anyhow::{Context, Result};
use egui::{Frame, Margin, RichText, Stroke, TextureHandle, TextureOptions, Ui};

use super::{DocItem, DocOptions, Format};
use crate::config::Store;
use crate::history;
use crate::ui::icons;
use crate::ui::theme::{self, Typo};

/// Largeur des vignettes.
const THUMB: u32 = 180;

/// Ouvre la fenêtre dans un nouveau processus (une fenêtre egui par processus sous
/// macOS) : pour l'espace de travail, l'éditeur ou l'agent.
// API pour l'espace de travail et l'éditeur (branchée à la fusion des lots).
#[allow(dead_code)]
pub fn open(paths: &[PathBuf]) -> Result<()> {
    let exe = std::env::current_exe().context("chemin de l'exécutable")?;
    std::process::Command::new(exe)
        .arg("--document")
        .args(paths)
        .spawn()
        .context("lancement du générateur de documents")?;
    Ok(())
}

pub fn run(store: Store, paths: Vec<PathBuf>) -> Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Snappy · Nouveau document")
        .with_inner_size([760.0, 820.0])
        .with_min_inner_size([560.0, 420.0])
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
            crate::platform::show_in_dock();
            crate::platform::activate(cc);
            Ok(Box::new(DocWindow::new(store, paths, &cc.egui_ctx)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("générateur de documents : {e}"))
}

fn window_icon() -> egui::IconData {
    #[cfg(target_os = "macos")]
    let (rgba, size): (&[u8], u32) = (include_bytes!("../../../assets/icon_mac_256.rgba"), 256);
    #[cfg(not(target_os = "macos"))]
    let (rgba, size): (&[u8], u32) = (include_bytes!("../../../assets/icon_64.rgba"), 64);
    egui::IconData {
        rgba: rgba.to_vec(),
        width: size,
        height: size,
    }
}

/// Nom du gestionnaire de fichiers.
const FILE_MANAGER: &str = if cfg!(windows) {
    "l'Explorateur"
} else {
    "le Finder"
};

struct Entry {
    item: DocItem,
    thumb: Option<TextureHandle>,
}

enum Action {
    Up(usize),
    Down(usize),
    Remove(usize),
}

struct DocWindow {
    store: Store,
    entries: Vec<Entry>,
    options: DocOptions,
    format: Format,
    job: Option<Receiver<Result<PathBuf, String>>>,
    status: Option<(String, bool)>,
    last: Option<PathBuf>,
}

impl DocWindow {
    fn new(store: Store, paths: Vec<PathBuf>, ctx: &egui::Context) -> Self {
        let mut this = Self {
            store,
            entries: Vec::new(),
            options: DocOptions::default(),
            format: Format::default(),
            job: None,
            status: None,
            last: None,
        };
        this.add(paths, ctx);
        this
    }

    /// Ajoute des captures (les illisibles sont signalées et ignorées).
    fn add(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let mut refused = Vec::new();
        for path in paths {
            match thumbnail(&path, ctx) {
                Ok(thumb) => self.entries.push(Entry {
                    item: DocItem::from_capture(&path),
                    thumb: Some(thumb),
                }),
                Err(e) => {
                    log::warn!("{e:#}");
                    refused.push(
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    );
                }
            }
        }
        if !refused.is_empty() {
            self.status = Some((format!("Images illisibles : {}", refused.join(", ")), false));
        }
    }

    fn generate(&mut self, ctx: &egui::Context) {
        let items: Vec<DocItem> = self.entries.iter().map(|e| e.item.clone()).collect();
        let options = self.options.clone();
        let format = self.format;
        let dir = self.store.save_dir(&self.store.load());
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let out = super::new_path(&dir, format);
            let result = super::generate(&items, &options, format, &out)
                .map(|()| out)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.job = Some(rx);
        self.status = Some(("Génération…".into(), true));
    }

    fn poll(&mut self) {
        let Some(rx) = &self.job else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.job = None;
        match result {
            Ok(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                history::reveal(&path);
                self.status = Some((format!("{} Document enregistré : {name}", icons::OK), true));
                self.last = Some(path);
            }
            Err(e) => {
                self.status = Some((
                    format!("{} Document non généré : {e}", icons::WARNING),
                    false,
                ));
            }
        }
    }

    // --- Liste des captures ---------------------------------------------------------

    fn entries(&mut self, ui: &mut Ui) {
        let count = self.entries.len();
        let mut action = None;
        for (i, entry) in self.entries.iter_mut().enumerate() {
            card(ui, |ui| {
                ui.horizontal_top(|ui| {
                    if let Some(thumb) = &entry.thumb {
                        let size = thumb.size_vec2();
                        let scale = (THUMB as f32 / size.x).min(140.0 / size.y).min(1.0);
                        ui.add(egui::Image::new(thumb).fit_to_exact_size(size * scale));
                    }
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 44.0).max(160.0));
                        let name = entry
                            .item
                            .image_path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        ui.label(
                            RichText::new(format!("{}. {name}", i + 1))
                                .size(theme::SIZE_S)
                                .color(theme::TEXT_DIM),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut entry.item.title)
                                .hint_text("Titre")
                                .desired_width(f32::INFINITY),
                        );
                        ui.add(
                            egui::TextEdit::multiline(&mut entry.item.description)
                                .hint_text("Description (HTML simple : <b>, <i>, <u>, <br>)")
                                .desired_rows(2)
                                .desired_width(f32::INFINITY),
                        );
                        for (number, text) in &mut entry.item.step_descriptions {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("{number}.")).bold());
                                ui.add(
                                    egui::TextEdit::singleline(text)
                                        .hint_text("Légende de la pastille")
                                        .desired_width(f32::INFINITY),
                                );
                            });
                        }
                    });
                    ui.vertical(|ui| {
                        if ui
                            .add_enabled(i > 0, egui::Button::new("Monter"))
                            .on_hover_text("Placer cette image plus haut")
                            .clicked()
                        {
                            action = Some(Action::Up(i));
                        }
                        if ui
                            .add_enabled(i + 1 < count, egui::Button::new("Descendre"))
                            .on_hover_text("Placer cette image plus bas")
                            .clicked()
                        {
                            action = Some(Action::Down(i));
                        }
                        if ui
                            .button(format!("{} Retirer", icons::DELETE))
                            .on_hover_text("Retirer du document (la capture reste)")
                            .clicked()
                        {
                            action = Some(Action::Remove(i));
                        }
                    });
                });
            });
            ui.add_space(8.0);
        }
        match action {
            Some(Action::Up(i)) => self.entries.swap(i, i - 1),
            Some(Action::Down(i)) => self.entries.swap(i, i + 1),
            Some(Action::Remove(i)) => {
                self.entries.remove(i);
            }
            None => {}
        }
        if ui
            .button(format!("{} Ajouter des images…", icons::ADD))
            .clicked()
            && let Some(paths) = rfd::FileDialog::new()
                .set_title("Images à ajouter au document")
                .add_filter("Images PNG", &["png"])
                .set_directory(self.store.captures_dir())
                .pick_files()
        {
            let ctx = ui.ctx().clone();
            self.add(paths, &ctx);
        }
    }

    // --- Options et génération ------------------------------------------------------

    fn options(&mut self, ui: &mut Ui) {
        let o = &mut self.options;
        ui.horizontal(|ui| {
            ui.label("Titre du document");
            ui.add(
                egui::TextEdit::singleline(&mut o.document_title)
                    .hint_text("nom du fichier")
                    .desired_width(f32::INFINITY),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut o.landscape_for_large,
                "Paysage pour les grandes images",
            );
            ui.checkbox(
                &mut o.title_from_filename,
                "Nom du fichier comme titre par défaut",
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut o.image_before_text, "Image avant le texte");
            ui.checkbox(&mut o.page_break, "Saut de page entre l'image et le texte");
        });
        ui.horizontal(|ui| {
            ui.label("Format");
            for format in Format::ALL {
                ui.radio_value(&mut self.format, format, format.label());
            }
            if self.format == Format::Gif {
                let mut seconds = o.gif_delay_ms as f32 / 1000.0;
                ui.label("Délai");
                if ui
                    .add(
                        egui::DragValue::new(&mut seconds)
                            .range(0.1..=60.0)
                            .speed(0.1)
                            .suffix(" s"),
                    )
                    .changed()
                {
                    o.gif_delay_ms = (seconds * 1000.0).round() as u32;
                }
            }
        });
    }

    fn actions(&mut self, ui: &mut Ui, frame: &eframe::Frame) {
        ui.horizontal(|ui| {
            let busy = self.job.is_some();
            let enabled = !busy && !self.entries.is_empty();
            let generate = egui::Button::new(
                RichText::new(format!("{} Générer", icons::SAVE))
                    .bold()
                    .color(theme::ON_ACCENT_2),
            )
            .fill(theme::ACCENT_2);
            if ui.add_enabled(enabled, generate).clicked() {
                let ctx = ui.ctx().clone();
                self.generate(&ctx);
            }
            if busy {
                ui.spinner();
            }
            if let Some(path) = self.last.clone() {
                if ui
                    .button(format!("{} Afficher dans {FILE_MANAGER}", icons::FOLDER))
                    .clicked()
                {
                    history::reveal(&path);
                }
                let share = ui.button(format!("{} Partager…", icons::OPEN));
                if share.clicked() {
                    match crate::share::share_sheet_from_frame(frame, share.rect, &[path]) {
                        Ok(delivery) => {
                            if let Some(message) = delivery.message() {
                                self.status = Some((message.into(), true));
                            }
                        }
                        Err(e) => {
                            self.status = Some((format!("{} {e:#}", icons::WARNING), false));
                        }
                    }
                }
            }
        });
        if let Some((text, ok)) = &self.status {
            let color = if *ok { theme::TEXT_DIM } else { theme::WARNING };
            ui.label(RichText::new(text).size(theme::SIZE_S).color(color));
        }
    }
}

impl eframe::App for DocWindow {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        self.poll();
        egui::Panel::top("header")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(16, 12)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(icons::IMAGE)
                            .size(theme::SIZE_ICON)
                            .color(theme::ACCENT_2),
                    );
                    ui.label(
                        RichText::new("Nouveau document")
                            .title()
                            .size(theme::SIZE_L),
                    );
                    ui.label(
                        RichText::new(format!("{} image(s)", self.entries.len()))
                            .color(theme::TEXT_DIM),
                    );
                });
            });
        egui::Panel::bottom("options")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(16, 12)),
            )
            .show(ui, |ui| {
                self.options(ui);
                ui.add_space(8.0);
                self.actions(ui, frame);
            });
        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.entries(ui));
            });
    }
}

fn card(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(theme::CARD_RADIUS)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
}

/// Vignette de la capture avec ses annotations.
fn thumbnail(path: &std::path::Path, ctx: &egui::Context) -> Result<TextureHandle> {
    let img = crate::share::flattened(path)?;
    let h = (img.height() as u64 * u64::from(THUMB) / u64::from(img.width().max(1))).max(1);
    let small = if img.width() > THUMB {
        image::imageops::thumbnail(&img, THUMB, h as u32)
    } else {
        img
    };
    let color = egui::ColorImage::from_rgba_unmultiplied(
        [small.width() as usize, small.height() as usize],
        small.as_raw(),
    );
    Ok(ctx.load_texture(path.to_string_lossy(), color, TextureOptions::LINEAR))
}

#[cfg(test)]
mod tests {
    use egui::epaint::text::{Fonts, TextOptions};
    use egui::{FontFamily, FontId};

    /// Comme `ui::tests::every_ui_character_has_a_glyph`, pour cette fenêtre.
    #[test]
    fn every_window_character_has_a_glyph() {
        let mut fonts = Fonts::new(TextOptions::default(), crate::ui::theme::fonts());
        let font = FontId::new(14.0, FontFamily::Proportional);
        for source in [include_str!("window.rs"), include_str!("../mod.rs")] {
            for line in source.lines() {
                let code = line.split("//").next().unwrap_or("");
                for (i, part) in code.split('"').enumerate() {
                    if i % 2 == 1 {
                        for c in part.chars().filter(|c| !c.is_ascii()) {
                            assert!(fonts.has_glyph(&font, c), "{c:?} absent : {line}");
                        }
                    }
                }
            }
        }
    }
}
