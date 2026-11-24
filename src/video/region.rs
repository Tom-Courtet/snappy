//! Zone enregistrée : rectangle en points globaux, écrans, tailles prédéfinies et
//! mémoire de la dernière zone (`last_video_region.json`).

#![cfg_attr(not(target_os = "macos"), allow(dead_code))] // enregistrement propre à macOS

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{Store, write_atomic};

const LAST_FILE: &str = "last_video_region.json";
/// Plus petite zone enregistrable, en points.
pub const MIN_SIZE: f64 = 16.0;

/// Rectangle en points, coordonnées globales CoreGraphics (origine en haut à gauche
/// de l'écran principal, y vers le bas), comme `capture::Frame`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn contains_point(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn contains(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right() + 1e-6
            && other.bottom() <= self.bottom() + 1e-6
    }

    /// Partie commune, `None` si elle est vide.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        (r > x && b > y).then(|| Rect::new(x, y, r - x, b - y))
    }

    fn area(&self) -> f64 {
        self.width.max(0.0) * self.height.max(0.0)
    }

    /// `"x,y,largeur,hauteur"` (variable `SNAPPY_VIDEO_RECT`, arguments).
    pub fn parse(text: &str) -> Option<Rect> {
        let v: Vec<f64> = text
            .split(',')
            .map(|p| p.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .ok()?;
        let [x, y, w, h] = v[..] else { return None };
        (w > 0.0 && h > 0.0 && [x, y, w, h].iter().all(|v| v.is_finite()))
            .then(|| Rect::new(x, y, w, h))
    }
}

/// Un écran : identifiant (`CGDirectDisplayID`), cadre en points, pixels par point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Display {
    pub id: u32,
    pub frame: Rect,
    pub scale: f64,
}

/// Écran de la zone : celui qui contient son centre, sinon celui qu'elle recouvre
/// le plus.
pub fn display_for(rect: &Rect, displays: &[Display]) -> Option<usize> {
    let (cx, cy) = rect.center();
    displays
        .iter()
        .position(|d| d.frame.contains_point(cx, cy))
        .or_else(|| {
            displays
                .iter()
                .enumerate()
                .filter_map(|(i, d)| Some((i, d.frame.intersect(rect)?.area())))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        })
}

/// Zone rognée à son écran et calée sur la grille des pixels de l'écran
/// (ScreenCaptureKit n'enregistre qu'un écran à la fois). `None` si trop petite.
pub fn fit(rect: &Rect, display: &Display) -> Option<Rect> {
    let r = rect.intersect(&display.frame)?;
    let s = display.scale.max(0.1);
    let snap = |v: f64, origin: f64| ((v - origin) * s).round() / s + origin;
    let x0 = snap(r.x, display.frame.x);
    let y0 = snap(r.y, display.frame.y);
    let x1 = snap(r.right(), display.frame.x);
    let y1 = snap(r.bottom(), display.frame.y);
    let out = Rect::new(x0, y0, x1 - x0, y1 - y0);
    (out.width >= MIN_SIZE && out.height >= MIN_SIZE).then_some(out)
}

/// Taille prédéfinie de la vidéo, en pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub label: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Tailles de la barre vidéo de Screenpresso (§ 13.5).
pub const PRESETS: &[Preset] = &[
    Preset {
        label: "VGA",
        width: 640,
        height: 480,
    },
    Preset {
        label: "SVGA",
        width: 800,
        height: 600,
    },
    Preset {
        label: "XGA",
        width: 1024,
        height: 768,
    },
    Preset {
        label: "HD",
        width: 1280,
        height: 720,
    },
    Preset {
        label: "Full HD",
        width: 1920,
        height: 1080,
    },
    Preset {
        label: "QHD",
        width: 2560,
        height: 1440,
    },
    Preset {
        label: "4K UHD",
        width: 3840,
        height: 2160,
    },
];

/// Zone de `width` × `height` **pixels** de l'écran, centrée sur la zone actuelle et
/// décalée pour tenir dans l'écran. `None` si l'écran est trop petit.
pub fn apply_preset(current: &Rect, display: &Display, width: u32, height: u32) -> Option<Rect> {
    let s = display.scale.max(0.1);
    let (w, h) = (f64::from(width) / s, f64::from(height) / s);
    let f = &display.frame;
    if w > f.width + 1e-6 || h > f.height + 1e-6 {
        return None;
    }
    let (cx, cy) = current.center();
    let x = (cx - w / 2.0).clamp(f.x, f.right() - w);
    let y = (cy - h / 2.0).clamp(f.y, f.bottom() - h);
    fit(&Rect::new(x, y, w, h), display)
}

/// Position de la barre (taille `bar` en points) : sous la zone, sinon au-dessus,
/// sinon dans la zone en bas ; toujours dans l'écran.
pub fn bar_position(rect: &Rect, display: &Rect, bar: (f64, f64)) -> (f64, f64) {
    const GAP: f64 = 10.0;
    let (bw, bh) = bar;
    let x = (rect.center().0 - bw / 2.0).clamp(display.x, (display.right() - bw).max(display.x));
    let y = if rect.bottom() + GAP + bh <= display.bottom() {
        rect.bottom() + GAP
    } else if rect.y - GAP - bh >= display.y {
        rect.y - GAP - bh
    } else {
        (rect.bottom() - GAP - bh).max(display.y)
    };
    (x, y)
}

