//! Fenêtre des réglages : `snappy --settings`, une fenêtre egui dans son propre
//! processus, comme l'éditeur. L'agent n'en garde qu'une à la fois.
//!
//! Chaque changement est écrit tout de suite dans `config.json`, puis l'agent relit
//! la config (raccourcis, menu) et dit quels raccourcis le système a refusés. Sans
//! agent, les réglages s'enregistrent quand même : il les lira à son lancement.

mod hotkey;
mod link;

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::{Align, Event, Frame, Layout, Margin, RichText, Stroke, Ui};

use crate::agent::hotkeys::{MODES, label_of};
use crate::config::{Config, DEFAULT_HOTKEYS, Store};
use crate::history;
use crate::ipc::{self, Mode};
use crate::platform;
use crate::ui::theme::{self, Typo};
use crate::ui::{icons, keys};
use hotkey::{Recorded, Recorder};
use link::{Agent, Command};

/// Argument de la fenêtre elle-même. `snappy --settings` passe par l'agent, qui
/// relance l'exécutable avec cet argument (une seule fenêtre à la fois).
pub const WINDOW_ARG: &str = "--settings-window";

/// Renouvellement de la suspension des raccourcis pendant un enregistrement.
const RENEW_EVERY: Duration = Duration::from_secs(5);
const STATUS_DURATION: Duration = Duration::from_secs(3);

pub fn run(store: Store) -> Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Snappy · Réglages")
        .with_inner_size([620.0, 800.0])
        .with_min_inner_size([520.0, 420.0])
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
            platform::activate(cc);
            let ctx = cc.egui_ctx.clone();
            Ok(Box::new(Settings::new(store, move || {
                ctx.request_repaint()
            })))
        }),
    )
    .map_err(|e| anyhow::anyhow!("réglages : {e}"))
}

/// Icône de la fenêtre (et du Dock sous macOS, d'où la taille de 256 px).
fn window_icon() -> egui::IconData {
    #[cfg(target_os = "macos")]
    let (rgba, size): (&[u8], u32) = (include_bytes!("../../assets/icon_mac_256.rgba"), 256);
    #[cfg(not(target_os = "macos"))]
    let (rgba, size): (&[u8], u32) = (include_bytes!("../../assets/icon_64.rgba"), 64);
    egui::IconData {
        rgba: rgba.to_vec(),
        width: size,
        height: size,
    }
}

/// Enregistrement d'un nouveau raccourci en cours.
struct Recording {
    mode: Mode,
    recorder: Recorder,
    /// Combinaison refusée, et pourquoi.
    error: Option<String>,
    /// Dernière demande de suspension à l'agent.
    renewed: Instant,
}

struct Settings {
    store: Store,
    config: Config,
    agent: Agent,
    recording: Option<Recording>,
    status: Option<(String, Instant)>,
}

