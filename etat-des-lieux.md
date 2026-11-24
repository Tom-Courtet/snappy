# Snappy × Screenpresso : état des lieux

> Écrit le 24/11/2026 à partir de `screenpresso-bilan.md` (le « bilan »), de l'inventaire du code et d'une étude de faisabilité macOS/Windows.
> **Mise à jour du 24/11/2026 (soir) : les 9 lots sont fusionnés** sur la branche `screenpresso` (voir § 8 Avancement et les nouvelles décisions D27–D41). 315 tests unitaires passent, zéro avertissement (clippy compris), la cible Windows compile.
> Mis à jour à la fin de chaque lot. Branche de travail : `screenpresso`. Le code d'avant les ajouts est sur `main` (commit `809bf81`).

## 1. Résumé

1. Snappy fait déjà le cœur de Screenpresso, et souvent mieux. C'est une application de barre des menus (macOS et Windows 10) avec des raccourcis globaux configurables. Elle capture une zone, l'écran ou la zone précédente sur un écran figé, sur plusieurs écrans. La surcouche détecte les fenêtres et a une loupe, un zoom et une pipette (C).
2. L'éditeur est vectoriel et non destructif (JSON à côté du PNG) : 15 outils, favoris, ombre, flèches courbes, loupe et tampons. Le même moteur (tiny-skia) dessine l'écran et l'export.
3. Ce qui manque le plus :
   - la **fenêtre Espace de travail** et le **clic gauche sur l'icône** (§ 11.6, exigence explicite) ;
   - la **vidéo** ;
   - l'**OCR et le QR** ;
   - la **capture défilante** ;
   - l'**onglet Image** de l'éditeur (recadrer, redimensionner, bordures) ;
   - le **partage** et l'**impression** ;
   - les réglages « Ensuite » et le contenu du presse-papiers.
4. Risques principaux :
   - la capture défilante (algorithme de recalage) ;
   - la vidéo (ScreenCaptureKit, synchronisation, Windows) ;
   - `CGDisplayCreateImage`, déclaré obsolète par Apple : il marche encore sur macOS 26 mais peut disparaître ;
   - les permissions macOS, perdues à chaque build tant que la signature est « ad hoc ».
