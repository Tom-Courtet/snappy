//! Dossier de données et fichier de configuration (`config.json`).
//!
//! macOS : `~/Library/Application Support/Snappy/`. On y trouve aussi les captures
//! d'origine (`captures/`), chacune avec ses annotations à côté (voir
//! [`crate::history`]). L'écriture passe par un fichier temporaire renommé : un
//! plantage en pleine sauvegarde ne corrompt jamais la config.
//!
//! La fenêtre des réglages (`snappy --settings`) l'écrit ; l'agent la relit quand
//! elle le lui demande.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::ipc::Mode;

const CONFIG_FILE: &str = "config.json";
/// Socket Unix ; sous Windows, fichier qui note le port TCP local de l'agent.
#[cfg(unix)]
const SOCKET_FILE: &str = "snappy.sock";
#[cfg(windows)]
const SOCKET_FILE: &str = "snappy.port";
const CAPTURES_DIR: &str = "captures";
const LAST_REGION_FILE: &str = "last_region.json";

/// Réglages de l'utilisateur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Raccourci global de la capture de zone, au format de `global-hotkey`
    /// (ex. `"super+shift+Digit2"`). Vide : aucun raccourci.
    pub region_hotkey: String,
    /// Raccourci global de la capture de l'écran entier.
    pub screen_hotkey: String,
    /// Raccourci global de la capture de la même zone que la fois précédente.
    pub previous_hotkey: String,
    /// Inclure le curseur de la souris dans les captures (M l'inverse pendant la
    /// sélection).
    pub capture_cursor: bool,
    /// Délai de la capture différée, en secondes (menus déroulants, survols).
    pub delay_seconds: u32,
    /// Dossier où « Enregistrer » exporte les images annotées. Vide : le dossier
    /// Images de l'utilisateur, sous-dossier `Snappy`.
    pub save_dir: Option<PathBuf>,
    /// Raccourcis des autres actions (OCR, vidéo, espace de travail…). Absent ou
    /// vide : aucun raccourci.
    pub extra_hotkeys: BTreeMap<Mode, String>,

    // --- Lot « Réglages, raccourcis, après capture » ---
    /// Ce qui suit une capture (« Ensuite »). Défaut : le comportement de Snappy.
    pub after_capture: AfterCapture,
    /// Ce que la capture met dans le presse-papiers. Défaut : comme Snappy.
    pub clipboard: ClipboardContent,
    /// Format des images enregistrées (« Enregistrer »). Défaut : PNG.
    pub image_format: ImageFormat,
    /// Qualité JPEG (1 à 100), si `image_format` vaut `jpeg`.
    pub jpeg_quality: u8,
    /// Nom des nouvelles captures (et donc de leurs exports).
    pub file_name: FileName,
    /// Lancer Snappy à l'ouverture de session. Le système n'est modifié que quand
    /// l'utilisateur coche ou décoche la case.
    pub launch_at_login: bool,
    /// Garder le contexte de la capture (fenêtre, application) : écrit par la capture.
    pub capture_context: bool,
    /// Filigrane ajouté à chaque capture, sous forme d'annotation modifiable.
    pub watermark: Watermark,
    /// Compte à rebours avant la capture (lu par la capture).
    pub countdown_before_capture: bool,
    /// Demander avant de supprimer une capture (espace de travail).
    pub confirm_delete: bool,
    /// OCR : copier le texte sans ouvrir de fenêtre.
    pub ocr_copy_without_window: bool,
    /// QR code : ouvrir les liens dans le navigateur.
    pub qr_open_browser: bool,
    /// Enregistrement vidéo (lu par l'enregistrement vidéo).
    pub video: VideoSettings,
}

