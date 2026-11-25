//! Estructura del fichero de datos (`vaults.enc`), copiada de `kpmanager/src-tauri/src/model.rs`.
//! Android solo lo lee: nunca lo escribe, así que no hace falta conservar nada al guardar.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zeroize::Zeroizing;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Vault {
    pub id: String,
    pub client: String,
    /// Ruta de Windows tal cual (con `\`): en Android no es una ruta local.
    pub path: String,
    pub keyfile: Option<String>,
    pub password: Option<Zeroizing<String>>,
    #[serde(default)]
    pub favorite: bool,
    /// Etiquetas para agrupar y filtrar clientes («Hosting», «Contabilidad»…).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Segundos desde la época Unix de la última apertura desde la app.
    #[serde(default)]
    pub last_opened: Option<u64>,
    /// Abrir siempre directamente en KeePassXC, sin la vista dentro de la app.
    #[serde(default)]
    pub open_external: bool,
    /// Campos que esta versión no conoce (los añadió una más nueva): se conservan al volver a guardar.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub keepassxc: Option<Value>,
    /// Keyfile que se ofrece al crear o editar un vault.
    #[serde(default)]
    pub default_keyfile: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Versión del esquema del fichero de datos que entiende esta versión de la app.
///
/// Se sube solo cuando un cambio **no** se puede leer con las versiones anteriores (renombrar o cambiar el
/// significado de un campo). Añadir campos nuevos no lo requiere: se ignoran y se conservan (`extra`). Quien
/// suba este número debe rellenar `min_app` con la primera versión de la app que lo entiende, para que las
/// anteriores muestren «actualiza la aplicación» en vez de un error.
pub const DATA_FORMAT: u32 = 1;

fn legacy_format() -> u32 {
    1
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Data {
    /// Esquema del fichero. Los ficheros anteriores a este campo son el 1.
    #[serde(default = "legacy_format")]
    pub format: u32,
    /// Primera versión de la app capaz de leer este fichero (solo si hubo un cambio incompatible).
    #[serde(default)]
    pub min_app: Option<String>,
    /// Última versión de la app que lo guardó (informativo).
    #[serde(default)]
    pub written_by: Option<String>,
    #[serde(default)]
    pub vaults: Vec<Vault>,
    #[serde(default)]
    pub settings: Settings,
    /// Carpetas en seguimiento: se revisan en busca de vaults nuevos o desaparecidos.
    #[serde(default)]
    pub watched: Vec<String>,
    /// Vaults detectados que el usuario decidió no añadir; no se vuelve a preguntar.
    #[serde(default)]
    pub ignored: Vec<String>,
    /// Campos de versiones más nuevas que esta no conoce: se conservan al guardar.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Data {
    fn default() -> Self {
        Self {
            format: DATA_FORMAT,
            min_app: None,
            written_by: None,
            vaults: Vec::new(),
            settings: Settings::default(),
            watched: Vec::new(),
            ignored: Vec::new(),
            extra: Map::new(),
        }
    }
}

impl Data {
    /// Lee los datos descifrados. Un fichero de un esquema más nuevo se rechaza con `dataNewer` (y la versión
    /// mínima que lo entiende, si consta): abrirlo y volver a guardarlo podría estropearlo.
    pub fn parse(plain: &[u8]) -> crate::error::Result<Data> {
        #[derive(Deserialize)]
        struct Probe {
            format: Option<u32>,
            min_app: Option<String>,
        }
        let probe: Probe = serde_json::from_slice(plain)?;
        if probe.format.unwrap_or(1) > DATA_FORMAT {
            return Err(crate::error::AppError::msg_arg("dataNewer", probe.min_app.unwrap_or_default()));
        }
        let mut data: Data = serde_json::from_slice(plain)?;
        data.migrate();
        Ok(data)
    }

    /// Lleva a los datos de un esquema anterior al actual. Hoy solo existe el 1.
    fn migrate(&mut self) {
        self.format = DATA_FORMAT;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_schema_is_refused_and_unknown_fields_survive() {
        // Un fichero sin «format» (anterior a este campo) es el esquema 1.
        let old = br#"{"vaults":[{"id":"1","client":"A","path":"a.kdbx","keyfile":null,"password":null}]}"#;
        let d = Data::parse(old).unwrap();
        assert_eq!((d.format, d.vaults.len()), (1, 1));

        // Campos desconocidos (de una versión más nueva, pero compatible) se conservan al reescribir.
        let newer_minor = br#"{"format":1,"futuro":{"x":1},"vaults":[{"id":"1","client":"A","path":"a.kdbx","keyfile":null,"password":null,"color":"rojo"}]}"#;
        let d = Data::parse(newer_minor).unwrap();
        let again = serde_json::to_value(&d).unwrap();
        assert_eq!(again["futuro"]["x"], 1);
        assert_eq!(again["vaults"][0]["color"], "rojo");

        // Un esquema más nuevo no se abre: se pide actualizar, con la versión mínima si consta.
        let newer = br#"{"format":2,"min_app":"9.0.0","vaults":[]}"#;
        assert!(matches!(Data::parse(newer), Err(crate::error::AppError::Msg { code: "dataNewer", ref arg }) if arg == "9.0.0"));
        let no_min = br#"{"format":7}"#;
        assert!(matches!(Data::parse(no_min), Err(crate::error::AppError::Msg { code: "dataNewer", ref arg }) if arg.is_empty()));
    }
}
