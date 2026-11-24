//! AVFoundation (macOS) pour le lecteur : informations d'une vidéo, images à un
//! instant donné, export découpé / accéléré / muet, assemblage de segments.
//!
//! Les accesseurs synchrones (`duration`, `tracksWithMediaType`,
//! `copyCGImageAtTime`) sont marqués obsolètes au profit de variantes à blocs ; ils
//! restent présents et sont bien plus simples ici, où tout tourne déjà hors du fil
//! de l'interface. Comme pour la capture d'écran, on les garde tant qu'ils marchent.

#![allow(deprecated)] // voir l'en-tête

use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use block2::RcBlock;
use image::RgbaImage;
use objc2::rc::Retained;
use objc2_av_foundation::{
    AVAsset, AVAssetExportPresetHighestQuality, AVAssetExportPresetPassthrough,
    AVAssetExportSession, AVAssetExportSessionStatus, AVAssetImageGenerator, AVAssetTrack,
    AVAudioTimePitchAlgorithmSpectral, AVFileTypeMPEG4, AVMediaType, AVMediaTypeAudio,
    AVMediaTypeVideo, AVMutableComposition, AVMutableCompositionTrack, AVURLAsset,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBlendMode, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGInterpolationQuality, kCGColorSpaceSRGB,
};
use objc2_core_media::{CMTime, CMTimeRange};
use objc2_foundation::{NSString, NSURL};

/// Précision des temps passés à AVFoundation (unités par seconde).
const TIMESCALE: i32 = 60_000;

fn time(seconds: f64) -> CMTime {
    // SAFETY : fonction pure de CoreMedia.
    unsafe { CMTime::with_seconds(seconds.max(0.0), TIMESCALE) }
}

fn seconds(t: CMTime) -> f64 {
    // SAFETY : fonction pure de CoreMedia.
    let s = unsafe { t.seconds() };
    if s.is_finite() { s } else { 0.0 }
}

fn range(start: f64, duration: f64) -> CMTimeRange {
    CMTimeRange {
        start: time(start),
        duration: time(duration),
    }
}

fn media(kind: Option<&'static AVMediaType>) -> Result<&'static AVMediaType> {
    kind.context("type de média AVFoundation indisponible")
}

fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn open(path: &Path) -> Result<Retained<AVURLAsset>> {
    ensure!(path.exists(), "{} introuvable", path.display());
    // SAFETY : URL de fichier valide, pas d'options.
    Ok(unsafe { AVURLAsset::URLAssetWithURL_options(&file_url(path), None) })
}

fn tracks(
    asset: &AVAsset,
    kind: Option<&'static AVMediaType>,
) -> Result<Vec<Retained<AVAssetTrack>>> {
    // SAFETY : accesseur en lecture.
    Ok(unsafe { asset.tracksWithMediaType(media(kind)?) }.to_vec())
}

/// Ce que le lecteur doit savoir d'une vidéo.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    /// Secondes.
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    /// Images par seconde (0 si inconnu).
    pub fps: f32,
    pub has_audio: bool,
}

pub fn info(path: &Path) -> Result<Info> {
    let asset = open(path)?;
    // SAFETY (bloc) : accesseurs en lecture.
    let video = tracks(&asset, unsafe { AVMediaTypeVideo })?;
    let track = video.first().context("aucune piste vidéo")?;
    let (size, fps, duration) = unsafe {
        (
            track.naturalSize(),
            track.nominalFrameRate(),
            seconds(asset.duration()),
        )
    };
    let has_audio = !tracks(&asset, unsafe { AVMediaTypeAudio })?.is_empty();
    Ok(Info {
        duration,
        width: size.width.round().max(0.0) as u32,
        height: size.height.round().max(0.0) as u32,
        fps,
        has_audio,
    })
}

/// Images d'une vidéo à des instants choisis.
pub struct Frames {
    generator: Retained<AVAssetImageGenerator>,
}

impl Frames {
    /// `max_width` : largeur maximale des images (0 : taille d'origine). `exact` :
    /// l'image exacte de l'instant demandé (plus lent) plutôt que l'image clé voisine.
    pub fn open(path: &Path, max_width: u32, exact: bool) -> Result<Self> {
        let asset = open(path)?;
        // SAFETY : réglages d'un générateur neuf.
        let generator = unsafe {
            let g = AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset);
            g.setAppliesPreferredTrackTransform(true);
            if max_width > 0 {
                g.setMaximumSize(CGSize::new(f64::from(max_width), 0.0));
            }
            let tolerance = if exact { time(0.0) } else { time(0.1) };
            g.setRequestedTimeToleranceBefore(tolerance);
            g.setRequestedTimeToleranceAfter(tolerance);
            g
        };
        Ok(Self { generator })
    }

    /// Image à `at` secondes, en sRGB opaque.
    pub fn at(&self, at: f64) -> Result<RgbaImage> {
        // SAFETY : générateur valide ; le temps réel n'est pas demandé.
        let image = unsafe {
            self.generator
                .copyCGImageAtTime_actualTime_error(time(at), std::ptr::null_mut())
        }
        .map_err(|e| anyhow!("{}", e.localizedDescription()))
        .context("lecture d'une image de la vidéo")?;
        to_rgba(&image)
    }
}

