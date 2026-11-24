//! Fenêtre « Espace de travail » : les captures en vignettes (clic gauche sur
//! l'icône de la barre des menus, raccourci « Espace de travail »).
//!
//! Un processus à part (`snappy --capture workspace`), lancé et refermé par l'agent,
//! comme les réglages. Contenu : le dossier des captures (« Base ») et ses
//! sous-dossiers (les autres espaces de travail). Les nouvelles captures vont
//! toujours dans « Base ».
//!
//! - [`library`] : liste, filtres et tri (logique pure) ;
//! - [`thumbs`] : vignettes avec annotations, en cache sur le disque ;
//! - [`view`] : l'interface ;
//! - [`dnd`] : glisser des captures vers d'autres applications ;
//! - [`prefs`] : préférences propres à la fenêtre (`workspace.json`) ;
//! - [`place`] : position sous l'icône de la barre des menus.

mod dnd;
mod library;
mod place;
mod prefs;
mod thumbs;
mod view;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result};
use egui::{TextureHandle, WindowLevel};

use crate::config::Store;
use crate::files;
use crate::history;
use crate::ipc::{self, Mode, Request};
use crate::platform;
use crate::ui::theme;
use library::{Filter, Item};
pub use place::{ANCHOR_ENV, Anchor};
use prefs::Prefs;
use thumbs::Thumbs;

/// Icône de l'application, 64 × 64 RGBA (logo de l'en-tête).
pub(crate) const ICON_64: &[u8] = include_bytes!("../../assets/icon_64.rgba");

/// Taille de la fenêtre de Screenpresso.
const SIZE: [f32; 2] = [893.0, 591.0];
/// Relecture du dossier (nouvelles captures, éditions).
const RESCAN_EVERY: Duration = Duration::from_secs(2);
const STATUS_DURATION: Duration = Duration::from_secs(3);
/// Délai entre la fermeture de la fenêtre et la capture demandée : la fenêtre ne
/// doit pas apparaître dessus.
const CLOSE_BEFORE_CAPTURE: Duration = Duration::from_millis(250);

/// Ce que la fenêtre demande à l'agent une fois fermée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum After {
    Capture(Mode),
    Quit,
}

