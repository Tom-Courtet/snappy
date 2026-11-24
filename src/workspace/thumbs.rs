//! Vignettes : rendues avec les annotations, réduites, gardées en cache sur le disque
//! (`.thumbnails/` du dossier, clé = dates de l'image et de ses annotations) et
//! fabriquées sur des fils à part. La grille demande celles qu'elle affiche ; les
//! plus récemment demandées passent d'abord (défilement rapide).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use anyhow::Result;
use egui::{ColorImage, TextureHandle, TextureOptions};
use image::RgbaImage;

use crate::files::{self, Kind, Media};
use crate::history;

/// Grand côté des vignettes en cache, en pixels (300 points sur un écran Retina,
/// un peu moins net au-delà de 160 points).
pub const THUMB_PX: u32 = 320;
/// Textures gardées en mémoire au plus (les autres se relisent depuis le disque).
const MAX_TEXTURES: usize = 400;

struct Job {
    path: PathBuf,
    kind: Kind,
    key: String,
}

struct Done {
    path: PathBuf,
    key: String,
    result: Result<(RgbaImage, Option<(u32, u32)>)>,
}

enum Entry {
    Ready {
        key: String,
        texture: TextureHandle,
        /// Dernière image où la vignette a servi (pour libérer les autres).
        used: u64,
    },
    /// Pas de vignette possible (vidéo sous Windows, fichier illisible…).
    Failed { key: String },
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    stop: bool,
}

pub struct Thumbs {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    done: Receiver<Done>,
    entries: HashMap<PathBuf, Entry>,
    /// Demandées, pas encore rendues (chemin → clé).
    pending: HashMap<PathBuf, String>,
    /// Définition des images (lue avec la vignette).
    dims: HashMap<PathBuf, (u32, u32)>,
    frame: u64,
}

impl Thumbs {
    pub fn new(repaint: impl Fn() + Send + Sync + 'static) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (tx, done) = mpsc::channel();
        let repaint = Arc::new(repaint);
        let workers = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 3);
        for n in 0..workers {
            let queue = Arc::clone(&queue);
            let tx: Sender<Done> = tx.clone();
            let repaint = Arc::clone(&repaint);
            let spawned = thread::Builder::new()
                .name(format!("snappy-thumbs-{n}"))
                .spawn(move || worker(&queue, &tx, &*repaint));
            if let Err(e) = spawned {
                log::warn!("fil des vignettes : {e}");
            }
        }
        Self {
            queue,
            done,
            entries: HashMap::new(),
            pending: HashMap::new(),
            dims: HashMap::new(),
            frame: 0,
        }
    }

    /// Début d'une image : range les vignettes finies, libère les textures oubliées.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        while let Ok(done) = self.done.try_recv() {
            if self.pending.get(&done.path) == Some(&done.key) {
                self.pending.remove(&done.path);
            }
            let entry = match done.result {
                Ok((img, dims)) => {
                    if let Some(d) = dims {
                        self.dims.insert(done.path.clone(), d);
                    }
                    let size = [img.width() as usize, img.height() as usize];
                    let color = ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                    let texture = ctx.load_texture(
                        format!("thumb:{}", done.path.display()),
                        color,
                        TextureOptions::LINEAR,
                    );
                    Entry::Ready {
                        key: done.key,
                        texture,
                        used: self.frame,
                    }
                }
                Err(e) => {
                    log::debug!("vignette de {} : {e:#}", done.path.display());
                    Entry::Failed { key: done.key }
                }
            };
            self.entries.insert(done.path, entry);
        }
        if self.entries.len() > MAX_TEXTURES {
            let mut used: Vec<(u64, PathBuf)> = self
                .entries
                .iter()
                .filter_map(|(p, e)| match e {
                    Entry::Ready { used, .. } => Some((*used, p.clone())),
                    Entry::Failed { .. } => None,
                })
                .collect();
            used.sort();
            let excess = self.entries.len() - MAX_TEXTURES;
            for (_, path) in used.into_iter().take(excess) {
                self.entries.remove(&path);
            }
        }
    }

    /// Vignette d'un média : `Some` si elle est prête (même ancienne, le temps que
    /// la nouvelle arrive) ; demandée sinon.
    pub fn get(&mut self, media: &Media) -> Option<TextureHandle> {
        let key = files::thumb_key(media.modified, media.doc_modified, THUMB_PX);
        let (texture, current) = match self.entries.get_mut(&media.path) {
            Some(Entry::Ready {
                key: k,
                texture,
                used,
            }) => {
                *used = self.frame;
                (Some(texture.clone()), *k == key)
            }
            Some(Entry::Failed { key: k }) => (None, *k == key),
            None => (None, false),
        };
        if !current && self.pending.get(&media.path) != Some(&key) {
            self.request(media, key);
        }
        texture
    }

    /// La vignette a-t-elle échoué (placeholder à dessiner) ?
    pub fn failed(&self, path: &Path) -> bool {
        matches!(self.entries.get(path), Some(Entry::Failed { .. }))
    }

    pub fn dims(&self, path: &Path) -> Option<(u32, u32)> {
        self.dims.get(path).copied()
    }

    fn request(&mut self, media: &Media, key: String) {
        self.pending.insert(media.path.clone(), key.clone());
        let (lock, cvar) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        queue.jobs.retain(|j| j.path != media.path);
        queue.jobs.push(Job {
            path: media.path.clone(),
            kind: media.kind,
            key,
        });
        cvar.notify_one();
    }

    /// Oublie les demandes en attente (changement de dossier).
    pub fn clear_queue(&mut self) {
        let (lock, _) = &*self.queue;
        lock.lock().unwrap_or_else(|e| e.into_inner()).jobs.clear();
        self.pending.clear();
    }

    /// Oublie la vignette d'un fichier renommé, déplacé ou supprimé.
    pub fn forget(&mut self, path: &Path) {
        self.entries.remove(path);
        self.pending.remove(path);
        self.dims.remove(path);
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.queue;
        lock.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        cvar.notify_all();
    }
}

