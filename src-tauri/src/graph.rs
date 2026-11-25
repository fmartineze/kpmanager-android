//! Microsoft 365: inicio de sesión (OAuth 2.0 con PKCE, cliente público) y lectura de OneDrive con Graph.
//!
//! - Solo se piden permisos delegados de lectura: `Files.Read` y `User.Read` (más `offline_access`).
//! - Los tokens nunca salen de Rust: la interfaz solo ve nombres de cuenta, carpetas y ficheros.
//! - Solo se habla con `login.microsoftonline.com` y `graph.microsoft.com` (y la URL de descarga que da Graph).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::aead::{OsRng, rand_core::RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

use crate::error::{AppError, Result};

pub const REDIRECT_URI: &str = "kpmanager://auth";
const SCOPES: &str = "offline_access Files.Read User.Read";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";
/// `vaults.enc` es pequeño; esto solo evita descargas absurdas.
const MAX_DOWNLOAD: usize = 64 * 1024 * 1024;

/// Aplicación registrada en Entra ID: el tenant (ID o dominio) y el ID de aplicación (cliente).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AppReg {
    pub tenant: String,
    pub client_id: String,
}

impl AppReg {
    /// Valida lo que escribe el usuario: un GUID o un dominio, nada que pueda alterar la URL.
    pub fn validated(tenant: &str, client_id: &str) -> Result<Self> {
        let tenant = tenant.trim();
        let client_id = client_id.trim();
        let ok_tenant = !tenant.is_empty()
            && tenant.len() <= 128
            && tenant.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
        let ok_client = client_id.len() == 36 && client_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        if !ok_tenant {
            return Err(AppError::msg("tenantInvalid"));
        }
        if !ok_client {
            return Err(AppError::msg("clientInvalid"));
        }
        Ok(Self { tenant: tenant.to_owned(), client_id: client_id.to_lowercase() })
    }

    fn base(&self) -> String {
        format!("https://login.microsoftonline.com/{}/oauth2/v2.0", self.tenant)
    }
}

