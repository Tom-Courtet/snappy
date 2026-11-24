//! Dialogue avec l'agent : une requête JSON par ligne, une réponse JSON par ligne.
//!
//! - macOS : socket Unix (`snappy.sock` dans le dossier de données) ;
//! - Windows : TCP sur 127.0.0.1, port choisi par le système et noté dans
//!   `snappy.port` (la bibliothèque standard n'a pas de socket Unix sous Windows).
//!
//! Sert à garder une seule instance de l'agent, à déclencher une capture depuis
//! un terminal ou un autre outil (`snappy --region`, `snappy --screen`), et à la
//! fenêtre des réglages pour suspendre et recharger les raccourcis de l'agent.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const TIMEOUT: Duration = Duration::from_secs(3);

/// Durée d'une suspension des raccourcis ([`Request::SuspendHotkeys`]) : passé ce
/// délai sans nouvelle demande, l'agent les reprend de lui-même. La fenêtre des
/// réglages renouvelle la demande pendant l'enregistrement d'un raccourci : si elle
/// se ferme brutalement, les raccourcis reviennent quand même.
pub const SUSPEND_LEASE: Duration = Duration::from_secs(15);

/// Action globale : type de capture, ou fenêtre à ouvrir. Chacune peut avoir un
/// raccourci global (voir [`crate::config::Config::hotkey`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Sélection d'une zone sur l'écran figé.
    Region,
    /// Écran entier (celui du curseur), ouvert directement dans l'éditeur.
    Screen,
    /// Même zone que la capture précédente, sans sélection.
    Previous,
    /// Tous les écrans, assemblés en une seule image.
    AllScreens,
    /// Fenêtre au premier plan.
    Window,
    /// Sélection d'une zone après le délai de la config (capture différée).
    Delayed,
    /// Capture avec défilement.
    Scrolling,
    /// Copier le texte d'une zone (OCR).
    Ocr,
    /// Décoder un QR code ou un code-barres dans une zone.
    Qr,
    /// Pipette de couleur.
    ColorPicker,
    /// Vidéo de la zone précédente (ou d'une nouvelle s'il n'y en a pas).
    Video,
    /// Vidéo d'une nouvelle zone.
    VideoRegion,
    /// Fenêtre « Espace de travail ».
    Workspace,
}

impl Mode {
    /// Toutes les actions, dans l'ordre des réglages et du menu.
    pub const ALL: [Mode; 13] = [
        Self::Region,
        Self::Previous,
        Self::Screen,
        Self::AllScreens,
        Self::Window,
        Self::Delayed,
        Self::Scrolling,
        Self::Ocr,
        Self::Qr,
        Self::ColorPicker,
        Self::Video,
        Self::VideoRegion,
        Self::Workspace,
    ];

    /// Argument de `snappy --capture`.
    pub fn arg(self) -> &'static str {
        match self {
            Self::Region => "region",
            Self::Screen => "screen",
            Self::Previous => "previous",
            Self::AllScreens => "all-screens",
            Self::Window => "window",
            Self::Delayed => "delayed",
            Self::Scrolling => "scrolling",
            Self::Ocr => "ocr",
            Self::Qr => "qr",
            Self::ColorPicker => "color-picker",
            Self::Video => "video",
            Self::VideoRegion => "video-region",
            Self::Workspace => "workspace",
        }
    }

    pub fn from_arg(arg: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.arg() == arg)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// L'agent est-il là ?
    Ping,
    /// `delay` : secondes d'attente avant la capture (0 : tout de suite).
    Capture {
        mode: Mode,
        #[serde(default)]
        delay: u32,
    },
    Quit,
    /// Ouvre la fenêtre des réglages, ou la passe au premier plan si elle est déjà
    /// ouverte.
    OpenSettings,
    /// Désactive les raccourcis globaux pendant [`SUSPEND_LEASE`] (enregistrement
    /// d'un nouveau raccourci dans les réglages). Réponse : [`Response::Ok`].
    SuspendHotkeys,
    /// Relit la config : raccourcis réenregistrés (fin d'une suspension comprise) et
    /// menu reconstruit. Réponse : [`Response::Reloaded`].
    Reload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Error {
        message: String,
    },
    /// Config rechargée ; `refused` : raccourcis que le système n'a pas acceptés.
    Reloaded {
        refused: Vec<Refused>,
    },
}