5. Le choix de l'interface est déjà tranché : egui/eframe, tout en Rust, sans Tauri. Le format des annotations est le JSON existant, étendu par des champs optionnels pour garder la compatibilité.
6. Méthode :
   - un socle commun (13 actions globales, points d'entrée vides), puis 9 lots en parallèle, chacun dans sa copie du dépôt et sur ses propres fichiers ;
   - fusion et vérification sur la branche `screenpresso` ;
   - rien d'existant n'est remplacé sans accord : tout ce qui le demanderait est listé en § 5.
7. Exception assumée : le clic gauche sur l'icône et l'ordre du menu changent, parce que c'est l'exigence explicite du § 11.6. Les entrées propres à Snappy sont conservées.

## 2. Inventaire de l'existant (à ne pas casser)

**Processus.**
- Un seul exécutable :
  - l'agent (barre des menus et raccourcis), dans un processus ;
  - chaque capture, l'éditeur et les réglages, chacun dans son propre processus.
- Communication locale : socket Unix sur macOS, TCP local sur Windows.
- CLI : `--region [--delay N]`, `--screen`, `--previous`, `--edit`, `--settings`, `--export`, `--quit`.

**Icône (avant les ajouts).**
- Clic gauche et clic droit ouvrent le même menu :
  - Capturer une zone, Capturer l'écran, Capturer la zone précédente, Capturer une zone dans N s ;
  - Historique des captures, Images enregistrées, Réglages… ;
  - (Autoriser l'enregistrement de l'écran…) ;
  - Quitter Snappy.
- Compte à rebours affiché à côté de l'icône.

**Après une capture.**

| Mode | Historique | Presse-papiers | Export | Éditeur |
|---|---|---|---|---|
| Zone → Copier (Entrée, ⌘C, double-clic) | oui | oui | – | – |
| Zone → Enregistrer (⌘S) | oui | – | PNG dans `~/Pictures/Snappy` | – |
| Zone → Annoter (E) | oui | – | – | oui |
| Écran | oui | oui | – | oui |
| Zone précédente | oui | oui | – | – (silencieux) |

- Historique : `~/Library/Application Support/Snappy/captures/snappy-AAAA-MM-JJ-HHMMSS.png` + `.json`.
- Raccourcis par défaut :
  - macOS : ⇧⌘2 zone, ⇧⌘1 écran, ⌥⇧⌘2 zone précédente ;
  - Windows : Impr. écran, Ctrl+Alt+Maj+Impr. écran, Maj+Impr. écran.

**Surcouche.**
- Touches : C copie la couleur, M curseur, H aide, 0 zoom 1:1, flèches (Maj ×10) déplacent la sélection, Entrée/⌘C/double-clic copie, ⌘S, E, Échap.
- Souris : clic sur une fenêtre pour la sélectionner ; clic sur le bord haut ou le bureau pour l'écran entier.
- Molette ou pincement : zoom jusqu'à ×12. Loupe 15 × 15.

**Éditeur.**
- Outils et touches : V A L R O H F B X P T U N M K.
- Palette de 8 couleurs + hexadécimal + pipette (Alt+clic). Tailles S/M/L (1/2/3).
- Flèches simple, double, grosse, connecteur, courbe. Rectangle arrondi ou plein. Texte encadré. Bulle avec pointe. Pastille avec pointe. Loupe ×2/3/4. 14 tampons. Ombre.
- Favoris ★ (8). ⌘Z/⌘⇧Z, ⌘D, ⌘], ⌘[, Suppr. ⌘C copie l'image, ⌘S enregistre.
- Le JSON est réécrit à chaque modification.

**Réglages.** Raccourcis (enregistreur, suspension pendant la saisie), curseur inclus, délai 1–10 s, dossier d'enregistrement.

**Fichiers.** `config.json`, `editor.json`, `last_region.json`, `<capture>.json`. Tous sont lus avec des valeurs par défaut pour rester compatibles. Il y a un test de compatibilité des anciens JSON.

## 3. Tableau des écarts

Statut avant les ajouts : **existe** / **partiel** / **absent** / **différent**. La colonne Lot indique qui le prend en charge (A–I, voir § 6). Taille : S < 1 j, M 1–3 j, L > 3 j.

### Capture (§ 3, 13.5)

| Fonction | § | Statut | Emplacement | Lot | Taille |
|---|---|---|---|---|---|
| Écran figé multi-écrans, détection de fenêtre | 3.1 | existe | overlay.rs, capture/ | – | – |
| Détection des sous-éléments (boutons, panneaux) | 3.1 | absent | – | décision D10 | M |
| Loupe + dimensions, zoom molette | 3.1 | existe | overlay.rs | – | – |
| Flèches au pixel | 3.1 | existe (déplacement de la sélection) | overlay.rs | – | – |
| `F` écran entier | 3.1 | absent | overlay.rs | C | S |
| `M` curseur | 3.1 | existe | overlay.rs | – | – |
| `O` pipette, bascule hexadécimal/décimal | 3.1, 8 | partiel (C copie l'hexadécimal) | overlay.rs | C | S |
| `P` imprimer, `W` filigrane pendant la capture | 3.1 | absent | – | décision D9 | S |
| Réticules gris | 13.5 | différent (repères en croix) | overlay.rs | C (option) | S |
| Capture immédiate au relâchement | 13.5 | différent (barre d'actions) | overlay.rs | décision D8 | S |
| Tous les écrans en une image | 3.2 | absent | capture_modes.rs | C | S |
| Fenêtre active | 3.2 | absent | capture_modes.rs | C | S |
| Capture différée | 3.2 | existe (menu) ; raccourci ajouté au socle | agent | socle | – |
| Capture avec défilement | 3.3 | absent | scroll/ | D | L |
| Contexte (appli, titre) enregistré | 3.1 | absent | history.rs Doc | C + B (réglage) | S |
| Masquer les icônes du bureau | 3.1 | absent | – | décision D11 | M |
| HDR / JXR | 3.1 | absent | – | non prévu (Windows) | – |
| Floutage auto des données sensibles | 3.1 | absent | vision/ | G (détection) ; éditeur à brancher | M |
| Ligne de commande | 3.5 | partiel | main.rs | socle + lots | S |

### Éditeur (§ 4, 13.1, 13.2)

| Fonction | § | Statut | Emplacement | Lot | Taille |
|---|---|---|---|---|---|
| Flèche (styles, ombre) | 4.2 | existe | model/render | – | – |
| Opacité, trait pointillé ou tiret-point | 13.2 | absent | model/render | E | S |
| Rectangle et ellipse : remplissage extérieur, flou extérieur | 13.2 | partiel (outil Projecteur) | model/render | E | S |
| Texte : gras, italique, souligné, alignement, halo | 4.2 | absent (texte encadré seulement) | render.rs | E | M |
| Choix de la police système | 13.2 | absent | – | décision D14 | M |
| Bulle : 3 types | 13.2 | partiel (avec pointe) | model | E | S |
| Numéro : formats 1/A/i, départ, connecteurs, renumérotation | 4.2 | partiel (pastille + pointe) | model | E | M |
| Surligneur, main levée, loupe | 4.2 | existe | – | – | – |
| Polygone | 4.2 | absent | tools/model | E | M |
| Accolades | 4.2 | absent | tools/model | E | S |
| Tampons par catégories, mes tampons, insertion d'image | 13.2 | partiel (14 tampons) | model/render | E | M |
| Sélection multiple, alignement, distribution | 4.2 | absent | editor/mod.rs | E | M |
| Couper, copier, coller des objets | 4.2 | absent | editor | E | S |
| Préréglages par outil | 13.2 | différent (favoris globaux) | prefs.rs | E (en plus) | M |
| Poignée de rotation | 13.2 | absent | – | décision D13 | M |
| Fenêtre d'options flottante | 13.2 | différent (barre d'options) | bars.rs | décision D13 | – |
| Valider, image précédente/suivante, ⌘O, supprimer l'image, propriétés, plein écran | 13.1 | absent | editor | E | M |
| Recadrer, rotation | 4.3 | absent | effects.rs | F | M |
| Couper une bande (6 modes) | 13.1 | absent | effects.rs | F | M |
| Redimensionner, DPI | 13.1 | absent | effects.rs | F | S |
| Zone de travail (canevas fixe ou automatique) | 13.1 | absent | effects.rs | F | M |
| Bordure et effets (déchiré, reflet, ombre, rotation, N&B) | 13.1 | absent | effects.rs | F | M |
| Changer l'image principale | 13.1 | absent | effects | F | S |
| Supprimer l'arrière-plan (IA locale) | 4.3 | absent | vision/ | G (fonction) ; éditeur à brancher | S |
| Retouche par IA distante (Gemini) | 4.3 | absent | – | décision D22 | M |
| Imprimer | 13.1 | absent | share/ | I | S |

### Vidéo (§ 5, 13.5)

| Fonction | § | Statut | Lot | Taille |
|---|---|---|---|---|
| Enregistrement d'une zone en MP4 (macOS 15+, ScreenCaptureKit) | 5.1–5.2 | absent | H | L |
| Barre vidéo : tailles, micro, son système, chrono, pause, refaire | 5.1, 13.5 | absent | H | M |
| Compte à rebours, arrêt automatique | 5.1 | absent | H | S |
| Surlignage du curseur et des clics | 5.4 | absent | H (si le temps le permet) | S |
| Webcam | 5.5 | absent | décision D19 | M |
| Lecteur : découpe, image courante, muet, vitesse, export GIF | 5.7 | absent | H | M |
| Sous-titres, chapitres, normalisation, réduction de bruit | 5.3, 5.7 | absent | décision D19 | L |
| Vidéo sous Windows | 5.2 | absent | décision D17 | L |

### Espace de travail, export, partage (§ 6, 7, 13.3)

| Fonction | § | Statut | Lot | Taille |
|---|---|---|---|---|
| Clic gauche → Espace de travail, clic droit → menu Screenpresso | 11.6 | différent | A | M |
| Fenêtre : vignettes, recherche, filtres de type et de date, tri, taille | 6.1, 13.3 | absent (le menu ouvre le dossier) | A | L |
| Plusieurs espaces de travail | 6.2 | absent | A | M |
| Menu contextuel : ouvrir, Finder, aperçu, enregistrer sous, copier le chemin, renommer, corbeille, fusionner, propriétés | 6.3 | absent | A | M |
| Glisser-déposer sortant et entrant | 6.1 | absent | A | M |
| Toujours devant, fermeture automatique, mémoriser la position | 13.3 | absent | A | S |
| Formats d'export (JPEG, WebP, BMP, TIFF, GIF) | 6.3, 9.1 | absent (PNG seulement) | A (enregistrer sous) + B (format par défaut) | S |
| Feuille de partage macOS, e-mail avec pièce jointe | 7.2 | absent | I | S |
| Services en ligne (S3, Drive, Dropbox, OneDrive, Imgur…) | 7.2 | absent | décision D20 | L |
| Générateur de documents PDF, HTML, GIF | 7.3 | absent | I | M |
| DOCX | 7.3 | absent | décision D21 | M |

### Fonctions annexes et réglages (§ 8, 9, 13.4, 14)

| Fonction | § | Statut | Lot | Taille |
|---|---|---|---|---|
| OCR « Copier le texte » + fenêtre de résultat | 8 | absent | G | S (Mac) / M (Windows) |
| QR et codes-barres | 8 | absent | G | S |
| Pipette autonome | 8 | partiel (C dans la surcouche) | C | S |
| Filigrane | 8 | absent | B | S |
| Aperçu rapide (3 dernières captures) | 8 | absent | A (aperçu Espace) | S |
| Réglage « Ensuite » (espace de travail, éditeur, info-bulle, rien) | 9.1, 11.6 | absent | B | S |
| Contenu du presse-papiers (image, chemin, les deux, rien) | 9.1 | absent | B | S |
| Format et nom de fichier (date, numérotation, modèle) | 9.1–9.2 | absent | B | S |
| Démarrer avec la session | 9.1 | absent | B | S |
| Onglet Vidéo des réglages | 13.4 | absent | B | S |
| Raccourcis pour toutes les actions + restaurer les défauts | 13.4 | partiel (3 actions) | socle + B | S |
| Exporter, importer, réinitialiser les réglages | 9.1 | absent | B | S |
| Centre Android, extension Chrome, stratégies, mises à jour, langues, licence PRO | 8, 9.4, 10 | absent | décision D24 | – |

## 4. Points où Snappy diffère de Screenpresso

| Point | Snappy | Screenpresso | Proposition |
|---|---|---|---|
| Après la sélection | barre Copier / Enregistrer / Annoter | capture immédiate puis action « Ensuite » | **garder**, option « capture immédiate » (D8) |
| Après une capture | selon le mode (voir § 2) | ouvre l'espace de travail | **garder par défaut** ; réglage « Ensuite » avec les choix Screenpresso (lot B) |
| Presse-papiers | seulement sur Copier, Écran et Zone précédente | toujours, image et chemin | **garder** ; réglage « Copier dans le presse-papiers » (lot B) |
| Emplacement des captures | `~/Library/Application Support/Snappy/captures` | `~/Pictures/Screenpresso` | **garder** (D3) |
| Barre d'outils de l'éditeur | en haut, avec une barre d'options | à gauche, fenêtre d'options flottante | **garder** (D12, D13) |
| Préréglages | favoris globaux (8) | palette numérotée par outil | **garder les favoris** et ajouter les préréglages par outil (lot E) |
| Pipette dans la surcouche | `C` copie l'hexadécimal | `O` pipette, `H` hexadécimal/décimal | **garder C** ; ajouter O, et D pour le décimal (H sert déjà à l'aide) |
| Raccourcis macOS | ⇧⌘2 / ⇧⌘1 / ⌥⇧⌘2 | `⌃⇧1…` suggérés par le bilan | **garder** ; nouvelles actions sans raccourci par défaut (D5) |
| Libellés du menu | « zone » | « région » | **garder « zone »** (D1) |
| Moteur de capture macOS | CoreGraphics (obsolète, marche encore) | GPU | **garder** pour l'instant (D7) |

## 5. Décisions à prendre

Classées par domaine. Les plus bloquantes sont en tête. « Bloque » indique ce qui attend la réponse. Tant qu'il n'y a pas de réponse, c'est la recommandation qui est appliquée quand elle ne touche pas à l'existant.

### Application, icône, espace de travail

- **D1 · Menu de l'icône : garder les libellés et les entrées propres à Snappy ?**
  - Contexte : le § 11.6 demande le contenu et l'ordre de Screenpresso. Snappy dit « zone », pas « région », et a en plus « Historique des captures » et « Images enregistrées ».
  - Options :
    - a) ordre Screenpresso, libellés Snappy, entrées en plus gardées ;
    - b) libellés Screenpresso à l'identique ;
    - c) retirer les entrées en plus.
  - **Reco : a)** (appliquée).
  - Bloque : rien.
