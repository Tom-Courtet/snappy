//! Capture avec défilement : l'image se construit pendant que l'utilisateur fait
//! défiler la zone (molette, barre de défilement, clavier).
//!
//! Déroulement :
//! 1. choix de la zone sur les écrans figés (`overlay::pick`). egui n'ouvre
//!    qu'une fenêtre par processus : la séance se poursuit donc dans un nouveau
//!    processus `snappy --capture scrolling`, qui reçoit la zone par la variable
//!    d'environnement [`RECT_ENV`] (utilisable aussi à la main pour essayer :
//!    `SNAPPY_SCROLL_RECT="x,y,largeur,hauteur[,écran]"`, en points globaux) ;
//! 2. séance en direct : un fil copie la zone ~15 fois par seconde
//!    ([`grab`]) et assemble les images au fil de l'eau ([`stitch`]) ; une petite
//!    fenêtre toujours au premier plan, hors de la zone et exclue des captures
//!    ([`window`], [`native`]), montre l'aperçu, la taille, et propose
//!    Terminé / Recommencer / Annuler ;
//! 3. Terminé : l'image va dans l'historique, dans le presse-papiers, puis
//!    l'éditeur s'ouvre (comme « Écran entier »).

mod grab;
mod native;
mod stitch;
mod window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use image::RgbaImage;

use crate::capture::{Capture, Frame};
use crate::config::Store;
use crate::history;
use crate::ipc::Mode;
use crate::overlay::{self, Purpose};
use stitch::{Axis, Step, Stitcher};

/// Zone passée au processus de la séance : `x,y,largeur,hauteur[,écran]`, en
/// points globaux (origine en haut à gauche de l'écran principal) ; `écran` :
/// indice dans la capture, sinon l'écran qui contient le centre de la zone.
pub const RECT_ENV: &str = "SNAPPY_SCROLL_RECT";

/// Longueur maximale de l'image assemblée, en pixels, dans le sens du défilement.
pub const MAX_LEN: u32 = 30_000;

/// Images par seconde visées.
const TARGET_FPS: u32 = 15;

/// Plus petite zone acceptée, en pixels.
const MIN_ZONE: u32 = 32;

pub fn run(store: Store, capture: Capture) -> Result<()> {
    match std::env::var(RECT_ENV) {
        Ok(spec) => {
            let zone = Zone::from_spec(&capture, &spec)
                .with_context(|| format!("{RECT_ENV}=« {spec} »"))?;
            session(store, zone)
        }
        Err(_) => {
            let Some(picked) = overlay::pick(store, capture, false, Purpose::Scrolling)? else {
                return Ok(());
            };
            let zone = Zone {
                display_id: picked.display_id,
                frame: picked.frame,
                scale: picked.scale,
                x: picked.x,
                y: picked.y,
                width: picked.width,
                height: picked.height,
            };
            // La fenêtre de la sélection vient de se fermer : la séance dans un
            // nouveau processus.
            let exe = std::env::current_exe().context("chemin de l'exécutable")?;
            std::process::Command::new(exe)
                .args(["--capture", Mode::Scrolling.arg()])
                .env(RECT_ENV, zone.spec(picked.screen))
                .spawn()
                .context("lancement de la capture avec défilement")?;
            Ok(())
        }
    }
}

