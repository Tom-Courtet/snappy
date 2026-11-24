//! Ajouts du lot « Éditeur : annotations » : sélection multiple (alignement,
//! répartition), presse-papiers d'annotations, images insérées (collage, glisser-
//! déposer, tampons personnels), polygone, numérotation des pastilles, et barre de
//! titre (image précédente / suivante, Valider, Propriétés, Ouvrir, Supprimer, plein
//! écran, copier une partie de l'image).
//!
//! ⌘X et ⌘V : egui ne transmet pas ces touches mais des évènements `Cut` et
//! `Paste(texte)` ; sans le presse-papiers système d'eframe (fonction `clipboard`
//! désactivée dans Cargo.toml), `Paste` n'arrive que si son presse-papiers interne
//! n'est pas vide. On y pose donc un marqueur invisible ([`PASTE_MARKER`]), retiré
//! de tout texte saisi. L'image et les fichiers viennent ensuite du vrai
//! presse-papiers (arboard).

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use egui::{Align, Layout, RichText, Ui};
use image::RgbaImage;

use super::preview::{self, View};
use super::tools::{FILE_PREFIX, LABEL_PREFIX, Tool, is_glyph_stamp};
use super::{Drag, Editor};
use crate::history;
use crate::model::{
    Align as AlignTo, Annotation, Bounds, Pt, Shape, align_offsets, distribute_offsets,
};
use crate::render;
use crate::ui::{icons, keys, theme};

/// Caractère posé dans le presse-papiers interne d'egui pour recevoir ⌘V (« séparateur
/// invisible »).
pub const PASTE_MARKER: char = '\u{2063}';

/// Bascule le plein écran de la fenêtre.
pub fn toggle_fullscreen(ctx: &egui::Context) {
    let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
}

/// Empreinte d'une image (comparer le presse-papiers à un instant donné).
fn mark(width: usize, height: usize, bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    (width, height).hash(&mut h);
    bytes.hash(&mut h);
    h.finish()
}

/// Image du presse-papiers système, avec son empreinte.
fn system_image() -> Option<(RgbaImage, u64)> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let data = clipboard.get_image().ok()?;
    let m = mark(data.width, data.height, &data.bytes);
    let img = RgbaImage::from_raw(
        data.width as u32,
        data.height as u32,
        data.bytes.into_owned(),
    )?;
    Some((img, m))
}

/// Fichiers du presse-papiers système (copiés dans le Finder).
fn system_files() -> Vec<PathBuf> {
    arboard::Clipboard::new()
        .ok()
        .and_then(|mut c| c.get().file_list().ok())
        .unwrap_or_default()
}

/// Met un fichier à la Corbeille du système (récupérable avec « Remettre »).
fn move_to_trash(path: &Path) -> Result<()> {
    crate::files::trash(&[path.to_owned()])
}

impl Editor {
    // --- Sélection multiple -----------------------------------------------------

    /// Annotations sélectionnées, dans l'ordre du document.
    pub(super) fn selection(&self) -> Vec<usize> {
        let Some(primary) = self.selected else {
            return Vec::new();
        };
        let mut all = if self.multi_for == Some(primary) {
            self.multi.clone()
        } else {
            Vec::new()
        };
        all.push(primary);
        all.sort_unstable();
        all.dedup();
        all.retain(|&i| i < self.doc.annotations.len());
        all
    }

    /// Sélectionne ces annotations ; la dernière donne les options affichées.
    fn set_selection(&mut self, mut indices: Vec<usize>) {
        indices.retain(|&i| i < self.doc.annotations.len());
        let primary = indices.pop();
        self.select(primary);
        self.multi = indices;
        self.multi_for = primary;
    }

    fn is_multi(&self, index: usize) -> bool {
        let sel = self.selection();
        sel.len() > 1 && sel.contains(&index)
    }

    /// Supprime toutes les annotations sélectionnées.
    pub(super) fn delete_many(&mut self) {
        let sel = self.selection();
        if sel.is_empty() {
            return;
        }
        self.checkpoint();
        for &i in sel.iter().rev() {
            self.doc.annotations.remove(i);
        }
        self.selected = None;
        self.multi.clear();
        self.changed();
    }

    /// Copies décalées de toutes les annotations sélectionnées, sélectionnées à leur
    /// tour.
    pub(super) fn duplicate_many(&mut self) {
        let sel = self.selection();
        if sel.is_empty() {
            return;
        }
        self.checkpoint();
        let d = 12.0 * self.density;
        let start = self.doc.annotations.len();
        for &i in &sel {
            let mut copy = self.doc.annotations[i].clone();
            copy.translate(d, d);
            self.doc.annotations.push(copy);
        }
        self.set_selection((start..self.doc.annotations.len()).collect());
        self.changed();
    }

    /// Le style vient de changer : il s'applique à toute la sélection.
    pub(super) fn restyle_many(&mut self) {
        let m = self.metrics();
        let sel = self.selection();
        let mut changed = false;
        let mut next = self.doc.annotations.clone();
        for &i in &sel {
            self.style.apply(&mut next[i], m);
            changed |= next[i] != self.doc.annotations[i];
        }
        if changed {
            self.checkpoint();
            self.doc.annotations = next;
            self.changed();
        }
    }