- **D2 · Fenêtre Espace de travail : « toujours devant » et « fermeture automatique après 3 s » cochés par défaut, comme dans Screenpresso ?**
  - Contexte : c'est une fonction nouvelle, sans comportement existant à préserver.
  - Options : a) oui (Screenpresso) ; b) non ; c) seulement « toujours devant ».
  - **Reco : a)**. Les deux cases sont dans le menu ▼ de la fenêtre.
  - Bloque : rien (modifiable ensuite).
- **D3 · L'espace de travail montre l'historique actuel (`Application Support/Snappy/captures`) ou faut-il déplacer les captures vers `~/Pictures/Snappy` comme Screenpresso ?**
  - Options : a) garder l'emplacement actuel ; b) migrer (remplacement : à faire seulement avec ton accord) ; c) réglage « dossier de travail ».
  - **Reco : a)**, puis c) plus tard.
  - Bloque : la synchronisation cloud d'un espace.
- **D4 · Plusieurs espaces : la nouvelle capture va-t-elle dans l'espace sélectionné ?**
  - Contexte : aujourd'hui tout va à la racine de l'historique.
  - Options : a) toujours à la racine (actuel) ; b) dans l'espace sélectionné (Screenpresso) ; c) réglage.
  - **Reco : c)**, avec a) par défaut.
  - Bloque : la finition des espaces.