/// Zone à faire défiler, en pixels de l'image de son écran.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    pub display_id: u32,
    /// Cadre de l'écran, en points globaux.
    pub frame: Frame,
    /// Pixels par point de l'écran.
    pub scale: f64,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Zone {
    /// Zone en points globaux : x, y, largeur, hauteur.
    fn points(&self) -> native::Rect {
        native::Rect {
            x: self.frame.x + f64::from(self.x) / self.scale,
            y: self.frame.y + f64::from(self.y) / self.scale,
            w: f64::from(self.width) / self.scale,
            h: f64::from(self.height) / self.scale,
        }
    }

    /// Texte pour [`RECT_ENV`] (les `f64` s'écrivent sans perte).
    fn spec(&self, screen: usize) -> String {
        let r = self.points();
        format!("{},{},{},{},{screen}", r.x, r.y, r.w, r.h)
    }

    /// Lit [`RECT_ENV`] et retrouve l'écran dans la capture.
    fn from_spec(capture: &Capture, spec: &str) -> Result<Self> {
        let values: Vec<f64> = spec
            .split(',')
            .map(|v| v.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .context("nombres attendus : x,y,largeur,hauteur[,écran]")?;
        ensure!(
            matches!(values.len(), 4 | 5) && values.iter().all(|v| v.is_finite()),
            "4 ou 5 nombres attendus : x,y,largeur,hauteur[,écran]"
        );
        let (x, y, w, h) = (values[0], values[1], values[2], values[3]);
        ensure!(
            w > 0.0 && h > 0.0,
            "largeur et hauteur doivent être positives"
        );
        let screen = match values.get(4) {
            Some(&i) => {
                ensure!(i >= 0.0 && i.fract() == 0.0, "indice d'écran invalide");
                i as usize
            }
            None => capture
                .shots
                .iter()
                .position(|s| s.frame.contains(x + w / 2.0, y + h / 2.0))
                .unwrap_or(capture.active),
        };
        let shot = capture
            .shots
            .get(screen)
            .with_context(|| format!("pas d'écran n° {screen}"))?;
        let (iw, ih) = shot.image.dimensions();
        let s = shot.scale;
        let left = snap((x - shot.frame.x) * s, f64::floor).clamp(0.0, f64::from(iw));
        let top = snap((y - shot.frame.y) * s, f64::floor).clamp(0.0, f64::from(ih));
        let right = snap((x + w - shot.frame.x) * s, f64::ceil).clamp(0.0, f64::from(iw));
        let bottom = snap((y + h - shot.frame.y) * s, f64::ceil).clamp(0.0, f64::from(ih));
        let (width, height) = ((right - left) as u32, (bottom - top) as u32);
        if width < MIN_ZONE || height < MIN_ZONE {
            bail!("zone trop petite ou hors de l'écran ({width}x{height} px)");
        }
        Ok(Self {
            display_id: shot.display_id,
            frame: shot.frame,
            scale: s,
            x: left as u32,
            y: top as u32,
            width,
            height,
        })
    }

    /// Coin haut gauche en pixels physiques du bureau (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    fn physical_origin(&self) -> (i32, i32) {
        (
            (self.frame.x * self.scale).round() as i32 + self.x as i32,
            (self.frame.y * self.scale).round() as i32 + self.y as i32,
        )
    }
}

/// Valeur entière à l'arrondi près (aller-retour points → pixels), sinon `round`
/// (plancher ou plafond, pour englober).
fn snap(v: f64, round: fn(f64) -> f64) -> f64 {
    if (v - v.round()).abs() < 1e-6 {
        v.round()
    } else {
        round(v)
    }
}

/// État de la séance, partagé entre le fil de copie et la fenêtre.
#[derive(Default)]
pub struct Shared {
    pub state: Mutex<State>,
    /// Demandes de la fenêtre au fil de copie.
    pub stop: AtomicBool,
    pub restart: AtomicBool,
    pub auto: AtomicBool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// Pas encore de mouvement.
    #[default]
    Waiting,
    Running,
    /// Raccord perdu depuis un moment.
    Lost,
    /// Longueur maximale atteinte.
    Full,
}

#[derive(Default)]
pub struct State {
    pub status: Status,
    /// Taille de l'image assemblée.
    pub size: Option<(u32, u32)>,
    pub axis: Option<Axis>,
    /// Aperçu réduit et son numéro (change à chaque nouvel aperçu).
    pub preview: Option<Arc<RgbaImage>>,
    pub preview_gen: u64,
    /// Partie visible en ce moment, dans le sens du défilement (pixels de l'image).
    pub view: Option<(u32, u32)>,
    /// Images copiées par seconde et durée moyenne d'une copie.
    pub fps: f32,
    pub grab_ms: f32,
    pub error: Option<String>,
    /// Message du défilement automatique (autorisation, fin de page).
    pub auto_note: Option<String>,
}

