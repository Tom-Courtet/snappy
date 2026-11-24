//! Lot « Aperçus et tampons » : panneaux flottants de l'éditeur, à la manière de
//! Screenpresso.
//!
//! - Palette des préréglages d'un outil : vignettes numérotées rendues avec chaque
//!   style (clic : appliquer, croix en haut à droite : supprimer, clic droit : écraser, « Nouveau »).
//!   Ouverte par le bouton « Préréglages » ou un second clic sur l'outil actif.
//! - Panneau des tampons : curseur « Taille », catégories (Classique, Icônes, Texte,
//!   Réactions, Souris, Mes tampons), chaque tampon montré tel qu'il sera dessiné.

use std::path::Path;

use anyhow::{Context as _, Result};
use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Key, Order, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2, vec2,
};

use super::Editor;
use super::previews::{self, Sample};
use super::tools::{
    FILE_PREFIX, LABEL_PREFIX, MORE_STAMPS, MOUSE_STAMPS, STAMP_SCALE_RANGE, STAMPS, TEXT_STAMPS,
    TOOLS, Tool, tool_of,
};
use crate::history;
use crate::model::{Annotation, PILL_PREFIX, Pt, RUBBER_PREFIX, Shape};
use crate::render::{self, EMOJI_PREFIX, stamp_art};
use crate::ui::{icons, theme};

/// Vignette de la palette des préréglages.
const PRESET_TILE: Vec2 = vec2(84.0, 52.0);
/// Vignette carrée d'un tampon.
const ICON_TILE: Vec2 = vec2(44.0, 44.0);
/// Vignette d'un tampon large (texte, pointeur à étiquette).
const WIDE_TILE: Vec2 = vec2(140.0, 40.0);
/// Largeur d'une colonne du panneau des tampons (6 tampons carrés).
const COLUMN: f32 = 6.0 * 44.0 + 5.0 * 4.0;

/// Agrandit (ou réduit) une image insérée autour de son centre.
pub fn scale_image(ann: &mut Annotation, k: f32) {
    if let Shape::Image { a, b, .. } = &mut ann.shape {
        let c = a.lerp(*b, 0.5);
        *a = Pt::new(c.x + (a.x - c.x) * k, c.y + (a.y - c.y) * k);
        *b = Pt::new(c.x + (b.x - c.x) * k, c.y + (b.y - c.y) * k);
    }
}

/// Copie une image (JPEG…) dans `dir` en PNG, sous un nom libre ; rend ce nom.
pub fn import_as_png(src: &Path, dir: &Path) -> Result<String> {
    let img = image::open(src)
        .with_context(|| format!("lecture de {}", src.display()))?
        .to_rgba8();
    std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("tampon");
    let dest = history::free_path(dir, stem, "png");
    history::write_png(&dest, &img)?;
    Ok(dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default())
}

/// Nom d'un tampon pour les infobulles (jamais d'émoji : ils ne passent pas par le
/// texte d'egui).
pub fn stamp_name(stamp: &str) -> String {
    if let Some(e) = stamp.strip_prefix(EMOJI_PREFIX) {
        return format!("Émoji : {}", crate::emoji::name(e));
    }
    if let Some(text) = stamp
        .strip_prefix(RUBBER_PREFIX)
        .or_else(|| stamp.strip_prefix(PILL_PREFIX))
        .or_else(|| stamp.strip_prefix(LABEL_PREFIX))
    {
        return text.to_owned();
    }
    if let Some(file) = stamp.strip_prefix(FILE_PREFIX) {
        return file.to_owned();
    }
    stamp_art::CLASSIC
        .iter()
        .chain(&stamp_art::MOUSE)
        .chain(&STAMPS)
        .chain(&MORE_STAMPS)
        .chain(&MOUSE_STAMPS)
        .find(|s| s.0 == stamp)
        .map_or_else(|| "Tampon".to_owned(), |s| s.1.to_owned())
}

