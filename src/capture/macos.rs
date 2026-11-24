//! macOS : CoreGraphics (voir l'en-tête de [`super`]).
//!
//! [`capture_all`] fige tous les écrans d'un coup : un fil par écran pour la copie
//! et la conversion sRGB (le plus long, ~30 ms par écran Retina), pendant que le
//! fil appelant relève la liste des fenêtres et le curseur. Les écrans sont donc
//! pris au même instant, à quelques millisecondes près, et le tout coûte à peine
//! plus qu'un seul écran.
//!
//! Fenêtres : `CGWindowListCopyWindowInfo`, déjà ordonnée de l'avant vers
//! l'arrière. Les titres n'y figurent qu'avec l'autorisation « Enregistrement de
//! l'écran ».
//!
//! Curseur : `NSCursor.currentSystemCursor`, le curseur affiché à l'écran quelle
//! que soit l'app au premier plan. Obsolète depuis le SDK 15 (Apple annonce qu'il
//! renverra un jour `nil`) mais fonctionnel sous macOS 26 ; s'il disparaît, la
//! capture se fait simplement sans curseur.

use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use image::RgbaImage;
use objc2_app_kit::NSCursor;
use objc2_core_foundation::{
    CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBlendMode, CGColorSpace, CGContext, CGDirectDisplayID,
    CGDisplayBounds, CGDisplayPrimaryDisplay, CGError, CGEvent, CGGetActiveDisplayList, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo, CGInterpolationQuality, CGMainDisplayID,
    CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess, CGWindowLevelForKey,
    CGWindowLevelKey, CGWindowListCopyWindowInfo, CGWindowListOption, kCGColorSpaceSRGB,
    kCGNullWindowID,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use super::{Capture, CursorPatch, Frame, Shot, WindowRect, display_at, make_opaque, scale_factor};

/// Au-delà, macOS ne gère de toute façon pas plus d'écrans actifs.
const MAX_DISPLAYS: usize = 32;

/// Plus petite fenêtre retenue, en points : en dessous, ce sont des pastilles,
/// des poignées ou des fenêtres techniques invisibles.
const MIN_WINDOW_SIZE: f64 = 20.0;

/// Position du curseur, en points globaux. Ne demande aucune autorisation.
pub fn cursor_position() -> Option<(f64, f64)> {
    let event = CGEvent::new(None)?;
    let point = CGEvent::location(Some(&event));
    Some((point.x, point.y))
}

/// Écrans actifs et leur cadre, l'écran principal en premier.
pub fn displays() -> Vec<(u32, Frame)> {
    let mut ids = [0 as CGDirectDisplayID; MAX_DISPLAYS];
    let mut count = 0u32;
    // SAFETY : le tableau a la place annoncée, `count` est un u32 valide.
    let err = unsafe { CGGetActiveDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
    if err != CGError::Success {
        log::warn!("liste des écrans illisible (CGError {})", err.0);
        return Vec::new();
    }
    let count = (count as usize).min(MAX_DISPLAYS);
    ids[..count]
        .iter()
        .map(|&id| (id, display_frame(id)))
        .collect()
}

fn display_frame(id: CGDirectDisplayID) -> Frame {
    let rect = CGDisplayBounds(id);
    Frame {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    }
}

/// Capture tous les écrans actifs (un seul par groupe de miroirs), figés au même
/// instant, avec leurs fenêtres et le curseur. À appeler avant d'afficher quoi
/// que ce soit.
pub fn capture_all() -> Result<Capture> {
    let ids: Vec<u32> = displays().iter().map(|&(id, _)| id).collect();
    let mut targets: Vec<(u32, Frame)> = mirror_masters(&ids, |id| CGDisplayPrimaryDisplay(id))
        .into_iter()
        .map(|id| (id, display_frame(id)))
        .collect();
    if targets.is_empty() {
        let id = CGMainDisplayID();
        targets.push((id, display_frame(id)));
    }
    capture_displays(&targets)
}

/// Un écran par groupe de miroirs : le maître du groupe, à la place du premier de
/// ses membres rencontré. L'ordre est conservé (l'écran principal reste en tête).
fn mirror_masters(ids: &[u32], master_of: impl Fn(u32) -> u32) -> Vec<u32> {
    let mut masters = Vec::with_capacity(ids.len());
    for &id in ids {
        let master = master_of(id);
        if !masters.contains(&master) {
            masters.push(master);
        }
    }
    masters
}

/// Capture les écrans demandés en parallèle, puis y range fenêtres et curseur.
/// Un écran illisible (en veille…) est sauté ; erreur seulement si aucun ne l'est.
fn capture_displays(targets: &[(u32, Frame)]) -> Result<Capture> {
    let start = Instant::now();
    if !has_permission() {
        // Sans autorisation, macOS 15+ ne renvoie que le fond d'écran et la barre
        // des menus, et la liste des fenêtres sans leurs titres.
        log::warn!(
            "capture sans l'autorisation « Enregistrement de l'écran » : image probablement incomplète"
        );
    }
    let (results, windows, position, cursor) = std::thread::scope(|scope| {
        let workers: Vec<_> = targets
            .iter()
            .map(|&(id, frame)| scope.spawn(move || grab_display(id, frame)))
            .collect();
        // Pendant ce temps, sur ce fil : position du curseur, son image, les fenêtres.
        let position = cursor_position();
        let cursor = position.and_then(|_| system_cursor());
        let windows = window_list();
        let results: Vec<Result<(u32, Frame, RgbaImage)>> = workers
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("fil de capture interrompu")))
            })
            .collect();
        (results, windows, position, cursor)
    });

    let mut shots = Vec::with_capacity(results.len());
    let mut first_error = None;
    for result in results {
        match result {
            Ok((id, frame, image)) => {
                let scale = scale_factor(image.width(), frame.width);
                shots.push(Shot {
                    windows: windows_on_screen(&windows, &frame, scale),
                    image,
                    frame,
                    scale,
                    display_id: id,
                    cursor: None,
                });
            }
            Err(err) => {
                log::warn!("{err:#}");
                first_error.get_or_insert(err);
            }
        }
    }
    if shots.is_empty() {
        return Err(first_error.unwrap_or_else(|| anyhow::anyhow!("aucun écran à capturer")));
    }

    let frames: Vec<(u32, Frame)> = shots.iter().map(|s| (s.display_id, s.frame)).collect();
    let mut active = 0;
    if let Some((x, y)) = position
        && let Some(index) =
            display_at(&frames, x, y).and_then(|id| shots.iter().position(|s| s.display_id == id))
    {
        active = index;
        let shot = &mut shots[index];
        shot.cursor = cursor.and_then(|cursor| place_cursor(shot, &cursor, x, y));
    }

    log::debug!(
        "{} écran(s) capturé(s) en {:?} ({} fenêtres, curseur {})",
        shots.len(),
        start.elapsed(),
        windows.len(),
        if shots.iter().any(|s| s.cursor.is_some()) {
            "inclus"
        } else {
            "absent"
        }
    );
    Ok(Capture { shots, active })
}