/// La séance : fenêtre de contrôle, puis enregistrement si Terminé.
fn session(store: Store, zone: Zone) -> Result<()> {
    log::info!(
        "capture avec défilement : zone {}x{} px en {},{} (écran {})",
        zone.width,
        zone.height,
        zone.x,
        zone.y,
        zone.display_id
    );
    let shared = Arc::new(Shared::default());
    let (decision, stitcher) = window::run(zone, Arc::clone(&shared))?;
    let (Some(stitcher), window::Decision::Done) = (stitcher, decision) else {
        return Ok(());
    };
    let Some(img) = stitcher.image() else {
        return Ok(());
    };
    let path = history::save_capture(&store.captures_dir(), &img)?;
    log::info!(
        "capture avec défilement : {}x{} px → {}",
        img.width(),
        img.height(),
        path.display()
    );
    if let Err(e) = history::copy_image(&img) {
        log::warn!("{e:#}");
    }
    // La fenêtre de contrôle a pris la seule fenêtre egui de ce processus.
    crate::open_editor(&path)
}

/// Mesure des images par seconde sur une fenêtre glissante d'une seconde.
struct Meter {
    times: std::collections::VecDeque<Instant>,
    grab_total: Duration,
    grabs: u32,
}

impl Meter {
    fn new() -> Self {
        Self {
            times: std::collections::VecDeque::new(),
            grab_total: Duration::ZERO,
            grabs: 0,
        }
    }

    fn tick(&mut self, grab: Duration) -> (f32, f32) {
        let now = Instant::now();
        self.times.push_back(now);
        while self
            .times
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(1))
        {
            self.times.pop_front();
        }
        self.grab_total += grab;
        self.grabs += 1;
        let avg = self.grab_total.as_secs_f32() * 1000.0 / self.grabs as f32;
        (self.times.len() as f32, avg)
    }
}

