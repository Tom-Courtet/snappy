//! Opérations sur les fichiers de l'historique, pour l'espace de travail : type d'un
//! média, liste d'un dossier, vignettes en cache, renommer, déplacer, mettre à la
//! Corbeille, importer, enregistrer sous un autre format, fusionner.
//!
//! Une capture, c'est l'image (`snappy-….png`), ses annotations à côté (même nom,
//! `.json`, voir [`crate::history`]) et ses vignettes (`.thumbnails/<nom>@<clé>.png`
//! dans le même dossier). Chaque opération les garde ensemble.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use image::RgbaImage;

use crate::history;

/// Sous-dossier des vignettes, dans chaque dossier de captures. Caché (point).
pub const THUMBS_DIR: &str = ".thumbnails";

/// Nature d'un fichier de l'espace de travail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Image,
    Video,
    Document,
}

const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp"];
const VIDEO_EXT: &[&str] = &["mp4", "mov", "m4v", "webm", "mkv", "avi"];
const DOC_EXT: &[&str] = &["pdf", "docx", "doc", "rtf", "txt", "md"];

/// Type d'un fichier d'après son extension ; `None` : pas un média (annotations,
/// fichiers inconnus).
pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let ext = ext.as_str();
    if IMAGE_EXT.contains(&ext) {
        Some(Kind::Image)
    } else if VIDEO_EXT.contains(&ext) {
        Some(Kind::Video)
    } else if DOC_EXT.contains(&ext) {
        Some(Kind::Document)
    } else {
        None
    }
}

/// Un fichier de l'espace de travail.
#[derive(Debug, Clone, PartialEq)]
pub struct Media {
    pub path: PathBuf,
    pub kind: Kind,
    /// Taille en octets.
    pub size: u64,
    pub modified: SystemTime,
    pub created: Option<SystemTime>,
    /// Dernière modification des annotations (`.json` à côté), s'il y en a.
    pub doc_modified: Option<SystemTime>,
}

impl Media {
    /// Lit les métadonnées ; `None` si ce n'est pas un média.
    pub fn read(path: &Path) -> Option<Self> {
        let kind = kind_of(path)?;
        let meta = fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let doc_modified = (kind == Kind::Image)
            .then(|| fs::metadata(history::doc_path(path)).ok()?.modified().ok())
            .flatten();
        Some(Self {
            path: path.to_owned(),
            kind,
            size: meta.len(),
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            created: meta.created().ok(),
            doc_modified,
        })
    }

    /// Nom du fichier, extension comprise.
    pub fn name(&self) -> String {
        file_name(&self.path)
    }

    /// Date de la capture : création si le système la connaît, sinon modification.
    pub fn date(&self) -> SystemTime {
        self.created.unwrap_or(self.modified)
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Fichier caché (`.thumbnails`, `.DS_Store`…).
fn hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

/// Médias d'un dossier (sans les sous-dossiers ni les fichiers cachés).
pub fn list(dir: &Path) -> Vec<Media> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| !hidden(p))
        .filter_map(|p| Media::read(&p))
        .collect()
}

/// Espaces de travail : les sous-dossiers visibles de `root`, par nom.
pub fn workspaces(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !hidden(p))
        .collect();
    dirs.sort_by_key(|p| file_name(p).to_lowercase());
    dirs
}

/// Un dossier est-il vide (fichiers cachés comme les vignettes mis à part) ?
pub fn is_empty_dir(dir: &Path) -> bool {
    fs::read_dir(dir)
        .map(|mut it| it.all(|e| e.ok().is_some_and(|e| hidden(&e.path()))))
        .unwrap_or(false)
}

// --- Noms ----------------------------------------------------------------------

/// Caractères refusés dans un nom (Windows en refuse plus que macOS : on prend
/// l'union, pour que l'historique reste portable).
const FORBIDDEN: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Nom saisi par l'utilisateur, nettoyé ; l'erreur dit pourquoi il est refusé.
pub fn validate_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("le nom est vide");
    }
    if name.starts_with('.') {
        return Err("le nom ne peut pas commencer par un point");
    }
    if name
        .chars()
        .any(|c| FORBIDDEN.contains(&c) || c.is_control())
    {
        return Err("caractères interdits : / \\ : * ? \" < > |");
    }
    if name.len() > 200 {
        return Err("le nom est trop long");
    }
    Ok(name.to_owned())
}

