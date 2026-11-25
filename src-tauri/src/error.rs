//! Errores con código estable: la interfaz los traduce (mismo patrón que la app de Windows).
//! Nunca llevan tokens, contraseñas ni contenido de los vaults.

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("La aplicación está bloqueada")]
    Locked,
    #[error("Contraseña maestra incorrecta")]
    WrongPassword,
    #[error("Fichero corrupto o con formato desconocido")]
    BadFormat,
    #[error("Error de cifrado: {0}")]
    Crypto(String),
    #[error("Error de E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("Error de serialización: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Sin conexión con Microsoft")]
    Network,
    /// Mensaje con código estable: la interfaz lo traduce al idioma elegido.
    #[error("{code} {arg}")]
    Msg { code: &'static str, arg: String },
}

pub type Result<T> = std::result::Result<T, AppError>;

impl AppError {
    pub fn msg(code: &'static str) -> Self {
        Self::Msg { code, arg: String::new() }
    }

    pub fn msg_arg(code: &'static str, arg: impl Into<String>) -> Self {
        Self::Msg { code, arg: arg.into() }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::WrongPassword => "wrongPassword",
            Self::BadFormat => "badFormat",
            Self::Crypto(_) => "crypto",
            Self::Io(_) => "io",
            Self::Json(_) => "json",
            Self::Network => "network",
            Self::Msg { code, .. } => code,
        }
    }

    fn arg(&self) -> String {
        match self {
            Self::Crypto(s) => s.clone(),
            Self::Io(e) => e.to_string(),
            Self::Json(e) => e.to_string(),
            Self::Msg { arg, .. } => arg.clone(),
            _ => String::new(),
        }
    }
}

/// Un fallo de red se reduce a «sin conexión»: el detalle de reqwest puede incluir la URL con parámetros.
impl From<reqwest::Error> for AppError {
    fn from(_: reqwest::Error) -> Self {
        Self::Network
    }
}

/// Se envía a la interfaz como `{ code, arg, message }`; `message` (en español) es el respaldo.
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("AppError", 3)?;
        st.serialize_field("code", self.code())?;
        st.serialize_field("arg", &self.arg())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}
