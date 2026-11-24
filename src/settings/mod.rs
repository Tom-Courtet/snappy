//! Fenêtre des réglages : `snappy --settings`, une fenêtre egui dans son propre
//! processus, comme l'éditeur. L'agent n'en garde qu'une à la fois.
//!
//! Chaque changement est écrit tout de suite dans `config.json`, puis l'agent relit
//! la config (raccourcis, menu) et dit quels raccourcis le système a refusés. Sans
//! agent, les réglages s'enregistrent quand même : il les lira à son lancement.

mod hotkey;
mod link;
// --- Lot « Réglages, raccourcis, après capture » ---
mod login;

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::{Align, Event, Frame, Layout, Margin, RichText, Stroke, Ui};

use crate::agent::hotkeys::label_of;
use crate::config::{AfterCapture, ClipboardContent, Corner, FileName, ImageFormat, VideoQuality};
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
            platform::show_in_dock();
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

/// Onglets de la fenêtre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Capture,
    Video,
    Hotkeys,
    Advanced,
}

impl Tab {
    const ALL: [Self; 5] = [
        Self::General,
        Self::Capture,
        Self::Video,
        Self::Hotkeys,
        Self::Advanced,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "Général",
            Self::Capture => "Capture",
            Self::Video => "Vidéo",
            Self::Hotkeys => "Raccourcis",
            Self::Advanced => "Avancé",
        }
    }
}

/// Action à confirmer dans une fenêtre modale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Confirm {
    RestoreHotkeys,
    RestoreSettings,
}

struct Settings {
    store: Store,
    config: Config,
    agent: Agent,
    recording: Option<Recording>,
    status: Option<(String, Instant)>,
    // --- Lot « Réglages, raccourcis, après capture » ---
    tab: Tab,
    /// Modèle de nom en cours de saisie (enregistré seulement s'il est valide).
    template: String,
    /// Textes en cours de saisie, enregistrés à la sortie du champ.
    watermark_text: String,
    mic_device: String,
    confirm: Option<Confirm>,
    /// Lancement à l'ouverture de session, tel que le système le voit.
    login: login::State,
}

