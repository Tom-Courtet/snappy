// Windows, en release : pas de fenêtre console à côté de l'app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! Snappy : capture d'écran légère avec annotations non destructives.
//!
//! Un seul exécutable, plusieurs rôles :
//! - sans argument : l'**agent**, discret dans la barre des menus. Il attend les
//!   raccourcis globaux (⌘⇧2 zone, ⌘⇧1 écran) et lance les captures ;
//! - `--capture region|screen` : une capture, dans son propre processus (lancé par
//!   l'agent). Sa mémoire est rendue au système dès qu'elle se termine ;
//! - `--edit <image.png>` : l'éditeur d'annotations ;
//! - `--settings` : la fenêtre des réglages (ouverte par l'agent, une seule à la
//!   fois ; sans agent, directement) ;
//! - `--region`, `--screen` : demande une capture à l'agent (scripts, raccourcis
//!   d'autres outils). Sans agent, la capture se fait directement.

mod agent;
mod capture;
mod config;
mod editor;
mod history;
mod ipc;
mod model;
mod overlay;
mod platform;
mod render;
mod settings;
mod ui;

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

use config::Store;
use ipc::{Mode, Request};
use overlay::Outcome;

const USAGE: &str = "\
Snappy : capture d'écran légère, annotations non destructives.

Usage :
  snappy                  lance l'agent (barre des menus)
  snappy --region         capture une zone
  snappy --region --delay 3
                          capture une zone dans 3 secondes (menus, survols)
  snappy --screen         capture l'écran du curseur et l'ouvre dans l'éditeur
  snappy --previous       capture la même zone que la fois précédente
  snappy --edit <png>     ouvre une image dans l'éditeur
  snappy --settings       ouvre les réglages (raccourcis, délai, dossier)
  snappy --export <png> <sortie.png>
                          aplatit une capture et ses annotations (.json à côté)
  snappy --quit           arrête l'agent
  snappy --version";

fn main() -> Result<()> {
    attach_console();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,snappy=info"))
        .init();
    let store = Store::open_default()?;
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("--background") => {
            if ipc::request(&store.socket_path(), &Request::Ping).is_ok() {
                println!("Snappy tourne déjà (icône dans la barre des menus).");
                return Ok(());
            }
            agent::run(store)
        }
        Some("--quit") => match ipc::request(&store.socket_path(), &Request::Quit) {
            Ok(_) => Ok(()),
            Err(_) => {
                println!("Snappy ne tourne pas.");
                Ok(())
            }
        },
        Some("--region") => trigger(store, Mode::Region, delay_arg(args)?),
        Some("--screen") => trigger(store, Mode::Screen, delay_arg(args)?),
        Some("--previous") => trigger(store, Mode::Previous, 0),
        Some("--capture") => {
            let arg = args.next().unwrap_or_default();
            let mode = Mode::from_arg(&arg)
                .with_context(|| format!("--capture attend region ou screen, pas « {arg} »"))?;
            run_capture(store, mode)
        }
        Some("--edit") => {
            let path = args.next().context("--edit attend le chemin d'une image")?;
            editor::run(store, PathBuf::from(path))
        }
        Some("--settings") => match ipc::request(&store.socket_path(), &Request::OpenSettings) {
            Ok(ipc::Response::Error { message }) => bail!("{message}"),
            Ok(_) => Ok(()),
            // Sans agent : la fenêtre ici.
            Err(_) => settings::run(store),
        },
        Some(settings::WINDOW_ARG) => settings::run(store),
        Some("--export") => {
            let input = PathBuf::from(args.next().context("--export attend une capture")?);
            let output = PathBuf::from(
                args.next()
                    .context("--export attend le fichier de sortie")?,
            );
            export(&input, &output)
        }
        Some("--version") => {
            println!("snappy {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help" | "-h") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("argument inconnu : {other}\n\n{USAGE}"),
    }
}

/// Demande la capture à l'agent ; sans agent, la fait ici.
fn trigger(store: Store, mode: Mode, delay: u32) -> Result<()> {
    match ipc::request(&store.socket_path(), &Request::Capture { mode, delay }) {
        Ok(ipc::Response::Error { message }) => bail!("{message}"),
        Ok(_) => Ok(()),
        Err(_) => {
            std::thread::sleep(std::time::Duration::from_secs(u64::from(delay)));
            run_capture(store, mode)
        }
    }
}

