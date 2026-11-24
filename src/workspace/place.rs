//! Position de la fenêtre : juste sous l'icône de la barre des menus (macOS), juste
//! au-dessus de l'icône de la zone de notification (Windows, barre des tâches en bas).
//!
//! L'agent passe le rectangle de l'icône par la variable d'environnement
//! [`ANCHOR_ENV`], en points logiques, origine en haut à gauche de l'écran principal.

/// Variable d'environnement du rectangle de l'icône : `x,y,largeur,hauteur`.
pub const ANCHOR_ENV: &str = "SNAPPY_WORKSPACE_ANCHOR";

/// Écart entre l'icône et la fenêtre.
const GAP: f32 = 6.0;
/// Marge minimale au bord de l'écran.
const MARGIN: f32 = 8.0;

/// Rectangle de l'icône, en points logiques.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Anchor {
    pub fn to_env(self) -> String {
        format!("{},{},{},{}", self.x, self.y, self.w, self.h)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let v: Vec<f32> = text
            .split(',')
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()?;
        match v[..] {
            [x, y, w, h] if v.iter().all(|n| n.is_finite()) && w >= 0.0 && h >= 0.0 => {
                Some(Self { x, y, w, h })
            }
            _ => None,
        }
    }

    pub fn from_env() -> Option<Self> {
        Self::parse(&std::env::var(ANCHOR_ENV).ok()?)
    }
}

/// Coin haut gauche de la fenêtre de taille `size` près de l'icône. `screen` :
/// taille de l'écran (origine 0, 0) si on la connaît, pour ne pas déborder.
///
/// Icône en haut de l'écran (barre des menus) : fenêtre dessous, centrée sur
/// l'icône. Icône plus bas (barre des tâches) : fenêtre au-dessus, son bord droit
/// aligné sur celui de l'icône.
pub fn place(anchor: Anchor, size: [f32; 2], screen: Option<[f32; 2]>) -> [f32; 2] {
    let [w, h] = size;
    let at_top = anchor.y < 120.0;
    let (mut x, mut y) = if at_top {
        (
            anchor.x + anchor.w / 2.0 - w / 2.0,
            anchor.y + anchor.h + GAP,
        )
    } else {
        (anchor.x + anchor.w - w, anchor.y - h - GAP)
    };
    // L'icône est-elle sur cet écran ? Sinon (écran secondaire), pas de bornes.
    if let Some([sw, sh]) = screen
        && anchor.x >= 0.0
        && anchor.x <= sw
        && anchor.y >= 0.0
        && anchor.y <= sh
    {
        x = x.min(sw - w - MARGIN);
        y = y.min(sh - h - MARGIN);
    }
    if anchor.x >= 0.0 {
        x = x.max(MARGIN);
    }
    if anchor.y >= 0.0 {
        y = y.max(MARGIN);
    }
    [x, y]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_round_trips_through_the_environment() {
        let a = Anchor {
            x: 1200.5,
            y: 0.0,
            w: 30.0,
            h: 24.0,
        };
        assert_eq!(Anchor::parse(&a.to_env()), Some(a));
        assert_eq!(Anchor::parse("1,2,3"), None);
        assert_eq!(Anchor::parse("1,2,x,4"), None);
        assert_eq!(Anchor::parse("1,2,-3,4"), None);
        assert_eq!(Anchor::parse("NaN,2,3,4"), None);
    }

    #[test]
    fn menu_bar_icon_opens_the_window_below() {
        let a = Anchor {
            x: 1000.0,
            y: 0.0,
            w: 30.0,
            h: 24.0,
        };
        assert_eq!(place(a, [400.0, 300.0], None), [815.0, 30.0]);
        // Près du bord droit : la fenêtre reste dans l'écran.
        let a = Anchor { x: 1420.0, ..a };
        assert_eq!(
            place(a, [400.0, 300.0], Some([1440.0, 900.0])),
            [1440.0 - 400.0 - MARGIN, 30.0]
        );
    }

    #[test]
    fn taskbar_icon_opens_the_window_above() {
        let a = Anchor {
            x: 1800.0,
            y: 1050.0,
            w: 24.0,
            h: 30.0,
        };
        assert_eq!(
            place(a, [893.0, 591.0], Some([1920.0, 1080.0])),
            [1824.0 - 893.0, 1050.0 - 591.0 - GAP]
        );
    }

    #[test]
    fn window_never_starts_off_screen_on_the_left() {
        let a = Anchor {
            x: 10.0,
            y: 0.0,
            w: 20.0,
            h: 24.0,
        };
        assert_eq!(place(a, [893.0, 591.0], None)[0], MARGIN);
    }
}
