//! Historique des captures : l'image d'origine (`captures/snappy-AAAA-MM-JJ-HHMMSS.png`)
//! et ses annotations à côté (même nom, `.json`). Aussi l'export (image aplatie dans
//! le dossier d'enregistrement), le presse-papiers et « Afficher dans le Finder ».

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ImageEncoder, RgbaImage};

use crate::config::write_atomic;
use crate::model::Doc;
// --- Lot « Réglages, raccourcis, après capture » ---
use crate::config::{Config, Corner, FileName, ImageFormat, Store, Watermark};
use crate::model::{Annotation, Pt, Rgba, Shape};

/// Nom de base d'une capture prise maintenant (heure locale), sans extension.
pub fn new_name() -> String {
    let now = jiff::ZonedDateTime::now();
    now.strftime("snappy-%Y-%m-%d-%H%M%S").to_string()
}

/// Chemin libre dans `dir` pour `stem` + `ext` : ajoute `-2`, `-3`… si besoin
/// (deux captures dans la même seconde).
pub fn free_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem}-{n}.{ext}")))
        .find(|p| !p.exists())
        .expect("suite infinie")
}

/// PNG rapide à écrire : les captures Retina sont grandes, la compression maximale
/// coûterait plusieurs centaines de millisecondes pour quelques pour cent.
pub fn write_png(path: &Path, img: &RgbaImage) -> Result<()> {
    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::Adaptive)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .context("encodage PNG")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    }
    write_atomic(path, &bytes)
}

pub fn read_png(path: &Path) -> Result<RgbaImage> {
    Ok(image::open(path)
        .with_context(|| format!("ouverture de {}", path.display()))?
        .to_rgba8())
}

/// Enregistre une nouvelle capture dans l'historique ; rend le chemin de l'image.
///
/// Son nom suit le réglage « Nom du fichier » (lu dans la config du dossier de
/// données qui contient `captures_dir`) : par défaut, [`new_name`].
pub fn save_capture(captures_dir: &Path, img: &RgbaImage) -> Result<PathBuf> {
    let rule = captures_dir
        .parent()
        .and_then(|root| Store::open(root).ok())
        .map(|store| store.peek().file_name)
        .unwrap_or_default();
    let stem = match rule {
        FileName::Date => new_name(),
        rule => name_for(
            &rule,
            captures_dir,
            &jiff::ZonedDateTime::now(),
            &name_context(),
        ),
    };
    let path = free_path(captures_dir, &stem, "png");
    write_png(&path, img)?;
    Ok(path)
}

/// Fichier des annotations d'une capture.
pub fn doc_path(image: &Path) -> PathBuf {
    image.with_extension("json")
}

/// Annotations d'une capture. Absentes ou illisibles : aucune (l'image reste ouvrable).
pub fn load_doc(image: &Path) -> Doc {
    let path = doc_path(image);
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("annotations illisibles ({}) : {e}", path.display());
            Doc::default()
        }),
        Err(_) => Doc::default(),
    }
}

pub fn save_doc(image: &Path, doc: &Doc) -> Result<()> {
    write_atomic(
        &doc_path(image),
        serde_json::to_string_pretty(doc)?.as_bytes(),
    )
}

// --- Lot « Capture » ---

/// Enregistre le contexte de la capture (application, fenêtre, date) dans son
/// `.json`, en gardant les annotations déjà présentes.
pub fn save_context(image: &Path, context: &crate::model::CaptureContext) -> Result<()> {
    let mut doc = load_doc(image);
    doc.context = Some(context.clone());
    save_doc(image, &doc)
}

/// Copie l'image dans le presse-papiers.
pub fn copy_image(img: &RgbaImage) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("presse-papiers indisponible")?;
    clipboard
        .set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: img.as_raw().into(),
        })
        .context("copie de l'image")
}

/// Exporte l'image aplatie dans `dir`, sous le nom de la capture d'origine, au
/// format choisi dans les réglages (PNG par défaut, écrit comme avant).
pub fn export(dir: &Path, original: &Path, img: &RgbaImage) -> Result<PathBuf> {
    let (format, quality) = export_format();
    export_as(dir, original, img, format, quality)
}