/// Nom de fichier (sans extension) d'un chemin.
pub fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn with_stem(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    if ext.is_empty() {
        dir.join(stem)
    } else {
        dir.join(format!("{stem}.{ext}"))
    }
}

/// Chemin libre dans `dir` pour `stem` + `ext` : `-2`, `-3`… si besoin.
pub fn free_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    if ext.is_empty() {
        let first = dir.join(stem);
        if !first.exists() {
            return first;
        }
        return (2..)
            .map(|n| dir.join(format!("{stem}-{n}")))
            .find(|p| !p.exists())
            .expect("suite infinie");
    }
    history::free_path(dir, stem, ext)
}

// --- Vignettes -------------------------------------------------------------------

/// Dossier des vignettes d'un média.
pub fn thumbs_dir(path: &Path) -> PathBuf {
    path.parent().unwrap_or(Path::new(".")).join(THUMBS_DIR)
}

fn millis(t: SystemTime) -> u128 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Clé du cache : change dès que l'image ou ses annotations changent, ou que la
/// taille des vignettes change.
pub fn thumb_key(modified: SystemTime, doc_modified: Option<SystemTime>, px: u32) -> String {
    let doc = doc_modified.map(millis).unwrap_or(0);
    format!("{:x}-{:x}-{px}", millis(modified), doc)
}

/// Vignette d'un média pour une clé.
pub fn thumb_path(path: &Path, key: &str) -> PathBuf {
    thumbs_dir(path).join(format!("{}@{key}.png", file_name(path)))
}

/// Toutes les vignettes en cache d'un média (anciennes clés comprises).
pub fn thumbs_of(path: &Path) -> Vec<PathBuf> {
    let prefix = format!("{}@", file_name(path));
    let Ok(entries) = fs::read_dir(thumbs_dir(path)) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| file_name(p).starts_with(&prefix))
        .collect()
}

/// Retire les vignettes d'un média sauf `keep`.
pub fn prune_thumbs(path: &Path, keep: &Path) {
    for old in thumbs_of(path) {
        if old != keep {
            let _ = fs::remove_file(old);
        }
    }
}

/// Nom de la vignette `thumb` (de `old`) une fois le média renommé ou déplacé en `new`.
fn moved_thumb(thumb: &Path, old: &Path, new: &Path) -> PathBuf {
    let name = file_name(thumb);
    let key = name
        .strip_prefix(&format!("{}@", file_name(old)))
        .unwrap_or(&name)
        .to_owned();
    thumbs_dir(new).join(format!("{}@{key}", file_name(new)))
}

// --- Renommer, déplacer, importer --------------------------------------------------

/// Déplace `from` vers `to` (renommage, ou copie puis suppression si les dossiers
/// sont sur deux volumes).
fn move_file(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to).with_context(|| format!("copie vers {}", to.display()))?;
    fs::remove_file(from).with_context(|| format!("suppression de {}", from.display()))?;
    Ok(())
}

/// Emmène les annotations et les vignettes de `old` vers `new` (l'image est déjà
/// déplacée).
fn carry_companions(old: &Path, new: &Path) -> Result<()> {
    let doc = history::doc_path(old);
    if kind_of(old) == Some(Kind::Image) && doc.exists() {
        move_file(&doc, &history::doc_path(new))?;
    }
    let thumbs = thumbs_of(old);
    if !thumbs.is_empty() {
        let _ = fs::create_dir_all(thumbs_dir(new));
    }
    for thumb in thumbs {
        // Une vignette perdue se régénère : pas une erreur.
        if move_file(&thumb, &moved_thumb(&thumb, old, new)).is_err() {
            let _ = fs::remove_file(&thumb);
        }
    }
    Ok(())
}

