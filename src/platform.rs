//! Réglages de fenêtre que winit et eframe n'exposent pas.
//!
//! La sélection de zone doit couvrir **tout** l'écran, barre des menus et Dock
//! compris, et passer au-dessus des apps en plein écran. Avec winit, une fenêtre
//! « toujours au premier plan » reste sous la barre des menus : on monte donc la
//! fenêtre native au niveau de l'économiseur d'écran (`NSScreenSaverWindowLevel`) et
//! on lui donne exactement le cadre de l'écran.
//!
//! Sous Windows, winit place les fenêtres en points selon l'échelle de l'écran
//! principal : avec des écrans d'échelles différentes, la surcouche tomberait à côté.
//! On la place donc en pixels physiques, au premier plan absolu (`HWND_TOPMOST`).

use raw_window_handle::HasWindowHandle;

/// Cadre d'un écran en points, coordonnées globales CoreGraphics (origine en haut à
/// gauche de l'écran principal, y vers le bas), comme `capture::Frame`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Cadre AppKit (origine en bas à gauche de l'écran principal, y vers le haut) d'un
/// cadre CoreGraphics. `main_height` : hauteur de l'écran principal, en points.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // propre à macOS
pub fn cg_to_appkit(r: ScreenRect, main_height: f64) -> ScreenRect {
    ScreenRect {
        y: main_height - (r.y + r.height),
        ..r
    }
}

/// Place la fenêtre en surcouche plein écran sur `frame` (en points ; `scale` :
/// pixels par point de cet écran) et lui donne le clavier.
pub fn make_overlay(window: &impl HasWindowHandle, frame: ScreenRect, scale: f64) {
    #[cfg(target_os = "macos")]
    {
        let _ = scale;
        macos::make_overlay(window, frame);
    }
    #[cfg(windows)]
    windows::make_overlay(window, frame, scale);
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (window, frame, scale);
}

/// Même chose que [`make_overlay`] pour une fenêtre secondaire d'egui (un écran de
/// plus), retrouvée par son titre parmi les fenêtres de l'application. Ne lui donne
/// pas le clavier. `false` si la fenêtre n'existe pas encore (à réessayer).
pub fn place_overlay_titled(title: &str, frame: ScreenRect, scale: f64) -> bool {
    #[cfg(target_os = "macos")]
    {
        let _ = scale;
        macos::place_overlay_titled(title, frame)
    }
    #[cfg(windows)]
    {
        windows::place_overlay_titled(title, frame, scale)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (title, frame, scale);
        true
    }
}

/// Passe l'application au premier plan (une app lancée par l'agent n'a pas le focus)
/// et déclare la fenêtre en sRGB, comme les captures.
pub fn activate(window: &impl HasWindowHandle) {
    #[cfg(target_os = "macos")]
    macos::activate(window);
    #[cfg(windows)]
    windows::activate(window);
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = window;
}

/// macOS : icône dans le Dock et place dans ⌘Tab tant que cette fenêtre (ce
/// processus) est ouverte. Snappy est une app de barre des menus (`LSUIElement`) :
/// sans cet appel, ses fenêtres n'y apparaissent pas et sont difficiles à retrouver.
/// L'icône disparaît avec le processus, à la fermeture de la fenêtre.
pub fn show_in_dock() {
    #[cfg(target_os = "macos")]
    macos::show_in_dock();
}

/// Autorise les processus lancés ensuite (la capture) à passer au premier plan.
/// Windows ne le permet qu'au processus qui a reçu la dernière entrée : ici l'agent,
/// qui vient de recevoir le raccourci.
pub fn allow_foreground() {
    #[cfg(windows)]
    // SAFETY : simple autorisation, sans pointeur.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        );
    }
}

