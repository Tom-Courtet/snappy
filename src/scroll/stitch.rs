//! Assemblage incrémental des images d'une zone qu'on fait défiler.
//!
//! Chaque nouvelle image est comparée à la **référence** (la dernière image
//! acceptée) pour trouver de combien de lignes le contenu s'est déplacé. On la
//! pose alors à sa place dans le **canevas**, une bande qui grandit vers le bas
//! (ou vers le haut si l'utilisateur remonte au-dessus de son point de départ).
//!
//! Recalage :
//! 1. lignes identiques **à la même place** en haut et en bas des deux images :
//!    en-têtes et pieds de page fixes (« collants »), ou rien n'a bougé ;
//! 2. entre les deux, chaque ligne a une **empreinte** (hachage de ses pixels, sans
//!    la marge droite où passe la barre de défilement). Les lignes uniques de la
//!    référence votent pour un décalage ; le meilleur est vérifié sur tout le
//!    recouvrement ;
//! 3. si les empreintes ne suffisent pas (lissage différent, bruit, contenu
//!    animé), repli sur un **profil gris réduit** de chaque ligne (32 cases) et la
//!    somme des écarts absolus, avec un minimum franc exigé.
//!
//! En-têtes et pieds fixes : les lignes du haut et du bas restées en place sont
//! posées « faibles » : elles ne remplissent le canevas que là où il n'y a encore
//! rien (ou une autre ligne faible), et une ligne de contenu les remplace ensuite.
//! L'en-tête reste donc une seule fois en haut, le pied une seule fois en bas.
//! Une ligne de contenu déjà posée n'est jamais remplacée : repasser sur une partie
//! déjà capturée ne la duplique pas.
//!
//! Sens : vertical d'abord ; si le premier mouvement n'est reconnu que dans
//! l'autre sens, toute la séance passe en horizontal (les images sont transposées,
//! le même algorithme s'applique aux colonnes). Le déplacement en deux dimensions
//! (cartes) n'est pas géré.

use std::collections::{HashMap, VecDeque};

use image::RgbaImage;

/// Cases du profil gris d'une ligne.
const BINS: usize = 32;
/// Recouvrement minimal, en lignes, entre deux images pour les raccorder.
const MIN_OVERLAP: usize = 8;
/// Votes minimaux pour retenir un décalage trouvé par les empreintes.
const MIN_VOTES: u32 = 4;
/// Part du recouvrement dont les empreintes doivent concorder.
const MIN_AGREEMENT: f32 = 0.6;
/// Écart moyen maximal (niveaux de gris par case) du repli par profils.
const FUZZY_MAX_COST: f32 = 4.0;
/// Le meilleur décalage du repli doit coûter au moins ce facteur de moins que le
/// décalage médian : sinon le contenu est trop uniforme pour conclure.
const FUZZY_CONTRAST: f32 = 2.5;
/// Lignes échantillonnées au plus par décalage essayé, dans le repli.
const FUZZY_SAMPLES: usize = 96;

/// Sens du défilement, fixé au premier mouvement reconnu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

/// Ce qu'a donné une image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Première image : elle sert de référence.
    First,
    /// Rien n'a bougé (ou seulement un détail : curseur de texte, animation) :
    /// image ignorée.
    Still,
    /// Contenu déplacé de `delta` lignes (positif : vers le bas ou la droite du
    /// document) ; `added` lignes nouvelles dans le canevas.
    Moved { delta: i32, added: u32 },
    /// Raccord impossible (défilement trop rapide, contenu changé) : image
    /// ignorée, la référence reste la même.
    Lost,
    /// Longueur maximale atteinte : la suite est ignorée.
    Full,
}

/// Empreintes et profils de chaque ligne d'une image.
struct Lines {
    sigs: Vec<u64>,
    profiles: Vec<[u8; BINS]>,
}

impl Lines {
    fn of(img: &RgbaImage) -> Self {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let cols = compare_width(w);
        let stride = w * 4;
        let raw = img.as_raw();
        let mut sigs = Vec::with_capacity(h);
        let mut profiles = Vec::with_capacity(h);
        for y in 0..h {
            let part = &raw[y * stride..y * stride + cols * 4];
            sigs.push(hash(part));
            profiles.push(profile(part));
        }
        Self { sigs, profiles }
    }

    fn len(&self) -> usize {
        self.sigs.len()
    }
}

/// Colonnes comparées : toute la largeur sauf la marge droite, où la barre de
/// défilement (ou son indicateur, qui apparaît pendant le défilement sous macOS)
/// change d'une image à l'autre.
fn compare_width(width: usize) -> usize {
    let margin = (width / 12).min(48);
    (width - margin).max(1).min(width)
}

