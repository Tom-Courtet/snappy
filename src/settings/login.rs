//! « Lancer Snappy à l'ouverture de session ». Le système n'est touché que quand
//! l'utilisateur coche ou décoche la case.
//!
//! - macOS : `SMAppService.mainApp` (élément d'ouverture de Réglages Système >
//!   Général > Ouverture). Ne fonctionne que depuis `Snappy.app` : un exécutable nu
//!   (`cargo run`) n'a pas de bundle à inscrire.
//! - Windows : valeur `Snappy` de `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

/// État vu par le système.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    On,
    Off,
    /// Inscrit, mais l'utilisateur doit l'autoriser dans les réglages du système
    /// (macOS seulement).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    NeedsApproval,
    /// Impossible ici, avec la raison.
    Unavailable(String),
}

impl State {
    /// Message à afficher sous la case, s'il y en a un.
    pub fn hint(&self) -> Option<String> {
        match self {
            Self::On | Self::Off => None,
            Self::NeedsApproval => Some(
                "À autoriser dans Réglages Système > Général > Ouverture (« Autoriser en \
                 arrière-plan »)."
                    .into(),
            ),
            Self::Unavailable(reason) => Some(reason.clone()),
        }
    }
}

/// État actuel.
pub fn state() -> State {
    imp::state()
}

/// Active ou désactive le lancement ; rend le nouvel état.
pub fn set(enabled: bool) -> anyhow::Result<State> {
    imp::set(enabled)
}

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::{Result, bail};
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    use super::State;

    /// Lancé depuis `Snappy.app` ?
    fn in_bundle() -> bool {
        std::env::current_exe()
            .map(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS/"))
            .unwrap_or(false)
    }

    fn not_bundled() -> State {
        State::Unavailable(
            "Disponible seulement pour Snappy.app (pas pour un exécutable lancé à la main).".into(),
        )
    }

    pub fn state() -> State {
        if !in_bundle() {
            return not_bundled();
        }
        // SAFETY : SMAppService existe depuis macOS 13 (Snappy demande macOS 14).
        let status = unsafe { SMAppService::mainAppService().status() };
        match status {
            SMAppServiceStatus::Enabled => State::On,
            SMAppServiceStatus::RequiresApproval => State::NeedsApproval,
            _ => State::Off,
        }
    }

    pub fn set(enabled: bool) -> Result<State> {
        if !in_bundle() {
            return Ok(not_bundled());
        }
        // SAFETY : voir `state`.
        let service = unsafe { SMAppService::mainAppService() };
        let result = if enabled {
            unsafe { service.registerAndReturnError() }
        } else {
            unsafe { service.unregisterAndReturnError() }
        };
        if let Err(e) = result {
            // Déjà dans l'état voulu : pas une erreur.
            let now = state();
            if (now == State::On) == enabled && now != State::NeedsApproval {
                return Ok(now);
            }
            bail!("{}", e.localizedDescription());
        }
        Ok(state())
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;

    use anyhow::{Context, Result, bail};
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey,
        RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };

    use super::State;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const NAME: &str = "Snappy";

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Clé `Run`, fermée à la fin de `f`.
    fn with_run_key<T>(access: u32, f: impl FnOnce(HKEY) -> T) -> Result<T> {
        let mut key: HKEY = std::ptr::null_mut();
        let path = wide(RUN);
        // SAFETY : chaîne terminée par un zéro, `key` reçoit la clé ouverte.
        let status =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, access, &mut key) };
        if status != ERROR_SUCCESS {
            bail!("clé Run inaccessible (erreur {status})");
        }
        let out = f(key);
        // SAFETY : clé ouverte ci-dessus.
        unsafe { RegCloseKey(key) };
        Ok(out)
    }

    pub fn state() -> State {
        let name = wide(NAME);
        let found = with_run_key(KEY_QUERY_VALUE, |key| {
            // SAFETY : on ne demande que l'existence de la valeur.
            let status = unsafe {
                RegQueryValueExW(
                    key,
                    name.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            status == ERROR_SUCCESS
        });
        match found {
            Ok(true) => State::On,
            Ok(false) => State::Off,
            Err(e) => State::Unavailable(format!("{e:#}")),
        }
    }

    pub fn set(enabled: bool) -> Result<State> {
        let name = wide(NAME);
        if enabled {
            let exe = std::env::current_exe().context("chemin de l'exécutable")?;
            let command = wide(&format!("\"{}\" --background", exe.display()));
            let bytes = u32::try_from(command.len() * 2).context("chemin trop long")?;
            let status = with_run_key(KEY_SET_VALUE, |key| {
                // SAFETY : `command` est une chaîne UTF-16 terminée par un zéro, de
                // `bytes` octets.
                unsafe {
                    RegSetValueExW(
                        key,
                        name.as_ptr(),
                        0,
                        REG_SZ,
                        command.as_ptr().cast(),
                        bytes,
                    )
                }
            })?;
            if status != ERROR_SUCCESS {
                bail!("inscription refusée (erreur {status})");
            }
        } else {
            let status = with_run_key(KEY_SET_VALUE, |key| {
                // SAFETY : nom terminé par un zéro.
                unsafe { RegDeleteValueW(key, name.as_ptr()) }
            })?;
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                bail!("désinscription refusée (erreur {status})");
            }
        }
        Ok(state())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::State;

    pub fn state() -> State {
        State::Unavailable("Pas encore disponible sur ce système.".into())
    }

    pub fn set(_enabled: bool) -> anyhow::Result<State> {
        Ok(state())
    }
}
