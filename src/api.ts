import { invoke } from "@tauri-apps/api/core";
import { errorText } from "./i18n";

export interface Status {
  configured: boolean;
  unlocked: boolean;
  version: string;
}

export interface Account {
  name: string;
  upn: string;
}

/** Carpeta o fichero de OneDrive. */
export interface Item {
  id: string;
  name: string;
  folder: boolean;
  path: string;
  etag: string;
}

/** Fichero de clave que usará un vault en este móvil. */
export interface KeyState {
  /** `auto`: por nombre o el predeterminado; `manual`: elegido a mano; `none`: sin fichero de clave. */
  mode: "auto" | "manual" | "none";
  id: string | null;
  name: string | null;
  by: "name" | "default" | null;
  /** En Windows usa fichero de clave, pero en este móvil no hay ninguno que usar. */
  missing: boolean;
}

export interface Vault {
  id: string;
  client: string;
  tags: string[];
  favorite: boolean;
  file: string;
  /** Nombre del fichero de clave que usa en Windows. */
  keyfile: string | null;
  key: KeyState;
  /** Último uso (segundos Unix), en Windows o en este móvil. */
  lastUsed: number | null;
}

/** Fichero de la biblioteca (el contenido nunca llega a la interfaz). */
export interface KeyFile {
  id: string;
  name: string;
  size: number;
  fingerprint: string;
  isDefault: boolean;
  added: number;
  uses: number;
}

export interface BioStatus {
  /** El móvil tiene huella de seguridad alta configurada. */
  available: boolean;
  /** El desbloqueo con huella está activado en la app. */
  enabled: boolean;
}

/** Copias de vaults guardadas en el móvil para abrirlos sin conexión. */
export interface CacheInfo {
  count: number;
  bytes: number;
  /** Días sin abrir tras los que se borra una copia (0 = nunca). */
  days: number;
}

/** Versión nueva publicada en GitHub. */
export interface Release {
  version: string;
  url: string;
}

export interface Library {
  added: string | null;
  keys: KeyFile[];
  vaults: Vault[];
}

export interface Info {
  account: Account;
  folder: string;
  dataFile: string;
  syncedAt: number | null;
}

export interface SyncResult {
  changed: boolean;
  vaults: Vault[];
  syncedAt: number | null;
}

/** Copia de seguridad abierta en el asistente. */
export interface RestoreInfo {
  tenant: string;
  clientId: string;
  account: Account;
  folder: Item;
  dataFile: Item;
  keys: number;
  created: number;
}

// ── Navegar un vault (mismos tipos que la app de Windows) ──

export type IconView = { kind: "std"; id: number } | { kind: "custom"; id: string };

export interface GroupNode {
  id: string;
  parent: string | null;
  name: string;
  icon: IconView | null;
}

export interface EntryRow {
  id: string;
  group: string;
  title: string;
  username: string;
  url: string;
  icon: IconView | null;
  tags: string[];
  hasTotp: boolean;
  hasAttachments: boolean;
}

/** El árbol del vault: nunca lleva contraseñas, notas ni semillas TOTP. */
export interface VaultTree {
  root: string;
  groups: GroupNode[];
  entries: EntryRow[];
  /** Iconos personalizados ya validados, como `data:image/png;base64,…`. */
  icons: Record<string, string>;
}

export interface Opened {
  tree: VaultTree;
  /** Sin conexión: es la copia guardada en el móvil. */
  offline: boolean;
}

export interface FieldInfo {
  name: string;
  protected: boolean;
  value: string | null;
}

export interface EntryDetail {
  id: string;
  title: string;
  username: string;
  url: string;
  notes: string;
  tags: string[];
  hasPassword: boolean;
  hasTotp: boolean;
  fields: FieldInfo[];
  attachments: { name: string; size: number }[];
}

export interface Totp {
  code: string;
  validFor: number;
  period: number;
}

/** Evento `login`: vuelta del navegador tras iniciar sesión con Microsoft. */
export interface LoginEvent {
  account: Account | null;
  error: unknown;
}

// El selector de ficheros de Android pasa la app a segundo plano: mientras está abierto no se bloquea por eso.
let pickers = 0;
export const pickerOpen = () => pickers > 0;
async function withPicker<T>(f: () => Promise<T>): Promise<T> {
  pickers++;
  try {
    return await f();
  } finally {
    pickers--;
  }
}

