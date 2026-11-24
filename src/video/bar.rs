//! Barre vidéo (`snappy --video-bar`) : une petite fenêtre sans bord sous la zone,
//! et un cadre en pointillés autour de la zone (seconde fenêtre, transparente,
//! traversée par la souris). Aucune des deux n'apparaît dans la vidéo : filtre
//! ScreenCaptureKit sans les fenêtres de Snappy, et `NSWindow.sharingType = none`
//! (invisibles aussi pour les autres logiciels de capture).
//!
//! Avant le départ : Enregistrer, Micro, Son système, taille de la zone, réglages,
//! ✕. Pendant : Pause / Reprendre, Arrêter, Refaire, Micro coupé, chrono.

use std::path::PathBuf;
use std::process::Command as Process;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use egui::{
    Align, Color32, CornerRadius, FontId, Frame, Key, Layout, Margin, Pos2, Rect as ERect,
    RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, ViewportBuilder, ViewportCommand,
    ViewportId, WindowLevel, pos2, vec2,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSWindowSharingType};

use super::options::VideoOptions;
use super::region::{self, Display, LastVideoRegion, PRESETS, Rect};
use super::sck::{Command, Event, Params, Worker};
use super::session::Session;
use super::{PLAY_ARG, clock};
use crate::config::Store;
use crate::history;
use crate::platform;
use crate::ui::{icons, theme};

const BAR_TITLE: &str = "Snappy · Vidéo";
const FRAME_TITLE: &str = "Snappy · Cadre vidéo";
const BAR_SIZE: (f64, f64) = (760.0, 52.0);
/// Marge du cadre autour de la zone, en points.
const FRAME_MARGIN: f32 = 4.0;
/// Dossier des segments, dans le dossier de données.
const SEGMENT_DIR: &str = "recording-tmp";