/// Empreinte d'une ligne (rapide, non cryptographique).
fn hash(bytes: &[u8]) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    let (chunks, rest) = bytes.as_chunks::<8>();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for chunk in chunks {
        h = (h.rotate_left(5) ^ u64::from_le_bytes(*chunk)).wrapping_mul(K);
    }
    for &b in rest {
        h = (h.rotate_left(5) ^ u64::from(b)).wrapping_mul(K);
    }
    h ^ (h >> 29)
}

/// Profil gris d'une ligne RGBA : moyenne (échantillonnée) de chaque case.
fn profile(row: &[u8]) -> [u8; BINS] {
    let px = row.len() / 4;
    let mut out = [0u8; BINS];
    if px == 0 {
        return out;
    }
    for (b, slot) in out.iter_mut().enumerate() {
        let start = (b * px / BINS).min(px - 1);
        let end = ((b + 1) * px / BINS).max(start + 1).min(px);
        let step = ((end - start) / 6).max(1);
        let (mut sum, mut n) = (0u32, 0u32);
        let mut x = start;
        while x < end {
            let p = &row[x * 4..x * 4 + 3];
            sum += (u32::from(p[0]) * 77 + u32::from(p[1]) * 150 + u32::from(p[2]) * 29) >> 8;
            n += 1;
            x += step;
        }
        *slot = (sum / n) as u8;
    }
    out
}

/// Résultat du recalage de deux images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Match {
    Still,
    /// Ligne `r` de la nouvelle image = ligne `r + delta` de la référence ;
    /// `lead` et `trail` lignes identiques en place en haut et en bas.
    Moved {
        delta: i32,
        lead: usize,
        trail: usize,
    },
    Lost,
}

/// Décalage entre la référence et la nouvelle image. `hint` : dernier décalage
/// trouvé, pour départager des ex aequo (contenu répétitif).
fn find_shift(prev: &Lines, cur: &Lines, hint: Option<i32>) -> Match {
    let h = cur.len();
    if prev.len() != h || h == 0 {
        return Match::Lost;
    }
    let same = |i: usize| prev.sigs[i] == cur.sigs[i];
    let lead = (0..h).take_while(|&i| same(i)).count();
    if lead == h {
        return Match::Still;
    }
    let trail = (lead..h).rev().take_while(|&i| same(i)).count();
    let (lo, hi) = (lead, h - trail);
    if hi - lo <= MIN_OVERLAP {
        // Seul un détail a changé.
        return Match::Still;
    }
    let found =
        exact_shift(prev, cur, lo, hi, hint).or_else(|| fuzzy_shift(prev, cur, lo, hi, hint));
    match found {
        Some(0) => Match::Still,
        Some(delta) => Match::Moved {
            delta: delta as i32,
            lead,
            trail,
        },
        None => Match::Lost,
    }
}

/// Lignes `r` de `[lo, hi)` telles que `r + d` y soit aussi.
fn overlap(lo: usize, hi: usize, d: isize) -> (usize, usize) {
    let a = (lo as isize).max(lo as isize - d);
    let b = (hi as isize).min(hi as isize - d);
    (a.max(0) as usize, b.max(a).max(0) as usize)
}

/// Le plus proche de `hint` (ou de zéro) parmi deux décalages ex aequo.
fn closer(a: isize, b: isize, hint: Option<i32>) -> bool {
    let target = hint.map_or(0, |h| h as isize);
    (a - target).abs() < (b - target).abs()
}

/// Recalage par les empreintes des lignes uniques.
fn exact_shift(
    prev: &Lines,
    cur: &Lines,
    lo: usize,
    hi: usize,
    hint: Option<i32>,
) -> Option<isize> {
    let h = cur.len() as isize;
    // Empreinte → ligne de la référence, `None` si elle y figure plusieurs fois
    // (lignes vides, motifs répétés) : elle ne vote pas.
    let mut index: HashMap<u64, Option<usize>> = HashMap::with_capacity(hi - lo);
    for p in lo..hi {
        index
            .entry(prev.sigs[p])
            .and_modify(|e| *e = None)
            .or_insert(Some(p));
    }
    let mut votes = vec![0u32; 2 * h as usize + 1];
    for r in lo..hi {
        if let Some(Some(p)) = index.get(&cur.sigs[r]) {
            votes[(*p as isize - r as isize + h) as usize] += 1;
        }
    }
    let mut best: Option<(isize, u32)> = None;
    for (i, &v) in votes.iter().enumerate() {
        let d = i as isize - h;
        if v > 0 && best.is_none_or(|(bd, bv)| v > bv || (v == bv && closer(d, bd, hint))) {
            best = Some((d, v));
        }
    }
    let (d, v) = best?;
    if v < MIN_VOTES {
        return None;
    }
    let (a, b) = overlap(lo, hi, d);
    if b - a < MIN_OVERLAP {
        return None;
    }
    let agree = (a..b)
        .filter(|&r| cur.sigs[r] == prev.sigs[(r as isize + d) as usize])
        .count();
    (agree as f32 >= MIN_AGREEMENT * (b - a) as f32).then_some(d)
}