/// Copie un écran et le convertit en sRGB opaque.
fn grab_display(id: CGDirectDisplayID, frame: Frame) -> Result<(u32, Frame, RgbaImage)> {
    ensure!(!frame.is_empty(), "écran {id} introuvable");
    let start = Instant::now();
    let raw = grab(id, &frame)
        .with_context(|| format!("capture de l'écran {id} refusée par le système"))?;
    let mut image =
        to_srgb_rgba(&raw).with_context(|| format!("conversion de la capture de l'écran {id}"))?;
    if !make_opaque(&mut image) {
        bail!("capture de l'écran {id} vide : écran en veille ou session verrouillée");
    }
    log::debug!(
        "écran {id} capturé en {:?} ({}x{} px)",
        start.elapsed(),
        image.width(),
        image.height()
    );
    Ok((id, frame, image))
}

/// Image brute de l'écran, dans son espace colorimétrique et son format natifs.
/// `CGDisplayCreateImage` relit l'image de l'écran (~15 ms sur un écran Retina) ;
/// `CGWindowListCreateImage` recompose les fenêtres (~45 ms), en secours.
#[allow(deprecated)] // obsolètes depuis le SDK 15, toujours là à l'exécution (voir l'en-tête)
fn grab(id: CGDirectDisplayID, frame: &Frame) -> Option<CFRetained<CGImage>> {
    use objc2_core_graphics::{CGDisplayCreateImage, CGWindowImageOption, CGWindowListCreateImage};
    CGDisplayCreateImage(id).or_else(|| {
        log::debug!("CGDisplayCreateImage a échoué pour l'écran {id}, essai par les fenêtres");
        let bounds = CGRect::new(
            CGPoint::new(frame.x, frame.y),
            CGSize::new(frame.width, frame.height),
        );
        CGWindowListCreateImage(
            bounds,
            CGWindowListOption::OptionOnScreenOnly,
            kCGNullWindowID,
            CGWindowImageOption::Default,
        )
    })
}

