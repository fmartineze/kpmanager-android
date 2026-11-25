//! Configuración de este teléfono, en la carpeta privada de la app.
//!
//! - `config.kpm`: cuenta de Microsoft, carpeta de origen, fichero de datos y token de refresco.
//!   Cifrado con la contraseña maestra (mismo contenedor KPM1 que `vaults.enc`): sin la maestra no sirve de nada.
//! - `vaults.enc`: copia local del fichero de datos de OneDrive, tal cual (ya va cifrado), para usar la app
//!   sin conexión.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

use crate::crypto::{DEFAULT_COST, Keys};
use crate::error::{AppError, Result};
use crate::graph::{Account, AppReg, Item};

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub reg: AppReg,
    pub account: Account,
    pub folder: Item,
    pub data_file: Item,
    pub refresh_token: Option<Zeroizing<String>>,
    /// Momento (segundos Unix) de la última descarga correcta del fichero de datos.
    #[serde(default)]
    pub synced_at: Option<u64>,
}

#[derive(Clone)]
pub struct Paths {
    dir: PathBuf,
}

impl Paths {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn config(&self) -> PathBuf {
        self.dir.join("config.kpm")
    }

    fn data(&self) -> PathBuf {
        self.dir.join("vaults.enc")
    }

    pub fn configured(&self) -> bool {
        self.config().is_file() && self.data().is_file()
    }

    /// Descifra la configuración. Devuelve también la clave para volver a guardarla sin pedir la maestra.
    pub fn load(&self, password: &str) -> Result<(Keys, Config)> {
        let blob = std::fs::read(self.config())?;
        let (keys, plain) = Keys::decrypt(password, &blob)?;
        Ok((keys, serde_json::from_slice(&plain)?))
    }

    /// Crea las claves de la configuración (al terminar el asistente).
    pub fn new_keys(password: &str) -> Result<Keys> {
        Keys::new(password, DEFAULT_COST)
    }

    pub fn save(&self, keys: &Keys, config: &Config) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(config)?);
        write_atomic(&self.config(), &keys.encrypt(&plain)?)
    }

    pub fn read_data(&self) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.data())?)
    }

    pub fn write_data(&self, blob: &[u8]) -> Result<()> {
        if !Keys::looks_valid(blob) {
            return Err(AppError::msg("notKpmData"));
        }
        write_atomic(&self.data(), blob)
    }

    /// Borra todo lo de este teléfono (Ajustes → Restablecer).
    pub fn reset(&self) -> Result<()> {
        for p in [self.config(), self.data()] {
            match std::fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}

/// Escribe en un temporal y lo renombra: un corte a mitad no deja el fichero a medias.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config {
        Config {
            version: 1,
            reg: AppReg::validated("contoso.com", "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0").unwrap(),
            account: Account { name: "Ana".into(), upn: "ana@contoso.com".into() },
            folder: Item { id: "F1".into(), name: "Clientes".into(), folder: true, path: "/Clientes".into(), etag: String::new() },
            data_file: Item { id: "D1".into(), name: "vaults.enc".into(), folder: false, path: "/Clientes/vaults.enc".into(), etag: "e1".into() },
            refresh_token: Some(Zeroizing::new("secreto-de-refresco".into())),
            synced_at: None,
        }
    }

    #[test]
    fn config_is_encrypted_and_needs_the_master_password() {
        let dir = std::env::temp_dir().join(format!("kpm-android-test-{}", std::process::id()));
        let paths = Paths::new(dir.clone());
        let keys = Keys::new("maestra", crate::crypto::KdfCost { m_kib: 8, t: 1, p: 1 }).unwrap();
        paths.save(&keys, &sample()).unwrap();

        let raw = std::fs::read(dir.join("config.kpm")).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(!text.contains("secreto-de-refresco") && !text.contains("ana@contoso.com"), "nada en claro");

        assert!(matches!(paths.load("otra"), Err(AppError::WrongPassword)));
        let (_, c) = paths.load("maestra").unwrap();
        assert_eq!(c.refresh_token.as_deref().map(|s| s.as_str()), Some("secreto-de-refresco"));

        assert!(paths.write_data(b"no es un vaults.enc").is_err());
        paths.reset().unwrap();
        assert!(!dir.join("config.kpm").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
