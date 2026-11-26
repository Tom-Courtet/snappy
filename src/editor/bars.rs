//! Barres de l'éditeur : outils et export (en haut), options du style (en dessous,
//! selon l'outil ou l'annotation sélectionnée), état (en bas).

use egui::{Align, Layout, RichText, Sense, Stroke, Ui, Vec2, vec2};

use super::Editor;
use super::prefs::Preset;
use super::preview;
use super::previews::{self, Sample};
use super::tools::{Size, Style, TOOLS, Tool, tool_of};
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
                let response = choice(ui, self.tool == tool, icon(glyph)).on_hover_text(tip);
                // Lot « Aperçus et tampons » : un second clic sur l'outil actif ouvre
                // sa palette de préréglages (le panneau des tampons pour le tampon).
                if tool == Tool::Stamp {
                    self.stamp_anchor = response.rect.left_bottom() + vec2(0.0, 4.0);
                }
                if response.clicked() {
                    if self.tool == tool {
                        let ctx = ui.ctx().clone();
                        let anchor = response.rect.left_bottom() + vec2(0.0, 4.0);
                        match tool {
                            Tool::Select => {}
                            Tool::Stamp => self.toggle_stamp_panel(&ctx, anchor),
                            _ => self.toggle_palette(&ctx, tool, anchor),
                        }
                    } else {
                        self.set_tool(tool);
                    }
                }
            }
            ui.separator();
            self.image_button(ui);
            // Favoris : ils défilent dans la place qui reste.
            egui::ScrollArea::horizontal()
                .id_salt("favoris")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal(|ui| self.presets_bar(ui));
                });
        });
    }

    /// Dossier, Enregistrer et Copier (dans la barre de titre, à gauche de
    /// « Valider », pour laisser toute la largeur aux outils). À appeler dans une
    /// mise en page de droite à gauche.
    pub(super) fn export_buttons(&mut self, ui: &mut Ui) {
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
            .button(RichText::new(format!("{}  Enregistrer", icons::SAVE)).size(theme::SIZE_M))
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
                    self.force_size = true;
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

    pub(super) fn color_options(&mut self, ui: &mut Ui) {
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
    pub(super) fn shape_options(&mut self, ui: &mut Ui) {
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
                    (
                        ArrowStyle::Inward,
                        icons::ARROW_DOUBLE,
                        "Cote par l'extérieur (pointes vers l'intérieur)",
                    ),
                ] {
                    // Lot « Aperçus et tampons » : la variante rendue (l'icône reste
                    // dans l'infobulle).
                    let _ = glyph;
                    let variant = Style {
                        arrow: style,
                        ..self.style.clone()
                    };
                    if self
                        .variant_tile(ui, Tool::Arrow, &variant, self.style.arrow == style)
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
                    // Le bouton « Plein » remet le remplissage à plein ou vide.
                    self.style.fill_level = 0.0;
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
                // Lot « Aperçus et tampons » : le tampon courant, tel qu'il sera
                // dessiné ; un clic ouvre le panneau des tampons.
                let ctx = ui.ctx().clone();
                let size = vec2(44.0, 28.0);
                let base = previews::stamp_base(&self.style);
                let stamp = self.style.stamp.clone();
                let tex = self.previews.stamp(&ctx, &stamp, &self.style, base, size);
                let open = self.stamp_panel.is_some();
                let response = previews::tile(ui, Some(tex), None, size, open, theme::SURFACE_HI)
                    .on_hover_text(format!("Choisir le tampon · {}", stamp_label(&stamp)));
                if response.clicked() {
                    let anchor = response.rect.left_bottom() + vec2(0.0, 4.0);
                    self.toggle_stamp_panel(&ctx, anchor);
                }
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
            // Lot « Aperçus et tampons » : vignette rendue de l'outil à ce style.
            let name = TOOLS
                .iter()
                .find(|t| t.0 == preset.tool)
                .map_or("", |t| t.2);
            let response = self
                .variant_tile(ui, preset.tool, &preset.style, false)
                .on_hover_text(format!("Favori : {name} · clic droit : retirer"));
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

/// Nom d'un tampon (infobulle) : jamais un émoji, qui ne passe pas par le texte
/// d'egui.
fn stamp_label(stamp: &str) -> String {
    super::palettes::stamp_name(stamp)
}

/// Lot « Aperçus et tampons » : vignette d'une variante d'option.
const OPTION_TILE: Vec2 = vec2(34.0, 24.0);

/// Petit libellé de choix, à la taille des barres.
fn word(text: &str) -> RichText {
    RichText::new(text).size(theme::SIZE_M)
}

impl Editor {
    /// Options ajoutées par ce lot, selon l'outil : type de trait, opacité, extérieur
    /// des formes, style du texte, bulles, pastilles, polygones, accolades.
    pub(super) fn extra_options(&mut self, ui: &mut Ui) {
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
                let _ = label;
                let variant = Style {
                    dash,
                    ..self.style.clone()
                };
                if self
                    .variant_tile(ui, tool, &variant, self.style.dash == dash)
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
                    let _ = label;
                    let variant = Style {
                        step_format: format,
                        step_tail: StepTail::Hidden,
                        ..self.style.clone()
                    };
                    if self
                        .variant_tile(ui, Tool::Step, &variant, self.style.step_format == format)
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
                let shown: Vec<_> = tails
                    .iter()
                    .map(|(tail, _)| {
                        self.variant_texture(
                            ui,
                            Tool::Step,
                            &Style {
                                step_tail: *tail,
                                ..self.style.clone()
                            },
                        )
                    })
                    .collect();
                egui::ComboBox::from_id_salt("lien_pastille")
                    .selected_text(current)
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for ((tail, label), (tex, bg)) in tails.into_iter().zip(&shown) {
                            ui.horizontal(|ui| {
                                let selected = self.style.step_tail == tail;
                                let tile = previews::tile(
                                    ui,
                                    Some(*tex),
                                    None,
                                    OPTION_TILE,
                                    selected,
                                    *bg,
                                );
                                if tile.clicked()
                                    | ui.selectable_value(&mut self.style.step_tail, tail, label)
                                        .changed()
                                {
                                    self.style.step_tail = tail;
                                    changed = true;
                                }
                            });
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
                    // Le bouton « Plein » remet le remplissage à plein ou vide.
                    self.style.fill_level = 0.0;
                    changed = true;
                }
                let ends = [
                    (EndShape::Plain, "Aucun"),
                    (EndShape::Arrow, "Flèche"),
                    (EndShape::Round, "Rond"),
                ];
                // Aperçus : une ligne ouverte avec ce bout au départ, ou à la fin.
                let mut shown = Vec::new();
                for start in [true, false] {
                    for (end, _) in ends {
                        let variant = Style {
                            poly_closed: false,
                            poly_start: if start { end } else { EndShape::Plain },
                            poly_end: if start { EndShape::Plain } else { end },
                            ..self.style.clone()
                        };
                        shown.push(self.variant_texture(ui, Tool::Polygon, &variant));
                    }
                }
                for (n, (id, label, value)) in [
                    ("depart_polygone", "Départ", &mut self.style.poly_start),
                    ("fin_polygone", "Fin", &mut self.style.poly_end),
                ]
                .into_iter()
                .enumerate()
                {
                    let current = ends.iter().find(|e| e.0 == *value).map_or("", |e| e.1);
                    egui::ComboBox::from_id_salt(id)
                        .selected_text(format!("{label} : {current}"))
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for (k, (shape, name)) in ends.into_iter().enumerate() {
                                let (tex, bg) = shown[n * ends.len() + k];
                                ui.horizontal(|ui| {
                                    let tile = previews::tile(
                                        ui,
                                        Some(tex),
                                        None,
                                        OPTION_TILE,
                                        *value == shape,
                                        bg,
                                    );
                                    if tile.clicked()
                                        | ui.selectable_value(value, shape, name).changed()
                                    {
                                        *value = shape;
                                        changed = true;
                                    }
                                });
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
                    let _ = label;
                    let variant = Style {
                        bracket: side,
                        ..self.style.clone()
                    };
                    if self
                        .variant_tile(ui, Tool::Bracket, &variant, self.style.bracket == side)
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
                let _ = label;
                let variant = Style {
                    bubble: kind,
                    ..self.style.clone()
                };
                if self
                    .variant_tile(ui, Tool::Bubble, &variant, self.style.bubble == kind)
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.style.bubble = kind;
                    changed = true;
                }
            }
        }
        let mut pct = (self.style.opacity * 100.0).round();
        // Dans « Options de dessin », l'opacité a son curseur.
        if !self.in_drawing_options
            && ui
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

    /// Préréglages numérotés de l'outil (en plus des favoris ★) : le bouton ouvre la
    /// palette des vignettes (lot « Aperçus et tampons », voir `palettes.rs`).
    fn tool_presets_menu(&mut self, ui: &mut Ui) {
        let tool = self.options_tool();
        let n = self.tool_presets.get(&tool).map_or(0, Vec::len);
        let label = if n == 0 {
            "Préréglages".to_owned()
        } else {
            format!("Préréglages ({n})")
        };
        let open = self.palette.is_some_and(|(t, _)| t == tool);
        let response = choice(ui, open, word(&label)).on_hover_text(
            "Préréglages de cet outil (ou second clic sur l'outil) : vignettes à appliquer",
        );
        if response.clicked() {
            let ctx = ui.ctx().clone();
            let anchor = response.rect.left_bottom() + vec2(0.0, 4.0);
            self.toggle_palette(&ctx, tool, anchor);
        }
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

    /// Texture et fond de la vignette d'un outil à ce style.
    fn variant_texture(
        &mut self,
        ui: &Ui,
        tool: Tool,
        style: &Style,
    ) -> (egui::TextureId, egui::Color32) {
        let ctx = ui.ctx().clone();
        let assets = self.user_stamps_dir();
        let tex = self
            .previews
            .tool(&ctx, tool, style, OPTION_TILE, Some(&assets));
        (tex, previews::tile_paper(Sample::Tool(tool), style))
    }

    /// Vignette cliquable d'un outil à ce style (variantes d'option, favoris).
    fn variant_tile(
        &mut self,
        ui: &mut Ui,
        tool: Tool,
        style: &Style,
        selected: bool,
    ) -> egui::Response {
        let (tex, bg) = self.variant_texture(ui, tool, style);
        previews::tile(ui, Some(tex), None, OPTION_TILE, selected, bg)
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
