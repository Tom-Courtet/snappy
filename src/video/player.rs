//! Lecteur vidéo (`snappy --play <mp4>`) : aperçu image par image (AVFoundation),
//! frise avec deux poignées de découpe et la tête de lecture, vitesse, muet, et
//! exports. Chaque export écrit un **nouveau** fichier ; l'original n'est jamais
//! modifié.
//!
//! L'aperçu n'a pas de son : pour regarder la vidéo avec le son, « Ouvrir dans
//! QuickTime » (un `AVPlayerView` dans une fenêtre egui demanderait une vue native
//! incrustée, hors de portée pour l'instant).

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result};
use egui::{
    Align2, Color32, CornerRadius, FontId, Frame, Key, Margin, Rect as ERect, RichText, Sense,
    Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, pos2, vec2,
};
use image::RgbaImage;

use super::av::{self, Edit, Frames, Info};
use super::gif_export::{self, FPS_CHOICES};
use super::timecode;
use crate::config::Store;
use crate::history;
use crate::platform;
use crate::ui::{icons, theme};

/// Vitesses du lecteur de Screenpresso (§ 13.5).
const SPEEDS: &[f64] = &[0.5, 1.0, 1.25, 1.5, 1.75, 2.0, 4.0, 8.0];
/// Largeurs maximales proposées pour le GIF (0 : taille d'origine).
const GIF_WIDTHS: &[u32] = &[480, 640, 800, 1024, 0];
/// Largeur maximale des images de l'aperçu.
const PREVIEW_WIDTH: u32 = 1600;
/// Plus court extrait gardé, en secondes.
const MIN_CLIP: f64 = 0.1;

pub fn run(store: Store, path: PathBuf) -> Result<()> {
    let info = av::info(&path).with_context(|| format!("lecture de {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Fenêtre à la taille de la vidéo, dans les limites d'un écran portable.
    let aspect = if info.height > 0 {
        info.width as f32 / info.height as f32
    } else {
        16.0 / 9.0
    };
    let w = (info.width as f32 / 2.0).clamp(640.0, 1100.0);
    let h = (w / aspect).clamp(240.0, 640.0) + 190.0;
    let viewport = egui::ViewportBuilder::default()
        .with_title(format!("Snappy · Lecteur vidéo – {name}"))
        .with_inner_size([w, h])
        .with_min_inner_size([640.0, 420.0])
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
            let frames = FrameWorker::spawn(path.clone(), move || ctx.request_repaint())?;
            Ok(Box::new(Player::new(store, path, info, frames)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("lecteur vidéo : {e}"))
}

/// Icône de la fenêtre (Dock), comme les réglages.
fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!("../../assets/icon_mac_256.rgba").to_vec(),
        width: 256,
        height: 256,
    }
}

/// Fil qui décode les images de l'aperçu : seule la dernière demande compte.
struct FrameWorker {
    tx: Sender<f64>,
    rx: Receiver<(f64, Result<RgbaImage, String>)>,
}

impl FrameWorker {
    fn spawn(path: PathBuf, repaint: impl Fn() + Send + 'static) -> Result<Self> {
        let (tx, requests) = mpsc::channel::<f64>();
        let (answers, rx) = mpsc::channel();
        thread::Builder::new()
            .name("snappy-frames".into())
            .spawn(move || {
                let frames = match Frames::open(&path, PREVIEW_WIDTH, true) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = answers.send((0.0, Err(format!("{e:#}"))));
                        repaint();
                        return;
                    }
                };
                while let Ok(mut at) = requests.recv() {
                    // Plusieurs demandes en attente : seule la plus récente.
                    while let Ok(newer) = requests.try_recv() {
                        at = newer;
                    }
                    let image = frames.at(at).map_err(|e| format!("{e:#}"));
                    if answers.send((at, image)).is_err() {
                        break;
                    }
                    repaint();
                }
            })
            .context("fil de l'aperçu")?;
        Ok(Self { tx, rx })
    }
}

/// Export en cours.
struct Job {
    label: &'static str,
    kind: JobKind,
    rx: Receiver<JobMsg>,
    progress: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Video,
    Gif,
    Image,
}

enum JobMsg {
    Progress(f32),
    Done(Result<PathBuf, String>),
}

