//! Une seule barre vidéo à la fois, et son arrêt depuis l'extérieur.
//!
//! La barre garde un verrou exclusif sur `<données>/recording.pid` (qui contient son
//! pid, pour information) tant qu'elle est ouverte ; le système le relâche à la fin
//! du processus, même après un plantage. Un autre `snappy --capture video` qui
//! trouve le verrou pris ne rouvre pas de barre : il dépose `recording.stop`, que la
//! barre relève quelques fois par seconde (départ si elle attend, arrêt si elle
//! enregistre). L'agent n'a besoin de rien savoir de plus : voir [`request_stop`].

#![cfg_attr(not(target_os = "macos"), allow(dead_code))] // enregistrement propre à macOS

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::config::Store;

const PID_FILE: &str = "recording.pid";
const STOP_FILE: &str = "recording.stop";

fn data_file(store: &Store, name: &str) -> PathBuf {
    store
        .config_path()
        .parent()
        .map(|d| d.join(name))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Barre vidéo en cours : tient le verrou jusqu'à sa destruction.
pub struct Session {
    file: File,
    stop: PathBuf,
}

impl Session {
    /// Prend la place de la barre vidéo. `None` : une autre barre est ouverte.
    pub fn acquire(store: &Store) -> Result<Option<Session>> {
        let path = data_file(store, PID_FILE);
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("ouverture de {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("verrou de {}", path.display()));
            }
        }
        file.set_len(0)?;
        write!(file, "{}", std::process::id())?;
        let stop = data_file(store, STOP_FILE);
        // Demande restée d'une barre précédente : sans objet.
        let _ = fs::remove_file(&stop);
        Ok(Some(Session { file, stop }))
    }

    /// Une demande d'arrêt (ou de départ) est arrivée depuis le dernier appel.
    pub fn take_request(&self) -> bool {
        fs::remove_file(&self.stop).is_ok()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        let _ = fs::remove_file(&self.stop);
    }
}

/// Une barre vidéo est-elle ouverte ? Si oui, lui demande d'arrêter
/// l'enregistrement (ou de le démarrer s'il n'a pas commencé) et rend `true`.
///
/// Pour l'agent : à appeler quand le raccourci « Vidéo » arrive alors qu'une
/// capture tourne encore, et pour un éventuel « Arrêter l'enregistrement » du menu.
pub fn request_stop(store: &Store) -> bool {
    let path = data_file(store, PID_FILE);
    let Ok(file) = OpenOptions::new().write(true).open(&path) else {
        return false;
    };
    match file.try_lock() {
        // Personne ne le tient : pas de barre ouverte.
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(_) => match fs::write(data_file(store, STOP_FILE), b"stop") {
            Ok(()) => true,
            Err(e) => {
                log::warn!("demande d'arrêt de l'enregistrement impossible : {e}");
                false
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_session_at_a_time_and_stop_requests() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert!(!request_stop(&store), "pas de barre");
        let session = Session::acquire(&store).unwrap().unwrap();
        assert!(Session::acquire(&store).unwrap().is_none(), "déjà prise");
        assert!(!session.take_request());
        assert!(request_stop(&store));
        assert!(session.take_request());
        assert!(!session.take_request(), "consommée");
        let pid = fs::read_to_string(dir.path().join(PID_FILE)).unwrap();
        assert_eq!(pid, std::process::id().to_string());
        drop(session);
        assert!(!request_stop(&store), "verrou relâché");
        assert!(Session::acquire(&store).unwrap().is_some());
    }
}
