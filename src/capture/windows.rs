//! Windows : GDI (`BitBlt` depuis l'écran), disponible partout depuis Windows XP,
//! sans autorisation ni bordure jaune (contrairement à Windows.Graphics.Capture).
//!
//! Coordonnées : le processus se déclare « DPI par écran » avant toute mesure, et le
//! fil qui mesure passe en « DPI par écran v2 » le temps de la mesure
//! ([`PhysicalPixels`]), pour lire de vrais pixels même si un autre réglage a été
//! posé avant nous. Tout est alors en pixels physiques du bureau virtuel, la seule
//! échelle commune à tous les écrans : écrans, fenêtres, curseur et copie GDI
//! parlent la même langue, et passer en pixels de l'image d'un écran n'est qu'une
//! translation, même avec des échelles mélangées (100 %, 125 %, 150 %…). Les cadres
//! sont rendus en points (pixels ÷ échelle de l'écran, comme sous macOS).
//!
//! [`capture_all`] fige tout le bureau d'une seule copie GDI, découpée ensuite par
//! écran ; la liste des fenêtres et le curseur sont lus juste autour de la copie.

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use image::RgbaImage;
use image::imageops::crop_imm;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    EnumDisplayMonitors, GdiFlush, GetDC, GetDIBits, GetMonitorInfoW, GetObjectW, HBITMAP, HDC,
    HMONITOR, MONITORINFO, MONITORINFOEXW, ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor,
    MDT_EFFECTIVE_DPI, SetProcessDpiAwarenessContext, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, EnumWindows, FindWindowExW, GWL_EXSTYLE,
    GetClassNameW, GetCursorInfo, GetCursorPos, GetIconInfo, GetWindowLongW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HCURSOR, ICONINFO, IsIconic,
    IsWindowVisible, MONITORINFOF_PRIMARY, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};
use windows_sys::core::BOOL;

use super::{Capture, CursorPatch, Frame, Shot, WindowRect};

