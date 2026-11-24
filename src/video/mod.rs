//! Enregistrement vidéo d'une zone de l'écran, lecteur et export.
//!
//! Trois processus, chacun avec au plus une fenêtre egui (règle de Snappy sous
//! macOS) :
//! 1. `snappy --capture video|video-region` ([`run`]) : choisit la zone (variable
//!    `SNAPPY_VIDEO_RECT`, dernière zone vidéo pour `video`, sinon la sélection
//!    [`crate::overlay::pick`], et l'écran entier si la sélection n'est pas
//!    disponible), puis lance la barre et s'arrête. Si une barre est déjà ouverte, il
//!    lui demande seulement d'arrêter (ou de démarrer) l'enregistrement : le
//!    raccourci « Vidéo » sert donc aussi d'arrêt, sans rien changer à l'agent.
//! 2. `snappy --video-bar <json>` ([`run_bar`], [`bar`]) : la barre et
//!    l'enregistrement ([`sck`], ScreenCaptureKit).
//! 3. `snappy --play <mp4>` ([`play`], [`player`]) : le lecteur, ouvert à la fin de
//!    l'enregistrement (réglable).
//!
//! Windows : pas encore d'enregistrement ; message clair.

mod gif_export;
mod options;
mod region;
mod session;

#[cfg(target_os = "macos")]
mod av;
#[cfg(target_os = "macos")]
mod bar;
#[cfg(target_os = "macos")]
mod player;
#[cfg(target_os = "macos")]
mod sck;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::capture::Capture;
use crate::config::Store;
use crate::ipc::Mode;

#[allow(unused_imports)] // pour l'agent (raccourci « Vidéo » pendant un enregistrement)
pub use session::request_stop;

use region::{Display, Rect};

/// Argument du processus de la barre vidéo.
pub const BAR_ARG: &str = "--video-bar";
/// Argument du lecteur vidéo.
pub const PLAY_ARG: &str = "--play";
/// Zone imposée en points globaux, `"x,y,largeur,hauteur"` (essais, scripts).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const RECT_ENV: &str = "SNAPPY_VIDEO_RECT";

#[cfg(not(target_os = "macos"))]
const NOT_YET: &str = "Enregistrement vidéo pas encore disponible sous Windows";

/// Ce que le processus de capture passe à la barre (un argument JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct BarArgs {
    rect: Rect,
    displays: Vec<Display>,
}

/// `Mode::Video` (dernière zone vidéo) ou `Mode::VideoRegion` (nouvelle zone).
pub fn run(store: Store, mode: Mode, capture: Capture) -> Result<()> {
    if session::request_stop(&store) {
        log::info!("barre vidéo déjà ouverte : arrêt (ou départ) demandé");
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (mode, capture);
        anyhow::bail!(NOT_YET)
    }
    #[cfg(target_os = "macos")]
    {
        if !sck::available() {
            anyhow::bail!("l'enregistrement vidéo demande macOS 15 ou plus récent");
        }
        let displays = displays_of(&capture);
        let Some(rect) = choose_region(&store, mode, capture, &displays)? else {
            return Ok(());
        };
        let index = region::display_for(&rect, &displays).context("zone hors des écrans")?;
        let rect = region::fit(&rect, &displays[index]).context("zone trop petite")?;
        let args = serde_json::to_string(&BarArgs { rect, displays })?;
        let exe = std::env::current_exe().context("chemin de l'exécutable")?;
        std::process::Command::new(exe)
            .arg(BAR_ARG)
            .arg(args)
            .spawn()
            .context("ouverture de la barre vidéo")?;
        Ok(())
    }
}

/// Processus de la barre vidéo (voir l'en-tête du module).
pub fn run_bar(store: Store, args: Option<String>) -> Result<()> {
    let args: BarArgs = serde_json::from_str(&args.context("--video-bar attend sa zone")?)
        .context("--video-bar : zone illisible")?;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (store, args);
        anyhow::bail!(NOT_YET)
    }
    #[cfg(target_os = "macos")]
    {
        let Some(session) = session::Session::acquire(&store)? else {
            log::info!("une barre vidéo est déjà ouverte");
            return Ok(());
        };
        bar::run(store, session, args.rect, args.displays)
    }
}

