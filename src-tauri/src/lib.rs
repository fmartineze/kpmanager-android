//! KP Manager para Android: consulta de solo lectura.
//!
//! Lee el mismo fichero de datos que la app de Windows (`vaults.enc`) desde OneDrive y nunca lo modifica.
//! Los secretos (tokens de Microsoft, claves derivadas, contraseñas de los vaults) no salen de Rust.

mod config;
mod crypto;
mod docs;
mod error;
mod graph;
mod library;
mod model;

use serde::Serialize;
use tauri::async_runtime::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_opener::OpenerExt;
use zeroize::Zeroizing;

use config::{Backup, Config, Paths};
use crypto::Keys;
use error::{AppError, Result};
use graph::{Account, AppReg, Item, PendingLogin, Tokens};
use library::{KeyState, KeyView, Library};
use model::Data;

/// Lo que la interfaz ve de cada vault: sin contraseñas ni rutas completas.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VaultView {
    id: String,
    client: String,
    tags: Vec<String>,
    favorite: bool,
    /// Nombre del `.kdbx` (sin la ruta de Windows).
    file: String,
    /// Nombre del fichero de clave que usa en Windows, si usa uno.
    keyfile: Option<String>,
    /// Fichero de clave de la biblioteca de este móvil que le corresponde.
    key: KeyState,
}

fn file_name(windows_path: &str) -> String {
    windows_path.rsplit(['\\', '/']).next().unwrap_or(windows_path).to_owned()
}

fn views(data: &Data) -> Vec<VaultView> {
    let mut v: Vec<VaultView> = data
        .vaults
        .iter()
        .map(|v| VaultView {
            id: v.id.clone(),
            client: v.client.clone(),
            tags: v.tags.clone(),
            favorite: v.favorite,
            file: file_name(&v.path),
            keyfile: v.keyfile.as_deref().map(file_name),
            key: KeyState::default(),
        })
        .collect();
    v.sort_by(|a, b| b.favorite.cmp(&a.favorite).then_with(|| a.client.to_lowercase().cmp(&b.client.to_lowercase())));
    v
}

/// Añade a cada vault la clave de la biblioteca que le toca.
fn with_keys(base: &[VaultView], lib: &Library) -> Vec<VaultView> {
    base.iter()
        .map(|v| VaultView { key: lib.resolve(&v.id, v.keyfile.as_deref()), ..v.clone() })
        .collect()
}

/// Descifra el fichero de datos con la maestra y devuelve la lista (y la clave, para próximas descargas).
fn open_data(password: &str, blob: &[u8]) -> Result<(Keys, Vec<VaultView>)> {
    let (keys, plain) = Keys::decrypt(password, blob)?;
    Ok((keys, views(&Data::parse(&plain)?)))
}

/// Ejecuta trabajo pesado (Argon2) fuera del hilo de la interfaz.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|_| AppError::msg("internal"))?
}

/// Asistente del primer arranque: lo que se va reuniendo antes de tener la maestra.
#[derive(Default)]
struct Wizard {
    reg: Option<AppReg>,
    account: Option<Account>,
    tokens: Option<Tokens>,
    /// Copia de seguridad elegida, aún sin abrir.
    restore_blob: Option<Vec<u8>>,
    /// Copia abierta: la maestra se guarda en memoria hasta terminar el asistente (no se pide dos veces).
    restore: Option<(Backup, Zeroizing<String>)>,
}

/// App desbloqueada.
struct Session {
    cfg_keys: Keys,
    config: Config,
    data_keys: Keys,
    vaults: Vec<VaultView>,
    tokens: Option<Tokens>,
}

impl Session {
    fn vaults(&self) -> Vec<VaultView> {
        with_keys(&self.vaults, &self.config.library)
    }

    fn save(&self, paths: &Paths) -> Result<()> {
        paths.save(&self.cfg_keys, &self.config)
    }
}

#[derive(Default)]
struct Inner {
    wizard: Wizard,
    session: Option<Session>,
    login: Option<PendingLogin>,
}

struct AppState {
    paths: Paths,
    http: reqwest::Client,
    inner: Mutex<Inner>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    configured: bool,
    unlocked: bool,
    version: &'static str,
}

#[tauri::command]
async fn status(state: State<'_, AppState>) -> Result<Status> {
    let inner = state.inner.lock().await;
    Ok(Status { configured: state.paths.configured(), unlocked: inner.session.is_some(), version: env!("CARGO_PKG_VERSION") })
}

