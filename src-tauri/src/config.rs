//! Configuración de este teléfono, en la carpeta privada de la app.
//!
//! - `config.kpm`: cuenta de Microsoft, carpeta de origen, fichero de datos y token de refresco.
//!   Cifrado con la contraseña maestra (mismo contenedor KPM1 que `vaults.enc`): sin la maestra no sirve de nada.
//! - `vaults.enc`: copia local del fichero de datos de OneDrive, tal cual (ya va cifrado), para usar la app
//!   sin conexión.
//! - `bio.bin`: la maestra cifrada con una clave del almacén de Android que exige huella (si se activó).
//! - `kdbx/<id>.kdbx`: copia de los vaults ya abiertos en el móvil, tal cual (los cifra KeePass), para abrirlos
//!   sin conexión. Solo se descargan de nuevo si cambiaron en OneDrive. Se borran solas si no se abren en
//!   `cache_days` días o si el vault ya no está en el fichero de datos, y se pueden borrar desde Ajustes.
//!
//! La copia de seguridad (`.kpmbak`) lleva lo mismo que `config.kpm` salvo el token de Microsoft, cifrada con
//! la contraseña maestra: sirve para configurar otro móvil sin repetir el asistente.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

use crate::crypto::{DEFAULT_COST, Keys};
use crate::error::{AppError, Result};
use crate::graph::{Account, AppReg, Item};
use crate::library::Library;

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
    /// Biblioteca de ficheros de clave de este móvil.
    #[serde(default)]
    pub library: Library,
    /// Dónde está en OneDrive el `.kdbx` de cada vault (id del vault → elemento, con el eTag de la copia local).
    #[serde(default)]
    pub vault_files: BTreeMap<String, Item>,
    /// Última vez (segundos Unix) que se abrió cada vault en el móvil, para la caducidad de las copias.
    #[serde(default)]
    pub vault_used: BTreeMap<String, u64>,
    /// Días sin abrir tras los que se borra la copia local de un vault (0 = nunca).
    #[serde(default = "default_cache_days")]
    pub cache_days: u32,
}

fn default_cache_days() -> u32 {
    DEFAULT_CACHE_DAYS
}

pub const DEFAULT_CACHE_DAYS: u32 = 30;

impl Config {
    /// Limpia las copias locales de vaults. Borra:
    /// - las de vaults que ya no están en el fichero de datos (`live`), y olvida su ubicación;
    /// - las que llevan más de `cache_days` sin abrirse (se conserva la ubicación, para descargarla rápido);
    /// - los ficheros sueltos que no corresponden a ningún vault.
    ///
    /// Las copias sin fecha (de versiones anteriores) empiezan a contar ahora. Devuelve si cambió la configuración.
    pub fn prune_cache(&mut self, live: &std::collections::HashSet<&str>, paths: &Paths, now: u64) -> bool {
        let mut changed = false;
        let gone: Vec<String> = self.vault_files.keys().filter(|k| !live.contains(k.as_str())).cloned().collect();
        for id in gone {
            if let Some(item) = self.vault_files.remove(&id) {
                paths.remove_kdbx(&item.id);
            }
            changed = true;
        }
        let before = self.vault_used.len();
        self.vault_used.retain(|k, _| live.contains(k.as_str()));
        changed |= self.vault_used.len() != before;
        for id in self.vault_files.keys() {
            if !self.vault_used.contains_key(id) {
                self.vault_used.insert(id.clone(), now);
                changed = true;
            }
        }
        if self.cache_days > 0 {
            let limit = u64::from(self.cache_days) * 86_400;
            for (vid, item) in &self.vault_files {
                if now.saturating_sub(self.vault_used.get(vid).copied().unwrap_or(now)) > limit {
                    paths.remove_kdbx(&item.id);
                }
            }
        }
        let keep: std::collections::HashSet<&str> = self.vault_files.values().map(|i| i.id.as_str()).collect();
        for (id, _) in paths.list_kdbx() {
            if !keep.contains(id.as_str()) {
                paths.remove_kdbx(&id);
            }
        }
        changed
    }
}

const BACKUP_KIND: &str = "kpmanager-android-backup";
const BACKUP_VERSION: u32 = 1;
/// La biblioteca está limitada a 16 MB; con el cifrado y el base64, una copia nunca llega a esto.
pub const MAX_BACKUP_BYTES: usize = 32 * 1024 * 1024;

/// Contenido de la copia de seguridad. Sin el token de refresco: en el móvil nuevo se vuelve a iniciar sesión.
#[derive(Serialize, Deserialize)]
pub struct Backup {
    pub kind: String,
    pub version: u32,
    pub created: u64,
    pub reg: AppReg,
    pub account: Account,
    pub folder: Item,
    pub data_file: Item,
    #[serde(default)]
    pub library: Library,
}