/// Renomme un média (l'extension ne change pas) avec ses annotations et ses
/// vignettes. Refuse si le nom est déjà pris.
pub fn rename(path: &Path, new_stem: &str) -> Result<PathBuf> {
    let new_stem = validate_name(new_stem).map_err(|e| anyhow::anyhow!("{e}"))?;
    let dir = path.parent().context("dossier du fichier")?;
    let new = with_stem(dir, &new_stem, &extension(path));
    if new == path {
        return Ok(new);
    }
    // Sous macOS et Windows, « Capture.png » et « capture.png » sont le même
    // fichier : seul le changement de casse est permis.
    let same_file = new.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase();
    if !same_file
        && (new.exists() || kind_of(path) == Some(Kind::Image) && history::doc_path(&new).exists())
    {
        bail!("« {} » existe déjà", file_name(&new));
    }
    fs::rename(path, &new).with_context(|| format!("renommage de {}", path.display()))?;
    carry_companions(path, &new)?;
    Ok(new)
}

/// Déplace un média dans un autre dossier (espace de travail), sous un nom libre.
pub fn move_to(path: &Path, dir: &Path) -> Result<PathBuf> {
    if path.parent() == Some(dir) {
        return Ok(path.to_owned());
    }
    fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    let new = free_path(dir, &stem(path), &extension(path));
    move_file(path, &new)?;
    carry_companions(path, &new)?;
    Ok(new)
}

/// Copie un fichier déposé dans la fenêtre (avec ses annotations s'il en a) dans
/// `dir`, sous un nom libre.
pub fn import(src: &Path, dir: &Path) -> Result<PathBuf> {
    if kind_of(src).is_none() {
        bail!(
            "« {} » n'est ni une image, ni une vidéo, ni un document",
            file_name(src)
        );
    }
    fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    let new = free_path(dir, &stem(src), &extension(src));
    fs::copy(src, &new).with_context(|| format!("copie de {}", src.display()))?;
    let doc = history::doc_path(src);
    if kind_of(src) == Some(Kind::Image) && doc.exists() {
        let _ = fs::copy(&doc, history::doc_path(&new));
    }
    Ok(new)
}

// --- Corbeille ---------------------------------------------------------------------

/// Met un média à la Corbeille avec ses annotations ; ses vignettes sont effacées
/// (elles se régénèrent si on le récupère).
pub fn trash_media(path: &Path) -> Result<()> {
    let mut items = vec![path.to_owned()];
    let doc = history::doc_path(path);
    if kind_of(path) == Some(Kind::Image) && doc.exists() {
        items.push(doc);
    }
    trash(&items)?;
    for thumb in thumbs_of(path) {
        let _ = fs::remove_file(thumb);
    }
    Ok(())
}

/// Met des fichiers ou des dossiers à la Corbeille (récupérables).
pub fn trash(paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        trash_one(path).with_context(|| format!("mise à la Corbeille de {}", path.display()))?;
    }
    Ok(())
}

/// NSFileManager directement : passer par le Finder (AppleScript) demanderait
/// l'autorisation « Automatisation ».
#[cfg(target_os = "macos")]
fn trash_one(path: &Path) -> Result<()> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|e| anyhow::anyhow!("{}", e.localizedDescription()))
}

/// SHFileOperationW avec FOF_ALLOWUNDO : la Corbeille de Windows.
#[cfg(windows)]
fn trash_one(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    // Liste terminée par deux zéros.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    // SAFETY : chaînes valides et terminées, structure initialisée.
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 || op.fAnyOperationsAborted != 0 {
        bail!("SHFileOperation a échoué (code {code})");
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn trash_one(_path: &Path) -> Result<()> {
    bail!("Corbeille non disponible sur ce système")
}

// --- Ouvrir, copier ---------------------------------------------------------------

/// Ouvre un fichier avec l'application par défaut du système.
pub fn open_default(path: &Path) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(windows)]
    let result = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd")
            .raw_arg(format!("/C start \"\" \"{}\"", path.display()))
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    if let Err(e) = result {
        log::warn!("ouverture de {} impossible : {e}", path.display());
    }
}

/// Copie un texte (un chemin) dans le presse-papiers.
pub fn copy_text(text: &str) -> Result<()> {
    arboard::Clipboard::new()
        .context("presse-papiers indisponible")?
        .set_text(text)
        .context("copie du texte")
}