/// En dessous de cette hauteur, la conversion se fait d'un bloc : découper ne
/// ferait que coûter des fils.
const MIN_BAND_ROWS: usize = 128;

/// Redessine `source` dans un tampon RGBA 8 bits sRGB, sans mise à l'échelle.
/// L'alpha reste prémultiplié : sans effet sur une capture d'écran, opaque.
///
/// Une grande image est convertie par bandes horizontales en parallèle (une
/// sous-image `CGImageCreateWithImageInRect` par bande, sans copie) : ~20 ms
/// d'un bloc pour un écran Retina, ~5 ms en bandes.
pub(super) fn to_srgb_rgba(source: &CGImage) -> Result<RgbaImage> {
    let width = CGImage::width(Some(source));
    let height = CGImage::height(Some(source));
    ensure!(width > 0 && height > 0, "image vide");
    let stride = width.checked_mul(4).context("image trop large")?;
    let mut pixels = vec![0u8; stride.checked_mul(height).context("image trop grande")?];
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    let bands = (height / MIN_BAND_ROWS).clamp(1, threads);
    if bands == 1 {
        draw_into(source, &mut pixels, width, height)?;
    } else {
        let band_rows = height.div_ceil(bands);
        std::thread::scope(|scope| {
            let workers: Vec<_> = pixels
                .chunks_mut(stride * band_rows)
                .enumerate()
                .map(|(index, chunk)| {
                    scope.spawn(move || {
                        let rows = chunk.len() / stride;
                        // Coordonnées de l'image : origine en haut à gauche.
                        let rect = CGRect::new(
                            CGPoint::new(0.0, (index * band_rows) as f64),
                            CGSize::new(width as f64, rows as f64),
                        );
                        let band = CGImage::with_image_in_rect(Some(source), rect)
                            .context("découpe de l'image en bandes")?;
                        draw_into(&band, chunk, width, rows)
                    })
                })
                .collect();
            workers.into_iter().try_for_each(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("fil de conversion interrompu")))
            })
        })?;
    }
    let (w, h) = (u32::try_from(width)?, u32::try_from(height)?);
    RgbaImage::from_raw(w, h, pixels).context("tampon de pixels incohérent")
}

/// Redessine `source` dans un nouveau tampon RGBA 8 bits sRGB de `width` ×
/// `height` pixels (alpha prémultiplié), mise à l'échelle lissée.
fn draw_srgb(source: &CGImage, width: usize, height: usize) -> Result<RgbaImage> {
    ensure!(width > 0 && height > 0, "image vide");
    let stride = width.checked_mul(4).context("image trop large")?;
    let mut pixels = vec![0u8; stride.checked_mul(height).context("image trop grande")?];
    draw_into(source, &mut pixels, width, height)?;
    let (w, h) = (u32::try_from(width)?, u32::try_from(height)?);
    RgbaImage::from_raw(w, h, pixels).context("tampon de pixels incohérent")
}

/// Dessine `source` sur tout `pixels`, tampon RGBA 8 bits sRGB de `width` ×
/// `height` pixels sans bourrage. ColorSync convertit l'espace colorimétrique et
/// l'ordre des octets en une seule passe ; la mise à l'échelle, s'il y en a une,
/// est lissée.
fn draw_into(source: &CGImage, pixels: &mut [u8], width: usize, height: usize) -> Result<()> {
    let stride = width * 4;
    ensure!(
        width > 0 && height > 0 && pixels.len() == stride * height,
        "tampon de pixels incohérent"
    );
    let same_size =
        CGImage::width(Some(source)) == width && CGImage::height(Some(source)) == height;
    // SAFETY : constante fournie par CoreGraphics.
    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .context("espace sRGB indisponible")?;
    // R, G, B, A dans cet ordre en mémoire : exactement la disposition de `RgbaImage`.
    let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    // SAFETY : `pixels` fait `stride * height` octets (vérifié) et survit au
    // contexte, détruit à la fin de cette fonction.
    let ctx = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width,
            height,
            8,
            stride,
            Some(&srgb),
            info,
        )
    }
    .context("création du contexte bitmap sRGB")?;
    CGContext::set_blend_mode(Some(&ctx), CGBlendMode::Copy);
    CGContext::set_interpolation_quality(
        Some(&ctx),
        if same_size {
            CGInterpolationQuality::None
        } else {
            CGInterpolationQuality::High
        },
    );
    let rect = CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(width as f64, height as f64),
    );
    CGContext::draw_image(Some(&ctx), rect, Some(source));
    Ok(())
}

