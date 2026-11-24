//! Agent : discret dans la barre des menus, il attend les raccourcis globaux et lance
//! les captures.
//!
//! Chaque capture tourne dans un processus à part (`snappy --capture region|screen`) :
//! l'agent reste minuscule en mémoire, et la mémoire d'une capture (image Retina,
//! fenêtres) est rendue au système dès qu'elle se termine. Les réglages
//! (`snappy --settings`) aussi, un seul à la fois : l'agent garde le processus et le
//! repasse au premier plan si on les redemande.
//!
//! Répartition :
//! - **thread principal** : boucle winit sans fenêtre (pas d'icône dans le Dock),
//!   icône de la barre des menus ([`tray`]) et raccourcis globaux ([`hotkeys`]), que
//!   macOS veut sur ce thread ;
//! - **fil du socket** ([`crate::ipc`]) : `snappy --region` depuis un terminal, et
//!   les demandes des réglages (suspendre ou recharger les raccourcis), transmises
//!   au thread principal par l'[`EventLoopProxy`].

mod foreground;
pub mod hotkeys;
mod layout;
mod tray;

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

use crate::capture;
use crate::config::{Config, Store};
use crate::history;
use crate::ipc::{self, Mode, Refused, Request, Response};
use hotkeys::Hotkeys;
use tray::{MenuChoice, MenuState, Tray};

/// Événements envoyés au thread principal.
#[derive(Debug)]
pub enum UiEvent {
    /// Compte à rebours d'une capture différée (`None` : terminé).
    Countdown(Option<u32>),
    /// Demande des réglages ; la réponse (raccourcis refusés) part par le canal.
    Hotkeys(HotkeyCommand, mpsc::Sender<Vec<Refused>>),
    Quit,
}

/// Ce que les réglages demandent aux raccourcis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyCommand {
    /// Les retirer pendant [`ipc::SUSPEND_LEASE`].
    Suspend,
    /// Relire la config, les réenregistrer et reconstruire le menu.
    Reload,
}

/// Attente maximale de la réponse du thread principal (le client attend 3 s).
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(2);

/// Lance l'agent ; rend la main quand on le quitte.
pub fn run(store: Store) -> Result<()> {
    let socket = store.socket_path();
    let listener = ipc::bind(&socket)?;
    let _socket_guard = SocketGuard(socket);

    let mut builder = EventLoop::<UiEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        // Pas d'icône dans le Dock ni de menu d'application : seulement la barre des menus.
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_default_menu(false);
    }
    let event_loop = builder.build().context("boucle d'événements")?;
    let proxy = event_loop.create_proxy();

    let launcher = Arc::new(Launcher::new(proxy.clone())?);
    let ipc_launcher = Arc::clone(&launcher);
    let ipc_proxy = proxy.clone();
    ipc::serve(listener, move |req| {
        let result = match req {
            Request::Ping => Ok(Response::Ok),
            Request::Capture { mode, delay } => {
                ipc_launcher.capture(mode, delay).map(|()| Response::Ok)
            }
            Request::Quit => {
                let _ = ipc_proxy.send_event(UiEvent::Quit);
                Ok(Response::Ok)
            }
            Request::OpenSettings => ipc_launcher.open_settings().map(|()| Response::Ok),
            Request::SuspendHotkeys => {
                on_main_thread(&ipc_proxy, HotkeyCommand::Suspend).map(|_| Response::Ok)
            }
            Request::Reload => on_main_thread(&ipc_proxy, HotkeyCommand::Reload)
                .map(|refused| Response::Reloaded { refused }),
        };
        result.unwrap_or_else(|e| Response::Error {
            message: format!("{e:#}"),
        })
    })?;

    if !capture::has_permission() {
        log::warn!("autorisation « Enregistrement de l'écran » absente : demande au système");
        capture::request_permission();
    }

    let mut app = App {
        store,
        launcher,
        proxy,
        tray: None,
        hotkeys: None,
        menu: None,
        suspended_until: None,
    };
    event_loop
        .run_app(&mut app)
        .context("boucle d'événements")?;
    log::info!("agent arrêté");
    Ok(())
}

/// Transmet une demande des réglages au thread principal et attend sa réponse.
fn on_main_thread(proxy: &EventLoopProxy<UiEvent>, command: HotkeyCommand) -> Result<Vec<Refused>> {
    let (tx, rx) = mpsc::channel();
    proxy
        .send_event(UiEvent::Hotkeys(command, tx))
        .map_err(|_| anyhow::anyhow!("l'agent s'arrête"))?;
    rx.recv_timeout(MAIN_THREAD_TIMEOUT)
        .context("l'agent n'a pas répondu à temps")
}