/// Image avec ses annotations, aplatie.
pub fn flattened(path: &Path) -> Result<RgbaImage> {
    crate::effects::flatten_file(path)
}

/// Formats proposés par « Enregistrer sous… » : (nom, extensions).
pub const SAVE_FORMATS: &[(&str, &[&str])] = &[
    ("PNG", &["png"]),
    ("JPEG", &["jpg", "jpeg"]),
    ("BMP", &["bmp"]),
    ("TIFF", &["tif", "tiff"]),
    ("GIF", &["gif"]),
    ("WebP", &["webp"]),
];

/// Format d'image d'après l'extension de `path`.
pub fn save_format(path: &Path) -> Option<image::ImageFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => image::ImageFormat::Png,
        "jpg" | "jpeg" => image::ImageFormat::Jpeg,
        "bmp" => image::ImageFormat::Bmp,
        "tif" | "tiff" => image::ImageFormat::Tiff,
        "gif" => image::ImageFormat::Gif,
        "webp" => image::ImageFormat::WebP,
        _ => return None,
    })
}

/// Enregistre l'image aplatie sous `dst`, au format de son extension.
pub fn save_as(img: &RgbaImage, dst: &Path) -> Result<()> {
    let format = save_format(dst).with_context(|| {
        format!(
            "format inconnu pour {} (png, jpg, bmp, tiff, gif, webp)",
            file_name(dst)
        )
    })?;
    if format == image::ImageFormat::Png {
        return history::write_png(dst, img);
    }
    // JPEG et BMP n'ont pas de transparence : on aplatit sur du blanc.
    let result = match format {
        image::ImageFormat::Jpeg | image::ImageFormat::Bmp => {
            image::DynamicImage::ImageRgb8(on_white(img)).save_with_format(dst, format)
        }
        _ => img.save_with_format(dst, format),
    };
    result.with_context(|| format!("écriture de {}", dst.display()))
}

fn on_white(img: &RgbaImage) -> image::RgbImage {
    image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let [r, g, b, a] = img.get_pixel(x, y).0;
        let a = u32::from(a);
        let mix = |c: u8| ((u32::from(c) * a + 255 * (255 - a) + 127) / 255) as u8;
        image::Rgb([mix(r), mix(g), mix(b)])
    })
}

/// Assemble des images côte à côte (`horizontal`) ou les unes sous les autres,
/// alignées en haut (ou à gauche) ; le reste est transparent.
pub fn merge(images: &[RgbaImage], horizontal: bool) -> RgbaImage {
    let (w, h) = merged_size(images.iter().map(|i| i.dimensions()), horizontal);
    let mut out = RgbaImage::new(w, h);
    let mut offset = 0i64;
    for img in images {
        let (x, y) = if horizontal { (offset, 0) } else { (0, offset) };
        image::imageops::overlay(&mut out, img, x, y);
        offset += i64::from(if horizontal {
            img.width()
        } else {
            img.height()
        });
    }
    out
}

fn merged_size(sizes: impl Iterator<Item = (u32, u32)>, horizontal: bool) -> (u32, u32) {
    sizes.fold((0, 0), |(w, h), (iw, ih)| {
        if horizontal {
            (w + iw, h.max(ih))
        } else {
            (w.max(iw), h + ih)
        }
    })
}

// --- Contexte, tailles -----------------------------------------------------------

/// Application et fenêtre au moment de la capture, si le `.json` les a notées
/// (`"context": {"app": …, "title": …}`). Lu avec indulgence : tout champ manquant
/// ou d'un autre type est ignoré.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureContext {
    pub app: Option<String>,
    pub title: Option<String>,
}

