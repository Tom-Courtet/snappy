//! Interface commune à la sélection de zone et à l'éditeur : thème, icônes, petits
//! éléments réutilisables.

pub mod icons;
pub mod keys;
pub mod theme;

#[cfg(test)]
mod tests {
    use egui::epaint::text::{Fonts, TextOptions};
    use egui::{FontFamily, FontId};

    /// Fichiers dont les textes s'affichent dans une fenêtre egui.
    const SOURCES: &[(&str, &str)] = &[
        ("overlay.rs", include_str!("../overlay.rs")),
        ("editor/mod.rs", include_str!("../editor/mod.rs")),
        ("model.rs", include_str!("../model.rs")),
        ("ui/icons.rs", include_str!("icons.rs")),
        ("ui/keys.rs", include_str!("keys.rs")),
        ("editor/bars.rs", include_str!("../editor/bars.rs")),
        ("editor/tools.rs", include_str!("../editor/tools.rs")),
        ("editor/prefs.rs", include_str!("../editor/prefs.rs")),
        ("editor/preview.rs", include_str!("../editor/preview.rs")),
        ("editor/extras.rs", include_str!("../editor/extras.rs")),
        ("render.rs", include_str!("../render.rs")),
    ];

    /// Caractères écrits `\u{…}` dans une chaîne (les icônes de [`super::icons`]).
    fn unicode_escapes(part: &str) -> impl Iterator<Item = char> + '_ {
        part.split("\\u{").skip(1).filter_map(|rest| {
            let hex = rest.split('}').next()?;
            char::from_u32(u32::from_str_radix(hex, 16).ok()?)
        })
    }

    /// Aucun symbole ne doit s'afficher comme un carré : chaque caractère non ASCII
    /// des chaînes de l'interface existe dans les polices embarquées.
    #[test]
    fn every_ui_character_has_a_glyph() {
        let mut fonts = Fonts::new(TextOptions::default(), super::theme::fonts());
        let font = FontId::new(14.0, FontFamily::Proportional);
        let mut missing = Vec::new();
        for (file, source) in SOURCES {
            for (line_no, line) in source.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                for (i, part) in code.split('"').enumerate() {
                    if i % 2 == 0 {
                        continue;
                    }
                    for c in part
                        .chars()
                        .chain(unicode_escapes(part))
                        .filter(|c| !c.is_ascii())
                    {
                        if !fonts.has_glyph(&font, c) {
                            missing.push(format!(
                                "{file}:{} {c:?} (U+{:04X})",
                                line_no + 1,
                                c as u32
                            ));
                        }
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "glyphes absents :\n{}",
            missing.join("\n")
        );
    }

    #[test]
    fn every_icon_is_in_the_icon_font() {
        let mut fonts = Fonts::new(TextOptions::default(), super::theme::fonts());
        let font = FontId::new(14.0, FontFamily::Proportional);
        for (name, glyph) in super::icons::ALL {
            let c = glyph.chars().next().unwrap();
            assert!(fonts.has_glyph(&font, c), "icône {name} absente");
        }
    }
}