pub fn run(store: Store, session: Session, rect: Rect, displays: Vec<Display>) -> Result<()> {
    let options = VideoOptions::load(&store);
    let show_preview = options.show_preview_after;
    let display = region::display_for(&rect, &displays).context("zone hors des écrans")?;
    let (x, y) = region::bar_position(&rect, &displays[display].frame, BAR_SIZE);
    let viewport = ViewportBuilder::default()
        .with_title(BAR_TITLE)
        .with_inner_size([BAR_SIZE.0 as f32, BAR_SIZE.1 as f32])
        .with_position([x as f32, y as f32])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_window_level(WindowLevel::AlwaysOnTop);
    let native = eframe::NativeOptions {
        viewport,
        persist_window: false,
        ..Default::default()
    };
    let saved: Arc<Mutex<Option<PathBuf>>> = Arc::default();
    let out = Arc::clone(&saved);
    eframe::run_native(
        "Snappy",
        native,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            platform::activate(cc);
            let ctx = cc.egui_ctx.clone();
            let worker = Worker::spawn(move || ctx.request_repaint())?;
            Ok(Box::new(Bar {
                store,
                session,
                options: options.clone(),
                microphone: options.microphone,
                system_audio: options.system_audio,
                mic_muted: false,
                rect,
                displays,
                display,
                worker,
                state: State::Ready,
                elapsed: Duration::ZERO,
                busy: false,
                after_discard: AfterDiscard::Nothing,
                error: None,
                saved: out,
                hidden_windows: 0,
                moved: false,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("barre vidéo : {e}"))?;

    let saved = saved.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(path) = saved {
        println!("{}", path.display());
        if show_preview {
            let exe = std::env::current_exe().context("chemin de l'exécutable")?;
            Process::new(exe)
                .arg(PLAY_ARG)
                .arg(&path)
                .spawn()
                .context("ouverture du lecteur")?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Ready,
    Countdown { until: Instant },
    Starting,
    Recording { since: Instant },
    Paused,
    Finishing,
    Closing,
}

/// Que faire quand l'enregistrement jeté est bien effacé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterDiscard {
    Nothing,
    Restart,
    Close,
}

struct Bar {
    store: Store,
    session: Session,
    options: VideoOptions,
    /// Choix de la barre pour cet enregistrement (défaut : la config).
    microphone: bool,
    system_audio: bool,
    mic_muted: bool,
    rect: Rect,
    displays: Vec<Display>,
    display: usize,
    worker: Worker,
    state: State,
    /// Temps enregistré avant la dernière reprise.
    elapsed: Duration,
    /// Ordre envoyé, réponse attendue : boutons inactifs.
    busy: bool,
    after_discard: AfterDiscard,
    error: Option<String>,
    saved: Arc<Mutex<Option<PathBuf>>>,
    /// Fenêtres déjà rendues invisibles aux captures.
    hidden_windows: usize,
    /// La zone a changé : replacer la barre.
    moved: bool,
}

impl Bar {
    fn recorded(&self) -> Duration {
        match self.state {
            State::Recording { since } => self.elapsed + since.elapsed(),
            _ => self.elapsed,
        }
    }

    fn is_recording(&self) -> bool {
        matches!(
            self.state,
            State::Starting | State::Recording { .. } | State::Paused
        )
    }

    fn request_start(&mut self) {
        self.error = None;
        if self.options.countdown > 0 {
            self.state = State::Countdown {
                until: Instant::now() + Duration::from_secs(u64::from(self.options.countdown)),
            };
        } else {
            self.start_now();
        }
    }

    fn start_now(&mut self) {
        let display = self.displays[self.display];
        let Some(rect) = region::fit(&self.rect, &display) else {
            self.error = Some("zone trop petite".into());
            self.state = State::Ready;
            return;
        };
        region::save_last(&self.store, &LastVideoRegion { display, rect });
        let captures = self.store.captures_dir();
        let output = history::free_path(&captures, &history::new_name(), "mp4");
        let data = self.store.config_path();
        let segment_dir = data
            .parent()
            .map(|d| d.join(SEGMENT_DIR))
            .unwrap_or_else(|| PathBuf::from(SEGMENT_DIR));
        let params = Params {
            display_id: display.id,
            rect,
            size: self
                .options
                .quality
                .output_size(rect.width, rect.height, display.scale),
            fps: self.options.fps,
            system_audio: self.system_audio,
            microphone: self.microphone,
            mark_clicks: self.options.mark_clicks,
            segment_dir,
        };
        log::info!("enregistrement : {params:?} → {}", output.display());
        self.mic_muted = false;
        self.elapsed = Duration::ZERO;
        self.state = State::Starting;
        self.busy = true;
        self.worker.send(Command::Start(params, output));
    }

    fn stop(&mut self) {
        if matches!(self.state, State::Recording { .. } | State::Paused) {
            self.busy = true;
            self.worker.send(Command::Stop);
        }
    }

    fn toggle_pause(&mut self) {
        match self.state {
            State::Recording { .. } => {
                self.busy = true;
                self.worker.send(Command::Pause);
            }
            State::Paused => {
                self.busy = true;
                self.worker.send(Command::Resume);
            }
            _ => {}
        }
    }

    fn redo(&mut self) {
        if self.is_recording() {
            self.busy = true;
            self.after_discard = AfterDiscard::Restart;
            self.worker.send(Command::Discard);
        }
    }

    fn abandon(&mut self, ctx: &egui::Context) {
        if self.is_recording() {
            self.busy = true;
            self.after_discard = AfterDiscard::Close;
            self.worker.send(Command::Discard);
        } else {
            self.close(ctx);
        }
    }

    fn close(&mut self, ctx: &egui::Context) {
        self.state = State::Closing;
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
    }

    /// Réponses du fil d'enregistrement.
    fn poll_worker(&mut self, ctx: &egui::Context) {
        while let Some(event) = self.worker.try_recv() {
            log::debug!("vidéo : {event:?}");
            match event {
                Event::Recording | Event::Resumed => {
                    self.busy = false;
                    self.state = State::Recording {
                        since: Instant::now(),
                    };
                }
                Event::Paused => {
                    self.busy = false;
                    self.elapsed = self.recorded();
                    self.state = State::Paused;
                }
                Event::Microphone(on) => {
                    self.busy = false;
                    self.mic_muted = !on;
                }
                Event::Finishing => {
                    self.elapsed = self.recorded();
                    self.state = State::Finishing;
                }
                Event::Saved(path) => {
                    *self.saved.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
                    self.close(ctx);
                }
                Event::Discarded => {
                    self.busy = false;
                    self.elapsed = Duration::ZERO;
                    self.state = State::Ready;
                    match std::mem::replace(&mut self.after_discard, AfterDiscard::Nothing) {
                        AfterDiscard::Restart => self.request_start(),
                        AfterDiscard::Close => self.close(ctx),
                        AfterDiscard::Nothing => {}
                    }
                }
                Event::Failed(message) => {
                    self.busy = false;
                    self.error = Some(message);
                    if matches!(self.state, State::Starting | State::Finishing) {
                        self.state = State::Ready;
                    }
                }
            }
        }
    }

    /// Minuteries : compte à rebours, arrêt automatique, demande extérieure.
    fn tick(&mut self) {
        if let State::Countdown { until } = self.state
            && Instant::now() >= until
        {
            self.start_now();
        }
        let limit = self.options.stop_after_minutes;
        if limit > 0
            && !self.busy
            && matches!(self.state, State::Recording { .. })
            && self.recorded() >= Duration::from_secs(u64::from(limit) * 60)
        {
            log::info!("arrêt automatique après {limit} min");
            self.stop();
        }
        if self.session.take_request() {
            match self.state {
                State::Ready => self.request_start(),
                State::Recording { .. } | State::Paused => self.stop(),
                _ => {}
            }
        }
    }

    fn set_rect(&mut self, rect: Rect, display: usize) {
        self.rect = rect;
        self.display = display;
        self.moved = true;
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        if escape {
            match self.state {
                State::Countdown { .. } => self.state = State::Ready,
                State::Ready => self.close(ctx),
                _ => {}
            }
        }
    }

    fn bar_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal_centered(|ui| {
            grip(ui, &ctx);
            match self.state {
                State::Ready => self.ready_ui(ui),
                State::Countdown { until } => {
                    let left = until
                        .saturating_duration_since(Instant::now())
                        .as_secs_f32();
                    ui.label(
                        RichText::new(format!("Départ dans {}…", left.ceil() as u32))
                            .font(theme::strong(theme::SIZE_L)),
                    );
                    if ui.button("Annuler").clicked() {
                        self.state = State::Ready;
                    }
                }
                State::Starting => {
                    ui.spinner();
                    ui.label("Démarrage…");
                }
                State::Recording { .. } | State::Paused => self.recording_ui(ui),
                State::Finishing | State::Closing => {
                    ui.spinner();
                    ui.label("Finalisation de la vidéo…");
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !matches!(self.state, State::Finishing | State::Closing),
                        egui::Button::new(RichText::new(icons::CLOSE).size(theme::SIZE_L)),
                    )
                    .on_hover_text("Abandonner la vidéo")
                    .clicked()
                {
                    self.abandon(&ctx);
                }
                let time = clock(self.recorded());
                let color = match self.state {
                    State::Recording { .. } => theme::DANGER,
                    State::Paused => theme::WARNING,
                    _ => theme::TEXT_DIM,
                };
                ui.label(
                    RichText::new(time)
                        .font(theme::strong(theme::SIZE_L))
                        .color(color),
                );
                if matches!(self.state, State::Recording { .. }) {
                    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 5.0, theme::DANGER);
                }
                if let Some(e) = &self.error {
                    ui.label(RichText::new(icons::WARNING).color(theme::WARNING))
                        .on_hover_text(e.as_str());
                }
            });
        });
    }

    fn ready_ui(&mut self, ui: &mut Ui) {
        if glyph_button(ui, Glyph::Record, "Enregistrer", true, true).clicked() {
            self.request_start();
        }
        ui.separator();
        ui.checkbox(&mut self.microphone, "Micro")
            .on_hover_text("Enregistrer le micro (autorisation « Microphone »)");
        ui.checkbox(&mut self.system_audio, "Son système")
            .on_hover_text("Enregistrer le son de l'ordinateur");
        ui.separator();
        self.size_menu(ui);
        if ui
            .button(RichText::new(icons::SETTINGS).size(theme::SIZE_L))
            .on_hover_text("Réglages")
            .clicked()
            && let Ok(exe) = std::env::current_exe()
        {
            let _ = Process::new(exe).arg("--settings").spawn();
        }
    }

    fn size_menu(&mut self, ui: &mut Ui) {
        let display = self.displays[self.display];
        let (w, h) = (
            (self.rect.width * display.scale).round() as u32,
            (self.rect.height * display.scale).round() as u32,
        );
        let mut choice: Option<(Rect, usize)> = None;
        egui::ComboBox::from_id_salt("taille")
            .selected_text(format!("{w} × {h}"))
            .width(150.0)
            .show_ui(ui, |ui| {
                for p in PRESETS {
                    let fitted = region::apply_preset(&self.rect, &display, p.width, p.height);
                    let label = format!("{} × {} {}", p.width, p.height, p.label);
                    let r = ui.add_enabled(
                        fitted.is_some(),
                        egui::Button::selectable(w == p.width && h == p.height, label),
                    );
                    if r.clicked()
                        && let Some(f) = fitted
                    {
                        choice = Some((f, self.display));
                    }
                }
                ui.separator();
                for (i, d) in self.displays.iter().enumerate() {
                    let label = format!(
                        "Plein écran {} ({} × {})",
                        i + 1,
                        (d.frame.width * d.scale).round(),
                        (d.frame.height * d.scale).round()
                    );
                    if ui
                        .add(egui::Button::selectable(
                            i == self.display && self.rect == d.frame,
                            label,
                        ))
                        .clicked()
                    {
                        choice = Some((d.frame, i));
                    }
                }
            });
        if let Some((rect, display)) = choice {
            self.set_rect(rect, display);
        }
    }

    fn recording_ui(&mut self, ui: &mut Ui) {
        let enabled = !self.busy;
        let paused = self.state == State::Paused;
        let (glyph, label) = if paused {
            (Glyph::Play, "Reprendre")
        } else {
            (Glyph::Pause, "Pause")
        };
        if glyph_button(ui, glyph, label, enabled, false).clicked() {
            self.toggle_pause();
        }
        if glyph_button(ui, Glyph::Stop, "Arrêter", enabled, false).clicked() {
            self.stop();
        }
        if ui
            .add_enabled(
                enabled,
                egui::Button::new(format!("{} Refaire", icons::UNDO)),
            )
            .on_hover_text("Jeter cette vidéo et recommencer")
            .clicked()
        {
            self.redo();
        }
        if self.microphone {
            let mut muted = self.mic_muted;
            if ui
                .add_enabled(enabled, egui::Checkbox::new(&mut muted, "Micro coupé"))
                .changed()
            {
                self.busy = true;
                self.worker.send(Command::Microphone(!muted));
            }
        }
        if paused {
            ui.label(RichText::new("En pause").color(theme::WARNING));
        }
    }

    /// Cadre en pointillés autour de la zone (fenêtre transparente, sans clic).
    fn frame_viewport(&self, ctx: &egui::Context) {
        let r = &self.rect;
        let m = FRAME_MARGIN;
        let builder = ViewportBuilder::default()
            .with_title(FRAME_TITLE)
            .with_position([r.x as f32 - m, r.y as f32 - m])
            .with_inner_size([r.width as f32 + 2.0 * m, r.height as f32 + 2.0 * m])
            .with_decorations(false)
            .with_transparent(true)
            .with_mouse_passthrough(true)
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        let color = match self.state {
            State::Recording { .. } => theme::DANGER,
            State::Paused => theme::WARNING,
            _ => Color32::WHITE,
        };
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("snappy-video-frame"),
            builder,
            |ui, _class| {
                egui::CentralPanel::default()
                    .frame(Frame::NONE)
                    .show(ui, |ui| {
                        let full = ui.max_rect();
                        let rect = full.shrink(m / 2.0);
                        dashed_rect(
                            ui.painter(),
                            rect.expand(0.5),
                            Color32::from_black_alpha(160),
                        );
                        dashed_rect(ui.painter(), rect, color);
                    });
            },
        );
    }
}

