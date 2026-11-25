//! Abrir y guardar documentos con el selector de Android (plugin Kotlin `DocsPlugin`).
//!
//! El contenido va del sistema a Rust sin pasar por la interfaz web: la interfaz no tiene permiso para llamar
//! al plugin (no figura en `capabilities`). Fuera de Android (pruebas en el PC) responde «no disponible».

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{AppHandle, Runtime};
use zeroize::Zeroizing;

use crate::error::{AppError, Result};

/// Documento elegido por el usuario.
pub struct Picked {
    pub name: String,
    pub data: Zeroizing<Vec<u8>>,
}

#[cfg(target_os = "android")]
struct Docs<R: Runtime>(tauri::plugin::PluginHandle<R>);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("kpdocs")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle = _api.register_android_plugin("com.kpmanager.android", "DocsPlugin")?;
                _app.manage(Docs(handle));
            }
            Ok(())
        })
        .build()
}

#[cfg(target_os = "android")]
fn handle<R: Runtime>(app: &AppHandle<R>) -> Result<tauri::plugin::PluginHandle<R>> {
    use tauri::Manager;
    Ok(app.try_state::<Docs<R>>().ok_or_else(|| AppError::msg("unsupported"))?.0.clone())
}

/// Traduce el rechazo del plugin. `None` = el usuario canceló.
#[cfg(target_os = "android")]
fn rejected<T>(e: tauri::plugin::mobile::PluginInvokeError) -> Result<Option<T>> {
    use tauri::plugin::mobile::PluginInvokeError::InvokeRejected;
    match e {
        InvokeRejected(r) => match r.message.as_deref() {
            Some("cancelled") => Ok(None),
            Some("tooLarge") => Err(AppError::msg("docTooLarge")),
            _ => Err(AppError::msg("docFailed")),
        },
        _ => Err(AppError::msg("docFailed")),
    }
}

/// Muestra el selector y lee el documento elegido (como mucho `max_bytes`). Bloquea: llamar fuera del hilo
/// principal.
#[cfg(target_os = "android")]
pub fn open<R: Runtime>(app: &AppHandle<R>, max_bytes: usize) -> Result<Option<Picked>> {
    use base64::Engine;
    #[derive(serde::Deserialize)]
    struct Resp {
        name: String,
        data: Zeroizing<String>,
    }
    let args = serde_json::json!({ "maxBytes": max_bytes });
    let resp: Resp = match handle(app)?.run_mobile_plugin("openDocument", args) {
        Ok(r) => r,
        Err(e) => return rejected(e),
    };
    let data = Zeroizing::new(base64::engine::general_purpose::STANDARD.decode(resp.data.as_bytes()).map_err(|_| AppError::msg("docFailed"))?);
    if data.len() > max_bytes {
        return Err(AppError::msg("docTooLarge"));
    }
    Ok(Some(Picked { name: clean_name(&resp.name), data }))
}

/// Muestra «Guardar como» y escribe `data`. Devuelve el nombre final, o `None` si el usuario canceló.
#[cfg(target_os = "android")]
pub fn save<R: Runtime>(app: &AppHandle<R>, file_name: &str, mime: &str, data: &[u8]) -> Result<Option<String>> {
    use base64::Engine;
    #[derive(serde::Deserialize)]
    struct Resp {
        name: String,
    }
    let args = serde_json::json!({
        "fileName": file_name,
        "mimeType": mime,
        "data": base64::engine::general_purpose::STANDARD.encode(data),
    });
    match handle(app)?.run_mobile_plugin::<Resp>("createDocument", args) {
        Ok(r) => Ok(Some(clean_name(&r.name))),
        Err(e) => rejected(e),
    }
}

/// Copia un texto al portapapeles como contenido sensible; se borra a los `clear_after_ms`.
#[cfg(target_os = "android")]
pub fn copy<R: Runtime>(app: &AppHandle<R>, text: &str, clear_after_ms: u64) -> Result<()> {
    let args = serde_json::json!({ "text": text, "clearAfterMs": clear_after_ms });
    handle(app)?.run_mobile_plugin::<serde_json::Value>("copyText", args).map(|_| ()).map_err(|_| AppError::msg("copyFailed"))
}

/// Borra del portapapeles lo último que copió la app (si nadie copió otra cosa después).
#[cfg(target_os = "android")]
pub fn clear_clipboard<R: Runtime>(app: &AppHandle<R>) {
    if let Ok(h) = handle(app) {
        let _ = h.run_mobile_plugin::<serde_json::Value>("clearClipboard", serde_json::json!({}));
    }
}

#[cfg(not(target_os = "android"))]
pub fn copy<R: Runtime>(_app: &AppHandle<R>, _text: &str, _clear_after_ms: u64) -> Result<()> {
    Err(AppError::msg("unsupported"))
}

#[cfg(not(target_os = "android"))]
pub fn clear_clipboard<R: Runtime>(_app: &AppHandle<R>) {}

#[cfg(not(target_os = "android"))]
pub fn open<R: Runtime>(_app: &AppHandle<R>, _max_bytes: usize) -> Result<Option<Picked>> {
    Err(AppError::msg("unsupported"))
}

#[cfg(not(target_os = "android"))]
pub fn save<R: Runtime>(_app: &AppHandle<R>, _file_name: &str, _mime: &str, _data: &[u8]) -> Result<Option<String>> {
    Err(AppError::msg("unsupported"))
}

/// El nombre lo da el proveedor de documentos: solo se usa para mostrarlo y para asignar claves por nombre.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let s: String = base.chars().filter(|c| !c.is_control()).take(120).collect();
    let s = s.trim();
    if s.is_empty() { "fichero".into() } else { s.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::clean_name;

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("comun.keyx"), "comun.keyx");
        assert_eq!(clean_name("../../x/clave.key"), "clave.key");
        assert_eq!(clean_name("a\u{0}b\nc"), "abc");
        assert_eq!(clean_name("   "), "fichero");
        assert_eq!(clean_name(&"x".repeat(500)).len(), 120);
    }
}
