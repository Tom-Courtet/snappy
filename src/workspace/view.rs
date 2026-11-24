//! Interface de l'espace de travail : en-tête, recherche, panneau des espaces,
//! grille de vignettes, barre du bas, menus, boîtes de dialogue, clavier, aperçu.
//!
//! Disposition reprise de Screenpresso (§ 11.6 et 13.3 du cahier des charges), dans
//! le thème sombre de Snappy.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Key, KeyboardShortcut, Layout, Margin,
    Modifiers, Popup, Pos2, Rect, RichText, Sense, Stroke, TextFormat, Ui, Vec2, pos2, vec2,
};

use super::library::{DateFilter, Sort};
use super::{After, Dialog, Workspace, dnd, prefs};
use crate::agent::hotkeys::label_of;
use crate::files::{self, Kind};
use crate::history;
use crate::ipc::Mode;
use crate::ui::theme::{self, Typo};
use crate::ui::{icons, keys};

/// Couleurs des boutons ronds et de la sélection (Screenpresso : rouge, vert, bleu,
/// orange).
const RED: Color32 = Color32::from_rgb(0xe0, 0x3a, 0x3a);
const GREEN: Color32 = Color32::from_rgb(0x5a, 0xa8, 0x4a);
const BLUE: Color32 = Color32::from_rgb(0x4a, 0x86, 0xd8);
const ORANGE: Color32 = Color32::from_rgb(0xe8, 0x92, 0x2a);
const SELECTION: Color32 = RED;

/// Hauteur du nom sous une vignette.
const NAME_HEIGHT: f32 = 26.0;
/// Marge autour d'une vignette (cadre de sélection compris).
const CELL_PAD: f32 = 12.0;

/// Actions des captures : menu ▼ « Capturer une zone ».
const CAPTURE_MODES: [(Mode, &str); 10] = [
    (Mode::Region, "Capturer une zone"),
    (Mode::Previous, "Capturer la zone précédente"),
    (Mode::Screen, "Capturer l'écran"),
    (Mode::AllScreens, "Capturer tous les écrans"),
    (Mode::Window, "Capturer la fenêtre active"),
    (Mode::Delayed, "Capture différée"),
    (Mode::Scrolling, "Capture avec défilement"),
    (Mode::ColorPicker, "Pipette de couleur"),
    (Mode::Ocr, "Copier le texte (OCR)"),
    (Mode::Qr, "Décoder un QR code"),
];
const VIDEO_MODES: [(Mode, &str); 2] = [
    (Mode::Video, "Enregistrer une vidéo"),
    (Mode::VideoRegion, "Enregistrer une zone en vidéo"),
];

/// Ce qu'un clic, un menu ou une touche demande ; appliqué après le dessin (la
/// liste peut changer).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Action {
    Open(PathBuf),
    OpenDefault(PathBuf),
    Reveal(PathBuf),
    Preview(PathBuf),
    SaveAs(PathBuf),
    CopyImage(PathBuf),
    CopyPaths(Vec<PathBuf>),
    Delete(Vec<PathBuf>),
    Rename(PathBuf),
    MoveTo(Vec<PathBuf>, PathBuf),
    Merge(Vec<PathBuf>, bool),
    Properties(PathBuf),
    Capture(Mode),
    Edit,
    Settings,
    Help,
    About,
    Quit,
    Close,
    SetWorkspace(PathBuf),
    NewWorkspace,
    RenameWorkspace(PathBuf),
    DeleteWorkspace(PathBuf),
    OpenFolder(PathBuf),
    AlwaysOnTop(bool),
    AutoClose(bool),
    RememberPosition(bool),
    ZoomThumbs(f32),
    Sort(Sort),
    TogglePanel,
    ToggleSearch,
    ToggleMaximize,
}

/// Libellé du raccourci global d'une action, d'après la config.
fn hotkey_labels() -> Vec<(Mode, String)> {
    let Ok(store) = crate::config::Store::open_default() else {
        return Vec::new();
    };
    let config = store.load();
    Mode::ALL
        .into_iter()
        .filter_map(|m| Some((m, label_of(config.hotkey(m))?)))
        .collect()
}

fn shortcut_of(labels: &[(Mode, String)], mode: Mode) -> String {
    labels
        .iter()
        .find(|(m, _)| *m == mode)
        .map(|(_, l)| l.clone())
        .unwrap_or_default()
}

/// Entrée de menu avec son raccourci à droite.
fn menu_item(ui: &mut Ui, text: &str, shortcut: &str, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(text).shortcut_text(RichText::new(shortcut).color(theme::TEXT_DIM)),
    )
    .clicked()
}

/// Entrée de menu à cocher.
fn menu_check(ui: &mut Ui, on: bool, text: &str, shortcut: &str) -> bool {
    let check = RichText::new(icons::OK).color(if on {
        theme::ACCENT_2
    } else {
        Color32::TRANSPARENT
    });
    ui.add(
        egui::Button::new((check, text))
            .shortcut_text(RichText::new(shortcut).color(theme::TEXT_DIM)),
    )
    .clicked()
}

/// Bouton d'icône de l'en-tête.
fn header_button(ui: &mut Ui, icon: &str, tip: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(icon).size(theme::SIZE_ICON)).frame(false))
        .on_hover_text(tip)
}