impl Default for Config {
    fn default() -> Self {
        let DefaultHotkeys {
            region,
            screen,
            previous,
        } = DEFAULT_HOTKEYS;
        Self {
            region_hotkey: region.into(),
            screen_hotkey: screen.into(),
            previous_hotkey: previous.into(),
            capture_cursor: false,
            delay_seconds: 3,
            save_dir: None,
            extra_hotkeys: BTreeMap::new(),
            after_capture: AfterCapture::default(),
            clipboard: ClipboardContent::default(),
            image_format: ImageFormat::default(),
            jpeg_quality: 90,
            file_name: FileName::default(),
            launch_at_login: false,
            capture_context: false,
            watermark: Watermark::default(),
            countdown_before_capture: false,
            confirm_delete: true,
            ocr_copy_without_window: false,
            qr_open_browser: true,
            video: VideoSettings::default(),
        }
    }
}

impl Config {
    /// Délai de la capture différée accepté par les réglages.
    pub const DELAY_RANGE: std::ops::RangeInclusive<u32> = 1..=10;

    /// Raccourci d'une capture (vide : aucun).
    pub fn hotkey(&self, mode: Mode) -> &str {
        match mode {
            Mode::Region => &self.region_hotkey,
            Mode::Screen => &self.screen_hotkey,
            Mode::Previous => &self.previous_hotkey,
            other => self.extra_hotkeys.get(&other).map_or("", String::as_str),
        }
    }

    pub fn hotkey_mut(&mut self, mode: Mode) -> &mut String {
        match mode {
            Mode::Region => &mut self.region_hotkey,
            Mode::Screen => &mut self.screen_hotkey,
            Mode::Previous => &mut self.previous_hotkey,
            other => self.extra_hotkeys.entry(other).or_default(),
        }
    }
}

// --- Lot « Réglages, raccourcis, après capture » ---------------------------------

/// Ce qui suit une capture (« Ensuite », comme dans Screenpresso).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AfterCapture {
    /// Comportement actuel de Snappy : rien après « Copier », « Enregistrer » et la
    /// zone précédente, l'éditeur après la capture de l'écran.
    #[default]
    Snappy,
    /// Ouvrir l'espace de travail.
    Workspace,
    /// Ouvrir l'éditeur après chaque capture.
    Editor,
    /// Notification « Capture enregistrée ».
    Notify,
    /// Ne rien faire.
    Nothing,
}

impl AfterCapture {
    pub const ALL: [Self; 5] = [
        Self::Snappy,
        Self::Workspace,
        Self::Editor,
        Self::Notify,
        Self::Nothing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Snappy => "Comme Snappy (par défaut)",
            Self::Workspace => "Ouvrir l'espace de travail",
            Self::Editor => "Ouvrir l'éditeur",
            Self::Notify => "Afficher une notification",
            Self::Nothing => "Ne rien faire",
        }
    }
}

/// Contenu du presse-papiers après une capture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardContent {
    /// Comportement actuel de Snappy : l'image après « Copier », l'écran entier et
    /// la zone précédente ; rien après « Enregistrer » et « Annoter ».
    #[default]
    Snappy,
    /// L'image et le fichier (coller dans le Finder colle le fichier).
    ImageAndPath,
    /// L'image, après chaque capture.
    Image,
    /// Le chemin du fichier, en texte.
    Path,
    /// Rien (le bouton « Copier » copie quand même l'image).
    Nothing,
}

impl ClipboardContent {
    pub const ALL: [Self; 5] = [
        Self::Snappy,
        Self::ImageAndPath,
        Self::Image,
        Self::Path,
        Self::Nothing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Snappy => "Comme Snappy (par défaut)",
            Self::ImageAndPath => "Image et chemin",
            Self::Image => "Image",
            Self::Path => "Chemin",
            Self::Nothing => "Ne rien faire",
        }
    }
}

/// Format des images enregistrées.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
    Bmp,
    Tiff,
    Gif,
    Webp,
}

impl ImageFormat {
    pub const ALL: [Self; 6] = [
        Self::Png,
        Self::Jpeg,
        Self::Bmp,
        Self::Tiff,
        Self::Gif,
        Self::Webp,
    ];

