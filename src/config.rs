//! Dossier de données et fichier de configuration (`config.json`).
//!
//! macOS : `~/Library/Application Support/Snappy/`. On y trouve aussi les captures
//! d'origine (`captures/`), chacune avec ses annotations à côté (voir
//! [`crate::history`]). L'écriture passe par un fichier temporaire renommé : un
//! plantage en pleine sauvegarde ne corrompt jamais la config.
//!
//! La fenêtre des réglages (`snappy --settings`) l'écrit ; l'agent la relit quand
//! elle le lui demande.

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
        }
    }

    pub fn hotkey_mut(&mut self, mode: Mode) -> &mut String {
        match mode {
            Mode::Region => &mut self.region_hotkey,
            Mode::Screen => &mut self.screen_hotkey,
            Mode::Previous => &mut self.previous_hotkey,
        }
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

    #[test]
    fn broken_file_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        fs::write(store.config_path(), "{ pas du json").unwrap();
        assert_eq!(store.load(), Config::default());
        assert!(dir.path().join("config.json.bad").exists());
    }
}
