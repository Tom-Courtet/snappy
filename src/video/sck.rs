//! Enregistrement macOS : ScreenCaptureKit, `SCStream` + `SCRecordingOutput`
//! (macOS 15+). Le système encode lui-même en H.264 et écrit le MP4 : pas
//! d'`AVAssetWriter`, pas de tampon d'images côté Rust.
//!
//! - Filtre : l'écran de la zone, **sans les fenêtres de Snappy** (barre, cadre),
//!   qui sont en plus marquées « non partageables » (`NSWindow.sharingType`).
//! - Pause : la sortie d'enregistrement est retirée du flux (le segment se termine
//!   proprement), puis une nouvelle sortie écrit le segment suivant à la reprise. Le
//!   flux, lui, continue : la reprise est immédiate. Les segments sont assemblés à la
//!   fin ([`super::av::concat`]).
//! - Micro coupé en direct : changer la configuration d'un flux arrête la sortie
//!   d'enregistrement en cours ; on coupe donc un segment au même moment.
//!
//! Toutes les réponses de ScreenCaptureKit (blocs de fin, délégué) arrivent sur ses
//! propres files : on les attend avec des canaux, dans le fil de [`Worker`], jamais
//! sur le fil de l'interface.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{AVFileTypeMPEG4, AVVideoCodecTypeH264};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_media::CMTime;
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol, NSString, NSURL};
use objc2_screen_capture_kit::{
    SCContentFilter, SCRecordingOutput, SCRecordingOutputConfiguration, SCRecordingOutputDelegate,
    SCRunningApplication, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCWindow,
};

use super::region::Rect;

/// Délai maximal d'une réponse de ScreenCaptureKit.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Code d'erreur ScreenCaptureKit « autorisation refusée ».
const USER_DECLINED: isize = -3801;

/// Ce qu'il faut enregistrer.
#[derive(Debug, Clone)]
pub struct Params {
    pub display_id: u32,
    /// Zone en points globaux (déjà dans l'écran, voir [`super::region::fit`]).
    pub rect: Rect,
    /// Taille de la vidéo en pixels (paire).
    pub size: (u32, u32),
    pub fps: u32,
    pub system_audio: bool,
    pub microphone: bool,
    pub mark_clicks: bool,
    /// Dossier des segments (créé au besoin).
    pub segment_dir: PathBuf,
}

/// ScreenCaptureKit sait écrire lui-même un fichier (macOS 15+).
pub fn available() -> bool {
    AnyClass::get(c"SCRecordingOutput").is_some()
}

/// Nouvelles venues du délégué.
#[derive(Debug)]
enum Note {
    Started,
    Finished,
    Failed(String),
    StreamStopped(String),
}

struct Ivars {
    tx: Mutex<Sender<Note>>,
}

define_class!(
    // SAFETY : NSObject n'a pas d'exigence de sous-classement ; pas de Drop.
    #[unsafe(super(NSObject))]
    #[name = "SnappyVideoRecordingDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl SCRecordingOutputDelegate for Delegate {
        #[unsafe(method(recordingOutputDidStartRecording:))]
        fn did_start(&self, _output: &SCRecordingOutput) {
            self.note(Note::Started);
        }

        #[unsafe(method(recordingOutput:didFailWithError:))]
        fn did_fail(&self, _output: &SCRecordingOutput, error: &NSError) {
            self.note(Note::Failed(describe(error)));
        }

        #[unsafe(method(recordingOutputDidFinishRecording:))]
        fn did_finish(&self, _output: &SCRecordingOutput) {
            self.note(Note::Finished);
        }
    }

    unsafe impl SCStreamDelegate for Delegate {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            self.note(Note::StreamStopped(describe(error)));
        }
    }
);

impl Delegate {
    fn new(tx: Sender<Note>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { tx: Mutex::new(tx) });
        // SAFETY : `init` de NSObject sur une instance fraîchement allouée.
        unsafe { msg_send![super(this), init] }
    }

    fn note(&self, note: Note) {
        if let Ok(tx) = self.ivars().tx.lock() {
            let _ = tx.send(note);
        }
    }
}

fn describe(error: &NSError) -> String {
    if error.code() == USER_DECLINED {
        return "autorisation refusée : accorde « Enregistrement de l'écran et audio \
                système » à Snappy dans Réglages Système > Confidentialité et sécurité"
            .into();
    }
    format!("{} ({})", error.localizedDescription(), error.code())
}