/// Panneau flottant posé en `anchor`, au-dessus du reste.
fn floating(
    ctx: &egui::Context,
    id: &str,
    anchor: Pos2,
    add: impl FnOnce(&mut Ui),
) -> egui::Response {
    egui::Area::new(Id::unique(id))
        .order(Order::Foreground)
        .fixed_pos(anchor)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, add);
        })
        .response
}

/// Titre d'une catégorie du panneau des tampons.
fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(
        RichText::new(title)
            .size(theme::SIZE_S)
            .color(theme::TEXT_DIM),
    );
    ui.separator();
}

/// Petit numéro en bas à droite d'une vignette.
fn number_badge(ui: &Ui, rect: Rect, n: usize) {
    let painter = ui.painter();
    let text = n.to_string();
    let font = FontId::proportional(11.0);
    let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
    let size = galley.size() + vec2(8.0, 2.0);
    let r = Rect::from_min_size(rect.right_bottom() - size - vec2(3.0, 3.0), size);
    painter.rect_filled(r, CornerRadius::same(4), Color32::from_black_alpha(170));
    painter.galley(r.min + vec2(4.0, 1.0), galley, Color32::WHITE);
}

/// ⊗ en haut à droite d'une vignette ; rend son centre et son rayon.
fn delete_badge(ui: &Ui, rect: Rect, hot: bool) -> (Pos2, f32) {
    let c = rect.right_top() + vec2(-9.0, 9.0);
    let r = 7.5;
    let painter = ui.painter();
    painter.circle_filled(c, r, if hot { theme::DANGER } else { theme::SURFACE });
    painter.circle_stroke(c, r, Stroke::new(1.0, theme::TEXT_DIM));
    painter.text(
        c,
        Align2::CENTER_CENTER,
        icons::CLOSE,
        FontId::proportional(10.0),
        theme::TEXT,
    );
    (c, r)
}

enum PaletteAct {
    Apply(usize),
    Delete(usize),
    Overwrite(usize),
    Add,
}

impl Editor {
    /// Panneaux flottants ouverts (à dessiner après le reste de la fenêtre).
    pub(super) fn floating_panels(&mut self, ctx: &egui::Context) {
        self.preset_palette(ctx);
        self.stamp_picker(ctx);
    }

    /// Le clic de cette image n'a-t-il pas ouvert le panneau, et ferme-t-il celui-ci
    /// (clic ailleurs, Échap) ?
    fn closes(&self, ctx: &egui::Context, response: &egui::Response) -> bool {
        ctx.cumulative_frame_nr() != self.popup_frame
            && (response.clicked_elsewhere() || ctx.input(|i| i.key_pressed(Key::Escape)))
    }

    /// Ouvre (ou ferme) la palette des préréglages de `tool` sous `anchor`.
    pub(super) fn toggle_palette(&mut self, ctx: &egui::Context, tool: Tool, anchor: Pos2) {
        if self.palette.is_some_and(|(t, _)| t == tool) {
            self.palette = None;
        } else {
            self.stamp_panel = None;
            self.palette = Some((tool, anchor));
            self.popup_frame = ctx.cumulative_frame_nr();
        }
    }

    /// Ouvre (ou ferme) le panneau des tampons sous `anchor`.
    pub(super) fn toggle_stamp_panel(&mut self, ctx: &egui::Context, anchor: Pos2) {
        if self.stamp_panel.is_some() {
            self.stamp_panel = None;
        } else {
            self.palette = None;
            self.stamp_panel = Some(anchor);
            self.popup_frame = ctx.cumulative_frame_nr();
        }
    }

    /// « Insérer un tampon… » (menus) : l'outil Tampon et son panneau, sous le bouton
    /// de l'outil.
    pub(super) fn open_stamp_panel(&mut self, ctx: &egui::Context) {
        if self.tool != Tool::Stamp {
            self.set_tool(Tool::Stamp);
        }
        self.palette = None;
        self.stamp_panel = Some(self.stamp_anchor);
        self.popup_frame = ctx.cumulative_frame_nr();
    }

