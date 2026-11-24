#!/usr/bin/env python3
"""Réduit la police d'icônes Lucide aux glyphes utilisés par Snappy.

Source : paquet npm officiel `lucide-static` (dossier `font/`), version 1.77.0 :
  https://registry.npmjs.org/lucide-static/-/lucide-static-1.77.0.tgz
Licence : ISC, et MIT pour les icônes issues de Feather (voir
assets/fonts/LICENSE-Lucide.txt, copie du fichier LICENSE du paquet).

Sortie : assets/fonts/Lucide.ttf. Les noms et les points de code doivent
correspondre aux constantes de src/ui/icons.rs : le script les affiche pour comparer.

Usage (fontTools installé, `pip install fonttools`) :
  tar -xzf lucide-static-1.77.0.tgz
  python3 tools/subset_icons.py package/font

Équivaut à `pyftsubset lucide.ttf --unicodes=<liste> --layout-features= --no-hinting
--notdef-outline --name-IDs='*'`, mais en retirant d'abord la table GSUB de Lucide, que
pyftsubset ne sait pas relire (ligatures).
"""

import json
import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont

# Constante de src/ui/icons.rs -> nom de l'icône Lucide.
ICONS = {
    "SELECT": "mouse-pointer-2",
    "ARROW": "move-up-right",
    "LINE": "minus",
    "RECT": "square",
    "ELLIPSE": "circle",
    "TEXT": "type",
    "PEN": "pen-line",
    "HIGHLIGHT": "highlighter",
    "PIXELATE": "grid-3x3",
    "STEP": "hash",
    "CROP": "crop",
    "COPY": "copy",
    "SAVE": "save",
    "UNDO": "undo-2",
    "REDO": "redo-2",
    "DELETE": "trash-2",
    "CLOSE": "x",
    "OK": "check",
    "FOLDER": "folder-open",
    "EDIT": "pencil",
    "REGION": "scan",
    "SCREEN": "monitor",
    "SETTINGS": "settings",
    "PALETTE": "palette",
    "ZOOM_IN": "zoom-in",
    "ZOOM_OUT": "zoom-out",
    "FIT": "maximize-2",
    "IMAGE": "image",
    "WARNING": "triangle-alert",
    "OPEN": "external-link",
    "PIN": "pin",
    # Lot 2 de l'éditeur.
    "ARROW_DOUBLE": "move-diagonal",
    "CURVE": "spline",
    "CONNECTOR": "git-commit-horizontal",
    "ARROW_BIG": "arrow-big-right",
    "SPOTLIGHT": "focus",
    "BUBBLE": "message-square",
    "MAGNIFIER": "search",
    "STAMP": "stamp",
    "BLUR": "droplet",
    "PIPETTE": "pipette",
    "FILL": "paint-bucket",
    "ROUNDED": "squircle",
    "SHADOW": "layers-2",
    "DUPLICATE": "copy-plus",
    "BRING_FRONT": "bring-to-front",
    "SEND_BACK": "send-to-back",
    "STAR": "star",
    "ADD": "plus",
    # Tampons : dessinés en grand dans l'image (voir render.rs).
    "STAMP_CHECK": "circle-check",
    "STAMP_CROSS": "circle-x",
    "STAMP_INFO": "info",
    "STAMP_QUESTION": "circle-help",
    "STAMP_ALERT": "circle-alert",
    "STAMP_CLICK": "mouse-pointer-click",
    "STAMP_THUMB": "thumbs-up",
    "STAMP_HEART": "heart",
    "STAMP_BAN": "ban",
    "STAMP_FLAG": "flag",
    "STAMP_IDEA": "lightbulb",
}

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "assets" / "fonts" / "Lucide.ttf"
ICONS_RS = ROOT / "src" / "ui" / "icons.rs"


def write_icons_rs(codepoints: dict) -> None:
    """Régénère src/ui/icons.rs à partir de ICONS (même ordre, mêmes points de code)."""
    lines = [
        "//! Icônes de l'interface : glyphes de la police Lucide (ISC/MIT), réduite aux icônes",
        "//! listées ici (`assets/fonts/Lucide.ttf`, régénérée par `tools/subset_icons.py`,",
        "//! qui réécrit aussi ce fichier : ne pas le modifier à la main).",
        "//!",
        "//! Chaque constante est un texte d'un caractère, à mêler au texte normal :",
        '//! `format!("{} Supprimer", icons::DELETE)`.',
        "#![allow(dead_code)]",
        "",
    ]
    for const, name in ICONS.items():
        lines += [f"/// `{name}`", f'pub const {const}: &str = "\\u{{{codepoints[name]:04X}}}";']
    lines += ["", "/// (nom, glyphe) de toutes les icônes, pour les tests.", "pub const ALL: &[(&str, &str)] = &["]
    lines += [f'    ("{const}", {const}),' for const in ICONS]
    lines += ["];", ""]
    ICONS_RS.write_text("\n".join(lines))
    print(f"{len(ICONS)} constantes -> {ICONS_RS}")


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit("usage : python3 tools/subset_icons.py <dossier font/ de lucide-static>")
    font_dir = Path(sys.argv[1])
    codepoints = json.loads((font_dir / "codepoints.json").read_text())
    unicodes = []
    for const, name in ICONS.items():
        cp = codepoints[name]
        unicodes.append(cp)
        print(f"{const:<12} {name:<16} U+{cp:04X}")
    font = TTFont(font_dir / "lucide.ttf")
    # Les ligatures de Lucide (GSUB) sont inutiles ici, et fontTools ne sait pas
    # relire cette table : on retire les tables de mise en forme avant de réduire.
    for tag in ("GSUB", "GPOS", "GDEF"):
        if tag in font:
            del font[tag]
    options = subset.Options()
    options.layout_features = []
    options.hinting = False
    options.notdef_outline = True
    options.name_IDs = ["*"]
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=unicodes)
    subsetter.subset(font)
    # Dans lucide.ttf, l'approche gauche (hmtx) vaut 0 alors que les contours commencent
    # à x = 42 : selon le moteur, l'icône serait décalée vers la gauche. On la recale sur
    # le contour, qui est centré dans sa chasse de 1000.
    glyf = font["glyf"]
    for name in font.getGlyphOrder():
        advance, _ = font["hmtx"][name]
        glyph = glyf[name]
        if glyph.numberOfContours:
            glyph.recalcBounds(glyf)
            font["hmtx"][name] = (advance, glyph.xMin)
    # La police voyage seule dans l'exe : on y inscrit l'avis de licence (ISC/MIT).
    name = font["name"]
    notice = "Copyright (c) Lucide Icons and Contributors (ISC); Feather icons (c) Cole Bemis (MIT)"
    name.setName(notice, 0, 3, 1, 0x409)
    name.setName("ISC License and MIT License, see https://lucide.dev/license", 13, 3, 1, 0x409)
    name.setName("https://lucide.dev/license", 14, 3, 1, 0x409)
    font.save(OUTPUT)
    write_icons_rs(codepoints)
    print(f"{len(unicodes)} glyphes -> {OUTPUT} ({OUTPUT.stat().st_size} octets)")


if __name__ == "__main__":
    main()