    /// Extension des fichiers, sans le point.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Bmp => "bmp",
            Self::Tiff => "tif",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG (par défaut)",
            Self::Jpeg => "JPEG",
            Self::Bmp => "BMP",
            Self::Tiff => "TIFF",
            Self::Gif => "GIF",
            Self::Webp => "WebP",
        }
    }

    /// Format d'après l'extension d'un fichier (sans tenir compte de la casse).
    /// Pour les autres lots (« Enregistrer sous… », espace de travail).
    #[allow(dead_code)]
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "png" => Self::Png,
            "jpg" | "jpeg" => Self::Jpeg,
            "bmp" => Self::Bmp,
            "tif" | "tiff" => Self::Tiff,
            "gif" => Self::Gif,
            "webp" => Self::Webp,
            _ => return None,
        })
    }
}

/// Nom des nouvelles captures.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileName {
    /// `snappy-AAAA-MM-JJ-HHMMSS` (nom actuel).
    #[default]
    Date,
    /// `snappy_1`, `snappy_2`…
    Counter,
    /// Modèle avec jetons (voir [`crate::history::TOKENS`]).
    Custom(String),
}

/// Coin (ou centre) d'un filigrane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
    Center,
}

impl Corner {
    pub const ALL: [Self; 5] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
        Self::Center,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "En haut à gauche",
            Self::TopRight => "En haut à droite",
            Self::BottomLeft => "En bas à gauche",
            Self::BottomRight => "En bas à droite",
            Self::Center => "Au centre",
        }
    }
}

/// Filigrane : un texte ajouté à chaque capture, annotation modifiable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watermark {
    pub enabled: bool,
    pub text: String,
    pub position: Corner,
    /// Opacité en pour cent (0 à 100).
    pub opacity: u8,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            enabled: false,
            text: String::new(),
            position: Corner::BottomRight,
            opacity: 60,
        }
    }
}

/// Qualité des vidéos (libellés et images par seconde de Screenpresso).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoQuality {
    VeryLow,
    Low,
    #[default]
    Normal,
    High,
    VeryHigh,
    Max,
}

impl VideoQuality {
    pub const ALL: [Self; 6] = [
        Self::VeryLow,
        Self::Low,
        Self::Normal,
        Self::High,
        Self::VeryHigh,
        Self::Max,
    ];

    /// Images par seconde (lu par l'enregistrement vidéo).
    #[allow(dead_code)]
    pub fn fps(self) -> u32 {
        match self {
            Self::VeryLow => 12,
            Self::Low => 15,
            Self::Normal => 30,
            Self::High | Self::VeryHigh | Self::Max => 60,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::VeryLow => "Très basse (12 i/s)",
            Self::Low => "Basse (15 i/s)",
            Self::Normal => "Normale (30 i/s, par défaut)",
            Self::High => "Haute (60 i/s)",
            Self::VeryHigh => "Très haute (60 i/s)",
            Self::Max => "Maximale (60 i/s, débit maximal)",
        }
    }
}

/// Réglages de l'enregistrement vidéo (lus par le lot « Vidéo »).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSettings {
    pub quality: VideoQuality,
    /// Arrêt automatique après ce nombre de minutes (aucun : pas d'arrêt).
    pub stop_after_minutes: Option<u32>,
    /// Compte à rebours avant l'enregistrement.
    pub countdown: bool,
    /// Enregistrer le micro.
    pub mic: bool,
    /// Nom du micro (vide : celui du système).
    pub mic_device: String,
    /// Enregistrer le son de l'ordinateur.
    pub system_audio: bool,
    /// Entourer le pointeur.
    pub highlight_cursor: bool,
    /// Marquer les clics.
    pub mark_clicks: bool,
    /// Ouvrir la vidéo après l'enregistrement.
    pub show_preview_after: bool,
    /// Incruster la webcam.
    pub webcam: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            quality: VideoQuality::Normal,
            stop_after_minutes: None,
            countdown: false,
            mic: false,
            mic_device: String::new(),
            system_audio: false,
            highlight_cursor: false,
            mark_clicks: false,
            show_preview_after: true,
            webcam: false,
        }
    }
}