    /// Aligne la sélection sur un bord ou sur le milieu de l'ensemble.
    pub(super) fn align_selection(&mut self, how: AlignTo) {
        let sel = self.selection();
        let boxes: Vec<Bounds> = sel
            .iter()
            .map(|&i| self.doc.annotations[i].bounds(render::text_size))
            .collect();
        self.shift_selection(&sel, &align_offsets(&boxes, how));
    }

    /// Répartit la sélection avec des écarts égaux.
    pub(super) fn distribute_selection(&mut self, horizontal: bool) {
        let sel = self.selection();
        let boxes: Vec<Bounds> = sel
            .iter()
            .map(|&i| self.doc.annotations[i].bounds(render::text_size))
            .collect();
        self.shift_selection(&sel, &distribute_offsets(&boxes, horizontal));
    }

    fn shift_selection(&mut self, sel: &[usize], offsets: &[(f32, f32)]) {
        if offsets.iter().all(|&(dx, dy)| dx == 0.0 && dy == 0.0) {
            return;
        }
        self.checkpoint();
        for (&i, &(dx, dy)) in sel.iter().zip(offsets) {
            self.doc.annotations[i].translate(dx, dy);
        }
        self.changed();
    }

    /// Annotations retirées du rendu exact : celle du geste en cours, toute la
    /// sélection quand on la déplace, le polygone en cours de tracé.
    pub(super) fn hidden_all(&self) -> Vec<usize> {
        let mut all: Vec<usize> = self.hidden().into_iter().collect();
        if let Some(Drag::Move { index, .. }) = self.drag
            && self.is_multi(index)
        {
            all.extend(self.selection());
        }
        all.extend(self.poly);
        all.sort_unstable();
        all.dedup();
        all
    }

    /// Sélectionne les annotations qui touchent le rectangle `area` (avec ⇧ : en plus
    /// de la sélection en cours).
    fn select_in(&mut self, area: Bounds, add: bool) {
        let mut chosen: Vec<usize> = if add { self.selection() } else { Vec::new() };
        if area.width() >= 2.0 || area.height() >= 2.0 {
            for (i, ann) in self.doc.annotations.iter().enumerate() {
                let b = ann.bounds(render::text_size);
                let touches = b.min.x <= area.max.x
                    && b.max.x >= area.min.x
                    && b.min.y <= area.max.y
                    && b.max.y >= area.min.y;
                if touches && !chosen.contains(&i) {
                    chosen.push(i);
                }
            }
        }
        self.set_selection(chosen);
    }

    // --- Souris -----------------------------------------------------------------

    /// Gestes ajoutés par ce lot. `true` : le geste est traité ici, et le code
    /// d'origine ([`Editor::pointer`]) n'est pas appelé pour cette image.
    pub(super) fn extra_pointer(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
        view: View,
    ) -> bool {
        let pos = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos());
        let (shift, alt) = ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
        let origin = || ctx.input(|i| i.pointer.press_origin());

        // Copier une partie de l'image : le glisser choisit la zone.
        if self.grab_part {
            if response.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if response.drag_started()
                && let Some(p) = origin().or(pos)
            {
                let from = view.to_image(p);
                self.drag = Some(Drag::Grab { from, to: from });
            }
            if let (Some(Drag::Grab { from, .. }), Some(p)) =
                (self.drag, response.interact_pointer_pos())
            {
                self.drag = Some(Drag::Grab {
                    from,
                    to: view.to_image(p),
                });
            }
            if response.drag_stopped()
                && let Some(Drag::Grab { from, to }) = self.drag.take()
            {
                self.grab_part = false;
                self.copy_part(Bounds::from_corners(from, to));
            }
            return true;
        }

        // Rectangle de sélection en cours.
        if let Some(Drag::Marquee { from, .. }) = self.drag {
            if let Some(p) = response.interact_pointer_pos() {
                self.drag = Some(Drag::Marquee {
                    from,
                    to: view.to_image(p),
                });
            }
            if response.drag_stopped()
                && let Some(Drag::Marquee { from, to }) = self.drag.take()
            {
                self.select_in(Bounds::from_corners(from, to), shift);
                self.rendered = None;
            }
            return true;
        }

        // Déplacement de plusieurs annotations ensemble.
        if let Some(Drag::Move { index, last, moved }) = self.drag
            && self.is_multi(index)
        {
            let mut moved = moved;
            if let Some(p) = response.interact_pointer_pos() {
                let pt = view.to_image(p);
                let (dx, dy) = (pt.x - last.x, pt.y - last.y);
                if dx != 0.0 || dy != 0.0 {
                    for i in self.selection() {
                        self.doc.annotations[i].translate(dx, dy);
                    }
                    moved = true;
                }
                self.drag = Some(Drag::Move {
                    index,
                    last: pt,
                    moved,
                });
            }
            if response.drag_stopped() {
                self.drag = None;
                if moved {
                    self.changed();
                } else {
                    self.undo.pop();
                }
                self.rendered = None;
            }
            return true;
        }

