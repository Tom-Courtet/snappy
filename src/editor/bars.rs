//! Barres de l'éditeur : outils et export (en haut), options du style (en dessous,
//! selon l'outil ou l'annotation sélectionnée), état (en bas).

use egui::{Align, Layout, RichText, Sense, Stroke, Ui, Vec2};

use super::Editor;
use super::prefs::Preset;
use super::preview;
use super::tools::{STAMPS, Size, TOOLS, Tool, tool_of};
use crate::history;
use crate::model::{ArrowStyle, PALETTE, Rgba};
use crate::ui::{icons, keys, theme};

/// Texte d'icône à la taille des barres.
fn icon(s: &str) -> RichText {
    RichText::new(s).size(theme::SIZE_ICON)
}

/// Choix sélectionnable : fond lime et texte noir quand il est actif.
fn choice(ui: &mut Ui, selected: bool, text: RichText) -> egui::Response {
    let text = if selected {
        text.color(theme::ON_ACCENT_2)
    } else {
        text
    };
    ui.selectable_label(selected, text)
}

/// Pastille de couleur cliquable.
fn swatch(ui: &mut Ui, color: Rgba, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    let painter = ui.painter();
    let c = rect.center();
    if selected {
        painter.circle_stroke(c, 10.0, Stroke::new(2.0, theme::ACCENT_2));
    } else if response.hovered() {
        painter.circle_stroke(c, 10.0, Stroke::new(1.0, theme::TEXT_DIM));
    }
    painter.circle_filled(c, 7.0, preview::color32(color));
    painter.circle_stroke(c, 7.0, Stroke::new(1.0, theme::BORDER));
    response
}

impl Editor {
    /// Outil dont on affiche les options : celui de l'annotation sélectionnée,
    /// sinon l'outil courant.
    fn options_tool(&self) -> Tool {
        self.selected
            .and_then(|i| self.doc.annotations.get(i))
            .map_or(self.tool, |a| tool_of(&a.shape))
    }