/// Autorisation « Enregistrement de l'écran » déjà accordée. Ne montre rien à
/// l'utilisateur. Après l'avoir accordée, macOS exige souvent de relancer l'app
/// pour que cette réponse change.
pub fn has_permission() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Demande l'autorisation : la première fois, macOS affiche sa fenêtre et
/// ajoute l'app dans Réglages Système > Confidentialité > Enregistrement de
/// l'écran. Renvoie l'état courant (faux tant que l'app n'a pas été relancée).
pub fn request_permission() -> bool {
    CGRequestScreenCaptureAccess()
}

// --- Fenêtres ---------------------------------------------------------------

/// Fenêtre telle que la décrit le serveur de fenêtres, en points globaux.
#[derive(Debug, Clone, PartialEq)]
struct WindowInfo {
    bounds: Frame,
    /// Niveau (`kCGWindowLayer`) : 0 pour les fenêtres d'app ordinaires.
    layer: i32,
    alpha: f64,
    pid: i32,
    title: String,
    app: String,
}

/// Niveaux des éléments système retenus en plus des fenêtres ordinaires.
#[derive(Debug, Clone, Copy)]
struct SystemLevels {
    menu_bar: i32,
    dock: i32,
}

impl SystemLevels {
    fn current() -> Self {
        Self {
            menu_bar: CGWindowLevelForKey(CGWindowLevelKey::MainMenuWindowLevelKey),
            dock: CGWindowLevelForKey(CGWindowLevelKey::DockWindowLevelKey),
        }
    }
}

/// La fenêtre mérite d'être proposée à la capture : fenêtre d'app ordinaire, ou
/// barre des menus, ou Dock ; visible, pas minuscule, pas à nous (la surcouche
/// de sélection ne doit pas se proposer elle-même).
///
/// Barre des menus et Dock sont reconnus à leur niveau **et** à leur nom, que
/// macOS ne traduit pas (contrairement au nom des apps : « Centre de contrôle ») :
/// la fenêtre « Menubar » du « Window Server » au niveau du menu principal, la
/// fenêtre « Dock » au niveau du Dock. Les icônes d'état (niveau 25) et autres
/// pastilles flottantes sont écartées.
fn is_wanted(window: &WindowInfo, own_pid: i32, levels: SystemLevels) -> bool {
    let ordinary = window.layer == 0;
    let menu_bar = window.layer == levels.menu_bar && window.title == "Menubar";
    let dock = window.layer == levels.dock && window.title == "Dock";
    (ordinary || menu_bar || dock)
        && window.alpha > 0.0
        && window.bounds.width >= MIN_WINDOW_SIZE
        && window.bounds.height >= MIN_WINDOW_SIZE
        && window.pid != own_pid
}

/// Fenêtres visibles retenues par [`is_wanted`], de l'avant vers l'arrière.
fn window_list() -> Vec<WindowInfo> {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
        log::warn!("liste des fenêtres indisponible");
        return Vec::new();
    };
    // SAFETY : la documentation garantit un tableau de dictionnaires à clés CFString.
    let list = unsafe { list.cast_unchecked::<CFDictionary<CFString, CFType>>() };
    let own_pid = i32::try_from(std::process::id()).unwrap_or(-1);
    let levels = SystemLevels::current();
    list.iter()
        .filter_map(|entry| parse_window(&entry))
        .filter(|window| is_wanted(window, own_pid, levels))
        .collect()
}

/// Lit une entrée de `CGWindowListCopyWindowInfo`. `None` si les champs
/// indispensables (cadre, niveau, processus) manquent.
fn parse_window(entry: &CFDictionary<CFString, CFType>) -> Option<WindowInfo> {
    // SAFETY : constantes fournies par CoreGraphics.
    let (bounds_key, layer_key, alpha_key, pid_key, name_key, owner_key) = unsafe {
        use objc2_core_graphics::{
            kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowName, kCGWindowOwnerName,
            kCGWindowOwnerPID,
        };
        (
            kCGWindowBounds,
            kCGWindowLayer,
            kCGWindowAlpha,
            kCGWindowOwnerPID,
            kCGWindowName,
            kCGWindowOwnerName,
        )
    };
    let number = |key: &CFString| entry.get(key)?.downcast_ref::<CFNumber>()?.as_f64();
    let text = |key: &CFString| {
        entry
            .get(key)
            .and_then(|value| value.downcast_ref::<CFString>().map(ToString::to_string))
            .unwrap_or_default()
    };

    let bounds = entry.get(bounds_key)?;
    let bounds = bounds.downcast_ref::<CFDictionary>()?;
    let mut rect = CGRect::default();
    // SAFETY : `bounds` est la représentation dictionnaire d'un CGRect (documenté),
    // `rect` est un pointeur valide.
    if !unsafe {
        objc2_core_graphics::CGRectMakeWithDictionaryRepresentation(Some(bounds), &mut rect)
    } {
        return None;
    }
    Some(WindowInfo {
        bounds: Frame {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        },
        // Entiers 32 bits côté système : la conversion est exacte.
        layer: number(layer_key)? as i32,
        alpha: number(alpha_key).unwrap_or(1.0),
        pid: number(pid_key)? as i32,
        title: text(name_key),
        app: text(owner_key),
    })
}