/// Raccourci de la config que l'agent n'a pas pu enregistrer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    pub mode: Mode,
    /// Raccourci tel qu'écrit dans la config (`"super+shift+Digit2"`).
    pub hotkey: String,
    /// Raison, en clair (« déjà pris par une autre application »).
    pub reason: String,
}

/// Envoie une requête à l'agent et attend sa réponse.
pub fn request(socket: &Path, req: &Request) -> Result<Response> {
    let mut stream = transport::connect(socket)
        .with_context(|| format!("agent injoignable ({})", socket.display()))?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    write_line(&mut stream, req)?;
    let mut reader = BufReader::new(stream);
    read_line(&mut reader)?.context("l'agent a fermé la connexion sans répondre")
}

/// Ouvre le socket de l'agent. Un fichier laissé par un agent arrêté brutalement est
/// retiré ; si un agent répond encore dessus, c'est une erreur.
pub fn bind(path: &Path) -> Result<transport::Listener> {
    if path.exists() {
        if transport::connect(path).is_ok() {
            bail!("Snappy tourne déjà ({})", path.display());
        }
        std::fs::remove_file(path)
            .with_context(|| format!("suppression de l'ancien socket {}", path.display()))?;
    }
    transport::listen(path).with_context(|| format!("socket {}", path.display()))
}

/// Sert les requêtes sur un fil : `handle` reçoit chaque requête et rend la réponse.
pub fn serve(
    listener: transport::Listener,
    handle: impl Fn(Request) -> Response + Send + 'static,
) -> Result<()> {
    thread::Builder::new()
        .name("snappy-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let result = stream.map_err(anyhow::Error::from).and_then(|stream| {
                    stream.set_read_timeout(Some(TIMEOUT))?;
                    stream.set_write_timeout(Some(TIMEOUT))?;
                    let mut reader = BufReader::new(stream.try_clone()?);
                    if let Some(req) = read_line::<Request>(&mut reader)? {
                        let mut stream = stream;
                        write_line(&mut stream, &handle(req))?;
                    }
                    Ok(())
                });
                if let Err(e) = result {
                    log::warn!("requête sur le socket : {e:#}");
                }
            }
        })
        .context("fil du socket")?;
    Ok(())
}

/// Écrit un message JSON suivi d'un saut de ligne.
fn write_line<T: Serialize>(w: &mut impl Write, msg: &T) -> Result<()> {
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    w.write_all(line.as_bytes())?;
    w.flush()?;
    Ok(())
}

/// Lit un message JSON (une ligne). `None` : fin de flux.
fn read_line<T: for<'de> Deserialize<'de>>(r: &mut impl BufRead) -> Result<Option<T>> {
    let mut line = String::new();
    if std::io::Read::take(&mut *r, 1 << 16).read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(line.trim_end())?))
}

/// Canal local : socket Unix, ou TCP sur la boucle locale sous Windows.
pub mod transport {
    use std::io;
    use std::path::Path;

    #[cfg(unix)]
    pub use std::os::unix::net::{UnixListener as Listener, UnixStream as Stream};

    #[cfg(unix)]
    pub fn connect(path: &Path) -> io::Result<Stream> {
        Stream::connect(path)
    }

    #[cfg(unix)]
    pub fn listen(path: &Path) -> io::Result<Listener> {
        Listener::bind(path)
    }

    #[cfg(windows)]
    pub use std::net::{TcpListener as Listener, TcpStream as Stream};