impl Config {
    /// Remet les raccourcis par défaut (les nouvelles actions n'en ont pas).
    pub fn restore_default_hotkeys(&mut self) {
        self.region_hotkey = DEFAULT_HOTKEYS.region.into();
        self.screen_hotkey = DEFAULT_HOTKEYS.screen.into();
        self.previous_hotkey = DEFAULT_HOTKEYS.previous.into();
        self.extra_hotkeys.clear();
    }

    /// Qualité JPEG utilisable (1 à 100).
    pub fn jpeg_quality(&self) -> u8 {
        self.jpeg_quality.clamp(1, 100)
    }
}

/// Raccourcis par défaut.
pub struct DefaultHotkeys {
    pub region: &'static str,
    pub screen: &'static str,
    pub previous: &'static str,
}

impl DefaultHotkeys {
    pub fn get(&self, mode: Mode) -> &'static str {
        match mode {
            Mode::Region => self.region,
            Mode::Screen => self.screen,
            Mode::Previous => self.previous,
            // Les nouvelles actions n'ont pas de raccourci par défaut.
            _ => "",
        }
    }
}

/// macOS : ⌘⇧3, ⌘⇧4 et ⌘⇧5 sont pris par le système ; ⌘⇧1 et ⌘⇧2 sont libres.
#[cfg(target_os = "macos")]
pub const DEFAULT_HOTKEYS: DefaultHotkeys = DefaultHotkeys {
    region: "super+shift+Digit2",
    screen: "super+shift+Digit1",
    previous: "alt+super+shift+Digit2",
};
/// Windows : les raccourcis de Screenpresso, autour d'Impr. écran (Win+Maj+chiffre
/// est pris par la barre des tâches, Win+Maj+S par l'outil Capture d'écran).
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_HOTKEYS: DefaultHotkeys = DefaultHotkeys {
    region: "PrintScreen",
    screen: "control+alt+shift+PrintScreen",
    previous: "shift+PrintScreen",
};

