import { useState } from "react";
import { KeyFile, Vault } from "../api";
import { useT } from "../i18n";
import { Modal } from "./Modal";
import { Note } from "./ui";
import { IconKey, IconPlus, IconStar, IconTrash } from "./Icons";

const size = (n: number) => (n < 1024 ? `${n} B` : `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB`);

/** Ajustes → Ficheros de clave: importar, elegir el predeterminado y quitar. */
export function KeysModal({ keys, busy, onClose, onImport, onDefault, onDelete }: {
  keys: KeyFile[];
  busy: boolean;
  onClose: () => void;
  onImport: () => void;
  onDefault: (id: string | null) => void;
  onDelete: (id: string) => void;
}) {
  const t = useT();
  const [confirm, setConfirm] = useState<string | null>(null);

  return (
    <Modal
      title={t("keys.title")}
      icon={<IconKey className="s16" />}
      onClose={onClose}
      footer={
        <button className="btn btn-primary btn-block" onClick={onImport} disabled={busy}>
          {busy ? <span className="spin" /> : <IconPlus className="s16" />} {t("keys.import")}
        </button>
      }
    >
      <p className="wiz-lede">{t("keys.lede")}</p>
      {keys.length === 0 ? (
        <div className="pick-empty">{t("keys.empty")}</div>
      ) : (
        <div className="keylist">
          {keys.map((k) => (
            <div key={k.id} className="keyrow">
              <div className="keyrow-main">
                <span className="keyrow-ic"><IconKey className="s16" /></span>
                <span className="row-text">
                  <span className="row-name">{k.name}</span>
                  <span className="row-sub">
                    <span className="mono">{k.fingerprint}</span> · {size(k.size)} · {k.uses === 0 ? t("keys.uses0") : t(k.uses === 1 ? "keys.uses1" : "keys.uses", { n: k.uses })}
                  </span>
                </span>
                {k.isDefault && <span className="badge badge-accent">{t("keys.default")}</span>}
              </div>
              {confirm === k.id ? (
                <div className="keyrow-confirm">
                  <span>{t("keys.deleteConfirm", { name: k.name })}</span>
                  <div className="btns">
                    <button className="btn btn-danger-solid btn-sm" onClick={() => { setConfirm(null); onDelete(k.id); }}>{t("keys.delete")}</button>
                    <button className="btn btn-ghost btn-sm" onClick={() => setConfirm(null)}>{t("common.cancel")}</button>
                  </div>
                </div>
              ) : (
                <div className="btns">
                  <button className="btn btn-ghost btn-sm" onClick={() => onDefault(k.isDefault ? null : k.id)}>
                    <IconStar className="s14" /> {k.isDefault ? t("keys.unsetDefault") : t("keys.makeDefault")}
                  </button>
                  <button className="btn btn-ghost btn-sm danger" onClick={() => setConfirm(k.id)}>
                    <IconTrash className="s14" /> {t("keys.delete")}
                  </button>
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </Modal>
  );
}

/** Lo que dice la opción «Automático» para este vault. */
function autoText(t: ReturnType<typeof useT>, v: Vault, auto: Vault["key"] | null): string {
  if (!v.keyfile) return t("vault.autoNone");
  if (!auto || auto.missing || !auto.name) return t("vault.autoMissing", { name: v.keyfile });
  return t(auto.by === "name" ? "vault.autoName" : "vault.autoDefault", { name: auto.name });
}

/** Ficha de un vault: qué fichero de clave usa en este móvil. */
export function VaultSheet({ vault, keys, auto, busy, onClose, onChoose, onImport }: {
  vault: Vault;
  keys: KeyFile[];
  /** Cómo quedaría en automático (para explicarlo aunque ahora esté elegido a mano). */
  auto: Vault["key"] | null;
  busy: boolean;
  onClose: () => void;
  onChoose: (choice: string) => void;
  onImport: () => void;
}) {
  const t = useT();
  const current = vault.key.mode === "manual" ? vault.key.id ?? "auto" : vault.key.mode;
  const options: { value: string; label: string; sub: string }[] = [
    { value: "auto", label: t("vault.auto"), sub: autoText(t, vault, auto) },
    ...keys.map((k) => ({ value: k.id, label: k.name, sub: k.fingerprint })),
    { value: "none", label: t("vault.none"), sub: t("vault.noneSub") },
  ];

  return (
    <Modal title={vault.client} subtitle={vault.file} icon={<IconKey className="s16" />} onClose={onClose}>
      <div className="section">
        <div className="kv"><span>{t("vault.windowsKey")}</span><b>{vault.keyfile ?? t("vault.noWindowsKey")}</b></div>
      </div>
      <div className="section">
        <div className="section-title">{t("vault.phoneKey")}</div>
        <div className="choice-list">
          {options.map((o) => (
            <label key={o.value} className={`choice-card compact stacked ${current === o.value ? "on" : ""}`}>
              <input type="radio" name="vkey" className="sr-only" checked={current === o.value} disabled={busy} onChange={() => onChoose(o.value)} />
              <span className="radio" />
              <span className="txt"><strong>{o.label}</strong><small>{o.sub}</small></span>
            </label>
          ))}
        </div>
        {keys.length === 0 && (
          <>
            <Note>{t("vault.noKeys")}</Note>
            <button className="btn btn-secondary btn-block" onClick={onImport} disabled={busy}>
              {busy ? <span className="spin" /> : <IconPlus className="s16" />} {t("vault.import")}
            </button>
          </>
        )}
      </div>
      <Note>{t("list.soon")}</Note>
    </Modal>
  );
}