/// Rectangle en points globaux vers pixels de l'image d'un écran
/// (`x`, `y`, largeur, hauteur), élargi au pixel entier, **non rogné**. `None`
/// si le rectangle ne recouvre pas l'écran.
fn to_screen_pixels(bounds: &Frame, screen: &Frame, scale: f64) -> Option<(i32, i32, u32, u32)> {
    let overlaps = bounds.x < screen.x + screen.width
        && bounds.x + bounds.width > screen.x
        && bounds.y < screen.y + screen.height
        && bounds.y + bounds.height > screen.y;
    if !overlaps || bounds.is_empty() {
        return None;
    }
    let left = ((bounds.x - screen.x) * scale).floor();
    let top = ((bounds.y - screen.y) * scale).floor();
    let right = ((bounds.x + bounds.width - screen.x) * scale).ceil();
    let bottom = ((bounds.y + bounds.height - screen.y) * scale).ceil();
    // Valeurs entières à l'échelle d'un bureau : conversions exactes.
    Some((
        left as i32,
        top as i32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

/// Fenêtres qui touchent l'écran, en pixels de son image, ordre conservé.
fn windows_on_screen(windows: &[WindowInfo], screen: &Frame, scale: f64) -> Vec<WindowRect> {
    windows
        .iter()
        .filter_map(|window| {
            let (x, y, width, height) = to_screen_pixels(&window.bounds, screen, scale)?;
            Some(WindowRect {
                x,
                y,
                width,
                height,
                title: window.title.clone(),
                app: window.app.clone(),
            })
        })
        .collect()
}

// --- Curseur ----------------------------------------------------------------

/// Curseur système courant : son image et son point chaud, en points (origine
/// en haut à gauche de l'image).
struct SystemCursor {
    image: CFRetained<CGImage>,
    size: NSSize,
    hot_spot: NSPoint,
}

/// Curseur affiché à l'écran, quelle que soit l'app. `None` s'il est caché ou
/// si macOS ne le donne plus.
fn system_cursor() -> Option<SystemCursor> {
    #[allow(deprecated)] // seul moyen public sans ScreenCaptureKit (voir l'en-tête)
    if !objc2_core_graphics::CGCursorIsVisible() {
        return None;
    }
    objc2::rc::autoreleasepool(|_| {
        #[allow(deprecated)] // idem
        let cursor = NSCursor::currentSystemCursor()?;
        let image = cursor.image();
        let size = image.size();
        if !(size.width > 0.0 && size.height > 0.0) {
            return None;
        }
        // Rectangle proposé deux fois plus grand : macOS choisit la meilleure
        // représentation pour un écran Retina ; on la remet à l'échelle exacte au
        // moment de la composer.
        let mut proposed = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(size.width * 2.0, size.height * 2.0),
        );
        // SAFETY : `proposed` est un pointeur valide, sans contexte ni indices.
        let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut proposed, None, None) }?;
        Some(SystemCursor {
            image: CFRetained::from(&*cg),
            size,
            hot_spot: cursor.hotSpot(),
        })
    })
}

/// Pose le curseur, à la position `(x, y)` en points globaux, sur la capture.
fn place_cursor(shot: &Shot, cursor: &SystemCursor, x: f64, y: f64) -> Option<CursorPatch> {
    let width = (cursor.size.width * shot.scale).round() as usize;
    let height = (cursor.size.height * shot.scale).round() as usize;
    let image = draw_srgb(&cursor.image, width, height)
        .inspect_err(|err| log::warn!("image du curseur illisible : {err:#}"))
        .ok()?;
    let left = ((x - shot.frame.x - cursor.hot_spot.x) * shot.scale).round() as i32;
    let top = ((y - shot.frame.y - cursor.hot_spot.y) * shot.scale).round() as i32;
    compose_cursor(&shot.image, &image, left, top)
}