impl Settings {
    fn new(store: Store, repaint: impl Fn() + Send + 'static) -> Self {
        let config = store.load();
        let agent = Agent::new(store.socket_path(), repaint);
        // Raccourcis refusés dès l'ouverture, et présence de l'agent.
        agent.send(Command::Reload);
        let template = match &config.file_name {
            FileName::Custom(t) => t.clone(),
            _ => "snappy-{date}".into(),
        };
        Self {
            watermark_text: config.watermark.text.clone(),
            mic_device: config.video.mic_device.clone(),
            template,
            store,
            config,
            agent,
            recording: None,
            status: None,
            // La liste des raccourcis reste la première vue, comme avant les onglets.
            tab: Tab::Hotkeys,
            confirm: None,
            login: login::state(),
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
            for (g, (title, modes)) in hotkey::mode_groups().into_iter().enumerate() {
                // Les 13 actions, par groupe.
                ui.add_space(if g > 0 { 10.0 } else { 2.0 });
                ui.label(
                    RichText::new(title.to_uppercase())
                        .size(theme::SIZE_S)
                        .color(theme::TEXT_DIM),
                );
                for (i, mode) in modes.into_iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    let refusal = refused
                        .iter()
                        .find(|r| r.mode == mode && r.hotkey == self.config.hotkey(mode))
                        .map(|r| r.reason.clone());
                    self.hotkey_row(ui, mode, refusal);
                }
            }
            ui.add_space(12.0);
            let restore = ui
                .add_enabled(
                    self.recording.is_none(),
                    egui::Button::new(format!(
                        "{} Restaurer les raccourcis par défaut",
                        icons::UNDO
                    )),
                )
                .on_hover_text("Les trois raccourcis d'origine ; aucun pour les autres actions");
            if restore.clicked() {
                self.confirm = Some(Confirm::RestoreHotkeys);
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
            let icon = hotkey::mode_icon(mode);
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

    // --- Lot « Réglages, raccourcis, après capture » -----------------------------

    fn general_tab(&mut self, ui: &mut Ui) {
        section(ui, icons::SETTINGS, "Général", |ui| {
            let mut on = self.config.launch_at_login;
            let available = !matches!(self.login, login::State::Unavailable(_));
            let label = if cfg!(windows) {
                "Lancer Snappy au démarrage de Windows"
            } else {
                "Lancer Snappy à l'ouverture de session"
            };
            if ui
                .add_enabled(available || on, egui::Checkbox::new(&mut on, label))
                .changed()
            {
                self.set_launch_at_login(on);
            }
            if let Some(text) = self.login.hint() {
                hint(ui, &text);
            }
            ui.add_space(6.0);
            if ui
                .checkbox(
                    &mut self.config.confirm_delete,
                    "Demander avant de supprimer une capture",
                )
                .changed()
            {
                self.save();
            }
            hint(ui, "Dans l'espace de travail.");
        });
        ui.add_space(12.0);
        section(ui, icons::FOLDER, "Dossiers", |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{} Dossier des données", icons::FOLDER))
                    .on_hover_text(self.store.root().display().to_string())
                    .clicked()
                {
                    history::open_dir(self.store.root());
                }
                if ui
                    .button(format!("{} Images enregistrées", icons::FOLDER))
                    .clicked()
                {
                    history::open_dir(&self.store.save_dir(&self.config));
                }
            });
            hint(
                ui,
                "Le dossier des données contient les réglages et l'historique des captures.",
            );
        });
    }

    /// Coche ou décoche le lancement à l'ouverture de session : seul moment où
    /// Snappy y touche.
    fn set_launch_at_login(&mut self, on: bool) {
        match login::set(on) {
            Ok(state) => {
                let applied = match &state {
                    login::State::On | login::State::NeedsApproval => on,
                    login::State::Off => !on,
                    // Décocher se note toujours (rien n'a pu être inscrit ici).
                    login::State::Unavailable(_) => !on,
                };
                self.login = state;
                if applied {
                    self.config.launch_at_login = on;
                    self.save();
                } else if let Some(text) = self.login.hint() {
                    self.say(format!("{} {text}", icons::WARNING));
                }
            }
            Err(e) => {
                self.login = login::state();
                self.say(format!("{} {e:#}", icons::WARNING));
            }
        }
    }

    fn capture_tab(&mut self, ui: &mut Ui) {
        // Les deux sections d'origine, telles quelles.
        self.capture_section(ui);
        ui.add_space(12.0);
        self.save_section(ui);
        ui.add_space(12.0);
        self.file_section(ui);
        ui.add_space(12.0);
        self.after_section(ui);
        ui.add_space(12.0);
        self.watermark_section(ui);
        ui.add_space(12.0);
        self.tools_section(ui);
    }

    fn file_section(&mut self, ui: &mut Ui) {
        section(ui, icons::IMAGE, "Fichiers", |ui| {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Format des images enregistrées");
                changed |= combo(
                    ui,
                    "image_format",
                    &mut self.config.image_format,
                    &ImageFormat::ALL,
                    ImageFormat::label,
                );
            });
            if self.config.image_format == ImageFormat::Jpeg {
                ui.horizontal(|ui| {
                    ui.label("Qualité JPEG");
                    let mut q = self.config.jpeg_quality();
                    if ui.add(egui::Slider::new(&mut q, 1..=100)).changed() {
                        self.config.jpeg_quality = q;
                        changed = true;
                    }
                });
            }
            hint(
                ui,
                "Pour « Enregistrer » (surcouche et éditeur). L'historique reste en PNG.",
            );
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.label("Nom des fichiers");
                let current = match self.config.file_name {
                    FileName::Date => 0,
                    FileName::Counter => 1,
                    FileName::Custom(_) => 2,
                };
                let names = ["Date (par défaut)", "Numérotation", "Personnalisé"];
                let mut choice = current;
                egui::ComboBox::from_id_salt("file_name")
                    .selected_text(names[current])
                    .show_ui(ui, |ui| {
                        for (i, name) in names.iter().enumerate() {
                            ui.selectable_value(&mut choice, i, *name);
                        }
                    });
                if choice != current {
                    self.config.file_name = match choice {
                        0 => FileName::Date,
                        1 => FileName::Counter,
                        _ if history::check_template(&self.template).is_ok() => {
                            FileName::Custom(self.template.clone())
                        }
                        _ => FileName::Custom("snappy-{date}".into()),
                    };
                    if let FileName::Custom(t) = &self.config.file_name {
                        self.template = t.clone();
                    }
                    changed = true;
                }
            });
            if matches!(self.config.file_name, FileName::Custom(_)) {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.template)
                        .hint_text("snappy-{date}")
                        .desired_width(f32::INFINITY),
                );
                match history::check_template(&self.template) {
                    Ok(()) => {
                        if edit.changed() {
                            self.config.file_name = FileName::Custom(self.template.clone());
                        }
                        if edit.lost_focus() {
                            changed = true;
                        }
                    }
                    Err(e) => {
                        ui.label(
                            RichText::new(format!("{} {e}", icons::WARNING))
                                .size(theme::SIZE_S)
                                .color(theme::WARNING),
                        );
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                    for (token, meaning) in history::TOKENS {
                        if ui.small_button(*token).on_hover_text(*meaning).clicked() {
                            self.template.push_str(token);
                            if history::check_template(&self.template).is_ok() {
                                self.config.file_name = FileName::Custom(self.template.clone());
                                changed = true;
                            }
                        }
                    }
                });
            }
            let example = match (
                &self.config.file_name,
                history::check_template(&self.template),
            ) {
                (FileName::Custom(_), Err(_)) => None,
                (FileName::Custom(_), Ok(())) => Some(history::example_name(&FileName::Custom(
                    self.template.clone(),
                ))),
                (rule, _) => Some(history::example_name(rule)),
            };
            if let Some(example) = example {
                hint(
                    ui,
                    &format!(
                        "Exemple : {example}.{} · pour les nouvelles captures",
                        self.config.image_format.extension()
                    ),
                );
            }
            if changed {
                self.save();
            }
        });
    }

    fn after_section(&mut self, ui: &mut Ui) {
        section(ui, icons::OK, "Après la capture", |ui| {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Copier dans le presse-papiers");
                changed |= combo(
                    ui,
                    "clipboard",
                    &mut self.config.clipboard,
                    &ClipboardContent::ALL,
                    ClipboardContent::label,
                );
            });
            hint(
                ui,
                match self.config.clipboard {
                    ClipboardContent::Snappy => {
                        "L'image après « Copier », l'écran entier et la zone précédente."
                    }
                    ClipboardContent::ImageAndPath => {
                        "Les éditeurs d'images collent l'image, le Finder colle le fichier."
                    }
                    ClipboardContent::Image => "L'image, après chaque capture.",
                    ClipboardContent::Path => "Le chemin du fichier, en texte.",
                    ClipboardContent::Nothing => {
                        "Rien, sauf avec le bouton « Copier » de la sélection."
                    }
                },
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Ensuite");
                changed |= combo(
                    ui,
                    "after_capture",
                    &mut self.config.after_capture,
                    &AfterCapture::ALL,
                    AfterCapture::label,
                );
            });
            hint(
                ui,
                match self.config.after_capture {
                    AfterCapture::Snappy => {
                        "L'éditeur après la capture de l'écran entier, rien après les autres. \
                         L'espace de travail ne s'ouvre pas tout seul."
                    }
                    AfterCapture::Workspace => "L'espace de travail s'ouvre après chaque capture.",
                    AfterCapture::Editor => "L'éditeur s'ouvre après chaque capture.",
                    AfterCapture::Notify => "Une notification indique le fichier enregistré.",
                    AfterCapture::Nothing => {
                        "Rien, même après la capture de l'écran entier (« Annoter » ouvre toujours \
                         l'éditeur)."
                    }
                },
            );
            ui.add_space(6.0);
            changed |= ui
                .checkbox(
                    &mut self.config.countdown_before_capture,
                    "Afficher un compte à rebours avant la capture",
                )
                .changed();
            changed |= ui
                .checkbox(
                    &mut self.config.capture_context,
                    "Garder le contexte de la capture (fenêtre, application)",
                )
                .changed();
            if changed {
                self.save();
            }
        });
    }

    fn watermark_section(&mut self, ui: &mut Ui) {
        section(ui, icons::STAMP, "Filigrane", |ui| {
            let mut changed = ui
                .checkbox(
                    &mut self.config.watermark.enabled,
                    "Ajouter un filigrane aux captures",
                )
                .changed();
            hint(
                ui,
                "Un texte ajouté comme annotation : il se modifie ou se supprime dans l'éditeur.",
            );
            ui.add_enabled_ui(self.config.watermark.enabled, |ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.watermark_text)
                        .hint_text("© Mon nom")
                        .desired_width(f32::INFINITY),
                );
                if edit.lost_focus() && self.watermark_text != self.config.watermark.text {
                    self.config.watermark.text = self.watermark_text.clone();
                    changed = true;
                }
                ui.horizontal(|ui| {
                    ui.label("Position");
                    changed |= combo(
                        ui,
                        "watermark_position",
                        &mut self.config.watermark.position,
                        &Corner::ALL,
                        Corner::label,
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Opacité");
                    let mut opacity = self.config.watermark.opacity.min(100);
                    if ui
                        .add(egui::Slider::new(&mut opacity, 10..=100).suffix(" %"))
                        .changed()
                    {
                        self.config.watermark.opacity = opacity;
                        changed = true;
                    }
                });
            });
            if changed {
                self.save();
            }
        });
    }

    fn tools_section(&mut self, ui: &mut Ui) {
        section(ui, icons::TEXT, "Texte et QR codes", |ui| {
            let mut changed = ui
                .checkbox(
                    &mut self.config.ocr_copy_without_window,
                    "OCR : copier le texte directement, sans fenêtre",
                )
                .changed();
            changed |= ui
                .checkbox(
                    &mut self.config.qr_open_browser,
                    "QR code : ouvrir les liens dans le navigateur",
                )
                .changed();
            if changed {
                self.save();
            }
        });
    }

    fn video_tab(&mut self, ui: &mut Ui) {
        section(ui, icons::SPOTLIGHT, "Vidéo", |ui| {
            hint(ui, "Utilisé par l'enregistrement vidéo.");
            ui.add_space(4.0);
            let video = &mut self.config.video;
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Qualité");
                changed |= combo(
                    ui,
                    "video_quality",
                    &mut video.quality,
                    &VideoQuality::ALL,
                    VideoQuality::label,
                );
            });
            ui.horizontal(|ui| {
                let mut stop = video.stop_after_minutes.is_some();
                if ui
                    .checkbox(&mut stop, "Arrêter l'enregistrement après")
                    .changed()
                {
                    video.stop_after_minutes = stop.then_some(10);
                    changed = true;
                }
                if let Some(minutes) = &mut video.stop_after_minutes {
                    changed |= ui
                        .add(egui::DragValue::new(minutes).range(1..=600).suffix(" min"))
                        .changed();
                }
            });
            changed |= ui
                .checkbox(
                    &mut video.countdown,
                    "Compte à rebours avant l'enregistrement",
                )
                .changed();
            ui.add_space(6.0);
            changed |= ui
                .checkbox(&mut video.mic, "Enregistrer le micro")
                .changed();
            let mic = video.mic;
            changed |= ui
                .checkbox(
                    &mut video.system_audio,
                    "Enregistrer le son de l'ordinateur",
                )
                .changed();
            changed |= ui
                .checkbox(&mut video.highlight_cursor, "Entourer le pointeur")
                .changed();
            changed |= ui
                .checkbox(&mut video.mark_clicks, "Marquer les clics")
                .changed();
            changed |= ui
                .checkbox(&mut video.webcam, "Incruster la webcam")
                .changed();
            changed |= ui
                .checkbox(
                    &mut video.show_preview_after,
                    "Ouvrir la vidéo après l'enregistrement",
                )
                .changed();
            ui.add_enabled_ui(mic, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Micro");
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.mic_device)
                            .hint_text("celui du système")
                            .desired_width(f32::INFINITY),
                    );
                    if edit.lost_focus() && self.mic_device != self.config.video.mic_device {
                        self.config.video.mic_device = self.mic_device.trim().to_owned();
                        changed = true;
                    }
                });
            });
            if changed {
                self.save();
            }
        });
    }

    fn advanced_tab(&mut self, ui: &mut Ui) {
        section(ui, icons::SETTINGS, "Avancé", |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(format!("{} Exporter les réglages…", icons::SAVE))
                    .clicked()
                {
                    self.export_settings();
                }
                if ui
                    .button(format!("{} Importer les réglages…", icons::OPEN))
                    .clicked()
                {
                    self.import_settings();
                }
            });
            hint(
                ui,
                "Un fichier JSON, pour une autre machine ou une sauvegarde.",
            );
            ui.add_space(6.0);
            if ui
                .button(format!("{} Restaurer les réglages par défaut", icons::UNDO))
                .clicked()
            {
                self.confirm = Some(Confirm::RestoreSettings);
            }
            ui.add_space(6.0);
            if ui
                .button(format!("{} Ouvrir le dossier des données", icons::FOLDER))
                .on_hover_text(self.store.root().display().to_string())
                .clicked()
            {
                history::open_dir(self.store.root());
            }
            hint(ui, &self.store.config_path().display().to_string());
        });
    }

    fn export_settings(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Exporter les réglages")
            .set_file_name("snappy-reglages.json")
            .add_filter("JSON", &["json"])
            .save_file()
        else {
            return;
        };
        let result = serde_json::to_string_pretty(&self.config)
            .map_err(anyhow::Error::from)
            .and_then(|text| crate::config::write_atomic(&path, text.as_bytes()));
        match result {
            Ok(()) => self.say(format!("{} Réglages exportés", icons::OK)),
            Err(e) => self.say(format!("{} {e:#}", icons::WARNING)),
        }
    }

    fn import_settings(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Importer les réglages")
            .add_filter("JSON", &["json"])
            .pick_file()
        else {
            return;
        };
        let parsed = std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|text| Ok(serde_json::from_str::<Config>(&text)?));
        match parsed {
            Ok(config) => {
                self.replace_config(config);
                self.say(format!("{} Réglages importés", icons::OK));
            }
            Err(e) => self.say(format!("{} Fichier illisible : {e:#}", icons::WARNING)),
        }
    }

    /// Nouvelle config entière (import, réglages par défaut). Le lancement à
    /// l'ouverture de session ne change pas : il ne bouge que par sa case.
    fn replace_config(&mut self, mut config: Config) {
        config.launch_at_login = self.config.launch_at_login;
        self.config = config;
        self.watermark_text = self.config.watermark.text.clone();
        self.mic_device = self.config.video.mic_device.clone();
        if let FileName::Custom(t) = &self.config.file_name {
            self.template = t.clone();
        }
        self.save();
    }

    /// Fenêtre de confirmation, si une action en attend une.
    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = self.confirm else { return };
        let (title, text, action) = match confirm {
            Confirm::RestoreHotkeys => (
                "Restaurer les raccourcis par défaut ?",
                "Les trois raccourcis d'origine reviennent ; les autres actions n'ont plus de \
                 raccourci.",
                "Restaurer",
            ),
            Confirm::RestoreSettings => (
                "Restaurer les réglages par défaut ?",
                "Tous les réglages reviennent à leur valeur d'origine, raccourcis compris. \
                 Les captures ne sont pas touchées.",
                "Restaurer",
            ),
        };
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::unique("snappy-confirmation")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.label(RichText::new(title).bold().size(theme::SIZE_L));
            ui.add_space(4.0);
            ui.label(text);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(action).clicked() {
                    answer = Some(true);
                }
                if ui.button("Annuler").clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                self.confirm = None;
                match confirm {
                    Confirm::RestoreHotkeys => {
                        self.config.restore_default_hotkeys();
                        self.save();
                    }
                    Confirm::RestoreSettings => self.replace_config(Config::default()),
                }
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    fn tab_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for tab in Tab::ALL {
                let selected = self.tab == tab;
                let text = RichText::new(tab.label()).size(theme::SIZE_M);
                let text = if selected {
                    text.color(theme::ACCENT_2).strong()
                } else {
                    text
                };
                if ui
                    .add_enabled(
                        self.recording.is_none() || selected,
                        egui::Button::selectable(selected, text),
                    )
                    .clicked()
                {
                    self.tab = tab;
                }
            }
        });
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
        self.draw(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Fermée pendant un enregistrement : les raccourcis reviennent tout de suite.
        if self.recording.take().is_some() {
            self.agent.reload_now();
        }
    }
}