    fn preset_palette(&mut self, ctx: &egui::Context) {
        use super::prefs::ToolPresets;
        let Some((tool, anchor)) = self.palette else {
            return;
        };
        let list = self.tool_presets.get(&tool).cloned().unwrap_or_default();
        let assets = self.user_stamps_dir();
        let mut act = None;
        let response = floating(ctx, "palette_prereglages", anchor, |ui| {
            let name = TOOLS.iter().find(|t| t.0 == tool).map_or("", |t| t.2);
            ui.label(RichText::new(format!("Préréglages · {name}")).strong());
            ui.label(
                RichText::new(format!(
                    "Clic : appliquer · clic droit : écraser · {} : supprimer",
                    icons::CLOSE
                ))
                .size(theme::SIZE_S)
                .color(theme::TEXT_DIM),
            );
            ui.add_space(4.0);
            egui::Grid::new("grille_prereglages")
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for (i, style) in list.iter().enumerate() {
                        let tex = self
                            .previews
                            .tool(ctx, tool, style, PRESET_TILE, Some(&assets));
                        let bg = previews::tile_paper(Sample::Tool(tool), style);
                        let current = *style == self.style;
                        let r = previews::tile(ui, Some(tex), None, PRESET_TILE, current, bg);
                        number_badge(ui, r.rect, i + 1);
                        let pointer = ui.input(|inp| inp.pointer.interact_pos());
                        if r.hovered() {
                            let near = |c: Pos2, rad: f32| {
                                pointer.is_some_and(|p| p.distance(c) <= rad + 2.0)
                            };
                            let (c, rad) = delete_badge(ui, r.rect, false);
                            if near(c, rad) {
                                delete_badge(ui, r.rect, true);
                            }
                            if r.clicked() {
                                act = Some(if near(c, rad) {
                                    PaletteAct::Delete(i)
                                } else {
                                    PaletteAct::Apply(i)
                                });
                            }
                        }
                        r.context_menu(|ui| {
                            if ui.button("Écraser avec le style courant").clicked() {
                                act = Some(PaletteAct::Overwrite(i));
                                ui.close();
                            }
                            if ui.button(format!("{}  Supprimer", icons::DELETE)).clicked() {
                                act = Some(PaletteAct::Delete(i));
                                ui.close();
                            }
                        });
                        r.on_hover_text(format!("Préréglage {}", i + 1));
                        if i % 4 == 3 {
                            ui.end_row();
                        }
                    }
                    let (rect, r) = ui.allocate_exact_size(PRESET_TILE, Sense::click());
                    let painter = ui.painter();
                    let color = if r.hovered() {
                        theme::ACCENT_2
                    } else {
                        theme::TEXT_DIM
                    };
                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(5),
                        Stroke::new(1.0, color),
                        StrokeKind::Inside,
                    );
                    painter.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        format!("{}  Nouveau", icons::ADD),
                        FontId::proportional(theme::SIZE_S),
                        color,
                    );
                    if r.on_hover_text("Ajouter le style courant").clicked() {
                        act = Some(PaletteAct::Add);
                    }
                });
        });
        let mut save = false;
        match act {
            Some(PaletteAct::Apply(i)) => {
                if let Some(style) = list.get(i) {
                    self.style = style.clone();
                    self.hex_input = self.style.color.hex();
                    self.style_changed();
                }
                self.palette = None;
                return;
            }
            Some(PaletteAct::Delete(i)) => {
                self.tool_presets.delete(tool, i);
                save = true;
            }
            Some(PaletteAct::Overwrite(i)) => {
                self.tool_presets.overwrite(tool, i, self.style.clone());
                save = true;
            }
            Some(PaletteAct::Add) => {
                self.tool_presets.add(tool, self.style.clone());
                save = true;
            }
            None => {}
        }
        if save {
            self.save_prefs();
        }
        if self.closes(ctx, &response) && act.is_none() {
            self.palette = None;
        }
    }

    /// Vignettes de tampons, à la suite (retour à la ligne à la largeur de la
    /// colonne).
    fn stamp_tiles(
        &mut self,
        ui: &mut Ui,
        items: &[(String, String)],
        size: Vec2,
        base: u64,
        chosen: &mut Option<String>,
    ) {
        let ctx = ui.ctx().clone();
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(COLUMN);
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for (stamp, name) in items {
                let tex = self.previews.stamp(&ctx, stamp, &self.style, base, size);
                let selected = self.style.stamp == *stamp;
                let r = previews::tile(ui, Some(tex), None, size, selected, theme::SURFACE_HI);
                if r.on_hover_text(name.as_str()).clicked() {
                    *chosen = Some(stamp.clone());
                }
            }
        });
    }

    fn stamp_picker(&mut self, ctx: &egui::Context) {
        let Some(anchor) = self.stamp_panel else {
            return;
        };
        let base = previews::stamp_base(&self.style);
        let list = |items: &[(&str, &str)]| -> Vec<(String, String)> {
            items
                .iter()
                .map(|(s, n)| ((*s).to_owned(), (*n).to_owned()))
                .collect()
        };
        let classic = list(&stamp_art::CLASSIC);
        let lucide: Vec<_> = list(&STAMPS)
            .into_iter()
            .chain(list(&MORE_STAMPS))
            .collect();
        let rubber: Vec<_> = stamp_art::RUBBER
            .iter()
            .map(|t| (format!("{RUBBER_PREFIX}{t}"), format!("Tampon « {t} »")))
            .collect();
        let labels: Vec<_> = TEXT_STAMPS
            .iter()
            .map(|t| (format!("{LABEL_PREFIX}{t}"), format!("Étiquette « {t} »")))
            .collect();
        let emoji = |items: &[(&str, &str)]| -> Vec<(String, String)> {
            items
                .iter()
                .map(|(e, n)| (format!("{EMOJI_PREFIX}{e}"), (*n).to_owned()))
                .collect()
        };
        let mouse = list(&stamp_art::MOUSE);
        let pills: Vec<_> = stamp_art::PILLS
            .iter()
            .map(|t| (format!("{PILL_PREFIX}{t}"), format!("Pointeur « {t} »")))
            .collect();
        let (old_mouse, old_labels): (Vec<_>, Vec<_>) = list(&MOUSE_STAMPS)
            .into_iter()
            .partition(|(s, _)| !s.starts_with(LABEL_PREFIX));
        let dir = self.user_stamps_dir();
        let mine = self.user_stamps();

        let mut chosen = None;
        let mut scale = None;
        let mut browse = false;
        let mut open_dir = false;
        let mut close = false;
        let max_height = (ctx.content_rect().bottom() - anchor.y - 70.0).max(220.0);
        let response = floating(ctx, "panneau_tampons", anchor, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Tampons et icônes").strong());
                ui.add_space(16.0);
                ui.label("Taille");
                let mut pct = self.style.stamp_factor() * 100.0;
                let range = STAMP_SCALE_RANGE.start() * 100.0..=STAMP_SCALE_RANGE.end() * 100.0;
                if ui
                    .add(
                        egui::Slider::new(&mut pct, range)
                            .logarithmic(true)
                            .suffix(" %")
                            .fixed_decimals(0),
                    )
                    .on_hover_text("Taille des tampons (et du tampon sélectionné)")
                    .changed()
                {
                    scale = Some(pct / 100.0);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(icons::CLOSE)
                        .on_hover_text("Fermer (Échap)")
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .min_scrolled_height(max_height)
                .auto_shrink([true, true])
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 18.0;
                        ui.vertical(|ui| {
                            ui.set_width(COLUMN);
                            section(ui, "Classique");
                            self.stamp_tiles(ui, &classic, ICON_TILE, base, &mut chosen);
                            section(ui, "Icônes (à la couleur choisie)");
                            self.stamp_tiles(ui, &lucide, ICON_TILE, base, &mut chosen);
                            section(ui, "Texte");
                            self.stamp_tiles(ui, &rubber, WIDE_TILE, base, &mut chosen);
                            ui.add_space(4.0);
                            self.stamp_tiles(ui, &labels, WIDE_TILE, base, &mut chosen);
                        });
                        ui.vertical(|ui| {
                            ui.set_width(COLUMN);
                            if crate::emoji::AVAILABLE {
                                section(ui, "Réactions positives");
                                let positive = emoji(&crate::emoji::POSITIVE);
                                self.stamp_tiles(ui, &positive, ICON_TILE, base, &mut chosen);
                                section(ui, "Réactions négatives");
                                let negative = emoji(&crate::emoji::NEGATIVE);
                                self.stamp_tiles(ui, &negative, ICON_TILE, base, &mut chosen);
                                section(ui, "Signes");
                                let signs = emoji(&crate::emoji::SIGNS);
                                self.stamp_tiles(ui, &signs, ICON_TILE, base, &mut chosen);
                            }
                            section(ui, "Souris");
                            self.stamp_tiles(ui, &mouse, ICON_TILE, base, &mut chosen);
                            ui.add_space(4.0);
                            self.stamp_tiles(ui, &pills, WIDE_TILE, base, &mut chosen);
                            ui.add_space(4.0);
                            self.stamp_tiles(ui, &old_mouse, ICON_TILE, base, &mut chosen);
                            ui.add_space(4.0);
                            self.stamp_tiles(ui, &old_labels, WIDE_TILE, base, &mut chosen);
                            section(ui, "Mes tampons");
                            ui.horizontal(|ui| {
                                if ui
                                    .button("Parcourir…")
                                    .on_hover_text("Ajouter des images PNG ou JPEG à mes tampons")
                                    .clicked()
                                {
                                    browse = true;
                                }
                                if ui
                                    .button(icons::FOLDER)
                                    .on_hover_text("Ouvrir le dossier de mes tampons")
                                    .clicked()
                                {
                                    open_dir = true;
                                }
                            });
                            ui.label(
                                RichText::new(dir.display().to_string())
                                    .size(theme::SIZE_XS)
                                    .color(theme::TEXT_DIM),
                            );
                            ui.horizontal_wrapped(|ui| {
                                ui.set_max_width(COLUMN);
                                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                                for name in &mine {
                                    let thumb = self.previews.thumb(
                                        ctx,
                                        &dir.join(name),
                                        ICON_TILE - vec2(6.0, 6.0),
                                    );
                                    let stamp = format!("{FILE_PREFIX}{name}");
                                    let selected = self.style.stamp == stamp;
                                    let r = previews::tile(
                                        ui,
                                        thumb.map(|t| t.0),
                                        thumb.map(|t| t.1),
                                        ICON_TILE,
                                        selected,
                                        theme::SURFACE_HI,
                                    );
                                    if r.on_hover_text(name.as_str()).clicked() {
                                        chosen = Some(stamp);
                                    }
                                }
                            });
                        });
                    });
                });
        });
        if let Some(k) = scale {
            self.set_stamp_scale(k);
        }
        if open_dir {
            let _ = std::fs::create_dir_all(&dir);
            history::open_dir(&dir);
        }
        if browse {
            self.add_user_stamps();
            // Le clic du dialogue ne doit pas fermer le panneau.
            self.popup_frame = ctx.cumulative_frame_nr();
        }
        if let Some(stamp) = chosen {
            self.pick_stamp(stamp);
            return;
        }
        if close || (self.closes(ctx, &response) && !browse) {
            self.stamp_panel = None;
        }
    }

    /// Curseur « Taille » : style des tampons, et tampon (ou image) sélectionné.
    fn set_stamp_scale(&mut self, k: f32) {
        let before = self.style.stamp_factor();
        self.style.stamp_scale = k;
        let after = self.style.stamp_factor();
        if let Some(i) = self.selected
            && self.selection().len() <= 1
            && matches!(
                self.doc.annotations.get(i).map(|a| &a.shape),
                Some(Shape::Image { .. })
            )
        {
            self.checkpoint();
            scale_image(&mut self.doc.annotations[i], after / before);
            self.changed();
        }
        self.style_changed();
    }

    /// Choix d'un tampon dans le panneau : il devient celui de l'outil Tampon, et
    /// remplace le tampon sélectionné s'il y en a un.
    fn pick_stamp(&mut self, stamp: String) {
        let target = self.selected.filter(|&i| {
            self.selection().len() <= 1
                && self
                    .doc
                    .annotations
                    .get(i)
                    .is_some_and(|a| tool_of(&a.shape) == Tool::Stamp)
        });
        self.style.stamp = stamp;
        if self.tool != Tool::Stamp {
            self.set_tool(Tool::Stamp);
        }
        self.stamp_panel = None;
        match target {
            Some(i) => self.replace_stamp(i),
            None => {
                if self.selected.is_some() {
                    self.select(None);
                }
                self.say("Clique sur l'image pour poser le tampon");
            }
        }
    }

    /// Remplace le tampon `index` par celui du style, au même endroit.
    fn replace_stamp(&mut self, index: usize) {
        let Some(old) = self.doc.annotations.get(index).cloned() else {
            return;
        };
        let c = old.bounds(render::text_size).center();
        let m = self.metrics();
        let stamp = self.style.stamp.clone();
        let shape = if let Some(text) = stamp.strip_prefix(LABEL_PREFIX) {
            Shape::Label {
                at: c,
                text: text.to_owned(),
                size: self.style.label_size(m),
            }
        } else if let Some(file) = stamp.strip_prefix(FILE_PREFIX) {
            let src = self.user_stamps_dir().join(file);
            let copied = image::image_dimensions(&src)
                .map_err(anyhow::Error::from)
                .and_then(|dims| {
                    super::extras::copy_asset(&src, &render::assets_dir(&self.path))
                        .map(|name| (name, dims))
                });
            let (name, (w, h)) = match copied {
                Ok(v) => v,
                Err(e) => {
                    self.say(format!("{} {e:#}", icons::WARNING));
                    return;
                }
            };
            let (bw, bh) = (self.base.width() as f32, self.base.height() as f32);
            let (w, h) = (w.max(1) as f32, h.max(1) as f32);
            let fit = (bw * 0.8 / w).min(bh * 0.8 / h).min(1.0) * self.style.stamp_factor();
            let (w, h) = (w * fit, h * fit);
            Shape::Image {
                a: c.offset(-w / 2.0, -h / 2.0),
                b: c.offset(w / 2.0, h / 2.0),
                file: name,
                ratio: w / h,
            }
        } else {
            Shape::Stamp {
                at: c,
                icon: stamp,
                size: m.stamp,
            }
        };
        let new = self.style.annotate(shape, m);
        self.checkpoint();
        self.doc.annotations[index] = new;
        self.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_scale_around_their_center() {
        let mut ann = Annotation::new(
            Shape::Image {
                a: Pt::new(10.0, 10.0),
                b: Pt::new(30.0, 50.0),
                file: "x.png".into(),
                ratio: 0.5,
            },
            crate::model::Rgba::rgb(0, 0, 0),
            1.0,
        );
        scale_image(&mut ann, 2.0);
        let Shape::Image { a, b, .. } = ann.shape else {
            unreachable!()
        };
        assert_eq!((a, b), (Pt::new(0.0, -10.0), Pt::new(40.0, 70.0)));
    }

    #[test]
    fn jpeg_stamps_are_imported_as_png() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("photo.jpg");
        image::RgbImage::from_pixel(40, 20, image::Rgb([200, 30, 30]))
            .save(&src)
            .unwrap();
        let stamps = dir.path().join("stamps");
        let name = import_as_png(&src, &stamps).unwrap();
        assert_eq!(name, "photo.png");
        let thumb = previews::thumbnail(&stamps.join(&name), 20).unwrap();
        assert_eq!(thumb.dimensions(), (20, 10));
    }

    #[test]
    fn stamp_names_never_show_emoji() {
        let thumb = format!("{EMOJI_PREFIX}{}", crate::emoji::POSITIVE[0].0);
        assert_eq!(stamp_name(&thumb), "Émoji : Pouce levé");
        assert_eq!(stamp_name("tampon:VRAI"), "VRAI");
        assert_eq!(stamp_name("dessin:coche-ronde"), "Validé");
        assert_eq!(stamp_name(STAMPS[0].0), STAMPS[0].1);
    }
}
