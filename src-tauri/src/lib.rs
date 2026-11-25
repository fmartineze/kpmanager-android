//! KP Manager para Android: consulta de solo lectura.
//!
//! Lee el mismo fichero de datos que la app de Windows (`vaults.enc`) desde OneDrive y nunca lo modifica.
//! Los secretos (tokens de Microsoft, claves derivadas, contraseñas de los vaults) no salen de Rust.

mod bio;
mod config;
mod crypto;
mod docs;
mod error;
mod graph;
mod library;
mod model;
mod vaultdb;

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

/// Descifra el fichero de datos con la maestra (y devuelve la clave, para próximas descargas).
fn open_data(password: &str, blob: &[u8]) -> Result<(Keys, Data)> {
    let (keys, plain) = Keys::decrypt(password, blob)?;
    Ok((keys, Data::parse(&plain)?))
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
    /// Fichero de datos descifrado: lleva las contraseñas de los vaults y nunca sale de Rust.
    data: Data,
    tokens: Option<Tokens>,
}

impl Session {
    fn vaults(&self) -> Vec<VaultView> {
        with_keys(&views(&self.data), &self.config.library)
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
    /// Vault abierto en el navegador (uno a la vez, solo en memoria).
    vault: vaultdb::Session,
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

/// Bloquea: suelta la configuración, el fichero de datos y el vault abierto.
///
/// `clear_clipboard`: vaciar también lo copiado. No se hace al bloquear por salir de la app, que es justo
/// cuando el usuario va a pegarlo en otra; en ese caso se borra solo pasado su tiempo.
#[tauri::command]
async fn lock(app: AppHandle, state: State<'_, AppState>, clear_clipboard: bool) -> Result<()> {
    state.inner.lock().await.session = None;
    state.vault.close();
    if clear_clipboard {
        let _ = blocking(move || {
            docs::clear_clipboard(&app);
            Ok(())
        })
        .await;
    }
    Ok(())
}

/// Borra la configuración de este teléfono y vuelve al asistente.
#[tauri::command]
async fn reset(state: State<'_, AppState>) -> Result<()> {
    *state.inner.lock().await = Inner::default();
    state.vault.close();
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

/// Token para navegar por OneDrive: el del asistente o, con la app desbloqueada, el de la sesión.
async fn browse_token(state: &AppState) -> Result<String> {
    {
        let inner = state.inner.lock().await;
        if inner.session.is_none() {
            return wizard_token(&inner);
        }
    }
    session_token(state).await
}

/// Subcarpetas de una carpeta de OneDrive (asistente y Ajustes → Carpeta de origen).
#[tauri::command]
async fn wizard_children(state: State<'_, AppState>, id: String) -> Result<Vec<Item>> {
    let token = browse_token(&state).await?;
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
    let (data_keys, data, cfg_keys) = {
        let blob = blob.clone();
        blocking(move || {
            let (data_keys, data) = open_data(&password, &blob)?;
            Ok((data_keys, data, Paths::new_keys(&password)?))
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
        vault_files: Default::default(),
    };
    state.paths.write_data(&blob)?;
    state.paths.save(&cfg_keys, &config)?;
    let s = Session { cfg_keys, config, data_keys, data, tokens: Some(tokens) };
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
    unlock_with(&state, Zeroizing::new(password)).await
}

async fn unlock_with(state: &AppState, password: Zeroizing<String>) -> Result<Vec<VaultView>> {
    let paths = state.paths.clone();
    let (cfg_keys, config, data_keys, data) = blocking(move || {
        let (cfg_keys, config) = paths.load(&password)?;
        let blob = paths.read_data()?;
        // La maestra abre la configuración pero no los datos: se cambió en Windows después de configurar el móvil.
        let (data_keys, data) = open_data(&password, &blob).map_err(|e| match e {
            AppError::WrongPassword => AppError::msg("masterChanged"),
            e => e,
        })?;
        Ok((cfg_keys, config, data_keys, data))
    })
    .await?;
    let s = Session { cfg_keys, config, data_keys, data, tokens: None };
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

fn info_of(s: &Session) -> Info {
    Info {
        account: s.config.account.clone(),
        folder: s.config.folder.path.clone(),
        data_file: s.config.data_file.path.clone(),
        synced_at: s.config.synced_at,
    }
}

#[tauri::command]
async fn info(state: State<'_, AppState>) -> Result<Info> {
    let inner = state.inner.lock().await;
    Ok(info_of(inner.session.as_ref().ok_or(AppError::Locked)?))
}

/// Cambia la carpeta de origen (donde están los vaults de los clientes). No cambia el fichero de datos.
#[tauri::command]
async fn set_folder(state: State<'_, AppState>, folder_id: String) -> Result<Info> {
    let token = session_token(&state).await?;
    let folder = graph::item(&state.http, &token, &folder_id).await?;
    if !folder.folder || folder.id == "root" {
        return Err(AppError::msg("itemInvalid"));
    }
    let mut inner = state.inner.lock().await;
    let s = inner.session.as_mut().ok_or(AppError::Locked)?;
    s.config.folder = folder;
    s.save(&state.paths)?;
    Ok(info_of(s))
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
            s.data = Data::parse(&plain)?;
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
        if !s.data.vaults.iter().any(|v| v.id == vault_id) {
            return Err(AppError::msg("vaultUnknown"));
        }
    }
    edit_library(&state, None, |lib| lib.assign(&vault_id, &choice)).await
}

// ── Navegar un vault (solo lectura) ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Opened {
    tree: vaultdb::TreeView,
    /// Sin conexión: se abrió la copia guardada en el móvil.
    offline: bool,
}

/// Descarga el `.kdbx` (o usa la copia local si no cambió en OneDrive) y recuerda dónde está.
async fn fetch_kdbx(state: &AppState, vault_id: &str, windows_path: &str, folder: &Item, known: Option<&Item>) -> Result<Vec<u8>> {
    let token = session_token(state).await?;
    let meta = match known {
        Some(k) => match graph::item(&state.http, &token, &k.id).await {
            Ok(m) => Some(m),
            Err(AppError::Msg { code: "itemMissing", .. }) => None,
            Err(e) => return Err(e),
        },
        None => None,
    };
    let meta = match meta {
        Some(m) => m,
        None => graph::locate_vault(&state.http, &token, folder, windows_path).await?,
    };
    if let Some(Ok(b)) = known.filter(|k| k.id == meta.id && k.etag == meta.etag).map(|k| state.paths.read_kdbx(&k.id)) {
        return Ok(b);
    }
    let bytes = graph::download(&state.http, &token, &meta.id).await?;
    state.paths.write_kdbx(&meta.id, &bytes)?;
    if let Some(k) = known.filter(|k| k.id != meta.id) {
        state.paths.remove_kdbx(&k.id);
    }
    let mut inner = state.inner.lock().await;
    let s = inner.session.as_mut().ok_or(AppError::Locked)?;
    s.config.vault_files.insert(vault_id.to_owned(), meta);
    s.save(&state.paths)?;
    Ok(bytes)
}

/// Abre un vault en solo lectura con la contraseña guardada en el fichero de datos y el fichero de clave de la
/// biblioteca. Devuelve el árbol **sin secretos**.
#[tauri::command]
async fn open_vault(state: State<'_, AppState>, vault_id: String) -> Result<Opened> {
    state.vault.close();
    let (path, password, keyfile, folder, known) = {
        let inner = state.inner.lock().await;
        let s = inner.session.as_ref().ok_or(AppError::Locked)?;
        let v = s.data.vaults.iter().find(|v| v.id == vault_id).ok_or_else(|| AppError::msg("vaultUnknown"))?;
        let windows_key = v.keyfile.as_deref().map(file_name);
        let key = s.config.library.resolve(&v.id, windows_key.as_deref());
        if key.missing {
            return Err(AppError::msg_arg("vaultKeyMissing", windows_key.unwrap_or_default()));
        }
        let keyfile = key.id.as_deref().map(|id| s.config.library.key_bytes(id)).transpose()?;
        (v.path.clone(), v.password.clone(), keyfile, s.config.folder.clone(), s.config.vault_files.get(&vault_id).cloned())
    };
    let (bytes, offline) = match fetch_kdbx(&state, &vault_id, &path, &folder, known.as_ref()).await {
        Ok(b) => (b, false),
        // Sin conexión: la última copia que se abrió en el móvil, si la hay.
        Err(AppError::Network) => match known.as_ref().and_then(|k| state.paths.read_kdbx(&k.id).ok()) {
            Some(b) => (b, true),
            None => return Err(AppError::msg("vaultOffline")),
        },
        Err(e) => return Err(e),
    };
    let db = blocking(move || vaultdb::open(&bytes, password.as_deref().map(|p| p.as_str()), keyfile.as_deref().map(|k| k.as_slice()))).await?;
    let tree = vaultdb::tree(&db, true);
    if state.inner.lock().await.session.is_none() {
        // Se bloqueó mientras se abría.
        return Err(AppError::Locked);
    }
    state.vault.set(vault_id, db);
    Ok(Opened { tree, offline })
}

#[tauri::command]
fn close_vault(state: State<'_, AppState>) {
    state.vault.close();
}

#[tauri::command]
fn entry_detail(state: State<'_, AppState>, vault_id: String, entry: String) -> Result<vaultdb::EntryDetail> {
    state.vault.with(&vault_id, |db| vaultdb::detail(db, &entry))
}

#[tauri::command]
fn entry_totp(state: State<'_, AppState>, vault_id: String, entry: String) -> Result<vaultdb::TotpView> {
    state.vault.with(&vault_id, |db| vaultdb::totp(db, &entry))
}

/// Muestra un campo protegido (botón «ver»). Es lo único que envía un secreto a la interfaz.
#[tauri::command]
fn reveal_secret(state: State<'_, AppState>, vault_id: String, entry: String, field: String) -> Result<String> {
    state.vault.with(&vault_id, |db| vaultdb::secret(db, &entry, &field)).map(|s| s.to_string())
}

/// Tiempo que lo copiado se queda en el portapapeles.
const CLIPBOARD_MS: u64 = 30_000;

/// Copia un campo al portapapeles desde Rust: el valor no pasa por la interfaz.
#[tauri::command]
async fn copy_secret(app: AppHandle, state: State<'_, AppState>, vault_id: String, entry: String, field: String) -> Result<()> {
    let text = state.vault.with(&vault_id, |db| vaultdb::secret(db, &entry, &field))?;
    blocking(move || docs::copy(&app, &text, CLIPBOARD_MS)).await
}

/// Guarda un adjunto donde elija el usuario. Devuelve el nombre, o nada si canceló.
#[tauri::command]
async fn save_attachment(app: AppHandle, state: State<'_, AppState>, vault_id: String, entry: String, name: String) -> Result<Option<String>> {
    let data = state.vault.with(&vault_id, |db| vaultdb::attachment(db, &entry, &name))?;
    if data.len() > MAX_ATTACHMENT {
        return Err(AppError::msg("docTooLarge"));
    }
    let suggested = docs::clean_name(&name);
    blocking(move || docs::save(&app, &suggested, "application/octet-stream", &data)).await
}

/// Los adjuntos viajan en base64 hasta el selector de Android: se limita el tamaño.
const MAX_ATTACHMENT: usize = 16 * 1024 * 1024;

/// Abre un enlace de una entrada en el navegador. Solo `https`, `http` y `mailto` (KeePass admite `cmd://`…).
#[tauri::command]
fn open_link(app: AppHandle, url: String) -> Result<()> {
    let parsed = url::Url::parse(url.trim()).map_err(|_| AppError::msg("linkBlocked"))?;
    if !matches!(parsed.scheme(), "https" | "http" | "mailto") {
        return Err(AppError::msg("linkBlocked"));
    }
    app.opener().open_url(parsed.as_str(), None::<&str>).map_err(|_| AppError::msg("browserFailed"))
}

// ── Desbloqueo con huella ────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct BioStatus {
    available: bool,
    enabled: bool,
}

#[tauri::command]
async fn bio_status(app: AppHandle, state: State<'_, AppState>) -> Result<BioStatus> {
    let enabled = state.paths.bio_enabled();
    let available = blocking(move || Ok(bio::available(&app))).await?;
    Ok(BioStatus { available, enabled })
}

/// Activa la huella: comprueba la maestra y la guarda cifrada con la clave protegida por huella.
#[tauri::command]
async fn bio_enable(app: AppHandle, state: State<'_, AppState>, password: String, title: String, cancel: String) -> Result<bool> {
    if state.inner.lock().await.session.is_none() {
        return Err(AppError::Locked);
    }
    let password = Zeroizing::new(password);
    let paths = state.paths.clone();
    let sealed = blocking(move || {
        // Debe ser la maestra con la que se abre este móvil.
        paths.load(&password)?;
        bio::enroll(&app, &password, &bio::Prompt { title, cancel })
    })
    .await?;
    match sealed {
        Some(s) => {
            state.paths.write_bio(&s)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[tauri::command]
async fn bio_disable(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    state.paths.remove_bio();
    blocking(move || {
        bio::remove(&app);
        Ok(())
    })
    .await
}

/// Desbloquea con la huella. `None` si el usuario canceló (entonces escribe la maestra).
#[tauri::command]
async fn bio_unlock(app: AppHandle, state: State<'_, AppState>, title: String, cancel: String) -> Result<Option<Vec<VaultView>>> {
    let sealed = state.paths.read_bio().map_err(|_| AppError::msg("bioInvalidated"))?;
    let password = match blocking(move || bio::decrypt(&app, &sealed, &bio::Prompt { title, cancel })).await {
        Ok(Some(p)) => p,
        Ok(None) => return Ok(None),
        Err(e) => {
            // Huellas cambiadas: la clave ya no existe; hay que volver a activarla con la maestra.
            if matches!(e, AppError::Msg { code: "bioInvalidated", .. }) {
                state.paths.remove_bio();
            }
            return Err(e);
        }
    };
    match unlock_with(&state, password).await {
        Ok(v) => Ok(Some(v)),
        Err(AppError::WrongPassword) => {
            state.paths.remove_bio();
            Err(AppError::msg("bioInvalidated"))
        }
        Err(e) => Err(e),
    }
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
        .plugin(bio::init())
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
                vault: vaultdb::Session::default(),
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
            set_folder,
            sync,
            keys,
            key_import,
            key_delete,
            key_default,
            vault_key,
            backup_save,
            open_vault,
            close_vault,
            entry_detail,
            entry_totp,
            reveal_secret,
            copy_secret,
            save_attachment,
            open_link,
            bio_status,
            bio_enable,
            bio_disable,
            bio_unlock,
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
        let v = views(&open_data("prueba-compat", blob).unwrap().1);
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