/// [`export`] dans un format donné (`jpeg_quality` : 1 à 100, pour le JPEG).
pub fn export_as(
    dir: &Path,
    original: &Path,
    img: &RgbaImage,
    format: ImageFormat,
    jpeg_quality: u8,
) -> Result<PathBuf> {
    let stem = original
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_owned)
        .unwrap_or_else(new_name);
    let path = free_path(dir, &stem, format.extension());
    write_image(&path, img, format, jpeg_quality)?;
    Ok(path)
}

/// Montre le fichier dans le Finder (l'Explorateur sous Windows).
pub fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    // Explorer veut `/select,"chemin"` tel quel : l'échappement habituel des
    // arguments le casse dès que le chemin contient un espace.
    #[cfg(windows)]
    let result = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let result = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
    if let Err(e) = result {
        log::warn!("affichage de {} impossible : {e}", path.display());
    }
}

/// Ouvre un dossier dans le Finder.
pub fn open_dir(dir: &Path) {
    let _ = fs::create_dir_all(dir);
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(windows)]
    let result = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let result = std::process::Command::new("xdg-open").arg(dir).spawn();
    if let Err(e) = result {
        log::warn!("ouverture de {} impossible : {e}", dir.display());
    }
}

// --- Lot « Réglages, raccourcis, après capture » ------------------------------------

/// Format et qualité JPEG des exports, d'après la config (lue sans rien écrire).
fn export_format() -> (ImageFormat, u8) {
    // Les tests ne dépendent pas de la config de la machine.
    if cfg!(test) {
        return (ImageFormat::Png, 90);
    }
    let config = Store::open_default()
        .map(|store| store.peek())
        .unwrap_or_default();
    (config.image_format, config.jpeg_quality())
}

/// Écrit `img` dans `path` au format donné. Le PNG passe par [`write_png`], comme
/// avant ce réglage.
pub fn write_image(
    path: &Path,
    img: &RgbaImage,
    format: ImageFormat,
    jpeg_quality: u8,
) -> Result<()> {
    if format == ImageFormat::Png {
        return write_png(path, img);
    }
    let bytes = encode(img, format, jpeg_quality)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    }
    write_atomic(path, &bytes)
}

/// Image encodée dans un format autre que le PNG.
fn encode(img: &RgbaImage, format: ImageFormat, jpeg_quality: u8) -> Result<Vec<u8>> {
    use image::codecs::jpeg::JpegEncoder;
    let mut out = std::io::Cursor::new(Vec::new());
    let target = match format {
        ImageFormat::Jpeg => {
            // Pas de transparence en JPEG : les captures sont opaques.
            let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
            JpegEncoder::new_with_quality(&mut out, jpeg_quality.clamp(1, 100))
                .write_image(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .context("encodage JPEG")?;
            return Ok(out.into_inner());
        }
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        ImageFormat::Gif => image::ImageFormat::Gif,
        ImageFormat::Webp => image::ImageFormat::WebP,
    };
    img.write_to(&mut out, target)
        .with_context(|| format!("encodage {}", format.label()))?;
    Ok(out.into_inner())
}

// --- Noms de fichiers ---

/// Jetons des modèles de nom, et ce qu'ils deviennent.
pub const TOKENS: &[(&str, &str)] = &[
    ("{date}", "date et heure (2026-11-24-143012)"),
    ("{yyyy}", "année"),
    ("{mm}", "mois"),
    ("{dd}", "jour"),
    ("{hh}", "heure"),
    ("{min}", "minutes"),
    ("{ss}", "secondes"),
    ("{title}", "titre de la fenêtre au premier plan"),
    ("{app}", "application au premier plan"),
    ("{user}", "nom de l'utilisateur"),
    ("{hostname}", "nom de l'ordinateur"),
    ("{n}", "numéro (1, 2, 3…)"),
];

/// Modèle du nom « Numérotation ».
pub const COUNTER_TEMPLATE: &str = "snappy_{n}";

/// Caractères refusés dans un nom de fichier (macOS et Windows).
const FORBIDDEN: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Fenêtre et application au premier plan au moment de la capture, pour les jetons
/// `{title}` et `{app}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameContext {
    pub title: String,
    pub app: String,
}

