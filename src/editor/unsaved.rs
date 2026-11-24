//! Modifications non enregistrées : l'éditeur n'écrit plus les annotations dans la
//! capture à chaque changement. À la fermeture (ou avant d'ouvrir une autre image),
//! il demande quoi faire : enregistrer sur la capture, enregistrer une copie (la
//! capture d'origine reste telle quelle) ou ne rien enregistrer.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use egui::RichText;

use super::Editor;
use crate::history;
use crate::render;
use crate::ui::{icons, theme};

/// Ce qui attend la réponse à « Enregistrer les modifications ? ».
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Pending {
    Close,
    Open(PathBuf),
}

/// Réponse de l'utilisateur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Save,
    SaveCopy,
    Discard,
    Cancel,
}

impl Editor {
    /// Des annotations ou des retouches diffèrent de ce qui est enregistré.
    pub(super) fn is_dirty(&self) -> bool {
        self.doc != self.saved_doc || self.text.as_ref().is_some_and(|t| !t.text.is_empty())
    }

    /// Écrit les annotations dans la capture ouverte.
    pub(super) fn save_capture(&mut self) -> Result<()> {
        history::save_doc(&self.path, &self.doc)?;
        self.saved_doc = self.doc.clone();
        Ok(())
    }

    /// Copie de la capture (image d'origine, annotations et images insérées) sous un
    /// nouveau nom, à côté de l'originale qui ne change pas. Rend le chemin de la
    /// copie.
    pub(super) fn save_copy(&self) -> Result<PathBuf> {
        let copy = copy_path(&self.path);
        fs::copy(&self.path, &copy).with_context(|| format!("copie vers {}", copy.display()))?;
        let assets = render::assets_dir(&self.path);
        if assets.is_dir() {
            copy_dir(&assets, &render::assets_dir(&copy))?;
        }
        history::save_doc(&copy, &self.doc)?;
        Ok(copy)
    }

    /// Fermeture demandée (bouton rouge, ⌘W, ⌘Q) : question s'il y a des
    /// modifications non enregistrées.
    pub(super) fn guard_close(&mut self, ctx: &egui::Context) {
        if self.closing || !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        self.commit_text();
        self.finish_polygon();
        if self.is_dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.ask_save = Some(Pending::Close);
        }
    }

    /// Ferme la fenêtre sans plus rien demander.
    pub(super) fn close_now(&mut self, ctx: &egui::Context) {
        self.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Ouvre une autre image, après la question s'il y a des modifications.
    pub(super) fn request_open(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.commit_text();
        self.finish_polygon();
        if self.is_dirty() {
            self.ask_save = Some(Pending::Open(path));
        } else {
            self.open_image(ctx, path);
        }
    }

    /// Fenêtre « Enregistrer les modifications ? ».
    pub(super) fn save_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.ask_save.clone() else {
            return;
        };
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut answer = None;
        let modal =
            egui::Modal::new(egui::Id::unique("enregistrer_modifications")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(
                    RichText::new("Enregistrer les modifications ?")
                        .size(theme::SIZE_L)
                        .strong(),
                );
                ui.label(format!(
                    "Les annotations de « {name} » n'ont pas été enregistrées. « Enregistrer une \
                 copie » garde la capture d'origine telle qu'elle était."
                ));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let save = egui::Button::new(
                        RichText::new(format!("{}  Enregistrer", icons::SAVE))
                            .color(theme::ON_ACCENT_2),
                    )
                    .fill(theme::ACCENT_2);
                    if ui.add(save).on_hover_text("Sur cette capture").clicked() {
                        answer = Some(Answer::Save);
                    }
                    if ui
                        .button(format!("{}  Enregistrer une copie", icons::DUPLICATE))
                        .on_hover_text("Nouvelle capture à côté ; l'originale ne change pas")
                        .clicked()
                    {
                        answer = Some(Answer::SaveCopy);
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new("Ne pas enregistrer").color(theme::DANGER))
                        .clicked()
                    {
                        answer = Some(Answer::Discard);
                    }
                    if ui.button("Annuler").clicked() {
                        answer = Some(Answer::Cancel);
                    }
                });
            });
        if modal.should_close() && answer.is_none() {
            answer = Some(Answer::Cancel);
        }
        let Some(answer) = answer else { return };
        self.ask_save = None;
        let result = match answer {
            Answer::Cancel => return,
            Answer::Discard => Ok(None),
            Answer::Save => self.save_capture().map(|()| None),
            Answer::SaveCopy => self.save_copy().map(Some),
        };
        match result {
            Ok(copy) => {
                if let Some(copy) = copy {
                    log::info!("copie enregistrée : {}", copy.display());
                }
                match pending {
                    Pending::Close => self.close_now(ctx),
                    Pending::Open(path) => self.open_image(ctx, path),
                }
            }
            Err(e) => self.say(format!("{} {e:#}", icons::WARNING)),
        }
    }
}

/// `<nom>-copie.png`, puis `-copie-2`… à côté de la capture.
fn copy_path(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("capture");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("png");
    history::free_path(dir, &format!("{stem}-copie"), ext)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_get_a_free_name_next_to_the_capture() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("snappy-1.png");
        fs::write(&png, b"x").unwrap();
        assert_eq!(copy_path(&png), dir.path().join("snappy-1-copie.png"));
        fs::write(dir.path().join("snappy-1-copie.png"), b"x").unwrap();
        assert_ne!(copy_path(&png), dir.path().join("snappy-1-copie.png"));
    }
}