/// Retire le fichier du socket quand l'agent s'arrête.
struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Lance les captures, une seule à la fois, et la fenêtre des réglages.
pub struct Launcher {
    exe: PathBuf,
    running: Mutex<Option<Child>>,
    /// Une capture différée attend son tour.
    waiting: AtomicBool,
    /// Fenêtre des réglages, une seule à la fois.
    settings: Mutex<Option<Child>>,
    proxy: EventLoopProxy<UiEvent>,
}

impl Launcher {
    fn new(proxy: EventLoopProxy<UiEvent>) -> Result<Self> {
        let exe = std::env::current_exe().context("chemin de l'exécutable")?;
        Ok(Self {
            exe,
            running: Mutex::new(None),
            waiting: AtomicBool::new(false),
            settings: Mutex::new(None),
            proxy,
        })
    }

    /// Démarre une capture, tout de suite ou après `delay` secondes (compte à
    /// rebours dans la barre des menus), sauf si une autre est déjà en cours.
    pub fn capture(self: &Arc<Self>, mode: Mode, delay: u32) -> Result<()> {
        if delay == 0 {
            return self.start(mode);
        }
        if self.waiting.swap(true, Ordering::SeqCst) {
            log::info!("capture différée déjà en attente");
            return Ok(());
        }
        let this = Arc::clone(self);
        thread::Builder::new()
            .name("snappy-delay".into())
            .spawn(move || {
                for n in (1..=delay).rev() {
                    let _ = this.proxy.send_event(UiEvent::Countdown(Some(n)));
                    thread::sleep(Duration::from_secs(1));
                }
                let _ = this.proxy.send_event(UiEvent::Countdown(None));
                this.waiting.store(false, Ordering::SeqCst);
                if let Err(e) = this.start(mode) {
                    log::error!("{e:#}");
                }
            })
            .context("fil de la capture différée")?;
        Ok(())
    }

    /// Lance le processus de capture, sauf si une autre est encore à l'écran.
    fn start(&self, mode: Mode) -> Result<()> {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(child) = running.as_mut()
            && matches!(child.try_wait(), Ok(None))
        {
            log::info!("capture déjà en cours");
            return Ok(());
        }
        crate::platform::allow_foreground();
        let child = Command::new(&self.exe)
            .args(["--capture", mode.arg()])
            .spawn()
            .context("lancement de la capture")?;
        log::info!("capture {} lancée (pid {})", mode.arg(), child.id());
        *running = Some(child);
        Ok(())
    }

    /// Ouvre la fenêtre des réglages, ou la passe au premier plan si elle est encore
    /// ouverte.
    pub fn open_settings(&self) -> Result<()> {
        let mut settings = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(child) = settings.as_mut()
            && matches!(child.try_wait(), Ok(None))
        {
            foreground::bring_to_front(child.id());
            return Ok(());
        }
        crate::platform::allow_foreground();
        let child = Command::new(&self.exe)
            .arg(crate::settings::WINDOW_ARG)
            .spawn()
            .context("ouverture des réglages")?;
        log::info!("réglages ouverts (pid {})", child.id());
        *settings = Some(child);
        Ok(())
    }
}

/// Partie « thread principal » de l'agent.
struct App {
    store: Store,
    launcher: Arc<Launcher>,
    proxy: EventLoopProxy<UiEvent>,
    tray: Option<Tray>,
    hotkeys: Option<Hotkeys>,
    /// Contenu actuel du menu (reconstruit seulement s'il change).
    menu: Option<MenuState>,
    /// Raccourcis suspendus (enregistrement dans les réglages) jusqu'à cet instant.
    suspended_until: Option<Instant>,
}

