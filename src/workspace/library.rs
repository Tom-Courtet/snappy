//! Contenu de l'espace de travail : les médias du dossier courant, filtrés (texte,
//! type, date) et triés. Logique pure, sans interface.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::files::{self, CaptureContext, Kind, Media};

/// Un média et ce qu'on sait de lui.
#[derive(Debug, Clone)]
pub struct Item {
    pub media: Media,
    /// Application et fenêtre capturées (`"context"` du `.json`), si notées.
    pub context: CaptureContext,
    /// Jour local de la capture (voir [`day_number`]).
    pub day: i64,
}

/// Ordre d'affichage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    /// Plus ancienne d'abord (Screenpresso), la plus récente en bas.
    #[default]
    DateAsc,
    DateDesc,
    NameAsc,
    NameDesc,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Self::DateAsc, Self::DateDesc, Self::NameAsc, Self::NameDesc];

    pub fn label(self) -> &'static str {
        match self {
            Self::DateAsc => "Date (plus ancienne d'abord)",
            Self::DateDesc => "Date (plus récente d'abord)",
            Self::NameAsc => "Nom (A → Z)",
            Self::NameDesc => "Nom (Z → A)",
        }
    }
}

/// Filtre de date de la barre de recherche.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DateFilter {
    #[default]
    All,
    Today,
    Week,
    Month,
    Older,
}

impl DateFilter {
    pub const ALL: [DateFilter; 5] = [Self::All, Self::Today, Self::Week, Self::Month, Self::Older];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "Tout",
            Self::Today => "Aujourd'hui",
            Self::Week => "7 derniers jours",
            Self::Month => "30 derniers jours",
            Self::Older => "Plus ancien",
        }
    }

    /// Le média du jour `day` passe-t-il, aujourd'hui étant `today` ?
    pub fn accepts(self, day: i64, today: i64) -> bool {
        let age = today - day;
        match self {
            Self::All => true,
            Self::Today => age <= 0,
            Self::Week => age < 7,
            Self::Month => age < 30,
            Self::Older => age >= 30,
        }
    }
}

/// Recherche et filtres.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub text: String,
    pub images: bool,
    pub videos: bool,
    pub documents: bool,
    pub date: DateFilter,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            text: String::new(),
            images: true,
            videos: true,
            documents: true,
            date: DateFilter::All,
        }
    }
}

impl Filter {
    /// Un filtre restreint-il l'affichage ?
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }

    fn accepts_kind(&self, kind: Kind) -> bool {
        // Aucun type coché : tout passe (plutôt qu'une grille vide).
        if !(self.images || self.videos || self.documents) {
            return true;
        }
        match kind {
            Kind::Image => self.images,
            Kind::Video => self.videos,
            Kind::Document => self.documents,
        }
    }

    /// Chaque mot du texte doit se trouver dans le nom du fichier, l'application ou
    /// le titre de la fenêtre capturée (sans tenir compte de la casse).
    fn accepts_text(&self, item: &Item) -> bool {
        let words: Vec<String> = self
            .text
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        if words.is_empty() {
            return true;
        }
        let haystack = [
            Some(item.media.name()),
            item.context.app.clone(),
            item.context.title.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
        words.iter().all(|w| haystack.contains(w.as_str()))
    }

    pub fn accepts(&self, item: &Item, today: i64) -> bool {
        self.accepts_kind(item.media.kind)
            && self.date.accepts(item.day, today)
            && self.accepts_text(item)
    }
}

/// Numéro du jour local d'un instant, `offset` étant le décalage horaire (en
/// secondes) à cet instant. Deux instants du même jour local ont le même numéro.
pub fn day_number(unix_seconds: i64, offset: i32) -> i64 {
    (unix_seconds + i64::from(offset)).div_euclid(86_400)
}

/// Jour local d'un instant, avec le fuseau du système.
pub fn local_day(t: SystemTime) -> i64 {
    let Ok(ts) = jiff::Timestamp::try_from(t) else {
        return 0;
    };
    let offset = jiff::tz::TimeZone::system().to_offset(ts).seconds();
    day_number(ts.as_second(), offset)
}