/// Une capture, de l'écran figé jusqu'à l'action choisie.
fn run_capture(store: Store, mode: Mode) -> Result<()> {
    if !capture::has_permission() {
        capture::request_permission();
        bail!(
            "Snappy n'a pas l'autorisation « Enregistrement de l'écran ». Accorde-la dans \
             Réglages Système > Confidentialité et sécurité, puis relance Snappy."
        );
    }
    let config = store.load();
    let capture = capture::capture_all()?;
    match mode {
        Mode::Region => region(store, capture, config.capture_cursor),
        Mode::Previous => match previous_region(&store, &capture, config.capture_cursor) {
            Some(img) => {
                history::save_capture(&store.captures_dir(), &img)?;
                history::copy_image(&img)
            }
            // Pas encore de zone précédente (ou écran changé) : sélection normale.
            None => region(store, capture, config.capture_cursor),
        },
        Mode::Screen => {
            let shot = &capture.shots[capture.active];
            let (w, h) = shot.image.dimensions();
            let img = shot.region(0, 0, w, h, config.capture_cursor);
            let path = history::save_capture(&store.captures_dir(), &img)?;
            if let Err(e) = history::copy_image(&img) {
                log::warn!("{e:#}");
            }
            editor::run(store, path)
        }
    }
}

/// Sélection de zone sur les écrans figés, puis l'action choisie.
fn region(store: Store, capture: capture::Capture, with_cursor: bool) -> Result<()> {
    match overlay::run(store, capture, with_cursor)? {
        Outcome::Edit(path) => open_editor(&path),
        Outcome::Saved(path) => {
            log::info!("enregistré : {}", path.display());
            Ok(())
        }
        Outcome::Copied | Outcome::Cancelled => Ok(()),
    }
}

/// La zone de la capture précédente, sur le même écran s'il a toujours la même
/// définition.
fn previous_region(
    store: &Store,
    capture: &capture::Capture,
    with_cursor: bool,
) -> Option<image::RgbaImage> {
    let last = store.last_region()?;
    let shot = capture.shots.iter().find(|s| {
        s.display_id == last.display_id
            && s.image.dimensions() == (last.image_width, last.image_height)
    })?;
    let fits = last.width > 0
        && last.height > 0
        && last.x + last.width <= last.image_width
        && last.y + last.height <= last.image_height;
    fits.then(|| shot.region(last.x, last.y, last.width, last.height, with_cursor))
}

/// L'éditeur dans un nouveau processus : une fenêtre egui ne peut être ouverte qu'une
/// fois par processus sous macOS, et la surcouche vient de se fermer.
fn open_editor(path: &std::path::Path) -> Result<()> {
    let exe = std::env::current_exe().context("chemin de l'exécutable")?;
    Command::new(exe)
        .arg("--edit")
        .arg(path)
        .spawn()
        .context("lancement de l'éditeur")?;
    Ok(())
}

/// Image aplatie (capture + annotations), sans fenêtre : scripts, automatisation.
fn export(input: &std::path::Path, output: &std::path::Path) -> Result<()> {
    let base = history::read_png(input)?;
    let doc = history::load_doc(input);
    let img = render::render(&base, &doc.annotations);
    history::write_png(output, &img)?;
    println!(
        "{} ({} annotations)",
        output.display(),
        doc.annotations.len()
    );
    Ok(())
}

/// Sous Windows, l'exe n'a pas de console (sous-système « windows ») : lancé depuis
/// un terminal avec des arguments, il reprend celle du terminal pour y écrire.
fn attach_console() {
    #[cfg(windows)]
    if std::env::args().len() > 1 {
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        // SAFETY : sans effet s'il n'y a pas de console parente.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}

/// `--delay N` après `--region` ou `--screen` (0 sans l'option).
fn delay_arg(mut args: impl Iterator<Item = String>) -> Result<u32> {
    match args.next().as_deref() {
        None => Ok(0),
        Some("--delay") => {
            let n = args
                .next()
                .context("--delay attend un nombre de secondes")?;
            n.parse()
                .with_context(|| format!("--delay attend un nombre de secondes, pas « {n} »"))
        }
        Some(other) => bail!("argument inconnu : {other}"),
    }
}
