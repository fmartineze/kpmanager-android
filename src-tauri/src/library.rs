//! Biblioteca de ficheros de clave (solo en Android; los ficheros de clave no se sincronizan).
//!
//! Los ficheros se guardan dentro de `config.kpm`, cifrados con la contraseña maestra. A la interfaz solo llega
//! el nombre, el tamaño y una huella corta; el contenido no sale de Rust.
//!
//! Qué clave usa cada vault:
//! - **Automático** (por defecto): si en Windows el vault usa un fichero de clave, el de la biblioteca con el
//!   mismo nombre; si no hay ninguno con ese nombre, el predeterminado.
//! - **Uno concreto** de la biblioteca, elegido a mano.
//! - **Ninguno**.

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

use crate::error::{AppError, Result};

/// Tamaño máximo de un fichero de clave (los de KeePassXC ocupan unos cientos de bytes).
pub const MAX_KEY_BYTES: usize = 1024 * 1024;
pub const MAX_KEYS: usize = 100;
/// Entre todos: la biblioteca viaja entera en la configuración y en la copia de seguridad.
pub const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;

const NONE: &str = "none";

#[derive(Serialize, Deserialize, Clone)]
pub struct KeyFile {
    /// Primeros 16 caracteres hexadecimales del SHA-256 del contenido: el mismo fichero no se importa dos veces.
    pub id: String,
    pub name: String,
    /// Contenido en base64.
    data: Zeroizing<String>,
    size: usize,
    pub added: u64,
}

impl KeyFile {
    /// Contenido del fichero, para abrir el vault (fase 3).
    #[allow(dead_code)]
    pub fn bytes(&self) -> Result<Zeroizing<Vec<u8>>> {
        base64::engine::general_purpose::STANDARD.decode(self.data.as_bytes()).map(Zeroizing::new).map_err(|_| AppError::BadFormat)
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Library {
    #[serde(default)]
    pub keys: Vec<KeyFile>,
    #[serde(default)]
    pub default_key: Option<String>,
    /// Elección por vault (id del vault → id de clave o `none`). Si no está, automático.
    #[serde(default)]
    pub assign: BTreeMap<String, String>,
}

/// Clave efectiva de un vault, tal como la ve la interfaz.
#[derive(Serialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeyState {
    /// `auto`, `manual` o `none`.
    pub mode: &'static str,
    /// Id y nombre de la clave que se usará.
    pub id: Option<String>,
    pub name: Option<String>,
    /// En automático: `name` (coincide con el nombre de Windows) o `default` (predeterminado).
    pub by: Option<&'static str>,
    /// El vault usa fichero de clave en Windows pero aquí no hay ninguno que usar.
    pub missing: bool,
}

/// Lo que la interfaz ve de cada fichero de clave.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct KeyView {
    pub id: String,
    pub name: String,
    pub size: usize,
    /// Huella corta para distinguir ficheros con el mismo nombre (no permite reconstruir el contenido).
    pub fingerprint: String,
    pub is_default: bool,
    pub added: u64,
    pub uses: usize,
}

impl Library {
    fn get(&self, id: &str) -> Option<&KeyFile> {
        self.keys.iter().find(|k| k.id == id)
    }