/// Élément de la frise qu'on fait glisser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grab {
    Start,
    End,
    Head,
}

struct Player {
    store: Store,
    path: PathBuf,
    info: Info,
    frames: FrameWorker,
    texture: Option<TextureHandle>,
    /// Instant de l'image affichée, et de la dernière demandée.
    shown: Option<f64>,
    requested: Option<f64>,
    pos: f64,
    trim: (f64, f64),
    speed: f64,
    mute: bool,
    /// Lecture : instant de départ et position à ce moment.
    playing: Option<(Instant, f64)>,
    grab: Option<Grab>,
    gif_fps: u32,
    gif_width: u32,
    job: Option<Job>,
    /// Dernier message, et le fichier produit s'il y en a un.
    status: Option<(String, Option<(JobKind, PathBuf)>)>,
}

impl Player {
    fn new(store: Store, path: PathBuf, info: Info, frames: FrameWorker) -> Self {
        let duration = info.duration.max(0.0);
        Self {
            store,
            path,
            frames,
            texture: None,
            shown: None,
            requested: None,
            pos: 0.0,
            trim: (0.0, duration),
            speed: 1.0,
            mute: false,
            playing: None,
            grab: None,
            gif_fps: 10,
            gif_width: 800,
            job: None,
            status: None,
            info,
        }
    }

    fn duration(&self) -> f64 {
        self.info.duration.max(0.0)
    }

    /// Pas d'une image (flèches gauche et droite).
    fn frame_step(&self) -> f64 {
        if self.info.fps > 1.0 {
            1.0 / f64::from(self.info.fps)
        } else {
            1.0 / 30.0
        }
    }

    fn seek(&mut self, at: f64) {
        self.pos = at.clamp(0.0, self.duration());
        if let Some((_, _)) = self.playing {
            self.playing = Some((Instant::now(), self.pos));
        }
    }

    fn toggle_play(&mut self) {
        if self.playing.take().is_none() {
            // Au bout de l'extrait : on repart de son début.
            if self.pos >= self.trim.1 - self.frame_step() || self.pos < self.trim.0 {
                self.pos = self.trim.0;
            }
            self.playing = Some((Instant::now(), self.pos));
        }
    }

    fn update_playback(&mut self) {
        if let Some((since, from)) = self.playing {
            let at = from + since.elapsed().as_secs_f64() * self.speed;
            if at >= self.trim.1 {
                self.pos = self.trim.1;
                self.playing = None;
            } else {
                self.pos = at;
            }
        }
    }