impl CaptureContext {
    pub fn read(image: &Path) -> Self {
        fs::read_to_string(history::doc_path(image))
            .ok()
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn parse(json: &str) -> Self {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
            return Self::default();
        };
        let ctx = value.get("context");
        let field = |name: &str| {
            ctx.and_then(|c| c.get(name))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Self {
            app: field("app"),
            title: field("title"),
        }
    }
}

/// Taille lisible : « 35 Ko », « 1,2 Mo ».
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["octets", "Ko", "Mo", "Go"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else if value < 10.0 {
        format!("{:.1} {}", value, UNITS[unit]).replace('.', ",")
    } else {
        format!("{:.0} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn kinds_by_extension() {
        assert_eq!(kind_of(Path::new("a.PNG")), Some(Kind::Image));
        assert_eq!(kind_of(Path::new("a.mov")), Some(Kind::Video));
        assert_eq!(kind_of(Path::new("a.pdf")), Some(Kind::Document));
        assert_eq!(kind_of(Path::new("a.json")), None);
        assert_eq!(kind_of(Path::new("noext")), None);
    }

    #[test]
    fn listing_skips_annotations_hidden_files_and_folders() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.png", "a.json", "b.mp4", ".DS_Store", "c.txt", "x.bin"] {
            touch(&dir.path().join(name));
        }
        fs::create_dir(dir.path().join("Projet")).unwrap();
        fs::create_dir(dir.path().join(THUMBS_DIR)).unwrap();
        let mut names: Vec<String> = list(dir.path()).iter().map(Media::name).collect();
        names.sort();
        assert_eq!(names, ["a.png", "b.mp4", "c.txt"]);
        let a = list(dir.path())
            .into_iter()
            .find(|m| m.name() == "a.png")
            .unwrap();
        assert!(a.doc_modified.is_some());
        assert_eq!(
            workspaces(dir.path()),
            vec![dir.path().join("Projet")],
            "les dossiers cachés ne sont pas des espaces"
        );
    }

