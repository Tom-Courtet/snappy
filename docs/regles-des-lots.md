# Règles communes des lots « ajouts Screenpresso »

Projet : Snappy, capture d'écran en Rust (edition 2024, egui/eframe 0.37 glow, winit 0.30 pour l'agent,
tray-icon 0.26, famille objc2 0.6 / objc2-* 0.3, windows-sys 0.61). Cibles : macOS (principale) et
Windows 10 (compilé depuis le Mac avec cargo-xwin).

Cahier des charges : /Users/tom/Downloads/SnappyxTidy/screenpresso-bilan.md. Lire l'encadré du début,
le § 11.6 et les sections de son lot. La section 13 (interface réelle) prime sur le reste.
Captures de référence : /Users/tom/Downloads/SnappyxTidy/captures/.

## Exigences du propriétaire (non négociables)
1. Ce que fait Snappy aujourd'hui lui convient : on AJOUTE (fonctions, options, réglages). Le comportement
   actuel reste le défaut.
2. Ne rien remplacer, modifier ni supprimer d'existant (comportement, interface, raccourci, format de fichier,
   réglage, dépendance) sans son accord. Si un ajout l'exige : NE PAS le faire, le noter dans le compte rendu,
   rubrique « Décisions à demander » (question, options, recommandation).
3. Quand Screenpresso fait autrement que Snappy : le proposer en option (désactivée par défaut), ou poser la question.
4. Rien d'irréversible ni de visible à l'extérieur : pas de push, pas de publication, pas de suppression de
   fichiers que tu n'as pas créés. Un agent Snappy tourne déjà sur la machine (dist/Snappy.app) : ne pas
   l'arrêter. Pour tout essai manuel : `SNAPPY_DATA_DIR=$(mktemp -d)` pour ne pas toucher aux vraies données.

## Socle déjà en place (commit « Socle des ajouts Screenpresso »)
- `ipc::Mode` a 13 actions : Region, Screen, Previous, AllScreens, Window, Delayed, Scrolling, Ocr, Qr,
  ColorPicker, Video, VideoRegion, Workspace (`Mode::ALL`, `arg()`, `from_arg()`).
- `Config::extra_hotkeys` (raccourcis des nouvelles actions, aucun par défaut), `Config::hotkey(mode)`.
- `snappy --capture <arg>` aiguille vers des points d'entrée vides à compléter :
  `workspace::run`, `vision::run`, `video::run`, `scroll::run`, `capture_modes::run`.
- `overlay::pick(store, capture, with_cursor, Purpose) -> Result<Option<Picked>>` : contrat de la sélection
  d'une zone pour l'OCR, le QR, la vidéo et le défilement (vide pour l'instant, lot « Capture »).

## Façon de travailler
- Style du code existant : commentaires et textes d'interface en français, `//!` et `///`, pas de nouvel
  avertissement, `cargo fmt`. Tests unitaires pour toute logique pure.
- Icônes Lucide via `src/ui/icons.rs` (généré par `tools/subset_icons.py` ; voir le script pour en ajouter).
  Thème : `src/ui/theme.rs`. Raccourcis affichés : `src/ui/keys.rs`.
- Nouveaux champs persistés : `#[serde(default)]`, valeur par défaut = comportement actuel. Les ajouter EN FIN
  de structure (Config, Doc…) dans un bloc commenté au nom du lot, pour faciliter la fusion.
- Dépendances : en fin de section du Cargo.toml, dans un bloc commenté au nom du lot. Préférer les crates
  légères ; signaler toute licence GPL/AGPL au lieu de l'ajouter.
- Code propre à macOS derrière `#[cfg(target_os = "macos")]` ; sous Windows, une implémentation ou un message
  clair « pas encore disponible sous Windows ». La cible Windows doit compiler :
  `cargo xwin check --target x86_64-pc-windows-msvc`.
- Vérifier : `cargo build`, `cargo test`, `cargo clippy`, le check Windows. Pas de build release (disque).
- À la fin : commit(s) sur ta branche (message en français, terminé par
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`), puis `cargo clean` dans ta copie.

## Compte rendu final (en français, concis)
1. Ajouté (par fonction, avec fichiers).
2. Touché dans l'existant (fichier, fonction, pourquoi) : normalement seulement des ajouts.
3. Décisions à demander au propriétaire (question · options · recommandation).
4. Ce qui reste à vérifier sur sa machine (permissions, raccourcis, multi-écrans…) et limites connues.
5. Nom de ta branche et liste des commits.