/// À appeler avant toute mesure. Échoue sans conséquence si c'est déjà fait
/// (winit le refait plus tard de son côté).
fn dpi_aware() {
    // SAFETY : simple réglage du processus, sans pointeur.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// Tant qu'il vit, le fil courant mesure en pixels physiques (DPI par écran v2),
/// quel que soit le réglage du processus ; l'ancien réglage du fil revient à la fin.
struct PhysicalPixels(DPI_AWARENESS_CONTEXT);

impl PhysicalPixels {
    fn enter() -> Self {
        dpi_aware();
        // SAFETY : simple réglage du fil courant, sans pointeur. Renvoie l'ancien
        // contexte, ou nul en cas d'échec (Windows antérieur à 10 1703).
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for PhysicalPixels {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY : contexte renvoyé par Windows juste avant, sur ce même fil.
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

/// Rectangle en pixels physiques, bords gauche et haut inclus, droit et bas exclus
/// (comme `RECT`), sans dépendre de Windows pour les tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct PxRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl PxRect {
    fn new(left: i32, top: i32, width: i32, height: i32) -> Self {
        Self {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    fn width(&self) -> i32 {
        self.right - self.left
    }

    fn height(&self) -> i32 {
        self.bottom - self.top
    }

    fn is_empty(&self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    /// Partie commune, `None` si elle est vide (deux rectangles qui se touchent
    /// seulement par un bord n'ont rien en commun).
    fn intersect(&self, other: &Self) -> Option<Self> {
        let r = Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (!r.is_empty()).then_some(r)
    }

    /// Plus petit rectangle qui contient les deux.
    fn union(&self, other: &Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

impl From<RECT> for PxRect {
    fn from(r: RECT) -> Self {
        Self {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
}

/// Position du curseur, en pixels physiques du bureau virtuel.
pub fn cursor_position() -> Option<(f64, f64)> {
    let _px = PhysicalPixels::enter();
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY : `p` est un POINT valide et modifiable.
    (unsafe { GetCursorPos(&mut p) } != 0).then(|| (f64::from(p.x), f64::from(p.y)))
}

/// Un écran : rectangle physique, échelle (DPI ÷ 96) et identifiant stable.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Screen {
    rect: PxRect,
    scale: f64,
    id: u32,
    primary: bool,
}

/// Identifiant stable d'un écran : FNV-1a 32 bits de son nom de périphérique
/// (`\\.\DISPLAY1`…), qui ne change pas tant que la configuration reste la même.
/// FNV plutôt que le hachage de la bibliothèque standard, qui peut changer d'une
/// version de Rust à l'autre : l'identifiant peut être enregistré.
fn display_id(device: &[u16]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for unit in device.iter().take_while(|&&u| u != 0) {
        for byte in unit.to_le_bytes() {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
    }
    hash
}

/// Description d'un écran ; `None` si Windows ne le connaît plus.
fn screen_info(handle: HMONITOR) -> Option<Screen> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    // SAFETY : `info` est un MONITORINFOEXW dont `cbSize` annonce la taille, ce qui
    // autorise Windows à remplir aussi `szDevice`.
    if unsafe { GetMonitorInfoW(handle, (&raw mut info).cast::<MONITORINFO>()) } == 0 {
        return None;
    }
    let (mut dx, mut dy) = (96u32, 96u32);
    // SAFETY : pointeurs vers deux u32 locaux.
    let hr = unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
    let scale = if hr == 0 && dx > 0 {
        f64::from(dx) / 96.0
    } else {
        1.0
    };
    Some(Screen {
        rect: info.monitorInfo.rcMonitor.into(),
        scale,
        id: display_id(&info.szDevice),
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
    })
}

/// Écrans actifs, dans l'ordre de Windows, sans ceux de taille nulle.
fn screens() -> Vec<Screen> {
    unsafe extern "system" fn collect(
        handle: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY : `data` est le `&mut Vec` passé à EnumDisplayMonitors ci-dessous.
        let list = unsafe { &mut *(data as *mut Vec<HMONITOR>) };
        list.push(handle);
        1
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY : la fonction de rappel ne vit que pendant l'appel, comme `handles`.
    unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(collect),
            &mut handles as *mut _ as LPARAM,
        )
    };
    handles
        .into_iter()
        .filter_map(screen_info)
        .filter(|s| !s.rect.is_empty())
        .collect()
}

fn frame_of(r: &PxRect, scale: f64) -> Frame {
    Frame {
        x: f64::from(r.left) / scale,
        y: f64::from(r.top) / scale,
        width: f64::from(r.width()) / scale,
        height: f64::from(r.height()) / scale,
    }
}

/// Écrans actifs (identifiant stable, cadre en points).
#[allow(dead_code)] // sélection sur plusieurs écrans (à venir)
pub fn displays() -> Vec<(u32, Frame)> {
    let _px = PhysicalPixels::enter();
    screens()
        .iter()
        .map(|s| (s.id, frame_of(&s.rect, s.scale)))
        .collect()
}

/// En-tête d'un DIB 32 bits de haut en bas, sans compression.
fn bitmap_info(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // négatif : lignes de haut en bas
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Inverse rouge et bleu (BGRA ↔ RGBA) et rend opaque : GDI laisse l'alpha à 0 (ou
/// quelconque après un dessin), l'écran est toujours opaque.
fn swap_red_blue(pixels: &mut [u8]) {
    for px in pixels.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        px[3] = u8::MAX;
    }
}

/// Copie GDI d'un rectangle du bureau virtuel, en RGBA.
fn grab(left: i32, top: i32, w: i32, h: i32) -> Result<RgbaImage> {
    // SAFETY : chaque objet GDI créé ici est libéré avant de sortir, dans l'ordre
    // inverse ; le tampon a exactement la taille demandée à GetDIBits.
    unsafe {
        let screen = GetDC(null_mut());
        ensure!(!screen.is_null(), "contexte de l'écran indisponible");
        let mem = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        if mem.is_null() || bitmap.is_null() {
            if !bitmap.is_null() {
                DeleteObject(bitmap);
            }
            if !mem.is_null() {
                DeleteDC(mem);
            }
            ReleaseDC(null_mut(), screen);
            bail!("mémoire GDI insuffisante pour une copie de {w}x{h} px");
        }
        let old = SelectObject(mem, bitmap);
        // CAPTUREBLT : inclut les fenêtres « layered » (menus, info-bulles). Certains
        // pilotes (bureau à distance, machines virtuelles) le refusent : copie simple.
        let copied = BitBlt(mem, 0, 0, w, h, screen, left, top, SRCCOPY | CAPTUREBLT) != 0
            || BitBlt(mem, 0, 0, w, h, screen, left, top, SRCCOPY) != 0;
        let mut info = bitmap_info(w, h);
        let mut buf = vec![0u8; w as usize * h as usize * 4];
        SelectObject(mem, old);
        let lines = GetDIBits(
            mem,
            bitmap,
            0,
            h as u32,
            buf.as_mut_ptr().cast(),
            &mut info,
            DIB_RGB_COLORS,
        );
        DeleteObject(bitmap);
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
        ensure!(copied, "copie de l'écran refusée par GDI (BitBlt)");
        ensure!(
            lines == h,
            "lecture de la copie refusée par GDI ({lines}/{h} lignes)"
        );
        swap_red_blue(&mut buf);
        RgbaImage::from_raw(w as u32, h as u32, buf)
            .ok_or_else(|| anyhow::anyhow!("taille d'image incohérente"))
    }
}

/// Plus petit rectangle qui contient tous les écrans (le bureau virtuel), `None`
/// sans écran.
fn desktop_rect(rects: &[PxRect]) -> Option<PxRect> {
    rects.iter().copied().reduce(|a, b| a.union(&b))
}

/// Découpe la copie du bureau virtuel (posée en `desktop`) en une image par écran.
/// Les zones du bureau qu'aucun écran ne couvre (écrans de tailles différentes)
/// sont noires dans la copie et ne se retrouvent dans aucune image.
fn split(image: RgbaImage, desktop: PxRect, rects: &[PxRect]) -> Vec<RgbaImage> {
    if let [only] = rects
        && *only == desktop
    {
        // Un seul écran : pas de copie.
        return vec![image];
    }
    rects
        .iter()
        .map(|r| {
            let x = (r.left - desktop.left).max(0) as u32;
            let y = (r.top - desktop.top).max(0) as u32;
            crop_imm(
                &image,
                x,
                y,
                r.width().max(0) as u32,
                r.height().max(0) as u32,
            )
            .to_image()
        })
        .collect()
}

/// Indice de l'écran sous le curseur ; l'écran principal (ou le premier) si le
/// curseur est inconnu ou hors de tout écran.
fn active_index(screens: &[Screen], cursor: Option<(i32, i32)>) -> usize {
    cursor
        .and_then(|(x, y)| screens.iter().position(|s| s.rect.contains(x, y)))
        .or_else(|| screens.iter().position(|s| s.primary))
        .unwrap_or(0)
}

/// Fenêtres plus petites (en pixels, sur un côté) : poignées, bulles, bouts de
/// barres d'outils, inutiles à capturer d'un clic.
const MIN_WINDOW_PX: i32 = 20;

/// Ce qu'on sait d'une fenêtre pour décider de la garder (testable sans Windows).
#[derive(Debug, Clone)]
struct WindowFacts<'a> {
    visible: bool,
    iconic: bool,
    /// Masquée par DWM : autre bureau virtuel, app UWP suspendue, menu Démarrer
    /// fermé… `IsWindowVisible` la dit visible alors qu'elle n'est pas à l'écran.
    cloaked: bool,
    /// Fenêtre de notre processus (surcouche, éditeur, fenêtres cachées de l'agent).
    ours: bool,
    class: &'a str,
    ex_style: u32,
    bounds: PxRect,
}

/// La fenêtre est-elle une cible plausible d'un clic ?
fn keep_window(f: &WindowFacts) -> bool {
    // Bureau : Progman (icônes), WorkerW (fond d'écran, diaporama).
    let desktop = matches!(f.class, "Progman" | "WorkerW");
    // Transparente aux clics : surcouches plein écran (NVIDIA, Xbox Game Bar,
    // outils d'accessibilité…) qui recouvriraient tout le reste.
    let click_through = f.ex_style & WS_EX_LAYERED != 0 && f.ex_style & WS_EX_TRANSPARENT != 0;
    f.visible
        && !f.iconic
        && !f.cloaked
        && !f.ours
        && !desktop
        && !click_through
        && f.bounds.width() >= MIN_WINDOW_PX
        && f.bounds.height() >= MIN_WINDOW_PX
}

/// Fenêtre retenue, en pixels physiques du bureau virtuel.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    bounds: PxRect,
    title: String,
    app: String,
}

/// Fenêtres qui touchent l'écran `screen`, en pixels de son image (sans rognage :
/// une fenêtre à cheval sur deux écrans apparaît sur les deux), ordre conservé.
fn windows_on(found: &[Found], screen: PxRect) -> Vec<WindowRect> {
    found
        .iter()
        .filter(|w| w.bounds.intersect(&screen).is_some())
        .map(|w| WindowRect {
            x: w.bounds.left - screen.left,
            y: w.bounds.top - screen.top,
            width: w.bounds.width().max(0) as u32,
            height: w.bounds.height().max(0) as u32,
            title: w.title.clone(),
            app: w.app.clone(),
        })
        .collect()
}

/// Nom de l'exécutable sans dossier ni `.exe` : `C:\…\chrome.exe` → `chrome`.
fn exe_stem(path: &str) -> String {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    match lower.strip_suffix(".exe") {
        Some(stem) if !stem.is_empty() => name[..stem.len()].to_owned(),
        _ => name.to_owned(),
    }
}

/// Chaîne UTF-16 terminée par un zéro, pour les API Windows.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY : tampon local, sa taille est passée à l'API.
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.clamp(0, buf.len() as i32) as usize])
}

/// Titre de la fenêtre. Pour une fenêtre d'un autre processus, Windows lit le titre
/// qu'il garde de son côté sans envoyer de message : pas de blocage sur une
/// application figée.
fn window_text(hwnd: HWND) -> String {
    // SAFETY : appel sans pointeur.
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    // SAFETY : tampon de `len + 1` unités, taille passée à l'API.
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.clamp(0, len) as usize])
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY : DWMWA_CLOAKED s'écrit dans un DWORD, dont on passe l'adresse et la taille.
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            (&raw mut cloaked).cast::<c_void>(),
            size_of::<u32>() as u32,
        )
    };
    hr == 0 && cloaked != 0
}

