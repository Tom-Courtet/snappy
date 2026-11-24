//! Barres de l'éditeur : outils et export (en haut), options du style (en dessous,
//! selon l'outil ou l'annotation sélectionnée), état (en bas).

use egui::{Align, Layout, RichText, Sense, Stroke, Ui, Vec2};

use super::Editor;
use super::prefs::Preset;
use super::preview;
use super::tools::{STAMPS, Size, TOOLS, Tool, tool_of};
use crate::history;
use crate::model::{ArrowStyle, PALETTE, Rgba, Shape};
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
            self.extra_options(ui);
            self.tool_presets_menu(ui);
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
                self.multi_actions(ui);
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
                        // Marqueur de ⌘V (voir `extras.rs`) : jamais dans le champ.
                        self.hex_input.retain(|c| c != super::extras::PASTE_MARKER);
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
                ui.menu_button(icon(&stamp_label(&self.style.stamp)), |ui| {
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
                    changed |= self.more_stamps(ui);
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
                self.zoom_slider(ui);
            });
        });
    }
}

// --- Lot « Éditeur : annotations » : options en plus --------------------------------

/// Texte affiché pour un tampon : son icône, ou son texte, ou le nom de son image.
fn stamp_label(stamp: &str) -> String {
    use super::tools::{FILE_PREFIX, LABEL_PREFIX};
    if let Some(button) = stamp.strip_prefix(crate::render::MOUSE_PREFIX) {
        format!("Souris {button}")
    } else if let Some(text) = stamp.strip_prefix(LABEL_PREFIX) {
        text.to_owned()
    } else if let Some(file) = stamp.strip_prefix(FILE_PREFIX) {
        format!("{} {file}", icons::IMAGE)
    } else {
        stamp.to_owned()
    }
}

/// Petit libellé de choix, à la taille des barres.
fn word(text: &str) -> RichText {
    RichText::new(text).size(theme::SIZE_M)
}