/// Fil de copie : copie la zone, assemble, publie l'aperçu, jusqu'à `stop`.
pub fn worker(zone: Zone, shared: Arc<Shared>, repaint: impl Fn()) -> Stitcher {
    let mut stitcher = Stitcher::new(MAX_LEN);
    let mut grabber = grab::Grabber::new(zone);
    let period = Duration::from_secs(1) / TARGET_FPS;
    let mut meter = Meter::new();
    let mut lost_since: Option<Instant> = None;
    let mut last_preview: Option<Instant> = None;
    let mut dirty = false;
    let mut auto = AutoScroll::new(zone);
    let mut frames = 0u64;
    let started = Instant::now();
    while !shared.stop.load(Ordering::Relaxed) {
        let t0 = Instant::now();
        if shared.restart.swap(false, Ordering::Relaxed) {
            stitcher.reset();
            lost_since = None;
            dirty = true;
        }
        let frame = match grabber.grab() {
            Ok(frame) => frame,
            Err(e) => {
                shared.state.lock().expect("état").error = Some(format!("{e:#}"));
                repaint();
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        let grab_time = t0.elapsed();
        frames += 1;
        let step = stitcher.push(frame);
        match step {
            Step::First | Step::Moved { .. } => {
                lost_since = None;
                dirty = true;
            }
            Step::Lost => {
                lost_since.get_or_insert(t0);
            }
            Step::Still | Step::Full => {}
        }
        let note = if shared.auto.load(Ordering::Relaxed) {
            auto.tick(step, stitcher.axis(), &shared)
        } else {
            auto.idle();
            None
        };
        let preview_due = last_preview.is_none_or(|t| t.elapsed() >= Duration::from_millis(150));
        let preview = (dirty && preview_due)
            .then(|| {
                dirty = false;
                last_preview = Some(Instant::now());
                let (w, h) = match stitcher.axis() {
                    Some(Axis::Horizontal) => (4096, 400),
                    _ => (400, 4096),
                };
                stitcher.preview(w, h).map(Arc::new)
            })
            .flatten();
        let (fps, grab_ms) = meter.tick(grab_time);
        {
            let mut st = shared.state.lock().expect("état");
            st.status = if stitcher.is_full() {
                Status::Full
            } else if lost_since.is_some_and(|t| t.elapsed() >= Duration::from_millis(400)) {
                Status::Lost
            } else if stitcher.axis().is_some() {
                Status::Running
            } else {
                Status::Waiting
            };
            st.size = stitcher.size();
            st.axis = stitcher.axis();
            st.view = stitcher.view();
            st.fps = fps;
            st.grab_ms = grab_ms;
            st.error = None;
            if let Some(note) = note {
                st.auto_note = Some(note);
            }
            if let Some(p) = preview {
                st.preview = Some(p);
                st.preview_gen += 1;
            }
        }
        repaint();
        std::thread::sleep(period.saturating_sub(t0.elapsed()));
    }
    let secs = started.elapsed().as_secs_f64();
    log::info!(
        "capture avec défilement : {frames} images en {secs:.1} s ({:.1} im/s, copie {:.1} ms en moyenne)",
        frames as f64 / secs.max(0.001),
        meter.grab_total.as_secs_f64() * 1000.0 / f64::from(meter.grabs.max(1)),
    );
    stitcher
}

/// Défilement automatique (désactivé par défaut) : un cran de molette simulé à
/// chaque image, au centre de la zone ; s'arrête seul au bout de la page.
struct AutoScroll {
    zone: Zone,
    /// Images sans mouvement depuis le dernier cran.
    still: u32,
    allowed: Option<bool>,
}

impl AutoScroll {
    fn new(zone: Zone) -> Self {
        Self {
            zone,
            still: 0,
            allowed: None,
        }
    }

    fn idle(&mut self) {
        self.still = 0;
    }

    /// Un cran ; rend un message pour la fenêtre s'il y a lieu.
    fn tick(&mut self, step: Step, axis: Option<Axis>, shared: &Shared) -> Option<String> {
        let allowed = *self
            .allowed
            .get_or_insert_with(|| native::can_post_events(true));
        if !allowed {
            shared.auto.store(false, Ordering::Relaxed);
            self.allowed = None;
            return Some(
                "Défilement automatique : autorisez Snappy dans Réglages Système > \
                 Confidentialité et sécurité > Accessibilité."
                    .to_owned(),
            );
        }
        match step {
            Step::Still => self.still += 1,
            Step::Full => self.still = u32::MAX,
            _ => self.still = 0,
        }
        // ~1 s sans mouvement malgré les crans : bout de la page.
        if self.still >= TARGET_FPS {
            shared.auto.store(false, Ordering::Relaxed);
            self.still = 0;
            return Some("Défilement automatique : fin de la page atteinte.".to_owned());
        }
        if step == Step::Lost {
            return None; // trop vite : on attend que ça raccorde
        }
        let r = self.zone.points();
        let horizontal = axis == Some(Axis::Horizontal);
        // Un quart de la zone par cran : assez de recouvrement pour raccorder.
        let amount = ((if horizontal { r.w } else { r.h }) / 4.0).max(10.0) as i32;
        native::scroll(
            (r.x + r.w / 2.0, r.y + r.h / 2.0),
            amount,
            horizontal,
            self.zone.scale,
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Shot;

    fn capture() -> Capture {
        let shot = |x: f64, id: u32, scale: f64, w: f64, h: f64| Shot {
            image: RgbaImage::new((w * scale) as u32, (h * scale) as u32),
            frame: Frame {
                x,
                y: 0.0,
                width: w,
                height: h,
            },
            scale,
            display_id: id,
            windows: Vec::new(),
            cursor: None,
        };
        Capture {
            shots: vec![
                shot(0.0, 1, 2.0, 1512.0, 982.0),
                shot(1512.0, 7, 1.25, 1536.0, 864.0),
            ],
            active: 0,
        }
    }

    #[test]
    fn spec_round_trips() {
        let cap = capture();
        let zone = Zone::from_spec(&cap, "100,50,600,400").unwrap();
        assert_eq!(
            (zone.display_id, zone.x, zone.y, zone.width, zone.height),
            (1, 200, 100, 1200, 800)
        );
        assert_eq!(Zone::from_spec(&cap, &zone.spec(0)).unwrap(), zone);
        // Écran à 125 % : des points non entiers.
        let zone = Zone {
            display_id: 7,
            frame: cap.shots[1].frame,
            scale: 1.25,
            x: 333,
            y: 101,
            width: 777,
            height: 555,
        };
        assert_eq!(Zone::from_spec(&cap, &zone.spec(1)).unwrap(), zone);
    }

    #[test]
    fn spec_picks_the_screen_of_the_center() {
        let cap = capture();
        let zone = Zone::from_spec(&cap, "1600, 100, 400, 300").unwrap();
        assert_eq!(zone.display_id, 7);
        assert_eq!(
            (zone.x, zone.y, zone.width, zone.height),
            (110, 125, 500, 375)
        );
        // Débordement : rogné à l'écran.
        let zone = Zone::from_spec(&cap, "1400,900,400,300,0").unwrap();
        assert_eq!(
            (zone.x, zone.y, zone.width, zone.height),
            (2800, 1800, 224, 164)
        );
    }

    #[test]
    fn bad_specs_are_refused() {
        let cap = capture();
        for spec in [
            "",
            "1,2,3",
            "a,b,c,d",
            "0,0,-5,10",
            "0,0,10,10",
            "0,0,100,100,9",
            "0,0,100,100,0.5",
            "5000,0,100,100,0",
        ] {
            assert!(Zone::from_spec(&cap, spec).is_err(), "{spec}");
        }
    }

    #[test]
    fn physical_origin_adds_the_screen_offset() {
        let cap = capture();
        let zone = Zone::from_spec(&cap, "1600,100,400,300").unwrap();
        assert_eq!(zone.physical_origin(), (1890 + 110, 125));
    }

    /// Assemble les images PNG d'un dossier (ordre alphabétique), par exemple des
    /// captures d'un vrai navigateur à plusieurs positions de défilement, et
    /// enregistre le résultat :
    /// `SNAPPY_SCROLL_FRAMES=dossier SNAPPY_SCROLL_OUT=sortie.png cargo test
    /// stitches_frames_from_a_folder -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn stitches_frames_from_a_folder() {
        let dir = std::env::var("SNAPPY_SCROLL_FRAMES").expect("SNAPPY_SCROLL_FRAMES");
        let mut paths: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "png"))
            .collect();
        paths.sort();
        let mut s = Stitcher::new(MAX_LEN);
        for p in &paths {
            let step = s.push(history::read_png(p).unwrap());
            eprintln!("{} : {step:?}", p.display());
        }
        let img = s.image().unwrap();
        eprintln!("résultat {}x{} ({:?})", img.width(), img.height(), s.axis());
        if let Ok(out) = std::env::var("SNAPPY_SCROLL_OUT") {
            history::write_png(std::path::Path::new(&out), &img).unwrap();
        }
    }

    /// Boucle de copie réelle pendant 3 s, sans fenêtre : images par seconde
    /// atteintes (écran allumé, autorisation « Enregistrement de l'écran »).
    /// `cargo test worker_fps -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn worker_fps() {
        let cap = crate::capture::capture_all().expect("capture");
        let shot = &cap.shots[cap.active];
        let (w, h) = shot.image.dimensions();
        for (zw, zh) in [(w / 2, h / 2), (w, h)] {
            let zone = Zone {
                display_id: shot.display_id,
                frame: shot.frame,
                scale: shot.scale,
                x: 0,
                y: 0,
                width: zw,
                height: zh,
            };
            let shared = Arc::new(Shared::default());
            let thread_shared = Arc::clone(&shared);
            let worker = std::thread::spawn(move || worker(zone, thread_shared, || {}));
            std::thread::sleep(Duration::from_secs(3));
            let (fps, grab_ms) = {
                let st = shared.state.lock().unwrap();
                (st.fps, st.grab_ms)
            };
            shared.stop.store(true, Ordering::Relaxed);
            worker.join().unwrap();
            eprintln!("zone {zw}x{zh} px : {fps} im/s, copie {grab_ms:.1} ms");
            assert!(fps >= 5.0);
        }
    }
}
