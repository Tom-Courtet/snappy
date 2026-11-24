//! Impression sous Windows : verbe « print » de l'Explorateur sur une copie aplatie
//! de la capture (l'application associée aux PNG affiche sa boîte d'impression).

use anyhow::{Result, bail};
use image::RgbaImage;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use super::PrintOptions;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// L'image est écrite dans `%TEMP%\snappy-partage` : l'application d'impression la
/// lit après notre retour, on ne peut donc pas l'effacer tout de suite.
///
/// Orientation, mise à l'échelle et nom du fichier : réglés dans la boîte
/// d'impression de Windows (l'application associée), pas ici.
pub fn print_image(img: &RgbaImage, name: &str, _options: PrintOptions) -> Result<()> {
    let dir = std::env::temp_dir().join("snappy-partage");
    let stem = std::path::Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "capture".into());
    let path = dir.join(format!("{stem}.png"));
    crate::history::write_png(&path, img)?;
    let verb = wide("print");
    let file = wide(&path.to_string_lossy());
    // SAFETY : chaînes UTF-16 terminées par un zéro, vivantes pendant l'appel.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Valeur > 32 : succès (convention de ShellExecute).
    if result as isize <= 32 {
        bail!(
            "impression impossible (code {}) : aucune application n'imprime les PNG ?",
            result as isize
        );
    }
    Ok(())
}