impl eframe::App for Bar {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.hidden_windows = hide_from_capture(self.hidden_windows);
        self.poll_worker(&ctx);
        if self.state == State::Closing {
            return;
        }
        self.tick();
        self.keyboard(&ctx);
        if std::mem::take(&mut self.moved) {
            let frame = self.displays[self.display].frame;
            let (x, y) = region::bar_position(&self.rect, &frame, BAR_SIZE);
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(x as f32, y as f32)));
        }
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::symmetric(10, 6)),
            )
            .show(ui, |ui| self.bar_ui(ui));
        self.frame_viewport(&ctx);
        // Chrono et demandes extérieures (`recording.stop`).
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Fermée par le système pendant un enregistrement : on garde ce qui est fait.
        if matches!(self.state, State::Recording { .. } | State::Paused) {
            self.worker.send(Command::Stop);
            for _ in 0..100 {
                std::thread::sleep(Duration::from_millis(50));
                match self.worker.try_recv() {
                    Some(Event::Saved(path)) => {
                        *self.saved.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
                        break;
                    }
                    Some(Event::Failed(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

/// Poignée de déplacement de la barre (fenêtre sans bord).
fn grip(ui: &mut Ui, ctx: &egui::Context) {
    let (r, response) = ui.allocate_exact_size(vec2(12.0, 28.0), Sense::drag());
    for row in 0..3 {
        for col in 0..2 {
            let c = pos2(
                r.min.x + 3.0 + col as f32 * 6.0,
                r.center().y - 6.0 + row as f32 * 6.0,
            );
            ui.painter().circle_filled(c, 1.5, theme::TEXT_DIM);
        }
    }
    if response.drag_started() {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    response.on_hover_cursor(egui::CursorIcon::Grab);
}

#[derive(Debug, Clone, Copy)]
enum Glyph {
    Record,
    Pause,
    Play,
    Stop,
}

/// Bouton avec un symbole dessiné (rond, pause, lecture, carré) et un texte.
fn glyph_button(ui: &mut Ui, glyph: Glyph, text: &str, enabled: bool, big: bool) -> egui::Response {
    let font = if big {
        theme::strong(theme::SIZE_M)
    } else {
        FontId::proportional(theme::SIZE_M)
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let icon = 14.0;
    let pad = vec2(10.0, 6.0);
    let size = vec2(pad.x * 2.0 + icon + 6.0 + galley.size().x, 30.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let visuals = ui.style().interact(&response);
    let fill = if big {
        if response.hovered() && enabled {
            Color32::from_rgb(0xe0, 0x2e, 0x2e)
        } else {
            Color32::from_rgb(0xc4, 0x22, 0x22)
        }
    } else {
        visuals.bg_fill
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(15),
        fill,
        visuals.bg_stroke,
        StrokeKind::Inside,
    );
    let alpha = if enabled { 1.0 } else { 0.4 };
    let fg = if big { Color32::WHITE } else { theme::TEXT }.gamma_multiply(alpha);
    let c = pos2(rect.min.x + pad.x + icon / 2.0, rect.center().y);
    paint_glyph(ui.painter(), glyph, c, icon, fg, big);
    let text_pos = pos2(
        c.x + icon / 2.0 + 6.0,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(text_pos, galley, fg);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn paint_glyph(p: &egui::Painter, glyph: Glyph, c: Pos2, size: f32, color: Color32, big: bool) {
    let h = size / 2.0;
    match glyph {
        Glyph::Record => {
            let dot = if big { Color32::WHITE } else { theme::DANGER };
            p.circle_filled(c, h * 0.8, dot);
        }
        Glyph::Pause => {
            for dx in [-h * 0.45, h * 0.45] {
                p.rect_filled(
                    ERect::from_center_size(c + vec2(dx, 0.0), vec2(h * 0.45, h * 1.6)),
                    CornerRadius::same(1),
                    color,
                );
            }
        }
        Glyph::Play => {
            p.add(Shape::convex_polygon(
                vec![
                    c + vec2(-h * 0.6, -h * 0.8),
                    c + vec2(h * 0.8, 0.0),
                    c + vec2(-h * 0.6, h * 0.8),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Glyph::Stop => {
            p.rect_filled(
                ERect::from_center_size(c, Vec2::splat(h * 1.4)),
                CornerRadius::same(2),
                theme::DANGER,
            );
        }
    }
}

/// Rectangle en pointillés.
fn dashed_rect(p: &egui::Painter, r: ERect, color: Color32) {
    let points = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    p.extend(Shape::dashed_line(
        &points,
        Stroke::new(2.0, color),
        8.0,
        5.0,
    ));
}

/// Rend toutes les fenêtres de l'app invisibles aux captures d'écran (la barre, le
/// cadre), au fur et à mesure qu'elles apparaissent. Rend leur nombre.
fn hide_from_capture(done: usize) -> usize {
    let Some(mtm) = MainThreadMarker::new() else {
        return done;
    };
    let windows = NSApplication::sharedApplication(mtm).windows();
    let count = windows.count();
    if count != done {
        for window in windows.iter() {
            window.setSharingType(NSWindowSharingType::None);
        }
    }
    count
}
