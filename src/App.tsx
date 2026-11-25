import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, BioStatus, CacheInfo, errCode, errMsg, Info, isNetwork, KeyFile, KeyState, Library, LoginEvent, pickerOpen, Status, Vault, VaultTree } from "./api";
import { rich, useT } from "./i18n";
import { setThemePref, ThemePref } from "./theme";
import { Wizard } from "./components/Wizard";
import { KeysModal, VaultSheet } from "./components/Keys";
import { VaultBrowser } from "./components/VaultBrowser";
import { FolderPicker } from "./components/FolderPicker";
import { Modal } from "./components/Modal";
import { Alert, Note, Segmented } from "./components/ui";
import { IconAlert, IconCheck, IconDownload, IconEdit, IconFingerprint, IconKey, IconTrash, IconLock, IconRefresh, IconSearch, IconSettings, IconShield, IconStar, IconX } from "./components/Icons";

type Phase = "loading" | "wizard" | "locked" | "ready";
type Toast = { msg: string; kind: "ok" | "error"; key: number } | null;

/** Sin actividad durante este tiempo, se bloquea. */
const AUTO_LOCK_MS = 3 * 60 * 1000;
/**
 * En segundo plano se bloquea pasado este margen: da tiempo a pegar en otra app lo copiado y volver sin
 * escribir otra vez la maestra.
 */
const BACKGROUND_GRACE_MS = 30 * 1000;
/** Errores al abrir un vault que se arreglan eligiendo otro fichero de clave. */
const KEY_ERRORS = ["vaultKeyMissing", "vaultWrongKey", "vaultKeyfile", "vaultNoCredentials"];

const fmtBytes = (n: number) => (n < 1024 * 1024 ? `${Math.max(1, Math.round(n / 1024))} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`);
const initial = (s: string) => (s.trim()[0] ?? "?").toUpperCase();
const hue = (s: string) => [...s].reduce((a, c) => (a * 31 + c.charCodeAt(0)) >>> 0, 7) % 6;
const norm = (s: string) => s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();

/** Qué fichero de clave tocaría en automático (misma regla que en Rust), para explicarlo en la ficha. */
function autoKey(v: Vault, keys: KeyFile[]): KeyState {
  if (v.key.mode === "auto") return v.key;
  const none: KeyState = { mode: "auto", id: null, name: null, by: null, missing: false };
  if (!v.keyfile) return none;
  const byName = keys.find((k) => k.name.toLowerCase() === v.keyfile!.toLowerCase());
  const k = byName ?? keys.find((x) => x.isDefault);
  return k ? { ...none, id: k.id, name: k.name, by: byName ? "name" : "default" } : { ...none, missing: true };
}

function Unlock({ onUnlocked }: { onUnlocked: (v: Vault[]) => void }) {
  const t = useT();
  const [pw, setPw] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [shake, setShake] = useState(false);
  const [bio, setBio] = useState(false);

  const tryBio = useCallback(async () => {
    setError("");
    try {
      const v = await api.bioUnlock(t("unlock.bioTitle"), t("unlock.bioCancel"));
      if (v) onUnlocked(v);
    } catch (e) {
      setError(errMsg(e));
      if (errCode(e) === "bioInvalidated") setBio(false);
    }
  }, [onUnlocked, t]);

  // Con la huella activada se pide sola al aparecer (o al volver a la app si se bloqueó en segundo plano).
  useEffect(() => {
    let done = false;
    const go = () => {
      if (done || document.visibilityState !== "visible") return;
      done = true;
      tryBio();
    };
    api.bioStatus().then((s) => {
      if (!s.enabled || !s.available) return;
      setBio(true);
      go();
    }).catch(() => {});
    document.addEventListener("visibilitychange", go);
    return () => document.removeEventListener("visibilitychange", go);
  }, [tryBio]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const v = await api.unlock(pw);
      setPw("");
      onUnlocked(v);
    } catch (err) {
      setError(errMsg(err));
      setShake(true);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="auth">
      <div className="shield-wrap"><div className="shield"><IconShield /></div></div>
      <span className="chip-lock"><IconLock className="s14" /> {t("unlock.chip")}</span>
      <h1>{t("unlock.title")}</h1>
      <p className="lede">{rich(t("unlock.lede"))}</p>
      <form className={`form ${shake ? "shake" : ""}`} onSubmit={submit} onAnimationEnd={() => setShake(false)}>
        <div className={`input ${error ? "is-error" : ""}`}>
          <IconKey className="s16" />
          <input className="input-el" type="password" autoComplete="off" placeholder={t("unlock.master")} value={pw} onChange={(e) => setPw(e.target.value)} />
        </div>
        {error && <Alert kind="err">{error}</Alert>}
        <button className="btn btn-primary btn-block" disabled={busy || !pw}>
          {busy && <span className="spin" />} {busy ? t("unlock.busy") : t("unlock.unlock")}
        </button>
        {bio && (
          <button type="button" className="btn btn-secondary btn-block" onClick={tryBio} disabled={busy}>
            <IconFingerprint className="s16" /> {t("unlock.bio")}
          </button>
        )}
      </form>
    </div>
  );
}