fn random_b64(n: usize) -> String {
    let mut b = vec![0u8; n];
    OsRng.fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

/// Inicio de sesión en curso: lo que hace falta para validar la vuelta del navegador.
pub struct PendingLogin {
    pub reg: AppReg,
    state: String,
    verifier: Zeroizing<String>,
}

impl PendingLogin {
    /// Prepara el inicio de sesión y devuelve también la URL que se abre en el navegador.
    pub fn start(reg: AppReg) -> (Self, String) {
        let verifier = Zeroizing::new(random_b64(32));
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = random_b64(16);
        let mut url = url::Url::parse(&format!("{}/authorize", reg.base())).expect("URL fija");
        url.query_pairs_mut()
            .append_pair("client_id", &reg.client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("response_mode", "query")
            .append_pair("scope", SCOPES)
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "select_account");
        (Self { reg, state, verifier }, url.into())
    }

    /// Extrae el código de la URL de vuelta (`kpmanager://auth?code=…&state=…`) comprobando el `state`.
    pub fn code_from(&self, url: &url::Url) -> Result<Zeroizing<String>> {
        let mut code = None;
        let mut state = None;
        let mut error = None;
        for (k, v) in url.query_pairs() {
            match k.as_ref() {
                "code" => code = Some(Zeroizing::new(v.into_owned())),
                "state" => state = Some(v.into_owned()),
                "error" => error = Some(v.into_owned()),
                _ => {}
            }
        }
        if state.as_deref() != Some(self.state.as_str()) {
            return Err(AppError::msg("loginState"));
        }
        if let Some(e) = error {
            // Solo el código de error (p. ej. `access_denied`), nunca la descripción completa.
            let e: String = e.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(64).collect();
            return Err(AppError::msg_arg("loginDenied", e));
        }
        code.ok_or_else(|| AppError::msg("loginState"))
    }

    pub async fn redeem(&self, http: &reqwest::Client, code: &str) -> Result<Tokens> {
        let form = [
            ("client_id", self.reg.client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("code_verifier", self.verifier.as_str()),
            ("scope", SCOPES),
        ];
        token_request(http, &self.reg, &form).await
    }
}

/// Tokens de Microsoft. El de refresco se guarda cifrado con la maestra (ver `config.rs`).
pub struct Tokens {
    pub access: Zeroizing<String>,
    pub expires: Instant,
    pub refresh: Option<Zeroizing<String>>,
}

impl Tokens {
    pub fn valid(&self) -> bool {
        Instant::now() + Duration::from_secs(60) < self.expires
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
}

async fn token_request(http: &reqwest::Client, reg: &AppReg, form: &[(&str, &str)]) -> Result<Tokens> {
    let resp = http.post(format!("{}/token", reg.base())).form(form).send().await?;
    if !resp.status().is_success() {
        let code = resp.json::<TokenError>().await.map(|e| e.error).unwrap_or_default();
        // `invalid_grant`: el token de refresco caducó o se revocó; hay que volver a iniciar sesión.
        return Err(if code == "invalid_grant" { AppError::msg("loginExpired") } else { AppError::msg_arg("loginFailed", code) });
    }
    let t: TokenResponse = resp.json().await?;
    Ok(Tokens {
        access: Zeroizing::new(t.access_token),
        expires: Instant::now() + Duration::from_secs(t.expires_in),
        refresh: t.refresh_token.map(Zeroizing::new),
    })
}

pub async fn refresh(http: &reqwest::Client, reg: &AppReg, refresh_token: &str) -> Result<Tokens> {
    let form = [
        ("client_id", reg.client_id.as_str()),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("scope", SCOPES),
    ];
    token_request(http, reg, &form).await
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub name: String,
    pub upn: String,
}

/// Elemento de OneDrive que se enseña en la interfaz (carpeta o fichero).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub name: String,
    pub folder: bool,
    /// Ruta legible dentro de OneDrive (`/Clientes/Acme`), solo informativa.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub etag: String,
}

/// Los ID de OneDrive solo llevan letras, cifras y `!`; se comprueban porque van dentro de la URL.
fn check_id(id: &str) -> Result<&str> {
    if id == "root" || (!id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '!')) {
        Ok(id)
    } else {
        Err(AppError::msg("itemInvalid"))
    }
}

fn to_item(v: &serde_json::Value) -> Option<Item> {
    let parent = v["parentReference"]["path"].as_str().unwrap_or("");
    // Graph da `/drive/root:/Clientes`; se enseña `/Clientes`.
    let parent = parent.split_once("root:").map(|(_, p)| p).unwrap_or("");
    let name = v["name"].as_str()?.to_owned();
    Some(Item {
        id: v["id"].as_str()?.to_owned(),
        path: format!("{parent}/{name}"),
        name,
        folder: v.get("folder").is_some(),
        etag: v["eTag"].as_str().unwrap_or("").to_owned(),
    })
}

async fn get_json(http: &reqwest::Client, token: &str, url: &str) -> Result<serde_json::Value> {
    let resp = http.get(url).bearer_auth(token).send().await?;
    match resp.status().as_u16() {
        200 => Ok(resp.json().await?),
        401 => Err(AppError::msg("loginExpired")),
        403 => Err(AppError::msg("graphForbidden")),
        404 => Err(AppError::msg("itemMissing")),
        s => Err(AppError::msg_arg("graphError", s.to_string())),
    }
}

/// Recorre las páginas de una respuesta de Graph (`@odata.nextLink`), con un límite razonable.
async fn get_all(http: &reqwest::Client, token: &str, first: String) -> Result<Vec<Item>> {
    let mut out = Vec::new();
    let mut next = Some(first);
    let mut pages = 0;
    while let Some(url) = next.take() {
        // nextLink siempre apunta a Graph; si no, se ignora.
        if !url.starts_with(GRAPH) || pages == 20 {
            break;
        }
        pages += 1;
        let v = get_json(http, token, &url).await?;
        out.extend(v["value"].as_array().into_iter().flatten().filter_map(to_item));
        next = v["@odata.nextLink"].as_str().map(str::to_owned);
    }
    Ok(out)
}

pub async fn me(http: &reqwest::Client, token: &str) -> Result<Account> {
    let v = get_json(http, token, &format!("{GRAPH}/me?$select=displayName,userPrincipalName")).await?;
    Ok(Account {
        name: v["displayName"].as_str().unwrap_or("").to_owned(),
        upn: v["userPrincipalName"].as_str().unwrap_or("").to_owned(),
    })
}

const SELECT: &str = "$select=id,name,folder,file,eTag,parentReference";

/// Hijos de una carpeta: subcarpetas y ficheros `.enc` (lo único que el asistente necesita ver).
pub async fn children(http: &reqwest::Client, token: &str, id: &str) -> Result<Vec<Item>> {
    let id = check_id(id)?;
    let mut items = get_all(http, token, format!("{GRAPH}/me/drive/items/{id}/children?{SELECT}&$top=200")).await?;
    items.retain(|i| i.folder || i.name.to_lowercase().ends_with(".enc"));
    items.sort_by(|a, b| b.folder.cmp(&a.folder).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(items)
}

pub async fn item(http: &reqwest::Client, token: &str, id: &str) -> Result<Item> {
    let id = check_id(id)?;
    let v = get_json(http, token, &format!("{GRAPH}/me/drive/items/{id}?{SELECT}")).await?;
    to_item(&v).ok_or(AppError::BadFormat)
}

/// Busca ficheros de datos de KP Manager (`*.enc`) dentro de una carpeta y sus subcarpetas.
pub async fn find_data_files(http: &reqwest::Client, token: &str, folder: &str) -> Result<Vec<Item>> {
    let folder = check_id(folder)?;
    let is_data = |i: &Item| !i.folder && i.name.to_lowercase().ends_with(".enc");
    // La búsqueda de Graph es rápida pero aproximada (y tarda en indexar ficheros nuevos)…
    let mut items = get_all(http, token, format!("{GRAPH}/me/drive/items/{folder}/search(q='.enc')?{SELECT}&$top=200")).await?;
    items.retain(is_data);
    // …así que, si no encuentra nada, se recorren las carpetas (con límite).
    if items.is_empty() {
        let mut queue = std::collections::VecDeque::from([folder.to_owned()]);
        let mut visited = 0;
        while let Some(id) = queue.pop_front() {
            visited += 1;
            if visited > 300 {
                break;
            }
            for i in children(http, token, &id).await? {
                if i.folder {
                    queue.push_back(i.id);
                } else if is_data(&i) {
                    items.push(i);
                }
            }
        }
    }
    // `vaults.enc` (el nombre por defecto) primero.
    items.sort_by_key(|i| (i.name.to_lowercase() != "vaults.enc", i.path.to_lowercase()));
    Ok(items)
}

/// Parte de una ruta de Windows que queda dentro de la carpeta de origen.
///
/// `C:\Users\ana\OneDrive - Org\Clientes\Acme\acme.kdbx` con la carpeta `/Clientes` da
/// `["Acme", "acme.kdbx"]`. Si la carpeta aparece varias veces se usa la última. `None` si no aparece o si
/// algún tramo no es un nombre normal (`..`, `:`).
pub fn relative_to(folder_path: &str, windows_path: &str) -> Option<Vec<String>> {
    let folder: Vec<&str> = folder_path.split('/').filter(|c| !c.is_empty()).collect();
    let parts: Vec<&str> = windows_path.split(['\\', '/']).filter(|c| !c.is_empty()).collect();
    if folder.is_empty() || parts.len() <= folder.len() {
        return None;
    }
    let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
    let start = (0..parts.len() - folder.len())
        .rev()
        .find(|&i| folder.iter().zip(&parts[i..]).all(|(f, p)| same(f, p)))?;
    let rest: Vec<String> = parts[start + folder.len()..].iter().map(|s| (*s).to_owned()).collect();
    if rest.is_empty() || rest.iter().any(|c| c == "." || c == ".." || c.contains(':')) {
        return None;
    }
    Some(rest)
}

/// Cuántos tramos finales comparten dos rutas (para elegir entre ficheros con el mismo nombre).
fn common_suffix(a: &str, b: &str) -> usize {
    let split = |s: &str| -> Vec<String> { s.split(['\\', '/']).filter(|c| !c.is_empty()).map(str::to_lowercase).collect() };
    let (a, b) = (split(a), split(b));
    a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count()
}

fn file_name(windows_path: &str) -> String {
    windows_path.rsplit(['\\', '/']).next().unwrap_or("").to_owned()
}

/// Elemento por ruta relativa a una carpeta (`/items/{id}:/a/b.kdbx`). Cada tramo se codifica.
async fn item_by_path(http: &reqwest::Client, token: &str, folder: &str, rel: &[String]) -> Result<Item> {
    let folder = check_id(folder)?;
    let mut url = url::Url::parse(GRAPH).map_err(|_| AppError::msg("internal"))?;
    {
        let mut seg = url.path_segments_mut().map_err(|_| AppError::msg("internal"))?;
        seg.extend(["me", "drive", "items", &format!("{folder}:")]);
        seg.extend(rel);
    }
    url.set_query(Some(SELECT));
    let v = get_json(http, token, url.as_str()).await?;
    to_item(&v).ok_or(AppError::BadFormat)
}

/// Localiza en OneDrive el `.kdbx` de un vault a partir de su ruta de Windows.
pub async fn locate_vault(http: &reqwest::Client, token: &str, folder: &Item, windows_path: &str) -> Result<Item> {
    let name = file_name(windows_path);
    let rel = relative_to(&folder.path, windows_path);
    // El mensaje dice qué se buscó: la ruta esperada dentro de OneDrive o, si no se pudo calcular, el nombre.
    let tried = match &rel {
        Some(r) => format!("{}/{}", folder.path.trim_end_matches('/'), r.join("/")),
        None => name.clone(),
    };
    let not_found = || AppError::msg_arg("vaultNotInOneDrive", tried.clone());
    let is_it = |i: &Item| !i.folder && i.name.eq_ignore_ascii_case(&name);
    let best = |found: Vec<Item>| found.into_iter().filter(is_it).max_by_key(|i| common_suffix(&i.path, windows_path));

    // 1. La misma ruta relativa que en Windows.
    if let Some(rel) = &rel {
        match item_by_path(http, token, &folder.id, rel).await {
            Ok(i) if !i.folder => return Ok(i),
            Ok(_) | Err(AppError::Msg { code: "itemMissing", .. }) => {}
            Err(e) => return Err(e),
        }
    }
    if name.is_empty() {
        return Err(not_found());
    }
    let search = |scope: Vec<String>| {
        let name = name.clone();
        async move {
            let mut url = url::Url::parse(GRAPH).map_err(|_| AppError::msg("internal"))?;
            {
                let mut seg = url.path_segments_mut().map_err(|_| AppError::msg("internal"))?;
                seg.extend(["me", "drive"]);
                seg.extend(scope);
                seg.push(&format!("search(q='{}')", name.replace('\'', "''")));
            }
            url.set_query(Some(&format!("{SELECT}&$top=200")));
            // Una búsqueda que Graph no admite (o falla) no es definitiva: se sigue con el siguiente método.
            match get_all(http, token, url.to_string()).await {
                Ok(v) => Ok(v),
                Err(AppError::Msg { code: "graphError" | "itemMissing" | "graphForbidden", .. }) => Ok(Vec::new()),
                Err(e) => Err(e),
            }
        }
    };

    // 2. Por nombre dentro de la carpeta de origen.
    if let Some(i) = best(search(vec!["items".into(), check_id(&folder.id)?.to_owned()]).await?) {
        return Ok(i);
    }
    // 3. Por nombre en todo el OneDrive (el vault puede estar fuera de la carpeta de origen).
    if let Some(i) = best(search(vec!["root".into()]).await?) {
        return Ok(i);
    }
    // 4. Recorriendo las subcarpetas de la de origen (la búsqueda de Graph tarda en indexar ficheros nuevos).
    let mut queue = std::collections::VecDeque::from([folder.id.clone()]);
    let mut visited = 0;
    while let Some(id) = queue.pop_front() {
        visited += 1;
        if visited > 500 {
            break;
        }
        let id = check_id(&id)?.to_owned();
        let items = get_all(http, token, format!("{GRAPH}/me/drive/items/{id}/children?{SELECT}&$top=200")).await?;
        for i in items {
            if i.folder {
                queue.push_back(i.id);
            } else if is_it(&i) {
                return Ok(i);
            }
        }
    }
    Err(not_found())
}

pub async fn download(http: &reqwest::Client, token: &str, id: &str) -> Result<Vec<u8>> {
    let id = check_id(id)?;
    let resp = http.get(format!("{GRAPH}/me/drive/items/{id}/content")).bearer_auth(token).send().await?;
    match resp.status().as_u16() {
        200 => {}
        401 => return Err(AppError::msg("loginExpired")),
        404 => return Err(AppError::msg("itemMissing")),
        s => return Err(AppError::msg_arg("graphError", s.to_string())),
    }
    if resp.content_length().is_some_and(|n| n as usize > MAX_DOWNLOAD) {
        return Err(AppError::BadFormat);
    }
    let bytes = resp.bytes().await?;
    if bytes.len() > MAX_DOWNLOAD {
        return Err(AppError::BadFormat);
    }
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_registration() {
        let id = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
        assert!(AppReg::validated("contoso.onmicrosoft.com", id).is_ok());
        assert!(AppReg::validated(" 72f988bf-86f1-41af-91ab-2d7cd011db47 ", id).is_ok());
        assert!(AppReg::validated("evil.com/x?y", id).is_err());
        assert!(AppReg::validated("", id).is_err());
        assert!(AppReg::validated("contoso.com", "no-es-un-guid").is_err());
        assert!(AppReg::validated("contoso.com", "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0&x=1").is_err());
    }

    #[test]
    fn login_url_has_pkce_and_checks_state() {
        let reg = AppReg::validated("contoso.com", "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0").unwrap();
        let (p, url) = PendingLogin::start(reg);
        let u = url::Url::parse(&url).unwrap();
        assert_eq!(u.host_str(), Some("login.microsoftonline.com"));
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["redirect_uri"], REDIRECT_URI);
        assert!(!q.contains_key("code_verifier"), "el verificador nunca va en la URL");

        let ok = url::Url::parse(&format!("kpmanager://auth?code=abc&state={}", q["state"])).unwrap();
        assert_eq!(p.code_from(&ok).unwrap().as_str(), "abc");
        let bad = url::Url::parse("kpmanager://auth?code=abc&state=otro").unwrap();
        assert!(p.code_from(&bad).is_err());
        let denied = url::Url::parse(&format!("kpmanager://auth?error=access_denied&error_description=x&state={}", q["state"])).unwrap();
        assert!(matches!(p.code_from(&denied), Err(AppError::Msg { code: "loginDenied", ref arg }) if arg == "access_denied"));
    }

    #[test]
    fn windows_paths_map_into_the_source_folder() {
        let p = r"C:\Users\ana\OneDrive - Org\Clientes\Acme\acme.kdbx";
        assert_eq!(relative_to("/Clientes", p).unwrap(), ["Acme", "acme.kdbx"]);
        assert_eq!(relative_to("/clientes", p).unwrap(), ["Acme", "acme.kdbx"], "sin distinguir mayúsculas");
        assert_eq!(relative_to("/Trabajo/Clientes", r"D:\OD\Trabajo\Clientes\x.kdbx").unwrap(), ["x.kdbx"]);
        assert_eq!(relative_to("/A", r"C:\A\B\A\c.kdbx").unwrap(), ["c.kdbx"], "la última aparición");
        assert!(relative_to("/Otra", p).is_none());
        assert!(relative_to("/Clientes", r"C:\Clientes").is_none());
        assert!(relative_to("/Clientes", r"C:\Clientes\..\x.kdbx").is_none());
        assert_eq!(common_suffix("/Clientes/Acme/acme.kdbx", p), 3);
        assert_eq!(common_suffix("/Otros/acme.kdbx", p), 1);
        assert_eq!(file_name(p), "acme.kdbx");
    }

    #[test]
    fn item_ids_are_checked() {
        assert!(check_id("root").is_ok());
        assert!(check_id("01ABCDEF!123").is_ok());
        assert!(check_id("../x").is_err());
        assert!(check_id("a/children?x").is_err());
        assert!(check_id("").is_err());
    }

    #[test]
    fn graph_item_paths_are_readable() {
        let v = serde_json::json!({"id":"1","name":"vaults.enc","file":{},"eTag":"e","parentReference":{"path":"/drive/root:/Clientes/KP"}});
        let i = to_item(&v).unwrap();
        assert_eq!((i.path.as_str(), i.folder), ("/Clientes/KP/vaults.enc", false));
    }
}
