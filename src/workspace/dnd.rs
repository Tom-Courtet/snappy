//! Glisser des captures de la fenêtre vers une autre application (Finder, Mail,
//! Messages…). macOS : session de glisser native (crate `drag`, NSDraggingSource sur
//! la vue de winit). Windows : pas encore (il faudrait OLE `DoDragDrop`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Session de glisser en cours.
#[derive(Default)]
pub struct DragOut {
    /// Une session a commencé et n'est pas finie.
    active: bool,
    /// Posé par le rappel de fin de session (autre fil).
    finished: Arc<AtomicBool>,
}

impl DragOut {
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Lance la session avec ces fichiers ; `image` : PNG affiché sous le curseur.
    /// `false` si ce n'est pas possible ici.
    #[cfg(target_os = "macos")]
    pub fn start(
        &mut self,
        frame: &eframe::Frame,
        files: Vec<PathBuf>,
        image: Vec<u8>,
        repaint: impl Fn() + Send + 'static,
    ) -> bool {
        if self.active || files.is_empty() {
            return false;
        }
        let finished = Arc::clone(&self.finished);
        finished.store(false, Ordering::SeqCst);
        let result = drag::start_drag(
            frame,
            drag::DragItem::Files(files),
            drag::Image::Raw(image),
            move |_result, _pos| {
                finished.store(true, Ordering::SeqCst);
                repaint();
            },
            drag::Options::default(),
        );
        match result {
            Ok(()) => {
                self.active = true;
                true
            }
            Err(e) => {
                log::warn!("glisser-déposer impossible : {e}");
                false
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn start(
        &mut self,
        _frame: &eframe::Frame,
        _files: Vec<PathBuf>,
        _image: Vec<u8>,
        _repaint: impl Fn() + Send + 'static,
    ) -> bool {
        false
    }

    /// À appeler avant chaque image (`raw_input_hook`) : la session native avale le
    /// relâchement du bouton, egui croirait le bouton encore enfoncé. On le relâche
    /// nous-mêmes à la fin de la session.
    pub fn fix_input(&mut self, raw: &mut egui::RawInput, last_pos: Option<egui::Pos2>) {
        if self.active && self.finished.swap(false, Ordering::SeqCst) {
            self.active = false;
            if let Some(pos) = last_pos {
                raw.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            raw.events.push(egui::Event::PointerGone);
        }
    }
}

/// Glisser-déposer vers l'extérieur disponible sur ce système ?
pub const SUPPORTED: bool = cfg!(target_os = "macos");

/// Petite image PNG (vignette réduite) à montrer sous le curseur.
pub fn drag_image(thumb: Option<&image::RgbaImage>) -> Vec<u8> {
    let img = match thumb {
        Some(t) => {
            let scale = (128.0 / t.width().max(t.height()).max(1) as f32).min(1.0);
            let (w, h) = (
                ((t.width() as f32 * scale) as u32).max(1),
                ((t.height() as f32 * scale) as u32).max(1),
            );
            image::imageops::thumbnail(t, w, h)
        }
        None => image::RgbaImage::from_raw(64, 64, super::ICON_64.to_vec())
            .unwrap_or_else(|| image::RgbaImage::new(1, 1)),
    };
    let mut bytes = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(img).write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    );
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_images_are_valid_pngs() {
        for thumb in [None, Some(image::RgbaImage::new(320, 100))] {
            let png = drag_image(thumb.as_ref());
            let img = image::load_from_memory(&png).unwrap();
            assert!(img.width() <= 128 && img.height() <= 128);
        }
    }

    #[test]
    fn button_release_is_injected_once_the_session_ends() {
        let mut d = DragOut {
            active: true,
            ..DragOut::default()
        };
        let mut raw = egui::RawInput::default();
        d.fix_input(&mut raw, Some(egui::pos2(1.0, 2.0)));
        assert!(raw.events.is_empty(), "session pas finie");
        d.finished.store(true, Ordering::SeqCst);
        d.fix_input(&mut raw, Some(egui::pos2(1.0, 2.0)));
        assert!(!d.is_active());
        assert!(matches!(
            raw.events[0],
            egui::Event::PointerButton { pressed: false, .. }
        ));
    }
}