/// Dernière zone vidéo, avec l'écran qui la contenait.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LastVideoRegion {
    pub display: Display,
    pub rect: Rect,
}

fn last_path(store: &Store) -> PathBuf {
    store
        .config_path()
        .parent()
        .map(|d| d.join(LAST_FILE))
        .unwrap_or_else(|| PathBuf::from(LAST_FILE))
}

/// Dernière zone, si son écran est toujours là, au même endroit et à la même
/// échelle.
pub fn load_last(store: &Store, displays: &[Display]) -> Option<Rect> {
    let text = std::fs::read_to_string(last_path(store)).ok()?;
    let last: LastVideoRegion = serde_json::from_str(&text).ok()?;
    let display = displays.iter().find(|d| **d == last.display)?;
    display
        .frame
        .contains(&last.rect)
        .then_some(last.rect)
        .and_then(|r| fit(&r, display))
}

pub fn save_last(store: &Store, region: &LastVideoRegion) {
    let result = serde_json::to_vec(region)
        .map_err(anyhow::Error::from)
        .and_then(|bytes| write_atomic(&last_path(store), &bytes));
    if let Err(e) = result {
        log::warn!("zone vidéo non mémorisée : {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retina() -> Display {
        Display {
            id: 1,
            frame: Rect::new(0.0, 0.0, 1512.0, 982.0),
            scale: 2.0,
        }
    }

    #[test]
    fn parse_accepts_four_numbers() {
        assert_eq!(
            Rect::parse("10, 20,300,200"),
            Some(Rect::new(10.0, 20.0, 300.0, 200.0))
        );
        assert_eq!(
            Rect::parse("-1512,0,10.5,20"),
            Some(Rect::new(-1512.0, 0.0, 10.5, 20.0))
        );
        assert_eq!(Rect::parse("1,2,3"), None);
        assert_eq!(Rect::parse("1,2,0,4"), None);
        assert_eq!(Rect::parse("a,b,c,d"), None);
    }

    #[test]
    fn the_display_is_the_one_under_the_center() {
        let second = Display {
            id: 2,
            frame: Rect::new(1512.0, 0.0, 1920.0, 1080.0),
            scale: 1.0,
        };
        let displays = [retina(), second];
        let r = Rect::new(1400.0, 100.0, 400.0, 100.0);
        assert_eq!(display_for(&r, &displays), Some(1));
        let outside = Rect::new(-50.0, -50.0, 60.0, 60.0);
        assert_eq!(display_for(&outside, &displays), Some(0));
        assert_eq!(
            display_for(&Rect::new(-500.0, -500.0, 10.0, 10.0), &displays),
            None
        );
    }

    #[test]
    fn fit_clips_and_snaps_to_pixels() {
        let d = retina();
        let r = fit(&Rect::new(-10.0, 10.3, 100.2, 50.0), &d).unwrap();
        assert_eq!(r.x, 0.0);
        assert_eq!(r.y, 10.5);
        assert_eq!(r.width, 90.0);
        assert!((r.height - 50.0).abs() < 0.51);
        assert_eq!(fit(&Rect::new(0.0, 0.0, 5.0, 100.0), &d), None);
    }

    #[test]
    fn presets_are_centered_and_kept_on_screen() {
        let d = retina();
        let current = Rect::new(100.0, 100.0, 200.0, 100.0);
        // 1280 × 720 pixels = 640 × 360 points sur un écran Retina.
        let r = apply_preset(&current, &d, 1280, 720).unwrap();
        assert_eq!((r.width, r.height), (640.0, 360.0));
        assert_eq!((r.x, r.y), (0.0, 0.0), "poussée dans l'écran");
        let centered = apply_preset(&Rect::new(700.0, 400.0, 100.0, 100.0), &d, 640, 480).unwrap();
        assert_eq!(centered.center(), (750.0, 450.0));
        assert_eq!(apply_preset(&current, &d, 3840, 2160), None);
        assert!(
            apply_preset(&current, &d, 3024, 1964).is_some(),
            "plein écran"
        );
    }

    #[test]
    fn the_bar_goes_below_then_above_then_inside() {
        let screen = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let bar = (400.0, 50.0);
        let (x, y) = bar_position(&Rect::new(300.0, 100.0, 400.0, 300.0), &screen, bar);
        assert_eq!((x, y), (300.0, 410.0));
        let (_, y) = bar_position(&Rect::new(300.0, 500.0, 400.0, 280.0), &screen, bar);
        assert_eq!(y, 440.0, "au-dessus");
        let (x, y) = bar_position(&Rect::new(0.0, 0.0, 1000.0, 800.0), &screen, bar);
        assert_eq!((x, y), (300.0, 740.0), "dans la zone");
        let (x, _) = bar_position(&Rect::new(-100.0, 0.0, 100.0, 100.0), &screen, bar);
        assert_eq!(x, 0.0);
    }

    #[test]
    fn last_region_needs_the_same_display() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let d = retina();
        assert_eq!(load_last(&store, &[d]), None);
        let rect = Rect::new(10.0, 20.0, 300.0, 200.0);
        save_last(&store, &LastVideoRegion { display: d, rect });
        assert_eq!(load_last(&store, &[d]), Some(rect));
        let moved = Display { scale: 1.0, ..d };
        assert_eq!(load_last(&store, &[moved]), None);
    }
}