impl NameContext {
    /// La fenêtre la plus en avant de l'écran actif (hors Snappy).
    pub fn from_capture(capture: &crate::capture::Capture) -> Self {
        let front = capture
            .shots
            .get(capture.active)
            .and_then(|shot| shot.windows.first())
            .or_else(|| capture.shots.iter().find_map(|s| s.windows.first()));
        front
            .map(|w| Self {
                title: w.title.clone(),
                app: w.app.clone(),
            })
            .unwrap_or_default()
    }
}

static CONTEXT: std::sync::Mutex<Option<NameContext>> = std::sync::Mutex::new(None);

/// Note le contexte de la capture en cours (un processus par capture).
pub fn set_name_context(context: NameContext) {
    *CONTEXT.lock().unwrap_or_else(|e| e.into_inner()) = Some(context);
}

fn name_context() -> NameContext {
    CONTEXT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

/// Vérifie un modèle de nom ; l'erreur est un message pour l'utilisateur.
pub fn check_template(template: &str) -> Result<(), String> {
    if template.trim().is_empty() {
        return Err("Le modèle est vide.".into());
    }
    let mut rest = template;
    while let Some(c) = rest.chars().next() {
        match c {
            '{' => {
                let Some(end) = rest.find('}') else {
                    return Err("Accolade « { » sans « } ».".into());
                };
                let token = &rest[..=end];
                if !TOKENS.iter().any(|(t, _)| *t == token) {
                    return Err(format!("Jeton inconnu : {token}"));
                }
                rest = &rest[end + 1..];
            }
            '}' => return Err("Accolade « } » sans « { ».".into()),
            c if FORBIDDEN.contains(&c) || c.is_control() => {
                return Err(format!(
                    "Caractère interdit dans un nom de fichier : « {c} »"
                ));
            }
            c => rest = &rest[c.len_utf8()..],
        }
    }
    Ok(())
}

/// Remplace les jetons (sauf `{n}`, remplacé par `n` s'il est donné). Les caractères
/// interdits des valeurs (titre de fenêtre…) deviennent des tirets.
fn expand_tokens(
    template: &str,
    now: &jiff::ZonedDateTime,
    context: &NameContext,
    n: Option<u32>,
) -> String {
    let clean = |text: &str, max: usize| -> String {
        text.chars()
            .filter(|c| !c.is_control())
            .map(|c| if FORBIDDEN.contains(&c) { '-' } else { c })
            .take(max)
            .collect::<String>()
            .trim()
            .to_owned()
    };
    // Un seul passage : une valeur (titre de fenêtre…) n'est jamais relue comme jeton.
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(len) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let token = &rest[start..=start + len];
        let value = match token {
            "{date}" => now.strftime("%Y-%m-%d-%H%M%S").to_string(),
            "{yyyy}" => now.strftime("%Y").to_string(),
            "{mm}" => now.strftime("%m").to_string(),
            "{dd}" => now.strftime("%d").to_string(),
            "{hh}" => now.strftime("%H").to_string(),
            "{min}" => now.strftime("%M").to_string(),
            "{ss}" => now.strftime("%S").to_string(),
            "{title}" => clean(&context.title, 80),
            "{app}" => clean(&context.app, 40),
            "{user}" => clean(&user_name(), 40),
            "{hostname}" => clean(hostname(), 40),
            "{n}" => n.map_or_else(|| token.to_owned(), |n| n.to_string()),
            other => other.to_owned(),
        };
        out.push_str(&value);
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out
}

/// Nom de l'utilisateur, pour `{user}`.
fn user_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
}