/// Appelle une méthode à bloc de fin `(NSError?)` et attend sa réponse.
fn wait_completion(call: impl FnOnce(&DynBlock<dyn Fn(*mut NSError)>)) -> Result<()> {
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let block = RcBlock::new(move |error: *mut NSError| {
        // SAFETY : ScreenCaptureKit passe une erreur valide ou nil.
        let result = match unsafe { error.as_ref() } {
            Some(e) => Err(describe(e)),
            None => Ok(()),
        };
        let _ = tx.send(result);
    });
    call(&block);
    rx.recv_timeout(TIMEOUT)
        .map_err(|_| anyhow!("ScreenCaptureKit ne répond pas"))?
        .map_err(|e| anyhow!(e))
}

fn shareable_content() -> Result<Retained<SCShareableContent>> {
    let (tx, rx) = mpsc::channel::<Result<Retained<SCShareableContent>, String>>();
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY : pointeurs valides ou nuls, fournis par ScreenCaptureKit ;
            // `retain` garde le contenu au-delà du bloc.
            let result = match unsafe { Retained::retain(content) } {
                Some(content) => Ok(content),
                None => Err(unsafe { error.as_ref() }
                    .map(describe)
                    .unwrap_or_else(|| "contenu de l'écran indisponible".into())),
            };
            let _ = tx.send(result);
        },
    );
    // SAFETY : bloc conforme à la signature attendue, gardé vivant par l'appelé.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    rx.recv_timeout(TIMEOUT)
        .map_err(|_| anyhow!("ScreenCaptureKit ne répond pas"))?
        .map_err(|e| anyhow!(e))
}

/// Un enregistrement en cours (flux démarré).
struct Recorder {
    stream: Retained<SCStream>,
    config: Retained<SCStreamConfiguration>,
    delegate: Retained<Delegate>,
    notes: Receiver<Note>,
    /// Sortie du segment en cours (`None` : en pause).
    output: Option<Retained<SCRecordingOutput>>,
    segments: Vec<PathBuf>,
    dir: PathBuf,
    running: bool,
}

