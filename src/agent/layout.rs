//! Caractère gravé sur une touche, selon la disposition du clavier (macOS).
//!
//! Sous macOS, un raccourci global désigne une **position** de touche, nommée comme
//! sur un clavier américain : sur un clavier AZERTY, `KeyQ` est la touche marquée A.
//! Pour afficher ⌘A et non ⌘Q, on demande au système le caractère de cette position
//! dans la disposition active. Sous Windows, le raccourci désigne déjà la touche
//! gravée : rien à traduire.

use global_hotkey::hotkey::Code;

/// Caractère de la touche `code` dans la disposition active, en majuscule. `None`
/// hors macOS, hors thread principal (exigence du système), pour les touches sans
/// caractère, ou si la disposition est illisible.
pub fn engraved(code: Code) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos::engraved(code)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = code;
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;

    use global_hotkey::hotkey::Code;
    use objc2::MainThreadMarker;

    type CFTypeRef = *const c_void;

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        static kTISPropertyUnicodeKeyLayoutData: CFTypeRef;
        fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
        fn TISGetInputSourceProperty(source: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn LMGetKbdType() -> u8;
        #[allow(clippy::too_many_arguments)]
        fn UCKeyTranslate(
            layout: *const c_void,
            virtual_key_code: u16,
            key_action: u16,
            modifier_key_state: u32,
            keyboard_type: u32,
            options: u32,
            dead_key_state: *mut u32,
            max_length: usize,
            actual_length: *mut usize,
            chars: *mut u16,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
        fn CFRelease(cf: CFTypeRef);
    }

    /// `kUCKeyActionDisplay` : le caractère affiché sur la touche.
    const KEY_ACTION_DISPLAY: u16 = 3;
    /// `kUCKeyTranslateNoDeadKeysMask` : ^ et ¨ rendus tels quels.
    const NO_DEAD_KEYS: u32 = 1;

    /// Code de touche virtuel (`kVK_ANSI_*`) des touches qui portent un caractère
    /// variable selon la disposition. Les chiffres restent des chiffres.
    fn virtual_key(code: Code) -> Option<u16> {
        use Code as C;
        Some(match code {
            C::KeyA => 0x00,
            C::KeyS => 0x01,
            C::KeyD => 0x02,
            C::KeyF => 0x03,
            C::KeyH => 0x04,
            C::KeyG => 0x05,
            C::KeyZ => 0x06,
            C::KeyX => 0x07,
            C::KeyC => 0x08,
            C::KeyV => 0x09,
            C::IntlBackslash => 0x0A,
            C::KeyB => 0x0B,
            C::KeyQ => 0x0C,
            C::KeyW => 0x0D,
            C::KeyE => 0x0E,
            C::KeyR => 0x0F,
            C::KeyY => 0x10,
            C::KeyT => 0x11,
            C::Equal => 0x18,
            C::Minus => 0x1B,
            C::BracketRight => 0x1E,
            C::KeyO => 0x1F,
            C::KeyU => 0x20,
            C::BracketLeft => 0x21,
            C::KeyI => 0x22,
            C::KeyP => 0x23,
            C::KeyL => 0x25,
            C::KeyJ => 0x26,
            C::Quote => 0x27,
            C::KeyK => 0x28,
            C::Semicolon => 0x29,
            C::Backslash => 0x2A,
            C::Comma => 0x2B,
            C::Slash => 0x2C,
            C::KeyN => 0x2D,
            C::KeyM => 0x2E,
            C::Period => 0x2F,
            C::Backquote => 0x32,
            _ => return None,
        })
    }

    pub fn engraved(code: Code) -> Option<String> {
        MainThreadMarker::new()?;
        let key = virtual_key(code)?;
        // SAFETY : fonctions de Carbon appelées sur le thread principal ; la source
        // copiée est relâchée après usage, les données de disposition lui
        // appartiennent et ne sont lues qu'avant ce relâchement ; tampon local.
        let chars = unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if source.is_null() {
                return None;
            }
            let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
            let mut chars = [0u16; 4];
            let mut len = 0usize;
            let mut dead = 0u32;
            let status = if data.is_null() {
                -1
            } else {
                UCKeyTranslate(
                    CFDataGetBytePtr(data).cast(),
                    key,
                    KEY_ACTION_DISPLAY,
                    0,
                    u32::from(LMGetKbdType()),
                    NO_DEAD_KEYS,
                    &mut dead,
                    chars.len(),
                    &mut len,
                    chars.as_mut_ptr(),
                )
            };
            CFRelease(source);
            if status != 0 {
                return None;
            }
            String::from_utf16(&chars[..len.min(chars.len())]).ok()?
        };
        let mut it = chars.chars();
        match (it.next(), it.next()) {
            (Some(c), None) if !c.is_control() && !c.is_whitespace() => {
                Some(c.to_uppercase().collect())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Les tests ne tournent pas sur le thread principal : pas de traduction, le
    /// libellé reste celui du clavier américain.
    #[test]
    fn no_translation_off_the_main_thread() {
        assert_eq!(engraved(Code::KeyA), None);
        assert_eq!(engraved(Code::Digit2), None);
        assert_eq!(engraved(Code::F13), None);
    }
}