/// Nom de l'ordinateur, pour `{hostname}` (lu une fois : sous macOS, la demande peut
/// passer par le réseau).
fn hostname() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            objc2_foundation::NSProcessInfo::processInfo()
                .hostName()
                .to_string()
                .trim_end_matches(".local")
                .to_owned()
        }
        #[cfg(not(target_os = "macos"))]
        {
            std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_default()
        }
    })
}

/// Nom (sans extension) d'une nouvelle capture dans `dir` d'après la règle `rule`.
/// `{n}` vaut un de plus que le plus grand numéro déjà présent pour ce modèle.
pub fn name_for(
    rule: &FileName,
    dir: &Path,
    now: &jiff::ZonedDateTime,
    context: &NameContext,
) -> String {
    let template = match rule {
        FileName::Date => return now.strftime("snappy-%Y-%m-%d-%H%M%S").to_string(),
        FileName::Counter => COUNTER_TEMPLATE,
        FileName::Custom(t) if check_template(t).is_ok() => t.as_str(),
        FileName::Custom(t) => {
            log::warn!("modèle de nom invalide « {t} » : nom par date");
            return now.strftime("snappy-%Y-%m-%d-%H%M%S").to_string();
        }
    };
    let n = match template.split_once("{n}") {
        Some((before, after)) => {
            let prefix = expand_tokens(before, now, context, None);
            let suffix = expand_tokens(&after.replace("{n}", ""), now, context, None);
            Some(next_number(dir, &prefix, &suffix))
        }
        None => None,
    };
    finish_name(expand_tokens(template, now, context, n), now)
}

/// Exemple de nom pour les réglages (contexte fictif, numéro 1).
pub fn example_name(rule: &FileName) -> String {
    let now = jiff::ZonedDateTime::now();
    let context = NameContext {
        title: "Document sans titre".into(),
        app: "Finder".into(),
    };
    match rule {
        FileName::Date => now.strftime("snappy-%Y-%m-%d-%H%M%S").to_string(),
        FileName::Counter => finish_name(
            expand_tokens(COUNTER_TEMPLATE, &now, &context, Some(1)),
            &now,
        ),
        FileName::Custom(t) => finish_name(expand_tokens(t, &now, &context, Some(1)), &now),
    }
}

/// Nom vide (jetons sans valeur) ou caché : nom par date.
fn finish_name(name: String, now: &jiff::ZonedDateTime) -> String {
    let name = name.trim().trim_start_matches('.').trim();
    if name.is_empty() {
        now.strftime("snappy-%Y-%m-%d-%H%M%S").to_string()
    } else {
        name.to_owned()
    }
}

/// Un de plus que le plus grand `N` des fichiers `{prefix}N{suffix}.*` de `dir`.
fn next_number(dir: &Path, prefix: &str, suffix: &str) -> u32 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 1;
    };
    entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            let stem = path.file_stem()?.to_str()?.to_owned();
            let middle = stem.strip_prefix(prefix)?.strip_suffix(suffix)?;
            if middle.is_empty() || !middle.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            middle.parse::<u32>().ok()
        })
        .max()
        .map_or(1, |n| n.saturating_add(1))
}

// --- Presse-papiers ---

/// Copie le chemin du fichier, en texte.
pub fn copy_path(path: &Path) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("presse-papiers indisponible")?;
    clipboard
        .set_text(path.display().to_string())
        .context("copie du chemin")
}

/// Copie l'image et le fichier : les éditeurs d'images collent l'image, le Finder
/// (l'Explorateur) colle le fichier.
pub fn copy_image_and_path(img: &RgbaImage, path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    return mac_clipboard::copy_image_and_file(img, path);
    #[cfg(windows)]
    return win_clipboard::copy_image_and_file(img, path);
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        log::info!(
            "image et chemin : seulement l'image ici ({})",
            path.display()
        );
        copy_image(img)
    }
}

