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
  lock: () => invoke<void>("lock"),
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
  info: () => invoke<Info>("info"),
  sync: () => invoke<SyncResult>("sync"),
  keys: () => invoke<Library>("keys"),
  keyImport: () => withPicker(() => invoke<Library>("key_import")),
  keyDelete: (id: string) => invoke<Library>("key_delete", { id }),
  keyDefault: (id: string | null) => invoke<Library>("key_default", { id }),
  /** `choice`: `auto`, `none` o el id de un fichero de la biblioteca. */
  vaultKey: (vaultId: string, choice: string) => invoke<Library>("vault_key", { vaultId, choice }),
  backupSave: () => withPicker(() => invoke<string | null>("backup_save")),
};

export const errMsg = (e: unknown) => errorText(e);
export const errCode = (e: unknown) => (e && typeof e === "object" ? (e as { code?: string }).code : undefined);

/** ¿Es este error un fallo de red (para seguir con la copia local sin alarmar)? */
export const isNetwork = (e: unknown) => errCode(e) === "network";