impl Config {
    /// Copia de seguridad cifrada con la misma clave que la configuración (es decir, con la maestra).
    pub fn backup(&self, keys: &Keys) -> Result<Vec<u8>> {
        let b = Backup {
            kind: BACKUP_KIND.into(),
            version: BACKUP_VERSION,
            created: now(),
            reg: self.reg.clone(),
            account: self.account.clone(),
            folder: self.folder.clone(),
            data_file: self.data_file.clone(),
            library: self.library.clone(),
        };
        let plain = Zeroizing::new(serde_json::to_vec(&b)?);
        keys.encrypt(&plain)
    }
}

impl Backup {
    /// Abre una copia con la maestra. Un fichero de datos u otro `.kpm` cifrado con la misma maestra se rechaza.
    pub fn open(password: &str, blob: &[u8]) -> Result<Self> {
        if !Keys::looks_valid(blob) {
            return Err(AppError::msg("notBackup"));
        }
        let (_, plain) = Keys::decrypt(password, blob)?;
        let v: serde_json::Value = serde_json::from_slice(&plain).map_err(|_| AppError::msg("notBackup"))?;
        if v.get("kind").and_then(|k| k.as_str()) != Some(BACKUP_KIND) {
            return Err(AppError::msg("notBackup"));
        }
        if v.get("version").and_then(|k| k.as_u64()).unwrap_or(0) > BACKUP_VERSION as u64 {
            return Err(AppError::msg("backupNewer"));
        }
        let mut b: Backup = serde_json::from_value(v).map_err(|_| AppError::msg("notBackup"))?;
        // Se revalida como si lo hubiera escrito el usuario.
        b.reg = AppReg::validated(&b.reg.tenant, &b.reg.client_id)?;
        Ok(b)
    }
}