impl Settings {
    fn new(store: Store, repaint: impl Fn() + Send + 'static) -> Self {
        let config = store.load();
        let agent = Agent::new(store.socket_path(), repaint);
        // Raccourcis refusés dès l'ouverture, et présence de l'agent.
        agent.send(Command::Reload);
        Self {
            store,
            config,
            agent,
            recording: None,
            status: None,
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), Instant::now()));
    }

    /// Écrit la config et demande à l'agent de la relire.
    fn save(&mut self) {
        match self.store.save(&self.config) {
            Ok(()) => {
                self.say(format!("{} Enregistré", icons::OK));
                self.agent.send(Command::Reload);
            }
            Err(e) => self.say(format!(
                "{} Réglages non enregistrés : {e:#}",
                icons::WARNING
            )),
        }
    }

    // --- Raccourcis -------------------------------------------------------------

    fn start_recording(&mut self, mode: Mode) {
        self.recording = Some(Recording {
            mode,
            recorder: Recorder::default(),
            error: None,
            renewed: Instant::now(),
        });
        self.agent.send(Command::Suspend);
    }

    /// Fin d'enregistrement sans changement : les raccourcis reviennent.
    fn cancel_recording(&mut self) {
        self.recording = None;
        self.agent.send(Command::Reload);
    }

    /// Nouveau raccourci (déjà validé) pour `mode`.
    fn set_hotkey(&mut self, mode: Mode, text: String) {
        self.recording = None;
        *self.config.hotkey_mut(mode) = text;
        self.save();
    }

    /// Combinaison tapée ou choisie pendant l'enregistrement.
    fn try_hotkey(&mut self, text: String) {
        let Some(rec) = &mut self.recording else {
            return;
        };
        let mode = rec.mode;
        match hotkey::validate(&text, mode, &self.config) {
            Ok(()) => self.set_hotkey(mode, text),
            Err(e) => {
                let shown = label_of(&text).unwrap_or(text);
                rec.error = Some(format!("{shown} : {e}"));
            }
        }
    }

    /// Clavier pendant l'enregistrement. Les événements sont retirés de la frame :
    /// Espace ou Entrée n'activent pas un bouton, Tab ne déplace pas le focus.
    fn read_keyboard(&mut self, ctx: &egui::Context) {
        let Some(rec) = &mut self.recording else {
            return;
        };
        if rec.renewed.elapsed() >= RENEW_EVERY {
            rec.renewed = Instant::now();
            self.agent.send(Command::Suspend);
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        let (events, modifiers, focused) = ctx.input_mut(|i| {
            let events = i.events.clone();
            i.events.retain(|e| {
                !matches!(
                    e,
                    Event::Key { .. }
                        | Event::Text(_)
                        | Event::Copy
                        | Event::Cut
                        | Event::Paste(_)
                        | Event::PasteImage(_)
                )
            });
            (events, i.modifiers, i.focused)
        });
        if !focused {
            rec.recorder.focus_lost();
        }
        match rec.recorder.read(&events, modifiers) {
            Some(Recorded::Combo(text)) => self.try_hotkey(text),
            Some(Recorded::Cancel) => self.cancel_recording(),
            Some(Recorded::Unsupported) => {
                rec.error = Some("Cette touche ne peut pas servir de raccourci global.".into());
            }
            None => {}
        }
    }

    fn hotkeys_section(&mut self, ui: &mut Ui, refused: &[ipc::Refused]) {
        section(ui, icons::REGION, "Raccourcis", |ui| {
            hint(
                ui,
                "Actifs partout, même quand Snappy n'est pas au premier plan.",
            );
            ui.add_space(4.0);
            for (i, mode) in MODES.into_iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                let refusal = refused
                    .iter()
                    .find(|r| r.mode == mode && r.hotkey == self.config.hotkey(mode))
                    .map(|r| r.reason.clone());
                self.hotkey_row(ui, mode, refusal);
            }
        });
    }

    fn hotkey_row(&mut self, ui: &mut Ui, mode: Mode, refusal: Option<String>) {
        let recording_this = self.recording.as_ref().is_some_and(|r| r.mode == mode);
        let busy = self.recording.is_some() && !recording_this;
        let current = self.config.hotkey(mode).to_owned();
        let default = DEFAULT_HOTKEYS.get(mode);
        let live = ui.input(|i| i.modifiers);

        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            let icon = match mode {
                Mode::Region => icons::REGION,
                Mode::Screen => icons::SCREEN,
                Mode::Previous => icons::CROP,
                _ => icons::REGION,
            };
            ui.label(
                RichText::new(icon)
                    .size(theme::SIZE_L)
                    .color(theme::TEXT_DIM),
            );
            ui.label(RichText::new(hotkey::mode_name(mode)).size(theme::SIZE_M));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if recording_this {
                    if ui
                        .button(format!("{} Annuler", icons::CLOSE))
                        .on_hover_text("Échap")
                        .clicked()
                    {
                        self.cancel_recording();
                    }
                } else {
                    let none = ui
                        .add_enabled(
                            !busy && !current.is_empty(),
                            egui::Button::new(RichText::new(icons::CLOSE)),
                        )
                        .on_hover_text("Aucun raccourci");
                    if none.clicked() {
                        self.set_hotkey(mode, String::new());
                    }
                    let reset = ui
                        .add_enabled(!busy && current != default, egui::Button::new("Par défaut"))
                        .on_hover_text(label_of(default).unwrap_or_default());
                    if reset.clicked() {
                        match hotkey::validate(default, mode, &self.config) {
                            Ok(()) => self.set_hotkey(mode, default.to_owned()),
                            Err(e) => self.say(format!(
                                "{} {} : {e}",
                                icons::WARNING,
                                label_of(default).unwrap_or_default()
                            )),
                        }
                    }
                    let edit = ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(format!("{} Modifier", icons::EDIT)),
                        )
                        .on_hover_text("Enregistrer une nouvelle combinaison");
                    if edit.clicked() {
                        // Sinon Entrée ou Espace recliqueraient ce bouton.
                        edit.surrender_focus();
                        self.start_recording(mode);
                    }
                }
                let keycap = match &self.recording {
                    Some(rec) if recording_this => {
                        let mods = rec.recorder.live(live);
                        if mods.any() {
                            Keycap::Recording(format!("{}…", mods.label()))
                        } else {
                            Keycap::Recording("Appuie sur la combinaison…".into())
                        }
                    }
                    _ => match label_of(&current) {
                        Some(l) => Keycap::Set(l, refusal.is_some()),
                        None if current.trim().is_empty() => Keycap::None,
                        None => Keycap::Set(current.clone(), true),
                    },
                };
                keycap.show(ui);
            });
        });

        if let Some(rec) = &self.recording
            && recording_this
        {
            let text = rec.error.clone().unwrap_or_else(|| {
                "Échap : annuler. Sans modificateur, seules F1 à F24 et Impr. écran sont \
                 acceptées."
                    .into()
            });
            let color = if rec.error.is_some() {
                theme::WARNING
            } else {
                theme::TEXT_DIM
            };
            ui.label(RichText::new(text).size(theme::SIZE_S).color(color));
            if !cfg!(target_os = "macos") {
                self.quick_keys(ui);
            }
        } else if let Some(reason) = refusal {
            ui.label(
                RichText::new(format!("{} Inactif : {reason}.", icons::WARNING))
                    .size(theme::SIZE_S)
                    .color(theme::WARNING),
            );
        } else if label_of(&current).is_none() && !current.trim().is_empty() {
            ui.label(
                RichText::new(format!(
                    "{} « {current} » illisible dans config.json.",
                    icons::WARNING
                ))
                .size(theme::SIZE_S)
                .color(theme::WARNING),
            );
        }
    }

    /// Touches qu'on ne peut pas toujours taper ici : Windows réserve parfois
    /// Impr. écran à son outil de capture.
    fn quick_keys(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            ui.label(
                RichText::new("Ou choisis (maintiens Ctrl, Alt ou Maj pour combiner) :")
                    .size(theme::SIZE_S)
                    .color(theme::TEXT_DIM),
            );
            for (key, name) in [
                (global_hotkey::hotkey::Code::PrintScreen, "Impr. écran"),
                (global_hotkey::hotkey::Code::Pause, "Pause"),
            ] {
                let response = ui.small_button(name);
                if response.clicked() {
                    response.surrender_focus();
                    let mods = ui.input(|i| {
                        let rec = self.recording.as_ref();
                        rec.map(|r| r.recorder.live(i.modifiers))
                            .unwrap_or_default()
                    });
                    self.try_hotkey(hotkey::config_text(mods, key));
                }
            }
        });
    }

    // --- Capture et enregistrement ------------------------------------------------

    fn capture_section(&mut self, ui: &mut Ui) {
        section(ui, icons::SCREEN, "Capture", |ui| {
            let mut changed = ui
                .checkbox(
                    &mut self.config.capture_cursor,
                    "Inclure le curseur de la souris",
                )
                .changed();
            hint(ui, "La touche M l'inverse pendant la sélection d'une zone.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Délai de la capture différée");
                let mut delay = self
                    .config
                    .delay_seconds
                    .clamp(*Config::DELAY_RANGE.start(), *Config::DELAY_RANGE.end());
                if ui
                    .add(egui::Slider::new(&mut delay, Config::DELAY_RANGE).suffix(" s"))
                    .changed()
                    && delay != self.config.delay_seconds
                {
                    self.config.delay_seconds = delay;
                    changed = true;
                }
            });
            hint(
                ui,
                "Le temps d'ouvrir un menu ou de survoler un élément avant la capture.",
            );
            if changed {
                self.save();
            }
        });
    }

    fn save_section(&mut self, ui: &mut Ui) {
        section(ui, icons::SAVE, "Enregistrement", |ui| {
            ui.label("Dossier des images enregistrées");
            let dir = self.store.save_dir(&self.config);
            Frame::new()
                .fill(theme::BG)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(8)
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(RichText::new(dir.display().to_string()).monospace())
                            .truncate(),
                    )
                    .on_hover_text(dir.display().to_string());
                });
            if self.config.save_dir.is_none() {
                hint(ui, "Par défaut : le dossier Images, sous-dossier Snappy.");
            }
            ui.horizontal(|ui| {
                if ui.button(format!("{} Choisir…", icons::FOLDER)).clicked() {
                    self.choose_dir(&dir);
                }
                if ui
                    .button(format!("{} Ouvrir", icons::OPEN))
                    .on_hover_text(keys::REVEAL)
                    .clicked()
                {
                    history::open_dir(&dir);
                }
                if self.config.save_dir.is_some() && ui.button("Par défaut").clicked() {
                    self.config.save_dir = None;
                    self.save();
                }
            });
        });
    }

    fn choose_dir(&mut self, current: &Path) {
        let mut dialog = rfd::FileDialog::new().set_title("Dossier des images enregistrées");
        if current.is_dir() {
            dialog = dialog.set_directory(current);
        }
        if let Some(dir) = dialog.pick_folder()
            && self.config.save_dir.as_deref() != Some(dir.as_path())
        {
            self.config.save_dir = Some(dir);
            self.save();
        }
    }

    // --- Barres -----------------------------------------------------------------

    fn header(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(icons::SETTINGS)
                    .size(theme::SIZE_ICON)
                    .color(theme::ACCENT_2),
            );
            ui.label(RichText::new("Réglages").title().size(theme::SIZE_L));
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, agent: &link::Status) {
        ui.horizontal(|ui| {
            let (dot, text) = match agent.running {
                Some(true) => (theme::ACCENT_2, "Snappy est actif".to_owned()),
                Some(false) => (
                    theme::WARNING,
                    "Snappy ne tourne pas : les réglages serviront à son prochain lancement"
                        .to_owned(),
                ),
                None => (theme::TEXT_DIM, "Connexion à Snappy…".to_owned()),
            };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 4.0, dot);
            ui.label(
                RichText::new(text)
                    .size(theme::SIZE_S)
                    .color(theme::TEXT_DIM),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some((text, at)) = &self.status {
                    if at.elapsed() < STATUS_DURATION {
                        ui.label(
                            RichText::new(text)
                                .size(theme::SIZE_S)
                                .color(theme::ACCENT_2),
                        );
                        ui.ctx().request_repaint_after(STATUS_DURATION);
                    } else {
                        self.status = None;
                    }
                }
            });
        });
        if let Some(error) = &agent.error {
            ui.label(
                RichText::new(format!("{} {error}", icons::WARNING))
                    .size(theme::SIZE_S)
                    .color(theme::WARNING),
            );
        }
    }
}