/// Copie selon le réglage « Copier dans le presse-papiers ». `Snappy` et `Image` :
/// l'image ; `Nothing` : rien.
pub fn copy_as(
    content: crate::config::ClipboardContent,
    img: &RgbaImage,
    path: &Path,
) -> Result<()> {
    use crate::config::ClipboardContent as C;
    match content {
        C::Snappy | C::Image => copy_image(img),
        C::ImageAndPath => copy_image_and_path(img, path),
        C::Path => copy_path(path),
        C::Nothing => Ok(()),
    }
}

#[cfg(target_os = "macos")]
mod mac_clipboard {
    use std::path::Path;

    use anyhow::{Context, Result, bail};
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ImageEncoder, RgbaImage};
    use objc2::AllocAnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSImage, NSPasteboard, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSData, NSString, NSURL};

    /// Deux éléments dans le presse-papiers : l'image (NSImage) et le fichier (NSURL).
    pub fn copy_image_and_file(img: &RgbaImage, path: &Path) -> Result<()> {
        let mut png = Vec::new();
        PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive)
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgba8,
            )
            .context("encodage PNG")?;
        let data = NSData::with_bytes(&png);
        let image = NSImage::initWithData(NSImage::alloc(), &data).context("image illisible")?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let objects: Retained<NSArray<ProtocolObject<dyn NSPasteboardWriting>>> =
            NSArray::from_retained_slice(&[
                ProtocolObject::from_retained(image),
                ProtocolObject::from_retained(url),
            ]);
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        if !pasteboard.writeObjects(&objects) {
            bail!("copie de l'image et du chemin refusée");
        }
        Ok(())
    }
}

#[cfg(windows)]
mod win_clipboard {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use anyhow::{Result, bail};
    use image::RgbaImage;
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    use windows_sys::Win32::System::Ole::{CF_DIB, CF_HDROP};

    /// CF_DIB (image) et CF_HDROP (fichier, comme une copie dans l'Explorateur).
    pub fn copy_image_and_file(img: &RgbaImage, path: &Path) -> Result<()> {
        let dib = dib(img);
        let drop = hdrop(path);
        // SAFETY : presse-papiers ouvert puis refermé ici ; chaque bloc mémoire passe
        // au système quand SetClipboardData réussit, et est libéré sinon.
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                bail!("presse-papiers occupé par une autre application");
            }
            EmptyClipboard();
            let result =
                set(u32::from(CF_DIB), &dib).and_then(|()| set(u32::from(CF_HDROP), &drop));
            CloseClipboard();
            result
        }
    }

    unsafe fn set(format: u32, bytes: &[u8]) -> Result<()> {
        // SAFETY : voir l'appelant.
        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if handle.is_null() {
                bail!("mémoire insuffisante pour le presse-papiers");
            }
            let dest = GlobalLock(handle).cast::<u8>();
            if dest.is_null() {
                GlobalFree(handle);
                bail!("mémoire du presse-papiers inaccessible");
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, bytes.len());
            GlobalUnlock(handle);
            if SetClipboardData(format, handle).is_null() {
                GlobalFree(handle);
                bail!("copie refusée par le presse-papiers");
            }
        }
        Ok(())
    }

    /// BITMAPINFOHEADER (40 octets) puis les pixels BGRA, de bas en haut.
    fn dib(img: &RgbaImage) -> Vec<u8> {
        let (w, h) = img.dimensions();
        let size = w as usize * h as usize * 4;
        let mut out = Vec::with_capacity(40 + size);
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(w as i32).to_le_bytes());
        out.extend_from_slice(&(h as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&[0; 16]); // résolution et palette
        for row in img.rows().rev() {
            for p in row {
                out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
        out
    }

    /// DROPFILES (20 octets, noms en UTF-16) puis le chemin et deux zéros.
    fn hdrop(path: &Path) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&20u32.to_le_bytes()); // pFiles
        out.extend_from_slice(&[0; 8]); // pt
        out.extend_from_slice(&0i32.to_le_bytes()); // fNC
        out.extend_from_slice(&1i32.to_le_bytes()); // fWide
        for unit in path.as_os_str().encode_wide().chain([0, 0]) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out
    }
}