/// Bouton rond à contour coloré, icône blanche et libellé dessous (Screenpresso).
/// `icon` : glyphe ; `None` : caméra dessinée (pas d'icône vidéo dans la police).
fn round_button(
    ui: &mut Ui,
    icon: Option<&str>,
    label: &str,
    color: Color32,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(112.0, 80.0), sense);
    let painter = ui.painter();
    let c = pos2(rect.center().x, rect.top() + 28.0);
    let (ring, text) = if enabled {
        (color, theme::TEXT)
    } else {
        (color.gamma_multiply(0.35), theme::TEXT_DIM)
    };
    let fill = if enabled && resp.hovered() {
        color.gamma_multiply(0.25)
    } else {
        Color32::TRANSPARENT
    };
    painter.circle(c, 24.0, fill, Stroke::new(2.5, ring));
    match icon {
        Some(icon) => {
            painter.text(
                c,
                Align2::CENTER_CENTER,
                icon,
                FontId::proportional(22.0),
                text,
            );
        }
        None => video_camera(painter, c, text),
    }
    painter.text(
        pos2(c.x, rect.bottom() - 12.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(theme::SIZE_S),
        text,
    );
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Petite flèche vers le bas à côté d'un bouton : ouvre son menu.
fn arrow_button(ui: &mut Ui, height: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0, height), Sense::click());
    let color = if resp.hovered() {
        theme::TEXT
    } else {
        theme::TEXT_DIM
    };
    down_arrow(ui.painter(), rect.center(), color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Triangle pointe en bas (les polices n'ont pas « ▾ »).
fn down_arrow(painter: &egui::Painter, c: Pos2, color: Color32) {
    painter.add(egui::Shape::convex_polygon(
        vec![
            c + vec2(-4.5, -2.5),
            c + vec2(4.5, -2.5),
            c + vec2(0.0, 3.0),
        ],
        color,
        Stroke::NONE,
    ));
}

/// Caméra vidéo : boîtier arrondi et objectif.
fn video_camera(painter: &egui::Painter, c: Pos2, color: Color32) {
    let body = Rect::from_min_size(c + vec2(-11.0, -7.0), vec2(15.0, 14.0));
    painter.rect_filled(body, CornerRadius::same(3), color);
    painter.add(egui::Shape::convex_polygon(
        vec![
            c + vec2(5.0, -2.0),
            c + vec2(11.0, -6.0),
            c + vec2(11.0, 6.0),
            c + vec2(5.0, 2.0),
        ],
        color,
        Stroke::NONE,
    ));
}

/// Texte sur une ligne, coupé par « … » s'il dépasse `width`.
fn elided(
    ui: &Ui,
    text: &str,
    width: f32,
    size: f32,
    color: Color32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: FontId::proportional(size),
            color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

/// Date lisible (heure locale).
fn format_date(t: SystemTime) -> String {
    jiff::ZonedDateTime::try_from(t)
        .map(|z| z.strftime("%d/%m/%Y à %H:%M:%S").to_string())
        .unwrap_or_else(|_| "?".into())
}

impl Workspace {
    pub(super) fn draw(&mut self, ui: &mut Ui, frame: &eframe::Frame) {
        let mut actions = Vec::new();
        egui::Panel::top("header")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(10, 6)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.header(ui, &mut actions));
        if self.search_open {
            egui::Panel::top("search")
                .frame(
                    Frame::new()
                        .fill(theme::SURFACE)
                        .inner_margin(Margin::symmetric(12, 6)),
                )
                .show_separator_line(false)
                .show(ui, |ui| self.search_bar(ui, &mut actions));
        }
        egui::Panel::bottom("actions")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(12, 8)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.bottom_bar(ui, &mut actions));
        if self.prefs.show_workspaces {
            egui::Panel::left("workspaces")
                .frame(
                    Frame::new()
                        .fill(theme::SURFACE)
                        .inner_margin(Margin::symmetric(8, 10)),
                )
                .exact_size(118.0)
                .resizable(false)
                .show_separator_line(false)
                .show(ui, |ui| self.side_panel(ui, &mut actions));
        }
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(theme::BG)
                    .inner_margin(Margin::symmetric(6, 6)),
            )
            .show(ui, |ui| {
                self.grid(ui, frame, &mut actions);
                self.overlays(ui);
            });
        let ctx = ui.ctx().clone();
        self.dialogs(&ctx);
        for action in actions {
            self.apply(action);
        }
    }

    // --- En-tête ---------------------------------------------------------------------

    fn header(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        // Le fond de l'en-tête déplace la fenêtre (pas de barre de titre système).
        let bar = ui.max_rect().expand2(vec2(10.0, 6.0));
        let drag = ui.interact(
            bar,
            ui.scope_id().with("drag-window"),
            Sense::click_and_drag(),
        );
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            actions.push(Action::ToggleMaximize);
        }
        ui.horizontal(|ui| {
            ui.add(egui::Image::new((self.logo.id(), vec2(24.0, 24.0))));
            if header_button(ui, icons::FOLDER, "Espaces de travail (F10)").clicked() {
                actions.push(Action::TogglePanel);
            }
            ui.label(RichText::new("Snappy").title().size(theme::SIZE_L));
            ui.label(
                RichText::new(format!("· {}", self.workspace_name(&self.current.clone())))
                    .color(theme::TEXT_DIM),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if header_button(ui, icons::CLOSE, "Fermer (Échap)").clicked() {
                    actions.push(Action::Close);
                }
                if header_button(ui, icons::FIT, "Agrandir").clicked() {
                    actions.push(Action::ToggleMaximize);
                }
                let arrow = arrow_button(ui, 24.0);
                let wrench = header_button(ui, icons::SETTINGS, "Options de l'espace de travail");
                let wrench = wrench | arrow;
                Popup::menu(&wrench).show(|ui| self.wrench_menu(ui, actions));
                let search = header_button(
                    ui,
                    icons::MAGNIFIER,
                    &format!("Rechercher ({})", keys::cmd(false, "F")),
                );
                if search.clicked() {
                    actions.push(Action::ToggleSearch);
                }
                if self.filter.is_active() {
                    ui.painter().circle_filled(
                        search.rect.right_top() + vec2(-2.0, 3.0),
                        3.5,
                        theme::ACCENT_2,
                    );
                }
            });
        });
    }

    fn wrench_menu(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        ui.set_min_width(280.0);
        let p = &self.prefs;
        if menu_check(ui, p.always_on_top, "Fenêtre toujours devant", "F8") {
            actions.push(Action::AlwaysOnTop(!p.always_on_top));
        }
        if menu_check(ui, p.auto_close, "Fermeture automatique", "F9") {
            actions.push(Action::AutoClose(!p.auto_close));
        }
        ui.separator();
        if menu_item(
            ui,
            "Agrandir les miniatures",
            &keys::cmd(false, "+"),
            p.thumb_size < prefs::THUMB_MAX,
        ) {
            actions.push(Action::ZoomThumbs(prefs::THUMB_STEP));
        }
        if menu_item(
            ui,
            "Réduire les miniatures",
            &keys::cmd(false, "−"),
            p.thumb_size > prefs::THUMB_MIN,
        ) {
            actions.push(Action::ZoomThumbs(-prefs::THUMB_STEP));
        }
        ui.separator();
        if menu_check(ui, p.remember_position, "Mémoriser la position", "") {
            actions.push(Action::RememberPosition(!p.remember_position));
        }
        ui.menu_button("Trier par", |ui| {
            for sort in Sort::ALL {
                if menu_check(ui, p.sort == sort, sort.label(), "") {
                    actions.push(Action::Sort(sort));
                }
            }
        });
        if menu_check(ui, p.show_workspaces, "Espaces de travail", "F10") {
            actions.push(Action::TogglePanel);
        }
        ui.separator();
        if menu_item(ui, "Paramètres…", "F11", true) {
            actions.push(Action::Settings);
        }
        if menu_item(ui, "Aide", "F1", true) {
            actions.push(Action::Help);
        }
        if menu_item(ui, "À propos de Snappy", "", true) {
            actions.push(Action::About);
        }
        ui.separator();
        if menu_item(ui, "Quitter Snappy", "", true) {
            actions.push(Action::Quit);
        }
    }

    // --- Recherche -------------------------------------------------------------------

    fn search_bar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let before = self.filter.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::MAGNIFIER).color(theme::TEXT_DIM));
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.filter.text)
                    .hint_text("Nom, application, titre de la fenêtre…")
                    .desired_width(260.0),
            );
            if std::mem::take(&mut self.focus_search) {
                edit.request_focus();
            }
            if edit.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                actions.push(Action::ToggleSearch);
            }
            ui.add_space(8.0);
            ui.toggle_value(&mut self.filter.images, format!("{} Images", icons::IMAGE));
            ui.toggle_value(&mut self.filter.videos, "Vidéos");
            ui.toggle_value(&mut self.filter.documents, "Documents");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if header_button(ui, icons::CLOSE, "Fermer la recherche").clicked() {
                    actions.push(Action::ToggleSearch);
                }
            });
        });
        ui.horizontal(|ui| {
            for date in DateFilter::ALL {
                ui.selectable_value(&mut self.filter.date, date, date.label());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let shown = self.view.len();
                let total = self.items.len();
                let text = if shown == total {
                    format!("{total} élément{}", if total > 1 { "s" } else { "" })
                } else {
                    format!("{shown} sur {total}")
                };
                ui.label(
                    RichText::new(text)
                        .color(theme::TEXT_DIM)
                        .size(theme::SIZE_S),
                );
            });
        });
        if self.filter != before {
            self.update_view();
        }
    }

    // --- Espaces de travail ----------------------------------------------------------

    fn side_panel(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        egui::ScrollArea::vertical()
            .id_salt("workspaces")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut dirs = vec![self.root.clone()];
                dirs.extend(self.workspaces.iter().cloned());
                for dir in dirs {
                    let base = dir == self.root;
                    let current = dir == self.current;
                    let resp = self.workspace_button(ui, &dir, base, current);
                    if resp.clicked() {
                        actions.push(Action::SetWorkspace(dir.clone()));
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Ouvrir le dossier").clicked() {
                            actions.push(Action::OpenFolder(dir.clone()));
                        }
                        if !base {
                            if ui.button("Renommer…").clicked() {
                                actions.push(Action::RenameWorkspace(dir.clone()));
                            }
                            if ui.button("Supprimer…").clicked() {
                                actions.push(Action::DeleteWorkspace(dir.clone()));
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let new = ui
                        .add(
                            egui::Button::new(format!("{} Nouveau…", icons::ADD))
                                .frame(false)
                                .min_size(vec2(76.0, 28.0)),
                        )
                        .on_hover_text("Nouvel espace de travail (un sous-dossier des captures)");
                    if new.clicked() {
                        actions.push(Action::NewWorkspace);
                    }
                    let more = arrow_button(ui, 56.0);
                    Popup::menu(&more).show(|ui| {
                        if ui.button("Nouvel espace de travail…").clicked() {
                            actions.push(Action::NewWorkspace);
                        }
                        if ui.button("Ouvrir le dossier des captures").clicked() {
                            actions.push(Action::OpenFolder(self.root.clone()));
                        }
                    });
                });
            });
    }

    fn workspace_button(
        &self,
        ui: &mut Ui,
        dir: &Path,
        base: bool,
        current: bool,
    ) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 62.0), Sense::click());
        let painter = ui.painter();
        let fill = if current {
            theme::SURFACE_HI
        } else if resp.hovered() {
            Color32::from_white_alpha(8)
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(rect, CornerRadius::same(10), fill);
        if current {
            // Barre rouge de l'espace courant.
            let bar = Rect::from_min_size(
                rect.left_top() + vec2(4.0, 16.0),
                vec2(3.0, rect.height() - 32.0),
            );
            painter.rect_filled(bar, CornerRadius::same(2), SELECTION);
        }
        let icon = if base { icons::IMAGE } else { icons::FOLDER };
        painter.text(
            pos2(rect.center().x, rect.top() + 22.0),
            Align2::CENTER_CENTER,
            icon,
            FontId::proportional(22.0),
            theme::TEXT,
        );
        let name = if base {
            "Base".to_owned()
        } else {
            files::file_name(dir)
        };
        let galley = elided(ui, &name, rect.width() - 12.0, theme::SIZE_S, theme::TEXT);
        let pos = pos2(
            rect.center().x - galley.size().x / 2.0,
            rect.bottom() - 20.0,
        );
        painter.galley(pos, galley, theme::TEXT);
        resp.on_hover_text(dir.display().to_string())
    }

    // --- Barre du bas ----------------------------------------------------------------

    fn bottom_bar(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let total = 5.0 * 112.0 + 2.0 * 18.0 + 6.0 * 12.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            let labels = || hotkey_labels();
            if round_button(ui, Some(icons::REGION), "Capturer une zone", RED, true)
                .on_hover_text("La fenêtre se ferme, puis la capture commence")
                .clicked()
            {
                actions.push(Action::Capture(Mode::Region));
            }
            let more = arrow_button(ui, 56.0);
            Popup::menu(&more).show(|ui| {
                ui.set_min_width(300.0);
                let labels = labels();
                for (mode, text) in CAPTURE_MODES {
                    if menu_item(ui, text, &shortcut_of(&labels, mode), true) {
                        actions.push(Action::Capture(mode));
                    }
                }
                ui.separator();
                if menu_item(ui, "Paramètres…", "F11", true) {
                    actions.push(Action::Settings);
                }
            });
            ui.add_space(8.0);
            if round_button(ui, None, "Enregistrer une vidéo", RED, true).clicked() {
                actions.push(Action::Capture(Mode::Video));
            }
            let more = arrow_button(ui, 56.0);
            Popup::menu(&more).show(|ui| {
                ui.set_min_width(300.0);
                let labels = labels();
                for (mode, text) in VIDEO_MODES {
                    if menu_item(ui, text, &shortcut_of(&labels, mode), true) {
                        actions.push(Action::Capture(mode));
                    }
                }
                ui.separator();
                if menu_item(ui, "Paramètres…", "F11", true) {
                    actions.push(Action::Settings);
                }
            });
            ui.add_space(8.0);
            let can_edit = !self.selection.is_empty();
            if round_button(ui, Some(icons::EDIT), "Éditer…", GREEN, can_edit)
                .on_hover_text("Ouvrir la sélection dans l'éditeur (Entrée)")
                .clicked()
            {
                actions.push(Action::Edit);
            }
            round_button(ui, Some(icons::OPEN), "Publier", BLUE, false)
                .on_hover_text("Bientôt : publication (PDF, courriel, partage)");
            if round_button(ui, Some(icons::CLOSE), "Fermer", ORANGE, true).clicked() {
                actions.push(Action::Close);
            }
        });
    }

    // --- Grille ----------------------------------------------------------------------

    fn grid(&mut self, ui: &mut Ui, frame: &eframe::Frame, actions: &mut Vec<Action>) {
        if self.view.is_empty() {
            ui.centered_and_justified(|ui| {
                let text = if self.items.is_empty() {
                    format!(
                        "Aucune capture dans « {} ».\nLes nouvelles captures arrivent dans « Base ».\nDéposez des fichiers ici pour les ajouter.",
                        self.workspace_name(&self.current.clone())
                    )
                } else {
                    "Aucun élément ne correspond à la recherche.".to_owned()
                };
                ui.label(RichText::new(text).color(theme::TEXT_DIM));
            });
            return;
        }
        let side = self.prefs.thumb_size;
        let cell = vec2(side + 2.0 * CELL_PAD, side + NAME_HEIGHT + 2.0 * CELL_PAD);
        let width = ui.available_width() - 14.0; // barre de défilement
        let columns = ((width / cell.x).floor() as usize).max(1);
        self.columns = columns;
        let rows = self.view.len().div_ceil(columns);
        let mut area = egui::ScrollArea::vertical()
            .id_salt("grid")
            .auto_shrink([false, false])
            .stick_to_bottom(self.prefs.sort == Sort::DateAsc);
        if let Some(row) = self.scroll_to_row.take() {
            let (offset, visible) = self.scroll;
            let top = row as f32 * cell.y;
            let bottom = top + cell.y;
            if top < offset {
                area = area.vertical_scroll_offset(top);
            } else if visible > 0.0 && bottom > offset + visible {
                area = area.vertical_scroll_offset(bottom - visible);
            }
        }
        // Colonnes réparties sur toute la largeur.
        let spread = (width - columns as f32 * cell.x) / columns as f32;
        let output = area.show_rows(ui, cell.y, rows, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in range {
                let (row_rect, _) = ui.allocate_exact_size(vec2(width, cell.y), Sense::hover());
                for col in 0..columns {
                    let n = row * columns + col;
                    let Some(&index) = self.view.get(n) else {
                        break;
                    };
                    let x = row_rect.left() + col as f32 * (cell.x + spread) + spread / 2.0;
                    let rect = Rect::from_min_size(pos2(x, row_rect.top()), cell);
                    self.cell(ui, frame, index, rect, actions);
                }
            }
        });
        self.scroll = (output.state.clamped_offset().y, output.inner_rect.height());
    }

    fn cell(
        &mut self,
        ui: &mut Ui,
        frame: &eframe::Frame,
        index: usize,
        rect: Rect,
        actions: &mut Vec<Action>,
    ) {
        let media = self.items[index].media.clone();
        let path = media.path.clone();
        let resp = ui.interact(rect, ui.scope_id().with(&path), Sense::click_and_drag());
        let selected = self.selection.contains(&path);
        let side = self.prefs.thumb_size;
        let frame_rect = rect.shrink(CELL_PAD / 2.0);
        let box_rect = Rect::from_min_size(rect.min + vec2(CELL_PAD, CELL_PAD), vec2(side, side));
        let painter = ui.painter_at(rect);
        if resp.hovered() && !selected {
            painter.rect_filled(
                frame_rect,
                CornerRadius::same(10),
                Color32::from_white_alpha(10),
            );
        }
        // Vignette, ajustée dans son carré, avec une ombre.
        let texture = self.thumbs.get(&media);
        match texture {
            Some(tex) => {
                let size = tex.size_vec2();
                let scale = (side / size.x.max(size.y))
                    .min(side / size.x.max(1.0))
                    .min(side / size.y.max(1.0));
                let shown = size * scale;
                let img_rect = Rect::from_center_size(box_rect.center(), shown);
                painter.rect_filled(
                    img_rect.translate(vec2(2.0, 3.0)),
                    CornerRadius::same(3),
                    Color32::from_black_alpha(120),
                );
                painter.image(
                    tex.id(),
                    img_rect,
                    Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                let placeholder = box_rect.shrink(side * 0.1);
                painter.rect_filled(placeholder, CornerRadius::same(6), theme::SURFACE_HI);
                let label = if self.thumbs.failed(&path) {
                    path.extension()
                        .map(|e| e.to_string_lossy().to_uppercase())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                painter.text(
                    placeholder.center(),
                    Align2::CENTER_CENTER,
                    label,
                    FontId::proportional(theme::SIZE_M),
                    theme::TEXT_DIM,
                );
            }
        }
        if media.kind == Kind::Video {
            play_badge(&painter, box_rect.center(), (side * 0.14).clamp(12.0, 26.0));
        }
        let name = media.name();
        let galley = elided(ui, &name, rect.width() - 10.0, theme::SIZE_S, theme::TEXT);
        let name_pos = pos2(
            rect.center().x - galley.size().x / 2.0,
            box_rect.bottom() + (NAME_HEIGHT - galley.size().y) / 2.0 + 2.0,
        );
        painter.galley(name_pos, galley, theme::TEXT);
        if selected {
            painter.rect_stroke(
                frame_rect,
                CornerRadius::same(10),
                Stroke::new(2.5, SELECTION),
                egui::StrokeKind::Inside,
            );
        }

        // Interactions.
        let mods = ui.input(|i| i.modifiers);
        if resp.clicked() {
            self.click(&path, mods);
        }
        if resp.double_clicked() {
            actions.push(Action::Open(path.clone()));
        }
        if resp.secondary_clicked() && !selected {
            self.selection = vec![path.clone()];
            self.cursor = Some(path.clone());
        }
        if resp.drag_started() {
            self.drag_out(frame, &path);
        }
        let resp = resp.on_hover_text(name);
        resp.context_menu(|ui| self.item_menu(ui, &path, actions));
    }

    /// Sélection au clic : seul, ⌘ ajoute ou retire, ⇧ prend la plage.
    fn click(&mut self, path: &Path, mods: Modifiers) {
        if mods.command {
            if let Some(i) = self.selection.iter().position(|p| p == path) {
                self.selection.remove(i);
            } else {
                self.selection.push(path.to_owned());
            }
            self.cursor = Some(path.to_owned());
        } else if mods.shift
            && let Some(from) = self.cursor.as_ref().and_then(|c| self.view_index(c))
            && let Some(to) = self.view_index(path)
        {
            let (a, b) = (from.min(to), from.max(to));
            self.selection = self.view[a..=b]
                .iter()
                .map(|&i| self.items[i].media.path.clone())
                .collect();
        } else {
            self.selection = vec![path.to_owned()];
            self.cursor = Some(path.to_owned());
        }
    }

    /// Glisser la sélection (ou l'élément saisi) vers une autre application.
    fn drag_out(&mut self, frame: &eframe::Frame, path: &Path) {
        if !dnd::SUPPORTED {
            return;
        }
        let paths = if self.selection.iter().any(|p| p == path) {
            self.selected()
        } else {
            vec![path.to_owned()]
        };
        let thumb = self.item(path).and_then(|item| {
            let key = files::thumb_key(
                item.media.modified,
                item.media.doc_modified,
                super::thumbs::THUMB_PX,
            );
            history::read_png(&files::thumb_path(path, &key)).ok()
        });
        let image = dnd::drag_image(thumb.as_ref());
        let ctx = self.ctx.clone();
        self.drag
            .start(frame, paths, image, move || ctx.request_repaint());
    }

    fn item_menu(&self, ui: &mut Ui, path: &Path, actions: &mut Vec<Action>) {
        ui.set_min_width(300.0);
        let selection = {
            let s = self.selected();
            if s.iter().any(|p| p == path) {
                s
            } else {
                vec![path.to_owned()]
            }
        };
        let kind = files::kind_of(path);
        let is_image = kind == Some(Kind::Image);
        // En-tête : dimensions et taille.
        if let Some(item) = self.item(path) {
            let dims = self.thumbs.dims(path).or_else(|| {
                is_image
                    .then(|| image::image_dimensions(path).ok())
                    .flatten()
            });
            let text = match dims {
                Some((w, h)) => format!(
                    "Dimensions : {w} × {h} ({})",
                    files::human_size(item.media.size)
                ),
                None => format!("Taille : {}", files::human_size(item.media.size)),
            };
            ui.label(
                RichText::new(text)
                    .color(theme::TEXT_DIM)
                    .size(theme::SIZE_S),
            );
            ui.separator();
        }
        let open_text = if is_image {
            "Ouvrir dans l'éditeur"
        } else {
            "Ouvrir"
        };
        if menu_item(ui, open_text, "↩", true) {
            actions.push(Action::Open(path.to_owned()));
        }
        if menu_item(ui, "Ouvrir avec l'application par défaut", "", true) {
            actions.push(Action::OpenDefault(path.to_owned()));
        }
        if menu_item(ui, keys::REVEAL, "", true) {
            actions.push(Action::Reveal(path.to_owned()));
        }
        ui.separator();
        if menu_item(ui, "Aperçu rapide", "Espace", is_image) {
            actions.push(Action::Preview(path.to_owned()));
        }
        if menu_item(ui, "Enregistrer sous…", &keys::cmd(false, "S"), is_image) {
            actions.push(Action::SaveAs(path.to_owned()));
        }
        if menu_item(ui, "Copier l'image", &keys::cmd(false, "C"), is_image) {
            actions.push(Action::CopyImage(path.to_owned()));
        }
        if menu_item(ui, "Copier le chemin", &keys::cmd(true, "C"), true) {
            actions.push(Action::CopyPaths(selection.clone()));
        }
        ui.separator();
        if menu_item(ui, "Supprimer", keys::DELETE, true) {
            actions.push(Action::Delete(selection.clone()));
        }
        if menu_item(ui, "Renommer…", "F2", true) {
            actions.push(Action::Rename(path.to_owned()));
        }
        let mut targets = vec![self.root.clone()];
        targets.extend(self.workspaces.iter().cloned());
        targets.retain(|d| *d != self.current);
        ui.add_enabled_ui(!targets.is_empty(), |ui| {
            ui.menu_button("Déplacer vers", |ui| {
                for dir in targets {
                    if ui.button(self.workspace_name(&dir)).clicked() {
                        actions.push(Action::MoveTo(selection.clone(), dir));
                    }
                }
            });
        });
        let images: Vec<PathBuf> = selection
            .iter()
            .filter(|p| files::kind_of(p) == Some(Kind::Image))
            .cloned()
            .collect();
        ui.add_enabled_ui(images.len() >= 2, |ui| {
            ui.menu_button("Fusionner", |ui| {
                if ui.button("Côte à côte").clicked() {
                    actions.push(Action::Merge(images.clone(), true));
                }
                if ui.button("Les unes sous les autres").clicked() {
                    actions.push(Action::Merge(images.clone(), false));
                }
            })
            .response
            .on_disabled_hover_text("Sélectionnez au moins deux images (⌘-clic ou ⇧-clic)");
        });
        ui.separator();
        if menu_item(ui, "Propriétés…", "F3", true) {
            actions.push(Action::Properties(path.to_owned()));
        }
    }

    // --- Superpositions ----------------------------------------------------------------

    fn overlays(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        // Fichiers survolés : où ils iront.
        let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
        if hovering && !self.drag.is_active() {
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(12), Color32::from_black_alpha(170));
            painter.rect_stroke(
                rect.shrink(6.0),
                CornerRadius::same(12),
                Stroke::new(2.0, theme::ACCENT_2),
                egui::StrokeKind::Inside,
            );
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!(
                    "Déposer pour ajouter à « {} »",
                    self.workspace_name(&self.current.clone())
                ),
                FontId::proportional(theme::SIZE_L),
                theme::TEXT,
            );
        }
        if let Some((text, at)) = &self.status {
            if at.elapsed() > super::STATUS_DURATION {
                self.status = None;
            } else {
                let galley = elided(ui, text, rect.width() - 40.0, theme::SIZE_S, theme::TEXT);
                let size = galley.size() + vec2(24.0, 12.0);
                let toast =
                    Rect::from_center_size(pos2(rect.center().x, rect.bottom() - size.y), size);
                let painter = ui.painter();
                painter.rect_filled(toast, CornerRadius::same(10), theme::SURFACE_HI);
                painter.rect_stroke(
                    toast,
                    CornerRadius::same(10),
                    Stroke::new(1.0, theme::BORDER),
                    egui::StrokeKind::Inside,
                );
                painter.galley(toast.min + vec2(12.0, 6.0), galley, theme::TEXT);
                ui.ctx().request_repaint_after(super::STATUS_DURATION);
            }
        }
    }

    // --- Boîtes de dialogue ------------------------------------------------------------

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let mut keep = true;
        let id = egui::Id::unique("workspace-dialog");
        let modal = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).fade_in(false))
            .backdrop_color(Color32::from_black_alpha(150))
            .show(ctx, |ui| {
            ui.set_width(380.0);
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            match &mut dialog {
                Dialog::Rename { path, text, error } => {
                    ui.label(RichText::new("Renommer").title().size(theme::SIZE_L));
                    ui.add_space(6.0);
                    let ext = path
                        .extension()
                        .map(|e| format!(".{}", e.to_string_lossy()))
                        .unwrap_or_default();
                    ui.horizontal(|ui| {
                        let edit = ui.add(egui::TextEdit::singleline(text).desired_width(280.0));
                        if !edit.has_focus() {
                            edit.request_focus();
                        }
                        ui.label(RichText::new(ext).color(theme::TEXT_DIM));
                    });
                    show_error(ui, error);
                    let (ok, cancel) = ok_cancel(ui, "Renommer");
                    if ok || enter {
                        let (path, name) = (path.clone(), text.clone());
                        match self.rename(&path, &name) {
                            Ok(()) => keep = false,
                            Err(e) => *error = Some(format!("{e:#}")),
                        }
                    }
                    keep &= !cancel;
                }
                Dialog::NewWorkspace { text, error } => {
                    ui.label(RichText::new("Nouvel espace de travail").title().size(theme::SIZE_L));
                    ui.label(
                        RichText::new("Un sous-dossier des captures. Les nouvelles captures vont toujours dans « Base ».")
                            .color(theme::TEXT_DIM)
                            .size(theme::SIZE_S),
                    );
                    ui.add_space(6.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(text)
                            .hint_text("Nom")
                            .desired_width(f32::INFINITY),
                    );
                    if !edit.has_focus() {
                        edit.request_focus();
                    }
                    show_error(ui, error);
                    let (ok, cancel) = ok_cancel(ui, "Créer");
                    if ok || enter {
                        let name = text.clone();
                        match self.create_workspace(&name) {
                            Ok(()) => keep = false,
                            Err(e) => *error = Some(format!("{e:#}")),
                        }
                    }
                    keep &= !cancel;
                }
                Dialog::RenameWorkspace { path, text, error } => {
                    ui.label(RichText::new("Renommer l'espace de travail").title().size(theme::SIZE_L));
                    ui.add_space(6.0);
                    let edit = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
                    if !edit.has_focus() {
                        edit.request_focus();
                    }
                    show_error(ui, error);
                    let (ok, cancel) = ok_cancel(ui, "Renommer");
                    if ok || enter {
                        let (path, name) = (path.clone(), text.clone());
                        match self.rename_workspace(&path, &name) {
                            Ok(()) => keep = false,
                            Err(e) => *error = Some(format!("{e:#}")),
                        }
                    }
                    keep &= !cancel;
                }
                Dialog::Delete { paths } => {
                    let n = paths.len();
                    let what = if n == 1 {
                        format!("« {} »", files::file_name(&paths[0]))
                    } else {
                        format!("ces {n} éléments")
                    };
                    ui.label(RichText::new("Mettre à la Corbeille ?").title().size(theme::SIZE_L));
                    ui.add_space(6.0);
                    ui.label(format!(
                        "{what} et {} annotations iront à la Corbeille, d'où on peut les récupérer.",
                        if n == 1 { "ses" } else { "leurs" }
                    ));
                    let (ok, cancel) = ok_cancel(ui, "Mettre à la Corbeille");
                    if ok || enter {
                        let paths = paths.clone();
                        self.trash(&paths);
                        keep = false;
                    }
                    keep &= !cancel;
                }
                Dialog::DeleteWorkspace { path } => {
                    let empty = files::is_empty_dir(path);
                    ui.label(RichText::new("Supprimer l'espace de travail ?").title().size(theme::SIZE_L));
                    ui.add_space(6.0);
                    if empty {
                        ui.label(format!("« {} » est vide : le dossier sera supprimé.", files::file_name(path)));
                    } else {
                        let n = files::list(path).len();
                        ui.label(format!(
                            "« {} » contient {n} élément{} : le dossier et son contenu iront à la Corbeille.",
                            files::file_name(path),
                            if n > 1 { "s" } else { "" }
                        ));
                    }
                    let (ok, cancel) = ok_cancel(ui, "Supprimer");
                    if ok || enter {
                        let path = path.clone();
                        self.delete_workspace(&path);
                        keep = false;
                    }
                    keep &= !cancel;
                }
                Dialog::Properties { path } => {
                    self.properties(ui, path);
                    keep &= !close_button(ui) && !enter;
                }
                Dialog::Help => {
                    help(ui);
                    keep &= !close_button(ui) && !enter;
                }
                Dialog::About => {
                    about(ui, &self.root);
                    keep &= !close_button(ui) && !enter;
                }
            }
        });
        if modal.should_close() {
            keep = false;
        }
        if keep && self.dialog.is_none() {
            self.dialog = Some(dialog);
        }
    }

    fn properties(&self, ui: &mut Ui, path: &Path) {
        ui.label(RichText::new("Propriétés").title().size(theme::SIZE_L));
        ui.add_space(6.0);
        let Some(item) = self.item(path) else {
            ui.label("Fichier introuvable.");
            return;
        };
        let m = &item.media;
        let kind = match m.kind {
            Kind::Image => "Image",
            Kind::Video => "Vidéo",
            Kind::Document => "Document",
        };
        let dims = self.thumbs.dims(path).or_else(|| {
            (m.kind == Kind::Image)
                .then(|| image::image_dimensions(path).ok())
                .flatten()
        });
        egui::Grid::new("properties")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                let row = |ui: &mut Ui, k: &str, v: String| {
                    ui.label(RichText::new(k).color(theme::TEXT_DIM));
                    ui.add(egui::Label::new(v).wrap());
                    ui.end_row();
                };
                row(ui, "Nom", m.name());
                row(ui, "Type", kind.into());
                if let Some((w, h)) = dims {
                    row(ui, "Dimensions", format!("{w} × {h} pixels"));
                }
                row(ui, "Taille", files::human_size(m.size));
                if let Some(created) = m.created {
                    row(ui, "Créé le", format_date(created));
                }
                row(ui, "Modifié le", format_date(m.modified));
                if let Some(doc) = m.doc_modified {
                    let n = history::load_doc(path).annotations.len();
                    row(
                        ui,
                        "Annotations",
                        format!("{n}, modifiées le {}", format_date(doc)),
                    );
                }
                if let Some(app) = &item.context.app {
                    row(ui, "Application", app.clone());
                }
                if let Some(title) = &item.context.title {
                    row(ui, "Fenêtre", title.clone());
                }
                row(
                    ui,
                    "Dossier",
                    path.parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                );
            });
    }

    // --- Clavier ---------------------------------------------------------------------

    pub(super) fn keyboard(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.preview.is_some() {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        let mut actions = Vec::new();
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let pressed = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
        let shift_cmd = Modifiers::COMMAND | Modifiers::SHIFT;
        // Les touches de fonction marchent même pendant la saisie.
        if pressed(sc(Modifiers::NONE, Key::F1)) {
            actions.push(Action::Help);
        }
        if pressed(sc(Modifiers::NONE, Key::F8)) {
            actions.push(Action::AlwaysOnTop(!self.prefs.always_on_top));
        }
        if pressed(sc(Modifiers::NONE, Key::F9)) {
            actions.push(Action::AutoClose(!self.prefs.auto_close));
        }
        if pressed(sc(Modifiers::NONE, Key::F10)) {
            actions.push(Action::TogglePanel);
        }
        if pressed(sc(Modifiers::NONE, Key::F11)) {
            actions.push(Action::Settings);
        }
        if pressed(sc(Modifiers::COMMAND, Key::F)) {
            self.search_open = true;
            self.focus_search = true;
        }
        if pressed(sc(Modifiers::COMMAND, Key::Plus))
            || pressed(sc(Modifiers::COMMAND, Key::Equals))
        {
            actions.push(Action::ZoomThumbs(prefs::THUMB_STEP));
        }
        if pressed(sc(Modifiers::COMMAND, Key::Minus)) {
            actions.push(Action::ZoomThumbs(-prefs::THUMB_STEP));
        }
        // ⌘ + molette : taille des vignettes.
        let zoom = ctx.input(|i| i.zoom_delta());
        if (zoom - 1.0).abs() > 0.01 {
            actions.push(Action::ZoomThumbs((zoom - 1.0) * 100.0));
        }
        if !typing {
            let target = self
                .cursor
                .clone()
                .or_else(|| self.selected().first().cloned());
            let selection = self.selected();
            if pressed(sc(shift_cmd, Key::C)) && !selection.is_empty() {
                actions.push(Action::CopyPaths(selection.clone()));
            }
            let copy = ctx.input_mut(|i| {
                let found = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                i.events.retain(|e| !matches!(e, egui::Event::Copy));
                found
            });
            if copy && let Some(t) = &target {
                actions.push(Action::CopyImage(t.clone()));
            }
            if pressed(sc(Modifiers::COMMAND, Key::A)) {
                self.selection = self
                    .view
                    .iter()
                    .map(|&i| self.items[i].media.path.clone())
                    .collect();
            }
            if pressed(sc(Modifiers::COMMAND, Key::S))
                && let Some(t) = &target
            {
                actions.push(Action::SaveAs(t.clone()));
            }
            if pressed(sc(Modifiers::NONE, Key::Enter))
                && let Some(t) = &target
            {
                actions.push(Action::Open(t.clone()));
            }
            if pressed(sc(Modifiers::NONE, Key::Space))
                && let Some(t) = &target
            {
                actions.push(Action::Preview(t.clone()));
            }
            if pressed(sc(Modifiers::NONE, Key::F2))
                && let Some(t) = &target
            {
                actions.push(Action::Rename(t.clone()));
            }
            if pressed(sc(Modifiers::NONE, Key::F3))
                && let Some(t) = &target
            {
                actions.push(Action::Properties(t.clone()));
            }
            let delete = pressed(sc(Modifiers::NONE, Key::Delete))
                || pressed(sc(Modifiers::NONE, Key::Backspace))
                || pressed(sc(Modifiers::COMMAND, Key::Backspace));
            if delete && !selection.is_empty() {
                actions.push(Action::Delete(selection));
            }
            let columns = self.columns.max(1) as isize;
            for (key, step) in [
                (Key::ArrowLeft, -1),
                (Key::ArrowRight, 1),
                (Key::ArrowUp, -columns),
                (Key::ArrowDown, columns),
            ] {
                if pressed(sc(Modifiers::NONE, key)) {
                    self.move_cursor(step);
                }
            }
            if pressed(sc(Modifiers::NONE, Key::Escape)) {
                if self.filter.is_active() || self.search_open {
                    self.filter = Default::default();
                    self.search_open = false;
                    self.update_view();
                } else {
                    actions.push(Action::Close);
                }
            }
            // Coller une image : nouvelle capture dans l'espace courant.
            let pasted: Vec<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::PasteImage(img) => Some(img.clone()),
                        _ => None,
                    })
                    .collect()
            });
            for img in pasted {
                self.paste_image(&img);
            }
            // Taper du texte : la recherche s'ouvre avec.
            let typed: String = ctx.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Text(t) if !i.modifiers.command => Some(t.clone()),
                        _ => None,
                    })
                    .collect()
            });
            let typed = typed.trim_start();
            if !typed.is_empty() {
                self.search_open = true;
                self.focus_search = true;
                self.filter.text.push_str(typed);
                self.update_view();
            }
        }
        for action in actions {
            self.apply(action);
        }
    }

    fn move_cursor(&mut self, step: isize) {
        if self.view.is_empty() {
            return;
        }
        let current = self.cursor.as_ref().and_then(|c| self.view_index(c));
        let next = match current {
            Some(i) => (i as isize + step).clamp(0, self.view.len() as isize - 1) as usize,
            None if step < 0 => self.view.len() - 1,
            None => 0,
        };
        let path = self.items[self.view[next]].media.path.clone();
        self.select_only(path);
    }

    fn paste_image(&mut self, img: &egui::ColorImage) {
        let [w, h] = img.size;
        let bytes: Vec<u8> = img
            .pixels
            .iter()
            .flat_map(|c| c.to_srgba_unmultiplied())
            .collect();
        let Some(rgba) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) else {
            return;
        };
        let result = history::save_capture(&self.current, &rgba);
        if let Some(path) = self.report(result) {
            self.refresh();
            self.select_only(path);
            self.say(format!("{} Image collée", icons::OK));
        }
    }

    // --- Actions ---------------------------------------------------------------------

    pub(super) fn apply(&mut self, action: Action) {
        match action {
            Action::Open(p) => self.open(&p),
            Action::OpenDefault(p) => files::open_default(&p),
            Action::Reveal(p) => history::reveal(&p),
            Action::Preview(p) => {
                self.cursor = Some(p.clone());
                if !self.selection.contains(&p) {
                    self.selection = vec![p];
                }
                self.toggle_preview();
            }
            Action::SaveAs(p) => self.save_as(&p),
            Action::CopyImage(p) => self.copy_image(&p),
            Action::CopyPaths(p) => self.copy_paths(&p),
            Action::Delete(paths) => self.dialog = Some(Dialog::Delete { paths }),
            Action::Rename(path) => {
                let text = files::stem(&path);
                self.dialog = Some(Dialog::Rename {
                    path,
                    text,
                    error: None,
                });
            }
            Action::MoveTo(paths, dir) => self.move_to(&paths, &dir),
            Action::Merge(paths, horizontal) => self.merge(&paths, horizontal),
            Action::Properties(path) => self.dialog = Some(Dialog::Properties { path }),
            Action::Capture(mode) => self.close(Some(After::Capture(mode))),
            Action::Edit => {
                for path in self.selected() {
                    self.open(&path);
                }
            }
            Action::Settings => super::open_settings(&self.store),
            Action::Help => self.dialog = Some(Dialog::Help),
            Action::About => self.dialog = Some(Dialog::About),
            Action::Quit => self.close(Some(After::Quit)),
            Action::Close => self.close(None),
            Action::SetWorkspace(dir) => self.set_workspace(dir),
            Action::NewWorkspace => {
                self.dialog = Some(Dialog::NewWorkspace {
                    text: String::new(),
                    error: None,
                })
            }
            Action::RenameWorkspace(path) => {
                let text = files::file_name(&path);
                self.dialog = Some(Dialog::RenameWorkspace {
                    path,
                    text,
                    error: None,
                });
            }
            Action::DeleteWorkspace(path) => self.dialog = Some(Dialog::DeleteWorkspace { path }),
            Action::OpenFolder(dir) => history::open_dir(&dir),
            Action::AlwaysOnTop(on) => self.set_always_on_top(on),
            Action::AutoClose(on) => {
                self.prefs.auto_close = on;
                self.save_prefs();
                self.say(if on {
                    "Fermeture automatique activée"
                } else {
                    "Fermeture automatique désactivée"
                });
            }
            Action::RememberPosition(on) => {
                self.prefs.remember_position = on;
                if !on {
                    self.prefs.position = None;
                }
                self.save_prefs();
            }
            Action::ZoomThumbs(delta) => self.zoom_thumbs(delta),
            Action::Sort(sort) => self.set_sort(sort),
            Action::TogglePanel => {
                self.prefs.show_workspaces = !self.prefs.show_workspaces;
                self.save_prefs();
            }
            Action::ToggleSearch => {
                self.search_open = !self.search_open;
                self.focus_search = self.search_open;
                if !self.search_open {
                    self.filter = Default::default();
                    self.update_view();
                }
            }
            Action::ToggleMaximize => {
                let maximized = self.ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                self.ctx
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
        }
    }

    // --- Aperçu rapide ---------------------------------------------------------------

    pub(super) fn show_preview(&mut self, ctx: &egui::Context) {
        let outer = ctx.input(|i| i.viewport().outer_rect);
        let monitor = ctx
            .input(|i| i.viewport().monitor_size)
            .unwrap_or(vec2(1440.0, 900.0));
        let ppp = ctx
            .input(|i| i.viewport().native_pixels_per_point)
            .unwrap_or(1.0)
            .max(0.5);
        let Some(preview) = &mut self.preview else {
            return;
        };
        if preview.texture.is_none() && preview.error.is_none() {
            let loaded = preview
                .loading
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            match loaded {
                Some(Ok(img)) => {
                    let size = [img.width() as usize, img.height() as usize];
                    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                    preview.texture =
                        Some(ctx.load_texture("preview", color, egui::TextureOptions::LINEAR));
                    // Taille réelle en points, réduite pour tenir dans 85 % de l'écran.
                    let natural = vec2(img.width() as f32, img.height() as f32) / ppp;
                    let max = monitor * 0.85;
                    let scale = (max.x / natural.x).min(max.y / natural.y).min(1.0);
                    let s = natural * scale;
                    preview.size = [s.x.max(160.0), s.y.max(120.0)];
                }
                Some(Err(e)) => preview.error = Some(e),
                None => {}
            }
        }
        let size = Vec2::from(preview.size);
        let center = outer
            .map(|r| r.center())
            .unwrap_or(pos2(monitor.x / 2.0, monitor.y / 2.0));
        let pos = (center - size / 2.0).max(Pos2::ZERO);
        let mut builder = egui::ViewportBuilder::default()
            .with_title(format!("Aperçu · {}", files::file_name(&preview.path)))
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_inner_size(size);
        if !preview.placed && preview.texture.is_some() {
            builder = builder.with_position(pos);
        }
        let texture = preview.texture.clone();
        let error = preview.error.clone();
        let ready = texture.is_some() || error.is_some();
        let mut close = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("snappy-preview"),
            builder,
            |ui, _class| {
                let rect = ui.max_rect();
                let painter = ui.painter();
                painter.rect_filled(rect, CornerRadius::ZERO, theme::BG);
                match (&texture, &error) {
                    (Some(tex), _) => {
                        painter.image(
                            tex.id(),
                            rect,
                            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    (None, Some(e)) => {
                        painter.text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            e,
                            FontId::proportional(theme::SIZE_M),
                            theme::DANGER,
                        );
                    }
                    _ => {
                        painter.text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            "Chargement…",
                            FontId::proportional(theme::SIZE_M),
                            theme::TEXT_DIM,
                        );
                    }
                }
                let resp = ui.interact(rect, ui.scope_id().with("preview"), Sense::click());
                close |= resp.clicked();
                close |= ui.input(|i| {
                    i.key_pressed(Key::Space)
                        || i.key_pressed(Key::Escape)
                        || i.viewport().close_requested()
                });
            },
        );
        if let Some(preview) = &mut self.preview {
            if ready {
                preview.placed = true;
            }
            preview.close |= close;
            if preview.close {
                self.preview = None;
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
                self.auto_close.reset();
            }
        }
    }
}

