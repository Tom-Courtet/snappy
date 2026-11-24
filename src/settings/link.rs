//! Liaison avec l'agent, sur un fil à part : la fenêtre ne se bloque jamais sur le
//! socket, et marche aussi sans agent (les demandes échouent sans bruit).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use crate::ipc::{self, Refused, Request, Response};

/// Demande à l'agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Suspendre les raccourcis (à renouveler, voir [`ipc::SUSPEND_LEASE`]).
    Suspend,
    /// Relire la config (et reprendre les raccourcis).
    Reload,
}

/// Ce que l'on sait de l'agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// `None` : pas encore de réponse ; `Some(false)` : l'agent ne tourne pas.
    pub running: Option<bool>,
    /// Raccourcis refusés lors du dernier rechargement.
    pub refused: Vec<Refused>,
    /// Erreur rendue par l'agent.
    pub error: Option<String>,
}

pub struct Agent {
    tx: mpsc::Sender<Command>,
    status: Arc<Mutex<Status>>,
    socket: PathBuf,
}

impl Agent {
    /// `repaint` : appelé après chaque réponse (redessiner la fenêtre).
    pub fn new(socket: PathBuf, repaint: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Command>();
        let status = Arc::new(Mutex::new(Status::default()));
        let shared = Arc::clone(&status);
        let path = socket.clone();
        let spawned = thread::Builder::new()
            .name("snappy-agent-link".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    // Seule la dernière demande en attente compte : une rafale de
                    // changements (curseur du délai) ne recharge qu'une fois.
                    let command = rx.try_iter().last().unwrap_or(first);
                    let update = send(&path, command);
                    let mut status = shared.lock().unwrap_or_else(|e| e.into_inner());
                    update(&mut status);
                    drop(status);
                    repaint();
                }
            });
        if let Err(e) = spawned {
            log::warn!("liaison avec l'agent impossible : {e}");
        }
        Self { tx, status, socket }
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Recharge tout de suite, sans passer par le fil (fermeture de la fenêtre : le
    /// fil ne vivrait pas assez longtemps).
    pub fn reload_now(&self) {
        let _ = ipc::request(&self.socket, &Request::Reload);
    }
}

/// Envoie la demande ; rend la mise à jour de l'état.
fn send(socket: &std::path::Path, command: Command) -> impl FnOnce(&mut Status) {
    let request = match command {
        Command::Suspend => Request::SuspendHotkeys,
        Command::Reload => Request::Reload,
    };
    let answer = ipc::request(socket, &request);
    move |status: &mut Status| match answer {
        Err(e) => {
            log::debug!("agent injoignable : {e:#}");
            status.running = Some(false);
            status.refused.clear();
            status.error = None;
        }
        Ok(response) => {
            status.running = Some(true);
            status.error = None;
            match response {
                Response::Reloaded { refused } => status.refused = refused,
                Response::Error { message } => status.error = Some(message),
                Response::Ok => {}
            }
        }
    }
}