- **D5 · Raccourcis macOS par défaut des nouvelles actions (espace de travail, vidéo, OCR, défilement…)**
  - Contexte : aucun par défaut aujourd'hui. Le bilan suggère `⌃⇧W`, `⌃⇧5`, `⌃⇧6`…
  - Options :
    - a) aucun, chacun les choisit dans les réglages ;
    - b) la série `⌃⇧` du bilan pour les **nouvelles** actions seulement ;
    - c) migrer aussi les 3 raccourcis existants vers `⌃⇧` (remplacement).
  - **Reco : b)**.
  - Bloque : rien (a) est appliqué en attendant).
- **D6 · Raccourcis Windows par défaut des nouvelles actions : ceux de Screenpresso ?**
  - Raccourcis : Alt+Impr. écran fenêtre, Ctrl+Impr. écran vidéo, Ctrl+Maj+Impr. écran zone vidéo, Win+Maj+Impr. écran défilement, Win+Impr. écran espace de travail.
  - **Reco : oui** : ce sont tes habitudes au travail.
  - Bloque : rien.
- **D25 · Signature de l'app macOS**
  - Contexte : la signature « ad hoc » fait perdre les autorisations (Enregistrement de l'écran, Micro, Accessibilité) à chaque nouveau build.
  - Options : a) certificat « Apple Development » gratuit, stable ; b) Developer ID + notarisation (99 $/an, pour distribuer) ; c) rester en ad hoc.
  - **Reco : a)** dès maintenant. Je ne touche pas à la signature sans ton accord.
  - Bloque : les tests vidéo, micro et défilement automatique dans le .app.