/// Recalage approché par les profils gris : somme des écarts absolus.
fn fuzzy_shift(
    prev: &Lines,
    cur: &Lines,
    lo: usize,
    hi: usize,
    hint: Option<i32>,
) -> Option<isize> {
    let middle = hi - lo;
    // Petit recouvrement : coût peu fiable (quelques lignes vides se ressemblent).
    let min_overlap = MIN_OVERLAP.max(middle / 4);
    if middle < min_overlap {
        return None;
    }
    let max_d = (middle - min_overlap) as isize;
    let mut costs: Vec<(isize, f32)> = Vec::with_capacity(2 * max_d as usize + 1);
    for d in -max_d..=max_d {
        let (a, b) = overlap(lo, hi, d);
        let step = ((b - a) / FUZZY_SAMPLES).max(1);
        let (mut sum, mut n) = (0u32, 0u32);
        let mut r = a;
        while r < b {
            let p = &prev.profiles[(r as isize + d) as usize];
            let c = &cur.profiles[r];
            sum += p
                .iter()
                .zip(c)
                .map(|(x, y)| u32::from(x.abs_diff(*y)))
                .sum::<u32>();
            n += 1;
            r += step;
        }
        costs.push((d, sum as f32 / (n.max(1) as usize * BINS) as f32));
    }
    let &(mut best_d, mut best_cost) = costs.first()?;
    for &(d, c) in &costs[1..] {
        if c < best_cost || (c == best_cost && closer(d, best_d, hint)) {
            (best_d, best_cost) = (d, c);
        }
    }
    let mut sorted: Vec<f32> = costs.iter().map(|&(_, c)| c).collect();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    (best_cost <= FUZZY_MAX_COST && best_cost * FUZZY_CONTRAST < median).then_some(best_d)
}

