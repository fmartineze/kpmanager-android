import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, errMsg, Info, isNetwork, LoginEvent, Status, Vault } from "./api";
import { rich, useT } from "./i18n";
import { setThemePref, ThemePref } from "./theme";
import { Wizard } from "./components/Wizard";
import { Modal } from "./components/Modal";
import { Alert, Note, Segmented } from "./components/ui";
import { IconAlert, IconCheck, IconKey, IconLock, IconRefresh, IconSearch, IconSettings, IconShield, IconStar, IconX } from "./components/Icons";

type Phase = "loading" | "wizard" | "locked" | "ready";
type Toast = { msg: string; kind: "ok" | "error"; key: number } | null;

/** Sin actividad durante este tiempo, se bloquea. Al pasar a segundo plano se bloquea en el acto. */
const AUTO_LOCK_MS = 3 * 60 * 1000;

const initial = (s: string) => (s.trim()[0] ?? "?").toUpperCase();
const hue = (s: string) => [...s].reduce((a, c) => (a * 31 + c.charCodeAt(0)) >>> 0, 7) % 6;
const norm = (s: string) => s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();

function Unlock({ onUnlocked }: { onUnlocked: (v: Vault[]) => void }) {
  const t = useT();
  const [pw, setPw] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [shake, setShake] = useState(false);

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
      </form>
    </div>
  );
}

function Settings({ version, onClose, onReset, toast }: { version: string; onClose: () => void; onReset: () => void; toast: (m: string, k?: "ok" | "error") => void }) {
  const t = useT();
  const [info, setInfo] = useState<Info | null>(null);
  const [confirm, setConfirm] = useState(false);
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

  return (
    <Modal title={t("set.title")} icon={<IconSettings className="s16" />} onClose={onClose}>
      <div className="section">
        <div className="kv"><span>{t("set.account")}</span><b>{info?.account.upn ?? "…"}</b></div>
        <div className="kv"><span>{t("set.folder")}</span><b>{info?.folder ?? "…"}</b></div>
        <div className="kv"><span>{t("set.data")}</span><b>{info?.dataFile ?? "…"}</b></div>
        <div className="kv"><span>{t("set.synced")}</span><b>{when}</b></div>
        <div className="btns">
          <button className="btn btn-secondary btn-sm" onClick={() => api.login().catch((e) => toast(errMsg(e), "error"))}>{t("set.relogin")}</button>
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

  const lock = useCallback(async () => {
    await api.lock().catch(() => {});
    setVaults([]);
    setQuery("");
    setTag(null);
    setSettings(false);
    setSelected(null);
    setPhase((p) => (p === "ready" ? "locked" : p));
  }, []);

  // Al pasar a segundo plano se bloquea (y se descarta la lista). Durante el asistente no: hay que ir al navegador.
  useEffect(() => {
    if (phase !== "ready") return;
    const onHide = () => document.visibilityState === "hidden" && lock();
    document.addEventListener("visibilitychange", onHide);
    let timer = window.setTimeout(lock, AUTO_LOCK_MS);
    const reset = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(lock, AUTO_LOCK_MS);
    };
    const events = ["pointerdown", "keydown", "scroll"] as const;
    events.forEach((e) => window.addEventListener(e, reset, { passive: true }));
    return () => {
      document.removeEventListener("visibilitychange", onHide);
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

  const unlocked = (v: Vault[]) => {
    setVaults(v);
    setPhase("ready");
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

  let body;
  if (phase === "loading") body = <div className="empty"><span className="spin lg" /></div>;
  else if (phase === "wizard") body = <Wizard onDone={(v) => { refreshStatus(); unlocked(v); }} />;
  else if (phase === "locked") body = <Unlock onUnlocked={unlocked} />;
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
              <button className="btn btn-icon" aria-label={t("tip.lock")} onClick={lock}><IconLock /></button>
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
                <button className="row-main" onClick={() => setSelected(selected === v.id ? null : v.id)}>
                  <span className={`avatar av-${hue(v.client)}`}>{initial(v.client)}</span>
                  <span className="row-text">
                    <span className="row-name">{v.client}</span>
                    <span className="row-sub">{v.file}{v.keyfile ? ` · ${t("list.keyfile", { name: v.keyfile })}` : ""}</span>
                  </span>
                  {v.tags.length > 0 && (
                    <span className="row-tags">
                      <span className="tag-badge">{v.tags[0]}</span>
                      {v.tags.length > 1 && <span className="tag-badge more">+{v.tags.length - 1}</span>}
                    </span>
                  )}
                  {v.favorite && <span className="row-fav"><IconStar className="s14" /></span>}
                </button>
              </div>
            ))}
            {selected && <Note>{t("list.soon")}</Note>}
          </div>
        )}
        {settings && status && (
          <Settings
            version={status.version}
            toast={showToast}
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