        // Clic droit sur une annotation d'une sélection multiple : la sélection reste
        // (pour Aligner, Répartir… dans le menu).
        if response.secondary_clicked()
            && let Some(p) = pos
            && let Some(i) = self.hit(view.to_image(p), view.scale)
            && self.is_multi(i)
        {
            self.commit_text();
            return true;
        }

        let picking = self.picking || alt;
        if self.tool == Tool::Polygon && !picking && !response.secondary_clicked() {
            self.polygon_pointer(ctx, response, view);
            return true;
        }

        // Tampons texte et tampons personnels.
        if self.tool == Tool::Stamp
            && response.clicked()
            && !picking
            && !is_glyph_stamp(&self.style.stamp)
            && let Some(p) = pos
        {
            self.commit_text();
            self.place_special_stamp(view.to_image(p));
            return true;
        }

        if self.tool != Tool::Select || self.picking {
            return false;
        }
        // ⇧+clic : ajoute à la sélection ou en retire.
        if response.clicked()
            && shift
            && !alt
            && let Some(p) = pos
        {
            self.commit_text();
            if let Some(i) = self.hit(view.to_image(p), view.scale) {
                let mut sel = self.selection();
                match sel.iter().position(|&j| j == i) {
                    Some(k) => {
                        sel.remove(k);
                    }
                    None => sel.push(i),
                }
                self.set_selection(sel);
            }
            return true;
        }
        if response.drag_started()
            && let Some(p) = origin().or(pos)
            && self.handle_at(p, view).is_none()
        {
            let start = view.to_image(p);
            match self.hit(start, view.scale) {
                // Dans le vide : rectangle de sélection.
                None => {
                    self.commit_text();
                    if !shift {
                        self.selected = None;
                    }
                    self.drag = Some(Drag::Marquee {
                        from: start,
                        to: start,
                    });
                    return true;
                }
                // Sur une annotation d'une sélection multiple : tout part ensemble
                // (Alt : les copies partent, les originaux restent).
                Some(i) if self.is_multi(i) => {
                    self.commit_text();
                    self.checkpoint();
                    let mut index = i;
                    if alt {
                        let sel = self.selection();
                        let first = self.doc.annotations.len();
                        for &j in &sel {
                            let copy = self.doc.annotations[j].clone();
                            self.doc.annotations.push(copy);
                        }
                        let pos_in_sel = sel.iter().position(|&j| j == i).unwrap_or(0);
                        index = first + pos_in_sel;
                        self.set_selection((first..self.doc.annotations.len()).collect());
                    }
                    self.drag = Some(Drag::Move {
                        index,
                        last: start,
                        moved: alt,
                    });
                    return true;
                }
                Some(_) => {}
            }
        }
        false
    }

    /// Dessins ajoutés par ce lot : annotations déplacées ensemble, cadres de la
    /// sélection multiple, rectangle de sélection, zone à copier, polygone en cours.
    pub(super) fn extra_paint(&self, painter: &egui::Painter, view: View) {
        // Bulle en cours de placement : de la pointe à l'endroit de la bulle.
        if let Some(Drag::Bubble { tip, end }) = self.drag {
            let color = preview::color32(self.style.color);
            let (a, b) = (view.to_screen(tip), view.to_screen(end));
            painter.line_segment([a, b], egui::Stroke::new(2.0, color));
            painter.circle_filled(a, 4.0, color);
            painter.rect_stroke(
                egui::Rect::from_center_size(b, egui::vec2(60.0, 32.0)),
                8.0,
                egui::Stroke::new(2.0, color),
                egui::StrokeKind::Middle,
            );
        }
        let single = self.hidden();
        if self.text.is_none() {
            for i in self.hidden_all() {
                if Some(i) != single
                    && let Some(ann) = self.doc.annotations.get(i)
                {
                    preview::paint(painter, view, ann);
                }
            }
        }
        let sel = self.selection();
        if sel.len() > 1 && self.drag.is_none() {
            for &i in &sel {
                if Some(i) != self.selected {
                    let b = self.doc.annotations[i].bounds(render::text_size);
                    preview::selection(painter, view.rect(b));
                }
            }
        }
        if let Some(Drag::Marquee { from, to } | Drag::Grab { from, to }) = self.drag {
            preview::marquee(painter, view.rect(Bounds::from_corners(from, to)));
        }
        // Polygone en cours : le premier sommet, où cliquer pour fermer.
        if let Some(i) = self.poly
            && let Some(Annotation {
                shape: Shape::Polygon { points, .. },
                ..
            }) = self.doc.annotations.get(i)
            && let Some(first) = points.first()
        {
            preview::handles(painter, &[view.to_screen(*first)]);
        }
    }

    // --- Polygone ---------------------------------------------------------------

    fn polygon_pointer(&mut self, ctx: &egui::Context, response: &egui::Response, view: View) {
        if response.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        // Le dernier sommet suit la souris.
        if let Some(i) = self.poly
            && let Some(p) = response.hover_pos()
            && let Some(Annotation {
                shape: Shape::Polygon { points, .. },
                ..
            }) = self.doc.annotations.get_mut(i)
            && let Some(last) = points.last_mut()
        {
            let pt = view.to_image(p);
            if *last != pt {
                *last = pt;
                ctx.request_repaint();
            }
        }
        if response.double_clicked() {
            self.finish_polygon();
            return;
        }
        let Some(p) = response
            .clicked()
            .then(|| response.interact_pointer_pos())
            .flatten()
        else {
            return;
        };
        let pt = view.to_image(p);
        match self.poly {
            None => {
                self.commit_text();
                let m = self.metrics();
                let shape = Shape::Polygon {
                    points: vec![pt, pt],
                    closed: false,
                    start: self.style.poly_start,
                    end: self.style.poly_end,
                };
                self.checkpoint();
                let ann = self.style.annotate(shape, m);
                self.doc.annotations.push(ann);
                self.poly = Some(self.doc.annotations.len() - 1);
                self.selected = None;
            }
            Some(i) => {
                let Some(Annotation {
                    shape: Shape::Polygon { points, closed, .. },
                    ..
                }) = self.doc.annotations.get_mut(i)
                else {
                    self.poly = None;
                    return;
                };
                // Clic sur le premier sommet : la forme se ferme.
                let near_first = points.len() >= 4
                    && view.to_screen(points[0]).distance(p) <= preview::HANDLE_RADIUS + 4.0;
                if near_first {
                    *closed = true;
                    self.finish_polygon();
                } else {
                    if let Some(last) = points.last_mut() {
                        *last = pt;
                    }
                    points.push(pt);
                }
            }
        }
    }

    /// Termine le polygone en cours (le sommet qui suivait la souris disparaît).
    pub(super) fn finish_polygon(&mut self) {
        let Some(i) = self.poly.take() else { return };
        let closed_style = self.style.poly_closed;
        if let Some(Annotation {
            shape: Shape::Polygon { points, closed, .. },
            ..
        }) = self.doc.annotations.get_mut(i)
        {
            points.pop();
            points.dedup_by(|a, b| a.dist(*b) < 1.0);
            *closed |= closed_style && points.len() >= 3;
        }
        if self
            .doc
            .annotations
            .get(i)
            .is_none_or(Annotation::is_degenerate)
        {
            if i < self.doc.annotations.len() {
                self.doc.annotations.remove(i);
            }
            self.undo.pop();
        } else {
            self.select(Some(i));
            self.changed();
        }
        self.rendered = None;
    }

    // --- Pastilles ----------------------------------------------------------------

    /// Numéro de la prochaine pastille.
    pub(super) fn peek_step(&self) -> u32 {
        self.step_from.unwrap_or_else(|| self.doc.next_step())
    }

    /// Numéro de la pastille qu'on pose (et la suite, si on a imposé une valeur).
    pub(super) fn take_step(&mut self) -> u32 {
        let n = self.peek_step();
        if self.step_from.is_some() {
            self.step_from = Some(n + 1);
        }
        n
    }

    /// Renumérote les pastilles dans l'ordre où elles ont été posées.
    pub(super) fn renumber_steps(&mut self) {
        let mut doc = self.doc.clone();
        if doc.renumber_steps() {
            self.checkpoint();
            self.doc.annotations = doc.annotations;
            self.changed();
            self.say("Pastilles renumérotées");
        }
    }

    // --- Presse-papiers d'annotations ----------------------------------------------

    pub(super) fn copy_annotations(&mut self) {
        let sel = self.selection();
        if sel.is_empty() {
            return;
        }
        self.clip = sel
            .iter()
            .map(|&i| self.doc.annotations[i].clone())
            .collect();
        self.clip_assets = Some(render::assets_dir(&self.path));
        // L'image du presse-papiers système à cet instant : si elle change ensuite
        // (⌘C, autre app), c'est elle que ⌘V colle.
        self.clip_mark = system_image().map(|(_, m)| m);
        self.pastes = 0;
        let n = self.clip.len();
        self.say(format!(
            "{} {n} annotation{} copiée{}",
            icons::COPY,
            if n > 1 { "s" } else { "" },
            if n > 1 { "s" } else { "" }
        ));
    }

    pub(super) fn cut_annotations(&mut self) {
        if self.selection().is_empty() {
            return;
        }
        self.copy_annotations();
        self.delete_selected();
    }

    /// ⌘V : la plus récente des deux, l'image du presse-papiers système ou les
    /// annotations copiées ; sinon un fichier image copié dans le Finder.
    pub(super) fn paste(&mut self, at: Option<Pt>) {
        self.commit_text();
        self.finish_polygon();
        let system = system_image();
        let image_is_newer = system
            .as_ref()
            .is_some_and(|(_, m)| self.clip.is_empty() || self.clip_mark != Some(*m));
        if image_is_newer && let Some((img, _)) = system {
            self.insert_image(&img, at);
            return;
        }
        if !self.clip.is_empty() {
            self.paste_annotations(at);
            return;
        }
        let files: Vec<PathBuf> = system_files().into_iter().filter(|p| is_png(p)).collect();
        if files.is_empty() {
            self.say("Rien à coller");
        }
        for f in files {
            self.insert_image_file(&f, at);
        }
    }

    fn paste_annotations(&mut self, at: Option<Pt>) {
        self.checkpoint();
        self.pastes += 1;
        let mut copies = self.clip.clone();
        let (dx, dy) = match at {
            Some(p) => {
                let all = copies
                    .iter()
                    .map(|a| a.bounds(render::text_size))
                    .reduce(Bounds::union);
                all.map_or((0.0, 0.0), |b| (p.x - b.center().x, p.y - b.center().y))
            }
            None => {
                let d = 12.0 * self.density * self.pastes as f32;
                (d, d)
            }
        };
        let here = render::assets_dir(&self.path);
        for ann in &mut copies {
            ann.translate(dx, dy);
            // Image venue d'une autre capture : son fichier est copié ici.
            if let Shape::Image { file, .. } = &mut ann.shape
                && let Some(from) = &self.clip_assets
                && *from != here
                && let Ok(name) = copy_asset(&from.join(&*file), &here)
            {
                *file = name;
            }
        }
        let start = self.doc.annotations.len();
        self.doc.annotations.extend(copies);
        self.set_selection((start..self.doc.annotations.len()).collect());
        self.changed();
    }

    // --- Images insérées -------------------------------------------------------------

    /// Insère une image (enregistrée dans `<capture>.assets/`), centrée sur `at` ou
    /// au milieu de la capture, réduite au besoin pour tenir dedans.
    pub(super) fn insert_image(&mut self, img: &RgbaImage, at: Option<Pt>) {
        let dir = render::assets_dir(&self.path);
        let path = history::free_path(&dir, "image", "png");
        if let Err(e) = history::write_png(&path, img) {
            self.say(format!("{} {e:#}", icons::WARNING));
            return;
        }
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.place_image(file, img.width(), img.height(), at);
    }

    /// Insère un fichier image (PNG).
    pub(super) fn insert_image_file(&mut self, path: &Path, at: Option<Pt>) {
        let (w, h) = match image::image_dimensions(path) {
            Ok(dims) => dims,
            Err(e) => {
                self.say(format!(
                    "{} Image non lue (PNG seulement) : {e}",
                    icons::WARNING
                ));
                return;
            }
        };
        match copy_asset(path, &render::assets_dir(&self.path)) {
            Ok(file) => self.place_image(file, w, h, at),
            Err(e) => self.say(format!("{} {e:#}", icons::WARNING)),
        }
    }

    fn place_image(&mut self, file: String, w: u32, h: u32, at: Option<Pt>) {
        let (bw, bh) = (self.base.width() as f32, self.base.height() as f32);
        let (w, h) = (w.max(1) as f32, h.max(1) as f32);
        let fit = (bw * 0.8 / w).min(bh * 0.8 / h).min(1.0);
        let (w, h) = (w * fit, h * fit);
        let c = at.unwrap_or(Pt::new(bw / 2.0, bh / 2.0));
        let shape = Shape::Image {
            a: c.offset(-w / 2.0, -h / 2.0),
            b: c.offset(w / 2.0, h / 2.0),
            file,
            ratio: w / h,
        };
        let m = self.metrics();
        let ann = self.style.annotate(shape, m);
        self.checkpoint();
        self.doc.annotations.push(ann);
        self.select(Some(self.doc.annotations.len() - 1));
        self.changed();
    }

    /// Fichiers glissés sur le canevas.
    pub(super) fn dropped_files(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
        view: View,
    ) {
        let files = ctx.input(|i| i.raw.dropped_files.clone());
        if files.is_empty() {
            return;
        }
        let at = response
            .hover_pos()
            .or_else(|| ctx.input(|i| i.pointer.hover_pos()))
            .map(|p| view.to_image(p));
        for file in files {
            self.insert_image_file(file.path(), at);
        }
    }

    /// Tampons personnels : `<données>/stamps/`.
    pub(super) fn user_stamps_dir(&self) -> PathBuf {
        self.prefs_path.with_file_name("stamps")
    }

    /// Noms des tampons personnels, triés.
    pub(super) fn user_stamps(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.user_stamps_dir())
            .map(|dir| {
                dir.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| is_png(p))
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// « Ajouter… » : copie des images choisies dans les tampons personnels.
    pub(super) fn add_user_stamps(&mut self) {
        let Some(files) = rfd::FileDialog::new()
            .set_title("Ajouter des tampons")
            // Lot « Aperçus et tampons » : JPEG aussi, converti en PNG.
            .add_filter(
                "Image PNG ou JPEG",
                &["png", "PNG", "jpg", "JPG", "jpeg", "JPEG"],
            )
            .pick_files()
        else {
            return;
        };
        let dir = self.user_stamps_dir();
        let mut last = None;
        for f in files {
            let copied = if is_png(&f) {
                copy_asset(&f, &dir)
            } else {
                super::palettes::import_as_png(&f, &dir)
            };
            match copied {
                Ok(name) => last = Some(name),
                Err(e) => self.say(format!("{} {e:#}", icons::WARNING)),
            }
        }
        if let Some(name) = last {
            self.style.stamp = format!("{FILE_PREFIX}{name}");
        }
    }

    /// « Insérer une image… »
    pub(super) fn insert_image_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Insérer une image")
            .add_filter("Image PNG", &["png", "PNG"])
            .pick_file()
        {
            self.insert_image_file(&path, None);
        }
    }

    /// Tampon texte ou personnel posé en `at`.
    fn place_special_stamp(&mut self, at: Pt) {
        let stamp = self.style.stamp.clone();
        if let Some(text) = stamp.strip_prefix(LABEL_PREFIX) {
            let m = self.metrics();
            let shape = Shape::Label {
                at,
                text: text.to_owned(),
                size: self.style.label_size(m),
            };
            let ann = self.style.annotate(shape, m);
            self.checkpoint();
            self.doc.annotations.push(ann);
            self.select(Some(self.doc.annotations.len() - 1));
            self.changed();
        } else if let Some(file) = stamp.strip_prefix(FILE_PREFIX) {
            let path = self.user_stamps_dir().join(file);
            let before = self.doc.annotations.len();
            self.insert_image_file(&path, Some(at));
            // Lot « Aperçus et tampons » : curseur « Taille » des tampons.
            let k = self.style.stamp_factor();
            if k != 1.0
                && self.doc.annotations.len() > before
                && let Some(ann) = self.doc.annotations.last_mut()
            {
                super::palettes::scale_image(ann, k);
                self.changed();
            }
        }
    }

    // --- Images voisines, ouverture, suppression ------------------------------------

    /// Images PNG du dossier de la capture, dans l'ordre des noms (l'ordre des prises
    /// pour les captures de Snappy).
    fn siblings(&self) -> Vec<PathBuf> {
        let Some(dir) = self.path.parent() else {
            return Vec::new();
        };
        let mut all: Vec<PathBuf> = fs::read_dir(dir)
            .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        all.retain(|p| is_png(p));
        all.sort();
        all
    }

    /// Image précédente (`delta` = -1) ou suivante (+1) du dossier.
    pub(super) fn open_sibling(&mut self, ctx: &egui::Context, delta: i32) {
        let all = self.siblings();
        let Some(here) = all.iter().position(|p| *p == self.path) else {
            return;
        };
        let target = here as i64 + i64::from(delta);
        match usize::try_from(target).ok().and_then(|i| all.get(i)) {
            Some(path) => self.open_image(ctx, path.clone()),
            None => self.say(if delta < 0 {
                "Première image"
            } else {
                "Dernière image"
            }),
        }
    }

    /// Ouvre une autre image dans cette fenêtre.
    pub(super) fn open_image(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.commit_text();
        self.finish_polygon();
        let base = match history::read_png(&path) {
            Ok(img) => img,
            Err(e) => {
                self.say(format!("{} {e:#}", icons::WARNING));
                return;
            }
        };
        self.doc = history::load_doc(&path);
        self.base = base;
        self.path = path;
        self.undo.clear();
        self.redo.clear();
        self.selected = None;
        self.multi.clear();
        self.drag = None;
        self.text = None;
        self.zoom = None;
        self.rendered = None;
        self.last_export = None;
        self.step_from = None;
        self.grab_part = false;
        let name = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("capture")
            .to_owned();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("Snappy · {name}")));
    }

    /// ⌘O : ouvrir une autre image.
    pub(super) fn open_dialog(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Ouvrir une image")
            .add_filter("Image PNG", &["png", "PNG"]);
        if let Some(dir) = self.path.parent() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open_image(ctx, path);
        }
    }

    /// Met l'image (et ses annotations) à la corbeille, puis ouvre sa voisine ou
    /// ferme l'éditeur.
    fn delete_image(&mut self, ctx: &egui::Context) {
        self.commit_text();
        self.finish_polygon();
        let all = self.siblings();
        let here = all.iter().position(|p| *p == self.path);
        let neighbor = here.and_then(|i| {
            all.get(i + 1)
                .or_else(|| i.checked_sub(1).and_then(|j| all.get(j)))
        });
        let neighbor = neighbor.cloned();
        if let Err(e) = move_to_trash(&self.path) {
            self.say(format!("{} {e:#}", icons::WARNING));
            return;
        }
        for extra in [
            history::doc_path(&self.path),
            render::assets_dir(&self.path),
        ] {
            if extra.exists()
                && let Err(e) = move_to_trash(&extra)
            {
                log::warn!("{e:#}");
            }
        }
        match neighbor {
            Some(next) => {
                self.open_image(ctx, next);
                self.say(format!("{} Image mise à la corbeille", icons::DELETE));
            }
            None => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    // --- Valider, copier une partie ------------------------------------------------

    /// « Valider » : enregistre, copie (l'image, ou le fichier exporté et son chemin)
    /// et ferme.
    pub(super) fn validate(&mut self, ctx: &egui::Context) {
        let img = self.flattened();
        if let Err(e) = history::save_doc(&self.path, &self.doc) {
            self.say(format!("{} {e:#}", icons::WARNING));
            return;
        }
        let result = if self.validate_with_path {
            let dir = self.store.save_dir(&self.store.load());
            history::export(&dir, &self.path, &img)
                .and_then(|file| history::copy_image_and_path(&img, &file))
        } else {
            history::copy_image(&img)
        };
        match result {
            Ok(()) => {
                self.save_prefs();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.say(format!("{} {e:#}", icons::WARNING)),
        }
    }

    /// Copie la zone `area` de l'image annotée.
    fn copy_part(&mut self, area: Bounds) {
        let img = self.flattened();
        let top = self.banner;
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let x0 = area.min.x.clamp(0.0, iw).floor();
        let y0 = (area.min.y + top).clamp(0.0, ih).floor();
        let x1 = area.max.x.clamp(0.0, iw).ceil();
        let y1 = (area.max.y + top).clamp(0.0, ih).ceil();
        if x1 - x0 < 2.0 || y1 - y0 < 2.0 {
            self.say("Zone trop petite");
            return;
        }
        let part = image::imageops::crop_imm(
            &img,
            x0 as u32,
            y0 as u32,
            (x1 - x0) as u32,
            (y1 - y0) as u32,
        )
        .to_image();
        match history::copy_image(&part) {
            Ok(()) => self.say(format!(
                "{} Partie copiée ({} × {} px)",
                icons::OK,
                part.width(),
                part.height()
            )),
            Err(e) => self.say(format!("{} {e:#}", icons::WARNING)),
        }
    }

    // --- Chaque image ---------------------------------------------------------------

    /// À chaque image : marqueur de ⌘V, sélection multiple orpheline, confirmation de
    /// suppression.
    pub(super) fn frame_extras(&mut self, ctx: &egui::Context) {
        if !self.paste_marker && self.text.is_none() {
            ctx.copy_text(PASTE_MARKER.to_string());
            self.paste_marker = true;
        }
        if self.selected.is_none() {
            self.multi.clear();
        }
        if self.confirm_delete {
            let name = self
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut choice = None;
            let modal = egui::Modal::new(egui::Id::unique("supprimer_image")).show(ctx, |ui| {
                ui.set_max_width(340.0);
                ui.label(
                    RichText::new("Supprimer l'image ?")
                        .size(theme::SIZE_L)
                        .strong(),
                );
                ui.label(format!(
                    "{name}, ses annotations et ses images insérées vont à la corbeille."
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(
                            RichText::new(format!("{}  Mettre à la corbeille", icons::DELETE))
                                .color(theme::DANGER),
                        )
                        .clicked()
                    {
                        choice = Some(true);
                    }
                    if ui.button("Annuler").clicked() {
                        choice = Some(false);
                    }
                });
            });
            if modal.should_close() && choice.is_none() {
                choice = Some(false);
            }
            if let Some(confirmed) = choice {
                self.confirm_delete = false;
                if confirmed {
                    self.delete_image(ctx);
                }
            }
        }
    }

    // --- Menus ------------------------------------------------------------------------

    /// Entrées ajoutées au menu du clic droit (`selected` : il y a une sélection).
    pub(super) fn extra_menu(&mut self, ui: &mut Ui, selected: bool) {
        ui.separator();
        if selected {
            if ui
                .button(format!("Couper\t{}", keys::cmd(false, "X")))
                .clicked()
            {
                self.cut_annotations();
                ui.close();
            }
            if ui
                .button(format!("{}  Copier l'annotation", icons::COPY))
                .clicked()
            {
                self.copy_annotations();
                ui.close();
            }
        }
        if ui
            .add_enabled(
                !self.clip.is_empty(),
                egui::Button::new(format!("Coller\t{}", keys::cmd(false, "V"))),
            )
            .clicked()
        {
            self.paste_annotations(None);
            ui.close();
        }
        let n = self.selection().len();
        if n >= 2 {
            ui.separator();
            ui.menu_button("Aligner", |ui| {
                for (how, label) in ALIGNMENTS {
                    if ui.button(label).clicked() {
                        self.align_selection(how);
                        ui.close();
                    }
                }
            });
            ui.add_enabled_ui(n >= 3, |ui| {
                ui.menu_button("Répartir", |ui| {
                    if ui.button("Horizontalement").clicked() {
                        self.distribute_selection(true);
                        ui.close();
                    }
                    if ui.button("Verticalement").clicked() {
                        self.distribute_selection(false);
                        ui.close();
                    }
                });
            });
        }
        let has_steps = self
            .doc
            .annotations
            .iter()
            .any(|a| matches!(a.shape, Shape::Step { .. }));
        if has_steps && ui.button(format!("{}  Renuméroter", icons::STEP)).clicked() {
            self.renumber_steps();
            ui.close();
        }
        // Lot « Aperçus et tampons ».
        if ui
            .button(format!("{}  Insérer un tampon…", icons::STAMP))
            .clicked()
        {
            ui.close();
            let ctx = ui.ctx().clone();
            self.open_stamp_panel(&ctx);
        }
    }

    /// Barre au-dessus des outils : image précédente / suivante et son nom ; à
    /// droite, Valider, Propriétés et le menu des autres commandes.
    pub(super) fn title_bar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui
                .small_button("‹")
                .on_hover_text("Image précédente (PgPréc)")
                .clicked()
            {
                self.open_sibling(&ctx, -1);
            }
            if ui
                .small_button("›")
                .on_hover_text("Image suivante (PgSuiv)")
                .clicked()
            {
                self.open_sibling(&ctx, 1);
            }
            let name = self
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            ui.label(RichText::new(name).color(theme::TEXT_DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button(RichText::new("…").size(theme::SIZE_L), |ui| {
                    self.more_menu(ui, &ctx);
                })
                .response
                .on_hover_text("Autres commandes");
                ui.menu_button("Propriétés", |ui| self.properties_menu(ui))
                    .response
                    .on_hover_text("Titre et description au-dessus de l'image");
                // Ce que « Valider » met dans le presse-papiers.
                ui.menu_button(RichText::new(icons::SETTINGS), |ui| {
                    ui.radio_value(
                        &mut self.validate_with_path,
                        false,
                        "Image seule dans le presse-papiers",
                    );
                    ui.radio_value(
                        &mut self.validate_with_path,
                        true,
                        "Image exportée + chemin du fichier",
                    )
                    .on_hover_text(
                        "L'image annotée est enregistrée dans le dossier d'export et copiée \
                         comme fichier : elle se colle comme image dans le Finder ou une \
                         messagerie, et comme chemin dans un terminal.",
                    );
                });
                let validate = egui::Button::new(
                    RichText::new(format!("{}  Valider", icons::OK)).color(theme::ON_ACCENT_2),
                )
                .fill(theme::ACCENT_2);
                if ui
                    .add(validate)
                    .on_hover_text("Enregistre, copie et ferme")
                    .clicked()
                {
                    self.validate(&ctx);
                }
                ui.add_space(8.0);
                self.export_buttons(ui);
            });
        });
    }

    fn more_menu(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        if ui
            .button(format!(
                "{}  Ouvrir…\t{}",
                icons::FOLDER,
                keys::cmd(false, "O")
            ))
            .clicked()
        {
            ui.close();
            self.open_dialog(ctx);
        }
        if ui
            .button(format!("{}  Insérer une image…", icons::IMAGE))
            .clicked()
        {
            ui.close();
            self.insert_image_dialog();
        }
        // Lot « Aperçus et tampons ».
        if ui
            .button(format!("{}  Insérer un tampon…", icons::STAMP))
            .on_hover_text("Tampons, icônes, émojis, pointeurs de souris")
            .clicked()
        {
            ui.close();
            self.open_stamp_panel(ctx);
        }
        if ui
            .button(format!("{}  Copier une partie de l'image", icons::CROP))
            .on_hover_text("Glisser sur l'image pour choisir la zone (Échap : annuler)")
            .clicked()
        {
            ui.close();
            self.commit_text();
            self.grab_part = true;
            self.say("Glisse sur l'image pour choisir la partie à copier");
        }
        if ui
            .button(format!("{}  Plein écran\tF11", icons::FIT))
            .clicked()
        {
            ui.close();
            toggle_fullscreen(ctx);
        }
        self.links_menu(ui);
        ui.separator();
        if ui
            .button(
                RichText::new(format!("{}  Supprimer l'image…", icons::DELETE))
                    .color(theme::DANGER),
            )
            .clicked()
        {
            ui.close();
            self.confirm_delete = true;
        }
    }

    fn properties_menu(&mut self, ui: &mut Ui) {
        ui.set_min_width(300.0);
        let mut changed = ui
            .checkbox(
                &mut self.doc.show_properties,
                "Afficher le bandeau au-dessus de l'image",
            )
            .changed();
        changed |= ui
            .add(
                egui::TextEdit::singleline(&mut self.doc.title)
                    .hint_text("Donnez un titre à l'image")
                    .desired_width(f32::INFINITY),
            )
            .changed();
        changed |= ui
            .add(
                egui::TextEdit::multiline(&mut self.doc.description)
                    .hint_text("Donnez une description à l'image")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            )
            .changed();
        if changed {
            self.doc.title.retain(|c| c != PASTE_MARKER);
            self.doc.description.retain(|c| c != PASTE_MARKER);
            self.changed();
        }
    }
}