fn worker(queue: &(Mutex<Queue>, Condvar), tx: &Sender<Done>, repaint: &(dyn Fn() + Send + Sync)) {
    let (lock, cvar) = queue;
    loop {
        let job = {
            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if q.stop {
                    return;
                }
                // La plus récente d'abord : ce qui est à l'écran maintenant.
                if let Some(job) = q.jobs.pop() {
                    break job;
                }
                q = cvar.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let result = make(&job.path, job.kind, &job.key);
        if tx
            .send(Done {
                path: job.path,
                key: job.key,
                result,
            })
            .is_err()
        {
            return;
        }
        repaint();
    }
}

/// Vignette d'un média, depuis le cache ou fabriquée (et mise en cache).
fn make(path: &Path, kind: Kind, key: &str) -> Result<(RgbaImage, Option<(u32, u32)>)> {
    let dims = (kind == Kind::Image)
        .then(|| image::image_dimensions(path).ok())
        .flatten();
    let cached = files::thumb_path(path, key);
    if let Ok(img) = history::read_png(&cached) {
        return Ok((img, dims));
    }
    let img = match kind {
        Kind::Image => {
            let flat = files::flattened(path)?;
            image::imageops::thumbnail(&flat, fit(flat.dimensions()).0, fit(flat.dimensions()).1)
        }
        Kind::Video | Kind::Document => system_thumbnail(path)?,
    };
    if let Err(e) = history::write_png(&cached, &img) {
        log::debug!("vignette non mise en cache : {e:#}");
    } else {
        files::prune_thumbs(path, &cached);
    }
    Ok((img, dims))
}

/// Taille de la vignette d'une image `w` × `h` : grand côté [`THUMB_PX`] au plus,
/// sans agrandir.
pub fn fit((w, h): (u32, u32)) -> (u32, u32) {
    let scale = (THUMB_PX as f32 / w.max(h).max(1) as f32).min(1.0);
    (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    )
}

/// Vignette d'une vidéo ou d'un document par Quick Look (`qlmanage`, livré avec
/// macOS) : aucune dépendance pour décoder les vidéos.
#[cfg(target_os = "macos")]
fn system_thumbnail(path: &Path) -> Result<RgbaImage> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let out = std::env::temp_dir().join(format!(
        "snappy-ql-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&out)?;
    let status = std::process::Command::new("qlmanage")
        .arg("-t")
        .args(["-s", &THUMB_PX.to_string()])
        .arg("-o")
        .arg(&out)
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| anyhow::anyhow!("qlmanage : {e}"))?;
    let png = out.join(format!("{}.png", files::file_name(path)));
    let result = if status.success() {
        history::read_png(&png)
    } else {
        Err(anyhow::anyhow!("qlmanage a échoué"))
    };
    let _ = std::fs::remove_dir_all(&out);
    let img = result?;
    let (w, h) = fit(img.dimensions());
    Ok(image::imageops::thumbnail(&img, w, h))
}

#[cfg(not(target_os = "macos"))]
fn system_thumbnail(_path: &Path) -> Result<RgbaImage> {
    anyhow::bail!("pas de vignette de vidéo ou de document sur ce système")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_fit_without_upscaling() {
        assert_eq!(fit((2940, 1912)), (320, 208));
        assert_eq!(fit((100, 50)), (100, 50));
        assert_eq!(fit((10, 4000)), (1, 320));
    }

    #[test]
    fn thumbnails_are_cached_with_annotations() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        history::write_png(
            &png,
            &RgbaImage::from_pixel(640, 320, image::Rgba([0, 0, 0, 255])),
        )
        .unwrap();
        let doc = crate::model::Doc {
            annotations: vec![crate::model::Annotation::new(
                crate::model::Shape::Rect {
                    a: crate::model::Pt::new(0.0, 0.0),
                    b: crate::model::Pt::new(640.0, 320.0),
                    radius: 0.0,
                },
                crate::model::Rgba(255, 0, 0, 255),
                40.0,
            )],
        };
        history::save_doc(&png, &doc).unwrap();
        let (img, dims) = make(&png, Kind::Image, "k1").unwrap();
        assert_eq!(img.dimensions(), (320, 160));
        assert_eq!(dims, Some((640, 320)));
        let edge = img.get_pixel(3, 80).0;
        assert!(
            edge[0] > 200 && edge[1] < 60,
            "annotations rendues : {edge:?}"
        );
        assert_eq!(img.get_pixel(160, 80).0, [0, 0, 0, 255]);
        assert!(files::thumb_path(&png, "k1").exists());
        // Nouvelle clé : l'ancienne vignette est retirée.
        make(&png, Kind::Image, "k2").unwrap();
        assert_eq!(files::thumbs_of(&png), vec![files::thumb_path(&png, "k2")]);
    }
}