    /// Demande l'image de la position courante si elle n'est pas déjà affichée.
    fn update_frame(&mut self, ctx: &egui::Context) {
        while let Ok((at, image)) = self.frames.rx.try_recv() {
            match image {
                Ok(img) => {
                    let color = egui::ColorImage::from_rgba_unmultiplied(
                        [img.width() as usize, img.height() as usize],
                        img.as_raw(),
                    );
                    match &mut self.texture {
                        Some(t) => t.set(color, TextureOptions::LINEAR),
                        None => {
                            self.texture =
                                Some(ctx.load_texture("image", color, TextureOptions::LINEAR));
                        }
                    }
                    self.shown = Some(at);
                }
                Err(e) => self.status = Some((format!("Aperçu : {e}"), None)),
            }
        }
        let near = |t: Option<f64>| t.is_some_and(|t| (t - self.pos).abs() < 1e-3);
        if !near(self.requested) {
            // Lecture : on attend l'image précédente avant d'en demander une autre.
            let waiting = self.playing.is_some() && self.requested != self.shown;
            if !waiting {
                let _ = self.frames.tx.send(self.pos);
                self.requested = Some(self.pos);
            }
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        let (space, left, right, home, end) = ctx.input(|i| {
            (
                i.key_pressed(Key::Space),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
            )
        });
        if space {
            self.toggle_play();
        }
        if left {
            self.seek(self.pos - self.frame_step());
        }
        if right {
            self.seek(self.pos + self.frame_step());
        }
        if home {
            self.seek(self.trim.0);
        }
        if end {
            self.seek(self.trim.1);
        }
    }

    fn edit(&self) -> Edit {
        Edit {
            start: self.trim.0,
            end: self.trim.1,
            speed: self.speed,
            mute: self.mute,
        }
    }

    fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "video".into())
    }

    fn out_dir(&self) -> PathBuf {
        self.path
            .parent()
            .map(Path::to_owned)
            .unwrap_or_else(|| self.store.captures_dir())
    }

    /// Lance un export dans un fil ; `work` reçoit de quoi signaler l'avancement.
    fn start_job(
        &mut self,
        ctx: &egui::Context,
        label: &'static str,
        kind: JobKind,
        work: impl FnOnce(&dyn Fn(f32)) -> Result<PathBuf> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        let spawned = thread::Builder::new()
            .name("snappy-export".into())
            .spawn(move || {
                let progress_tx = tx.clone();
                let progress_ctx = repaint.clone();
                let progress = move |p: f32| {
                    let _ = progress_tx.send(JobMsg::Progress(p));
                    progress_ctx.request_repaint();
                };
                let result = work(&progress).map_err(|e| format!("{e:#}"));
                let _ = tx.send(JobMsg::Done(result));
                repaint.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.status = None;
                self.job = Some(Job {
                    label,
                    kind,
                    rx,
                    progress: 0.0,
                });
            }
            Err(e) => self.status = Some((format!("Export impossible : {e}"), None)),
        }
    }

    fn poll_job(&mut self) {
        let Some(job) = &mut self.job else { return };
        while let Ok(msg) = job.rx.try_recv() {
            match msg {
                JobMsg::Progress(p) => job.progress = p.clamp(0.0, 1.0),
                JobMsg::Done(result) => {
                    let kind = job.kind;
                    self.status = Some(match result {
                        Ok(path) => (
                            format!("{} Enregistré : {}", icons::OK, file_name(&path)),
                            Some((kind, path)),
                        ),
                        Err(e) => (format!("{} {e}", icons::WARNING), None),
                    });
                    self.job = None;
                    return;
                }
            }
        }
    }

    fn export_mp4(&mut self, ctx: &egui::Context) {
        let src = self.path.clone();
        let dst = history::free_path(&self.out_dir(), &format!("{}-montage", self.stem()), "mp4");
        let edit = self.edit();
        self.start_job(ctx, "Export MP4", JobKind::Video, move |progress| {
            av::export(&src, &dst, edit, &mut |p| progress(p))?;
            Ok(dst)
        });
    }

    fn export_gif(&mut self, ctx: &egui::Context) {
        let src = self.path.clone();
        let dst = history::free_path(&self.out_dir(), &self.stem(), "gif");
        let (start, end, speed) = (self.trim.0, self.trim.1, self.speed);
        let (fps, width) = (self.gif_fps, self.gif_width);
        self.start_job(ctx, "Export GIF", JobKind::Gif, move |progress| {
            let frames = Frames::open(&src, width, true)?;
            let times = gif_export::frame_times(start, end, fps, speed);
            let total = times.len().max(1) as f32;
            gif_export::write_gif(&dst, fps, times.into_iter().map(|t| frames.at(t)), |n| {
                progress(n as f32 / total)
            })?;
            Ok(dst)
        });
    }

    fn export_image(&mut self, ctx: &egui::Context) {
        let src = self.path.clone();
        let captures = self.store.captures_dir();
        let at = self.pos.min((self.duration() - self.frame_step()).max(0.0));
        self.start_job(ctx, "Export de l'image", JobKind::Image, move |_| {
            let image = Frames::open(&src, 0, true)?.at(at)?;
            history::save_capture(&captures, &image)
        });
    }

    fn preview(&mut self, ui: &mut Ui) {
        let avail = ui.available_rect_before_wrap();
        ui.allocate_rect(avail, Sense::click())
            .clicked()
            .then(|| self.toggle_play());
        ui.painter()
            .rect_filled(avail, CornerRadius::same(8), Color32::from_rgb(8, 8, 10));
        if let Some(texture) = &self.texture {
            let size = texture.size_vec2();
            // Ajustée à la place disponible, proportions gardées.
            let scale = (avail.width() / size.x)
                .min(avail.height() / size.y)
                .max(0.01);
            let rect = ERect::from_center_size(avail.center(), size * scale);
            ui.painter().image(
                texture.id(),
                rect,
                ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            ui.painter().text(
                avail.center(),
                Align2::CENTER_CENTER,
                "Chargement…",
                FontId::proportional(theme::SIZE_M),
                theme::TEXT_DIM,
            );
        }
    }

    /// Frise : extrait gardé, poignées de découpe, tête de lecture.
    fn timeline(&mut self, ui: &mut Ui) {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(vec2(width, 36.0), Sense::click_and_drag());
        let duration = self.duration().max(1e-6);
        let inner = rect.shrink2(vec2(8.0, 0.0));
        let x_of = |t: f64| inner.left() + (t / duration) as f32 * inner.width();
        let t_of =
            |x: f32| (((x - inner.left()) / inner.width()) as f64 * duration).clamp(0.0, duration);
        let p = ui.painter();
        let track = ERect::from_min_max(
            pos2(inner.left(), rect.center().y - 6.0),
            pos2(inner.right(), rect.center().y + 6.0),
        );
        p.rect_filled(track, CornerRadius::same(4), theme::SURFACE_HI);
        let kept = ERect::from_min_max(
            pos2(x_of(self.trim.0), track.top()),
            pos2(x_of(self.trim.1), track.bottom()),
        );
        p.rect_filled(
            kept,
            CornerRadius::same(4),
            theme::ACCENT.gamma_multiply(0.7),
        );
        for (t, grab) in [(self.trim.0, Grab::Start), (self.trim.1, Grab::End)] {
            let x = x_of(t);
            let handle = ERect::from_center_size(pos2(x, rect.center().y), vec2(8.0, 30.0));
            let active = self.grab == Some(grab);
            p.rect(
                handle,
                CornerRadius::same(3),
                if active { theme::ACCENT_2 } else { theme::TEXT },
                Stroke::new(1.0, theme::BG),
                StrokeKind::Inside,
            );
        }
        let head = x_of(self.pos);
        p.line_segment(
            [pos2(head, rect.top()), pos2(head, rect.bottom())],
            Stroke::new(2.0, theme::DANGER),
        );
        p.circle_filled(pos2(head, rect.top() + 3.0), 4.0, theme::DANGER);

        if let Some(at) = response.interact_pointer_pos() {
            if response.drag_started() || response.clicked() || self.grab.is_none() {
                let near = |t: f64| (x_of(t) - at.x).abs() <= 8.0;
                self.grab = Some(if near(self.trim.0) && !near(self.pos) {
                    Grab::Start
                } else if near(self.trim.1) && !near(self.pos) {
                    Grab::End
                } else {
                    Grab::Head
                });
            }
            let t = t_of(at.x);
            match self.grab {
                Some(Grab::Start) => {
                    self.trim.0 = t.min(self.trim.1 - MIN_CLIP).max(0.0);
                    self.seek(self.trim.0);
                }
                Some(Grab::End) => {
                    self.trim.1 = t.max(self.trim.0 + MIN_CLIP).min(duration);
                    self.seek(self.trim.1);
                }
                _ => self.seek(t),
            }
        }
        if !response.dragged() && !response.is_pointer_button_down_on() {
            self.grab = None;
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }

    fn controls(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            let play = if self.playing.is_some() {
                "Pause"
            } else {
                "Lecture"
            };
            if ui
                .button(play)
                .on_hover_text("Aperçu sans le son (Espace)")
                .clicked()
            {
                self.toggle_play();
            }
            ui.label(
                RichText::new(format!(
                    "{} / {}",
                    timecode(self.pos),
                    timecode(self.duration())
                ))
                .font(theme::strong(theme::SIZE_M)),
            );
            ui.separator();
            if ui
                .button("Début ici")
                .on_hover_text("Couper ce qui précède la tête de lecture")
                .clicked()
            {
                self.trim.0 = self.pos.min(self.trim.1 - MIN_CLIP).max(0.0);
            }
            if ui
                .button("Fin ici")
                .on_hover_text("Couper ce qui suit la tête de lecture")
                .clicked()
            {
                self.trim.1 = self.pos.max(self.trim.0 + MIN_CLIP).min(self.duration());
            }
            let removed = self.duration() - (self.trim.1 - self.trim.0);
            if removed > 0.005 {
                ui.label(
                    RichText::new(format!("{} retirés", timecode(removed))).color(theme::TEXT_DIM),
                );
                if ui.small_button("Tout garder").clicked() {
                    self.trim = (0.0, self.duration());
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Vitesse");
            egui::ComboBox::from_id_salt("vitesse")
                .selected_text(speed_label(self.speed))
                .width(70.0)
                .show_ui(ui, |ui| {
                    for &s in SPEEDS {
                        ui.selectable_value(&mut self.speed, s, speed_label(s));
                    }
                });
            ui.add_enabled(
                self.info.has_audio,
                egui::Checkbox::new(&mut self.mute, "Muet"),
            )
            .on_disabled_hover_text("Cette vidéo n'a pas de son");
            ui.separator();
            ui.label("GIF");
            egui::ComboBox::from_id_salt("gif-fps")
                .selected_text(format!("{} i/s", self.gif_fps))
                .width(70.0)
                .show_ui(ui, |ui| {
                    for &f in FPS_CHOICES {
                        ui.selectable_value(&mut self.gif_fps, f, format!("{f} i/s"));
                    }
                });
            egui::ComboBox::from_id_salt("gif-largeur")
                .selected_text(width_label(self.gif_width))
                .width(110.0)
                .show_ui(ui, |ui| {
                    for &w in GIF_WIDTHS {
                        ui.selectable_value(&mut self.gif_width, w, width_label(w));
                    }
                });
        });
        ui.horizontal(|ui| {
            let idle = self.job.is_none();
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("{} Exporter l'image", icons::IMAGE)),
                )
                .on_hover_text("L'image affichée, en PNG dans l'historique")
                .clicked()
            {
                self.export_image(&ctx);
            }
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("{} Enregistrer (MP4)", icons::SAVE)),
                )
                .on_hover_text("Nouveau fichier : extrait, vitesse et son choisis")
                .clicked()
            {
                self.export_mp4(&ctx);
            }
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("{} Enregistrer (GIF)", icons::SAVE)),
                )
                .clicked()
            {
                self.export_gif(&ctx);
            }
            ui.separator();
            if ui
                .button(format!("{} Ouvrir dans QuickTime", icons::OPEN))
                .on_hover_text("Regarder la vidéo avec le son")
                .clicked()
            {
                let _ = Process::new("open")
                    .args(["-a", "QuickTime Player"])
                    .arg(&self.path)
                    .spawn();
            }
            if ui
                .button(icons::FOLDER)
                .on_hover_text("Afficher dans le Finder")
                .clicked()
            {
                history::reveal(&self.path);
            }
        });
        ui.horizontal(|ui| {
            if let Some(job) = &self.job {
                ui.spinner();
                ui.add(
                    egui::ProgressBar::new(job.progress)
                        .desired_width(220.0)
                        .text(job.label),
                );
            } else if let Some((text, file)) = self.status.clone() {
                ui.label(text);
                if let Some((kind, path)) = file {
                    if ui.small_button("Afficher").clicked() {
                        history::reveal(&path);
                    }
                    if kind == JobKind::Image
                        && ui.small_button("Annoter").clicked()
                        && let Ok(exe) = std::env::current_exe()
                    {
                        let _ = Process::new(exe).arg("--edit").arg(&path).spawn();
                    }
                    if kind == JobKind::Video
                        && ui.small_button("Ouvrir").clicked()
                        && let Ok(exe) = std::env::current_exe()
                    {
                        let _ = Process::new(exe).arg(super::PLAY_ARG).arg(&path).spawn();
                    }
                }
            }
        });
    }
}

impl eframe::App for Player {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.keyboard(&ctx);
        self.update_playback();
        self.update_frame(&ctx);
        self.poll_job();
        egui::Panel::bottom("commandes")
            .frame(
                Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| {
                self.timeline(ui);
                self.controls(ui);
            });
        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(12)))
            .show(ui, |ui| self.preview(ui));
        if self.playing.is_some() {
            ctx.request_repaint();
        }
    }
}

fn speed_label(s: f64) -> String {
    let text = format!("{s}").replace('.', ",");
    format!("{text}×")
}

fn width_label(w: u32) -> String {
    if w == 0 {
        "Taille d'origine".into()
    } else {
        format!("{w} px")
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(speed_label(1.25), "1,25×");
        assert_eq!(speed_label(2.0), "2×");
        assert_eq!(width_label(0), "Taille d'origine");
        assert_eq!(width_label(640), "640 px");
    }
}