#[tauri::command]
async fn lock(state: State<'_, AppState>) -> Result<()> {
    state.inner.lock().await.session = None;
    Ok(())
}

/// Borra la configuración de este teléfono y vuelve al asistente.
#[tauri::command]
async fn reset(state: State<'_, AppState>) -> Result<()> {
    *state.inner.lock().await = Inner::default();
    state.paths.reset()
}

/// Abre el inicio de sesión de Microsoft en el navegador. El resultado llega con el evento `login`.
#[tauri::command]
async fn login(app: AppHandle, state: State<'_, AppState>, tenant: Option<String>, client_id: Option<String>) -> Result<()> {
    let mut inner = state.inner.lock().await;
    // Con la app desbloqueada (volver a iniciar sesión) se usa el registro guardado; en el asistente, el escrito.
    let reg = match (&inner.session, tenant, client_id) {
        (Some(s), _, _) => s.config.reg.clone(),
        (None, Some(t), Some(c)) => AppReg::validated(&t, &c)?,
        _ => return Err(AppError::msg("clientInvalid")),
    };
    let (pending, url) = PendingLogin::start(reg);
    inner.login = Some(pending);
    app.opener().open_url(url, None::<&str>).map_err(|_| AppError::msg("browserFailed"))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginEvent {
    account: Option<Account>,
    error: Option<serde_json::Value>,
}

/// Vuelta del navegador (`kpmanager://auth?…`): canjea el código y avisa a la interfaz.
async fn finish_login(app: AppHandle, url: url::Url) {
    let state = app.state::<AppState>();
    let result: Result<Account> = async {
        let pending = state.inner.lock().await.login.take().ok_or_else(|| AppError::msg("loginState"))?;
        let code = pending.code_from(&url)?;
        let tokens = pending.redeem(&state.http, &code).await?;
        let account = graph::me(&state.http, &tokens.access).await?;
        let mut inner = state.inner.lock().await;
        if let Some(s) = inner.session.as_mut() {
            if tokens.refresh.is_some() {
                s.config.refresh_token = tokens.refresh.clone();
            }
            s.config.account = account.clone();
            state.paths.save(&s.cfg_keys, &s.config)?;
            s.tokens = Some(tokens);
        } else {
            let w = &mut inner.wizard;
            w.reg = Some(pending.reg);
            w.account = Some(account.clone());
            w.tokens = Some(tokens);
        }
        Ok(account)
    }
    .await;
    let ev = match result {
        Ok(a) => LoginEvent { account: Some(a), error: None },
        Err(e) => LoginEvent { account: None, error: serde_json::to_value(&e).ok() },
    };
    let _ = app.emit("login", ev);
}

fn wizard_token(inner: &Inner) -> Result<String> {
    match inner.wizard.tokens.as_ref() {
        Some(t) if t.valid() => Ok(t.access.to_string()),
        _ => Err(AppError::msg("loginExpired")),
    }
}

#[tauri::command]
async fn wizard_children(state: State<'_, AppState>, id: String) -> Result<Vec<Item>> {
    let token = wizard_token(&*state.inner.lock().await)?;
    graph::children(&state.http, &token, &id).await
}

#[tauri::command]
async fn wizard_find(state: State<'_, AppState>, folder_id: String) -> Result<Vec<Item>> {
    let token = wizard_token(&*state.inner.lock().await)?;
    graph::find_data_files(&state.http, &token, &folder_id).await
}

/// Último paso del asistente: descarga el fichero de datos, lo abre con la maestra y guarda la configuración.
/// Al restaurar una copia, la maestra ya se escribió al abrirla y `password` llega vacío.
#[tauri::command]
async fn wizard_finish(state: State<'_, AppState>, folder_id: String, data_id: String, password: Option<String>) -> Result<Vec<VaultView>> {
    let (token, password) = {
        let inner = state.inner.lock().await;
        let password = match (password, &inner.wizard.restore) {
            (Some(p), _) => Zeroizing::new(p),
            (None, Some((_, p))) => p.clone(),
            (None, None) => return Err(AppError::WrongPassword),
        };
        (wizard_token(&inner)?, password)
    };
    let folder = graph::item(&state.http, &token, &folder_id).await?;
    let data_file = graph::item(&state.http, &token, &data_id).await?;
    let blob = graph::download(&state.http, &token, &data_id).await?;
    if !Keys::looks_valid(&blob) {
        return Err(AppError::msg("notKpmData"));
    }
    let (data_keys, vaults, cfg_keys) = {
        let blob = blob.clone();
        blocking(move || {
            let (data_keys, vaults) = open_data(&password, &blob)?;
            Ok((data_keys, vaults, Paths::new_keys(&password)?))
        })
        .await?
    };

    let mut inner = state.inner.lock().await;
    let w = std::mem::take(&mut inner.wizard);
    let (Some(reg), Some(account), Some(tokens)) = (w.reg, w.account, w.tokens) else {
        return Err(AppError::msg("loginExpired"));
    };
    let library = w.restore.map(|(b, _)| b.library).unwrap_or_default();
    let config = Config {
        version: 1,
        reg,
        account,
        folder,
        data_file,
        refresh_token: tokens.refresh.clone(),
        synced_at: Some(config::now()),
        library,
    };
    state.paths.write_data(&blob)?;
    state.paths.save(&cfg_keys, &config)?;
    let s = Session { cfg_keys, config, data_keys, vaults, tokens: Some(tokens) };
    let out = s.vaults();
    inner.session = Some(s);
    Ok(out)
}

/// Lo que se enseña de una copia de seguridad abierta.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RestoreInfo {
    tenant: String,
    client_id: String,
    account: Account,
    folder: Item,
    data_file: Item,
    keys: usize,
    created: u64,
}