/// Transposée d'une image : la colonne `x` devient la ligne `x`.
pub fn transpose(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let src = img.as_raw();
    let mut out = vec![0u8; src.len()];
    let (w, h) = (w as usize, h as usize);
    for y in 0..h {
        for x in 0..w {
            let s = (y * w + x) * 4;
            let d = (x * h + y) * 4;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    RgbaImage::from_raw(h as u32, w as u32, out).expect("taille conservée")
}

fn orient(img: RgbaImage, axis: Axis) -> RgbaImage {
    match axis {
        Axis::Vertical => img,
        Axis::Horizontal => transpose(&img),
    }
}

/// Une ligne du canevas (orienté) : ses pixels, et si c'est du contenu (« forte »)
/// ou une ligne d'en-tête ou de pied fixe (« faible », remplaçable).
struct Line {
    data: Box<[u8]>,
    strong: bool,
}

/// Bande assemblée, en coordonnées du document : `lines[0]` est la ligne `top`.
#[derive(Default)]
struct Canvas {
    width: usize,
    top: i64,
    lines: VecDeque<Line>,
}

impl Canvas {
    fn len(&self) -> usize {
        self.lines.len()
    }

    /// Pose `img` (orientée) avec sa ligne 0 en `pos` ; les `lead` premières et
    /// `trail` dernières lignes sont faibles. Une ligne de contenu remplace une
    /// ligne faible ; une ligne faible n'en remplace une autre que du côté où l'on
    /// avance (`down` : le pied en bas, sinon l'en-tête en haut), pour que
    /// l'en-tête et le pied restent entiers. Rend le nombre de lignes ajoutées et
    /// si la longueur maximale a arrêté la pose.
    fn write(
        &mut self,
        img: &RgbaImage,
        pos: i64,
        (lead, trail): (usize, usize),
        down: bool,
        max: usize,
    ) -> (u32, bool) {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let stride = w * 4;
        let raw = img.as_raw();
        if self.lines.is_empty() {
            self.width = w;
            self.top = pos;
        }
        let strong = |r: usize| r >= lead && r + trail < h;
        let line = |r: usize| Line {
            data: raw[r * stride..(r + 1) * stride].into(),
            strong: strong(r),
        };
        let (mut added, mut capped) = (0u32, false);
        // Au-dessus du canevas : de bas en haut, pour rester contigu.
        let above = (self.top - pos).clamp(0, h as i64) as usize;
        for r in (0..above).rev() {
            if self.len() >= max {
                capped = true;
                break;
            }
            self.lines.push_front(line(r));
            self.top -= 1;
            added += 1;
        }
        for r in above..h {
            let y = pos + r as i64;
            let i = y - self.top;
            if i < 0 {
                continue; // au-dessus, non posée faute de place
            }
            let i = i as usize;
            if i < self.len() {
                let edge = if down { r + trail >= h } else { r < lead };
                if !self.lines[i].strong && (strong(r) || edge) {
                    self.lines[i] = line(r);
                }
            } else if i == self.len() {
                if self.len() >= max {
                    capped = true;
                    break;
                }
                self.lines.push_back(line(r));
                added += 1;
            }
        }
        (added, capped)
    }

    fn to_image(&self) -> RgbaImage {
        let mut raw = Vec::with_capacity(self.width * 4 * self.len());
        for line in &self.lines {
            raw.extend_from_slice(&line.data);
        }
        RgbaImage::from_raw(self.width as u32, self.len() as u32, raw).expect("lignes complètes")
    }
}

/// Dernière image acceptée.
struct Reference {
    /// Orientée si le sens est fixé, brute sinon.
    image: RgbaImage,
    lines: Lines,
    /// Position de sa ligne 0 dans le canevas.
    pos: i64,
}

/// Assemblage d'une séance de défilement.
pub struct Stitcher {
    max_len: usize,
    axis: Option<Axis>,
    reference: Option<Reference>,
    canvas: Canvas,
    hint: Option<i32>,
    full: bool,
}

impl Stitcher {
    /// `max_len` : longueur maximale de l'image, en pixels, dans le sens du
    /// défilement.
    pub fn new(max_len: u32) -> Self {
        Self {
            max_len: max_len.max(1) as usize,
            axis: None,
            reference: None,
            canvas: Canvas::default(),
            hint: None,
            full: false,
        }
    }

    /// Recommence à zéro.
    pub fn reset(&mut self) {
        *self = Self::new(self.max_len as u32);
    }

    pub fn axis(&self) -> Option<Axis> {
        self.axis
    }

    pub fn is_full(&self) -> bool {
        self.full
    }

    /// Ajoute une image de la zone.
    pub fn push(&mut self, frame: RgbaImage) -> Step {
        if self.full {
            return Step::Full;
        }
        let Some(reference) = &self.reference else {
            self.reference = Some(Reference {
                lines: Lines::of(&frame),
                image: frame,
                pos: 0,
            });
            return Step::First;
        };
        let raw_size = match self.axis {
            Some(Axis::Horizontal) => {
                let (w, h) = reference.image.dimensions();
                (h, w)
            }
            _ => reference.image.dimensions(),
        };
        if frame.dimensions() != raw_size {
            return Step::Lost;
        }
        match self.axis {
            Some(axis) => {
                let image = orient(frame, axis);
                let lines = Lines::of(&image);
                match find_shift(&reference.lines, &lines, self.hint) {
                    Match::Still => Step::Still,
                    Match::Lost => Step::Lost,
                    Match::Moved { delta, lead, trail } => {
                        let pos = reference.pos + i64::from(delta);
                        self.accept(Reference { image, lines, pos }, delta, lead, trail)
                    }
                }
            }
            None => self.first_move(frame),
        }
    }

    /// Avant le premier mouvement : vertical d'abord, sinon horizontal.
    fn first_move(&mut self, frame: RgbaImage) -> Step {
        let reference = self.reference.take().expect("référence présente");
        let lines = Lines::of(&frame);
        let vertical = find_shift(&reference.lines, &lines, None);
        if let Match::Moved { delta, lead, trail } = vertical {
            return self.lock(
                Axis::Vertical,
                &reference.image,
                frame,
                lines,
                delta,
                lead,
                trail,
            );
        }
        if reference.lines.sigs != lines.sigs {
            let (ref_t, cur_t) = (transpose(&reference.image), transpose(&frame));
            let cur_lines = Lines::of(&cur_t);
            if let Match::Moved { delta, lead, trail } =
                find_shift(&Lines::of(&ref_t), &cur_lines, None)
            {
                return self.lock(
                    Axis::Horizontal,
                    &ref_t,
                    cur_t,
                    cur_lines,
                    delta,
                    lead,
                    trail,
                );
            }
        }
        if vertical == Match::Still {
            self.reference = Some(reference);
            return Step::Still;
        }
        // Rien n'est encore posé : le contenu a changé (chargement d'une page…),
        // on repart de l'image actuelle.
        self.reference = Some(Reference {
            image: frame,
            lines,
            pos: 0,
        });
        Step::Lost
    }

    /// Fixe le sens, pose la première image puis la nouvelle.
    #[allow(clippy::too_many_arguments)]
    fn lock(
        &mut self,
        axis: Axis,
        first: &RgbaImage,
        image: RgbaImage,
        lines: Lines,
        delta: i32,
        lead: usize,
        trail: usize,
    ) -> Step {
        self.axis = Some(axis);
        let (first_added, capped) =
            self.canvas
                .write(first, 0, (lead, trail), delta > 0, self.max_len);
        self.full = capped;
        let reference = Reference {
            image,
            lines,
            pos: i64::from(delta),
        };
        match self.accept(reference, delta, lead, trail) {
            Step::Moved { delta, added } => Step::Moved {
                delta,
                added: added + first_added,
            },
            other => other,
        }
    }

    fn accept(&mut self, reference: Reference, delta: i32, lead: usize, trail: usize) -> Step {
        let (added, capped) = self.canvas.write(
            &reference.image,
            reference.pos,
            (lead, trail),
            delta > 0,
            self.max_len,
        );
        self.full |= capped;
        self.hint = Some(delta);
        self.reference = Some(reference);
        Step::Moved { delta, added }
    }

    /// Taille de l'image assemblée (de la première image tant que rien n'a bougé).
    pub fn size(&self) -> Option<(u32, u32)> {
        let (w, len) = (self.canvas.width as u32, self.canvas.len() as u32);
        match self.axis {
            Some(Axis::Vertical) if len > 0 => Some((w, len)),
            Some(Axis::Horizontal) if len > 0 => Some((len, w)),
            _ => self.reference.as_ref().map(|r| r.image.dimensions()),
        }
    }

    /// Partie visible en ce moment, dans le sens du défilement : début et longueur
    /// en pixels de l'image assemblée.
    pub fn view(&self) -> Option<(u32, u32)> {
        let r = self.reference.as_ref()?;
        self.axis?;
        let start = (r.pos - self.canvas.top).max(0) as u32;
        Some((start, r.image.height()))
    }

    /// Image assemblée ; la première image si rien n'a bougé.
    pub fn image(&self) -> Option<RgbaImage> {
        match self.axis {
            Some(axis) if self.canvas.len() > 0 => {
                let img = self.canvas.to_image();
                Some(match axis {
                    Axis::Vertical => img,
                    Axis::Horizontal => transpose(&img),
                })
            }
            _ => self.reference.as_ref().map(|r| r.image.clone()),
        }
    }

    /// Aperçu réduit (au plus `max_w` × `max_h`, sans agrandir), au plus proche
    /// voisin : peu coûteux, recalculé à chaque mouvement.
    pub fn preview(&self, max_w: u32, max_h: u32) -> Option<RgbaImage> {
        let (fw, fh) = self.size()?;
        let s = (f64::from(max_w) / f64::from(fw))
            .min(f64::from(max_h) / f64::from(fh))
            .min(1.0);
        let pw = ((f64::from(fw) * s).round() as u32).max(1);
        let ph = ((f64::from(fh) * s).round() as u32).max(1);
        let src_x = |px: u32| ((f64::from(px) + 0.5) / s).min(f64::from(fw - 1)) as usize;
        let src_y = |py: u32| ((f64::from(py) + 0.5) / s).min(f64::from(fh - 1)) as usize;
        let mut out = RgbaImage::new(pw, ph);
        match self.axis {
            Some(axis) if self.canvas.len() > 0 => {
                for py in 0..ph {
                    for px in 0..pw {
                        let (fx, fy) = (src_x(px), src_y(py));
                        // Coordonnées dans le canevas orienté : (colonne, ligne).
                        let (col, row) = match axis {
                            Axis::Vertical => (fx, fy),
                            Axis::Horizontal => (fy, fx),
                        };
                        let d = &self.canvas.lines[row].data[col * 4..col * 4 + 4];
                        out.put_pixel(px, py, image::Rgba([d[0], d[1], d[2], d[3]]));
                    }
                }
            }
            _ => {
                let img = &self.reference.as_ref()?.image;
                for py in 0..ph {
                    for px in 0..pw {
                        out.put_pixel(px, py, *img.get_pixel(src_x(px) as u32, src_y(py) as u32));
                    }
                }
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// Nombre pseudo-aléatoire stable.
    fn mix(mut x: u64) -> u64 {
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
        x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        x ^ (x >> 33)
    }

    /// « Page » synthétique : fond blanc, lignes de « texte » (blocs sombres de
    /// longueur variable), interlignes vides, et une bande vide tous les 200 px.
    fn page(w: u32, h: u32, seed: u64) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let band = y % 200 >= 180;
            let line = y / 14;
            let in_text = y % 14 < 10 && !band && x >= 6;
            let word = mix(seed ^ (u64::from(line) << 20) ^ u64::from(x / 9));
            if in_text
                && !word.is_multiple_of(5)
                && x < 6 + (mix(seed + u64::from(line)) % u64::from(w)) as u32
            {
                let v = (mix(word ^ u64::from(x) ^ (u64::from(y) << 32)) % 120) as u8;
                Rgba([v, v / 2, 255 - v, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        })
    }

    fn crop(img: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> RgbaImage {
        image::imageops::crop_imm(img, x, y, w, h).to_image()
    }

    /// Image de la zone quand la page est défilée de `offset` lignes, avec un
    /// en-tête et un pied fixes éventuels.
    fn view(
        doc: &RgbaImage,
        offset: u32,
        h: u32,
        header: Option<&RgbaImage>,
        footer: Option<&RgbaImage>,
    ) -> RgbaImage {
        let w = doc.width();
        let mut out = crop(doc, 0, offset, w, h);
        if let Some(hd) = header {
            image::imageops::replace(&mut out, hd, 0, 0);
        }
        if let Some(ft) = footer {
            image::imageops::replace(&mut out, ft, 0, i64::from(h - ft.height()));
        }
        out
    }

    fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(c))
    }

    fn run(stitcher: &mut Stitcher, frames: impl IntoIterator<Item = RgbaImage>) -> Vec<Step> {
        frames.into_iter().map(|f| stitcher.push(f)).collect()
    }

    #[test]
    fn scrolling_down_rebuilds_the_page() {
        let doc = page(160, 1500, 1);
        let mut s = Stitcher::new(30_000);
        let offsets: Vec<u32> = (0..=1200).step_by(37).chain([1200]).collect();
        let steps = run(
            &mut s,
            offsets.iter().map(|&o| view(&doc, o, 300, None, None)),
        );
        assert_eq!(steps[0], Step::First);
        assert!(
            matches!(steps[1], Step::Moved { delta: 37, .. }),
            "{:?}",
            steps[1]
        );
        assert_eq!(s.axis(), Some(Axis::Vertical));
        let out = s.image().unwrap();
        assert_eq!(out.dimensions(), (160, 1500));
        assert!(out == doc, "image différente de la page");
        assert_eq!(s.view(), Some((1200, 300)));
    }

    #[test]
    fn identical_frames_are_ignored() {
        let doc = page(120, 600, 2);
        let mut s = Stitcher::new(30_000);
        let f = view(&doc, 100, 200, None, None);
        assert_eq!(s.push(f.clone()), Step::First);
        assert_eq!(s.push(f.clone()), Step::Still);
        assert_eq!(s.push(f.clone()), Step::Still);
        assert_eq!(s.axis(), None);
        assert_eq!(s.image().unwrap(), f);
        assert_eq!(s.size(), Some((120, 200)));
    }

    #[test]
    fn sticky_header_and_footer_appear_once() {
        let doc = page(200, 1400, 3);
        let header = RgbaImage::from_fn(200, 40, |x, y| {
            Rgba([20, (x % 50) as u8 * 3, y as u8 * 4, 255])
        });
        let footer = RgbaImage::from_fn(200, 24, |x, _| Rgba([(x * 7 % 255) as u8, 200, 30, 255]));
        let h = 320;
        let mut s = Stitcher::new(30_000);
        let last = 1400 - h;
        let offsets: Vec<u32> = (0..last).step_by(23).chain([last]).collect();
        let steps = run(
            &mut s,
            offsets
                .iter()
                .map(|&o| view(&doc, o, h, Some(&header), Some(&footer))),
        );
        assert!(steps.iter().all(|s| !matches!(s, Step::Lost)), "{steps:?}");
        let out = s.image().unwrap();
        // En-tête, toute la page sous l'en-tête, pied.
        let content = 1400 - 40 - 24;
        assert_eq!(
            out.dimensions(),
            (200, 40 + content + 24),
            "{:?}",
            out.dimensions()
        );
        assert_eq!(crop(&out, 0, 0, 200, 40), header);
        assert_eq!(
            crop(&out, 0, 40, 200, content),
            crop(&doc, 0, 40, 200, content)
        );
        assert_eq!(crop(&out, 0, 40 + content, 200, 24), footer);
    }

    #[test]
    fn scrolling_back_up_does_not_duplicate() {
        let doc = page(150, 1600, 4);
        let mut s = Stitcher::new(30_000);
        let offsets = [
            0, 50, 120, 200, 260, 300, 240, 150, 60, 10, 90, 200, 350, 500, 640,
        ];
        let steps = run(
            &mut s,
            offsets.iter().map(|&o| view(&doc, o, 250, None, None)),
        );
        assert!(steps.iter().all(|s| !matches!(s, Step::Lost)), "{steps:?}");
        let out = s.image().unwrap();
        assert_eq!(out, crop(&doc, 0, 0, 150, 640 + 250));
    }

    #[test]
    fn scrolling_above_the_start_extends_upwards() {
        let doc = page(150, 1600, 5);
        let mut s = Stitcher::new(30_000);
        let offsets = [800, 760, 690, 600, 520, 470, 520, 600, 700, 900];
        let steps = run(
            &mut s,
            offsets.iter().map(|&o| view(&doc, o, 250, None, None)),
        );
        assert!(steps.iter().all(|s| !matches!(s, Step::Lost)), "{steps:?}");
        let out = s.image().unwrap();
        assert_eq!(out, crop(&doc, 0, 470, 150, 900 + 250 - 470));
        assert_eq!(s.view(), Some((900 - 470, 250)));
    }

    #[test]
    fn sticky_header_when_scrolling_up() {
        let doc = page(180, 1500, 6);
        let header = solid(180, 30, [40, 40, 90, 255]);
        let mut s = Stitcher::new(30_000);
        let offsets = [900, 850, 780, 700, 640, 600];
        run(
            &mut s,
            offsets
                .iter()
                .map(|&o| view(&doc, o, 280, Some(&header), None)),
        );
        let out = s.image().unwrap();
        // L'en-tête en haut, puis le contenu de la page de 630 à 900 + 280.
        assert_eq!(crop(&out, 0, 0, 180, 30), header);
        assert_eq!(out.height(), 30 + (900 + 280 - 630));
        assert_eq!(
            crop(&out, 0, 30, 180, out.height() - 30),
            crop(&doc, 0, 630, 180, 900 + 280 - 630)
        );
    }

    #[test]
    fn too_fast_is_lost_then_recovers() {
        let doc = page(140, 2000, 7);
        let mut s = Stitcher::new(30_000);
        assert_eq!(s.push(view(&doc, 0, 200, None, None)), Step::First);
        assert!(matches!(
            s.push(view(&doc, 60, 200, None, None)),
            Step::Moved { .. }
        ));
        // Saut plus grand que la zone : aucun recouvrement.
        assert_eq!(s.push(view(&doc, 600, 200, None, None)), Step::Lost);
        assert_eq!(s.push(view(&doc, 900, 200, None, None)), Step::Lost);
        // Retour près de la dernière position acceptée : ça reprend.
        assert!(matches!(
            s.push(view(&doc, 150, 200, None, None)),
            Step::Moved { delta: 90, .. }
        ));
        assert_eq!(s.image().unwrap(), crop(&doc, 0, 0, 140, 350));
    }

    #[test]
    fn horizontal_scrolling_is_detected() {
        // Page large : on la transpose pour réutiliser le générateur.
        let doc = transpose(&page(140, 1200, 8));
        assert_eq!(doc.dimensions(), (1200, 140));
        let w = 300;
        let mut s = Stitcher::new(30_000);
        let offsets: Vec<u32> = (0..=900).step_by(41).chain([900]).collect();
        let frames = offsets.iter().map(|&o| crop(&doc, o, 0, w, 140));
        let steps = run(&mut s, frames);
        assert!(steps.iter().all(|s| !matches!(s, Step::Lost)), "{steps:?}");
        assert_eq!(s.axis(), Some(Axis::Horizontal));
        assert_eq!(s.size(), Some((1200, 140)));
        assert!(s.image().unwrap() == doc);
        let p = s.preview(120, 100).unwrap();
        assert_eq!(p.dimensions(), (120, 14));
    }

    #[test]
    fn length_is_capped() {
        let doc = page(100, 3000, 9);
        let mut s = Stitcher::new(700);
        let steps = run(
            &mut s,
            (0..=2700)
                .step_by(50)
                .map(|o| view(&doc, o, 300, None, None)),
        );
        assert!(steps.contains(&Step::Full));
        assert!(s.is_full());
        let out = s.image().unwrap();
        assert_eq!(out.dimensions(), (100, 700));
        assert_eq!(out, crop(&doc, 0, 0, 100, 700));
    }

    #[test]
    fn noisy_frames_use_the_fuzzy_fallback() {
        let doc = page(200, 1400, 10);
        let mut s = Stitcher::new(30_000);
        let mut seed = 0u64;
        let mut noisy = |img: RgbaImage| {
            seed += 1;
            let mut img = img;
            for (i, p) in img.pixels_mut().enumerate() {
                let n = (mix(seed * 1_000_003 + i as u64) % 5) as i16 - 2;
                for c in &mut p.0[..3] {
                    *c = (i16::from(*c) + n).clamp(0, 255) as u8;
                }
            }
            img
        };
        let offsets = [0u32, 31, 70, 140, 190, 260, 330, 400];
        let steps: Vec<Step> = offsets
            .iter()
            .map(|&o| s.push(noisy(view(&doc, o, 260, None, None))))
            .collect();
        let deltas: Vec<i32> = steps
            .iter()
            .filter_map(|s| match s {
                Step::Moved { delta, .. } => Some(*delta),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, [31, 39, 70, 50, 70, 70, 70], "{steps:?}");
        assert_eq!(s.size(), Some((200, 400 + 260)));
        // Bruit seul, sans mouvement : rien ne bouge.
        assert_eq!(s.push(noisy(view(&doc, 400, 260, None, None))), Step::Still);
    }

    #[test]
    fn blank_content_never_moves() {
        let mut s = Stitcher::new(30_000);
        let white = solid(100, 200, [255, 255, 255, 255]);
        assert_eq!(s.push(white.clone()), Step::First);
        assert_eq!(s.push(white.clone()), Step::Still);
        // Contenu uniforme mais différent : impossible de recaler, on repart de là.
        let grey = solid(100, 200, [200, 200, 200, 255]);
        assert_eq!(s.push(grey), Step::Lost);
        assert_eq!(s.axis(), None);
    }

    #[test]
    fn small_changes_are_not_movement() {
        let doc = page(160, 600, 11);
        let mut s = Stitcher::new(30_000);
        let base = view(&doc, 100, 240, None, None);
        s.push(base.clone());
        // Curseur de texte qui clignote : un petit rectangle change.
        let mut caret = base.clone();
        for y in 100..118 {
            for x in 50..52 {
                caret.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        assert_eq!(s.push(caret), Step::Still);
        assert_eq!(s.push(base), Step::Still);
    }

    #[test]
    fn moving_scrollbar_and_animation_are_tolerated() {
        let doc = page(220, 1600, 12);
        let mut s = Stitcher::new(30_000);
        let h = 300;
        let offsets: Vec<u32> = (0..=1300).step_by(45).chain([1300]).collect();
        let frames = offsets.iter().enumerate().map(|(i, &o)| {
            let mut f = view(&doc, o, h, None, None);
            // Barre de défilement à droite : le curseur descend avec la page.
            let thumb = o * h / 1600;
            for y in 0..h {
                let on = y >= thumb && y < thumb + 56;
                for x in 210..220 {
                    f.put_pixel(
                        x,
                        y,
                        if on {
                            Rgba([90, 90, 90, 255])
                        } else {
                            Rgba([235, 235, 235, 255])
                        },
                    );
                }
            }
            // Petite animation fixe à l'écran (indicateur de chargement).
            for y in 150..170 {
                for x in 20..40 {
                    f.put_pixel(x, y, Rgba([(i * 40 % 255) as u8, 0, 0, 255]));
                }
            }
            f
        });
        let steps = run(&mut s, frames);
        assert!(steps.iter().all(|s| !matches!(s, Step::Lost)), "{steps:?}");
        assert_eq!(s.size(), Some((220, 1600)));
        // Le contenu, hors barre de défilement et animation, est celui de la page.
        let out = s.image().unwrap();
        assert_eq!(crop(&out, 0, 400, 200, 800), crop(&doc, 0, 400, 200, 800));
    }

    #[test]
    fn reset_starts_over() {
        let doc = page(100, 800, 13);
        let mut s = Stitcher::new(30_000);
        run(&mut s, [0, 40, 80].map(|o| view(&doc, o, 200, None, None)));
        assert_eq!(s.size(), Some((100, 280)));
        s.reset();
        assert_eq!(s.size(), None);
        assert_eq!(s.push(view(&doc, 300, 200, None, None)), Step::First);
    }

    #[test]
    fn preview_keeps_the_aspect_ratio() {
        let doc = page(200, 2000, 14);
        let mut s = Stitcher::new(30_000);
        run(
            &mut s,
            (0..=1700)
                .step_by(60)
                .chain([1700])
                .map(|o| view(&doc, o, 300, None, None)),
        );
        let p = s.preview(100, 4000).unwrap();
        assert_eq!(p.dimensions(), (100, 1000));
        let p = s.preview(100, 500).unwrap();
        assert_eq!(p.dimensions(), (50, 500));
        assert_eq!(*p.get_pixel(0, 0), *doc.get_pixel(2, 2));
    }

    /// Coût d'une image de la taille d'une demi-zone Retina (1470 × 956 px).
    /// `cargo test stitch_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn stitch_cost() {
        let doc = page(1470, 6000, 15);
        let frames: Vec<RgbaImage> = (0..40)
            .map(|i| view(&doc, i * 120, 956, None, None))
            .collect();
        let mut s = Stitcher::new(30_000);
        let start = std::time::Instant::now();
        for f in frames.iter().cloned() {
            s.push(f);
        }
        let exact = start.elapsed() / 40;
        let still = std::time::Instant::now();
        for _ in 0..10 {
            s.push(frames[39].clone());
        }
        let still = still.elapsed() / 10;
        let p = std::time::Instant::now();
        s.preview(400, 4096);
        let preview = p.elapsed();
        // Repli par profils : bruit d'un niveau sur une image sur deux.
        let mut s = Stitcher::new(30_000);
        let start = std::time::Instant::now();
        for (i, f) in frames.iter().take(10).enumerate() {
            let mut f = f.clone();
            if i % 2 == 1 {
                for p in f.pixels_mut() {
                    p.0[0] ^= 1;
                }
            }
            s.push(f);
        }
        let fuzzy = start.elapsed() / 10;
        eprintln!(
            "par image : {exact:?} (empreintes), {still:?} (immobile), {fuzzy:?} (repli) ; aperçu {preview:?}"
        );
        assert_eq!(s.size().map(|(w, _)| w), Some(1470));
    }

    #[test]
    fn transpose_swaps_axes() {
        let img = RgbaImage::from_fn(3, 2, |x, y| Rgba([x as u8, y as u8, 0, 255]));
        let t = transpose(&img);
        assert_eq!(t.dimensions(), (2, 3));
        assert_eq!(t.get_pixel(1, 2).0, [2, 1, 0, 255]);
        assert_eq!(transpose(&t), img);
    }

    #[test]
    fn different_frame_size_is_rejected() {
        let mut s = Stitcher::new(30_000);
        s.push(solid(10, 10, [0, 0, 0, 255]));
        assert_eq!(s.push(solid(12, 10, [0, 0, 0, 255])), Step::Lost);
    }
}
