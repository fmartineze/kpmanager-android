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

export interface Vault {
  id: string;
  client: string;
  tags: string[];
  favorite: boolean;
  file: string;
  keyfile: string | null;
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

/** Evento `login`: vuelta del navegador tras iniciar sesión con Microsoft. */
export interface LoginEvent {
  account: Account | null;
  error: unknown;
}

export const api = {
  status: () => invoke<Status>("status"),
  lock: () => invoke<void>("lock"),
  reset: () => invoke<void>("reset"),
  /** Sin argumentos: volver a iniciar sesión con la app desbloqueada. */
  login: (tenant?: string, clientId?: string) => invoke<void>("login", { tenant, clientId }),
  children: (id: string) => invoke<Item[]>("wizard_children", { id }),
  findData: (folderId: string) => invoke<Item[]>("wizard_find", { folderId }),
  finish: (folderId: string, dataId: string, password: string) => invoke<Vault[]>("wizard_finish", { folderId, dataId, password }),
  unlock: (password: string) => invoke<Vault[]>("unlock", { password }),
  info: () => invoke<Info>("info"),
  sync: () => invoke<SyncResult>("sync"),
};

export const errMsg = (e: unknown) => errorText(e);

/** ¿Es este error un fallo de red (para seguir con la copia local sin alarmar)? */
export const isNetwork = (e: unknown) => !!e && typeof e === "object" && (e as { code?: string }).code === "network";