export const api = {
  status: () => invoke<Status>("status"),
  /** `clearClipboard`: vaciar también lo copiado (no al bloquear por salir de la app). */
  lock: (clearClipboard: boolean) => invoke<void>("lock", { clearClipboard }),
  reset: () => invoke<void>("reset"),
  /** Sin argumentos: volver a iniciar sesión con la app desbloqueada. */
  login: (tenant?: string, clientId?: string) => invoke<void>("login", { tenant, clientId }),
  children: (id: string) => invoke<Item[]>("wizard_children", { id }),
  findData: (folderId: string) => invoke<Item[]>("wizard_find", { folderId }),
  /** `password` nulo al restaurar una copia (la maestra se escribió al abrirla). */
  finish: (folderId: string, dataId: string, password: string | null) => invoke<Vault[]>("wizard_finish", { folderId, dataId, password }),
  restorePick: () => withPicker(() => invoke<string | null>("wizard_restore_pick")),
  restoreOpen: (password: string) => invoke<RestoreInfo>("wizard_restore_open", { password }),
  unlock: (password: string) => invoke<Vault[]>("unlock", { password }),
  /** Completa el cambio de maestra con la contraseña que faltaba (la nueva o la anterior). */
  rekey: (password: string) => invoke<Vault[]>("rekey", { password }),
  /** Versión más nueva publicada, si la hay (la consulta la hace Rust). */
  checkUpdate: () => invoke<Release | null>("check_update"),
  info: () => invoke<Info>("info"),
  /** Cambia la carpeta de origen (los vaults); no cambia el fichero de datos. */
  setFolder: (folderId: string) => invoke<Info>("set_folder", { folderId }),
  sync: () => invoke<SyncResult>("sync"),
  keys: () => invoke<Library>("keys"),
  keyImport: () => withPicker(() => invoke<Library>("key_import")),
  keyDelete: (id: string) => invoke<Library>("key_delete", { id }),
  keyDefault: (id: string | null) => invoke<Library>("key_default", { id }),
  /** `choice`: `auto`, `none` o el id de un fichero de la biblioteca. */
  vaultKey: (vaultId: string, choice: string) => invoke<Library>("vault_key", { vaultId, choice }),
  backupSave: () => withPicker(() => invoke<string | null>("backup_save")),
  openVault: (vaultId: string) => invoke<Opened>("open_vault", { vaultId }),
  closeVault: () => invoke<void>("close_vault"),
  entryDetail: (vaultId: string, entry: string) => invoke<EntryDetail>("entry_detail", { vaultId, entry }),
  entryTotp: (vaultId: string, entry: string) => invoke<Totp>("entry_totp", { vaultId, entry }),
  /** Devuelve el valor para mostrarlo (botón «ver»). */
  revealSecret: (vaultId: string, entry: string, field: string) => invoke<string>("reveal_secret", { vaultId, entry, field }),
  /** Copia desde Rust: el valor no pasa por la interfaz. */
  copySecret: (vaultId: string, entry: string, field: string) => invoke<void>("copy_secret", { vaultId, entry, field }),
  saveAttachment: (vaultId: string, entry: string, name: string) => withPicker(() => invoke<string | null>("save_attachment", { vaultId, entry, name })),
  openLink: (url: string) => invoke<void>("open_link", { url }),
  /** Menú Compartir de Android con la entrada en texto (lo construye Rust). */
  shareEntry: (vaultId: string, entry: string, includeCustom: boolean, includeNotes: boolean, lang: string, title: string) =>
    invoke<void>("share_entry", { vaultId, entry, includeCustom, includeNotes, lang, title }),
  cacheInfo: () => invoke<CacheInfo>("cache_info"),
  cacheClear: () => invoke<CacheInfo>("cache_clear"),
  setCacheDays: (days: number) => invoke<CacheInfo>("set_cache_days", { days }),
  bioStatus: () => invoke<BioStatus>("bio_status"),
  /** Pide la huella para guardar la maestra cifrada. `false` si se canceló. */
  bioEnable: (password: string, title: string, cancel: string) => invoke<boolean>("bio_enable", { password, title, cancel }),
  bioDisable: () => invoke<void>("bio_disable"),
  /** `null` si el usuario canceló el diálogo de la huella. */
  bioUnlock: (title: string, cancel: string) => invoke<Vault[] | null>("bio_unlock", { title, cancel }),
};

/** Segundos que lo copiado se queda en el portapapeles (igual que `CLIPBOARD_MS` en Rust). */
export const CLIPBOARD_SECS = 30;

export const errMsg = (e: unknown) => errorText(e);
export const errCode = (e: unknown) => (e && typeof e === "object" ? (e as { code?: string }).code : undefined);

/** ¿Es este error un fallo de red (para seguir con la copia local sin alarmar)? */
export const isNetwork = (e: unknown) => errCode(e) === "network";