impl Recorder {
    fn start(p: &Params) -> Result<Self> {
        if !available() {
            bail!("l'enregistrement vidéo demande macOS 15 ou plus récent");
        }
        fs::create_dir_all(&p.segment_dir)
            .with_context(|| format!("création de {}", p.segment_dir.display()))?;
        let content = shareable_content()?;
        // SAFETY (bloc) : accesseurs en lecture de ScreenCaptureKit.
        let (filter, display_frame) = unsafe {
            let displays = content.displays();
            let display = displays
                .iter()
                .find(|d| d.displayID() == p.display_id)
                .with_context(|| {
                    let ids: Vec<u32> = displays.iter().map(|d| d.displayID()).collect();
                    format!("écran {} introuvable (écrans : {ids:?})", p.display_id)
                })?;
            let pid = std::process::id() as i32;
            let own: Vec<Retained<SCRunningApplication>> = content
                .applications()
                .iter()
                .filter(|a| a.processID() == pid)
                .collect();
            let filter = if own.is_empty() {
                // Lancé hors du bundle : on exclut au moins nos fenêtres connues.
                let windows: Vec<Retained<SCWindow>> = content
                    .windows()
                    .iter()
                    .filter(|w| w.owningApplication().is_some_and(|a| a.processID() == pid))
                    .collect();
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::from_retained_slice(&windows),
                )
            } else {
                SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::from_retained_slice(&own),
                    &NSArray::new(),
                )
            };
            (filter, display.frame())
        };

        // SAFETY : réglages d'un objet de configuration neuf.
        let config = unsafe {
            let config = SCStreamConfiguration::new();
            let r = &p.rect;
            config.setSourceRect(CGRect::new(
                CGPoint::new(r.x - display_frame.origin.x, r.y - display_frame.origin.y),
                CGSize::new(r.width, r.height),
            ));
            config.setWidth(p.size.0 as usize);
            config.setHeight(p.size.1 as usize);
            config.setMinimumFrameInterval(CMTime::new(1, p.fps.clamp(1, 120) as i32));
            config.setQueueDepth(6);
            config.setShowsCursor(true);
            config.setShowMouseClicks(p.mark_clicks);
            config.setCapturesAudio(p.system_audio);
            config.setExcludesCurrentProcessAudio(true);
            config.setCaptureMicrophone(p.microphone);
            config
        };

        let (tx, notes) = mpsc::channel();
        let delegate = Delegate::new(tx);
        // SAFETY : filtre et configuration valides ; le délégué est gardé par `self`.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*delegate)),
            )
        };
        let mut rec = Self {
            stream,
            config,
            delegate,
            notes,
            output: None,
            segments: Vec::new(),
            dir: p.segment_dir.clone(),
            running: false,
        };
        // La sortie avant le départ : la toute première image est enregistrée.
        rec.attach_output()?;
        // SAFETY : bloc conforme, gardé vivant par l'appelé.
        wait_completion(|b| unsafe { rec.stream.startCaptureWithCompletionHandler(Some(b)) })
            .context("démarrage de la capture")?;
        rec.running = true;
        rec.wait_for(|n| matches!(n, Note::Started))
            .context("démarrage de l'enregistrement")?;
        Ok(rec)
    }

    /// Nouvelle sortie d'enregistrement vers le segment suivant.
    fn attach_output(&mut self) -> Result<()> {
        let path = self
            .dir
            .join(format!("segment-{}.mp4", self.segments.len() + 1));
        let _ = fs::remove_file(&path);
        // SAFETY : objets neufs ; constantes AVFoundation.
        let output = unsafe {
            let config = SCRecordingOutputConfiguration::new();
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            config.setOutputURL(&url);
            if let Some(codec) = AVVideoCodecTypeH264 {
                config.setVideoCodecType(codec);
            }
            if let Some(mp4) = AVFileTypeMPEG4 {
                config.setOutputFileType(mp4);
            }
            SCRecordingOutput::initWithConfiguration_delegate(
                SCRecordingOutput::alloc(),
                &config,
                ProtocolObject::from_ref(&*self.delegate),
            )
        };
        // SAFETY : sortie valide, flux vivant.
        unsafe { self.stream.addRecordingOutput_error(&output) }
            .map_err(|e| anyhow!(describe(&e)))
            .context("ajout de la sortie d'enregistrement")?;
        self.segments.push(path);
        self.output = Some(output);
        Ok(())
    }

    /// Termine le segment en cours (le fichier est complet au retour).
    fn detach_output(&mut self) -> Result<()> {
        let Some(output) = self.output.take() else {
            return Ok(());
        };
        // SAFETY : sortie ajoutée à ce flux.
        unsafe { self.stream.removeRecordingOutput_error(&output) }
            .map_err(|e| anyhow!(describe(&e)))
            .context("fin du segment")?;
        self.wait_for(|n| matches!(n, Note::Finished))
    }

    fn pause(&mut self) -> Result<()> {
        self.detach_output()
    }

    fn resume(&mut self) -> Result<()> {
        if self.output.is_some() {
            return Ok(());
        }
        self.attach_output()?;
        self.wait_for(|n| matches!(n, Note::Started))
    }

    /// Micro actif ou coupé (voir l'en-tête : coupe un segment si on enregistre).
    fn set_microphone(&mut self, on: bool) -> Result<()> {
        let recording = self.output.is_some();
        self.detach_output()?;
        // SAFETY : configuration de ce flux ; bloc conforme.
        unsafe { self.config.setCaptureMicrophone(on) };
        wait_completion(|b| unsafe {
            self.stream
                .updateConfiguration_completionHandler(&self.config, Some(b))
        })
        .context("changement du micro")?;
        if recording {
            self.resume()?;
        }
        Ok(())
    }

    /// Arrête tout ; rend les segments écrits, dans l'ordre.
    fn stop(mut self) -> Result<Vec<PathBuf>> {
        let had_output = self.output.take().is_some();
        self.running = false;
        // SAFETY : bloc conforme, gardé vivant par l'appelé.
        wait_completion(|b| unsafe { self.stream.stopCaptureWithCompletionHandler(Some(b)) })
            .context("arrêt de la capture")?;
        if had_output {
            // L'arrêt du flux termine aussi le segment en cours.
            self.wait_for(|n| matches!(n, Note::Finished))?;
        }
        let segments = std::mem::take(&mut self.segments)
            .into_iter()
            .filter(|p| fs::metadata(p).is_ok_and(|m| m.len() > 0))
            .collect();
        Ok(segments)
    }

    /// Attend la nouvelle voulue ; une erreur du délégué l'emporte.
    fn wait_for(&self, wanted: impl Fn(&Note) -> bool) -> Result<()> {
        loop {
            let note = self
                .notes
                .recv_timeout(TIMEOUT)
                .map_err(|_| anyhow!("ScreenCaptureKit ne répond pas"))?;
            match note {
                n if wanted(&n) => return Ok(()),
                Note::Failed(e) | Note::StreamStopped(e) => bail!(e),
                _ => {}
            }
        }
    }

    /// Erreur survenue en route (flux coupé, disque plein…).
    fn failure(&self) -> Option<String> {
        loop {
            match self.notes.try_recv().ok()? {
                Note::Failed(e) | Note::StreamStopped(e) => return Some(e),
                _ => {}
            }
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if self.running {
            // SAFETY : flux vivant ; pas de bloc de fin.
            unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
        }
    }
}

/// Ordre de l'interface au fil d'enregistrement.
#[derive(Debug)]
pub enum Command {
    /// Démarre ; le fichier final sera `output`.
    Start(Params, PathBuf),
    Pause,
    Resume,
    Microphone(bool),
    /// Arrête et assemble le fichier final.
    Stop,
    /// Arrête et jette l'enregistrement (Refaire, ✕).
    Discard,
}