/// Asistente → «Restaurar copia»: el usuario elige el fichero `.kpmbak`.
#[tauri::command]
async fn wizard_restore_pick(app: AppHandle, state: State<'_, AppState>) -> Result<Option<String>> {
    let Some(doc) = blocking(move || docs::open(&app, config::MAX_BACKUP_BYTES)).await? else {
        return Ok(None);
    };
    if !Keys::looks_valid(&doc.data) {
        return Err(AppError::msg("notBackup"));
    }
    let mut inner = state.inner.lock().await;
    inner.wizard.restore = None;
    inner.wizard.restore_blob = Some(doc.data.to_vec());
    Ok(Some(doc.name))
}

/// Abre la copia elegida con la maestra.
#[tauri::command]
async fn wizard_restore_open(state: State<'_, AppState>, password: String) -> Result<RestoreInfo> {
    let password = Zeroizing::new(password);
    let blob = state.inner.lock().await.wizard.restore_blob.clone().ok_or_else(|| AppError::msg("notBackup"))?;
    let (backup, password) = blocking(move || Ok((Backup::open(&password, &blob)?, password))).await?;
    let info = RestoreInfo {
        tenant: backup.reg.tenant.clone(),
        client_id: backup.reg.client_id.clone(),
        account: backup.account.clone(),
        folder: backup.folder.clone(),
        data_file: backup.data_file.clone(),
        keys: backup.library.keys.len(),
        created: backup.created,
    };
    let mut inner = state.inner.lock().await;
    inner.wizard.restore_blob = None;
    inner.wizard.restore = Some((backup, password));
    Ok(info)
}

/// Desbloquea con la maestra usando la copia local (funciona sin conexión).
#[tauri::command]
async fn unlock(state: State<'_, AppState>, password: String) -> Result<Vec<VaultView>> {
    let password = Zeroizing::new(password);
    let paths = state.paths.clone();
    let (cfg_keys, config, data_keys, vaults) = blocking(move || {
        let (cfg_keys, config) = paths.load(&password)?;
        let blob = paths.read_data()?;
        // La maestra abre la configuración pero no los datos: se cambió en Windows después de configurar el móvil.
        let (data_keys, vaults) = open_data(&password, &blob).map_err(|e| match e {
            AppError::WrongPassword => AppError::msg("masterChanged"),
            e => e,
        })?;
        Ok((cfg_keys, config, data_keys, vaults))
    })
    .await?;
    let s = Session { cfg_keys, config, data_keys, vaults, tokens: None };
    let out = s.vaults();
    state.inner.lock().await.session = Some(s);
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    account: Account,
    folder: String,
    data_file: String,
    synced_at: Option<u64>,
}