function Settings({ version, keyCount, onClose, onReset, onKeys, onFolder, toast }: {
  version: string;
  keyCount: number;
  onClose: () => void;
  onReset: () => void;
  onKeys: () => void;
  onFolder: () => void;
  toast: (m: string, k?: "ok" | "error") => void;
}) {
  const t = useT();
  const [info, setInfo] = useState<Info | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [saving, setSaving] = useState(false);
  const [bio, setBio] = useState<BioStatus | null>(null);
  const [cache, setCache] = useState<CacheInfo | null>(null);
  const [cacheConfirm, setCacheConfirm] = useState(false);
  const [bioPw, setBioPw] = useState("");
  const [bioBusy, setBioBusy] = useState(false);
  const [theme, setTheme] = useState<ThemePref>(() => {
    try {
      const v = localStorage.getItem("kp.theme");
      return v === "light" || v === "dark" ? v : "system";
    } catch {
      return "system";
    }
  });

  useEffect(() => {
    api.info().then(setInfo).catch(() => {});
    api.bioStatus().then(setBio).catch(() => {});
    api.cacheInfo().then(setCache).catch(() => {});
    const un = listen<LoginEvent>("login", ({ payload }) => {
      if (payload.account) {
        toast(t("wiz.signedAs", { name: payload.account.upn }).replace(/<\/?b>/g, ""));
        api.info().then(setInfo).catch(() => {});
      } else toast(errMsg(payload.error), "error");
    });
    return () => {
      un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const when = info?.syncedAt ? new Date(info.syncedAt * 1000).toLocaleString() : t("set.never");

  async function enableBio(e: FormEvent) {
    e.preventDefault();
    setBioBusy(true);
    try {
      if (await api.bioEnable(bioPw, t("set.bioTitle"), t("common.cancel"))) {
        setBio({ available: true, enabled: true });
        toast(t("set.bioEnabled"));
      }
      setBioPw("");
    } catch (err) {
      toast(errMsg(err), "error");
    } finally {
      setBioBusy(false);
    }
  }

  async function disableBio() {
    await api.bioDisable().catch(() => {});
    setBio((b) => (b ? { ...b, enabled: false } : b));
  }

  async function backup() {
    setSaving(true);
    try {
      const name = await api.backupSave();
      if (name) toast(t("set.backupSaved", { name }));
    } catch (e) {
      toast(errMsg(e), "error");
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal title={t("set.title")} icon={<IconSettings className="s16" />} onClose={onClose}>
      <div className="section">
        <div className="kv"><span>{t("set.account")}</span><b>{info?.account.upn ?? "…"}</b></div>
        <div className="kv-edit">
          <div className="kv"><span>{t("set.folder")}</span><b>{info?.folder ?? "…"}</b></div>
          <button className="btn btn-icon plain" onClick={onFolder} aria-label={t("set.folderChange")}><IconEdit className="s16" /></button>
        </div>
        <div className="kv"><span>{t("set.data")}</span><b>{info?.dataFile ?? "…"}</b></div>
        <div className="kv"><span>{t("set.synced")}</span><b>{when}</b></div>
        <div className="btns">
          <button className="btn btn-secondary btn-sm" onClick={() => api.login().catch((e) => toast(errMsg(e), "error"))}>{t("set.relogin")}</button>
        </div>
      </div>
      <div className="section">
        <div className="section-title">{t("set.bio")}</div>
        {!bio ? null : !bio.available ? (
          <Note>{t("set.bioUnavailable")}</Note>
        ) : bio.enabled ? (
          <>
            <div className="kv"><span>{t("set.bio")}</span><b>{t("set.bioOn")}</b></div>
            <div className="btns">
              <button className="btn btn-secondary btn-sm" onClick={disableBio}>{t("set.bioDisable")}</button>
            </div>
          </>
        ) : (
          <form className="section" onSubmit={enableBio}>
            <Note>{t("set.bioHelp")}</Note>
            <div className="input">
              <IconKey className="s16" />
              <input className="input-el" type="password" autoComplete="off" placeholder={t("set.bioMaster")} value={bioPw} onChange={(e) => setBioPw(e.target.value)} />
            </div>
            <div className="btns">
              <button className="btn btn-secondary btn-sm" disabled={!bioPw || bioBusy}>
                {bioBusy ? <span className="spin" /> : <IconFingerprint className="s14" />} {t("set.bioEnable")}
              </button>
            </div>
          </form>
        )}
      </div>
      <div className="section">
        <div className="section-title">{t("set.cache")}</div>
        <div className="kv">
          <span>{t("set.cacheStored")}</span>
          <b>{cache ? (cache.count === 0 ? t("set.cacheNone") : t("set.cacheCount", { n: cache.count, size: fmtBytes(cache.bytes) })) : "…"}</b>
        </div>
        <div className="label">{t("set.cacheDays")}</div>
        <Segmented
          label={t("set.cacheDays")}
          value={String(cache?.days ?? 30)}
          onChange={(v) => api.setCacheDays(Number(v)).then(setCache).catch((e) => toast(errMsg(e), "error"))}
          options={[
            { value: "7", label: t("set.cacheDaysN", { n: 7 }) },
            { value: "30", label: t("set.cacheDaysN", { n: 30 }) },
            { value: "90", label: t("set.cacheDaysN", { n: 90 }) },
            { value: "0", label: t("set.cacheNever") },
          ]}
        />
        <Note>{t("set.cacheHelp")}</Note>
        <div className="btns">
          {cacheConfirm ? (
            <>
              <button className="btn btn-danger-solid btn-sm" onClick={async () => {
                setCacheConfirm(false);
                try {
                  setCache(await api.cacheClear());
                  toast(t("set.cacheCleared"));
                } catch (e) {
                  toast(errMsg(e), "error");
                }
              }}>{t("set.cacheClearConfirm")}</button>
              <button className="btn btn-ghost btn-sm" onClick={() => setCacheConfirm(false)}>{t("common.cancel")}</button>
            </>
          ) : (
            <button className="btn btn-danger btn-sm" disabled={!cache || cache.count === 0} onClick={() => setCacheConfirm(true)}>
              <IconTrash className="s14" /> {t("set.cacheClear")}
            </button>
          )}
        </div>
      </div>
      <div className="section">
        <div className="section-title">{t("set.keys")}</div>
        <div className="kv"><span>{t("set.keysHere")}</span><b>{keyCount}</b></div>
        <div className="btns">
          <button className="btn btn-secondary btn-sm" onClick={onKeys}><IconKey className="s14" /> {t("set.keysManage")}</button>
        </div>
      </div>
      <div className="section">
        <div className="section-title">{t("set.backup")}</div>
        <Note>{t("set.backupHelp")}</Note>
        <div className="btns">
          <button className="btn btn-secondary btn-sm" onClick={backup} disabled={saving}>
            {saving ? <span className="spin" /> : <IconDownload className="s14" />} {t("set.backupSave")}
          </button>
        </div>
      </div>
      <div className="section">
        <div className="section-title">{t("set.appearance")}</div>
        <Segmented
          label={t("set.appearance")}
          value={theme}
          onChange={(v) => {
            setTheme(v);
            setThemePref(v);
          }}
          options={[
            { value: "system", label: t("set.themeSystem") },
            { value: "light", label: t("set.themeLight") },
            { value: "dark", label: t("set.themeDark") },
          ]}
        />
      </div>
      <div className="section">
        <div className="section-title">{t("set.reset")}</div>
        <Note>{t("set.resetHelp")}</Note>
        <div className="btns">
          {confirm ? (
            <>
              <button className="btn btn-danger-solid btn-sm" onClick={onReset}>{t("set.resetConfirm")}</button>
              <button className="btn btn-ghost btn-sm" onClick={() => setConfirm(false)}>{t("common.cancel")}</button>
            </>
          ) : (
            <button className="btn btn-danger btn-sm" onClick={() => setConfirm(true)}>{t("set.reset")}</button>
          )}
        </div>
        <div className="help">{t("set.version", { v: version })}</div>
      </div>
    </Modal>
  );
}

export default function App() {
  const t = useT();
  const [phase, setPhase] = useState<Phase>("loading");
  const [status, setStatus] = useState<Status | null>(null);
  const [vaults, setVaults] = useState<Vault[]>([]);
  const [query, setQuery] = useState("");
  const [tag, setTag] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [settings, setSettings] = useState(false);
  const [toast, setToast] = useState<Toast>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [keys, setKeys] = useState<KeyFile[]>([]);
  const [keysOpen, setKeysOpen] = useState(false);
  const [folderOpen, setFolderOpen] = useState(false);
  const [libBusy, setLibBusy] = useState(false);
  const [browsing, setBrowsing] = useState<{ vault: Vault; tree: VaultTree; offline: boolean } | null>(null);
  const [opening, setOpening] = useState<string | null>(null);
  const toastTimer = useRef<number>(0);

  const showToast = useCallback((msg: string, kind: "ok" | "error" = "ok") => {
    window.clearTimeout(toastTimer.current);
    setToast({ msg, kind, key: Date.now() });
    toastTimer.current = window.setTimeout(() => setToast(null), 3800);
  }, []);

  const refreshStatus = useCallback(async () => {
    const s = await api.status();
    setStatus(s);
    setPhase(!s.configured ? "wizard" : s.unlocked ? "ready" : "locked");
  }, []);

  useEffect(() => {
    refreshStatus().catch(() => setPhase("wizard"));
  }, [refreshStatus]);

  /** `clearClipboard`: no al bloquear por salir de la app (lo copiado es para pegarlo en otra). */
  const lock = useCallback(async (clearClipboard = true) => {
    await api.lock(clearClipboard).catch(() => {});
    setBrowsing(null);
    setOpening(null);
    setVaults([]);
    setQuery("");
    setTag(null);
    setSettings(false);
    setSelected(null);
    setKeys([]);
    setKeysOpen(false);
    setFolderOpen(false);
    setPhase((p) => (p === "ready" ? "locked" : p));
  }, []);

  // En segundo plano se bloquea pasado el margen (y se descarta la lista). Durante el asistente no: hay que ir al
  // navegador. Con el selector de ficheros abierto tampoco: también pasa la app a segundo plano.
  useEffect(() => {
    if (phase !== "ready") return;
    let hiddenAt = 0;
    let bgTimer = 0;
    const onVisibility = () => {
      if (document.visibilityState === "hidden") {
        if (pickerOpen()) return;
        hiddenAt = Date.now();
        // Puede no dispararse con la app en segundo plano: por eso también se comprueba al volver.
        bgTimer = window.setTimeout(() => lock(false), BACKGROUND_GRACE_MS);
      } else {
        window.clearTimeout(bgTimer);
        if (hiddenAt && Date.now() - hiddenAt >= BACKGROUND_GRACE_MS) lock(false);
        hiddenAt = 0;
      }
    };
    document.addEventListener("visibilitychange", onVisibility);
    let timer = window.setTimeout(lock, AUTO_LOCK_MS);
    const reset = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(lock, AUTO_LOCK_MS);
    };
    const events = ["pointerdown", "keydown", "scroll"] as const;
    events.forEach((e) => window.addEventListener(e, reset, { passive: true }));
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      window.clearTimeout(bgTimer);
      window.clearTimeout(timer);
      events.forEach((e) => window.removeEventListener(e, reset));
    };
  }, [phase, lock]);

  const sync = useCallback(
    async (manual: boolean) => {
      setSyncing(true);
      try {
        const r = await api.sync();
        setVaults(r.vaults);
        if (r.changed) showToast(t("sync.updated"));
        else if (manual) showToast(t("sync.same"));
      } catch (e) {
        if (isNetwork(e)) {
          if (manual) showToast(t("sync.offline"), "error");
        } else showToast(errMsg(e), "error");
      } finally {
        setSyncing(false);
      }
    },
    [showToast, t],
  );

  const applyLib = (l: Library) => {
    setKeys(l.keys);
    setVaults(l.vaults);
  };

  // Acciones sobre la biblioteca: todas devuelven la biblioteca y la lista actualizadas.
  const libAction = useCallback(
    async (f: () => Promise<Library>, ok?: (l: Library) => string | null) => {
      setLibBusy(true);
      try {
        const l = await f();
        setKeys(l.keys);
        setVaults(l.vaults);
        const msg = ok?.(l);
        if (msg) showToast(msg);
      } catch (e) {
        showToast(errMsg(e), "error");
      } finally {
        setLibBusy(false);
      }
    },
    [showToast],
  );
  const importKey = () => libAction(api.keyImport, (l) => (l.added ? t("keys.added", { name: l.added }) : null));

  const openVault = async (v: Vault) => {
    if (opening) return;
    setOpening(v.id);
    try {
      const r = await api.openVault(v.id);
      setBrowsing({ vault: v, tree: r.tree, offline: r.offline });
    } catch (e) {
      showToast(errMsg(e), "error");
      // Si es cosa del fichero de clave, se abre su ficha para elegir otro.
      if (KEY_ERRORS.includes(errCode(e) ?? "")) setSelected(v.id);
    } finally {
      setOpening(null);
    }
  };

  const closeVault = () => {
    api.closeVault().catch(() => {});
    setBrowsing(null);
  };

  const unlocked = (v: Vault[]) => {
    setVaults(v);
    setPhase("ready");
    api.keys().then(applyLib).catch(() => {});
    // Busca cambios en OneDrive en segundo plano; la lista ya se ve con la copia local.
    sync(false);
  };

  const tags = useMemo(() => {
    const m = new Map<string, { label: string; n: number }>();
    vaults.forEach((v) => v.tags.forEach((x) => {
      const k = x.toLowerCase();
      m.set(k, { label: m.get(k)?.label ?? x, n: (m.get(k)?.n ?? 0) + 1 });
    }));
    return [...m.values()].sort((a, b) => a.label.localeCompare(b.label));
  }, [vaults]);

  const q = norm(query.trim());
  const filtered = vaults.filter(
    (v) => (!q || norm(v.client).includes(q) || norm(v.file).includes(q)) && (!tag || v.tags.some((x) => x.toLowerCase() === tag.toLowerCase())),
  );
  const filtering = !!q || !!tag;
  const selectedVault = vaults.find((v) => v.id === selected) ?? null;

  let body;
  if (phase === "loading") body = <div className="empty"><span className="spin lg" /></div>;
  else if (phase === "wizard") body = <Wizard onDone={(v) => { refreshStatus(); unlocked(v); }} />;
  else if (phase === "locked") body = <Unlock onUnlocked={unlocked} />;
  else if (browsing)
    body = <VaultBrowser vault={browsing.vault} tree={browsing.tree} offline={browsing.offline} onBack={closeVault} notify={showToast} />;
  else
    body = (
      <>
        <div className="head">
          <div className="meta">
            <span className="count">
              {filtering
                ? rich(t("count.of", { n: `<b>${filtered.length}</b>`, total: vaults.length }))
                : rich(t(vaults.length === 1 ? "count.client" : "count.clients", { n: `<b>${vaults.length}</b>` }))}
            </span>
            <div className="actions">
              <button className="btn btn-icon" aria-label={t("tip.sync")} onClick={() => sync(true)} disabled={syncing}>
                {syncing ? <span className="spin" /> : <IconRefresh />}
              </button>
              <button className="btn btn-icon" aria-label={t("tip.settings")} onClick={() => setSettings(true)}><IconSettings /></button>
              <button className="btn btn-icon" aria-label={t("tip.lock")} onClick={() => lock()}><IconLock /></button>
            </div>
          </div>
          <div className="search">
            <IconSearch />
            <input className="input-el" type="search" enterKeyHint="search" placeholder={t("search.placeholder")} value={query} onChange={(e) => setQuery(e.target.value)} />
            {query && <button className="clear" onClick={() => setQuery("")} aria-label={t("search.clear")}><IconX className="s14" /></button>}
          </div>
          {tags.length > 0 && (
            <div className="tagbar" role="group" aria-label={t("list.tagFilter")}>
              <button className={`tagchip ${tag ? "" : "on"}`} onClick={() => setTag(null)}>{t("list.tagAll")}</button>
              {tags.map((x) => {
                const on = tag?.toLowerCase() === x.label.toLowerCase();
                return (
                  <button key={x.label} className={`tagchip ${on ? "on" : ""}`} onClick={() => setTag(on ? null : x.label)}>
                    {x.label} <span className="n">{x.n}</span>
                  </button>
                );
              })}
            </div>
          )}
        </div>
        {vaults.length === 0 ? (
          <div className="empty"><p>{t("list.empty")}</p></div>
        ) : filtered.length === 0 ? (
          <div className="empty">
            <div className="empty-art neutral"><IconSearch /></div>
            <h2>{t("list.noResults")}</h2>
            <p>{t("list.noResultsText", { q: query.trim() })}</p>
          </div>
        ) : (
          <div className="list">
            {filtered.map((v, i) => (
              <div key={v.id} className={`row tall ${selected === v.id ? "is-focus" : ""}`}
                style={{ animation: "kp-fade-up var(--kp-dur-base) var(--kp-ease-out) backwards", animationDelay: `${Math.min(i, 12) * 22}ms` }}>
                <button className="row-main" onClick={() => openVault(v)} disabled={!!opening}>
                  <span className={`avatar av-${hue(v.client)}`}>{initial(v.client)}</span>
                  <span className="row-text">
                    <span className="row-name">{v.client}</span>
                    {v.key.missing ? (
                      <span className="row-sub warn">{t("list.keyMissing", { name: v.keyfile ?? "" })}</span>
                    ) : (
                      <span className="row-sub">{v.file}{v.key.name ? ` · ${t("list.keyfile", { name: v.key.name })}` : ""}</span>
                    )}
                  </span>
                  {v.tags.length > 0 && (
                    <span className="row-tags">
                      <span className="tag-badge">{v.tags[0]}</span>
                      {v.tags.length > 1 && <span className="tag-badge more">+{v.tags.length - 1}</span>}
                    </span>
                  )}
                  {v.favorite && <span className="row-fav"><IconStar className="s14" /></span>}
                  {opening === v.id && <span className="spin" aria-label={t("vault.opening", { name: v.client })} />}
                </button>
                <button className={`btn btn-icon plain ${v.key.missing ? "warn" : ""}`} onClick={() => setSelected(v.id)} aria-label={t("vault.keySettings")}>
                  <IconKey className="s16" />
                </button>
              </div>
            ))}
          </div>
        )}
        {selectedVault && (
          <VaultSheet
            vault={selectedVault}
            keys={keys}
            auto={autoKey(selectedVault, keys)}
            busy={libBusy}
            onClose={() => setSelected(null)}
            onChoose={(choice) => libAction(() => api.vaultKey(selectedVault.id, choice))}
            onImport={importKey}
            onOpen={() => {
              const v = selectedVault;
              setSelected(null);
              openVault(v);
            }}
          />
        )}
        {folderOpen && (
          <FolderPicker
            onClose={() => setFolderOpen(false)}
            onPick={async (f) => {
              try {
                const info = await api.setFolder(f.id);
                setFolderOpen(false);
                showToast(t("set.folderChanged", { path: info.folder }));
              } catch (e) {
                showToast(errMsg(e), "error");
              }
            }}
          />
        )}
        {keysOpen && (
          <KeysModal
            keys={keys}
            busy={libBusy}
            onClose={() => setKeysOpen(false)}
            onImport={importKey}
            onDefault={(id) => libAction(() => api.keyDefault(id))}
            onDelete={(id) => libAction(() => api.keyDelete(id), () => t("keys.deleted"))}
          />
        )}
        {settings && status && (
          <Settings
            version={status.version}
            keyCount={keys.length}
            toast={showToast}
            onKeys={() => {
              setSettings(false);
              setKeysOpen(true);
            }}
            onFolder={() => {
              setSettings(false);
              setFolderOpen(true);
            }}
            onClose={() => setSettings(false)}
            onReset={async () => {
              await api.reset().catch(() => {});
              setSettings(false);
              setVaults([]);
              setPhase("wizard");
            }}
          />
        )}
      </>
    );

  return (
    <div className="win app-win mobile">
      {body}
      {toast && (
        <div className="toasts">
          <div key={toast.key} className={`toast ${toast.kind === "error" ? "err" : "ok"}`}>
            <span className="toast-ic">{toast.kind === "error" ? <IconAlert className="s16" /> : <IconCheck className="s16" />}</span>
            <div className="toast-txt">{toast.msg}</div>
            <button className="btn btn-icon plain" onClick={() => setToast(null)} aria-label={t("common.close")}><IconX className="s14" /></button>
          </div>
        </div>
      )}
    </div>
  );
}