impl Settings {
    /// Toute la fenêtre (une frame).
    fn draw(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.read_keyboard(&ctx);
        let agent = self.agent.status();
        egui::Panel::top("header")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(16, 12)),
            )
            .show(ui, |ui| {
                self.header(ui);
                ui.add_space(8.0);
                self.tab_bar(ui);
            });
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
                    .id_salt(self.tab.label())
                    .show(ui, |ui| match self.tab {
                        Tab::General => self.general_tab(ui),
                        Tab::Capture => self.capture_tab(ui),
                        Tab::Video => self.video_tab(ui),
                        Tab::Hotkeys => self.hotkeys_section(ui, &agent.refused),
                        Tab::Advanced => self.advanced_tab(ui),
                    });
            });
        self.confirm_dialog(&ctx);
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

/// Liste déroulante d'une valeur parmi `all` ; `true` si elle a changé.
fn combo<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: &str,
    value: &mut T,
    all: &[T],
    label: fn(T) -> &'static str,
) -> bool {
    let before = *value;
    egui::ComboBox::from_id_salt(id)
        .selected_text(label(*value))
        .show_ui(ui, |ui| {
            for &option in all {
                ui.selectable_value(value, option, label(option));
            }
        });
    *value != before
}

/// Texte d'aide, discret.
fn hint(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(theme::SIZE_S)
            .color(theme::TEXT_DIM),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chaque onglet et chaque confirmation se dessinent sans paniquer, sans rien
    /// changer à la config.
    #[test]
    fn every_tab_draws() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut settings = Settings::new(store.clone(), || {});
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut confirms = [
            None,
            Some(Confirm::RestoreHotkeys),
            Some(Confirm::RestoreSettings),
        ];
        for tab in Tab::ALL {
            for confirm in &mut confirms {
                settings.tab = tab;
                settings.confirm = *confirm;
                for _ in 0..2 {
                    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| settings.draw(ui));
                    out.textures_delta.clear();
                }
            }
        }
        assert_eq!(settings.config, Config::default());
        assert_eq!(store.load(), Config::default());
    }
}
