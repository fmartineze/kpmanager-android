import { FormEvent, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Account, api, errMsg, Item, LoginEvent, Vault } from "../api";
import { rich, useT } from "../i18n";
import { IconBack, IconCheck, IconDatabase, IconFolder, IconKey, IconShield, IconUser } from "./Icons";
import { Alert, Note } from "./ui";

type Step = "welcome" | "microsoft" | "folder" | "data" | "master" | "done";
const ORDER: Step[] = ["welcome", "microsoft", "folder", "data", "master", "done"];

// El registro de Entra ID no es secreto: se recuerda en el navegador para no volver a escribirlo.
const REG_KEY = "kp.reg";
function savedReg(): { tenant: string; clientId: string } {
  try {
    const v = JSON.parse(localStorage.getItem(REG_KEY) ?? "{}");
    return { tenant: String(v.tenant ?? ""), clientId: String(v.clientId ?? "") };
  } catch {
    return { tenant: "", clientId: "" };
  }
}

/** Asistente del primer arranque: Microsoft 365 → carpeta → fichero de datos → contraseña maestra. */
export function Wizard({ onDone }: { onDone: (vaults: Vault[]) => void }) {
  const t = useT();
  const [step, setStep] = useState<Step>("welcome");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  const [reg, setReg] = useState(savedReg);
  const [account, setAccount] = useState<Account | null>(null);
  const [waiting, setWaiting] = useState(false);

  // Navegación de carpetas: pila de carpetas abiertas (la primera es la raíz).
  const [trail, setTrail] = useState<Item[]>([]);
  const [items, setItems] = useState<Item[] | null>(null);
  const folder = trail[trail.length - 1];

  const [candidates, setCandidates] = useState<Item[] | null>(null);
  const [dataId, setDataId] = useState("");
  const [pw, setPw] = useState("");
  const [vaults, setVaults] = useState<Vault[]>([]);

  const idx = ORDER.indexOf(step);
  const go = (s: Step) => {
    setError("");
    setStep(s);
  };

  // Vuelta del navegador.
  useEffect(() => {
    const un = listen<LoginEvent>("login", ({ payload }) => {
      setWaiting(false);
      if (payload.account) {
        setAccount(payload.account);
        setError("");
      } else setError(errMsg(payload.error));
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  async function signIn() {
    setError("");
    try {
      localStorage.setItem(REG_KEY, JSON.stringify(reg));
    } catch {
      /* sin memoria: no pasa nada */
    }
    try {
      setWaiting(true);
      setAccount(null);
      await api.login(reg.tenant, reg.clientId);
    } catch (e) {
      setWaiting(false);
      setError(errMsg(e));
    }
  }

  async function openFolder(f: Item, push = true) {
    setError("");
    setItems(null);
    if (push) setTrail((tr) => [...tr, f]);
    try {
      setItems(await api.children(f.id));
    } catch (e) {
      setItems([]);
      setError(errMsg(e));
    }
  }

  function upFolder() {
    if (trail.length <= 1) return;
    const next = trail.slice(0, -1);
    setTrail(next);
    openFolder(next[next.length - 1], false);
  }

  useEffect(() => {
    if (step === "folder" && trail.length === 0) openFolder({ id: "root", name: t("wiz.root"), folder: true, path: "", etag: "" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [step]);

  async function searchData() {
    go("data");
    setCandidates(null);
    try {
      const found = await api.findData(folder.id);
      setCandidates(found);
      setDataId(found[0]?.id ?? "");
    } catch (e) {
      setCandidates([]);
      setError(errMsg(e));
    }
  }

  async function finish(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const v = await api.finish(folder.id, dataId, pw);
      setPw("");
      setVaults(v);
      go("done");
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  const back = () => go(ORDER[Math.max(0, idx - 1)]);
  const subfolders = items?.filter((i) => i.folder) ?? null;

  return (
    <div className="wiz">
      {step !== "welcome" && step !== "done" && (
        <div className="wiz-top">
          <button className="btn btn-icon plain" onClick={back} aria-label={t("common.back")} disabled={busy}>
            <IconBack className="s16" />
          </button>
          <div className="wiz-progress" aria-label={t("wiz.step", { n: idx, total: ORDER.length - 2 })}>
            {ORDER.slice(1, -1).map((s, i) => <b key={s} className={i < idx ? "on" : ""} />)}
          </div>
        </div>
      )}

      {step === "welcome" && (
        <div className="auth">
          <div className="shield-wrap"><div className="shield"><IconShield /></div></div>
          <h1>{t("wiz.welcomeTitle")}</h1>
          <p className="lede">{rich(t("wiz.welcomeLede"))}</p>
          <div className="form">
            <div className="label">{t("wiz.haveBackup")}</div>
            <button className="btn btn-secondary btn-block" disabled title={t("wiz.restoreSoon")}>{t("wiz.restore")}</button>
            <div className="help">{t("wiz.restoreSoon")}</div>
            <button className="btn btn-primary btn-block" onClick={() => go("microsoft")}>{t("wiz.fresh")}</button>
          </div>
        </div>
      )}

      {step === "microsoft" && (
        <div className="wiz-body">
          <h2 className="wiz-title"><IconUser /> {t("wiz.msTitle")}</h2>
          <p className="wiz-lede">{t("wiz.msLede")}</p>
          <div className="field">
            <label className="label" htmlFor="tenant">{t("wiz.tenant")}</label>
            <div className="input">
              <input id="tenant" className="input-el" autoCapitalize="off" autoCorrect="off" spellCheck={false} placeholder={t("wiz.tenantPh")}
                value={reg.tenant} onChange={(e) => setReg({ ...reg, tenant: e.target.value })} />
            </div>
          </div>
          <div className="field">
            <label className="label" htmlFor="client">{t("wiz.client")}</label>
            <div className="input">
              <input id="client" className="input-el mono" autoCapitalize="off" autoCorrect="off" spellCheck={false} placeholder={t("wiz.clientPh")}
                value={reg.clientId} onChange={(e) => setReg({ ...reg, clientId: e.target.value })} />
            </div>
          </div>
          {account ? (
            <>
              <Alert kind="ok">{rich(t("wiz.signedAs", { name: account.upn || account.name }))}</Alert>
              <button className="btn btn-link suggest" onClick={signIn}>{t("wiz.otherAccount")}</button>
            </>
          ) : (
            <button className="btn btn-secondary btn-block" onClick={signIn} disabled={!reg.tenant.trim() || !reg.clientId.trim()}>
              {waiting && <span className="spin" />} {waiting ? t("wiz.waiting") : t("wiz.signIn")}
            </button>
          )}
          {error && <Alert kind="err">{error}</Alert>}
          <div className="grow" />
          <button className="btn btn-primary btn-block" disabled={!account} onClick={() => { setTrail([]); go("folder"); }}>{t("common.next")}</button>
        </div>
      )}

      {step === "folder" && (
        <div className="wiz-body">
          <h2 className="wiz-title"><IconFolder /> {t("wiz.folderTitle")}</h2>
          <p className="wiz-lede">{t("wiz.folderLede")}</p>
          <div className="crumbs">
            {trail.length > 1 && (
              <button className="btn btn-icon plain" onClick={upFolder} aria-label={t("common.back")}><IconBack className="s14" /></button>
            )}
            <span className="crumb-path">{folder ? folder.path || "/" : ""}</span>
          </div>
          <div className="pick-list">
            {subfolders === null && <div className="pick-empty"><span className="spin" /></div>}
            {subfolders?.length === 0 && <div className="pick-empty">{t("wiz.folderEmpty")}</div>}
            {subfolders?.map((f) => (
              <button key={f.id} className="pick" onClick={() => openFolder(f)}>
                <IconFolder className="s16" /> <span>{f.name}</span>
              </button>
            ))}
          </div>
          {error && <Alert kind="err">{error}</Alert>}
          <button className="btn btn-primary btn-block" disabled={!folder || folder.id === "root"} onClick={searchData}>
            {t("wiz.folderUse")}
          </button>
        </div>
      )}

      {step === "data" && (
        <div className="wiz-body">
          <h2 className="wiz-title"><IconDatabase /> {t("wiz.dataTitle")}</h2>
          <p className="wiz-lede">{rich(t("wiz.dataLede"))}</p>
          {candidates === null && <div className="pick-empty"><span className="spin" /> {t("wiz.searching")}</div>}
          {candidates?.length === 0 && (
            <>
              <Note>{t("wiz.dataNone")}</Note>
              <button className="btn btn-secondary btn-block" onClick={() => go("folder")}>{t("wiz.chooseOther")}</button>
            </>
          )}
          {candidates && candidates.length > 0 && (
            <div className="choice-list">
              {candidates.map((c) => (
                <label key={c.id} className={`choice-card compact ${dataId === c.id ? "on" : ""}`}>
                  <input type="radio" name="data" className="sr-only" checked={dataId === c.id} onChange={() => setDataId(c.id)} />
                  <span className="radio" />
                  <span className="txt"><strong>{c.name}</strong><small>{c.path}</small></span>
                </label>
              ))}
            </div>
          )}
          {error && <Alert kind="err">{error}</Alert>}
          <div className="grow" />
          <button className="btn btn-primary btn-block" disabled={!dataId} onClick={() => go("master")}>{t("common.next")}</button>
        </div>
      )}

      {step === "master" && (
        <form className="wiz-body" onSubmit={finish}>
          <h2 className="wiz-title"><IconKey /> {t("wiz.masterTitle")}</h2>
          <p className="wiz-lede">{t("wiz.masterLede")}</p>
          <div className="field">
            <div className={`input ${error ? "is-error" : ""}`}>
              <IconKey className="s16" />
              <input className="input-el" type="password" autoComplete="off" autoFocus placeholder={t("unlock.master")} value={pw} onChange={(e) => setPw(e.target.value)} />
            </div>
          </div>
          {error && <Alert kind="err">{error}</Alert>}
          <div className="grow" />
          <button className="btn btn-primary btn-block" disabled={busy || !pw}>
            {busy && <span className="spin" />} {busy ? t("wiz.opening") : t("wiz.finish")}
          </button>
        </form>
      )}

      {step === "done" && (
        <div className="auth">
          <div className="shield-wrap"><div className="shield"><IconCheck /></div></div>
          <h1>{t("wiz.doneTitle")}</h1>
          <p className="lede">{t("wiz.doneLede", { n: vaults.length })}</p>
          <div className="form">
            <button className="btn btn-primary btn-block" onClick={() => onDone(vaults)}>{t("wiz.start")}</button>
          </div>
        </div>
      )}
    </div>
  );
}