/// ▶ au centre d'une vignette de vidéo.
fn play_badge(painter: &egui::Painter, center: Pos2, r: f32) {
    painter.circle_filled(center, r, Color32::from_black_alpha(160));
    painter.circle_stroke(center, r, Stroke::new(1.5, Color32::from_white_alpha(200)));
    let s = r * 0.5;
    painter.add(egui::Shape::convex_polygon(
        vec![
            center + vec2(-s * 0.7, -s),
            center + vec2(s, 0.0),
            center + vec2(-s * 0.7, s),
        ],
        Color32::WHITE,
        Stroke::NONE,
    ));
}

fn show_error(ui: &mut Ui, error: &Option<String>) {
    if let Some(e) = error {
        ui.label(RichText::new(format!("{} {e}", icons::WARNING)).color(theme::DANGER));
    }
}

/// Boutons « Annuler » et `ok` en bas d'une boîte ; rend (ok, annuler).
fn ok_cancel(ui: &mut Ui, ok: &str) -> (bool, bool) {
    ui.add_space(10.0);
    let mut result = (false, false);
    // Une rangée : sans elle, la mise en page de droite à gauche prendrait toute la
    // hauteur restante.
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            result.0 = ui
                .add(
                    egui::Button::new(RichText::new(ok).color(theme::ON_ACCENT_2))
                        .fill(theme::ACCENT_2),
                )
                .clicked();
            result.1 = ui.button("Annuler").clicked();
        });
    });
    result
}