/// Image CoreGraphics → RGBA 8 bits sRGB opaque.
fn to_rgba(source: &CGImage) -> Result<RgbaImage> {
    let width = CGImage::width(Some(source));
    let height = CGImage::height(Some(source));
    ensure!(width > 0 && height > 0, "image vide");
    let stride = width.checked_mul(4).context("image trop large")?;
    let mut pixels = vec![0u8; stride.checked_mul(height).context("image trop grande")?];
    // SAFETY : constante fournie par CoreGraphics.
    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .context("espace sRGB indisponible")?;
    let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    {
        // SAFETY : `pixels` fait `stride * height` octets et survit au contexte,
        // détruit à la fin de ce bloc.
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
        CGContext::set_interpolation_quality(Some(&ctx), CGInterpolationQuality::None);
        let rect = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(width as f64, height as f64),
        );
        CGContext::draw_image(Some(&ctx), rect, Some(source));
    }
    for px in pixels.as_chunks_mut::<4>().0 {
        px[3] = u8::MAX;
    }
    RgbaImage::from_raw(u32::try_from(width)?, u32::try_from(height)?, pixels)
        .context("tampon de pixels incohérent")
}

/// Modifications d'un export.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edit {
    /// Extrait gardé, en secondes.
    pub start: f64,
    pub end: f64,
    /// 2 : deux fois plus rapide.
    pub speed: f64,
    /// Sans les pistes audio.
    pub mute: bool,
}

/// Composition de morceaux `(fichier, début, durée)` mis bout à bout.
fn compose(
    parts: &[(Retained<AVURLAsset>, f64, f64)],
    mute: bool,
) -> Result<Retained<AVMutableComposition>> {
    // SAFETY (bloc) : composition neuve, pistes et temps valides.
    unsafe {
        let video_kind = media(AVMediaTypeVideo)?;
        let audio_kind = media(AVMediaTypeAudio)?;
        let comp = AVMutableComposition::composition();
        let video = comp
            .addMutableTrackWithMediaType_preferredTrackID(video_kind, 0)
            .context("piste vidéo de la composition")?;
        let mut audio: Vec<Retained<AVMutableCompositionTrack>> = Vec::new();
        let mut cursor = 0.0;
        for (asset, start, duration) in parts {
            let source = tracks(asset, AVMediaTypeVideo)?;
            let source = source.first().context("aucune piste vidéo")?;
            video
                .insertTimeRange_ofTrack_atTime_error(
                    range(*start, *duration),
                    source,
                    time(cursor),
                )
                .map_err(|e| anyhow!("{}", e.localizedDescription()))
                .context("découpe de la vidéo")?;
            video.setPreferredTransform(source.preferredTransform());
            if !mute {
                for (i, track) in tracks(asset, AVMediaTypeAudio)?.iter().enumerate() {
                    if audio.len() <= i {
                        audio.push(
                            comp.addMutableTrackWithMediaType_preferredTrackID(audio_kind, 0)
                                .context("piste audio de la composition")?,
                        );
                    }
                    // Une piste audio commence souvent un peu après l'image : on ne
                    // prend que ce qu'elle couvre de l'extrait.
                    let own = track.timeRange();
                    let a0 = start.max(seconds(own.start));
                    let a1 = (start + duration).min(seconds(own.start) + seconds(own.duration));
                    if a1 > a0 {
                        audio[i]
                            .insertTimeRange_ofTrack_atTime_error(
                                range(a0, a1 - a0),
                                track,
                                time(cursor + (a0 - start)),
                            )
                            .map_err(|e| anyhow!("{}", e.localizedDescription()))
                            .context("découpe du son")?;
                    }
                }
            }
            cursor += duration;
        }
        Ok(comp)
    }
}

/// Exporte `asset` en MP4 dans `dst` (qui ne doit pas exister). `progress` reçoit
/// l'avancement entre 0 et 1.
fn export_asset(
    asset: &AVAsset,
    passthrough: bool,
    dst: &Path,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    ensure!(!dst.exists(), "{} existe déjà", dst.display());
    // SAFETY (bloc) : session neuve ; bloc conforme, gardé vivant par l'appelé.
    let session = unsafe {
        let preset = if passthrough {
            AVAssetExportPresetPassthrough
        } else {
            AVAssetExportPresetHighestQuality
        };
        let session = AVAssetExportSession::exportSessionWithAsset_presetName(asset, preset)
            .context("export impossible pour cette vidéo")?;
        session.setOutputURL(Some(&file_url(dst)));
        session.setOutputFileType(AVFileTypeMPEG4);
        if !passthrough && let Some(pitch) = AVAudioTimePitchAlgorithmSpectral {
            // Vitesse changée sans voix de canard.
            session.setAudioTimePitchAlgorithm(pitch);
        }
        session
    };
    let (tx, rx) = mpsc::channel::<()>();
    let block = RcBlock::new(move || {
        let _ = tx.send(());
    });
    // SAFETY : voir plus haut.
    unsafe { session.exportAsynchronouslyWithCompletionHandler(&block) };
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(()) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => progress(unsafe { session.progress() }),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    // SAFETY : accesseurs en lecture.
    let (status, error) = unsafe { (session.status(), session.error()) };
    if status == AVAssetExportSessionStatus::Completed {
        progress(1.0);
        return Ok(());
    }
    let _ = fs::remove_file(dst);
    match error {
        Some(e) => bail!("export : {}", e.localizedDescription()),
        None => bail!("export interrompu"),
    }
}