impl App {
    /// Icône et raccourcis, une fois la boucle lancée (exigence de macOS).
    fn start(&mut self) {
        let config = self.store.load();
        self.register_hotkeys(&config);

        let launcher = Arc::clone(&self.launcher);
        let store = self.store.clone();
        let proxy = self.proxy.clone();
        let on_choice = move |choice| match choice {
            MenuChoice::Capture(mode) => {
                if let Err(e) = launcher.capture(mode, 0) {
                    log::error!("{e:#}");
                }
            }
            MenuChoice::CaptureDelayed => {
                let delay = store.load().delay_seconds.max(1);
                if let Err(e) = launcher.capture(Mode::Region, delay) {
                    log::error!("{e:#}");
                }
            }
            MenuChoice::OpenCaptures => history::open_dir(&store.captures_dir()),
            MenuChoice::OpenSaved => history::open_dir(&store.save_dir(&store.load())),
            MenuChoice::OpenSettings => {
                if let Err(e) = launcher.open_settings() {
                    log::error!("{e:#}");
                }
            }
            MenuChoice::Permission => {
                capture::request_permission();
                open_privacy_settings();
            }
            MenuChoice::Quit => {
                let _ = proxy.send_event(UiEvent::Quit);
            }
        };
        let menu = self.menu_state(&config);
        match Tray::new(&menu, on_choice) {
            Ok(tray) => {
                self.tray = Some(tray);
                self.menu = Some(menu);
            }
            Err(e) => log::error!("icône de la barre des menus impossible : {e:#}"),
        }
    }

    /// (Ré)enregistre les raccourcis de la config ; rend ceux que le système refuse.
    fn register_hotkeys(&mut self, config: &Config) -> Vec<Refused> {
        // Les anciens d'abord : ils libèrent leurs combinaisons et le gestionnaire
        // d'événements, qui est global.
        self.hotkeys = None;
        let launcher = Arc::clone(&self.launcher);
        let hotkeys = Hotkeys::register(config, move |mode| {
            if let Err(e) = launcher.capture(mode, 0) {
                log::error!("{e:#}");
            }
        });
        let refused = hotkeys.refused();
        self.hotkeys = Some(hotkeys);
        refused
    }

    fn menu_state(&self, config: &Config) -> MenuState {
        MenuState {
            labels: self
                .hotkeys
                .as_ref()
                .map(Hotkeys::labels)
                .unwrap_or_default(),
            delay: config.delay_seconds.max(1),
            needs_permission: !capture::has_permission(),
        }
    }

    /// Relit la config : raccourcis (fin de suspension comprise) et menu.
    fn reload(&mut self) -> Vec<Refused> {
        self.suspended_until = None;
        let config = self.store.load();
        let refused = self.register_hotkeys(&config);
        let menu = self.menu_state(&config);
        if self.menu.as_ref() != Some(&menu)
            && let Some(tray) = &self.tray
        {
            match tray.set_menu(&menu) {
                Ok(()) => self.menu = Some(menu),
                Err(e) => log::error!("menu non reconstruit : {e:#}"),
            }
        }
        log::info!("config rechargée");
        refused
    }

    /// Retire les raccourcis pendant que les réglages en enregistrent un nouveau :
    /// sinon l'ancien déclencherait une capture au lieu d'arriver dans la fenêtre.
    fn suspend(&mut self) {
        if self.suspended_until.is_none() {
            self.hotkeys = None;
            log::info!("raccourcis suspendus");
        }
        self.suspended_until = Some(Instant::now() + ipc::SUSPEND_LEASE);
    }
}

impl ApplicationHandler<UiEvent> for App {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if cause == StartCause::Init {
            event_loop.set_control_flow(ControlFlow::Wait);
            self.start();
        }
    }

    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UiEvent) {
        match event {
            UiEvent::Countdown(n) => {
                if let Some(tray) = &mut self.tray {
                    tray.countdown(n);
                }
            }
            UiEvent::Hotkeys(command, reply) => {
                let refused = match command {
                    HotkeyCommand::Suspend => {
                        self.suspend();
                        Vec::new()
                    }
                    HotkeyCommand::Reload => self.reload(),
                };
                let _ = reply.send(refused);
            }
            UiEvent::Quit => {
                // Raccourcis et icône retirés avant de rendre la main.
                self.hotkeys = None;
                self.tray = None;
                event_loop.exit();
            }
        }
    }

    /// Fin de suspension : si les réglages n'ont pas renouvelé leur demande (fermés
    /// brutalement), les raccourcis reviennent d'eux-mêmes.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(until) = self.suspended_until
            && Instant::now() >= until
        {
            log::info!("suspension des raccourcis expirée");
            self.reload();
        }
        event_loop.set_control_flow(match self.suspended_until {
            Some(until) => ControlFlow::WaitUntil(until),
            None => ControlFlow::Wait,
        });
    }
}

/// Réglages Système > Confidentialité > Enregistrement de l'écran.
fn open_privacy_settings() {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .spawn();
}