/// Bornes visibles de la fenêtre, sans l'ombre ni la bordure de redimensionnement
/// invisible de Windows 10 et 11 (~7 px de chaque côté) ; `GetWindowRect` si DWM ne
/// répond pas.
fn window_bounds(hwnd: HWND) -> Option<PxRect> {
    let mut r = RECT::default();
    // SAFETY : DWMWA_EXTENDED_FRAME_BOUNDS s'écrit dans un RECT, adresse et taille
    // passées. Toujours en pixels physiques, quel que soit le réglage DPI.
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&raw mut r).cast::<c_void>(),
            size_of::<RECT>() as u32,
        )
    };
    if hr == 0 && !PxRect::from(r).is_empty() {
        return Some(r.into());
    }
    // SAFETY : `r` est un RECT valide et modifiable.
    (unsafe { GetWindowRect(hwnd, &mut r) } != 0).then(|| r.into())
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY : `pid` est un u32 local modifiable.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

/// Chemin complet de l'exécutable d'un processus. Échoue pour les processus
/// protégés (antivirus…) : nom vide.
fn exe_path(pid: u32) -> Option<String> {
    // SAFETY : la poignée ouverte est refermée avant de sortir ; le tampon est
    // local et sa taille passée à l'API, qui y écrit la longueur réelle.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(process);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]))
    }
}

/// Nom de l'application d'une fenêtre, un processus n'étant interrogé qu'une fois.
fn app_name(hwnd: HWND, pid: u32, cache: &mut HashMap<u32, String>) -> String {
    let mut name_of = |pid: u32| {
        cache
            .entry(pid)
            .or_insert_with(|| exe_path(pid).map(|p| exe_stem(&p)).unwrap_or_default())
            .clone()
    };
    let name = name_of(pid);
    if name.eq_ignore_ascii_case("ApplicationFrameHost") {
        // App UWP (Calculatrice, Paramètres…) : le cadre appartient à
        // ApplicationFrameHost, l'application à sa fenêtre enfant CoreWindow.
        let class = wide("Windows.UI.Core.CoreWindow");
        // SAFETY : `class` est une chaîne UTF-16 terminée par un zéro, vivante
        // pendant l'appel.
        let child = unsafe { FindWindowExW(hwnd, null_mut(), class.as_ptr(), null()) };
        if !child.is_null() {
            let inner = window_pid(child);
            if inner != 0 && inner != pid {
                let inner_name = name_of(inner);
                if !inner_name.is_empty() {
                    return inner_name;
                }
            }
        }
    }
    name
}

/// Fenêtres de premier niveau, de la plus en avant à la plus en arrière (ordre de
/// `EnumWindows`), filtrées par [`keep_window`].
fn list_windows() -> Vec<Found> {
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY : `data` est le `&mut Vec` passé à EnumWindows ci-dessous.
        let list = unsafe { &mut *(data as *mut Vec<HWND>) };
        list.push(hwnd);
        1
    }
    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY : la fonction de rappel ne vit que pendant l'appel, comme `handles`.
    // Elle ne fait que noter les poignées : tout le reste se fait après, hors rappel.
    unsafe { EnumWindows(Some(collect), &mut handles as *mut _ as LPARAM) };
    // SAFETY : appel sans pointeur.
    let own_pid = unsafe { GetCurrentProcessId() };
    let mut apps = HashMap::new();
    handles
        .into_iter()
        .filter_map(|hwnd| describe_window(hwnd, own_pid, &mut apps))
        .collect()
}

/// Fenêtre retenue, ou `None`. Les tests bon marché d'abord : la plupart des
/// fenêtres de premier niveau sont invisibles.
fn describe_window(hwnd: HWND, own_pid: u32, apps: &mut HashMap<u32, String>) -> Option<Found> {
    // SAFETY : appels sans pointeur ; une poignée devenue invalide entre-temps
    // donne simplement « invisible ».
    let visible = unsafe { IsWindowVisible(hwnd) } != 0;
    let iconic = unsafe { IsIconic(hwnd) } != 0;
    if !visible || iconic {
        return None;
    }
    let pid = window_pid(hwnd);
    let class = class_name(hwnd);
    // SAFETY : appel sans pointeur. Les styles sont des bits : la conversion de
    // signe ne perd rien.
    let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    let bounds = window_bounds(hwnd)?;
    let facts = WindowFacts {
        visible,
        iconic,
        cloaked: is_cloaked(hwnd),
        ours: pid == own_pid,
        class: &class,
        ex_style,
        bounds,
    };
    if !keep_window(&facts) {
        return None;
    }
    Some(Found {
        bounds,
        title: window_text(hwnd),
        app: app_name(hwnd, pid, apps),
    })
}

