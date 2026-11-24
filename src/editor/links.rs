//! Branchements de l'éditeur vers les autres modules (menu « … ») : texte de l'image
//! (OCR), floutage des données sensibles, suppression de l'arrière-plan, partage,
//! courriel, impression et générateur de documents.

use egui::Ui;

use super::Editor;
use crate::effects::ImageEdit;
use crate::history;
use crate::model::{Annotation, Pt, Shape};
use crate::render;
use crate::ui::icons;

impl Editor {
    /// Entrées du menu « … » ajoutées par les branchements.
    pub(super) fn links_menu(&mut self, ui: &mut Ui) {
        ui.separator();
        if ui
            .button(format!("{}  Extraire le texte (OCR)", icons::COPY))
            .on_hover_text("Copie le texte de l'image dans le presse-papiers")
            .clicked()
        {
            ui.close();
            self.copy_text();
        }
        if ui
            .button(format!("{}  Flouter les données sensibles", icons::BLUR))
            .on_hover_text(
                "Courriels, cartes bancaires, IBAN, adresses IP, clés d'API, téléphones : \
                 des zones de flou modifiables sont ajoutées",
            )
            .clicked()
        {
            ui.close();
            self.blur_sensitive();
        }
        if ui
            .button(format!("{}  Supprimer l'arrière-plan", icons::IMAGE))
            .on_hover_text(
                "Détourage local (macOS 14+) ; « Annuler la dernière retouche » le retire",
            )
            .clicked()
        {
            ui.close();
            self.remove_background();
        }
        ui.separator();
        if ui.button(format!("{}  Partager…", icons::OPEN)).clicked() {
            ui.close();
            self.publish(Publish::Share);
        }
        if ui.button("Envoyer par courriel…").clicked() {
            ui.close();
            self.publish(Publish::Mail);
        }
        if ui.button("Imprimer…").clicked() {
            ui.close();
            self.publish(Publish::Print);
        }
        if ui.button("Nouveau document (PDF, HTML, GIF)…").clicked() {
            ui.close();
            self.publish(Publish::Document);
        }
    }

    fn copy_text(&mut self) {
        self.commit_text();
        let result = crate::vision::ocr(&self.base).and_then(|lines| {
            let text = crate::vision::lines_text(&lines);
            anyhow::ensure!(!text.trim().is_empty(), "aucun texte trouvé");
            arboard::Clipboard::new()
                .and_then(|mut c| c.set_text(text))
                .map_err(|e| anyhow::anyhow!("presse-papiers : {e}"))?;
            Ok(lines.len())
        });
        match result {
            Ok(n) => self.say(format!("{} Texte copié ({n} lignes)", icons::OK)),
            Err(e) => self.say(format!("{} OCR : {e:#}", icons::WARNING)),
        }
    }

    fn blur_sensitive(&mut self) {
        self.commit_text();
        match crate::vision::sensitive_regions(&self.base) {
            Ok(hits) if hits.is_empty() => self.say("Aucune donnée sensible trouvée"),
            Ok(hits) => {
                self.checkpoint();
                // Un peu de marge autour du texte, et le flou habituel de l'outil.
                let width = 4.0 * self.density;
                let pad = 2.0 * self.density;
                for hit in &hits {
                    let r = hit.rect;
                    let shape = Shape::Blur {
                        a: Pt::new(r.x as f32 - pad, r.y as f32 - pad),
                        b: Pt::new((r.x + r.width) as f32 + pad, (r.y + r.height) as f32 + pad),
                    };
                    self.doc
                        .annotations
                        .push(Annotation::new(shape, self.style.color, width));
                }
                self.changed();
                self.say(format!(
                    "{} {} zone(s) floutée(s), modifiables comme les autres annotations",
                    icons::OK,
                    hits.len()
                ));
            }
            Err(e) => self.say(format!("{} Données sensibles : {e:#}", icons::WARNING)),
        }
    }

    fn remove_background(&mut self) {
        self.commit_text();
        let result = crate::vision::remove_background(&self.base).and_then(|img| {
            let dir = render::assets_dir(&self.path);
            std::fs::create_dir_all(&dir)?;
            let path = history::free_path(&dir, "sans-arriere-plan", "png");
            history::write_png(&path, &img)?;
            Ok(path)
        });
        match result {
            Ok(path) => self.push_edit(ImageEdit::ReplaceBase { path }),
            Err(e) => self.say(format!("{} Arrière-plan : {e:#}", icons::WARNING)),
        }
    }

    fn publish(&mut self, how: Publish) {
        self.commit_text();
        self.finish_polygon();
        if let Err(e) = history::save_doc(&self.path, &self.doc) {
            self.say(format!("{} {e:#}", icons::WARNING));
            return;
        }
        let paths = [self.path.clone()];
        let result = match how {
            Publish::Share => crate::share::share_sheet(&paths).map(|d| d.message()),
            Publish::Mail => crate::share::email(&paths).map(|d| d.message()),
            Publish::Print => crate::share::print(
                &self.path,
                crate::share::PrintOptions {
                    file_name: true,
                    ..Default::default()
                },
            )
            .map(|()| None),
            Publish::Document => crate::share::docgen::window::open(&paths).map(|()| None),
        };
        match result {
            Ok(Some(text)) => self.say(text),
            Ok(None) => {}
            Err(e) => self.say(format!("{} {e:#}", icons::WARNING)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Publish {
    Share,
    Mail,
    Print,
    Document,
}
