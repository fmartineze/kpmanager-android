//! Desbloqueo con huella (plugin Kotlin `BioPlugin`).
//!
//! La maestra se cifra con una clave del almacén de claves de Android que exige huella en cada uso; aquí solo
//! se guarda el resultado (`bio.bin`). La maestra va de Rust a Kotlin y vuelve por JNI, nunca por la interfaz.
//! Fuera de Android no hay huella.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{AppHandle, Runtime};
use zeroize::Zeroizing;

use crate::error::{AppError, Result};

#[cfg(target_os = "android")]
struct Bio<R: Runtime>(tauri::plugin::PluginHandle<R>);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("kpbio")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle = _api.register_android_plugin("com.kpmanager.android", "BioPlugin")?;
                _app.manage(Bio(handle));
            }
            Ok(())
        })
        .build()
}

/// Textos del diálogo del sistema (en el idioma de la interfaz).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub struct Prompt {
    pub title: String,
    pub cancel: String,
}

#[cfg(target_os = "android")]
mod imp {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use tauri::Manager;

    fn handle<R: Runtime>(app: &AppHandle<R>) -> Result<tauri::plugin::PluginHandle<R>> {
        Ok(app.try_state::<Bio<R>>().ok_or_else(|| AppError::msg("bioUnavailable"))?.0.clone())
    }

    #[derive(serde::Deserialize)]
    struct Data {
        data: Zeroizing<String>,
    }

    /// `None` = el usuario canceló.
    fn run<R: Runtime>(app: &AppHandle<R>, cmd: &str, data: &str, p: &Prompt) -> Result<Option<Zeroizing<String>>> {
        use tauri::plugin::mobile::PluginInvokeError::InvokeRejected;
        let args = serde_json::json!({ "data": data, "title": p.title, "cancel": p.cancel });
        match handle(app)?.run_mobile_plugin::<Data>(cmd, args) {
            Ok(r) => Ok(Some(r.data)),
            Err(InvokeRejected(r)) => match r.message.as_deref() {
                Some("cancelled") => Ok(None),
                Some("invalidated") => Err(AppError::msg("bioInvalidated")),
                Some("lockout") => Err(AppError::msg("bioLockout")),
                Some("unavailable") => Err(AppError::msg("bioUnavailable")),
                _ => Err(AppError::msg("bioFailed")),
            },
            Err(_) => Err(AppError::msg("bioFailed")),
        }
    }

    pub fn available<R: Runtime>(app: &AppHandle<R>) -> bool {
        #[derive(serde::Deserialize)]
        struct S {
            available: bool,
        }
        handle(app)
            .and_then(|h| h.run_mobile_plugin::<S>("status", serde_json::json!({})).map_err(|_| AppError::msg("bioFailed")))
            .map(|s| s.available)
            .unwrap_or(false)
    }

    pub fn enroll<R: Runtime>(app: &AppHandle<R>, secret: &str, p: &Prompt) -> Result<Option<Vec<u8>>> {
        let b64 = Zeroizing::new(STANDARD.encode(secret.as_bytes()));
        match run(app, "enroll", &b64, p)? {
            Some(sealed) => Ok(Some(STANDARD.decode(sealed.as_bytes()).map_err(|_| AppError::msg("bioFailed"))?)),
            None => Ok(None),
        }
    }

    pub fn decrypt<R: Runtime>(app: &AppHandle<R>, sealed: &[u8], p: &Prompt) -> Result<Option<Zeroizing<String>>> {
        let Some(plain) = run(app, "decrypt", &STANDARD.encode(sealed), p)? else {
            return Ok(None);
        };
        let bytes = Zeroizing::new(STANDARD.decode(plain.as_bytes()).map_err(|_| AppError::msg("bioFailed"))?);
        let text = std::str::from_utf8(&bytes).map_err(|_| AppError::msg("bioFailed"))?;
        Ok(Some(Zeroizing::new(text.to_owned())))
    }

    pub fn remove<R: Runtime>(app: &AppHandle<R>) {
        if let Ok(h) = handle(app) {
            let _ = h.run_mobile_plugin::<serde_json::Value>("remove", serde_json::json!({}));
        }
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    use super::*;

    pub fn available<R: Runtime>(_app: &AppHandle<R>) -> bool {
        false
    }

    pub fn enroll<R: Runtime>(_app: &AppHandle<R>, _secret: &str, _p: &Prompt) -> Result<Option<Vec<u8>>> {
        Err(AppError::msg("bioUnavailable"))
    }

    pub fn decrypt<R: Runtime>(_app: &AppHandle<R>, _sealed: &[u8], _p: &Prompt) -> Result<Option<Zeroizing<String>>> {
        Err(AppError::msg("bioUnavailable"))
    }

    pub fn remove<R: Runtime>(_app: &AppHandle<R>) {}
}

pub use imp::{available, decrypt, enroll, remove};