/// Lecteur vidéo.
pub fn play(store: Store, path: PathBuf) -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (store, path);
        anyhow::bail!("Lecteur vidéo pas encore disponible sous Windows")
    }
    #[cfg(target_os = "macos")]
    player::run(store, path)
}

#[cfg(target_os = "macos")]
fn displays_of(capture: &Capture) -> Vec<Display> {
    capture
        .shots
        .iter()
        .map(|s| Display {
            id: s.display_id,
            frame: Rect::new(s.frame.x, s.frame.y, s.frame.width, s.frame.height),
            scale: s.scale,
        })
        .collect()
}

/// Zone à enregistrer, en points globaux. `None` : sélection annulée.
#[cfg(target_os = "macos")]
fn choose_region(
    store: &Store,
    mode: Mode,
    capture: Capture,
    displays: &[Display],
) -> Result<Option<Rect>> {
    if let Ok(text) = std::env::var(RECT_ENV) {
        let rect = Rect::parse(&text).with_context(|| {
            format!("{RECT_ENV} : « {text} » n'est pas « x,y,largeur,hauteur »")
        })?;
        return Ok(Some(rect));
    }
    if mode == Mode::Video
        && let Some(rect) = region::load_last(store, displays)
    {
        return Ok(Some(rect));
    }
    let full_screen = displays
        .get(capture.active)
        .or(displays.first())
        .map(|d| d.frame)
        .context("aucun écran")?;
    use crate::overlay::{Purpose, pick};
    match pick(store.clone(), capture, false, Purpose::Video) {
        Ok(Some(p)) => Ok(Some(Rect::new(
            p.frame.x + f64::from(p.x) / p.scale,
            p.frame.y + f64::from(p.y) / p.scale,
            f64::from(p.width) / p.scale,
            f64::from(p.height) / p.scale,
        ))),
        Ok(None) => Ok(None),
        Err(e) => {
            log::warn!("sélection de zone indisponible ({e:#}) : écran entier");
            Ok(Some(full_screen))
        }
    }
}

/// Chrono de la barre : `0:00:00`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// Position du lecteur : `0:00:09.21`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn timecode(seconds: f64) -> String {
    let centis = (seconds.max(0.0) * 100.0).round() as u64;
    let s = centis / 100;
    format!(
        "{}:{:02}:{:02}.{:02}",
        s / 3600,
        s / 60 % 60,
        s % 60,
        centis % 100
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks() {
        assert_eq!(clock(Duration::ZERO), "0:00:00");
        assert_eq!(clock(Duration::from_millis(61_900)), "0:01:01");
        assert_eq!(clock(Duration::from_secs(3 * 3600 + 5)), "3:00:05");
        assert_eq!(timecode(9.21), "0:00:09.21");
        assert_eq!(timecode(3725.5), "1:02:05.50");
        assert_eq!(timecode(-1.0), "0:00:00.00");
    }

    #[test]
    fn bar_args_round_trip() {
        let args = BarArgs {
            rect: Rect::new(1.0, 2.0, 300.0, 200.0),
            displays: vec![Display {
                id: 3,
                frame: Rect::new(0.0, 0.0, 1512.0, 982.0),
                scale: 2.0,
            }],
        };
        let text = serde_json::to_string(&args).unwrap();
        assert_eq!(serde_json::from_str::<BarArgs>(&text).unwrap(), args);
    }

    /// Chaque caractère non ASCII des textes de la vidéo existe dans les polices
    /// embarquées (même vérification que `ui::tests` pour les autres fenêtres).
    #[test]
    fn every_video_ui_character_has_a_glyph() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontFamily, FontId};
        let sources = [
            include_str!("bar.rs"),
            include_str!("player.rs"),
            include_str!("mod.rs"),
            include_str!("sck.rs"),
            include_str!("av.rs"),
        ];
        let mut fonts = Fonts::new(TextOptions::default(), crate::ui::theme::fonts());
        let font = FontId::new(14.0, FontFamily::Proportional);
        let mut missing = Vec::new();
        for source in sources {
            for line in source.lines() {
                let code = line.split("//").next().unwrap_or("");
                for (i, part) in code.split('"').enumerate() {
                    if i % 2 == 1 {
                        missing.extend(
                            part.chars()
                                .filter(|c| !c.is_ascii() && !fonts.has_glyph(&font, *c)),
                        );
                    }
                }
            }
        }
        assert!(missing.is_empty(), "glyphes absents : {missing:?}");
    }
}