    /// Añade un fichero. El primero que se importa pasa a ser el predeterminado.
    pub fn add(&mut self, name: &str, data: &[u8], now: u64) -> Result<String> {
        if data.is_empty() {
            return Err(AppError::msg("keyEmpty"));
        }
        if data.len() > MAX_KEY_BYTES {
            return Err(AppError::msg("docTooLarge"));
        }
        let id: String = Sha256::digest(data).iter().take(8).map(|b| format!("{b:02x}")).collect();
        if let Some(k) = self.get(&id) {
            return Err(AppError::msg_arg("keyExists", k.name.clone()));
        }
        if self.keys.len() >= MAX_KEYS || self.keys.iter().map(|k| k.size).sum::<usize>() + data.len() > MAX_TOTAL_BYTES {
            return Err(AppError::msg("keyLimit"));
        }
        self.keys.push(KeyFile {
            id: id.clone(),
            name: name.to_owned(),
            data: Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(data)),
            size: data.len(),
            added: now,
        });
        self.keys.sort_by_key(|k| k.name.to_lowercase());
        if self.default_key.is_none() {
            self.default_key = Some(id.clone());
        }
        Ok(id)
    }

    /// Quita un fichero. Los vaults que lo tenían elegido a mano vuelven a automático.
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let before = self.keys.len();
        self.keys.retain(|k| k.id != id);
        if self.keys.len() == before {
            return Err(AppError::msg("keyUnknown"));
        }
        if self.default_key.as_deref() == Some(id) {
            self.default_key = None;
        }
        self.assign.retain(|_, v| v != id);
        Ok(())
    }

    pub fn set_default(&mut self, id: Option<&str>) -> Result<()> {
        match id {
            Some(id) if self.get(id).is_none() => Err(AppError::msg("keyUnknown")),
            _ => {
                self.default_key = id.map(str::to_owned);
                Ok(())
            }
        }
    }

    /// `choice`: `auto`, `none` o el id de una clave de la biblioteca.
    pub fn assign(&mut self, vault_id: &str, choice: &str) -> Result<()> {
        match choice {
            "auto" => {
                self.assign.remove(vault_id);
            }
            NONE => {
                self.assign.insert(vault_id.to_owned(), NONE.to_owned());
            }
            id if self.get(id).is_some() => {
                self.assign.insert(vault_id.to_owned(), id.to_owned());
            }
            _ => return Err(AppError::msg("keyUnknown")),
        }
        Ok(())
    }

    /// Clave efectiva de un vault. `windows_key` es el nombre del fichero de clave que usa en Windows.
    pub fn resolve(&self, vault_id: &str, windows_key: Option<&str>) -> KeyState {
        let found = |k: &KeyFile| (Some(k.id.clone()), Some(k.name.clone()));
        match self.assign.get(vault_id).map(String::as_str) {
            Some(NONE) => return KeyState { mode: "none", ..Default::default() },
            Some(id) => {
                if let Some(k) = self.get(id) {
                    let (id, name) = found(k);
                    return KeyState { mode: "manual", id, name, ..Default::default() };
                }
            }
            None => {}
        }
        let Some(wanted) = windows_key else {
            return KeyState { mode: "auto", ..Default::default() };
        };
        let by_name = self.keys.iter().find(|k| k.name.eq_ignore_ascii_case(wanted));
        let (key, by) = match by_name {
            Some(k) => (Some(k), "name"),
            None => (self.default_key.as_deref().and_then(|d| self.get(d)), "default"),
        };
        match key {
            Some(k) => {
                let (id, name) = found(k);
                KeyState { mode: "auto", id, name, by: Some(by), missing: false }
            }
            None => KeyState { mode: "auto", missing: true, ..Default::default() },
        }
    }

    /// `uses`: cuántos vaults usarían cada clave.
    pub fn views(&self, uses: &BTreeMap<String, usize>) -> Vec<KeyView> {
        self.keys
            .iter()
            .map(|k| KeyView {
                id: k.id.clone(),
                name: k.name.clone(),
                size: k.size,
                fingerprint: format!("{} {}", &k.id[..4], &k.id[4..8]).to_uppercase(),
                is_default: self.default_key.as_deref() == Some(&k.id),
                added: k.added,
                uses: uses.get(&k.id).copied().unwrap_or(0),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_resolve_by_name_then_default() {
        let mut lib = Library::default();
        let comun = lib.add("comun.keyx", b"clave-comun", 1).unwrap();
        let otra = lib.add("Otra.KEY", b"clave-otra", 2).unwrap();
        assert_eq!(lib.default_key.as_deref(), Some(comun.as_str()), "la primera es la predeterminada");
        assert!(matches!(lib.add("copia.keyx", b"clave-comun", 3), Err(AppError::Msg { code: "keyExists", .. })));
        assert!(lib.add("vacia", b"", 3).is_err());

        let r = lib.resolve("v1", Some("otra.key"));
        assert_eq!((r.id.as_deref(), r.by), (Some(otra.as_str()), Some("name")), "por nombre, sin distinguir mayúsculas");
        let r = lib.resolve("v1", Some("desconocida.keyx"));
        assert_eq!((r.id.as_deref(), r.by), (Some(comun.as_str()), Some("default")));
        assert_eq!(lib.resolve("v1", None), KeyState { mode: "auto", ..Default::default() }, "sin clave en Windows");

        lib.assign("v1", &otra).unwrap();
        assert_eq!(lib.resolve("v1", None).mode, "manual");
        lib.assign("v2", "none").unwrap();
        assert_eq!(lib.resolve("v2", Some("comun.keyx")).id, None);
        assert!(lib.assign("v3", "no-existe").is_err());

        // Al quitar una clave, quien la tenía elegida vuelve a automático; sin predeterminado, falta.
        lib.remove(&otra).unwrap();
        assert_eq!(lib.resolve("v1", None).mode, "auto");
        lib.remove(&comun).unwrap();
        assert!(lib.resolve("v1", Some("comun.keyx")).missing);
        assert_eq!(lib.default_key, None);
    }

    #[test]
    fn views_do_not_carry_the_content() {
        let mut lib = Library::default();
        lib.add("comun.keyx", b"contenido-secreto", 1).unwrap();
        let json = serde_json::to_string(&lib.views(&BTreeMap::new())).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"contenido-secreto");
        assert!(!json.contains("contenido-secreto") && !json.contains(&b64));
        assert_eq!(lib.keys[0].bytes().unwrap().as_slice(), b"contenido-secreto");
    }
}
