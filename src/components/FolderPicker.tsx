import { useEffect, useState } from "react";
import { api, errMsg, Item } from "../api";
import { useT } from "../i18n";
import { Modal } from "./Modal";
import { Alert } from "./ui";
import { IconBack, IconFolder } from "./Icons";

/** Ajustes → Carpeta de origen: navegar por OneDrive y elegir otra carpeta (con la sesión ya iniciada). */
export function FolderPicker({ onClose, onPick }: { onClose: () => void; onPick: (f: Item) => Promise<void> }) {
  const t = useT();
  const [trail, setTrail] = useState<Item[]>([]);
  const [items, setItems] = useState<Item[] | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const folder = trail[trail.length - 1];

  async function open(f: Item, next: Item[]) {
    setError("");
    setItems(null);
    setTrail(next);
    try {
      setItems((await api.children(f.id)).filter((i) => i.folder));
    } catch (e) {
      setItems([]);
      setError(errMsg(e));
    }
  }

  useEffect(() => {
    const root: Item = { id: "root", name: t("wiz.root"), folder: true, path: "", etag: "" };
    open(root, [root]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <Modal
      title={t("set.folder")}
      icon={<IconFolder className="s16" />}
      onClose={onClose}
      fixedHeight
      footer={
        <button
          className="btn btn-primary btn-block"
          disabled={!folder || folder.id === "root" || busy}
          onClick={async () => {
            setBusy(true);
            await onPick(folder);
            setBusy(false);
          }}
        >
          {busy && <span className="spin" />} {t("wiz.folderUse")}
        </button>
      }
    >
      <p className="wiz-lede">{t("set.folderLede")}</p>
      <div className="crumbs">
        {trail.length > 1 && (
          <button className="btn btn-icon plain" onClick={() => open(trail[trail.length - 2], trail.slice(0, -1))} aria-label={t("common.back")}>
            <IconBack className="s14" />
          </button>
        )}
        <span className="crumb-path">{folder ? folder.path || "/" : ""}</span>
      </div>
      <div className="pick-list">
        {items === null && <div className="pick-empty"><span className="spin" /></div>}
        {items?.length === 0 && <div className="pick-empty">{t("wiz.folderEmpty")}</div>}
        {items?.map((f) => (
          <button key={f.id} className="pick" onClick={() => open(f, [...trail, f])}>
            <IconFolder className="s16" /> <span>{f.name}</span>
          </button>
        ))}
      </div>
      {error && <Alert kind="err">{error}</Alert>}
    </Modal>
  );
}
