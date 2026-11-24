//! Réglages système de la fenêtre de contrôle et défilement automatique.
//!
//! La fenêtre de contrôle est placée hors de la zone et exclue des captures
//! d'écran (macOS : `NSWindow.sharingType = none` ; Windows 10 2004+ :
//! `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`), au cas où elle
//! déborderait sur la zone (zone de la taille de l'écran).

use raw_window_handle::HasWindowHandle;

/// Rectangle en points globaux (origine en haut à gauche de l'écran principal).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn right(&self) -> f64 {
        self.x + self.w
    }

    fn bottom(&self) -> f64 {
        self.y + self.h
    }
}

/// Coin haut gauche de la fenêtre de contrôle (`panel` : largeur, hauteur en
/// points) : à droite de la zone si la place le permet, sinon à gauche, dessous,
/// dessus ; à défaut dans le coin bas droit de l'écran (elle chevauche alors la
/// zone, mais elle est exclue des captures). Toujours dans l'écran.
pub fn place_panel(screen: Rect, zone: Rect, panel: (f64, f64), margin: f64) -> (f64, f64) {
    let (pw, ph) = panel;
    // Barre des menus (macOS) : on garde une marge en haut.
    let top = screen.y + margin.max(32.0);
    let clamp_y = |y: f64| y.min(screen.bottom() - ph - margin).max(top);
    let clamp_x = |x: f64| x.min(screen.right() - pw - margin).max(screen.x + margin);
    if zone.right() + margin + pw + margin <= screen.right() {
        return (zone.right() + margin, clamp_y(zone.y));
    }
    if zone.x - margin - pw - margin >= screen.x {
        return (zone.x - margin - pw, clamp_y(zone.y));
    }
    if zone.bottom() + margin + ph + margin <= screen.bottom() {
        return (clamp_x(zone.right() - pw), zone.bottom() + margin);
    }
    if zone.y - margin - ph >= top {
        return (clamp_x(zone.right() - pw), zone.y - margin - ph);
    }
    (clamp_x(screen.right()), clamp_y(screen.bottom()))
}

/// Exclut la fenêtre des captures, la montre sur tous les bureaux et la place en
/// `top_left` (points globaux ; `scale` : pixels par point de l'écran visé), sans
/// prendre le focus à l'application qu'on fait défiler.
pub fn prepare(window: &impl HasWindowHandle, top_left: (f64, f64), scale: f64) {
    #[cfg(target_os = "macos")]
    {
        let _ = scale;
        macos::prepare(window, top_left);
    }
    #[cfg(windows)]
    windows::prepare(window, top_left, scale);
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (window, top_left, scale);
}

/// Le défilement automatique peut-il envoyer des événements ? Sous macOS, il faut
/// l'autorisation « Accessibilité » ; `ask` ouvre la demande du système.
pub fn can_post_events(ask: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2_core_graphics::{CGPreflightPostEventAccess, CGRequestPostEventAccess};
        CGPreflightPostEventAccess() || (ask && CGRequestPostEventAccess())
    }
    #[cfg(windows)]
    {
        let _ = ask;
        true
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = ask;
        false
    }
}

/// Un cran de défilement automatique, au point `at` (points globaux), de
/// `amount` points vers le bas (ou la droite si `horizontal`).
pub fn scroll(at: (f64, f64), amount: i32, horizontal: bool, scale: f64) {
    #[cfg(target_os = "macos")]
    {
        let _ = scale;
        macos::scroll(at, amount, horizontal);
    }
    #[cfg(windows)]
    windows::scroll(at, amount, horizontal, scale);
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (at, amount, horizontal, scale);
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSScreen, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowSharingType,
    };
    use objc2_core_foundation::CGPoint;
    use objc2_core_graphics::{CGEvent, CGEventTapLocation, CGScrollEventUnit};
    use objc2_foundation::NSPoint;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    fn ns_window(window: &impl HasWindowHandle) -> Option<Retained<NSWindow>> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(h) = handle.as_raw() else {
            return None;
        };
        // SAFETY : winit fournit un pointeur valide vers la NSView de la fenêtre,
        // vivante tant que la fenêtre existe (on est appelé pendant sa création).
        let view: &NSView = unsafe { h.ns_view.cast().as_ref() };
        view.window()
    }

    pub fn prepare(window: &impl HasWindowHandle, (x, y): (f64, f64)) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(win) = ns_window(window) else {
            log::warn!("fenêtre de contrôle introuvable");
            return;
        };
        win.setSharingType(NSWindowSharingType::None);
        win.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        let main_height = NSScreen::screens(mtm)
            .firstObject()
            .map_or(0.0, |s| s.frame().size.height);
        // AppKit : origine en bas à gauche de l'écran principal, y vers le haut.
        win.setFrameTopLeftPoint(NSPoint::new(x, main_height - y));
        win.orderFrontRegardless();
    }

    pub fn scroll((x, y): (f64, f64), amount: i32, horizontal: bool) {
        // Valeurs positives : vers le haut / la gauche ; on veut avancer.
        let (dy, dx) = if horizontal {
            (0, -amount)
        } else {
            (-amount, 0)
        };
        let Some(event) =
            CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Pixel, 2, dy, dx, 0)
        else {
            return;
        };
        CGEvent::set_location(Some(&event), CGPoint::new(x, y));
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    }
}