### Capture

- **D7 · Moteur de capture macOS : passer de `CGDisplayCreateImage` (obsolète depuis macOS 15, fonctionne encore) à ScreenCaptureKit (`SCScreenshotManager`) ?**
  - Options : a) migrer maintenant ; b) migrer plus tard dans un lot dédié, avec mesure de la vitesse ; c) ne rien faire.
  - **Reco : b)**.
  - Bloque : D11, le HDR, l'exclusion propre des fenêtres.
- **D8 · Option « capture immédiate au relâchement » (Screenpresso) en plus de la barre d'actions de Snappy ?**
  - **Reco : oui, désactivée par défaut**.
  - Bloque : rien.
- **D9 · Ajouter `P` (imprimer) et `W` (filigrane) pendant la sélection ?**
  - **Reco : oui** (ajouts).
  - Bloque : rien.
- **D10 · Détection des sous-éléments de fenêtre (boutons, panneaux) via l'Accessibilité ?**
  - Contexte : demande la permission Accessibilité.
  - **Reco : plus tard, en option**.
- **D11 · Masquer les icônes du bureau pendant la capture ?**
  - Contexte : nécessite ScreenCaptureKit (D7).
  - **Reco : avec D7**.

### Éditeur

- **D12 · Barre d'outils en haut (Snappy) ou à gauche (Screenpresso) ?**
  - **Reco : garder en haut**, éventuellement une option plus tard.
- **D13 · Poignée de rotation des objets et fenêtre d'options flottante ?**
  - **Reco** : rotation plus tard (M) ; garder la barre d'options (pas de fenêtre flottante).
- **D14 · Choix de la police du texte parmi les polices système ?**
  - Contexte : ajoute une dépendance (`fontdb`) et du rendu.
  - **Reco : plus tard**, si tu en as l'usage.
- **D15 · ⌘C/⌘V dans l'éditeur**
  - Contexte : ⌘C copie l'image aujourd'hui.
  - **Reco** : couper, copier et coller les **objets** par le menu contextuel ; ⌘V colle une image du presse-papiers (ajout). Ne rien changer à ⌘C.

### Vidéo

- **D17 · Vidéo sous Windows**
  - Options :
    - a) `windows-capture` (Windows.Graphics.Capture + Media Foundation, MIT) ; sous Windows 10, une bordure jaune est imposée par le système ;
    - b) ffmpeg à côté (30–100 Mo) ;
    - c) macOS seulement.
  - **Reco : a)** dans un lot suivant.
- **D18 · Version minimale de macOS**
  - Contexte : l'écriture directe en MP4 (`SCRecordingOutput`) exige macOS 15 ; le bundle déclare 14.
  - Options : a) vidéo disponible à partir de 15, message sur 14 ; b) passer le minimum à 15 ; c) écrire via AVAssetWriter (compatible 13+, plus de code).
  - **Reco : a)**.
- **D19 · Quelles fonctions vidéo avancées ?**
  - Liste : webcam ronde (permission Caméra), sous-titres automatiques (whisper, modèle de 75 à 500 Mo), chapitres, normalisation du son, réduction de bruit.
  - **Reco** : webcam d'abord ; sous-titres plus tard.

### Partage, IA