#[cfg(windows)]
mod windows {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, GetWindowTextW, HWND_TOPMOST, SET_WINDOW_POS_FLAGS, SWP_NOACTIVATE,
        SWP_SHOWWINDOW, SetForegroundWindow, SetWindowPos,
    };
    use windows_sys::core::BOOL;

    use super::ScreenRect;

    fn hwnd(window: &impl HasWindowHandle) -> Option<HWND> {
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
            _ => None,
        }
    }

    pub fn make_overlay(window: &impl HasWindowHandle, frame: ScreenRect, scale: f64) {
        let Some(hwnd) = hwnd(window) else {
            log::warn!("surcouche : fenêtre native introuvable");
            return;
        };
        place(hwnd, frame, scale, SWP_SHOWWINDOW);
        // SAFETY : `hwnd` est la fenêtre vivante que winit vient de créer.
        unsafe { SetForegroundWindow(hwnd) };
    }

    /// Au premier plan absolu, en pixels physiques.
    fn place(hwnd: HWND, frame: ScreenRect, scale: f64, flags: SET_WINDOW_POS_FLAGS) {
        let px = |v: f64| (v * scale).round() as i32;
        // SAFETY : `hwnd` est une fenêtre vivante de ce processus.
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                px(frame.x),
                px(frame.y),
                px(frame.width),
                px(frame.height),
                flags,
            );
        }
    }

    pub fn place_overlay_titled(title: &str, frame: ScreenRect, scale: f64) -> bool {
        unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
            // SAFETY : `data` est le `&mut Vec` passé à EnumThreadWindows ci-dessous.
            let list = unsafe { &mut *(data as *mut Vec<HWND>) };
            list.push(hwnd);
            1
        }
        let mut windows: Vec<HWND> = Vec::new();
        // SAFETY : les fenêtres de winit vivent sur ce thread ; la fonction de rappel
        // ne vit que pendant l'appel, comme `windows`.
        unsafe {
            EnumThreadWindows(
                GetCurrentThreadId(),
                Some(collect),
                &mut windows as *mut _ as LPARAM,
            )
        };
        let wanted: Vec<u16> = title.encode_utf16().collect();
        let found = windows.into_iter().find(|&hwnd| {
            let mut buf = [0u16; 256];
            // SAFETY : tampon local de 256 caractères, taille passée à l'API.
            let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
            n > 0 && buf[..n as usize] == wanted[..]
        });
        match found {
            Some(hwnd) => {
                place(hwnd, frame, scale, SWP_SHOWWINDOW | SWP_NOACTIVATE);
                true
            }
            None => false,
        }
    }

    pub fn activate(window: &impl HasWindowHandle) {
        if let Some(hwnd) = hwnd(window) {
            // SAFETY : fenêtre vivante, créée par winit.
            unsafe { SetForegroundWindow(hwnd) };
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSColorSpace, NSScreen, NSScreenSaverWindowLevel, NSView, NSWindow,
        NSWindowCollectionBehavior,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::{ScreenRect, cg_to_appkit};

    pub fn show_in_dock() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        NSApplication::sharedApplication(mtm)
            .setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Regular);
    }

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

    /// Les captures sont converties en sRGB : sans espace déclaré, macOS afficherait
    /// ces valeurs telles quelles sur un écran P3, et l'écran figé paraîtrait terne.
    fn srgb(win: &NSWindow) {
        win.setColorSpace(Some(&NSColorSpace::sRGBColorSpace()));
    }

    pub fn make_overlay(window: &impl HasWindowHandle, frame: ScreenRect) {
        let Some(mtm) = MainThreadMarker::new() else {
            log::warn!("surcouche : pas sur le thread principal");
            return;
        };
        let Some(win) = ns_window(window) else {
            log::warn!("surcouche : fenêtre native introuvable");
            return;
        };
        place(&win, frame, mtm);
        win.makeKeyAndOrderFront(None);
        NSApplication::sharedApplication(mtm).activate();
    }

    pub fn place_overlay_titled(title: &str, frame: ScreenRect) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let windows = NSApplication::sharedApplication(mtm).windows();
        let found = windows.iter().find(|w| w.title().to_string() == title);
        match found {
            Some(win) => {
                place(&win, frame, mtm);
                win.orderFrontRegardless();
                true
            }
            None => false,
        }
    }

    /// Niveau économiseur d'écran, tous les bureaux, cadre exact de l'écran.
    fn place(win: &NSWindow, frame: ScreenRect, mtm: MainThreadMarker) {
        let main_height = NSScreen::screens(mtm)
            .firstObject()
            .map(|s| s.frame().size.height)
            .unwrap_or(frame.height);
        let r = cg_to_appkit(frame, main_height);
        win.setLevel(NSScreenSaverWindowLevel);
        srgb(win);
        win.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        win.setHasShadow(false);
        win.setFrame_display(
            NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.width, r.height)),
            true,
        );
    }

    pub fn activate(window: &impl HasWindowHandle) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if let Some(win) = ns_window(window) {
            srgb(&win);
            win.makeKeyAndOrderFront(None);
        }
        NSApplication::sharedApplication(mtm).activate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appkit_frames_are_flipped() {
        // Écran principal 1512×982 ; second écran au-dessus, décalé à droite.
        let main = ScreenRect {
            x: 0.0,
            y: 0.0,
            width: 1512.0,
            height: 982.0,
        };
        assert_eq!(cg_to_appkit(main, 982.0), main);
        let above = ScreenRect {
            x: 200.0,
            y: -1080.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert_eq!(
            cg_to_appkit(above, 982.0),
            ScreenRect {
                x: 200.0,
                y: 982.0,
                width: 1920.0,
                height: 1080.0
            }
        );
    }
}