impl Editor {
    /// Options ajoutées par ce lot, selon l'outil : type de trait, opacité, extérieur
    /// des formes, style du texte, bulles, pastilles, polygones, accolades.
    fn extra_options(&mut self, ui: &mut Ui) {
        use crate::model::{BracketSide, BubbleKind, EndShape, LineStyle, StepFormat, StepTail};
        let tool = self.options_tool();
        let mut changed = false;
        let stroked = matches!(
            tool,
            Tool::Arrow
                | Tool::Line
                | Tool::Rect
                | Tool::Ellipse
                | Tool::Pen
                | Tool::Polygon
                | Tool::Bracket
        );
        if stroked {
            for (dash, label, tip) in [
                (LineStyle::Solid, "—", "Trait plein"),
                (LineStyle::Dotted, "···", "Pointillé"),
                (LineStyle::DashDot, "-·-", "Tiret-point"),
            ] {
                if choice(ui, self.style.dash == dash, word(label))
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.style.dash = dash;
                    changed = true;
                }
            }
        }
        match tool {
            Tool::Rect | Tool::Ellipse => {
                if choice(ui, self.style.dim_outside, icon(icons::SPOTLIGHT))
                    .on_hover_text("Assombrir l'extérieur (projecteur)")
                    .clicked()
                {
                    self.style.dim_outside = !self.style.dim_outside;
                    changed = true;
                }
                if choice(ui, self.style.blur_outside, icon(icons::BLUR))
                    .on_hover_text("Flouter l'extérieur")
                    .clicked()
                {
                    self.style.blur_outside = !self.style.blur_outside;
                    changed = true;
                }
            }
            Tool::Text | Tool::Bubble => changed |= self.text_style_options(ui),
            Tool::Step => {
                for (format, label, tip) in [
                    (StepFormat::Number, "1", "1, 2, 3…"),
                    (StepFormat::Letter, "A", "A, B, C…"),
                    (StepFormat::Roman, "i", "i, ii, iii…"),
                ] {
                    if choice(ui, self.style.step_format == format, word(label))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.style.step_format = format;
                        changed = true;
                    }
                }
                let tails = [
                    (StepTail::Pointer, "Pointe"),
                    (StepTail::Line, "Trait"),
                    (StepTail::Arrow, "Flèche"),
                    (StepTail::Pin, "Épingle"),
                    (StepTail::Hidden, "Sans lien"),
                ];
                let current = tails
                    .iter()
                    .find(|t| t.0 == self.style.step_tail)
                    .map_or("Pointe", |t| t.1);
                egui::ComboBox::from_id_salt("lien_pastille")
                    .selected_text(current)
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for (tail, label) in tails {
                            if ui
                                .selectable_value(&mut self.style.step_tail, tail, label)
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    })
                    .response
                    .on_hover_text("Lien vers l'élément désigné (glisser depuis la pastille)");
                let ink = self
                    .style
                    .step_text
                    .unwrap_or_else(|| self.style.color.contrast());
                ui.menu_button(RichText::new("Aa").color(preview::color32(ink)), |ui| {
                    if ui
                        .selectable_label(self.style.step_text.is_none(), "Automatique")
                        .clicked()
                    {
                        self.style.step_text = None;
                        changed = true;
                    }
                    ui.horizontal(|ui| {
                        for (color, name) in PALETTE {
                            if swatch(ui, color, self.style.step_text == Some(color))
                                .on_hover_text(name)
                                .clicked()
                            {
                                self.style.step_text = Some(color);
                                changed = true;
                            }
                        }
                    });
                })
                .response
                .on_hover_text("Couleur du numéro");
                self.step_numbers(ui);
            }
            Tool::Polygon => {
                if choice(ui, self.style.poly_closed, word("Fermé"))
                    .on_hover_text("Forme fermée (ou clic sur le premier point)")
                    .clicked()
                {
                    self.style.poly_closed = !self.style.poly_closed;
                    changed = true;
                }
                if choice(ui, self.style.fill, icon(icons::FILL))
                    .on_hover_text("Plein")
                    .clicked()
                {
                    self.style.fill = !self.style.fill;
                    changed = true;
                }
                let ends = [
                    (EndShape::Plain, "Aucun"),
                    (EndShape::Arrow, "Flèche"),
                    (EndShape::Round, "Rond"),
                ];
                for (id, label, value) in [
                    ("depart_polygone", "Départ", &mut self.style.poly_start),
                    ("fin_polygone", "Fin", &mut self.style.poly_end),
                ] {
                    let current = ends.iter().find(|e| e.0 == *value).map_or("", |e| e.1);
                    egui::ComboBox::from_id_salt(id)
                        .selected_text(format!("{label} : {current}"))
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for (shape, name) in ends {
                                if ui.selectable_value(value, shape, name).changed() {
                                    changed = true;
                                }
                            }
                        });
                }
            }
            Tool::Bracket => {
                for (side, label, tip) in [
                    (BracketSide::Left, "{", "Pointe à gauche"),
                    (BracketSide::Right, "}", "Pointe à droite"),
                    (BracketSide::Top, "Haut", "Pointe en haut"),
                    (BracketSide::Bottom, "Bas", "Pointe en bas"),
                ] {
                    if choice(ui, self.style.bracket == side, word(label))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.style.bracket = side;
                        changed = true;
                    }
                }
            }
            _ => {}
        }
        // Bulle : son type, en plus des options du texte.
        if tool == Tool::Bubble {
            for (kind, label, tip) in [
                (BubbleKind::Tail, "Pointe", "Bulle avec pointe"),
                (BubbleKind::NoTail, "Sans pointe", "Bulle sans pointe"),
                (BubbleKind::Simple, "Simple", "Cadre simple"),
            ] {
                if choice(ui, self.style.bubble == kind, word(label))
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.style.bubble = kind;
                    changed = true;
                }
            }
        }
        let mut pct = (self.style.opacity * 100.0).round();
        if ui
            .add(
                egui::DragValue::new(&mut pct)
                    .range(10.0..=100.0)
                    .speed(1.0)
                    .suffix(" %"),
            )
            .on_hover_text("Opacité")
            .changed()
        {
            self.style.opacity = pct / 100.0;
            changed = true;
        }
        if changed {
            self.style_changed();
        }
    }

    /// Gras, italique, souligné, alignement, lueur, et A− / A+ sur le texte sélectionné.
    fn text_style_options(&mut self, ui: &mut Ui) -> bool {
        use crate::model::{Glow, Rgba, TextAlign};
        let mut changed = false;
        let density = self.density;
        let t = &mut self.style.text_style;
        for (on, text, tip) in [
            (&mut t.bold, word("G").strong(), "Gras"),
            (&mut t.italic, word("I").italics(), "Italique"),
            (&mut t.underline, word("S").underline(), "Souligné"),
        ] {
            if choice(ui, *on, text).on_hover_text(tip).clicked() {
                *on = !*on;
                changed = true;
            }
        }
        let aligns = [
            (TextAlign::Left, "À gauche"),
            (TextAlign::Center, "Centré"),
            (TextAlign::Right, "À droite"),
        ];
        let current = aligns.iter().find(|a| a.0 == t.align).map_or("", |a| a.1);
        egui::ComboBox::from_id_salt("alignement_texte")
            .selected_text(current)
            .width(80.0)
            .show_ui(ui, |ui| {
                for (align, label) in aligns {
                    changed |= ui.selectable_value(&mut t.align, align, label).changed();
                }
            })
            .response
            .on_hover_text("Alignement des lignes");
        let glowing = t.glow.is_some();
        ui.menu_button(
            word("Lueur").color(if glowing {
                theme::ACCENT_2
            } else {
                theme::TEXT
            }),
            |ui| {
                let mut on = t.glow.is_some();
                if ui
                    .checkbox(&mut on, "Lueur autour du texte (lisible sur tout fond)")
                    .changed()
                {
                    t.glow = on.then_some(Glow {
                        color: Rgba::rgb(255, 255, 255),
                        size: 3.0 * density,
                        blur: 4.0 * density,
                    });
                    changed = true;
                }
                if let Some(glow) = &mut t.glow {
                    ui.horizontal(|ui| {
                        for (color, name) in PALETTE {
                            if swatch(ui, color, glow.color == color)
                                .on_hover_text(name)
                                .clicked()
                            {
                                glow.color = color;
                                changed = true;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Taille");
                        changed |= ui
                            .add(egui::DragValue::new(&mut glow.size).range(0.0..=40.0))
                            .changed();
                        ui.label("Flou");
                        changed |= ui
                            .add(egui::DragValue::new(&mut glow.blur).range(0.0..=40.0))
                            .changed();
                    });
                }
            },
        )
        .response
        .on_hover_text("Lueur (couleur, taille, flou)");
        // Taille du texte sélectionné, en plus de S / M / L. (⌘+ / ⌘− agrandissent
        // déjà toute l'interface, réglage d'egui : non repris ici.)
        let target = self.selected.filter(|&i| {
            matches!(
                self.doc.annotations.get(i).map(|a| &a.shape),
                Some(Shape::Text { .. } | Shape::Bubble { .. })
            )
        });
        if let Some(i) = target {
            for (label, factor, tip) in [
                ("A−", 1.0 / 1.15, "Texte plus petit"),
                ("A+", 1.15, "Texte plus grand"),
            ] {
                if ui.button(word(label)).on_hover_text(tip).clicked() {
                    self.checkpoint();
                    if let Some(Shape::Text { size, .. } | Shape::Bubble { size, .. }) =
                        self.doc.annotations.get_mut(i).map(|a| &mut a.shape)
                    {
                        *size = (*size * factor).clamp(6.0, 600.0);
                    }
                    self.changed();
                }
            }
        }
        changed
    }

    /// Prochain numéro (« continuer la numérotation depuis cette valeur »), numéro de
    /// la pastille sélectionnée, renumérotation.
    fn step_numbers(&mut self, ui: &mut Ui) {
        let mut next = self.peek_step();
        let r = ui
            .add(
                egui::DragValue::new(&mut next)
                    .range(1..=9999)
                    .prefix("Suivante : "),
            )
            .on_hover_text("Continuer la numérotation depuis cette valeur");
        if r.changed() {
            self.step_from = Some(next);
        }
        let selected = self.selected.filter(|&i| {
            matches!(
                self.doc.annotations.get(i).map(|a| &a.shape),
                Some(Shape::Step { .. })
            )
        });
        if let Some(i) = selected
            && let Some(Shape::Step { number, .. }) = self.doc.annotations.get(i).map(|a| &a.shape)
        {
            let mut n = *number;
            let r = ui
                .add(
                    egui::DragValue::new(&mut n)
                        .range(1..=9999)
                        .prefix("Valeur : "),
                )
                .on_hover_text("Numéro de la pastille sélectionnée");
            if r.drag_started() || r.gained_focus() {
                self.checkpoint();
            }
            if r.changed()
                && let Some(Shape::Step { number, .. }) =
                    self.doc.annotations.get_mut(i).map(|a| &mut a.shape)
            {
                *number = n;
                self.changed();
            }
        }
        if ui
            .button(word("Renuméroter"))
            .on_hover_text("1, 2, 3… dans l'ordre où les pastilles ont été posées")
            .clicked()
        {
            self.renumber_steps();
        }
    }

    /// Préréglages numérotés de l'outil (en plus des favoris ★) : un clic applique,
    /// « Nouveau » ajoute le style courant, l'icône d'enregistrement l'écrase, ✕ le
    /// supprime.
    fn tool_presets_menu(&mut self, ui: &mut Ui) {
        use super::prefs::ToolPresets;
        let tool = self.options_tool();
        let list = self.tool_presets.get(&tool).cloned().unwrap_or_default();
        let label = if list.is_empty() {
            "Préréglages".to_owned()
        } else {
            format!("Préréglages ({})", list.len())
        };
        ui.menu_button(word(&label), |ui| {
            let name = TOOLS.iter().find(|t| t.0 == tool).map_or("", |t| t.2);
            ui.label(RichText::new(name).color(theme::TEXT_DIM));
            let mut save = false;
            for (i, style) in list.iter().enumerate() {
                ui.horizontal(|ui| {
                    let number = RichText::new(format!(" {} ", i + 1))
                        .size(theme::SIZE_L)
                        .strong()
                        .color(preview::color32(style.color));
                    if ui
                        .add(egui::Button::new(number).fill(theme::SURFACE_HI))
                        .on_hover_text("Appliquer")
                        .clicked()
                    {
                        self.style = style.clone();
                        self.hex_input = self.style.color.hex();
                        self.style_changed();
                        ui.close();
                    }
                    if ui
                        .small_button(icons::SAVE)
                        .on_hover_text("Écraser avec le style courant")
                        .clicked()
                    {
                        self.tool_presets.overwrite(tool, i, self.style.clone());
                        save = true;
                    }
                    if ui
                        .small_button(icons::CLOSE)
                        .on_hover_text("Supprimer ce préréglage")
                        .clicked()
                    {
                        self.tool_presets.delete(tool, i);
                        save = true;
                    }
                });
            }
            if ui
                .button(format!("{}  Nouveau", icons::ADD))
                .on_hover_text("Ajouter le style courant")
                .clicked()
            {
                self.tool_presets.add(tool, self.style.clone());
                save = true;
            }
            if save {
                self.save_prefs();
            }
        })
        .response
        .on_hover_text("Préréglages de cet outil");
    }

    /// Plusieurs annotations sélectionnées : aligner, répartir.
    fn multi_actions(&mut self, ui: &mut Ui) {
        let n = self.selection().len();
        if n < 2 {
            return;
        }
        ui.label(RichText::new(format!("{n} sélectionnées")).color(theme::TEXT_DIM));
        ui.menu_button(word("Aligner"), |ui| {
            for (how, label) in super::extras::ALIGNMENTS {
                if ui.button(label).clicked() {
                    self.align_selection(how);
                    ui.close();
                }
            }
        });
        ui.add_enabled_ui(n >= 3, |ui| {
            ui.menu_button(word("Répartir"), |ui| {
                if ui.button("Horizontalement").clicked() {
                    self.distribute_selection(true);
                    ui.close();
                }
                if ui.button("Verticalement").clicked() {
                    self.distribute_selection(false);
                    ui.close();
                }
            })
            .response
            .on_disabled_hover_text("Au moins 3 annotations");
        });
    }

    /// Catégories de tampons après les classiques : souris, texte, tampons personnels.
    fn more_stamps(&mut self, ui: &mut Ui) -> bool {
        use super::tools::{FILE_PREFIX, LABEL_PREFIX, MOUSE_STAMPS, TEXT_STAMPS};
        let mut chosen: Option<String> = None;
        let current = self.style.stamp.clone();
        let mut pick = |ui: &mut Ui, stamp: String, text: RichText, tip: &str| {
            if choice(ui, current == stamp, text)
                .on_hover_text(tip)
                .clicked()
            {
                chosen = Some(stamp);
                ui.close();
            }
        };
        ui.separator();
        ui.label(RichText::new("Souris").color(theme::TEXT_DIM));
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(320.0);
            for (stamp, name) in MOUSE_STAMPS {
                let text = if stamp.contains(':') {
                    word(&stamp_label(stamp))
                } else {
                    RichText::new(stamp).size(28.0)
                };
                pick(ui, stamp.to_owned(), text, name);
            }
        });
        ui.label(RichText::new("Texte").color(theme::TEXT_DIM));
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(320.0);
            for text in TEXT_STAMPS {
                pick(
                    ui,
                    format!("{LABEL_PREFIX}{text}"),
                    word(text),
                    "Tampon texte",
                );
            }
        });
        ui.label(RichText::new("Mes tampons").color(theme::TEXT_DIM));
        let mine = self.user_stamps();
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(320.0);
            for name in mine {
                pick(
                    ui,
                    format!("{FILE_PREFIX}{name}"),
                    word(&name),
                    "Tampon personnel",
                );
            }
        });
        let mut changed = false;
        if let Some(stamp) = chosen {
            self.style.stamp = stamp;
            changed = true;
        }
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Ajouter…", icons::ADD))
                .on_hover_text("Copier des images PNG dans mes tampons")
                .clicked()
            {
                self.add_user_stamps();
                changed = true;
            }
            if ui
                .button(icon(icons::FOLDER))
                .on_hover_text("Ouvrir le dossier de mes tampons")
                .clicked()
            {
                history::open_dir(&self.user_stamps_dir());
            }
        });
        changed
    }

    /// Curseur de zoom (en % de la taille réelle).
    fn zoom_slider(&mut self, ui: &mut Ui) {
        let mut pct = self.shown_scale * self.density * 100.0;
        let response = ui
            .add(
                egui::Slider::new(&mut pct, 10.0..=400.0)
                    .logarithmic(true)
                    .suffix(" %")
                    .fixed_decimals(0),
            )
            .on_hover_text("Zoom");
        if response.changed() {
            self.zoom = Some(pct / 100.0 / self.density);
        }
    }
}