/// Ouvre la fenêtre (processus `snappy --capture workspace`).
pub fn run(store: Store) -> Result<()> {
    let prefs = Prefs::load(&store);
    let anchor = Anchor::from_env();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Snappy · Espace de travail")
        .with_inner_size(SIZE)
        .with_min_inner_size([520.0, 360.0])
        .with_decorations(false)
        .with_icon(window_icon())
        .with_window_level(level(prefs.always_on_top));
    if let Some(pos) = initial_position(&prefs, anchor) {
        viewport = viewport.with_position(pos);
    }
    let options = eframe::NativeOptions {
        viewport,
        persist_window: false,
        ..Default::default()
    };
    let after = Arc::new(Mutex::new(None));
    let app_after = Arc::clone(&after);
    let app_store = store.clone();
    eframe::run_native(
        "Snappy",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            platform::activate(cc);
            // ⌘+ / ⌘− règlent la taille des vignettes, pas le zoom de l'interface.
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            Ok(Box::new(Workspace::new(
                &cc.egui_ctx,
                app_store,
                prefs,
                anchor,
                app_after,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("espace de travail : {e}"))?;
    let after = after.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(after) = after {
        thread::sleep(CLOSE_BEFORE_CAPTURE);
        perform(&store, after);
    }
    Ok(())
}

fn level(on_top: bool) -> WindowLevel {
    if on_top {
        WindowLevel::AlwaysOnTop
    } else {
        WindowLevel::Normal
    }
}

/// Position mémorisée, sinon sous (ou au-dessus de) l'icône.
fn initial_position(prefs: &Prefs, anchor: Option<Anchor>) -> Option<[f32; 2]> {
    if prefs.remember_position
        && let Some(pos) = prefs.position
    {
        return Some(pos);
    }
    anchor.map(|a| place::place(a, SIZE, None))
}

/// Icône de la fenêtre (et du Dock sous macOS).
fn window_icon() -> egui::IconData {
    #[cfg(target_os = "macos")]
    let (rgba, size): (&[u8], u32) = (include_bytes!("../../assets/icon_mac_256.rgba"), 256);
    #[cfg(not(target_os = "macos"))]
    let (rgba, size): (&[u8], u32) = (ICON_64, 64);
    egui::IconData {
        rgba: rgba.to_vec(),
        width: size,
        height: size,
    }
}

/// Demande à l'agent ; sans agent, lance l'action directement.
fn perform(store: &Store, after: After) {
    let socket = store.socket_path();
    match after {
        After::Capture(mode) => {
            let req = Request::Capture { mode, delay: 0 };
            if ipc::request(&socket, &req).is_err() {
                spawn_self(&["--capture", mode.arg()]);
            }
        }
        After::Quit => {
            let _ = ipc::request(&socket, &Request::Quit);
        }
    }
}

/// Relance l'exécutable avec ces arguments (éditeur, réglages, capture sans agent).
fn spawn_self(args: &[&str]) {
    let result = std::env::current_exe()
        .context("chemin de l'exécutable")
        .and_then(|exe| Command::new(exe).args(args).spawn().context("lancement"));
    if let Err(e) = result {
        log::error!("snappy {} : {e:#}", args.join(" "));
    }
}

/// Réglages : par l'agent (une seule fenêtre), sinon directement.
fn open_settings(store: &Store) {
    let socket = store.socket_path();
    thread::spawn(move || {
        if !matches!(
            ipc::request(&socket, &Request::OpenSettings),
            Ok(ipc::Response::Ok)
        ) {
            spawn_self(&[crate::settings::WINDOW_ARG]);
        }
    });
}

/// Fermeture automatique après la perte du focus (Screenpresso :
/// `WorkspaceAutoClose`, 3 s).
#[derive(Debug, Default)]
struct AutoClose {
    /// La fenêtre a eu le focus au moins une fois (sinon, elle vient de s'ouvrir).
    ever_focused: bool,
    lost_since: Option<Instant>,
}

impl AutoClose {
    /// `Some(reste)` : pas encore ; `None` : rien à surveiller. Rend `true` dans
    /// `close` quand il faut fermer.
    fn update(
        &mut self,
        focused: Option<bool>,
        now: Instant,
        enabled: bool,
        delay: Duration,
        busy: bool,
    ) -> (bool, Option<Duration>) {
        if focused == Some(true) {
            self.ever_focused = true;
            self.lost_since = None;
            return (false, None);
        }
        if busy || !enabled || !self.ever_focused || focused.is_none() {
            self.lost_since = None;
            return (false, None);
        }
        let since = *self.lost_since.get_or_insert(now);
        let elapsed = now.saturating_duration_since(since);
        if elapsed >= delay {
            (true, None)
        } else {
            (false, Some(delay - elapsed))
        }
    }

    /// Après une boîte de dialogue native (elle prend le focus) : on recompte.
    fn reset(&mut self) {
        self.lost_since = None;
    }
}

/// Boîte de dialogue de la fenêtre.
#[derive(Debug, Clone, PartialEq)]
enum Dialog {
    Rename {
        path: PathBuf,
        text: String,
        error: Option<String>,
    },
    NewWorkspace {
        text: String,
        error: Option<String>,
    },
    RenameWorkspace {
        path: PathBuf,
        text: String,
        error: Option<String>,
    },
    /// Confirmation de la mise à la Corbeille (Screenpresso : `ConfirmDelete`).
    Delete {
        paths: Vec<PathBuf>,
    },
    DeleteWorkspace {
        path: PathBuf,
    },
    Properties {
        path: PathBuf,
    },
    Help,
    About,
}

/// Aperçu rapide (Espace) : l'image aplatie dans une fenêtre sans bordure.
struct Preview {
    path: PathBuf,
    texture: Option<TextureHandle>,
    size: [f32; 2],
    loading: Arc<Mutex<Option<Result<image::RgbaImage, String>>>>,
    error: Option<String>,
    close: bool,
    /// Position calculée à l'ouverture (centre de la fenêtre principale).
    placed: bool,
}

pub struct Workspace {
    store: Store,
    /// Dossier des captures (« Base »).
    root: PathBuf,
    /// Espace ouvert.
    current: PathBuf,
    prefs: Prefs,
    items: Vec<Item>,
    /// Indices de `items` affichés, dans l'ordre.
    view: Vec<usize>,
    filter: Filter,
    search_open: bool,
    focus_search: bool,
    today: i64,
    workspaces: Vec<PathBuf>,
    selection: Vec<PathBuf>,
    /// Élément actif (clavier, plage de ⇧-clic).
    cursor: Option<PathBuf>,
    thumbs: Thumbs,
    last_scan: Instant,
    dialog: Option<Dialog>,
    status: Option<(String, Instant)>,
    preview: Option<Preview>,
    auto_close: AutoClose,
    after: Arc<Mutex<Option<After>>>,
    logo: TextureHandle,
    anchor: Option<Anchor>,
    /// Position ajustée à l'écran (première image).
    placed: bool,
    /// Ligne de la grille à faire apparaître.
    scroll_to_row: Option<usize>,
    /// Défilement et hauteur visibles de la grille à l'image précédente.
    scroll: (f32, f32),
    /// Colonnes de la grille à l'image précédente (flèches haut et bas).
    columns: usize,
    drag: dnd::DragOut,
    last_pointer: Option<egui::Pos2>,
    /// Dernière position enregistrée (Mémoriser la position).
    saved_at: Instant,
    ctx: egui::Context,
}

impl Workspace {
    fn new(
        ctx: &egui::Context,
        store: Store,
        prefs: Prefs,
        anchor: Option<Anchor>,
        after: Arc<Mutex<Option<After>>>,
    ) -> Self {
        let root = store.captures_dir();
        let _ = std::fs::create_dir_all(&root);
        let current = workspace_dir(&root, &prefs.workspace);
        let repaint_ctx = ctx.clone();
        let logo = ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied([64, 64], ICON_64),
            egui::TextureOptions::LINEAR,
        );
        let mut ws = Self {
            store,
            root,
            current,
            prefs,
            items: Vec::new(),
            view: Vec::new(),
            filter: Filter::default(),
            search_open: false,
            focus_search: false,
            today: 0,
            workspaces: Vec::new(),
            selection: Vec::new(),
            cursor: None,
            thumbs: Thumbs::new(move || repaint_ctx.request_repaint()),
            last_scan: Instant::now(),
            dialog: None,
            status: None,
            preview: None,
            auto_close: AutoClose::default(),
            after,
            logo,
            anchor,
            placed: false,
            scroll_to_row: None,
            scroll: (0.0, 0.0),
            columns: 1,
            drag: dnd::DragOut::default(),
            last_pointer: None,
            saved_at: Instant::now(),
            ctx: ctx.clone(),
        };
        ws.refresh();
        // La plus récente sélectionnée, comme dans Screenpresso.
        if let Some(latest) = ws
            .items
            .iter()
            .max_by_key(|i| i.media.date())
            .map(|i| i.media.path.clone())
        {
            ws.select_only(latest);
        }
        ws
    }

    // --- Données -------------------------------------------------------------------

    /// Relit le dossier courant et la liste des espaces.
    fn refresh(&mut self) {
        if !self.current.is_dir() {
            self.current = self.root.clone();
            self.prefs.workspace.clear();
        }
        self.items = library::scan(&self.current, &self.items);
        self.workspaces = files::workspaces(&self.root);
        self.today = library::local_day(SystemTime::now());
        self.last_scan = Instant::now();
        self.update_view();
        let present: std::collections::HashSet<&PathBuf> =
            self.items.iter().map(|i| &i.media.path).collect();
        self.selection.retain(|p| present.contains(p));
        if self.cursor.as_ref().is_some_and(|c| !present.contains(c)) {
            self.cursor = self.selection.last().cloned();
        }
    }

    fn update_view(&mut self) {
        self.view = library::view(&self.items, &self.filter, self.prefs.sort, self.today);
    }

    fn item(&self, path: &Path) -> Option<&Item> {
        self.items.iter().find(|i| i.media.path == path)
    }

    /// Position d'un chemin dans l'affichage.
    fn view_index(&self, path: &Path) -> Option<usize> {
        self.view
            .iter()
            .position(|&i| self.items[i].media.path == path)
    }

    /// Sélection, dans l'ordre de l'affichage.
    fn selected(&self) -> Vec<PathBuf> {
        self.view
            .iter()
            .map(|&i| &self.items[i].media.path)
            .filter(|p| self.selection.contains(p))
            .cloned()
            .collect()
    }

    fn select_only(&mut self, path: PathBuf) {
        self.selection = vec![path.clone()];
        if let Some(i) = self.view_index(&path) {
            self.scroll_to_row = Some(i / self.columns.max(1));
        }
        self.cursor = Some(path);
    }

    fn save_prefs(&self) {
        self.prefs.save(&self.store);
    }

    fn say(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), Instant::now()));
    }

    fn report<T>(&mut self, result: Result<T>) -> Option<T> {
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                self.say(format!("{} {e:#}", crate::ui::icons::WARNING));
                None
            }
        }
    }

    // --- Fenêtre ---------------------------------------------------------------------

    /// Ferme la fenêtre, puis fait `after` (capture : la fenêtre ne doit pas être
    /// dessus).
    fn close(&mut self, after: Option<After>) {
        *self.after.lock().unwrap_or_else(|e| e.into_inner()) = after;
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn set_always_on_top(&mut self, on: bool) {
        self.prefs.always_on_top = on;
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::WindowLevel(level(on)));
        self.save_prefs();
    }

    fn zoom_thumbs(&mut self, delta: f32) {
        self.prefs.zoom_thumbs(delta);
        self.save_prefs();
    }

    fn set_sort(&mut self, sort: library::Sort) {
        self.prefs.sort = sort;
        self.update_view();
        self.save_prefs();
    }

    fn set_workspace(&mut self, dir: PathBuf) {
        if dir == self.current {
            return;
        }
        self.prefs.workspace = if dir == self.root {
            String::new()
        } else {
            files::file_name(&dir)
        };
        self.current = dir;
        self.items.clear();
        self.selection.clear();
        self.cursor = None;
        self.thumbs.clear_queue();
        self.refresh();
        self.save_prefs();
    }

    fn workspace_name(&self, dir: &Path) -> String {
        if dir == self.root {
            "Base".into()
        } else {
            files::file_name(dir)
        }
    }

    // --- Actions sur les captures ----------------------------------------------------

    /// Ouvrir : l'éditeur pour une image, l'application par défaut sinon.
    fn open(&mut self, path: &Path) {
        if files::kind_of(path) == Some(files::Kind::Image) {
            let arg = path.to_string_lossy().into_owned();
            spawn_self(&["--edit", &arg]);
            // Comme Screenpresso (`WorkspaceCloseOnEdit`) : la fenêtre, toujours
            // devant, cacherait l'éditeur.
            self.close(None);
        } else {
            files::open_default(path);
        }
    }

    fn copy_image(&mut self, path: &Path) {
        if files::kind_of(path) != Some(files::Kind::Image) {
            self.say("Seules les images se copient dans le presse-papiers.");
            return;
        }
        let result = files::flattened(path).and_then(|img| history::copy_image(&img));
        if self.report(result).is_some() {
            self.say(format!("{} Image copiée", crate::ui::icons::OK));
        }
    }

    fn copy_paths(&mut self, paths: &[PathBuf]) {
        let text = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        if self.report(files::copy_text(&text)).is_some() {
            self.say(format!("{} Chemin copié", crate::ui::icons::OK));
        }
    }

    fn save_as(&mut self, path: &Path) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Enregistrer sous")
            .set_file_name(format!("{}.png", files::stem(path)));
        for (name, exts) in files::SAVE_FORMATS {
            dialog = dialog.add_filter(*name, exts);
        }
        let target = dialog.save_file();
        self.auto_close.reset();
        let Some(target) = target else { return };
        // Sans extension : PNG.
        let target = if files::save_format(&target).is_none() && target.extension().is_none() {
            target.with_extension("png")
        } else {
            target
        };
        let result = files::flattened(path).and_then(|img| files::save_as(&img, &target));
        if self.report(result).is_some() {
            self.say(format!(
                "{} Enregistré : {}",
                crate::ui::icons::OK,
                target.display()
            ));
        }
    }

    fn trash(&mut self, paths: &[PathBuf]) {
        let mut done = 0;
        for path in paths {
            let result = files::trash_media(path);
            if self.report(result).is_some() {
                self.thumbs.forget(path);
                done += 1;
            }
        }
        if done > 0 {
            self.say(format!(
                "{} {done} élément{} à la Corbeille",
                crate::ui::icons::DELETE,
                if done > 1 { "s" } else { "" }
            ));
        }
        // Sélection : l'élément suivant.
        let next = paths.last().and_then(|p| self.view_index(p)).and_then(|i| {
            self.view[i..]
                .iter()
                .chain(self.view[..i].iter().rev())
                .map(|&j| self.items[j].media.path.clone())
                .find(|p| !paths.contains(p))
        });
        self.refresh();
        if let Some(next) = next {
            self.select_only(next);
        }
    }

    fn rename(&mut self, path: &Path, name: &str) -> Result<()> {
        let new = files::rename(path, name)?;
        self.thumbs.forget(path);
        self.refresh();
        self.select_only(new);
        Ok(())
    }

    fn move_to(&mut self, paths: &[PathBuf], dir: &Path) {
        let mut moved = 0;
        for path in paths {
            if self.report(files::move_to(path, dir)).is_some() {
                self.thumbs.forget(path);
                moved += 1;
            }
        }
        self.refresh();
        if moved > 0 {
            self.say(format!(
                "{moved} élément{} déplacé{} vers « {} »",
                if moved > 1 { "s" } else { "" },
                if moved > 1 { "s" } else { "" },
                self.workspace_name(dir)
            ));
        }
    }

    /// Fusionne les images sélectionnées en une nouvelle capture.
    fn merge(&mut self, paths: &[PathBuf], horizontal: bool) {
        let images: Result<Vec<_>> = paths.iter().map(|p| files::flattened(p)).collect();
        let result = images
            .map(|imgs| files::merge(&imgs, horizontal))
            .and_then(|img| history::save_capture(&self.current, &img));
        if let Some(new) = self.report(result) {
            self.refresh();
            self.select_only(new);
            self.say(format!("{} Images fusionnées", crate::ui::icons::OK));
        }
    }

    /// Fichiers déposés dans la fenêtre : copiés dans l'espace courant.
    fn import(&mut self, dropped: Vec<PathBuf>) {
        let mut added = Vec::new();
        for src in dropped {
            // Glissé depuis cette fenêtre et relâché dessus : rien à faire.
            if src.parent() == Some(self.current.as_path()) {
                continue;
            }
            if let Some(new) = self.report(files::import(&src, &self.current)) {
                added.push(new);
            }
        }
        if let Some(last) = added.last().cloned() {
            self.refresh();
            self.select_only(last);
            self.say(format!(
                "{} {} fichier{} ajouté{} à « {} »",
                crate::ui::icons::OK,
                added.len(),
                if added.len() > 1 { "s" } else { "" },
                if added.len() > 1 { "s" } else { "" },
                self.workspace_name(&self.current.clone())
            ));
        }
    }

    // --- Espaces de travail ----------------------------------------------------------

    fn create_workspace(&mut self, name: &str) -> Result<()> {
        let name = files::validate_name(name).map_err(|e| anyhow::anyhow!("{e}"))?;
        let dir = self.root.join(&name);
        if dir.exists() {
            anyhow::bail!("« {name} » existe déjà");
        }
        std::fs::create_dir_all(&dir).with_context(|| format!("création de {}", dir.display()))?;
        self.refresh();
        self.set_workspace(dir);
        Ok(())
    }

    fn rename_workspace(&mut self, dir: &Path, name: &str) -> Result<()> {
        let name = files::validate_name(name).map_err(|e| anyhow::anyhow!("{e}"))?;
        let new = self.root.join(&name);
        if new == dir {
            return Ok(());
        }
        let same = new.to_string_lossy().to_lowercase() == dir.to_string_lossy().to_lowercase();
        if new.exists() && !same {
            anyhow::bail!("« {name} » existe déjà");
        }
        std::fs::rename(dir, &new).with_context(|| format!("renommage de {}", dir.display()))?;
        let was_current = self.current == dir;
        self.refresh();
        if was_current {
            self.current = self.root.clone();
            self.set_workspace(new);
        }
        Ok(())
    }

    /// Supprime un espace : vide, il disparaît ; sinon il part à la Corbeille avec
    /// son contenu (après confirmation).
    fn delete_workspace(&mut self, dir: &Path) {
        let result = if files::is_empty_dir(dir) {
            std::fs::remove_dir_all(dir).context("suppression du dossier")
        } else {
            files::trash(&[dir.to_owned()])
        };
        if self.report(result).is_some() {
            if self.current == dir {
                self.set_workspace(self.root.clone());
            }
            self.refresh();
            self.say(format!("Espace « {} » supprimé", files::file_name(dir)));
        }
    }

    // --- Aperçu rapide ---------------------------------------------------------------

    fn toggle_preview(&mut self) {
        if self.preview.take().is_some() {
            return;
        }
        let Some(path) = self
            .cursor
            .clone()
            .or_else(|| self.selected().first().cloned())
        else {
            return;
        };
        if files::kind_of(&path) != Some(files::Kind::Image) {
            files::open_default(&path);
            return;
        }
        let loading = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&loading);
        let ctx = self.ctx.clone();
        let source = path.clone();
        thread::spawn(move || {
            let result = files::flattened(&source).map_err(|e| format!("{e:#}"));
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            ctx.request_repaint();
        });
        self.preview = Some(Preview {
            path,
            texture: None,
            size: [400.0, 300.0],
            loading,
            error: None,
            close: false,
            placed: false,
        });
    }
}