// --- Filigrane ---

/// Ajoute le filigrane de la config aux annotations de la capture `image` (texte
/// modifiable, l'image d'origine n'est pas touchée). Rend `true` s'il a été ajouté :
/// rien si le filigrane est désactivé, vide, ou déjà là.
pub fn apply_watermark(image: &Path, config: &Config) -> Result<bool> {
    let mark = &config.watermark;
    if !mark.enabled || mark.text.trim().is_empty() {
        return Ok(false);
    }
    let mut doc = load_doc(image);
    let present = doc
        .annotations
        .iter()
        .any(|a| matches!(&a.shape, Shape::Text { text, .. } if *text == mark.text));
    if present {
        return Ok(false);
    }
    let (width, height) = image::image_dimensions(image)
        .with_context(|| format!("lecture de {}", image.display()))?;
    doc.annotations
        .push(watermark_annotation(mark, width, height));
    save_doc(image, &doc)?;
    Ok(true)
}

/// Texte blanc à ombre portée (lisible sur fond clair comme sombre), à la taille de
/// l'image, dans le coin choisi.
pub fn watermark_annotation(mark: &Watermark, width: u32, height: u32) -> Annotation {
    let (w, h) = (width as f32, height as f32);
    let size = (w.min(h) / 24.0).clamp(14.0, 48.0);
    let (tw, th) = crate::render::text_size(&mark.text, size);
    let margin = size * 0.75;
    let left = margin;
    let right = (w - tw - margin).max(0.0);
    let top = margin;
    let bottom = (h - th - margin).max(0.0);
    let at = match mark.position {
        Corner::TopLeft => Pt::new(left, top),
        Corner::TopRight => Pt::new(right, top),
        Corner::BottomLeft => Pt::new(left, bottom),
        Corner::BottomRight => Pt::new(right, bottom),
        Corner::Center => Pt::new(((w - tw) / 2.0).max(0.0), ((h - th) / 2.0).max(0.0)),
    };
    let alpha = (f32::from(mark.opacity.min(100)) * 2.55).round() as u8;
    let mut annotation = Annotation::new(
        Shape::Text {
            at,
            text: mark.text.clone(),
            size,
            boxed: false,
        },
        Rgba(255, 255, 255, alpha),
        4.0,
    );
    annotation.shadow = true;
    annotation
}

