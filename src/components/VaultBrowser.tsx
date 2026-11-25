import { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { api, CLIPBOARD_SECS, EntryDetail, EntryRow, errMsg, GroupNode, Totp, Vault, VaultTree } from "../api";
import { getLang, useT } from "../i18n";
import { Markdown } from "./Markdown";
import { VaultIcon } from "./VaultIcon";
import { Alert, Check, Note } from "./ui";
import { Modal } from "./Modal";
import {
  IconAlert, IconBack, IconClock, IconCopy, IconDownload, IconExternal, IconEye, IconEyeOff, IconFolder, IconHome, IconPaperclip,
  IconSearch, IconShare, IconX,
} from "./Icons";

type Notify = (msg: string, kind?: "ok" | "error") => void;

const norm = (s: string) => s.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
const REVEAL_MS = 10_000; // una contraseña mostrada se vuelve a ocultar sola
const fmtSize = (n: number) => (n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`);

type Item = { kind: "group"; g: GroupNode } | { kind: "entry"; e: EntryRow };

interface Props {
  vault: Vault;
  tree: VaultTree;
  offline: boolean;
  onBack: () => void;
  notify: Notify;
}

/** Navegador de un vault en solo lectura: grupos y entradas (lista) → ficha de la entrada. */
export function VaultBrowser({ vault, tree, offline, onBack, notify }: Props) {
  const t = useT();
  const [path, setPath] = useState<string[]>([tree.root]);
  const [entry, setEntry] = useState<EntryRow | null>(null);
  const [query, setQuery] = useState("");
  const [link, setLink] = useState<string | null>(null);
  const [showEmpty, setShowEmpty] = useState(false);
  const deferred = useDeferredValue(query);

  const byId = useMemo(() => new Map(tree.groups.map((g) => [g.id, g])), [tree]);
  const here = path[path.length - 1];

  // Grupos con alguna entrada en ellos o en sus subgrupos; el resto se consideran vacíos.
  const filled = useMemo(() => {
    const set = new Set<string>();
    for (const e of tree.entries) {
      for (let g = byId.get(e.group); g && !set.has(g.id); g = g.parent ? byId.get(g.parent) : undefined) set.add(g.id);
    }
    return set;
  }, [tree, byId]);

  const groupPath = useCallback(
    (id: string) => {
      const names: string[] = [];
      for (let g = byId.get(id); g && g.id !== tree.root; g = g.parent ? byId.get(g.parent) : undefined) names.unshift(g.name);
      return names.join(" › ");
    },
    [byId, tree.root],
  );

  const sortByName = <T,>(arr: T[], name: (x: T) => string) => arr.sort((a, b) => name(a).localeCompare(name(b), undefined, { sensitivity: "base" }));

  const { items, hiddenEmpty } = useMemo(() => {
    const q = norm(deferred.trim());
    if (q) {
      const groups = tree.groups.filter((g) => g.id !== tree.root && norm(g.name).includes(q));
      const entries = tree.entries.filter((e) => [e.title, e.username, e.url, ...e.tags].some((x) => norm(x).includes(q)));
      return {
        items: [...sortByName(groups, (g) => g.name).map((g) => ({ kind: "group", g }) as Item), ...sortByName(entries, (e) => e.title).map((e) => ({ kind: "entry", e }) as Item)],
        hiddenEmpty: 0,
      };
    }
    const sub = tree.groups.filter((g) => g.parent === here);
    const visible = showEmpty ? sub : sub.filter((g) => filled.has(g.id));
    const entries = tree.entries.filter((e) => e.group === here);
    return {
      items: [...sortByName(visible, (g) => g.name).map((g) => ({ kind: "group", g }) as Item), ...sortByName(entries, (e) => e.title).map((e) => ({ kind: "entry", e }) as Item)],
      hiddenEmpty: sub.length - visible.length,
    };
  }, [deferred, tree, here, filled, showEmpty]);

  const up = () => {
    if (path.length > 1) setPath(path.slice(0, -1));
    else onBack();
  };

  const pick = (it: Item) => {
    if (it.kind === "group") {
      // Desde la búsqueda se salta directamente a la carpeta, con su ruta completa.
      const chain: string[] = [];
      for (let g: GroupNode | undefined = it.g; g; g = g.parent ? byId.get(g.parent) : undefined) chain.unshift(g.id);
      setPath(chain[0] === tree.root ? chain : [tree.root, ...chain]);
      setQuery("");
    } else setEntry(it.e);
  };

  // El botón «atrás» de Android cierra el nivel actual en vez de salir de la app: se añade una entrada al
  // historial al abrir el vault y se repone cada vez que se usa.
  const backRef = useRef<() => void>(() => {});
  backRef.current = () => {
    if (link) setLink(null);
    else if (entry) setEntry(null);
    else up();
  };
  useEffect(() => {
    history.pushState(null, "");
    const onPop = () => {
      history.pushState(null, "");
      backRef.current();
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const confirmLink = link && <LinkConfirm url={link} onCancel={() => setLink(null)} onOpen={() => {
    const url = link;
    setLink(null);
    api.openLink(url).catch((e) => notify(errMsg(e), "error"));
  }} />;

  if (entry) {
    return (
      <>
        <EntryView
          key={entry.id}
          vault={vault}
          row={entry}
          groupName={groupPath(entry.group)}
          custom={tree.icons}
          onBack={() => setEntry(null)}
          onHome={onBack}
          onLink={setLink}
          notify={notify}
        />
        {confirmLink}
      </>
    );
  }

  return (
    <>
      <div className="head">
        <div className="vb-top">
          <button className="btn btn-icon" onClick={up} aria-label={t("vault.back")}><IconBack /></button>
          <div className="vb-title">
            <span className="vb-name">{vault.client}</span>
            <nav className="crumbs" aria-label={t("vault.path")}>
              {path.map((id, i) => (
                <span key={id} className="crumb-wrap">
                  {i > 0 && <span className="sep">›</span>}
                  <button className={`crumb ${i === path.length - 1 ? "on" : ""}`} onClick={() => setPath(path.slice(0, i + 1))} disabled={i === path.length - 1}>
                    {i === 0 ? t("vault.rootName") : byId.get(id)?.name}
                  </button>
                </span>
              ))}
            </nav>
          </div>
          <button className="btn btn-icon" onClick={onBack} aria-label={t("vault.home")}><IconHome /></button>
        </div>
        <div className="search">
          <IconSearch />
          <input className="input-el" type="search" enterKeyHint="search" placeholder={t("vault.search")} value={query} onChange={(e) => setQuery(e.target.value)} />
          {query && <button className="clear" onClick={() => setQuery("")} aria-label={t("search.clear")}><IconX className="s14" /></button>}
        </div>
        {offline && <Note>{t("vault.offline")}</Note>}
      </div>

      {items.length === 0 && !hiddenEmpty ? (
        <div className="empty">
          <div className="empty-art neutral">{query ? <IconSearch /> : <IconFolder />}</div>
          <h2>{query ? t("list.noResults") : t("vault.emptyGroup")}</h2>
          {query && <p>{t("list.noResultsText", { q: query.trim() })}</p>}
        </div>
      ) : (
        <div className="list">
          {items.map((it) => {
            const isGroup = it.kind === "group";
            const title = (isGroup ? it.g.name : it.e.title) || t("vault.untitled");
            const sub = isGroup ? (query && it.g.parent ? groupPath(it.g.parent) : "") : query ? [groupPath(it.e.group), it.e.username].filter(Boolean).join(" · ") : it.e.username;
            return (
              <div key={isGroup ? it.g.id : it.e.id} className="row tall">
                <button className="row-main" onClick={() => pick(it)}>
                  <span className={`vicon ${isGroup ? "group" : ""}`}><VaultIcon icon={isGroup ? it.g.icon : it.e.icon} kind={isGroup ? "group" : "entry"} custom={tree.icons} /></span>
                  <span className="row-text">
                    <span className="row-name">{title}</span>
                    {sub && <span className="row-sub">{sub}</span>}
                  </span>
                  {!isGroup && it.e.hasTotp && <IconClock className="s14 totp-ic" aria-label={t("vault.totp")} role="img" />}
                  {!isGroup && it.e.hasAttachments && <IconPaperclip className="s14 muted" />}
                </button>
              </div>
            );
          })}
          {hiddenEmpty > 0 && (
            <div className="empty-note">
              <span>{t(hiddenEmpty === 1 ? "vault.hiddenEmpty1" : "vault.hiddenEmptyN", { n: hiddenEmpty })}</span>
              <button className="link-btn" onClick={() => setShowEmpty(true)}>{t("vault.showEmpty")}</button>
            </div>
          )}
        </div>
      )}
      {confirmLink}
    </>
  );
}

/** Confirma antes de abrir un enlace del vault, enseñando la dirección real. */
function LinkConfirm({ url, onOpen, onCancel }: { url: string; onOpen: () => void; onCancel: () => void }) {
  const t = useT();
  return (
    <div className="link-confirm" role="alertdialog" aria-label={t("vault.linkAsk")}>
      <div className="alert alert-info">
        <IconExternal className="s16" />
        <div>
          <div>{t("vault.linkAsk")}</div>
          <div className="link-url">{url}</div>
          <div className="btns">
            <button className="btn btn-primary btn-sm" onClick={onOpen}>{t("vault.linkOpen")}</button>
            <button className="btn btn-ghost btn-sm" onClick={onCancel}>{t("common.cancel")}</button>
          </div>
        </div>
      </div>
    </div>
  );
}

// ───────────────────────── Ficha de una entrada ─────────────────────────

const NOTES_KEY = "kp.notesMode";
const readMode = (): "md" | "text" => {
  try {
    return localStorage.getItem(NOTES_KEY) === "text" ? "text" : "md";
  } catch {
    return "md";
  }
};

function EntryView({ vault, row, groupName, custom, onBack, onHome, onLink, notify }: {
  vault: Vault;
  row: EntryRow;
  groupName: string;
  custom: Record<string, string>;
  onBack: () => void;
  onHome: () => void;
  onLink: (url: string) => void;
  notify: Notify;
}) {
  const t = useT();
  const [d, setD] = useState<EntryDetail | null>(null);
  const [error, setError] = useState("");
  const [shown, setShown] = useState<Record<string, string>>({});
  const [totp, setTotp] = useState<Totp | null>(null);
  const [left, setLeft] = useState(0);
  const [mode, setMode] = useState<"md" | "text">(readMode);
  const [sharing, setSharing] = useState(false);
  const timers = useRef<Record<string, number>>({});

  useEffect(() => {
    let alive = true;
    api.entryDetail(vault.id, row.id).then((x) => alive && setD(x)).catch((e) => alive && setError(errMsg(e)));
    const tm = timers.current;
    return () => {
      alive = false;
      // Al salir de la ficha se olvidan los valores mostrados.
      Object.values(tm).forEach((id) => window.clearTimeout(id));
    };
  }, [vault.id, row.id]);

  // Código TOTP: se pide a Rust (la semilla no sale de allí) y se renueva al caducar.
  const loadTotp = useCallback(() => {
    api.entryTotp(vault.id, row.id).then((x) => { setTotp(x); setLeft(x.validFor); }).catch(() => setTotp(null));
  }, [vault.id, row.id]);
  useEffect(() => {
    if (d?.hasTotp) loadTotp();
  }, [d?.hasTotp, loadTotp]);
  useEffect(() => {
    if (!totp) return;
    const id = window.setInterval(() => setLeft((l) => (l <= 1 ? (loadTotp(), l) : l - 1)), 1000);
    return () => window.clearInterval(id);
  }, [totp, loadTotp]);

  const copy = (field: string) =>
    api
      .copySecret(vault.id, row.id, field)
      .then(() => notify(t("vault.copiedClear", { s: CLIPBOARD_SECS })))
      .catch((e) => notify(errMsg(e), "error"));

  const hide = (field: string) => {
    window.clearTimeout(timers.current[field]);
    setShown((s) => {
      const { [field]: _gone, ...rest } = s;
      return rest;
    });
  };

  const toggle = async (field: string) => {
    if (field in shown) return hide(field);
    try {
      const v = await api.revealSecret(vault.id, row.id, field);
      setShown((s) => ({ ...s, [field]: v }));
      timers.current[field] = window.setTimeout(() => hide(field), REVEAL_MS);
    } catch (e) {
      notify(errMsg(e), "error");
    }
  };

  const save = async (name: string) => {
    try {
      const saved = await api.saveAttachment(vault.id, row.id, name);
      if (saved) notify(t("vault.saved", { name: saved }));
    } catch (e) {
      notify(errMsg(e), "error");
    }
  };

  const setNotesMode = (m: "md" | "text") => {
    setMode(m);
    try {
      localStorage.setItem(NOTES_KEY, m);
    } catch {
      /* sin memoria: no pasa nada */
    }
  };

  const copyBtn = (field: string) => (
    <button className="btn btn-icon plain" onClick={() => copy(field)} aria-label={t("vault.copy")}><IconCopy className="s16" /></button>
  );

  const secret = (label: string, field: string) => {
    const v = shown[field];
    return (
      <div className="fld">
        <span className="fld-label">{label}</span>
        <div className="fld-row">
          <span className={`fld-val mono ${v === undefined ? "dots" : ""}`}>{v ?? "••••••••••"}</span>
          <button className="btn btn-icon plain" onClick={() => toggle(field)} aria-label={v === undefined ? t("vault.show") : t("vault.hide")}>
            {v === undefined ? <IconEye className="s16" /> : <IconEyeOff className="s16" />}
          </button>
          {copyBtn(field)}
        </div>
      </div>
    );
  };

  const text = (label: string, value: string, field: string, extra?: React.ReactNode) => (
    <div className="fld">
      <span className="fld-label">{label}</span>
      <div className="fld-row">
        <span className="fld-val">{value}</span>
        {extra}
        {copyBtn(field)}
      </div>
    </div>
  );

  const half = totp ? Math.ceil(totp.code.length / 2) : 0;

  return (
    <>
      <div className="head">
        <div className="vb-top">
          <button className="btn btn-icon" onClick={onBack} aria-label={t("vault.back")}><IconBack /></button>
          <div className="vb-title">
            <span className="vb-name">{row.title || t("vault.untitled")}</span>
            <span className="crumbs"><span className="crumb on">{[vault.client, groupName].filter(Boolean).join(" › ")}</span></span>
          </div>
          {row.icon && <span className="vicon big"><VaultIcon icon={row.icon} kind="entry" custom={custom} /></span>}
          <button className="btn btn-icon" onClick={() => setSharing(true)} disabled={!d} aria-label={t("share.title")}><IconShare /></button>
          <button className="btn btn-icon" onClick={onHome} aria-label={t("vault.home")}><IconHome /></button>
        </div>
      </div>

      <div className="detail">
        {error && <div className="alert alert-err"><IconAlert className="s16" /><div>{error}</div></div>}
        {!d && !error && <div className="detail-loading"><span className="spin lg" /></div>}
        {d && (
          <>
            {d.username && text(t("vault.username"), d.username, "username")}
            {d.hasPassword && secret(t("vault.password"), "password")}
            {d.hasTotp && totp && (
              <div className="fld">
                <span className="fld-label">{t("vault.totp")}</span>
                <div className="fld-row">
                  <span className="fld-val mono totp">{`${totp.code.slice(0, half)} ${totp.code.slice(half)}`}</span>
                  <span className={`totp-left ${left <= 5 ? "low" : ""}`}>{left}s</span>
                  {copyBtn("totp")}
                </div>
              </div>
            )}
            {d.url &&
              text(
                t("vault.url"),
                d.url,
                "url",
                <button className="btn btn-icon plain" onClick={() => onLink(d.url)} aria-label={t("vault.linkOpen")}><IconExternal className="s16" /></button>,
              )}
            {d.fields.map((f) =>
              f.protected ? <div key={f.name}>{secret(f.name, `custom:${f.name}`)}</div> : f.value ? <div key={f.name}>{text(f.name, f.value, `custom:${f.name}`)}</div> : null,
            )}
            {d.tags.length > 0 && (
              <div className="fld">
                <span className="fld-label">{t("vault.tags")}</span>
                <div className="row-tags">{d.tags.map((x) => <span key={x} className="tag-badge">{x}</span>)}</div>
              </div>
            )}
            {d.notes && (
              <div className="fld">
                <div className="fld-head">
                  <span className="fld-label">{t("vault.notes")}</span>
                  <span className="grow" />
                  <div className="seg seg-sm" role="radiogroup" aria-label={t("vault.notes")}>
                    <button type="button" role="radio" aria-checked={mode === "md"} className={`seg-btn ${mode === "md" ? "on" : ""}`} onClick={() => setNotesMode("md")}>Markdown</button>
                    <button type="button" role="radio" aria-checked={mode === "text"} className={`seg-btn ${mode === "text" ? "on" : ""}`} onClick={() => setNotesMode("text")}>{t("vault.plain")}</button>
                  </div>
                  {copyBtn("notes")}
                </div>
                {mode === "md" ? <Markdown text={d.notes} onLink={onLink} /> : <pre className="notes-plain">{d.notes}</pre>}
              </div>
            )}
            {d.attachments.length > 0 && (
              <div className="fld">
                <span className="fld-label">{t("vault.attachments")}</span>
                {d.attachments.map((a) => (
                  <div key={a.name} className="fld-row att">
                    <IconPaperclip className="s16 muted" />
                    <span className="fld-val">{a.name}</span>
                    <span className="att-size">{fmtSize(a.size)}</span>
                    <button className="btn btn-icon plain" onClick={() => save(a.name)} aria-label={t("vault.saveAs", { name: a.name })}><IconDownload className="s16" /></button>
                  </div>
                ))}
              </div>
            )}
            {!d.username && !d.hasPassword && !d.url && !d.notes && d.fields.length === 0 && d.attachments.length === 0 && <div className="help">{t("vault.emptyEntry")}</div>}
          </>
        )}
      </div>
      {sharing && d && (
        <ShareDialog vault={vault} entry={d} onClose={() => setSharing(false)} notify={notify} />
      )}
    </>
  );
}

/**
 * Compartir una entrada con otra app (WhatsApp, Telegram, correo…). El texto lo construye Rust y lo entrega al
 * menú Compartir de Android; aquí solo se elige qué incluir.
 */
function ShareDialog({ vault, entry, onClose, notify }: { vault: Vault; entry: EntryDetail; onClose: () => void; notify: Notify }) {
  const t = useT();
  const [custom, setCustom] = useState(false);
  const [notes, setNotes] = useState(false);
  const [busy, setBusy] = useState(false);
  const hasCustom = entry.fields.length > 0;

  async function share() {
    setBusy(true);
    try {
      await api.shareEntry(vault.id, entry.id, custom, notes, getLang(), t("share.chooser"));
      onClose();
    } catch (e) {
      notify(errMsg(e), "error");
      setBusy(false);
    }
  }

  return (
    <Modal
      title={t("share.title")}
      subtitle={entry.title || t("vault.untitled")}
      icon={<IconShare className="s16" />}
      onClose={onClose}
      footer={
        <button className="btn btn-primary btn-block" onClick={share} disabled={busy}>
          {busy ? <span className="spin" /> : <IconShare className="s16" />} {t("share.go")}
        </button>
      }
    >
      <Alert kind="err"><b>{t("share.warnTitle")}</b> {t("share.warn")}</Alert>
      <div className="section">
        <span className="section-title">{t("share.include")}</span>
        <div className="help">{t("share.always")}</div>
        {hasCustom && <Check checked={custom} onChange={setCustom}>{t("share.custom")}</Check>}
        {entry.notes && <Check checked={notes} onChange={setNotes}>{t("share.notes")}</Check>}
        {notes && <div className="help">{t("share.notesHelp")}</div>}
      </div>
    </Modal>
  );
}