- **D20 · Services en ligne : lesquels ?**
  - Candidats : S3 (lien présigné), SFTP, Google Drive, Dropbox, OneDrive, Imgur, Linear. Il faut des comptes, OAuth et le Trousseau.
  - **Reco** : feuille de partage macOS d'abord (faite), puis S3 et Google Drive si tu les utilises.
- **D21 · Générateur de documents : DOCX nécessaire, ou PDF + HTML suffisent ?**
  - **Reco : PDF + HTML**.
- **D22 · IA : retouche d'image (Gemini, OpenAI, Claude…) et traduction de l'OCR (DeepL, Google) ?**
  - Contexte : envoie tes captures à un service externe avec ta clé.
  - Options : a) rien ; b) Claude API avec ta clé, interrupteur global « pas d'IA » ; c) Gemini comme Screenpresso.
  - **Reco : a)** par défaut, b) si tu le veux.
- **D23 · OCR sous Windows**
  - Contexte : Windows.Media.Ocr via la crate `windows`, plus lourde à compiler. Exige le pack de langue.
  - **Reco : oui**.

### Hors sujet sur Mac, ou plus tard

- **D24 · Ce qu'on ne fait pas, ou plus tard**
  - Liste : Centre Android (ADB), extension Chrome, stratégies d'entreprise et MDM, mises à jour automatiques (Sparkle), 22 langues, licence PRO, HDR/JXR.
  - **Reco** :
    - rien de tout ça pour l'instant ;
    - Android via ADB est facile (S) si tu t'en sers ;
    - interface en français seulement.
- **D26 · Format des préréglages et des réglages**
  - Contexte : le bilan suggère TOML.
  - **Reco** : garder le JSON actuel (`config.json`, `editor.json`, `workspace.json`), compatible avec les anciennes versions.

## 6. Découpage en lots

**Fait** : socle commun.
- 13 actions avec raccourcis ;
- points d'entrée `workspace`, `vision`, `video`, `scroll`, `capture_modes` ;
- contrat `overlay::pick`.

**En cours, en parallèle.** Chaque lot travaille dans sa copie du dépôt et sur ses propres fichiers.