// --- Fenêtre au premier plan (lot « Capture ») --------------------------------

/// Même exécutable que Snappy (l'agent qui a lancé cette capture, les réglages…),
/// chemins comparés sans tenir compte de la casse.
fn same_exe(path: &str, ours: &str) -> bool {
    !path.is_empty() && path.eq_ignore_ascii_case(ours)
}

/// Fenêtre au premier plan (`GetForegroundWindow`), en pixels physiques du bureau
/// virtuel, bornes visibles DWM. `None` si c'est le bureau, une fenêtre de Snappy,
/// ou une fenêtre qu'un clic ne viserait pas (voir [`keep_window`]).
pub fn front_window() -> Option<super::FrontWindow> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, GetForegroundWindow};
    let _px = PhysicalPixels::enter();
    // SAFETY : appels sans pointeur ; une poignée nulle est vérifiée.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    // SAFETY : idem.
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let hwnd = if root.is_null() { hwnd } else { root };
    let pid = window_pid(hwnd);
    let ours = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if exe_path(pid).is_some_and(|p| same_exe(&p, &ours)) {
        return None;
    }
    // SAFETY : appel sans pointeur.
    let own_pid = unsafe { GetCurrentProcessId() };
    let found = describe_window(hwnd, own_pid, &mut HashMap::new())?;
    let b = found.bounds;
    Some(super::FrontWindow {
        bounds: Frame {
            x: f64::from(b.left),
            y: f64::from(b.top),
            width: f64::from(b.width()),
            height: f64::from(b.height()),
        },
        title: found.title,
        app: found.app,
    })
}

/// Curseur affiché au moment de la capture, en pixels physiques.
struct Pointer {
    /// Position du point chaud.
    pos: (i32, i32),
    /// Point chaud, depuis le coin haut gauche de l'image du curseur.
    hotspot: (i32, i32),
    /// Taille réelle de l'image du curseur.
    size: (i32, i32),
    handle: HCURSOR,
}