#[cfg(windows)]
mod windows {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::{HWND, POINT};
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_TOPMOST, PostMessageW, SWP_NOACTIVATE, SWP_NOSIZE, SetWindowDisplayAffinity,
        SetWindowPos, WDA_EXCLUDEFROMCAPTURE, WDA_MONITOR, WM_MOUSEHWHEEL, WM_MOUSEWHEEL,
        WindowFromPoint,
    };

    fn hwnd(window: &impl HasWindowHandle) -> Option<HWND> {
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
            _ => None,
        }
    }

    pub fn prepare(window: &impl HasWindowHandle, (x, y): (f64, f64), scale: f64) {
        let Some(hwnd) = hwnd(window) else {
            log::warn!("fenêtre de contrôle introuvable");
            return;
        };
        // SAFETY : `hwnd` est la fenêtre vivante que winit vient de créer.
        unsafe {
            // Windows 10 2004+ ; avant, WDA_MONITOR (fenêtre noire dans les
            // captures, ce qui reste mieux que la fenêtre elle-même).
            if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) == 0 {
                SetWindowDisplayAffinity(hwnd, WDA_MONITOR);
            }
            // Pixels physiques : point global × échelle de l'écran visé (les
            // cadres des écrans sont en pixels physiques ÷ leur échelle).
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                (x * scale).round() as i32,
                (y * scale).round() as i32,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    pub fn scroll((x, y): (f64, f64), amount: i32, horizontal: bool, scale: f64) {
        let (px, py) = ((x * scale).round() as i32, (y * scale).round() as i32);
        // SAFETY : appels sans pointeur, sur la fenêtre sous le point.
        unsafe {
            let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let target = WindowFromPoint(POINT { x: px, y: py });
            if !previous.is_null() {
                SetThreadDpiAwarenessContext(previous);
            }
            if target.is_null() {
                return;
            }
            // Un cran de molette vaut 120 ; ~3 lignes de 16 points.
            let delta = (amount * 120 / 48).clamp(-1200, 1200);
            let (msg, delta) = if horizontal {
                (WM_MOUSEHWHEEL, delta)
            } else {
                (WM_MOUSEWHEEL, -delta)
            };
            let wparam = ((delta as i16 as u16 as usize) << 16) as usize;
            let lparam = ((py as i16 as u16 as isize) << 16) | (px as i16 as u16 as isize);
            PostMessageW(target, msg, wparam, lparam);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1500.0,
        h: 1000.0,
    };

    fn zone(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn panel_goes_right_of_the_zone() {
        let p = place_panel(
            SCREEN,
            zone(100.0, 200.0, 600.0, 500.0),
            (260.0, 440.0),
            12.0,
        );
        assert_eq!(p, (712.0, 200.0));
    }

    #[test]
    fn panel_goes_left_when_the_right_is_full() {
        let p = place_panel(
            SCREEN,
            zone(800.0, 100.0, 650.0, 800.0),
            (260.0, 440.0),
            12.0,
        );
        assert_eq!(p, (528.0, 100.0));
    }

    #[test]
    fn panel_goes_below_or_above_for_wide_zones() {
        let p = place_panel(SCREEN, zone(0.0, 40.0, 1500.0, 400.0), (260.0, 440.0), 12.0);
        assert_eq!(p, (1228.0, 452.0));
        let p = place_panel(
            SCREEN,
            zone(0.0, 520.0, 1500.0, 480.0),
            (260.0, 440.0),
            12.0,
        );
        assert_eq!(p, (1228.0, 68.0));
    }

    #[test]
    fn panel_stays_on_screen_when_the_zone_fills_it() {
        let p = place_panel(SCREEN, zone(0.0, 0.0, 1500.0, 1000.0), (260.0, 440.0), 12.0);
        assert_eq!(p, (1228.0, 548.0));
        // Écran secondaire à gauche, coordonnées négatives.
        let left = Rect {
            x: -1920.0,
            y: -200.0,
            w: 1920.0,
            h: 1080.0,
        };
        let p = place_panel(left, zone(-1900.0, 0.0, 600.0, 400.0), (260.0, 440.0), 12.0);
        assert_eq!(p, (-1288.0, 0.0));
        // Zone tout en haut : la fenêtre descend sous la barre des menus.
        let p = place_panel(SCREEN, zone(100.0, 0.0, 400.0, 300.0), (260.0, 440.0), 12.0);
        assert_eq!(p, (512.0, 32.0));
    }
}