/// Clé de tri d'un nom : sans casse, et les nombres comparés par valeur
/// (« capture-2 » avant « capture-10 »).
fn natural_key(name: &str) -> Vec<(u8, String, u64)> {
    let mut parts = Vec::new();
    let mut chars = name.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut digits = String::new();
            while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                digits.push(d);
                chars.next();
            }
            parts.push((0, String::new(), digits.parse().unwrap_or(u64::MAX)));
        } else {
            let mut text = String::new();
            while let Some(&t) = chars.peek().filter(|t| !t.is_ascii_digit()) {
                text.extend(t.to_lowercase());
                chars.next();
            }
            parts.push((1, text, 0));
        }
    }
    parts
}

/// Indices des éléments à afficher, dans l'ordre demandé.
pub fn view(items: &[Item], filter: &Filter, sort: Sort, today: i64) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..items.len())
        .filter(|&i| filter.accepts(&items[i], today))
        .collect();
    let by_date = |a: &usize, b: &usize| {
        let (a, b) = (&items[*a].media, &items[*b].media);
        a.date().cmp(&b.date()).then_with(|| a.path.cmp(&b.path))
    };
    let by_name = |a: &usize, b: &usize| {
        let (a, b) = (&items[*a].media, &items[*b].media);
        natural_key(&a.name())
            .cmp(&natural_key(&b.name()))
            .then_with(|| a.path.cmp(&b.path))
    };
    match sort {
        Sort::DateAsc => idx.sort_by(by_date),
        Sort::DateDesc => idx.sort_by(|a, b| by_date(b, a)),
        Sort::NameAsc => idx.sort_by(by_name),
        Sort::NameDesc => idx.sort_by(|a, b| by_name(b, a)),
    }
    idx
}

