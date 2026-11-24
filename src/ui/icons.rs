//! Icônes de l'interface : glyphes de la police Lucide (ISC/MIT), réduite aux icônes
//! listées ici (`assets/fonts/Lucide.ttf`, régénérée par `tools/subset_icons.py`,
//! qui réécrit aussi ce fichier : ne pas le modifier à la main).
//!
//! Chaque constante est un texte d'un caractère, à mêler au texte normal :
//! `format!("{} Supprimer", icons::DELETE)`.
#![allow(dead_code)]

/// `mouse-pointer-2`
pub const SELECT: &str = "\u{E1C3}";
/// `move-up-right`
pub const ARROW: &str = "\u{E493}";
/// `minus`
pub const LINE: &str = "\u{E11C}";
/// `square`
pub const RECT: &str = "\u{E167}";
/// `circle`
pub const ELLIPSE: &str = "\u{E076}";
/// `type`
pub const TEXT: &str = "\u{E198}";
/// `pen-line`
pub const PEN: &str = "\u{E130}";
/// `highlighter`
pub const HIGHLIGHT: &str = "\u{E0F4}";
/// `grid-3x3`
pub const PIXELATE: &str = "\u{E0E9}";
/// `hash`
pub const STEP: &str = "\u{E0EF}";
/// `crop`
pub const CROP: &str = "\u{E0AB}";
/// `copy`
pub const COPY: &str = "\u{E09E}";
/// `save`
pub const SAVE: &str = "\u{E14D}";
/// `undo-2`
pub const UNDO: &str = "\u{E2A1}";
/// `redo-2`
pub const REDO: &str = "\u{E2A0}";
/// `trash-2`
pub const DELETE: &str = "\u{E18E}";
/// `x`
pub const CLOSE: &str = "\u{E1B2}";
/// `check`
pub const OK: &str = "\u{E06C}";
/// `folder-open`
pub const FOLDER: &str = "\u{E247}";
/// `pencil`
pub const EDIT: &str = "\u{E1F9}";
/// `scan`
pub const REGION: &str = "\u{E257}";
/// `monitor`
pub const SCREEN: &str = "\u{E11D}";
/// `settings`
pub const SETTINGS: &str = "\u{E154}";
/// `palette`
pub const PALETTE: &str = "\u{E1DD}";
/// `zoom-in`
pub const ZOOM_IN: &str = "\u{E1B6}";
/// `zoom-out`
pub const ZOOM_OUT: &str = "\u{E1B7}";
/// `maximize-2`
pub const FIT: &str = "\u{E113}";
/// `image`
pub const IMAGE: &str = "\u{E0F6}";
/// `triangle-alert`
pub const WARNING: &str = "\u{E193}";
/// `external-link`
pub const OPEN: &str = "\u{E0B9}";
/// `pin`
pub const PIN: &str = "\u{E259}";
/// `move-diagonal`
pub const ARROW_DOUBLE: &str = "\u{E1C4}";
/// `spline`
pub const CURVE: &str = "\u{E38B}";
/// `git-commit-horizontal`
pub const CONNECTOR: &str = "\u{E0E3}";
/// `arrow-big-right`
pub const ARROW_BIG: &str = "\u{E1E3}";
/// `focus`
pub const SPOTLIGHT: &str = "\u{E29E}";
/// `message-square`
pub const BUBBLE: &str = "\u{E117}";
/// `search`
pub const MAGNIFIER: &str = "\u{E151}";
/// `stamp`
pub const STAMP: &str = "\u{E3BB}";
/// `droplet`
pub const BLUR: &str = "\u{E0B4}";
/// `pipette`
pub const PIPETTE: &str = "\u{E13B}";
/// `paint-bucket`
pub const FILL: &str = "\u{E2E6}";
/// `squircle`
pub const ROUNDED: &str = "\u{E57A}";
/// `layers-2`
pub const SHADOW: &str = "\u{E52A}";
/// `copy-plus`
pub const DUPLICATE: &str = "\u{E3FD}";
/// `bring-to-front`
pub const BRING_FRONT: &str = "\u{E4EF}";
/// `send-to-back`
pub const SEND_BACK: &str = "\u{E4F3}";
/// `star`
pub const STAR: &str = "\u{E176}";
/// `plus`
pub const ADD: &str = "\u{E13D}";
/// `circle-check`
pub const STAMP_CHECK: &str = "\u{E226}";
/// `circle-x`
pub const STAMP_CROSS: &str = "\u{E084}";
/// `info`
pub const STAMP_INFO: &str = "\u{E0F9}";
/// `circle-help`
pub const STAMP_QUESTION: &str = "\u{E082}";
/// `circle-alert`
pub const STAMP_ALERT: &str = "\u{E077}";
/// `mouse-pointer-click`
pub const STAMP_CLICK: &str = "\u{E120}";
/// `thumbs-up`
pub const STAMP_THUMB: &str = "\u{E18A}";
/// `heart`
pub const STAMP_HEART: &str = "\u{E0F2}";
/// `ban`
pub const STAMP_BAN: &str = "\u{E051}";
/// `flag`
pub const STAMP_FLAG: &str = "\u{E0D1}";
/// `lightbulb`
pub const STAMP_IDEA: &str = "\u{E1C2}";