#[tauri::command]
async fn info(state: State<'_, AppState>) -> Result<Info> {
    let inner = state.inner.lock().await;
    let s = inner.session.as_ref().ok_or(AppError::Locked)?;
    Ok(Info {
        account: s.config.account.clone(),
        folder: s.config.folder.path.clone(),
        data_file: s.config.data_file.path.clone(),
        synced_at: s.config.synced_at,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncResult {
    changed: bool,
    vaults: Vec<VaultView>,
    synced_at: Option<u64>,
}

/// Token de acceso válido para la sesión; si caducó, se renueva con el de refresco (y se guarda el nuevo).
async fn session_token(state: &AppState) -> Result<String> {
    let (reg, refresh) = {
        let inner = state.inner.lock().await;
        let s = inner.session.as_ref().ok_or(AppError::Locked)?;
        if let Some(t) = s.tokens.as_ref().filter(|t| t.valid()) {
            return Ok(t.access.to_string());
        }
        (s.config.reg.clone(), s.config.refresh_token.clone().ok_or_else(|| AppError::msg("loginExpired"))?)
    };
    let tokens = graph::refresh(&state.http, &reg, &refresh).await?;
    let access = tokens.access.to_string();
    let mut inner = state.inner.lock().await;
    let s = inner.session.as_mut().ok_or(AppError::Locked)?;
    if let Some(r) = tokens.refresh.clone() {
        s.config.refresh_token = Some(r);
        state.paths.save(&s.cfg_keys, &s.config)?;
    }
    s.tokens = Some(tokens);
    Ok(access)
}

/// Comprueba si el fichero de datos cambió en OneDrive y, si es así, lo descarga.
#[tauri::command]
async fn sync(state: State<'_, AppState>) -> Result<SyncResult> {
    let token = session_token(&state).await?;
    let id = {
        let inner = state.inner.lock().await;
        inner.session.as_ref().ok_or(AppError::Locked)?.config.data_file.id.clone()
    };
    let meta = graph::item(&state.http, &token, &id).await?;
    let unchanged = {
        let inner = state.inner.lock().await;
        inner.session.as_ref().ok_or(AppError::Locked)?.config.data_file.etag == meta.etag
    };
    let blob = if unchanged { None } else { Some(graph::download(&state.http, &token, &id).await?) };

    let mut inner = state.inner.lock().await;
    let s = inner.session.as_mut().ok_or(AppError::Locked)?;
    let changed = match blob {
        Some(blob) => {
            // Misma clave mientras no cambie la maestra en Windows; si cambió, hay que volver a desbloquear.
            let plain = s.data_keys.open_with_key(&blob)?;
            s.vaults = views(&Data::parse(&plain)?);
            state.paths.write_data(&blob)?;
            true
        }
        None => false,
    };
    s.config.data_file = meta;
    s.config.synced_at = Some(config::now());
    s.save(&state.paths)?;
    Ok(SyncResult { changed, vaults: s.vaults(), synced_at: s.config.synced_at })
}

// ── Biblioteca de ficheros de clave ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryOut {
    /// Nombre del fichero recién importado (al importar).
    added: Option<String>,
    keys: Vec<KeyView>,
    vaults: Vec<VaultView>,
}

fn library_out(s: &Session, added: Option<String>) -> LibraryOut {
    let vaults = s.vaults();
    let mut uses = std::collections::BTreeMap::new();
    for id in vaults.iter().filter_map(|v| v.key.id.clone()) {
        *uses.entry(id).or_insert(0) += 1;
    }
    LibraryOut { added, keys: s.config.library.views(&uses), vaults }
}

/// Cambia la biblioteca, la guarda cifrada y devuelve el nuevo estado.
async fn edit_library(state: &AppState, added: Option<String>, f: impl FnOnce(&mut Library) -> Result<()>) -> Result<LibraryOut> {
    let mut inner = state.inner.lock().await;
    let s = inner.session.as_mut().ok_or(AppError::Locked)?;
    let mut lib = s.config.library.clone();
    f(&mut lib)?;
    s.config.library = lib;
    s.save(&state.paths)?;
    Ok(library_out(s, added))
}

#[tauri::command]
async fn keys(state: State<'_, AppState>) -> Result<LibraryOut> {
    let inner = state.inner.lock().await;
    Ok(library_out(inner.session.as_ref().ok_or(AppError::Locked)?, None))
}

/// Importa un fichero de clave elegido con el selector del sistema.
#[tauri::command]
async fn key_import(app: AppHandle, state: State<'_, AppState>) -> Result<LibraryOut> {
    if state.inner.lock().await.session.is_none() {
        return Err(AppError::Locked);
    }
    let Some(doc) = blocking(move || docs::open(&app, library::MAX_KEY_BYTES)).await? else {
        return keys(state).await;
    };
    let name = doc.name.clone();
    edit_library(&state, Some(name.clone()), |lib| lib.add(&name, &doc.data, config::now()).map(|_| ())).await
}

#[tauri::command]
async fn key_delete(state: State<'_, AppState>, id: String) -> Result<LibraryOut> {
    edit_library(&state, None, |lib| lib.remove(&id)).await
}

#[tauri::command]
async fn key_default(state: State<'_, AppState>, id: Option<String>) -> Result<LibraryOut> {
    edit_library(&state, None, |lib| lib.set_default(id.as_deref())).await
}

/// `choice`: `auto`, `none` o el id de una clave de la biblioteca.
#[tauri::command]
async fn vault_key(state: State<'_, AppState>, vault_id: String, choice: String) -> Result<LibraryOut> {
    {
        let inner = state.inner.lock().await;
        let s = inner.session.as_ref().ok_or(AppError::Locked)?;
        if !s.vaults.iter().any(|v| v.id == vault_id) {
            return Err(AppError::msg("vaultUnknown"));
        }
    }
    edit_library(&state, None, |lib| lib.assign(&vault_id, &choice)).await
}

// ── Copia de seguridad ───────────────────────────────────────────────────────────────────────────

/// Guarda la copia de seguridad donde elija el usuario. Devuelve el nombre, o nada si canceló.
#[tauri::command]
async fn backup_save(app: AppHandle, state: State<'_, AppState>) -> Result<Option<String>> {
    let blob = {
        let inner = state.inner.lock().await;
        let s = inner.session.as_ref().ok_or(AppError::Locked)?;
        s.config.backup(&s.cfg_keys)?
    };
    let name = config::backup_name(config::now());
    blocking(move || docs::save(&app, &name, "application/octet-stream", &blob)).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(docs::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            app.manage(AppState {
                paths: Paths::new(dir),
                http: reqwest::Client::builder()
                    .https_only(true)
                    .timeout(std::time::Duration::from_secs(60))
                    .build()
                    .expect("cliente HTTP"),
                inner: Mutex::new(Inner::default()),
            });
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    if url.scheme() == "kpmanager" && url.host_str() == Some("auth") {
                        tauri::async_runtime::spawn(finish_login(handle.clone(), url));
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status,
            lock,
            reset,
            login,
            wizard_children,
            wizard_find,
            wizard_finish,
            wizard_restore_pick,
            wizard_restore_open,
            unlock,
            info,
            sync,
            keys,
            key_import,
            key_delete,
            key_default,
            vault_key,
            backup_save,
        ])
        .run(tauri::generate_context!())
        .expect("error al arrancar KP Manager");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tests/fixtures/vaults-windows.enc` se generó con el código de la app de Windows (KPM1 + `Data`), con
    /// datos ficticios y la maestra `prueba-compat`. Si este test falla, Android ya no lee lo que escribe Windows.
    #[test]
    fn reads_a_data_file_written_by_the_windows_app() {
        let blob = include_bytes!("../tests/fixtures/vaults-windows.enc");
        assert!(matches!(open_data("otra", blob), Err(AppError::WrongPassword)));
        let (_, v) = open_data("prueba-compat", blob).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].client.as_str(), v[0].favorite, v[0].file.as_str()), ("Cliente Ejemplo", true, "ejemplo.kdbx"));
        assert_eq!(v[0].keyfile.as_deref(), Some("comun.keyx"));
        assert_eq!(v[0].tags, ["Hosting"]);
        assert_eq!((v[1].client.as_str(), v[1].keyfile.as_deref()), ("Otro Cliente", None));
    }

    #[test]
    fn views_hide_secrets_and_windows_paths() {
        let plain = br#"{"format":1,"settings":{"keepassxc":null,"default_keyfile":"C:\\K\\comun.keyx"},"vaults":[
            {"id":"1","client":"Zeta","path":"C:\\Users\\x\\OneDrive - Org\\Clientes\\zeta.kdbx","keyfile":"D:\\keys\\zeta.key","password":"secreta1"},
            {"id":"2","client":"alfa","path":"C:\\a\\alfa.kdbx","keyfile":null,"password":"secreta2","favorite":true,"tags":["Hosting"]}]}"#;
        let v = views(&Data::parse(plain).unwrap());
        assert_eq!(v[0].client, "alfa", "favoritos primero");
        assert_eq!((v[1].file.as_str(), v[1].keyfile.as_deref()), ("zeta.kdbx", Some("zeta.key")));
        assert_eq!(v[0].keyfile, None);
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("secreta") && !json.contains("OneDrive - Org"), "ni contraseñas ni rutas completas");
    }
}