/// Dossier d'un espace d'après son nom (vide : la base).
fn workspace_dir(root: &Path, name: &str) -> PathBuf {
    if name.is_empty() || files::validate_name(name).is_err() {
        root.to_owned()
    } else {
        root.join(name)
    }
}

impl eframe::App for Workspace {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.drag.fix_input(raw, self.last_pointer);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.thumbs.begin_frame(&ctx);
        if let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) {
            self.last_pointer = Some(pos);
        }
        if self.last_scan.elapsed() >= RESCAN_EVERY && self.dialog.is_none() {
            self.refresh();
        }
        ctx.request_repaint_after(RESCAN_EVERY);
        self.place_window(&ctx);
        self.remember_position(&ctx);
        self.dropped_files(&ctx);
        self.keyboard(&ctx);
        self.draw(ui, frame);
        self.show_preview(&ctx);
        self.watch_focus(&ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(theme::BG).to_array()
    }
}

impl Workspace {
    /// Première image : la fenêtre ne déborde pas de l'écran.
    fn place_window(&mut self, ctx: &egui::Context) {
        if self.placed {
            return;
        }
        let (monitor, outer) = ctx.input(|i| (i.viewport().monitor_size, i.viewport().outer_rect));
        let (Some(monitor), Some(outer)) = (monitor, outer) else {
            return;
        };
        self.placed = true;
        if self.prefs.remember_position && self.prefs.position.is_some() {
            return;
        }
        if let Some(anchor) = self.anchor {
            let [x, y] = place::place(
                anchor,
                [outer.width(), outer.height()],
                Some([monitor.x, monitor.y]),
            );
            if (outer.min.x - x).abs() > 1.0 || (outer.min.y - y).abs() > 1.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
            }
        }
    }

    /// « Mémoriser la position » : la note quand la fenêtre a bougé (au plus une
    /// fois par seconde).
    fn remember_position(&mut self, ctx: &egui::Context) {
        if !self.prefs.remember_position || self.saved_at.elapsed() < Duration::from_secs(1) {
            return;
        }
        let Some(outer) = ctx.input(|i| i.viewport().outer_rect) else {
            return;
        };
        let pos = [outer.min.x, outer.min.y];
        let moved = self
            .prefs
            .position
            .is_none_or(|p| (p[0] - pos[0]).abs() > 1.0 || (p[1] - pos[1]).abs() > 1.0);
        if moved {
            self.prefs.position = Some(pos);
            self.save_prefs();
            self.saved_at = Instant::now();
        }
    }

    fn dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_owned())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() && !self.drag.is_active() {
            self.import(dropped);
        }
    }

    fn watch_focus(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused);
        let busy = self.preview.is_some() || self.drag.is_active() || self.dialog.is_some();
        let (close, remaining) = self.auto_close.update(
            focused,
            Instant::now(),
            self.prefs.auto_close,
            Duration::from_secs_f32(self.prefs.auto_close_delay),
            busy,
        );
        if close {
            log::info!("espace de travail fermé (focus perdu)");
            self.close(None);
        } else if let Some(remaining) = remaining {
            ctx.request_repaint_after(remaining);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_close_waits_for_the_delay_after_focus_loss() {
        let mut a = AutoClose::default();
        let t0 = Instant::now();
        let d = Duration::from_secs(3);
        // Jamais eu le focus (fenêtre qui s'ouvre) : rien.
        assert_eq!(a.update(Some(false), t0, true, d, false), (false, None));
        assert_eq!(a.update(Some(true), t0, true, d, false), (false, None));
        let (close, left) = a.update(Some(false), t0, true, d, false);
        assert!(!close);
        assert_eq!(left, Some(d));
        assert!(
            !a.update(Some(false), t0 + Duration::from_secs(2), true, d, false)
                .0
        );
        assert!(
            a.update(Some(false), t0 + Duration::from_secs(3), true, d, false)
                .0
        );
    }

    #[test]
    fn auto_close_is_suspended_while_busy_or_disabled() {
        let mut a = AutoClose::default();
        let t0 = Instant::now();
        let d = Duration::from_secs(3);
        a.update(Some(true), t0, true, d, false);
        a.update(Some(false), t0, true, d, false);
        // Aperçu ouvert, boîte de dialogue… : le compte repart de zéro ensuite.
        assert!(
            !a.update(Some(false), t0 + Duration::from_secs(5), true, d, true)
                .0
        );
        assert!(
            !a.update(Some(false), t0 + Duration::from_secs(6), true, d, false)
                .0
        );
        assert!(
            !a.update(Some(false), t0 + Duration::from_secs(60), false, d, false)
                .0
        );
        // Focus retrouvé : remis à zéro.
        a.update(Some(false), t0, true, d, false);
        a.update(Some(true), t0 + Duration::from_secs(1), true, d, false);
        assert!(
            !a.update(Some(false), t0 + Duration::from_secs(4), true, d, false)
                .0
        );
    }

    #[test]
    fn workspace_names_map_to_subfolders() {
        let root = Path::new("/c");
        assert_eq!(workspace_dir(root, ""), root);
        assert_eq!(workspace_dir(root, "Projet"), root.join("Projet"));
        // Nom trafiqué dans workspace.json : la base, jamais en dehors.
        assert_eq!(workspace_dir(root, "../x"), root);
    }

    #[test]
    fn initial_position_prefers_the_remembered_one() {
        let anchor = Some(Anchor {
            x: 1000.0,
            y: 0.0,
            w: 30.0,
            h: 24.0,
        });
        let mut prefs = Prefs::default();
        assert!(initial_position(&prefs, anchor).is_some());
        assert_eq!(initial_position(&prefs, None), None);
        prefs.remember_position = true;
        prefs.position = Some([5.0, 6.0]);
        assert_eq!(initial_position(&prefs, anchor), Some([5.0, 6.0]));
    }

    /// Les textes de la fenêtre s'affichent avec les polices embarquées (pas de
    /// carré à la place d'un caractère).
    #[test]
    fn every_workspace_character_has_a_glyph() {
        use egui::epaint::text::{Fonts, TextOptions};
        let mut fonts = Fonts::new(TextOptions::default(), theme::fonts());
        let font = egui::FontId::new(14.0, egui::FontFamily::Proportional);
        let sources = [
            include_str!("mod.rs"),
            include_str!("view.rs"),
            include_str!("library.rs"),
        ];
        let mut missing = Vec::new();
        for source in sources {
            for line in source.lines() {
                let code = line.split("//").next().unwrap_or("");
                for (i, part) in code.split('"').enumerate() {
                    if i % 2 == 0 {
                        continue;
                    }
                    for c in part.chars().filter(|c| !c.is_ascii()) {
                        if !fonts.has_glyph(&font, c) {
                            missing.push(c);
                        }
                    }
                }
            }
        }
        assert!(missing.is_empty(), "glyphes absents : {missing:?}");
    }
}