impl eframe::App for Settings {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.read_keyboard(&ctx);
        let agent = self.agent.status();
        egui::Panel::top("header")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(16, 12)),
            )
            .show(ui, |ui| self.header(ui));
        egui::Panel::bottom("status")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(16, 6)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.status_bar(ui, &agent));
        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.hotkeys_section(ui, &agent.refused);
                        ui.add_space(12.0);
                        self.capture_section(ui);
                        ui.add_space(12.0);
                        self.save_section(ui);
                    });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Fermée pendant un enregistrement : les raccourcis reviennent tout de suite.
        if self.recording.take().is_some() {
            self.agent.reload_now();
        }
    }
}

/// Raccourci affiché comme une touche.
enum Keycap {
    /// Libellé ; `true` : inactif (refusé ou illisible).
    Set(String, bool),
    None,
    Recording(String),
}

impl Keycap {
    fn show(self, ui: &mut Ui) {
        let (text, stroke, fill) = match self {
            Self::Set(text, false) => (
                RichText::new(text).bold().size(theme::SIZE_M),
                theme::BORDER,
                theme::SURFACE_HI,
            ),
            Self::Set(text, true) => (
                RichText::new(text)
                    .bold()
                    .size(theme::SIZE_M)
                    .color(theme::WARNING)
                    .strikethrough(),
                theme::WARNING,
                theme::SURFACE_HI,
            ),
            Self::None => (
                RichText::new("Aucun")
                    .size(theme::SIZE_M)
                    .color(theme::TEXT_DIM),
                theme::BORDER,
                theme::BG,
            ),
            Self::Recording(text) => (
                RichText::new(text)
                    .size(theme::SIZE_M)
                    .color(theme::ACCENT_2),
                theme::ACCENT_2,
                theme::BG,
            ),
        };
        Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(6)
            .inner_margin(Margin::symmetric(10, 4))
            .show(ui, |ui| ui.label(text));
    }
}

/// Carte d'une section, avec son titre.
fn section(ui: &mut Ui, icon: &str, title: &str, body: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(theme::CARD_RADIUS)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(icon)
                        .size(theme::SIZE_L)
                        .color(theme::ACCENT_2),
                );
                ui.label(RichText::new(title).bold().size(theme::SIZE_L));
            });
            ui.add_space(4.0);
            body(ui);
        });
}

/// Texte d'aide, discret.
fn hint(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(theme::SIZE_S)
            .color(theme::TEXT_DIM),
    );
}