/// Nouveau fichier `dst` : l'extrait de `src` modifié selon `edit`. L'original
/// n'est jamais touché.
pub fn export(src: &Path, dst: &Path, edit: Edit, progress: &mut dyn FnMut(f32)) -> Result<()> {
    ensure!(edit.end > edit.start, "extrait vide");
    ensure!(edit.speed > 0.0, "vitesse invalide");
    let asset = open(src)?;
    let duration = edit.end - edit.start;
    let comp = compose(&[(asset, edit.start, duration)], edit.mute)?;
    let normal_speed = (edit.speed - 1.0).abs() < 1e-6;
    if !normal_speed {
        // SAFETY : plage incluse dans la composition.
        unsafe {
            comp.scaleTimeRange_toDuration(range(0.0, duration), time(duration / edit.speed))
        };
    }
    // Vitesse normale : copie sans réencodage (rapide, sans perte).
    export_asset(&comp, normal_speed, dst, progress)
}

/// Met bout à bout des segments d'un même enregistrement (pauses), sans réencoder.
pub fn concat(segments: &[std::path::PathBuf], dst: &Path) -> Result<()> {
    let mut parts = Vec::new();
    for path in segments {
        let asset = open(path)?;
        // SAFETY : accesseur en lecture.
        let duration = seconds(unsafe { asset.duration() });
        if duration > 0.0 {
            parts.push((asset, 0.0, duration));
        }
    }
    ensure!(!parts.is_empty(), "segments vides");
    let comp = compose(&parts, false)?;
    export_asset(&comp, true, dst, &mut |_| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vidéo de test (320 × 240, 30 i/s, avec son) fabriquée par ffmpeg.
    fn sample(dir: &Path, name: &str, seconds: u32) -> Option<std::path::PathBuf> {
        let path = dir.join(name);
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("testsrc=size=320x240:rate=30:duration={seconds}"))
            .args(["-f", "lavfi", "-i"])
            .arg(format!("sine=frequency=440:duration={seconds}"))
            .args([
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        ok.then_some(path)
    }

    /// Lecture, images, découpe, vitesse, muet, assemblage et GIF sur une vraie
    /// vidéo. Demande ffmpeg (pour fabriquer la vidéo) : `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn avfoundation_for_real() {
        let dir = tempfile::tempdir().unwrap();
        let Some(src) = sample(dir.path(), "src.mp4", 4) else {
            eprintln!("ffmpeg absent : test sauté");
            return;
        };
        let i = info(&src).unwrap();
        assert!((i.duration - 4.0).abs() < 0.1, "{i:?}");
        assert_eq!((i.width, i.height), (320, 240));
        assert!(i.has_audio);
        assert!((i.fps - 30.0).abs() < 0.5);

        let frames = Frames::open(&src, 160, true).unwrap();
        let img = frames.at(1.0).unwrap();
        assert_eq!((img.width(), img.height()), (160, 120));
        assert!(img.pixels().all(|p| p.0[3] == 255));

        let trimmed = dir.path().join("trim.mp4");
        let edit = Edit {
            start: 1.0,
            end: 3.0,
            speed: 1.0,
            mute: true,
        };
        export(&src, &trimmed, edit, &mut |_| {}).unwrap();
        let t = info(&trimmed).unwrap();
        assert!((t.duration - 2.0).abs() < 0.15, "{t:?}");
        assert!(!t.has_audio);

        let fast = dir.path().join("fast.mp4");
        let edit = Edit {
            start: 0.0,
            end: 4.0,
            speed: 2.0,
            mute: false,
        };
        let mut last = 0.0;
        export(&src, &fast, edit, &mut |p| last = p).unwrap();
        let f = info(&fast).unwrap();
        assert!((f.duration - 2.0).abs() < 0.15, "{f:?}");
        assert!(f.has_audio);
        assert_eq!(last, 1.0);

        let other = sample(dir.path(), "b.mp4", 2).unwrap();
        let joined = dir.path().join("joined.mp4");
        concat(&[src.clone(), other], &joined).unwrap();
        let j = info(&joined).unwrap();
        assert!((j.duration - 6.0).abs() < 0.2, "{j:?}");
        assert!(j.has_audio);

        let gif = dir.path().join("x.gif");
        let times = super::super::gif_export::frame_times(0.0, 1.0, 10, 1.0);
        super::super::gif_export::write_gif(&gif, 10, times.iter().map(|&t| frames.at(t)), |_| {})
            .unwrap();
        assert!(fs::metadata(&gif).unwrap().len() > 1000);
        assert!(src.exists(), "l'original reste");
    }
}