/// Capture la plus récente de l'historique (image), s'il y en a une.
pub fn latest_capture(captures_dir: &Path) -> Option<PathBuf> {
    fs::read_dir(captures_dir)
        .ok()?
        .filter_map(|e| {
            let entry = e.ok()?;
            let path = entry.path();
            let is_png = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"));
            let modified = entry.metadata().ok()?.modified().ok()?;
            is_png.then_some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Annotation, PALETTE, Pt, Shape};

    #[test]
    fn names_are_sortable_timestamps() {
        let name = new_name();
        assert!(name.starts_with("snappy-20"), "{name}");
        assert_eq!(name.len(), "snappy-2026-11-20-140312".len());
    }

    #[test]
    fn free_path_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let a = free_path(dir.path(), "x", "png");
        fs::write(&a, b"").unwrap();
        let b = free_path(dir.path(), "x", "png");
        assert_eq!(b, dir.path().join("x-2.png"));
    }

    #[test]
    fn capture_and_annotations_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]));
        let path = save_capture(dir.path(), &img).unwrap();
        assert_eq!(read_png(&path).unwrap(), img);
        assert_eq!(load_doc(&path), Doc::default());
        let doc = Doc {
            annotations: vec![Annotation::new(
                Shape::Step {
                    at: Pt::new(1.0, 1.0),
                    number: 1,
                    tail: None,
                },
                PALETTE[0].0,
                4.0,
            )],
            ..Default::default()
        };
        save_doc(&path, &doc).unwrap();
        assert_eq!(load_doc(&path), doc);
    }

    #[test]
    fn export_keeps_the_capture_name() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbaImage::new(2, 2);
        let out = export(
            dir.path(),
            Path::new("/x/snappy-2026-01-02-030405.png"),
            &img,
        )
        .unwrap();
        assert_eq!(out, dir.path().join("snappy-2026-01-02-030405.png"));
    }

    // --- Lot « Réglages, raccourcis, après capture » ---

    fn moment() -> jiff::ZonedDateTime {
        "2026-01-02T03:04:05[UTC]".parse().unwrap()
    }

    #[test]
    fn png_export_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbaImage::from_pixel(5, 4, image::Rgba([10, 20, 30, 255]));
        let original = Path::new("/x/snappy-2026-01-02-030405.png");
        let a = export(dir.path(), original, &img).unwrap();
        let by_hand = dir.path().join("par-write-png.png");
        write_png(&by_hand, &img).unwrap();
        assert_eq!(fs::read(&a).unwrap(), fs::read(&by_hand).unwrap());
    }

    #[test]
    fn export_in_other_formats() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbaImage::from_pixel(6, 4, image::Rgba([200, 100, 50, 255]));
        let original = Path::new("/x/capture.png");
        for format in ImageFormat::ALL {
            let out = export_as(dir.path(), original, &img, format, 90).unwrap();
            assert_eq!(
                out,
                dir.path().join(format!("capture.{}", format.extension()))
            );
            assert_eq!(ImageFormat::from_path(&out), Some(format));
            let back = image::open(&out).unwrap();
            assert_eq!((back.width(), back.height()), (6, 4), "{format:?}");
            let guessed = image::ImageReader::open(&out)
                .unwrap()
                .with_guessed_format()
                .unwrap()
                .format();
            assert!(guessed.is_some(), "{format:?}");
        }
    }

    #[test]
    fn templates_are_checked() {
        assert!(check_template("snappy-{date}").is_ok());
        assert!(check_template("{app} {title} {n}").is_ok());
        assert!(check_template("{yyyy}{mm}{dd}-{hh}{min}{ss}_{user}@{hostname}").is_ok());
        assert!(check_template("").is_err());
        assert!(check_template("   ").is_err());
        assert!(check_template("{inconnu}").is_err());
        assert!(check_template("a{date").is_err());
        assert!(check_template("a}b").is_err());
        assert!(check_template("a/b").is_err());
        assert!(check_template("a:b").is_err());
    }

    #[test]
    fn templates_expand() {
        let now = moment();
        let context = NameContext {
            title: "Rapport: final/v2".into(),
            app: "Safari".into(),
        };
        let dir = tempfile::tempdir().unwrap();
        let name = |t: &str| name_for(&FileName::Custom(t.into()), dir.path(), &now, &context);
        assert_eq!(name("snappy-{date}"), "snappy-2026-01-02-030405");
        assert_eq!(
            name("{yyyy}.{mm}.{dd} {hh}h{min}m{ss}"),
            "2026.01.02 03h04m05"
        );
        assert_eq!(name("{app} - {title}"), "Safari - Rapport- final-v2");
        assert_eq!(name("{app}_{n}"), "Safari_1");
        // Jetons sans valeur : jamais de nom vide.
        let empty = NameContext::default();
        assert_eq!(
            name_for(
                &FileName::Custom("{title}".into()),
                dir.path(),
                &now,
                &empty
            ),
            "snappy-2026-01-02-030405"
        );
        // La date par défaut, comme avant.
        assert_eq!(
            name_for(&FileName::Date, dir.path(), &now, &context),
            "snappy-2026-01-02-030405"
        );
        // Modèle invalide (config modifiée à la main) : nom par date.
        assert_eq!(name("{oups}"), "snappy-2026-01-02-030405");
        // Un titre qui contient un jeton reste tel quel.
        let tricky = NameContext {
            title: "{date} {n}".into(),
            app: String::new(),
        };
        assert_eq!(
            name_for(
                &FileName::Custom("{title}".into()),
                dir.path(),
                &now,
                &tricky
            ),
            "{date} {n}"
        );
        assert!(!expand_tokens("{hostname}", &now, &context, None).contains('{'));
    }

    #[test]
    fn counter_follows_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let now = moment();
        let context = NameContext::default();
        let next = || name_for(&FileName::Counter, dir.path(), &now, &context);
        assert_eq!(next(), "snappy_1");
        fs::write(dir.path().join("snappy_1.png"), b"").unwrap();
        fs::write(dir.path().join("snappy_1.json"), b"").unwrap();
        assert_eq!(next(), "snappy_2");
        fs::write(dir.path().join("snappy_7.png"), b"").unwrap();
        fs::write(dir.path().join("snappy_x.png"), b"").unwrap();
        fs::write(dir.path().join("snappy-2026-01-02-030405.png"), b"").unwrap();
        assert_eq!(next(), "snappy_8");
        let custom = FileName::Custom("cap-{n}-{yyyy}".into());
        fs::write(dir.path().join("cap-4-2026.png"), b"").unwrap();
        fs::write(dir.path().join("cap-9-2025.png"), b"").unwrap();
        assert_eq!(name_for(&custom, dir.path(), &now, &context), "cap-5-2026");
    }

    #[test]
    fn examples_for_the_settings() {
        assert!(example_name(&FileName::Date).starts_with("snappy-20"));
        assert_eq!(example_name(&FileName::Counter), "snappy_1");
        assert_eq!(
            example_name(&FileName::Custom("{app}-{n}".into())),
            "Finder-1"
        );
    }

    #[test]
    fn captures_follow_the_naming_setting() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let img = RgbaImage::new(2, 2);
        // Sans config : le nom par date, comme avant.
        let first = save_capture(&store.captures_dir(), &img).unwrap();
        let stem = first.file_stem().unwrap().to_str().unwrap();
        assert!(stem.starts_with("snappy-20"), "{stem}");
        store
            .save(&Config {
                file_name: FileName::Counter,
                ..Config::default()
            })
            .unwrap();
        let a = save_capture(&store.captures_dir(), &img).unwrap();
        let b = save_capture(&store.captures_dir(), &img).unwrap();
        assert_eq!(a, store.captures_dir().join("snappy_1.png"));
        assert_eq!(b, store.captures_dir().join("snappy_2.png"));
    }

    #[test]
    fn watermark_is_an_editable_annotation() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbaImage::from_pixel(400, 300, image::Rgba([0, 0, 0, 255]));
        let path = save_capture(dir.path(), &img).unwrap();
        let mut config = Config::default();
        // Désactivé par défaut : rien.
        assert!(!apply_watermark(&path, &config).unwrap());
        assert_eq!(load_doc(&path), Doc::default());
        config.watermark.enabled = true;
        config.watermark.text = "© Snappy".into();
        assert!(apply_watermark(&path, &config).unwrap());
        // Une seule fois.
        assert!(!apply_watermark(&path, &config).unwrap());
        let doc = load_doc(&path);
        assert_eq!(doc.annotations.len(), 1);
        let Shape::Text { at, text, size, .. } = &doc.annotations[0].shape else {
            panic!("pas un texte");
        };
        assert_eq!(text, "© Snappy");
        // En bas à droite, dans l'image.
        let (tw, th) = crate::render::text_size(text, *size);
        assert!(at.x > 200.0 && at.x + tw <= 400.0, "{at:?}");
        assert!(at.y > 150.0 && at.y + th <= 300.0, "{at:?}");
        // L'image d'origine n'a pas changé.
        assert_eq!(read_png(&path).unwrap(), img);
    }

    #[test]
    fn latest_capture_is_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(latest_capture(dir.path()), None);
        let img = RgbaImage::new(1, 1);
        let a = dir.path().join("a.png");
        write_png(&a, &img).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = dir.path().join("b.png");
        write_png(&b, &img).unwrap();
        fs::write(dir.path().join("c.json"), b"{}").unwrap();
        assert_eq!(latest_capture(dir.path()), Some(b));
    }
}
