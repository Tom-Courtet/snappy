//! Passe au premier plan la fenêtre d'un autre processus de Snappy (les réglages,
//! déjà ouverts quand on les redemande).
//!
//! L'agent vient de recevoir le clic dans son menu : c'est lui qui a le droit de
//! donner le premier plan à un autre processus.

/// Active les fenêtres du processus `pid`.
pub fn bring_to_front(pid: u32) {
    #[cfg(target_os = "macos")]
    macos::bring_to_front(pid);
    #[cfg(windows)]
    windows::bring_to_front(pid);
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = pid;
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};

    pub fn bring_to_front(pid: u32) {
        let Ok(pid) = i32::try_from(pid) else { return };
        let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
            log::warn!("réglages : processus {pid} introuvable");
            return;
        };
        if !app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows) {
            log::warn!("réglages : le système a refusé le premier plan");
        }
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE,
        SetForegroundWindow, ShowWindow,
    };
    use windows_sys::core::BOOL;

    /// Recherche de la première fenêtre visible d'un processus.
    struct Search {
        pid: u32,
        found: HWND,
    }

    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY : `data` est le `&mut Search` passé à EnumWindows ci-dessous.
        let search = unsafe { &mut *(data as *mut Search) };
        let mut owner = 0u32;
        // SAFETY : `hwnd` vient d'EnumWindows ; `owner` est une variable locale.
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        // SAFETY : simple lecture de l'état de la fenêtre.
        if owner == search.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
            search.found = hwnd;
            return 0; // fin de l'énumération
        }
        1
    }

    pub fn bring_to_front(pid: u32) {
        let mut search = Search {
            pid,
            found: std::ptr::null_mut(),
        };
        // SAFETY : la fonction de rappel ne vit que pendant l'appel, comme `search`.
        unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
        if search.found.is_null() {
            log::warn!("réglages : fenêtre du processus {pid} introuvable");
            return;
        }
        // SAFETY : fenêtre trouvée à l'instant ; sans effet si elle vient de se fermer.
        unsafe {
            if IsIconic(search.found) != 0 {
                ShowWindow(search.found, SW_RESTORE);
            }
            SetForegroundWindow(search.found);
        }
    }
}