/// Réponse du fil d'enregistrement.
#[derive(Debug)]
pub enum Event {
    Recording,
    Paused,
    Resumed,
    Microphone(bool),
    /// Assemblage des segments en cours.
    Finishing,
    Saved(PathBuf),
    Discarded,
    Failed(String),
}

/// Fil qui possède les objets ScreenCaptureKit.
pub struct Worker {
    tx: Sender<Command>,
    rx: Receiver<Event>,
}

impl Worker {
    pub fn spawn(repaint: impl Fn() + Send + 'static) -> Result<Self> {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        thread::Builder::new()
            .name("snappy-video".into())
            .spawn(move || work(&commands, &events, &repaint))
            .context("fil d'enregistrement")?;
        Ok(Self { tx, rx })
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.rx.try_recv().ok()
    }
}

fn work(commands: &Receiver<Command>, events: &Sender<Event>, repaint: &dyn Fn()) {
    let mut rec: Option<Recorder> = None;
    let mut output = PathBuf::new();
    let send = |e: Event| {
        let _ = events.send(e);
        repaint();
    };
    loop {
        let command = match commands.recv_timeout(Duration::from_millis(250)) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => {
                if let Some(e) = rec.as_ref().and_then(Recorder::failure) {
                    log::error!("enregistrement interrompu : {e}");
                    // Ce qui a été écrit jusque-là est gardé.
                    if let Some(r) = rec.take() {
                        let dir = r.dir.clone();
                        let segments = r.segments.clone();
                        drop(r);
                        finish(segments, &output, &dir, &send, Some(e));
                    }
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let result: Result<()> = match command {
            Command::Start(params, out) => match Recorder::start(&params) {
                Ok(r) => {
                    rec = Some(r);
                    output = out;
                    send(Event::Recording);
                    Ok(())
                }
                Err(e) => Err(e),
            },
            Command::Pause => with(&mut rec, Recorder::pause).map(|()| send(Event::Paused)),
            Command::Resume => with(&mut rec, Recorder::resume).map(|()| send(Event::Resumed)),
            Command::Microphone(on) => {
                with(&mut rec, |r| r.set_microphone(on)).map(|()| send(Event::Microphone(on)))
            }
            Command::Stop => match rec.take() {
                Some(r) => {
                    let dir = r.dir.clone();
                    send(Event::Finishing);
                    match r.stop() {
                        Ok(segments) => {
                            finish(segments, &output, &dir, &send, None);
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
                None => Ok(()),
            },
            Command::Discard => {
                if let Some(r) = rec.take() {
                    let dir = r.dir.clone();
                    let _ = r.stop();
                    remove_segments(&dir);
                }
                send(Event::Discarded);
                Ok(())
            }
        };
        if let Err(e) = result {
            log::error!("enregistrement : {e:#}");
            send(Event::Failed(format!("{e:#}")));
        }
    }
    if let Some(r) = rec.take() {
        let dir = r.dir.clone();
        drop(r);
        remove_segments(&dir);
    }
}

fn with(rec: &mut Option<Recorder>, f: impl FnOnce(&mut Recorder) -> Result<()>) -> Result<()> {
    match rec.as_mut() {
        Some(r) => f(r),
        None => Ok(()),
    }
}

/// Segments → fichier final (renommage, ou assemblage s'il y a eu des pauses).
fn finish(
    segments: Vec<PathBuf>,
    output: &std::path::Path,
    dir: &std::path::Path,
    send: &dyn Fn(Event),
    interrupted: Option<String>,
) {
    let result = (|| -> Result<()> {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        match segments.as_slice() {
            [] => bail!("rien n'a été enregistré"),
            [one] => fs::rename(one, output)
                .with_context(|| format!("écriture de {}", output.display()))?,
            many => super::av::concat(many, output).context("assemblage des segments")?,
        }
        Ok(())
    })();
    remove_segments(dir);
    match (result, interrupted) {
        (Ok(()), None) => send(Event::Saved(output.to_owned())),
        (Ok(()), Some(e)) => {
            send(Event::Failed(format!(
                "enregistrement interrompu ({e}) ; début gardé dans {}",
                output.display()
            )));
            send(Event::Saved(output.to_owned()));
        }
        (Err(e), _) => send(Event::Failed(format!("{e:#}"))),
    }
}

/// Efface les segments (fichiers créés par nous, dans notre dossier temporaire).
fn remove_segments(dir: &std::path::Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let ours = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("segment-") && n.ends_with(".mp4"));
            if ours {
                let _ = fs::remove_file(path);
            }
        }
    }
    let _ = fs::remove_dir(dir);
}