/// Relit le dossier ; le contexte des captures (lu dans leur `.json`) est repris
/// de `previous` tant que le `.json` n'a pas changé.
pub fn scan(dir: &Path, previous: &[Item]) -> Vec<Item> {
    let known: HashMap<&PathBuf, &Item> = previous.iter().map(|i| (&i.media.path, i)).collect();
    files::list(dir)
        .into_iter()
        .map(|media| {
            let context = match known.get(&media.path) {
                Some(old) if old.media.doc_modified == media.doc_modified => old.context.clone(),
                _ if media.doc_modified.is_some() => CaptureContext::read(&media.path),
                _ => CaptureContext::default(),
            };
            let day = local_day(media.date());
            Item {
                media,
                context,
                day,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn item(name: &str, secs: u64, kind: Kind, app: Option<&str>, title: Option<&str>) -> Item {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        Item {
            media: Media {
                path: PathBuf::from("/c").join(name),
                kind,
                size: 1,
                modified: t,
                created: Some(t),
                doc_modified: None,
            },
            context: CaptureContext {
                app: app.map(str::to_owned),
                title: title.map(str::to_owned),
            },
            day: day_number(secs as i64, 0),
        }
    }

    fn names(items: &[Item], idx: &[usize]) -> Vec<String> {
        idx.iter().map(|&i| items[i].media.name()).collect()
    }

    #[test]
    fn day_numbers_follow_local_midnight() {
        // 23 h 30 UTC le 1er janvier 1970 : encore le jour 0 à UTC, déjà le jour 1 à UTC+1.
        let t = 23 * 3600 + 1800;
        assert_eq!(day_number(t, 0), 0);
        assert_eq!(day_number(t, 3600), 1);
        assert_eq!(day_number(-1, 0), -1);
    }

    #[test]
    fn date_buckets() {
        let today = 100;
        assert!(DateFilter::Today.accepts(100, today));
        assert!(!DateFilter::Today.accepts(99, today));
        assert!(DateFilter::Week.accepts(94, today));
        assert!(!DateFilter::Week.accepts(93, today));
        assert!(DateFilter::Month.accepts(71, today));
        assert!(!DateFilter::Month.accepts(70, today));
        assert!(DateFilter::Older.accepts(70, today));
        assert!(!DateFilter::Older.accepts(71, today));
        assert!(DateFilter::All.accepts(-5, today));
    }

    #[test]
    fn text_matches_name_app_and_title() {
        let items = vec![
            item(
                "snappy-1.png",
                10,
                Kind::Image,
                Some("Safari"),
                Some("Facture EDF"),
            ),
            item("bug-menu.png", 20, Kind::Image, None, None),
            item("demo.mp4", 30, Kind::Video, Some("Keynote"), None),
        ];
        let f = |text: &str| Filter {
            text: text.into(),
            ..Filter::default()
        };
        let all = |filter: &Filter| names(&items, &view(&items, filter, Sort::DateAsc, 0));
        assert_eq!(all(&f("safari")), ["snappy-1.png"]);
        assert_eq!(all(&f("facture edf")), ["snappy-1.png"]);
        assert_eq!(all(&f("BUG")), ["bug-menu.png"]);
        assert_eq!(all(&f("keynote demo")), ["demo.mp4"]);
        assert!(all(&f("safari keynote")).is_empty());
        assert_eq!(all(&f("  ")).len(), 3);
    }

    #[test]
    fn type_filters() {
        let items = vec![
            item("a.png", 1, Kind::Image, None, None),
            item("b.mov", 2, Kind::Video, None, None),
            item("c.pdf", 3, Kind::Document, None, None),
        ];
        let only_videos = Filter {
            images: false,
            documents: false,
            ..Filter::default()
        };
        assert_eq!(
            names(&items, &view(&items, &only_videos, Sort::DateAsc, 0)),
            ["b.mov"]
        );
        let none = Filter {
            images: false,
            videos: false,
            documents: false,
            ..Filter::default()
        };
        assert_eq!(view(&items, &none, Sort::DateAsc, 0).len(), 3);
        assert!(!Filter::default().is_active());
        assert!(only_videos.is_active());
    }

    #[test]
    fn date_filter_uses_capture_days() {
        let items = vec![
            item("old.png", 0, Kind::Image, None, None),
            item("new.png", 40 * 86_400, Kind::Image, None, None),
        ];
        let today = 40;
        let f = |date| Filter {
            date,
            ..Filter::default()
        };
        assert_eq!(
            names(
                &items,
                &view(&items, &f(DateFilter::Today), Sort::DateAsc, today)
            ),
            ["new.png"]
        );
        assert_eq!(
            names(
                &items,
                &view(&items, &f(DateFilter::Older), Sort::DateAsc, today)
            ),
            ["old.png"]
        );
    }

    #[test]
    fn sorting() {
        let items = vec![
            item("capture-10.png", 3, Kind::Image, None, None),
            item("Capture-2.png", 1, Kind::Image, None, None),
            item("alpha.png", 2, Kind::Image, None, None),
        ];
        let f = Filter::default();
        let sorted = |sort| names(&items, &view(&items, &f, sort, 0));
        assert_eq!(
            sorted(Sort::DateAsc),
            ["Capture-2.png", "alpha.png", "capture-10.png"]
        );
        assert_eq!(
            sorted(Sort::DateDesc),
            ["capture-10.png", "alpha.png", "Capture-2.png"]
        );
        assert_eq!(
            sorted(Sort::NameAsc),
            ["alpha.png", "Capture-2.png", "capture-10.png"]
        );
        assert_eq!(
            sorted(Sort::NameDesc),
            ["capture-10.png", "Capture-2.png", "alpha.png"]
        );
    }

    #[test]
    fn sort_is_stored_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&Sort::DateDesc).unwrap(),
            r#""date_desc""#
        );
    }

    #[test]
    fn scan_reads_contexts_once() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        std::fs::write(&png, b"x").unwrap();
        std::fs::write(
            crate::history::doc_path(&png),
            r#"{"annotations":[],"context":{"app":"Mail"}}"#,
        )
        .unwrap();
        let first = scan(dir.path(), &[]);
        assert_eq!(first[0].context.app.as_deref(), Some("Mail"));
        // Contexte repris tel quel si le .json n'a pas changé.
        let mut previous = first.clone();
        previous[0].context.app = Some("en cache".into());
        assert_eq!(
            scan(dir.path(), &previous)[0].context.app.as_deref(),
            Some("en cache")
        );
    }
}