/// Alignements proposés, dans l'ordre des menus.
pub const ALIGNMENTS: [(AlignTo, &str); 6] = [
    (AlignTo::Left, "À gauche"),
    (AlignTo::Center, "Au centre"),
    (AlignTo::Right, "À droite"),
    (AlignTo::Top, "En haut"),
    (AlignTo::Middle, "Au milieu"),
    (AlignTo::Bottom, "En bas"),
];

fn is_png(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
}

/// Copie `src` dans `dir` sous un nom libre ; rend ce nom.
pub(super) fn copy_asset(src: &Path, dir: &Path) -> Result<String> {
    fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    let dest = history::free_path(dir, stem, "png");
    fs::copy(src, &dest).with_context(|| format!("copie de {}", src.display()))?;
    Ok(dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_are_copied_under_a_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("logo.png");
        history::write_png(&src, &RgbaImage::new(2, 2)).unwrap();
        let assets = dir.path().join("capture.assets");
        assert_eq!(copy_asset(&src, &assets).unwrap(), "logo.png");
        assert_eq!(copy_asset(&src, &assets).unwrap(), "logo-2.png");
        assert!(assets.join("logo-2.png").exists());
        assert!(is_png(Path::new("a/B.PNG")) && !is_png(Path::new("a.jpg")));
    }

    #[test]
    fn clipboard_marks_tell_images_apart() {
        assert_eq!(mark(2, 1, &[1, 2]), mark(2, 1, &[1, 2]));
        assert_ne!(mark(2, 1, &[1, 2]), mark(1, 2, &[1, 2]));
        assert_ne!(mark(2, 1, &[1, 2]), mark(2, 1, &[1, 3]));
    }
}