    /// Port noté dans le fichier par l'agent.
    #[cfg(windows)]
    pub fn connect(path: &Path) -> io::Result<Stream> {
        let port: u16 = std::fs::read_to_string(path)?
            .trim()
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "port illisible"))?;
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        Stream::connect_timeout(&addr, std::time::Duration::from_millis(500))
    }

    /// Écoute sur un port libre de 127.0.0.1 (injoignable depuis le réseau) et le
    /// note dans `path`.
    #[cfg(windows)]
    pub fn listen(path: &Path) -> io::Result<Listener> {
        let listener = Listener::bind(("127.0.0.1", 0))?;
        std::fs::write(path, listener.local_addr()?.port().to_string())?;
        Ok(listener)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_reach_the_handler_and_answers_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snappy.sock");
        let listener = bind(&path).unwrap();
        // Deuxième agent : refusé tant que le premier écoute.
        assert!(bind(&path).is_err());
        serve(listener, |req| match req {
            Request::Capture {
                mode: Mode::Screen, ..
            } => Response::Ok,
            other => Response::Error {
                message: format!("{other:?}"),
            },
        })
        .unwrap();
        assert_eq!(
            request(
                &path,
                &Request::Capture {
                    mode: Mode::Screen,
                    delay: 0
                }
            )
            .unwrap(),
            Response::Ok
        );
        assert_eq!(
            request(&path, &Request::Ping).unwrap(),
            Response::Error {
                message: "Ping".into()
            }
        );
    }

    #[test]
    fn stale_socket_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snappy.sock");
        std::fs::write(&path, b"").unwrap();
        let _listener = bind(&path).expect("socket remplacé");
        assert!(transport::connect(&path).is_ok());
    }

    #[test]
    fn messages_are_tagged_json() {
        let req = Request::Capture {
            mode: Mode::Region,
            delay: 3,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"capture","mode":"region","delay":3}"#);
        // Sans délai (anciennes versions) : 0.
        let req: Request = serde_json::from_str(r#"{"type":"capture","mode":"previous"}"#).unwrap();
        assert_eq!(
            req,
            Request::Capture {
                mode: Mode::Previous,
                delay: 0
            }
        );
        for mode in Mode::ALL {
            assert_eq!(Mode::from_arg(mode.arg()), Some(mode));
        }
    }

    #[test]
    fn settings_messages_are_tagged_json() {
        for (req, json) in [
            (Request::OpenSettings, r#"{"type":"open_settings"}"#),
            (Request::SuspendHotkeys, r#"{"type":"suspend_hotkeys"}"#),
            (Request::Reload, r#"{"type":"reload"}"#),
        ] {
            assert_eq!(serde_json::to_string(&req).unwrap(), json);
            assert_eq!(serde_json::from_str::<Request>(json).unwrap(), req);
        }
        let resp = Response::Reloaded {
            refused: vec![Refused {
                mode: Mode::Previous,
                hotkey: "alt+super+shift+Digit2".into(),
                reason: "déjà pris par une autre application".into(),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            json,
            r#"{"type":"reloaded","refused":[{"mode":"previous","hotkey":"alt+super+shift+Digit2","reason":"déjà pris par une autre application"}]}"#
        );
        assert_eq!(serde_json::from_str::<Response>(&json).unwrap(), resp);
    }

    #[test]
    fn reload_answer_travels_over_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snappy.sock");
        serve(bind(&path).unwrap(), |req| match req {
            Request::Reload => Response::Reloaded {
                refused: vec![Refused {
                    mode: Mode::Region,
                    hotkey: "F13".into(),
                    reason: "test".into(),
                }],
            },
            _ => Response::Ok,
        })
        .unwrap();
        assert_eq!(
            request(&path, &Request::SuspendHotkeys).unwrap(),
            Response::Ok
        );
        match request(&path, &Request::Reload).unwrap() {
            Response::Reloaded { refused } => assert_eq!(refused[0].hotkey, "F13"),
            other => panic!("réponse inattendue : {other:?}"),
        }
    }
}