| Lot | Contenu | Fichiers |
|---|---|---|
| A | Icône (clic gauche, clic droit, menu) + fenêtre Espace de travail | agent/tray.rs, agent/mod.rs, workspace/, files.rs |
| B | Réglages (onglets), « Ensuite », presse-papiers, formats, noms, démarrage, filigrane, réglages vidéo | config.rs, settings/, agent/hotkeys.rs, history.rs, main.rs |
| C | Surcouche : `pick`, F, pipette O/D, tous les écrans, fenêtre active, contexte, réticules | overlay.rs, capture/, platform.rs, capture_modes.rs |
| D | Capture avec défilement | scroll/ |
| E | Éditeur : annotations (opacité, pointillés, texte, numéros, polygone, accolades, tampons, multisélection, préréglages, barre « Valider »…) | model.rs, render.rs, editor/ |
| F | Éditeur : onglet Image (recadrer, couper une bande, redimensionner, canevas, bordures, changer l'image) | effects.rs, editor/image_tab.rs |
| G | Vision : OCR, QR, détourage, données sensibles | vision/ |
| H | Vidéo macOS : enregistrement, barre, lecteur, export GIF | video/ |
| I | Partage macOS, impression, générateur PDF/HTML/GIF | share/ |

**Ensuite, selon tes réponses :**
1. Branchements entre lots : l'OCR et le floutage dans l'éditeur, « Publier » dans l'espace de travail, l'arrêt vidéo par raccourci.
2. Migration ScreenCaptureKit (D7, D11).
3. Vidéo 2 : webcam, Windows, sous-titres (D17, D19).
4. Services en ligne (D20).
5. IA (D22).
6. Sous-éléments d'interface (D10).

## 7. Ce que je n'ai pas pu vérifier

- La capture Windows sur un vrai PC (jamais essayée ; seulement compilée et lancée sous Wine), et le multi-écrans réel.
- Les écrans d'échelles différentes, les Spaces et les applications en plein écran.
- Les permissions TCC du .app après chaque build (signature ad hoc), le micro et l'Accessibilité.
- Le comportement de Screenpresso pour la capture défilante, la webcam et les sous-titres (non observés, § 13.6).


## 8. Avancement (fin des lots du 24/11/2026)

Tout ce qui suit est **fusionné et compilé** (macOS + Windows), couvert par des tests unitaires quand c'est de la logique pure. **Rien n'a encore été essayé à la souris** : l'écran du Mac était verrouillé ou sans autorisation pendant les essais des agents. C'est la première chose à vérifier (§ 9).

| Lot | Statut | Ce qui est fait | Limites connues |
|---|---|---|---|
| Socle | fait | 13 actions avec raccourci (aucun raccourci par défaut pour les nouvelles), `--capture <action>` | – |
| A · Icône + Espace de travail | fait | Clic gauche → fenêtre (second clic la ferme), clic droit → menu dans l'ordre Screenpresso avec icônes de couleur et raccourcis ; fenêtre 893 × 591 : vignettes annotées en cache, recherche, filtres type/date, tri, taille des vignettes, espaces de travail (sous-dossiers), menu contextuel complet, Corbeille récupérable, renommer, déplacer, fusionner, enregistrer sous (6 formats), aperçu rapide, glisser vers le Finder/Mail, dépôt de fichiers, toujours devant, fermeture auto, **Publier** (partage, courriel, impression, document) | Windows : pas de glisser sortant ; vignettes vidéo seulement sur Mac |
| B · Réglages + après capture | fait | Onglets Général / Capture / Vidéo / Raccourcis / Avancé ; « Ensuite » (Snappy par défaut, espace de travail, éditeur, notification, rien) ; presse-papiers (image, chemin, les deux, rien) ; format (PNG par défaut, JPEG, BMP, TIFF, GIF, WebP) ; nom (date, numéro, modèle avec jetons) ; démarrer avec la session ; filigrane ; réglages vidéo ; export/import/réinitialisation | Démarrage avec la session : seulement depuis Snappy.app |
| C · Capture | fait | `pick` pour OCR/QR/vidéo/défilement, F écran entier, O pipette, D hexadécimal/décimal, tous les écrans, fenêtre active, pipette seule, contexte (appli, titre) si activé, réticules (option) | Fenêtre active = découpe de l'écran figé (pas de coins transparents) |
| D · Défilement | fait | Séance en direct ~15 i/s, assemblage vertical et horizontal robuste aux en-têtes/pieds fixes, aller-retour, aperçu, Terminé/Recommencer/Annuler, défilement automatique en option | Pas de 2D (cartes) ; pas de cadre affiché autour de la zone |
| E · Éditeur, annotations | fait | Opacité, pointillés, extérieur assombri/flou, texte gras/italique/souligné/aligné/halo, 3 bulles, pastilles 1/A/i + liens + renumérotation, polygone (G), accolade (J), tampons Souris/Texte/Mes tampons, images insérées (glisser, coller, menu), multisélection + aligner/répartir, couper/copier/coller d'annotations (⌘X/⌘V), préréglages par outil (en plus des favoris), barre de titre (‹ ›, Valider, Propriétés, Ouvrir, Supprimer l'image, plein écran), curseur de zoom | Voir D27, D29, D30 |
| F · Éditeur, image | fait | Recadrer, pivoter (⌘R), miroirs, couper une bande (6 modes), redimensionner (5 modes, DPI), zone de travail fixe/auto, bordure et effets avec aperçu (bord, coins, déchiré, N&B, reflet, ombre, rotation), changer l'image principale — tout non destructif | Effets visibles à l'export, pas sur le canevas |
| G · Vision | fait | OCR (Vision / Windows.Media.Ocr), QR + codes-barres, détourage (macOS 14+), données sensibles (courriels, cartes avec Luhn, IBAN, IP, clés d'API, téléphones) ; branchés dans l'éditeur (menu « … ») | Windows : pas de détourage, QR seulement |
| H · Vidéo | fait (macOS 15+) | Zone → MP4 H.264 (ScreenCaptureKit), barre (micro, son système, tailles, pause, refaire, chrono), compte à rebours, arrêt auto, clics mis en évidence, lecteur (découpe, vitesse, muet, image courante, GIF) ; le raccourci vidéo arrête l'enregistrement en cours | **Jamais essayé en vrai** (écran verrouillé) ; pas de webcam, sous-titres, halo du curseur ; rien sous Windows |
| I · Partage | fait | Feuille de partage macOS, courriel avec pièce jointe, impression, générateur PDF/HTML/GIF | Windows : partage = afficher dans l'Explorateur |
| Branchements | fait | Publier (espace de travail), OCR/flou/détourage/partage (éditeur), Ensuite → espace de travail via l'agent, réglages capture/contexte/vidéo reliés, un seul aplatissement des captures pour tout | – |

**Ce qui a été touché dans l'existant** (en plus des ajouts) :
- le clic gauche de l'icône et l'ordre du menu (§ 11.6, demandé) ;
- la fenêtre des réglages réorganisée en onglets (toutes les sections d'origine gardées, elle s'ouvre toujours sur Raccourcis) ;
- deux gestes de l'outil Sélection de l'éditeur (voir **D27**) ;
- `Alt+⌫` supprimait déjà l'annotation : inchangé.

**À savoir** : un test du lot A a mis deux fichiers d'essai vides dans ta Corbeille (`snappy-essai-corbeille.png` et `.json`). Tu peux les supprimer.

## 9. Nouvelles décisions (remontées par les lots)

- **D27 · Éditeur, outil Sélection : deux gestes ont changé de sens.** Maj+clic ajoute/retire de la sélection (avant : sélection simple) et glisser dans le vide trace un rectangle de sélection (avant : désélectionnait). Options : a) garder (comme la plupart des éditeurs) ; b) revenir à l'ancien comportement et mettre la multisélection derrière ⌘+clic. **Reco : a)**, mais c'est un changement de l'existant : à toi de dire.
- **D28 · Icônes manquantes** (polygone, accolade, souris, vidéo, QR, OCR, défilement…) : il faut régénérer la police Lucide (télécharger `lucide-static` sur npm + fontTools). En attendant, icônes voisines ou texte. **Reco : oui**, une seule régénération.
- **D29 · ⌘+ / ⌘− pour la taille du texte** : egui s'en sert pour zoomer toute l'interface ; boutons A− / A+ à la place. **Reco : garder.**
- **D30 · Plein écran de l'éditeur** : F11 est pris par macOS (« Afficher le bureau »). Ajouter ⌃⌘F ? **Reco : oui.**
- **D31 · ⌘Z et les retouches d'image** (recadrage, rotation…) : aujourd'hui « Annuler la dernière retouche » à part. **Reco : les mettre dans ⌘Z.**
- **D32 · « Changer l'image principale »** garde le chemin du fichier choisi. **Reco : copier le fichier dans `<capture>.assets/`.**
- **D33 · Espace de travail : tri** du plus ancien au plus récent (Screenpresso) ou le plus récent en haut ? Et fermeture de la fenêtre quand on ouvre l'éditeur (Screenpresso) ? **Reco : récent en haut ; garder la fermeture.**
- **D34 · Vidéo : le raccourci** démarre/arrête (Screenpresso : pause/reprise). Mémoriser les cases Micro / Son système de la barre ? **Reco : garder démarrer/arrêter ; mémoriser.**
- **D35 · Vidéo sous macOS 14** : pas de vidéo (SCRecordingOutput exige 15). Voir D18.
- **D36 · `snappy --export x.jpg`** écrit du PNG : choisir le format d'après l'extension ? **Reco : oui.**
- **D37 · Notification « Capture enregistrée »** passe par osascript et s'affiche au nom de « Éditeur de script ». Faire une vraie notification (UserNotifications, demande d'autorisation) ? **Reco : oui, plus tard.**
- **D38 · Cadre en pointillés** autour de la zone pendant la capture défilante (comme la vidéo). **Reco : oui, petit lot.**
- **D39 · Données sensibles** : inclure les empreintes hexadécimales (hash de commit) ? **Reco : non.**
- **D40 · Windows** : glisser-déposer sortant (OLE), feuille de partage, courriel MAPI, vidéo (`windows-capture`), détourage. **Reco : vidéo d'abord (D17), le reste si besoin.**
- **D41 · « Afficher l'espace de travail après une capture »** (§ 11.6) : c'est le choix « Ensuite → Ouvrir l'espace de travail » de l'onglet Capture, désactivé par défaut. Te convient-il sous cette forme ?