/// Taille d'un curseur d'après ses bitmaps : celle de l'image couleur s'il en a
/// une ; sinon (curseur monochrome) le masque empile ET puis OU exclusif, d'où la
/// moitié de sa hauteur.
fn cursor_size(color: Option<(i32, i32)>, mask: Option<(i32, i32)>) -> Option<(i32, i32)> {
    let size = match (color, mask) {
        (Some(c), _) => c,
        (None, Some((w, h))) => (w, h / 2),
        (None, None) => return None,
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

fn bitmap_size(bitmap: HBITMAP) -> Option<(i32, i32)> {
    if bitmap.is_null() {
        return None;
    }
    let mut bm = BITMAP::default();
    // SAFETY : `bm` est un BITMAP local dont la taille est passée à l'API.
    let n = unsafe {
        GetObjectW(
            bitmap,
            size_of::<BITMAP>() as i32,
            (&raw mut bm).cast::<c_void>(),
        )
    };
    (n != 0).then_some((bm.bmWidth, bm.bmHeight.abs()))
}

/// Point chaud et taille d'un curseur.
fn cursor_geometry(handle: HCURSOR) -> Option<((i32, i32), (i32, i32))> {
    let mut info = ICONINFO::default();
    // SAFETY : `info` est un ICONINFO local ; les bitmaps qu'il reçoit sont des
    // copies à notre charge, libérées juste après lecture de leur taille.
    unsafe {
        if GetIconInfo(handle, &mut info) == 0 {
            return None;
        }
        let color = bitmap_size(info.hbmColor);
        let mask = bitmap_size(info.hbmMask);
        if !info.hbmColor.is_null() {
            DeleteObject(info.hbmColor);
        }
        if !info.hbmMask.is_null() {
            DeleteObject(info.hbmMask);
        }
        let hotspot = (info.xHotspot as i32, info.yHotspot as i32);
        Some((hotspot, cursor_size(color, mask)?))
    }
}

/// Curseur affiché, `None` s'il est caché (saisie au clavier, jeu, écran tactile).
fn pointer() -> Option<Pointer> {
    let mut info = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY : `info` est initialisé avec sa taille, comme l'exige l'API.
    if unsafe { GetCursorInfo(&mut info) } == 0
        || info.flags & CURSOR_SHOWING == 0
        || info.hCursor.is_null()
    {
        return None;
    }
    let (hotspot, size) = cursor_geometry(info.hCursor)?;
    Some(Pointer {
        pos: (info.ptScreenPos.x, info.ptScreenPos.y),
        hotspot,
        size,
        handle: info.hCursor,
    })
}

/// Où poser le curseur sur l'image de l'écran `screen` : le morceau d'image qu'il
/// recouvre (rogné à l'image, en pixels de l'image) et la position du coin haut
/// gauche du curseur dans ce morceau (négative s'il déborde de l'écran).
fn patch_rect(
    pos: (i32, i32),
    hotspot: (i32, i32),
    size: (i32, i32),
    screen: PxRect,
) -> Option<(PxRect, (i32, i32))> {
    let left = pos.0 - hotspot.0 - screen.left;
    let top = pos.1 - hotspot.1 - screen.top;
    let cursor = PxRect::new(left, top, size.0, size.1);
    let image = PxRect::new(0, 0, screen.width(), screen.height());
    let patch = cursor.intersect(&image)?;
    Some((patch, (left - patch.left, top - patch.top)))
}

/// Dessine le curseur sur `background` (coin haut gauche du curseur en `at`) avec
/// `DrawIconEx`, comme Windows à l'écran : transparence des curseurs 32 bits,
/// inversion des curseurs monochromes (le I de texte) sur le vrai fond.
fn draw_cursor(background: &RgbaImage, at: (i32, i32), handle: HCURSOR) -> Result<RgbaImage> {
    let (w, h) = background.dimensions();
    ensure!(w > 0 && h > 0, "morceau d'écran vide sous le curseur");
    let info = bitmap_info(w as i32, h as i32);
    // SAFETY : le DIB est créé, sélectionné, désélectionné puis libéré ici ; `bits`
    // pointe sur ses w × h × 4 octets (32 bits, lignes sans bourrage) tant qu'il vit,
    // et GdiFlush garantit que le dessin y est terminé avant la lecture.
    unsafe {
        let dc = CreateCompatibleDC(null_mut());
        ensure!(!dc.is_null(), "contexte GDI indisponible pour le curseur");
        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if dib.is_null() || bits.is_null() {
            if !dib.is_null() {
                DeleteObject(dib);
            }
            DeleteDC(dc);
            bail!("bitmap GDI du curseur refusé ({w}x{h} px)");
        }
        let old = SelectObject(dc, dib);
        let pixels = std::slice::from_raw_parts_mut(bits.cast::<u8>(), w as usize * h as usize * 4);
        pixels.copy_from_slice(background.as_raw());
        swap_red_blue(pixels);
        let drawn = DrawIconEx(dc, at.0, at.1, handle, 0, 0, 0, null_mut(), DI_NORMAL) != 0;
        GdiFlush();
        let mut out = pixels.to_vec();
        SelectObject(dc, old);
        DeleteObject(dib);
        DeleteDC(dc);
        ensure!(drawn, "dessin du curseur refusé (DrawIconEx)");
        swap_red_blue(&mut out);
        RgbaImage::from_raw(w, h, out).context("taille du morceau de curseur incohérente")
    }
}

/// Morceau d'écran avec le curseur, si le curseur touche cette image.
fn cursor_patch(
    image: &RgbaImage,
    screen: PxRect,
    pointer: &Pointer,
) -> Result<Option<CursorPatch>> {
    let Some((patch, at)) = patch_rect(pointer.pos, pointer.hotspot, pointer.size, screen) else {
        return Ok(None);
    };
    let background = crop_imm(
        image,
        patch.left as u32,
        patch.top as u32,
        patch.width() as u32,
        patch.height() as u32,
    )
    .to_image();
    let drawn = draw_cursor(&background, at, pointer.handle)?;
    Ok(Some(CursorPatch {
        image: drawn,
        x: patch.left,
        y: patch.top,
    }))
}

/// Windows ne demande aucune autorisation pour capturer l'écran.
pub fn has_permission() -> bool {
    true
}

pub fn request_permission() -> bool {
    true
}

/// Tous les écrans, d'une seule copie GDI du bureau virtuel, avec les fenêtres
/// visibles et le curseur.
pub fn capture_all() -> Result<Capture> {
    let _px = PhysicalPixels::enter();
    let start = Instant::now();
    let screens = screens();
    ensure!(!screens.is_empty(), "aucun écran trouvé");
    let rects: Vec<PxRect> = screens.iter().map(|s| s.rect).collect();
    let desktop = desktop_rect(&rects).context("aucun écran trouvé")?;
    // Curseur juste avant la copie, fenêtres juste après : le tout au même instant
    // à quelques millisecondes près.
    let pointer = pointer();
    let images = match grab(desktop.left, desktop.top, desktop.width(), desktop.height()) {
        Ok(image) => split(image, desktop, &rects),
        // Bureau très grand (plusieurs écrans 4K en L) : GDI peut refuser un seul
        // bitmap de cette taille. Un écran à la fois, à quelques ms d'écart.
        Err(e) if rects.len() > 1 => {
            log::warn!(
                "copie unique du bureau ({}x{} px) impossible, un écran à la fois : {e:#}",
                desktop.width(),
                desktop.height()
            );
            rects
                .iter()
                .map(|r| {
                    grab(r.left, r.top, r.width(), r.height()).with_context(|| {
                        format!(
                            "copie de l'écran {}x{} px en {},{}",
                            r.width(),
                            r.height(),
                            r.left,
                            r.top
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?
        }
        Err(e) => {
            return Err(e.context(format!(
                "copie du bureau ({}x{} px en {},{})",
                desktop.width(),
                desktop.height(),
                desktop.left,
                desktop.top
            )));
        }
    };
    let grabbed = start.elapsed();
    let found = list_windows();
    let position = pointer
        .as_ref()
        .map(|p| p.pos)
        .or_else(|| cursor_position().map(|(x, y)| (x as i32, y as i32)));
    let active = active_index(&screens, position);
    let shots: Vec<Shot> = screens
        .iter()
        .zip(images)
        .map(|(screen, image)| {
            let cursor = pointer
                .as_ref()
                .filter(|p| screen.rect.contains(p.pos.0, p.pos.1))
                .and_then(|p| {
                    cursor_patch(&image, screen.rect, p).unwrap_or_else(|e| {
                        log::warn!("curseur non dessiné : {e:#}");
                        None
                    })
                });
            Shot {
                windows: windows_on(&found, screen.rect),
                frame: frame_of(&screen.rect, screen.scale),
                scale: screen.scale,
                display_id: screen.id,
                cursor,
                image,
            }
        })
        .collect();
    log::debug!(
        "{} écran(s) capturé(s) en {:?} (copie {:?}, bureau {}x{} px), {} fenêtre(s), curseur {}",
        shots.len(),
        start.elapsed(),
        grabbed,
        desktop.width(),
        desktop.height(),
        found.len(),
        if shots.iter().any(|s| s.cursor.is_some()) {
            "inclus"
        } else {
            "absent"
        }
    );
    Ok(Capture { shots, active })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(rect: PxRect, primary: bool) -> Screen {
        Screen {
            rect,
            scale: 1.0,
            id: 0,
            primary,
        }
    }

    fn found(bounds: PxRect, title: &str) -> Found {
        Found {
            bounds,
            title: title.to_owned(),
            app: "app".to_owned(),
        }
    }

    fn facts(class: &str) -> WindowFacts<'_> {
        WindowFacts {
            visible: true,
            iconic: false,
            cloaked: false,
            ours: false,
            class,
            ex_style: 0,
            bounds: PxRect::new(100, 100, 800, 600),
        }
    }

    #[test]
    fn rect_geometry() {
        let a = PxRect::new(0, 0, 1920, 1080);
        assert_eq!((a.width(), a.height()), (1920, 1080));
        assert!(a.contains(0, 0) && a.contains(1919, 1079));
        assert!(!a.contains(1920, 0) && !a.contains(0, 1080) && !a.contains(-1, 5));
        // Écran voisin à droite : se touchent par un bord, rien en commun.
        let b = PxRect::new(1920, -200, 2560, 1440);
        assert_eq!(a.intersect(&b), None);
        assert_eq!(
            a.intersect(&PxRect::new(1900, 1000, 100, 100)),
            Some(PxRect::new(1900, 1000, 20, 80))
        );
        assert_eq!(
            a.union(&b),
            PxRect {
                left: 0,
                top: -200,
                right: 4480,
                bottom: 1240
            }
        );
        assert!(PxRect::new(5, 5, 0, 10).is_empty());
        assert!(PxRect::new(5, 5, 10, -1).is_empty());
    }

    #[test]
    fn display_id_is_stable_and_distinct() {
        let one = wide(r"\\.\DISPLAY1");
        let two = wide(r"\\.\DISPLAY2");
        // Valeurs figées : l'identifiant peut être enregistré entre deux lancements.
        assert_eq!(display_id(&one), 0x6910_07a4);
        assert_eq!(display_id(&two), 0xdb17_76df);
        // `szDevice` est un tableau fixe complété de zéros : seuls comptent les
        // caractères avant le premier zéro.
        let mut padded = [0u16; 32];
        padded[..one.len()].copy_from_slice(&one);
        padded[20] = b'x' as u16;
        assert_eq!(display_id(&padded), display_id(&one));
    }

    #[test]
    fn frames_are_in_points() {
        // Écran à 150 % à gauche du principal : 2560 px = 1706,67 points.
        let f = frame_of(&PxRect::new(-2560, 0, 2560, 1440), 1.5);
        assert!((f.x + 2560.0 / 1.5).abs() < 1e-9);
        assert!((f.width - 2560.0 / 1.5).abs() < 1e-9);
        assert!((f.height - 960.0).abs() < 1e-9);
    }

    #[test]
    fn desktop_covers_every_screen() {
        assert_eq!(desktop_rect(&[]), None);
        // Principal 1920×1080 à 100 %, second 2560×1440 à 150 % à gauche, plus haut.
        let rects = [
            PxRect::new(0, 0, 1920, 1080),
            PxRect::new(-2560, -360, 2560, 1440),
        ];
        assert_eq!(
            desktop_rect(&rects),
            Some(PxRect {
                left: -2560,
                top: -360,
                right: 1920,
                bottom: 1080
            })
        );
    }

    #[test]
    fn split_cuts_one_image_per_screen() {
        let rects = [PxRect::new(0, 0, 4, 2), PxRect::new(-3, -1, 3, 3)];
        let desktop = desktop_rect(&rects).unwrap();
        assert_eq!(
            desktop,
            PxRect {
                left: -3,
                top: -1,
                right: 4,
                bottom: 2
            }
        );
        // Chaque pixel porte ses coordonnées de bureau (décalées de 10 pour rester
        // positives).
        let image = RgbaImage::from_fn(7, 3, |x, y| {
            image::Rgba([(x as i32 - 3 + 10) as u8, (y as i32 - 1 + 10) as u8, 0, 255])
        });
        let parts = split(image, desktop, &rects);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].dimensions(), (4, 2));
        assert_eq!(parts[1].dimensions(), (3, 3));
        // Coin haut gauche de chaque écran = son origine sur le bureau.
        assert_eq!(parts[0].get_pixel(0, 0).0, [10, 10, 0, 255]);
        assert_eq!(parts[0].get_pixel(3, 1).0, [13, 11, 0, 255]);
        assert_eq!(parts[1].get_pixel(0, 0).0, [7, 9, 0, 255]);
        assert_eq!(parts[1].get_pixel(2, 2).0, [9, 11, 0, 255]);
    }

    #[test]
    fn split_single_screen_keeps_the_image() {
        let rect = PxRect::new(0, 0, 3, 2);
        let image = RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]));
        let parts = split(image.clone(), rect, &[rect]);
        assert_eq!(parts, vec![image]);
    }

    #[test]
    fn active_screen_is_under_the_cursor() {
        let screens = [
            screen(PxRect::new(-2560, 0, 2560, 1440), false),
            screen(PxRect::new(0, 0, 1920, 1080), true),
        ];
        assert_eq!(active_index(&screens, Some((-1, 10))), 0);
        assert_eq!(active_index(&screens, Some((0, 10))), 1);
        // Inconnu ou hors écran : le principal.
        assert_eq!(active_index(&screens, None), 1);
        assert_eq!(active_index(&screens, Some((5000, 5000))), 1);
        assert_eq!(
            active_index(&[screen(PxRect::new(0, 0, 10, 10), false)], None),
            0
        );
    }

    #[test]
    fn our_own_executable_is_recognised() {
        let ours = r"C:\Program Files\Snappy\snappy.exe";
        assert!(same_exe(r"c:\program files\snappy\SNAPPY.EXE", ours));
        assert!(!same_exe(r"C:\Windows\notepad.exe", ours));
        assert!(!same_exe("", ""));
    }

    #[test]
    fn window_filter() {
        assert!(keep_window(&facts("Chrome_WidgetWin_1")));
        // La barre des tâches reste.
        assert!(keep_window(&facts("Shell_TrayWnd")));
        assert!(!keep_window(&facts("Progman")));
        assert!(!keep_window(&facts("WorkerW")));
        let base = facts("Notepad");
        assert!(!keep_window(&WindowFacts {
            visible: false,
            ..base.clone()
        }));
        assert!(!keep_window(&WindowFacts {
            iconic: true,
            ..base.clone()
        }));
        assert!(!keep_window(&WindowFacts {
            cloaked: true,
            ..base.clone()
        }));
        assert!(!keep_window(&WindowFacts {
            ours: true,
            ..base.clone()
        }));
        // Surcouche transparente aux clics ; une fenêtre « layered » seule reste
        // (fenêtres à coins arrondis, lanceurs…).
        let overlay = WS_EX_LAYERED | WS_EX_TRANSPARENT;
        assert!(!keep_window(&WindowFacts {
            ex_style: overlay,
            ..base.clone()
        }));
        assert!(keep_window(&WindowFacts {
            ex_style: WS_EX_LAYERED,
            ..base.clone()
        }));
        // Taille minimale, sur chaque côté.
        let sized = |w, h| WindowFacts {
            bounds: PxRect::new(-5, -5, w, h),
            ..base.clone()
        };
        assert!(keep_window(&sized(20, 20)));
        assert!(!keep_window(&sized(19, 400)));
        assert!(!keep_window(&sized(400, 19)));
    }

    #[test]
    fn windows_are_moved_into_each_screen_image() {
        let left = PxRect::new(-2560, -360, 2560, 1440);
        let main = PxRect::new(0, 0, 1920, 1080);
        let list = [
            found(PxRect::new(100, 50, 400, 300), "devant, principal"),
            found(PxRect::new(-200, 100, 600, 400), "à cheval"),
            found(PxRect::new(-2000, -300, 500, 500), "derrière, gauche"),
            // Touche le principal par un bord seulement : sur l'écran de gauche.
            found(PxRect::new(-300, 0, 300, 100), "bord"),
        ];
        let on_main = windows_on(&list, main);
        let titles: Vec<&str> = on_main.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(titles, ["devant, principal", "à cheval"]);
        assert_eq!((on_main[1].x, on_main[1].y), (-200, 100));
        assert_eq!((on_main[1].width, on_main[1].height), (600, 400));

        let on_left = windows_on(&list, left);
        let titles: Vec<&str> = on_left.iter().map(|w| w.title.as_str()).collect();
        // Ordre z conservé.
        assert_eq!(titles, ["à cheval", "derrière, gauche", "bord"]);
        // Pixels de l'image de l'écran de gauche : origine en (-2560, -360).
        assert_eq!((on_left[0].x, on_left[0].y), (2360, 460));
        assert_eq!((on_left[1].x, on_left[1].y), (560, 60));
        assert_eq!(on_left[1].app, "app");
    }

    #[test]
    fn exe_names() {
        assert_eq!(
            exe_stem(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            "chrome"
        );
        assert_eq!(exe_stem(r"C:\Windows\explorer.EXE"), "explorer");
        assert_eq!(exe_stem("Code.exe"), "Code");
        assert_eq!(exe_stem(r"C:\outils\sans-extension"), "sans-extension");
        assert_eq!(exe_stem(r"C:\bizarre\.exe"), ".exe");
        assert_eq!(exe_stem(""), "");
    }

    #[test]
    fn cursor_sizes() {
        assert_eq!(cursor_size(Some((48, 48)), Some((48, 48))), Some((48, 48)));
        // Monochrome : masque ET + masque OU exclusif empilés.
        assert_eq!(cursor_size(None, Some((32, 64))), Some((32, 32)));
        assert_eq!(cursor_size(None, None), None);
        assert_eq!(cursor_size(Some((0, 32)), None), None);
    }

    #[test]
    fn cursor_patch_placement() {
        // Écran de droite à 150 %, origine (1920, 0) ; curseur 48×48, point chaud (8, 4).
        let screen = PxRect::new(1920, 0, 3840, 2160);
        assert_eq!(
            patch_rect((2000, 100), (8, 4), (48, 48), screen),
            Some((PxRect::new(72, 96, 48, 48), (0, 0)))
        );
        // Coin haut gauche : le curseur déborde, le morceau est rogné à l'image et le
        // curseur se dessine décalé dedans.
        assert_eq!(
            patch_rect((1922, 1), (8, 4), (48, 48), screen),
            Some((PxRect::new(0, 0, 42, 45), (-6, -3)))
        );
        // Bord droit et bas.
        assert_eq!(
            patch_rect((1920 + 3830, 2150), (0, 0), (32, 32), screen),
            Some((PxRect::new(3830, 2150, 10, 10), (0, 0)))
        );
        // Entièrement hors de l'écran.
        assert_eq!(patch_rect((100, 100), (0, 0), (32, 32), screen), None);
    }

    #[test]
    fn red_and_blue_swap_and_become_opaque() {
        let mut px = [1, 2, 3, 0, 10, 20, 30, 77];
        swap_red_blue(&mut px);
        assert_eq!(px, [3, 2, 1, 255, 30, 20, 10, 255]);
    }

    /// Dessin de vrais curseurs sur des images en mémoire : GDI sans écran,
    /// fonctionne aussi sous Wine.
    mod drawing {
        use super::super::*;
        use windows_sys::Win32::Graphics::Gdi::CreateBitmap;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateCursor, CreateIconIndirect, DestroyCursor, DestroyIcon, IDC_ARROW, LoadCursorW,
        };

        const BACKGROUND: [u8; 4] = [10, 20, 30, 255];

        fn background(w: u32, h: u32) -> RgbaImage {
            RgbaImage::from_pixel(w, h, image::Rgba(BACKGROUND))
        }

        /// Curseur monochrome 16×16, point chaud (3, 4). Ligne du haut : noir, blanc,
        /// inversé, puis transparent.
        fn monochrome_cursor() -> HCURSOR {
            // Une ligne = 2 octets (16 bits, bit de poids fort à gauche).
            let mut and = [0xFFu8; 32];
            let mut xor = [0u8; 32];
            and[0] = 0b0011_1111; // colonnes 0 et 1 opaques
            xor[0] = 0b0110_0000; // colonne 1 blanche, colonne 2 inversée (ET = 1)
            // SAFETY : plans de 16 × 16 bits, comme annoncé.
            let cursor = unsafe {
                CreateCursor(
                    null_mut(),
                    3,
                    4,
                    16,
                    16,
                    and.as_ptr().cast(),
                    xor.as_ptr().cast(),
                )
            };
            assert!(!cursor.is_null(), "CreateCursor");
            cursor
        }

        #[test]
        fn monochrome_cursor_inverts_the_real_background() {
            let cursor = monochrome_cursor();
            assert_eq!(cursor_geometry(cursor), Some(((3, 4), (16, 16))));
            let image = draw_cursor(&background(20, 20), (2, 1), cursor).unwrap();
            // SAFETY : curseur créé par ce test.
            unsafe { DestroyCursor(cursor) };
            assert_eq!(image.dimensions(), (20, 20));
            assert!(image.pixels().all(|p| p[3] == 255));
            assert_eq!(image.get_pixel(2, 1).0, [0, 0, 0, 255]);
            assert_eq!(image.get_pixel(3, 1).0, [255, 255, 255, 255]);
            assert_eq!(image.get_pixel(4, 1).0, [245, 235, 225, 255]);
            assert_eq!(image.get_pixel(5, 1).0, BACKGROUND);
            assert_eq!(image.get_pixel(2, 2).0, BACKGROUND);
            assert_eq!(image.get_pixel(0, 0).0, BACKGROUND);
        }

        #[test]
        fn color_cursor_is_alpha_blended() {
            // Curseur 32 bits 16×16, point chaud (1, 1) : rouge opaque en (0, 0), bleu
            // à moitié transparent en (1, 0), le reste transparent.
            let info = bitmap_info(16, 16);
            // SAFETY : bitmaps créés, remplis dans leurs bornes et libérés ici ;
            // CreateIconIndirect en garde ses propres copies.
            let cursor = unsafe {
                let mut bits: *mut c_void = null_mut();
                let color =
                    CreateDIBSection(null_mut(), &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
                assert!(!color.is_null() && !bits.is_null(), "CreateDIBSection");
                let px = std::slice::from_raw_parts_mut(bits.cast::<u8>(), 16 * 16 * 4);
                px.fill(0);
                px[..8].copy_from_slice(&[0, 0, 255, 255, 255, 0, 0, 128]); // BGRA
                let zeros = [0u8; 32];
                let mask = CreateBitmap(16, 16, 1, 1, zeros.as_ptr().cast());
                assert!(!mask.is_null(), "CreateBitmap");
                let icon_info = ICONINFO {
                    fIcon: 0,
                    xHotspot: 1,
                    yHotspot: 1,
                    hbmMask: mask,
                    hbmColor: color,
                };
                let cursor = CreateIconIndirect(&icon_info);
                DeleteObject(color);
                DeleteObject(mask);
                assert!(!cursor.is_null(), "CreateIconIndirect");
                cursor
            };
            assert_eq!(cursor_geometry(cursor), Some(((1, 1), (16, 16))));
            let white = RgbaImage::from_pixel(16, 16, image::Rgba([255; 4]));
            let image = draw_cursor(&white, (0, 0), cursor).unwrap();
            // SAFETY : icône créée par ce test.
            unsafe { DestroyIcon(cursor) };
            assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 255]);
            let [r, g, b, a] = image.get_pixel(1, 0).0;
            assert!(
                r.abs_diff(127) <= 2 && g.abs_diff(127) <= 2 && b == 255 && a == 255,
                "mélange : {:?}",
                (r, g, b, a)
            );
            assert_eq!(image.get_pixel(2, 0).0, [255; 4]);
            assert_eq!(image.get_pixel(5, 5).0, [255; 4]);
        }

        #[test]
        fn system_arrow_draws_something() {
            // SAFETY : curseur partagé du système, à ne pas détruire.
            let arrow = unsafe { LoadCursorW(null_mut(), IDC_ARROW) };
            assert!(!arrow.is_null(), "LoadCursorW");
            let (hotspot, size) = cursor_geometry(arrow).expect("géométrie de la flèche");
            eprintln!("flèche : point chaud {hotspot:?}, taille {size:?}");
            assert!(size.0 >= 16 && size.1 >= 16);
            assert!(hotspot.0 < size.0 && hotspot.1 < size.1);
            let (w, h) = (size.0 as u32, size.1 as u32);
            let image = draw_cursor(&background(w, h), (0, 0), arrow).unwrap();
            assert!(image.pixels().all(|p| p[3] == 255));
            assert!(image.pixels().any(|p| p.0 != BACKGROUND), "rien de dessiné");
            assert!(
                image.pixels().any(|p| p.0 == BACKGROUND),
                "fond entièrement recouvert"
            );
        }

        #[test]
        fn cursor_patch_is_cut_from_the_screen_image() {
            let cursor = monochrome_cursor();
            let pointer = Pointer {
                pos: (1003, 504),
                hotspot: (3, 4),
                size: (16, 16),
                handle: cursor,
            };
            // Écran de droite, origine (1000, 500) : coin du curseur en (0, 0) de l'image.
            let screen = PxRect::new(1000, 500, 40, 30);
            let mut image = background(40, 30);
            image.put_pixel(15, 15, image::Rgba([1, 2, 3, 255]));
            let patch = cursor_patch(&image, screen, &pointer).unwrap().unwrap();
            // SAFETY : curseur créé par ce test.
            unsafe { DestroyCursor(cursor) };
            assert_eq!((patch.x, patch.y), (0, 0));
            assert_eq!(patch.image.dimensions(), (16, 16));
            assert_eq!(patch.image.get_pixel(0, 0).0, [0, 0, 0, 255]);
            assert_eq!(patch.image.get_pixel(2, 0).0, [245, 235, 225, 255]);
            // Le fond vient bien de l'image de l'écran.
            assert_eq!(patch.image.get_pixel(15, 15).0, [1, 2, 3, 255]);
        }
    }

    /// Sur une vraie session Windows : `cargo test -- --ignored`. Sous Wine pour
    /// Mac, la copie de l'écran échoue (limite connue de Wine) : le test vérifie
    /// alors seulement les écrans, les fenêtres et le curseur.
    #[test]
    #[ignore]
    fn live_capture() {
        let _px = PhysicalPixels::enter();
        let screens = screens();
        eprintln!("écrans : {screens:?}");
        assert!(!screens.is_empty());
        for (i, s) in screens.iter().enumerate() {
            assert!(!s.rect.is_empty() && s.scale >= 1.0);
            assert!(
                screens[..i].iter().all(|o| o.id != s.id),
                "identifiants en double"
            );
        }
        let windows = list_windows();
        eprintln!("{} fenêtre(s)", windows.len());
        for w in windows.iter().take(15) {
            eprintln!("  {:?} « {} » ({})", w.bounds, w.title, w.app);
        }
        assert!(
            windows
                .iter()
                .all(|w| w.bounds.width() >= MIN_WINDOW_PX && w.bounds.height() >= MIN_WINDOW_PX)
        );
        match pointer() {
            Some(p) => {
                eprintln!(
                    "curseur en {:?}, point chaud {:?}, taille {:?}",
                    p.pos, p.hotspot, p.size
                );
                assert!(p.size.0 > 0 && p.size.1 > 0);
            }
            None => eprintln!("curseur caché ou inconnu"),
        }
        match capture_all() {
            Ok(capture) => {
                eprintln!("{capture:?}");
                assert!(capture.active < capture.shots.len());
                for shot in &capture.shots {
                    let (w, _) = shot.image.dimensions();
                    assert!((f64::from(w) / shot.scale - shot.frame.width).abs() < 1e-6);
                    if let Some(c) = &shot.cursor {
                        let (cw, ch) = c.image.dimensions();
                        assert!(c.x >= 0 && c.y >= 0);
                        assert!(c.x as u32 + cw <= shot.image.width());
                        assert!(c.y as u32 + ch <= shot.image.height());
                    }
                }
            }
            Err(e) => eprintln!("capture refusée (attendu sous Wine pour Mac) : {e:#}"),
        }
    }
}