/// Découpe la zone de `screen` sous le curseur (posé en `x`, `y` pixels, alpha
/// prémultiplié) et y compose le curseur. Le morceau est rogné à l'écran ;
/// `None` si le curseur est entièrement en dehors.
fn compose_cursor(screen: &RgbaImage, cursor: &RgbaImage, x: i32, y: i32) -> Option<CursorPatch> {
    let (sw, sh) = (i64::from(screen.width()), i64::from(screen.height()));
    let (cw, ch) = (i64::from(cursor.width()), i64::from(cursor.height()));
    let (x, y) = (i64::from(x), i64::from(y));
    let left = x.max(0);
    let top = y.max(0);
    let right = (x + cw).min(sw);
    let bottom = (y + ch).min(sh);
    if right <= left || bottom <= top {
        return None;
    }
    // Bornés par la taille de l'écran : conversions exactes.
    let (left, top) = (left as u32, top as u32);
    let (width, height) = (
        (right - i64::from(left)) as u32,
        (bottom - i64::from(top)) as u32,
    );
    let mut patch = image::imageops::crop_imm(screen, left, top, width, height).to_image();
    let (dx, dy) = ((i64::from(left) - x) as u32, (i64::from(top) - y) as u32);
    for (px, py, pixel) in patch.enumerate_pixels_mut() {
        let source = cursor.get_pixel(px + dx, py + dy).0;
        let alpha = u32::from(source[3]);
        for channel in 0..3 {
            let under = u32::from(pixel[channel]);
            // Source prémultipliée : résultat = source + fond × (1 − alpha).
            let value = u32::from(source[channel]) + (under * (255 - alpha) + 127) / 255;
            pixel[channel] = value.min(255) as u8;
        }
        pixel[3] = u8::MAX;
    }
    Some(CursorPatch {
        image: patch,
        x: i32::try_from(left).ok()?,
        y: i32::try_from(top).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    fn frame(x: f64, y: f64, width: f64, height: f64) -> Frame {
        Frame {
            x,
            y,
            width,
            height,
        }
    }

    fn window(layer: i32, title: &str, bounds: Frame) -> WindowInfo {
        WindowInfo {
            bounds,
            layer,
            alpha: 1.0,
            pid: 42,
            title: title.into(),
            app: "App".into(),
        }
    }

    const LEVELS: SystemLevels = SystemLevels {
        menu_bar: 24,
        dock: 20,
    };

    #[test]
    fn mirrors_keep_only_their_master() {
        // 1 principal ; 5 et 9 forment un miroir dont 9 est le maître ; 3 seul.
        let master = |id| if id == 5 { 9 } else { id };
        assert_eq!(mirror_masters(&[1, 5, 3, 9], master), vec![1, 9, 3]);
        assert_eq!(mirror_masters(&[], master), Vec::<u32>::new());
    }

    #[test]
    fn window_pixels_follow_the_screen_origin_and_scale() {
        let main = frame(0.0, 0.0, 1470.0, 956.0);
        let bounds = frame(144.0, 53.0, 1181.0, 738.0);
        assert_eq!(
            to_screen_pixels(&bounds, &main, 2.0),
            Some((288, 106, 2362, 1476))
        );
        // Écran à gauche du principal, plus haut, non Retina.
        let left = frame(-2560.0, -300.0, 2560.0, 1440.0);
        let bounds = frame(-100.0, -50.0, 300.0, 200.0);
        assert_eq!(
            to_screen_pixels(&bounds, &left, 1.0),
            Some((2460, 250, 300, 200))
        );
        // La même fenêtre déborde sur le principal.
        assert_eq!(
            to_screen_pixels(&bounds, &main, 2.0),
            Some((-200, -100, 600, 400))
        );
    }

    #[test]
    fn window_pixels_round_outwards() {
        let screen = frame(0.0, 0.0, 100.0, 100.0);
        let bounds = frame(10.25, 10.75, 10.0, 10.0);
        assert_eq!(
            to_screen_pixels(&bounds, &screen, 2.0),
            Some((20, 21, 21, 21))
        );
    }

    #[test]
    fn windows_elsewhere_are_not_on_the_screen() {
        let screen = frame(0.0, 0.0, 100.0, 100.0);
        // Contiguë à droite : ne touche pas.
        assert_eq!(
            to_screen_pixels(&frame(100.0, 0.0, 50.0, 50.0), &screen, 2.0),
            None
        );
        assert_eq!(
            to_screen_pixels(&frame(-50.0, -50.0, 50.0, 50.0), &screen, 2.0),
            None
        );
        assert_eq!(
            to_screen_pixels(&frame(10.0, 10.0, 0.0, 50.0), &screen, 2.0),
            None
        );
    }

    #[test]
    fn windows_on_screen_keep_the_front_to_back_order() {
        let screen = frame(0.0, 0.0, 100.0, 100.0);
        let windows = [
            window(0, "devant", frame(10.0, 10.0, 30.0, 30.0)),
            window(0, "ailleurs", frame(500.0, 10.0, 30.0, 30.0)),
            window(0, "derrière", frame(0.0, 0.0, 100.0, 100.0)),
        ];
        let rects = windows_on_screen(&windows, &screen, 2.0);
        let titles: Vec<_> = rects.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(titles, ["devant", "derrière"]);
        assert_eq!(
            (rects[0].x, rects[0].width, rects[0].app.as_str()),
            (20, 60, "App")
        );
    }

    #[test]
    fn only_useful_windows_are_kept() {
        let big = frame(0.0, 0.0, 400.0, 300.0);
        assert!(is_wanted(&window(0, "Document", big), 1, LEVELS));
        assert!(is_wanted(&window(0, "", big), 1, LEVELS));
        // La nôtre.
        assert!(!is_wanted(&window(0, "Snappy", big), 42, LEVELS));
        // Invisible.
        let mut transparent = window(0, "x", big);
        transparent.alpha = 0.0;
        assert!(!is_wanted(&transparent, 1, LEVELS));
        // Trop petite dans un sens ou dans l'autre.
        assert!(!is_wanted(
            &window(0, "x", frame(0.0, 0.0, 19.0, 300.0)),
            1,
            LEVELS
        ));
        assert!(!is_wanted(
            &window(0, "x", frame(0.0, 0.0, 300.0, 19.5)),
            1,
            LEVELS
        ));
        assert!(is_wanted(
            &window(0, "x", frame(0.0, 0.0, 20.0, 20.0)),
            1,
            LEVELS
        ));
        // Barre des menus et Dock, mais pas les icônes d'état ni les pastilles.
        let bar = frame(0.0, 0.0, 1470.0, 33.0);
        assert!(is_wanted(&window(24, "Menubar", bar), 1, LEVELS));
        assert!(is_wanted(
            &window(20, "Dock", frame(300.0, 880.0, 800.0, 76.0)),
            1,
            LEVELS
        ));
        assert!(!is_wanted(
            &window(25, "Clock", frame(1327.0, 0.0, 145.0, 33.0)),
            1,
            LEVELS
        ));
        assert!(!is_wanted(&window(24, "Autre", bar), 1, LEVELS));
        assert!(!is_wanted(
            &window(27, "", frame(415.0, 0.0, 640.0, 210.0)),
            1,
            LEVELS
        ));
    }

    /// Écran gris uni 10 × 10 et curseur 4 × 4 : une colonne noire opaque, une
    /// colonne blanche à moitié transparente (prémultipliée), le reste vide.
    fn screen_and_cursor() -> (RgbaImage, RgbaImage) {
        let screen = RgbaImage::from_pixel(10, 10, Rgba([100, 100, 100, 255]));
        let cursor = RgbaImage::from_fn(4, 4, |x, _| match x {
            0 => Rgba([0, 0, 0, 255]),
            1 => Rgba([128, 128, 128, 128]),
            _ => Rgba([0, 0, 0, 0]),
        });
        (screen, cursor)
    }

    #[test]
    fn cursor_is_composed_over_the_screen() {
        let (screen, cursor) = screen_and_cursor();
        let patch = compose_cursor(&screen, &cursor, 3, 5).unwrap();
        assert_eq!((patch.x, patch.y, patch.image.dimensions()), (3, 5, (4, 4)));
        assert_eq!(patch.image.get_pixel(0, 0).0, [0, 0, 0, 255]);
        // 128 + 100 × 127 / 255 ≈ 178.
        assert_eq!(patch.image.get_pixel(1, 2).0, [178, 178, 178, 255]);
        assert_eq!(patch.image.get_pixel(3, 3).0, [100, 100, 100, 255]);
    }

    #[test]
    fn cursor_patch_is_clipped_to_the_screen() {
        let (screen, cursor) = screen_and_cursor();
        // Débord à gauche et en haut : la colonne noire est hors de l'écran.
        let patch = compose_cursor(&screen, &cursor, -1, -2).unwrap();
        assert_eq!((patch.x, patch.y, patch.image.dimensions()), (0, 0, (3, 2)));
        assert_eq!(patch.image.get_pixel(0, 0).0, [178, 178, 178, 255]);
        // Débord à droite et en bas.
        let patch = compose_cursor(&screen, &cursor, 8, 9).unwrap();
        assert_eq!((patch.x, patch.y, patch.image.dimensions()), (8, 9, (2, 1)));
        assert_eq!(patch.image.get_pixel(0, 0).0, [0, 0, 0, 255]);
        // Entièrement dehors.
        assert!(compose_cursor(&screen, &cursor, 10, 0).is_none());
        assert!(compose_cursor(&screen, &cursor, -4, 0).is_none());
    }

    /// Image sRGB RGBA opaque synthétique, une couleur différente par ligne.
    fn striped_image(width: usize, height: usize) -> CFRetained<CGImage> {
        let mut data: Vec<u8> = (0..height)
            .flat_map(|y| {
                let row = [(y % 256) as u8, (y / 256) as u8, 7, 255];
                std::iter::repeat_n(row, width).flatten()
            })
            .collect();
        // SAFETY : constante fournie par CoreGraphics.
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).unwrap();
        let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
        // SAFETY : `data` a la taille annoncée et survit au contexte ; l'image créée
        // garde sa propre copie des pixels.
        let ctx = unsafe {
            CGBitmapContextCreate(
                data.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                Some(&srgb),
                info,
            )
        }
        .unwrap();
        objc2_core_graphics::CGBitmapContextCreateImage(Some(&ctx)).unwrap()
    }

    #[test]
    fn banded_conversion_keeps_every_row_in_place() {
        // Assez haute pour être découpée, hauteur non multiple du nombre de bandes.
        let (width, height) = (3, 1001);
        let image = to_srgb_rgba(&striped_image(width, height)).unwrap();
        assert_eq!(image.dimensions(), (3, 1001));
        for (x, y, pixel) in image.enumerate_pixels() {
            let expected = [(y % 256) as u8, (y / 256) as u8, 7, 255];
            assert_eq!(pixel.0, expected, "pixel ({x}, {y})");
        }
    }

    #[test]
    fn draw_srgb_scales_the_image() {
        let image = draw_srgb(&striped_image(2, 2), 4, 4).unwrap();
        assert_eq!(image.dimensions(), (4, 4));
        assert!(image.pixels().all(|p| p[2] == 7 && p[3] == 255));
    }

    /// Vraie capture de tous les écrans : autorisation « Enregistrement de
    /// l'écran », écran allumé et session déverrouillée.
    /// `cargo test capture_all_for_real -- --ignored --nocapture`. Avec
    /// `SNAPPY_CURSOR_PNG=chemin.png`, enregistre aussi le morceau du curseur.
    #[test]
    #[ignore]
    fn capture_all_for_real() {
        let _ = env_logger::builder().is_test(true).try_init();
        assert!(
            has_permission(),
            "autorisation « Enregistrement de l'écran » absente"
        );
        let _ = capture_all().expect("capture (chauffe)");
        let mut timings = Vec::new();
        let mut capture = None;
        for _ in 0..5 {
            let start = Instant::now();
            capture = Some(capture_all().expect("capture"));
            timings.push(start.elapsed());
        }
        let capture = capture.unwrap();
        eprintln!(
            "{} écran(s), actif : {}, durées : {timings:?}",
            capture.shots.len(),
            capture.active
        );
        for shot in &capture.shots {
            eprintln!("{shot:?}");
            for window in &shot.windows {
                eprintln!(
                    "  [{}, {}, {}x{}] {} — {}",
                    window.x, window.y, window.width, window.height, window.app, window.title
                );
            }
            if let Some(cursor) = &shot.cursor {
                eprintln!(
                    "  curseur : morceau {}x{} en ({}, {})",
                    cursor.image.width(),
                    cursor.image.height(),
                    cursor.x,
                    cursor.y
                );
                if let Some(path) = std::env::var_os("SNAPPY_CURSOR_PNG") {
                    cursor.image.save(&path).expect("enregistrement du curseur");
                }
            }
        }
        eprintln!("position du curseur : {:?}", cursor_position());
        assert!(capture.active < capture.shots.len());
        let shot = &capture.shots[capture.active];
        assert!(shot.image.pixels().all(|p| p[3] == 255));
        assert!(!shot.windows.is_empty(), "aucune fenêtre trouvée");
        if let Some(cursor) = &shot.cursor {
            assert!(cursor.image.pixels().all(|p| p[3] == 255));
            assert_ne!(
                cursor.image,
                image::imageops::crop_imm(
                    &shot.image,
                    cursor.x as u32,
                    cursor.y as u32,
                    cursor.image.width(),
                    cursor.image.height()
                )
                .to_image(),
                "curseur invisible sur son morceau"
            );
        }
    }
}