/// Dernière zone capturée, pour « Capturer la zone précédente ».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRegion {
    /// Écran (voir `capture::Shot::display_id`) et taille de son image : si l'écran
    /// a changé de définition, la zone n'a plus de sens.
    pub display_id: u32,
    pub image_width: u32,
    pub image_height: u32,
    /// Zone en pixels de l'image de l'écran.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Dossier de données de l'utilisateur, ou celui de la variable `SNAPPY_DATA_DIR`
    /// (essais avec une config jetable, sans toucher à la vraie).
    pub fn open_default() -> Result<Self> {
        if let Some(dir) = std::env::var_os("SNAPPY_DATA_DIR") {
            return Self::open(Path::new(&dir));
        }
        let dirs = directories::ProjectDirs::from("", "", "Snappy")
            .context("dossier personnel introuvable")?;
        Self::open(dirs.data_dir())
    }

    /// Dossier donné (tests).
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root).with_context(|| format!("création de {}", root.display()))?;
        Ok(Self {
            root: root.to_owned(),
        })
    }

    /// Dossier de données (réglages « Ouvrir le dossier des données »).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Config sur le disque, sans rien écrire : absente ou illisible, la config par
    /// défaut (lectures annexes, depuis l'historique par exemple).
    pub fn peek(&self) -> Config {
        fs::read_to_string(self.config_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    /// Socket de l'agent (une seule instance, commandes `snappy --region`…).
    ///
    /// Un chemin de socket Unix est limité à 104 octets sous macOS : avec un dossier
    /// de données trop long (`SNAPPY_DATA_DIR`), le socket va dans le dossier
    /// temporaire, sous un nom propre à ce dossier de données.
    pub fn socket_path(&self) -> PathBuf {
        let path = self.root.join(SOCKET_FILE);
        if path.as_os_str().len() < 100 {
            return path;
        }
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut h = DefaultHasher::new();
        self.root.hash(&mut h);
        std::env::temp_dir().join(format!("snappy-{:016x}.sock", h.finish()))
    }

    /// Captures d'origine et leurs annotations.
    pub fn captures_dir(&self) -> PathBuf {
        self.root.join(CAPTURES_DIR)
    }

    /// Dernière zone capturée, si elle existe et se lit.
    pub fn last_region(&self) -> Option<LastRegion> {
        let text = fs::read_to_string(self.root.join(LAST_REGION_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save_last_region(&self, region: &LastRegion) {
        let path = self.root.join(LAST_REGION_FILE);
        let result = serde_json::to_vec(region)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| write_atomic(&path, &bytes));
        if let Err(e) = result {
            log::warn!("zone précédente non enregistrée : {e:#}");
        }
    }

    /// Dossier d'export des images annotées.
    pub fn save_dir(&self, config: &Config) -> PathBuf {
        if let Some(dir) = &config.save_dir {
            return dir.clone();
        }
        directories::UserDirs::new()
            .and_then(|d| d.picture_dir().map(Path::to_owned))
            .unwrap_or_else(|| self.root.clone())
            .join("Snappy")
    }

    /// Config sur le disque. Absente : config par défaut, écrite pour que l'utilisateur
    /// puisse la modifier. Illisible : mise de côté (`config.json.bad`) et config par
    /// défaut, pour ne jamais bloquer le démarrage.
    pub fn load(&self) -> Config {
        let path = self.config_path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let config = Config::default();
                if let Err(e) = self.save(&config) {
                    log::warn!("config par défaut non écrite : {e:#}");
                }
                return config;
            }
            Err(e) => {
                log::warn!("lecture de {} impossible : {e}", path.display());
                return Config::default();
            }
        };
        match serde_json::from_str::<Config>(&text) {
            Ok(config) => config,
            Err(e) => {
                let bad = path.with_extension("json.bad");
                log::warn!("config illisible ({e}), copiée dans {}", bad.display());
                let _ = fs::copy(&path, &bad);
                Config::default()
            }
        }
    }

    pub fn save(&self, config: &Config) -> Result<()> {
        write_atomic(
            &self.config_path(),
            serde_json::to_string_pretty(config)?.as_bytes(),
        )
    }
}

/// Écrit `bytes` dans `path` via un fichier temporaire renommé.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes).with_context(|| format!("écriture de {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("remplacement de {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_is_created_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.load(), Config::default());
        assert!(store.config_path().exists());
    }

    #[test]
    fn save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let config = Config {
            region_hotkey: "control+alt+KeyS".into(),
            save_dir: Some(dir.path().join("exports")),
            ..Config::default()
        };
        store.save(&config).unwrap();
        assert_eq!(store.load(), config);
    }

    #[test]
    fn partial_config_keeps_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        fs::write(store.config_path(), r#"{ "save_dir": "/tmp/x" }"#).unwrap();
        let config = store.load();
        assert_eq!(config.region_hotkey, Config::default().region_hotkey);
        assert_eq!(config.save_dir, Some(PathBuf::from("/tmp/x")));
    }

    #[test]
    fn long_data_dirs_get_a_short_socket() {
        let short = Store {
            root: PathBuf::from("/tmp/s"),
        };
        assert_eq!(
            short.socket_path(),
            PathBuf::from("/tmp/s").join(SOCKET_FILE)
        );
        let long = Store {
            root: PathBuf::from(format!("/tmp/{}", "x".repeat(120))),
        };
        let socket = long.socket_path();
        assert!(socket.as_os_str().len() < 104, "{}", socket.display());
        assert_eq!(socket, long.socket_path(), "stable d'un appel à l'autre");
    }

    #[test]
    fn last_region_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.last_region(), None);
        let region = LastRegion {
            display_id: 7,
            image_width: 2940,
            image_height: 1912,
            x: 10,
            y: 20,
            width: 300,
            height: 200,
        };
        store.save_last_region(&region);
        assert_eq!(store.last_region(), Some(region));
    }

    #[test]
    fn hotkeys_by_mode() {
        let mut config = Config::default();
        for mode in [Mode::Region, Mode::Screen, Mode::Previous] {
            assert_eq!(config.hotkey(mode), DEFAULT_HOTKEYS.get(mode));
        }
        config.hotkey_mut(Mode::Screen).clear();
        assert_eq!(config.screen_hotkey, "");
        assert_eq!(config.region_hotkey, DEFAULT_HOTKEYS.region);
    }

    // --- Lot « Réglages, raccourcis, après capture » ---

    /// Une config écrite avant ce lot (sans les nouveaux champs) garde le
    /// comportement actuel.
    #[test]
    fn old_config_files_keep_current_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        fs::write(
            store.config_path(),
            r#"{
  "region_hotkey": "super+shift+Digit2",
  "screen_hotkey": "super+shift+Digit1",
  "previous_hotkey": "alt+super+shift+Digit2",
  "capture_cursor": true,
  "delay_seconds": 5,
  "save_dir": null,
  "extra_hotkeys": {}
}"#,
        )
        .unwrap();
        let config = store.load();
        assert!(config.capture_cursor);
        assert_eq!(config.delay_seconds, 5);
        assert_eq!(config.after_capture, AfterCapture::Snappy);
        assert_eq!(config.clipboard, ClipboardContent::Snappy);
        assert_eq!(config.image_format, ImageFormat::Png);
        assert_eq!(config.file_name, FileName::Date);
        assert!(!config.launch_at_login);
        assert!(!config.watermark.enabled);
        assert!(config.confirm_delete);
        assert!(config.qr_open_browser);
        assert_eq!(config.video, VideoSettings::default());
        assert_eq!(config.video.quality.fps(), 30);
        // Un fichier vide aussi.
        fs::write(store.config_path(), "{}").unwrap();
        assert_eq!(store.load(), Config::default());
    }

    #[test]
    fn new_settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let config = Config {
            after_capture: AfterCapture::Workspace,
            clipboard: ClipboardContent::ImageAndPath,
            image_format: ImageFormat::Jpeg,
            jpeg_quality: 75,
            file_name: FileName::Custom("{app}-{n}".into()),
            watermark: Watermark {
                enabled: true,
                text: "© Moi".into(),
                position: Corner::TopLeft,
                opacity: 40,
            },
            video: VideoSettings {
                quality: VideoQuality::Max,
                stop_after_minutes: Some(10),
                mic_device: "Micro USB".into(),
                ..VideoSettings::default()
            },
            ..Config::default()
        };
        store.save(&config).unwrap();
        assert_eq!(store.load(), config);
        assert_eq!(store.peek(), config);
        let text = fs::read_to_string(store.config_path()).unwrap();
        assert!(text.contains(r#""custom": "{app}-{n}""#), "{text}");
    }

    #[test]
    fn peek_never_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.peek(), Config::default());
        assert!(!store.config_path().exists());
    }

    #[test]
    fn formats_by_extension() {
        for format in ImageFormat::ALL {
            let path = PathBuf::from(format!("a.{}", format.extension()));
            assert_eq!(ImageFormat::from_path(&path), Some(format));
        }
        assert_eq!(
            ImageFormat::from_path(Path::new("x/B.JPEG")),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(
            ImageFormat::from_path(Path::new("c.tiff")),
            Some(ImageFormat::Tiff)
        );
        assert_eq!(ImageFormat::from_path(Path::new("d.txt")), None);
        assert_eq!(ImageFormat::from_path(Path::new("sans-extension")), None);
    }

    #[test]
    fn restore_default_hotkeys_clears_new_actions() {
        let mut config = Config::default();
        *config.hotkey_mut(Mode::Region) = "F13".into();
        *config.hotkey_mut(Mode::Ocr) = "F14".into();
        config.restore_default_hotkeys();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn broken_file_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        fs::write(store.config_path(), "{ pas du json").unwrap();
        assert_eq!(store.load(), Config::default());
        assert!(dir.path().join("config.json.bad").exists());
    }
}