/// Nombre propuesto para la copia: `kpmanager-android-AAAA-MM-DD.kpmbak`.
pub fn backup_name(secs: u64) -> String {
    // Días desde 1970 → fecha civil (algoritmo de Howard Hinnant).
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("kpmanager-android-{y:04}-{m:02}-{d:02}.kpmbak")
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

    fn kdbx_dir(&self) -> PathBuf {
        self.dir.join("kdbx")
    }

    /// Ruta de la copia local de un `.kdbx`. El id es de OneDrive: solo letras, cifras y `!`.
    fn kdbx(&self, item_id: &str) -> Result<PathBuf> {
        if item_id.is_empty() || item_id.len() > 128 || !item_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '!') {
            return Err(AppError::msg("itemInvalid"));
        }
        Ok(self.kdbx_dir().join(format!("{item_id}.kdbx")))
    }

    pub fn read_kdbx(&self, item_id: &str) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.kdbx(item_id)?)?)
    }

    pub fn write_kdbx(&self, item_id: &str, bytes: &[u8]) -> Result<()> {
        write_atomic(&self.kdbx(item_id)?, bytes)
    }

    pub fn remove_kdbx(&self, item_id: &str) {
        if let Ok(p) = self.kdbx(item_id) {
            let _ = std::fs::remove_file(p);
        }
    }

    /// Copias locales de vaults: (id de OneDrive, bytes).
    pub fn list_kdbx(&self) -> Vec<(String, u64)> {
        let Ok(dir) = std::fs::read_dir(self.kdbx_dir()) else { return Vec::new() };
        dir.filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let id = name.strip_suffix(".kdbx")?.to_owned();
                Some((id, e.metadata().ok()?.len()))
            })
            .collect()
    }

    /// Borra todas las copias locales de vaults (Ajustes → Eliminar vaults guardados).
    pub fn clear_kdbx(&self) -> Result<()> {
        match std::fs::remove_dir_all(self.kdbx_dir()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn bio(&self) -> PathBuf {
        self.dir.join("bio.bin")
    }

    pub fn bio_enabled(&self) -> bool {
        self.bio().is_file()
    }

    pub fn read_bio(&self) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.bio())?)
    }

    pub fn write_bio(&self, sealed: &[u8]) -> Result<()> {
        write_atomic(&self.bio(), sealed)
    }

    pub fn remove_bio(&self) {
        let _ = std::fs::remove_file(self.bio());
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
        self.clear_kdbx()?;
        for p in [self.config(), self.data(), self.bio()] {
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
            library: Library::default(),
            vault_files: Default::default(),
            vault_used: Default::default(),
            cache_days: DEFAULT_CACHE_DAYS,
        }
    }

    fn item(id: &str) -> Item {
        Item { id: id.into(), name: format!("{id}.kdbx"), folder: false, path: String::new(), etag: String::new() }
    }

    #[test]
    fn cache_is_pruned_by_age_and_by_vaults_that_are_gone() {
        let dir = std::env::temp_dir().join(format!("kpm-android-cache-{}", std::process::id()));
        let paths = Paths::new(dir.clone());
        let mut c = sample();
        for (vault, id) in [("v1", "A1"), ("v2", "B2"), ("v3", "C3")] {
            paths.write_kdbx(id, b"kdbx").unwrap();
            c.vault_files.insert(vault.into(), item(id));
        }
        paths.write_kdbx("SUELTO", b"kdbx").unwrap();
        let day = 86_400;
        let now = 100 * day;
        c.vault_used.insert("v1".into(), now - 5 * day);
        c.vault_used.insert("v2".into(), now - 40 * day);
        // v3 no tiene fecha (copia de una versión anterior): empieza a contar ahora.
        let live: std::collections::HashSet<&str> = ["v1", "v2", "v3"].into();
        assert!(c.prune_cache(&live, &paths, now));
        let mut left: Vec<String> = paths.list_kdbx().into_iter().map(|(id, _)| id).collect();
        left.sort();
        assert_eq!(left, ["A1", "C3"], "caduca la de 40 días y la suelta; v3 se conserva");
        assert!(c.vault_files.contains_key("v2"), "se recuerda dónde está para volver a descargarla");
        assert_eq!(c.vault_used.get("v3"), Some(&now));

        // Un vault que desaparece del fichero de datos se olvida del todo.
        let live: std::collections::HashSet<&str> = ["v3"].into();
        assert!(c.prune_cache(&live, &paths, now));
        assert!(!c.vault_files.contains_key("v1") && !c.vault_used.contains_key("v1"));
        assert_eq!(paths.list_kdbx().len(), 1);

        // Sin caducidad no se borra por antigüedad.
        c.cache_days = 0;
        assert!(!c.prune_cache(&live, &paths, now + 1000 * day));
        assert_eq!(paths.list_kdbx().len(), 1);

        paths.clear_kdbx().unwrap();
        assert!(paths.list_kdbx().is_empty());
        let _ = std::fs::remove_dir_all(dir);
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
        paths.write_kdbx("ABC!12", b"kdbx").unwrap();
        assert_eq!(paths.read_kdbx("ABC!12").unwrap(), b"kdbx");
        assert!(paths.write_kdbx("../fuera", b"x").is_err(), "el id no puede salir de la carpeta");
        paths.reset().unwrap();
        assert!(!dir.join("config.kpm").exists());
        assert!(!dir.join("kdbx").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn backup_needs_the_master_and_leaves_out_the_token() {
        let keys = Keys::new("maestra", crate::crypto::KdfCost { m_kib: 8, t: 1, p: 1 }).unwrap();
        let mut cfg = sample();
        cfg.library.add("comun.keyx", b"clave-comun", 1).unwrap();
        let blob = cfg.backup(&keys).unwrap();
        assert!(!String::from_utf8_lossy(&blob).contains("ana@contoso.com"), "cifrada");

        assert!(matches!(Backup::open("otra", &blob), Err(AppError::WrongPassword)));
        let b = Backup::open("maestra", &blob).unwrap();
        assert_eq!((b.account.upn.as_str(), b.data_file.id.as_str()), ("ana@contoso.com", "D1"));
        assert_eq!(b.library.keys[0].bytes().unwrap().as_slice(), b"clave-comun");
        let plain = Keys::decrypt("maestra", &blob).unwrap().1;
        assert!(!String::from_utf8_lossy(&plain).contains("secreto-de-refresco"), "sin token de Microsoft");

        // Otro fichero cifrado con la misma maestra (la configuración, un vaults.enc) no es una copia.
        let other = keys.encrypt(&serde_json::to_vec(&cfg).unwrap()).unwrap();
        assert!(matches!(Backup::open("maestra", &other), Err(AppError::Msg { code: "notBackup", .. })));
        assert!(matches!(Backup::open("maestra", b"basura"), Err(AppError::Msg { code: "notBackup", .. })));
    }

    #[test]
    fn backup_names_carry_the_date() {
        assert_eq!(backup_name(0), "kpmanager-android-1970-01-01.kpmbak");
        assert_eq!(backup_name(1_795_564_800), "kpmanager-android-2026-11-25.kpmbak");
        assert_eq!(backup_name(951_782_400), "kpmanager-android-2000-02-29.kpmbak");
    }
}