    pub(super) fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for (tool, glyph, name, key) in TOOLS {
                let tip = format!("{name} ({})", key.name());
                if choice(ui, self.tool == tool, icon(glyph))
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.set_tool(tool);
                }
            }
            ui.separator();
            self.presets_bar(ui);
            self.image_button(ui);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let copy = egui::Button::new(
                    RichText::new(format!("{}  Copier", icons::COPY))
                        .size(theme::SIZE_M)
                        .color(theme::ON_ACCENT_2),
                )
                .fill(theme::ACCENT_2);
                if ui.add(copy).on_hover_text(keys::cmd(false, "C")).clicked() {
                    self.copy();
                }
                if ui
                    .button(
                        RichText::new(format!("{}  Enregistrer", icons::SAVE)).size(theme::SIZE_M),
                    )
                    .on_hover_text(format!(
                        "{} : image annotée dans le dossier d'enregistrement",
                        keys::cmd(false, "S")
                    ))
                    .clicked()
                {
                    self.save();
                }
                let target = self
                    .last_export
                    .clone()
                    .unwrap_or_else(|| self.path.clone());
                if ui
                    .button(icon(icons::FOLDER))
                    .on_hover_text(keys::REVEAL)
                    .clicked()
                {
                    history::reveal(&target);
                }
            });
        });
    }

    pub(super) fn options_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            self.color_options(ui);
            ui.separator();
            for (size, label, key) in [
                (Size::S, "S", "1"),
                (Size::M, "M", "2"),
                (Size::L, "L", "3"),
            ] {
                if choice(
                    ui,
                    self.style.size == size,
                    RichText::new(label).size(theme::SIZE_M),
                )
                .on_hover_text(format!("Épaisseur et taille ({key})"))
                .clicked()
                {
                    self.style.size = size;
                    self.style_changed();
                }
            }
            ui.separator();
            self.shape_options(ui);
            ui.separator();
            if ui
                .add_enabled(!self.undo.is_empty(), egui::Button::new(icon(icons::UNDO)))
                .on_hover_text(format!("Annuler ({})", keys::cmd(false, "Z")))
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(!self.redo.is_empty(), egui::Button::new(icon(icons::REDO)))
                .on_hover_text(format!("Rétablir ({})", keys::cmd(true, "Z")))
                .clicked()
            {
                self.redo();
            }
            if self.selected.is_some() {
                ui.separator();
                self.selection_actions(ui);
            }
        });
    }

    fn color_options(&mut self, ui: &mut Ui) {
        for (color, name) in PALETTE {
            if swatch(ui, color, self.style.color == color)
                .on_hover_text(name)
                .clicked()
            {
                self.style.color = color;
                self.style_changed();
            }
        }
        // Couleur personnalisée : sélecteur et code hexadécimal.
        let custom = !PALETTE.iter().any(|(c, _)| *c == self.style.color);
        let mut color = preview::color32(self.style.color);
        let response = ui
            .menu_button(
                RichText::new(icons::PALETTE)
                    .size(theme::SIZE_ICON)
                    .color(if custom { color } else { theme::TEXT }),
                |ui| {
                    ui.set_min_width(220.0);
                    let before = color;
                    egui::color_picker::color_picker_color32(
                        ui,
                        &mut color,
                        egui::color_picker::Alpha::Opaque,
                    );
                    if color != before {
                        self.style.color = Rgba::rgb(color.r(), color.g(), color.b());
                        self.style_changed();
                    }
                    ui.horizontal(|ui| {
                        ui.label("Hexadécimal");
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut self.hex_input)
                                .desired_width(90.0)
                                .hint_text("#C6FF34"),
                        );
                        if edit.changed()
                            && let Some(c) = Rgba::from_hex(&self.hex_input)
                        {
                            self.style.color = c;
                            let hex = std::mem::take(&mut self.hex_input);
                            self.style_changed();
                            // On garde ce que l'utilisateur tape (minuscules, sans #…).
                            self.hex_input = hex;
                        }
                    });
                },
            )
            .response;
        response.on_hover_text("Couleur personnalisée");
        if choice(ui, self.picking, icon(icons::PIPETTE))
            .on_hover_text("Pipette : prendre une couleur dans l'image (ou Alt+clic)")
            .clicked()
        {
            self.picking = !self.picking;
        }
    }

    /// Options propres à la forme (outil courant ou annotation sélectionnée).
    fn shape_options(&mut self, ui: &mut Ui) {
        let tool = self.options_tool();
        let mut changed = false;
        match tool {
            Tool::Arrow => {
                for (style, glyph, tip) in [
                    (ArrowStyle::Simple, icons::ARROW, "Flèche simple"),
                    (ArrowStyle::Double, icons::ARROW_DOUBLE, "Double pointe"),
                    (ArrowStyle::Big, icons::ARROW_BIG, "Grosse flèche"),
                    (
                        ArrowStyle::Connector,
                        icons::CONNECTOR,
                        "Connecteur (ronds aux bouts)",
                    ),
                ] {
                    if choice(ui, self.style.arrow == style, icon(glyph))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.style.arrow = style;
                        changed = true;
                    }
                }
                ui.label(
                    RichText::new(format!("{} poignée du milieu : courber", icons::CURVE))
                        .color(theme::TEXT_DIM),
                );
            }
            Tool::Rect | Tool::Ellipse => {
                if tool == Tool::Rect
                    && choice(ui, self.style.rounded, icon(icons::ROUNDED))
                        .on_hover_text("Coins arrondis")
                        .clicked()
                {
                    self.style.rounded = !self.style.rounded;
                    changed = true;
                }
                if choice(ui, self.style.fill, icon(icons::FILL))
                    .on_hover_text("Plein (masquer une zone)")
                    .clicked()
                {
                    self.style.fill = !self.style.fill;
                    changed = true;
                }
            }
            Tool::Text => {
                if choice(ui, self.style.boxed, icon(icons::FILL))
                    .on_hover_text("Texte sur un fond de la couleur")
                    .clicked()
                {
                    self.style.boxed = !self.style.boxed;
                    changed = true;
                }
            }
            Tool::Magnifier => {
                for zoom in [2.0, 3.0, 4.0] {
                    let selected = (self.style.zoom - zoom).abs() < 0.05;
                    if choice(
                        ui,
                        selected,
                        RichText::new(format!("×{zoom}")).size(theme::SIZE_M),
                    )
                    .on_hover_text("Grossissement")
                    .clicked()
                    {
                        self.style.zoom = zoom;
                        changed = true;
                    }
                }
            }
            Tool::Stamp => {
                ui.menu_button(icon(&self.style.stamp), |ui| {
                    egui::Grid::new("tampons")
                        .spacing([4.0, 4.0])
                        .show(ui, |ui| {
                            for (i, (glyph, name)) in STAMPS.iter().enumerate() {
                                let text = RichText::new(*glyph).size(28.0);
                                if choice(ui, self.style.stamp == *glyph, text)
                                    .on_hover_text(*name)
                                    .clicked()
                                {
                                    self.style.stamp = (*glyph).to_owned();
                                    changed = true;
                                    ui.close();
                                }
                                if i % 7 == 6 {
                                    ui.end_row();
                                }
                            }
                        });
                })
                .response
                .on_hover_text("Choisir le tampon");
            }
            Tool::Step => {
                ui.label(
                    RichText::new("Glisser : pastille avec une pointe").color(theme::TEXT_DIM),
                );
            }
            Tool::Bubble => {
                ui.label(RichText::new("Poignée : déplacer la pointe").color(theme::TEXT_DIM));
            }
            _ => {}
        }
        let effect = matches!(
            tool,
            Tool::Highlight | Tool::Spotlight | Tool::Blur | Tool::Pixelate
        );
        if !effect
            && choice(ui, self.style.shadow, icon(icons::SHADOW))
                .on_hover_text("Ombre portée")
                .clicked()
        {
            self.style.shadow = !self.style.shadow;
            changed = true;
        }
        if changed {
            self.style_changed();
        }
    }

    fn selection_actions(&mut self, ui: &mut Ui) {
        if ui
            .button(icon(icons::DUPLICATE))
            .on_hover_text(format!(
                "Dupliquer ({}, ou Alt+glisser)",
                keys::cmd(false, "D")
            ))
            .clicked()
        {
            self.duplicate_selected();
        }
        if ui
            .button(icon(icons::BRING_FRONT))
            .on_hover_text(format!("Premier plan ({})", keys::cmd(false, "]")))
            .clicked()
        {
            self.reorder_selected(true);
        }
        if ui
            .button(icon(icons::SEND_BACK))
            .on_hover_text(format!("Arrière-plan ({})", keys::cmd(false, "[")))
            .clicked()
        {
            self.reorder_selected(false);
        }
        if ui
            .button(icon(icons::DELETE))
            .on_hover_text(format!("Supprimer ({})", keys::DELETE))
            .clicked()
        {
            self.delete_selected();
        }
    }

    /// Favoris : l'outil et son style en un clic. Clic droit : retirer.
    fn presets_bar(&mut self, ui: &mut Ui) {
        if ui
            .button(icon(icons::STAR))
            .on_hover_text("Garder cet outil et ce style en favori")
            .clicked()
        {
            let preset = Preset {
                tool: self.options_tool(),
                style: self.style.clone(),
            };
            let mut prefs = super::prefs::Prefs {
                presets: std::mem::take(&mut self.presets),
                ..Default::default()
            };
            let added = prefs.add_preset(preset);
            self.presets = prefs.presets;
            self.save_prefs();
            self.say(if added {
                format!("{} Favori ajouté", icons::STAR)
            } else {
                "Déjà dans les favoris".into()
            });
        }
        let mut remove = None;
        for (i, preset) in self.presets.clone().iter().enumerate() {
            let glyph = TOOLS
                .iter()
                .find(|t| t.0 == preset.tool)
                .map_or(icons::STAR, |t| t.1);
            let text = icon(glyph).color(preview::color32(preset.style.color));
            let response = ui
                .add(egui::Button::new(text).fill(theme::SURFACE_HI))
                .on_hover_text("Favori · clic droit : retirer");
            if response.clicked() {
                self.set_tool(preset.tool);
                self.style = preset.style.clone();
                self.hex_input = self.style.color.hex();
                self.selected = None;
            }
            if response.secondary_clicked() {
                remove = Some(i);
            }
        }
        if let Some(i) = remove {
            self.presets.remove(i);
            self.save_prefs();
        }
    }

    pub(super) fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let (w, h) = self.base.dimensions();
            ui.label(RichText::new(format!("{w} × {h} px")).color(theme::TEXT_DIM));
            let n = self.doc.annotations.len();
            let plural = if n > 1 { "s" } else { "" };
            ui.label(RichText::new(format!("{n} annotation{plural}")).color(theme::TEXT_DIM));
            if let Some((text, at)) = &self.status {
                if at.elapsed() < super::STATUS_DURATION {
                    ui.label(RichText::new(text).color(theme::ACCENT_2));
                    ui.ctx().request_repaint_after(super::STATUS_DURATION);
                } else {
                    self.status = None;
                }
            } else if self.picking {
                ui.label(
                    RichText::new("Clique dans l'image pour prendre sa couleur")
                        .color(theme::ACCENT_2),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let fit = RichText::new(icons::FIT).size(theme::SIZE_L);
                if ui
                    .selectable_label(self.zoom.is_none(), fit)
                    .on_hover_text(format!("Ajuster ({})", keys::cmd(false, "0")))
                    .clicked()
                {
                    self.zoom = None;
                }
                let actual = self.zoom == Some(1.0 / self.density);
                if ui
                    .selectable_label(actual, "100 %")
                    .on_hover_text(format!("Taille réelle ({})", keys::cmd(false, "1")))
                    .clicked()
                {
                    self.zoom = Some(1.0 / self.density);
                }
            });
        });
    }
}