    #[test]
    fn empty_dir_ignores_hidden_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(is_empty_dir(dir.path()));
        fs::create_dir(dir.path().join(THUMBS_DIR)).unwrap();
        assert!(is_empty_dir(dir.path()));
        touch(&dir.path().join("a.png"));
        assert!(!is_empty_dir(dir.path()));
    }

    #[test]
    fn names_are_validated() {
        assert_eq!(validate_name("  Mon projet "), Ok("Mon projet".into()));
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name(".cache").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a:b").is_err());
        assert!(validate_name(&"x".repeat(300)).is_err());
    }

    #[test]
    fn thumbnail_key_follows_image_and_annotations() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_000);
        let later = t + std::time::Duration::from_millis(1);
        assert_eq!(thumb_key(t, None, 320), "3e8-0-320");
        assert_ne!(thumb_key(t, None, 320), thumb_key(later, None, 320));
        assert_ne!(thumb_key(t, None, 320), thumb_key(t, Some(later), 320));
        assert_ne!(thumb_key(t, None, 320), thumb_key(t, None, 160));
        assert_eq!(
            thumb_path(Path::new("/c/snappy-1.png"), "k"),
            Path::new("/c/.thumbnails/snappy-1.png@k.png")
        );
    }

    #[test]
    fn rename_keeps_annotations_and_thumbnails() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        touch(&png);
        touch(&history::doc_path(&png));
        fs::create_dir(dir.path().join(THUMBS_DIR)).unwrap();
        touch(&thumb_path(&png, "k1"));
        let new = rename(&png, "Bug du menu").unwrap();
        assert_eq!(new, dir.path().join("Bug du menu.png"));
        assert!(new.exists() && !png.exists());
        assert!(dir.path().join("Bug du menu.json").exists());
        assert!(!dir.path().join("a.json").exists());
        assert!(thumb_path(&new, "k1").exists());
        assert!(thumbs_of(&png).is_empty());
    }

    #[test]
    fn rename_refuses_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        touch(&a);
        touch(&dir.path().join("b.png"));
        assert!(rename(&a, "b").is_err());
        // Annotations orphelines au nom visé : refusé aussi, pour ne pas les adopter.
        touch(&dir.path().join("c.json"));
        assert!(rename(&a, "c").is_err());
        assert!(rename(&a, "bad/name").is_err());
        assert!(a.exists());
        // Même nom : rien à faire.
        assert_eq!(rename(&a, "a").unwrap(), a);
    }

    #[test]
    fn move_picks_a_free_name_and_carries_companions() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("Projet");
        let png = dir.path().join("a.png");
        touch(&png);
        touch(&history::doc_path(&png));
        fs::create_dir_all(ws.join(THUMBS_DIR)).unwrap();
        fs::create_dir_all(dir.path().join(THUMBS_DIR)).unwrap();
        touch(&thumb_path(&png, "k"));
        touch(&ws.join("a.png"));
        let new = move_to(&png, &ws).unwrap();
        assert_eq!(new, ws.join("a-2.png"));
        assert!(ws.join("a-2.json").exists());
        assert!(thumb_path(&new, "k").exists());
        assert!(!png.exists() && !history::doc_path(&png).exists());
    }

    #[test]
    fn import_copies_with_annotations() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let png = src.path().join("shot.png");
        touch(&png);
        touch(&history::doc_path(&png));
        let new = import(&png, dst.path()).unwrap();
        assert_eq!(new, dst.path().join("shot.png"));
        assert!(dst.path().join("shot.json").exists());
        assert!(png.exists(), "copie, pas déplacement");
        assert_eq!(
            import(&png, dst.path()).unwrap(),
            dst.path().join("shot-2.png")
        );
        assert!(import(&src.path().join("x.bin"), dst.path()).is_err());
    }

    #[test]
    fn context_is_read_leniently() {
        let ctx = CaptureContext::parse(
            r#"{"annotations":[],"context":{"app":"Safari","title":" Accueil ","x":1}}"#,
        );
        assert_eq!(ctx.app.as_deref(), Some("Safari"));
        assert_eq!(ctx.title.as_deref(), Some("Accueil"));
        assert_eq!(
            CaptureContext::parse(r#"{"annotations":[]}"#),
            CaptureContext::default()
        );
        assert_eq!(
            CaptureContext::parse(r#"{"context":{"app":3}}"#),
            CaptureContext::default()
        );
        assert_eq!(
            CaptureContext::parse("pas du json"),
            CaptureContext::default()
        );
    }

    #[test]
    fn merge_places_images_side_by_side_or_stacked() {
        let a = RgbaImage::from_pixel(2, 3, image::Rgba([255, 0, 0, 255]));
        let b = RgbaImage::from_pixel(4, 1, image::Rgba([0, 0, 255, 255]));
        let h = merge(&[a.clone(), b.clone()], true);
        assert_eq!(h.dimensions(), (6, 3));
        assert_eq!(h.get_pixel(2, 0).0, [0, 0, 255, 255]);
        assert_eq!(h.get_pixel(2, 2).0[3], 0, "transparent sous l'image basse");
        let v = merge(&[a, b], false);
        assert_eq!(v.dimensions(), (4, 4));
        assert_eq!(v.get_pixel(0, 3).0, [0, 0, 255, 255]);
    }

    #[test]
    fn save_as_uses_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 128]));
        for ext in ["png", "jpg", "bmp", "tiff", "gif", "webp"] {
            let path = dir.path().join(format!("out.{ext}"));
            save_as(&img, &path).unwrap();
            let back = image::open(&path).unwrap();
            assert_eq!((back.width(), back.height()), (3, 2), "{ext}");
        }
        assert!(save_as(&img, &dir.path().join("out.xyz")).is_err());
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(human_size(512), "512 octets");
        assert_eq!(human_size(35 * 1024), "35 Ko");
        assert_eq!(human_size(1_300_000), "1,2 Mo");
    }

    /// Met un vrai fichier à la Corbeille de l'utilisateur : lancé à la main
    /// seulement (`cargo test -- --ignored trash`).
    #[test]
    #[ignore]
    fn trash_moves_media_and_annotations_to_the_bin() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("snappy-essai-corbeille.png");
        touch(&png);
        touch(&history::doc_path(&png));
        fs::create_dir(dir.path().join(THUMBS_DIR)).unwrap();
        touch(&thumb_path(&png, "k"));
        trash_media(&png).unwrap();
        assert!(!png.exists());
        assert!(!history::doc_path(&png).exists());
        assert!(thumbs_of(&png).is_empty());
    }
}