fn close_button(ui: &mut Ui) -> bool {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.button("Fermer").clicked()
        })
        .inner
    })
    .inner
}

fn help(ui: &mut Ui) {
    ui.label(RichText::new("Aide").title().size(theme::SIZE_L));
    ui.add_space(6.0);
    let rows = [
        (
            "Double-clic, Entrée".to_owned(),
            "Ouvrir dans l'éditeur (vidéos : application par défaut)",
        ),
        (
            "Espace".into(),
            "Aperçu rapide (Espace ou Échap pour fermer)",
        ),
        ("F2".into(), "Renommer (les annotations suivent)"),
        (keys::DELETE.into(), "Mettre à la Corbeille"),
        (keys::cmd(false, "C"), "Copier l'image avec ses annotations"),
        (keys::cmd(true, "C"), "Copier le chemin"),
        (
            keys::cmd(false, "S"),
            "Enregistrer sous (PNG, JPEG, BMP, TIFF, GIF, WebP)",
        ),
        (keys::cmd(false, "F"), "Rechercher (ou tapez simplement)"),
        (
            format!(
                "{} {} molette",
                keys::cmd(false, "+"),
                keys::cmd(false, "−")
            ),
            "Taille des vignettes",
        ),
        (
            "F8 · F9 · F10".into(),
            "Toujours devant · Fermeture automatique · Espaces",
        ),
        ("F3".into(), "Propriétés"),
    ];
    egui::Grid::new("help")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for (k, v) in rows {
                ui.label(RichText::new(k).strong());
                ui.label(v);
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.label(
        RichText::new(
            "Glissez une capture vers le Finder, Mail ou Messages pour la partager ; déposez des \
             fichiers dans la fenêtre pour les ajouter à l'espace courant.",
        )
        .color(theme::TEXT_DIM)
        .size(theme::SIZE_S),
    );
}

fn about(ui: &mut Ui, root: &Path) {
    ui.label(
        RichText::new(format!("Snappy {}", env!("CARGO_PKG_VERSION")))
            .title()
            .size(theme::SIZE_L),
    );
    ui.label("Capture d'écran légère, annotations non destructives.");
    ui.add_space(6.0);
    ui.label(
        RichText::new("Licence MIT. Icônes Lucide (ISC/MIT), polices Outfit et Unbounded (OFL).")
            .color(theme::TEXT_DIM)
            .size(theme::SIZE_S),
    );
    ui.label(
        RichText::new(format!("Captures : {}", root.display()))
            .color(theme::TEXT_DIM)
            .size(theme::SIZE_S),
    );
}