## 10. Changements demandés après essai (24/11/2026, soir)

- **Éditeur : enregistrement à la demande** (demande du propriétaire, remplace l'enregistrement automatique). Les annotations ne sont plus écrites dans la capture à chaque modification. « Valider » enregistre ; à la fermeture (ou avant d'ouvrir une autre image) Snappy demande : **Enregistrer** (sur la capture), **Enregistrer une copie** (`<nom>-copie.png` à côté, l'originale ne change pas), **Ne pas enregistrer**, **Annuler**. La barre de titre indique « modifié, non enregistré ». Partager / Imprimer / Document publient l'état affiché sans toucher à la capture.
- **Fenêtres dans le Dock et ⌘Tab** tant qu'elles sont ouvertes : éditeur, réglages, lecteur vidéo, générateur de documents, résultat OCR/QR. L'agent reste dans la barre des menus ; l'espace de travail reste un panneau sous l'icône.
- **Flèches, pastilles, bulles** : on appuie sur ce qu'on montre et on glisse vers le départ.
- **Snappy rouvert depuis le Finder** alors qu'il tourne : l'espace de travail s'affiche.
- **Aperçus rendus** (préréglages, favoris, variantes) et **panneau de tampons** (Classique en couleur, Réactions, Souris, Texte, Mes tampons).
- Note : l'icône de la barre des menus était masquée par **Ice** (gestionnaire de barre des menus) : la déplacer dans sa section visible.