/// (nom, glyphe) de toutes les icônes, pour les tests.
pub const ALL: &[(&str, &str)] = &[
    ("SELECT", SELECT),
    ("ARROW", ARROW),
    ("LINE", LINE),
    ("RECT", RECT),
    ("ELLIPSE", ELLIPSE),
    ("TEXT", TEXT),
    ("PEN", PEN),
    ("HIGHLIGHT", HIGHLIGHT),
    ("PIXELATE", PIXELATE),
    ("STEP", STEP),
    ("CROP", CROP),
    ("COPY", COPY),
    ("SAVE", SAVE),
    ("UNDO", UNDO),
    ("REDO", REDO),
    ("DELETE", DELETE),
    ("CLOSE", CLOSE),
    ("OK", OK),
    ("FOLDER", FOLDER),
    ("EDIT", EDIT),
    ("REGION", REGION),
    ("SCREEN", SCREEN),
    ("SETTINGS", SETTINGS),
    ("PALETTE", PALETTE),
    ("ZOOM_IN", ZOOM_IN),
    ("ZOOM_OUT", ZOOM_OUT),
    ("FIT", FIT),
    ("IMAGE", IMAGE),
    ("WARNING", WARNING),
    ("OPEN", OPEN),
    ("PIN", PIN),
    ("ARROW_DOUBLE", ARROW_DOUBLE),
    ("CURVE", CURVE),
    ("CONNECTOR", CONNECTOR),
    ("ARROW_BIG", ARROW_BIG),
    ("SPOTLIGHT", SPOTLIGHT),
    ("BUBBLE", BUBBLE),
    ("MAGNIFIER", MAGNIFIER),
    ("STAMP", STAMP),
    ("BLUR", BLUR),
    ("PIPETTE", PIPETTE),
    ("FILL", FILL),
    ("ROUNDED", ROUNDED),
    ("SHADOW", SHADOW),
    ("DUPLICATE", DUPLICATE),
    ("BRING_FRONT", BRING_FRONT),
    ("SEND_BACK", SEND_BACK),
    ("STAR", STAR),
    ("ADD", ADD),
    ("STAMP_CHECK", STAMP_CHECK),
    ("STAMP_CROSS", STAMP_CROSS),
    ("STAMP_INFO", STAMP_INFO),
    ("STAMP_QUESTION", STAMP_QUESTION),
    ("STAMP_ALERT", STAMP_ALERT),
    ("STAMP_CLICK", STAMP_CLICK),
    ("STAMP_THUMB", STAMP_THUMB),
    ("STAMP_HEART", STAMP_HEART),
    ("STAMP_BAN", STAMP_BAN),
    ("STAMP_FLAG", STAMP_FLAG),
    ("STAMP_IDEA", STAMP_IDEA),
];
